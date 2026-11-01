//! Shared backend code (conventions "Shared Rust code: ownership and build order").
//!
//! This is the 03 core milestone: `config`, `db`, `redact`, `clock`, `awn`, `ratelimit`.
//! `crypto` (HPKE credential encryption) and `testdb` (feature `testdb`) belong to 04.
//!
//! The guides call this crate `core`; the package is `famweather-core` (library `famweather_core`)
//! because a crate named `core` would shadow Rust's built-in `core`.

pub mod acis;
pub mod awn;
pub mod clock;
pub mod config;
pub mod crypto;
pub mod db;
pub mod heartbeat;
pub mod rain;
pub mod ratelimit;
pub mod redact;
#[cfg(feature = "testdb")]
pub mod testdb;
pub mod time;
