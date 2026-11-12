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
