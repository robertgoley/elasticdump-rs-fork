//! File encoding of the metadata types.
//!
//! elasticdump writes every metadata type except `mapping` as one line holding
//! a JSON *string literal* of the document (`JSON.stringify` of the response
//! body), and its restore path can only read that form. elasticdump-rs writes
//! the same form by default (`--metaFormat object` writes a plain object,
//! which Node cannot read) and accepts both on restore.

use std::io::Read;
use std::path::PathBuf;

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
pub fn read_first_value(path: Option<PathBuf>) -> Result<Value> {
    let mut reader = crate::pipeline::file_source::open_reader(path.as_ref())?;
    let mut splitter = ValueSplitter::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut first: Option<Value> = None;
    loop {
        let n = reader.read(&mut buf).context("Failed to read input")?;
        if n > 0 {
            splitter.push(&buf[..n]);
        }
        while let Some(v) = splitter.next_value(n == 0).map_err(|e| anyhow!(e))? {
            if first.is_none() {
                first = Some(serde_json::from_slice(&v).context("Invalid JSON in input")?);
            } else {
                log::warn!(
                    "Input holds more than one JSON value; only the first is used (elasticdump behaviour)"
                );
                return Ok(first.unwrap());
            }
        }
        if n == 0 {
            return first.ok_or_else(|| anyhow!("Input is empty"));
        }
    }
}

/// A metadata file line: a JSON string literal holding the object, or the
/// object itself.
pub fn decode_any(v: Value) -> Result<Value> {
    match v {
        Value::String(s) => {
            let inner: Value =
                serde_json::from_str(&s).context("the string in the input is not JSON")?;
            if !inner.is_object() {
                bail!("the input must hold a JSON object (or a string literal of one)");
            }
            Ok(inner)
        }
        v @ Value::Object(_) => Ok(v),
        _ => bail!("the input must hold a JSON object (or a string literal of one)"),
    }
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
