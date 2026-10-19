//! The panic hook is process-global, so it is tested alone in its own test binary.

use std::io::Write;
use std::sync::{Arc, Mutex};

use famweather_core::redact;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

#[test]
fn panic_payload_never_reaches_the_log() {
    let cap = Capture::default();
    let sub = tracing_subscriber::fmt()
        .json()
        .with_writer(redact::ScrubMakeWriter(cap.clone()))
        .finish();
    redact::install_panic_hook();
    tracing::subscriber::with_default(sub, || {
        let r = std::panic::catch_unwind(|| {
            panic!("payload with TESTKEY-abcdef123456 and https://rt.example.net/v1/devices?apiKey=abc");
        });
        assert!(r.is_err());
    });
    let out = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
    assert!(out.contains("\"message\":\"panic\""), "{out}");
    assert!(out.contains("panic_hook.rs"), "file is logged: {out}");
    assert!(out.contains("\"line\":"), "line is logged: {out}");
    assert!(!out.contains("TESTKEY"), "{out}");
    assert!(!out.contains("payload"), "{out}");
    assert!(!out.contains("example.net"), "{out}");
}
