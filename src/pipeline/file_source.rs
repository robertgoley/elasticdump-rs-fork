//! Streaming reader of JSON documents from a file or stdin, emitted as
//! NDJSON batches of `limit` documents. gzip input is detected by its magic
//! bytes (multi-member streams supported).

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// zstd frame magic number (little-endian 0xFD2FB528).
pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

pub fn open_reader(path: Option<&PathBuf>) -> Result<Box<dyn Read + Send>> {
    open_reader_counted(path, Arc::new(AtomicU64::new(0)))
}

/// Counts the bytes read from the underlying (possibly compressed) input.
struct Counted<R> {
    inner: R,
    n: Arc<AtomicU64>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let k = self.inner.read(buf)?;
        self.n.fetch_add(k as u64, Ordering::Relaxed);
        Ok(k)
    }
}

/// [`open_reader`], with `raw_bytes` counting the bytes taken from the file itself, so a
/// decode error can name the compressed offset (TST-BRK-IN-001/002).
pub fn open_reader_counted(
    path: Option<&PathBuf>,
    raw_bytes: Arc<AtomicU64>,
) -> Result<Box<dyn Read + Send>> {
    let raw: Box<dyn Read + Send> = match path {
        Some(p) => Box::new(
            std::fs::File::open(p)
                .with_context(|| format!("Failed to open input file '{}'", p.display()))?,
        ),
        None => Box::new(std::io::stdin()),
    };
    let raw: Box<dyn Read + Send> = Box::new(Counted {
        inner: raw,
        n: raw_bytes,
    });
    decode_input(raw, path.map(|p| p.as_path()))
}

/// Largest zstd window accepted on input (2^27 = 128 MiB, libzstd's own default
/// limit, set explicitly): a frame that declares more is refused before any
/// memory is reserved for it (TST-SUP-020).
pub const ZSTD_WINDOW_LOG_MAX: u32 = 27;

/// Wraps raw input bytes in the right decoder, chosen by magic bytes: gzip
/// (multi-member), zstd (concatenated and skippable frames) or plain.
pub fn decode_input(
    raw: Box<dyn Read + Send>,
    path: Option<&std::path::Path>,
) -> Result<Box<dyn Read + Send>> {
    let mut buffered = BufReader::with_capacity(READ_CHUNK, raw);
    let head = buffered.fill_buf().context("Failed to read input")?;
    let name = path
        .map(|p| format!("'{}'", p.display()))
        .unwrap_or_else(|| "stdin".into());
    if let Some(kind) = unsupported_format(head) {
        anyhow::bail!(
            "Input {name} is {kind}, which is not supported; decompress it first (only plain, gzip and zstd input are read)"
        );
    }
    if head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b {
        log::debug!("Input is gzip-compressed");
        no_tar(
            Box::new(flate2::read::MultiGzDecoder::new(buffered)),
            &name,
            "gzip",
        )
    } else if head.starts_with(&ZSTD_MAGIC) {
        log::debug!("Input is zstd-compressed");
        // Concatenated frames (e.g. `cat` of parts) are decoded in sequence.
        let mut d = zstd::stream::read::Decoder::with_buffer(buffered).context("zstd decoder")?;
        d.window_log_max(ZSTD_WINDOW_LOG_MAX)
            .context("zstd decoder")?;
        no_tar(Box::new(d), &name, "zstd")
    } else if is_tar(head) {
        anyhow::bail!(
            "Input {name} is a tar archive, which is not supported; extract the dump file first"
        )
    } else if path.is_some_and(|p| p.extension().is_some_and(|e| e == "zst" || e == "zstd"))
        && !head.is_empty()
    {
        anyhow::bail!(
            "Input '{}' has a .zst extension but is not a zstd stream",
            path.unwrap().display()
        )
    } else {
        Ok(Box::new(buffered))
    }
}

/// Compression and archive formats recognised by magic bytes but not read (TST-BRK-IN-003),
/// e.g. the Rally corpora ship as bzip2.
fn unsupported_format(head: &[u8]) -> Option<&'static str> {
    const MAGIC: &[(&[u8], &str)] = &[
        (b"BZh", "bzip2-compressed"),
        (&[0xfd, b'7', b'z', b'X', b'Z', 0x00], "xz-compressed"),
        (&[0x5d, 0x00, 0x00], "lzma-compressed"),
        (&[0x04, 0x22, 0x4d, 0x18], "lz4-compressed"),
        (b"PK\x03\x04", "a zip archive"),
        (&[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c], "a 7z archive"),
        (b"\x89LZO", "lzop-compressed"),
    ];
    MAGIC
        .iter()
        .find(|(m, _)| head.starts_with(m))
        .map(|(_, k)| *k)
}

fn is_tar(head: &[u8]) -> bool {
    head.len() >= 262 && &head[257..262] == b"ustar"
}

/// Refuses a compressed tar archive (`.tar.gz`, `.tar.zst`): its member headers and padding
/// would otherwise be read as document text (TST-BRK-IN-003).
fn no_tar(inner: Box<dyn Read + Send>, name: &str, codec: &str) -> Result<Box<dyn Read + Send>> {
    let mut b = BufReader::with_capacity(READ_CHUNK, inner);
    match b.fill_buf() {
        Ok(head) if is_tar(head) => anyhow::bail!(
            "Input {name} is a {codec}-compressed tar archive, which is not supported; extract the dump file first"
        ),
        Ok(_) => Ok(Box::new(b)),
        // a decode error is handed to the first read, where it gets the offsets context
        Err(e) => Ok(Box::new(FirstReadError(Some(e)))),
    }
}

struct FirstReadError(Option<std::io::Error>);

impl Read for FirstReadError {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        match self.0.take() {
            Some(e) => Err(e),
            None => Ok(0),
        }
    }
}

fn read_loop(
    path: Option<PathBuf>,
    limit: usize,
    max_doc: Option<usize>,
    tx: &mpsc::Sender<Result<FileBatch>>,
    cancel: &CancellationToken,
) -> Result<()> {
    let name = path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "stdin".to_string());
    let raw_bytes = Arc::new(AtomicU64::new(0));
    let mut reader = open_reader_counted(path.as_ref(), Arc::clone(&raw_bytes))?;
    let mut decoded = 0u64;
    let mut docs_read = 0u64;
    let mut splitter = ValueSplitter::with_max_value(max_doc);
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut batch = FileBatch::default();
    let mut eof = false;
    while !eof {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let n = reader.read(&mut chunk).map_err(|e| {
            let hint = if e.to_string().contains("too much memory") {
                format!(
                    "; zstd frames with a window over 2^{ZSTD_WINDOW_LOG_MAX} bytes are refused (recompress without --long or with --long={ZSTD_WINDOW_LOG_MAX})"
                )
            } else {
                String::new()
            };
            // the BufReader reads ahead, so the offset is where reading had got to
            anyhow!(
                "Failed to read {name}: {e}{hint} (truncated or corrupt input: stopped near byte {} of the file, after {decoded} decoded bytes and {docs_read} complete documents; documents of earlier batches may already have been written)",
                raw_bytes.load(Ordering::Relaxed)
            )
        })?;
        decoded += n as u64;
        if n == 0 {
            eof = true;
        } else {
            splitter.push(&chunk[..n]);
        }
        loop {
            let value = splitter
                .next_value(eof)
                .map_err(|e| {
                    if e.starts_with("a single document is larger") {
                        anyhow!(
                            "{name}: document {} of the input: {e}; the {docs_read} documents before it were read. Raise --maxDocMb (0 = unlimited) if the destination accepts requests that big (Elasticsearch/OpenSearch refuse bodies over http.max_content_length, 100 MB by default), or remove the document from the input",
                            docs_read + 1
                        )
                    } else {
                        anyhow!("Invalid JSON in {name}: {e}")
                    }
                })?;
            let Some(value) = value else { break };
            let line = single_line(value);
            if docs_read == 0 {
                if let Some(found) = crate::meta::metadata_dump_type(&line) {
                    return Err(anyhow::Error::new(crate::UsageError(format!(
                        "{name}: this input looks like a --type {found} dump, not --type data (its first line is {found} metadata, which would be indexed as a document); use --type {found}. Nothing was written"
                    ))));
                }
            }
            batch.buf.extend_from_slice(&line);
            batch.buf.push(b'\n');
            batch.docs += 1;
            docs_read += 1;
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
    max_doc: Option<usize>,
    buffer: usize,
    cancel: CancellationToken,
) -> mpsc::Receiver<Result<FileBatch>> {
    let (tx, rx) = mpsc::channel(buffer.max(1));
    tokio::task::spawn_blocking(move || {
        if let Err(e) = read_loop(path, limit, max_doc, &tx, &cancel) {
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
        let mut rx = spawn_file_source(Some(path), limit, None, 2, CancellationToken::new());
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
        let mut rx = spawn_file_source(Some(p), 10, None, 2, CancellationToken::new());
        let mut saw_err = false;
        while let Some(b) = rx.recv().await {
            if b.is_err() {
                saw_err = true;
            }
        }
        assert!(saw_err);
    }

    /// L-447 (TST-BRK-IN-001/002): a gzip or zstd file cut anywhere before its end is an
    /// error naming the byte offset, never a clean (silently partial) end of input.
    #[tokio::test]
    async fn truncated_gzip_and_zstd_are_errors_with_an_offset() {
        let mut plain = Vec::new();
        for i in 0..3000 {
            writeln!(
                plain,
                "{{\"_id\":\"{i}\",\"_source\":{{\"a\":{i},\"t\":\"text {i}\"}}}}"
            )
            .unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&plain).unwrap();
        let gz = gz.finish().unwrap();
        let zst = zstd::stream::encode_all(&plain[..], 3).unwrap();
        let dir = tempfile::tempdir().unwrap();
        for (ext, full) in [("gz", gz), ("zst", zst)] {
            let step = (full.len() / 40).max(1);
            for cut in (2..full.len()).step_by(step) {
                let p = dir.path().join(format!("t{cut}.json.{ext}"));
                std::fs::write(&p, &full[..cut]).unwrap();
                let mut rx = spawn_file_source(Some(p), 500, None, 2, CancellationToken::new());
                let mut err = None;
                let mut docs = 0;
                while let Some(b) = rx.recv().await {
                    match b {
                        Ok(b) => docs += b.docs,
                        Err(e) => err = Some(format!("{e:#}")),
                    }
                }
                let err =
                    err.unwrap_or_else(|| panic!("{ext} cut at {cut}: no error, {docs} docs"));
                // shorter than the zstd magic number: refused as "not a zstd stream"
                if cut >= 4 {
                    assert!(err.contains("stopped near byte"), "{ext} {cut}: {err}");
                }
                assert!(docs < 3000, "{ext} {cut}");
            }
            // trailing garbage after the last frame/member is an error too
            let p = dir.path().join(format!("garbage.json.{ext}"));
            std::fs::write(&p, [&full[..], b"garbage after the stream"].concat()).unwrap();
            let mut rx = spawn_file_source(Some(p), 500, None, 2, CancellationToken::new());
            let mut failed = false;
            while let Some(b) = rx.recv().await {
                failed |= b.is_err();
            }
            assert!(failed, "{ext}: trailing garbage accepted");
            let p = dir.path().join(format!("full.json.{ext}"));
            std::fs::write(&p, &full).unwrap();
            let total: u64 = collect(p, 500).await.iter().map(|(_, n)| n).sum();
            assert_eq!(total, 3000, "{ext}");
        }
    }
}
