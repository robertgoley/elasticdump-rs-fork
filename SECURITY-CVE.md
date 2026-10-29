# Security: dependency advisories and image CVEs

This file tracks every advisory and image CVE that the scanners below report for elasticdump-rs, with
its status. Status values:

- **open**: affects a shipped artifact and has no fix yet;
- **resolved**: fixed, and the "Resolved in" column says where;
- **not-affected**: present, with a justification.

Related docs: [TESTING-MATRIX.md](TESTING-MATRIX.md) and [NODE-DIFFERENCES.md](NODE-DIFFERENCES.md).

## Last scan

| Date (America/New_York) | Commit | Tool (DB) | Target | Result |
|---|---|---|---|---|
| 2026-10-29 | ae2531f | cargo-audit 0.22.2 (RustSec advisory-db, 1386 advisories) | Cargo.lock (271 crates) | **0 vulnerabilities, 0 warnings** |
| 2026-10-29 | ae2531f | cargo-deny 0.20.2, `check advisories bans licenses sources` | Cargo.lock + deny.toml | **advisories ok, bans ok, licenses ok, sources ok** |
| 2026-10-29 | c620b80 | grype (DB v6.1.10, built 2026-10-29 02:23 EDT) | `make image-alpine` (rust:1.99.1-alpine3.24 → alpine:3.24.3) | **0 Critical, 0 High**, 3 Medium (one CVE, see below) |
| 2026-10-29 | c620b80 | trivy | `make image-alpine` | **0** (all severities) |
| 2026-10-29 | ae2531f | grype | `make image-public` (rust:1.99-trixie → debian:trixie-slim) | 54 High, 47 Medium, 10 Low, 46 Negligible (157 matches, 0 fixable) |
| 2026-10-29 | ae2531f | trivy | `make image-public` | 43 HIGH, 58 MEDIUM, 60 LOW, 1 UNKNOWN (162) |
| – | – | – | `make image` (dhi.io bases, the default) | **not scanned**: dhi.io needs a login that this environment does not have (see below) |

## Before / after

| Scan | Before (2d1ab85) | After |
|---|---|---|
| `cargo audit` | 24 vulnerabilities + 17 warnings (2 unmaintained, 15 unsound) | 0 + 0 (ae2531f) |
| `cargo deny check advisories bans licenses sources` | advisories: 13 vulnerability, 13 unsound, 2 unmaintained errors; licenses: 1 rejected (tiny-keccak CC0-1.0); no deny.toml before | all four checks ok (ae2531f) |
| Image, grype High+Critical | Debian 13 public fallback: 54 | Alpine/musl fallback: 0 |
| Image, trivy HIGH+CRITICAL | Debian 13 public fallback: 43 | Alpine/musl fallback: 0 |

## Scanning method

- **Rust dependencies**
  - `cargo audit` (RustSec database, fetched at scan time) reads Cargo.lock, which is committed.
  - `cargo deny check advisories bans licenses sources` uses [deny.toml](deny.toml):
    - advisories: yanked, unmaintained and unsound crates are all errors;
    - licenses: an explicit allow list;
    - bans: wildcard versions are denied, duplicate versions only warn;
    - sources: crates.io only.
  - CI runs `cargo audit --deny warnings`, so a new unmaintained or unsound warning also fails the job.
- **Container image**
  - `make save` writes `image.tar`; `make scan` runs `grype --fail-on high` and `trivy --severity HIGH,CRITICAL --exit-code 1` on it.
  - Both scanners see the OS packages only. The Rust binary is not built with `cargo auditable`, so they cannot see the crates inside it; cargo-audit covers those.
- **SBOMs**
  - `make sbom` uses syft to write `sbom-image.cdx.json` (CycloneDX) and `sbom-image.spdx.json` (SPDX).
  - CI also runs `cargo cyclonedx` to produce the crate SBOM (`sbom-cargo.cdx.json`).
  - SBOMs are uploaded as workflow artifacts and are not committed.
- **CI**
  - [.github/workflows/security.yml](.github/workflows/security.yml) runs on push to `main`/`rewrite`, on manual dispatch, and weekly.
  - It fails on any RustSec advisory or warning, a cargo-deny violation, or a HIGH/CRITICAL image CVE.
  - It never logs in to push and never publishes images. The dhi.io login is pull-only and is used only when the manual `base=dhi` input is chosen.
- **Reproduce locally**
  - `cargo audit`
  - `cargo deny check advisories bans licenses sources`
  - `make image-alpine scan sbom`

## Rust dependency advisories (Cargo.lock)

All entries below were reported against Cargo.lock at 2d1ab85. Each was fixed by upgrading or replacing the crate.

| ID | CVE | Crate (before) | Kind / severity | Title | Status | Resolved by | Commit |
|---|---|---|---|---|---|---|---|
| RUSTSEC-2026-0342 | – | quinn-proto 0.11.10 | vulnerability; no CVSS in advisory | Zero-length DATAGRAM frames bypass the receive buffer limit in quinn-proto | resolved | 0.11.19: `cargo update` (transitive, reqwest) | ae2531f |
| RUSTSEC-2026-0338 | – | quinn-proto 0.11.10 | vulnerability; no CVSS in advisory | Remote peer can panic quinn-proto with zero-length STREAM frames during unordered reads | resolved | 0.11.19: `cargo update` (transitive, reqwest) | ae2531f |
| RUSTSEC-2026-0037 | CVE-2026-31812 | quinn-proto 0.11.10 | vulnerability; `CVSS:4.0/AV:N/AC:L/AT:N/PR:N/UI:N/VC:N/VI:N/VA:H/SC:N/SI:N/SA:N` | Denial of service in Quinn endpoints | resolved | 0.11.19: `cargo update` (transitive, reqwest) | ae2531f |
| RUSTSEC-2026-0339 | – | quinn-proto 0.11.10 | vulnerability; no CVSS in advisory | Unbounded growth of pending connection ID retirements in quinn-proto | resolved | 0.11.19: `cargo update` (transitive, reqwest) | ae2531f |
| RUSTSEC-2026-0340 | – | quinn-proto 0.11.10 | vulnerability; `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H` | Peer minimum ACK delay can panic quinn-proto when ACK_FREQUENCY transmission is enabled | resolved | 0.11.19: `cargo update` (transitive, reqwest) | ae2531f |
| RUSTSEC-2026-0185 | CVE-2026-25800 | quinn-proto 0.11.10 | vulnerability; `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H` | Remote memory exhaustion in quinn-proto from unbounded out-of-order stream reassembly | resolved | 0.11.19: `cargo update` (transitive, reqwest) | ae2531f |
| RUSTSEC-2026-0235 | – | rkyv 0.8.10 | vulnerability; no CVSS in advisory | Insufficient archive validation can cause out-of-bounds reads in archives containing Rc/Arc | resolved | 0.8.18: `cargo update` (transitive) | ae2531f |
| RUSTSEC-2026-0001 | – | rkyv 0.8.10 | vulnerability; no CVSS in advisory | Potential Undefined Behaviors in `Arc<T>`/`Rc<T>` impls of `from_value` on OOM | resolved | 0.8.18: `cargo update` (transitive) | ae2531f |
| RUSTSEC-2026-0234 | – | rkyv 0.8.10 | vulnerability; no CVSS in advisory | Insufficient archive validation can cause out-of-bounds reads in archives containing hash tables | resolved | 0.8.18: `cargo update` (transitive) | ae2531f |
| RUSTSEC-2026-0233 | – | rkyv 0.8.10 | vulnerability; no CVSS in advisory | Crafted archives can cause a use-after-free during deserialization | resolved | 0.8.18: `cargo update` (transitive) | ae2531f |
| RUSTSEC-2026-0349 | – | rustls 0.23.26 | vulnerability; `CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:L/I:N/A:N` | Unauthenticated ServerHello can downgrade later connections to classical key exchange | resolved | 0.23.46: `cargo update` | ae2531f |
| RUSTSEC-2026-0348 | – | rustls 0.23.26 | vulnerability; `CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:N/I:N/A:L` | Brotli certificate decompression can allocate up to 1 GiB before authentication | resolved | 0.23.46: `cargo update` | ae2531f |
| RUSTSEC-2026-0346 | – | rustls 0.23.26 | vulnerability; `CVSS:3.1/AV:L/AC:H/PR:H/UI:R/S:U/C:H/I:N/A:N` | `Debug` impls for client session values expose TLS secrets | resolved | 0.23.46: `cargo update` | ae2531f |
| RUSTSEC-2026-0285 | – | rustls 0.23.26 | vulnerability; `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:L/I:N/A:N` | TLS 1.3 handshake messages incorrectly accepted across encryption level boundaries | resolved | 0.23.46: `cargo update` | ae2531f |
| RUSTSEC-2026-0382 | CVE-2026-111943 | rustls-webpki 0.103.1 | vulnerability; no CVSS in advisory | Certificate verification may accept distrusted intermediates | resolved | 0.103.16: `cargo update` | ae2531f |
| RUSTSEC-2026-0104 | CVE-2026-93599 | rustls-webpki 0.103.1 | vulnerability; no CVSS in advisory | Reachable panic in certificate revocation list parsing | resolved | 0.103.16: `cargo update` | ae2531f |
| RUSTSEC-2026-0098 | CVE-2026-93600 | rustls-webpki 0.103.1 | vulnerability; no CVSS in advisory | Name constraints for URI names were incorrectly accepted | resolved | 0.103.16: `cargo update` | ae2531f |
| RUSTSEC-2026-0049 | CVE-2026-93602 | rustls-webpki 0.103.1 | vulnerability; no CVSS in advisory | CRLs not considered authoritative by Distribution Point due to faulty matching logic | resolved | 0.103.16: `cargo update` | ae2531f |
| RUSTSEC-2026-0099 | CVE-2026-93601 | rustls-webpki 0.103.1 | vulnerability; no CVSS in advisory | Name constraints were accepted for certificates asserting a wildcard name | resolved | 0.103.16: `cargo update` | ae2531f |
| RUSTSEC-2026-0009 | CVE-2026-25727 | time 0.3.41 | vulnerability; `CVSS:4.0/AV:N/AC:H/AT:N/PR:L/UI:A/VC:N/VI:N/VA:H/SC:N/SI:N/SA:H` | Denial of Service via Stack Exhaustion | resolved | removed (no longer in the tree): dropped the `elasticsearch` 8.19.0-alpha.1 dev-dependency (serde_with → time); the in-tree fix needs Rust 1.88, above the 1.85 MSRV | ae2531f |
| RUSTSEC-2026-0381 | CVE-2026-111943 | webpki-roots 0.26.8 | vulnerability; no CVSS in advisory | Bundled root certificates allow trust in distrusted emSign intermediates | resolved | 1.0.11: direct dependency bumped 0.26 → 1 | ae2531f |
| RUSTSEC-2026-0391 | – | zstd 0.13.3 | vulnerability; no CVSS in advisory | Data from an abandoned stream leaks into the next stream on a reused context | resolved | 0.14.3: direct dependency bumped 0.13 → 0.14 | ae2531f |
| RUSTSEC-2026-0398 | – | zstd-safe 7.3.0 | vulnerability; no CVSS in advisory | `Seekable::decompress` sets a `Vec` length beyond its capacity for offsets past the end | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0403 | – | zstd-safe 7.3.0 | vulnerability; no CVSS in advisory | Uninitialized memory exposed when streaming decompression of a legacy frame fails | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2025-0119 | – | number_prefix 0.4.0 | unmaintained (warning) | number_prefix crate is unmaintained | resolved | removed (indicatif 0.18 uses unit-prefix): indicatif 0.17 → 0.18, console 0.15 → 0.16 | ae2531f |
| RUSTSEC-2025-0134 | – | rustls-pemfile 2.2.0 | unmaintained (warning) | rustls-pemfile is unmaintained | resolved | removed (rustls-pki-types `PemObject`): replaced in `src/http/tls.rs` | ae2531f |
| RUSTSEC-2026-0190 | – | anyhow 1.0.102 | unsound (warning) | Unsoundness in `Error::downcast_mut()` | resolved | 1.0.104: `cargo update` | ae2531f |
| RUSTSEC-2026-0097 | – | rand 0.8.5 | unsound (warning) | Rand is unsound with a custom logger using `rand::rng()` | resolved | 0.8.8 / 0.10.3: `cargo update` (rand 0.8 is a lipsum dev-dependency) | ae2531f |
| RUSTSEC-2026-0097 | – | rand 0.9.1 | unsound (warning) | Rand is unsound with a custom logger using `rand::rng()` | resolved | 0.8.8 / 0.10.3: `cargo update` (rand 0.8 is a lipsum dev-dependency) | ae2531f |
| RUSTSEC-2026-0122 | – | rkyv 0.8.10 | unsound (warning) | Potential use-after-free due to lack of panic safety in `InlineVec::clear` and `SerVec::clear` | resolved | 0.8.18: `cargo update` (transitive) | ae2531f |
| RUSTSEC-2026-0386 | – | tokio 1.44.2 | unsound (warning) | `AsyncFd::new` and related constructors violate I/O safety | resolved | 1.54.1: `cargo update` | ae2531f |
| RUSTSEC-2026-0396 | – | zstd-safe 7.3.0 | unsound (warning) | `Seekable` reads a NULL or out-of-bounds seek table | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0397 | – | zstd-safe 7.3.0 | unsound (warning) | `OutBuffer::around_pos` exposes uninitialized memory | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0405 | – | zstd-safe 7.3.0 | unsound (warning) | Out-of-bounds read with an out-of-range `InBuffer::pos` | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0401 | – | zstd-safe 7.3.0 | unsound (warning) | Out-of-bounds read in `train_from_buffer` when sample sizes overflow | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0399 | – | zstd-safe 7.3.0 | unsound (warning) | Use-after-free through `init_using_cdict` and `init_using_ddict` | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0402 | – | zstd-safe 7.3.0 | unsound (warning) | Heap buffer overflow in `CCtx::try_clone` on a multithreaded context | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0406 | – | zstd-safe 7.3.0 | unsound (warning) | Use-after-free when changing a multithreaded context with an unfinished frame | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0404 | – | zstd-safe 7.3.0 | unsound (warning) | Double free in `DCtx::try_clone` | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0400 | – | zstd-safe 7.3.0 | unsound (warning) | Use-after-free of a dictionary after a one-shot call on a context with an open stream | resolved | 8.2.0: via zstd 0.14 | ae2531f |
| RUSTSEC-2026-0395 | – | zstd-safe 7.3.0 | unsound (warning) | Use-after-free with `CParameter::StableInBuffer` | resolved | 8.2.0: via zstd 0.14 | ae2531f |

Related cargo-deny finding:

- **License: tiny-keccak 2.0.2 is CC0-1.0.**
  - It reaches the build through rust-ini → ordered-multimap → dlv-list → const-random.
  - CC0-1.0 is a permissive public-domain dedication, so it was added to the allow list in deny.toml.
  - Commit: ae2531f.

## Container image CVEs

### Runtime bases

| Build | Builder | Runtime | libc | Scanned |
|---|---|---|---|---|
| `make image` (default) | `dhi.io/rust:1.99.1-debian13-dev` | `dhi.io/debian-base:trixie-debian13-dev` | glibc 2.41 / glibc 2.41 | no (login needed) |
| `make image-public` | `docker.io/library/rust:1.99-trixie` | `docker.io/library/debian:trixie-slim` | glibc 2.41 / glibc 2.41 | yes: 54 High (grype) |
| `make image-alpine` | `docker.io/library/rust:1.99.1-alpine3.24` | `docker.io/library/alpine:3.24.3`, plus `bash` and `ca-certificates` | musl / musl | yes: 0 High or Critical |

About the dhi.io bases:

- Docker's catalog lists `dhi.io/debian-base:trixie-debian13-dev` at 0 Critical / 0 High / 0 Medium / 1 Low / 0 Unspecified.
- This could not be verified here: `podman pull dhi.io/...` fails with `authentication required`.
- To check it, run `podman login dhi.io`, then `make image scan`.

### Alpine image (`make image-alpine`)

| ID | Component | Severity | Status | Justification / resolution |
|---|---|---|---|---|
| CVE-2025-60876 | busybox 1.37.0-r31, busybox-binsh, ssl_client | Medium (grype, CVSS 6.5); trivy does not report it | not-affected; no Alpine fix yet | The flaw is in the busybox `wget` applet (and `ssl_client`, which only `wget` uses). Nothing in the image calls `wget`: the tool does HTTP itself through reqwest/rustls, and the entrypoint is the Rust binary. Re-check at the next scan. |

### Debian 13 public fallback (`make image-public`)

- All 157 grype matches are Debian `wont-fix` or `not-fixed`, so none can be fixed by upgrading packages.
- The HIGH/CRITICAL findings are listed below (15 distinct CVEs, grouped by CVE).
- They come from the base OS (util-linux, glibc, gcc runtime libs, ncurses, perl-base, zlib, acl, systemd libs). They do not come from elasticdump-rs.
- The binary links only glibc (libc, libm) and libgcc_s. Even so, glibc CVE-2026-19499 and CVE-2026-5435 cannot be ruled out for a dynamically linked program, which is why this base is not recommended.
- Status: **open on this base**, and resolved by choosing another base: the Alpine/musl build or the dhi.io base.

| ID | Packages | Severity | Status | Resolution |
|---|---|---|---|---|
| CVE-2025-69720 | libtinfo6, ncurses-base, ncurses-bin | grype High / trivy HIGH | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-102010 | gcc-14-base, libgcc-s1, libstdc++6 | grype High / trivy MEDIUM | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-16742 | libsystemd0, libudev1 | grype Medium / trivy HIGH | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-19499 | libc-bin, libc6 | grype High / trivy MEDIUM | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-5435 | libc-bin, libc6 | grype High / trivy MEDIUM | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-54369 | libacl1 | grype High / trivy HIGH | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-54370 | libacl1 | grype High / trivy MEDIUM | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-76642 | bsdutils, libblkid1, liblastlog2-2, libmount1, libsmartcols1, libuuid1, login, mount, util-linux | grype High / trivy HIGH | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-78408 | bsdutils, libblkid1, liblastlog2-2, libmount1, libsmartcols1, libuuid1, login, mount, util-linux | grype High / trivy HIGH | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-78409 | bsdutils, libblkid1, liblastlog2-2, libmount1, libsmartcols1, libuuid1, login, mount, util-linux | grype High / trivy HIGH | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-78410 | bsdutils, libblkid1, liblastlog2-2, libmount1, libsmartcols1, libuuid1, login, mount, util-linux | grype High / trivy HIGH | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-82560 | perl-base | grype High / trivy UNKNOWN | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-85091 | zlib1g | grype High / trivy MEDIUM | open on this base (Debian: not-fixed / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-9538 | perl-base | grype High / trivy HIGH | open on this base (Debian: wont-fix / fix_deferred); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |
| CVE-2026-95619 | gcc-14-base, libgcc-s1, libstdc++6 | grype High / trivy MEDIUM | open on this base (Debian: wont-fix / affected); not present in the Alpine image | no Debian fix; avoided by `make image-alpine` (or dhi.io base) |

## Decisions and follow-ups

- **Default base.** dhi.io stays the default because it was requested. Without a dhi.io login, `make image-alpine` is the zero-HIGH public option, and CI scans that build by default. `make image-public` (Debian) is kept for glibc parity, but it fails `make scan`.
- **MSRV.** No advisory needed an MSRV bump once the `elasticsearch` dev-dependency was removed. If `time` comes back into the tree, its fix (0.3.47) needs Rust 1.88.
- **Image visibility into the binary's crates.** Building with `cargo auditable` would let grype and trivy see the crates inside the binary. It is not adopted yet; cargo-audit on the committed Cargo.lock covers those crates instead.
