//! Endpoint inventory matcher and response-pointer checker (TST-API-001, L-401).
//! The inventory is `tests/golden/endpoints.toml`.

use std::path::PathBuf;

use regex::Regex;
use serde_json::Value;

#[derive(Debug)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub methods: Vec<String>,
    pub path: Regex,
    pub query: Option<String>,
    pub reads: Vec<String>,
}

pub struct Inventory {
    pub rows: Vec<Row>,
}

pub fn inventory_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/endpoints.toml")
}

impl Inventory {
    pub fn load() -> Inventory {
        let text = std::fs::read_to_string(inventory_path()).expect("tests/golden/endpoints.toml");
        let mut rows = vec![];
        let mut cur: Option<Vec<(String, Value)>> = None;
        let mut flush = |cur: &mut Option<Vec<(String, Value)>>| {
            if let Some(kv) = cur.take() {
                let get = |k: &str| kv.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
                let strs = |v: Option<Value>| -> Vec<String> {
                    v.and_then(|v| v.as_array().cloned())
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                };
                let s = |k: &str| get(k).and_then(|v| v.as_str().map(String::from));
                rows.push(Row {
                    id: s("id").expect("row id"),
                    title: s("title").unwrap_or_default(),
                    methods: strs(get("method")),
                    path: Regex::new(&s("path").expect("row path")).expect("row path regex"),
                    query: s("query"),
                    reads: strs(get("reads")),
                });
            }
        };
        for line in text.lines().map(str::trim) {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if line == "[[endpoint]]" {
                flush(&mut cur);
                cur = Some(vec![]);
            } else if let Some((k, v)) = line.split_once(" = ") {
                let v: Value = serde_json::from_str(v)
                    .unwrap_or_else(|e| panic!("endpoints.toml: {line}: {e}"));
                cur.as_mut()
                    .expect("key outside [[endpoint]]")
                    .push((k.to_string(), v));
            }
        }
        flush(&mut cur);
        Inventory { rows }
    }

    /// First row matching the request; `query` holds the query parameter names.
    pub fn matching(&self, method: &str, path: &str, query: &[String]) -> Option<&Row> {
        self.rows.iter().find(|r| {
            r.methods.iter().any(|m| m.eq_ignore_ascii_case(method))
                && r.path.is_match(path)
                && r.query
                    .as_ref()
                    .is_none_or(|q| query.iter().any(|k| k == q))
        })
    }

    /// Requests (method, path, query names) that match no row, formatted for an assertion.
    pub fn unlisted<'a>(
        &self,
        reqs: impl IntoIterator<Item = (&'a str, &'a str, Vec<String>)>,
    ) -> Vec<String> {
        reqs.into_iter()
            .filter(|(m, p, q)| self.matching(m, p, q).is_none())
            .map(|(m, p, q)| format!("{m} {p} (query: {q:?})"))
            .collect()
    }
}

/// Check `pointer:type|type` (with `[*]` array walks and optional `?`) against a JSON body.
/// Returns a description of the first violation.
pub fn check_read(body: &Value, spec: &str) -> Result<(), String> {
    let (ptr, types) = spec
        .rsplit_once(':')
        .ok_or_else(|| format!("bad read spec {spec}"))?;
    let (ptr, optional) = match ptr.strip_suffix('?') {
        Some(p) => (p, true),
        None => (ptr, false),
    };
    let types: Vec<&str> = types.split('|').collect();
    let mut cur: Vec<&Value> = vec![body];
    for seg in ptr.split('.') {
        let (key, all) = match seg.strip_suffix("[*]") {
            Some(k) => (k, true),
            None => (seg, false),
        };
        let mut next = vec![];
        for v in cur {
            match v.get(key) {
                Some(x) if all => match x.as_array() {
                    Some(a) => next.extend(a.iter()),
                    None => return Err(format!("{ptr}: `{key}` is not an array")),
                },
                Some(x) => next.push(x),
                None if optional => return Ok(()),
                None => return Err(format!("{ptr}: `{key}` missing")),
            }
        }
        cur = next;
    }
    for v in cur {
        let t = match v {
            Value::String(_) => "string",
            Value::Number(n) if n.is_i64() || n.is_u64() => "int",
            Value::Number(_) => "number",
            Value::Bool(_) => "bool",
            Value::Object(_) => "object",
            Value::Array(_) => "array",
            Value::Null => "null",
        };
        if !(types.contains(&t) || (t == "int" && types.contains(&"number"))) {
            return Err(format!("{ptr}: expected {}, got {t}", types.join("|")));
        }
    }
    Ok(())
}
