//! multielasticdump: dump or load many indices at once with elasticdump-rs.
//!
//! Each selected index gets its own settings, mapping, optional alias and data files, in the
//! layout that `scripts/oslogs.py` writes (`INDEX.settings.json`, `INDEX.mapping.json`,
//! `INDEX.data.json.gz` / `.zst`), so a directory written by either tool loads with either.
//! Indices are selected by `--match` (and `--exclude`) regexes and can be broken out into
//! per-index or per-group subdirectories (`--layout`). The per-index work runs as child
//! `elasticdump-rs` processes, at most `--parallel` indices at a time; one index failing does not
//! stop the others. A manifest (`multielasticdump.json`) records what was dumped.
//!
//! Credentials: use `ELASTICDUMP_INPUT_*` / `ELASTICDUMP_OUTPUT_*` environment variables or
//! `--httpAuthFile`; children inherit them. `user:pass@` in a URL works but puts the password on
//! every child's command line (visible in `ps`).
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, ValueEnum};
use elasticdump_rs::endpoint::read_auth_file;
use elasticdump_rs::http::gate::RequestGate;
use elasticdump_rs::http::tls::TlsOptions;
use elasticdump_rs::http::{Body, ClientOptions, EsClient};
use regex::Regex;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use url::Url;

pub const MANIFEST: &str = "multielasticdump.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Direction {
    Dump,
    Load,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// DIR/INDEX.* (oslogs.py layout)
    Flat,
    /// DIR/INDEX/INDEX.*
    Index,
    /// DIR/GROUP/INDEX.*, GROUP from the `group` named capture of --match (or its first group)
    Group,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    Gzip,
    Zstd1,
    Zstd3,
    None,
}

impl Compression {
    pub fn suffix(self) -> &'static str {
        match self {
            Compression::Gzip => "json.gz",
            Compression::Zstd1 | Compression::Zstd3 => "json.zst",
            Compression::None => "json",
        }
    }
    fn flags(self) -> &'static [&'static str] {
        match self {
            Compression::Gzip => &["--fsCompress"],
            Compression::Zstd1 => &["--compressionType=zstd", "--compressionLevel=1"],
            Compression::Zstd3 => &["--compressionType=zstd", "--compressionLevel=3"],
            Compression::None => &[],
        }
    }
    /// The codec of an existing data file, from its name.
    pub fn of_file(name: &str) -> Option<Compression> {
        if name.ends_with(".data.json.gz") {
            Some(Compression::Gzip)
        } else if name.ends_with(".data.json.zst") {
            Some(Compression::Zstd3)
        } else if name.ends_with(".data.json") {
            Some(Compression::None)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Lifecycle {
    /// --excludeSettings=index.lifecycle,index.routing.allocation.include._tier_preference
    Drop,
    /// --convertIlmToIsm (translates ILM policy documents only; index.lifecycle settings are
    /// not rewritten, so an OpenSearch target rejects them: use drop there)
    Convert,
}

/// Same defaults as scripts/oslogs.py for data runs.
const DATA_FLAGS: &[&str] = &[
    "--limit=5000",
    "--scrollTime=10m",
    "--retryAttempts=5",
    "--retryDelay=2000",
    "--timeout=300000",
    "--maxResponseMb=0",
];

/// Per-index item types, in the order they are dumped and loaded.
pub const TYPE_ORDER: &[&str] = &["settings", "mapping", "alias", "data"];

#[derive(Parser, Debug)]
#[command(
    name = "multielasticdump",
    version,
    about = "Dump or load many indices at once with elasticdump-rs, selected by regex",
    after_help = "Examples:\n  \
      # every index matching logs-*, gzip, one set of files per index (oslogs.py layout)\n  \
      ELASTICDUMP_INPUT_USERNAME=admin ELASTICDUMP_INPUT_PASSWORD=... \\\n    \
      multielasticdump --direction dump --input https://os:9200 --output /backup --match '^logs-'\n  \
      # break out by month: /backup/2026.10/logs-app-2026.10.31.*\n  \
      multielasticdump --input https://os:9200 --output /backup --layout group \\\n    \
      --match '^logs-.*-(?P<group>\\d{4}\\.\\d{2})\\.\\d{2}$'\n  \
      # load everything back (ELASTICDUMP_OUTPUT_* credentials), 4 indices at a time\n  \
      multielasticdump --direction load --input /backup --output https://os2:9200 --parallel 4\n\n\
      Extra arguments after `--` are passed to every elasticdump-rs data run."
)]
pub struct Args {
    /// dump: cluster -> directory; load: directory -> cluster
    #[arg(long, value_enum, default_value = "dump")]
    pub direction: Direction,
    /// dump: cluster base URL (https://host:9200); load: dump directory
    #[arg(long)]
    pub input: String,
    /// dump: output directory (created if missing); load: cluster base URL
    #[arg(long)]
    pub output: String,
    /// Regex an index name must match (unanchored; use ^...$ for whole names)
    #[arg(long = "match", default_value = "^.*$")]
    pub match_re: String,
    /// Regex of index names to leave out
    #[arg(long)]
    pub exclude: Option<String>,
    /// Also select hidden/system indices (names starting with '.'); off by default
    #[arg(long = "includeHidden", alias = "include-hidden")]
    pub include_hidden: bool,
    /// Directory layout for each index's files (dump); load finds files in any layout
    #[arg(long, value_enum, default_value = "flat")]
    pub layout: Layout,
    /// Item types per index, comma separated: settings, mapping, alias, data.
    /// Default: dump settings,mapping,data (as oslogs.py); load settings,mapping,alias,data
    /// (an index without an alias file just skips that step)
    #[arg(long, value_delimiter = ',')]
    pub types: Vec<String>,
    /// Types to skip (Node multielasticdump --ignoreType), comma separated
    #[arg(long = "ignoreType", value_delimiter = ',')]
    pub ignore_type: Vec<String>,
    /// How many indices to process at once
    #[arg(long, default_value_t = 2)]
    pub parallel: usize,
    /// Data-file compression (dump). Load detects it from each file name
    #[arg(long, value_enum, default_value = "gzip")]
    pub compression: Compression,
    /// Lifecycle settings on load: drop them, or convert ILM to ISM
    #[arg(long, value_enum, default_value = "drop")]
    pub lifecycle: Lifecycle,
    /// Load: prepend this to every target index name
    #[arg(long, default_value = "")]
    pub prefix: String,
    /// Load: append this to every target index name
    #[arg(long, default_value = "")]
    pub suffix: String,
    /// Load: skip an index that already exists on the target instead of failing it
    #[arg(long = "skipExisting", alias = "skip-existing")]
    pub skip_existing: bool,
    /// Dump: skip indices the manifest already records as done (files still present)
    #[arg(long)]
    pub resume: bool,
    /// Skip TLS certificate verification (also passed to elasticdump-rs)
    #[arg(long)]
    pub insecure: bool,
    /// INI file with user= and password= for the cluster (also passed to elasticdump-rs)
    #[arg(long = "httpAuthFile")]
    pub http_auth_file: Option<String>,
    /// elasticdump-rs binary (default: next to this binary, then $ELASTICDUMP_BIN, then PATH)
    #[arg(long = "elasticdumpBin")]
    pub elasticdump_bin: Option<PathBuf>,
    /// Where each child run's output is logged (default: a new temp directory)
    #[arg(long = "logDir")]
    pub log_dir: Option<PathBuf>,
    /// Do not add the oslogs.py data-run defaults (--limit=5000 --scrollTime=10m ...)
    #[arg(long = "noDefaultDataFlags")]
    pub no_default_data_flags: bool,
    /// Print the selected indices and the files they map to, then exit
    #[arg(long = "dryRun", alias = "dry-run")]
    pub dry_run: bool,
    /// Extra elasticdump-rs arguments for every data run
    #[arg(last = true)]
    pub extra: Vec<String>,
}

// ----------------------------------------------------------------------------- selection

/// Index-name selection: `--match`, `--exclude`, hidden indices, and the break-out group.
pub struct Selector {
    include: Regex,
    exclude: Option<Regex>,
    include_hidden: bool,
}

impl Selector {
    pub fn new(include: &str, exclude: Option<&str>, include_hidden: bool) -> Result<Self> {
        let include =
            Regex::new(include).with_context(|| format!("invalid --match regex '{include}'"))?;
        let exclude = exclude
            .map(|e| Regex::new(e).with_context(|| format!("invalid --exclude regex '{e}'")))
            .transpose()?;
        Ok(Self {
            include,
            exclude,
            include_hidden,
        })
    }

    pub fn selects(&self, name: &str) -> bool {
        // Hidden/system indices only with --includeHidden or a --match that starts with a literal dot.
        let p = self.include.as_str();
        let explicit_dot = p.starts_with("^\\.") || p.starts_with("\\.");
        if name.starts_with('.') && !self.include_hidden && !explicit_dot {
            return false;
        }
        self.include.is_match(name) && !self.exclude.as_ref().is_some_and(|e| e.is_match(name))
    }

    pub fn has_group(&self) -> bool {
        self.include.captures_len() > 1
    }

    /// The break-out group: the `group` named capture, else the first capture group.
    pub fn group(&self, name: &str) -> Option<String> {
        let caps = self.include.captures(name)?;
        let m = caps.name("group").or_else(|| caps.get(1))?;
        Some(sanitize_dir(m.as_str()))
    }
}

/// A group value as one safe directory name.
pub fn sanitize_dir(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '@') {
                c
            } else {
                '_'
            }
        })
        .collect();
    match s.as_str() {
        "" => "_".into(),
        "." | ".." => s.replace('.', "_"),
        _ => s,
    }
}

/// Index names are used as file names: refuse anything that could leave the directory.
pub fn check_index_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0', '\n', '\r'])
        || name.starts_with('-')
    {
        bail!("refusing unsafe index name {name:?}");
    }
    Ok(())
}

/// The directory (relative to the dump root) that holds one index's files.
pub fn rel_dir(layout: Layout, sel: &Selector, index: &str) -> Result<PathBuf> {
    Ok(match layout {
        Layout::Flat => PathBuf::new(),
        Layout::Index => PathBuf::from(sanitize_dir(index)),
        Layout::Group => {
            PathBuf::from(sel.group(index).ok_or_else(|| {
                anyhow!("--layout group: --match has no capture group for {index}")
            })?)
        }
    })
}

pub fn item_file(index: &str, kind: &str, compression: Compression) -> String {
    if kind == "data" {
        format!("{index}.data.{}", compression.suffix())
    } else {
        format!("{index}.{kind}.json")
    }
}

pub fn resolve_types(types: &[String], ignore: &[String]) -> Result<Vec<&'static str>> {
    for t in types.iter().chain(ignore) {
        if !TYPE_ORDER.contains(&t.trim()) {
            bail!("unknown type '{t}' (supported: settings, mapping, alias, data)");
        }
    }
    let out: Vec<&'static str> = TYPE_ORDER
        .iter()
        .copied()
        .filter(|t| types.iter().any(|x| x.trim() == *t) && !ignore.iter().any(|x| x.trim() == *t))
        .collect();
    if out.is_empty() {
        bail!("no types left to process");
    }
    Ok(out)
}

// ----------------------------------------------------------------------------- manifest

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct IndexEntry {
    /// Directory relative to the dump root ("" for flat).
    pub dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_count: Option<u64>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub tool: String,
    pub version: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub layout: Option<Layout>,
    #[serde(default)]
    pub compression: Option<Compression>,
    pub indices: BTreeMap<String, IndexEntry>,
}

impl Manifest {
    pub fn load(root: &Path) -> Result<Option<Manifest>> {
        let p = root.join(MANIFEST);
        if !p.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
        Ok(Some(
            serde_json::from_str(&text).with_context(|| format!("parse {}", p.display()))?,
        ))
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let tmp = root.join(format!(".{MANIFEST}.tmp"));
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, root.join(MANIFEST))?;
        Ok(())
    }
}

// ----------------------------------------------------------------------------- discovery (load)

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub index: String,
    pub dir: PathBuf,
    pub data: Option<PathBuf>,
    pub records_hint: Option<u64>,
}

/// Indices in a dump directory: from the manifest when there is one, else every
/// `INDEX.settings.json` / `INDEX.mapping.json` / `INDEX.data.json*` up to two levels down.
pub fn discover(root: &Path) -> Result<Vec<Found>> {
    let mut out: BTreeMap<String, Found> = BTreeMap::new();
    if let Some(m) = Manifest::load(root)? {
        for (name, e) in &m.indices {
            if e.status != "done" {
                eprintln!(
                    "note: {name} is recorded as {} in the manifest; skipping it",
                    e.status
                );
                continue;
            }
            check_index_name(name)?;
            let dir = root.join(&e.dir);
            let data = e
                .data_file
                .as_ref()
                .map(|f| dir.join(f))
                .filter(|p| p.is_file());
            out.insert(
                name.clone(),
                Found {
                    index: name.clone(),
                    dir,
                    data,
                    records_hint: e.records,
                },
            );
        }
        return Ok(out.into_values().collect());
    }
    let mut dirs = vec![(root.to_path_buf(), 0)];
    while let Some((d, depth)) = dirs.pop() {
        for ent in fs::read_dir(&d).with_context(|| format!("read {}", d.display()))? {
            let ent = ent?;
            let path = ent.path();
            let name = ent.file_name().to_string_lossy().into_owned();
            if ent.file_type()?.is_dir() {
                if depth < 2 {
                    dirs.push((path, depth + 1));
                }
                continue;
            }
            let index = ["settings.json", "mapping.json", "alias.json"]
                .iter()
                .find_map(|k| name.strip_suffix(&format!(".{k}")))
                .map(str::to_string)
                .or_else(|| {
                    Compression::of_file(&name).and_then(|c| {
                        name.strip_suffix(&format!(".data.{}", c.suffix()))
                            .map(str::to_string)
                    })
                });
            let Some(index) = index else { continue };
            if check_index_name(&index).is_err() {
                continue;
            }
            let entry = out
                .entry(format!("{}\0{index}", d.display()))
                .or_insert_with(|| Found {
                    index: index.clone(),
                    dir: d.clone(),
                    data: None,
                    records_hint: None,
                });
            if Compression::of_file(&name).is_some() && fs::metadata(&path)?.len() > 0 {
                if let Some(prev) = &entry.data {
                    bail!(
                        "{index}: more than one data file ({} and {name})",
                        prev.display()
                    );
                }
                entry.data = Some(path);
            }
        }
    }
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut found: Vec<Found> = out.into_values().collect();
    found.sort_by(|a, b| a.index.cmp(&b.index));
    for f in &found {
        if let Some(prev) = seen.insert(f.index.clone(), f.dir.clone()) {
            bail!(
                "index {} appears in both {} and {}",
                f.index,
                prev.display(),
                f.dir.display()
            );
        }
    }
    Ok(found)
}

// ----------------------------------------------------------------------------- data files

/// Records (lines) in a data file, decompressing by its name.
pub fn count_records(path: &Path) -> Result<u64> {
    let f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut r: Box<dyn Read> = match Compression::of_file(&name) {
        Some(Compression::Gzip) => Box::new(flate2::read::MultiGzDecoder::new(f)),
        Some(Compression::Zstd1 | Compression::Zstd3) => {
            Box::new(zstd::stream::read::Decoder::new(f)?)
        }
        _ => Box::new(f),
    };
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let k = r
            .read(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if k == 0 {
            return Ok(n);
        }
        n += memchr::memchr_iter(b'\n', &buf[..k]).count() as u64;
    }
}

/// elasticdump writes settings and mappings as JSON, sometimes as a JSON string of JSON.
pub fn mappings_body(text: &str) -> Result<serde_json::Value> {
    let mut doc: serde_json::Value = serde_json::from_str(text.trim())?;
    if let serde_json::Value::String(s) = &doc {
        doc = serde_json::from_str(s)?;
    }
    let obj = doc
        .as_object()
        .filter(|o| !o.is_empty())
        .ok_or_else(|| anyhow!("mapping dump is empty"))?;
    let (_, body) = obj.iter().next().unwrap();
    Ok(body
        .get("mappings")
        .cloned()
        .unwrap_or_else(|| body.clone()))
}

// ----------------------------------------------------------------------------- cluster

/// Base URL without credentials, plus credentials (flag file, env, then URL userinfo).
pub fn split_url(
    raw: &str,
    env_prefix: &str,
    auth_file: Option<&(String, String)>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(Url, Option<String>, Option<String>)> {
    let mut url = Url::parse(raw).with_context(|| {
        format!(
            "invalid URL '{}'",
            elasticdump_rs::endpoint::redacted_endpoint(raw)
        )
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("URL must start with http:// or https://");
    }
    let dec = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    let mut user = (!url.username().is_empty()).then(|| dec(url.username()));
    let mut pass = url.password().map(dec);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    if !url.path().ends_with('/') {
        let p = format!("{}/", url.path());
        url.set_path(&p);
    }
    if user.is_none() || pass.is_none() {
        if let Some((u, p)) = auth_file {
            user = Some(u.clone());
            pass = Some(p.clone());
        } else if let Some(u) = env(&format!("{env_prefix}_USERNAME")) {
            user = Some(u.trim_end_matches(['\r', '\n']).to_string());
            pass = env(&format!("{env_prefix}_PASSWORD"))
                .map(|p| p.trim_end_matches(['\r', '\n']).to_string());
        }
    }
    Ok((url, user, pass))
}

pub struct Cluster {
    client: EsClient,
    /// What children get as --input/--output (as the user gave it, without a trailing slash).
    child_base: String,
}

impl Cluster {
    pub fn new(raw: &str, env_prefix: &str, args: &Args) -> Result<Self> {
        let auth = args
            .http_auth_file
            .as_deref()
            .map(read_auth_file)
            .transpose()?;
        let (base, username, password) =
            split_url(raw, env_prefix, auth.as_ref(), &|k| std::env::var(k).ok())?;
        let opts = ClientOptions {
            base,
            username,
            password,
            headers: vec![],
            params: vec![],
            tls: TlsOptions {
                ca: None,
                cert: None,
                key: None,
                pass: None,
                insecure: args.insecure,
            },
            timeout: Some(Duration::from_secs(300)),
            idle_timeout: None,
            compress: false,
            max_sockets: None,
            debug: false,
            no_proxy: false,
        };
        Ok(Self {
            client: EsClient::new(&opts, RequestGate::new(None, None))?,
            child_base: raw.trim_end_matches('/').to_string(),
        })
    }

    pub fn index_url(&self, index: &str) -> String {
        let enc = percent_encoding::utf8_percent_encode(index, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
        format!("{}/{enc}", self.child_base)
    }

    async fn call(
        &self,
        method: Method,
        segs: &[&str],
        query: &[(&str, &str)],
        body: Body,
    ) -> Result<(u16, Vec<u8>)> {
        let resp = self
            .client
            .send(method.clone(), segs, query, false, body)
            .await
            .map_err(|e| anyhow!("{method} /{}: {e}", segs.join("/")))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await?.to_vec();
        Ok((status, bytes))
    }

    async fn ok_json(
        &self,
        method: Method,
        segs: &[&str],
        query: &[(&str, &str)],
        body: Body,
    ) -> Result<serde_json::Value> {
        let (status, bytes) = self.call(method.clone(), segs, query, body).await?;
        if !(200..300).contains(&status) {
            let detail = String::from_utf8_lossy(&bytes[..bytes.len().min(2000)]).into_owned();
            let hint = if status == 401 {
                " (credentials rejected: check ELASTICDUMP_*_USERNAME/PASSWORD or --httpAuthFile)"
            } else {
                ""
            };
            bail!(
                "{method} /{} -> HTTP {status}: {detail}{hint}",
                segs.join("/")
            );
        }
        if bytes.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Open indices (closed ones cannot be dumped), sorted by name.
    pub async fn list_indices(&self) -> Result<Vec<String>> {
        let v = self
            .ok_json(
                Method::GET,
                &["_cat", "indices"],
                &[
                    ("format", "json"),
                    ("h", "index,status"),
                    ("expand_wildcards", "all"),
                    ("s", "index"),
                ],
                Body::Empty,
            )
            .await?;
        let mut out = vec![];
        for row in v
            .as_array()
            .ok_or_else(|| anyhow!("unexpected _cat/indices response"))?
        {
            let name = row
                .get("index")
                .and_then(|x| x.as_str())
                .unwrap_or_default();
            let status = row.get("status").and_then(|x| x.as_str()).unwrap_or("open");
            if name.is_empty() {
                continue;
            }
            if status != "open" {
                eprintln!("note: skipping closed index {name}");
                continue;
            }
            out.push(name.to_string());
        }
        out.sort();
        Ok(out)
    }

    pub async fn exists(&self, index: &str) -> Result<bool> {
        let (status, bytes) = self
            .call(Method::GET, &[index, "_settings"], &[], Body::Empty)
            .await?;
        match status {
            200..=299 => Ok(true),
            404 => Ok(false),
            401 => bail!("GET /{index}/_settings -> HTTP 401: credentials rejected"),
            s => bail!(
                "GET /{index}/_settings -> HTTP {s}: {}",
                String::from_utf8_lossy(&bytes[..bytes.len().min(500)])
            ),
        }
    }

    pub async fn count(&self, index: &str, refresh: bool) -> Result<u64> {
        if refresh {
            self.ok_json(Method::POST, &[index, "_refresh"], &[], Body::Empty)
                .await?;
        }
        let v = self
            .ok_json(Method::GET, &[index, "_count"], &[], Body::Empty)
            .await?;
        v.get("count")
            .and_then(|c| c.as_u64())
            .ok_or_else(|| anyhow!("no count in /{index}/_count response"))
    }

    pub async fn put_mapping(&self, index: &str, mappings: &serde_json::Value) -> Result<()> {
        self.ok_json(Method::PUT, &[index, "_mapping"], &[], Body::json(mappings))
            .await
            .map(|_| ())
    }
}

// ----------------------------------------------------------------------------- child runs

pub struct Runner {
    pub bin: PathBuf,
    pub log_dir: PathBuf,
    pub common: Vec<String>,
    pub data_flags: Vec<String>,
    pub cancelled: Arc<AtomicBool>,
}

impl Runner {
    /// Run elasticdump-rs once; output goes to LOGDIR/INDEX.KIND.log.
    pub fn run(
        &self,
        index: &str,
        kind: &str,
        input: &str,
        output: &str,
        extra: &[String],
    ) -> Result<()> {
        if self.cancelled.load(Ordering::SeqCst) {
            bail!("interrupted");
        }
        let log_path = self.log_dir.join(format!("{index}.{kind}.log"));
        let log = fs::File::create(&log_path)
            .with_context(|| format!("create {}", log_path.display()))?;
        let mut cmd = Command::new(&self.bin);
        cmd.arg(format!("--input={input}"))
            .arg(format!("--output={output}"))
            .arg(format!("--type={kind}"))
            .arg("--overwrite")
            .args(&self.common)
            .args(extra)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        let status = cmd
            .status()
            .with_context(|| format!("could not start {}", self.bin.display()))?;
        if !status.success() {
            bail!(
                "elasticdump-rs --type={kind} failed ({status}); last lines of {}:\n{}",
                log_path.display(),
                tail(&log_path, 15)
            );
        }
        Ok(())
    }
}

fn tail(path: &Path, n: usize) -> String {
    let Ok(f) = fs::File::open(path) else {
        return String::new();
    };
    let lines: Vec<String> = BufReader::new(f).lines().map_while(Result::ok).collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_bin(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Some(p) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("elasticdump-rs")))
        .filter(|p| p.is_file())
    {
        return p;
    }
    if let Ok(p) = std::env::var("ELASTICDUMP_BIN") {
        return PathBuf::from(p);
    }
    PathBuf::from("elasticdump-rs")
}

fn nonempty(p: &Path) -> bool {
    fs::metadata(p)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

// ----------------------------------------------------------------------------- dump / load

#[derive(Debug)]
pub enum Outcome {
    Done(String),
    Skipped(String),
    Failed(String),
}

struct Ctx {
    args: Args,
    types: Vec<&'static str>,
    sel: Selector,
    runner: Runner,
}

async fn dump_one(
    ctx: &Ctx,
    cluster: &Cluster,
    root: &Path,
    manifest: &Mutex<Manifest>,
    index: &str,
) -> Outcome {
    let rel = match rel_dir(ctx.args.layout, &ctx.sel, index) {
        Ok(r) => r,
        Err(e) => return Outcome::Failed(format!("{e:#}")),
    };
    let dir = root.join(&rel);
    let comp = ctx.args.compression;
    let mut entry = IndexEntry {
        dir: rel.to_string_lossy().into_owned(),
        group: (ctx.args.layout == Layout::Group).then(|| rel.to_string_lossy().into_owned()),
        types: ctx.types.iter().map(|t| t.to_string()).collect(),
        data_file: ctx
            .types
            .contains(&"data")
            .then(|| item_file(index, "data", comp)),
        status: "running".into(),
        ..Default::default()
    };
    let res: Result<String> = async {
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let source = cluster.index_url(index);
        entry.source_count = cluster.count(index, false).await.ok();
        for kind in &ctx.types {
            let file = dir.join(item_file(index, kind, comp));
            let out = file.to_string_lossy().into_owned();
            let extra: Vec<String> = if *kind == "data" {
                comp.flags().iter().map(|s| s.to_string()).chain(ctx.runner.data_flags.iter().cloned()).collect()
            } else {
                vec![]
            };
            let (i, k) = (index.to_string(), kind.to_string());
            let runner = &ctx.runner;
            tokio::task::block_in_place(|| runner.run(&i, &k, &source, &out, &extra))?;
            if *kind != "data" && !nonempty(&file) {
                bail!("{kind} export is empty");
            }
        }
        let mut summary = String::new();
        if ctx.types.contains(&"data") {
            let file = dir.join(item_file(index, "data", comp));
            let records = if file.exists() { count_records(&file)? } else { 0 };
            entry.records = Some(records);
            if let Some(src) = entry.source_count {
                if records == 0 && src > 0 {
                    bail!("data export is empty but the index has {src} documents");
                }
                if records != src {
                    eprintln!("warning: {index}: {records} records written, the index reported {src} before the dump (still being written to?)");
                }
            }
            summary = format!(" ({records} records)");
        }
        Ok(format!("{}{summary}", dir.display()))
    }
    .await;
    let outcome = match res {
        Ok(s) => {
            entry.status = "done".into();
            Outcome::Done(s)
        }
        Err(e) => {
            entry.status = "failed".into();
            entry.error = Some(format!("{e:#}"));
            Outcome::Failed(format!("{e:#}"))
        }
    };
    let mut m = manifest.lock().unwrap();
    m.indices.insert(index.to_string(), entry);
    if let Err(e) = m.save(root) {
        eprintln!("warning: could not write the manifest: {e:#}");
    }
    outcome
}

async fn load_one(ctx: &Ctx, cluster: &Cluster, f: &Found) -> Outcome {
    let target = format!("{}{}{}", ctx.args.prefix, f.index, ctx.args.suffix);
    let res: Result<Option<String>> = async {
        check_index_name(&target)?;
        if cluster.exists(&target).await? {
            if ctx.args.skip_existing {
                return Ok(None);
            }
            bail!("index {target} already exists on the target (use --skipExisting to skip it)");
        }
        let url = cluster.index_url(&target);
        let mut records = None;
        for kind in &ctx.types {
            let (file, extra): (PathBuf, Vec<String>) = match *kind {
                "data" => {
                    let Some(data) = &f.data else {
                        if f.records_hint.unwrap_or(0) == 0 && nonempty(&f.dir.join(format!("{}.settings.json", f.index))) {
                            eprintln!("note: {}: no data file (empty index); loading metadata only", f.index);
                            continue;
                        }
                        bail!("missing data file for {} in {}", f.index, f.dir.display());
                    };
                    let name = data.file_name().unwrap().to_string_lossy().into_owned();
                    let c = Compression::of_file(&name).unwrap_or(Compression::None);
                    records = Some(count_records(data)?);
                    (data.clone(), c.flags().iter().map(|s| s.to_string()).chain(ctx.runner.data_flags.iter().cloned()).collect())
                }
                k => {
                    let p = f.dir.join(item_file(&f.index, k, Compression::None));
                    if !nonempty(&p) {
                        if k == "alias" {
                            continue;
                        }
                        bail!("missing {k} file {}", p.display());
                    }
                    let extra = if k == "settings" {
                        match ctx.args.lifecycle {
                            Lifecycle::Drop => vec!["--excludeSettings=index.lifecycle,index.routing.allocation.include._tier_preference".into()],
                            Lifecycle::Convert => vec!["--convertIlmToIsm".into()],
                        }
                    } else {
                        vec![]
                    };
                    (p, extra)
                }
            };
            let input = file.to_string_lossy().into_owned();
            let (i, k) = (f.index.clone(), kind.to_string());
            let runner = &ctx.runner;
            let r = tokio::task::block_in_place(|| runner.run(&i, &k, &input, &url, &extra));
            if let Err(e) = r {
                if *kind == "mapping" {
                    eprintln!("note: {target}: mapping via elasticdump-rs failed, applying it directly");
                    let text = fs::read_to_string(&file)?;
                    cluster.put_mapping(&target, &mappings_body(&text)?).await.with_context(|| format!("{e:#}"))?;
                } else {
                    return Err(e);
                }
            }
        }
        if let Some(want) = records {
            let got = cluster.count(&target, true).await?;
            if got != want {
                bail!("restored {target} has {got} documents, the backup file has {want} records");
            }
            return Ok(Some(format!("{target} docs={got}")));
        }
        Ok(Some(target.clone()))
    }
    .await;
    match res {
        Ok(Some(s)) => Outcome::Done(s),
        Ok(None) => Outcome::Skipped(format!("{target} already exists")),
        Err(e) => Outcome::Failed(format!("{e:#}")),
    }
}

async fn run(args: Args) -> Result<i32> {
    if args.parallel == 0 {
        bail!("--parallel must be at least 1");
    }
    let default_types: Vec<String> = match args.direction {
        Direction::Dump => vec!["settings", "mapping", "data"],
        Direction::Load => vec!["settings", "mapping", "alias", "data"],
    }
    .into_iter()
    .map(String::from)
    .collect();
    let types = resolve_types(
        if args.types.is_empty() {
            &default_types
        } else {
            &args.types
        },
        &args.ignore_type,
    )?;
    let sel = Selector::new(&args.match_re, args.exclude.as_deref(), args.include_hidden)?;
    if args.layout == Layout::Group && !sel.has_group() {
        bail!("--layout group needs a capture group in --match, e.g. '^logs-(?P<group>[a-z]+)-'");
    }
    let creds_in = |u: &str| {
        Url::parse(u)
            .map(|u| u.password().is_some())
            .unwrap_or(false)
    };
    if creds_in(&args.input) || creds_in(&args.output) {
        eprintln!(
            "warning: a password in the URL is passed to every elasticdump-rs run and shows in `ps`; prefer ELASTICDUMP_*_PASSWORD or --httpAuthFile"
        );
    }
    let log_dir = match &args.log_dir {
        Some(d) => d.clone(),
        None => std::env::temp_dir().join(format!(
            "multielasticdump-{}-{}",
            std::process::id(),
            chrono_stamp()
        )),
    };
    let mut common = vec![];
    if args.insecure {
        common.push("--insecure".to_string());
    }
    if let Some(f) = &args.http_auth_file {
        common.push(format!("--httpAuthFile={f}"));
    }
    let mut data_flags: Vec<String> = if args.no_default_data_flags {
        vec![]
    } else {
        DATA_FLAGS.iter().map(|s| s.to_string()).collect()
    };
    data_flags.extend(args.extra.iter().cloned());
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let c = cancelled.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("interrupted: finishing running steps, starting no new ones");
                c.store(true, Ordering::SeqCst);
            }
        });
    }
    let runner = Runner {
        bin: find_bin(args.elasticdump_bin.as_deref()),
        log_dir: log_dir.clone(),
        common,
        data_flags,
        cancelled: cancelled.clone(),
    };
    let ctx = Arc::new(Ctx {
        args,
        types,
        sel,
        runner,
    });
    let started = Instant::now();
    let sem = Arc::new(tokio::sync::Semaphore::new(ctx.args.parallel));
    let mut set = tokio::task::JoinSet::new();

    match ctx.args.direction {
        Direction::Dump => {
            let cluster = Arc::new(Cluster::new(
                &ctx.args.input,
                "ELASTICDUMP_INPUT",
                &ctx.args,
            )?);
            let all = cluster.list_indices().await?;
            let mut selected = vec![];
            for name in all.iter().filter(|n| ctx.sel.selects(n)) {
                check_index_name(name)?;
                selected.push(name.clone());
            }
            let root = PathBuf::from(&ctx.args.output);
            if ctx.args.dry_run {
                println!("{} of {} indices selected:", selected.len(), all.len());
                for i in &selected {
                    let rel = rel_dir(ctx.args.layout, &ctx.sel, i)?;
                    let files: Vec<String> = ctx
                        .types
                        .iter()
                        .map(|k| {
                            root.join(&rel)
                                .join(item_file(i, k, ctx.args.compression))
                                .display()
                                .to_string()
                        })
                        .collect();
                    println!("  {i}\n    {}", files.join("\n    "));
                }
                return Ok(0);
            }
            if selected.is_empty() {
                eprintln!(
                    "no index matches --match '{}' (of {} indices)",
                    ctx.args.match_re,
                    all.len()
                );
                return Ok(1);
            }
            fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
            fs::create_dir_all(&log_dir)?;
            let mut m = Manifest::load(&root)?.unwrap_or_default();
            if let Some(prev) = m.compression.filter(|c| *c != ctx.args.compression) {
                if !m.indices.is_empty() {
                    eprintln!(
                        "warning: this directory already holds a {prev:?} dump; new indices use {:?}",
                        ctx.args.compression
                    );
                }
            }
            m.tool = "multielasticdump".into();
            m.version = env!("CARGO_PKG_VERSION").into();
            m.source = elasticdump_rs::endpoint::redacted_endpoint(&ctx.args.input);
            m.layout = Some(ctx.args.layout);
            m.compression = Some(ctx.args.compression);
            let mut todo = vec![];
            for i in selected {
                let done = m.indices.get(&i).is_some_and(|e| {
                    e.status == "done"
                        && e.types.iter().all(|k| {
                            nonempty(&root.join(&e.dir).join(item_file(
                                &i,
                                k,
                                ctx.args.compression,
                            ))) || (k == "data" && e.records == Some(0))
                        })
                });
                if ctx.args.resume && done {
                    println!("skip {i} (already dumped)");
                    continue;
                }
                todo.push(i);
            }
            m.save(&root)?;
            println!(
                "dumping {} indices to {} ({} at a time, logs in {})",
                todo.len(),
                root.display(),
                ctx.args.parallel,
                log_dir.display()
            );
            let manifest = Arc::new(Mutex::new(m));
            for i in todo {
                let (ctx, cluster, root, manifest, sem) = (
                    ctx.clone(),
                    cluster.clone(),
                    root.clone(),
                    manifest.clone(),
                    sem.clone(),
                );
                set.spawn(async move {
                    let _p = sem.acquire().await;
                    let o = dump_one(&ctx, &cluster, &root, &manifest, &i).await;
                    (i, o)
                });
            }
        }
        Direction::Load => {
            let root = PathBuf::from(&ctx.args.input);
            if !root.is_dir() {
                bail!("dump directory does not exist: {}", root.display());
            }
            let found: Vec<Found> = discover(&root)?
                .into_iter()
                .filter(|f| ctx.sel.selects(&f.index))
                .collect();
            if ctx.args.dry_run {
                println!("{} indices to load:", found.len());
                for f in &found {
                    println!(
                        "  {} -> {}{}{} (from {})",
                        f.index,
                        ctx.args.prefix,
                        f.index,
                        ctx.args.suffix,
                        f.dir.display()
                    );
                }
                return Ok(0);
            }
            if found.is_empty() {
                eprintln!(
                    "no dumped index in {} matches --match '{}'",
                    root.display(),
                    ctx.args.match_re
                );
                return Ok(1);
            }
            let cluster = Arc::new(Cluster::new(
                &ctx.args.output,
                "ELASTICDUMP_OUTPUT",
                &ctx.args,
            )?);
            fs::create_dir_all(&log_dir)?;
            println!(
                "loading {} indices from {} ({} at a time, logs in {})",
                found.len(),
                root.display(),
                ctx.args.parallel,
                log_dir.display()
            );
            for f in found {
                let (ctx, cluster, sem) = (ctx.clone(), cluster.clone(), sem.clone());
                set.spawn(async move {
                    let _p = sem.acquire().await;
                    let o = load_one(&ctx, &cluster, &f).await;
                    (f.index, o)
                });
            }
        }
    }

    let (mut ok, mut skipped, mut failed) = (0, 0, vec![]);
    while let Some(r) = set.join_next().await {
        let (index, outcome) = r?;
        match outcome {
            Outcome::Done(s) => {
                ok += 1;
                println!("ok      {index}: {s}");
            }
            Outcome::Skipped(s) => {
                skipped += 1;
                println!("skipped {index}: {s}");
            }
            Outcome::Failed(s) => {
                eprintln!("FAILED  {index}: {s}");
                failed.push(index);
            }
        }
        let _ = std::io::stdout().flush();
    }
    failed.sort();
    println!(
        "\n{ok} done, {skipped} skipped, {} failed in {:.0?}{}",
        failed.len(),
        started.elapsed(),
        if failed.is_empty() {
            String::new()
        } else {
            format!(": {}", failed.join(", "))
        }
    );
    if cancelled.load(Ordering::SeqCst) {
        return Ok(130);
    }
    Ok(if failed.is_empty() { 0 } else { 1 })
}

fn chrono_stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn main() {
    let args = Args::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let code = match rt.block_on(run(args)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("multielasticdump: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_include_exclude_hidden() {
        let s = Selector::new("^logs-", Some("-debug-"), false).unwrap();
        assert!(s.selects("logs-app-2026.10.01"));
        assert!(!s.selects("logs-debug-2026.10.01"));
        assert!(!s.selects("metrics-1"));
        let all = Selector::new("^.*$", None, false).unwrap();
        assert!(all.selects("a"));
        assert!(
            !all.selects(".kibana"),
            "hidden indices need --includeHidden or an explicit match"
        );
        assert!(
            Selector::new("^.*$", None, true)
                .unwrap()
                .selects(".kibana")
        );
        assert!(
            Selector::new("^\\.opendistro", None, false)
                .unwrap()
                .selects(".opendistro-ism-config")
        );
        assert!(Selector::new("(", None, false).is_err());
    }

    #[test]
    fn group_breakout() {
        let s = Selector::new(r"^logs-.*-(?P<group>\d{4}\.\d{2})\.\d{2}$", None, false).unwrap();
        assert!(s.has_group());
        assert_eq!(s.group("logs-app-2026.10.31").as_deref(), Some("2026.10"));
        assert_eq!(
            rel_dir(Layout::Group, &s, "logs-app-2026.10.31").unwrap(),
            PathBuf::from("2026.10")
        );
        let first = Selector::new(r"^([a-z]+)-", None, false).unwrap();
        assert_eq!(first.group("web-1").as_deref(), Some("web"));
        let none = Selector::new("^x", None, false).unwrap();
        assert!(!none.has_group());
        assert_eq!(rel_dir(Layout::Flat, &none, "x1").unwrap(), PathBuf::new());
        assert_eq!(
            rel_dir(Layout::Index, &none, "x1").unwrap(),
            PathBuf::from("x1")
        );
        assert_eq!(sanitize_dir("a/b"), "a_b");
        assert_eq!(sanitize_dir(".."), "__");
        assert_eq!(sanitize_dir(""), "_");
    }

    #[test]
    fn names_and_types() {
        assert_eq!(
            item_file("i", "settings", Compression::Gzip),
            "i.settings.json"
        );
        assert_eq!(item_file("i", "data", Compression::Gzip), "i.data.json.gz");
        assert_eq!(
            item_file("i", "data", Compression::Zstd1),
            "i.data.json.zst"
        );
        assert_eq!(item_file("i", "data", Compression::None), "i.data.json");
        assert_eq!(
            Compression::of_file("x.data.json.zst"),
            Some(Compression::Zstd3)
        );
        assert_eq!(Compression::of_file("x.mapping.json"), None);
        let t = resolve_types(
            &[
                "data".into(),
                "settings".into(),
                "alias".into(),
                "mapping".into(),
            ],
            &["alias".into()],
        )
        .unwrap();
        assert_eq!(t, vec!["settings", "mapping", "data"]);
        assert!(resolve_types(&["bogus".into()], &[]).is_err());
        assert!(resolve_types(&["data".into()], &["data".into()]).is_err());
        for bad in ["", ".", "..", "a/b", "a\\b", "-x", "a\nb"] {
            assert!(check_index_name(bad).is_err(), "{bad:?}");
        }
        check_index_name(".ds-logs-000001").unwrap();
    }

    #[test]
    fn mappings_body_shapes() {
        let plain = r#"{"idx":{"mappings":{"properties":{"a":{"type":"keyword"}}}}}"#;
        assert_eq!(
            mappings_body(plain).unwrap()["properties"]["a"]["type"],
            "keyword"
        );
        let stringified = serde_json::to_string(plain).unwrap();
        assert_eq!(
            mappings_body(&stringified).unwrap()["properties"]["a"]["type"],
            "keyword"
        );
        assert!(mappings_body("{}").is_err());
    }

    #[test]
    fn credentials_order_and_redaction() {
        let env = |k: &str| match k {
            "ELASTICDUMP_INPUT_USERNAME" => Some("envu".to_string()),
            "ELASTICDUMP_INPUT_PASSWORD" => Some("envp\n".to_string()),
            _ => None,
        };
        let (u, user, pass) = split_url("https://h:9200", "ELASTICDUMP_INPUT", None, &env).unwrap();
        assert_eq!(u.as_str(), "https://h:9200/");
        assert_eq!(
            (user.as_deref(), pass.as_deref()),
            (Some("envu"), Some("envp"))
        );
        let (u, user, pass) = split_url(
            "https://a:b%40c@h:9200/base",
            "ELASTICDUMP_INPUT",
            None,
            &env,
        )
        .unwrap();
        assert_eq!(u.as_str(), "https://h:9200/base/");
        assert_eq!((user.as_deref(), pass.as_deref()), (Some("a"), Some("b@c")));
        let file = ("fu".to_string(), "fp".to_string());
        let (_, user, _) =
            split_url("https://h:9200", "ELASTICDUMP_INPUT", Some(&file), &env).unwrap();
        assert_eq!(user.as_deref(), Some("fu"));
        assert!(split_url("ftp://h", "ELASTICDUMP_INPUT", None, &env).is_err());
    }

    #[test]
    fn record_counts_all_codecs() {
        let d = tempfile::tempdir().unwrap();
        let body = b"{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n";
        let plain = d.path().join("i.data.json");
        fs::write(&plain, body).unwrap();
        let gz = d.path().join("i.data.json.gz");
        let mut e = flate2::write::GzEncoder::new(
            fs::File::create(&gz).unwrap(),
            flate2::Compression::default(),
        );
        e.write_all(body).unwrap();
        e.finish().unwrap();
        let zst = d.path().join("j.data.json.zst");
        fs::write(&zst, zstd::stream::encode_all(&body[..], 3).unwrap()).unwrap();
        for p in [&plain, &gz, &zst] {
            assert_eq!(count_records(p).unwrap(), 3, "{}", p.display());
        }
    }

    #[test]
    fn discover_layouts_and_manifest() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        // oslogs.py flat files
        for f in ["a.settings.json", "a.mapping.json"] {
            fs::write(r.join(f), "{}").unwrap();
        }
        fs::write(r.join("a.data.json.gz"), b"x").unwrap();
        // grouped
        fs::create_dir_all(r.join("2026.10")).unwrap();
        for f in ["b.settings.json", "b.mapping.json", "b.data.json.zst"] {
            fs::write(r.join("2026.10").join(f), "{}").unwrap();
        }
        fs::write(r.join("notes.txt"), "x").unwrap();
        let found = discover(r).unwrap();
        let names: Vec<_> = found.iter().map(|f| f.index.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        assert_eq!(found[1].dir, r.join("2026.10"));
        assert!(found[1].data.as_ref().unwrap().ends_with("b.data.json.zst"));
        // duplicate across directories is an error
        fs::create_dir_all(r.join("dup")).unwrap();
        fs::write(r.join("dup").join("a.settings.json"), "{}").unwrap();
        assert!(discover(r).is_err());
        fs::remove_dir_all(r.join("dup")).unwrap();
        // two data files for one index is an error
        fs::write(r.join("a.data.json.zst"), b"x").unwrap();
        assert!(discover(r).is_err());
        fs::remove_file(r.join("a.data.json.zst")).unwrap();
        // a manifest wins and failed entries are skipped
        let mut m = Manifest {
            tool: "multielasticdump".into(),
            version: "t".into(),
            ..Default::default()
        };
        m.indices.insert(
            "a".into(),
            IndexEntry {
                dir: "".into(),
                types: vec!["data".into()],
                data_file: Some("a.data.json.gz".into()),
                records: Some(1),
                status: "done".into(),
                ..Default::default()
            },
        );
        m.indices.insert(
            "c".into(),
            IndexEntry {
                status: "failed".into(),
                ..Default::default()
            },
        );
        m.save(r).unwrap();
        let found = discover(r).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].index, "a");
        assert_eq!(found[0].records_hint, Some(1));
    }
}
