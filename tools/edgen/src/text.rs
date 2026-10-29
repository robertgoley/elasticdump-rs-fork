//! Small helpers: an order-1 Markov text model trained on a bundled corpus,
//! RFC 3339 timestamps without a date library, base36 ids.

use rand::Rng;
use std::sync::OnceLock;

/// Bundled training text (written for this project; no third-party content).
const CORPUS: &str = "request received from upstream gateway for user session \
started processing batch of documents in worker pool \
connection to backend database established after retry \
cache miss for key in region falling back to origin \
user authenticated successfully with token issued by identity provider \
slow query detected on table orders taking longer than threshold \
health check passed for service instance in zone \
scheduled job completed and wrote results to storage bucket \
failed to parse payload from client because field was missing \
retrying request after timeout waiting for response from cluster \
index refresh finished and segments were merged in background \
disk usage on node crossed the high watermark and shards were moved \
rate limit exceeded for api key so the request was rejected \
configuration reloaded from file and new settings were applied \
worker thread pool resized because queue length was growing \
garbage collection pause was longer than expected on node \
certificate for host will expire soon and must be renewed \
message published to topic and acknowledged by broker \
order created for customer and payment authorized by provider \
shipment dispatched from warehouse and tracking number assigned \
search returned results for query with filters applied by user \
upload completed and file checksum verified against manifest \
session expired for user and they were redirected to login page \
background task cleaned up temporary files older than seven days";

struct Model {
    words: Vec<&'static str>,
    next: Vec<Vec<u32>>,
}

fn model() -> &'static Model {
    static M: OnceLock<Model> = OnceLock::new();
    M.get_or_init(|| {
        let toks: Vec<&'static str> = CORPUS.split_whitespace().collect();
        let mut words: Vec<&'static str> = toks.clone();
        words.sort_unstable();
        words.dedup();
        let idx = |w: &str| words.binary_search(&w).unwrap() as u32;
        let mut next = vec![Vec::new(); words.len()];
        for p in toks.windows(2) {
            next[idx(p[0]) as usize].push(idx(p[1]));
        }
        for (i, n) in next.iter_mut().enumerate() {
            if n.is_empty() {
                n.push((i as u32 + 1) % words.len() as u32);
            }
        }
        Model { words, next }
    })
}

/// `n` words of Markov text appended to `out`.
pub fn sentence<R: Rng>(rng: &mut R, n: usize, out: &mut String) {
    let m = model();
    let mut w = rng.gen_range(0..m.words.len());
    // One RNG word gives four 16-bit choices (multiply-shift; the tiny bias
    // is irrelevant for filler text and keeps generation fast).
    let mut bits = 0u64;
    for i in 0..n {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(m.words[w]);
        let nx = &m.next[w];
        if i % 4 == 0 {
            bits = rng.next_u64();
        }
        let r = (bits >> ((i % 4) * 16)) & 0xffff;
        w = nx[((r * nx.len() as u64) >> 16) as usize] as usize;
    }
}

/// Days since 1970-01-01 -> (y, m, d) (Howard Hinnant's civil_from_days).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Epoch milliseconds -> `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn rfc3339_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let (y, mo, d) = civil(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

pub fn base36(mut v: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if v == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while v > 0 {
        out.push(D[(v % 36) as usize]);
        v /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

pub fn hex(b: &[u8]) -> String {
    const H: &[u8] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

pub fn hex_bytes<R: Rng>(rng: &mut R, n: usize) -> String {
    const H: &[u8] = b"0123456789abcdef";
    let mut s = String::with_capacity(n * 2);
    let mut buf = [0u8; 64];
    let n = n.min(64);
    rng.fill_bytes(&mut buf[..n]);
    for &b in &buf[..n] {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

/// UUID v4 text from the RNG.
pub fn uuid4<R: Rng>(rng: &mut R) -> String {
    let mut b: [u8; 16] = rng.r#gen();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex(&b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dates() {
        assert_eq!(rfc3339_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339_ms(1_761_739_200_123), "2025-10-29T12:00:00.123Z");
        assert_eq!(rfc3339_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
    #[test]
    fn b36() {
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
    }
}
