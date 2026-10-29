//! Podman-backed integration suite. Run via `scripts/it/run.sh`; requires the
//! environment written by `scripts/it/up.sh`.

mod harness;

mod concurrency;
mod cross;
mod data;
mod engine;
mod files;
mod meta;
mod smoke;
mod tls_auth;
