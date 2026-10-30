//! Retry classification and execution shared by every Elasticsearch/OpenSearch
//! request.
//!
//! Data-safety rule (inherited from elasticdump-rs): scroll continuations
//! advance a server-side cursor, so they are retried only when the request
//! provably did not take effect (HTTP 429 or a connect-phase failure). Every
//! other request is idempotent and retried per the configured [`RetryOn`] set.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use anyhow::{Result, anyhow};
use bytes::Bytes;
use log::warn;
use tokio_util::sync::CancellationToken;

use super::client::SendError;

/// Sentinel error marking a task that stopped because the pipeline was
/// cancelled (Ctrl+C, closed stdout reader, size cap reached, or a failing
/// sibling task) rather than because it failed itself.
#[derive(Debug, Clone, Copy)]
pub struct DumpCancelled;

impl fmt::Display for DumpCancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("dump cancelled")
    }
}

impl std::error::Error for DumpCancelled {}

/// `true` when `error` is (or wraps) the [`DumpCancelled`] sentinel.
pub fn is_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<DumpCancelled>().is_some()
}

/// A non-2xx response, kept structured so callers can match on the status or
/// on the error type in the body (e.g. `resource_already_exists_exception`).
#[derive(Debug, Clone)]
pub struct EsHttpError {
    pub prefix: String,
    pub operation: String,
    pub status: u16,
    pub body: String,
}

impl fmt::Display for EsHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}Elasticsearch {} failed with HTTP {}: {}",
            self.prefix, self.operation, self.status, self.body
        )
    }
}

impl std::error::Error for EsHttpError {}

impl EsHttpError {
    pub fn error_type_is(&self, ty: &str) -> bool {
        self.body.contains(&format!("\"type\":\"{ty}\""))
    }
}

/// How the delay grows between attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backoff {
    /// Same delay every time (elasticdump/requestretry behaviour).
    #[default]
    Fixed,
    /// Doubling delay capped at [`MAX_RETRY_DELAY`].
    Exponential,
}

/// Which failures are retryable for idempotent requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryOn {
    pub network: bool,
    pub timeout: bool,
    pub any_5xx: bool,
    pub s429: bool,
    pub s408: bool,
    /// Additional explicit statuses (0 = unused slot).
    pub extra: [u16; 8],
}

impl Default for RetryOn {
    /// elasticdump's `HTTPOrNetworkError` strategy (network errors and 5xx),
    /// plus 429/408, which elasticdump does not retry but which are the most
    /// common transient rejections.
    fn default() -> Self {
        Self {
            network: true,
            timeout: true,
            any_5xx: true,
            s429: true,
            s408: true,
            extra: [0; 8],
        }
    }
}

impl RetryOn {
    /// Parse a `--retryOn` list: `network,timeout,5xx,429,408,<status>...`.
    pub fn parse(spec: &str) -> Result<Self> {
        let mut on = Self {
            network: false,
            timeout: false,
            any_5xx: false,
            s429: false,
            s408: false,
            extra: [0; 8],
        };
        let mut n_extra = 0;
        for raw in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match raw.to_ascii_lowercase().as_str() {
                "network" => on.network = true,
                "timeout" => on.timeout = true,
                "5xx" => on.any_5xx = true,
                "429" => on.s429 = true,
                "408" => on.s408 = true,
                other => {
                    let status: u16 = other
                        .parse()
                        .map_err(|_| anyhow!("--retryOn: unknown condition '{raw}'"))?;
                    if !(100..600).contains(&status) {
                        return Err(anyhow!("--retryOn: '{raw}' is not an HTTP status"));
                    }
                    if n_extra == on.extra.len() {
                        return Err(anyhow!("--retryOn: at most 8 explicit statuses"));
                    }
                    on.extra[n_extra] = status;
                    n_extra += 1;
                }
            }
        }
        Ok(on)
    }

    pub fn status(&self, status: u16) -> bool {
        (self.any_5xx && (500..600).contains(&status))
            || (self.s429 && status == 429)
            || (self.s408 && status == 408)
            || self.extra.contains(&status)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Extra attempts after the first one; 0 disables retries.
    pub attempts: usize,
    /// Delay before the first retry.
    pub base_delay: Duration,
    pub backoff: Backoff,
    pub retry_on: RetryOn,
}

impl RetryPolicy {
    pub fn none() -> Self {
        Self {
            attempts: 0,
            base_delay: Duration::ZERO,
            backoff: Backoff::Fixed,
            retry_on: RetryOn::default(),
        }
    }

    pub fn delay_for(&self, attempt: usize) -> Duration {
        match self.backoff {
            Backoff::Fixed => self.base_delay,
            Backoff::Exponential => {
                let exponent = u32::try_from(attempt).unwrap_or(u32::MAX);
                self.base_delay
                    .saturating_mul(2u32.saturating_pow(exponent))
                    .min(MAX_RETRY_DELAY)
            }
        }
    }
}

pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// How a request may be retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryMode {
    /// Safe to repeat: PIT open/pages, initial searches, bulk, metadata calls.
    Idempotent,
    /// Scroll continuations: retry only on HTTP 429 or a connect failure.
    RejectionOnly,
}

/// Classified failure of one send attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOutcome {
    /// The connection could not be established; the server never saw it.
    ConnectError,
    /// The request timed out; the server may or may not have processed it.
    Timeout,
    /// Any other transport failure (reset, broken pipe, body transfer).
    TransportError,
    /// The server answered with this non-2xx status.
    Status(u16),
}

pub fn should_retry(policy: &RetryPolicy, mode: RetryMode, outcome: &RetryOutcome) -> bool {
    let on = &policy.retry_on;
    match mode {
        RetryMode::Idempotent => match outcome {
            RetryOutcome::ConnectError | RetryOutcome::TransportError => on.network,
            RetryOutcome::Timeout => on.timeout,
            RetryOutcome::Status(status) => on.status(*status),
        },
        RetryMode::RejectionOnly => match outcome {
            RetryOutcome::ConnectError => on.network,
            RetryOutcome::Status(429) => on.s429 || on.status(429),
            _ => false,
        },
    }
}

fn classify(error: &reqwest::Error) -> RetryOutcome {
    if error.is_connect() {
        RetryOutcome::ConnectError
    } else if error.is_timeout() {
        RetryOutcome::Timeout
    } else {
        RetryOutcome::TransportError
    }
}

/// `error: source: source...`, so TLS and connect failures name their cause
/// (reqwest's own Display stops at "error sending request").
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

fn prefix_for(slice_id: Option<usize>) -> String {
    match slice_id {
        Some(id) => format!("Slice {id}: "),
        None => String::new(),
    }
}

/// Send a request built by `build_and_send`, validate the status, and retry
/// per `mode`. Cancellation (checked before each attempt and while sleeping)
/// yields [`DumpCancelled`].
pub async fn send_checked_with_retry<F, Fut>(
    retry: RetryPolicy,
    cancel: &CancellationToken,
    mode: RetryMode,
    slice_id: usize,
    operation: &str,
    build_and_send: F,
) -> Result<Bytes>
where
    F: Fn() -> Fut,
    Fut: Future<Output = std::result::Result<reqwest::Response, SendError>>,
{
    send_inner(
        retry,
        cancel,
        mode,
        Some(slice_id),
        operation,
        build_and_send,
    )
    .await
}

/// Same as [`send_checked_with_retry`] without a slice prefix in messages.
pub async fn send_with_retry<F, Fut>(
    retry: RetryPolicy,
    cancel: &CancellationToken,
    mode: RetryMode,
    operation: &str,
    build_and_send: F,
) -> Result<Bytes>
where
    F: Fn() -> Fut,
    Fut: Future<Output = std::result::Result<reqwest::Response, SendError>>,
{
    send_inner(retry, cancel, mode, None, operation, build_and_send).await
}

async fn send_inner<F, Fut>(
    retry: RetryPolicy,
    cancel: &CancellationToken,
    mode: RetryMode,
    slice_id: Option<usize>,
    operation: &str,
    build_and_send: F,
) -> Result<Bytes>
where
    F: Fn() -> Fut,
    Fut: Future<Output = std::result::Result<reqwest::Response, SendError>>,
{
    let prefix = prefix_for(slice_id);
    let mut attempt: usize = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(anyhow::Error::new(DumpCancelled));
        }

        let mut retry_after: Option<Duration> = None;
        let (outcome, error) = match build_and_send().await {
            Ok(response) => {
                let status = response.status();
                if matches!(status.as_u16(), 429 | 503) {
                    retry_after = parse_retry_after(response.headers());
                }
                match response.bytes().await {
                    Ok(bytes) if status.is_success() => return Ok(bytes),
                    Ok(bytes) => {
                        let error =
                            validate_response_bytes(status.as_u16(), bytes, &prefix, operation)
                                .expect_err("non-success status must produce an error");
                        (RetryOutcome::Status(status.as_u16()), error)
                    }
                    // The server already processed the request; classify as a
                    // transport error so scroll continuations never retry it.
                    Err(e) => (
                        RetryOutcome::TransportError,
                        anyhow!("{prefix}Failed to read {operation} response bytes: {e}"),
                    ),
                }
            }
            // Refused before sending (unsafe path): never retried.
            Err(SendError::InvalidPath(m)) => {
                return Err(anyhow!("{prefix}{operation}: {m}"));
            }
            Err(SendError::Http(e)) => (
                classify(&e),
                anyhow!(
                    "{prefix}Elasticsearch {operation} request failed: {}",
                    error_chain(&e)
                ),
            ),
        };

        if attempt >= retry.attempts || !should_retry(&retry, mode, &outcome) {
            return Err(error);
        }

        // A server-sent `Retry-After` (seconds) on 429/503 is honoured as a
        // minimum delay, capped at MAX_RETRY_DELAY.
        let delay = retry
            .delay_for(attempt)
            .max(retry_after.unwrap_or_default().min(MAX_RETRY_DELAY));
        warn!(
            "{prefix}{operation} failed (attempt {} of {}), retrying in {:?}: {:#}",
            attempt + 1,
            retry.attempts + 1,
            delay,
            error
        );

        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = cancel.cancelled() => return Err(anyhow::Error::new(DumpCancelled)),
        }
        attempt += 1;
    }
}

/// `Retry-After` in delay-seconds form (the HTTP-date form is ignored).
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let v = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    v.trim().parse::<u64>().ok().map(Duration::from_secs)
}

pub fn validate_response_bytes(
    status: u16,
    response_bytes: Bytes,
    prefix: &str,
    operation: &str,
) -> Result<Bytes> {
    if !(200..300).contains(&status) {
        return Err(anyhow::Error::new(EsHttpError {
            prefix: prefix.to_string(),
            operation: operation.to_string(),
            status,
            body: String::from_utf8_lossy(response_bytes.as_ref()).into_owned(),
        }));
    }
    Ok(response_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RetryPolicy {
        RetryPolicy {
            attempts: 3,
            base_delay: Duration::from_millis(100),
            backoff: Backoff::Fixed,
            retry_on: RetryOn::default(),
        }
    }

    #[test]
    fn idempotent_requests_retry_network_5xx_429_408() {
        let p = policy();
        for o in [
            RetryOutcome::ConnectError,
            RetryOutcome::TransportError,
            RetryOutcome::Timeout,
            RetryOutcome::Status(500),
            RetryOutcome::Status(502),
            RetryOutcome::Status(503),
            RetryOutcome::Status(504),
            RetryOutcome::Status(429),
            RetryOutcome::Status(408),
        ] {
            assert!(should_retry(&p, RetryMode::Idempotent, &o), "{o:?}");
        }
        for s in [400, 401, 403, 404, 409] {
            assert!(!should_retry(
                &p,
                RetryMode::Idempotent,
                &RetryOutcome::Status(s)
            ));
        }
    }

    #[test]
    fn scroll_continuations_retry_only_when_provably_not_applied() {
        let p = policy();
        assert!(should_retry(
            &p,
            RetryMode::RejectionOnly,
            &RetryOutcome::Status(429)
        ));
        assert!(should_retry(
            &p,
            RetryMode::RejectionOnly,
            &RetryOutcome::ConnectError
        ));
        for o in [
            RetryOutcome::Timeout,
            RetryOutcome::TransportError,
            RetryOutcome::Status(500),
            RetryOutcome::Status(503),
        ] {
            assert!(!should_retry(&p, RetryMode::RejectionOnly, &o), "{o:?}");
        }
    }

    #[test]
    fn retry_on_parses_lists() {
        let on = RetryOn::parse("network,503").unwrap();
        assert!(on.network && !on.timeout && !on.any_5xx && !on.s429);
        assert!(on.status(503) && !on.status(502) && !on.status(429));
        assert!(RetryOn::parse("bogus").is_err());
        assert!(RetryOn::parse("999").is_err());
    }

    #[test]
    fn backoff_fixed_and_exponential() {
        let mut p = policy();
        assert_eq!(p.delay_for(0), Duration::from_millis(100));
        assert_eq!(p.delay_for(5), Duration::from_millis(100));
        p.backoff = Backoff::Exponential;
        assert_eq!(p.delay_for(3), Duration::from_millis(800));
        assert_eq!(p.delay_for(30), MAX_RETRY_DELAY);
    }

    /// Scripted responses: each call pops the next (status, Retry-After) and
    /// records the (virtual) instant of the attempt.
    struct Script {
        replies: std::sync::Mutex<std::collections::VecDeque<(u16, Option<&'static str>)>>,
        at: std::sync::Mutex<Vec<tokio::time::Instant>>,
    }

    impl Script {
        fn new(replies: &[(u16, Option<&'static str>)]) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                replies: std::sync::Mutex::new(replies.iter().copied().collect()),
                at: std::sync::Mutex::new(vec![]),
            })
        }
        fn gaps(&self) -> Vec<Duration> {
            let at = self.at.lock().unwrap();
            at.windows(2).map(|w| w[1] - w[0]).collect()
        }
    }

    async fn drive(
        p: RetryPolicy,
        mode: RetryMode,
        script: &std::sync::Arc<Script>,
    ) -> Result<Bytes> {
        let cancel = CancellationToken::new();
        send_with_retry(p, &cancel, mode, "op", || {
            let s = script.clone();
            async move {
                s.at.lock().unwrap().push(tokio::time::Instant::now());
                let (status, ra) = s
                    .replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected attempt");
                let mut b = http::Response::builder().status(status);
                if let Some(ra) = ra {
                    b = b.header("Retry-After", ra);
                }
                Ok(reqwest::Response::from(b.body("{}").unwrap()))
            }
        })
        .await
    }

    // L-426 (TST-API-050): exact attempt counts and delays per request kind.
    #[tokio::test(start_paused = true)]
    async fn attempts_and_delays_per_request_kind() {
        // Idempotent kinds (initial search, PIT, whole _bulk, meta PUTs): 429 twice then 200.
        let s = Script::new(&[(429, None), (429, None), (200, None)]);
        drive(policy(), RetryMode::Idempotent, &s).await.unwrap();
        assert_eq!(s.gaps(), vec![Duration::from_millis(100); 2]);
        // Exponential backoff: 100, 200, 400 ms; then attempts exhausted (1 + 3).
        let mut p = policy();
        p.backoff = Backoff::Exponential;
        let s = Script::new(&[(503, None), (502, None), (500, None), (504, None)]);
        let e = drive(p, RetryMode::Idempotent, &s).await.unwrap_err();
        assert!(e.to_string().contains("HTTP 504"), "{e}");
        assert_eq!(
            s.gaps(),
            [100, 200, 400].map(Duration::from_millis).to_vec()
        );
        // Scroll continuation: 429 is retried ...
        let s = Script::new(&[(429, None), (200, None)]);
        drive(policy(), RetryMode::RejectionOnly, &s).await.unwrap();
        assert_eq!(s.gaps().len(), 1);
        // ... 5xx is not (one attempt only).
        let s = Script::new(&[(503, None)]);
        assert!(drive(policy(), RetryMode::RejectionOnly, &s).await.is_err());
        assert_eq!(s.at.lock().unwrap().len(), 1);
        // --retryAttempts 0: exactly one attempt.
        let mut p = policy();
        p.attempts = 0;
        let s = Script::new(&[(429, None)]);
        assert!(drive(p, RetryMode::Idempotent, &s).await.is_err());
        assert_eq!(s.at.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_after_is_a_capped_minimum() {
        let s = Script::new(&[
            (429, Some("7")),
            (503, Some("600")),
            (429, Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            (200, None),
        ]);
        drive(policy(), RetryMode::Idempotent, &s).await.unwrap();
        assert_eq!(
            s.gaps(),
            vec![
                Duration::from_secs(7),
                MAX_RETRY_DELAY,
                Duration::from_millis(100)
            ]
        );
        // Retry-After below the configured delay does not shorten it.
        let mut p = policy();
        p.base_delay = Duration::from_secs(5);
        let s = Script::new(&[(429, Some("1")), (200, None)]);
        drive(p, RetryMode::RejectionOnly, &s).await.unwrap();
        assert_eq!(s.gaps(), vec![Duration::from_secs(5)]);
    }

    #[test]
    fn is_cancelled_detects_wrapped_sentinel() {
        let e = anyhow::Error::new(DumpCancelled).context("outer");
        assert!(is_cancelled(&e));
        assert!(!is_cancelled(&anyhow!("other")));
    }

    #[test]
    fn http_errors_keep_status_and_type() {
        let err = validate_response_bytes(
            400,
            Bytes::from_static(br#"{"error":{"type":"resource_already_exists_exception"}}"#),
            "",
            "index create",
        )
        .unwrap_err();
        let http = err.downcast_ref::<EsHttpError>().unwrap();
        assert_eq!(http.status, 400);
        assert!(http.error_type_is("resource_already_exists_exception"));
        assert!(validate_response_bytes(200, Bytes::new(), "", "x").is_ok());
    }
}
