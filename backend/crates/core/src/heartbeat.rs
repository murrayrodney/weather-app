//! Dead-man's-switch pings (healthchecks.io, 03 sections 3.5 and 4.4). The URL is a secret: it is never logged,
//! and a failed ping logs a `warn` without it and never fails the job.

use std::sync::OnceLock;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};

use crate::redact;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PingKind {
    Start,
    Success,
    Fail,
}

const TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_PAUSE: Duration = Duration::from_millis(500);

fn client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(TIMEOUT)
                .connect_timeout(TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("family-weather/", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()
        })
        .as_ref()
}

/// The URL for `kind` derived from the check URL: `<url>/start`, `<url>`, `<url>/fail`.
pub fn ping_url(base: &str, kind: PingKind) -> String {
    let base = base.trim_end_matches('/');
    match kind {
        PingKind::Start => format!("{base}/start"),
        PingKind::Success => base.to_owned(),
        PingKind::Fail => format!("{base}/fail"),
    }
}

/// `GET` the ping URL; timeout 5 s, one retry. Returns whether a 2xx answer arrived. Never panics, never errors.
pub async fn ping(kind: PingKind, url: &SecretString) -> bool {
    redact::register(url);
    let Some(client) = client() else {
        tracing::warn!("healthcheck ping skipped: client unavailable");
        return false;
    };
    let target = ping_url(url.expose_secret(), kind);
    for attempt in 0..2 {
        match client.get(&target).send().await {
            Ok(r) if r.status().is_success() => return true,
            Ok(r) => {
                tracing::warn!(
                    status = r.status().as_u16(),
                    attempt,
                    ?kind,
                    "healthcheck ping rejected"
                )
            }
            Err(e) => {
                let kind_of_error = redact::http_error(e);
                tracing::warn!(error = %kind_of_error, attempt, ?kind, "healthcheck ping failed");
            }
        }
        if attempt == 0 {
            tokio::time::sleep(RETRY_PAUSE).await;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn urls() {
        assert_eq!(
            ping_url("https://h.example/abc", PingKind::Start),
            "https://h.example/abc/start"
        );
        assert_eq!(
            ping_url("https://h.example/abc/", PingKind::Success),
            "https://h.example/abc"
        );
        assert_eq!(
            ping_url("https://h.example/abc", PingKind::Fail),
            "https://h.example/abc/fail"
        );
    }

    #[tokio::test]
    async fn pings_start_success_fail_and_retries_once() {
        let server = MockServer::start().await;
        for p in ["/uuid/start", "/uuid", "/uuid/fail"] {
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;
        }
        let url = SecretString::from(format!("{}/uuid", server.uri()));
        assert!(ping(PingKind::Start, &url).await);
        assert!(ping(PingKind::Success, &url).await);
        assert!(ping(PingKind::Fail, &url).await);
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn failure_is_reported_not_raised() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let url = SecretString::from(format!("{}/uuid", server.uri()));
        assert!(!ping(PingKind::Success, &url).await);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            2,
            "one retry"
        );
    }
}
