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
| endpoint_parse | 300 s | 1,253,924 | 4193 | ok |
| extract_batch | 300 s | 3,554,483 | 3443 | ok |
| ilm_to_ism_convert | 300 s | 2,906,168 | 1351 | ok |
| ini_auth_file | 300 s | 2,621,675 | 1161 | ok |
| json_split | 300 s | 2,407,718 | 223 | ok |
| parse_size | 300 s | 56,205,718 | 182 | ok |
| renamer_new_index | 300 s | 497,607 | 14079 | ok |

Also from L-505 (property test, not libFuzzer): F-505-2, `url_props::redaction_never_shows_the_password`
shrank to `http://a:/Qz%E5?\éK#@ziclmqln/us`: a password with `/` spills into the path, and the
percent-decode error quoted the "index name" (a password fragment). Fixed: when `@` is present but no
password was parsed, errors and the query-string warning quote no URL text. Seed kept in
`proptest-regressions/endpoint.txt`.

Result: all 10 targets ran 300 s with no open crash; 3 findings fixed (F-505-0, F-505-1, F-505-2).
Corpora and artefacts are not kept (regenerate with `scripts/fuzz/seed.py`).

## L-508 (TST-SUP-020): compressed input

| Target | Run | Execs | Coverage | Result |
|---|---|---|---|---|
| zstd_in | 300 s | 763,130 | 344 | ok (libzstd C code is not coverage-instrumented; the count is the Rust side) |
| gzip_in | 300 s | 1,110,153 | 1078 | ok |

Contract test `hostile_compressed_input_fails_cleanly`: concatenated zstd frames with a skippable frame and
gzip multi-member restore every document; truncated frames/members, trailing garbage, a 2^31 window and a
64 MiB RLE bomb value (`--maxDocMb 16`) exit 1 with a clear message and no output file.
`scripts/res/bombs.sh` (256 MB cgroup): 2 GiB value bomb (zst, 32000:1) exit 1 with the `--maxDocMb` error,
peak 70 MB; 2 GiB whitespace bombs (zst 32000:1, gzip 1000:1) exit 0, peak 4 MB; no OOM kill.

ruzstd (decode only), measured on a 49.6 MB NDJSON dump compressed to 7.7 MB: libzstd 1.5.7 about
1600 MB/s, ruzstd 0.7.3 about 480 MB/s (both far above restore throughput). ruzstd 0.8 needs rustc 1.87
(MSRV here is 1.85). Decision (recorded, for the owner): keep libzstd for decode as well, since it stays linked
for compression anyway, with the explicit window cap and these fuzz targets; switching the restore path
to ruzstd 0.7 is a small, optional change if untrusted `.zst` input becomes a primary threat.

## L-509 (TST-SUP-022): sonic-rs against serde_json

`sonic_vs_serde` (ASan, the cargo-fuzz default): `check_doc_line` accepts exactly the lines serde_json parses
as one object; on valid objects/arrays `to_object_iter` / `to_array_iter` / `get` return spans that re-parse to
serde_json's values (last duplicate wins in both).

| Run | Execs | Coverage | Result |
|---|---|---|---|
| 300 s | - | - | oracle gap: serde_json refuses unpaired surrogate escapes (`"\ud800"`, valid JSON; the engines accept it). Excluded from the comparison |
| 300 s | - | - | oracle gap: serde_json's `arbitrary_precision` reserves the key `$serde_json::private::Number` (a serde_json quirk; affects only metadata parsed with serde_json). Excluded |
| 300 s | 4,204,562 | 4246 | ok: no divergence, no sanitizer report |

Known, by design, already guarded: sonic-rs does not validate escapes inside skipped string values (F-505-1,
`check_escapes`), and `to_object_iter` stops at the closing brace without looking at trailing bytes
(`check_doc_line` rejects those). sonic-rs is pinned to `=0.5.10` in Cargo.toml so an upgrade re-runs this
target first. No divergence needed filing upstream; the 4 h nightly campaign is B.

## L-512 (TST-INJ-015): injection oracles

F-512-1 (fixed, found by the `url_segments` unit sample before the run): the URL parser drops tab, CR and LF
from path segments, so an index name `lo\ngs` (e.g. a metadata key from a file) would have addressed `logs`.
`EsClient::url` now refuses any segment with a control character.

| Target | Run | Execs | Coverage | Result |
|---|---|---|---|---|
| file_to_bulk | 300 s | 2,448,842 | 3970 | ok |
| url_segments | 300 s | 10,727,995 | 425 | ok |
| meta_names | 300 s | 54,495,383 | 147 | ok |
| extract_batch | 300 s | - | - | CRASH (see PROGRESS/notes) |
| renamer_new_index | 300 s | 574,383 | 13632 | ok |
