//! edgen: seeded, byte-reproducible test data (TST-INF-009, TST-DATA-001).
//!
//! Document `i` is generated from its own ChaCha20 stream seeded with
//! `SHA-256("edgen/v1" ‖ profile ‖ seed ‖ i)`, so any range of the corpus can
//! be produced independently and the output is identical for every thread
//! count. Ids and routing use a second stream (`profile#meta`), so changing
//! `--ids` never changes document content.

pub mod convert;
pub mod edge;
pub mod json;
pub mod mapping;
pub mod profiles;
pub mod text;

use anyhow::{Result, bail};
use json::{J, esc};
use profiles::{Meta, Params};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use sha2::{Digest, Sha256};

pub const PROFILES: &[&str] = &[
    "logs", "metrics", "nested", "geo", "wide", "huge", "edge", "mixed",
];
/// `mixed`: weighted sub-profiles (no `edge`: it overrides ids and indices).
pub const MIXED: &[(&str, u32)] = &[("logs", 60), ("metrics", 20), ("nested", 10), ("geo", 10)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// One `_source` per line.
    Source,
    /// `{"_index","_id","_source"[,"_routing"]}` (elasticdump-rs --input=file).
    Elasticdump,
    /// `_bulk` action + source line (harness seeding).
    Bulk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ids {
    Det,
    Uuid,
    None,
}

#[derive(Clone, Debug)]
pub struct GenOpts {
    pub profile: String,
    pub seed: u64,
    pub format: Format,
    pub index: String,
    pub ids: Ids,
    pub routing_pct: u32,
    pub params: Params,
    pub edge_file_only: bool,
    pub edge_index_cases: bool,
}

impl GenOpts {
    pub fn new(profile: &str, seed: u64) -> Self {
        Self {
            profile: profile.into(),
            seed,
            format: Format::Elasticdump,
            index: format!("edgen-{profile}"),
            ids: Ids::Det,
            routing_pct: 10,
            params: Params::default(),
            edge_file_only: false,
            edge_index_cases: false,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !PROFILES.contains(&self.profile.as_str()) {
            bail!("unknown profile {} ({})", self.profile, PROFILES.join(", "));
        }
        if self.routing_pct > 100 {
            bail!("--routing-pct must be 0..=100");
        }
        if self.params.fields == 0 || self.params.fields > 10_000 {
            bail!("--fields must be 1..=10000");
        }
        if self.params.huge_min > self.params.huge_max {
            bail!("huge min > max");
        }
        Ok(())
    }
}

pub fn rng_for(seed: u64, stream: &str, i: u64) -> ChaCha20Rng {
    let mut h = Sha256::new();
    h.update(b"edgen/v1\0");
    h.update(stream.as_bytes());
    h.update([0]);
    h.update(seed.to_be_bytes());
    h.update(i.to_be_bytes());
    ChaCha20Rng::from_seed(h.finalize().into())
}

/// Reusable per-thread state.
pub struct Worker {
    j: J,
    meta_stream: String,
    edge_cases: Vec<&'static edge::Case>,
}

impl Worker {
    pub fn new(o: &GenOpts) -> Self {
        Self {
            j: J::with_capacity(4096),
            meta_stream: format!("{}#meta", o.profile),
            edge_cases: edge::active(o.edge_file_only, o.edge_index_cases),
        }
    }

    fn source(&mut self, o: &GenOpts, profile: &str, i: u64) -> Meta {
        let mut rng = rng_for(o.seed, profile, i);
        let j = &mut self.j;
        match profile {
            "logs" => profiles::logs(&mut rng, i, j),
            "metrics" => profiles::metrics(&mut rng, i, j),
            "nested" => profiles::nested(&mut rng, i, j),
            "geo" => profiles::geo(&mut rng, i, j),
            "wide" => profiles::wide(&mut rng, i, j, &o.params),
            "huge" => profiles::huge(&mut rng, i, j, &o.params),
            "edge" => {
                let c = self.edge_cases[(i % self.edge_cases.len() as u64) as usize];
                return edge::write(c, i, j);
            }
            _ => unreachable!(),
        }
        Meta::default()
    }

    /// Append the output record(s) for document `i` to `out`.
    pub fn doc(&mut self, o: &GenOpts, i: u64, out: &mut Vec<u8>) {
        self.j.clear();
        let meta = if o.profile == "mixed" {
            let mut r = rng_for(o.seed, "mixed#pick", i);
            let total: u32 = MIXED.iter().map(|x| x.1).sum();
            let mut x = r.gen_range(0..total);
            let mut sub = MIXED[0].0;
            for (p, w) in MIXED {
                if x < *w {
                    sub = p;
                    break;
                }
                x -= w;
            }
            // {"kind":sub, sub:{...}}
            let m = self.source(o, sub, i);
            let body = std::mem::take(&mut self.j.b);
            self.j.clear();
            self.j
                .obj()
                .kstr("kind", sub)
                .key(sub)
                .raw(std::str::from_utf8(&body).unwrap())
                .end_obj();
            m
        } else {
            let p = o.profile.clone();
            self.source(o, &p, i)
        };
        let mut mr = rng_for(o.seed, &self.meta_stream, i);
        let id = match (&meta.id, o.ids, meta.no_id) {
            (Some(id), _, _) => Some(id.clone()),
            (None, _, true) | (None, Ids::None, _) => None,
            (None, Ids::Det, _) => Some(format!("{}-{}-{}", o.profile, o.seed, text::base36(i))),
            (None, Ids::Uuid, _) => Some(text::uuid4(&mut mr)),
        };
        let routing = meta.routing.clone().or_else(|| {
            let r = mr.gen_range(0..100u32);
            let v = mr.gen_range(0..16u32);
            (r < o.routing_pct).then(|| format!("r{v}"))
        });
        let index = meta.index.as_deref().unwrap_or(&o.index);
        match o.format {
            Format::Source => out.extend_from_slice(&self.j.b),
            Format::Elasticdump => {
                out.extend_from_slice(b"{\"_index\":");
                esc(out, index);
                if let Some(id) = &id {
                    out.extend_from_slice(b",\"_id\":");
                    esc(out, id);
                }
                out.extend_from_slice(b",\"_source\":");
                out.extend_from_slice(&self.j.b);
                if let Some(r) = &routing {
                    out.extend_from_slice(b",\"_routing\":");
                    esc(out, r);
                }
                out.push(b'}');
            }
            Format::Bulk => {
                out.extend_from_slice(b"{\"index\":{\"_index\":");
                esc(out, index);
                if let Some(id) = &id {
                    out.extend_from_slice(b",\"_id\":");
                    esc(out, id);
                }
                if let Some(r) = &routing {
                    out.extend_from_slice(b",\"routing\":");
                    esc(out, r);
                }
                out.extend_from_slice(b"}}\n");
                out.extend_from_slice(&self.j.b);
            }
        }
        out.push(b'\n');
    }
}

/// Generate `[start, start+count)` into `sink` using `threads` workers;
/// chunks are written in order so the bytes never depend on `threads`.
pub fn generate(
    o: &GenOpts,
    start: u64,
    count: u64,
    threads: usize,
    sink: &mut dyn FnMut(&[u8], u64) -> Result<()>,
) -> Result<()> {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, mpsc};
    o.validate()?;
    let chunk: u64 = match o.profile.as_str() {
        "huge" => 1,
        "wide" | "edge" => 64,
        _ => 2048,
    };
    let n_chunks = count.div_ceil(chunk);
    let threads = threads.max(1);
    let next = Arc::new(AtomicU64::new(0));
    // In-flight window bounds memory: workers wait while too far ahead.
    let done = Arc::new(AtomicU64::new(0));
    let window = (threads as u64) * 4;
    let (tx, rx) = mpsc::sync_channel::<(u64, Vec<u8>, u64)>(threads * 2);
    std::thread::scope(|s| -> Result<()> {
        for _ in 0..threads {
            let (tx, next, done, o) = (tx.clone(), next.clone(), done.clone(), o.clone());
            s.spawn(move || {
                let mut w = Worker::new(&o);
                loop {
                    let c = next.fetch_add(1, Ordering::SeqCst);
                    if c >= n_chunks {
                        break;
                    }
                    while c >= done.load(Ordering::SeqCst) + window {
                        std::thread::sleep(std::time::Duration::from_micros(200));
                    }
                    let a = start + c * chunk;
                    let b = (a + chunk).min(start + count);
                    let mut buf = Vec::with_capacity(((b - a) as usize) * 600);
                    for i in a..b {
                        w.doc(&o, i, &mut buf);
                    }
                    if tx.send((c, buf, b - a)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut pending = BTreeMap::new();
        let mut want = 0u64;
        for (c, buf, n) in rx.iter() {
            pending.insert(c, (buf, n));
            while let Some((buf, n)) = pending.remove(&want) {
                sink(&buf, n)?;
                want += 1;
                done.store(want, Ordering::SeqCst);
            }
        }
        if want != n_chunks {
            bail!("generation stopped early ({want}/{n_chunks} chunks)");
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(o: &GenOpts, start: u64, count: u64, threads: usize) -> Vec<u8> {
        let mut out = Vec::new();
        generate(o, start, count, threads, &mut |b, _| {
            out.extend_from_slice(b);
            Ok(())
        })
        .unwrap();
        out
    }

    #[test]
    fn same_bytes_for_any_thread_count_and_range() {
        for p in ["logs", "metrics", "nested", "geo", "edge", "mixed"] {
            let o = GenOpts::new(p, 7);
            let a = run(&o, 0, 5000, 1);
            let b = run(&o, 0, 5000, 4);
            assert_eq!(a, b, "{p}");
            let mut c = run(&o, 0, 2500, 3);
            c.extend(run(&o, 2500, 2500, 2));
            assert_eq!(a, c, "{p}: ranges");
        }
    }

    #[test]
    fn every_line_is_valid_json_and_formats_agree() {
        for p in PROFILES.iter().filter(|p| **p != "huge") {
            let mut o = GenOpts::new(p, 1);
            o.params.fields = 50;
            let ed = run(&o, 0, 300, 2);
            // lone_surrogate is deliberately not strict JSON (serde_json rejects it).
            for l in ed
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty() && !l.windows(14).any(|w| w == b"lone_surrogate"))
            {
                let v: serde_json::Value = serde_json::from_slice(l).unwrap_or_else(|e| {
                    panic!(
                        "{p}: {e}: {}",
                        String::from_utf8_lossy(&l[..l.len().min(200)])
                    )
                });
                assert!(v["_source"].is_object(), "{p}");
            }
            o.format = Format::Bulk;
            let bulk = run(&o, 0, 300, 2);
            assert_eq!(bulk.iter().filter(|b| **b == b'\n').count(), 600, "{p}");
        }
    }

    #[test]
    fn ids_routing_and_seed() {
        let o = GenOpts::new("logs", 1);
        let a = String::from_utf8(run(&o, 0, 1000, 1)).unwrap();
        assert!(a.starts_with("{\"_index\":\"edgen-logs\",\"_id\":\"logs-1-0\""));
        let routed = a.lines().filter(|l| l.contains("\"_routing\"")).count();
        assert!((50..150).contains(&routed), "{routed}");
        let mut o2 = o.clone();
        o2.seed = 2;
        assert_ne!(run(&o, 0, 10, 1), run(&o2, 0, 10, 1));
        // id mode changes ids only, never the source
        let mut o3 = o.clone();
        o3.ids = Ids::Uuid;
        o3.format = Format::Source;
        let mut o4 = o.clone();
        o4.format = Format::Source;
        assert_eq!(run(&o3, 0, 100, 1), run(&o4, 0, 100, 1));
    }

    #[test]
    fn huge_docs_have_requested_size() {
        let mut o = GenOpts::new("huge", 1);
        o.params.huge_min = 200_000;
        o.params.huge_max = 300_000;
        let a = run(&o, 0, 3, 2);
        for l in a.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            assert!((200_000..320_000).contains(&l.len()), "{}", l.len());
        }
    }

    #[test]
    fn mappings_cover_profiles() {
        for p in PROFILES {
            let m = mapping::mapping(p, "es", 20);
            assert!(m["mappings"]["properties"].is_object(), "{p}");
        }
        assert_eq!(
            mapping::mapping("logs", "os", 1)["mappings"]["properties"]["labels"]["type"],
            "flat_object"
        );
    }
}
