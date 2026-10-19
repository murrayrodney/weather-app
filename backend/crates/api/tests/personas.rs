//! Authorization matrix (02 7.2): every route and persona, hidden stations look like nonexistent ones, revocation and
//! deactivation take effect on the next request, and the guard tests.

mod common;

use api::routes::{specs, RouteKind};
use axum::http::StatusCode;
use common::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

struct Seeded {
    s1: Uuid,
    s2: Uuid,
}

async fn seed(w: &World) -> Seeded {
    let s1 = w
        .add_station("owner", "AA:BB:CC:00:00:01", "Backyard", 40.4, -105.1, None)
        .await;
    let s2 = w
        .add_station("owner", "AA:BB:CC:00:00:02", "Garden", 46.6, -112.0, None)
        .await;
    Seeded { s1, s2 }
}

fn expected(who: &str, kind: RouteKind, owner_only: bool) -> u16 {
    match who {
        "anonymous" | "deleted_session" => 401,
        "inactive" | "no_app_row" => 403,
        "owner" => 200,
        "approved_follower" if !owner_only => 200,
        _ => match kind {
            RouteKind::User => 200,
            _ => 404,
        },
    }
}

fn sample(name: &str, s: &Seeded) -> (&'static str, String, Option<Value>) {
    match name {
        "me" => ("GET", "/api/v1/me".into(), None),
        "stations" => ("GET", "/api/v1/stations".into(), None),
        "station" => ("GET", format!("/api/v1/stations/{}", s.s1), None),
        "ytd" => (
            "GET",
            format!("/api/v1/ytd?station_ids={},{}&year=2026", s.s1, s.s2),
            None,
        ),
        "forecast" => ("GET", format!("/api/v1/stations/{}/forecast", s.s1), None),
        "forecast_changes" => (
            "GET",
            format!(
                "/api/v1/stations/{}/forecast/changes?from=2026-10-10&to=2026-10-12",
                s.s1
            ),
            None,
        ),
        "follows" => ("GET", "/api/v1/follows".into(), None),
        "connect_status" => ("GET", "/api/v1/connect/status".into(), None),
        "users_search" => ("GET", "/api/v1/users/search?q=zz".into(), None),
        "baseline_candidates" => (
            "GET",
            format!("/api/v1/stations/{}/baseline/candidates", s.s1),
            None,
        ),
        other => panic!("no sample for {other}"),
    }
}

#[sqlx::test(migrations = false)]
async fn persona_matrix(db: PgPool) {
    let w = World::new(db).await;
    let s = seed(&w).await;
    let mut covered = 0;
    for spec in specs() {
        if spec.kind == RouteKind::Public || spec.method != "GET" || spec.name == "follows_dummy" {
            continue;
        }
        let owner_only = spec.name == "baseline_candidates";
        let (m, path, body) = sample(spec.name, &s);
        for who in ["anonymous"].into_iter().chain(PERSONAS) {
            let token = (who != "anonymous").then(|| w.tok(who));
            let r = send(&w.router, m, &path, token.as_deref(), body.clone()).await;
            let want = expected(who, spec.kind, owner_only);
            assert_eq!(
                r.status.as_u16(),
                want,
                "{} as {who}: {}",
                spec.name,
                r.text()
            );
            assert_eq!(r.headers["cache-control"], "no-store");
            assert_eq!(r.headers["x-content-type-options"], "nosniff");
        }
        covered += 1;
    }
    assert!(covered >= 9);
}

#[sqlx::test(migrations = false)]
async fn mutating_routes_need_a_live_user(db: PgPool) {
    let w = World::new(db).await;
    let s = seed(&w).await;
    let target = w.p("stranger").id;
    let routes: Vec<(&str, String, Value)> = vec![
        (
            "POST",
            "/api/v1/follows".into(),
            json!({ "followee_id": target }),
        ),
        (
            "POST",
            format!("/api/v1/follows/{target}/approve"),
            json!({}),
        ),
        (
            "POST",
            "/api/v1/connect/validate".into(),
            json!({ "api_key": "fixture-valid-key" }),
        ),
        (
            "POST",
            "/api/v1/connect/save".into(),
            json!({ "api_key": "fixture-valid-key", "devices": [] }),
        ),
        (
            "PUT",
            format!("/api/v1/stations/{}/baseline", s.s1),
            json!({ "reference_station_id": "USW00094035" }),
        ),
    ];
    for (m, path, body) in routes {
        for (who, want) in [
            ("anonymous", 401),
            ("deleted_session", 401),
            ("inactive", 403),
            ("no_app_row", 403),
        ] {
            let token = (who != "anonymous").then(|| w.tok(who));
            let r = send(&w.router, m, &path, token.as_deref(), Some(body.clone())).await;
            assert_eq!(r.status.as_u16(), want, "{m} {path} as {who}");
        }
    }
}

#[sqlx::test(migrations = false)]
async fn hidden_and_missing_stations_have_identical_bodies(db: PgPool) {
    let w = World::new(db).await;
    let s = seed(&w).await;
    let hidden = w
        .get("stranger", &format!("/api/v1/stations/{}", s.s1))
        .await;
    let missing = w
        .get("stranger", &format!("/api/v1/stations/{}", Uuid::new_v4()))
        .await;
    let malformed = w.get("stranger", "/api/v1/stations/not-a-uuid").await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    assert_eq!(hidden.body, missing.body);
    assert_eq!(hidden.body, malformed.body);
}

#[sqlx::test(migrations = false)]
async fn mixed_station_lists_return_404_without_partial_data(db: PgPool) {
    let w = World::new(db).await;
    let s = seed(&w).await;
    let theirs = w
        .add_station(
            "stranger",
            "AA:BB:CC:00:00:09",
            "Theirs",
            40.4,
            -105.1,
            None,
        )
        .await;
    for ids in [
        format!("{},{}", s.s1, theirs),
        format!("{},{}", theirs, s.s1),
        format!("{},{}", s.s1, Uuid::new_v4()),
    ] {
        let r = w
            .get("owner", &format!("/api/v1/ytd?station_ids={ids}&year=2026"))
            .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{ids}");
        assert!(!r.text().contains("stations"));
    }
    // percent-encoded commas work, more than five ids and a missing parameter are 422.
    let ok = w
        .get(
            "owner",
            &format!("/api/v1/ytd?station_ids={}%2C{}&year=2026", s.s1, s.s2),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    let many = (0..6)
        .map(|_| Uuid::new_v4().to_string())
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        w.get(
            "owner",
            &format!("/api/v1/ytd?station_ids={many}&year=2026")
        )
        .await
        .status
        .as_u16(),
        422
    );
    assert_eq!(
        w.get("owner", "/api/v1/ytd?year=2026")
            .await
            .status
            .as_u16(),
        422
    );
}

#[sqlx::test(migrations = false)]
async fn approve_then_revoke_takes_effect_immediately(db: PgPool) {
    let w = World::new(db).await;
    let s = seed(&w).await;
    let path = format!("/api/v1/stations/{}", s.s1);
    assert_eq!(w.get("pending", &path).await.status, StatusCode::NOT_FOUND);
    let pending = w.p("pending").id;
    let r = w
        .post(
            "owner",
            &format!("/api/v1/follows/{pending}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(w.get("pending", &path).await.status, StatusCode::OK);
    let r = w
        .post(
            "owner",
            &format!("/api/v1/follows/{pending}/revoke"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(w.get("pending", &path).await.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = false)]
async fn deactivation_and_session_removal_apply_to_the_same_token(db: PgPool) {
    let w = World::new(db).await;
    assert_eq!(w.get("owner", "/api/v1/me").await.status, StatusCode::OK);
    sqlx::query("UPDATE app.users SET is_active = false, deactivated_at = now() WHERE id = $1")
        .bind(w.p("owner").id)
        .execute(&w.db)
        .await
        .unwrap();
    let r = w.get("owner", "/api/v1/me").await;
    assert_eq!(
        (r.status.as_u16(), r.code().as_str()),
        (403, "account_inactive")
    );

    assert_eq!(w.get("stranger", "/api/v1/me").await.status, StatusCode::OK);
    sqlx::query("DELETE FROM auth.sessions WHERE id = $1")
        .bind(w.p("stranger").session)
        .execute(&w.db)
        .await
        .unwrap();
    assert_eq!(
        w.get("stranger", "/api/v1/me").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = false)]
async fn station_list_is_filtered_and_exposes_nothing_private(db: PgPool) {
    let w = World::new(db).await;
    let s = seed(&w).await;
    w.add_station(
        "stranger",
        "AA:BB:CC:00:00:09",
        "Theirs",
        40.4,
        -105.1,
        None,
    )
    .await;
    let own = w.get("owner", "/api/v1/stations").await.json();
    assert_eq!(own["stations"].as_array().unwrap().len(), 2);
    let fol = w.get("approved_follower", "/api/v1/stations").await.json();
    let list = fol["stations"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list
        .iter()
        .all(|x| x["is_own"] == false && x["owner"]["display_name"] == "Olive"));
    let text = own.to_string();
    for banned in ["mac", "credential_id", "AA:BB:CC", "email", "is_enabled"] {
        assert!(!text.contains(banned), "{banned}");
    }
    let st = own["stations"][0].clone();
    assert_eq!(st["id"], s.s1.to_string());
    assert_eq!(st["coordinates_rounded"], true);
    assert!(st["baseline"].is_null());
    for (k, v) in [("lat", &st["lat"]), ("lon", &st["lon"])] {
        let t = v.to_string();
        assert!(t.split('.').nth(1).map_or(0, str::len) <= 2, "{k}={t}");
    }
    assert!(
        w.get("stranger", "/api/v1/stations").await.json()["stations"]
            .as_array()
            .unwrap()
            .len()
            == 1
    );
}

#[sqlx::test(migrations = false)]
async fn me_reports_profile_and_credential(db: PgPool) {
    let w = World::new(db).await;
    let r = w.get("owner", "/api/v1/me").await.json();
    assert_eq!(r["display_name"], "Olive");
    assert_eq!(r["is_admin"], false);
    assert!(r["credential"].is_null());
    w.add_credential(w.p("owner").id).await;
    assert_eq!(
        w.get("owner", "/api/v1/me").await.json()["credential"]["status"],
        "ok"
    );
}

#[test]
fn guard_tests_scan_the_sources() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    walk(&root, &mut files);
    let field = regex::Regex::new(r"(^|[^A-Za-z0-9_])station_ids?:").unwrap();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // SQL is static text with bind parameters: never assembled with format!/concatenation (02 4.12).
        for line in text.lines() {
            let upper = line.to_uppercase();
            assert!(
                !(line.contains("format!(")
                    && ["SELECT ", "INSERT ", "UPDATE ", "DELETE "]
                        .iter()
                        .any(|k| upper.contains(k))),
                "{rel}: SQL built with format!: {line}"
            );
        }
        // `.route(` only inside the register helper.
        if rel != "routes/mod.rs" {
            assert!(
                !text.contains(".route("),
                "{rel} registers a route outside routes::register"
            );
        }
        if rel.starts_with("routes/") {
            assert!(!text.contains("Path<"), "{rel} reads a raw path id");
            assert!(!field.is_match(&text), "{rel} has a raw station id field");
        }
        // No derive(Debug) on the secret-bearing request and verifier structs.
        for (file, name) in [
            ("routes/connect.rs", "ConnectValidateRequest"),
            ("routes/connect.rs", "ConnectSaveRequest"),
            ("auth/token.rs", "TokenVerifier"),
            ("config.rs", "ApiConfig"),
        ] {
            if rel == file {
                let at = text.find(&format!("pub struct {name}")).unwrap();
                let before = &text[..at];
                let attrs = before.rsplit("\n\n").next().unwrap_or("");
                let derives_debug = attrs
                    .lines()
                    .any(|l| l.contains("derive") && l.contains("Debug"));
                assert!(!derives_debug, "{name} derives Debug");
            }
        }
    }
}
