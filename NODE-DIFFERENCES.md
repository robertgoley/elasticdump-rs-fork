# Differences caused by Node elasticdump being incomplete or buggy

This file lists only the places where elasticdump-rs behaves differently **because Node elasticdump
(6.125.1, `lib/…` paths below) is incomplete, buggy, or has dead behaviour**. Two other lists are separate:

* intentional design choices (zstd, staged split files, `--filterManaged`, …): README,
  "Intentional differences from Node elasticdump";
* every change to what gets loaded (stripped settings, conversions, renames): TESTING-MATRIX.md §3.

"File-compatible" means a file written by one tool restores with the other.

| # | Area | Node behaviour (source) | elasticdump-rs behaviour | File-compatible | Test coverage |
|---|---|---|---|---|---|
| 1 | PIT sort on OpenSearch | `searchBody.sort = ['_shard_doc']` for every engine (`lib/transports/__es__/_data.js:39`). OpenSearch rejects `_shard_doc` ("No mapping found for [_shard_doc]"), and a `_doc`-only sort was measured to lose documents across PIT pages | OpenSearch: `[{"_doc":"asc"},{"_id":{"order":"asc","unmapped_type":"keyword"}}]`; ES ≥ 7.12: `_shard_doc`; ES 7.10/7.11: `_doc` + `_id` | n/a (search only) | `engine::pit_request_shape_matches_engine`, `smoke::pit_dump_returns_all_docs_unsliced_and_sliced` (all profiles) |
| 2 | PIT on clusters without it | Opens a PIT and fails (ES OSS builds, OpenSearch < 2.4); sliced PIT fails on ES < 7.15 ("`slice` cannot be used outside of a scroll context") | Warns and falls back to scroll (sliced scroll for slices); same documents | n/a | `engine::pit_request_shape_matches_engine`, `data::exact_size_with_scroll_and_sliced_pit` on es710 / os13 |
| 3 | Dead throttling flags | `--intervalCap`, `--concurrencyInterval`, `--carryoverConcurrencyCount` are parsed and documented but have done nothing since 6.119.0 (p-queue removed) | Implemented with p-queue semantics, **only when set explicitly** | n/a | unit `http::gate::interval_cap_admits_cap_per_window`; IT `concurrency::interval_cap_and_throttle_timing` |
| 4 | `--offset` with scroll | Sends `from=` on the scroll request (`_data.js:108`); ES 7+/OpenSearch reject it | Skips the first N documents client-side | n/a | IT `smoke::size_and_offset`, contract `file_to_file_…_with_offset_and_size` |
| 5 | Client certificates | `--ca/--cert/--key` silently ignored unless `--tlsAuth` (`lib/transports/elasticsearch.js:102`) | Always honoured; warns when `--tlsAuth` is missing | n/a | `tls_auth` suite (secure profiles) |
| 6 | Hidden retries | `requestretry` with `maxAttempts: retryAttempts \|\| 5` (`elasticsearch.js:92`): `--retryAttempts 0` still retries 4×; strategy `HTTPOrNetworkError` (`:94`) never retries 429 | `--retryAttempts 0` = Node's implicit 4 retries unless `--noImplicitRetry`; 429 and 408 are retried; scroll continuations only when provably not applied | n/a | unit `http::retry::idempotent_requests_retry_network_5xx_429_408`, `scroll_continuations_retry_only_when_provably_not_applied` |
| 7 | OpenSearch policy restore never works | `async.forEachOf(data, …)` over the dump object (`_policy.js:47`): for an ISM dump (`{"policies":[…],"total_policies":N}`) the "policy names" are `policies` and `total_policies`; no `if_seq_no`/`if_primary_term`, so overwriting an existing ISM policy (409) is impossible | Iterates `policies[]`, strips `policy_id`/`last_updated_time`/`schema_version`, creates or overwrites with the sequence-number check; a second 409 is reported as a concurrent change | yes (same dump shape) | IT `meta_types::policy_round_trip_same_engine` (asserts Node's restore leaves the policy missing); contract `ism_overwrite_uses_sequence_numbers` |
| 8 | Policy list not paged | One `GET _plugins/_ism/policies/` (`_policy.js:11`): OpenSearch returns its default page (20) | Pages with `size=1000&from=N` until all are read | yes | contract `ism_list_dump_pages_through_all_policies`; IT dumps 25 policies |
| 9 | Alias host-only input | URL `${host}/${index}/_alias/…` with no index → `GET /undefined/_alias/*` (`_alias.js:12`) | `GET /*/_alias/*` | yes | contract `host_only_index_and_alias_dumps_use_wildcards_and_node_alias_url` |
| 10 | Alias restore aborts on empty lists | `cb(new Error('no aliases detected'))` for any index without aliases (`_alias.js:48`), failing the whole restore | Skips such entries | yes | IT `alias_round_trip_filters_prefix_and_node_forms` (wildcard dumps include alias-less indices) |
| 11 | Alias restore ignores output index / prefix / suffix | Uses the index names from the file as-is (`_alias.js:52`) | Targets follow `--output-index` (single source only), `--prefix`/`--suffix` and rename rules | yes | contract `alias_restore_is_one_atomic_post_with_prefixed_targets`; IT prefix restore |
| 12 | Index / settings host-only input | `GET ${base.url}/` with no index = `GET /` (`_setting.js:12`): writes the cluster banner as the "index" | `GET /*` | yes | contract `host_only_index_and_alias_dumps_use_wildcards_…`; IT wildcard dump |
| 13 | Generated settings not stripped for wildcard dumps | `settingsExclude` applied only to `output[this.base.index]` (`_setting.js:25`), so `uuid`, `creation_date`, `version`, `provided_name` survive for `*`/pattern dumps and the restore fails | Stripped for every index in the dump | yes | IT `index_round_trip_wildcard_dump_and_node_interop` |
| 14 | Analyzer restore | `PUT` the index first (fails if it exists; `_analyzer.js:78`), waits for `green` (`:95`, 30 s timeout; never green with replicas on one node), then close → PUT → open for every index in the file against the **one** output index; a failed PUT leaves the index **closed** (`:50`) | New index: created with the analysis, no green wait. Existing index: error, `--skip-existing`, or `--closeIndex` (close → PUT → always reopen, also on failure or Ctrl+C) | yes | IT `analyzer_create_close_index_and_failure_reopens`; contract `analyzer_close_index_reopens_even_when_the_update_fails` |
| 15 | Default `--templateRegex` | `^(metrics\|logs\|.+_audit_log\|.+-index-template\|\..+)$` (`bin/elasticdump:41`) misses the ES 8/9 built-ins (`*@template`, `*@mappings`, APM/OTel …); the help text documents a different default, `((metrics\|logs\|\..+)(-.+)?)` (`lib/help.txt:346`) | Same regex kept for parity, plus `--filterManaged` (default on) dropping `_meta.managed: true` templates/policies; help text shows the real default | yes | IT `managed_templates_and_policies_are_filtered_on_elasticsearch` |
| 16 | Unordered template restores | `async.forEachOf` PUTs all templates in parallel (`_template.js:107`); an index template can arrive before its component templates | Sequential; `composed_of` components are checked first with "restore component templates first (--type component_template)" | yes | IT `templates_round_trip_order_and_node_fixtures`; contract `template_skip_existing_uses_create_and_index_template_needs_components` |
| 17 | Wrong-type files | A dump of one `--type` restored with another is sent anyway (e.g. an alias dump PUT as settings) | Shape check: "this input looks like a --type X dump …; use --type X" | yes | contract `unknown_type_and_file_to_file_metadata_are_usage_errors`; IT templates test |
| 18 | Parallel script restore | `async.each` (`_script.js:64`) | Sequential, `--skip-existing` honoured | yes | IT `scripts_round_trip_search_template_and_node` |
| 19 | ISM → ES / ILM → OpenSearch | Sends the other engine's body to the wrong API and fails with a server error | Clear refusal; ILM → ISM only with `--convertIlmToIsm` | n/a | IT `cross::*policy*` |
| 20 | Typed 6.x template/mapping files into 7+/OpenSearch | Sent as-is; rejected by typeless clusters | Single mapping type unwrapped with an info log | yes | IT templates test (Node `template_6x.json` fixture) |

Found during this implementation (not in the original list): #2, #14 (closed-index on failure, many-to-one
application), #18, #19, #20. The Node interop tests run when Node elasticdump is installed at
`/tmp/ed/node_modules/.bin/elasticdump`; they also assert Node's broken ISM restore (#7) so a fixed Node release
would be noticed.

`it::node_interop::node_interop_every_type` (L-433) runs all 11 types in both directions (Node dump → Rust
restore, Rust dump → Node restore) on OpenSearch 2.19.7 and Elasticsearch 8.19.23: 22 directions per engine.
Node's dumps are committed as `tests/fixtures/node/<engine>-<version>/<type>.json` (unique prefix replaced by
`__U__`; re-record with `IT_RECORD_NODE_FIXTURES=1`), so the Node → Rust half also runs without Node
(`IT_NODE=0` forces that path). One shape difference is normalised by the test: for a single **named** ISM
policy, Node writes the raw `GET _plugins/_ism/policies/<id>` response while Rust always writes the list
shape `{"policies":[…],"total_policies":N}`; Rust restores both.
