//! Supabase Auth (GoTrue) admin API client (04 section 10.1 "Conventions").
//!
//! Errors never carry URLs, request bodies or response bodies: a failed call is reduced to an HTTP status and GoTrue's
//! machine-readable `error_code`. Locally the service key is an HS256 JWT sent as `apikey` and as a bearer token; a
//! production `sb_secret_...` key is sent in the `apikey` header only.

use std::time::Duration;

use famweather_core::redact::{self, HttpErrorKind};
use reqwest::{Method, RequestBuilder};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use url::Url;
use uuid::Uuid;

const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 1000;
const TIMEOUT: Duration = Duration::from_secs(20);
/// About 100 years: GoTrue's "ban" (04 10.5).
pub const BAN_FOREVER: &str = "876000h";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GoTrueError {
    #[error("auth service request failed ({0})")]
    Http(HttpErrorKind),
    #[error("auth service returned status {status}{}", .code.as_deref().map(|c| format!(" ({c})")).unwrap_or_default())]
    Status { status: u16, code: Option<String> },
    #[error("auth service returned an unusable response")]
    BadResponse,
}

impl GoTrueError {
    /// GoTrue's `error_code` (for example `email_exists`), when it sent one.
    pub fn code(&self) -> Option<&str> {
        match self {
            GoTrueError::Status { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    pub fn is_email_exists(&self) -> bool {
        matches!(
            self.code(),
            Some("email_exists" | "user_already_exists" | "identity_already_exists")
        )
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, GoTrueError::Status { status: 404, .. })
    }
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub email: Option<String>,
}

fn parse_user(v: &Value) -> Option<AuthUser> {
    Some(AuthUser {
        id: v.get("id")?.as_str()?.parse().ok()?,
        email: v.get("email").and_then(Value::as_str).map(str::to_owned),
    })
}

pub struct GoTrue {
    http: reqwest::Client,
    base: String,
    key: SecretString,
    bearer: bool,
}

impl GoTrue {
    /// `supabase_url` is `SUPABASE_URL`. Local GoTrue reached directly (host `auth`, the compose test project) serves the
    /// admin routes at the root; behind Caddy or on hosted Supabase they live under `/auth/v1`.
    pub fn new(supabase_url: &Url, key: &SecretString) -> Result<Self, GoTrueError> {
        let root = supabase_url.as_str().trim_end_matches('/').to_owned();
        let base = if supabase_url.host_str() == Some("auth") {
            root
        } else {
            format!("{root}/auth/v1")
        };
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| GoTrueError::Http(redact::http_error(e)))?;
        Ok(Self {
            http,
            base,
            bearer: key.expose_secret().starts_with("eyJ"),
            key: key.clone(),
        })
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let url = format!("{}/{}", self.base, path.trim_start_matches('/'));
        let rb = self
            .http
            .request(method, url)
            .header("apikey", self.key.expose_secret());
        if self.bearer {
            rb.bearer_auth(self.key.expose_secret())
        } else {
            rb
        }
    }

    async fn send(&self, rb: RequestBuilder) -> Result<Value, GoTrueError> {
        let resp = rb
            .send()
            .await
            .map_err(|e| GoTrueError::Http(redact::http_error(e)))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| GoTrueError::Http(redact::http_error(e)))?;
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        if status.is_success() {
            return Ok(body);
        }
        let code = body
            .get("error_code")
            .and_then(Value::as_str)
            .filter(|c| c.len() <= 64 && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .map(str::to_owned);
        Err(GoTrueError::Status {
            status: status.as_u16(),
            code,
        })
    }

    /// Creates a confirmed user. `password: None` creates a passwordless login (invite); `id` is used by restore.
    pub async fn create_user(
        &self,
        email: &str,
        display_name: Option<&str>,
        password: Option<&SecretString>,
        id: Option<Uuid>,
    ) -> Result<Uuid, GoTrueError> {
        let mut body = json!({ "email": email, "email_confirm": true });
        if let Some(name) = display_name {
            body["user_metadata"] = json!({ "display_name": name });
        }
        if let Some(pw) = password {
            body["password"] = json!(pw.expose_secret());
        }
        if let Some(id) = id {
            body["id"] = json!(id);
        }
        let v = self
            .send(self.request(Method::POST, "admin/users").json(&body))
            .await?;
        parse_user(&v).map(|u| u.id).ok_or(GoTrueError::BadResponse)
    }

    pub async fn delete_user(&self, id: Uuid) -> Result<(), GoTrueError> {
        self.send(self.request(Method::DELETE, &format!("admin/users/{id}")))
            .await
            .map(|_| ())
    }

    pub async fn get_user(&self, id: Uuid) -> Result<Option<AuthUser>, GoTrueError> {
        match self
            .send(self.request(Method::GET, &format!("admin/users/{id}")))
            .await
        {
            Ok(v) => parse_user(&v).map(Some).ok_or(GoTrueError::BadResponse),
            Err(e) if e.is_not_found() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every auth user, paging `GET /admin/users`.
    pub async fn list_users(&self) -> Result<Vec<AuthUser>, GoTrueError> {
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let v = self
                .send(self.request(Method::GET, "admin/users").query(&[
                    ("page", page.to_string()),
                    ("per_page", PAGE_SIZE.to_string()),
                ]))
                .await?;
            let users = v
                .get("users")
                .and_then(Value::as_array)
                .ok_or(GoTrueError::BadResponse)?;
            out.extend(users.iter().filter_map(parse_user));
            if users.len() < PAGE_SIZE {
                break;
            }
        }
        Ok(out)
    }

    pub async fn find_user_by_email(&self, email: &str) -> Result<Option<AuthUser>, GoTrueError> {
        Ok(self.list_users().await?.into_iter().find(|u| {
            u.email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(email))
        }))
    }

    /// Mints a one-time recovery link and returns only its `hashed_token`; `action_link` and `email_otp` are dropped.
    pub async fn generate_recovery_token(&self, email: &str) -> Result<SecretString, GoTrueError> {
        let v = self
            .send(
                self.request(Method::POST, "admin/generate_link")
                    .json(&json!({ "type": "recovery", "email": email })),
            )
            .await?;
        let token = v
            .get("hashed_token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_graphic()))
            .ok_or(GoTrueError::BadResponse)?;
        Ok(SecretString::from(token.to_owned()))
    }

    pub async fn ban_user(&self, id: Uuid) -> Result<(), GoTrueError> {
        self.send(
            self.request(Method::PUT, &format!("admin/users/{id}"))
                .json(&json!({ "ban_duration": BAN_FOREVER })),
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> SecretString {
        SecretString::from(s.to_owned())
    }

    #[test]
    fn base_path_depends_on_the_host() {
        let direct = GoTrue::new(&"http://auth:9999".parse().unwrap(), &key("eyJabc")).unwrap();
        assert_eq!(direct.base, "http://auth:9999");
        let proxied =
            GoTrue::new(&"http://localhost:8000".parse().unwrap(), &key("eyJabc")).unwrap();
        assert_eq!(proxied.base, "http://localhost:8000/auth/v1");
        let hosted = GoTrue::new(
            &"https://x.supabase.example/".parse().unwrap(),
            &key("sb_secret_abc"),
        )
        .unwrap();
        assert_eq!(hosted.base, "https://x.supabase.example/auth/v1");
    }

    #[test]
    fn secret_keys_are_never_sent_as_a_bearer_token() {
        let jwt = GoTrue::new(&"http://auth:9999".parse().unwrap(), &key("eyJabc")).unwrap();
        assert!(jwt.bearer);
        let sb = GoTrue::new(&"https://x.example".parse().unwrap(), &key("sb_secret_abc")).unwrap();
        assert!(!sb.bearer);
        let req = sb.request(Method::GET, "admin/users").build().unwrap();
        assert!(req.headers().get("authorization").is_none());
        assert!(req.headers().get("apikey").is_some());
    }

    #[test]
    fn error_messages_carry_no_url_or_body() {
        let e = GoTrueError::Status {
            status: 422,
            code: Some("email_exists".into()),
        };
        assert_eq!(
            e.to_string(),
            "auth service returned status 422 (email_exists)"
        );
        assert!(e.is_email_exists());
        assert!(!GoTrueError::Http(HttpErrorKind::Connect)
            .to_string()
            .contains("http"));
    }

    #[test]
    fn user_json_is_parsed() {
        let v = json!({"id": "7b1c8f64-6a0e-4a0c-9d35-5e4a8f2a0c11", "email": "a@example.test", "created_at": "2026-01-01T00:00:00Z"});
        let u = parse_user(&v).unwrap();
        assert_eq!(u.email.as_deref(), Some("a@example.test"));
        assert!(parse_user(&json!({"id": "nope"})).is_none());
    }
}
