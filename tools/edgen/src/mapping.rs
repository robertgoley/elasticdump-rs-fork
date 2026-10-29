//! Mapping and settings per profile (TST-DATA-002). `engine` picks the
//! variant where engines differ: `es` (flattened), `os` (flat_object,
//! OpenSearch >= 2.7) and `os1` (plain disabled object for OpenSearch 1.x).

use serde_json::{Value, json};

fn labels(engine: &str) -> Value {
    match engine {
        "es" => json!({"type": "flattened"}),
        "os" => json!({"type": "flat_object"}),
        _ => json!({"type": "object", "enabled": false}),
    }
}

fn kw() -> Value {
    json!({"type": "keyword", "ignore_above": 256})
}

pub fn profile_props(profile: &str, engine: &str, fields: usize) -> Value {
    match profile {
        "logs" => json!({
            "@timestamp": {"type": "date"},
            "host": {"properties": {"name": kw()}},
            "service": {"properties": {"name": kw()}},
            "log": {"properties": {"level": kw()}},
            "http": {"properties": {
                "request": {"properties": {"method": kw()}},
                "response": {"properties": {"status_code": {"type": "short"}, "bytes": {"type": "long"}}}}},
            "url": {"properties": {"path": kw()}},
            "client": {"properties": {"ip": {"type": "ip"}}},
            "user_agent": {"properties": {"original": {"type": "text", "fields": {"keyword": kw()}}}},
            "event": {"properties": {"duration": {"type": "long"}}},
            "trace": {"properties": {"id": kw()}},
            "labels": labels(engine),
            "tags": kw(),
            "message": {"type": "text"}
        }),
        "metrics" => {
            let mut p = serde_json::Map::new();
            p.insert("@timestamp".into(), json!({"type": "date"}));
            p.insert("host".into(), json!({"properties": {"name": kw()}}));
            for g in ["cpu", "memory", "disk", "network"] {
                let mut q = serde_json::Map::new();
                for k in 0..8 {
                    q.insert(
                        format!("m{k}"),
                        if k % 2 == 0 {
                            json!({"type": "scaled_float", "scaling_factor": 1000})
                        } else {
                            json!({"type": "long"})
                        },
                    );
                }
                p.insert(g.into(), json!({"properties": q}));
            }
            Value::Object(p)
        }
        "nested" => json!({
            "title": {"type": "text"},
            "body": {"type": "text"},
            "created": {"type": "date"},
            "user": {"properties": {"id": {"type": "long"}, "name": kw()}},
            "tags": kw(),
            "answers": {"type": "nested", "properties": {
                "user": {"type": "long"}, "body": {"type": "text"}, "score": {"type": "integer"}, "created": {"type": "date"},
                "comments": {"type": "nested", "properties": {"user": {"type": "long"}, "text": {"type": "text"}}}}}
        }),
        "geo" => json!({
            "name": kw(), "country": kw(), "population": {"type": "long"},
            "location": {"type": "geo_point"}, "area": {"type": "geo_shape"}
        }),
        "wide" => {
            let mut p = serde_json::Map::new();
            for f in 0..fields {
                let t = match f % 4 {
                    0 => kw(),
                    1 => json!({"type": "long"}),
                    2 => json!({"type": "double"}),
                    _ => json!({"type": "boolean"}),
                };
                p.insert(format!("f{f:05}"), t);
            }
            Value::Object(p)
        }
        "huge" => {
            json!({"n": {"type": "long"}, "bytes": {"type": "long"}, "blob": {"type": "object", "enabled": false}})
        }
        "edge" => json!({
            "case": kw(), "expect": kw(), "n": {"type": "long"},
            "text": {"type": "text"},
            "kw": {"type": "keyword", "ignore_above": 1024},
            "raw": {"type": "object", "enabled": false},
            "hits": kw(), "sort": kw(), "pit_id": kw(), "_scroll_id": kw(),
            "ts_nanos": {"type": "date_nanos"},
            "ts_millis": {"type": "date", "format": "epoch_millis"},
            "ts_custom": {"type": "date", "format": "dd/MM/yyyy HH:mm"}
        }),
        "mixed" => {
            let mut p = serde_json::Map::new();
            p.insert("kind".into(), kw());
            for s in crate::MIXED.iter().map(|x| x.0) {
                p.insert(
                    s.into(),
                    json!({"properties": profile_props(s, engine, fields)}),
                );
            }
            // edge-style dates and raw parts live inside "edge"; disable dynamic
            // mapping surprises there.
            Value::Object(p)
        }
        _ => json!({}),
    }
}

/// `{"settings": …, "mappings": …}` for index creation.
pub fn mapping(profile: &str, engine: &str, fields: usize) -> Value {
    let mut settings = json!({"index": {"number_of_shards": 1, "number_of_replicas": 0}});
    match profile {
        "wide" => settings["index"]["mapping"] = json!({"total_fields": {"limit": fields + 100}}),
        "mixed" => {
            settings["index"]["mapping"] =
                json!({"total_fields": {"limit": 2000}, "nested_objects": {"limit": 20000}})
        }
        "nested" => settings["index"]["mapping"] = json!({"nested_objects": {"limit": 20000}}),
        "edge" => settings["index"]["mapping"] = json!({"total_fields": {"limit": 2000}}),
        _ => {}
    }
    let dynamic = if profile == "edge" || profile == "mixed" {
        json!(true)
    } else {
        json!("strict")
    };
    let props = profile_props(profile, engine, fields);
    json!({"settings": settings, "mappings": {"dynamic": dynamic, "properties": props}})
}
