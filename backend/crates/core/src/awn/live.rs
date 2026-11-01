//! `LiveAwn`: the real HTTPS client. Base URL is a constant; only tests can override it.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::redirect::Policy;
use reqwest::StatusCode;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use url::Url;

use super::models::{decode_rows, parse_device, ApiKey, AwnDevice, AwnObservation, Mac};
use super::{AwnApi, AwnError, MAX_HISTORY_LIMIT};
use crate::redact::{self, HttpErrorKind};

const BASE_URL: &str = "https://rt.ambientweather.net/v1/";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Response bodies above this abort with `BadResponse`.
pub(crate) const MAX_BODY_BYTES: usize = 256 * 1024;
/// A `Retry-After` above this is clamped (the caller's gates and backoff decide the real wait).
const MAX_RETRY_AFTER: Duration = Duration::from_secs(3600);

pub struct LiveAwn {
    client: reqwest::Client,
    base: Url,
    app_key: SecretString,
}

impl std::fmt::Debug for LiveAwn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveAwn { .. }")
    }
}

fn map_http(e: reqwest::Error) -> AwnError {
    match redact::http_error(e) {
        HttpErrorKind::Timeout | HttpErrorKind::Connect | HttpErrorKind::Body => {
            AwnError::Unavailable
        }
        HttpErrorKind::Decode | HttpErrorKind::Status(_) | HttpErrorKind::Other => {
            AwnError::BadResponse
        }
    }
}

/// Maps an HTTP status (and `Retry-After` seconds) to the error enum; `Ok(())` for 2xx.
pub(crate) fn classify(status: StatusCode, headers: &HeaderMap) -> Result<(), AwnError> {
    if status.is_success() {
        Ok(())
    } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        Err(AwnError::Unauthorized)
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        let retry_after = headers
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|s| Duration::from_secs(s).min(MAX_RETRY_AFTER));
        Err(AwnError::RateLimited { retry_after })
    } else if status.is_server_error() {
        Err(AwnError::Unavailable)
    } else {
        Err(AwnError::BadResponse)
    }
}

impl LiveAwn {
    /// Production client: fixed AWN base URL, HTTPS only. `call_timeout` is the whole-request timeout
    /// (poller 20 s, API 5 s).
    pub fn new(app_key: SecretString, call_timeout: Duration) -> Result<Self, AwnError> {
        Self::build(
            Url::parse(BASE_URL).map_err(|_| AwnError::BadResponse)?,
            app_key,
            call_timeout,
            true,
        )
    }

    /// Test-only: point the client at a mock server (plain HTTP allowed).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_base(
        base: Url,
        app_key: SecretString,
        call_timeout: Duration,
    ) -> Result<Self, AwnError> {
        Self::build(base, app_key, call_timeout, false)
    }

    fn build(
        mut base: Url,
        app_key: SecretString,
        call_timeout: Duration,
        https_only: bool,
    ) -> Result<Self, AwnError> {
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        redact::register(&app_key);
        let client = reqwest::Client::builder()
            .timeout(call_timeout)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::none())
            .https_only(https_only)
            .user_agent(concat!("family-weather/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| AwnError::BadResponse)?;
        Ok(Self {
            client,
            base,
            app_key,
        })
    }

    /// GET `path` (relative to the base) with the two keys in the query string; returns the capped body bytes.
    async fn get(
        &self,
        path: &str,
        key: &ApiKey,
        extra: &[(&str, String)],
    ) -> Result<Vec<u8>, AwnError> {
        let url = self.base.join(path).map_err(|_| AwnError::BadResponse)?;
        let mut query: Vec<(&str, &str)> = vec![
            ("apiKey", key.expose()),
            ("applicationKey", self.app_key.expose_secret()),
        ];
        query.extend(extra.iter().map(|(k, v)| (*k, v.as_str())));
        let mut resp = self
            .client
            .get(url)
            .query(&query)
            .send()
            .await
            .map_err(map_http)?;
        classify(resp.status(), resp.headers())?;
        if resp
            .content_length()
            .is_some_and(|n| n as usize > MAX_BODY_BYTES)
        {
            return Err(AwnError::BadResponse);
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(map_http)? {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(AwnError::BadResponse);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

#[async_trait]
impl AwnApi for LiveAwn {
    async fn list_devices(&self, key: &ApiKey) -> Result<Vec<AwnDevice>, AwnError> {
        let body = self.get("devices", key, &[]).await?;
        let arr: Vec<Value> = serde_json::from_slice(&body).map_err(|_| AwnError::BadResponse)?;
        Ok(arr.iter().filter_map(parse_device).collect())
    }

    async fn history(
        &self,
        key: &ApiKey,
        mac: &Mac,
        end_date: Option<DateTime<Utc>>,
        limit: u16,
    ) -> Result<Vec<AwnObservation>, AwnError> {
        let mut extra = vec![("limit", limit.clamp(1, MAX_HISTORY_LIMIT).to_string())];
        if let Some(end) = end_date {
            extra.push(("endDate", end.timestamp_millis().to_string()));
        }
        let body = self.get(&format!("devices/{mac}"), key, &extra).await?;
        let arr: Vec<Value> = serde_json::from_slice(&body).map_err(|_| AwnError::BadResponse)?;
        Ok(decode_rows(arr).0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KEY: &str = "TESTKEY-abcdef123456";
    const APP: &str = "TESTAPP-0123456789ab";

    fn client(server: &MockServer, timeout: Duration) -> LiveAwn {
        LiveAwn::with_base(
            Url::parse(&server.uri()).unwrap(),
            SecretString::from(APP),
            timeout,
        )
        .unwrap()
    }
    fn key() -> ApiKey {
        ApiKey::new(SecretString::from(KEY))
    }
    async fn stub(server: &MockServer, status: u16) {
        Mock::given(method("GET"))
            .and(path("/devices"))
            .respond_with(ResponseTemplate::new(status))
            .mount(server)
            .await;
    }
    /// Nothing that could identify the request may appear in an error's text.
    fn assert_clean(e: &AwnError) {
        let text = format!("{e} {e:?} {}", redact::scrub(&format!("{e:?}")));
        for needle in [KEY, APP, "apiKey", "applicationKey", "http://", "127.0.0.1"] {
            assert!(!text.contains(needle), "leaked {needle} in {text}");
        }
    }

    #[tokio::test]
    async fn list_devices_sends_both_keys_and_parses() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/devices"))
            .and(query_param("apiKey", KEY))
            .and(query_param("applicationKey", APP))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"macAddress": "AA:BB:CC:00:01:01", "lastData": {"tz": "America/Denver"}, "info": {"name": "One", "coords": {"coords": {"lat": 40.4, "lon": -105.1}, "elevation": 5000.0}}},
                {"macAddress": "garbage"}
            ])))
            .expect(1)
            .mount(&s)
            .await;
        let devices = client(&s, Duration::from_secs(5))
            .list_devices(&key())
            .await
            .unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].mac.as_str(), "AA:BB:CC:00:01:01");
        assert_eq!(devices[0].coords, Some((40.4, -105.1)));
    }

    #[tokio::test]
    async fn history_sends_limit_and_end_date() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/devices/AA:BB:CC:00:01:01"))
            .and(query_param("limit", "288"))
            .and(query_param("endDate", "1700000000000"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!([{"dateutc": 1_699_999_980_000i64, "tempf": 40.0}, {"nodate": 1}]),
            ))
            .expect(1)
            .mount(&s)
            .await;
        let end = DateTime::from_timestamp_millis(1_700_000_000_000).unwrap();
        let mac = Mac::parse("AA:BB:CC:00:01:01").unwrap();
        let rows = client(&s, Duration::from_secs(5))
            .history(&key(), &mac, Some(end), 1000)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].dateutc, 1_699_999_980_000);
    }

    #[tokio::test]
    async fn status_mapping() {
        for (status, want) in [
            (401, AwnError::Unauthorized),
            (403, AwnError::Unauthorized),
            (429, AwnError::RateLimited { retry_after: None }),
            (500, AwnError::Unavailable),
            (503, AwnError::Unavailable),
            (404, AwnError::BadResponse),
            (302, AwnError::BadResponse),
        ] {
            let s = MockServer::start().await;
            stub(&s, status).await;
            let err = client(&s, Duration::from_secs(5))
                .list_devices(&key())
                .await
                .unwrap_err();
            assert_eq!(err, want, "status {status}");
            assert_clean(&err);
        }
    }

    #[tokio::test]
    async fn retry_after_seconds_are_parsed() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "7"))
            .mount(&s)
            .await;
        let err = client(&s, Duration::from_secs(5))
            .list_devices(&key())
            .await
            .unwrap_err();
        assert_eq!(
            err,
            AwnError::RateLimited {
                retry_after: Some(Duration::from_secs(7))
            }
        );
    }

    #[tokio::test]
    async fn undecodable_and_oversize_bodies_are_bad_responses() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json {"))
            .mount(&s)
            .await;
        let err = client(&s, Duration::from_secs(5))
            .list_devices(&key())
            .await
            .unwrap_err();
        assert_eq!(err, AwnError::BadResponse);
        assert_clean(&err);

        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAX_BODY_BYTES + 1]))
            .mount(&s)
            .await;
        let err = client(&s, Duration::from_secs(5))
            .list_devices(&key())
            .await
            .unwrap_err();
        assert_eq!(err, AwnError::BadResponse);

        // A JSON object instead of an array is also unusable.
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"error": "x"})))
            .mount(&s)
            .await;
        assert_eq!(
            client(&s, Duration::from_secs(5))
                .list_devices(&key())
                .await
                .unwrap_err(),
            AwnError::BadResponse
        );
    }

    #[tokio::test]
    async fn timeout_and_connection_refused_are_unavailable_without_leaks() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
            .mount(&s)
            .await;
        let err = client(&s, Duration::from_millis(150))
            .list_devices(&key())
            .await
            .unwrap_err();
        assert_eq!(err, AwnError::Unavailable);
        assert_clean(&err);

        let dead = LiveAwn::with_base(
            Url::parse("http://127.0.0.1:1").unwrap(),
            SecretString::from(APP),
            Duration::from_secs(2),
        )
        .unwrap();
        let err = dead.list_devices(&key()).await.unwrap_err();
        assert_eq!(err, AwnError::Unavailable);
        assert_clean(&err);
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "http://127.0.0.1:1/x"),
            )
            .mount(&s)
            .await;
        assert_eq!(
            client(&s, Duration::from_secs(5))
                .list_devices(&key())
                .await
                .unwrap_err(),
            AwnError::BadResponse
        );
    }

    #[test]
    fn production_constructor_is_https_and_debug_is_clean() {
        let c = LiveAwn::new(SecretString::from(APP), Duration::from_secs(5)).unwrap();
        assert_eq!(c.base.as_str(), BASE_URL);
        assert!(!format!("{c:?}").contains(APP));
    }
}
