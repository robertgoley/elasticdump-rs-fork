# Sources of the fixtures in tests/fixtures/node

## Copied from elasticsearch-dump (Apache-2.0)

These 12 files are byte-identical copies from
[elasticsearch-dump/elasticsearch-dump](https://github.com/elasticsearch-dump/elasticsearch-dump), directory
`test/test-resources/`, commit `600db62` (`600db6273a9fa1424d3a7474bb75ddc1fde25ff5`). They are unchanged and stay
under the Apache License 2.0, Copyright elasticsearch-dump contributors. The license text is in
[LICENSE-APACHE-2.0](LICENSE-APACHE-2.0) (the upstream `LICENSE.txt` at that commit, unchanged). Verify a copy with
`git hash-object FILE` against the blob id.

| file here | upstream path | commit | changes | license | git blob |
|---|---|---|---|---|---|
| alias.json | test/test-resources/alias.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | d29b8c029efa0bbf90ca6e8d2920d70c52c35f0b |
| stored_scripts.jsonl | test/test-resources/stored_scripts.jsonl | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | 70692768b78a5d16a771086f78372eea88f6a023 |
| template_6x.json | test/test-resources/template_6x.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | cced8e952e51364498d1a9b9d308ac4a0c9313a0 |
| template_7x.json | test/test-resources/template_7x.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | 8501f05386db8222a14cbe2df747a7bda62d1ff6 |
| template_7x_component_template.json | test/test-resources/template_7x_component_template.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | 4bec70dabd346882f185471d421e15f163a50601 |
| template_7x_index_template.json | test/test-resources/template_7x_index_template.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | e01179fd5987ded2037fc5d3f3edc266289c92b3 |
| bigint/bigint.json | test/test-resources/bigint.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | 6f1072dad7e8eba5ff3fd5b40418466f9d05dfca |
| bigint/bigint2.json | test/test-resources/bigint2.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | 8e6d7e4523b16478651faba4b739f02fd1b3c07d |
| bigint/bigint3.json | test/test-resources/bigint3.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | 6ff7c9fd2c5defa8ce9a719922780fe41cdd58af |
| bigint/bigint_mapping.json | test/test-resources/bigint_mapping.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | 46497cabb435726fd96492f203e677a3c45a2afd |
| bigint/bigint_mapping2.json | test/test-resources/bigint_mapping2.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | fa8872e9962ae4fa7162a5dac7a89e6697f721d5 |
| bigint/bigint_mapping3.json | test/test-resources/bigint_mapping3.json | 600db62 | unchanged | Apache-2.0, Copyright elasticsearch-dump contributors | 3f5b4d6d1a62dde22a1f44319249a1e6d07ba59b |

The blob ids were checked against the upstream tree on 2026-11-01 (L-808c): all 12 match.
bigint/SOURCE.md holds the same facts for the bigint files.

## Our own data (MIT)

`elasticsearch-8.19.23/` and `opensearch-2.19.7/` are output of elasticdump tools run against our own test data.
They are part of this project and are covered by the project's MIT license (see the root LICENSE).
