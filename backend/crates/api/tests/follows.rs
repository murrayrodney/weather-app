//! Follows and user search (02 5.8, 5.11, 7.6).

mod common;

use std::sync::Arc;

use api::limits::UserLimiters;
use axum::http::StatusCode;
use common::*;
use governor::clock::FakeRelativeClock;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn names(v: &Value) -> Vec<String> {
    v["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["display_name"].as_str().unwrap().to_owned())
        .collect()
}

async fn search(w: &World, who: &str, q: &str) -> Resp {
    w.get(who, &format!("/api/v1/users/search?q={q}")).await
}

/// Invalid searches also spend the search budget, so tests that make many get a relaxed limiter.
async fn relaxed(db: PgPool) -> World {
    World::with(db, |b| {
        let clock = FakeRelativeClock::default();
        b.limiters = Some(Arc::new(UserLimiters::with_search_quota(clock, 6000, 6000)));
    })
    .await
}

#[sqlx::test(migrations = false)]
async fn search_shape_and_validation(db: PgPool) {
    let w = relaxed(db).await;
    let r = search(&w, "stranger", "fe").await;
    assert_eq!(r.status, StatusCode::OK);
    let j = r.json();
    assert_eq!(names(&j), ["Fern"]);
    let u = j["users"][0].as_object().unwrap();
    let mut keys: Vec<_> = u.keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["display_name", "follow_status", "id"]);
    let body = r.text();
    for banned in ["@", "email", "is_admin", "is_active", "mac", "station"] {
        assert!(!body.contains(banned), "{banned}");
    }
    for q in [
        "",
        "a",
        "%20%20a%20",
        &"x".repeat(41),
        "a%01b",
        "fe&bogus=1",
        "fe&q=fe",
    ] {
        let r = w
            .get("stranger", &format!("/api/v1/users/search?q={q}"))
            .await;
        assert_eq!(r.status.as_u16(), 422, "q={q}");
    }
    assert_eq!(
        w.get("stranger", "/api/v1/users/search")
            .await
            .status
            .as_u16(),
        422
    );
    assert_eq!(
        names(&search(&w, "stranger", "%20fe%20").await.json()),
        ["Fern"]
    );
    assert_eq!(names(&search(&w, "stranger", "FER").await.json()), ["Fern"]);
    for q in ["er", "%25%25", "_e", "%5C%5C", "F%25"] {
        assert!(
            names(&search(&w, "stranger", q).await.json()).is_empty(),
            "q={q}"
        );
    }
}

#[sqlx::test(migrations = false)]
async fn search_visibility_follow_status_cap_and_order(db: PgPool) {
    let w = relaxed(db).await;
    assert!(
        names(&search(&w, "owner", "ol").await.json()).is_empty(),
        "never the caller"
    );
    assert!(
        names(&search(&w, "owner", "iv").await.json()).is_empty(),
        "inactive users are hidden"
    );
    assert_eq!(names(&search(&w, "owner", "pe").await.json()), ["Penny"]);
    sqlx::query("UPDATE app.users SET is_active = false, deactivated_at = now() WHERE id = $1")
        .bind(w.p("pending").id)
        .execute(&w.db)
        .await
        .unwrap();
    assert!(names(&search(&w, "owner", "pe").await.json()).is_empty());

    for (who, want) in [
        ("approved_follower", "approved"),
        ("declined", "declined"),
        ("revoked", "revoked"),
        ("stranger", "none"),
    ] {
        let j = search(&w, who, "ol").await.json();
        assert_eq!(j["users"][0]["follow_status"], want, "{who}");
    }
    // POST then search shows pending.
    let r = w
        .post(
            "stranger",
            "/api/v1/follows",
            json!({ "followee_id": w.p("owner").id }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(
        search(&w, "stranger", "ol").await.json()["users"][0]["follow_status"],
        "pending"
    );

    for i in (1..=12).rev() {
        w.add_user(&format!("Zed {i:02}"), true).await;
    }
    let n = names(&search(&w, "owner", "zed").await.json());
    assert_eq!(n.len(), 10);
    let mut sorted = n.clone();
    sorted.sort();
    assert_eq!(n, sorted);
    assert_eq!(n[0], "Zed 01");
}

#[sqlx::test(migrations = false)]
async fn search_has_its_own_rate_limit(db: PgPool) {
    let clock = FakeRelativeClock::default();
    let l = Arc::new(UserLimiters::with_clock(clock.clone()));
    let w = World::with(db, |b| b.limiters = Some(l)).await;
    for i in 0..10 {
        assert_eq!(
            search(&w, "owner", "fe").await.status,
            StatusCode::OK,
            "{i}"
        );
    }
    let r = search(&w, "owner", "fe").await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (429, "rate_limited")
    );
    assert!(r.headers.contains_key("retry-after"));
    // Another user is unaffected, and the owner can still use other routes.
    assert_eq!(search(&w, "stranger", "fe").await.status, StatusCode::OK);
    assert_eq!(w.get("owner", "/api/v1/me").await.status, StatusCode::OK);
    // 30 a minute is one every two seconds.
    clock.advance(std::time::Duration::from_secs(2));
    assert_eq!(search(&w, "owner", "fe").await.status, StatusCode::OK);
    assert_eq!(search(&w, "owner", "fe").await.status.as_u16(), 429);
}

#[sqlx::test(migrations = false)]
async fn follow_requests(db: PgPool) {
    let w = World::new(db).await;
    let target = w.p("owner").id;
    // New request.
    let r = w
        .post(
            "stranger",
            "/api/v1/follows",
            json!({ "followee_id": target }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    assert_eq!(
        r.json(),
        json!({ "user": { "id": target, "display_name": "Olive" }, "status": "pending" })
    );
    assert_eq!(w.audit_count("follow.requested").await, 1);
    let row: (Uuid, Value) = sqlx::query_as(
        "SELECT subject_id, detail FROM app.audit_log WHERE action = 'follow.requested'",
    )
    .fetch_one(&w.db)
    .await
    .unwrap();
    assert_eq!(row.0, w.p("stranger").id);
    assert_eq!(row.1["followee_id"], target.to_string());
    // Repeating it, or asking after approval, changes nothing and writes no audit row.
    let r = w
        .post(
            "stranger",
            "/api/v1/follows",
            json!({ "followee_id": target }),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["status"].clone()),
        (StatusCode::OK, json!("pending"))
    );
    let r = w
        .post(
            "approved_follower",
            "/api/v1/follows",
            json!({ "followee_id": target }),
        )
        .await;
    assert_eq!(
        (r.status, r.json()["status"].clone()),
        (StatusCode::OK, json!("approved"))
    );
    assert_eq!(w.audit_count("follow.requested").await, 1);
    // After a decline or a revoke the request goes back to pending with an audit row.
    for who in ["declined", "revoked"] {
        let before = w.audit_count("follow.requested").await;
        let r = w
            .post(who, "/api/v1/follows", json!({ "followee_id": target }))
            .await;
        assert_eq!(
            (r.status, r.json()["status"].clone()),
            (StatusCode::OK, json!("pending")),
            "{who}"
        );
        assert_eq!(w.audit_count("follow.requested").await, before + 1);
    }
    // The owner sees only id and display name of requesters.
    let j = w.get("owner", "/api/v1/follows").await.json();
    let req = j["followers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["user"]["display_name"] == "Stan")
        .unwrap();
    let mut keys: Vec<_> = req["user"].as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["display_name", "id"]);
    assert_eq!(req["status"], "pending");
}

#[sqlx::test(migrations = false)]
async fn follow_request_rejections(db: PgPool) {
    let w = World::new(db).await;
    let unknown = w
        .post(
            "stranger",
            "/api/v1/follows",
            json!({ "followee_id": Uuid::new_v4() }),
        )
        .await;
    let inactive = w
        .post(
            "stranger",
            "/api/v1/follows",
            json!({ "followee_id": w.p("inactive").id }),
        )
        .await;
    let malformed = w
        .post(
            "stranger",
            "/api/v1/follows",
            json!({ "followee_id": "nope" }),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.body, inactive.body);
    assert_eq!(unknown.body, malformed.body);
    let me = w.p("stranger").id;
    assert_eq!(
        w.post("stranger", "/api/v1/follows", json!({ "followee_id": me }))
            .await
            .status
            .as_u16(),
        422
    );
    for body in [
        json!({ "followee_display_name": "Olive" }),
        json!({ "followee_id": w.p("owner").id, "extra": 1 }),
        json!({}),
    ] {
        assert_eq!(
            w.post("stranger", "/api/v1/follows", body)
                .await
                .status
                .as_u16(),
            422
        );
    }
    assert_eq!(w.audit_count("follow.requested").await, 0);
}

#[sqlx::test(migrations = false)]
async fn an_inactive_follower_cannot_be_approved(db: PgPool) {
    let w = World::new(db).await;
    sqlx::query("UPDATE app.users SET is_active = false, deactivated_at = now() WHERE id = $1")
        .bind(w.p("pending").id)
        .execute(&w.db)
        .await
        .unwrap();
    let path = format!("/api/v1/follows/{}/approve", w.p("pending").id);
    let r = w.post("owner", &path, json!({})).await;
    let hidden = w
        .post(
            "owner",
            &format!("/api/v1/follows/{}/approve", Uuid::new_v4()),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.body, hidden.body, "looks like any hidden request");
    for verb in ["decline", "revoke"] {
        let p = format!("/api/v1/follows/{}/{verb}", w.p("pending").id);
        assert_eq!(
            w.post("owner", &p, json!({})).await.status,
            StatusCode::NOT_FOUND
        );
    }
    let status: String =
        sqlx::query_scalar("SELECT status FROM app.follows WHERE follower_id = $1")
            .bind(w.p("pending").id)
            .fetch_one(&w.db)
            .await
            .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(w.audit_count("follow.approved").await, 0);
}

#[sqlx::test(migrations = false)]
async fn follow_transitions_and_callers(db: PgPool) {
    let w = World::new(db).await;
    let act = |who: &'static str, follower: &'static str, verb: &'static str| {
        let path = format!("/api/v1/follows/{}/{verb}", w.p(follower).id);
        let w = &w;
        async move { w.post(who, &path, json!({})).await }
    };
    // Wrong caller: the follower acting on their own request, and a stranger.
    assert_eq!(
        act("pending", "pending", "approve").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        act("stranger", "pending", "approve").await.status,
        StatusCode::NOT_FOUND
    );
    // Wrong state.
    for (follower, verb) in [
        ("approved_follower", "approve"),
        ("approved_follower", "decline"),
        ("declined", "approve"),
        ("declined", "revoke"),
        ("revoked", "approve"),
        ("pending", "revoke"),
    ] {
        assert_eq!(
            act("owner", follower, verb).await.status,
            StatusCode::NOT_FOUND,
            "{follower} {verb}"
        );
    }
    assert_eq!(
        w.audit_count("follow.approved").await
            + w.audit_count("follow.declined").await
            + w.audit_count("follow.revoked").await,
        0
    );
    // Right state and caller: exactly one audit row each.
    let r = act("owner", "pending", "approve").await;
    assert_eq!(r.status, StatusCode::OK);
    let j = r.json();
    assert_eq!(j["status"], "approved");
    assert_eq!(j["user"]["display_name"], "Penny");
    assert!(j["decided_at"].is_string());
    assert_eq!(w.audit_count("follow.approved").await, 1);
    assert_eq!(
        act("owner", "approved_follower", "revoke").await.json()["status"],
        "revoked"
    );
    assert_eq!(w.audit_count("follow.revoked").await, 1);
    // decline needs a fresh pending row.
    let again = w
        .post(
            "revoked",
            "/api/v1/follows",
            json!({ "followee_id": w.p("owner").id }),
        )
        .await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(
        act("owner", "revoked", "decline").await.json()["status"],
        "declined"
    );
    assert_eq!(w.audit_count("follow.declined").await, 1);
    // Malformed follower id is a 404.
    let r = w
        .post("owner", "/api/v1/follows/not-a-uuid/approve", json!({}))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // The follows list is two-directional and holds only active users.
    let j = w.get("approved_follower", "/api/v1/follows").await.json();
    assert_eq!(j["following"].as_array().unwrap().len(), 1);
    assert_eq!(j["following"][0]["user"]["display_name"], "Olive");
}
