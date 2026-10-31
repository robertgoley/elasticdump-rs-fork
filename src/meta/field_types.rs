//! L-655 (BRK-XV-004): mapping field types that the target engine or version has no
//! type for. A mapping with one is refused before anything is written, naming every such
//! field, its type and the closest type the target does have, instead of the server's
//! first "No handler for type" (OpenSearch) or "mapper type [..] does not exist"
//! (Elasticsearch) after the index was already created.
//!
//! Availability was checked live on OpenSearch 1.3.20, 2.19.7 and 3.9.0 and
//! Elasticsearch 7.10.2 (default and OSS), 8.19.23 and 9.1.2; the other first versions
//! are from the engines' release notes. Plugin types (`annotated_text`, `murmur3`,
//! `icu_collation_keyword`) depend on the installation and are left to the server.

use serde_json::Value;

use crate::http::ClusterInfo;

/// `(major, minor)` from which a type exists, and (Elasticsearch) the major it was
/// removed in.
#[derive(Clone, Copy)]
struct Range {
    from: (u64, u64),
    until_major: Option<u64>,
}

const fn from(major: u64, minor: u64) -> Range {
    Range {
        from: (major, minor),
        until_major: None,
    }
}

struct TypeInfo {
    name: &'static str,
    /// Elasticsearch versions with the type; empty = never.
    es: &'static [Range],
    /// Needs X-Pack (not in the 6.3-7.10 OSS builds).
    xpack: bool,
    /// OpenSearch versions with the type; empty = never.
    os: &'static [Range],
    /// The closest type on the other engine.
    instead: &'static str,
}

const TYPES: &[TypeInfo] = &[
    TypeInfo {
        name: "flattened",
        es: &[from(7, 3)],
        xpack: true,
        os: &[],
        instead: "flat_object on OpenSearch >= 2.7 (different query semantics)",
    },
    TypeInfo {
        name: "flat_object",
        es: &[],
        xpack: false,
        os: &[from(2, 7)],
        instead: "flattened on Elasticsearch >= 7.3",
    },
    TypeInfo {
        name: "dense_vector",
        es: &[from(7, 3)],
        xpack: true,
        os: &[],
        instead: "knn_vector (k-NN plugin, `dimension` instead of `dims`; its own dimension limit)",
    },
    TypeInfo {
        name: "knn_vector",
        es: &[],
        xpack: false,
        os: &[from(1, 0)],
        instead: "dense_vector (`dims` instead of `dimension`; its own dims limit)",
    },
    TypeInfo {
        name: "semantic_text",
        es: &[from(8, 15)],
        xpack: true,
        os: &[],
        instead: "semantic on OpenSearch >= 3.1 (needs a model_id)",
    },
    TypeInfo {
        name: "semantic",
        es: &[],
        xpack: false,
        os: &[from(3, 1)],
        instead: "semantic_text on Elasticsearch >= 8.15 (needs an inference endpoint)",
    },
    TypeInfo {
        name: "match_only_text",
        es: &[from(7, 14)],
        xpack: false,
        os: &[from(2, 12)],
        instead: "text",
    },
    TypeInfo {
        name: "wildcard",
        es: &[from(7, 9)],
        xpack: true,
        os: &[from(2, 15)],
        instead: "keyword",
    },
    TypeInfo {
        name: "constant_keyword",
        es: &[from(7, 7)],
        xpack: true,
        os: &[from(2, 14)],
        instead: "keyword",
    },
    TypeInfo {
        name: "unsigned_long",
        es: &[from(7, 10)],
        xpack: true,
        os: &[from(2, 8)],
        instead: "long (values above 2^63-1 do not fit)",
    },
    TypeInfo {
        name: "version",
        es: &[from(7, 10)],
        xpack: true,
        os: &[from(3, 0)],
        instead: "keyword (sorts as text, not as versions)",
    },
    TypeInfo {
        name: "aggregate_metric_double",
        es: &[from(7, 11)],
        xpack: true,
        os: &[],
        instead: "one double field per metric",
    },
    TypeInfo {
        name: "histogram",
        es: &[from(7, 6)],
        xpack: true,
        os: &[],
        instead: "none (keep the values and counts as two numeric arrays)",
    },
    TypeInfo {
        name: "counted_keyword",
        es: &[from(8, 12)],
        xpack: true,
        os: &[],
        instead: "keyword",
    },
    TypeInfo {
        name: "rank_vectors",
        es: &[from(8, 18)],
        xpack: true,
        os: &[],
        instead: "knn_vector per vector (no late interaction)",
    },
    TypeInfo {
        name: "shape",
        es: &[from(7, 4)],
        xpack: true,
        os: &[],
        instead: "xy_shape on OpenSearch >= 2.4",
    },
    TypeInfo {
        name: "point",
        es: &[from(7, 6)],
        xpack: true,
        os: &[],
        instead: "xy_point on OpenSearch >= 2.4",
    },
    TypeInfo {
        name: "xy_shape",
        es: &[],
        xpack: false,
        os: &[from(2, 4)],
        instead: "shape on Elasticsearch >= 7.4",
    },
    TypeInfo {
        name: "xy_point",
        es: &[],
        xpack: false,
        os: &[from(2, 4)],
        instead: "point on Elasticsearch >= 7.6",
    },
    TypeInfo {
        name: "passthrough",
        es: &[from(8, 13)],
        xpack: true,
        os: &[],
        instead: "object",
    },
    // Elasticsearch 7.x had a sparse_vector (removed in 8.0) and 8.11 brought a new one;
    // OpenSearch 3.x has its own with a required `method` (see `foreign_definition`).
    TypeInfo {
        name: "sparse_vector",
        es: &[
            Range {
                from: (7, 0),
                until_major: Some(8),
            },
            from(8, 11),
        ],
        xpack: true,
        os: &[from(3, 0)],
        instead: "rank_features",
    },
];

/// A mapping field whose type the target has no type for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unportable {
    /// Dotted field path; `dynamic_templates[name]` for a dynamic template.
    pub path: String,
    pub ty: String,
    pub reason: String,
}

impl std::fmt::Display for Unportable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` ({}: {})", self.path, self.ty, self.reason)
    }
}

fn has(ranges: &[Range], v: (u64, u64)) -> bool {
    ranges
        .iter()
        .any(|r| v >= r.from && r.until_major.is_none_or(|u| v.0 < u))
}

fn first(ranges: &[Range]) -> Option<(u64, u64)> {
    ranges.iter().map(|r| r.from).min()
}

/// Why `target` cannot take a field of type `ty` defined as `def`, if it cannot.
fn refusal(ty: &str, def: &Value, target: &ClusterInfo) -> Option<String> {
    let t = TYPES.iter().find(|t| t.name == ty)?;
    let v = (target.version.major, target.version.minor);
    let engine = format!("{} {}", target.flavor_name(), target.version);
    let (ranges, other) = if target.is_opensearch() {
        (t.os, "Elasticsearch")
    } else {
        (t.es, "OpenSearch")
    };
    if ranges.is_empty() {
        return Some(format!(
            "an {other}-only type, {engine} has none; closest: {}",
            t.instead
        ));
    }
    if !has(ranges, v) {
        let since = first(ranges).map_or_else(String::new, |(a, b)| format!(" (from {a}.{b})"));
        return Some(format!(
            "{engine} does not have it{since}; closest: {}",
            t.instead
        ));
    }
    if !target.is_opensearch() && t.xpack && target.oss {
        return Some(format!(
            "the {engine} OSS build has no X-Pack types; closest: {}",
            t.instead
        ));
    }
    foreign_definition(ty, def, target)
}

/// Same type name, other engine's definition: OpenSearch's `sparse_vector` needs a
/// `method`, Elasticsearch's has none.
fn foreign_definition(ty: &str, def: &Value, target: &ClusterInfo) -> Option<String> {
    if ty != "sparse_vector" {
        return None;
    }
    let os_style = def.get("method").is_some();
    match (target.is_opensearch(), os_style) {
        (true, false) => Some(format!(
            "an Elasticsearch sparse_vector (no `method`), which OpenSearch {}'s sparse_vector cannot take; closest: rank_features",
            target.version
        )),
        (false, true) => Some(format!(
            "an OpenSearch sparse_vector (with `method`), which Elasticsearch {} cannot take; closest: sparse_vector without `method`",
            target.version
        )),
        _ => None,
    }
}

/// Every field of one index's `mappings` (typed 6.x wrappers included) that `target`
/// has no type for, in mapping order.
pub fn unportable(mappings: &Value, target: &ClusterInfo) -> Vec<Unportable> {
    let mut out = Vec::new();
    let body = match mappings.as_object() {
        Some(o) if !o.contains_key("properties") && o.len() == 1 => match o.values().next() {
            Some(inner) if inner.get("properties").is_some() => inner,
            _ => mappings,
        },
        _ => mappings,
    };
    walk_properties(body.get("properties"), "", target, &mut out);
    if let Some(Value::Object(rt)) = body.get("runtime") {
        for (name, def) in rt {
            check(def, &format!("runtime.{name}"), target, &mut out);
        }
    }
    if let Some(Value::Array(dts)) = body.get("dynamic_templates") {
        for dt in dts {
            let Some(o) = dt.as_object() else { continue };
            for (name, t) in o {
                if let Some(m) = t.get("mapping") {
                    check(m, &format!("dynamic_templates[{name}]"), target, &mut out);
                }
            }
        }
    }
    out
}

fn check(def: &Value, path: &str, target: &ClusterInfo, out: &mut Vec<Unportable>) {
    if let Some(ty) = def.get("type").and_then(Value::as_str) {
        if let Some(reason) = refusal(ty, def, target) {
            out.push(Unportable {
                path: path.to_string(),
                ty: ty.to_string(),
                reason,
            });
        }
    }
}

fn walk_properties(
    props: Option<&Value>,
    prefix: &str,
    target: &ClusterInfo,
    out: &mut Vec<Unportable>,
) {
    let Some(Value::Object(props)) = props else {
        return;
    };
    for (name, def) in props {
        let path = format!("{prefix}{name}");
        check(def, &path, target, out);
        walk_properties(def.get("properties"), &format!("{path}."), target, out);
        walk_properties(def.get("fields"), &format!("{path}."), target, out);
    }
}

/// The refusal for one index: every field, then what to do.
pub fn refusal_message(index: &str, found: &[Unportable], target: &ClusterInfo) -> String {
    let list: Vec<String> = found.iter().map(ToString::to_string).collect();
    format!(
        "the mapping of '{index}' has {} field type(s) that {} {} has no type for: {}; nothing was written. Change or remove those fields in a dumped mapping file and restore that",
        found.len(),
        target.flavor_name(),
        target.version,
        list.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::info::Flavor;
    use semver::Version;
    use serde_json::json;

    fn os(v: &str) -> ClusterInfo {
        ClusterInfo {
            flavor: Flavor::OpenSearch,
            version: Version::parse(v).unwrap(),
            oss: false,
        }
    }
    fn es(v: &str, oss: bool) -> ClusterInfo {
        ClusterInfo {
            flavor: Flavor::Elastic,
            version: Version::parse(v).unwrap(),
            oss,
        }
    }
    fn def(ty: &str) -> Value {
        match ty {
            "dense_vector" | "rank_vectors" => json!({"type": ty, "dims": 3}),
            "knn_vector" => json!({"type": ty, "dimension": 3}),
            "constant_keyword" => json!({"type": ty, "value": "x"}),
            "aggregate_metric_double" => {
                json!({"type": ty, "metrics": ["min", "max"], "default_metric": "max"})
            }
            "semantic" => json!({"type": ty, "model_id": "m"}),
            _ => json!({"type": ty}),
        }
    }
    fn accepted(target: &ClusterInfo) -> Vec<&'static str> {
        TYPES
            .iter()
            .filter(|t| unportable(&json!({"properties": {"f": def(t.name)}}), target).is_empty())
            .map(|t| t.name)
            .collect()
    }

    // The live probes (PUT an index with one field of each type) of L-655, except where the
    // probe answered "other": the type exists but the minimal definition was incomplete.
    #[test]
    fn availability_matches_the_live_probes() {
        let mut want = vec!["knn_vector"];
        assert_eq!(accepted(&os("1.3.20")), want);
        want = vec![
            "flat_object",
            "knn_vector",
            "match_only_text",
            "wildcard",
            "constant_keyword",
            "unsigned_long",
            "xy_shape",
            "xy_point",
        ];
        assert_eq!(accepted(&os("2.19.7")), want);
        // 3.9.0: also `version`, `semantic`; its `sparse_vector` needs `method`
        let mut a = accepted(&os("3.9.0"));
        a.sort_unstable();
        let mut w = want.clone();
        w.extend(["version", "semantic"]);
        w.sort_unstable();
        assert_eq!(a, w);
        assert!(
            unportable(
                &json!({"properties": {"s": {"type": "sparse_vector", "method": {"name": "seismic"}}}}),
                &os("3.9.0")
            )
            .is_empty()
        );
        let es710 = [
            "flattened",
            "dense_vector",
            "wildcard",
            "constant_keyword",
            "unsigned_long",
            "version",
            "histogram",
            "shape",
            "point",
            "sparse_vector",
        ];
        assert_eq!(accepted(&es("7.10.2", false)), es710);
        assert!(accepted(&es("7.10.2", true)).is_empty());
        let es8 = [
            "flattened",
            "dense_vector",
            "semantic_text",
            "match_only_text",
            "wildcard",
            "constant_keyword",
            "unsigned_long",
            "version",
            "aggregate_metric_double",
            "histogram",
            "counted_keyword",
            "rank_vectors",
            "shape",
            "point",
            "passthrough",
            "sparse_vector",
        ];
        assert_eq!(accepted(&es("8.19.23", false)), es8);
        assert_eq!(accepted(&es("9.1.2", false)), es8);
        // removed in 8.0, back in 8.11
        assert!(!accepted(&es("8.6.1", false)).contains(&"sparse_vector"));
    }

    #[test]
    fn every_field_is_named_with_its_path_type_and_the_closest_type() {
        let m = json!({
            "dynamic_templates": [{"vecs": {"match": "v_*", "mapping": {"type": "dense_vector", "dims": 4}}}],
            "properties": {
                "k": {"type": "keyword", "fields": {"wc": {"type": "wildcard"}}},
                "labels": {"type": "flattened"},
                "obj": {"properties": {"emb": {"type": "dense_vector", "dims": 2048}, "n": {"type": "long"}}},
                "nested": {"type": "nested", "properties": {"sem": {"type": "semantic_text"}}},
                "sv": {"type": "sparse_vector"}
            }
        });
        let found = unportable(&m, &os("2.19.7"));
        let got: Vec<(&str, &str)> = found
            .iter()
            .map(|u| (u.path.as_str(), u.ty.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("labels", "flattened"),
                ("obj.emb", "dense_vector"),
                ("nested.sem", "semantic_text"),
                ("sv", "sparse_vector"),
                ("dynamic_templates[vecs]", "dense_vector"),
            ]
        );
        assert!(found[0].reason.contains("flat_object"), "{found:?}");
        assert!(found[1].reason.contains("knn_vector"), "{found:?}");
        let msg = refusal_message("idx", &found, &os("2.19.7"));
        assert!(
            msg.contains(
                "the mapping of 'idx' has 5 field type(s) that OpenSearch 2.19.7 has no type for"
            ) && msg.contains("`labels` (flattened: an Elasticsearch-only type")
                && msg.contains("nothing was written"),
            "{msg}"
        );
        // OpenSearch 1.3 has no wildcard yet: the multi-field is named too
        let found = unportable(&m, &os("1.3.20"));
        assert_eq!(found[0].path, "k.wc");
        assert!(found[0].reason.contains("(from 2.15)"), "{found:?}");
        // the other direction
        let m = json!({"_doc": {"properties": {"fo": {"type": "flat_object"}, "kv": {"type": "knn_vector", "dimension": 3}, "rt": {"type": "keyword"}}}});
        let found = unportable(&m, &es("8.19.23", false));
        let got: Vec<&str> = found.iter().map(|u| u.path.as_str()).collect();
        assert_eq!(got, ["fo", "kv"], "typed wrapper unwrapped");
        assert!(found[0].reason.contains("flattened"), "{found:?}");
        let found = unportable(
            &json!({"properties": {"s": {"type": "sparse_vector", "method": {"name": "seismic"}}}}),
            &es("9.1.2", false),
        );
        assert!(
            found[0].reason.contains("OpenSearch sparse_vector"),
            "{found:?}"
        );
        let found = unportable(
            &json!({"properties": {"f": {"type": "flattened"}}}),
            &es("7.10.2", true),
        );
        assert!(found[0].reason.contains("OSS build"), "{found:?}");
    }

    #[test]
    fn ordinary_and_unknown_types_are_left_to_the_server() {
        let m = json!({"properties": {
            "a": {"type": "text"}, "b": {"type": "murmur3"}, "c": {"type": "no_such_type"},
            "d": {"properties": {"e": {"type": "geo_point"}}}, "f": {"type": "object", "enabled": false}
        }});
        for t in [
            os("1.3.20"),
            os("3.9.0"),
            es("7.10.2", true),
            es("9.1.2", false),
        ] {
            assert!(unportable(&m, &t).is_empty());
        }
        assert!(unportable(&json!({}), &os("2.19.7")).is_empty());
        assert!(unportable(&json!("x"), &os("2.19.7")).is_empty());
    }
}
