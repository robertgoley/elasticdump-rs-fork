# Changelog

All notable changes to searchdump-rs. Versions follow [Semantic Versioning](https://semver.org/);
a version with a hyphen (`1.0.0-beta1`) is a pre-release. The release workflow
(`.github/workflows/release.yml`) uses the section for a version as the release notes of its draft
GitHub release.

## Unreleased (1.1)

Work in progress on the local `release/1.1` branch (roadmap: `FEATURES-TODO.md`, "Roadmap: 1.1
release"; order: `project/plan/ROADMAP-1.1-PLAN.md`). Not released, not yet A/B measured.

### Changed

- **Perf harness timer (1.1-5).** `scripts/load/measure.py` now blocks in `wait4` and samples
  `/proc` on a side thread, instead of polling `wait4` every 0.1 s. Wall times are no longer
  rounded up to the next ~0.1 s tick, so short cases (mapping, settings, small dumps) get
  meaningful ratios. Results carry `"timer": "blocking-wait4"`; beta1 numbers measured with the
  old harness are re-measured before any 1.1 A/B comparison. Binaries are unchanged.

- **Refresh cadence (1.1-1), new flag `--refreshInterval=<seconds>` (default `5`).** The writer
  used to refresh the target after every bulk batch, as Node elasticdump does; at L0 a single
  restore spent most of its wall time waiting for those refreshes. It now refreshes the indices
  it wrote at most every 5 s while writing (the clock starts at the first finished batch) and
  always once at the end of the run, including the `--concurrency 1` path, which before relied on
  the last batch's refresh. `--refreshInterval=0` restores the per-batch refresh (Node-identical
  request pattern); `--noRefresh` still sends no refresh and wins over `--refreshInterval`.
  Node compatibility: Node refreshes after every batch (`_data.js` `reindex()` after each
  write); this is a deliberate difference. After the run the same documents are visible; only
  readers watching *during* a restore see them up to the interval later. The goldens
  `data-restore` and `bulk-gzip` now run with `--refreshInterval=0` to keep their recorded
  per-batch refresh. NODE-DIFFERENCES.md #26. Perf suite: opt-in A/B cases `restore-norefresh`
  and `copy-norefresh` (`PERF_AB_CASES=1`; the default case manifest and so the baseline
  identity are unchanged).

- **`_doc` sort for scroll dumps (1.1-3), new opt-in flag `--scrollDocSort` (default off).**
  Adds `"sort":["_doc"]` (the cheapest scroll order) to a scroll body with no `sort` and a
  `match_all`/`match_none` query; skipped with an info log for PIT, `--searchAfter`, `--scrollId`,
  `--searchWithTemplate`, a user sort, scoring queries and `track_scores`/`min_score`/`rescore`.
  The injected `sort` member is stripped from written hits; `"_score": null` remains. Node
  compatibility: Node sets `sort: ['_doc']` on its request options object, not the body
  (`_data.js:115`), so it never actually sends the sort and writes `"_score": 1`; that is why the
  flag is opt-in and, without it, requests and dump lines are unchanged. NODE-DIFFERENCES.md #27.

- **Scroll slices capped to the shard count (1.1-4), new flags `--sliceCap` (default `true`) and
  `--sliceField=<field>`.** A sliced scroll with more slices than primary shards makes the engine
  filter every document of a shard per extra slice (ES 7.10, one shard: 2 slices 2.1-2.2 s vs
  0.8-0.9 s unsliced). A scroll dump now reads `index.number_of_shards` of the input indices and
  lowers `--slices` to the smallest count with a warning (one shard: unsliced). `--sliceCap=false`
  keeps the requested number. `--sliceField` adds `"field"` to every slice (numeric/date field
  with doc values, checked with `GET /<index>/_mapping/field/<field>`; a missing, non-numeric or
  doc-values-less field is an error before the first search) and disables the cap. PIT is not
  capped. When the settings or mapping cannot be read, a warning is printed and the flags are
  used as given. The live IT tests that exercise slicing on purpose pass `--sliceCap=false`.
  Node compatibility: none (Node has no slicing). Perf note: `dump-scroll-s2/s4/s8` on the
  one-shard perf index now run unsliced.

- **Compression runs on its own thread (1.1-9).** A compressed file output (`--fsCompress`,
  `--compressionType gzip|zstd`, split parts too) now feeds each part's encoder on a dedicated
  `searchdump-compress` thread through a 2-deep queue, so gzip/zstd no longer stalls the writer
  between batches. Input and output buffers are recycled (no allocation per 256 KiB drain). The
  compressed bytes are identical to 1.0 (same encoder, same input, same order; checked byte for
  byte against the inline encoder, including a proptest over chunk sizes). An encoder error
  surfaces on the next write, flush or seal; dropping a part (cancel, abort, error) stops its
  thread. The memory estimate counts 3 extra batches for compressed output. No flag. zstd
  multithreading (`zstdmt`) is not used.

### Fixed

- **No whole-cluster refresh (1.1-2).** A restore with no output index (documents keep their
  `_index`) used to send `POST /_refresh`, refreshing every index in the cluster after every
  batch. It now refreshes only the indices the run wrote (after `--prefix`/`--suffix`/rename),
  as `POST /a,b,…/_refresh?ignore_unavailable=true&allow_no_indices=true`, split so each URL stays
  under ~3 KB, and sends nothing when nothing was written. With an output index the request is
  unchanged. Node compatibility: Node elasticdump refreshes the whole cluster after every batch
  in this case (`_data.js` `reindex()` posts `${base.url}/_refresh`); this is a deliberate
  difference. The data visible after the run is the same. NODE-DIFFERENCES.md #25.

## 1.0.0-beta1

First tagged release under the new name, as a beta. Version 1.0.0 will follow after a full
regression run.

### Disclaimer

searchdump-rs is provided "as is", without warranty of any kind, as stated in the MIT license
(and the Apache-2.0 license of the code it is modelled on). The authors are not liable for any
damage from its use, including data loss, corruption, downtime or service disruption, in
production or elsewhere. It has been stable and fast in our testing, but no test suite covers
every cluster version, configuration, plugin and storage backend, and a beta release in particular
may still contain bugs. Test in a non-production environment first, and verify every backup and
restore yourself (restore it, compare document counts) before you rely on it. See
[Disclaimer](README.md#disclaimer).

### Changed

- **Renamed** from elasticdump-rs-fork to searchdump-rs (repository
  github.com/robertgoley/searchdump-rs). The binaries are now `searchdump` (was `elasticdump-rs`)
  and `searchdump-multi` (was `multielasticdump`); the library crate is `searchdump_rs`.
- Environment variables are now `SEARCHDUMP_*`. The old `ELASTICDUMP_*` names still work as
  deprecated aliases and print a one-time warning; when both are set, the new name wins.
- Minimum Rust version is 1.91 (`rust-toolchain.toml` pins 1.91.1).
- The default container image is the dhi.io Debian 13 image with a shell (`dhi-debian`, linux/amd64).
  The shell-less `dhi.io/static` image, the previous default, is still built as `dhi-static`.
- README rewritten: origin and credits (elasticdump-rs by TenYears, Node elasticdump), install,
  usage, compatibility notes, performance figures from recorded runs only, trademarks.

### Added

- **Object storage and SFTP** through OpenDAL 0.59: `s3://`, `b2://`, `sftp://` and `fs://` as
  `--input`/`--output` for `searchdump` and `searchdump-multi` (copy and staged dump/load),
  settings in `--inputStorageConfig`/`--outputStorageConfig` files or environment variables (no
  secrets in URLs). Tested against MinIO and RustFS (including SSE-C), OpenSSH and Dropbear SFTP,
  and the local filesystem; Backblaze B2 and AWS S3 are compile-tested only so far.
- `searchdump-multi` (formerly `multielasticdump`) ships in the release archives next to
  `searchdump`.
- Compatibility names in every container image and release archive: `elasticdump` and
  `elasticdump-rs` -> `searchdump`, `multielasticdump` -> `searchdump-multi`.
- Release workflow: archives for x86_64/aarch64 glibc and musl, SHA-256 checksums, a CycloneDX
  SBOM, GitHub build-provenance attestations, container images on ghcr.io, and a draft GitHub
  release for review.

### Known issues

- `sftp://` URLs do not work inside the container images. OpenDAL's SFTP backend runs the system OpenSSH `ssh`
  client, which the images do not include (Dropbear's client is not compatible). SFTP works with the host binaries
  and release archives on systems with OpenSSH installed. S3, B2 and `fs://` are not affected.
- Backblaze B2 (`b2://`) is compile-tested only. It has not been run against a real B2 account (pending owner
  credentials; see TESTING-TODO.md).
- The full performance-suite numbers for this tag were still being collected at release time. Earlier recorded
  runs are in [README Performance](README.md#performance); the v1.0.0-beta1 suite results will be added when that
  run finishes.

### Upgrade notes

- **Binaries.** `elasticdump-rs` is now `searchdump`, and `multielasticdump` is now `searchdump-multi`. Every
  container image and release archive also ships compatibility names: `elasticdump` and `elasticdump-rs` →
  `searchdump`, `multielasticdump` → `searchdump-multi`.
- **Environment variables.** The new names are `SEARCHDUMP_*`. The old `ELASTICDUMP_*` names still work as
  deprecated aliases and print a one-time warning; when both are set, the new name wins.
- **Manifest.** `searchdump-multi` still writes and reads the manifest file as `multielasticdump.json` (unchanged,
  so existing dump directories keep working).

### Security

- opendal 0.59.5 removes the dependency advisories RUSTSEC-2026-0194, RUSTSEC-2026-0195 and
  RUSTSEC-2026-0385; `deny.toml` and the cargo-audit configuration no longer ignore any advisory.
