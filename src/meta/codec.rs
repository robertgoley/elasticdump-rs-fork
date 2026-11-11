//! File encoding of the metadata types.
//!
//! elasticdump writes every metadata type except `mapping` as one line holding
//! a JSON *string literal* of the document (`JSON.stringify` of the response
//! body), and its restore path can only read that form. elasticdump-rs writes
//! the same form by default (`--metaFormat object` writes a plain object,
//! which Node cannot read) and accepts both on restore.

use std::io::Read;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::pipeline::json_split::ValueSplitter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEncoding {
    /// `"{\"a\":1}"`: elasticdump's format.
    StringLiteral,
    /// `{"a":1}`.
    Object,
}

/// Read the first JSON value of a file / stdin (gzip and zstd auto-detected).
/// Further values are ignored with a warning (elasticdump ignores them
/// silently).
pub fn read_first_value(input: impl Into<crate::pipeline::file_source::InputLoc>) -> Result<Value> {
    let input = input.into();
    let mut reader = crate::pipeline::file_source::open_input_counted(
        &input,
        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    )?;
    let mut splitter = ValueSplitter::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut first: Option<Value> = None;
    loop {
        let n = reader.read(&mut buf).context("Failed to read input")?;
        if n > 0 {
            splitter.push(&buf[..n]);
        }
        while let Some(v) = splitter.next_value(n == 0).map_err(|e| anyhow!(e))? {
            if let Some(value) = first.take() {
                log::warn!(
                    "Input holds more than one JSON value; only the first is used (elasticdump behaviour)"
                );
                return Ok(value);
            }
            {
                let v = match repair_lone_surrogates(&String::from_utf8_lossy(&v)) {
                    Some(fixed) => fixed.into_bytes(),
                    None => v.to_vec(),
                };
                first = Some(serde_json::from_slice(&v).map_err(|e| {
                    let name = input.name();
                    let hint = if e.to_string().contains("recursion limit") {
                        " (nested more than 128 levels deep; Elasticsearch and OpenSearch refuse mappings deeper than index.mapping.depth.limit, 20 by default)"
                    } else {
                        ""
                    };
                    anyhow!("Invalid JSON in input {name}: {e}{hint}")
                })?);
            }
        }
        if n == 0 {
            return first.ok_or_else(|| {
                anyhow!(
                    "Input {} is empty (no JSON value: empty, whitespace or a byte-order mark only); a metadata restore needs the dumped object",
                    match &input {
                        crate::pipeline::file_source::InputLoc::Stdin => "stdin".into(),
                        other => format!("'{}'", other.name()),
                    }
                )
            });
        }
    }
}

/// A metadata file line: a JSON string literal holding the object, or the
/// object itself.
pub fn decode_any(v: Value) -> Result<Value> {
    match v {
        Value::String(s) => {
            let s = repair_lone_surrogates(&s).unwrap_or(s);
            let inner: Value =
                serde_json::from_str(&s).context("the string in the input is not JSON")?;
            match inner {
                Value::Object(_) => Ok(inner),
                Value::String(_) => bail!(
                    "the input is a JSON string holding another JSON string (encoded more than twice); elasticdump writes metadata as an object or as one string literal of it"
                ),
                other => bail!(
                    "the input must hold a JSON object (or a string literal of one), got a string holding {}",
                    kind(&other)
                ),
            }
        }
        v @ Value::Object(_) => Ok(v),
        other => bail!(
            "the input must hold a JSON object (or a string literal of one), got {}",
            kind(&other)
        ),
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Node (>= 12) writes a lone UTF-16 surrogate as an escape (`"\ud800"`), which serde_json
/// refuses (TST-BRK-IN-013). Each unpaired `\uD800`-`\uDFFF` escape becomes `\ufffd`, with a
/// warning; `None` when there is none. Backslashes only occur inside strings, and an escaped
/// backslash (`\\`) is skipped as a pair, so `\\ud800` (a literal text) is left alone.
pub fn repair_lone_surrogates(s: &str) -> Option<String> {
    fn hex4(b: &[u8]) -> Option<u32> {
        let t = std::str::from_utf8(b.get(..4)?).ok()?;
        u32::from_str_radix(t, 16).ok()
    }
    let b = s.as_bytes();
    memchr::memmem::find(b, b"\\u")?;
    let mut out = Vec::with_capacity(b.len());
    let (mut i, mut fixed) = (0usize, 0usize);
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        if b.get(i + 1) == Some(&b'u')
            && let Some(cp) = hex4(&b[i + 2..])
        {
            let low_next = b.get(i + 6) == Some(&b'\\')
                && b.get(i + 7) == Some(&b'u')
                && hex4(&b[(i + 8).min(b.len())..]).is_some_and(|c| (0xdc00..=0xdfff).contains(&c));
            if (0xd800..=0xdbff).contains(&cp) && low_next {
                out.extend_from_slice(&b[i..i + 12]);
                i += 12;
                continue;
            }
            if (0xd800..=0xdfff).contains(&cp) {
                out.extend_from_slice(b"\\ufffd");
                fixed += 1;
                i += 6;
                continue;
            }
        }
        // any other escape: copy the backslash and the escaped character together
        out.extend_from_slice(&b[i..(i + 2).min(b.len())]);
        i += 2;
    }
    if fixed == 0 {
        return None;
    }
    log::warn!(
        "{fixed} unpaired UTF-16 surrogate escape(s) (as Node writes them) replaced by U+FFFD; Elasticsearch/OpenSearch cannot store them as text"
    );
    String::from_utf8(out).ok()
}

pub fn encode_line(doc: &Value, enc: LineEncoding) -> Result<String> {
    Ok(match enc {
        LineEncoding::Object => serde_json::to_string(doc)?,
        LineEncoding::StringLiteral => {
            serde_json::to_string(&Value::String(serde_json::to_string(doc)?))?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_literal_round_trip() {
        let v = json!({"i": {"settings": {"index": {"number_of_shards": "1"}}}});
        let line = encode_line(&v, LineEncoding::StringLiteral).unwrap();
        assert!(line.starts_with("\"{\\\"i\\\""));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(decode_any(parsed).unwrap(), v);
        assert_eq!(decode_any(v.clone()).unwrap(), v);
        assert_eq!(
            encode_line(&v, LineEncoding::Object).unwrap(),
            v.to_string()
        );
        assert!(decode_any(json!([1])).is_err());
        assert!(decode_any(json!("[1]")).is_err());
    }
}

/// L-502 (TST-API-081): the double-encoded (string literal) metadata line format.
#[cfg(test)]
mod props {
    use super::*;
    use proptest::prelude::*;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{Config, TestRunner};

    fn text() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                Just('"'),
                Just('\\'),
                Just('/'),
                Just('\n'),
                Just('\u{0}'),
                Just('\u{1f}'),
                Just('\u{7f}'),
                Just('\u{2028}'),
                Just('é'),
                Just('😀'),
                proptest::char::range(' ', '~'),
            ],
            0..10,
        )
        .prop_map(|v| v.into_iter().collect())
    }

    /// Objects as metadata responses hold them. Integers stay within ±2^53
    /// so JavaScript reads them exactly (the Node comparison below).
    fn object() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            (-(1i64 << 53)..(1i64 << 53)).prop_map(Value::from),
            text().prop_map(Value::String),
        ];
        let inner = leaf.prop_recursive(4, 24, 5, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                proptest::collection::vec((text(), inner), 0..4)
                    .prop_map(|kv| Value::Object(kv.into_iter().collect())),
            ]
        });
        proptest::collection::vec((text(), inner), 0..5)
            .prop_map(|kv| Value::Object(kv.into_iter().collect()))
    }

    fn cases() -> u32 {
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000)
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

        #[test]
        fn both_encodings_round_trip_on_one_line(doc in object()) {
            for enc in [LineEncoding::StringLiteral, LineEncoding::Object] {
                let line = encode_line(&doc, enc).unwrap();
                prop_assert!(!line.contains('\n') && !line.contains('\r'));
                let parsed: Value = serde_json::from_str(&line).unwrap();
                prop_assert_eq!(&decode_any(parsed).unwrap(), &doc);
            }
        }
    }

    /// Every metadata line Node elasticdump wrote in the committed corpus
    /// re-encodes byte for byte.
    #[test]
    fn node_corpus_reencodes_byte_equal() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/node");
        let mut checked = 0;
        for dir in std::fs::read_dir(&root).unwrap() {
            let dir = dir.unwrap().path();
            if !dir.is_dir() {
                continue;
            }
            for f in std::fs::read_dir(&dir).unwrap() {
                let f = f.unwrap().path();
                if f.file_name().unwrap() == "data.json"
                    || f.extension().is_none_or(|e| e != "json")
                {
                    continue;
                }
                for line in std::fs::read_to_string(&f).unwrap().lines() {
                    let v: Value = serde_json::from_str(line).unwrap();
                    let enc = if v.is_string() {
                        LineEncoding::StringLiteral
                    } else {
                        LineEncoding::Object
                    };
                    let doc = decode_any(v).unwrap();
                    assert_eq!(encode_line(&doc, enc).unwrap(), line, "{}", f.display());
                    checked += 1;
                }
            }
        }
        assert!(checked >= 20, "{checked} lines");
    }

    /// Random documents: `encode_line(StringLiteral)` equals Node's
    /// `JSON.stringify(JSON.stringify(doc))` byte for byte.
    #[test]
    fn string_literal_matches_node_json_stringify() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("SKIP: node not installed");
            return;
        }
        let n: u32 = std::env::var("CODEC_NODE_DOCS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2_000);
        let mut runner = TestRunner::new(Config::default());
        let strategy = object();
        let docs: Vec<Value> = (0..n)
            .map(|_| strategy.new_tree(&mut runner).unwrap().current())
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("docs.ndjson");
        let mut body = String::new();
        for d in &docs {
            body.push_str(&serde_json::to_string(d).unwrap());
            body.push('\n');
        }
        std::fs::write(&input, body).unwrap();
        let out = std::process::Command::new("node")
            .arg("-e")
            .arg(
                "const fs=require('fs');const ls=fs.readFileSync(process.argv[1],'utf8').split('\\n').filter(Boolean);\
                 process.stdout.write(ls.map(l=>JSON.stringify(JSON.stringify(JSON.parse(l)))).join('\\n')+'\\n');",
            )
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let node = String::from_utf8(out.stdout).unwrap();
        let mut mismatches = 0;
        let mut reordered = 0;
        for (d, want) in docs.iter().zip(node.lines()) {
            let mut got = encode_line(d, LineEncoding::StringLiteral).unwrap();
            if got != want {
                // The one documented difference (NODE-DIFFERENCES): JavaScript
                // objects list integer-like keys first, in numeric order.
                got = encode_line(&js_key_order(d), LineEncoding::StringLiteral).unwrap();
                reordered += 1;
            }
            if got != want {
                mismatches += 1;
                if mismatches <= 3 {
                    eprintln!("rust: {got}\nnode: {want}");
                }
            }
        }
        assert_eq!(node.lines().count(), docs.len());
        assert_eq!(
            mismatches, 0,
            "{reordered} differed only in integer-key order"
        );
    }

    /// Object keys in JavaScript property order: array-index keys
    /// (`0`..`4294967294`, no leading zeros) ascending, then the rest.
    fn js_key_order(v: &Value) -> Value {
        let is_index = |k: &str| {
            !k.is_empty()
                && (k == "0" || !k.starts_with('0'))
                && k.bytes().all(|b| b.is_ascii_digit())
                && k.parse::<u64>().is_ok_and(|n| n < u32::MAX as u64)
        };
        match v {
            Value::Array(a) => Value::Array(a.iter().map(js_key_order).collect()),
            Value::Object(o) => {
                let mut idx: Vec<(&String, &Value)> =
                    o.iter().filter(|(k, _)| is_index(k)).collect();
                idx.sort_by_key(|(k, _)| k.parse::<u64>().unwrap());
                let rest = o.iter().filter(|(k, _)| !is_index(k));
                Value::Object(
                    idx.into_iter()
                        .chain(rest)
                        .map(|(k, v)| (k.clone(), js_key_order(v)))
                        .collect(),
                )
            }
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod surrogate_tests {
    use super::repair_lone_surrogates as r;
    #[test]
    fn lone_surrogates_become_replacement_chars() {
        assert_eq!(r(r#"{"s":"a\ud800b"}"#).unwrap(), r#"{"s":"a\ufffdb"}"#);
        assert_eq!(r(r#"{"s":"\udc00"}"#).unwrap(), r#"{"s":"\ufffd"}"#);
        // a valid pair, an escaped backslash before u, and plain escapes are untouched
        assert_eq!(r(r#"{"s":"\ud83d\ude00"}"#), None);
        assert_eq!(r(r#"{"s":"\\ud800"}"#), None);
        assert_eq!(r(r#"{"s":"\u0000\n"}"#), None);
        assert_eq!(
            r(r#"{"s":"\ud83d\ud83d\ude00"}"#).unwrap(),
            r#"{"s":"\ufffd\ud83d\ude00"}"#
        );
    }
}
