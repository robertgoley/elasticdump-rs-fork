//! Podman-backed integration suite. Run via `scripts/it/run.sh`; requires the
//! environment written by `scripts/it/up.sh`.

mod harness;

mod smoke;
mod tls_auth;
