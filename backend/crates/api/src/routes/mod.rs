//! Route table. Routes are registered ONLY through [`register`] (source-scan guard test, 02 7.2); the same list
//! feeds [`specs`] so tests can iterate every route.

use axum::routing::{get, post, put, MethodRouter};
use axum::Router;

use crate::state::AppState;

pub mod baseline;
pub mod connect;
pub mod follows;
pub mod forecast;
pub mod health;
pub mod me;
pub mod stations;
pub mod users;
pub mod ytd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    Public,
    User,
    Station,
    Stations,
}

#[derive(Debug, Clone, Copy)]
pub struct RouteSpec {
    pub name: &'static str,
    pub method: &'static str,
    /// Full path template, for example `/api/v1/stations/{station_id}`.
    pub path: &'static str,
    pub kind: RouteKind,
}

pub const V1: &str = "/api/v1";

fn spec(
    name: &'static str,
    method: &'static str,
    path: &'static str,
    kind: RouteKind,
) -> RouteSpec {
    RouteSpec {
        name,
        method,
        path,
        kind,
    }
}

fn all() -> Vec<(RouteSpec, MethodRouter<AppState>)> {
    use RouteKind::*;
    vec![
        (
            spec("health", "GET", "/api/health", Public),
            get(health::health),
        ),
        (spec("me", "GET", "/api/v1/me", User), get(me::me)),
        (
            spec("stations", "GET", "/api/v1/stations", User),
            get(stations::list),
        ),
        (
            spec("station", "GET", "/api/v1/stations/{station_id}", Station),
            get(stations::one),
        ),
        (spec("ytd", "GET", "/api/v1/ytd", Stations), get(ytd::ytd)),
        (
            spec(
                "forecast",
                "GET",
                "/api/v1/stations/{station_id}/forecast",
                Station,
            ),
            get(forecast::forecast),
        ),
        (
            spec(
                "forecast_changes",
                "GET",
                "/api/v1/stations/{station_id}/forecast/changes",
                Station,
            ),
            get(forecast::changes),
        ),
        (
            spec("follows", "GET", "/api/v1/follows", User),
            get(follows::list),
        ),
        (
            spec("follow_request", "POST", "/api/v1/follows", User),
            post(follows::request),
        ),
        (
            spec(
                "follow_approve",
                "POST",
                "/api/v1/follows/{follower_id}/approve",
                User,
            ),
            post(follows::approve),
        ),
        (
            spec(
                "follow_decline",
                "POST",
                "/api/v1/follows/{follower_id}/decline",
                User,
            ),
            post(follows::decline),
        ),
        (
            spec(
                "follow_revoke",
                "POST",
                "/api/v1/follows/{follower_id}/revoke",
                User,
            ),
            post(follows::revoke),
        ),
        (
            spec("connect_validate", "POST", "/api/v1/connect/validate", User),
            post(connect::validate),
        ),
        (
            spec("connect_save", "POST", "/api/v1/connect/save", User),
            post(connect::save),
        ),
        (
            spec("connect_status", "GET", "/api/v1/connect/status", User),
            get(connect::status),
        ),
        (
            spec("users_search", "GET", "/api/v1/users/search", User),
            get(users::search),
        ),
        (
            spec(
                "baseline_candidates",
                "GET",
                "/api/v1/stations/{station_id}/baseline/candidates",
                Station,
            ),
            get(baseline::candidates),
        ),
        (
            spec(
                "baseline_put",
                "PUT",
                "/api/v1/stations/{station_id}/baseline",
                Station,
            ),
            put(baseline::put),
        ),
    ]
}

/// Every registered route (for tests).
pub fn specs() -> Vec<RouteSpec> {
    all().into_iter().map(|(s, _)| s).collect()
}

/// The ONLY place a route is added to a router.
fn register(
    router: Router<AppState>,
    path: &str,
    handler: MethodRouter<AppState>,
) -> Router<AppState> {
    router.route(path, handler)
}

/// `/api/health` (no authentication).
pub fn public_router() -> Router<AppState> {
    all()
        .into_iter()
        .filter(|(s, _)| s.kind == RouteKind::Public)
        .fold(Router::new(), |r, (s, h)| register(r, s.path, h))
}

/// Every `/api/v1` route, paths relative to the nest point.
pub fn v1_router() -> Router<AppState> {
    all()
        .into_iter()
        .filter(|(s, _)| s.kind != RouteKind::Public)
        .fold(Router::new(), |r, (s, h)| {
            register(r, s.path.strip_prefix(V1).unwrap_or(s.path), h)
        })
}
