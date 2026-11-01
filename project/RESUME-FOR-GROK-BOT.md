# Resume elasticdump-rs (written for the Grok Bot picking this up)

## Project
A Rust rewrite of Node elasticdump, based on https://github.com/tenyears-tech/elasticdump-rs. The project lives at https://github.com/robertgoley/elasticdump-rs-fork (public). **Commit future work to `main` and push it to this fork with a normal `git push` (never force).** Never push to upstream (tenyears-tech) or any other repository, and never open PRs.

## Restore
    gh auth setup-git      # gh must be logged in as the fork owner
    git clone https://github.com/robertgoley/elasticdump-rs-fork.git /workspace/elasticdump-rs
    mkdir -p /workspace/elasticdump-rs-plan && cp -a /workspace/elasticdump-rs/project/plan/. /workspace/elasticdump-rs-plan/
`origin` is the fork and `main` tracks `origin/main`; that is the branch to work on and push. The old local branch `rewrite` (code only, no project/) was merged into `main` at a3b00c5 and is no longer the work branch.
The junk files `oom` and `test_query_elasticdump_rs_test_857109_16.json` were removed from git in L-803 and are now in .gitignore; if they show up again, never commit them; keep `target/` out of commits.
Keep project/plan/ and project/handoff/plan/PROGRESS.md in sync with /workspace/elasticdump-rs-plan when you commit (the OpenCode run-card.sh claim check reads handoff/plan/PROGRESS.md; update its line in handoff/SHA256SUMS).

## Status
- Tasks are done through **L-808b** (on `main`; L-643 c38c46d, L-644 4f9115c, L-644a a3b00c5, L-645 + D-4 48ca62e, L-646 7802d7e, L-647 4ccb2fb, L-648 34ecd28, L-108a 7976c8c, L-649 9736389, L-431a b0f9106 (TLS flake fix), L-650 acb12de (resume semantics end to end; setup-window signal fix), L-650a 384df3e (signal guard for metadata dumps to a file), L-651 e5319c7 (unconvertible ILM refused before any PUT; live ISM acceptance), L-638a 73a482f (l638 time limit sized to the --intervalCap worst case), L-652 ed53602 (odd but valid ILM; live on OS 1.3.20/2.19.7/3.9.0), L-650b ab6de91 (l650a timeout diagnostics: process state, wchan, gdb stacks, stderr; no hang found), L-653 44ee5bc (ES built-in policies convert or are refused clearly; ISM → ILM refusal names the policies; live on OS 1.3.20/2.19.7/3.9.0), L-654 89a0f46 (OpenSearch with compatibility.override_main_response_version detected at its real version via GET _nodes; an unresolvable 7.10.2 fails clearly; live on OS 1.3.20/2.19.7/3.9.0), L-655 f3e9884 (field types without an equivalent refused before any write, naming path, type and closest type; cross-engine copy to a missing target refused; live on OS 1.3.20/2.19.7/3.9.0 and ES 7.10.2-9.1.2), L-108b 38fb47c (handoff SHA256SUMS: the uncommitted bundle is a note; `sha256sum -c` passes), L-656 9c92547 (features the target lacks, ES 7.10 OSS and OS 1.3: data streams, ILM and X-Pack settings refused before any write naming the exclusion flag; OSS credentials warning; data-stream `create` hint; live on OS 1.3.20/2.19.7/3.9.0 and ES 7.10.2-9.1.2), L-455a a74656d (stored_fields without _source in the search body: still refused by default, the error names the body; already failing on 38fb47c), L-657 88b576d (integer hits.total and `_type` pinned end to end in tests/chaos_local_l657.rs; tests only), L-701 62f0c7d (egress-denied test runs: scripts/sup/egress.sh in a `podman pod --network none` with a logging DNS sink and OutNoRoutes counters; nightly job `egress`), L-651a a3f090a (policy set rolled back when the cluster rejects one mid-set: created deleted, overwritten restored, the error says whether rollback succeeded; wait_for_snapshot refused even in lenient mode; live on OS 1.3.20/2.19.7/3.9.0), L-455b f2818bf (behaviour change: a search body with "_source": false is refused unless --allowMissingSource, naming the search body; a _source filter whose hits lack _source likewise; live on OS 1.3.20/2.19.7/3.9.0 and ES 7.17-9.1), L-642a 748a15b (same-target check compares the output index after --prefix/--suffix; fixes it::data::cluster_to_cluster_with_prefix_suffix_and_gzip_requests), L-629a 0f08c23 (numeric `_id`/routing literals kept byte for byte, `-0` -> "-0", reversing L-629's Node match; fixes node_interop::xv_escaping_corpus_round_trips_byte_for_byte), L-702 948f73f (strace allow-list check scripts/sup/trace.sh), L-702a 259d3c4 (workflow lint fix-ups), L-642b a38c3bd (host-only output same-target fix-up), L-704 fdab304 (reproducible builds), L-705 73c3ad6 (release-candidate behaviour diff), L-801 8cad039 (LOAD-004 scenario matrix), L-455c 3c2ebbf (empty _source refused unless --allowMissingSource), L-802 64e7f27 (LOAD-005 memory checks scripts/load/memcheck.py; README memory formula x3 + 15 MB, lead review), L-803 03156fc (LOAD-006 kill/resume runs scripts/load/resume.py; all digests equal at 100k and 1M), L-802a 767d422 (allocator study scripts/load/allocbench.py; optional jemalloc/mimalloc features, default unchanged; recommendation for the owner in results/local/L-802a.md), L-804 (LOAD-007 fault injection scripts/load/faults.py; PASS at 1M, no exit 0 with loss; lead classifies the non-zero exits), L-804 56aa0c6, L-805 18c4334 (LOAD-008 compression levels scripts/load/compress.py; README compression table updated), L-806 92a2a69 (LOAD-009 splitting at scale scripts/load/split.py; PASS at 1M, no code change), L-807 e9d8cc5 (LOAD-010 concurrency curves scripts/load/curves.py; box baseline scripts/load/baselines/box-L1L-es717-plain-curves.json), L-808 27d3b7a (LOAD-011 Node comparison scripts/load/nodecmp.py at 100k/1M; digests equal; README Performance table re-measured; Rust restore/copy CPU about 2x M6, A/B-confirmed, unattributed, for the lead), L-808a 4a0dc41 (docs: TESTING-TODO row for networked compression performance at scale), L-808b (user decision: glibc + mimalloc default build and container image on dhi.io/static glibc, Alpine image kept as :<ver>-alpine, scripts/build-image.sh, gnu release targets)). **Next: L-809 (LOAD-014 many indices and many fields)**. D-5 (leave a killed run's hidden `.part` files in place and warn about them on the next run) is implemented in L-648. The cli_contract::tls_failures_have_one_clear_message flake is fixed in L-431a.
- Pending with the user: the L-645 behavior choices (credential precedence, auth-file values literal, world-readable warning) are being checked; keep them as they are unless told otherwise.
- The full queue is in plan/TASKS-LOCAL.md and handoff/HANDOFF-GROK-OPENCODE.md §8.
- plan/PROGRESS.md has one row per task and ends with a resume note. plan/progress-add.sh appends a row.

## Toolchain to reinstall (names checked by grepping the repo)
- Rust: Cargo.toml sets `rust-version = "1.85"` and there is no rust-toolchain file. The box used rustc 1.85.1. Use nightly for cargo-fuzz, with `--target x86_64-unknown-linux-gnu` (musl fails) and a C++ compiler (g++) installed. The Dockerfile builder is dhi.io/rust 1.99.0-alpine3.23-dev.
- Cargo tools: cargo-nextest (config in .config/nextest.toml), cargo-fuzz (nightly, fuzz/ and scripts/fuzz/smoke.sh), cargo-vet (supply-chain/), cargo-deny (deny.toml), cargo-geiger (scripts/sup/geiger-gate.sh), cargo-machete, cargo-udeps (nightly), cargo-llvm-cov (scripts/sup/coverage.sh).
- Other tools: gitleaks (scripts/sup/secret-scan.sh); grype and trivy for CVE scans (SECURITY-CVE.md); podman (+ docker-compose.yaml); Python 3 (scripts/it/*.py).
- Node elasticdump 6.125.1 for interop tests (scripts/node/bulk-diff.py defaults to `/tmp/ed/node_modules/elasticdump/bin/elasticdump`, or set `NODE_ELASTICDUMP`). It is installed at `/workspace/tools/node-ed` (kept across box restores) and `/tmp/ed` is a symlink to it. If either is missing: `npm i --prefix /workspace/tools/node-ed --ignore-scripts --no-audit --no-fund elasticdump@6.125.1 && ln -sfn /workspace/tools/node-ed /tmp/ed` (upstream code is untrusted: `--ignore-scripts`, and run it only in tests).
- With sudo approval: strace, perf, valgrind. On the last box apt's own downloads broke at the proxy; a helper that downloads .debs with curl and then runs apt install was used instead.

## Container images (scripts/it/images.lock, pinned by digest; up.sh adds @digest unless IT_UNPINNED=1)
- docker.elastic.co/elasticsearch/elasticsearch:7.10.2, 7.17.29, 8.19.23
- docker.io/bitnamilegacy/elasticsearch:7.10.2, 8.6.1-debian-11-r0, 9.1.2-debian-12-r0
- docker.io/opensearchproject/opensearch:1.3.20, 2, 3
- docker.io/library/alpine:3.24.3
- ghcr.io/shopify/toxiproxy:2.12.0
- Dockerfile build images: dhi.io/rust:1.99.0-alpine3.23-dev and dhi.io/alpine-base:3.24-alpine3.24-dev (both pinned by digest)

## Credentials (NOT included)
- You need a dhi.io registry login for the `-dev` Dockerfiles. Ask the user for the token through your secret-request mechanism, then run `podman login dhi.io --username "${DHI_USERNAME}" --password-stdin` with the token on stdin (`DHI_USERNAME=` and `DHI_TOKEN=` are set only in your shell, never written down). Keep the token only in podman's auth file: never in repo files, images, logs, or chat. **Never print DHI_TOKEN.** The plan docs name the env vars DHI_TOKEN and DHI_USERNAME for this pull-only login.
- Dockerfile build args: BUILDER_IMAGE, BUILDER_TAG, BASE_IMAGE, BASE_TAG (the registry variables), CARGO_BUILD_JOBS (default 2), VERSION, VCS_REF, BUILD_DATE.
- The IT scripts use IT_ADMIN_PW for the test cluster admin password. It is generated or set locally and is not a real secret.
- The user's local OpenCode setup reads XAI_API_KEY from the user's own shell. You never need it.

## Standing user rules
- Check weekly usage at each milestone. If it is over 90%, stop and message the user so they can take a handoff bundle.
- Don't touch other agents' containers or folders (for example /workspace/eti-opensearch).
- No AWS without approval.
- The junk files `oom` and `test_query_elasticdump_rs_test_857109_16.json` were removed in L-803 (user approved); never commit them again.
- Treat upstream code as untrusted.
- Sudo scope that was approved in the old account. **Re-confirm it with the user in the new account before using it:** cgroup memory-limit OOM tests; switching apt to HTTPS and installing strace/perf/valgrind; tc network fault injection on test containers only; tmpfs/loop mounts for disk-full tests.

## Work style
- Delegate long work to subagents.
- During long fuzz or test stretches, commit about every 20 minutes.
- Make one commit per task, titled `L-NNN: ...` (follow-ups to a done task: `L-NNNa: ...`), and update PROGRESS.md for each task. Then push `main` to the fork (`git push`, never `--force`), after the gates and `scripts/sup/secret-scan.sh` pass.

## Confirmed user decisions (handoff/DECISIONS.md; final, do not reopen)
- D-1: Keep refusing partial results on a red index by default. Add an opt-in `--allowPartialResults` that logs a warning.
- D-2: The compression default stays gzip; zstd only via `--compressionType zstd`. Check the docs/help and add a test if one is missing.
- D-3: The Dockerfiles default to the dhi.io Alpine `-dev` images, with a Debian build arg/target as the alternate. Rebuild and re-run the CVE scans.
- D-4: Add `--output-username`/`--output-password` and confirm parity for `--input-*`. Passwords must never appear in output. This one is lead-only.
- D-5: After an OOM kill, leave the hidden `.part` file in place. On the next run to the same output, print a warning naming it. Don't delete it.
- The same file also lists "keep as implemented" items.
- L-644a (2026-10-31): symlinked directories in the parent path of `--output` are followed; a symlink as the final output file stays refused (exit 2); a dangling parent link is a clear exit-2 error.

## User's local AI setup
The user runs Grok 4.7 locally through the OpenCode CLI. The handoff/ folder (opencode/, cards/, HANDOFF-GROK-OPENCODE.md) is for that setup. You and the local setup must never work on the same task. **Claim rule:** a task belongs to whoever already has a commit or a PROGRESS row for it.
