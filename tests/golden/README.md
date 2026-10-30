# Golden recordings (TST-INF-003)

`<engine>-<version>/<case>/` holds one recorded run of the binary against a live engine:

- `NNN.req.json` / `NNN.resp.json`: each HTTP exchange in arrival order, normalised (see
  `tests/support/golden.rs`: `took` -> 0, uuids, `creation_date`, node name, scroll/PIT ids -> stable tokens;
  no request headers are stored, so no credentials);
- `meta.toml`: engine, version, pinned image, tool arguments (`{{URL}}`, `{{OUT}}`, `{{DIR}}` placeholders), exit code;
- `expected.out`: the output file of the recording run (when the case writes one);
- `input.ndjson`: the input file of restore cases.

Record: `scripts/golden/record.sh <profile>` (one engine at a time). Drift check: `scripts/golden/record.sh <profile> --check`
re-records into a temp dir and requires a zero diff. Replay (offline, T1): `cargo nextest run --profile contract -E 'binary(golden_replay)'`;
differences in output bytes are written to `target/golden-diff/`, request-shape differences are listed in the failure.

Finding G-1 (L-303): the tool's search request bodies are `sonic_rs` objects whose key order is randomised per process once
the object is mutated (e.g. `size` inserted). Elasticsearch/OpenSearch ignore key order, so this is harmless on the wire, but
the user's `--searchBody` key order is not preserved. The golden normaliser therefore stores non-NDJSON request bodies with
sorted keys; `_bulk` NDJSON keeps exact line and key order.

Coverage (L-402, API-002): 19 cases (`tests/it/golden.rs`: data via scroll / PIT / search_after / search template,
data restore, mapping, settings, index, analyzer, alias, legacy / component / composable templates, ILM-ISM policy,
stored script, and their restores) on 9 engine versions: OpenSearch 1.3.20, 2.19.7, 3.9.0; Elasticsearch 7.10.2 OSS
(bitnamilegacy), 7.10.2 default, 7.17.29, 8.6.1, 8.19.23, 9.1.2. The generated endpoint x version and case x version
tables are in TESTING-MATRIX.md ("Per-version contract"). `expected.out` is produced by replaying the fresh recording
(so volatile values in the output, e.g. ISM `last_updated_time`, are the normalised ones) and the recorder asserts
that the recording replays with the live exit code. Expected failures are recorded, not skipped: `--type policy` on
Elasticsearch OSS 7.10.2 exits 1 (no ILM/ISM API). `--searchWithPit` on versions without a PIT API (ES OSS 7.10,
OpenSearch 1.3) falls back to scroll; the recording asserts the scroll request shape.
Single case: `GOLDEN_CASE=<name> scripts/golden/record.sh <profile>`.

Bulk shapes (L-411, API-020): `bulk-create`, `bulk-update`, `bulk-delete` (one action each, chained on golden_dst),
`bulk-version` (`--handleVersion --versionType external` with `_routing`), `bulk-gzip` (`--esCompress`: the gzip
request body is stored decoded; the recorded 2xx is the per-version "accepts gzip" evidence), `bulk-split`
(`--maxBulkBytes 120b` -> one document per request). All 9 versions accept every shape with 0 item errors.
`--parseExtraFields` is not in the goldens: any extra meta key it copies is engine-specific (covered by unit tests).
ILM `in_use_by` lists are sorted and dot-prefixed (system) indices/data streams dropped by the normaliser: ES creates
`.logs-deprecation.*` lazily, which made the policy goldens flap.
