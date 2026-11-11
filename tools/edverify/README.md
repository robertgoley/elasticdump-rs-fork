# edverify: a document-set verifier (TST-INF-005)

edverify is a test-only tool. It does not depend on searchdump, so a bug in the tool under test cannot hide itself.

```
edverify digest [--canonical [--decimal-numbers]] [--index-as NAME] <input>...       # JSON digest on stdout (or --out)
edverify compare a.json b.json                                    # exit 0 equal, 1 different
edverify diff [--canonical [--decimal-numbers]] [--index-as NAME] --a <in>... --b <in>... [--limit 10] [--out r.json]
```

## Inputs

An input is one of:

- **Files.** NDJSON, `.gz` or `.zst`. List every part of a split dump.
  - A line with a `_source` member is an elasticdump or hit record.
  - Any other line is treated as a bare source document.
- **A cluster index.** `http(s)://[user:pass@]host:port/<index>`.
  - edverify reads it with its own sliced scroll (`--slices`, `--size`). Defaults remain 4 slices and 1000 docs per page.
  - HTTP 429 responses retry up to six times with 250/500/1000/2000/4000/4000 ms backoff; transport errors and other HTTP errors are not retried (a lost continuation response may already have advanced the scroll).
  - Every returned scroll ID is tracked before page processing. All slice tasks are joined even when one errors or panics, then only their own IDs are cleared before returning success/error or checking `_count`. Cleanup 429s use the same retry budget; failed cleanup is reported, not ignored. Already-freed IDs (HTTP 404) are harmless. No cluster-wide clear is used. edverify opens scrolls only, never PITs; network loss/process termination can still prevent remote cleanup, which must not be mistaken for a successful verification.
  - It then checks the number of documents read against `_count`.
  - Credentials can also come from `EDV_USER` and `EDV_PASSWORD`. Use `--insecure` or `--ca` for TLS.

## Digest

For each document:

`h = SHA-256(index ‖ 0 ‖ _id ‖ 0 ‖ routing ‖ 0 ‖ _source bytes)`

The set digest is `(Σh mod 2^256, XOR h, count)`.

Each document also goes into one of 1,024 buckets, chosen by `SHA-256(index ‖ 0 ‖ _id)`.

- `diff` re-scans only the buckets that differ.
- It names the first `--limit` ids, each marked as changed, missing, extra, or count differs (duplicate).

Options:

- `--index-as` replaces `_index`. Use it when you compare a source index with a renamed copy.
- `--canonical` re-serialises `_source` with sorted keys before hashing. Numbers keep the digits as written. Use it across engines, and when the source documents were pretty-printed: since the INJ-F1 fix, the dump flattens line breaks in `_source`. Escapes of unpaired surrogates (`"\uD800"`, valid JSON that engines store) are hashed as their lowercase escape text instead of being dropped as bad lines.
- `--decimal-numbers` (with `--canonical`) writes each number as a normal form of its exact decimal value, so `79.746790`, `79.74679` and `7.974679e1` hash alike while `9007199254740993` and `9007199254740992`, or `-0` and `0`, still differ. Use it for output from tools that re-serialise numbers (Node elasticdump).

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Equal or clean |
| 1 | Different, or lines that are not JSON objects |
| 2 | Usage or I/O error |

## Measured on the box (8 vCPU, 2026-10-29)

These figures come from a 2M-document file, about 380 B per document.

| Mode | Threads | Speed |
|---|---|---|
| Raw digest | 1 | 1.39M docs/s |
| Raw digest | 4 | 5.1M docs/s |
| `--canonical` | 4 | 1.76M docs/s |

`tests/detect.rs` builds a 200k-document set. It checks that edverify detects, and names the id of:

- one changed byte;
- one missing document;
- one duplicate.

It also checks that a reordered two-part split, gzip and zstd input all match the original.
