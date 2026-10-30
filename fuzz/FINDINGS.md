# Fuzz findings (L-505, TST-API-084)

Short smoke runs only (<= 300 s per target, one at a time, `-rss_limit_mb=1024`,
`-max_len=65536`); long campaigns are B tasks. Corpus seeded by `scripts/fuzz/seed.py`
(not committed). Build: `cargo +nightly fuzz build -O --target x86_64-unknown-linux-gnu`.

| Target | Run | Execs | Coverage | Result |
|---|---|---|---|---|
| bulk_encode_doc | 60 s | - | - | F-505-0: ASan heap-buffer-overflow (read) in sonic-rs 0.5 string serialisation, a deliberate same-page SIMD tail read. Fixed: sonic-rs `sanitize` feature in Cargo.toml |
| bulk_encode_doc | 60 s | - | - | harness bug: fed lines the write path rejects first (`check_doc_line`); harness now mirrors it |
| bulk_encode_doc | 60 s | - | - | F-505-1: `check_doc_line` accepted invalid `\u` escapes inside string values (sonic-rs skips values without validating), so one document could make the cluster reject a whole bulk request. Fixed: `check_escapes` |
| bulk_encode_doc | 300 s | 5,135,888 | 4311 | ok |
| bulk_handle_response | 300 s | 4,086,084 | 2154 | ok |
| codec_decode | 300 s | 8,094,539 | 2010 | ok |
