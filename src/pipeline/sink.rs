//! Output sinks. Every source produces batches of `\n`-terminated JSON
//! documents (Elasticsearch hits or file values); sinks consume them.

use std::path::PathBuf;

use anyhow::Result;
use sonic_rs::JsonValueTrait;

use crate::output::{OutputTarget, create_output_target};

pub enum Sink {
    File(FileSink),
}

pub struct FileSink {
    target: OutputTarget,
    source_only: bool,
    scratch: Vec<u8>,
}

impl FileSink {
    pub async fn create(path: Option<PathBuf>, overwrite: bool, source_only: bool) -> Result<Self> {
        Ok(Self {
            target: create_output_target(path.as_deref(), overwrite).await?,
            source_only,
            scratch: Vec::new(),
        })
    }
}

/// Lazily located `_source` of one hit line, if present.
pub fn source_of(line: &[u8]) -> Option<sonic_rs::LazyValue<'_>> {
    sonic_rs::get(line, ["_source"]).ok()
}

fn source_only_lines(buf: &[u8], out: &mut Vec<u8>) {
    out.clear();
    for line in buf.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        match source_of(line) {
            Some(src) => out.extend_from_slice(src.as_raw_str().as_bytes()),
            None => {
                // Not a hit (e.g. file→file of plain documents): keep as-is.
                let v: Option<sonic_rs::Value> = sonic_rs::from_slice(line).ok();
                if v.as_ref().is_some_and(|v| v.get("_source").is_some()) {
                    log::debug!("Could not extract _source; writing document unchanged");
                }
                out.extend_from_slice(line);
            }
        }
        out.push(b'\n');
    }
}

impl Sink {
    pub async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        match self {
            Sink::File(f) => {
                if f.source_only {
                    let mut scratch = std::mem::take(&mut f.scratch);
                    source_only_lines(buf, &mut scratch);
                    let r = f.target.write_all(&scratch).await;
                    f.scratch = scratch;
                    r
                } else {
                    f.target.write_all(buf).await
                }
            }
        }
    }

    pub async fn flush(&mut self) -> Result<()> {
        match self {
            Sink::File(f) => f.target.flush().await,
        }
    }

    pub fn is_stdout(&self) -> bool {
        match self {
            Sink::File(f) => f.target.is_stdout(),
        }
    }

    pub async fn finalize(self) -> Result<()> {
        match self {
            Sink::File(f) => f.target.finalize().await,
        }
    }

    pub async fn abort(self) {
        match self {
            Sink::File(f) => f.target.abort().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_only_extracts_raw_source() {
        let mut out = vec![];
        source_only_lines(
            b"{\"_index\":\"i\",\"_id\":\"1\",\"_source\":{\"n\":12345678901234567890123,\"s\":\"\\u00e9\"}}\n{\"plain\":1}\n",
            &mut out,
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\"n\":12345678901234567890123,\"s\":\"\\u00e9\"}\n{\"plain\":1}\n"
        );
    }
}
