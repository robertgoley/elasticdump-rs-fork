//! HTTP layer: client, TLS, retries, admission control, cluster detection.

pub mod client;
pub mod gate;
pub mod info;
pub mod retry;
pub mod tls;

pub use client::{Body, ClientOptions, EsClient};
pub use info::{ClusterInfo, Flavor, PitApi};
pub use retry::{
    Backoff, DumpCancelled, EsHttpError, RetryMode, RetryOn, RetryPolicy, is_cancelled,
    send_checked_with_retry, send_with_retry,
};
