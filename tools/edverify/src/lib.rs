//! `edverify` core: an order-independent multiset digest of a document set
//! (TST-INF-005).
//!
//! Per document: `h = SHA-256(index_rel ‖ 0x00 ‖ _id ‖ 0x00 ‖ routing ‖ 0x00 ‖ source)`
//! where `source` is the raw `_source` bytes (or, with `--canonical`, the
//! `_source` re-serialised with sorted keys and numbers kept as written).
//! The set digest is `(Σ h mod 2^256, XOR h, count)`. Documents are also
//! assigned to one of `B` buckets by `SHA-256(index_rel ‖ 0x00 ‖ _id)`, so a
//! changed, missing or duplicated document changes exactly one bucket and a
//! re-scan of that bucket names the `_id`.
//!
//! This crate deliberately does not depend on elasticdump-rs.

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest as _, Sha256};
use sonic_rs::JsonValueTrait;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

pub const DEFAULT_BUCKETS: usize = 1024;

/// How each document is hashed.
#[derive(Clone, Debug, Default)]
pub struct Opts {
    /// Re-serialise `_source` canonically (sorted keys) before hashing.
    pub canonical: bool,
    /// Use this index name for every document instead of its `_index`.
    pub index_as: Option<String>,
    /// Number of buckets (sub-digests).
    pub buckets: usize,
}

impl Opts {
    pub fn buckets(&self) -> usize {
        if self.buckets == 0 {
            DEFAULT_BUCKETS
        } else {
            self.buckets
        }
    }
}

/// 256-bit little-endian accumulator for Σ h mod 2^256.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct U256([u64; 4]);

impl U256 {
    pub fn add_bytes(&mut self, h: &[u8; 32]) {
        let mut carry = 0u128;
        for i in 0..4 {
            // h is read big-endian: limb 0 = least significant 8 bytes.
            let start = 32 - 8 * (i + 1);
            let limb = u64::from_be_bytes(h[start..start + 8].try_into().unwrap());
            let s = self.0[i] as u128 + limb as u128 + carry;
            self.0[i] = s as u64;
            carry = s >> 64;
        }
    }
    pub fn add(&mut self, o: &U256) {
        let mut carry = 0u128;
        for i in 0..4 {
            let s = self.0[i] as u128 + o.0[i] as u128 + carry;
            self.0[i] = s as u64;
            carry = s >> 64;
        }
    }
    pub fn to_hex(self) -> String {
        use std::fmt::Write;
        self.0
            .iter()
            .rev()
            .fold(String::with_capacity(64), |mut s, l| {
                let _ = write!(s, "{l:016x}");
                s
            })
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Bucket {
    pub count: u64,
    pub sum: U256,
}

/// Order-independent digest of a document set; merge partial digests freely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetDigest {
    pub count: u64,
    pub sum: U256,
    pub xor: [u8; 32],
    pub buckets: Vec<Bucket>,
    /// Lines that were not a JSON object (counted, never hashed).
    pub bad_lines: u64,
}

impl SetDigest {
    pub fn new(buckets: usize) -> Self {
        Self {
            count: 0,
            sum: U256::default(),
            xor: [0; 32],
            buckets: vec![Bucket::default(); buckets],
            bad_lines: 0,
        }
    }
    pub fn add(&mut self, bucket: usize, h: &[u8; 32]) {
        self.count += 1;
        self.sum.add_bytes(h);
        for (x, b) in self.xor.iter_mut().zip(h) {
            *x ^= b;
        }
        let b = &mut self.buckets[bucket];
        b.count += 1;
        b.sum.add_bytes(h);
    }
    pub fn merge(&mut self, o: &SetDigest) {
        self.count += o.count;
        self.bad_lines += o.bad_lines;
        self.sum.add(&o.sum);
        for (x, b) in self.xor.iter_mut().zip(o.xor.iter()) {
            *x ^= b;
        }
        for (a, b) in self.buckets.iter_mut().zip(o.buckets.iter()) {
            a.count += b.count;
            a.sum.add(&b.sum);
        }
    }
    /// One short value for logs: SHA-256(count ‖ sum ‖ xor), first 32 hex.
    pub fn short(&self) -> String {
        let mut s = Sha256::new();
        s.update(self.count.to_be_bytes());
        s.update(self.sum.to_hex().as_bytes());
        s.update(self.xor);
        hex(&s.finalize()[..16])
    }
    pub fn bucket_fp(b: &Bucket) -> String {
        let mut s = Sha256::new();
        s.update(b.count.to_be_bytes());
        s.update(b.sum.to_hex().as_bytes());
        hex(&s.finalize()[..8])
    }
    pub fn to_json(&self, specs: &[String], opts: &Opts) -> serde_json::Value {
        serde_json::json!({
            "tool": "edverify",
            "version": env!("CARGO_PKG_VERSION"),
            "inputs": specs,
            "mode": if opts.canonical { "canonical" } else { "raw" },
            "index_as": opts.index_as,
            "count": self.count,
            "bad_lines": self.bad_lines,
            "sum": self.sum.to_hex(),
            "xor": hex(&self.xor),
            "digest": self.short(),
            "buckets": self.buckets.len(),
            "bucket_fp": self.buckets.iter().map(Self::bucket_fp).collect::<Vec<_>>(),
        })
    }
}

pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

/// One parsed document: borrowed raw pieces.
pub struct Doc<'a> {
    pub index: String,
    pub id: String,
    pub routing: String,
    pub source: std::borrow::Cow<'a, [u8]>,
}

fn id_like(v: &sonic_rs::LazyValue) -> String {
    match v.as_str() {
        Some(s) => s.to_string(),
        None => v.as_raw_str().to_string(),
    }
}

/// Parse one line or hit object. A line with a `_source` member is an
/// elasticdump/hit record; anything else is a bare source document.
pub fn parse_doc<'a>(line: &'a [u8], opts: &Opts) -> Result<Doc<'a>> {
    let mut index = None;
    let mut id = None;
    let mut routing = None;
    let mut source: Option<std::borrow::Cow<'a, [u8]>> = None;
    for m in sonic_rs::to_object_iter(line) {
        let (k, v) = m.map_err(|e| anyhow!("not a JSON object: {e}"))?;
        match k.as_ref() {
            "_index" => index = Some(id_like(&v)),
            "_id" => id = Some(id_like(&v)),
            "_routing" | "routing" => {
                if !v.is_null() {
                    routing = Some(id_like(&v))
                }
            }
            "_source" => source = Some(sub_slice(line, v.as_raw_str().as_bytes())),
            _ => {}
        }
    }
    let raw: std::borrow::Cow<'a, [u8]> = match source {
        Some(s) => s,
        None => std::borrow::Cow::Borrowed(trim(line)),
    };
    let source = if opts.canonical {
        std::borrow::Cow::Owned(canonical(&raw)?)
    } else {
        raw
    };
    Ok(Doc {
        index: opts.index_as.clone().or(index).unwrap_or_default(),
        id: id.unwrap_or_default(),
        routing: routing.unwrap_or_default(),
        source,
    })
}

/// `raw` as a sub-slice of `line` when it points into it (the usual case:
/// sonic-rs hands out borrowed raw text), else an owned copy.
fn sub_slice<'a>(line: &'a [u8], raw: &[u8]) -> std::borrow::Cow<'a, [u8]> {
    let (l, r) = (line.as_ptr() as usize, raw.as_ptr() as usize);
    if r >= l && r + raw.len() <= l + line.len() {
        std::borrow::Cow::Borrowed(&line[r - l..r - l + raw.len()])
    } else {
        std::borrow::Cow::Owned(raw.to_vec())
    }
}

fn trim(b: &[u8]) -> &[u8] {
    let s = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let e = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(s, |e| e + 1);
    &b[s..e]
}

/// Sorted keys at every level; strings re-escaped by serde_json; numbers
/// kept exactly as written (arbitrary_precision).
pub fn canonical(raw: &[u8]) -> Result<Vec<u8>> {
    let v: serde_json::Value = serde_json::from_slice(raw).context("canonical: invalid JSON")?;
    let mut out = Vec::with_capacity(raw.len());
    write_canon(&v, &mut out);
    Ok(out)
}

fn write_canon(v: &serde_json::Value, out: &mut Vec<u8>) {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                serde_json::to_writer(&mut *out, k).unwrap();
                out.push(b':');
                write_canon(&m[k], out);
            }
            out.push(b'}');
        }
        Value::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canon(x, out);
            }
            out.push(b']');
        }
        other => serde_json::to_writer(&mut *out, other).unwrap(),
    }
}

pub fn doc_hash(d: &Doc) -> [u8; 32] {
    let mut s = Sha256::new();
    s.update(d.index.as_bytes());
    s.update([0]);
    s.update(d.id.as_bytes());
    s.update([0]);
    s.update(d.routing.as_bytes());
    s.update([0]);
    s.update(&d.source);
    s.finalize().into()
}

pub fn bucket_of(d: &Doc, n: usize) -> usize {
    let mut s = Sha256::new();
    s.update(d.index.as_bytes());
    s.update([0]);
    s.update(d.id.as_bytes());
    let h = s.finalize();
    (u64::from_be_bytes(h[..8].try_into().unwrap()) % n as u64) as usize
}

/// What a scan collects besides the digest: for `diff`, the per-id hashes of
/// documents that fall in the selected buckets.
#[derive(Default)]
pub struct Collect {
    pub buckets: Option<std::collections::HashSet<usize>>,
    pub docs: HashMap<(String, String), Vec<[u8; 32]>>,
}

/// Feed every line of a chunk (complete lines only) into `dig`/`col`.
pub fn digest_lines(chunk: &[u8], opts: &Opts, dig: &mut SetDigest, col: &mut Collect) {
    for line in chunk.split(|&b| b == b'\n') {
        if line.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        digest_one(line, opts, dig, col);
    }
}

pub fn digest_one(obj: &[u8], opts: &Opts, dig: &mut SetDigest, col: &mut Collect) {
    match parse_doc(obj, opts) {
        Ok(d) => {
            let h = doc_hash(&d);
            let b = bucket_of(&d, dig.buckets.len());
            dig.add(b, &h);
            if let Some(sel) = &col.buckets {
                if sel.contains(&b) {
                    col.docs.entry((d.index, d.id)).or_default().push(h);
                }
            }
        }
        Err(_) => dig.bad_lines += 1,
    }
}

pub fn open(path: &Path) -> Result<Box<dyn Read + Send>> {
    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let name = path.to_string_lossy();
    Ok(if name.ends_with(".gz") {
        Box::new(flate2::read::MultiGzDecoder::new(BufReader::with_capacity(
            1 << 20,
            f,
        )))
    } else if name.ends_with(".zst") || name.ends_with(".zstd") {
        Box::new(zstd::stream::read::Decoder::new(f)?)
    } else {
        Box::new(f)
    })
}

/// Digest a set of files (a dump, or all parts of a split dump) with
/// `threads` hashing workers. Memory is bounded by the chunk queue.
pub fn digest_files(
    paths: &[std::path::PathBuf],
    opts: &Opts,
    threads: usize,
    collect_buckets: Option<std::collections::HashSet<usize>>,
) -> Result<(SetDigest, Collect)> {
    use std::sync::{Arc, Mutex, mpsc};
    let threads = threads.max(1);
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(threads * 2);
    let rx = Arc::new(Mutex::new(rx));
    let mut workers = Vec::new();
    for _ in 0..threads {
        let rx = rx.clone();
        let opts = opts.clone();
        let sel = collect_buckets.clone();
        workers.push(std::thread::spawn(move || {
            let mut dig = SetDigest::new(opts.buckets());
            let mut col = Collect {
                buckets: sel,
                ..Default::default()
            };
            loop {
                let chunk = { rx.lock().unwrap().recv() };
                match chunk {
                    Ok(c) => digest_lines(&c, &opts, &mut dig, &mut col),
                    Err(_) => break,
                }
            }
            (dig, col)
        }));
    }
    let mut res = Ok(());
    'files: for p in paths {
        let r = match open(p) {
            Ok(r) => r,
            Err(e) => {
                res = Err(e);
                break;
            }
        };
        let mut br = BufReader::with_capacity(1 << 20, r);
        let mut chunk = Vec::with_capacity(4 << 20);
        loop {
            let before = chunk.len();
            match br.read_until(b'\n', &mut chunk) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    res = Err(anyhow!("read {}: {e}", p.display()));
                    break 'files;
                }
            }
            let _ = before;
            if chunk.len() >= 4 << 20
                && tx
                    .send(std::mem::replace(&mut chunk, Vec::with_capacity(4 << 20)))
                    .is_err()
            {
                break 'files;
            }
        }
        if !chunk.is_empty() && tx.send(chunk).is_err() {
            break;
        }
    }
    drop(tx);
    let mut dig = SetDigest::new(opts.buckets());
    let mut col = Collect {
        buckets: collect_buckets,
        ..Default::default()
    };
    for w in workers {
        let (d, c) = w.join().map_err(|_| anyhow!("worker panicked"))?;
        dig.merge(&d);
        for (k, mut v) in c.docs {
            col.docs.entry(k).or_default().append(&mut v);
        }
    }
    res?;
    Ok((dig, col))
}

/// Buckets whose fingerprints differ between two digests.
pub fn mismatched_buckets(a: &SetDigest, b: &SetDigest) -> Result<Vec<usize>> {
    if a.buckets.len() != b.buckets.len() {
        bail!(
            "bucket counts differ ({} vs {})",
            a.buckets.len(),
            b.buckets.len()
        );
    }
    Ok((0..a.buckets.len())
        .filter(|&i| a.buckets[i] != b.buckets[i])
        .collect())
}

#[derive(Debug, PartialEq, Eq)]
pub enum IdDiff {
    Missing {
        index: String,
        id: String,
        count: usize,
    },
    Extra {
        index: String,
        id: String,
        count: usize,
    },
    Changed {
        index: String,
        id: String,
    },
    CountDiffers {
        index: String,
        id: String,
        a: usize,
        b: usize,
    },
}

impl std::fmt::Display for IdDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdDiff::Missing { index, id, count } => {
                write!(f, "missing in B: {index}/{id} (x{count})")
            }
            IdDiff::Extra { index, id, count } => write!(f, "extra in B: {index}/{id} (x{count})"),
            IdDiff::Changed { index, id } => write!(f, "changed: {index}/{id}"),
            IdDiff::CountDiffers { index, id, a, b } => {
                write!(f, "duplicate/count differs: {index}/{id} A={a} B={b}")
            }
        }
    }
}

/// Compare the collected per-id hashes of A and B (sorted by key).
pub fn id_diffs(a: &Collect, b: &Collect, limit: usize) -> Vec<IdDiff> {
    let mut keys: Vec<&(String, String)> = a.docs.keys().chain(b.docs.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut out = Vec::new();
    for k in keys {
        if out.len() >= limit {
            break;
        }
        let (index, id) = (k.0.clone(), k.1.clone());
        match (a.docs.get(k), b.docs.get(k)) {
            (Some(x), None) => out.push(IdDiff::Missing {
                index,
                id,
                count: x.len(),
            }),
            (None, Some(y)) => out.push(IdDiff::Extra {
                index,
                id,
                count: y.len(),
            }),
            (Some(x), Some(y)) => {
                if x.len() != y.len() {
                    out.push(IdDiff::CountDiffers {
                        index,
                        id,
                        a: x.len(),
                        b: y.len(),
                    });
                } else {
                    let (mut x, mut y) = (x.clone(), y.clone());
                    x.sort();
                    y.sort();
                    if x != y {
                        out.push(IdDiff::Changed { index, id });
                    }
                }
            }
            (None, None) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dig(lines: &[&str], opts: &Opts) -> SetDigest {
        let mut d = SetDigest::new(opts.buckets());
        let mut c = Collect::default();
        digest_lines(lines.join("\n").as_bytes(), opts, &mut d, &mut c);
        d
    }

    #[test]
    fn order_independent_and_sensitive() {
        let o = Opts::default();
        let a = dig(
            &[
                r#"{"_index":"i","_id":"1","_source":{"a":1}}"#,
                r#"{"_index":"i","_id":"2","_source":{"a":2}}"#,
            ],
            &o,
        );
        let b = dig(
            &[
                r#"{"_index":"i","_id":"2","_source":{"a":2}}"#,
                r#"{"_index":"i","_id":"1","_source":{"a":1}}"#,
            ],
            &o,
        );
        assert_eq!(a, b);
        let c = dig(
            &[
                r#"{"_index":"i","_id":"2","_source":{"a":3}}"#,
                r#"{"_index":"i","_id":"1","_source":{"a":1}}"#,
            ],
            &o,
        );
        assert_ne!(a.sum, c.sum);
        assert_eq!(mismatched_buckets(&a, &c).unwrap().len(), 1);
        // a duplicate changes count and sum
        let d = dig(
            &[
                r#"{"_index":"i","_id":"1","_source":{"a":1}}"#,
                r#"{"_index":"i","_id":"1","_source":{"a":1}}"#,
                r#"{"_index":"i","_id":"2","_source":{"a":2}}"#,
            ],
            &o,
        );
        assert_eq!(d.count, 3);
        assert_ne!(a.sum, d.sum);
    }

    #[test]
    fn duplicate_pair_cancels_xor_but_not_sum() {
        let o = Opts::default();
        let one = dig(&[r#"{"_id":"1","_source":{}}"#], &o);
        let three = dig(
            &[
                r#"{"_id":"1","_source":{}}"#,
                r#"{"_id":"1","_source":{}}"#,
                r#"{"_id":"1","_source":{}}"#,
            ],
            &o,
        );
        assert_eq!(one.xor, three.xor);
        assert_ne!(one.sum, three.sum);
    }

    #[test]
    fn canonical_ignores_key_order_and_whitespace() {
        let o = Opts {
            canonical: true,
            ..Default::default()
        };
        let a = dig(
            &[r#"{"_id":"1","_source":{"b":1,"a":{"y":2,"x":12345678901234567890123}}}"#],
            &o,
        );
        let b = dig(
            &[r#"{"_id":"1","_source":{ "a" : {"x":12345678901234567890123,"y":2}, "b":1 }}"#],
            &o,
        );
        assert_eq!(a, b);
        let raw = Opts::default();
        assert_ne!(
            dig(&[r#"{"_id":"1","_source":{"b":1,"a":2}}"#], &raw),
            dig(&[r#"{"_id":"1","_source":{"a":2,"b":1}}"#], &raw)
        );
    }

    #[test]
    fn index_as_and_routing() {
        let o = Opts {
            index_as: Some("x".into()),
            ..Default::default()
        };
        assert_eq!(
            dig(&[r#"{"_index":"a","_id":"1","_source":{}}"#], &o),
            dig(&[r#"{"_index":"b","_id":"1","_source":{}}"#], &o)
        );
        let r = Opts::default();
        assert_ne!(
            dig(&[r#"{"_id":"1","_routing":"r","_source":{}}"#], &r),
            dig(&[r#"{"_id":"1","_source":{}}"#], &r)
        );
    }

    #[test]
    fn sum_wraps_mod_2_256() {
        let mut u = U256([u64::MAX; 4]);
        let mut one = [0u8; 32];
        one[31] = 1;
        u.add_bytes(&one);
        assert_eq!(u, U256([0; 4]));
    }

    #[test]
    fn diff_names_ids() {
        let o = Opts::default();
        let sel: std::collections::HashSet<usize> = (0..o.buckets()).collect();
        let mut ca = Collect {
            buckets: Some(sel.clone()),
            ..Default::default()
        };
        let mut cb = Collect {
            buckets: Some(sel),
            ..Default::default()
        };
        let mut da = SetDigest::new(o.buckets());
        let mut db = SetDigest::new(o.buckets());
        digest_lines(b"{\"_id\":\"1\",\"_source\":{\"a\":1}}\n{\"_id\":\"2\",\"_source\":{}}\n{\"_id\":\"3\",\"_source\":{}}", &o, &mut da, &mut ca);
        digest_lines(b"{\"_id\":\"1\",\"_source\":{\"a\":2}}\n{\"_id\":\"3\",\"_source\":{}}\n{\"_id\":\"3\",\"_source\":{}}\n{\"_id\":\"4\",\"_source\":{}}", &o, &mut db, &mut cb);
        let d = id_diffs(&ca, &cb, 10);
        assert_eq!(d.len(), 4, "{d:?}");
        assert!(matches!(&d[0], IdDiff::Changed { id, .. } if id == "1"));
        assert!(matches!(&d[1], IdDiff::Missing { id, .. } if id == "2"));
        assert!(matches!(&d[2], IdDiff::CountDiffers { id, a: 1, b: 2, .. } if id == "3"));
        assert!(matches!(&d[3], IdDiff::Extra { id, .. } if id == "4"));
    }
}
