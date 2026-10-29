//! File / stdout output: optional gzip (`--fsCompress`, `--compressionLevel`)
//! and splitting into parts (`--fileSize`, `--maxRows`) with elasticdump's
//! part naming. Every part is staged and only committed once the whole dump
//! succeeded, so a failed run leaves no partial split set behind.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use flate2::Compression;
use flate2::write::GzEncoder;

use crate::output::{OutputTarget, SealedOutput, create_output_target};

/// Pending compressed bytes are handed to the file in chunks of this size.
const GZ_DRAIN: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitBy {
    /// Uncompressed line length in bytes, newline excluded (Node counts the
    /// string length of each line).
    Bytes(u64),
    Rows(u64),
}

impl SplitBy {
    fn limit(self) -> u64 {
        match self {
            SplitBy::Bytes(n) | SplitBy::Rows(n) => n.max(1),
        }
    }
}

/// Output compression. gzip is elasticdump's `--fsCompress`; zstd is a Rust
/// extension selected with `--compressionType zstd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Gzip(u32),
    Zstd(i32),
}

impl Codec {
    /// File suffix appended to split parts (Node appends `.gz`).
    pub fn suffix(self) -> &'static str {
        match self {
            Codec::Gzip(_) => ".gz",
            Codec::Zstd(_) => ".zst",
        }
    }
}

enum Encoder {
    Gzip(GzEncoder<Vec<u8>>),
    Zstd(zstd::stream::write::Encoder<'static, Vec<u8>>),
}

impl Encoder {
    fn new(codec: Codec) -> Result<Self> {
        let buf = Vec::with_capacity(GZ_DRAIN * 2);
        Ok(match codec {
            Codec::Gzip(level) => Encoder::Gzip(GzEncoder::new(buf, Compression::new(level))),
            Codec::Zstd(level) => {
                let mut enc =
                    zstd::stream::write::Encoder::new(buf, level).context("zstd encoder")?;
                // Lets readers size their buffers; harmless for streaming.
                enc.include_checksum(true).context("zstd checksum")?;
                Encoder::Zstd(enc)
            }
        })
    }

    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Encoder::Gzip(e) => e.write_all(bytes),
            Encoder::Zstd(e) => e.write_all(bytes),
        }
    }

    fn pending(&mut self) -> &mut Vec<u8> {
        match self {
            Encoder::Gzip(e) => e.get_mut(),
            Encoder::Zstd(e) => e.get_mut(),
        }
    }

    fn finish(self) -> std::io::Result<Vec<u8>> {
        match self {
            Encoder::Gzip(e) => e.finish(),
            Encoder::Zstd(e) => e.finish(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FileOptions {
    /// `None` = stdout.
    pub path: Option<PathBuf>,
    pub overwrite: bool,
    pub source_only: bool,
    /// Compression for file outputs; stdout is never compressed (Node parity).
    pub codec: Option<Codec>,
    pub split: Option<SplitBy>,
    /// `--throttleInterval`: pause after each batch.
    pub throttle: Option<std::time::Duration>,
    /// `--format human`: pretty-printed JSON documents (Node uses
    /// `util.inspect`; pretty JSON stays restorable).
    pub human: bool,
}

/// Parse a size the way the `bytes` npm module does: a number with an
/// optional `b`, `kb`, `mb`, `gb`, `tb` or `pb` unit, base 1024, decimals allowed.
pub fn parse_size(raw: &str) -> Result<u64> {
    let s = raw.trim().to_ascii_lowercase();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '+'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num
        .parse()
        .map_err(|_| anyhow!("invalid size '{raw}' (expected e.g. 512kb, 10mb)"))?;
    let mult: f64 = match unit.trim() {
        "" | "b" => 1.0,
        "kb" => 1024.0,
        "mb" => 1024f64.powi(2),
        "gb" => 1024f64.powi(3),
        "tb" => 1024f64.powi(4),
        "pb" => 1024f64.powi(5),
        u => bail!("invalid size unit '{u}' in '{raw}'"),
    };
    let v = (n * mult).floor();
    if v < 1.0 {
        bail!("size '{raw}' must be at least 1 byte");
    }
    Ok(v as u64)
}

/// `{dir}/{base}.split-{n}{ext}[.gz|.zst]`, as Node's
/// `StreamSplitter.generateFilePath` (`.zst` for the zstd extension).
pub fn part_path(root: &Path, n: u64, codec: Option<Codec>) -> PathBuf {
    let (base, ext) = stem_ext(root);
    let name = format!("{base}.split-{n}{ext}{}", codec.map_or("", Codec::suffix));
    match root.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(name),
        _ => PathBuf::from(name),
    }
}

/// Node's `path.extname` / `path.basename(p, ext)`: a leading dot does not
/// start an extension.
fn stem_ext(root: &Path) -> (String, String) {
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name, String::new()),
    }
}

fn existing_parts(root: &Path) -> Vec<PathBuf> {
    let (base, _) = stem_ext(root);
    let prefix = format!("{base}.split-");
    let dir = match root.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

/// Pretty-print each line; numbers keep their exact text.
fn humanize(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len() * 2);
    for line in buf.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        match serde_json::from_slice::<serde_json::Value>(line) {
            Ok(v) => {
                let s = serde_json::to_string_pretty(&v).expect("serialize");
                out.extend_from_slice(s.as_bytes());
            }
            Err(_) => out.extend_from_slice(line),
        }
        out.push(b'\n');
    }
    out
}

struct Part {
    target: OutputTarget,
    enc: Option<Encoder>,
    count: u64,
}

impl Part {
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        match &mut self.enc {
            Some(enc) => {
                enc.write_all(bytes).context("compress")?;
                if enc.pending().len() >= GZ_DRAIN {
                    let out = std::mem::take(enc.pending());
                    self.target.write_all(&out).await?;
                }
                Ok(())
            }
            None => self.target.write_all(bytes).await,
        }
    }

    async fn flush(&mut self) -> Result<()> {
        if let Some(enc) = &mut self.enc {
            let out = std::mem::take(enc.pending());
            if !out.is_empty() {
                self.target.write_all(&out).await?;
            }
        }
        self.target.flush().await
    }

    async fn seal(mut self) -> Result<Option<SealedOutput>> {
        if let Some(enc) = self.enc.take() {
            let out = enc.finish().context("compress")?;
            self.target.write_all(&out).await?;
        }
        self.target.seal().await
    }
}

pub struct FileSink {
    opts: FileOptions,
    current: Option<Part>,
    sealed: Vec<SealedOutput>,
    next_part: u64,
    pub(crate) scratch: Vec<u8>,
    pub(crate) written: u64,
}

impl FileSink {
    pub async fn create(opts: FileOptions) -> Result<Self> {
        let mut sink = Self {
            opts,
            current: None,
            sealed: Vec::new(),
            next_part: 0,
            scratch: Vec::new(),
            written: 0,
        };
        match (&sink.opts.path, sink.opts.split) {
            (Some(root), Some(_)) => {
                if !sink.opts.overwrite {
                    if let Some(p) = existing_parts(root).first() {
                        bail!(
                            "File `{}` already exists, quitting (use --overwrite to replace split parts)",
                            p.display()
                        );
                    }
                }
            }
            (None, Some(_)) => bail!("--fileSize/--maxRows require a file --output"),
            _ => sink.current = Some(sink.open(sink.opts.path.clone()).await?),
        }
        Ok(sink)
    }

    pub fn source_only(&self) -> bool {
        self.opts.source_only
    }

    pub fn is_stdout(&self) -> bool {
        self.opts.path.is_none()
    }

    async fn open(&self, path: Option<PathBuf>) -> Result<Part> {
        let enc = match (&path, self.opts.codec) {
            (Some(_), Some(codec)) => Some(Encoder::new(codec)?),
            _ => None,
        };
        Ok(Part {
            target: create_output_target(path.as_deref(), self.opts.overwrite).await?,
            enc,
            count: 0,
        })
    }

    async fn rotate(&mut self) -> Result<()> {
        if let Some(part) = self.current.take() {
            if let Some(sealed) = part.seal().await? {
                log::info!("Created {}", sealed.final_path().display());
                self.sealed.push(sealed);
            }
        }
        Ok(())
    }

    /// Write one batch of complete NDJSON lines, then honour `--throttleInterval`.
    pub async fn write_batch(&mut self, buf: &[u8]) -> Result<()> {
        if self.opts.human {
            let pretty = humanize(buf);
            self.write_lines(&pretty).await?;
        } else {
            self.write_lines(buf).await?;
        }
        if let Some(d) = self.opts.throttle {
            tokio::time::sleep(d).await;
        }
        Ok(())
    }

    /// Write complete NDJSON lines.
    pub async fn write_lines(&mut self, buf: &[u8]) -> Result<()> {
        let Some(split) = self.opts.split else {
            let part = self.current.as_mut().expect("unsplit output is open");
            return part.write(buf).await;
        };
        let limit = split.limit();
        let mut start = 0;
        while start < buf.len() {
            if self.current.is_none() {
                let root = self.opts.path.clone().expect("split output has a path");
                let path = part_path(&root, self.next_part, self.opts.codec);
                self.next_part += 1;
                self.current = Some(self.open(Some(path)).await?);
            }
            let part = self.current.as_mut().unwrap();
            // Take lines until the part reaches its limit.
            let mut end = start;
            let mut full = false;
            while end < buf.len() {
                let nl = memchr::memchr(b'\n', &buf[end..]).map_or(buf.len(), |i| end + i);
                part.count += match split {
                    SplitBy::Bytes(_) => (nl - end) as u64,
                    SplitBy::Rows(_) => 1,
                };
                end = (nl + 1).min(buf.len());
                if part.count >= limit {
                    full = true;
                    break;
                }
            }
            part.write(&buf[start..end]).await?;
            start = end;
            if full {
                self.rotate().await?;
            }
        }
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<()> {
        match &mut self.current {
            Some(p) => p.flush().await,
            None => Ok(()),
        }
    }

    /// Seal the open part and commit every staged part, in order.
    pub async fn finalize(mut self) -> Result<()> {
        if let Err(e) = self.rotate().await {
            for s in self.sealed.drain(..) {
                s.discard().await;
            }
            return Err(e);
        }
        let mut sealed = std::mem::take(&mut self.sealed).into_iter();
        while let Some(s) = sealed.next() {
            if let Err(e) = s.commit().await {
                for rest in sealed {
                    rest.discard().await;
                }
                return Err(e);
            }
        }
        Ok(())
    }

    pub async fn abort(mut self) {
        if let Some(p) = self.current.take() {
            p.target.abort().await;
        }
        for s in self.sealed.drain(..) {
            s.discard().await;
        }
    }
}

#[cfg(test)]
#[allow(clippy::format_collect)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse_like_the_bytes_module() {
        assert_eq!(parse_size("64kb").unwrap(), 65536);
        assert_eq!(parse_size("10MB").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("1.5kb").unwrap(), 1536);
        assert_eq!(parse_size("100").unwrap(), 100);
        assert_eq!(parse_size("2 gb").unwrap(), 2 * 1024 * 1024 * 1024);
        assert!(parse_size("ten").is_err());
        assert!(parse_size("5xb").is_err());
    }

    #[test]
    fn part_names_match_node() {
        let gz = Some(Codec::Gzip(6));
        let zst = Some(Codec::Zstd(3));
        let cases = [
            ("out.json", 0, None, "out.split-0.json"),
            ("/d/out.json", 3, gz, "/d/out.split-3.json.gz"),
            ("/d/out.json", 3, zst, "/d/out.split-3.json.zst"),
            ("/d/a.b.ndjson", 1, None, "/d/a.b.split-1.ndjson"),
            ("/d/noext", 2, None, "/d/noext.split-2"),
            ("/d/.hidden", 0, None, "/d/.hidden.split-0"),
        ];
        for (root, n, codec, want) in cases {
            assert_eq!(part_path(Path::new(root), n, codec), PathBuf::from(want));
        }
    }

    fn tmpdir(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("edrs-fs-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn read_maybe_gz(p: &Path) -> String {
        let mut s = String::new();
        std::io::Read::read_to_string(
            &mut crate::pipeline::file_source::open_reader(Some(&p.to_path_buf())).unwrap(),
            &mut s,
        )
        .unwrap();
        let raw = std::fs::read(p).unwrap();
        if raw.starts_with(&crate::pipeline::file_source::ZSTD_MAGIC) {
            return s;
        }
        if raw.starts_with(&[0x1f, 0x8b]) {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut flate2::read::MultiGzDecoder::new(&raw[..]), &mut s)
                .unwrap();
            s
        } else {
            String::from_utf8(raw).unwrap()
        }
    }

    #[tokio::test]
    async fn split_by_rows_and_bytes_with_and_without_gzip() {
        let lines: String = (0..25).map(|i| format!("{{\"n\":{i:04}}}\n")).collect(); // 10 bytes + \n
        for (split, gz, parts) in [
            (SplitBy::Rows(10), None, 3),
            (SplitBy::Bytes(35), Some(Codec::Gzip(6)), 7), // 4 lines (40 bytes) per part
            (SplitBy::Rows(25), Some(Codec::Gzip(1)), 1),
            (SplitBy::Rows(4), Some(Codec::Zstd(3)), 7),
            (SplitBy::Bytes(35), Some(Codec::Zstd(19)), 7),
        ] {
            let d = tmpdir(&format!("{split:?}{gz:?}").replace(['(', ')'], ""));
            let root = d.join("out.json");
            let mut sink = FileSink::create(FileOptions {
                path: Some(root.clone()),
                codec: gz,
                split: Some(split),
                ..Default::default()
            })
            .await
            .unwrap();
            // batches of 7 lines so rotation happens mid-buffer
            let mut i = 0;
            let ls: Vec<&str> = lines.split_inclusive('\n').collect();
            while i < ls.len() {
                let batch: String = ls[i..(i + 7).min(ls.len())].concat();
                sink.write_lines(batch.as_bytes()).await.unwrap();
                i += 7;
            }
            assert!(!root.exists());
            sink.finalize().await.unwrap();
            let mut all = String::new();
            for n in 0..parts {
                let p = part_path(&root, n, gz);
                if let Some(Codec::Zstd(_)) = gz {
                    assert!(
                        std::fs::read(&p)
                            .unwrap()
                            .starts_with(&crate::pipeline::file_source::ZSTD_MAGIC)
                    );
                }
                let content = read_maybe_gz(&p);
                if n + 1 < parts {
                    let c = match split {
                        SplitBy::Rows(_) => content.lines().count() as u64,
                        SplitBy::Bytes(_) => content.lines().map(|l| l.len() as u64).sum(),
                    };
                    assert!(
                        c >= split.limit() && c < split.limit() + 10,
                        "{split:?} part {n}: {c}"
                    );
                }
                all.push_str(&content);
            }
            assert!(!part_path(&root, parts, gz).exists());
            assert_eq!(all, lines);
            // existing parts block a second run without --overwrite
            assert!(
                FileSink::create(FileOptions {
                    path: Some(root.clone()),
                    split: Some(split),
                    codec: gz,
                    ..Default::default()
                })
                .await
                .is_err()
            );
            std::fs::remove_dir_all(&d).unwrap();
        }
    }

    #[tokio::test]
    async fn abort_leaves_nothing_behind() {
        let d = tmpdir("abort");
        let root = d.join("out.json");
        let mut sink = FileSink::create(FileOptions {
            path: Some(root.clone()),
            split: Some(SplitBy::Rows(1)),
            ..Default::default()
        })
        .await
        .unwrap();
        sink.write_lines(b"{}\n{}\n{}\n").await.unwrap();
        sink.abort().await;
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn single_gzip_file() {
        let d = tmpdir("gz1");
        let root = d.join("out.json");
        let mut sink = FileSink::create(FileOptions {
            path: Some(root.clone()),
            codec: Some(Codec::Gzip(9)),
            ..Default::default()
        })
        .await
        .unwrap();
        sink.write_lines(b"{\"a\":1}\n").await.unwrap();
        sink.flush().await.unwrap();
        sink.write_lines(b"{\"a\":2}\n").await.unwrap();
        sink.finalize().await.unwrap();
        assert_eq!(read_maybe_gz(&root), "{\"a\":1}\n{\"a\":2}\n");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn single_zstd_file_and_concatenated_frames() {
        let d = tmpdir("zst1");
        let root = d.join("out.json.zst");
        let mut sink = FileSink::create(FileOptions {
            path: Some(root.clone()),
            codec: Some(Codec::Zstd(3)),
            ..Default::default()
        })
        .await
        .unwrap();
        let body: String = (0..5000).map(|i| format!("{{\"i\":{i}}}\n")).collect();
        sink.write_lines(body.as_bytes()).await.unwrap();
        sink.flush().await.unwrap();
        sink.write_lines(b"{\"last\":true}\n").await.unwrap();
        sink.finalize().await.unwrap();
        let raw = std::fs::read(&root).unwrap();
        assert!(raw.starts_with(&crate::pipeline::file_source::ZSTD_MAGIC));
        assert!(raw.len() < body.len() / 4, "compressed");
        let want = format!("{body}{{\"last\":true}}\n");
        assert_eq!(read_maybe_gz(&root), want);
        // two complete streams back to back (e.g. `cat a.zst b.zst`) read as one
        let cat = d.join("cat.zst");
        std::fs::write(&cat, [raw.clone(), raw].concat()).unwrap();
        assert_eq!(read_maybe_gz(&cat), format!("{want}{want}"));
        // a .zst name over plain bytes is a clear error
        let fake = d.join("plain.zst");
        std::fs::write(&fake, "{}\n").unwrap();
        assert!(crate::pipeline::file_source::open_reader(Some(&fake)).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
