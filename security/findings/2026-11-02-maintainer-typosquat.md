# Maintainer and typosquat review — 2026-11-02, Grok Bot

Method: crates.io API (`/api/v1/crates/<c>/owners`, `/crates/<c>`, `/versions`), 1 request/s, custom User-Agent, for the
36 direct crates.io dependencies (from `cargo metadata`) plus notable transitive crates (ring, zstd-sys/safe,
libmimalloc-sys, hyper, h2, aws-lc-rs). cargo-supply-chain 0.3.8 is installed but needs the full crates.io DB dump;
the API was cheaper. Typosquat: Levenshtein distance 1 (after `_`→`-`) of all 269 Cargo.lock names vs ~50 popular crates.

## Owners (login list | all-time downloads | last update | newest)
| Crate | Owners | Downloads | Updated |
|---|---|---|---|
| anyhow | dtolnay | 1120839426 | 2026-07-18 1.0.104 |
| base64 | alicemaz,marshallpierce | 1872370922 | 2026-08-04 0.23.1 |
| bytes | carllerche,Darksonn,github:tokio-rs:core | 1222767384 | 2026-07-08 1.12.1 |
| bytesize | hyunsik,robjtede | 118428327 | 2026-08-02 2.7.0 |
| clap | kbknapp,github:rust-cli:maintainers,github:clap-rs:admins | 1294266378 | 2026-09-14 4.6.7 |
| console | mitsuhiko,djc | 404854391 | 2026-09-10 0.16.6 |
| env_logger | huonw,sfackler,sebasmagri,github:rust-lang-nursery:log-owners,github:env-logger-rs:publishers | 612438987 | 2026-06-25 0.11.11 |
| flate2 | rust-lang-owner | 810710984 | 2026-11-02 1.1.10 |
| http | carllerche,seanmonstar | 1198781350 | 2026-07-29 1.5.0 |
| indicatif | mitsuhiko,djc | 248465399 | 2026-07-01 0.18.6 |
| lipsum | mgeisler | 6897651 | 2024-03-13 0.9.1 |
| log | rust-lang-owner | 1465608580 | 2026-10-30 0.4.35 |
| memchr | BurntSushi | 1666387732 | 2026-07-08 2.8.3 |
| mimalloc | octavonce | 70632332 | 2026-05-22 0.1.52 |
| percent-encoding | SimonSapin,github:servo:cargo-publish,github:servo:rust-url | 1054575569 | 2025-08-21 2.3.2 |
| pkcs8 | tarcieri,newpavlov,github:rustcrypto:formats | 480251741 | 2026-04-27 0.11.0 |
| proptest | Centril,AltSysrq,github:proptest-rs:publish | 231738975 | 2026-10-10 1.12.0 |
| rand | dhardy,github:rust-random:maintainers | 2068490970 | 2026-09-20 0.10.3 |
| rand_chacha | dhardy,github:rust-random:maintainers | 1605487012 | 2026-02-02 0.10.0 |
| regex | BurntSushi,rust-lang-owner,github:rust-lang-nursery:regex-owners | 1337886565 | 2026-07-15 1.13.1 |
| reqwest | seanmonstar | 867048887 | 2026-09-08 0.13.5 |
| rust-ini | zonyitoo | 140824735 | 2025-08-30 0.21.3 |
| rustls | ctz,djc,github:rustls:publishers | 1073874232 | 2026-10-14 0.23.46 |
| rustls-native-certs | ctz,djc,github:rustls:publishers | 652371259 | 2026-10-13 0.8.5 |
| semver | dtolnay | 1152669133 | 2026-04-04 1.0.28 |
| serde | dtolnay,github:serde-rs:publish | 1635385243 | 2026-07-18 1.0.229 |
| serde_json | dtolnay,github:serde-rs:publish | 1540419905 | 2026-10-11 1.0.154 |
| sha2 | tarcieri,newpavlov,github:rustcrypto:hashes | 1159710598 | 2026-10-12 0.11.1 |
| sonic-rs | liuq19,shenyj3,github:cloudwego:volo-team | 8453558 | 2026-09-11 0.5.10 |
| tempfile | Stebalien | 952989583 | 2026-03-11 3.27.0 |
| tikv-jemallocator | BusyJay | 110323251 | 2026-05-25 0.7.0 |
| tokio | carllerche,Darksonn,github:tokio-rs:core | 1140815836 | 2026-10-21 1.54.1 |
| tokio-util | carllerche,Darksonn,github:tokio-rs:core | 912388348 | 2026-10-09 0.7.20 |
| url | SimonSapin,Hoverbear,seanmonstar,github:servo:cargo-publish,github:servo:rust-url | 1021055957 | 2026-01-05 2.5.8 |
| webpki-roots | ctz,djc,github:rustls:publishers | 885106716 | 2026-10-23 1.0.11 |
| zstd | gyscos | 457641425 | 2026-11-01 0.13.4 |
| ring | briansmith | 864165890 | 2025-03-11 0.17.14 |
| zstd-sys | gyscos | 420948292 | 2026-11-01 2.0.17+zstd.1.5.7 |
| zstd-safe | gyscos | 468091929 | 2026-11-01 7.4.0 |
| libmimalloc-sys | octavonce | 70679318 | 2026-05-22 0.1.49 |
| hyper | seanmonstar,github:hyperium:core | 1079031439 | 2026-10-06 1.12.0 |
| h2 | carllerche,seanmonstar,olix0r | 939273022 | 2026-10-06 0.4.20 |
| aws-lc-rs | justsmth,skmcgrail,crypto-alg,github:aws:aws-lc-rs-team | 292502197 | 2026-10-28 1.19.0 |

## Observations
- **Single-maintainer critical crates** (one human owner, no team): `ring` (briansmith; last release 0.17.14 on
  2025-03-11 — crypto core, low release cadence), `reqwest` (seanmonstar), `anyhow`/`semver` (dtolnay), `memchr` (BurntSushi),
  `tempfile` (Stebalien), `rust-ini` (zonyitoo), `zstd`/`zstd-sys`/`zstd-safe` (gyscos), `mimalloc`/`libmimalloc-sys`
  (octavonce), `tikv-jemallocator` (BusyJay), `lipsum` (mgeisler, dev-only, last release 2024). All long-standing, very
  high-download owners; risk is bus-factor/account takeover, not identity.
- **Recent owner changes:** the API does not expose owner history; no owner looked out of place for its project.
  `flate2` and `log` are owned by `rust-lang-owner` (rust-lang org) — expected.
- **Release timing:** `zstd-sys 2.0.17` / `zstd-safe 7.4.0` were published 2026-11-01 by the usual publisher (gyscos) as
  back-ports on the old major line after 2.1.2 / 8.2.0 (2026-10-16, which we use) — benign. Several crates updated in the
  last 2 weeks (tokio 1.54.1, rustls, webpki-roots, aws-lc-rs, log, zstd); Dependabot's 7-day cooldown already delays them.
- **Typosquat:** one distance-1 hit: `rend` vs `rand` — `rend` is rkyv's endianness crate (same author family as
  `rkyv`/`ptr_meta`/`munge`), pulled in by rkyv; benign. No other look-alikes.

**Suspicious findings: 0.**

## Follow-ups
- Run `cargo supply-chain publishers` with the DB dump in CI monthly to capture owner *changes* (diff against a committed list).
- Pin a cargo-vet `trusted` entry only for team-owned publishers; keep single-maintainer crypto/native crates (ring, zstd-sys,
  libmimalloc-sys) under explicit audits.
