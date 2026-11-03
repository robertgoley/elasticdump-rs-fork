# Release binary inspection — 2026-11-02, Grok Bot

Binary: `target/release/elasticdump-rs` (host `cargo build --release` at 37fa3ac, glibc + mimalloc, 9.1 MB, stripped PIE).

## Commands
```
readelf -d target/release/elasticdump-rs | grep NEEDED
readelf --dyn-syms -W target/release/elasticdump-rs | awk '$7=="UND"{print $8}'   # imported libc functions
strings -n 8 target/release/elasticdump-rs > /tmp/str.txt                          # 10,388 strings
rg -o 'https?://[A-Za-z0-9._/-]+' /tmp/str.txt ; rg -o '\b([0-9]{1,3}\.){3}[0-9]{1,3}\b' /tmp/str.txt
rg -o -i '\b[a-z0-9-]+\.(com|net|org|io|ru|cn|xyz|top|onion|tech|sh)\b' /tmp/str.txt
rg -e '/bin/(ba)?sh' -e 'curl ' -e 'wget ' -e '/dev/tcp' -e wallet/miner/pastebin/ngrok/telegram/discord patterns, BTC/ETH address shapes
```

## Results
| Check | Result | Verdict |
|---|---|---|
| Linked libraries | `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` only | expected |
| Imported process/network functions | 141 undefined symbols; of the risky set only `socket`, `connect`, `bind`, `getaddrinfo`, `dlsym`. **No `execve`/`execvp`/`posix_spawn`/`fork`/`system`/`popen`** — the binary cannot spawn processes | expected (HTTP client; `bind` from std/tokio socket API, `dlsym` from std weak-symbol lookup) |
| URLs | `https://github.com/clap-rs/clap/issues` (clap internal-error text), `https://docs.rs/rustls/...` (rustls close_notify error), fragments of help text (`http://host`, `https://...`) and byte noise | benign |
| IP-shaped strings | 6, e.g. `011.012.012.113`, `24.04.15.05` — byte-table noise, not routable addresses used anywhere | benign |
| Domains | `crates.io` (446, embedded source paths), `openssl.org`/`SSL.com`/`GoDaddy.com` (webpki root CA subject names), `tenyears.tech` (Cargo.toml `authors` = upstream `TenYears <opensource@tenyears.tech>`, shown in `--version`/about), `github.com` | benign |
| Shell commands / exfil / wallets | none; only `0x…` hits are rustls enum-name tables | benign |

**Suspicious findings: 0.**

## Follow-ups
- Build paths leak into the binary (`/home/box/.cargo/registry/src/...`): add `--remap-path-prefix` (or `trim-paths` when stable) for release builds; also helps reproducibility (B11).
- `authors` still names upstream TenYears; decide whether the fork should carry its own maintainer contact.
- Repeat on the image binary and on release artifacts in release.yml.
