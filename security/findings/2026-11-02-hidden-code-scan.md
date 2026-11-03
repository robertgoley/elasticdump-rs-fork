# Hidden-code scan (TESTING-TODO §2) — 2026-11-02, Grok Bot

Commit scanned: 37fa3ac (main). Scope: all `git ls-files` (tracked) plus the 280 vendored crates in `.vendor/crates`
(produced by `scripts/vendor-crates.sh` from Cargo.lock).

## Method / commands
```
B='[\x{202A}-\x{202E}\x{2066}-\x{2069}\x{200B}-\x{200F}\x{2060}-\x{2064}\x{FEFF}\x{00AD}\x{180E}]'
git ls-files -z | xargs -0 rg -l --pcre2 "$B"                                    # tracked: bidi + invisible
rg -l --pcre2 '[\x{202A}-\x{202E}\x{2066}-\x{2069}]' .vendor/crates            # vendor: bidi controls
rg -l --pcre2 '[\x{200B}-\x{200F}\x{2060}-\x{2064}\x{FEFF}]' -g '*.rs' .vendor/crates   # vendor: invisible
git ls-files '*.rs' | xargs rg -n --pcre2 '[A-Za-z_]\w*[^\x00-\x7F]\w*\s*[(=:]'  # non-ASCII identifiers
git ls-files | xargs rg -l --pcre2 '[A-Za-z0-9+/]{120,}|\b[0-9a-fA-F]{100,}\b'      # base64/hex blobs
rg -e 'base64 -d' -e 'eval *\(' -e 'curl.*\| *sh' -e '\\x..\\x..\\x..\\x' -e 'exec\('  # obfuscation (rs/sh/py/yml/Dockerfile/Makefile)
```

## Results
| Check | Hits | Verdict |
|---|---|---|
| Bidi controls / invisible chars, tracked files | 0 | clean |
| Bidi controls, vendor | 1 file: `url-2.5.8/tests/urltestdata.json` | benign: WHATWG URL test vectors (not compiled) |
| Invisible chars in vendor `.rs` | 5 files: `clap_builder-4.6.7/src/output/textwrap/core.rs`, `console-0.16.6/src/utils.rs`, `icu_provider-2.1.1/src/hello_world.rs`, `unicode-width-0.2.3/{src/lib.rs,tests/tests.rs}` | benign: U+200D ZWJ / U+200E LRM / U+200C ZWNJ inside emoji and Persian/Arabic string literals and doc comments used by width/text tests |
| Non-ASCII identifiers (confusables), tracked `.rs` | 0 (1 regex hit: `src/retrieval/pit_search.rs:333` `"FGluY2x1…("` — an ellipsis inside a test string literal) | clean |
| Non-ASCII anywhere in tracked `.rs` | 44 files, all in comments/strings (em dashes, arrows, `…`) | benign |
| Long base64 / hex blobs (>=120 b64 / >=100 hex) in tracked files | 0 | clean |
| Obfuscation patterns | `scripts/fuzz/seed.py`, `scripts/res/bombs.sh`, `tests/chaos_brk.rs`: `\x..` byte strings are zstd/lz4/xz/zip magic numbers for fuzz/decompression-bomb tests; `security.yml`: comment "no `curl | sh`" | benign |

**Suspicious findings: 0.**

## Follow-ups
- Add the tracked-files bidi/invisible check to `scripts/lint-hygiene.sh` (cheap, catches Trojan Source in PRs).
- Optionally add `#![deny(confusable_idents, mixed_script_confusables)]` (rustc lints, on by default as warn) to CI with `-D warnings`.
- Re-run the vendor bidi scan whenever Cargo.lock changes (Dependabot PRs).
