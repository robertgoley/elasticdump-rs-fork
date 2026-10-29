//! `--type mapping` and `--type settings`: dump to a file and restore to a
//! cluster, with elasticdump-compatible file formats.
//!
//! * mapping: one line holding the `GET /_mapping` response object.
//! * settings: one line holding a JSON *string literal* of the `GET
//!   /_settings` response (elasticdump's `JSON.stringify` format), with
//!   cluster-generated keys removed. Plain objects are accepted on restore.

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::Method;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::http::{
    Body, ClusterInfo, EsClient, EsHttpError, RetryMode, RetryPolicy, send_with_retry,
};
use crate::pipeline::WriteReport;
use crate::pipeline::file_sink::{FileOptions, FileSink};
use crate::pipeline::json_split::ValueSplitter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaKind {
    Mapping,
    Settings,
}

impl MetaKind {
    fn api(self) -> &'static str {
        match self {
            MetaKind::Mapping => "_mapping",
            MetaKind::Settings => "_settings",
        }
    }
}

/// Paths (below `settings.index`) that a cluster generates and that cannot
/// be sent back on index creation (elasticdump's `settingsExclude`).
pub const SETTINGS_EXCLUDE: &[&str] = &[
    "version",
    "creation_date",
    "uuid",
    "provided_name",
    "resize",
    "routing.allocation.initial_recovery",
];

/// Additional keys rejected at index-creation time.
const SETTINGS_NOT_CREATABLE: &[&str] =
    &["history.uuid", "verified_before_close", "store.snapshot"];

/// Engine-specific settings removed when copying between flavors.
const ELASTIC_ONLY: &[&str] = &["lifecycle", "routing.allocation.include._tier_preference"];
const OPENSEARCH_ONLY: &[&str] = &["replication.type", "plugins", "opendistro"];

const MAPPING_PARAMS: &[&str] = &[
    "properties",
    "dynamic",
    "_source",
    "_routing",
    "_meta",
    "dynamic_templates",
    "date_detection",
    "numeric_detection",
    "runtime",
    "enabled",
    "_field_names",
    "_all",
    "dynamic_date_formats",
    "_size",
];

/// Remove `path` (dotted, relative to the `index` settings object) in both
/// nested and flat (`index.a.b`) forms. Returns whether anything was removed.
fn remove_path(index_settings: &mut Map<String, Value>, path: &str) -> bool {
    let mut removed = index_settings.shift_remove(path).is_some();
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = &mut *index_settings;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            removed |= cur.shift_remove(*part).is_some();
            break;
        }
        match cur.get_mut(*part) {
            Some(Value::Object(next)) => cur = next,
            _ => break,
        }
    }
    removed
}

/// The `index` settings object of one index definition (`{"settings":{"index":{..}}}`),
/// normalising flat `index.*` keys into it.
fn index_settings_mut(def: &mut Value) -> Option<&mut Map<String, Value>> {
    let settings = def.get_mut("settings")?.as_object_mut()?;
    let flat: Vec<String> = settings
        .keys()
        .filter(|k| k.starts_with("index."))
        .cloned()
        .collect();
    let mut moved = Vec::new();
    for k in flat {
        if let Some(v) = settings.shift_remove(&k) {
            moved.push((k["index.".len()..].to_string(), v));
        }
    }
    let index = settings
        .entry("index")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;
    for (k, v) in moved {
        index.insert(k, v);
    }
    Some(index)
}

pub fn strip_settings(def: &mut Value, extra: &[&str]) {
    if let Some(index) = index_settings_mut(def) {
        for p in SETTINGS_EXCLUDE.iter().chain(extra) {
            remove_path(index, p);
        }
    }
}

fn strip_for_flavor(def: &mut Value, target: &ClusterInfo) -> Vec<String> {
    let list = if target.is_opensearch() {
        ELASTIC_ONLY
    } else {
        OPENSEARCH_ONLY
    };
    let mut dropped = Vec::new();
    if let Some(index) = index_settings_mut(def) {
        for p in list {
            if remove_path(index, p) {
                dropped.push(format!("index.{p}"));
            }
        }
    }
    dropped
}

/// Typeless mapping body for ES >= 7 / OpenSearch from one index's
/// `mappings` value (unwraps typed 6.x exports, drops `_all`).
pub fn typeless_mapping(mappings: &Value) -> Value {
    let mut m = mappings.clone();
    if let Value::Object(obj) = &m {
        let non_params: Vec<&String> = obj
            .keys()
            .filter(|k| !MAPPING_PARAMS.contains(&k.as_str()))
            .collect();
        if obj.len() == 1 && non_params.len() == 1 {
            let inner = &obj[non_params[0]];
            if inner.get("properties").is_some() || inner.get("dynamic").is_some() {
                log::info!("Unwrapping typed mapping '{}'", non_params[0]);
                m = inner.clone();
            }
        }
    }
    strip_all_fields(&mut m);
    m
}

fn strip_all_fields(v: &mut Value) {
    if let Value::Object(obj) = v {
        if obj.shift_remove("_all").is_some() {
            log::warn!("Dropping '_all' from mapping: not supported by the target cluster");
        }
        obj.shift_remove("include_in_all");
        for (_, child) in obj.iter_mut() {
            strip_all_fields(child);
        }
    }
}

/// Read the first JSON value of a file / stdin (gzip auto-detected).
fn read_first_value(path: Option<PathBuf>) -> Result<Value> {
    let mut reader = crate::pipeline::file_source::open_reader(path.as_ref())?;
    let mut splitter = ValueSplitter::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).context("Failed to read input")?;
        if n > 0 {
            splitter.push(&buf[..n]);
        }
        if let Some(v) = splitter.next_value(n == 0).map_err(|e| anyhow!(e))? {
            return serde_json::from_slice(&v).context("Invalid JSON in input");
        }
        if n == 0 {
            bail!("Input is empty");
        }
    }
}

/// A settings file line: a JSON string literal holding the object, or the
/// object itself.
fn decode_settings(v: Value) -> Result<Value> {
    match v {
        Value::String(s) => serde_json::from_str(&s).context("settings string is not JSON"),
        other => Ok(other),
    }
}

async fn get_json(
    client: &EsClient,
    retry: RetryPolicy,
    segments: &[&str],
    what: &str,
) -> Result<Value> {
    let cancel = CancellationToken::new();
    let bytes = send_with_retry(retry, &cancel, RetryMode::Idempotent, what, move || {
        client.send(Method::GET, segments, &[], true, Body::Empty)
    })
    .await?;
    serde_json::from_slice(&bytes).with_context(|| format!("{what} returned invalid JSON"))
}

async fn send_json(
    client: &EsClient,
    retry: RetryPolicy,
    method: Method,
    segments: &[&str],
    body: &Value,
    what: &str,
) -> Result<Value> {
    let cancel = CancellationToken::new();
    let body = Body::json(body);
    let body = &body;
    let method = &method;
    let bytes = send_with_retry(retry, &cancel, RetryMode::Idempotent, what, move || {
        client.send(method.clone(), segments, &[], true, body.clone())
    })
    .await?;
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if v.get("error").is_some_and(|e| !e.is_null()) {
        bail!("{what} failed: {v}");
    }
    Ok(v)
}

fn is_already_exists(e: &anyhow::Error) -> bool {
    e.downcast_ref::<EsHttpError>()
        .is_some_and(|h| h.error_type_is("resource_already_exists_exception"))
}

pub struct MetaSource {
    pub client: Option<EsClient>,
    pub index: Option<String>,
    pub file: Option<PathBuf>,
}

pub struct MetaTarget {
    pub client: Option<EsClient>,
    pub info: Option<ClusterInfo>,
    pub index: Option<String>,
    pub doc_type: Option<String>,
    pub prefix: String,
    pub suffix: String,
    pub file: FileOptions,
}

pub struct MetaOptions {
    pub kind: MetaKind,
    pub retry: RetryPolicy,
    pub skip_existing: bool,
    pub ignore_errors: bool,
    pub exclude_settings: Vec<String>,
    /// Flavor of the source cluster (cluster→cluster only): engine-specific
    /// settings are stripped when it differs from the target's.
    pub source_info: Option<ClusterInfo>,
}

/// Read the mapping/settings document from the source.
pub async fn read(src: &MetaSource, opts: &MetaOptions) -> Result<Value> {
    let mut v = match &src.client {
        Some(client) => {
            let mut segs: Vec<&str> = Vec::new();
            if let Some(i) = src.index.as_deref() {
                segs.push(i);
            }
            segs.push(opts.kind.api());
            get_json(
                client,
                opts.retry,
                &segs,
                &format!("GET {}", opts.kind.api()),
            )
            .await?
        }
        None => {
            let file = src.file.clone();
            let v = tokio::task::spawn_blocking(move || read_first_value(file)).await??;
            match opts.kind {
                MetaKind::Settings => decode_settings(v)?,
                MetaKind::Mapping => v,
            }
        }
    };
    let Value::Object(obj) = &mut v else {
        bail!(
            "{} document must be a JSON object keyed by index",
            opts.kind.api()
        );
    };
    if opts.kind == MetaKind::Settings {
        for (_, def) in obj.iter_mut() {
            strip_settings(def, &[]);
        }
    }
    Ok(v)
}

/// Write the document to a file target (one line) or apply it to a cluster.
pub async fn write(doc: Value, dst: &MetaTarget, opts: &MetaOptions) -> Result<WriteReport> {
    let Some(client) = &dst.client else {
        let line = match opts.kind {
            MetaKind::Mapping => serde_json::to_string(&doc)?,
            MetaKind::Settings => {
                serde_json::to_string(&Value::String(serde_json::to_string(&doc)?))?
            }
        };
        let mut opts = dst.file.clone();
        opts.split = None;
        opts.source_only = false;
        let mut sink = FileSink::create(opts).await?;
        sink.write_lines(format!("{line}\n").as_bytes()).await?;
        sink.finalize().await?;
        return Ok(WriteReport {
            written: 1,
            failed: 0,
        });
    };
    let info = dst.info.as_ref().expect("target cluster info");
    let Value::Object(indices) = doc else {
        unreachable!()
    };
    if dst.index.is_some() && indices.len() > 1 {
        bail!(
            "the {} document holds {} indices but a single output index was given; use a host-only --output to restore each under its own name",
            opts.kind.api(),
            indices.len()
        );
    }
    let mut report = WriteReport::default();
    for (src_index, mut def) in indices {
        let target = format!(
            "{}{}{}",
            dst.prefix,
            dst.index.clone().unwrap_or(src_index.clone()),
            dst.suffix
        );
        let result = match opts.kind {
            MetaKind::Settings => apply_settings(client, info, &target, &mut def, opts).await,
            MetaKind::Mapping => apply_mapping(client, info, &target, &def, dst, opts).await,
        };
        match result {
            Ok(true) => report.written += 1,
            Ok(false) => {}
            Err(e) if opts.ignore_errors => {
                log::error!(
                    "{} for '{target}' failed (--ignore-errors): {e:#}",
                    opts.kind.api()
                );
                report.failed += 1;
            }
            Err(e) => return Err(e.context(format!("restoring {} of '{target}'", opts.kind.api()))),
        }
    }
    Ok(report)
}

async fn apply_settings(
    client: &EsClient,
    info: &ClusterInfo,
    target: &str,
    def: &mut Value,
    opts: &MetaOptions,
) -> Result<bool> {
    if def.get("settings").is_none() {
        *def = serde_json::json!({ "settings": def.clone() });
    }
    let extra: Vec<&str> = SETTINGS_NOT_CREATABLE
        .iter()
        .copied()
        .chain(
            opts.exclude_settings
                .iter()
                .map(|s| s.trim_start_matches("index.")),
        )
        .collect();
    strip_settings(def, &extra);
    if let Some(src) = &opts.source_info {
        if src.flavor != info.flavor {
            let dropped = strip_for_flavor(def, info);
            if !dropped.is_empty() {
                log::warn!(
                    "Dropping {} settings not supported by {}: {}",
                    src.flavor_name(),
                    info.flavor_name(),
                    dropped.join(", ")
                );
            }
        }
    }
    let body = serde_json::json!({ "settings": def["settings"].clone() });
    match send_json(client, opts.retry, Method::PUT, &[target], &body, "create index").await {
        Ok(_) => Ok(true),
        Err(e) if is_already_exists(&e) && opts.skip_existing => {
            log::warn!("Index '{target}' already exists; skipping its settings (--skip-existing)");
            Ok(false)
        }
        Err(e) if is_already_exists(&e) => Err(e.context(
            "settings can only be applied when creating an index (use --skip-existing to skip existing indices)",
        )),
        Err(e) if opts.source_info.is_none() => Err(e.context(
            "if this setting is engine-specific (Elasticsearch vs OpenSearch), drop it with --excludeSettings",
        )),
        Err(e) => Err(e),
    }
}

async fn apply_mapping(
    client: &EsClient,
    info: &ClusterInfo,
    target: &str,
    def: &Value,
    dst: &MetaTarget,
    opts: &MetaOptions,
) -> Result<bool> {
    // Make sure the index exists.
    match send_json(
        client,
        opts.retry,
        Method::PUT,
        &[target],
        &serde_json::json!({}),
        "create index",
    )
    .await
    {
        Ok(_) => {}
        Err(e) if is_already_exists(&e) => {}
        Err(e) => return Err(e),
    }
    let mappings = def.get("mappings").unwrap_or(def);
    if info.has_types() {
        // Elasticsearch < 7: one PUT per type.
        let typed: Vec<(String, Value)> = match mappings {
            Value::Object(o)
                if o.keys().all(|k| !MAPPING_PARAMS.contains(&k.as_str())) && !o.is_empty() =>
            {
                o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
            other => vec![(
                dst.doc_type.clone().unwrap_or_else(|| "_doc".to_string()),
                other.clone(),
            )],
        };
        for (ty, body) in typed {
            send_json(
                client,
                opts.retry,
                Method::PUT,
                &[target, "_mapping", &ty],
                &body,
                "put mapping",
            )
            .await?;
        }
    } else {
        let body = typeless_mapping(mappings);
        send_json(
            client,
            opts.retry,
            Method::PUT,
            &[target, "_mapping"],
            &body,
            "put mapping",
        )
        .await?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_exclusions_nested_and_flat() {
        let mut def = json!({"settings": {
            "index.uuid": "x",
            "index": {
                "number_of_shards": "2",
                "creation_date": "1",
                "provided_name": "p",
                "version": {"created": "1"},
                "routing": {"allocation": {"initial_recovery": {"_id": "n"}, "include": {"_tier_preference": "data_hot"}}},
                "analysis": {"analyzer": {"a": {"type": "custom", "tokenizer": "whitespace"}}}
            }
        }});
        strip_settings(&mut def, &[]);
        assert_eq!(
            def,
            json!({"settings": {"index": {
                "number_of_shards": "2",
                "routing": {"allocation": {"include": {"_tier_preference": "data_hot"}}},
                "analysis": {"analyzer": {"a": {"type": "custom", "tokenizer": "whitespace"}}}
            }}})
        );
        let os = ClusterInfo::new(crate::http::Flavor::OpenSearch, "2.19.0");
        assert_eq!(
            strip_for_flavor(&mut def, &os),
            vec!["index.routing.allocation.include._tier_preference"]
        );
    }

    #[test]
    fn typed_mapping_is_unwrapped_and_all_dropped() {
        let typed = json!({"doc": {"_all": {"enabled": false}, "properties": {"a": {"type": "text", "include_in_all": false}}}});
        assert_eq!(
            typeless_mapping(&typed),
            json!({"properties": {"a": {"type": "text"}}})
        );
        // a field literally named "properties" is not mistaken for a type
        let tricky = json!({"properties": {"properties": {"type": "keyword"}}});
        assert_eq!(typeless_mapping(&tricky), tricky);
        let params = json!({"dynamic": "strict", "properties": {}});
        assert_eq!(typeless_mapping(&params), params);
    }

    #[test]
    fn settings_string_literal_round_trip() {
        let v = json!({"i": {"settings": {"index": {"number_of_shards": "1"}}}});
        let line =
            serde_json::to_string(&Value::String(serde_json::to_string(&v).unwrap())).unwrap();
        assert!(line.starts_with("\"{\\\"i\\\""));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(decode_settings(parsed).unwrap(), v);
        assert_eq!(decode_settings(v.clone()).unwrap(), v);
    }

    #[test]
    fn big_numbers_survive() {
        let v: Value = serde_json::from_str(r#"{"a":123456789012345678901234567890}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"a":123456789012345678901234567890}"#
        );
    }
}
