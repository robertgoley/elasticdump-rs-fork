//! `--type template` (legacy `_template`), `component_template` and
//! `index_template` (composable).

use anyhow::{Result, bail};
use reqwest::Method;
use serde_json::{Map, Value, json};

use super::http::{body_contains, get_json_opt, send_json_q};
use super::index_scoped::{
    foreign_settings, other_engine, strip_for_flavor, strip_settings, typeless_mapping,
};
use super::{MetaOptions, MetaTarget, TemplateKind};
use crate::http::{ClusterInfo, EsClient};
use crate::pipeline::WriteReport;

/// elasticdump's `--templateRegex` default.
pub const DEFAULT_TEMPLATE_REGEX: &str = r"^(metrics|logs|.+_audit_log|.+-index-template|\..+)$";

/// Read-only fields added by Elasticsearch 9.2+ to template GET responses.
const GENERATED_FIELDS: &[&str] = &[
    "created_date",
    "created_date_millis",
    "modified_date",
    "modified_date_millis",
];

/// Template body fields only one engine understands (dotted paths).
pub const ELASTIC_ONLY_FIELDS: &[&str] = &[
    "allow_auto_create",
    "ignore_missing_component_templates",
    "deprecated",
    "data_stream_options",
    "template.lifecycle",
    "template.data_stream_options",
    "data_stream.hidden",
    "data_stream.allow_custom_routing",
    "data_stream.failure_store",
];
pub const OPENSEARCH_ONLY_FIELDS: &[&str] = &["data_stream.timestamp_field", "context"];

fn list_key(kind: TemplateKind) -> Option<(&'static str, &'static str)> {
    match kind {
        TemplateKind::Legacy => None,
        TemplateKind::Component => Some(("component_templates", "component_template")),
        TemplateKind::Composable => Some(("index_templates", "index_template")),
    }
}

pub async fn fetch(
    client: &EsClient,
    kind: TemplateKind,
    name: Option<&str>,
    opts: &MetaOptions,
) -> Result<Value> {
    let name = name.unwrap_or("*");
    let got = get_json_opt(
        client,
        opts.retry,
        &[kind.api(), name],
        &[],
        &format!("GET {}/{name}", kind.api()),
    )
    .await?;
    let mut v = got.unwrap_or_else(|| match list_key(kind) {
        Some((k, _)) => json!({ k: [] }),
        None => json!({}),
    });
    if let Some(re) = &opts.template_filter {
        filter_names(&mut v, kind, |n| re.is_match(n), "matching --templateRegex");
    }
    Ok(v)
}

fn filter_names(v: &mut Value, kind: TemplateKind, drop: impl Fn(&str) -> bool, why: &str) {
    let mut dropped = Vec::new();
    match list_key(kind) {
        None => {
            if let Value::Object(m) = v {
                m.retain(|k, _| {
                    let d = drop(k);
                    if d {
                        dropped.push(k.clone());
                    }
                    !d
                });
            }
        }
        Some((key, _)) => {
            if let Some(list) = v.get_mut(key).and_then(Value::as_array_mut) {
                list.retain(|it| {
                    let n = it.get("name").and_then(Value::as_str).unwrap_or("");
                    let d = drop(n);
                    if d {
                        dropped.push(n.to_string());
                    }
                    !d
                });
            }
        }
    }
    if !dropped.is_empty() {
        log::info!(
            "Skipping {} {}s {why}: {}",
            dropped.len(),
            kind.type_name(),
            dropped.join(", ")
        );
    }
}

fn is_managed(body: &Value) -> bool {
    body.pointer("/_meta/managed").and_then(Value::as_bool) == Some(true)
}

/// `--filterManaged`: drop `_meta.managed: true` entries (Elasticsearch
/// built-ins).
pub fn drop_managed(v: &mut Value, kind: TemplateKind) {
    let mut managed = Vec::new();
    match list_key(kind) {
        None => {
            if let Value::Object(m) = v {
                m.retain(|k, b| {
                    let d = is_managed(b);
                    if d {
                        managed.push(k.clone());
                    }
                    !d
                });
            }
        }
        Some((key, item)) => {
            if let Some(list) = v.get_mut(key).and_then(Value::as_array_mut) {
                list.retain(|it| {
                    let d = it.get(item).is_some_and(is_managed);
                    if d {
                        managed.push(it["name"].as_str().unwrap_or("").to_string());
                    }
                    !d
                });
            } else if let Value::Object(m) = v {
                m.retain(|k, b| {
                    let d = is_managed(b);
                    if d {
                        managed.push(k.clone());
                    }
                    !d
                });
            }
        }
    }
    if !managed.is_empty() {
        log::info!(
            "Skipping {} managed {}s (--filterManaged; --filterManaged=false keeps them): {}",
            managed.len(),
            kind.type_name(),
            managed.join(", ")
        );
    }
}

/// `(name, body)` pairs from the elasticdump list shape or a `{name: body}`
/// map.
pub fn normalize(doc: &Value, kind: TemplateKind) -> Result<Vec<(String, Value)>> {
    let Value::Object(m) = doc else {
        bail!("{} document must be a JSON object", kind.type_name());
    };
    if let Some((key, item)) = list_key(kind) {
        if let Some(list) = m.get(key) {
            let Some(list) = list.as_array() else {
                bail!("'{key}' must be an array");
            };
            return list
                .iter()
                .map(|it| {
                    let name = it.get("name").and_then(Value::as_str);
                    let body = it.get(item);
                    match (name, body) {
                        (Some(n), Some(b)) => Ok((n.to_string(), b.clone())),
                        _ => bail!("{key} entry without name/{item}: {it}"),
                    }
                })
                .collect();
        }
    }
    Ok(m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// Remove a dotted path from an object; empty parents are kept.
pub fn remove_dotted(v: &mut Value, path: &str) -> bool {
    let mut cur = v;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, p) in parts.iter().enumerate() {
        let Value::Object(m) = cur else { return false };
        if i == parts.len() - 1 {
            return m.shift_remove(*p).is_some();
        }
        match m.get_mut(*p) {
            Some(next) => cur = next,
            None => return false,
        }
    }
    false
}

/// Where the settings / mappings / aliases of a template body live.
fn section(body: &mut Value, kind: TemplateKind) -> Option<&mut Map<String, Value>> {
    match kind {
        TemplateKind::Legacy => body.as_object_mut(),
        _ => body.get_mut("template").and_then(Value::as_object_mut),
    }
}

/// Prepare one template body for the target. Returns the cleaned body.
pub fn prepare(
    name: &str,
    mut body: Value,
    kind: TemplateKind,
    info: &ClusterInfo,
    dst: &MetaTarget,
    opts: &MetaOptions,
) -> Result<Value> {
    for f in GENERATED_FIELDS {
        remove_dotted(&mut body, f);
    }
    for f in &opts.exclude_template_fields {
        remove_dotted(&mut body, f);
    }
    let foreign_fields = if info.is_opensearch() {
        ELASTIC_ONLY_FIELDS
    } else {
        OPENSEARCH_ONLY_FIELDS
    };
    let cross = opts.source_info.as_ref().map(|s| s.flavor != info.flavor);
    let present: Vec<&str> = foreign_fields
        .iter()
        .copied()
        .filter(|f| remove_dotted(&mut body.clone(), f))
        .collect();
    if !present.is_empty() {
        match cross {
            Some(true) => {
                for f in &present {
                    remove_dotted(&mut body, f);
                }
                log::warn!(
                    "{} '{name}': dropping {}-only fields: {}",
                    kind.type_name(),
                    other_engine(info),
                    present.join(", ")
                );
            }
            Some(false) => {}
            None => log::warn!(
                "{} '{name}' has {}-only fields that {} may reject: {}; drop them with --excludeTemplateFields={}",
                kind.type_name(),
                other_engine(info),
                info.flavor_name(),
                present.join(", "),
                present.join(",")
            ),
        }
    }
    if let Some(sec) = section(&mut body, kind) {
        if let Some(settings) = sec.get("settings").cloned() {
            let mut def = json!({ "settings": settings });
            let extra: Vec<&str> = opts
                .exclude_settings
                .iter()
                .map(|s| s.trim_start_matches("index."))
                .collect();
            strip_settings(&mut def, &extra);
            match cross {
                Some(true) => {
                    let d = strip_for_flavor(&mut def, info);
                    if !d.is_empty() {
                        log::warn!(
                            "{} '{name}': dropping settings {}",
                            kind.type_name(),
                            d.join(", ")
                        );
                    }
                }
                Some(false) => {}
                None => {
                    let f = foreign_settings(&def, info);
                    if !f.is_empty() {
                        log::warn!(
                            "{} '{name}' settings contain {}-only keys: {}; drop them with --excludeSettings={}",
                            kind.type_name(),
                            other_engine(info),
                            f.join(","),
                            f.join(",")
                        );
                    }
                }
            }
            sec.insert("settings".into(), def["settings"].clone());
        }
        if !info.has_types() {
            if let Some(m) = sec.get("mappings") {
                let m = typeless_mapping(m);
                sec.insert("mappings".into(), m);
            }
        }
        if opts.rename_aliases {
            if let Some(Value::Object(aliases)) = sec.get("aliases") {
                let mut out = Map::new();
                for (a, o) in aliases {
                    out.insert(dst.renamer.alias(a)?, o.clone());
                }
                sec.insert("aliases".into(), Value::Object(out));
            }
        }
    }
    if opts.rename_patterns {
        rename_patterns(&mut body, dst)?;
    }
    Ok(body)
}

fn rename_patterns(body: &mut Value, dst: &MetaTarget) -> Result<()> {
    if let Some(Value::Array(ps)) = body.get_mut("index_patterns") {
        for p in ps.iter_mut() {
            if let Value::String(s) = p {
                *s = dst.renamer.pattern(s)?;
            }
        }
    } else if let Some(Value::String(s)) = body.get_mut("index_patterns") {
        *s = dst.renamer.pattern(s)?;
    } else if let Some(Value::String(s)) = body.get_mut("template") {
        // elasticdump's 1.x/2.x legacy fixtures: "template": "pattern"
        *s = dst.renamer.pattern(s)?;
    }
    Ok(())
}

pub async fn write(
    doc: Value,
    kind: TemplateKind,
    client: &EsClient,
    info: &ClusterInfo,
    dst: &MetaTarget,
    opts: &MetaOptions,
) -> Result<WriteReport> {
    let items = normalize(&doc, kind)?;
    if kind == TemplateKind::Legacy
        && !info.is_opensearch()
        && info.version.major >= 8
        && !items.is_empty()
    {
        log::warn!(
            "Legacy templates (_template) are deprecated on Elasticsearch {}; composable index templates take precedence over them",
            info.version
        );
    }
    let mut report = WriteReport::default();
    for (name, body) in items {
        let result = put_one(&name, body, kind, client, info, dst, opts).await;
        match result {
            Ok(true) => report.written += 1,
            Ok(false) => {}
            Err(e) if opts.ignore_errors => {
                log::error!(
                    "{} '{name}' failed (--ignore-errors): {e:#}",
                    kind.type_name()
                );
                report.failed += 1;
            }
            Err(e) => return Err(e.context(format!("restoring {} '{name}'", kind.type_name()))),
        }
    }
    Ok(report)
}

async fn put_one(
    name: &str,
    body: Value,
    kind: TemplateKind,
    client: &EsClient,
    info: &ClusterInfo,
    dst: &MetaTarget,
    opts: &MetaOptions,
) -> Result<bool> {
    let body = prepare(name, body, kind, info, dst, opts)?;
    if kind == TemplateKind::Composable {
        let ignore: Vec<&str> = body
            .get("ignore_missing_component_templates")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let composed: Vec<String> = body
            .get("composed_of")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        for c in composed.iter().filter(|c| !ignore.contains(&c.as_str())) {
            let found = get_json_opt(
                client,
                opts.retry,
                &["_component_template", c],
                &[],
                "get component template",
            )
            .await?;
            if found.is_none() {
                bail!(
                    "index template '{name}' is composed of component template '{c}', which does not exist on the target; restore component templates first (--type component_template)"
                );
            }
        }
    }
    let query: &[(&str, &str)] = if opts.skip_existing {
        &[("create", "true")]
    } else {
        &[]
    };
    match send_json_q(
        client,
        opts.retry,
        Method::PUT,
        &[kind.api(), name],
        query,
        &body,
        "put template",
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(e) if opts.skip_existing && body_contains(&e, "already exists") => {
            log::warn!(
                "{} '{name}' already exists; skipping it (--skip-existing)",
                kind.type_name()
            );
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Flavor;
    use crate::rename::IndexRenamer;

    fn target(rules: &[&str]) -> MetaTarget {
        MetaTarget {
            client: None,
            info: None,
            index: None,
            doc_type: None,
            renamer: IndexRenamer::new(
                &rules.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                &[],
                "",
                "",
            )
            .unwrap(),
            file: Default::default(),
        }
    }

    fn opts() -> MetaOptions {
        MetaOptions::new(
            super::super::MetaKind::Template(TemplateKind::Composable),
            crate::http::RetryPolicy::none(),
        )
    }

    #[test]
    fn default_regex_matches_node_semantics() {
        let re = regex::Regex::new(DEFAULT_TEMPLATE_REGEX).unwrap();
        for n in [
            "metrics",
            "logs",
            ".kibana",
            "x_audit_log",
            "a-index-template",
        ] {
            assert!(re.is_match(n), "{n}");
        }
        for n in [
            "logs@mappings",
            "metrics-x",
            "synthetics",
            "ilm-history-7",
            "mine",
        ] {
            assert!(!re.is_match(n), "{n}");
        }
    }

    #[test]
    fn normalize_shapes_and_managed_filter() {
        let mut v = json!({"index_templates": [
            {"name": "a", "index_template": {"index_patterns": ["a*"], "_meta": {"managed": true}}},
            {"name": "b", "index_template": {"index_patterns": ["b*"]}}
        ]});
        drop_managed(&mut v, TemplateKind::Composable);
        let n = normalize(&v, TemplateKind::Composable).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].0, "b");
        let m = normalize(
            &json!({"t": {"index_patterns": ["x"]}}),
            TemplateKind::Composable,
        )
        .unwrap();
        assert_eq!(m[0].0, "t");
        let mut legacy = json!({".x": {}, "keep": {}});
        let re = regex::Regex::new(DEFAULT_TEMPLATE_REGEX).unwrap();
        filter_names(&mut legacy, TemplateKind::Legacy, |n| re.is_match(n), "");
        assert_eq!(legacy, json!({"keep": {}}));
    }

    #[test]
    fn prepare_strips_dates_foreign_fields_and_types() {
        let es92 = json!({
            "index_patterns": ["logstash-*"],
            "created_date_millis": 1, "modified_date": "x",
            "allow_auto_create": true,
            "data_stream": {"hidden": false, "allow_custom_routing": false},
            "template": {
                "lifecycle": {"enabled": true},
                "settings": {"index": {"lifecycle": {"name": "p"}, "number_of_shards": "1", "uuid": "u"}},
                "mappings": {"_doc": {"properties": {"a": {"type": "keyword"}}}},
                "aliases": {"logstash-all": {}}
            }
        });
        let os = ClusterInfo::new(Flavor::OpenSearch, "2.19.0");
        let es = ClusterInfo::new(Flavor::Elastic, "9.1.2");
        let mut o = opts();
        o.source_info = Some(es.clone());
        o.rename_patterns = true;
        o.rename_aliases = true;
        let dst = target(&["logstash-:logstash-srctag-"]);
        let b = prepare("t", es92.clone(), TemplateKind::Composable, &os, &dst, &o).unwrap();
        assert_eq!(
            b,
            json!({
                "index_patterns": ["logstash-srctag-*"],
                "data_stream": {},
                "template": {
                    "settings": {"index": {"number_of_shards": "1"}},
                    "mappings": {"properties": {"a": {"type": "keyword"}}},
                    "aliases": {"logstash-srctag-all": {}}
                }
            })
        );
        // file input: foreign fields are kept (with a warning) unless excluded
        let mut o = opts();
        o.exclude_template_fields = vec!["template.lifecycle".into()];
        let b = prepare("t", es92, TemplateKind::Composable, &os, &target(&[]), &o).unwrap();
        assert!(b.get("allow_auto_create").is_some());
        assert!(b.pointer("/template/lifecycle").is_none());
        assert!(b.get("created_date_millis").is_none());
        assert_eq!(b["index_patterns"], json!(["logstash-*"]));
    }
}
