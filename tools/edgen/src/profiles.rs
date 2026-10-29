//! Document profiles (TST-DATA-001). Each writes one `_source` into `j`
//! using only the per-document RNG, so document `i` is independent of all
//! others (counter-based seeding).

use crate::json::J;
use crate::text::{hex_bytes, rfc3339_ms, sentence};
use rand::Rng;
use rand_chacha::ChaCha20Rng;

/// 2026-01-01T00:00:00Z
pub const BASE_MS: i64 = 1_767_225_600_000;

#[derive(Clone, Debug)]
pub struct Params {
    /// `wide`: number of fields.
    pub fields: usize,
    /// `huge`: document size range in bytes.
    pub huge_min: usize,
    pub huge_max: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            fields: 1000,
            huge_min: 1 << 20,
            huge_max: 10 << 20,
        }
    }
}

/// Per-document metadata a profile may override (edge cases).
#[derive(Default, Debug, Clone)]
pub struct Meta {
    pub id: Option<String>,
    pub index: Option<String>,
    pub routing: Option<String>,
    /// Omit `_id` even when ids are on.
    pub no_id: bool,
}

const LEVELS: &[(&str, u32)] = &[
    ("INFO", 70),
    ("DEBUG", 12),
    ("WARN", 10),
    ("ERROR", 6),
    ("FATAL", 2),
];
const METHODS: &[&str] = &[
    "GET", "GET", "GET", "POST", "PUT", "DELETE", "HEAD", "PATCH",
];
const STATUS: &[(u64, u32)] = &[
    (200, 80),
    (201, 3),
    (204, 2),
    (301, 2),
    (304, 4),
    (400, 2),
    (401, 1),
    (403, 1),
    (404, 3),
    (429, 1),
    (500, 1),
];
const SERVICES: &[&str] = &[
    "checkout",
    "search",
    "auth",
    "catalog",
    "payments",
    "shipping",
    "gateway",
    "inventory",
];
const PATHS: &[&str] = &[
    "/api/v1/orders",
    "/api/v1/items",
    "/login",
    "/search",
    "/static/app.js",
    "/health",
    "/api/v2/cart",
    "/images/logo.png",
];
const UAS: &[&str] = &[
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_6) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:131.0) Gecko/20100101 Firefox/131.0",
    "curl/8.10.1",
    "Go-http-client/2.0",
    "python-requests/2.32.3",
];

fn weighted<T: Copy, R: Rng>(rng: &mut R, items: &[(T, u32)]) -> T {
    let total: u32 = items.iter().map(|x| x.1).sum();
    let mut r = rng.gen_range(0..total);
    for (v, w) in items {
        if r < *w {
            return *v;
        }
        r -= w;
    }
    items[0].0
}

fn pick<'a, R: Rng>(rng: &mut R, items: &'a [&'a str]) -> &'a str {
    items[rng.gen_range(0..items.len())]
}

fn ip<R: Rng>(rng: &mut R) -> String {
    if rng.gen_range(0..20) == 0 {
        let a: [u16; 4] = rng.r#gen();
        format!("2001:db8:{:x}:{:x}::{:x}:{:x}", a[0], a[1], a[2], a[3])
    } else {
        let a: [u8; 4] = rng.r#gen();
        format!(
            "{}.{}.{}.{}",
            10u8.wrapping_add(a[0] % 200),
            a[1],
            a[2],
            a[3].max(1)
        )
    }
}

pub fn logs(rng: &mut ChaCha20Rng, i: u64, j: &mut J) {
    // Monotonic with jitter: slot of 50 ms per doc, jitter inside the slot.
    let ts = BASE_MS + (i as i64) * 50 + rng.gen_range(0..50);
    let level = weighted(rng, LEVELS);
    let status = weighted(rng, STATUS);
    let mut msg = String::with_capacity(400);
    let words = rng.gen_range(8..120);
    sentence(rng, words, &mut msg);
    j.obj();
    j.kstr("@timestamp", &rfc3339_ms(ts));
    let mut tmp = String::with_capacity(32);
    {
        use std::fmt::Write;
        let _ = write!(tmp, "web-{:02}", rng.gen_range(0..40));
    }
    j.key("host").obj().kstr("name", &tmp).end_obj();
    j.key("service")
        .obj()
        .kstr("name", pick(rng, SERVICES))
        .end_obj();
    j.key("log").obj().kstr("level", level).end_obj();
    j.key("http").obj();
    j.key("request")
        .obj()
        .kstr("method", pick(rng, METHODS))
        .end_obj();
    j.key("response")
        .obj()
        .ku("status_code", status)
        .ku("bytes", rng.gen_range(0..2_000_000))
        .end_obj();
    j.end_obj();
    j.key("url")
        .obj()
        .kstr(
            "path",
            &format!("{}/{}", pick(rng, PATHS), rng.gen_range(0..100_000)),
        )
        .end_obj();
    j.key("client").obj().kstr("ip", &ip(rng)).end_obj();
    j.key("user_agent")
        .obj()
        .kstr("original", pick(rng, UAS))
        .end_obj();
    j.key("event")
        .obj()
        .key("duration")
        .u(rng.gen_range(10_000..5_000_000_000))
        .end_obj();
    j.key("trace")
        .obj()
        .kstr("id", &hex_bytes(rng, 16))
        .end_obj();
    j.key("labels")
        .obj()
        .kstr("env", if rng.gen_bool(0.9) { "prod" } else { "staging" })
        .kstr("team", pick(rng, SERVICES))
        .end_obj();
    j.key("tags").arr();
    for _ in 0..rng.gen_range(0..4) {
        j.str(pick(rng, &["edge", "internal", "beta", "canary", "batch"]));
    }
    j.end_arr();
    j.kstr("message", &msg);
    j.end_obj();
}

pub fn metrics(rng: &mut ChaCha20Rng, i: u64, j: &mut J) {
    let ts = BASE_MS + (i as i64) * 1000;
    j.obj();
    j.kstr("@timestamp", &rfc3339_ms(ts));
    j.key("host")
        .obj()
        .kstr("name", &format!("node-{:03}", rng.gen_range(0..200)))
        .end_obj();
    for g in ["cpu", "memory", "disk", "network"] {
        j.key(g).obj();
        for k in 0..8 {
            j.key(&format!("m{k}"));
            if k % 2 == 0 {
                j.fixed(rng.gen_range(0..1_000_000), 3);
            } else {
                j.u(rng.gen_range(0..u32::MAX as u64));
            }
        }
        j.end_obj();
    }
    j.end_obj();
}

pub fn nested(rng: &mut ChaCha20Rng, i: u64, j: &mut J) {
    let ts = BASE_MS + (i as i64) * 60_000;
    let mut t = String::new();
    {
        let n = rng.gen_range(4..12);
        sentence(rng, n, &mut t);
    }
    let mut body = String::new();
    {
        let n = rng.gen_range(20..80);
        sentence(rng, n, &mut body);
    }
    j.obj();
    j.kstr("title", &t)
        .kstr("body", &body)
        .kstr("created", &rfc3339_ms(ts));
    j.key("user")
        .obj()
        .ku("id", rng.gen_range(1..1_000_000))
        .kstr("name", &format!("user{}", rng.gen_range(1..100_000)))
        .end_obj();
    j.key("tags").arr();
    for _ in 0..rng.gen_range(1..5) {
        j.str(pick(
            rng,
            &[
                "rust",
                "elasticsearch",
                "opensearch",
                "json",
                "http",
                "tls",
                "linux",
            ],
        ));
    }
    j.end_arr();
    j.key("answers").arr();
    for a in 0..rng.gen_range(0..8) {
        let mut ab = String::new();
        {
            let n = rng.gen_range(10..50);
            sentence(rng, n, &mut ab);
        }
        j.obj()
            .ku("user", rng.gen_range(1..1_000_000))
            .kstr("body", &ab)
            .key("score")
            .i(rng.gen_range(-5..100));
        j.kstr("created", &rfc3339_ms(ts + (a + 1) * 3_600_000));
        j.key("comments").arr();
        for _ in 0..rng.gen_range(0..3) {
            let mut cb = String::new();
            {
                let n = rng.gen_range(3..15);
                sentence(rng, n, &mut cb);
            }
            j.obj()
                .ku("user", rng.gen_range(1..1_000_000))
                .kstr("text", &cb)
                .end_obj();
        }
        j.end_arr().end_obj();
    }
    j.end_arr();
    j.end_obj();
}

pub fn geo(rng: &mut ChaCha20Rng, i: u64, j: &mut J) {
    let lat = rng.gen_range(-90_000_000i64..=90_000_000);
    let lon = rng.gen_range(-180_000_000i64..=180_000_000);
    j.obj();
    j.kstr("name", &format!("place-{i}"));
    j.kstr(
        "country",
        pick(rng, &["DE", "US", "JP", "BR", "IN", "FR", "ZA", "AU"]),
    );
    j.ku("population", rng.gen_range(0..20_000_000));
    // Rotate through the geo_point encodings ES/OS accept.
    j.key("location");
    match i % 4 {
        0 => {
            j.obj()
                .key("lat")
                .fixed(lat, 6)
                .key("lon")
                .fixed(lon, 6)
                .end_obj();
        }
        1 => {
            let mut s = Vec::new();
            let mut t = J::with_capacity(32);
            t.fixed(lat, 6);
            s.extend_from_slice(&t.b);
            s.push(b',');
            t.clear();
            t.fixed(lon, 6);
            s.extend_from_slice(&t.b);
            j.str(std::str::from_utf8(&s).unwrap());
        }
        2 => {
            j.arr().fixed(lon, 6).fixed(lat, 6).end_arr();
        }
        _ => {
            let mut t = J::with_capacity(64);
            t.fixed(lon, 6);
            let lo = String::from_utf8(t.b.clone()).unwrap();
            t.clear();
            t.fixed(lat, 6);
            let la = String::from_utf8(t.b.clone()).unwrap();
            j.str(&format!("POINT ({lo} {la})"));
        }
    }
    // A small polygon around the point, as WKT or GeoJSON.
    let d = 10_000i64; // 0.01 degree
    let (la, lo) = (
        lat.clamp(-89_000_000, 89_000_000),
        lon.clamp(-179_000_000, 179_000_000),
    );
    let f = |v: i64| {
        let mut t = J::with_capacity(16);
        t.fixed(v, 6);
        String::from_utf8(t.b).unwrap()
    };
    j.key("area");
    if i % 2 == 0 {
        j.str(&format!(
            "POLYGON (({a} {b}, {c} {b}, {c} {e}, {a} {e}, {a} {b}))",
            a = f(lo),
            b = f(la),
            c = f(lo + d),
            e = f(la + d)
        ));
    } else {
        j.obj()
            .kstr("type", "polygon")
            .key("coordinates")
            .arr()
            .arr();
        for (x, y) in [
            (lo, la),
            (lo + d, la),
            (lo + d, la + d),
            (lo, la + d),
            (lo, la),
        ] {
            j.arr().fixed(x, 6).fixed(y, 6).end_arr();
        }
        j.end_arr().end_arr().end_obj();
    }
    j.end_obj();
}

pub fn wide(rng: &mut ChaCha20Rng, _i: u64, j: &mut J, p: &Params) {
    j.obj();
    for f in 0..p.fields {
        j.key(&format!("f{f:05}"));
        match f % 4 {
            0 => j.str(&format!("v{}", rng.gen_range(0..1000))),
            1 => j.u(rng.gen_range(0..1_000_000)),
            2 => j.fixed(rng.gen_range(0..100_000_000), 4),
            _ => j.bool(rng.r#gen()),
        };
    }
    j.end_obj();
}

pub fn huge(rng: &mut ChaCha20Rng, i: u64, j: &mut J, p: &Params) {
    let target = if p.huge_max > p.huge_min {
        rng.gen_range(p.huge_min..=p.huge_max)
    } else {
        p.huge_min
    };
    j.obj().ku("n", i).ku("bytes", target as u64);
    // Body: 64 KiB strings of printable ASCII in a disabled object.
    j.key("blob").obj().key("parts").arr();
    let mut left = target;
    let mut buf = vec![0u8; 65_536];
    while left > 0 {
        let n = left.min(65_536);
        rng.fill(&mut buf[..n]);
        for b in &mut buf[..n] {
            *b = b'a' + (*b % 26);
        }
        j.str(std::str::from_utf8(&buf[..n]).unwrap());
        left -= n;
    }
    j.end_arr().end_obj().end_obj();
}
