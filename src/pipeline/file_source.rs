//! Streaming reader of JSON documents from a file or stdin, emitted as
//! NDJSON batches of `limit` documents. gzip input is detected by its magic
//! bytes (multi-member streams supported).

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::json_split::{ValueSplitter, single_line};

#[derive(Debug, Default)]
pub struct FileBatch {
    /// `\n`-terminated JSON documents.
    pub buf: Vec<u8>,
    pub docs: u64,
}

const READ_CHUNK: usize = 256 * 1024;

fn open_reader(path: Option<&PathBuf>) -> Result<Box<dyn Read + Send>> {
    let raw: Box<dyn Read + Send> = match path {
        Some(p) => Box::new(
            std::fs::File::open(p)
                .with_context(|| format!("Failed to open input file '{}'", p.display()))?,
        ),
        None => Box::new(std::io::stdin()),
    };
    let mut buffered = BufReader::with_capacity(READ_CHUNK, raw);
    let head = buffered.fill_buf().context("Failed to read input")?;
    if head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b {
        log::debug!("Input is gzip-compressed");
        Ok(Box::new(flate2::read::MultiGzDecoder::new(buffered)))
    } else {
        Ok(Box::new(buffered))
    }
}

fn read_loop(
    path: Option<PathBuf>,
    limit: usize,
    tx: &mpsc::Sender<Result<FileBatch>>,
    cancel: &CancellationToken,
) -> Result<()> {
    let name = path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "stdin".to_string());
    let mut reader = open_reader(path.as_ref())?;
    let mut splitter = ValueSplitter::new();
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut batch = FileBatch::default();
    let mut eof = false;
    while !eof {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let n = reader
            .read(&mut chunk)
            .with_context(|| format!("Failed to read {name}"))?;
        if n == 0 {
            eof = true;
        } else {
            splitter.push(&chunk[..n]);
        }
        loop {
            let value = splitter
                .next_value(eof)
                .map_err(|e| anyhow!("Invalid JSON in {name}: {e}"))?;
            let Some(value) = value else { break };
            batch.buf.extend_from_slice(&single_line(value));
            batch.buf.push(b'\n');
            batch.docs += 1;
            if batch.docs as usize >= limit {
                let full = std::mem::take(&mut batch);
                if tx.blocking_send(Ok(full)).is_err() {
                    return Ok(());
                }
            }
        }
    }
    if batch.docs > 0 {
        let _ = tx.blocking_send(Ok(batch));
    }
    Ok(())
}

/// Start reading on a blocking thread. The channel closes at end of input;
/// a read or parse error arrives as an `Err` item.
pub fn spawn_file_source(
    path: Option<PathBuf>,
    limit: usize,
    buffer: usize,
    cancel: CancellationToken,
) -> mpsc::Receiver<Result<FileBatch>> {
    let (tx, rx) = mpsc::channel(buffer.max(1));
    tokio::task::spawn_blocking(move || {
        if let Err(e) = read_loop(path, limit, &tx, &cancel) {
            let _ = tx.blocking_send(Err(e));
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    async fn collect(path: PathBuf, limit: usize) -> Vec<(String, u64)> {
        let mut rx = spawn_file_source(Some(path), limit, 2, CancellationToken::new());
        let mut out = vec![];
        while let Some(b) = rx.recv().await {
            let b = b.unwrap();
            out.push((String::from_utf8(b.buf).unwrap(), b.docs));
        }
        out
    }

    #[tokio::test]
    async fn batches_by_limit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("in.json");
        std::fs::write(&p, "{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n").unwrap();
        let got = collect(p, 2).await;
        assert_eq!(
            got,
            vec![
                ("{\"a\":1}\n{\"a\":2}\n".to_string(), 2),
                ("{\"a\":3}\n".to_string(), 1)
            ]
        );
    }

    #[tokio::test]
    async fn reads_multi_member_gzip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("in.json.gz");
        let mut bytes = vec![];
        for part in ["{\"a\":1}\n", "{\"a\":2}\n"] {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(part.as_bytes()).unwrap();
            bytes.extend(e.finish().unwrap());
        }
        std::fs::write(&p, bytes).unwrap();
        let got = collect(p, 10).await;
        assert_eq!(got, vec![("{\"a\":1}\n{\"a\":2}\n".to_string(), 2)]);
    }

    #[tokio::test]
    async fn invalid_json_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.json");
        std::fs::write(&p, "{\"a\":1}\n{\"a\":").unwrap();
        let mut rx = spawn_file_source(Some(p), 10, 2, CancellationToken::new());
        let mut saw_err = false;
        while let Some(b) = rx.recv().await {
            if b.is_err() {
                saw_err = true;
            }
        }
        assert!(saw_err);
    }
}
