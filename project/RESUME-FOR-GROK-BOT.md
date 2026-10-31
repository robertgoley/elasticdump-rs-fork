# Resume elasticdump-rs (written for the Grok Bot picking this up)

## Project
A Rust rewrite of Node elasticdump, based on https://github.com/tenyears-tech/elasticdump-rs. Work happens only in the local fork, on branch `rewrite`. **NEVER push or open PRs.**

## Restore
    git clone repo/elasticdump-rs-full.bundle /workspace/elasticdump-rs
    git -C /workspace/elasticdump-rs checkout rewrite
    mkdir -p /workspace/elasticdump-rs-plan && cp -a plan/. /workspace/elasticdump-rs-plan/
The bundle holds every ref (main, rewrite, origin/*). It replaces the older h1/h2 bundles and the handoff r3 bundle, so those are left out.
When the export was made, HEAD on `rewrite` was `b4134af` and the working tree was clean. The only untracked items were the ignored junk files `oom` and `test_query_elasticdump_rs_test_857109_16.json`. They are left out of this export. If they show up again, leave them alone.

## Status
- Tasks are done through **L-642** (HEAD b4134af, "refuse same input and output file/index unless --force"). **L-643 is next** (card: handoff/cards/T-L643-regex-abuse.md).
- The full queue is in plan/TASKS-LOCAL.md and handoff/HANDOFF-GROK-OPENCODE.md §8.
- plan/PROGRESS.md has one row per task and ends with a resume note. plan/progress-add.sh appends a row.

## Toolchain to reinstall (names checked by grepping the repo)
- Rust: Cargo.toml sets `rust-version = "1.85"` and there is no rust-toolchain file. The box used rustc 1.85.1. Use nightly for cargo-fuzz. The Dockerfile builder is dhi.io/rust 1.99.0-alpine3.23-dev.
- Cargo tools: cargo-nextest (config in .config/nextest.toml), cargo-fuzz (nightly, fuzz/ and scripts/fuzz/smoke.sh), cargo-vet (supply-chain/), cargo-deny (deny.toml), cargo-geiger (scripts/sup/geiger-gate.sh), cargo-machete, cargo-udeps (nightly), cargo-llvm-cov (scripts/sup/coverage.sh).
- Other tools: gitleaks (scripts/sup/secret-scan.sh); grype and trivy for CVE scans (SECURITY-CVE.md); podman (+ docker-compose.yaml); Python 3 (scripts/it/*.py).
- Node elasticdump for interop tests. It used to be installed at `/tmp/ed` (for example `npm i --prefix /tmp/ed elasticdump`; see scripts/node).
- With sudo approval: strace, perf, valgrind.

## Container images (scripts/it/images.lock, pinned by digest; up.sh adds @digest unless IT_UNPINNED=1)
- docker.elastic.co/elasticsearch/elasticsearch:7.10.2, 7.17.29, 8.19.23
- docker.io/bitnamilegacy/elasticsearch:7.10.2, 8.6.1-debian-11-r0, 9.1.2-debian-12-r0
- docker.io/opensearchproject/opensearch:1.3.20, 2, 3
- docker.io/library/alpine:3.24.3
- ghcr.io/shopify/toxiproxy:2.12.0
- Dockerfile build images: dhi.io/rust:1.99.0-alpine3.23-dev and dhi.io/alpine-base:3.24-alpine3.24-dev (both pinned by digest)

## Credentials (NOT included)
- You need a dhi.io registry login for the `-dev` Dockerfiles. Ask the user for the token through your secret-request mechanism, then run `podman login dhi.io --username <user> --password-stdin`. Keep the token only in podman's auth file: never in repo files, images, logs, or chat. **Never print DHI_TOKEN.** The plan docs name the env vars DHI_TOKEN and DHI_USERNAME for this pull-only login.
- Dockerfile build args: BUILDER_IMAGE, BUILDER_TAG, BASE_IMAGE, BASE_TAG (the registry variables), CARGO_BUILD_JOBS (default 2), VERSION, VCS_REF, BUILD_DATE.
- The IT scripts use IT_ADMIN_PW for the test cluster admin password. It is generated or set locally and is not a real secret.
- The user's local OpenCode setup reads XAI_API_KEY from the user's own shell. You never need it.

## Standing user rules
- Check weekly usage at each milestone. If it is over 90%, stop and message the user so they can take a handoff bundle.
- Don't touch other agents' containers or folders (for example /workspace/eti-opensearch).
- No AWS without approval.
- Leave the junk files `oom` and `test_query_elasticdump_rs_test_857109_16.json` alone.
- Treat upstream code as untrusted.
- Sudo scope that was approved in the old account. **Re-confirm it with the user in the new account before using it:** cgroup memory-limit OOM tests; switching apt to HTTPS and installing strace/perf/valgrind; tc network fault injection on test containers only; tmpfs/loop mounts for disk-full tests.

## Work style
- Delegate long work to subagents.
- During long fuzz or test stretches, commit about every 20 minutes.
- Make one commit per task, titled `L-NNN: ...`, and update PROGRESS.md for each task.

## Confirmed user decisions (handoff/DECISIONS.md; final, do not reopen)
- D-1: Keep refusing partial results on a red index by default. Add an opt-in `--allowPartialResults` that logs a warning.
- D-2: The compression default stays gzip; zstd only via `--compressionType zstd`. Check the docs/help and add a test if one is missing.
- D-3: The Dockerfiles default to the dhi.io Alpine `-dev` images, with a Debian build arg/target as the alternate. Rebuild and re-run the CVE scans.
- D-4: Add `--output-username`/`--output-password` and confirm parity for `--input-*`. Passwords must never appear in output. This one is lead-only.
- D-5: After an OOM kill, leave the hidden `.part` file in place. On the next run to the same output, print a warning naming it. Don't delete it.
- The same file also lists "keep as implemented" items.

## User's local AI setup
The user runs Grok 4.7 locally through the OpenCode CLI. The handoff/ folder (opencode/, cards/, HANDOFF-GROK-OPENCODE.md) is for that setup. You and the local setup must never work on the same task. **Claim rule:** a task belongs to whoever already has a commit or a PROGRESS row for it.
