//! Thin Elasticsearch/OpenSearch HTTP client on top of reqwest.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use base64::prelude::*;
use bytes::Bytes;
use http::header::{
    ACCEPT, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue,
    USER_AGENT,
};
use reqwest::Method;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::gate::RequestGate;
use super::info::ClusterInfo;
use super::retry::{RetryMode, RetryPolicy, send_with_retry};
use super::tls::{TlsOptions, build_client_config};

/// Request body variants. Bytes are cheap to clone, so retries resend them.
#[derive(Debug, Clone)]
pub enum Body {
    Empty,
    Json(Bytes),
    Ndjson(Bytes),
    /// Gzip-compressed NDJSON (`--esCompress` on bulk writes).
    NdjsonGzip(Bytes),
}

impl Body {
    pub fn json(value: &impl serde::Serialize) -> Self {
        Body::Json(Bytes::from(
            serde_json::to_vec(value).expect("serializable body"),
        ))
    }

    pub fn sonic(value: &sonic_rs::Value) -> Self {
        Body::Json(Bytes::from(
            sonic_rs::to_vec(value).expect("serializable body"),
        ))
    }
}

/// Everything needed to build a client for one endpoint.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Base URL ending in `/` (may include a proxy base path).
    pub base: Url,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Extra request headers (`--headers`, `--{input,output}-headers`), already
    /// merged in precedence order.
    pub headers: Vec<(String, String)>,
    /// Extra query parameters (`--{input,output}-params` or `--params`).
    pub params: Vec<(String, String)>,
    pub tls: TlsOptions,
    pub timeout: Option<Duration>,
    /// Ask for compressed responses and gzip bulk request bodies.
    pub compress: bool,
    pub max_sockets: Option<usize>,
    pub debug: bool,
}

impl ClientOptions {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            username: None,
            password: None,
            headers: vec![],
            params: vec![],
            tls: TlsOptions::default(),
            timeout: None,
            compress: false,
            max_sockets: None,
            debug: false,
        }
    }
}

struct Inner {
    http: reqwest::Client,
    base: Url,
    params: Vec<(String, String)>,
    compress: bool,
    gate: RequestGate,
    info: OnceCell<ClusterInfo>,
    debug: bool,
}

/// Error from [`EsClient::send`]: a transport error, or a request that was
/// refused before anything was sent because its path was unsafe.
#[derive(Debug)]
pub enum SendError {
    Http(reqwest::Error),
    InvalidPath(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Http(e) => e.fmt(f),
            SendError::InvalidPath(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SendError::Http(e) => Some(e),
            SendError::InvalidPath(_) => None,
        }
    }
}

#[derive(Clone)]
pub struct EsClient(Arc<Inner>);

impl std::fmt::Debug for EsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EsClient")
            .field("base", &self.0.base.as_str())
            .finish()
    }
}

impl EsClient {
    pub fn new(opts: &ClientOptions, gate: RequestGate) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("elasticdump"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        match (opts.username.as_deref(), opts.password.as_deref()) {
            (Some(user), pass) => {
                if pass.is_none() {
                    log::warn!(
                        "No password provided for user '{user}'; sending basic auth with an empty password"
                    );
                }
                let token = BASE64_STANDARD.encode(format!("{user}:{}", pass.unwrap_or("")));
                let mut value = HeaderValue::from_str(&format!("Basic {token}"))
                    .context("Invalid basic-auth credentials")?;
                value.set_sensitive(true);
                headers.insert(AUTHORIZATION, value);
            }
            (None, Some(_)) => {
                return Err(anyhow!("A password was provided without a username"));
            }
            (None, None) => {}
        }

        for (name, value) in &opts.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .with_context(|| format!("Invalid header name '{name}'"))?;
            let mut value = HeaderValue::from_str(value)
                .with_context(|| format!("Invalid value for header '{name}'"))?;
            if matches!(name.as_str(), "authorization" | "cookie" | "x-api-key") {
                value.set_sensitive(true);
            }
            headers.insert(name, value);
        }

        if opts.tls.insecure {
            log::warn!(
                "TLS certificate verification is DISABLED for {}",
                crate::endpoint::redacted_url(&opts.base)
            );
        }
        let tls = build_client_config(&opts.tls)?;

        let mut builder = reqwest::Client::builder()
            .default_headers(headers)
            .use_preconfigured_tls(tls)
            .gzip(opts.compress);
        if let Some(timeout) = opts.timeout {
            builder = builder.timeout(timeout);
        }
        if let Some(n) = opts.max_sockets {
            builder = builder.pool_max_idle_per_host(n);
        }
        let http = builder.build().context("Failed to build HTTP client")?;

        let mut base = opts.base.clone();
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }

        Ok(Self(Arc::new(Inner {
            http,
            base,
            params: opts.params.clone(),
            compress: opts.compress,
            gate,
            info: OnceCell::new(),
            debug: opts.debug,
        })))
    }

    pub fn base(&self) -> &Url {
        &self.0.base
    }

    pub fn compress_requests(&self) -> bool {
        self.0.compress
    }

    /// Build the URL for path `segments` below the base (each segment is
    /// percent-encoded individually) plus query pairs.
    ///
    /// A `.`, `..` or empty segment is an error: the `url` crate would drop
    /// it silently, so `PUT /{..}/_settings` would become `PUT /_settings`
    /// (INJ-F2, TST-INJ-007).
    pub fn url(
        &self,
        segments: &[&str],
        query: &[(&str, &str)],
        with_params: bool,
    ) -> std::result::Result<Url, SendError> {
        if let Some(bad) = segments
            .iter()
            .find(|s| s.is_empty() || **s == "." || **s == "..")
        {
            return Err(SendError::InvalidPath(format!(
                "refusing to send a request with the path segment {bad:?} in /{}",
                segments.join("/")
            )));
        }
        let mut url = self.0.base.clone();
        {
            let mut segs = url.path_segments_mut().expect("http(s) base URL");
            segs.pop_if_empty();
            for s in segments {
                segs.push(s);
            }
        }
        {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in query {
                pairs.append_pair(k, v);
            }
            if with_params {
                for (k, v) in &self.0.params {
                    pairs.append_pair(k, v);
                }
            }
        }
        if url.query() == Some("") {
            url.set_query(None);
        }
        Ok(url)
    }

    /// Send one request. `segments` are path segments below the base URL;
    /// `with_params` appends the user's `--params` for this direction.
    pub async fn send(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, &str)],
        with_params: bool,
        body: Body,
    ) -> std::result::Result<reqwest::Response, SendError> {
        let url = self.url(segments, query, with_params)?;
        if self.0.debug {
            log::debug!("{} {}", method, url);
        }
        let mut rb = self.0.http.request(method, url);
        rb = match body {
            Body::Empty => rb,
            Body::Json(b) => rb.header(CONTENT_TYPE, "application/json").body(b),
            Body::Ndjson(b) => rb.header(CONTENT_TYPE, "application/x-ndjson").body(b),
            Body::NdjsonGzip(b) => rb
                .header(CONTENT_TYPE, "application/x-ndjson")
                .header(CONTENT_ENCODING, "gzip")
                .body(b),
        };
        let _permit = self.0.gate.acquire().await;
        rb.send().await.map_err(SendError::Http)
    }

    /// Cluster flavor and version (`GET /`), fetched once and cached.
    pub async fn info(
        &self,
        retry: RetryPolicy,
        cancel: &CancellationToken,
    ) -> Result<&ClusterInfo> {
        self.0
            .info
            .get_or_try_init(|| async {
                let body = send_with_retry(
                    retry,
                    cancel,
                    RetryMode::Idempotent,
                    "version check",
                    move || self.send(Method::GET, &[], &[], false, Body::Empty),
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to contact {}",
                        crate::endpoint::redacted_url(&self.0.base)
                    )
                })?;
                let info = ClusterInfo::parse(&body);
                log::debug!(
                    "Detected {} {} at {}",
                    info.flavor_name(),
                    info.version,
                    crate::endpoint::redacted_url(&self.0.base)
                );
                Ok(info)
            })
            .await
    }

    /// Pre-seed the cached cluster info (tests).
    #[cfg(test)]
    pub fn set_info(&self, info: ClusterInfo) {
        let _ = self.0.info.set(info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(base: &str, params: Vec<(String, String)>) -> EsClient {
        let mut o = ClientOptions::new(Url::parse(base).unwrap());
        o.params = params;
        EsClient::new(&o, RequestGate::default()).unwrap()
    }

    #[test]
    fn url_joins_segments_under_base_path_and_encodes() {
        let c = client("https://h:9200/proxy", vec![]);
        assert_eq!(
            c.url(&["my index", "_search"], &[("scroll", "1m")], false)
                .unwrap()
                .as_str(),
            "https://h:9200/proxy/my%20index/_search?scroll=1m"
        );
        let c = client(
            "http://h:9200/",
            vec![("preference".into(), "_local".into())],
        );
        assert_eq!(c.url(&[], &[], false).unwrap().as_str(), "http://h:9200/");
        assert_eq!(
            c.url(&["a,b*", "_bulk"], &[], true).unwrap().as_str(),
            "http://h:9200/a,b*/_bulk?preference=_local"
        );
    }

    /// TST-INJ-007: segments never vanish; unsafe ones are an error.
    #[test]
    fn url_segments_never_vanish() {
        let c = client("http://h:9200/base", vec![]);
        for bad in [".", "..", ""] {
            assert!(c.url(&[bad, "_settings"], &[], false).is_err(), "{bad:?}");
            assert!(c.url(&["i", bad], &[], false).is_err(), "{bad:?}");
        }
        let samples = [
            "a/b",
            "a?b",
            "a#b",
            "a%b",
            "a b",
            "\u{e9}t\u{e9}",
            "...",
            ".a",
            "a.",
            "%2e",
            "%2E%2E",
            "+",
            "\u{1F600}",
        ];
        for s in samples {
            let u = c.url(&[s, "_doc", s], &[], false).unwrap();
            let segs: Vec<&str> = u.path_segments().unwrap().collect();
            assert_eq!(segs.len(), 4, "{s:?} -> {u}");
            assert_eq!(segs[0], "base");
            let decoded = percent_encoding::percent_decode_str(segs[1])
                .decode_utf8()
                .unwrap();
            assert_eq!(decoded, s, "{s:?} -> {u}");
            for ch in ['/', '?', '#', ' '] {
                assert!(!segs[1].contains(ch), "{s:?} -> {u}");
            }
        }
    }

    /// TST-INJ-012: query values with & = # % + and Unicode are encoded so
    /// the server decodes them unchanged (golden URLs).
    #[test]
    fn query_values_are_encoded_exactly() {
        let c = client(
            "http://h:9200/",
            vec![("q".into(), "a&b=c#d%e+f \u{e9}".into())],
        );
        let u = c
            .url(&["i", "_search"], &[("scroll_id", "AbC==/+x")], true)
            .unwrap();
        assert_eq!(
            u.as_str(),
            "http://h:9200/i/_search?scroll_id=AbC%3D%3D%2F%2Bx&q=a%26b%3Dc%23d%25e%2Bf+%C3%A9"
        );
        let pairs: Vec<(String, String)> = u.query_pairs().into_owned().collect();
        assert_eq!(
            pairs,
            vec![
                ("scroll_id".to_string(), "AbC==/+x".to_string()),
                ("q".to_string(), "a&b=c#d%e+f \u{e9}".to_string())
            ]
        );
    }

    #[test]
    fn password_without_user_is_rejected() {
        let mut o = ClientOptions::new(Url::parse("http://h:9200/").unwrap());
        o.password = Some("x".into());
        let err = EsClient::new(&o, RequestGate::default()).unwrap_err();
        assert!(err.to_string().contains("without a username"));
    }

    #[test]
    fn invalid_header_is_rejected() {
        let mut o = ClientOptions::new(Url::parse("http://h:9200/").unwrap());
        o.headers = vec![("bad header".into(), "v".into())];
        assert!(EsClient::new(&o, RequestGate::default()).is_err());
    }
}
