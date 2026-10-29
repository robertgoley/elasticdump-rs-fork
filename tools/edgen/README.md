# edgen: seeded test data (TST-INF-009, TST-DATA-001/005/010)

edgen generates byte-reproducible test documents and converts external log corpora into elasticdump format.

```
edgen gen --profile logs --seed 1 --count 100000 [--start N] [--format elasticdump|source|bulk]
          [--index NAME] [--ids det|uuid|none] [--routing-pct 10] [--compress zstd|gzip] [--level N]
          [--split-bytes N --out FILE] [--threads N]
          [--fields N] [--huge-min-mb 1 --huge-max-mb 10] [--edge-file-only] [--edge-index-cases]
edgen mapping --profile logs [--engine es|os|os1]     # {"settings","mappings"} for index creation
edgen cases                                            # edge catalogue with expected outcomes
edgen convert --kind rally|clf|loghub --dataset NAME --index NAME [--input FILE|-] [--file NAME] [--max-docs N] [--out F] [--compress zstd]
```

## Determinism

Each document *i* gets its own ChaCha20 stream, seeded with `SHA-256("edgen/v1" ‖ profile ‖ seed ‖ i)`. Ids and routing use a separate stream.

As a result, the output does not depend on the thread count. The `--start`/`--count` ranges also join together exactly. `tests/golden.rs` pins the SHA-256 of every profile at 1 and 3 threads; CI runs it with `cargo test --release -p edgen`.

## Ids and routing

- **Ids:** by default, `<profile>-<seed>-<base36(i)>`. `--ids uuid` uses a UUID v4 from the RNG; `--ids none` emits no `_id`.
- **Routing:** `--routing-pct` (default 10, per §5.3) gives that share of documents a `_routing` value of `r0`…`r15`.

## Profiles

| Profile | Content | Speed on the box (1 thread) |
|---|---|---|
| `logs` | ECS-like: monotonic `@timestamp` with jitter, Markov message, ip v4/v6, user agent, `labels` (flattened / flat_object); about 1 KB | **~315k docs/s** (target ≥ 300k); 4 threads: ~730k |
| `metrics` | 32 numeric fields | ~345k docs/s |
| `nested` | Questions with nested answers and comments | ~77k docs/s |
| `geo` | `geo_point` in all four encodings, `geo_shape` as WKT or GeoJSON | — |
| `wide` | `--fields N` (up to 10,000) fields of 4 types; the mapping raises `total_fields.limit` | — |
| `huge` | 1–10 MB documents (`--huge-*-mb`) in a disabled object | — |
| `edge` | The §5.4 catalogue: 45 cases, each tagged with `case` and `expect` | — |
| `mixed` | logs 60%, metrics 20%, nested 10%, geo 10%, as `{"kind":p, p:{…}}` | — |

## The edge profile

- **File-only cases:** depth 1,000/10,000 and 10/90 MB documents need `--edge-file-only`.
- **Index-name cases:** cases that write to other indices (unicode, `#`, hidden, date math, 255 bytes, leading `-`) need `--edge-index-cases`.
- **Expected outcomes:** `expect` is `store`, `reject`, `store_last_wins`, or `verify`. `verify` means the outcome differs by engine or version and is recorded per version in A4 (TST-DATA-003).
- **`lone_surrogate`:** this case is deliberately not strict JSON.

## Converters

Every converter writes `{"_index","_id","_source"}` lines with the deterministic id `_id = hex(SHA-256(dataset ‖ 0 ‖ file ‖ 0 ‖ line_no))[..20]` (TST-DATA-010).

| Kind | Input | What it does |
|---|---|---|
| `rally` | One source object per line | Copies each line verbatim. Lines that are not exactly one object are skipped and counted. |
| `clf` | Common or combined log format | Writes `@timestamp` with its offset, host, user, request, method, path, status, size, referrer and user agent. |
| `loghub` | Raw lines | Extracts timestamp, level and component for HDFS, Spark/Hadoop, BGL and ISO-style sets. The raw line is always kept in `message`. |

`.gz` and `.zst` inputs are read directly. For bz2, pipe through `bzcat` and pass `--input - --file <name>`.
