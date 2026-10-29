# Datasets (TST-DATA-004/005/010, TST-INF-008)

- **Manifest:** `datasets.toml`. Every entry has a URL, size, SHA-256, licence, licence class, tier, format, compression and converter.
- **Pinned entries:** hashes were measured on 2026-10-29 by streaming each file once.
- **Unpinned entries** (`sha256 = ""`) fall into two groups:
  - large L1/L2 corpora;
  - two files whose 2026-10-29 transfer failed.

  `fetch.sh` refuses them unless you pass `--allow-unpinned`. With that flag it prints the hash, which B pins in task B1.

```
scripts/data/fetch.sh --list                    # table: tier, licence class, pinned?
scripts/data/fetch.sh --verify                  # manifest, licence gate and cached checksums
scripts/data/fetch.sh http_logs_1k nasa_jul95   # download into $ED_DATA_CACHE (default /var/tmp/elasticdump-rs-data)
scripts/data/fetch.sh --max-docs 100000 --out head.ndjson http_logs_191998   # prefix without full decompression
scripts/data/convert.sh http_logs_191998 [--index NAME] [--max-docs N]       # -> converted/<name>.ndjson.zst (+ sha256)
scripts/data/selftest.sh                        # offline test against a loopback HTTP server
```

## How fetching works

- A second fetch is a no-op: the cached file is checked against its SHA-256.
- An interrupted download resumes from the `.part` file using an HTTP Range request.
- A checksum mismatch deletes the download and fails.

## Licence gate

`fetch.sh` sorts each entry by its licence class:

| Class | Meaning | Examples |
|---|---|---|
| `ok` | Allowed in CI artefacts. | |
| `ask` | Can be used locally. Use in CI needs the owner's OK (Q9). | Loghub |
| `blocked` | Never used. The licence is non-commercial or research-only, or the download needs a login. | AIT (CC-BY-NC-SA), Kaggle |

With `CI=true` (or `--ci`), only the classes listed in `policy.ci_license_classes` are allowed.

## Converters

The converters are `edgen convert --kind rally|clf|loghub` (see `tools/edgen/README.md`).

- **Deterministic ids:** `_id = hex(SHA-256(dataset ‖ 0 ‖ file ‖ 0 ‖ line_no))[..20]`.
- **Reproducible output:** the same input always gives the same output hash.
- **Compressed inputs:** bz2 and xz inputs are decompressed with Python's stdlib. The box has no `bzcat` or `zstd` binary.
