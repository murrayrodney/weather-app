//! 03 task T3a: DB-backed rate-gate and job-lock tests against a migrated test database (03 section 12.3).
//!
//! `#[sqlx::test]` reads the superuser URL from `DATABASE_URL` (set by `make test-backend`); `testdb::prepare` installs the
//! auth stub, `auth_access.sql` and the migrations. The code under test runs through `pool_as(.., "app_poller")` and the
//! detached lock connection gets `SET ROLE app_poller` as well, so the real grants apply.

use std::time::{Duration, Instant};

use famweather_core::ratelimit::{
    job_lock_key, reserve, try_job_lock, try_job_lock_with, GateBusyReason, JobLock, LockLost,
    LockTimeouts,
};
use famweather_core::testdb::{self, pool_as, role_hook};
use sqlx::postgres::PgConnectOptions;
use sqlx::PgPool;

async fn ready(pool: &PgPool) -> PgConnectOptions {
    testdb::prepare(pool).await.expect("prepare test database");
    (*pool.connect_options()).clone()
}

async fn lock(opts: &PgConnectOptions, job: &str) -> Option<JobLock> {
    try_job_lock(opts, job, Some(role_hook("app_poller").unwrap()))
        .await
        .expect("lock connection")
}

/// The server notices a closed socket asynchronously, so "free after drop" is polled briefly.
async fn acquires_soon(opts: &PgConnectOptions, job: &str) -> JobLock {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(l) = lock(opts, job).await {
            return l;
        }
        assert!(Instant::now() < deadline, "lock {job} was not freed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[sqlx::test(migrations = false)]
async fn one_holder_per_job_and_different_jobs_run_together(pool: PgPool) {
    let opts = ready(&pool).await;
    let a = lock(&opts, "awn_ingest").await.expect("first lock");
    assert!(
        lock(&opts, "awn_ingest").await.is_none(),
        "second holder must lose"
    );
    let b = lock(&opts, "daily_rollup")
        .await
        .expect("a different job locks independently");
    let c = lock(&opts, "reference_sync").await.expect("and another");
    a.release().await.unwrap();
    let again = lock(&opts, "awn_ingest")
        .await
        .expect("free after a normal release");
    again.release().await.unwrap();
    b.release().await.unwrap();
    c.release().await.unwrap();
    assert_ne!(job_lock_key("awn_ingest"), job_lock_key("daily_rollup"));
}

#[sqlx::test(migrations = false)]
async fn the_lock_is_free_after_a_panic(pool: PgPool) {
    let opts = ready(&pool).await;
    let o = opts.clone();
    let held = tokio::spawn(async move {
        let _lock = lock(&o, "awn_ingest").await.expect("lock");
        panic!("job panic while holding the lock");
    });
    assert!(held.await.unwrap_err().is_panic());
    acquires_soon(&opts, "awn_ingest")
        .await
        .release()
        .await
        .unwrap();
}

#[sqlx::test(migrations = false)]
async fn the_lock_is_free_after_cancellation(pool: PgPool) {
    let opts = ready(&pool).await;
    let o = opts.clone();
    let (taken_tx, taken_rx) = tokio::sync::oneshot::channel::<()>();
    let held = tokio::spawn(async move {
        let _lock = lock(&o, "daily_rollup").await.expect("lock");
        let _ = taken_tx.send(()); // explicit signal: the lock is held now
        tokio::time::sleep(Duration::from_secs(3600)).await;
    });
    // Wait for the task's own signal (not by probing: a probe that wins the race would take the lock itself and make the
    // task's acquisition fail). Then confirm the lock really is held, and cancel the task.
    tokio::time::timeout(Duration::from_secs(10), taken_rx)
        .await
        .expect("the spawned task never took the lock")
        .expect("the spawned task ended before taking the lock");
    assert!(
        lock(&opts, "daily_rollup").await.is_none(),
        "the lock must be held while the task sleeps"
    );
    held.abort();
    let _ = held.await;
    acquires_soon(&opts, "daily_rollup")
        .await
        .release()
        .await
        .unwrap();
}

#[sqlx::test(migrations = false)]
async fn killing_the_holder_connection_frees_the_lock_and_check_reports_lock_lost(pool: PgPool) {
    let opts = ready(&pool).await;
    let mut held = lock(&opts, "awn_ingest").await.expect("lock");
    held.check().await.expect("healthy lock connection");

    // The only advisory lock in THIS database belongs to the holder connection. pg_locks is cluster-wide and tests run
    // in parallel in their own databases, so the lookup is scoped to the current database.
    let pid: i32 = sqlx::query_scalar(
        "SELECT l.pid FROM pg_locks l JOIN pg_database d ON d.oid = l.database \
         WHERE l.locktype = 'advisory' AND l.granted AND d.datname = current_database() LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let killed: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(killed);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match held.check().await {
            Err(LockLost) => break,
            Ok(()) => {
                assert!(Instant::now() < deadline, "check() never reported LockLost");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    acquires_soon(&opts, "awn_ingest")
        .await
        .release()
        .await
        .unwrap();
}

#[sqlx::test(migrations = false)]
async fn a_healthy_lock_connection_answers_check_inside_the_configured_bound(pool: PgPool) {
    // check() is bounded by LockTimeouts::check (the black-holed case is a unit test in core::ratelimit).
    let opts = ready(&pool).await;
    let t = LockTimeouts {
        connect: Duration::from_secs(10),
        check: Duration::from_millis(500),
    };
    let mut l = try_job_lock_with(
        &opts,
        "awn_ingest",
        Some(role_hook("app_poller").unwrap()),
        t,
    )
    .await
    .unwrap()
    .expect("lock");
    let started = Instant::now();
    l.check().await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    l.release().await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn sixteen_concurrent_reservations_across_two_pools_are_spaced(pool: PgPool) {
    ready(&pool).await;
    let poller = pool_as(&pool, "app_poller").await.unwrap();
    let api = pool_as(&pool, "app_api").await.unwrap();
    // Warm both pools (8 connections each) so all callers reach the database within a few milliseconds of each other;
    // otherwise connection setup spreads the arrivals and the queue runs empty between callers, which is legitimate.
    for p in [&poller, &api] {
        let mut held = Vec::new();
        for _ in 0..8 {
            held.push(p.acquire().await.unwrap());
        }
    }
    let spacing = Duration::from_millis(100);
    // Database time, not the local clock: slot spacing is a database property, and the local clock reading at return
    // jitters by tens of milliseconds on a busy host.
    let db_before: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&pool)
        .await
        .unwrap();

    let start = std::sync::Arc::new(tokio::sync::Barrier::new(16));
    let mut tasks = Vec::new();
    for i in 0..16 {
        let p = if i % 2 == 0 {
            poller.clone()
        } else {
            api.clone()
        };
        let start = start.clone();
        tasks.push(tokio::spawn(async move {
            start.wait().await;
            reserve(&p, "awn:app:poller", spacing, Duration::from_secs(10))
                .await
                .expect("slot granted")
        }));
    }
    let mut waits = Vec::new();
    for t in tasks {
        waits.push(t.await.unwrap());
    }
    waits.sort();
    // Sixteen callers never share a slot, and the queue reaches well into the future.
    waits.dedup();
    assert_eq!(waits.len(), 16, "two callers were granted the same wait");
    assert!(
        *waits.last().unwrap() >= Duration::from_millis(15 * 100 / 2),
        "the last caller waits only {:?}",
        waits.last()
    );
    // Every reservation advanced the gate by exactly one spacing: the stored slot is 16 spacings past the first
    // request (further only if the queue briefly ran empty between two callers).
    let ahead: f64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM next_allowed_at - $1)::float8 FROM app.rate_gates WHERE name = 'awn:app:poller'",
    )
    .bind(db_before)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        (1.6..3.0).contains(&ahead),
        "next_allowed_at is {ahead:.3}s past the first request, expected 1.6 to 3.0"
    );
}

#[sqlx::test(migrations = false)]
async fn exceeding_max_wait_is_gate_busy(pool: PgPool) {
    ready(&pool).await;
    let p = pool_as(&pool, "app_api").await.unwrap();
    let spacing = Duration::from_secs(1);
    let max_wait = Duration::from_millis(1500);
    let first = reserve(&p, "awn:app:api", spacing, max_wait).await.unwrap();
    assert_eq!(first, Duration::ZERO, "the first slot is immediate");
    let second = reserve(&p, "awn:app:api", spacing, max_wait).await.unwrap();
    assert!(
        second > Duration::from_millis(500) && second <= Duration::from_secs(1),
        "{second:?}"
    );
    let busy = reserve(&p, "awn:app:api", spacing, max_wait)
        .await
        .unwrap_err();
    assert_eq!(busy.reason, GateBusyReason::Busy);
    assert!(!busy.is_permanent());
    // Distinct gate names are independent.
    reserve(
        &p,
        "awn:key:00000000-0000-0000-0000-000000000001",
        spacing,
        max_wait,
    )
    .await
    .unwrap();
}

#[sqlx::test(migrations = false)]
async fn a_task_cancelled_while_sleeping_leaves_no_stuck_state(pool: PgPool) {
    ready(&pool).await;
    let p = pool_as(&pool, "app_poller").await.unwrap();
    let spacing = Duration::from_millis(400);
    let max_wait = Duration::from_secs(10);
    assert_eq!(
        reserve(&p, "acis:api", spacing, max_wait).await.unwrap(),
        Duration::ZERO
    );

    let p2 = p.clone();
    let started = Instant::now();
    let sleeper = tokio::spawn(async move {
        let wait = reserve(&p2, "acis:api", spacing, max_wait).await.unwrap();
        tokio::time::sleep(wait).await; // cancelled while sleeping
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    sleeper.abort();
    let _ = sleeper.await;

    // The cancelled reservation still counts: the next slot is the third one, two spacings after the first.
    let third = reserve(&p, "acis:api", spacing, max_wait).await.unwrap();
    let elapsed = started.elapsed();
    let expected = Duration::from_millis(800).saturating_sub(elapsed);
    let diff = third.abs_diff(expected);
    assert!(
        diff < Duration::from_millis(150),
        "third wait {third:?}, expected about {expected:?}"
    );
}

#[sqlx::test(migrations = false)]
async fn a_role_without_the_grant_gets_a_permanent_gate_error(pool: PgPool) {
    ready(&pool).await;
    let ci = pool_as(&pool, "app_ci_check").await.unwrap();
    let err = reserve(
        &ci,
        "awn:app:poller",
        Duration::from_millis(50),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    assert!(matches!(err.reason, GateBusyReason::Database(_)), "{err:?}");
    assert!(
        err.is_permanent(),
        "a missing privilege must be reported as permanent: {err:?}"
    );
}
