//! Index-scoped metadata: `mapping`, `settings`, `index` and `analyzer`.
//!
//! * mapping: one line holding the `GET /_mapping` response object.
//! * settings / analyzer: one line holding a JSON *string literal* of the
//!   `GET /_settings` response with cluster-generated keys removed.
//! * index: the same for `GET /{index}` (`aliases`, `mappings`, `settings`).

use anyhow::{Result, bail};
use reqwest::Method;
use serde_json::{Map, Value, json};

use super::http::{exists, get_json, is_already_exists, send_json};
use super::{MetaKind, MetaOptions, MetaTarget};
use crate::http::{ClusterInfo, EsClient};
use crate::pipeline::WriteReport;

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

pub(crate) const MAPPING_PARAMS: &[&str] = &[
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
pub(crate) fn index_settings_mut(def: &mut Value) -> Option<&mut Map<String, Value>> {
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

/// Keys in `def` that belong to the other engine, without changing `def`.
pub(crate) fn foreign_settings(def: &Value, target: &ClusterInfo) -> Vec<String> {
    strip_for_flavor(&mut def.clone(), target)
}

pub(crate) fn strip_for_flavor(def: &mut Value, target: &ClusterInfo) -> Vec<String> {
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
            // a type wrapper holds only mapping parameters (`properties`,
            // `_source`, ...), e.g. `{"type1":{"_source":{"enabled":false}}}`
            let only_params = inner.as_object().is_some_and(|o| {
                !o.is_empty() && o.keys().all(|k| MAPPING_PARAMS.contains(&k.as_str()))
            });
            if only_params {
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

/// Fetch the document for an index-scoped kind (`index` = endpoint index,
/// `*` / all indices when host-only).
pub async fn fetch(client: &EsClient, index: Option<&str>, opts: &MetaOptions) -> Result<Value> {
    let mut segs: Vec<&str> = Vec::new();
    match opts.kind {
        MetaKind::Index => segs.push(index.unwrap_or("*")),
        MetaKind::Mapping => {
            segs.extend(index);
            segs.push("_mapping");
        }
        _ => {
            segs.extend(index);
            segs.push("_settings");
        }
    }
    let what = format!(
        "GET {} for --type {}",
        segs.join("/"),
        opts.kind.type_name()
    );
    let mut doc = get_json(client, opts.retry, &segs, &what).await?;
    skip_data_stream_backing(&mut doc, opts.kind.type_name());
    Ok(doc)
}

/// TST-API-032: data stream backing indices (`.ds-*`, or a `data_stream` field in the GET
/// index response) are not restorable as plain indices; drop them with a warning naming the
/// data stream.
pub(crate) fn skip_data_stream_backing(doc: &mut Value, type_name: &str) {
    let Value::Object(obj) = doc else { return };
    let backing: Vec<(String, Option<String>)> = obj
        .iter()
        .filter_map(|(name, def)| {
            let ds = def
                .get("data_stream")
                .and_then(Value::as_str)
                .map(str::to_string);
            (name.starts_with(".ds-") || ds.is_some()).then(|| (name.clone(), ds))
        })
        .collect();
    for (name, ds) in backing {
        obj.remove(&name);
        log::warn!(
            "--type {type_name}: skipping data stream backing index {name}{}; dump the data stream's \
             documents and restore them with --bulkAction create into a data stream",
            ds.map(|d| format!(" (data stream {d})"))
                .unwrap_or_default()
        );
    }
}

/// Remove generated settings from every index of a dump.
pub fn strip_generated(doc: &mut Value, kind: MetaKind) {
    if kind == MetaKind::Mapping {
        return;
    }
    if let Value::Object(obj) = doc {
        for (_, def) in obj.iter_mut() {
            if def.get("settings").is_some() {
                strip_settings(def, &[]);
            }
        }
    }
}

/// Apply an index-scoped document to a cluster, one index at a time.
pub async fn write(
    doc: Value,
    client: &EsClient,
    info: &ClusterInfo,
    dst: &MetaTarget,
    opts: &MetaOptions,
) -> Result<WriteReport> {
    let Value::Object(indices) = doc else {
        bail!(
            "--type {} document must be a JSON object keyed by index",
            opts.kind.type_name()
        )
    };
    if dst.index.is_some() && indices.len() > 1 {
        bail!(
            "the {} document holds {} indices but a single output index was given; use a host-only --output to restore each under its own name",
            opts.kind.type_name(),
            indices.len()
        );
    }
    if opts.close_index && opts.kind == MetaKind::Analyzer {
        log::warn!(
            "--closeIndex: existing target indices are closed (OFFLINE for reads and writes) while their analysis settings change"
        );
    }
    let mut report = WriteReport::default();
    let mut indices = indices;
    refuse_unportable_fields(&mut indices, info, dst, opts, &mut report)?;
    for (src_index, mut def) in indices {
        let target = dst.target_index(&src_index)?;
        let result = match opts.kind {
            MetaKind::Settings => apply_settings(client, info, &target, &mut def, opts).await,
            MetaKind::Mapping => apply_mapping(client, info, &target, &def, dst, opts).await,
            MetaKind::Index => apply_index(client, info, &target, def, dst, opts).await,
            MetaKind::Analyzer => apply_analyzer(client, info, &target, def, opts).await,
            _ => unreachable!("not an index-scoped kind"),
        };
        match result {
            Ok(true) => report.written += 1,
            Ok(false) => {}
            Err(e) if opts.ignore_errors => {
                log::error!(
                    "{} for '{target}' failed (--ignore-errors): {e:#}",
                    opts.kind.type_name()
                );
                report.failed += 1;
            }
            Err(e) => {
                return Err(e.context(format!("restoring {} of '{target}'", opts.kind.type_name())));
            }
        }
    }
    Ok(report)
}

/// L-655 (BRK-XV-004): before anything is written, refuse every index whose mapping has a
/// field type the target has no type for, naming each field (see [`super::field_types`]).
/// With `--ignore-errors` those indices are skipped as failures and the rest is restored.
fn refuse_unportable_fields(
    indices: &mut Map<String, Value>,
    info: &ClusterInfo,
    dst: &MetaTarget,
    opts: &MetaOptions,
    report: &mut WriteReport,
) -> Result<()> {
    if !matches!(opts.kind, MetaKind::Mapping | MetaKind::Index) {
        return Ok(());
    }
    let mut refused = Vec::new();
    for (src_index, def) in indices.iter() {
        let mappings = match (opts.kind, def.get("mappings")) {
            (_, Some(m)) => m,
            (MetaKind::Mapping, None) => def,
            _ => continue,
        };
        let found = super::field_types::unportable(mappings, info);
        if !found.is_empty() {
            let target = dst.target_index(src_index)?;
            refused.push((
                src_index.clone(),
                super::field_types::refusal_message(&target, &found, info),
            ));
        }
    }
    if refused.is_empty() {
        return Ok(());
    }
    if !opts.ignore_errors {
        let all: Vec<&str> = refused.iter().map(|(_, m)| m.as_str()).collect();
        bail!("--type {}: {}", opts.kind.type_name(), all.join("; "));
    }
    for (src_index, msg) in refused {
        log::error!("--type {} (--ignore-errors): {msg}", opts.kind.type_name());
        indices.shift_remove(&src_index);
        report.failed += 1;
    }
    Ok(())
}

/// Strip generated, excluded and (for known sources) foreign-engine keys from
/// `def` (`{"settings":{..}}`); warn about foreign keys from files.
fn prepare_settings(def: &mut Value, info: &ClusterInfo, target: &str, opts: &MetaOptions) {
    if def.get("settings").is_none() {
        *def = json!({ "settings": def.clone() });
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
    } else {
        // File input: the source engine is unknown, so keep the settings as
        // written but flag keys that only the other engine understands. OpenSearch
        // silently stores Elasticsearch-only keys such as `_tier_preference`.
        let foreign = foreign_settings(def, info);
        if !foreign.is_empty() {
            log::warn!(
                "Settings for {target} contain {}-only keys that {} does not use: {}; drop them with --excludeSettings={}",
                other_engine(info),
                info.flavor_name(),
                foreign.join(","),
                foreign.join(",")
            );
        }
    }
}

/// The key of a server's `unknown setting [key]` error.
fn unknown_setting(msg: &str) -> Option<&str> {
    let rest = &msg[msg.find("unknown setting [")? + "unknown setting [".len()..];
    let key = &rest[..rest.find(']')?];
    (!key.is_empty() && !key.contains(char::is_whitespace)).then_some(key)
}

pub(crate) fn other_engine(info: &ClusterInfo) -> &'static str {
    if info.is_opensearch() {
        "Elasticsearch"
    } else {
        "OpenSearch"
    }
}

fn create_error(e: anyhow::Error, opts: &MetaOptions, what: &str) -> Result<bool> {
    if opts.source_info.is_none() {
        // name the key when the server did (`unknown setting [index.replication.type]`, e.g. an
        // OpenSearch 2 dump restored to OpenSearch 1.3), so the hint can be pasted as is
        let msg = format!("{e:#}");
        let hint = match unknown_setting(&msg) {
            Some(key) => format!(
                "if this setting is engine- or version-specific, drop it with --excludeSettings={key} ({what})"
            ),
            None => format!(
                "if this setting is engine-specific (Elasticsearch vs OpenSearch), drop it with --excludeSettings ({what})"
            ),
        };
        Err(e.context(hint))
    } else {
        Err(e)
    }
}

async fn apply_settings(
    client: &EsClient,
    info: &ClusterInfo,
    target: &str,
    def: &mut Value,
    opts: &MetaOptions,
) -> Result<bool> {
    prepare_settings(def, info, target, opts);
    let body = json!({ "settings": def["settings"].clone() });
    match send_json(client, opts.retry, Method::PUT, &[target], &body, "create index").await {
        Ok(_) => Ok(true),
        Err(e) if is_already_exists(&e) && opts.skip_existing => {
            log::warn!("Index '{target}' already exists; skipping its settings (--skip-existing)");
            Ok(false)
        }
        Err(e) if is_already_exists(&e) => Err(e.context(
            "settings can only be applied when creating an index (use --skip-existing to skip existing indices)",
        )),
        Err(e) => create_error(e, opts, "settings"),
    }
}

/// `--type index`: create the index with its settings, mappings and aliases
/// in one request.
async fn apply_index(
    client: &EsClient,
    info: &ClusterInfo,
    target: &str,
    mut def: Value,
    dst: &MetaTarget,
    opts: &MetaOptions,
) -> Result<bool> {
    let Value::Object(obj) = &mut def else {
        bail!("index definition for '{target}' is not an object");
    };
    if obj.shift_remove("data_stream").is_some() {
        log::warn!(
            "'{target}' was a data stream backing index; restoring it as a plain index (data_stream dropped)"
        );
    }
    let mut body = Map::new();
    if obj.contains_key("settings") {
        let mut s = json!({ "settings": obj["settings"].clone() });
        prepare_settings(&mut s, info, target, opts);
        body.insert("settings".into(), s["settings"].clone());
    }
    if let Some(m) = obj.get("mappings") {
        let m = if info.has_types() {
            m.clone()
        } else {
            typeless_mapping(m)
        };
        body.insert("mappings".into(), m);
    }
    if let Some(a) = obj.get("aliases") {
        let a = match a {
            Value::Object(m) if opts.rename_aliases => {
                let mut out = Map::new();
                for (k, v) in m {
                    out.insert(dst.renamer.alias(k)?, v.clone());
                }
                Value::Object(out)
            }
            other => other.clone(),
        };
        body.insert("aliases".into(), a);
    }
    let body = Value::Object(body);
    match send_json(
        client,
        opts.retry,
        Method::PUT,
        &[target],
        &body,
        "create index",
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(e) if is_already_exists(&e) && opts.skip_existing => {
            log::warn!("Index '{target}' already exists; skipping it (--skip-existing)");
            Ok(false)
        }
        Err(e) if is_already_exists(&e) => {
            Err(e
                .context("the index already exists (use --skip-existing to skip existing indices)"))
        }
        Err(e) => create_error(e, opts, "index"),
    }
}

/// `--type analyzer`: create the target with the `analysis` settings, or,
/// for an existing index, skip it (`--skip-existing`) or close it, update
/// the analysis and reopen it (`--closeIndex`).
async fn apply_analyzer(
    client: &EsClient,
    _info: &ClusterInfo,
    target: &str,
    mut def: Value,
    opts: &MetaOptions,
) -> Result<bool> {
    if def.get("settings").is_none() {
        def = json!({ "settings": def });
    }
    let analysis = index_settings_mut(&mut def).and_then(|i| i.get("analysis").cloned());
    let Some(analysis) = analysis else {
        log::warn!("No analysis settings for '{target}'; skipping it");
        return Ok(false);
    };
    if !exists(client, opts.retry, &[target]).await? {
        let body = json!({"settings": {"index": {"analysis": analysis}}});
        send_json(
            client,
            opts.retry,
            Method::PUT,
            &[target],
            &body,
            "create index",
        )
        .await?;
        return Ok(true);
    }
    if opts.skip_existing {
        log::warn!("Index '{target}' already exists; skipping its analyzers (--skip-existing)");
        return Ok(false);
    }
    if !opts.close_index {
        bail!(
            "index '{target}' already exists; analysis settings can only change on a closed index. Use --closeIndex to close, update and reopen it (takes the index offline), or --skip-existing to leave it alone"
        );
    }
    close_put_open(
        client,
        target,
        &json!({"index": {"analysis": analysis}}),
        opts,
    )
    .await?;
    Ok(true)
}

/// Close `target`, PUT `_settings`, and always try to reopen it, also when
/// the update fails or the user interrupts (SIGINT or SIGTERM: the update is
/// abandoned, the index reopened, and the run exits 130 / 143). If a second
/// signal force-quits before the reopen finished, the forced exit names the
/// index that may still be closed (TST-BRK-SIG-004).
pub(crate) async fn close_put_open(
    client: &EsClient,
    target: &str,
    settings: &Value,
    opts: &MetaOptions,
) -> Result<()> {
    // Only ever close ONE concrete index: never `*`, `_all` or `a,b`
    // (INJ-F2, TST-INJ-016).
    crate::rename::validate_index_name(target)
        .map_err(|e| anyhow::anyhow!("refusing to close '{target}': {e}"))?;
    let _graceful = crate::signals::graceful();
    let interrupt = crate::signals::token();
    if interrupt.is_cancelled() {
        return Err(
            crate::signals::interrupted_error().context(format!("index '{target}' was not closed"))
        );
    }
    let closed_note = crate::signals::note_on_force_quit(format!(
        "index '{target}' may still be CLOSED (offline); reopen it with POST /{target}/_open"
    ));
    send_json(
        client,
        opts.retry,
        Method::POST,
        &[target, "_close"],
        &Value::Null,
        "close index",
    )
    .await?;
    log::warn!("Index '{target}' is CLOSED (offline) while its analysis settings are updated");
    let segs = [target, "_settings"];
    let put = tokio::select! {
        r = send_json(client, opts.retry, Method::PUT, &segs, settings, "update settings") => r.map(|_| ()),
        _ = interrupt.cancelled() => Err(crate::signals::interrupted_error()),
    };
    let open = send_json(
        client,
        opts.retry,
        Method::POST,
        &[target, "_open"],
        &Value::Null,
        "open index",
    )
    .await;
    match &open {
        Ok(_) => {
            drop(closed_note);
            log::info!("Index '{target}' reopened");
            if !interrupt.is_cancelled() {
                let _ = super::http::get_json_q(
                    client,
                    opts.retry,
                    &["_cluster", "health", target],
                    &[("wait_for_status", "yellow"), ("timeout", "60s")],
                    "cluster health",
                )
                .await;
            }
        }
        Err(e) => log::error!("Index '{target}' could NOT be reopened; it is still closed: {e:#}"),
    }
    let interrupted = |p: &anyhow::Error| p.downcast_ref::<crate::signals::Interrupted>().is_some();
    match (put, open) {
        (Err(p), Ok(_)) if interrupted(&p) => Err(p.context(format!(
            "interrupted while index '{target}' was closed: its analysis settings were not updated and the index was reopened"
        ))),
        (Ok(()), Ok(_)) if interrupt.is_cancelled() => Err(crate::signals::interrupted_error()
            .context(format!(
                "interrupted after index '{target}' was updated and reopened"
            ))),
        (Ok(()), Ok(_)) => Ok(()),
        (Err(p), Ok(_)) => Err(p.context(format!(
            "updating '{target}' failed (the index was reopened)"
        ))),
        (Ok(()), Err(o)) => Err(o.context(format!("index '{target}' is still closed"))),
        (Err(p), Err(o)) => Err(anyhow::anyhow!(
            "updating '{target}' failed: {p:#}; reopening failed too, the index is still closed: {o:#}"
        )),
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
        &json!({}),
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
    fn data_stream_backing_indices_are_skipped() {
        let mut doc = serde_json::json!({
            "a": {"settings": {}},
            ".ds-logs-2026.10.30-000001": {"settings": {}},
            "renamed_backing": {"data_stream": "logs", "settings": {}}
        });
        super::skip_data_stream_backing(&mut doc, "index");
        assert_eq!(doc.as_object().unwrap().keys().collect::<Vec<_>>(), ["a"]);
    }

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
        let before = def.clone();
        assert_eq!(
            foreign_settings(&def, &os),
            vec!["index.routing.allocation.include._tier_preference"]
        );
        assert_eq!(def, before);
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
        // elasticdump's template_6x fixture: a type holding only `_source`
        let six = json!({"type1": {"_source": {"enabled": false}}});
        assert_eq!(
            typeless_mapping(&six),
            json!({"_source": {"enabled": false}})
        );
    }

    #[test]
    fn big_numbers_survive() {
        let v: Value = serde_json::from_str(r#"{"a":123456789012345678901234567890}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"a":123456789012345678901234567890}"#
        );
    }

    #[test]
    fn unknown_setting_key_is_named_in_the_hint() {
        let m = "HTTP 400: illegal_argument_exception: unknown setting [index.replication.type] please check that any required plugins are installed";
        assert_eq!(super::unknown_setting(m), Some("index.replication.type"));
        assert_eq!(super::unknown_setting("unknown setting [] x"), None);
        assert_eq!(super::unknown_setting("other error"), None);
    }
}
