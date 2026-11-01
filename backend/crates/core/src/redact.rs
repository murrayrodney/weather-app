//! Log redaction (S7): scrub set, `ScrubWriter`, tracing init, panic hook, URL-stripping HTTP error helper.

use std::borrow::Cow;
use std::io::{self, Write};
use std::panic::Location;
use std::sync::{OnceLock, RwLock};

use regex::Regex;
use secrecy::{ExposeSecret, SecretString};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::EnvFilter;

/// Replacement for registered literals and `key=value` secrets.
pub const REDACTED: &str = "[REDACTED]";
/// Default filter when `RUST_LOG` is unset (03 section 8).
pub const DEFAULT_FILTER: &str = "info,hyper=warn,reqwest=warn,sqlx=warn,h2=warn";
/// Shortest secret that is registered as a literal.
const MIN_LITERAL_LEN: usize = 8;

struct Entry {
    value: String,
    /// Registered for the life of the process (application keys, config secrets).
    permanent: bool,
    /// Live `ScrubGuard`s holding this literal (short-lived keys).
    scoped: usize,
}

fn literals() -> &'static RwLock<Vec<Entry>> {
    static SET: OnceLock<RwLock<Vec<Entry>>> = OnceLock::new();
    SET.get_or_init(|| RwLock::new(Vec::new()))
}

fn kv_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(apikey|applicationkey|api_key|token|key)=[^&\s"']+"#)
            .expect("static regex")
    })
}

fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"https?://[^\s"']+"#).expect("static regex"))
}

fn add(v: &str, permanent: bool) {
    let mut set = literals().write().unwrap_or_else(|p| p.into_inner());
    match set.iter_mut().find(|e| e.value == v) {
        Some(e) if permanent => e.permanent = true,
        Some(e) => e.scoped += 1,
        None => {
            set.push(Entry {
                value: v.to_owned(),
                permanent,
                scoped: usize::from(!permanent),
            });
            // Longest first so a secret containing another registered secret is replaced whole.
            set.sort_by_key(|e| std::cmp::Reverse(e.value.len()));
        }
    }
}

/// Adds a secret literal to the scrub set for the rest of the process. For long-lived secrets only
/// (application key, configuration secrets). Values shorter than 8 bytes are ignored (they would over-match).
pub fn register(secret: &SecretString) {
    let v = secret.expose_secret();
    if v.len() >= MIN_LITERAL_LEN {
        add(v, true);
    }
}

/// Keeps a literal in the scrub set while alive; dropping the guard removes it again unless it was also
/// registered permanently or held by another guard.
#[must_use = "the literal is scrubbed only while the guard is alive"]
pub struct ScrubGuard(Option<String>);

impl Drop for ScrubGuard {
    fn drop(&mut self) {
        let Some(v) = self.0.take() else { return };
        let mut set = literals().write().unwrap_or_else(|p| p.into_inner());
        if let Some(i) = set.iter().position(|e| e.value == v) {
            set[i].scoped = set[i].scoped.saturating_sub(1);
            if !set[i].permanent && set[i].scoped == 0 {
                set.remove(i);
            }
        }
    }
}

/// Registers a short-lived secret (a user's AWN key) for scrubbing until the returned guard is dropped,
/// so the global set stays bounded.
pub fn register_scoped(secret: &SecretString) -> ScrubGuard {
    let v = secret.expose_secret();
    if v.len() < MIN_LITERAL_LEN {
        return ScrubGuard(None);
    }
    add(v, false);
    ScrubGuard(Some(v.to_owned()))
}

/// Removes registered literals, `key=value` secrets and URLs from `s`.
pub fn scrub(s: &str) -> Cow<'_, str> {
    let mut out: Cow<'_, str> = Cow::Borrowed(s);
    {
        let set = literals().read().unwrap_or_else(|p| p.into_inner());
        for e in set.iter() {
            if out.contains(e.value.as_str()) {
                out = Cow::Owned(out.replace(e.value.as_str(), REDACTED));
            }
        }
    }
    if let Cow::Owned(v) = url_re().replace_all(&out, "[URL]") {
        out = Cow::Owned(v);
    }
    if let Cow::Owned(v) = kv_re().replace_all(&out, "$1=[REDACTED]") {
        out = Cow::Owned(v);
    }
    out
}

/// Wraps a writer and scrubs every complete line before it reaches the inner writer.
pub struct ScrubWriter<W: Write> {
    inner: W,
    buf: Vec<u8>,
}

impl<W: Write> ScrubWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buf: Vec::new(),
        }
    }

    fn emit(&mut self, bytes: &[u8]) -> io::Result<()> {
        let text = String::from_utf8_lossy(bytes);
        self.inner.write_all(scrub(&text).as_bytes())
    }
}

impl<W: Write> Write for ScrubWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        if let Some(pos) = self.buf.iter().rposition(|b| *b == b'\n') {
            let rest = self.buf.split_off(pos + 1);
            let complete = std::mem::replace(&mut self.buf, rest);
            self.emit(&complete)?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Drop for ScrubWriter<W> {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let rest = std::mem::take(&mut self.buf);
            let _ = self.emit(&rest);
        }
        let _ = self.inner.flush();
    }
}

/// `MakeWriter` producing a [`ScrubWriter`] over another `MakeWriter`.
#[derive(Clone)]
pub struct ScrubMakeWriter<M>(pub M);

impl<'a, M> MakeWriter<'a> for ScrubMakeWriter<M>
where
    M: MakeWriter<'a>,
{
    type Writer = ScrubWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        ScrubWriter::new(self.0.make_writer())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RedactError {
    #[error("tracing is already initialised")]
    AlreadyInitialised,
    #[error("invalid RUST_LOG filter")]
    InvalidFilter,
}

/// Initialises the global tracing subscriber (stdout through [`ScrubWriter`]) and installs the panic hook.
/// `json` selects JSON output (production and Compose); text is for local terminals and tests.
pub fn init_tracing(json: bool) -> Result<(), RedactError> {
    let filter = match std::env::var("RUST_LOG") {
        Ok(v) if !v.trim().is_empty() => {
            EnvFilter::try_new(v).map_err(|_| RedactError::InvalidFilter)?
        }
        _ => EnvFilter::new(DEFAULT_FILTER),
    };
    let writer = ScrubMakeWriter(io::stdout);
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer);
    let res = if json {
        builder.json().try_init()
    } else {
        builder.try_init()
    };
    res.map_err(|_| RedactError::AlreadyInitialised)?;
    install_panic_hook();
    Ok(())
}

/// Logs one `error` event carrying only `file` and `line` and the fixed message `panic`.
/// The panic payload (which may contain secrets or upstream bodies) is never read.
pub fn log_panic(location: Option<&Location<'_>>) {
    match location {
        Some(l) => tracing::error!(file = l.file(), line = l.line(), "panic"),
        None => tracing::error!("panic"),
    }
}

/// Replaces the default panic hook: file:line and the fixed message "panic" only, NEVER the payload.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| log_panic(info.location())));
}

/// What a failed HTTP call is allowed to say about itself. No URL, no source chain, no body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpErrorKind {
    #[error("timeout")]
    Timeout,
    #[error("connect")]
    Connect,
    #[error("body")]
    Body,
    #[error("decode")]
    Decode,
    #[error("status {0}")]
    Status(u16),
    #[error("other")]
    Other,
}

/// Classifies a `reqwest::Error`, dropping the URL (`without_url`) and the source chain.
pub fn http_error(e: reqwest::Error) -> HttpErrorKind {
    let e = e.without_url();
    if e.is_timeout() {
        HttpErrorKind::Timeout
    } else if e.is_connect() {
        HttpErrorKind::Connect
    } else if let Some(s) = e.status() {
        HttpErrorKind::Status(s.as_u16())
    } else if e.is_body() {
        HttpErrorKind::Body
    } else if e.is_decode() {
        HttpErrorKind::Decode
    } else {
        HttpErrorKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Capture {
            self.clone()
        }
    }
    impl Capture {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    #[test]
    fn scrubs_registered_literals_key_values_and_urls() {
        register(&SecretString::from("TESTKEY-abcdef123456"));
        let s = scrub("x TESTKEY-abcdef123456 y apiKey=abc123&applicationKey=zzz9 https://rt.example.net/v1/devices?apiKey=q z");
        assert!(!s.contains("TESTKEY-abcdef123456"));
        assert!(!s.contains("abc123"));
        assert!(!s.contains("zzz9"));
        assert!(!s.contains("example.net"));
        assert!(s.contains("[URL]"));
        // Text without secrets is returned borrowed.
        assert!(matches!(scrub("nothing to see"), Cow::Borrowed(_)));
    }

    #[test]
    fn scoped_registration_is_removed_when_the_guard_drops() {
        let lit = SecretString::from("scoped-literal-AAAA1111");
        let g1 = register_scoped(&lit);
        let g2 = register_scoped(&lit);
        assert_eq!(scrub("x scoped-literal-AAAA1111 y"), "x [REDACTED] y");
        drop(g1);
        assert_eq!(
            scrub("scoped-literal-AAAA1111"),
            "[REDACTED]",
            "still held by the second guard"
        );
        drop(g2);
        assert_eq!(
            scrub("scoped-literal-AAAA1111"),
            "scoped-literal-AAAA1111",
            "set does not grow forever"
        );

        // A permanent registration survives guards.
        let perm = SecretString::from("permanent-literal-BBBB2222");
        register(&perm);
        drop(register_scoped(&perm));
        assert_eq!(scrub("permanent-literal-BBBB2222"), "[REDACTED]");
    }

    #[test]
    fn short_secrets_are_not_registered() {
        register(&SecretString::from("abc"));
        assert_eq!(scrub("abc abc"), "abc abc");
    }

    #[test]
    fn key_value_regex_is_case_insensitive() {
        assert_eq!(scrub("APIKEY=hunter2xx rest"), "APIKEY=[REDACTED] rest");
        assert_eq!(scrub("token=abc&x=1"), "token=[REDACTED]&x=1");
    }

    #[test]
    fn scrub_writer_scrubs_lines_even_when_split_across_writes() {
        register(&SecretString::from("split-secret-value-42"));
        let cap = Capture::default();
        {
            let mut w = ScrubWriter::new(cap.clone());
            w.write_all(b"before split-secret-").unwrap();
            w.write_all(b"value-42 after\nnext line https://h.example/p?a=b\n")
                .unwrap();
            w.write_all(b"unterminated key=zzzz").unwrap();
        }
        let out = cap.text();
        assert!(!out.contains("split-secret-value-42"));
        assert!(!out.contains("h.example"));
        assert!(!out.contains("zzzz"));
        assert!(out.contains("before [REDACTED] after"));
    }

    #[test]
    fn tracing_output_through_scrub_make_writer_is_scrubbed() {
        register(&SecretString::from("TESTKEY-abcdef123456"));
        let cap = Capture::default();
        let sub = tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(ScrubMakeWriter(cap.clone()))
            .finish();
        tracing::subscriber::with_default(sub, || {
            tracing::warn!(error = "GET https://rt.ambientweather.net/v1/devices?apiKey=TESTKEY-abcdef123456 failed", "request");
        });
        let out = cap.text();
        assert!(out.contains("request"));
        assert!(!out.contains("TESTKEY-abcdef123456"));
        assert!(!out.contains("ambientweather"));
    }

    #[test]
    fn panic_log_has_location_and_fixed_message_only() {
        let cap = Capture::default();
        let sub = tracing_subscriber::fmt()
            .json()
            .with_writer(cap.clone())
            .finish();
        tracing::subscriber::with_default(sub, || log_panic(Some(Location::caller())));
        let out = cap.text();
        assert!(out.contains("\"message\":\"panic\""));
        assert!(out.contains("redact.rs"));
    }

    async fn serve_once(response: &'static str) -> std::net::SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = l.accept().await {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(response.as_bytes()).await;
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn http_error_kinds_carry_no_url() {
        // Connect: nothing listens on port 1.
        let e = reqwest::get("http://127.0.0.1:1/secret-path?apiKey=SHOULDNOTLEAK")
            .await
            .unwrap_err();
        let k = http_error(e);
        assert_eq!(k, HttpErrorKind::Connect);
        assert!(!format!("{k} {k:?}").contains("SHOULDNOTLEAK"));

        // Status: a 500 turned into an error.
        let a = serve_once("HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n").await;
        let e = reqwest::get(format!("http://{a}/x"))
            .await
            .unwrap()
            .error_for_status()
            .unwrap_err();
        assert_eq!(http_error(e), HttpErrorKind::Status(500));

        // Decode: 200 with a body that is not JSON.
        let a = serve_once(
            "HTTP/1.1 200 OK\r\ncontent-length: 4\r\ncontent-type: application/json\r\n\r\nnope",
        )
        .await;
        let e = reqwest::Client::new()
            .get(format!("http://{a}/x"))
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap_err();
        assert_eq!(http_error(e), HttpErrorKind::Decode);

        // Timeout: the server accepts and never answers.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = l.local_addr().unwrap();
        tokio::spawn(async move {
            let _held = l.accept().await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        });
        let c = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(100))
            .build()
            .unwrap();
        let e = c.get(format!("http://{a}/x")).send().await.unwrap_err();
        assert_eq!(http_error(e), HttpErrorKind::Timeout);
    }
}
