//! Job framework tests (03 section 12.3): locks, `job_runs`, pings, sweep, cancellation, shutdown, disabled mode.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use common::{capture_logs, kill_lock_connection, Builder};
use famweather_core::ratelimit::try_job_lock;
use famweather_core::testdb::role_hook;
use poller_lib::job::{
    run_job, Job, JobContext, JobError, JobFlags, RunOutcome, RunStatus, SkipReason,
};
use poller_lib::jobs::awn_ingest::AwnIngest;
use poller_lib::scheduler;
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::sync::Notify;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone)]
enum Behavior {
    Ok(Value),
    Fail(&'static str, Value),
    Panic,
    Huge,
    /// Signals `started`, then waits for `proceed`.
    Wait(Arc<Notify>, Arc<Notify>),
    /// Loops on `checkpoint` until it fails.
    Spin,
    /// One committed unit that cancels the shutdown token mid-way, then a checkpoint.
    CancelMidUnit,
}

struct Dummy {
    name: &'static str,
    ping: bool,
    behavior: Behavior,
}

fn dummy(name: &'static str, behavior: Behavior) -> Dummy {
    Dummy {
        name,
        ping: false,
        behavior,
    }
}

#[async_trait]
impl Job for Dummy {
    fn name(&self) -> &'static str {
        self.name
    }
    fn ping_suffix(&self) -> Option<&'static str> {
        self.ping.then_some("AWN_INGEST")
    }
    async fn run(&self, ctx: &mut JobContext) -> Result<Value, JobError> {
        match self.behavior.clone() {
            Behavior::Ok(v) => Ok(v),
            Behavior::Fail(msg, partial) => Err(JobError::failed(msg, partial)),
            Behavior::Panic => panic!("job panic carrying TESTKEY-abcdef123456"),
            Behavior::Huge => Ok(json!({"blob": "x".repeat(9000)})),
            Behavior::Wait(started, proceed) => {
                started.notify_one();
                proceed.notified().await;
                Ok(json!({}))
            }
            Behavior::Spin => loop {
                if let Err(i) = ctx.checkpoint().await {
                    return Err(JobError::interrupted(i, json!({"spun": true})));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            },
            Behavior::CancelMidUnit => {
                // The unit: a committed write, with shutdown requested in the middle of it.
                ctx.env.cancel.cancel();
                sqlx::query("INSERT INTO app.rate_gates (name, next_allowed_at) VALUES ('unit-committed', now())")
                    .execute(ctx.pool())
                    .await
                    .unwrap();
                match ctx.checkpoint().await {
                    Err(i) => Err(JobError::interrupted(i, json!({"units": 1}))),
                    Ok(()) => Ok(json!({"units": 2})),
                }
            }
        }
    }
}

async fn statuses(db: &PgPool, job: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT status FROM app.job_runs WHERE job = $1 ORDER BY started_at, id")
        .bind(job)
        .fetch_all(db)
        .await
        .unwrap()
}

async fn ping_server() -> (MockServer, String) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let url = format!("{}/check-uuid", server.uri());
    (server, url)
}

async fn paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect()
}

#[sqlx::test(migrations = false)]
async fn a_held_lock_skips_the_run_without_rows_or_pings(db: PgPool) {
    let (server, url) = ping_server().await;
    let w = Builder::new(db).ping("AWN_INGEST", url).build().await;
    let held = try_job_lock(
        &w.env.db_opts,
        "held_job",
        Some(role_hook("app_poller").unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let job = Dummy {
        name: "held_job",
        ping: true,
        behavior: Behavior::Ok(json!({})),
    };
    let out = run_job(&w.env, &job, None, JobFlags::default())
        .await
        .unwrap();
    assert!(matches!(out, RunOutcome::Skipped(SkipReason::LockHeld)));
    assert_eq!(out.exit_code(), 3);
    assert_eq!(w.count("SELECT count(*) FROM app.job_runs").await, 0);
    assert!(paths(&server).await.is_empty());
    held.release().await.unwrap();
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn success_failed_and_cancelled_rows_and_the_pings_that_go_with_them(db: PgPool) {
    let (server, url) = ping_server().await;
    let w = Builder::new(db).ping("AWN_INGEST", url).build().await;
    let ok = Dummy {
        name: "t_ok",
        ping: true,
        behavior: Behavior::Ok(json!({"rows": 3})),
    };
    let out = run_job(&w.env, &ok, None, JobFlags::default())
        .await
        .unwrap();
    assert_eq!(out.exit_code(), 0);
    assert_eq!(paths(&server).await, ["/check-uuid/start", "/check-uuid"]);

    let failing = Dummy {
        name: "t_fail",
        ping: true,
        behavior: Behavior::Fail("boom", json!({"rows_seen": 7, "gaps": [1]})),
    };
    let out = run_job(&w.env, &failing, None, JobFlags::default())
        .await
        .unwrap();
    assert_eq!(out.exit_code(), 1);
    let detail: Value = sqlx::query_scalar("SELECT detail FROM app.job_runs WHERE job = 't_fail'")
        .fetch_one(&w.db)
        .await
        .unwrap();
    assert_eq!(detail["rows_seen"], 7, "partial counters are kept");
    assert_eq!(detail["gaps"], json!([1]));
    assert_eq!(detail["error"], "boom");
    assert_eq!(paths(&server).await.last().unwrap(), "/check-uuid/fail");

    let before = paths(&server).await.len();
    let cancelled = Dummy {
        name: "t_cancel",
        ping: true,
        behavior: Behavior::CancelMidUnit,
    };
    let out = run_job(&w.env, &cancelled, None, JobFlags::default())
        .await
        .unwrap();
    assert!(matches!(
        out,
        RunOutcome::Finished {
            status: RunStatus::Cancelled,
            ..
        }
    ));
    let new: Vec<String> = paths(&server).await.split_off(before);
    assert_eq!(
        new,
        ["/check-uuid/start"],
        "no /fail and no success ping for a cancelled run"
    );

    assert_eq!(statuses(&w.db, "t_ok").await, ["success"]);
    assert_eq!(statuses(&w.db, "t_fail").await, ["failed"]);
    assert_eq!(statuses(&w.db, "t_cancel").await, ["cancelled"]);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn cancel_mid_unit_lets_the_unit_commit_then_returns_cancelled(db: PgPool) {
    let w = Builder::new(db).build().await;
    let out = run_job(
        &w.env,
        &dummy("t_unit", Behavior::CancelMidUnit),
        None,
        JobFlags::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        out,
        RunOutcome::Finished {
            status: RunStatus::Cancelled,
            ..
        }
    ));
    assert_eq!(
        w.count("SELECT count(*) FROM app.rate_gates WHERE name = 'unit-committed'")
            .await,
        1,
        "the unit committed"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn long_errors_are_cut_and_an_oversized_detail_falls_back(db: PgPool) {
    let w = Builder::new(db).build().await;
    let long = "e".repeat(3000);
    let leaked: &'static str = Box::leak(long.into_boxed_str());
    run_job(
        &w.env,
        &dummy("t_long", Behavior::Fail(leaked, json!({}))),
        None,
        JobFlags::default(),
    )
    .await
    .unwrap();
    let err: String =
        sqlx::query_scalar("SELECT detail->>'error' FROM app.job_runs WHERE job = 't_long'")
            .fetch_one(&w.db)
            .await
            .unwrap();
    assert_eq!(err.len(), 1000);

    let out = run_job(
        &w.env,
        &dummy("t_huge", Behavior::Huge),
        None,
        JobFlags::default(),
    )
    .await
    .unwrap();
    assert_eq!(out.exit_code(), 0, "the status survives");
    let detail: Value = sqlx::query_scalar("SELECT detail FROM app.job_runs WHERE job = 't_huge'")
        .fetch_one(&w.db)
        .await
        .unwrap();
    assert_eq!(detail, json!({"error": "detail_too_large"}));
    assert_eq!(statuses(&w.db, "t_huge").await, ["success"]);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn the_sweep_marks_stale_running_rows_abandoned_regardless_of_age(db: PgPool) {
    let w = Builder::new(db).build().await;
    sqlx::query(
        "INSERT INTO app.job_runs (job, status, started_at) VALUES \
         ('t_sweep','running', now() - interval '1 minute'), ('t_sweep','running', now() - interval '400 days'), \
         ('t_other','running', now() - interval '1 minute')",
    )
    .execute(&w.db)
    .await
    .unwrap();
    run_job(
        &w.env,
        &dummy("t_sweep", Behavior::Ok(json!({}))),
        None,
        JobFlags::default(),
    )
    .await
    .unwrap();
    let mut s = statuses(&w.db, "t_sweep").await;
    s.sort();
    assert_eq!(s, ["abandoned", "abandoned", "success"]);
    assert_eq!(
        statuses(&w.db, "t_other").await,
        ["running"],
        "other jobs are untouched"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_panic_is_a_failed_run_and_never_leaks_its_payload_and_the_lock_is_free(db: PgPool) {
    let w = Builder::new(db).build().await;
    let (_g, logs) = capture_logs();
    famweather_core::redact::install_panic_hook();
    poller_lib::job::run_job(
        &w.env,
        &dummy("t_panic", Behavior::Panic),
        None,
        JobFlags::default(),
    )
    .await
    .unwrap();
    assert_eq!(statuses(&w.db, "t_panic").await, ["failed"]);
    let detail: Value = sqlx::query_scalar("SELECT detail FROM app.job_runs WHERE job = 't_panic'")
        .fetch_one(&w.db)
        .await
        .unwrap();
    assert_eq!(detail["error"], "panic");
    assert!(!detail.to_string().contains("TESTKEY"));
    // The lock is free right away.
    let again = try_job_lock(
        &w.env.db_opts,
        "t_panic",
        Some(role_hook("app_poller").unwrap()),
    )
    .await
    .unwrap();
    assert!(again.is_some());
    again.unwrap().release().await.unwrap();
    assert!(
        !logs.text().contains("TESTKEY"),
        "the panic hook logs file and line only"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn a_lost_lock_connection_aborts_the_job_as_failed(db: PgPool) {
    let w = Builder::new(db).build().await;
    let env = w.env.clone();
    let task = tokio::spawn(async move {
        run_job(
            &env,
            &dummy("t_lost", Behavior::Spin),
            None,
            JobFlags::default(),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    kill_lock_connection(&w.db).await;
    let out = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("the job notices")
        .unwrap()
        .unwrap();
    assert!(matches!(
        out,
        RunOutcome::Finished {
            status: RunStatus::Failed,
            ..
        }
    ));
    let detail: Value = sqlx::query_scalar("SELECT detail FROM app.job_runs WHERE job = 't_lost'")
        .fetch_one(&w.db)
        .await
        .unwrap();
    assert_eq!(detail["error"], "lock_lost");
    assert_eq!(detail["spun"], true, "partial detail kept");
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn two_concurrent_runs_on_separate_pools_exactly_one_runs(db: PgPool) {
    let w = Builder::new(db).build().await;
    // A second "machine": its own pool against the same database.
    let pool2 = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect({
            let hook = role_hook("app_poller").unwrap();
            move |c, _| {
                let h = hook.clone();
                Box::pin(async move { h(c).await })
            }
        })
        .connect_with((*w.db.connect_options()).clone())
        .await
        .unwrap();
    let mut env2 = w.env.clone();
    env2.pool = pool2.clone();
    let (started, proceed) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let env1 = w.env.clone();
    let (s1, p1) = (started.clone(), proceed.clone());
    let first = tokio::spawn(async move {
        run_job(
            &env1,
            &dummy("awn_ingest", Behavior::Wait(s1, p1)),
            None,
            JobFlags::default(),
        )
        .await
    });
    started.notified().await;
    let second = run_job(
        &env2,
        &dummy("awn_ingest", Behavior::Ok(json!({}))),
        None,
        JobFlags::default(),
    )
    .await
    .unwrap();
    assert!(matches!(second, RunOutcome::Skipped(SkipReason::LockHeld)));
    assert_eq!(
        w.count(
            "SELECT count(*) FROM app.job_runs WHERE job = 'awn_ingest' AND status = 'running'"
        )
        .await,
        1
    );
    proceed.notify_one();
    first.await.unwrap().unwrap();
    assert_eq!(
        w.count("SELECT count(*) FROM app.job_runs WHERE job = 'awn_ingest'")
            .await,
        1,
        "the skipped run wrote nothing"
    );
    pool2.close().await;
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn aborting_the_task_after_grace_drops_the_lock_connection_and_frees_the_lock(db: PgPool) {
    let w = Builder::new(db).build().await;
    let (started, proceed) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let env = w.env.clone();
    let (s, p) = (started.clone(), proceed.clone());
    let handle = tokio::spawn(async move {
        run_job(
            &env,
            &dummy("t_abort", Behavior::Wait(s, p)),
            None,
            JobFlags::default(),
        )
        .await
    });
    started.notified().await;
    assert!(try_job_lock(
        &w.env.db_opts,
        "t_abort",
        Some(role_hook("app_poller").unwrap())
    )
    .await
    .unwrap()
    .is_none());
    handle.abort();
    let _ = handle.await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(l) = try_job_lock(
            &w.env.db_opts,
            "t_abort",
            Some(role_hook("app_poller").unwrap()),
        )
        .await
        .unwrap()
        {
            l.release().await.unwrap();
            break;
        }
        assert!(Instant::now() < deadline, "lock never freed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Shutdown then marks the run this process created as cancelled, and only that one.
    sqlx::query("INSERT INTO app.job_runs (job, status) VALUES ('t_foreign', 'running')")
        .execute(&w.db)
        .await
        .unwrap();
    scheduler::mark_own_runs_cancelled(&w.env).await;
    assert_eq!(statuses(&w.db, "t_abort").await, ["cancelled"]);
    assert_eq!(
        statuses(&w.db, "t_foreign").await,
        ["running"],
        "another process's row is untouched"
    );
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn disabled_mode_logs_the_exact_line_once_touches_the_heartbeat_and_no_table(db: PgPool) {
    let w = Builder::new(db).build().await;
    let (_g, logs) = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let hb = dir.path().join("poller.heartbeat");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(scheduler::run_disabled(
        hb.clone(),
        Box::pin(async move {
            let _ = rx.await;
        }),
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        scheduler::heartbeat_fresh(&hb, scheduler::HEARTBEAT_MAX_AGE),
        "heartbeat touched"
    );
    let _ = tx.send(());
    task.await.unwrap();
    assert_eq!(
        logs.text()
            .matches("poller disabled: POLLER_ENABLED=false")
            .count(),
        1
    );
    assert_eq!(w.count("SELECT count(*) FROM app.job_runs").await, 0);
    assert_eq!(w.count("SELECT count(*) FROM app.rate_gates").await, 0);
    w.close().await;
}

#[sqlx::test(migrations = false)]
async fn ingest_sends_fail_only_when_every_station_failed_or_three_runs_in_a_row_failed(
    db: PgPool,
) {
    let w = Builder::new(db).build().await;
    let pool = &w.env.pool;
    let ing = AwnIngest;
    // Partial failure: one of three stations failed -> success ping.
    assert!(
        !ing.ping_fail_on_failure(pool, &json!({"stations": 3, "failed_units": 1}))
            .await
    );
    // Every station failed -> /fail.
    assert!(
        ing.ping_fail_on_failure(pool, &json!({"stations": 3, "failed_units": 3}))
            .await
    );
    assert!(
        ing.ping_fail_on_failure(pool, &json!({"stations": 1, "failed_units": 1}))
            .await
    );
    assert!(
        ing.ping_fail_on_failure(
            pool,
            &json!({"stations": 3, "failed_units": 0, "error": "suspected_application_key_failure"})
        )
        .await
    );
    // Three completed runs in a row with failures -> /fail even for a partial one.
    for _ in 0..3 {
        sqlx::query("INSERT INTO app.job_runs (job, status, finished_at) VALUES ('awn_ingest', 'failed', now())").execute(&w.db).await.unwrap();
    }
    assert!(
        ing.ping_fail_on_failure(pool, &json!({"stations": 3, "failed_units": 1}))
            .await
    );
    sqlx::query("INSERT INTO app.job_runs (job, status, finished_at) VALUES ('awn_ingest', 'success', now() + interval '1 second')").execute(&w.db).await.unwrap();
    assert!(
        !ing.ping_fail_on_failure(pool, &json!({"stations": 3, "failed_units": 1}))
            .await,
        "a success in the last three breaks the streak"
    );
    w.close().await;
}
