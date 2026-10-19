//! Real GoTrue round trip (02 7.5). NOT an `sqlx::test`: it uses the migrated `postgres` database of the test project
//! with a unique user and explicit cleanup. Skipped with a message when `SUPABASE_URL` is unset.

mod common;

use api::app::build_router;
use axum::http::StatusCode;
use axum::Router;
use common::*;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn gotrue_roundtrip() {
    let (Ok(auth), Ok(secret), Ok(url)) = (
        std::env::var("SUPABASE_URL"),
        std::env::var("TEST_JWT_HS256_SECRET"),
        std::env::var("TEST_SUPERUSER_DATABASE_URL"),
    ) else {
        eprintln!("gotrue_roundtrip skipped: SUPABASE_URL, TEST_JWT_HS256_SECRET or TEST_SUPERUSER_DATABASE_URL is unset");
        return;
    };
    let issuer = "http://localhost:8000/auth/v1";
    let http = reqwest::Client::new();
    // GoTrue is reached directly (no Caddy, so no /auth/v1 prefix); the service-role JWT is minted from the HS256 secret.
    let service = mint(
        json!({ "role": "service_role", "iss": issuer, "aud": "authenticated", "exp": unix_now() + 300 }),
        &secret,
    );
    let suffix = Uuid::new_v4().simple().to_string();
    let email = format!("roundtrip-{suffix}@example.test");
    let password = format!("pw-{suffix}-Aa1");
    let created: Value = http
        .post(format!("{auth}/admin/users"))
        .bearer_auth(&service)
        .json(&json!({ "email": email, "password": password, "email_confirm": true }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_id = Uuid::parse_str(created["id"].as_str().expect("user id")).unwrap();

    let su = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO app.users (id, display_name) VALUES ($1, $2)")
        .bind(user_id)
        .bind(format!("RT {}", &suffix[..8]))
        .execute(&su)
        .await
        .unwrap();

    let result = async {
        let tok: Value = http
            .post(format!("{auth}/token?grant_type=password"))
            .json(&json!({ "email": email, "password": password }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let access = tok["access_token"].as_str().expect("password grant must be enabled").to_owned();

        let mut b = Build::new(api_pool(&su).await);
        b.hs_secret = Some(secret.clone());
        let router: Router = build_router(b.state_with_issuer(issuer), Router::new());
        let me = send(&router, "GET", "/api/v1/me", Some(&access), None).await;
        assert_eq!(me.status, StatusCode::OK, "{}", me.text());
        assert_eq!(me.json()["id"], user_id.to_string());

        // The real token's session is live; logging out ends it and the SAME token is refused (S2).
        let out = http.post(format!("{auth}/logout")).bearer_auth(&access).send().await.unwrap();
        assert!(out.status().is_success(), "{}", out.status());
        let again = send(&router, "GET", "/api/v1/me", Some(&access), None).await;
        assert_eq!(again.status, StatusCode::UNAUTHORIZED);

        // An anon-role token signed with the same secret is refused (S1).
        let anon = mint(json!({ "iss": issuer, "aud": "authenticated", "role": "anon", "exp": unix_now() + 300 }), &secret);
        let r = send(&router, "GET", "/api/v1/me", Some(&anon), None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    .await;

    // Cleanup runs whatever happened above.
    let _ = sqlx::query("DELETE FROM app.users WHERE id = $1")
        .bind(user_id)
        .execute(&su)
        .await;
    let _ = http
        .delete(format!("{auth}/admin/users/{user_id}"))
        .bearer_auth(&service)
        .send()
        .await;
    result
}
