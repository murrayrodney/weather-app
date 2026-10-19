//! Family weather API (02-api). Library + binary so `tests/` can build the router.

pub mod access;
pub mod app;
pub mod audit;
pub mod auth;
pub mod baseline;
pub mod config;
pub mod error;
pub mod extract;
pub mod limits;
pub mod openmeteo;
pub mod proxy;
pub mod repo;
pub mod routes;
pub mod state;
