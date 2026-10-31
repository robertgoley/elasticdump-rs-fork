# elasticdump-rs: handoff for Grok 4.7 via the OpenCode CLI (single model)

Base: bundle `elasticdump-rs-b4134af.bundle`, branch `rewrite` at `b4134af` (L-642). Revised 2026-10-31.
The whole local setup is **one model, Grok 4.7, driven through OpenCode** (`xai` provider). There is no local model
server. Work is split across four OpenCode **agents** on the same model, so that writing, reviewing and deciding happen
in separate sessions with separate permissions:

- `impl`: bounded code/test cards in a worktree, edits limited to the card's `files_allowed`.
- `runner`: campaign lane. Scripts run the commands, and the agent only summarises logs into `results/`.
- `review`: read-only, fresh session, hostile review of `git diff rewrite...<branch>`.
- `lead`: the interactive session you supervise. It handles security-sensitive code, design decisions, merges and bundle cuts.

**Key rule: the session that wrote a change never reviews it.** `review` always runs as a new `opencode run`.

Source of truth for what's done: `plan/PROGRESS.md` (bundle snapshot) and the commit subjects on `rewrite`.
Read `DECISIONS.md` first. Its confirmed decisions are folded into §10.

| Path | What |
|---|---|
| `HANDOFF-GROK-OPENCODE.md` | this file |
| `DECISIONS.md` | Robert's confirmed decisions and work items D-1..D-5 (2026-10-30) |
| `opencode/opencode.json` | provider `xai`, model `xai/grok-4.7`, agents `impl`, `runner`, `review`, `lead`; deny-by-default permissions; **no API key** |
| `opencode/AGENTS.md` | repo rules for the agents (copied into each worktree by run-card.sh, excluded from git) |
| `opencode/prompts/{impl,runner,review,lead}.md` | system prompts of the four agents |
| `opencode/scripts/run-card.sh` | card → worktree → (campaign) → `opencode run` → gate |
| `opencode/scripts/gate.sh` | merge gate (policy, scan, fmt, clippy, nextest, acceptance, deny, geiger, gitleaks, clean tree) |
| `opencode/scripts/card.py` | card parser and checker (`card.py check <card>`) |
| `opencode/scripts/smoke-grok.sh` | OpenCode install, xai model listed, one tiny `opencode run` in a scratch dir |
| `cards/` | `CARD-TEMPLATE.md`, `G-00` (lead, first) and ready cards `T-B0 T-B1 T-B2a T-B7 T-L643 T-L651 T-L652 T-L657` |
| `plan/` | plan snapshot at the bundle (PROGRESS, TASKS-LOCAL, HANDOFF-TASKPLAN, TEST-REQUIREMENTS, PLAN, TYPES-PLAN, GAPS) |

---

## 1. Bootstrap

```bash
cd ~/handoff && sha256sum -c SHA256SUMS
git clone --branch rewrite elasticdump-rs-b4134af.bundle ~/src/elasticdump-rs
cd ~/src/elasticdump-rs && git log --oneline -1                 # b4134af L-642 ...
git remote set-url --push origin no-push-allowed                 # origin is the bundle; never push
git config core.hooksPath /dev/null                              # no repo hooks run (code is untrusted)
```

Toolchain (versions from PROGRESS/TASKS-LOCAL §1; check each):

```bash
rustup toolchain install 1.99 nightly --profile minimal -c rustfmt,clippy
rustc +1.99 -V; cargo +nightly -V
cargo install --locked cargo-nextest cargo-deny cargo-vet cargo-machete cargo-fuzz cargo-geiger cargo-udeps   # whatever is missing
gitleaks version; actionlint -version; zizmor --version; jq --version; zstd --version; python3 -V
podman info --format '{{.Host.CgroupsVersion}}'                  # v2
cat /sys/fs/cgroup/user.slice/user-$(id -u).slice/cgroup.controllers   # must list memory (delegation)
podman run --rm --memory 64m docker.io/library/alpine:3 cat /sys/fs/cgroup/memory.max   # 67108864
podman login dhi.io                                              # the user's own credentials (not included)
```

Images come from `images.lock` in the repo. `scripts/it/up.sh` pulls them on first use.

OpenCode and Grok:

```bash
opencode --version                                    # see §12 for 1.x vs 2.x
opencode auth login                                   # pick xAI: SuperGrok OAuth or paste an API key (stored in ~/.local/share/opencode/auth.json)
#   or: export XAI_API_KEY=...  (in your shell or a secrets manager only; see §12 item 2)
opencode auth list                                    # xAI must be listed
opencode models xai                                   # confirm the exact Grok 4.7 id (§12 item 1)
OPENCODE_CONFIG=~/handoff/opencode/opencode.json opencode debug config | jq '.provider, (.agent | keys)'
~/handoff/opencode/scripts/smoke-grok.sh              # exit 4 = opencode missing or model id not listed
```

**The API key never goes in** `opencode.json`, the repo, cards, feedback files, logs, `results/`, or bundles.
gate.sh's secret scan (`scripts/sup/secret-scan.sh`) runs on every branch. Before a bundle cut, also run
`gitleaks detect` on `~/handoff-out`.

Finally, do **G-00** in the lead session (`cards/G-00-test-scaffold.md`). Every impl code card depends on it.

---

## 2. Roles and split rules

| Agent | Session | Can edit | Bash | Does |
|---|---|---|---|---|
| `impl` | `opencode run` per card, in a worktree | only `files_allowed` + `results/cards/<id>.md` (injected per card) | cargo build/test/fmt/clippy, read-only git, `git add/commit`; **no podman, no `scripts/it/*`, no push/branch/rebase** | bounded code and test cards |
| `runner` | `opencode run` per campaign card | `results/` only | log readers, jq, `git add results/`, commit | summarise campaign logs that run-card.sh produced |
| `review` | **fresh** `opencode run`, never the writer's session | nothing | `git diff/log/show`, read-only tools, cargo test/nextest/clippy | §7 checklist on `git diff rewrite...card/<id>` → verdict + feedback |
| `lead` | interactive `opencode --agent lead`, which you supervise | with approval (`ask`) | most commands `ask`; push and remotes denied | security code, design, merges, bundle cuts, owner questions |

**impl** gets single-file or single-crate changes where the card names the files, the requirement row and the expected
behaviour, plus new tests from a requirement ID (each in its own `tests/chaos_local_<id>.rs`), mechanical refactors with a
compiler-checked outcome, doc tables and dataset converters in `scripts/data/`.

**runner** gets long campaigns (fuzz, load, IT matrix, datasets). run-card.sh runs the `campaign:` lines, and the agent only
writes `result.json`/`SUMMARY.md`/`results/cards/<id>.md`.

**lead only** (security-sensitive or judgment calls; impl may write a **test-only** card against these and mark the gap
with `// FINDING:`, but may not change the code):
- the bulk/NDJSON encoder (`src/bulk.rs`, `src/pipeline/*` encode path, `extract.rs`)
- injection guards INJ-F1..F4: index/id/URL segment validation, `url_segments`, rename output validation
- URL and credential redaction, `--httpAuthFile`/ini parsing, `secrets` canaries (and D-4)
- TLS (`--*-ca/cert/key/pass`, `encrypted-keys`), proxies
- retry, idempotency and backoff (`src/http/retry.rs`), timeouts, Retry-After
- `unsafe`/FFI (zstd, sonic-rs), `Cargo.toml`/`Cargo.lock`, `deny.toml`, `supply-chain/`, `.github/`, `.config/`
- any change to `tests/golden/` or an existing assertion
- cross-cutting design, anything that picks between the owner's open options (§10), every merge and every bundle cut

**Tie-breaks:**
- A card that would need a lead-only file becomes a test-only card. impl reports `// FINDING:`, and lead does the fix.
- If a fix touches more than 2 source files, or the spec doesn't fit in about 15 lines, it is a lead task.
- **Never let the same session that wrote a change review it.** Lead's own changes also go through a fresh `review` run,
  and you read that verdict before merging.

---

## 3. Context tuning for Grok

Grok 4.7 has a large context, so the old tight-context rules (line-range-only reads, per-card token budgets) are
relaxed. `files_readonly` entries may still use `path:start-end` as a *pointer* to the relevant part, and the agent may read
whole files. Things that still matter, for quality and for cost:

- **Bounded cards:** one requirement per card, with a numbered Spec of 3–6 testable steps. Quote the requirement row in
  Context. Don't paste whole plan files into the prompt (the agent can read them if it really needs to).
- **Explicit scope:** `files_allowed` is enforced twice (OpenCode edit permission injected by run-card.sh, then gate.sh
  policy). Say what **not** to fix ("report, don't change extract.rs").
- **Explicit acceptance:** use the exact commands the gate runs, with exit codes. impl runs them before committing.
- **Caps:** `steps` caps the tool calls (impl 150, runner 80, review 80). The shell `timeout` caps wall time (`timeout_min`).
- **Retries:** the feedback file holds the review verdict lines, the failing gate lines and a short log excerpt (not the
  whole gate.log). At most 2 retries, and then the card goes to lead.

---

## 4. Task card format

Use `cards/CARD-TEMPLATE.md`, and check every new card with `opencode/scripts/card.py check <card>`.
Front matter (restricted YAML: scalars, `[a, b]` lists, `- item` lists and `- run/exit/expect` maps):

| Field | Meaning |
|---|---|
| `id` | `T-<plan-id>` (impl/runner) or `G-<nn>` (lead); also the commit prefix and the branch `card/<id>` |
| `plan_task` | L-6xx / Bn / D-n. run-card refuses a card whose L-task already has a PROGRESS row or an `L-NNN:` commit on `rewrite` (exit 3) |
| `lane` | `code` or `campaign` (campaign: run-card runs `campaign:` first and logs to `target/cards/<id>/`) |
| `agent` | `impl`, `runner` or `lead` (run-card.sh refuses `lead` cards, which are done interactively; exit 5) |
| `requirements` | TST-… IDs (go into the commit subject) |
| `goal` | one sentence with an observable outcome |
| `depends_on` | card or plan IDs that need a commit `<ID>: …` on `rewrite` (missing → exit 4) |
| `timeout_min`, `campaign_timeout_min`, `heavy_gb` | agent wall time; campaign wall time; >0 → run under `scripts/res/heavy.sh <GB>` |
| `files_allowed` | globs impl may edit |
| `files_readonly` | `path` or `path:start-end` pointers |
| `forbidden` | card-specific don'ts (AGENTS.md has the global ones) |
| `campaign` | shell lines run by run-card.sh with `bash -o pipefail`, env `ART=target/cards/<id>` |
| `acceptance` | `- run:` / `exit:` / `expect:`; the gate reruns these |
| `summary` | `results/cards/<id>.md` |

**Output contract:** branch `card/<id>` with exactly **one** commit `<id>: <req IDs>: <summary>`, including
`results/cards/<id>.md` with the sections Result (pass|fail|partial|blocked), Changes, Commands (each with exit code
and PASS/FAIL) and Findings. No branch, push, amend, rebase or stash.

---

## 5. Workflow loop

```text
write/choose card (lead) ──► run-card.sh card ──► worktree ~/src/elasticdump-rs-wt/<id> on card/<id>
        ▲                       │  (campaign lane: script runs commands, logs in target/cards/<id>/)
        │                       ▼
        │          opencode run --agent impl|runner --format json ──► events.jsonl, session.json, tokens.json
        │                       ▼
        │                 gate.sh --card ──► gate.txt (PASS/FAIL per step)
        │                       ▼
        │          NEW session: opencode run --agent review  (git diff rewrite...card/<id>, §7)
        │                       ▼
        │          lead reads gate.txt + review verdict ── ok → merge (below), progress log (§9)
        └─ reject ── feedback.md → run-card.sh card --retry N --feedback feedback.md (N ≤ 2)
                     after 2 rejected retries: lead finishes it on card/<id>-rejected-2 or rewrites it
```

```bash
export REPO=~/src/elasticdump-rs PAR_MAX=2 OC_MODEL=xai/grok-4.7
H=~/handoff/opencode/scripts
$H/run-card.sh ~/handoff/cards/T-L643-regex-abuse.md --dry-run     # checks only
$H/run-card.sh ~/handoff/cards/T-L643-regex-abuse.md; echo $?      # 0 pass · 3 done · 4 dep missing · 5 lead card · 10 gate fail · 11 no commit · 12 campaign fail · 124 timeout
cat ~/src/elasticdump-rs-wt/T-L643/target/cards/T-L643/gate.txt
( cd ~/src/elasticdump-rs-wt/T-L643 && OPENCODE_CONFIG=~/handoff/opencode/opencode.json \
  OPENCODE_DB=/tmp/review-T-L643.db timeout 30m opencode run --agent review --title review-T-L643 \
  "Review card/T-L643: git diff rewrite...card/T-L643 against ~/handoff/cards/T-L643-regex-abuse.md, checklist §7 of ~/handoff/HANDOFF-GROK-OPENCODE.md" ) | tee /tmp/review-T-L643.txt
```

Merge (lead session or by hand, in the main clone; fast-forward keeps one commit per card):

```bash
cd ~/src/elasticdump-rs && git switch rewrite
git merge --ff-only card/T-L643 || {                       # rewrite moved on: rebase in the worktree, re-gate, re-review
  ( cd ../elasticdump-rs-wt/T-L643 && git rebase rewrite &&
    ~/handoff/opencode/scripts/gate.sh --card ~/handoff/cards/T-L643-regex-abuse.md ) &&
  git merge --ff-only card/T-L643; }
git worktree remove ../elasticdump-rs-wt/T-L643 && git branch -d card/T-L643
```

If lead amends an impl commit, add `Reviewed-and-amended-by: lead` to the message, say what changed in the progress
log, and run a fresh `review` on the amended branch.

The commands the gate runs (from the repo and the A-agent's gate) are listed in the `gate.sh` header:
`cargo fmt --all -- --check`, `cargo build -q`,
`cargo clippy -q --workspace --lib --bins --tests --features elasticdump-rs/load,elasticdump-rs/it -- -D warnings`,
`cargo nextest run --profile unit|contract[|chaos]`, the card acceptance,
`cargo deny check advisories bans licenses sources`, `scripts/sup/geiger-gate.sh`,
`scripts/sup/secret-scan.sh rewrite..HEAD`, `scripts/check-clean-tree.sh` and `scripts/lint-hygiene.sh`.

**Before each bundle cut** (and weekly), also run: `cargo vet --locked`, `cargo machete`,
`cargo +nightly udeps --workspace --all-targets --features elasticdump-rs/load`, `scripts/lint-workflows.sh`,
`scripts/fuzz/smoke.sh` and `scripts/sup/offline-build.sh`.

---

## 6. Parallelism, cost and resource limits

The limits are now **API cost and rate limits** plus **local RAM for builds and containers**. There is no GPU and no
local model.

- `PAR_MAX` (default **2**) is the number of headless `opencode run` sessions at once (flock slots in run-card.sh). Raise
  it only if xAI rate limits allow it and `free -g` stays above 8 GB available during a gate. If you hit 429s, lower it.
- **Memory budget:** about 4 GB per concurrent cargo build/test, **plus** each engine container (ES/OS: 2–6 GB heap +
  overhead), **plus** 4 GB for the OS and the OpenCode processes.
- **CPU:** `CARGO_BUILD_JOBS=4` per worktree (run-card exports it). Each worktree has its own `target/`, about 3–6 GB of
  disk. Delete merged worktrees.
- **Container-heavy work is serialised:**
  - `scripts/it/run.sh` already takes `flock /tmp/elasticdump-rs-it.lock` (guards.sh).
  - Campaign cards with `heavy_gb > 0` run under `scripts/res/heavy.sh <GB>`. It takes `flock /tmp/edrs-heavy.lock`,
    exits 75 if any `it-*` container is running, and memguards the run.
  - **One engine at a time** unless `free -g` shows room for two and B9/B4 explicitly needs it.
- **Code cards never start containers** (AGENTS.md). impl's bash permission denies `podman` and `scripts/it/*`.
- **Disk:** `ED_DATA_CACHE` (default /var/tmp/elasticdump-rs-data) and `LOAD_WORK` (/var/tmp/elasticdump-rs-load).
  B1 needs about 150 GB and L2 needs 100 GB+. Check `df -h` before campaigns.
- **Fuzz:** `FUZZ_JOBS` = cores/4 alongside other work, or cores-2 when fuzz is the only job. `-rss_limit_mb=4096` per job.
- Don't run two cards that edit the same file at the same time. `src/lib.rs` (`validate`) is shared by T-L643 and
  several A6 follow-ups, so run those **one after another**.

**Cost control:**
- Per-card caps: `steps` in opencode.json and `timeout_min` in the card. Keep them tight, and split a card rather than
  raising them.
- Prefer the **campaign lane** for long work: the script runs the hours of fuzz/load/IT, and the model only reads the logs.
- Keep cards bounded so a retry is cheap. Stop after 2 rejected retries (lead takes over).
- run-card writes `tokens.json` per run (from `step_finish` events). Glance at it weekly, and check spend on the xAI console.
- `review` is cheaper than a broken merge. Don't skip it, but keep the prompt to the diff plus the card.

---

## 7. Review checklist (hostile-code mindset; used by the `review` agent and by lead before merging)

Treat each diff like an outside PR, and review it in a session that didn't write it. Reject it if any answer is wrong:

1. Does it touch only `files_allowed`? (gate policy) Does it touch none of the lead-only paths (§2) or `oom` /
   `test_query_elasticdump_rs_test_857109_16.json`?
2. Does it add no network endpoint, socket, `Command`, env var read, telemetry or file write outside temp dirs and
   `results/`? Check the gate `WARN` lines by hand.
3. Does it add no dependency, `unsafe`, `#[allow]`, `#[ignore]` or feature change? (A dependency only through lead,
   following docs/SUPPLY-CHAIN.md and docs/DEPENDENCY-POLICY.md, with `cargo deny`/`vet` and the SBOM step.)
4. Is no test weakened? Look for removed asserts, loosened `contains`, bigger timeouts, `retries`, `|| true`,
   `let _ =` on results, and tests that pass whatever the outcome.
5. Are goldens unchanged? A golden edit needs a reason tied to a requirement row and lead sign-off.
6. Is nothing secret logged? Look for `Authorization`, passwords, `user:pw@` URLs or key material in `log::`, `eprintln!`
   or error strings. Redaction helpers must be used.
7. Does the behaviour match the requirement row and the decisions in §10 (exit codes 0/1/2/3/130, refusal wording,
   no silent precedence)?
8. Does every new usage error or bound appear in the summary's Findings, so lead can judge it as a Node deviation?
   Do NODE-DIFFERENCES.md and the docs reflect it?
9. Are the results honest? Read `target/cards/<id>/campaign.log` against the claimed numbers. The agent copies numbers;
   it mustn't invent them.
10. Is it one commit, with the subject format, and with the summary file present?

---

## 8. Ordered work queue

**State:**
- The bundle has everything through **L-642** (HEAD `b4134af`), plus L-901/H1 and L-901/H2. See `plan/PROGRESS.md`.
- The original agent has **stopped**. This local setup owns everything from **L-643** on: the rest of A6, A7
  (L-701..705), A8 (L-801..817), the B tasks, and D-1..D-5 from DECISIONS.md.
- run-card.sh still refuses a card whose L-task already has a PROGRESS row or an `L-NNN:` commit (exit 3).

Legend (runner = campaign card; summary reviewed like impl): **impl** = impl card (a fresh review agent reviews it), **impl/runner → lead** = impl or runner writes tests or
collects results, then lead fixes or decides, **lead** = lead session, **O** = owner decision first.

### Now (no dependencies beyond the bundle)

| # | Item | Req | Who | Why | Card / deps |
|---|---|---|---|---|---|
| 1 | G-00 test scaffold | TST-INF-001 | lead | touches `.config/nextest.toml` (lead path); unblocks parallel cards | `cards/G-00` |
| 2 | B0 environment | — | runner | pure fact collection | `cards/T-B0` |
| 3 | B1 datasets | DATA-004/005/010 | runner → lead | long downloads + conversions; lead pins hashes in datasets.toml | `cards/T-B1`; B0 |
| 4 | B7 long fuzz | API-084, INJ-015, SUP-020/022 | runner → lead | campaign; crash fixes are lead's (parsers on the security list) | `cards/T-B7`; H2 ✓ |
| 5 | B2a L1 smoke cells | LOAD-004/005/017 | runner | campaign on existing scenarios | `cards/T-B2a`; B0 |

### Owner decisions (DECISIONS.md D-1..D-5)

| Item | Who | Why |
|---|---|---|
| D-1 `--allowPartialResults` opt-in for red indices (default still refuses; warn with the failed-shard count) | impl (review agent reviews) | small flag + tests for both paths; lead checks the warning wording |
| D-2 gzip stays the default, zstd only via `--compressionType zstd` | impl (review agent reviews) | docs/help/defaults check + test if missing |
| D-3 Dockerfiles default to dhi.io Alpine `-dev` images, Debian as an alternate build arg/target, re-run the CVE scan | impl/runner → lead | Dockerfile edit by impl; the CVE scan campaign by runner; lead signs off on SECURITY-CVE.md |
| D-4 `--output-username`/`--output-password` (+ input parity), never logged, flags override URL credentials | **lead only** | credential handling (security list); add redaction/fuzz tests |
| D-5 leave the hidden `.part` after an OOM kill, warn on the next run naming it, never delete it | impl (review agent reviews) | file-write semantics are close to L-644, so lead reviews closely |

### A6 tail (L-638..L-642 are done in the bundle)

| Item | Req | Who | Why |
|---|---|---|---|
| L-643 | BRK-CLI-004 | impl/runner → lead | regex errors; lead checks INJ-F2 on rename output — `cards/T-L643` |
| L-644 | BRK-CLI-005 | lead | output path / symlink / hard-link commit semantics (file writes) |
| L-645 | BRK-CLI-006 | lead | credential precedence (secrets); do it with D-4 |
| L-646 | BRK-CLI-008 | impl/runner → lead | scheme-less URL warnings; the parser is URL handling, so review closely |
| L-647 | BRK-SIG-002 | lead | signal handling in restore/copy, in-flight bulks |
| L-648 | BRK-SIG-003 | impl/runner → lead | kill -9 ×50 campaign (runner); findings to lead |
| L-649 | BRK-SIG-004 | lead | forced-exit path |
| L-650 | BRK-SIG-005 | lead | resume semantics; touches owner Q4 (resume) |
| L-651 | BRK-ILM-001 | impl | no partial policy set — `cards/T-L651` |
| L-652 | BRK-ILM-002 | impl | converter unit tests — `cards/T-L652`; after T-L651 |
| L-653 | BRK-ILM-003 | impl | refusal tests for managed policies / ISM→ILM; IT on one engine (runner for the IT part) |
| L-654 | BRK-XV-001 | lead | OS compatibility override detection (decided: OS 1.0 feature set) |
| L-655 | BRK-XV-004 | impl/runner → lead | field-type refusal table on 2 engines; wording decided by lead |
| L-656 | BRK-XV-005 | runner | capability errors per engine (IT campaign) |
| L-657 | BRK-XV-006 | impl/runner → lead | test only; `_type` behaviour reported — `cards/T-L657` |

### A7 runtime supply chain (gate H4)

| Item | Req | Who | Why |
|---|---|---|---|
| L-701 | SUP-001 | lead | egress-denied runner: network sandbox design |
| L-702 | SUP-002 | impl/runner → lead | strace collection campaign is mechanical; the allow-list is lead's |
| L-703 | SUP-003 | lead | seccomp/Landlock profile (security boundary) |
| L-704 | SUP-010 | impl/runner → lead | two builds in different dirs/users, compare hashes; lead explains diffs |
| L-705 | SUP-016 | runner | RC behaviour-diff artefact (scripted) |
| H4 | gate | lead | cut bundle #4 (§9) once L-701..704 merge → unblocks B11, B12 |

### A8 loads (L0 100k, L1-lite 1M; disk ≤ 12 GB) → Final gate

| Item | Req | Who | Why |
|---|---|---|---|
| L-801 | LOAD-004 | runner | scenario matrix cells via `scripts/load/run.sh L0/L1-lite`; new scenario TOMLs are mechanical |
| L-802 | LOAD-005 | impl/runner → lead | flat-memory runs; lead updates the README memory formula if wrong |
| L-803 | LOAD-006 | impl/runner → lead | kill/resume runs; the documented recipe is owner Q4-dependent |
| L-804 | LOAD-007 | impl/runner → lead | fault injection under load; failures classified by lead |
| L-805 | LOAD-008 | runner | compression levels table |
| L-806 | LOAD-009 | runner | splitting at scale |
| L-807 | LOAD-010 | runner | concurrency curves (noisy here; B10 is authoritative) |
| L-808 | LOAD-011 | runner | Node comparison at 100k/1M (Node in /tmp/ed) |
| L-809 | LOAD-014 | runner | 300 indices campaign |
| L-810 | LOAD-015 | impl/runner → lead | large docs; 413/limit wording is lead's |
| L-811 | LOAD-017 | impl | leak check in teardown of each run (script change in scripts/load) |
| L-812 | LOAD-018 | runner | data streams 1M |
| L-813 | LOAD-020 | lead | trend store / regression gate design (CI) |
| L-814 | DATA-002 | impl | edgen mapping/settings per engine (single crate tools/edgen) |
| L-815 | DATA-003 | impl/runner → lead | edge catalogue `--profile edge` (tools/edgen/src/edge.rs exists; check first); expectations reviewed by lead |
| L-816 | DATA-007 | impl | OSB generate-data cross-check |
| L-817 | DATA-008 | impl | corpus generator with `--seed` |
| Final | gate | lead | bundle after A8 → B2 (full), B10 |

### B tasks (HANDOFF-TASKPLAN §8; gates H0–H3 are already met by this bundle)

| Item | Req | Who | Depends | Why |
|---|---|---|---|---|
| B0 | — | runner | H0 ✓ | `cards/T-B0` |
| B1 | DATA-004/005/010 | runner → lead | H0/H1 ✓ | `cards/T-B1`; lead pins datasets.toml |
| B2 | LOAD-004…012, 014 | runner (cells) / lead (matrix design) | H1 ✓, A8 preferred | start with `cards/T-B2a`, then one card per scenario × engine |
| B3 | LOAD-005/008/009 at L2 | runner | B2 | big-host campaign; big5-100 / full http_logs need lead to add manifest entries |
| B4 | LOAD-013 | runner | B2, H3 ✓ | two clusters at once: only if memory allows |
| B5 | LOAD-016, DATA-009 | runner | B3 | 6–12 h soak; `SOAK_RUNNER` var (owner item) |
| B6 | LOAD-019, DATA-006 | impl/runner → lead | B1 | Rally/OSB baselines need owner Q8 (Rally/OSB) answered |
| B7 | API-084, INJ-015, SUP-020/022 | runner → lead | H2 ✓ | `cards/T-B7` |
| B8 | BRK-NET-009, BRK-SRV-012 | lead (design) → impl | H1 ✓ | multi-node compose and scenarios are new harness code |
| B9 | API-070, API-072 | runner | H3 ✓ | `scripts/it/xmatrix.sh all`, `xpairs.sh`, serialised |
| B10 | LOAD-010/011 | runner | B2 | dedicated cores, 3 runs each |
| B11 | SUP-010 | impl/runner → lead | H4 | second machine/VM needed |
| B12 | SUP-002/003 | lead | H4, B2 | root eBPF: optional, ask the owner |
| B13 | GAPS §3/§4 | O | L-A04/A05 + owner approval | **blocked**: no AWS without owner approval and a named sandbox account |

### A10 feature backlog (decision-gated: owner first, then lead designs, impl implements slices)

| Item | Who | Note |
|---|---|---|
| L-A01 INJ-013 `--allow-index`/`--dry-run` | O → lead | owner Q13 |
| L-A02 transforms | O → lead | scope (owner Q7) |
| L-A03 CSV | O → impl | mechanical once in scope |
| L-A04 S3 (MinIO), L-A05 SigV4/AOSS | O → lead | credentials/signing are lead's; real AWS = B13 |
| L-A06 SOCKS5 | O → lead | network |
| L-A07 `--delete` | O → lead | destructive |
| L-A08 small deviations | O → impl | per-item cards |
| L-A09 multielasticdump | O → lead, then impl | design first |
| L-A10 ruzstd backend | O → lead | owner Q14 (unsafe/ruzstd) |
| L-A11 `--fileSize` UTF-16 mode | O → impl | only if wanted |

L-902 (merge B results back) is lead's (§9).

---

## 9. Reporting back and bundle cuts

**Progress log:** append one row per merged card or task to `results/cards/PROGRESS-LOCAL.md` (in the repo, committed
with the merge). PROGRESS.md is the bundle snapshot from the original agent; leave it as is.

```text
| ID | plan task | status | commit | tests / results | findings / notes | by |
| T-L643 | L-643 | done | <sha> | chaos_local_l643 42/42; contract 71/71; gate PASS | 6 new usage errors (list) | impl, reviewed by review agent, merged by lead |
```

`status` is one of done, partial, blocked (reason) or rejected→lead. Add a `resume:` line at the bottom saying what runs
next. Campaign results follow HANDOFF-TASKPLAN §9 (`edrs-result/1`, `results/<Bn>/…`).

**Cut a bundle for the owner when:** a gate is reached (H4, Final); every 10 merged cards; or before any
owner question. Use HANDOFF-TASKPLAN §3 with `BR=rewrite`:

```bash
cd ~/src/elasticdump-rs && test -z "$(git status --short)"
GATE=local-$(date +%Y%m%d-%H%M); OUT=~/handoff-out; mkdir -p "$OUT"
git rev-parse rewrite > "$OUT/handoff-$GATE.BASE-SHA"
git bundle create "$OUT/elasticdump-rs-$GATE.bundle" b4134af..rewrite   # incremental; the owner fetches it on top
git bundle verify "$OUT/elasticdump-rs-$GATE.bundle"
( cd "$OUT" && sha256sum elasticdump-rs-$GATE.bundle handoff-$GATE.BASE-SHA > handoff-$GATE.sha256 )
```

Include `results/cards/PROGRESS-LOCAL.md` and the open owner questions in the message to the owner. Don't merge
the owner's newer bundle blindly: fetch it to `refs/remotes/owner/rewrite`, and then rebase or merge after a gate run.

---

## 10. Decisions already made (from PROGRESS.md; don't reopen)

| Topic | Decision |
|---|---|
| Duplicate keys | in metadata are rejected (Q13); inside `_source` they're forwarded verbatim |
| Failed items | by default the tool continues and exits 3 (Q3); `--ignore-errors` gives exit 0 |
| Auto-id batches | `RejectionOnly` retry (Q6) |
| Redirects | never followed |
| DNS "no such host" | permanent (no retry) |
| Timeouts | `--timeout` is an idle bound; `--idleTimeout` defaults to 300 s; `--requestTimeout` is total |
| Retry-After | honoured, capped at 30 s |
| `allow_partial_search_results` | never set |
| Limits | nesting > 1000 rejected; response cap 256 MiB |
| Encodings | invalid UTF-8 rejected; lone surrogates → U+FFFD |
| `_id` | numeric `_id` stringified as Node does; non-finite refused |
| Engines | OpenSearch compatibility override → treated as the OS 1.0 feature set |
| Inputs | staging files refused as input; a CA file that also holds a key is accepted with a warning |
| Build | http2 removed; `encrypted-keys` on by default |
| Signals | SIGINT/SIGTERM exit 130/143 |
| Totals | scroll/PIT total mismatch is an error |
| Slices | `--slices` above max → exit 1 with the server reason, nothing written (L-405) |
| ILM | a phase-less policy is refused, even in lenient mode (L-507) |

**Confirmed by Robert on 2026-10-30 (DECISIONS.md; final, keep as implemented):**

| Topic | Decision |
|---|---|
| `--offset` + `--slices` | usage error (sliced results have no stable order) |
| Compression flags | `--compressionType` overrides `--fsCompress`; default stays gzip (Node parity), zstd only on request (D-2) |
| Same source/destination | the check does not detect aliases or alternate hostnames (accepted limitation) |
| Deleted index mid-restore | checked only on restores longer than 5 s (L-619) |
| Restore input | rejects invalid UTF-8, nesting > 1000 and duplicate metadata keys (default kept) |
| Junk files | `oom` and `test_query_elasticdump_rs_test_857109_16.json` stay in the tree |
| zstd / sonic-rs | zstd decoding stays on libzstd; sonic-rs stays pinned `=0.5.10` with sanitize |
| Red index | refuse partial results by default; opt-in flag (D-1) |
| Credential flags | `--output-username/--output-password` + input parity, override URL credentials, never logged (D-4, lead) |
| `.part` after OOM | leave it; warn naming it on the next run; never delete (D-5) |
| Images | Dockerfiles default to dhi.io Alpine `-dev`, Debian as the alternate (D-3) |

**Owner items listed in PROGRESS** (don't decide them; carry them in reports):
- the CODEOWNERS `@OWNER` placeholder, branch protection and the `sbom-reviewed` label
- `cargo vet` trust decisions
- backoff has no jitter
- scroll is cleared on failure, so the message says rerun rather than `--scrollId`
- deleting a missing id counts as a failed write
- `--filterSystemTemplates` regex also drops user templates
- the SIGKILL `.part` sweep (OOM case decided: D-5)
- the `SOAK_RUNNER` variable
- the hidden-index warning

**Open questions for the owner** (see the plan files):
- TEST-REQUIREMENTS §10 Q1–14: runners, storage, exit codes, resume, timeouts, non-idempotent retries, scope,
  Rally/OSB, dataset licences, supply chain, upstream, chaos on the shared box, INJ-013 allow-index,
  unsafe/sonic/ruzstd.
- PLAN §11 Q1–9 and TYPES-PLAN §8 items 1–9.
- The GAPS / A10 backlog.

---

---

## 11. Constraints carried over

- **Never push.** The repo is a local fork; `origin` is a bundle with its push URL disabled. No PRs and no remotes.
- **No AWS or other cloud infrastructure** (the xAI API that OpenCode calls is the only external service) without the owner's explicit approval and a named sandbox account (B13 stays blocked).
- Don't touch the tracked junk files **`oom`** and **`test_query_elasticdump_rs_test_857109_16.json`** (no delete,
  edit or `git rm`).
- Isolated networks for engines (`edb-internal`), no production clusters, and treat the code under test as
  untrusted (HANDOFF-TASKPLAN §0).
- Don't weaken tests. Goldens change only with a reason. Respect licences (AIT-LDS CC-BY-NC-SA excluded; loghub
  waits on Q9; Kaggle needs an account).
- `podman login dhi.io` uses the user's own credentials, which aren't included. Don't store them in the repo,
  logs or cards.
- **Never print or store secrets**: no xAI API key, dhi.io credentials, passwords or tokens in opencode.json, the repo,
  cards, feedback files, logs, `results/` or bundles. Use `opencode auth login` or `XAI_API_KEY` from your shell only.
- Don't decide owner questions (§10) on your own. Write the options in the report.
- Be resumable: every card is idempotent (worktree + branch per card), and PROGRESS-LOCAL says what runs next.

---

## 12. Verify locally (not confirmed from docs)

Checked against opencode.ai/docs/providers on 2026-10-31. That page confirms: the `xai` provider with SuperGrok OAuth or an
API key via `/connect`, credentials stored in `~/.local/share/opencode/auth.json`, `opencode auth list`, the
`provider.<id>.whitelist` option, and `{env:VAR}` for `options.apiKey`. The rest was carried over from r2 or not confirmed:

1. **Model id.** `xai/grok-4.7` is assumed. Confirm with `opencode models xai` (or the xAI console models list). If it
   differs (for example a dated or `-latest` suffix), change `model`, `small_model`, every `agent.*.model` and
   `provider.xai.whitelist` in opencode.json (`sed -i 's#grok-4.7#<id>#g' opencode/opencode.json`) and export
   `OC_MODEL=xai/<id>` for run-card.sh/smoke-grok.sh.
2. Whether the xai provider picks up `XAI_API_KEY` from the environment automatically (the docs only show `/connect`).
   If not, use `opencode auth login`. Don't add `apiKey` to the file, except as `"{env:XAI_API_KEY}"` if you really must.
3. The CLI spellings `opencode auth login` (vs the TUI `/connect`) and `opencode models <provider>`.
4. `opencode run` with permission `ask`: only `lead` (interactive) uses `ask`. Headless agents use explicit allow/deny,
   because a non-interactive run may hang or auto-reject on `ask`.
5. Edit-permission path matching: relative or absolute patterns. run-card injects both forms; check
   `opencode debug agent impl`.
6. Whether `OPENCODE_CONFIG_CONTENT` deep-merges `agent.<name>.permission` over the file's settings (same check).
7. Whether keys `share`, `autoupdate`, `default_agent`, `enabled_providers`, `steps` and `provider.xai.options.timeout`
   are accepted, and the 1.x vs 2.x schema (2.x may rename `provider`→`providers`, `snapshot`→`snapshots`). Rerun
   `opencode debug config` after any upgrade.
8. Per-task `OPENCODE_DB` (separate session stores for parallel runs).
9. `--format json` event field names (`type`, `sessionID`, `part.tokens`). tokens.json may be empty if they differ.
10. xAI rate limits and pricing for Grok 4.7 on your plan, which set the real `PAR_MAX`.
11. The nextest filterset `binary(/^chaos_local_/)` regex syntax used in G-00 (nextest ≥ 0.9.70).
12. `cargo deny check --disable-fetch` (used when `CARGO_DENY_OFFLINE=1`).

