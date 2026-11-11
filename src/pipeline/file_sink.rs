//! File / stdout output: optional gzip (`--fsCompress`, `--compressionLevel`)
//! and splitting into parts (`--fileSize`, `--maxRows`) with elasticdump's
//! part naming. Every part is staged and only committed once the whole dump
//! succeeded, so a failed run leaves no partial split set behind.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use flate2::Compression;
use flate2::write::GzEncoder;

use crate::output::{OutputTarget, SealedOutput, create_output_target, create_remote_target};

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
    /// L-809a: write through OpenDAL instead of the local file system. `path` is then the
    /// object key (split parts are named from it the same way).
    pub remote: Option<crate::remote::Remote>,
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

/// Part count that triggers the "many parts" warning (TST-BRK-RES-006).
const MANY_PARTS: u64 = 10_000;

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

/// D-5 (TST-BRK-SIG-003): files a killed run (`kill -9`, the OOM killer, a power cut) left
/// next to this output, for the output itself and for any of its split parts: staged files
/// that were never committed (`.name.part-PID-NANOS`) and originals an `--overwrite` commit
/// had moved aside (`.name.edrs-replaced-PID`). Found before this run stages anything.
fn leftovers_of(root: &Path) -> Vec<PathBuf> {
    let Some(name) = root.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return vec![];
    };
    let (base, ext) = stem_ext(root);
    let dir = match root.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let split_prefix = format!("{base}.split-");
    let suffixes = [ext.clone(), format!("{ext}.gz"), format!("{ext}.zst")];
    let ours = |committed: &str| {
        committed == name
            || committed.strip_prefix(&split_prefix).is_some_and(|r| {
                let d = r.bytes().take_while(u8::is_ascii_digit).count();
                d > 0 && suffixes.iter().any(|x| x == &r[d..])
            })
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let Some(hidden) = n.strip_prefix('.') else {
                return false;
            };
            if let Some(i) = hidden.rfind(".part-") {
                hidden[i + 6..]
                    .split_once('-')
                    .is_some_and(|(pid, ts)| digits(pid) && digits(ts))
                    && ours(&hidden[..i])
            } else if let Some(i) = hidden.rfind(".edrs-replaced-") {
                digits(&hidden[i + 15..]) && ours(&hidden[..i])
            } else {
                false
            }
        })
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

/// The PID a staging or moved-aside file name carries.
fn leftover_pid(name: &str) -> Option<u32> {
    if let Some(i) = name.rfind(".part-") {
        name[i + 6..].split('-').next()?.parse().ok()
    } else {
        let i = name.rfind(".edrs-replaced-")?;
        name[i + 15..].parse().ok()
    }
}

/// D-5: name every leftover of an earlier killed run at the start of the next run to the
/// same output. They are never deleted or overwritten here: a staged file may hold the only
/// copy of data, and a moved-aside file is the previous contents of an `--overwrite` target.
fn warn_about_leftovers(root: &Path, overwrite: bool) {
    if let Some(msg) = leftover_warning(root, overwrite) {
        log::warn!("{msg}");
    }
}

/// The D-5 warning text, `None` when nothing was left. With `--overwrite` it also says that
/// the overwrite does not remove them: only the output itself (and its parts) is replaced.
fn leftover_warning(root: &Path, overwrite: bool) -> Option<String> {
    let found = leftovers_of(root);
    if found.is_empty() {
        return None;
    }
    let mut list = String::new();
    for p in &found {
        let n = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let what = if n.contains(".edrs-replaced-") {
            "the previous contents an --overwrite run had moved aside; rename it back if its file is missing"
        } else {
            "staged output that was never committed; it may be incomplete"
        };
        let running = leftover_pid(&n)
            .filter(|pid| *pid != std::process::id())
            .filter(|pid| Path::new(&format!("/proc/{pid}")).exists())
            .map(|pid| {
                format!(
                    "; process {pid} is still running, so another run may be writing this output"
                )
            })
            .unwrap_or_default();
        if !list.is_empty() {
            list.push_str("; ");
        }
        list.push_str(&p.display().to_string());
        list.push_str(" (");
        list.push_str(&size.to_string());
        list.push_str(" bytes, ");
        list.push_str(what);
        list.push_str(&running);
        list.push(')');
    }
    let overwrite_note = if overwrite {
        " (--overwrite replaces only the output itself; it does not remove these files)"
    } else {
        ""
    };
    Some(format!(
        "{} file(s) next to --output '{}' were left by an earlier run that was killed before it finished (kill -9, out of memory, power loss); they are left in place, not deleted{overwrite_note}: {list}. Remove them once you have checked them",
        found.len(),
        root.display()
    ))
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
    /// Named by a forced exit (second SIGINT) until the sink is committed or aborted.
    _uncommitted: Option<crate::signals::ForceQuitNote>,
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
            _uncommitted: None,
        };
        if let (Some(root), Some(r)) = (&sink.opts.path, &sink.opts.remote) {
            sink._uncommitted = Some(crate::signals::note_on_force_quit(format!(
                "--output {} was not finished: an upload in progress is not completed, but split parts already uploaded stay in place",
                r.display_key(&root.to_string_lossy())
            )));
        } else if let Some(root) = &sink.opts.path {
            warn_about_leftovers(root, sink.opts.overwrite);
            sink._uncommitted = Some(crate::signals::note_on_force_quit(format!(
                "--output '{}' was not committed (if the force-quit hit its final commit, a split set may be partly committed); its hidden staging files are left next to it and the next run to it names them",
                root.display()
            )));
        }
        match (&sink.opts.path, sink.opts.split) {
            (Some(_), Some(_)) if sink.opts.remote.is_some() => {}
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
        let target = match (&self.opts.remote, &path) {
            (Some(r), Some(key)) => {
                let r = std::sync::Arc::new(r.with_key(key.to_string_lossy().into_owned()));
                create_remote_target(r, self.opts.overwrite).await?
            }
            _ => create_output_target(path.as_deref(), self.opts.overwrite).await?,
        };
        Ok(Part {
            target,
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
                if self.next_part == MANY_PARTS {
                    log::warn!(
                        "the split has reached {MANY_PARTS} parts; every part stays staged until the run ends, so a very small --fileSize/--maxRows costs one file (and inode) per part and a long commit at the end; consider a larger split size"
                    );
                }
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
        let sealed = std::mem::take(&mut self.sealed);
        if self.opts.remote.is_some() {
            // every object is stored as its writer closes; nothing to move into place
            return Ok(());
        }
        commit_all(sealed).await?;
        if let (Some(root), Some(_)) = (&self.opts.path, self.opts.split) {
            let stale: Vec<PathBuf> = existing_parts(root)
                .into_iter()
                .filter(|p| !(0..self.next_part).any(|n| part_path(root, n, self.opts.codec) == *p))
                .collect();
            if !stale.is_empty() {
                log::warn!(
                    "{} older split part(s) were left in place and are not part of this dump (remove them before reading the set back): {}",
                    stale.len(),
                    stale
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
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

/// Commit every staged part, or none: files that would be replaced
/// (`--overwrite`) are first moved aside, and if any commit fails the parts
/// already committed are removed, the rest discarded and the moved-aside files
/// restored. Renames within one directory need no free space, so a full disk
/// cannot strike half-way through.
async fn commit_all(sealed: Vec<SealedOutput>) -> Result<()> {
    let finals: Vec<PathBuf> = sealed
        .iter()
        .filter(|s| s.replaces())
        .map(|s| s.final_path().to_path_buf())
        .collect();
    let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new();
    let restore = |backups: Vec<(PathBuf, PathBuf)>| async move {
        for (orig, bak) in backups {
            if let Err(e) = tokio::fs::rename(&bak, &orig).await {
                log::error!(
                    "could not restore '{}' from '{}': {e}",
                    orig.display(),
                    bak.display()
                );
            }
        }
    };
    for f in &finals {
        let is_file = tokio::fs::symlink_metadata(f)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false);
        if !is_file {
            continue;
        }
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bak = f.with_file_name(format!(".{name}.edrs-replaced-{}", std::process::id()));
        if let Err(e) = tokio::fs::rename(f, &bak).await {
            restore(backups).await;
            for s in sealed {
                s.discard().await;
            }
            return Err(anyhow!(
                "could not move '{}' aside to replace it: {e}; nothing was committed and existing files are unchanged",
                f.display()
            ));
        }
        backups.push((f.clone(), bak));
    }
    let mut committed: Vec<PathBuf> = Vec::new();
    let mut iter = sealed.into_iter();
    while let Some(s) = iter.next() {
        let path = s.final_path().to_path_buf();
        if let Err(e) = s.commit().await {
            for rest in iter {
                rest.discard().await;
            }
            for c in &committed {
                let _ = tokio::fs::remove_file(c).await;
            }
            restore(backups).await;
            return Err(e.context(format!(
                "nothing was committed: {} part(s) already moved into place were removed again and existing files are unchanged",
                committed.len()
            )));
        }
        committed.push(path);
    }
    for (_, bak) in backups {
        let _ = tokio::fs::remove_file(&bak).await;
    }
    Ok(())
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

    /// L-809a: the OpenDAL path (fs service in a temp dir): split parts with compression are
    /// stored under their keys, read back through the remote reader, and an aborted run
    /// leaves nothing behind.
    #[cfg(feature = "opendal")]
    #[tokio::test(flavor = "multi_thread")]
    async fn opendal_fs_split_compressed_round_trip_and_abort() {
        let d = tempfile::tempdir().unwrap();
        let url = format!("fs://{}/sub/out.json", d.path().display());
        let spec = std::sync::Arc::new(crate::remote::parse(&url, None).unwrap().unwrap());
        let lines: String = (0..25).map(|i| format!("{{\"n\":{i:04}}}\n")).collect();
        for codec in [None, Some(Codec::Gzip(6)), Some(Codec::Zstd(3))] {
            let root = PathBuf::from(&spec.key);
            let mut sink = FileSink::create(FileOptions {
                path: Some(root.clone()),
                codec,
                split: Some(SplitBy::Rows(10)),
                overwrite: true,
                remote: Some(spec.clone()),
                ..Default::default()
            })
            .await
            .unwrap();
            sink.write_lines(lines.as_bytes()).await.unwrap();
            sink.finalize().await.unwrap();
            let mut all = String::new();
            for n in 0..3 {
                let key = part_path(&root, n, codec);
                assert!(std::path::Path::new("/").join(&key).is_file(), "{key:?}");
                let loc = crate::pipeline::file_source::InputLoc::Remote(std::sync::Arc::new(
                    spec.with_key(key.to_string_lossy().into_owned()),
                ));
                let text = tokio::task::spawn_blocking(move || {
                    let mut r = crate::pipeline::file_source::open_input_counted(
                        &loc,
                        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    )
                    .unwrap();
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut r, &mut s).unwrap();
                    s
                })
                .await
                .unwrap();
                all.push_str(&text);
            }
            assert_eq!(all, lines, "{codec:?}");
        }
        // without --overwrite an existing object is refused
        let err = FileSink::create(FileOptions {
            path: Some(PathBuf::from(&spec.with_key(format!("{}x", spec.key)).key)),
            remote: Some(spec.clone()),
            ..Default::default()
        })
        .await;
        assert!(err.is_ok());
        let existing = part_path(&PathBuf::from(&spec.key), 0, None);
        let mut sink = FileSink::create(FileOptions {
            path: Some(PathBuf::from(&spec.key)),
            split: Some(SplitBy::Rows(10)),
            remote: Some(spec.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
        let e = sink.write_lines(lines.as_bytes()).await.unwrap_err();
        assert!(format!("{e:#}").contains("already exists"), "{e:#}");
        assert!(std::path::Path::new("/").join(&existing).is_file());
        // an aborted unsplit write leaves no object
        let fresh = spec.with_key(format!("{}.aborted", spec.key));
        let mut sink = FileSink::create(FileOptions {
            path: Some(PathBuf::from(&fresh.key)),
            remote: Some(std::sync::Arc::new(fresh.clone())),
            ..Default::default()
        })
        .await
        .unwrap();
        sink.write_lines(lines.as_bytes()).await.unwrap();
        sink.abort().await;
        assert!(!std::path::Path::new("/").join(&fresh.key).exists());
        let leftovers: Vec<_> = std::fs::read_dir(d.path().join("sub"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("aborted"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
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
    async fn a_failed_commit_leaves_all_or_nothing() {
        let names = |d: &Path| {
            let mut v: Vec<String> = std::fs::read_dir(d)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        // without --overwrite: part 2 appears while the dump runs
        let d = tmpdir("commit-race");
        let root = d.join("out.json");
        let mut sink = FileSink::create(FileOptions {
            path: Some(root.clone()),
            split: Some(SplitBy::Rows(1)),
            ..Default::default()
        })
        .await
        .unwrap();
        sink.write_lines(b"{\"n\":0}\n{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n")
            .await
            .unwrap();
        std::fs::write(part_path(&root, 2, None), "theirs\n").unwrap();
        let e = format!("{:#}", sink.finalize().await.unwrap_err());
        assert!(e.contains("nothing was committed"), "{e}");
        assert_eq!(names(&d), vec!["out.split-2.json".to_string()], "{e}");
        assert_eq!(
            std::fs::read_to_string(part_path(&root, 2, None)).unwrap(),
            "theirs\n"
        );
        std::fs::remove_dir_all(&d).unwrap();

        // with --overwrite: an earlier set is restored when a later part cannot be replaced
        let d = tmpdir("commit-overwrite");
        let root = d.join("out.json");
        for n in 0..2 {
            std::fs::write(part_path(&root, n, None), format!("old{n}\n")).unwrap();
        }
        let blocker = part_path(&root, 2, None);
        std::fs::create_dir(&blocker).unwrap();
        std::fs::write(blocker.join("keep"), "x").unwrap();
        let mut sink = FileSink::create(FileOptions {
            path: Some(root.clone()),
            split: Some(SplitBy::Rows(1)),
            overwrite: true,
            ..Default::default()
        })
        .await
        .unwrap();
        sink.write_lines(b"{\"n\":0}\n{\"n\":1}\n{\"n\":2}\n")
            .await
            .unwrap();
        let e = format!("{:#}", sink.finalize().await.unwrap_err());
        assert!(e.contains("nothing was committed"), "{e}");
        assert_eq!(
            names(&d),
            vec!["out.split-0.json", "out.split-1.json", "out.split-2.json"],
            "{e}"
        );
        for n in 0..2 {
            assert_eq!(
                std::fs::read_to_string(part_path(&root, n, None)).unwrap(),
                format!("old{n}\n")
            );
        }
        // and a clean overwrite replaces the set and leaves no backups
        std::fs::remove_dir_all(&blocker).unwrap();
        let mut sink = FileSink::create(FileOptions {
            path: Some(root.clone()),
            split: Some(SplitBy::Rows(1)),
            overwrite: true,
            ..Default::default()
        })
        .await
        .unwrap();
        sink.write_lines(b"{\"n\":0}\n").await.unwrap();
        sink.finalize().await.unwrap();
        assert_eq!(names(&d), vec!["out.split-0.json", "out.split-1.json"]);
        assert_eq!(
            std::fs::read_to_string(part_path(&root, 0, None)).unwrap(),
            "{\"n\":0}\n"
        );
        std::fs::remove_dir_all(&d).unwrap();
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

    /// D-5: only this output's staging and moved-aside leftovers (itself and its split
    /// parts, any codec) are found; look-alikes and other outputs' leftovers are not.
    #[test]
    fn leftovers_of_finds_only_this_outputs_staging_and_moved_aside_files() {
        let d = tmpdir("leftovers");
        let ours = [
            ".d.json.part-123-456",
            ".d.split-0.json.part-7-8",
            ".d.split-12.json.gz.part-7-9",
            ".d.split-3.json.zst.part-1-2",
            ".d.json.edrs-replaced-77",
            ".d.split-1.json.edrs-replaced-77",
        ];
        let others = [
            "d.json",
            "d.split-0.json",
            ".e.json.part-1-2",
            ".d.json.part-x-2",
            ".d.json.part-12",
            ".d.split-.json.part-1-2",
            ".d.split-1.ndjson.part-1-2",
            "d.json.part-1-2",
            ".d.json.edrs-replaced-",
            ".d.json.bak",
        ];
        for n in ours.iter().chain(others.iter()) {
            std::fs::write(d.join(n), "x").unwrap();
        }
        let mut want: Vec<PathBuf> = ours.iter().map(|n| d.join(n)).collect();
        want.sort();
        assert_eq!(leftovers_of(&d.join("d.json")), want);
        assert_eq!(leftover_pid(".d.json.part-123-456"), Some(123));
        assert_eq!(leftover_pid(".d.json.edrs-replaced-77"), Some(77));
        assert!(leftovers_of(&d.join("none.json")).is_empty());
        warn_about_leftovers(&d.join("d.json"), false);
        let plain = leftover_warning(&d.join("d.json"), false).unwrap();
        assert!(plain.contains("left in place, not deleted"), "{plain}");
        assert!(!plain.contains("--overwrite replaces only"), "{plain}");
        let ow = leftover_warning(&d.join("d.json"), true).unwrap();
        assert!(
            ow.contains(
                "--overwrite replaces only the output itself; it does not remove these files"
            ),
            "{ow}"
        );
        assert!(leftover_warning(&d.join("none.json"), true).is_none());
        for n in ours.iter().chain(others.iter()) {
            assert!(d.join(n).exists(), "{n} must never be deleted");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}
