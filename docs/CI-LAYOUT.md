# CI layout (INF-010)

| When | Workflow / job | Tiers | Notes |
|---|---|---|---|
| every PR and push | `ci.yml` `unit` | T0 unit, T1 contract | fmt, clippy, flake policy (INF-013), nextest `unit` + `contract` |
| every PR and push | `ci.yml` `supply-chain`, `security.yml` | T8 gates | vet, machete, native-code allow-list, pinned images, matrix freshness, unsafe baseline, advisories, SBOM diff, secret scan |
| every PR and push | `ci.yml` `it` | T2 subset | OpenSearch 2.19 plain + secure, Elasticsearch 9.1 plain, Elasticsearch 7.10 OSS |
| PRs touching `src/`, `Cargo.*`, `scripts/load` | `ci.yml` `load-smoke` | T4 | L0: 100k docs + 10k edge docs, digest-verified, metrics schema-checked |
| nightly 05:23 UTC | `nightly.yml` `it-full` | T2 full | all 16 profiles |
| nightly | `nightly.yml` `cross` | T3 | file legs per engine, then cluster to cluster |
| nightly | `nightly.yml` `load-nightly` | T5 | L1R (5M docs, zstd) on a hosted runner; full L1 runs on the soak host |
| nightly | `nightly.yml` `chaos` | T7 | nextest `chaos` profile; each test prints `CHAOS_SEED` |
| weekly (Sat 02:41 UTC) or manual | `soak.yml` | T6 | L2 / L1 on the runner named by the `SOAK_RUNNER` repository variable; skipped when unset |
| weekly (Mon 06:17 UTC) | `security.yml` schedule | T8 full | advisories, udeps, coverage, publishers, offline build |

Failures open **no ticket** (owner's decision). Every nightly/soak job and failed PR job uploads artefacts collected
by `scripts/ci/collect-artifacts.sh`: tool logs (`$IT_DIR/*.log`), `podman ps` and per-container logs, metrics
JSON and load digests under `results/`, golden diffs (`target/golden-diff`) and nextest JUnit files.

Owner set-up needed: the `SOAK_RUNNER` variable and a big runner (Phase B), branch protection requiring the PR jobs.
