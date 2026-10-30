//! Podman-backed integration suite. Run via `scripts/it/run.sh`; requires the
//! environment written by `scripts/it/up.sh`.

mod harness;
#[path = "../support/mod.rs"]
mod support;

mod golden;

mod api;

mod concurrency;
mod cross;
mod data;
mod engine;
mod files;
mod meta;
mod meta_types;
mod node_interop;
mod smoke;
mod tls_auth;
mod xmatrix;
