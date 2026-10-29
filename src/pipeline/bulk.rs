//! Bulk writer: turns document lines (search hits or plain documents) into
//! `_bulk` requests.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use reqwest::Method;
use sonic_rs::{JsonValueTrait, LazyValue};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::http::{Body, ClusterInfo, EsClient, RetryMode, RetryPolicy, send_with_retry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkAction {
    Index,
    Create,
    Update,
    Delete,
}

impl BulkAction {
    pub fn parse(raw: &str) -> Result<Self> {
        Ok(match raw {
            "index" => Self::Index,
            "create" => Self::Create,
            "update" => Self::Update,
            "delete" => Self::Delete,
            other => bail!("invalid --bulkAction '{other}' (index, create, update, delete)"),
        })
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Index => "index",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BulkOptions {
    pub info: ClusterInfo,
    /// `--output-index` / URL index; `None` = use each document's `_index`.
    pub index: Option<String>,
    pub doc_type: Option<String>,
    pub prefix: String,
    pub suffix: String,
    pub action: BulkAction,
    pub handle_version: bool,
    pub version_type: Option<String>,
    /// Meta fields copied into the action line (bare names).
    pub meta_fields: Vec<String>,
    pub refresh: bool,
    pub ignore_es_write_errors: bool,
    pub ignore_errors: bool,
    pub max_bulk_bytes: usize,
    pub retry: RetryPolicy,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WriteReport {
    pub written: u64,
    pub failed: u64,
}

impl WriteReport {
    fn add(&mut self, other: WriteReport) {
        self.written += other.written;
        self.failed += other.failed;
    }
}

/// `_bulk` writer. With `--concurrency N > 1` each batch is written by a
/// worker task (a clone sharing client, options and the refresh lock); at most
/// N batches are in flight and [`BulkSink::drain`] collects their reports.
pub struct BulkSink {
    client: EsClient,
    opts: Arc<BulkOptions>,
    cancel: CancellationToken,
    pub report: WriteReport,
    warned_ttl: Arc<AtomicBool>,
    concurrency: usize,
    throttle: Option<Duration>,
    inflight: JoinSet<Result<WriteReport>>,
    /// At most one `_refresh` in flight (others are skipped).
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
}

fn get<'a>(line: &'a [u8], path: &[&str]) -> Option<LazyValue<'a>> {
    sonic_rs::get(line, path).ok().filter(|v| !v.is_null())
}

fn push_json_str(out: &mut Vec<u8>, s: &str) {
    sonic_rs::to_writer(&mut *out, s).expect("string serialization");
}

/// The top-level members of one document line, parsed in a single pass
/// (a hit has a handful of keys; looking each one up with `sonic_rs::get`
/// rescanned the whole line per lookup).
struct Top<'a> {
    members: Vec<(std::borrow::Cow<'a, str>, LazyValue<'a>)>,
}

impl<'a> Top<'a> {
    fn parse(line: &'a [u8]) -> Self {
        let members = sonic_rs::to_object_iter(line)
            .filter_map(|m| m.ok())
            .filter(|(_, v)| !v.is_null())
            .collect();
        Self { members }
    }

    fn get(&self, key: &str) -> Option<&LazyValue<'a>> {
        self.members.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn str(&self, key: &str) -> Option<String> {
        self.get(key).and_then(|v| v.as_str().map(str::to_string))
    }
}

/// Meta field lookup in elasticdump order: `X`, `_X`, `fields.X`, `fields._X`
/// (single-element arrays from `fields` are unwrapped). Returns raw JSON.
fn meta(top: &Top, name: &str) -> Option<String> {
    let under = format!("_{name}");
    if let Some(v) = top.get(name).or_else(|| top.get(&under)) {
        return Some(v.as_raw_str().to_string());
    }
    let fields = top.get("fields")?.as_raw_str().as_bytes();
    let v = get(fields, &[name]).or_else(|| get(fields, &[under.as_str()]))?;
    if !v.is_array() {
        return Some(v.as_raw_str().to_string());
    }
    let elems: Vec<LazyValue> = sonic_rs::to_array_iter(v.as_raw_str().as_bytes())
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    match elems.as_slice() {
        [one] => Some(one.as_raw_str().to_string()),
        _ => None,
    }
}

impl BulkSink {
    pub fn new(client: EsClient, opts: BulkOptions, cancel: CancellationToken) -> Self {
        Self {
            client,
            opts: Arc::new(opts),
            cancel,
            report: WriteReport::default(),
            warned_ttl: Arc::new(AtomicBool::new(false)),
            concurrency: 1,
            throttle: None,
            inflight: JoinSet::new(),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// `--concurrency` (max in-flight write batches) and `--throttleInterval`
    /// (pause after each batch is handed off).
    pub fn with_concurrency(mut self, concurrency: usize, throttle: Option<Duration>) -> Self {
        self.concurrency = concurrency.max(1);
        self.throttle = throttle.filter(|d| !d.is_zero());
        self
    }

    fn worker(&self) -> BulkSink {
        BulkSink {
            client: self.client.clone(),
            opts: Arc::clone(&self.opts),
            cancel: self.cancel.clone(),
            report: WriteReport::default(),
            warned_ttl: Arc::clone(&self.warned_ttl),
            concurrency: 1,
            throttle: None,
            inflight: JoinSet::new(),
            refresh_lock: Arc::clone(&self.refresh_lock),
        }
    }

    fn merge(
        &mut self,
        joined: std::result::Result<Result<WriteReport>, tokio::task::JoinError>,
    ) -> Result<()> {
        match joined {
            Ok(Ok(r)) => {
                self.report.add(r);
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(e) if e.is_cancelled() => Ok(()),
            Err(e) => Err(anyhow!("bulk worker panicked: {e}")),
        }
    }

    /// Hand one batch to the writer. Sequential with `--concurrency 1`.
    pub async fn write_batch(&mut self, buf: &[u8]) -> Result<()> {
        if self.concurrency <= 1 {
            self.process_batch(buf).await?;
        } else {
            while self.inflight.len() >= self.concurrency {
                let joined = self.inflight.join_next().await.expect("non-empty");
                if let Err(e) = self.merge(joined) {
                    self.inflight.abort_all();
                    return Err(e);
                }
            }
            let mut worker = self.worker();
            let data = Bytes::copy_from_slice(buf);
            self.inflight.spawn(async move {
                worker.process_batch(&data).await?;
                Ok(worker.report)
            });
        }
        if let Some(d) = self.throttle {
            tokio::time::sleep(d).await;
        }
        Ok(())
    }

    /// Wait for every in-flight batch; the first error wins.
    pub async fn drain(&mut self) -> Result<()> {
        let concurrent = self.concurrency > 1;
        while let Some(joined) = self.inflight.join_next().await {
            if let Err(e) = self.merge(joined) {
                self.inflight.abort_all();
                return Err(e);
            }
        }
        if concurrent && self.opts.refresh {
            // A per-batch refresh may have been skipped while another ran.
            let _g = self.refresh_lock.lock().await;
            self.refresh().await;
        }
        Ok(())
    }

    pub fn abort(&mut self) {
        self.inflight.abort_all();
    }

    fn index_for(&self, top: &Top) -> Result<String> {
        let base = match &self.opts.index {
            Some(i) => i.clone(),
            None => top.str("_index").ok_or_else(|| {
                anyhow!("document has no _index and no output index was given (--output-index)")
            })?,
        };
        Ok(format!("{}{}{}", self.opts.prefix, base, self.opts.suffix))
    }

    /// Append the action (and body) lines for one document.
    pub fn encode_doc(&mut self, line: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let action = self.opts.action;
        let top = Top::parse(line);
        let source = top.get("_source");
        let is_hit = source.is_some() || top.get("_id").is_some();

        out.extend_from_slice(b"{\"");
        out.extend_from_slice(action.as_str().as_bytes());
        out.extend_from_slice(b"\":{\"_index\":");
        push_json_str(out, &self.index_for(&top)?);
        if is_hit {
            if let Some(id) = top.get("_id") {
                out.extend_from_slice(b",\"_id\":");
                out.extend_from_slice(id.as_raw_str().as_bytes());
            }
        }
        if self.opts.info.has_types() {
            let ty = self
                .opts
                .doc_type
                .clone()
                .or_else(|| top.str("_type"))
                .unwrap_or_else(|| "_doc".to_string());
            out.extend_from_slice(b",\"_type\":");
            push_json_str(out, &ty);
        }
        if is_hit {
            for name in self.opts.meta_fields.iter() {
                if matches!(name.as_str(), "timestamp" | "ttl")
                    && !self.opts.info.supports_ttl_meta()
                {
                    if meta(&top, name).is_some() && !self.warned_ttl.swap(true, Ordering::Relaxed)
                    {
                        log::warn!("Dropping _{name}: not supported by the target cluster");
                    }
                    continue;
                }
                if let Some(v) = meta(&top, name) {
                    out.extend_from_slice(b",\"");
                    out.extend_from_slice(name.as_bytes());
                    out.extend_from_slice(b"\":");
                    out.extend_from_slice(v.as_bytes());
                }
            }
            if self.opts.handle_version {
                if let Some(v) = top.get("_version").or_else(|| top.get("version")) {
                    out.extend_from_slice(b",\"version\":");
                    out.extend_from_slice(v.as_raw_str().as_bytes());
                    if let Some(vt) = &self.opts.version_type {
                        out.extend_from_slice(b",\"version_type\":");
                        push_json_str(out, vt);
                    }
                }
            }
        }
        out.extend_from_slice(b"}}\n");

        match action {
            BulkAction::Delete => {}
            BulkAction::Update => {
                out.extend_from_slice(b"{\"doc\":");
                out.extend_from_slice(source_bytes(line, source, is_hit));
                out.extend_from_slice(b"}\n");
            }
            BulkAction::Index | BulkAction::Create => {
                out.extend_from_slice(source_bytes(line, source, is_hit));
                out.push(b'\n');
            }
        }
        Ok(())
    }

    fn bulk_segments(&self) -> Vec<String> {
        match &self.opts.index {
            Some(i) => vec![
                format!("{}{}{}", self.opts.prefix, i, self.opts.suffix),
                "_bulk".to_string(),
            ],
            None => vec!["_bulk".to_string()],
        }
    }

    fn compress(&self, body: Vec<u8>) -> Result<Body> {
        if !self.client.compress_requests() {
            return Ok(Body::Ndjson(Bytes::from(body)));
        }
        let mut enc = flate2::write::GzEncoder::new(
            Vec::with_capacity(body.len() / 4),
            flate2::Compression::fast(),
        );
        enc.write_all(&body)?;
        Ok(Body::NdjsonGzip(Bytes::from(enc.finish()?)))
    }

    async fn send_bulk(&self, body: Vec<u8>) -> Result<Bytes> {
        let segments = self.bulk_segments();
        let seg_refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        let seg_refs = &seg_refs;
        let body = self.compress(body)?;
        let body = &body;
        let client = &self.client;
        send_with_retry(
            self.opts.retry,
            &self.cancel,
            RetryMode::Idempotent,
            "bulk",
            move || client.send(Method::POST, seg_refs, &[], true, body.clone()),
        )
        .await
    }

    /// Write one batch of document lines; returns once every item was
    /// written, retried or reported.
    async fn process_batch(&mut self, buf: &[u8]) -> Result<()> {
        let lines: Vec<&[u8]> = buf
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        let mut start = 0;
        while start < lines.len() {
            // Split oversized batches at --maxBulkBytes.
            let mut body = Vec::with_capacity(buf.len() + lines.len() * 64);
            let mut end = start;
            while end < lines.len() {
                let before = body.len();
                self.encode_doc(lines[end], &mut body)?;
                end += 1;
                if body.len() >= self.opts.max_bulk_bytes && end - start >= 1 {
                    if body.len() > self.opts.max_bulk_bytes && end - start > 1 {
                        body.truncate(before);
                        end -= 1;
                    }
                    break;
                }
            }
            let chunk = &lines[start..end];
            match self.write_chunk(chunk, body).await {
                Ok(()) => {}
                Err(e) if self.opts.ignore_errors && !crate::http::is_cancelled(&e) => {
                    log::error!(
                        "Bulk request failed; skipping {} documents (--ignore-errors): {e:#}",
                        chunk.len()
                    );
                    self.report.failed += chunk.len() as u64;
                }
                Err(e) => return Err(e),
            }
            start = end;
        }
        if self.opts.refresh {
            // Skip when another worker's refresh is already running.
            let lock = Arc::clone(&self.refresh_lock);
            if let Ok(_g) = lock.try_lock() {
                self.refresh().await;
            }
        }
        Ok(())
    }

    async fn write_chunk(&mut self, chunk: &[&[u8]], body: Vec<u8>) -> Result<()> {
        let mut pending: Vec<usize> = (0..chunk.len()).collect();
        let mut body = body;
        let mut attempt = 0usize;
        loop {
            let resp = self.send_bulk(body).await?;
            let rejected = self.handle_response(&resp, chunk, &pending)?;
            if rejected.is_empty() {
                return Ok(());
            }
            if attempt >= self.opts.retry.attempts {
                for &i in &rejected {
                    self.report_failure(chunk[i], "rejected (429/503) after retries")?;
                }
                return Ok(());
            }
            let delay = self
                .opts
                .retry
                .delay_for(attempt)
                .max(Duration::from_millis(1));
            log::warn!(
                "{} bulk item(s) rejected with 429/503; retrying them in {:?}",
                rejected.len(),
                delay
            );
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = self.cancel.cancelled() => return Err(anyhow::Error::new(crate::http::DumpCancelled)),
            }
            attempt += 1;
            body = Vec::new();
            for &i in &rejected {
                self.encode_doc(chunk[i], &mut body)?;
            }
            pending = rejected;
        }
    }

    fn report_failure(&mut self, doc: &[u8], item: &str) -> Result<()> {
        self.report.failed += 1;
        if self.opts.ignore_es_write_errors {
            eprintln!("{item}");
            Ok(())
        } else {
            let id = get(doc, &["_id"]).map(|v| v.as_raw_str().to_string());
            Err(anyhow!(
                "bulk write failed for document {}: {item}",
                id.unwrap_or_else(|| "<auto id>".into())
            ))
        }
    }

    /// Account the response for `pending` (indices into `chunk`); returns the
    /// item indices rejected with 429/503 for retry.
    fn handle_response(
        &mut self,
        resp: &[u8],
        chunk: &[&[u8]],
        pending: &[usize],
    ) -> Result<Vec<usize>> {
        let errors = get(resp, &["errors"]).and_then(|v| v.as_bool());
        if errors == Some(false) {
            self.report.written += pending.len() as u64;
            return Ok(vec![]);
        }
        let items = get(resp, &["items"]).context("bulk response has no items")?;
        let items: Vec<LazyValue> = sonic_rs::to_array_iter(items.as_raw_str().as_bytes())
            .collect::<std::result::Result<_, _>>()
            .context("bulk response items are not an array")?;
        if items.len() != pending.len() {
            bail!(
                "bulk response has {} items for {} documents",
                items.len(),
                pending.len()
            );
        }
        let mut rejected = Vec::new();
        for (item, &doc_idx) in items.iter().zip(pending) {
            let raw = item.as_raw_str().as_bytes();
            let action = self.opts.action.as_str();
            let status = get(raw, &[action, "status"])
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if (200..400).contains(&status) {
                self.report.written += 1;
            } else if (status == 429 || status == 503) && self.opts.retry.attempts > 0 {
                rejected.push(doc_idx);
            } else {
                self.report_failure(chunk[doc_idx], item.as_raw_str())?;
            }
        }
        Ok(rejected)
    }

    async fn refresh(&self) {
        let segments = match &self.opts.index {
            Some(i) => vec![
                format!("{}{}{}", self.opts.prefix, i, self.opts.suffix),
                "_refresh".into(),
            ],
            None => vec!["_refresh".to_string()],
        };
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        match self
            .client
            .send(Method::POST, &refs, &[], false, Body::Empty)
            .await
        {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => log::warn!("refresh failed: HTTP {}", r.status()),
            Err(e) => log::warn!("refresh failed: {e}"),
        }
    }
}

fn source_bytes<'a>(line: &'a [u8], source: Option<&'a LazyValue<'_>>, is_hit: bool) -> &'a [u8] {
    match source {
        Some(s) => s.as_raw_str().as_bytes(),
        // a hit without _source (e.g. stored_fields only): empty document
        None if is_hit => b"{}",
        // a plain document (e.g. a --sourceOnly dump): the whole line
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{ClientOptions, Flavor, gate::RequestGate};

    fn sink(info: ClusterInfo, f: impl FnOnce(&mut BulkOptions)) -> BulkSink {
        let client = EsClient::new(
            &ClientOptions::new(url::Url::parse("http://localhost:9200/").unwrap()),
            RequestGate::default(),
        )
        .unwrap();
        let mut o = BulkOptions {
            info,
            index: None,
            doc_type: None,
            prefix: String::new(),
            suffix: String::new(),
            action: BulkAction::Index,
            handle_version: false,
            version_type: None,
            meta_fields: vec![
                "routing".into(),
                "parent".into(),
                "timestamp".into(),
                "ttl".into(),
            ],
            refresh: true,
            ignore_es_write_errors: true,
            ignore_errors: false,
            max_bulk_bytes: 100 << 20,
            retry: RetryPolicy::none(),
        };
        f(&mut o);
        BulkSink::new(client, o, CancellationToken::new())
    }

    fn enc(s: &mut BulkSink, line: &str) -> String {
        let mut out = vec![];
        s.encode_doc(line.as_bytes(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn os() -> ClusterInfo {
        ClusterInfo::new(Flavor::OpenSearch, "2.19.0")
    }

    #[test]
    fn index_action_keeps_raw_source_and_routing() {
        let mut s = sink(os(), |_| {});
        assert_eq!(
            enc(
                &mut s,
                r#"{"_index":"src","_type":"_doc","_id":"a\"b","_routing":"r1","_source":{"n":123456789012345678901234567890}}"#
            ),
            "{\"index\":{\"_index\":\"src\",\"_id\":\"a\\\"b\",\"routing\":\"r1\"}}\n{\"n\":123456789012345678901234567890}\n"
        );
    }

    #[test]
    fn output_index_prefix_suffix_and_types_for_old_es() {
        let mut s = sink(ClusterInfo::new(Flavor::Elastic, "6.8.0"), |o| {
            o.index = Some("dst".into());
            o.prefix = "p_".into();
            o.suffix = "_s".into();
        });
        assert_eq!(
            enc(
                &mut s,
                r#"{"_index":"src","_type":"doc","_id":"1","_source":{}}"#
            ),
            "{\"index\":{\"_index\":\"p_dst_s\",\"_id\":\"1\",\"_type\":\"doc\"}}\n{}\n"
        );
    }

    #[test]
    fn update_delete_create_and_versions() {
        let mut s = sink(os(), |o| {
            o.action = BulkAction::Update;
        });
        assert_eq!(
            enc(&mut s, r#"{"_index":"i","_id":"1","_source":{"a":1}}"#),
            "{\"update\":{\"_index\":\"i\",\"_id\":\"1\"}}\n{\"doc\":{\"a\":1}}\n"
        );
        let mut s = sink(os(), |o| o.action = BulkAction::Delete);
        assert_eq!(
            enc(&mut s, r#"{"_index":"i","_id":"1","_source":{"a":1}}"#),
            "{\"delete\":{\"_index\":\"i\",\"_id\":\"1\"}}\n"
        );
        let mut s = sink(os(), |o| {
            o.action = BulkAction::Create;
            o.handle_version = true;
            o.version_type = Some("external".into());
        });
        assert_eq!(
            enc(
                &mut s,
                r#"{"_index":"i","_id":"1","_version":7,"_source":{}}"#
            ),
            "{\"create\":{\"_index\":\"i\",\"_id\":\"1\",\"version\":7,\"version_type\":\"external\"}}\n{}\n"
        );
    }

    #[test]
    fn plain_documents_need_an_output_index() {
        let mut s = sink(os(), |_| {});
        let mut out = vec![];
        assert!(s.encode_doc(br#"{"a":1}"#, &mut out).is_err());
        let mut s = sink(os(), |o| o.index = Some("t".into()));
        assert_eq!(
            enc(&mut s, r#"{"a":1}"#),
            "{\"index\":{\"_index\":\"t\"}}\n{\"a\":1}\n"
        );
    }

    #[test]
    fn extra_fields_and_ttl_drop() {
        let mut s = sink(os(), |o| o.meta_fields.push("pipeline".into()));
        assert_eq!(
            enc(
                &mut s,
                r#"{"_index":"i","_id":"1","_ttl":5,"pipeline":"p","_source":{}}"#
            ),
            "{\"index\":{\"_index\":\"i\",\"_id\":\"1\",\"pipeline\":\"p\"}}\n{}\n"
        );
    }

    #[test]
    fn response_accounting() {
        let mut s = sink(os(), |o| o.ignore_es_write_errors = true);
        let docs: Vec<&[u8]> = vec![b"{}", b"{}", b"{}"];
        let r = s
            .handle_response(br#"{"errors":false,"items":[{},{},{}]}"#, &docs, &[0, 1, 2])
            .unwrap();
        assert!(r.is_empty());
        assert_eq!(s.report.written, 3);
        let r = s
            .handle_response(
                br#"{"errors":true,"items":[{"index":{"status":201}},{"index":{"status":400,"error":{"type":"mapper_parsing_exception"}}},{"index":{"status":429}}]}"#,
                &docs,
                &[0, 1, 2],
            )
            .unwrap();
        assert!(r.is_empty(), "no retries configured");
        assert_eq!(
            s.report,
            WriteReport {
                written: 4,
                failed: 2
            }
        );

        let mut s = sink(os(), |o| {
            o.ignore_es_write_errors = false;
        });
        assert!(
            s.handle_response(
                br#"{"errors":true,"items":[{"index":{"status":400}}]}"#,
                &docs[..1],
                &[0]
            )
            .is_err()
        );

        let mut s = sink(os(), |o| o.retry.attempts = 2);
        let r = s
            .handle_response(
                br#"{"errors":true,"items":[{"index":{"status":201}},{"index":{"status":429}},{"index":{"status":503}}]}"#,
                &docs,
                &[0, 1, 2],
            )
            .unwrap();
        assert_eq!(r, vec![1, 2]);
    }
}
