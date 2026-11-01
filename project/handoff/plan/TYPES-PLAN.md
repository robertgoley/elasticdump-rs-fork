# elasticdump-rs: Implementation Plan for the Remaining `--type` Values

> **Audience:** a coding agent or Robert, working directly from this document. It extends
> [`PLAN.md`](./PLAN.md) (M0–M6, `data`/`mapping`/`settings`), and its milestones continue that numbering (M7 onwards).
> **Reference implementation:** `elasticsearch-dump` @ `600db62` (`package.json` 6.125.1), cloned at
> `/workspace/elasticsearch-dump`. Also installed for interop tests at `/tmp/ed/node_modules/.bin/elasticdump` (6.125.1).
> **Rewrite:** `/workspace/elasticdump-rs`, branch `rewrite` @ `704641c` (M6), plus uncommitted work in progress
> on the Elasticsearch harness. The repo was only read; no files, builds or containers were touched.
> **Written:** 2026-10-29. **[verified]** = checked in the Node source, the Rust source, or a vendor API doc fetched
> on 2026-10-29. **[uncertain]** = to be confirmed against a live cluster in the podman harness during implementation.
> Nothing in this document came from running a cluster: the harness was busy with other work.
> **Targets:** OpenSearch 2.x/3.x, Elasticsearch 8.6.1 and 9.x (harness: 9.1.2).

---

## 0. TL;DR

* Node accepts **11** `--type` values. The rewrite implements **3** (`data`, `mapping`, `settings`). The **8 missing** are:
  `index`, `analyzer`, `alias`, `template`, `component_template`, `index_template`, `policy`, `script`.
  * `script` is a real, working type that is **missing from Node's `--help` and README** list.
  * `multielasticdump` covers only `index, mapping, analyzer, alias, settings, template` (+ data).
* **One file format for all 8.** Each is a single "get everything once, put everything once" exchange. Each writes exactly **one line holding a
  JSON string literal** (double-encoded JSON, like `settings`). Node's restore path calls `JSON.parse(line)` on what it reads, so
  **Node can only read the string-literal form** [verified, `jsonparser.js`]. Rust must write that form and accept both on read.
* **Node bugs that matter for the targets:**
  1. **OpenSearch ISM policy restore is broken in every case.** `setPolicies` iterates the keys of the ISM *list* response (`policies`,
     `total_policies`) as if they were policy names. It PUTs `_plugins/_ism/policies/policies` with body `{}` [verified by reading
     the source; confirm in the harness]. ISM updates also need `if_seq_no`/`if_primary_term`, which Node never sends.
  2. **The ISM dump is probably truncated.** The list API is paginated (`size`/`from`) and Node sends neither [default page size
     uncertain, believed 20].
  3. **`--type alias` with a host-only `--input`** requests `GET /undefined/_alias/*` [verified].
  4. **`--type index` with a host-only `--input`** dumps `GET /` (the cluster banner) [verified].
  5. **`analyzer` restore only works on an index that does not exist yet.** Node creates the index empty, then waits for health
     `green`. On a single-node cluster with replicas ≥ 1 that wait **stalls 30 s** (the 408 is ignored). Then it closes the
     index, PUTs `analysis` and reopens it [verified]. The close/open step is only ever applied to a fresh, empty index.
  6. **The default `--templateRegex` misses ES 8/9 built-ins.** It only matches the exact names `metrics` and `logs`, dot-names,
     `*_audit_log` and `*-index-template`. Built-ins like `logs@mappings`, `ecs@mappings`, `synthetics` and `ilm-history-7` are
     dumped and later restored. The help text's default regex and its description are both wrong.
* **Cross-engine.** Templates, aliases, analyzers, scripts and `index` copy across engines with field stripping, warnings and
  server-side errors for unmappable content. **ILM ↔ ISM is not 1:1.** Default: refuse, with a clear error. Optional M12: best-effort
  **ILM → ISM** translation with a strict refusal list. **ISM → ILM** is refused.
* **Plan:** 7 milestones, **M7–M13**.
  * Core (M7–M11, M13): **~17–22 working days**.
  * With the optional ILM→ISM translator (M12): **~21–28 days**.

---

## 1. Node `--type` values and their status in the rewrite

Sources [verified]:
* `lib/transports/elasticsearch.js` (`get`/`set` dispatch);
* `lib/transports/__es__/*.js` (per-type modules);
* `lib/help.txt` and `README.md` (`--type` options list);
* `bin/elasticdump` (defaults);
* `bin/multielasticdump` (`supportedTypes`).

Node does not validate `--type` up front. An unknown value fails at the first ES `get`/`set` with `unknown type option`.

| `--type` | Node dispatch (get / set) | Node version gate | In help/README | multielasticdump | Rust status | Plan |
|---|---|---|---|---|---|---|
| `data` (default) | `getData` / `setData` | – | ✅ | ✅ | ✅ done (M2–M6) | – |
| `mapping` | `getMapping` / `setMapping` | – | ✅ | ✅ | ✅ done (M3) | – |
| `settings` | `getSettings` / `setSettings` | – | ✅ | ✅ | ✅ done (M3) | – |
| `index` | `getSettings` (with `GET /{idx}/`) / `setSettings` | – | ✅ | ✅ | ❌ (`DumpType::parse` rejects) | **M8** |
| `analyzer` | `getSettings` (`_settings`) / `setAnalyzer` | – | ✅ | ✅ | ❌ | **M9** |
| `alias` | `getAliases` / `setAliases` | – | ✅ | ✅ | ❌ | **M8** |
| `template` | `getTemplates` / `setTemplates` (legacy `_template`) | – | ✅ | ✅ | ❌ | **M10** |
| `component_template` | `getTemplates` / `setTemplates` | ES ≥ 7.8.0 | ✅ | ❌ | ❌ | **M10** |
| `index_template` | `getTemplates` / `setTemplates` | ES ≥ 7.8.0 | ✅ | ❌ | ❌ | **M10** |
| `policy` | `getPolicies` / `setPolicies` (ILM, or ISM on OpenSearch) | ES ≥ 6.6.0 | ✅ | ❌ | ❌ | **M11** (+ M12 translation) |
| `script` | `getScripts` / `setScripts` (stored scripts) | ES ≥ 7.10.0 | **❌ undocumented** | ❌ | ❌ | **M10** (small; ships with templates) |

**Version gates on OpenSearch.** Node compares the gates against `--force-os-version` (default `7.10.2`), so all of them pass on
OpenSearch [verified, `_base.js`].

**Out of Node's `--type` set entirely,** so these are also out of scope here:
* ingest pipelines;
* data streams (multielasticdump has a `matchType=datastream`, but there is no `--type`);
* SLM policies;
* index lifecycle *attachment* (`_plugins/_ism/add`);
* security roles;
* Kibana objects;
* search applications;
* synonyms sets.

See Q8.

---

## 2. Cross-cutting Node behaviour (all 8 types) [verified]

* **Get once, set once.** Every getter sets a `gotX` flag, so the second `get` returns `[]` and the processor stops. Every setter
  sets `haveSetX` and only uses `data[0]`. `--limit`, `--offset`, `--size`, `--fileSize`, `--maxRows` and `--concurrency` do
  nothing. `--searchBody` is ignored.
* **File format.** The getter pushes the response body **string** (or a re-stringified object) as the single payload element. The
  file transport then `JSON.stringify`s each element, so the line is a JSON string literal, for example:
  `"{\"source_index\":{\"aliases\":{\"the-source\":{},\"test-2018\":{}}}}"`.
  * The setters `jsonParser.parse(data[0])`, which only works when `data[0]` is a string.
  * Node's own fixtures in `test/test-resources/` (`alias.json`, `template_*.json`, `stored_scripts.jsonl`) are all in this form.
  * **`mapping` is the only meta type written as a plain object.**
* **`--fsCompress`** gzips the single line. `--overwrite` applies as usual.
* **URL parsing** (`parse-base-url.js`):
  * `http://h:9200/a` → `index=a`.
  * `http://h:9200/a/b` → `index=a, type=b`.
  * `--input-index a/b` does the same.
  * For the cluster-level types (`template*`, `policy`, `script`), **"index" means the resource name filter**, e.g.
    `--input=http://es:9200/template-filter` (README).
  * For `alias`, the "type" segment is the **alias name filter** (README: `--input=http://es.com:9200/index-name/alias-filter`).
* **`--prefix`/`--suffix`** are applied only to the output URL's index. The setters for `alias`, `template*`, `policy` and `script`
  use `base.host`, so prefix and suffix have no effect for them. `analyzer` and `index` use `base.url`, so they do apply.
* **Parallelism.** Setters use `async.forEachOf` with no limit, so all PUTs within a run are concurrent and unordered.
* **Errors.** Most setters stop at the first non-200 (`handleError`). There is no `--ignore-errors` handling for these types.

### 2.1 Rewrite architecture these must fit (read from `src/`, not modified)

* `cli.rs`:
  * `DumpType {Data, Mapping, Settings}`. `DumpType::parse` explicitly rejects the 8 names with "not supported by
    elasticdump-rs".
  * `OUT_OF_SCOPE` downgrades `--templateRegex` and `--filterSystemTemplates` to a warning, and `--force-os-version` to "flavor is
    detected automatically".
* `lib.rs`:
  * `run()` dispatches `Data` to the retrieval or file pipeline and `Mapping | Settings` to `run_meta()`.
  * `run_meta()` builds input and output `EsClient`s, fetches `ClusterInfo` (flavor + semver), builds `MetaSource`, `MetaTarget`
    and `MetaOptions`, then calls `meta::read` and `meta::write`.
  * `validate()` already refuses non-data types when neither side is ES.
* `meta.rs`:
  * `MetaKind {Mapping, Settings}`.
  * `read()`: GET `[index]/_api`, or the first JSON value of a file via `ValueSplitter` (gzip and zstd auto-detected),
    then `decode_settings` (string literal or object).
  * `write()`: one line via `FileSink` (split and `sourceOnly` forced off), or a per-index apply loop with target selection
    (output index or file key, plus prefix/suffix, refusing many→one).
  * Helpers: `strip_settings`, `strip_for_flavor` (`ELASTIC_ONLY`, `OPENSEARCH_ONLY`), `typeless_mapping`, `get_json`/`send_json`
    (with `send_with_retry(RetryMode::Idempotent)`, treating a 200 body with `error` as a failure), and `is_already_exists`.
* `http/info.rs`: `ClusterInfo {flavor: Elastic|OpenSearch, version}` with capability helpers (`has_types`, `pit_api`, …). This is
  where new capability checks go.
* `endpoint.rs`: **divergence from Node.**
  * The *last* URL path segment is the index, and earlier segments form a base path (reverse-proxy prefix). So Node's
    `http://h/index/alias` README form parses in Rust as base `/index/` with index `alias`.
  * `--input-index index/alias` already splits into `index` + `doc_type`, matching Node. See §4.3 and Q3.
* Tests:
  * `tests/it/` is the podman harness: `harness.rs` (`Cluster`, `Engine`, `run`, `tmp_path`, `unique`), `engine.rs`
    (engine-specific assertions), `meta.rs` (round trips plus `node_interop_settings_and_mapping`, which uses `/tmp/ed`), and
    `cross.rs` (cross-engine file legs and a `cross` cluster→cluster profile).
  * `tests/support/mock_es.rs` for request-shape contract tests.
  * `scripts/it/run.sh` profiles: `opensearch-{plain,secure}`, `elasticsearch{,8}-{plain,secure}`, `cross`, `all`.

---

## 3. Shared design for the new types

### 3.1 Module layout

Turn `src/meta.rs` into a module directory. This is a mechanical move, so the existing tests keep passing unchanged:

```
src/meta/
  mod.rs          // MetaKind dispatch, read()/write() entry points, MetaSource/Target/Options (unchanged API)
  codec.rs        // read_first_value, LineEncoding {StringLiteral, Object}, decode_any (string literal or object)
  http.rs         // get_json / send_json / put_json_opt (status-aware), is_already_exists, error_type_is helpers
  index_scoped.rs // existing mapping + settings code, plus `index`, `analyzer` (shared target selection)
  alias.rs
  templates.rs    // legacy / component / composable, filtering, stripping, ordering
  policy/
    mod.rs        // ILM + ISM dump/restore, shape detection
    ilm_to_ism.rs // M12 (optional)
  scripts.rs
```

* `MetaKind` grows to `Mapping | Settings | Index | Analyzer | Alias | Template(TemplateKind) | Policy | Script`, where
  `TemplateKind = Legacy | Component | Composable`.
* Keep enum dispatch, which matches the current code. A trait object buys nothing here: every kind runs once per process, and
  edition 2024 `async fn` in traits is not dyn-compatible.
* Each kind implements four functions:
  * `fetch(client, info, src_filter, opts) -> Value` returns a **Node-shaped** document.
  * `normalize(Value) -> Value` takes any accepted file shape and returns the canonical shape.
  * `apply(client, info, doc, target, opts) -> WriteReport` restores it.
  * `encoding()` is `Object` for `Mapping` and `StringLiteral` for everything else.

### 3.2 CLI

* `DumpType` gains the 8 variants. Error text for an unknown type lists all 11.
* `--templateRegex` and `--filterSystemTemplates` become **real flags**, removed from `OUT_OF_SCOPE`:
  * Node defaults: `true` and `^(metrics|logs|.+_audit_log|.+-index-template|\..+)$`.
  * Node semantics: JS `RegExp.test`, so an unanchored pattern matches anywhere.
  * Use the `regex` crate. Patterns using JS-only features (lookaround, backrefs) fail with a usage error naming the feature.
* `--force-os-version` stays an accepted warning. Its only effect in Node is passing version gates on OpenSearch.
* Reused existing flags with their meaning per type:
  * `--skip-existing`, `--ignore-errors`;
  * `--input-index`/`--output-index`, `--prefix`/`--suffix`;
  * `--excludeSettings` (applied inside `index`, `analyzer` and template `settings`);
  * `--headers`, `--input-params`/`--output-params`, auth/TLS, retries, `--fsCompress`/`--compressionType`, `--overwrite`.
* New rs-only flags (documented extensions, all with conservative defaults):

| Flag | Default | Types | Meaning |
|---|---|---|---|
| `--filterManaged` | `true` (see Q2) | template*, policy | Skip entries with `_meta.managed: true` (ES built-ins) on **dump**, with an info log listing them. Node has no equivalent. |
| `--closeIndex` | `false` | analyzer | Allow applying `analysis` to an **existing** index: `_close` → `PUT _settings` → `_open`, always reopening. |
| `--policyTranslate` | `off` | policy | `ilm-to-ism` (M12) enables best-effort translation; otherwise a cross-engine policy restore is refused. |
| `--metaFormat` | `string` | all except mapping | `object` writes a plain JSON object line, which Node cannot read. Same idea as PLAN.md's `--settingsFormat`; cover `settings` with it too. |

### 3.3 File codec

* **Write:** a string literal for the 7 non-mapping types, like `settings`. Byte-compare against Node in the interop tests after
  canonicalising key order. Node uses `JSON.stringify` of `JSON.parse(body)`, or the raw body, so the key order is whatever the
  server sent. Rust keeps server order (`serde_json` `preserve_order` is already on, given `shift_remove`).
* **Read:**
  * Use `codec::decode_any`: a string literal is parsed again, an object is taken as-is, and anything else is an error.
  * Gzip and zstd are auto-detected, as they are today.
  * Only the first value is used. Warn if more values follow, since Node ignores them silently.
* **Shape detection** runs per kind (below). A file from the wrong `--type` fails with a hint ("this looks like an
  `index_template` dump; use `--type index_template`") instead of Node's silent nonsense PUTs.

### 3.4 Ordering guidance (README "full restore order")

Restore order:

**`component_template` → `index_template` → `template` → `policy` → `script` → (`index` | `settings` [+ `analyzer`]) → `mapping` →
`alias` → `data`.**

* Component templates must exist before index templates that are `composed_of` them, unless
  `ignore_missing_component_templates` is set.
* ISM `ism_template` auto-attaches policies at index creation, so policies come before indices.
* Aliases need their indices to exist.
* `multielasticdump`'s restore order is `template, index, settings, analyzer, mapping, alias, data`.

An integration test exercises exactly this order (M13).

---

## 4. Per-type specifications

Each section follows the same order: Node behaviour, then engine differences, then the Rust design, then tests and acceptance.

### 4.1 `index`

**Node [verified]:**
* **GET:** `GET {url}/` (`type==='index'` skips `_settings`). The response for one index is `{idx:{aliases,mappings,settings}}`.
  * `_.omit(output[base.index], settingsExclude)` strips `settings.index.{version,creation_date,uuid,provided_name,resize,
    routing.allocation.initial_recovery}`, but **only for the key equal to the URL index**.
  * The result is written as a string literal.
* **SET:** for each `(name, body)` in the file, `PUT {output url}` with `_.omit(body, settingsExclude)`, i.e.
  `{aliases, mappings, settings}`. This creates the index in one call. The ES<7 `.settings` unwrap applies only to `settings`,
  not `index`.
* **Bugs:**
  1. A host-only input sends `GET /` and dumps the cluster banner. Restore then iterates `name`, `cluster_name`, `version`, … and
     PUTs garbage.
  2. With a wildcard or comma list, the excludes are not applied (key ≠ `base.index`). Restore then fails:
     `private index setting [index.uuid] can not be set explicitly` [uncertain wording].
  3. The file index name is ignored, so a multi-index file PUTs every index to the same URL, and the second gets
     `resource_already_exists`.
  4. There is no `--skip-existing` handling, though `analyzer` and `mapping` have it.

**Engines:**
* `GET /{idx}` has the same shape on ES 8/9 and OpenSearch 2/3.
* ES data-stream backing indices add `"data_stream": "<name>"` to the response, which `PUT /{idx}` rejects [uncertain]. Strip it
  with a warning, since restoring a backing index as a plain index is rarely what anyone wants (Q8).
* Hidden and system indices are excluded by the default `expand_wildcards=open`.

**Rust:**
* **Dump:**
  * `GET /{index}` when an index or pattern is given.
  * Host-only → `GET /*` (fixes bug 1), with the same no-dot-filtering parity note as data.
  * `strip_settings` on **every** key (fixes bug 2).
  * Output is one string-literal line.
* **Restore:** for each index, pick the target like `settings` does (output index or file key, plus prefix/suffix; many→one
  refused). Then build the body:
  * `settings`: run the `apply_settings` strip chain (`SETTINGS_NOT_CREATABLE`, `--excludeSettings`, and cross-flavor stripping
    when the source is known or a warning when it is not).
  * `mappings`: `typeless_mapping` (ES ≥ 7 / OpenSearch); typed ES<7 targets keep the type wrapper.
  * `aliases`: kept as-is.
  * Drop `data_stream`.
  * Send one `PUT /{target}`. Handle `resource_already_exists` with `--skip-existing` like `settings` does.
* **Reuse:** factor the body builder out of `apply_settings` and `apply_mapping`. This must not change the behaviour of
  `settings` or `mapping`.

**Tests (all profiles):**
* Seed an index with custom analysis, a mapping and two aliases (one with `filter` + `is_write_index`). Then:
  * rs dump → rs restore: compare `GET /dst` minus generated keys;
  * Node dump → rs restore;
  * rs dump → Node restore (Node can only target one index);
  * a host-only multi-index dump restored with `--prefix`;
  * a wildcard dump has no `uuid`/`creation_date` (regression for Node bug 2);
  * the cross-engine file leg: OpenSearch `index.replication.type` is stripped going to ES, and ES `_tier_preference` going to OS.
* Mock: request lines are exactly `GET /src`, `PUT /dst`.

**Acceptance:** same-engine round trip is byte-identical after canonical sort. A Node-produced file restores, and a Rust file
restores with Node. Cross-engine legs are green on `all`.

### 4.2 `alias`

**Node [verified]:**
* **GET:** `GET {host}/{base.index}/_alias/{base.type || '*'}`. The raw body is written as a string literal. Shape:
  `{idx:{aliases:{name:{filter?,index_routing?,search_routing?,routing?,is_write_index?,is_hidden?}}}}`.
* **SET:** builds one `POST /_aliases` with `actions: [{add: {index: <file key>, alias, ...options}}]` for every index × alias,
  then sends a **single atomic request**.
  * It never removes or replaces aliases.
  * It uses the **file's index key** and ignores `--output-index`, `--prefix` and `--suffix`.
* **Bugs and dead behaviour:**
  1. A host-only input gives `/undefined/_alias/*` → 404 `index_not_found`. Users must pass `/*` or an index.
  2. Any index entry whose `aliases` is missing or empty aborts the whole restore with `no aliases detected`. This happens e.g.
     with `GET /idx/_alias` output, or with a hand-edited file [uncertain whether `GET /idx/_alias/*` ever returns an empty entry].
  3. An empty file document (`{}`) POSTs `{"actions":[]}`, which the server rejects ("no actions specified") [uncertain wording].
  4. The README URL form `host/index/alias` differs from the Rust URL rule (§2.1).

**Engines:**
* Same API on ES 8/9 and OpenSearch 2/3.
* ES 7.14+ data-stream aliases appear keyed by data-stream name, and `add` with `index: <ds>` works.
* ES-only option: `is_write_index` on data-stream aliases (fine).
* Aliases with `filter` carry query DSL, which may use engine-specific queries (rare). The server error is surfaced.
* Cross-engine is otherwise 1:1.

**Rust:**
* **Dump:**
  * Index from the endpoint, `*` when host-only (fixes bug 1).
  * The alias filter comes from `doc_type` (`--input-index idx/alias`).
  * Send `GET /{index}/_alias/{alias|*}`. Output is a string literal.
* **Restore:**
  * Normalise and skip entries with no aliases at debug level (fixes bug 2).
  * Target index = the file key, or `--output-index` when the file has exactly one index (many→one refused), then prefix/suffix
    applied. **Divergence from Node**, documented: without it a prefixed data restore gets aliases pointing at the old names
    (Q4).
  * Send all actions in **one atomic `POST /_aliases`** (parity), skipped when empty (fixes bug 3).
  * On failure with `--ignore-errors`, retry per index so that one bad index (missing or system) does not block the rest, and
    report the failed count.
  * Option keys are passed through unchanged.
* **URL form:** for `--type alias` only, when `--input` has exactly two path segments and no `--input-index`, **warn** that Node
  reads this as `index/alias`, and interpret it as Node does. `--input-index` is the unambiguous form (Q3).

**Tests:**
* Round trip with filter, routing and write-index aliases.
* Node fixture `test/test-resources/alias.json` (copy into `tests/fixtures/node/`) restores with Rust.
* Rust dump restores with Node.
* Alias-name filter via `--input-index idx/al*`.
* A prefix restore points aliases at the prefixed indices.
* An empty-alias index is skipped.
* ES-only: an alias on a data stream (create a DS through the built-in `logs-*-*` template, which needs stack templates; enabled
  in the harness).
* Mock: one `POST /_aliases` whose action list matches the golden file.

**Acceptance:** `GET /_alias` on the target equals the source for the restored indices, on all profiles plus cross legs.

### 4.3 `analyzer`

**Node [verified]:**
* **GET:** same as `settings` (`GET {url}/_settings`, excludes only on the URL index key, string literal). **An analyzer file *is*
  a settings file.**
* **SET** (`_analyzer.js`):
  1. `PUT {output url}` (create the index empty). If it already exists: with `--skip-existing`, return 0 and **apply nothing**;
     otherwise fail.
  2. `GET /_cluster/health/{idx}?wait_for_status=green`. The callback only checks transport errors, so on a single node with
     `number_of_replicas: 1` (ES 8/9 and OS default) this **stalls for the default 30 s timeout**, gets 408 and carries on.
  3. For each index in the file and each key of its `settings` (normally only `index`): delete everything except `analysis`,
     then `POST {url}/_close`, `PUT {url}/_settings {"index":{"analysis":…}}`, and finally `POST {url}/_open`.
* **Bugs and dead behaviour:**
  * The close/open path exists but only ever runs on the index Node has just created, so it is equivalent to "create the index
    with `analysis`" plus a 30 s delay.
  * Every file index is applied to the single URL index. With several indices, closes run concurrently and the callback can fire
    more than once.
  * Flat-key settings (`"index.analysis.analyzer.x": …`) are iterated as objects and break.
  * The `_open` request has no `.catch`.
  * The output URL must carry an index (`PUT host` fails).

**Engines:**
* Analysis settings are static, so applying them to an existing index requires closing it, on all targets.
* ES 9 still supports `_close`/`_open`. `cluster.indices.close.enable` is true by default.
* A data stream's write index cannot be closed (error surfaced).
* Components that differ across engines are rejected by the server; no translation:
  * ES 8.10+ `synonyms_set` (synonyms API);
  * ES-only plugins (ICU/kuromoji are plugins on both);
  * OS-only analyzers from plugins.

**Rust:**
* **Dump:** identical to `settings`. Reuse `MetaKind::Settings` fetch and string literal.
* **Restore**, per index, with target selection as for `settings`:
  1. Extract `settings.index.analysis`, normalising flat keys via `index_settings_mut` (fixes the flat-key bug). If absent, warn
     and skip.
  2. If the target does not exist (`HEAD /{target}` 404): `PUT /{target} {"settings":{"index":{"analysis":…}}}`. That is one
     call, no health wait and no close/open, with the same end state as Node.
  3. If it exists:
     * with `--skip-existing`: skip (parity);
     * with `--closeIndex` (rs-only): `POST _close` → `PUT _settings` → `POST _open`, then
       `GET _cluster/health/{t}?wait_for_status=yellow&timeout=60s`. **Always attempt `_open`** even if the PUT failed, and report
       both errors.
     * otherwise: error with a hint naming both flags.
* **Avoid Node's 30 s stall:** no green wait. Document it as a performance divergence.

**Tests:**
* Restore to a new index: analyzer present, and a `_analyze` call with the custom analyzer works.
* Restore to an existing index with `--closeIndex`: analyzer added, docs still searchable, index open.
* `--closeIndex` with an invalid analyzer: index still open, exit 1.
* Node analyzer dump → rs restore.
* rs dump → Node restore (assert success, and measure that Node takes ≥ 30 s on a single node: a soft check that documents the
  quirk).
* Cross-engine leg.

**Acceptance:** `GET /t/_settings` `analysis` equals the source's after canonical sort. No path leaves the target index closed.

### 4.4 `template` (legacy), `component_template`, `index_template`

**Node [verified] (`_template.js`):**
* **GET:** `GET {host}/_{type}/{base.index || '*'}` (`_template`, `_component_template`, `_index_template`). Shapes:
  * legacy: `{name:{order,index_patterns,settings,mappings,aliases,version?}}`;
  * composable: `{index_templates:[{name,index_template:{…}}]}` / `{component_templates:[{name,component_template:{…}}]}`.
* **Filtering** only on GET, when `--filterSystemTemplates` (default **true**):
  * legacy: drops every template whose name matches the template regex;
  * composable (`featureFlag`): drops every entry of the array under the first key whose `name` matches the regex.
  * Default regex: `^(metrics|logs|.+_audit_log|.+-index-template|\..+)$`.
  * Written as a string literal, whether filtered or not.
* **SET:**
  * composable files are turned into `{name: body}` via `_transformTemplates`;
  * then, in parallel, `PUT {host}/_{type}/{name}` with the body as dumped;
  * PUT **overwrites** existing templates.
* **Quirks and bugs:**
  1. `help.txt` says the default is `((metrics|logs|\..+)(-.+)?)` and that it removes "metrics-\*-\* and logs-\*-\*". Both are
     wrong. The code default only drops the exact names `metrics`/`logs`, dot-names, `*_audit_log` and `*-index-template`.
  2. On ES 8/9 the built-in **component** templates (`logs@mappings`, `logs@settings`, `metrics@*`, `ecs@mappings`,
     `data-streams@mappings`, `synthetics@*`, …) and index templates such as `synthetics` and `ilm-history-7` are **not
     filtered**, so they get dumped and restored [uncertain: exact list per version; enumerate in the harness].
  3. Filtering never applies to file → cluster.
  4. A type/file mismatch (e.g. an `index_template` file with `--type template`) PUTs `_template/index_templates` with an array
     body.
  5. Parallel PUTs, so there is no ordering. Combined with the absence of any `component_template` → `index_template`
     coordination, the user must run component templates first.
  6. Templates with ES<7 typed mappings (`template_6x.json`) fail on ES ≥ 7 / OS.

**Engines:**
* Legacy `_template`: deprecated since ES 7.8 but **still present in ES 9** (v9 API docs list GET/PUT/DELETE as
  "Deprecated, Generally available") [verified]. Present in OpenSearch 2/3. Composable templates win over legacy ones.
* `_component_template` and `_index_template`: ES ≥ 7.8 and OpenSearch ≥ 1.0.
* **ES 9.2+** adds read-only `created_date`, `created_date_millis`, `modified_date` and `modified_date_millis` to GET responses
  of both [verified in the v9 API docs]. Strip them on restore [uncertain whether PUT rejects them]. The harness's 9.1.2 does not
  emit them, so add a unit test with a 9.2-shaped fixture.
* ES-only template body fields:
  * `allow_auto_create`, `ignore_missing_component_templates`, `deprecated`;
  * `template.lifecycle` (data-stream lifecycle, ES 8.11+);
  * `data_stream.{hidden,allow_custom_routing}`, `data_stream_options` (ES 9) [uncertain list].
* OpenSearch-only: `data_stream.timestamp_field`, and `context` (OS 2.16+) [uncertain list].
* Settings inside templates carry the same engine-specific keys as index settings (`index.lifecycle.*` and `_tier_preference`
  vs `index.plugins.*` and `index.replication.type`).
* Mapping types that do not exist on the other engine are rejected by the server; no translation:
  * ES-only: `semantic_text`, `dense_vector`, `constant_keyword`, `aggregate_metric_double`, `version`, `flattened`, …;
  * OS-only: `knn_vector`, `flat_object`, …
* An overlapping pattern at equal priority with an existing template fails ("multiple index templates may not match"). On ES
  this typically hits the built-in `logs` (pattern `logs-*-*`, priority 100) [uncertain priority].

**Rust:**
* **Dump:**
  * Name filter from the endpoint index, `*` by default.
  * `--filterSystemTemplates` + `--templateRegex` behave exactly like Node.
  * Then `--filterManaged` (Q2) also drops entries whose `_meta.managed == true` and lists them at info level. Node-shaped
    output, string literal.
* **Restore:**
  1. Detect the shape: a legacy map, a composable list, or a plain `{name: body}` map for composable types (rs-friendly). A
     mismatch with `--type` is an error with a hint (fixes quirk 4).
  2. **Strip:** the 9.2 date fields; cross-engine fields (above) when the source flavor is known (cluster→cluster) or, for file
     input, when the target rejects them. Same policy as settings: warn and suggest `--excludeSettings` / the new
     `--excludeTemplateFields` (Q6). Apply `strip_for_flavor` to `settings`/`template.settings`, and `typeless_mapping` to legacy
     `mappings` and `template.mappings` for ES ≥ 7 / OS (fixes quirk 6).
  3. **Ordering:**
     * Sequential PUTs in file order.
     * For `index_template`, first check that each `composed_of` entry exists on the target (`GET /_component_template/{n}`),
       unless it is listed in `ignore_missing_component_templates`. If missing, fail early with "restore component templates
       first (`--type component_template`)".
  4. `PUT /_{kind}/{name}` overwrites (parity). With `--skip-existing`, add `?create=true` and treat the "already exists" 400 as a
     skip. Supported on legacy and composable on both engines [uncertain for OS component templates: probe].
  5. Against ES ≥ 8 with `--type template`, log one warning that legacy templates are deprecated and lose to composable ones.
     Surface server `Warning:` headers at debug.

**Tests:**
* For each of the three kinds:
  * rs round trip;
  * the Node fixtures (`template_7x.json`, `template_7x_component_template.json`, `template_7x_index_template.json`) → rs
    restore, then `GET` equals fixture after decode;
  * rs dump → Node restore;
  * Node and rs dumps of the same cluster are equal after decode (filtering parity, including the default regex);
  * `template_6x.json` restores on ES 8/9 and OS with the type unwrapped.
* ES profiles:
  * built-in managed templates are absent from the dump by default, and present with `--filterManaged=false`;
  * `index_template` before its component template fails with the hint.
* A unit test for 9.2 date stripping.
* Cross legs: an ES index template with `template.lifecycle` restored to OS is stripped with a warning; an OS `context`
  template restored to ES is stripped.

**Acceptance:** all three kinds round-trip on every profile. Node fixtures restore. The Rust dump with default flags equals
Node's after decode on OpenSearch, and on ES equals Node's minus the `_meta.managed` entries listed in the log.

### 4.5 `policy` (ES ILM / OpenSearch ISM)

**Node [verified] (`_policy.js`):**
* **GET:**
  * OpenSearch: `GET {host}/_plugins/_ism/policies/{base.index || ''}`. Note the trailing slash when no name is given.
  * ES: `GET {host}/_ilm/policy/{base.index || ''}`.
  * The raw body is written as a string literal.
* **SET:** `async.forEachOf(data, (_policy, name) => PUT {ilm|ism}/{name} {policy: _policy.policy})`.
* **ES ILM: works.**
  * GET shape: `{name:{version, modified_date, policy:{phases,_meta?}, in_use_by:{indices,data_streams,composable_templates}}}`.
  * Taking `.policy` drops the read-only fields.
  * But **all built-in policies are dumped and restored**. On ES 8/9: `logs`, `metrics`, `synthetics`, `7-days-default`
    … `365-days-default`, `ilm-history-ilm-policy`, `slm-history-ilm-policy`, `watch-history-ilm-policy-16`, `.fleet-*`,
    `.deprecation-indexing-ilm-policy`, … [uncertain exact list and which carry `_meta.managed: true`].
* **OpenSearch ISM is broken:**
  1. The list response is `{policies:[{_id,_seq_no,_primary_term,policy:{policy_id,description,last_updated_time,
     schema_version,error_notification,default_state,states,ism_template}}], total_policies:N}` [verified, OpenSearch ISM API
     docs]. `forEachOf` iterates `policies` and `total_policies`, sending `PUT _plugins/_ism/policies/policies` with `{}`
     (`[].policy` is undefined) → 400.
  2. The single-policy response (`{_id,_version,_seq_no,_primary_term,policy}`) iterates `_id`, `_version`, …, which is equally
     broken.
  3. Even with correct iteration, PUT on an **existing** ISM policy returns 409 `version_conflict_engine_exception` unless
     `?if_seq_no=&if_primary_term=` is sent (by design) [verified, ISM docs and index-management#266].
  4. The list is paginated (`size`, `from`, `sortField`, `sortOrder`, `queryString`) [verified]. Node sends no `size`, so big
     clusters are truncated [uncertain default size, believed 20].
  5. Whether the trailing-slash GET `…/policies/` routes to the list handler on OS 2/3 is [uncertain]. Verify with Node in the
     harness.
* The version gate (≥ 6.6.0) passes on OpenSearch via `force-os-version`.
* Cross-engine: Node would PUT ILM bodies into ISM and vice versa, which the server rejects (400).

**Engines:**
* ES 8.6 and 9.x: same `_ilm/policy` API.
  * The `searchable_snapshot` action and the `frozen` phase need an enterprise license. The bitnami images run basic, so such
    policies fail to PUT [uncertain message].
  * `freeze` was removed in 8.0.
  * `migrate` is implicit in warm/cold.
* OpenSearch 2.x/3.x: `_plugins/_ism`. The `_opendistro/_ism` alias is legacy (ignore). ISM fills defaults on PUT (`retry`
  blocks, `copy_alias`), so an echo is not byte-equal to the input.

**Rust (M11, same-engine):**
* **Dump:**
  * ES: `GET /_ilm/policy[/{name}]` → Node shape.
  * OS: page through `GET /_plugins/_ism/policies?size=1000&from=N` until `total_policies` is reached (fixes Node bug 4), then
    write the **list shape** `{policies:[…], total_policies}`. A single name → `GET …/policies/{name}`, normalised into the list
    shape: a small Node divergence, which is moot because Node cannot restore either.
  * `--filterManaged` drops ES policies with `policy._meta.managed == true`.
  * A name filter with a wildcard is supported on ES natively (`_ilm/policy/foo*`? [uncertain]); on OS it is applied client-side.
* **Shape detection on restore:**
  * **ILM** = map whose values have `policy.phases`.
  * **ISM** = `policies[]` list, or a single `{_id, policy:{states}}`, or a map whose values have `policy.states`.
  * Anything else is an error.
* **Restore to ES (ILM file):** `PUT /_ilm/policy/{name}` with `{"policy": P}`, where `P` keeps `phases`, `_meta` and
  `deprecated` (if present). Overwrite is parity. `--skip-existing` → `GET` first, skip on 200.
* **Restore to OS (ISM file):** for each policy:
  1. Body `{"policy": P}` with `policy_id`, `last_updated_time` and `schema_version` stripped, plus `last_updated_time` inside
     `ism_template[]` entries [uncertain which of these PUT rejects; strip all, which is safe].
  2. `PUT /_plugins/_ism/policies/{_id}`.
  3. On 409, if `--skip-existing`: skip. Otherwise `GET` the current `_seq_no`/`_primary_term` and re-PUT with
     `?if_seq_no=&if_primary_term=`. That is overwrite semantics, consistent with ILM (Q5). A second 409 is a real concurrent
     edit: fail.
* **Cross-engine (ILM file → OS, or ISM file → ES, or cluster→cluster between flavors):** refuse with exit 1 and a message
  naming `--policyTranslate=ilm-to-ism` (when that direction is supported) or "ISM → ILM is not supported".
* **Not done:** attaching policies to indices. That is ES `index.lifecycle.name` (already carried in settings and templates),
  and on OS `ism_template` or `POST _plugins/_ism/add`. Document this.

**Tests (M11):**
* ES (8.6, 9.1):
  * create two user policies (hot rollover + delete; warm forcemerge/shrink/allocate);
  * rs dump: built-ins filtered by default;
  * restore to a clean name space (delete first), then `GET` equals source `.policy`;
  * Node dump → rs restore;
  * rs dump → Node restore works (parity).
* OS (2.x, 3.x):
  * create 25 policies to exercise pagination; rs dump has 25;
  * restore to the same cluster (overwrite path: 409 → seq_no PUT) and after delete (create path);
  * `--skip-existing` skips;
  * Node dump → rs restore;
  * **rs dump → Node restore fails**: an assertion documenting the Node bug;
  * Node dump count = min(25, default page size), which documents the truncation.
* Cross legs: an ILM file to OS (and ISM file to ES) exits 1 with the refusal message.

**Acceptance:**
* ILM and ISM same-engine round trips are equal after removing server-filled defaults. Compare with ISM's echo normalised, i.e.
  drop `retry` and `copy_alias` defaults when they match the defaults.
* No silent cross-engine writes.

### 4.6 ILM → ISM translation (M12, optional, rs-only)

**Feasibility.**
* ILM is a fixed, ordered phase pipeline. Each phase is `min_age` plus a fixed action set, and the order of actions within a
  phase is chosen by ES.
* ISM is an arbitrary state machine: states with ordered actions, and transitions on conditions.
* **ILM → ISM is expressible for a common subset. ISM → ILM is not:** cron transitions, arbitrary graphs, notifications, rollup,
  transform, snapshot, close/open, alias actions and `ism_template` have no ILM form. Refuse ISM → ILM entirely.

**Mapping (strict: anything not listed refuses the whole policy, naming the offending phase and action):**

| ILM | ISM | Notes |
|---|---|---|
| phase `hot`/`warm`/`cold`/`delete` (in order) | one state per present phase; `default_state` = first | `frozen` → **refuse** (searchable snapshots only) |
| next phase `min_age: X` | transition to next state `conditions.min_index_age: X`, or **`min_rollover_age`** when the hot phase has `rollover` | ILM `min_age` counts from rollover when rolled over; ISM `min_index_age` counts from creation, so `min_rollover_age` gives closer semantics [uncertain: available on OS 2.x/3.x, probe] |
| `rollover{max_age,max_docs,max_size,max_primary_shard_size}` | `rollover{min_index_age,min_doc_count,min_size,min_primary_shard_size}` | `max_primary_shard_docs`, any `min_*` rollover condition (ES 8.4+) → **refuse**. ISM rollover needs `index.plugins.index_state_management.rollover_alias` on the index (ILM uses `index.lifecycle.rollover_alias` or data streams). **Warn**; settings are not rewritten |
| `set_priority{priority}` | `index_priority{priority}` | |
| `allocate{number_of_replicas,include,exclude,require}` | `replica_count{number_of_replicas}` + `allocation{include,exclude,require}` | `total_shards_per_node` → refuse |
| `forcemerge{max_num_segments}` | `force_merge{max_num_segments}` | `index_codec` → refuse |
| `shrink{number_of_shards \| max_primary_shard_size}` | `shrink{num_new_shards \| max_shard_size}` | `allow_write_after_shrink` → refuse |
| `readonly{}` | `read_only{}` | |
| `delete{}` | state with `delete{}` | `delete_searchable_snapshot` ignored (no-op without snapshots) |
| implicit `migrate` in warm/cold, `migrate{enabled:false}` | dropped with a warning | OpenSearch has no data tiers |
| `searchable_snapshot`, `downsample`, `wait_for_snapshot`, `unfollow`, `freeze` | **refuse** | no equivalent (ISM `snapshot` *creates* a snapshot; not equivalent to `wait_for_snapshot`) |
| `_meta` | `description` (from `_meta.description`) | other `_meta` dropped with a warning |

**Other details:**
* There is no `ism_template` in the output: ILM attachment happens via index settings and templates, which this tool does not
  rewrite. Print a note on how to attach policies (`ism_template` or `_plugins/_ism/add`).
* `--policyTranslate=ilm-to-ism` translates every policy or none. A refused policy fails the run unless `--ignore-errors` is set,
  in which case the others proceed and the failures are reported.

**Tests:**
* Unit: a table of ILM inputs → expected ISM JSON, including each refusal.
* Harness (cross profile and file leg): translate the two ES user policies, PUT them on OS, then `_plugins/_ism/explain` on a
  test index attached via `add` shows the expected first state.

**Acceptance:** every documented mapping is unit-tested. Every refusal names the phase and action. No partially translated
policy is ever written.

### 4.7 `script` (stored scripts)

**Node [verified] (`_script.js`):**
* **GET:** `GET {host}/_cluster/state/metadata?filter_path=**.stored_scripts.{base.index || '*'}`, then `_transformMeta`
  produces `{stored_scripts:[{_id, found:true, script:{lang, source, options?}}]}` (mimicking `GET _scripts/{id}`), written as a
  string literal. No scripts → `{stored_scripts:[]}`.
* **SET:** in parallel, `POST {host}/_scripts/{_id}` with `{script: …}` (`_.pick(s,'script')`).
* **Quirks:**
  * Undocumented in help and README.
  * The version gate ≥ 7.10.0 is arbitrary (stored scripts predate it).
  * The cluster-state API needs the `monitor` cluster privilege and is heavy on large clusters. There is no list API for stored
    scripts, so this is the only way.

**Engines:**
* Same on ES 8/9 and OS 2/3: cluster state `metadata.stored_scripts`, `PUT|POST _scripts/{id}`.
* Painless is compiled at store time, so engine-specific Painless (ES 8+ fields API, newer whitelist methods) fails loudly on
  OpenSearch, and vice versa. Mustache search templates are portable.
* Cross-engine: allowed, and server errors are surfaced with the script id.

**Rust:**
* **Dump:** the same request (with `filter_path` URL-encoded), the same transform and output shape.
* **Restore:**
  * Accept the Node list shape, or a `{id: {lang, source, options}}` map.
  * Send sequential `PUT /_scripts/{id}` with `{"script":{lang,source,options?}}`. PUT and POST are equivalent here; use POST for
    request-line parity in mock tests.
  * Empty `options` is dropped.
  * Existing scripts are overwritten (parity). `--skip-existing` → `GET _scripts/{id}` first.
* No version gate except ES < 6, which errors.

**Tests:**
* Seed one Painless script and one Mustache search template; round trip.
* Node fixture `stored_scripts.jsonl` → rs restore.
* rs dump → Node restore.
* `--searchWithTemplate` with a restored template returns the expected docs: a nice end-to-end check of the existing feature.
* Cross legs with the Mustache template only.

**Acceptance:** `GET _scripts/{id}` on the target equals the source for all restored ids, on every profile.

---

## 5. Test plan summary (podman harness)

* **New files:** `tests/it/meta_types.rs` (round trips per type), `tests/it/node_interop_types.rs` (uses `/tmp/ed`; SKIP when
  absent, like `meta.rs`), additions to `engine.rs` (engine-specific asserts) and `cross.rs` (file legs: each plain profile
  exports one file per type to `$IT_CROSS_DIR/<engine>/` and restores the other engine's).
* **Golden fixtures:** copy `elasticsearch-dump/test/test-resources/{alias.json, template_*.json, stored_scripts.jsonl}` into
  `tests/fixtures/node/`. Add Node-produced dumps captured from each engine on first run (`index`, `analyzer`, ISM/ILM policy,
  managed-template-heavy ES dumps). Keep them small.
* **Mock contract tests** (`tests/support/mock_es.rs`, no containers): for every type, the exact request lines (method, path,
  query, body) for dump and restore, including:
  * the ISM 409 → GET → conditional PUT sequence;
  * `?create=true` for `--skip-existing` on templates;
  * the alias single-POST / per-index fallback;
  * the analyzer create path (no close/open) vs the `--closeIndex` path (close, put, open even on put failure).
* **Profiles:** every type on `opensearch-{plain,secure}` (OS 2 and 3 via `OS_TAG`), `elasticsearch-{plain,secure}` (9.1.2),
  `elasticsearch8-{plain,secure}` (8.6.1), plus `cross` for cluster→cluster.
  * Secure profiles check that the existing auth/TLS plumbing covers the new endpoints.
  * Policy and template tests need `manage_ilm`/`manage_index_templates`; they run as the admin user.
* **Isolation:** templates, policies and scripts are cluster-global, so tests must use `unique()` names and clean up in a
  `Drop`-style guard. Parallel tests must never assert on the *whole* list; they filter by their own prefix via the name filter.
  Built-in filtering tests assert on known built-in names only.
* **Format parity check:** for each type, dump the same seeded cluster with Node and Rust. Assert both lines are string literals,
  and that the decoded JSON is equal (ES: modulo `--filterManaged` entries, which are listed).

---

## 6. Milestones

Effort is in focused working days for one engineer or agent familiar with the code base. Each milestone ends green on `all`.

### M7: Meta-type foundation (2–3 d)
* Move `meta.rs` into `src/meta/` (§3.1), with no behaviour change.
* `DumpType` gets all 11 values, with an updated error text.
* `codec::decode_any`, `--metaFormat`, and per-kind shape detection with hint errors.
* `--templateRegex` and `--filterSystemTemplates` become real flags (`regex` crate, JS-feature detection); `--filterManaged`.
* Mock-test scaffolding for multi-request sequences.
* **Accept:** existing unit, mock and IT suites pass unchanged. `--type alias` on a file→file pair still gives the usage error.
  The CLI contract tests are updated.

### M8: `index` and `alias` (3–4 d)
* §4.1 and §4.2, including the host-only fixes, per-key stripping, `--output-index`/prefix for aliases, the empty-alias skip,
  the two-segment URL warning, and the `--ignore-errors` per-index fallback.
* **Accept:** §4.1 and §4.2 acceptance on all profiles; Node interop both directions.

### M9: `analyzer` (2 d)
* §4.3: the create-with-analysis path, `--skip-existing`, and `--closeIndex` with guaranteed reopen.
* **Accept:** §4.3 acceptance. A fault-injection mock (PUT `_settings` 400) proves `_open` is still sent.

### M10: templates (three kinds) and `script` (4–5 d)
* §4.4 and §4.7: filtering parity, managed filtering, 9.2 field stripping, cross-engine field stripping, typed-mapping unwrap,
  the `composed_of` pre-check, sequential PUTs, `?create=true`, and the legacy deprecation warning. Stored scripts.
* **Accept:** §4.4 and §4.7 acceptance. The list of ES 8.6/9.1 built-ins seen in the harness is recorded in the README.

### M11: `policy`, same-engine (3–4 d)
* §4.5: ILM dump/restore; ISM paginated dump, list-shape file, 409 → seq_no overwrite, `--skip-existing`; cross-engine refusal.
* **Accept:** §4.5 acceptance, including the tests that document Node's ISM bug and truncation.

### M12 (optional): ILM → ISM translation (4–6 d)
* §4.6 behind `--policyTranslate=ilm-to-ism`.
* **Accept:** §4.6 acceptance. Docs carry a semantic-differences table (min_age clock, no tiers, rollover alias setting).

### M13: Docs, parity matrix, full-restore test (2 d)
* README "Types" section with the §3.4 order, divergences (every Node bug fixed here, each with a one-line rationale), and the new
  flags.
* `docs/PARITY.md` rows for all 11 types. `--help` text lists `script`.
* One IT test per engine doing a full backup and restore of a seeded cluster in the §3.4 order (every type, then data counts).
* **Accept:** a doc review, and the full-restore test green on all profiles plus cross.

**Totals:**
* M7–M11 + M13: **16–20 d** (call it ~17–22 with harness flakiness and the ES built-in surprises).
* Including M12: **~21–28 d**.

---

## 7. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Cluster-global resources in a shared test cluster (templates, policies, scripts) | Flaky or parallel-unsafe tests; a restored template affecting other tests' indices | Unique name prefixes; index patterns scoped to `it_<pid>_*`; cleanup guards; no whole-list asserts |
| Restoring ES built-in templates/policies into another version or engine | Older built-ins overwrite newer ones (ES re-installs them later); on OpenSearch, `logs-*-*` templates hijack user indices | `--filterManaged` default on (Q2); log what was skipped |
| A restored index template collides with built-in priorities (`logs-*-*`) | 400 on restore | Surface the error with the conflicting template name; document |
| ISM PUT field acceptance (`policy_id`, `last_updated_time`, `schema_version`) differs by OS version | Restore 400 on some versions | Strip all three (safe); probe on OS 2 and 3 in M11 |
| ISM page size / trailing-slash routing assumptions wrong | Wrong Node-parity tests | Tests assert the observed Node behaviour and document it, rather than hard-coding 20 |
| `_cluster/state` cost or permissions for `script` | Slow or 403 on large or locked-down clusters | Keep `filter_path`; a clear 403 message naming the `monitor` privilege |
| `--closeIndex` leaves an index closed after a crash or Ctrl-C | Outage of that index | Always-reopen path, a Ctrl-C handler that reopens before exit, and a loud log line naming the index while it is closed |
| Cross-engine field lists (templates, settings) incomplete | Restore 400 on some content | Same approach as settings: warn and extend from harness failures; `--excludeSettings` and Q6 escape hatch |
| ILM→ISM semantic drift (age clocks, rollover alias) | Policies that behave differently | Strict refusal list, an explicit warnings table, opt-in only |
| Node URL form `host/index/alias` vs the Rust base-path rule | Silent misparse | Two-segment warning plus Node interpretation for `alias` only (Q3) |
| Concurrent harness work (ES profiles still landing) | Merge conflicts in `harness.rs`, `up.sh`, `meta.rs` | Start M7 after the ES harness work is committed; keep new tests in new files |

---

## 8. Open questions for Robert

1. **Policy translation:** ship M12 (ILM → ISM best-effort, opt-in), or only refuse cross-engine policy restores? Is ISM → ILM
   ever needed? (The plan refuses it.)
2. **Managed ES built-ins:** should `--filterManaged` default to **on**, skipping `_meta.managed: true` templates and policies on
   dump? That is a divergence from Node's regex-only filter. Alternative: off by default, but on automatically for cross-engine
   restores.
3. **Alias URL form:** for `--type alias`, interpret a two-segment `--input` URL as Node's `index/alias` (with a warning), or
   require `--input-index idx/alias` and treat the URL with the Rust base-path rule?
4. **Alias target names:** apply `--output-index`/`--prefix`/`--suffix` to alias restore targets (a divergence, but consistent
   with prefixed data restores), or keep Node's "file key only"?
5. **ISM overwrite:** on an existing ISM policy, overwrite via `if_seq_no` by default (consistent with ILM and templates), or
   skip unless a flag is set?
6. **Template field stripping on file restores** (unknown source engine): strip known foreign fields automatically, or only warn
   and require an explicit `--excludeTemplateFields` (a new flag, analogous to `--excludeSettings`)?
7. **`analyzer` on existing indices:** is `--closeIndex` wanted at all, given that it closes a live index? Node never really does
   this.
8. **Scope beyond Node:** ingest pipelines, data streams, SLM/snapshot policies, and ISM attachment are not Node types. Add any of
   them later as rs-only types?
9. **Multi-type convenience:** a rs-only `--type=index,alias,…` or a `multielasticdump`-style wrapper that runs the §3.4 order in
   one invocation? (Not planned; would follow M13.)

---

## Appendix A: Node source references (for implementers)

| Concern | File:symbol |
|---|---|
| Type dispatch and version gates | `lib/transports/elasticsearch.js` `get()`/`set()` |
| `index`/`settings`/`analyzer` GET, excludes | `lib/transports/__es__/_setting.js` `getSettings`; `settingsExclude` in `elasticsearch.js` constructor |
| analyzer restore | `_analyzer.js` `setAnalyzer` |
| alias | `_alias.js` `getAliases`/`setAliases` |
| templates, regex filter, `_transformTemplates` | `_template.js`; defaults in `bin/elasticdump` (`filterSystemTemplates`, `templateRegex`) |
| policies | `_policy.js` |
| scripts | `_script.js` `_transformMeta` |
| URL → index/type, prefix/suffix (output only) | `lib/parse-base-url.js`, `lib/ioHelper.js` |
| string-literal parsing | `lib/jsonparser.js` `parse` (plain `JSON.parse`) |
| Node test fixtures | `test/test-resources/{alias.json, template_{1x,2x,6x,7x}.json, template_7x_{component,index}_template.json, stored_scripts.jsonl}` |
| multielasticdump types and order | `bin/multielasticdump` `supportedTypes`, dump/restore arg lists |
