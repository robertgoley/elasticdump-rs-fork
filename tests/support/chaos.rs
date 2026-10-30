//! Reproducible randomness for chaos/fault tests (INF-013).
//!
//! Every chaos test takes its seed from [`seed`], which honours `CHAOS_SEED`
//! (decimal or `0x` hex) and otherwise picks one from the clock. The seed is
//! always printed to stderr, so a failure in CI can be replayed exactly:
//! `CHAOS_SEED=<n> cargo nextest run --profile chaos -E 'test(=<name>)'`.

/// The run's seed: `CHAOS_SEED` if set, else derived from the clock. Printed.
pub fn seed(test: &str) -> u64 {
    let s = match std::env::var("CHAOS_SEED") {
        Ok(v) => {
            parse(&v).unwrap_or_else(|| panic!("CHAOS_SEED={v:?} is not a u64 (decimal or 0x hex)"))
        }
        Err(_) => {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            t.as_nanos() as u64 ^ u64::from(std::process::id()).rotate_left(32)
        }
    };
    eprintln!(
        "CHAOS_SEED={s} (replay: CHAOS_SEED={s} cargo nextest run --profile chaos -E 'test(={test})')"
    );
    s
}

pub fn parse(v: &str) -> Option<u64> {
    let v = v.trim();
    match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => v.parse().ok(),
    }
}

/// SplitMix64: tiny, dependency-free, identical sequence for the same seed.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
    /// True with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
}
