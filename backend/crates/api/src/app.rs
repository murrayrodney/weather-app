//! Router and layer order (02 section 4.2). The LAST layer added runs FIRST on a request.

use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use tower::limit::GlobalConcurrencyLimitLayer;
use tower::load_shed::LoadShedLayer;
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::propagate_header::PropagateHeaderLayer;
use tower_http::request_id::{MakeRequestUuid, SetRequestIdLayer};
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::auth;
use crate::error::{envelope_fixup, not_found_json, overloaded, ApiError};
use crate::proxy::{self, ProxySecret};
use crate::routes;
use crate::state::AppState;

pub const BODY_LIMIT: usize = 16 * 1024;
const REQUEST_ID: &str = "x-request-id";

async fn strip_incoming_request_id(mut req: Request, next: Next) -> Response {
    req.headers_mut().remove(REQUEST_ID);
    next.run(req).await
}

fn on_panic(_: Box<dyn std::any::Any + Send + 'static>) -> Response<Body> {
    ApiError::Internal.into_response()
}

/// `extra` is merged INSIDE `/api/v1` (so the guard applies); production passes `Router::new()`.
pub fn build_router(state: AppState, extra: Router<AppState>) -> Router {
    let request_timeout: Duration = state.cfg.request_timeout;
    let max_in_flight = state.cfg.max_in_flight;
    let proxy_secret = state.cfg.env.netlify_proxy_secret.clone();

    let v1 = routes::v1_router()
        .merge(extra)
        .route_layer(from_fn_with_state(state.clone(), auth::guard))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            request_timeout,
        ));

    let mut app = Router::new()
        .merge(routes::public_router())
        .nest(routes::V1, v1)
        .fallback(not_found_json)
        .with_state(state);

    if let Some(secret) = proxy_secret {
        app = app.layer(from_fn_with_state(
            ProxySecret(secret),
            proxy::verify_signature,
        ));
    }
    app.layer(
        ServiceBuilder::new()
            .layer(axum::error_handling::HandleErrorLayer::new(overloaded))
            .layer(LoadShedLayer::new())
            .layer(GlobalConcurrencyLimitLayer::new(max_in_flight)),
    )
    .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
    .layer(CatchPanicLayer::custom(on_panic))
    .layer(from_fn(envelope_fixup))
    .layer(SetResponseHeaderLayer::overriding(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    ))
    .layer(SetResponseHeaderLayer::overriding(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    ))
    .layer(PropagateHeaderLayer::new(HeaderName::from_static(
        REQUEST_ID,
    )))
    .layer(
        TraceLayer::new_for_http()
            .make_span_with(|req: &Request<Body>| {
                let route = req
                    .extensions()
                    .get::<axum::extract::MatchedPath>()
                    .map_or("unmatched", |m| m.as_str());
                let id = req
                    .headers()
                    .get(REQUEST_ID)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                // Never the URI or any header value: only the matched route template.
                tracing::info_span!(
                    "request",
                    request_id = %id,
                    method = %req.method(),
                    route = %route,
                    user_id = tracing::field::Empty,
                )
            })
            .on_response(
                |resp: &Response<Body>, latency: Duration, _span: &tracing::Span| {
                    tracing::info!(
                        status = resp.status().as_u16(),
                        latency_ms = latency.as_millis() as u64,
                        "request"
                    );
                },
            ),
    )
    .layer(SetSensitiveRequestHeadersLayer::new([
        header::AUTHORIZATION,
        HeaderName::from_static("x-nf-sign"),
        header::COOKIE,
    ]))
    .layer(SetRequestIdLayer::new(
        HeaderName::from_static(REQUEST_ID),
        MakeRequestUuid,
    ))
    .layer(from_fn(strip_incoming_request_id))
}
