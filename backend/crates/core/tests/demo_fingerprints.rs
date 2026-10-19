//! 05 section 3.4: `backend/demo-fingerprints.txt` and the repo's `.env.example` must not drift. Every line
//! `<sha256-hex>  <NAME>` must equal the SHA-256 of `NAME`'s value in `.env.example`.
//!
//! `.env.example` lives outside `backend/`, so it is read at run time (it is mounted at `/work` in `make test-backend`).
//! When it is not there (a bare checkout of `backend/`), the test says so and passes vacuously.

use std::path::PathBuf;

use famweather_core::config::DEMO_FINGERPRINTS;
use sha2::{Digest, Sha256};

#[test]
fn fingerprints_match_env_example() {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", "..", ".env.example"]
        .iter()
        .collect();
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("demo_fingerprints: .env.example not found, skipping");
        return;
    };
    let value_of = |name: &str| -> String {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| {
                panic!("{name} is listed in demo-fingerprints.txt but missing from .env.example")
            })
            .trim_matches('\'')
            .to_owned()
    };
    let mut checked = 0;
    for line in DEMO_FINGERPRINTS.lines().filter(|l| !l.trim().is_empty()) {
        let (hash, name) = line
            .split_once(char::is_whitespace)
            .expect("<sha256>  <NAME>");
        let name = name.trim();
        let got = hex::encode(Sha256::digest(value_of(name).as_bytes()));
        assert_eq!(
            got, hash,
            "{name}: .env.example no longer matches backend/demo-fingerprints.txt"
        );
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} fingerprint lines checked");
}
