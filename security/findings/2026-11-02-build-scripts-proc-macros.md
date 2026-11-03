# Build scripts and proc macros inventory (TESTING-TODO §2) — 2026-11-02, Grok Bot

Source: `cargo metadata --format-version 1 --locked --offline` at 37fa3ac (whole workspace, all targets incl.
non-host platforms in Cargo.lock). Registry crates with a `custom-build` (build.rs) or `proc-macro` target: **63**.

## Method
- Inventory from metadata target kinds.
- Skim each build.rs (and `build/*.rs`) with
  `rg -o -e 'Command::new' -e 'TcpStream|reqwest|ureq|curl|http://|https://' -e 'fs::write|File::create' -e 'env::var("(HOME|USER|AWS|GITHUB|SSH)'`;
  then checked that no `http(s)://` appears outside comments/error strings.
- Proc macros: all are well-known derive/attribute crates (serde, clap, thiserror, tokio, zerocopy, rkyv, icu/yoke/zerovec, wasm-bindgen, jiff, displaydoc, ref-cast, ptr_meta, munge, const-random, defmt); they run at compile time with full process rights by design, no network code found.

## What runs at build time (summary)
- **rustc version probes** (`Command::new(rustc)` / `$RUSTC_WRAPPER`): anyhow, getrandom, httparse, libc, portable-atomic-util, proc-macro2, quote, ref-cast, rustix, rustversion, serde, serde_core, thiserror, zerocopy, zmij — spawn the compiler to detect features; write probe files in OUT_DIR. Low risk.
- **C compilation via `cc`**: ring 0.17.14 (BoringSSL-derived C/asm; spawns cc, and perl/nasm only when building from a git checkout, not from the crates.io tarball), libmimalloc-sys 0.1.49 (mimalloc C), zstd-sys 2.1.2 (zstd C). Spawn the C compiler; output to OUT_DIR. Medium (native code supply chain) but expected.
- **wasm-bindgen-shared 0.2.129**: `git rev-parse HEAD` (schema hash), harmless; only matters for wasm targets.
- **crc32fast**: `Command::new(env)` probe; **crunchy**, **defmt**, **ref-cast**, **serde(_core)**, **thiserror**, **libmimalloc-sys**: `fs::write`/`File::create` into `OUT_DIR` (generated code).
- **icu_*_data, generic-array, num-traits, parking_lot_core, quinn(-udp), rustls, tiny-keccak, ahash, serde_json, wit-bindgen, windows_* (import-lib link hints)**: cfg emission only.
- No build script opens sockets, reads HOME/AWS/GITHUB/SSH env vars, or writes outside OUT_DIR. (openssl-sys appears in the registry cache from other projects but is **not** in this Cargo.lock.)

**Suspicious findings: 0.**

## Inventory
| Crate | Version | Kind | Build-time pattern hits (count + pattern) |
|---|---|---|---|
| ahash | 0.8.12 | build.rs | - |
| anyhow | 1.0.104 | build.rs | 2 Command::new(rustc; 4 https:// |
| clap_derive | 4.6.7 | proc-macro | - |
| const-random-macro | 0.1.16 | proc-macro | - |
| crc32fast | 1.5.3 | build.rs | 1 Command::new(env; 2 https:// |
| crunchy | 0.2.4 | build.rs | 1 File::create |
| defmt | 1.2.0 | build.rs | 1 fs::write |
| defmt-macros | 1.2.0 | build.rs,proc-macro | - |
| displaydoc | 0.2.7 | proc-macro | - |
| generic-array | 0.14.7 | build.rs | - |
| getrandom | 0.3.4 | build.rs | 1 Command::new(rustc; 1 Command::new(wrapper |
| getrandom | 0.4.3 | build.rs | - |
| httparse | 1.10.1 | build.rs | 1 Command::new(rustc |
| icu_normalizer_data | 2.1.1 | build.rs | 1 https:// |
| icu_properties_data | 2.1.2 | build.rs | 1 https:// |
| jiff-static | 0.2.38 | proc-macro | - |
| libc | 0.2.191 | build.rs | 1 Command::new("freebsd; 1 Command::new(emcc; 2 Command::new(rustc; 1 Command::new(wrapper |
| libmimalloc-sys | 0.1.49 | build.rs | 1 fs::write; 1 https:// |
| munge_macro | 0.4.7 | proc-macro | - |
| num-traits | 0.2.19 | build.rs | - |
| parking_lot_core | 0.9.12 | build.rs | - |
| portable-atomic | 1.15.0 | build.rs | 80 https:// |
| portable-atomic-util | 0.2.8 | build.rs | 1 Command::new(rustc; 13 https:// |
| proc-macro2 | 1.0.107 | build.rs | 2 Command::new(rustc; 3 https:// |
| ptr_meta_derive | 0.3.2 | proc-macro | - |
| quinn | 0.11.12 | build.rs | - |
| quinn-udp | 0.5.16 | build.rs | - |
| quote | 1.0.47 | build.rs | 1 Command::new(rustc; 1 https:// |
| ref-cast | 1.0.27 | build.rs | 1 Command::new(rustc; 1 fs::write |
| ref-cast-impl | 1.0.27 | proc-macro | - |
| ring | 0.17.14 | build.rs | 1 Command::new("; 1 Command::new(command_name; 1 File::create |
| rkyv_derive | 0.8.18 | proc-macro | - |
| rustix | 1.1.6 | build.rs | 1 Command::new(rustc; 1 Command::new(wrapper; 2 https:// |
| rustls | 0.23.46 | build.rs | - |
| rustversion | 1.0.23 | build.rs,proc-macro | 1 /home/box/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rustversion-1.0.23/build/build.rs:Command::new(; 1 /home/box/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rustversion-1.0.23/build/build.rs:Command::new(wrapped_rustc; 1 /home/box/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rustversion-1.0.23/build/build.rs:fs::write; 2 /home/box/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rustversion-1.0.23/build/build.rs:https:// |
| serde | 1.0.229 | build.rs | 1 Command::new(rustc; 1 fs::write; 1 https:// |
| serde_core | 1.0.229 | build.rs | 1 Command::new(rustc; 1 fs::write; 5 https:// |
| serde_derive | 1.0.229 | proc-macro | - |
| serde_json | 1.0.154 | build.rs | - |
| thiserror | 2.0.21 | build.rs | 2 /home/box/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/thiserror-2.0.21/build.rs:Command::new(rustc; 1 /home/box/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/thiserror-2.0.21/build.rs:fs::write; 4 /home/box/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/thiserror-2.0.21/build.rs:https:// |
| thiserror-impl | 2.0.21 | proc-macro | - |
| tiny-keccak | 2.0.2 | build.rs | - |
| tokio-macros | 2.7.2 | proc-macro | - |
| wasm-bindgen | 0.2.129 | build.rs | - |
| wasm-bindgen-macro | 0.2.129 | proc-macro | - |
| wasm-bindgen-shared | 0.2.129 | build.rs | 1 Command::new("git |
| windows_aarch64_gnullvm | 0.52.6 | build.rs | - |
| windows_aarch64_msvc | 0.52.6 | build.rs | - |
| windows_i686_gnu | 0.52.6 | build.rs | - |
| windows_i686_gnullvm | 0.52.6 | build.rs | - |
| windows_i686_msvc | 0.52.6 | build.rs | - |
| windows_x86_64_gnu | 0.52.6 | build.rs | - |
| windows_x86_64_gnullvm | 0.52.6 | build.rs | - |
| windows_x86_64_msvc | 0.52.6 | build.rs | - |
| wit-bindgen | 0.46.0 | build.rs | - |
| yoke-derive | 0.8.4 | proc-macro | - |
| zerocopy | 0.8.63 | build.rs | 1 Command::new(rustc_cmd_name; 4 https:// |
| zerocopy-derive | 0.8.63 | proc-macro | - |
| zerofrom-derive | 0.1.8 | proc-macro | - |
| zerovec-derive | 0.11.7 | proc-macro | - |
| zmij | 1.0.23 | build.rs | 1 Command::new(rustc; 1 https:// |
| zstd-safe | 8.2.0 | build.rs | - |
| zstd-sys | 2.1.2+zstd.1.5.7 | build.rs | 6 https:// |

## Follow-ups
- The image build already compiles with `--network=none` (L-808i), which neutralises any build-time network exfil; consider the same for CI (`cargo fetch` then `cargo build --offline` in a no-network step).
- Track native-code crates (ring, libmimalloc-sys, zstd-sys) in `supply-chain/` (cargo-vet) audits with "safe-to-deploy".
- Re-run this inventory on every Cargo.lock change (diff the crate list; new build.rs/proc-macro crates need review).
