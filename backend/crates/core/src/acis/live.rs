//! `LiveAcis`: POST JSON to `data.rcc-acis.org`. Fixed host, keyless, no redirects, bounded bodies, no URLs in errors.

use std::time::Duration;

use async_trait::async_trait;
use chrono::NaiveDate;
use reqwest::redirect::Policy;
use reqwest::StatusCode;
use serde_json::{json, Value};
use url::Url;

use super::parse::{parse_station_data, parse_station_meta};
use super::{AcisApi, AcisDay, AcisError, AcisStationMeta, BBox, MAX_DATA_BYTES, MAX_META_BYTES};
use crate::redact::{self, HttpErrorKind};

const BASE_URL: &str = "https://data.rcc-acis.org/";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct LiveAcis {
    client: reqwest::Client,
    base: Url,
}

impl std::fmt::Debug for LiveAcis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveAcis { .. }")
    }
}

fn map_http(e: reqwest::Error) -> AcisError {
    match redact::http_error(e) {
        HttpErrorKind::Timeout | HttpErrorKind::Connect | HttpErrorKind::Body => {
            AcisError::Unavailable
        }
        _ => AcisError::BadResponse,
    }
}

impl LiveAcis {
    /// Production client: fixed host over HTTPS. `call_timeout` is the whole-request timeout
    /// (poller 60 s for `StnData`, API 5 s for `StnMeta`).
    pub fn new(call_timeout: Duration) -> Result<Self, AcisError> {
        Self::build(
            Url::parse(BASE_URL).map_err(|_| AcisError::BadResponse)?,
            call_timeout,
            true,
        )
    }

    /// Test-only: point the client at a mock server (plain HTTP allowed).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_base(base: Url, call_timeout: Duration) -> Result<Self, AcisError> {
        Self::build(base, call_timeout, false)
    }

    fn build(mut base: Url, call_timeout: Duration, https_only: bool) -> Result<Self, AcisError> {
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        let client = reqwest::Client::builder()
            .timeout(call_timeout)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::none())
            .https_only(https_only)
            .user_agent(concat!("family-weather/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| AcisError::BadResponse)?;
        Ok(Self { client, base })
    }

    async fn post(&self, path: &str, body: &Value, cap: usize) -> Result<Vec<u8>, AcisError> {
        let url = self.base.join(path).map_err(|_| AcisError::BadResponse)?;
        let mut resp = self
            .client
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(map_http)?;
        let status = resp.status();
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            return Err(AcisError::Unavailable);
        }
        if !status.is_success() {
            return Err(AcisError::BadResponse);
        }
        let mut out: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(map_http)? {
            if out.len() + chunk.len() > cap {
                return Err(AcisError::BadResponse);
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

#[async_trait]
impl AcisApi for LiveAcis {
    async fn station_data(
        &self,
        sid: &str,
        sdate: NaiveDate,
        edate: NaiveDate,
    ) -> Result<Vec<AcisDay>, AcisError> {
        let body = json!({
            "sid": sid,
            "sdate": sdate.format("%Y-%m-%d").to_string(),
            "edate": edate.format("%Y-%m-%d").to_string(),
            "elems": [
                {"name": "maxt", "add": "f"},
                {"name": "mint", "add": "f"},
                {"name": "pcpn", "add": "f"},
            ],
        });
        let bytes = self.post("StnData", &body, MAX_DATA_BYTES).await?;
        Ok(parse_station_data(&bytes)?.days)
    }

    async fn nearby_stations(&self, bbox: BBox) -> Result<Vec<AcisStationMeta>, AcisError> {
        let body = json!({
            "bbox": bbox.to_string(),
            "meta": "name,sids,ll,elev,valid_daterange",
            "elems": "pcpn,maxt,mint",
        });
        let bytes = self.post("StnMeta", &body, MAX_META_BYTES).await?;
        parse_station_meta(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> LiveAcis {
        LiveAcis::with_base(
            Url::parse(&server.uri()).unwrap(),
            Duration::from_millis(400),
        )
        .unwrap()
    }
    fn bbox() -> BBox {
        BBox {
            west: -105.4,
            south: 40.1,
            east: -104.7,
            north: 40.7,
        }
    }
    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[tokio::test]
    async fn station_data_request_and_parse() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/StnData"))
            .and(body_partial_json(json!({"sid": "USW00099001", "sdate": "2020-01-01", "edate": "2020-01-02"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "meta": {}, "data": [["2020-01-01", ["30", " "], "10", "T"], ["2020-01-02", "M", "M", "0.5"]]
            })))
            .mount(&server)
            .await;
        let days = client(&server)
            .station_data("USW00099001", d("2020-01-01"), d("2020-01-02"))
            .await
            .unwrap();
        assert_eq!(days.len(), 2);
        assert!(days[0].precip_trace);
    }

    #[tokio::test]
    async fn station_meta_request_and_parse() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/StnMeta"))
            .and(body_partial_json(
                json!({"bbox": "-105.4,40.1,-104.7,40.7", "elems": "pcpn,maxt,mint"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"meta": []})))
            .mount(&server)
            .await;
        assert_eq!(
            client(&server).nearby_stations(bbox()).await.unwrap(),
            vec![]
        );
    }

    #[tokio::test]
    async fn errors_carry_no_url_and_map_as_documented() {
        let server = MockServer::start().await;
        Mock::given(path("/StnMeta"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        Mock::given(path("/StnData"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;
        let c = client(&server);
        assert_eq!(
            c.nearby_stations(bbox()).await.unwrap_err(),
            AcisError::Unavailable
        );
        let e = c
            .station_data("X", d("2020-01-01"), d("2020-01-02"))
            .await
            .unwrap_err();
        assert_eq!(e, AcisError::BadResponse);
        let text = format!("{e} {e:?}");
        assert!(!text.contains("http") && !text.contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn oversize_invalid_and_slow_bodies() {
        let server = MockServer::start().await;
        Mock::given(path("/StnMeta"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAX_META_BYTES + 10]),
            )
            .mount(&server)
            .await;
        Mock::given(path("/StnData"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        let c = client(&server);
        assert_eq!(
            c.nearby_stations(bbox()).await.unwrap_err(),
            AcisError::BadResponse
        );
        assert_eq!(
            c.station_data("X", d("2020-01-01"), d("2020-01-02"))
                .await
                .unwrap_err(),
            AcisError::BadResponse
        );
        let slow = MockServer::start().await;
        Mock::given(path("/StnMeta"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(2))
                    .set_body_json(json!({"meta": []})),
            )
            .mount(&slow)
            .await;
        assert_eq!(
            client(&slow).nearby_stations(bbox()).await.unwrap_err(),
            AcisError::Unavailable
        );
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let server = MockServer::start().await;
        Mock::given(path("/StnMeta"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://127.0.0.1:1/x"),
            )
            .mount(&server)
            .await;
        assert_eq!(
            client(&server).nearby_stations(bbox()).await.unwrap_err(),
            AcisError::BadResponse
        );
    }
}
