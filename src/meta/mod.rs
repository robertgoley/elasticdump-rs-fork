//! Metadata types (`--type` other than `data`): dump to a file and restore
//! to a cluster, or copy cluster to cluster, with elasticdump-compatible
//! file formats.
//!
//! Every type is "get everything once, put everything once": one JSON
//! document per run, written as one line (see [`codec`]).

pub mod alias;
pub mod codec;
mod http;
pub mod index_scoped;
pub mod policy;
pub mod scripts;
pub mod templates;

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde_json::Value;

pub use index_scoped::{SETTINGS_EXCLUDE, strip_settings, typeless_mapping};

use crate::http::{ClusterInfo, EsClient, RetryPolicy};
use crate::pipeline::WriteReport;
use crate::pipeline::file_sink::{FileOptions, FileSink};
use crate::rename::IndexRenamer;
use codec::LineEncoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    /// `_template`
    Legacy,
    /// `_component_template`
    Component,
    /// `_index_template`
    Composable,
}

impl TemplateKind {
    pub fn api(self) -> &'static str {
        match self {
            TemplateKind::Legacy => "_template",
            TemplateKind::Component => "_component_template",
            TemplateKind::Composable => "_index_template",
        }
    }
    pub fn type_name(self) -> &'static str {
        match self {
            TemplateKind::Legacy => "template",
            TemplateKind::Component => "component_template",
            TemplateKind::Composable => "index_template",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaKind {
    Mapping,
    Settings,
    Index,
    Analyzer,
    Alias,
    Template(TemplateKind),
    Policy,
    Script,
}

impl MetaKind {
    pub fn type_name(self) -> &'static str {
        match self {
            MetaKind::Mapping => "mapping",
            MetaKind::Settings => "settings",
            MetaKind::Index => "index",
            MetaKind::Analyzer => "analyzer",
            MetaKind::Alias => "alias",
            MetaKind::Template(t) => t.type_name(),
            MetaKind::Policy => "policy",
            MetaKind::Script => "script",
        }
    }

    /// Index-scoped kinds name indices; the others name cluster resources.
    pub fn index_scoped(self) -> bool {
        matches!(
            self,
            MetaKind::Mapping | MetaKind::Settings | MetaKind::Index | MetaKind::Analyzer
        )
    }

    fn encoding(self, object_format: bool) -> LineEncoding {
        if self == MetaKind::Mapping || object_format {
            LineEncoding::Object
        } else {
            LineEncoding::StringLiteral
        }
    }
}

pub struct MetaSource {
    pub client: Option<EsClient>,
    pub info: Option<ClusterInfo>,
    /// Index (index-scoped types and alias) or resource-name filter.
    pub index: Option<String>,
    /// Alias-name filter (`--type alias`).
    pub doc_type: Option<String>,
    pub file: Option<PathBuf>,
}

pub struct MetaTarget {
    pub client: Option<EsClient>,
    pub info: Option<ClusterInfo>,
    pub index: Option<String>,
    pub doc_type: Option<String>,
    /// `--renamePrefix`/`--renameIndex` rules plus `--prefix`/`--suffix`.
    pub renamer: IndexRenamer,
    pub file: FileOptions,
}

impl MetaTarget {
    /// Target index for a source index: the explicit output index, or the
    /// renamed source name; then prefix/suffix.
    pub fn target_index(&self, src: &str) -> Result<String> {
        match &self.index {
            Some(i) => {
                let out = format!("{}{}{}", self.renamer.prefix, i, self.renamer.suffix);
                crate::rename::validate_index_name(&out)
                    .map_err(|e| anyhow::anyhow!("invalid output index '{out}': {e}"))?;
                Ok(out)
            }
            None => self.renamer.index(src),
        }
    }
}

pub struct MetaOptions {
    pub kind: MetaKind,
    pub retry: RetryPolicy,
    pub skip_existing: bool,
    pub ignore_errors: bool,
    pub exclude_settings: Vec<String>,
    /// Flavor of the source cluster (cluster→cluster only): engine-specific
    /// settings and template fields are stripped when it differs from the
    /// target's.
    pub source_info: Option<ClusterInfo>,
    /// `--metaFormat object`: write a plain object line (Node cannot read it).
    pub object_format: bool,
    /// `--filterSystemTemplates` with `--templateRegex` (dump only).
    pub template_filter: Option<regex::Regex>,
    /// `--filterManaged`: skip `_meta.managed: true` templates and policies.
    pub filter_managed: bool,
    /// `--closeIndex`: allow `analyzer` on existing indices.
    pub close_index: bool,
    /// `--excludeTemplateFields`: dotted paths dropped from template bodies.
    pub exclude_template_fields: Vec<String>,
    /// `--convertIlmToIsm`: translate ILM policies for an OpenSearch target.
    pub convert_ilm_to_ism: bool,
    /// `--lenientPolicyConversion`: drop unconvertible ILM parts with a warning.
    pub lenient_policy_conversion: bool,
    /// `--renameAliases`: apply rename rules to alias names.
    pub rename_aliases: bool,
    /// `--renamePatterns`: apply rename rules to template / ISM index patterns.
    pub rename_patterns: bool,
}

impl MetaOptions {
    pub fn new(kind: MetaKind, retry: RetryPolicy) -> Self {
        Self {
            kind,
            retry,
            skip_existing: false,
            ignore_errors: false,
            exclude_settings: Vec::new(),
            source_info: None,
            object_format: false,
            template_filter: None,
            filter_managed: true,
            close_index: false,
            exclude_template_fields: Vec::new(),
            convert_ilm_to_ism: false,
            lenient_policy_conversion: false,
            rename_aliases: false,
            rename_patterns: false,
        }
    }
}

/// Guess which `--type` produced `v` (for "wrong --type" hints).
pub fn classify(v: &Value) -> Option<&'static str> {
    let obj = v.as_object()?;
    if obj.is_empty() {
        return None;
    }
    // a data dump line (a search hit)
    if obj.get("_source").is_some_and(Value::is_object)
        || (obj.get("_id").is_some_and(Value::is_string) && obj.contains_key("_index"))
    {
        return Some("data");
    }
    if obj.get("stored_scripts").is_some_and(Value::is_array) {
        return Some("script");
    }
    if obj.get("index_templates").is_some_and(Value::is_array) {
        return Some("index_template");
    }
    if obj.get("component_templates").is_some_and(Value::is_array) {
        return Some("component_template");
    }
    if obj.get("policies").is_some_and(Value::is_array)
        || (obj.contains_key("_id") && obj.get("policy").is_some_and(|p| p.get("states").is_some()))
    {
        return Some("policy");
    }
    let values: Vec<&serde_json::Map<String, Value>> =
        obj.values().filter_map(Value::as_object).collect();
    if values.len() != obj.len() {
        return None;
    }
    let all = |f: &dyn Fn(&serde_json::Map<String, Value>) -> bool| values.iter().all(|o| f(o));
    if all(&|o| {
        o.get("policy")
            .is_some_and(|p| p.get("phases").is_some() || p.get("states").is_some())
    }) {
        return Some("policy");
    }
    if all(&|o| {
        o.contains_key("index_patterns") || (o.contains_key("order") && o.contains_key("template"))
    }) {
        return Some("template");
    }
    let keys_in = |o: &serde_json::Map<String, Value>, allowed: &[&str]| {
        !o.is_empty() && o.keys().all(|k| allowed.contains(&k.as_str()))
    };
    if all(&|o| keys_in(o, &["aliases"])) {
        return Some("alias");
    }
    if all(&|o| keys_in(o, &["settings"])) {
        return Some("settings");
    }
    if all(&|o| keys_in(o, &["mappings"])) {
        return Some("mapping");
    }
    if all(&|o| {
        keys_in(o, &["aliases", "mappings", "settings", "data_stream"])
            && o.contains_key("settings")
            && o.contains_key("mappings")
    }) {
        return Some("index");
    }
    None
}

/// For `--type data` input (TST-BRK-IN-004): the first line of a metadata dump, with the
/// type that wrote it. Stricter than [`classify`] so that ordinary `--sourceOnly` documents
/// are never refused: a JSON string line (Node's string meta format) holding a metadata
/// object, a top-level key only metadata responses have, or `{name: {settings: {index: {}}}}`
/// / `{name: {mappings: {properties: {}}}}` / `{name: {aliases: {..objects..}}}` shapes.
pub fn metadata_dump_type(line: &[u8]) -> Option<&'static str> {
    let v: Value = serde_json::from_slice(line).ok()?;
    let v = match v {
        Value::String(s) => serde_json::from_str(&s).ok()?,
        v => v,
    };
    let found = classify(&v)?;
    let inner = |o: &Value, key: &str, sub: &str| {
        o.get(key)
            .and_then(|x| x.get(sub))
            .is_some_and(Value::is_object)
    };
    let values = || v.as_object().into_iter().flat_map(|o| o.values());
    let strict = match found {
        "data" => false,
        "script" | "index_template" | "component_template" | "policy" => true,
        "settings" => values().all(|o| inner(o, "settings", "index")),
        "mapping" => values().all(|o| inner(o, "mappings", "properties")),
        "index" => values().all(|o| {
            inner(o, "settings", "index") && o.get("mappings").is_some_and(Value::is_object)
        }),
        "alias" => values().all(|o| {
            o.get("aliases")
                .and_then(Value::as_object)
                .is_some_and(|a| !a.is_empty() && a.values().all(Value::is_object))
        }),
        "template" => values().all(|o| o.get("index_patterns").is_some_and(Value::is_array)),
        _ => false,
    };
    strict.then_some(found)
}

/// Refuse a file that was clearly written by another `--type`.
pub fn check_shape(v: &Value, kind: MetaKind) -> Result<()> {
    let Some(found) = classify(v) else {
        return Ok(());
    };
    let ok: &[&str] = match kind {
        MetaKind::Mapping => &["mapping", "index"],
        MetaKind::Settings | MetaKind::Analyzer => &["settings", "index"],
        MetaKind::Index => &["index", "settings", "mapping"],
        MetaKind::Alias => &["alias", "index"],
        MetaKind::Template(TemplateKind::Legacy) => &["template"],
        // a plain {name: body} map is accepted for composable kinds too
        MetaKind::Template(TemplateKind::Component) => &["component_template", "template"],
        MetaKind::Template(TemplateKind::Composable) => &["index_template", "template"],
        MetaKind::Policy => &["policy"],
        MetaKind::Script => &["script"],
    };
    if ok.contains(&found) {
        return Ok(());
    }
    bail!(
        "this input looks like a --type {found} dump, not --type {}; use --type {found}",
        kind.type_name()
    )
}

/// Read the document from the source cluster or file.
/// Per-version availability of the cluster-level APIs (minimum supported:
/// Elasticsearch 7.x, OpenSearch 1.x).
pub fn supports(info: &ClusterInfo, kind: MetaKind) -> Result<()> {
    use crate::http::Flavor;
    let v = &info.version;
    let missing = match (info.flavor, kind) {
        (
            Flavor::Elastic,
            MetaKind::Template(TemplateKind::Component | TemplateKind::Composable),
        ) if (v.major, v.minor) < (7, 8) => {
            Some("component and composable index templates need Elasticsearch >= 7.8")
        }
        (Flavor::Elastic, MetaKind::Policy) if (v.major, v.minor) < (6, 6) => {
            Some("ILM policies need Elasticsearch >= 6.6")
        }
        (Flavor::Elastic, MetaKind::Policy) if info.oss => {
            Some("ILM is an X-Pack feature and this is an OSS build (build_flavor \"oss\")")
        }
        _ => None,
    };
    match missing {
        Some(m) => Err(anyhow::anyhow!(
            "--type {}: {m}; this cluster is {} {}",
            kind.type_name(),
            info.flavor_name(),
            info.version
        )),
        None => Ok(()),
    }
}

pub async fn read(src: &MetaSource, opts: &MetaOptions) -> Result<Value> {
    let mut v = match &src.client {
        Some(client) => {
            let info = src.info.as_ref().expect("source cluster info");
            match opts.kind {
                k if k.index_scoped() => {
                    index_scoped::fetch(client, src.index.as_deref(), opts).await?
                }
                MetaKind::Alias => {
                    alias::fetch(client, src.index.as_deref(), src.doc_type.as_deref(), opts)
                        .await?
                }
                MetaKind::Template(t) => {
                    supports(info, opts.kind)?;
                    templates::fetch(client, t, src.index.as_deref(), opts).await?
                }
                MetaKind::Policy => {
                    supports(info, opts.kind)?;
                    policy::fetch(client, info, src.index.as_deref(), opts).await?
                }
                MetaKind::Script => scripts::fetch(client, src.index.as_deref(), opts).await?,
                _ => unreachable!(),
            }
        }
        None => {
            let file = src.file.clone();
            let v = tokio::task::spawn_blocking(move || codec::read_first_value(file)).await??;
            let v = codec::decode_any(v)?;
            check_shape(&v, opts.kind)?;
            v
        }
    };
    if !v.is_object() {
        bail!(
            "--type {} document must be a JSON object",
            opts.kind.type_name()
        );
    }
    index_scoped::strip_generated(&mut v, opts.kind);
    match opts.kind {
        MetaKind::Template(t) if opts.filter_managed => templates::drop_managed(&mut v, t),
        MetaKind::Policy if opts.filter_managed => policy::drop_managed(&mut v),
        _ => {}
    }
    Ok(v)
}

/// Write the document to a file target (one line) or apply it to a cluster.
pub async fn write(doc: Value, dst: &MetaTarget, opts: &MetaOptions) -> Result<WriteReport> {
    let Some(client) = &dst.client else {
        let line = codec::encode_line(&doc, opts.kind.encoding(opts.object_format))?;
        let mut fopts = dst.file.clone();
        fopts.split = None;
        fopts.source_only = false;
        let mut sink = FileSink::create(fopts).await?;
        sink.write_lines(format!("{line}\n").as_bytes()).await?;
        sink.finalize().await?;
        return Ok(WriteReport {
            written: 1,
            failed: 0,
            ..Default::default()
        });
    };
    let info = dst.info.as_ref().expect("target cluster info");
    if !opts.kind.index_scoped() && opts.kind != MetaKind::Alias && dst.index.is_some() {
        log::warn!(
            "The output index is ignored for --type {}: resources are restored under their own names",
            opts.kind.type_name()
        );
    }
    supports(info, opts.kind)?;
    match opts.kind {
        k if k.index_scoped() => index_scoped::write(doc, client, info, dst, opts).await,
        MetaKind::Alias => alias::write(doc, client, dst, opts).await,
        MetaKind::Template(t) => templates::write(doc, t, client, info, dst, opts).await,
        MetaKind::Policy => policy::write(doc, client, info, dst, opts).await,
        MetaKind::Script => scripts::write(doc, client, opts).await,
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classify_and_wrong_type_hints() {
        let cases = [
            (json!({"stored_scripts": []}), Some("script")),
            (json!({"index_templates": []}), Some("index_template")),
            (
                json!({"component_templates": []}),
                Some("component_template"),
            ),
            (json!({"policies": [], "total_policies": 0}), Some("policy")),
            (json!({"p": {"policy": {"phases": {}}}}), Some("policy")),
            (
                json!({"t": {"order": 0, "index_patterns": ["a*"]}}),
                Some("template"),
            ),
            (json!({"i": {"aliases": {"a": {}}}}), Some("alias")),
            (json!({"i": {"settings": {}}}), Some("settings")),
            (json!({"i": {"mappings": {}}}), Some("mapping")),
            (
                json!({"i": {"aliases": {}, "mappings": {}, "settings": {}}}),
                Some("index"),
            ),
            (json!({}), None),
        ];
        for (v, want) in cases {
            assert_eq!(classify(&v), want, "{v}");
        }
        let e = check_shape(
            &json!({"index_templates": []}),
            MetaKind::Template(TemplateKind::Legacy),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("use --type index_template"), "{e}");
        assert!(check_shape(&json!({"i": {"settings": {}}}), MetaKind::Analyzer).is_ok());
        assert!(check_shape(&json!({"stored_scripts": []}), MetaKind::Alias).is_err());
        assert!(
            check_shape(
                &json!({"t": {"index_patterns": ["a"]}}),
                MetaKind::Template(TemplateKind::Composable)
            )
            .is_ok()
        );
    }
}
