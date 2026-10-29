//! Small JSON request helpers shared by the metadata types.

use anyhow::{Context, Result, bail};
use reqwest::Method;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::http::{Body, EsClient, EsHttpError, RetryMode, RetryPolicy, send_with_retry};

pub async fn get_json(
    client: &EsClient,
    retry: RetryPolicy,
    segments: &[&str],
    what: &str,
) -> Result<Value> {
    get_json_q(client, retry, segments, &[], what).await
}

pub async fn get_json_q(
    client: &EsClient,
    retry: RetryPolicy,
    segments: &[&str],
    query: &[(&str, &str)],
    what: &str,
) -> Result<Value> {
    let cancel = CancellationToken::new();
    let bytes = send_with_retry(retry, &cancel, RetryMode::Idempotent, what, move || {
        client.send(Method::GET, segments, query, true, Body::Empty)
    })
    .await?;
    serde_json::from_slice(&bytes).with_context(|| format!("{what} returned invalid JSON"))
}

/// GET that maps a 404 to `None`.
pub async fn get_json_opt(
    client: &EsClient,
    retry: RetryPolicy,
    segments: &[&str],
    query: &[(&str, &str)],
    what: &str,
) -> Result<Option<Value>> {
    match get_json_q(client, retry, segments, query, what).await {
        Ok(v) => Ok(Some(v)),
        Err(e) if status_of(&e) == Some(404) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `HEAD` request: whether the resource exists.
pub async fn exists(client: &EsClient, retry: RetryPolicy, segments: &[&str]) -> Result<bool> {
    let cancel = CancellationToken::new();
    match send_with_retry(retry, &cancel, RetryMode::Idempotent, "HEAD", move || {
        client.send(Method::HEAD, segments, &[], true, Body::Empty)
    })
    .await
    {
        Ok(_) => Ok(true),
        Err(e) if status_of(&e) == Some(404) => Ok(false),
        Err(e) => Err(e),
    }
}

pub async fn send_json(
    client: &EsClient,
    retry: RetryPolicy,
    method: Method,
    segments: &[&str],
    body: &Value,
    what: &str,
) -> Result<Value> {
    send_json_q(client, retry, method, segments, &[], body, what).await
}

pub async fn send_json_q(
    client: &EsClient,
    retry: RetryPolicy,
    method: Method,
    segments: &[&str],
    query: &[(&str, &str)],
    body: &Value,
    what: &str,
) -> Result<Value> {
    let cancel = CancellationToken::new();
    let body = if body.is_null() {
        Body::Empty
    } else {
        Body::json(body)
    };
    let body = &body;
    let method = &method;
    let bytes = send_with_retry(retry, &cancel, RetryMode::Idempotent, what, move || {
        client.send(method.clone(), segments, query, true, body.clone())
    })
    .await?;
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if v.get("error").is_some_and(|e| !e.is_null()) {
        bail!("{what} failed: {v}");
    }
    Ok(v)
}

pub fn status_of(e: &anyhow::Error) -> Option<u16> {
    e.downcast_ref::<EsHttpError>().map(|h| h.status)
}

pub fn error_type_is(e: &anyhow::Error, ty: &str) -> bool {
    e.downcast_ref::<EsHttpError>()
        .is_some_and(|h| h.error_type_is(ty))
}

pub fn body_contains(e: &anyhow::Error, needle: &str) -> bool {
    e.downcast_ref::<EsHttpError>()
        .is_some_and(|h| h.body.contains(needle))
}

pub fn is_already_exists(e: &anyhow::Error) -> bool {
    error_type_is(e, "resource_already_exists_exception")
}
