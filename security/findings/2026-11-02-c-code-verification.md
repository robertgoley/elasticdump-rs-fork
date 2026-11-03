# Bundled C code verification — 2026-11-02, Grok Bot

Vendored sources: `scripts/vendor-crates.sh` output (`.vendor/crates`, from Cargo.lock at 37fa3ac; cargo checks each
`.crate` against the Cargo.lock checksum, e.g. ring `a4689e6c…83b7`). Upstream fetched fresh from GitHub.

## zstd-sys 2.1.2+zstd.1.5.7 (libzstd 1.5.7)
- Upstream `zstd-1.5.7.tar.gz` from the GitHub release: `sha256sum -c zstd-1.5.7.tar.gz.sha256` → OK;
  `gpg --verify zstd-1.5.7.tar.gz.sig` → **Good signature** from "Zstandard Release Signing Key <signing@zstd.net>",
  RSA key `4EF4AC63455FC9F4545D9B7DEF8FE99528B52FFD` (fetched from keyserver.ubuntu.com; key not cross-certified → "[unknown]" trust).
- `diff -rq zstd-1.5.7/lib .vendor/crates/zstd-sys-*/zstd/lib`: **0 differing files, 0 vendor-only files**; 7 upstream-only
  (build files: `.gitignore`, `BUCK`, `Makefile`, `README.md`, `dll/`, …).

## libmimalloc-sys 0.1.49 (mimalloc v2.3.2 and v3.3.2)
- Bundles both trees (`c_src/mimalloc/v2` = 2.3.2, `v3` = 3.3.2 per `MI_MALLOC_VERSION`). Compared with GitHub tag archives
  `v2.3.2` / `v3.3.2` (mimalloc does not publish signed tarballs; tag archive trusted via HTTPS/GitHub only).
- `diff -rq`: **0 differing, 0 vendor-only** for both; 9 upstream-only each (`.github`, `bin/`, `cmake/`, docs/tests).

## ring 0.17.14
- No `0.17.14` git tag exists upstream (ring stopped tagging); release commit found by message:
  `2723abbca9e83347d82b056d5b239c6604f786df` ("0.17.14."). Archive of that commit compared.
- `crypto/`, `include/`, `third_party/`, `src/`: **0 differing, 0 vendor-only** files (upstream-only: tests/benches/CI files).
- `pregenerated/` (104 generated asm files, not in git): regenerated with upstream perlasm (`perl <file>.pl elf|linux64 out.S`)
  for all **x86_64 ELF (17) and aarch64 Linux (11)** files — the targets this project builds — **all 28 byte-identical**.
  Not regenerated: macOS/iOS/Windows (nasm/win32n/win64) and armv4/7 files (not built for linux x86_64/aarch64);
  `ring_core_generated/prefix_symbols*.h` not regenerated.

**Suspicious findings: 0.** All differences are upstream-only build/test files omitted from the crate packages.

## Follow-ups
- Re-run on each Cargo.lock bump of these three crates (cheap: three tarballs + `diff -rq`).
- Import and pin the zstd signing key fingerprint above in `supply-chain/` notes so future checks are not trust-on-first-use.
- Consider cargo-vet audits (`safe-to-deploy`) recording these comparisons.
