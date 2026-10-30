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
