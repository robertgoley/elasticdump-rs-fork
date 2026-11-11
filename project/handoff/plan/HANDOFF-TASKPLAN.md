# elasticdump-rs: Hand-off Task Plan for an Outside Agent ("B")

> Written 2026-10-29 for an AI agent **with no prior context** that runs on a larger machine than the one where this plan was written (called "A" below).
> The plan is self-contained. Requirement IDs (TST-…) refer to `TEST-REQUIREMENTS.md`, which ships in the plan tarball (§3). The full text of every requirement you own is repeated in Appendix A.
> Tasks for A are in `TASKS-LOCAL.md`; you do not need it, but it tells you what A delivers at each gate.

## 0. Guardrails (read first; they are not optional)

1. **Never push to any remote.** The repo has no usable remote. Do not add one, do not open PRs, do not publish images, crates or packages. Return work only as described in §9.
2. **The tool under test makes no external network calls.**
   - Run `elasticdump-rs`, `edverify`, `edgen` and the test suites on an **isolated container network** (`podman network create --internal …` or docker `--internal`), with only the throwaway test clusters on it.
   - The only outbound traffic allowed on the machine is your own: the dataset downloads in §7 (exact URLs), crates.io/static.rust-lang.org for the toolchain, container registries for the images in §6, and pip/npm for the listed tools.
   - If the tool tries to reach anything else, stop and report it as a finding.
3. **Treat the code as untrusted.** The branch contains AI-written upstream code (`tenyears-tech/elasticdump-rs`, commits `b32b6e9` and earlier) and many third-party crates.
   - Build and run it **in containers or a dedicated VM/user** with no credentials, SSH keys, cloud tokens or browser sessions reachable.
   - Do not run it as root on the host.
   - Do not add dependencies, `build.rs` files or CI changes; propose them in the report instead.
4. **Never point anything at a production or shared cluster.** Only use clusters you started for this work, on throwaway volumes, with generated passwords.
   - `--delete`, `--closeIndex` and restore tests are destructive by design.
5. **Cloud (B13) only with explicit approval.**
   - Use a sandbox AWS account that the owner names, with least-privilege, short-lived credentials stored outside the repo.
   - Never commit or log credentials, and delete every resource you create.
   - Without approval, skip B13 and report it as "blocked: needs approval".
6. **Do not weaken tests to make them pass,** and do not "fix" product code in B unless the task says so.
   - Report failures with reproduction steps; A fixes them.
   - Exception: test-harness bugs may be fixed in separate, clearly labelled commits.
7. **Respect licences** (§7): CC-BY-NC-SA data (AIT-LDS) is excluded; keep attribution files; do not redistribute corpora inside bundles.
8. **Be resumable:** one commit per finished task or cell; checkpoints for long runs; never leave a run half-reported. Write results incrementally.

## 1. Project overview

- **elasticdump-rs** is a Rust rewrite of the Node.js tool **elasticdump** (`elasticsearch-dump`). It moves data and metadata between Elasticsearch/OpenSearch clusters and files.
- **What it supports:**
  - `--type` `data`, `mapping`, `settings`, `index`, `analyzer`, `alias`, `template`, `component_template`, `index_template`, `policy` (ILM/ISM, with ILM→ISM conversion) and `script`;
  - file input and output with gzip/zstd and splitting;
  - cluster→cluster copy;
  - scroll / PIT / search_after with slices;
  - TLS/mTLS;
  - retries and concurrency.
- **Stack:** reqwest + rustls (ring), sonic-rs for the hot JSON path, serde_json for metadata, flate2/zlib-rs, zstd (C), tokio and clap. The project's own code has **0 `unsafe`**.
- **Engines and versions in scope** (9 images): OpenSearch 1.3.20, 2.x, 3.x; Elasticsearch 7.10.2 OSS, 7.10.2 default, 7.17.29, 8.6.1, 8.19.23, 9.1.2.
- **Why you are needed:** the test plan (`TEST-REQUIREMENTS.md`, 204 requirements) has work that does not fit A, which has 8 shared vCPUs, 16 GB RAM shared with a 12.5 GB VM, 35 GB disk, one node, and no cloud accounts. That work is:
  - 10M- and 100M-document loads (1–110 GB);
  - long fuzz campaigns and soak runs;
  - multi-node clusters;
  - all-versions-in-parallel matrices;
  - cross-machine reproducible builds;
  - AWS tests.
- **Known security fixes that A does before handing off:**
  - **INJ-F1 (High):** pretty-printed `_source` split NDJSON framing, so one document could inject bulk writes into other indices during cluster→cluster copy, and dump files got multi-line records.
  - **INJ-F2 (Medium):** index names from dump files (`*`, `_all`, `..`) reached URL paths.
  - INJ-F3 and INJ-F4 (Low).
  - Supply-chain findings F-01…F-14.
  - **Do not run L1/L2 loads on a bundle older than gate H1.** Earlier code has these bugs, and your results would be invalid.

## 2. Repo state

- **Repo:** a local fork. The branch is **`rewrite`**; HEAD was `958e9d5` when this plan was written ("IT: deflake fault-injection restore…"). `main` mirrors upstream.
- **Upstream origin:** `https://github.com/tenyears-tech/elasticdump-rs`. Its push URL is set to `no-push-allowed`; the `rewrite` branch exists only locally.
- **Key paths:**
  - `src/` (library + binary)
  - `tests/` (unit-adjacent, `cli_contract`, `integration_test`, `tests/it/*`)
  - `scripts/it/{up,down,run}.sh` (podman IT harness with profiles)
  - `Makefile`, `Dockerfile`
  - `deny.toml`
  - `.github/workflows/{ci,release,security}.yml`
  - `README.md`, `NODE-DIFFERENCES.md`, `SECURITY-CVE.md`
  - `TESTING-MATRIX.md` (missing at HEAD `958e9d5`; A creates it in INF-011).
- **What A adds before each gate (all in the bundle):**
  - `tools/edgen` (seeded generator);
  - `tools/edverify` (count plus multiset digest);
  - `scripts/load/run.sh` (load runner);
  - `scripts/load/measure.sh`;
  - `datasets.toml` (manifest with SHA-256);
  - converters;
  - nextest profiles;
  - fuzz targets under `fuzz/`;
  - `scripts/res/*` (resource guards).

  If a gate's bundle lacks one of these, report "blocked on A" for the tasks that need it rather than writing your own version.

## 3. Getting the code (bundles; there is no network remote)

**On A (run by A's operator or agent at each gate, in the repo that holds the work; it only reads the repo):**

```bash
cd /workspace/elasticdump-rs            # or /workspace/edrs-work while the main repo is busy
GATE=h1                                 # h1 | h2 | h3 | h4 | final
BR=rewrite                              # or local/tests (the A work branch)
OUT=/workspace/elasticdump-rs-plan
git rev-parse "$BR" > "$OUT/handoff-$GATE.BASE-SHA"
git bundle create "$OUT/elasticdump-rs-$GATE.bundle" "$BR" main
git bundle verify "$OUT/elasticdump-rs-$GATE.bundle"
tar -C /workspace -czf "$OUT/elasticdump-rs-plan-$GATE.tar.gz" \
    elasticdump-rs-plan/TEST-REQUIREMENTS.md elasticdump-rs-plan/HANDOFF-TASKPLAN.md \
    elasticdump-rs-plan/TASKS-LOCAL.md elasticdump-rs-plan/PLAN.md \
    elasticdump-rs-plan/TYPES-PLAN.md elasticdump-rs-plan/GAPS.md
( cd "$OUT" && sha256sum "elasticdump-rs-$GATE.bundle" "elasticdump-rs-plan-$GATE.tar.gz" "handoff-$GATE.BASE-SHA" > "handoff-$GATE.sha256" )
```

Uncommitted changes are **not** in a bundle. Commit first, and check that `git status --short` is empty.

**On B:**

```bash
sha256sum -c handoff-h1.sha256
git clone --branch rewrite elasticdump-rs-h1.bundle elasticdump-rs     # use the branch name A used for BR
cd elasticdump-rs
git remote set-url --push origin no-push-allowed                      # origin = the bundle file
git switch -c b/results                                              # all your commits go here
cat ../handoff-h1.BASE-SHA; git rev-parse HEAD                         # must match
```

To take a newer gate later: `git fetch ../elasticdump-rs-h2.bundle rewrite:refs/remotes/h2/rewrite`, then `git rebase h2/rewrite` on `b/results` (or merge it). Record the new base in your results.

## 4. Recommended hardware

| Resource | Minimum (B1–B4, B7–B9) | Recommended (all tasks incl. L2 and soak) | Why |
|---|---|---|---|
| CPU | 16 cores | 32 cores, dedicated (no noisy neighbours) | Scaling curves to 16 slices/workers; fuzz campaigns in parallel; Node comparison |
| RAM | 64 GB | 128 GB | Up to 9 single-node engines (2 GB heap each), or 3-node clusters × 2, plus the tool and Node |
| Disk | 1 TB NVMe | 2 TB NVMe | big5-100 (107 GB raw) + index (~1.2–1.5×) + dump files + split parts + nyc_taxis (80 GB) |
| Network | 100 Mbit/s | 1 Gbit/s | ~25 GB of compressed downloads (~1 h at 100 Mbit/s) |
| OS | Linux x86_64, kernel ≥ 5.15, cgroup v2 with memory delegation (rootful podman/docker, or rootless with systemd delegation) | same, plus root for eBPF (B12) | `--memory` limits must be enforced (on A they are **not**) |
| Optional | a second machine for B11 (repro builds), a separate AWS sandbox (B13) | | |

## 5. Prerequisites and tools

- **Containers:** podman ≥ 4.9 (or docker ≥ 24) with compose; `sysctl vm.max_map_count=262144`.
- **Rust:** rustup with **1.91.1** (the project's `rust-version` is 1.91 since L-809d; edition 2024) plus **nightly** (cargo-fuzz, ASan).
- **Cargo tools:** `cargo install --locked cargo-nextest cargo-fuzz cargo-deny cargo-cyclonedx`.
- **Node.js 20+:** `npm i -g elasticdump@6` (the reference tool for the comparison tests), installed in a container or as an unprivileged user.
- **CLI utilities:** `jq`, `zstd`, `pbzip2` (or `lbzip2`), `pigz`, `curl`, `python3` (≥ 3.10) with `requests`, `hyperfine`, `sysstat` (`pidstat`), `strace`, and `bpftrace` for B12.
- **Optional:** `esrally` (Python, B6) and `opensearch-benchmark` (B6), each in its own venv; `toxiproxy` (container `ghcr.io/shopify/toxiproxy`).
- **Record versions:** put every tool version in `results/B0/result.json`.

## 6. Environment setup (B0)

```bash
# isolated network for the tool under test and the clusters
podman network create --internal edb-internal
# the 9 images used by scripts/it/run.sh at HEAD 958e9d5 (pin by digest after the first pull; record digests in results/B0)
for img in docker.io/opensearchproject/opensearch:1.3.20 docker.io/opensearchproject/opensearch:2 \
           docker.io/opensearchproject/opensearch:3 \
           docker.io/bitnamilegacy/elasticsearch:7.10.2 \
           docker.elastic.co/elasticsearch/elasticsearch:7.10.2 \
           docker.elastic.co/elasticsearch/elasticsearch:7.17.29 \
           docker.io/bitnamilegacy/elasticsearch:8.6.1-debian-11-r0 \
           docker.elastic.co/elasticsearch/elasticsearch:8.19.23 \
           docker.io/bitnamilegacy/elasticsearch:9.1.2-debian-12-r0 ; do podman pull "$img"; done
# bitnamilegacy 7.10.2 = OSS build (no X-Pack); docker.elastic.co 7.10.2 = default distribution
```

- **Images and heaps:**
  - `scripts/it/run.sh` and `up.sh` in the bundle are the source of truth for profile variants (plain/secure) and environment settings.
  - Heap per node: 2 GB for L1 and 4–8 GB for L2 (`ES_JAVA_OPTS=-Xms4g -Xmx4g`), and a container `--memory` of heap × 2.
  - Single-node: `discovery.type=single-node`. For multi-node (B8), use a 3-node compose with dedicated volumes.
- **Build:** build the tool and test binaries in a container (`rust:1.91` image or newer, or the Dockerfile builder stage) with `--network none` after `cargo fetch --locked`. Run them on `edb-internal`.
- **Ports:** never publish them on 0.0.0.0. Use `127.0.0.1:` binds when host access is needed.

## 7. Datasets (links verified by curl on 2026-10-29)

**Download rules:**
- download only these URLs;
- record SHA-256 in `results/B1/datasets.json`;
- keep the files outside the repo, e.g. `/data/corpora`;
- never put them in a bundle.

| Corpus | URL | Size (compressed) | Docs / raw size | Licence | Tier |
|---|---|---|---|---|---|
| Rally `http_logs` 1998 day files | `https://rally-tracks.elastic.co/http_logs/documents-<N>.json.bz2`, N ∈ {181998, 191998, 201998, 211998, 221998, 231998, 241998}; all returned 200 | 13.8 MB, 49.5 MB, 65.8 MB, 88.4 MB, 54.3 MB, 61.0 MB, 907 MB | 2.71M, 9.70M, 13.05M, 17.65M, 10.72M, 11.96M, 181.46M docs; **247.2M** in total, 33.4 GB parsed | Apache-2.0 (track); data from the WorldCup98 traces | L1: 191998; L2: all |
| OSB `http_logs` zst mirror | `https://dbyiw3u3rf9yr.cloudfront.net/corpora/http_logs/documents-181998.json.zst` | 16.1 MB | 2.71M | Apache-2.0 | L1 (zst input test) |
| OSB **big5** | `https://dbyiw3u3rf9yr.cloudfront.net/corpora/big5/documents-100.json.zst` (also `documents-60.json.zst`, 4.31 GB) | **7.31 GB** | 116M docs, ~107 GB raw (60 GB variant: 69.2M) | Apache-2.0 | L2 |
| `nyc_taxis` | `https://rally-tracks.elastic.co/nyc_taxis/documents.json.bz2` or `https://dbyiw3u3rf9yr.cloudfront.net/corpora/nyc_taxis/documents.json.zst` | 4.82 GB / 4.81 GB | 165M docs, 79.8 GB | public domain (NYC TLC) | L2 |
| OSB eventdata | `https://dbyiw3u3rf9yr.cloudfront.net/corpora/eventdata/eventdata.json.bz2` | 793 MB | 20M, 16.4 GB | Apache-2.0 | L1 |
| geonames | `https://rally-tracks.elastic.co/geonames/documents-2.json.bz2` | 265 MB | 11.4M | CC-BY-3.0 (attribute GeoNames) | L1 variety |
| nested | `https://rally-tracks.elastic.co/nested/documents.json.bz2` | 696 MB | 11.2M | CC-BY-SA-3.0 (StackOverflow) | L1 variety |
| pmc | `https://rally-tracks.elastic.co/pmc/documents.json.bz2` | 5.93 GB | 574k large docs, 23.3 GB | CC-BY-2.0 subset | L1 large docs |
| noaa | `https://rally-tracks.elastic.co/noaa/documents.json.bz2` | 995 MB | 33.7M | US Gov (public) | optional |
| NASA HTTP | `https://ita.ee.lbl.gov/traces/NASA_access_log_Jul95.gz`, `…_Aug95.gz` | 20.7 / 16.6 MB | ~3.5M lines (CLF) | freely redistributable | converter test |
| Loghub | Zenodo API `https://zenodo.org/api/records/8196385` (the HTML page returns 403 to curl) | 57 MB – 2 GB per set | raw text | CC-BY-4.0 (repo README says "research"; the owner must confirm, Q9) | optional, only after the owner confirms |

**Excluded:** OSB `elb_logs` (403), the NASA ftp mirror (dead), and AIT-LDS (CC-BY-NC-SA).

**Generators:**
- **`edgen` from the bundle** (seeded, reproducible: `edgen --profile logs --seed 1 --count 10000000 --format elasticdump --compress zstd`);
- `flog` (`go install github.com/mingrammer/flog@latest`; MIT; no seed, soak only);
- OSB `generate-data` (optional).

**Loading a source cluster** must use an **independent loader**, not the tool under test, so that tool bugs cannot contaminate the source:
- Rally/OSB corpora are bulk-ready source lines. Load them with `edgen`'s `--format bulk` path or the reference loader below.
- Assign deterministic ids (`hash(dataset, file, line)`, DATA-010) so digests are comparable.

```python
# ref_load.py: independent reference bulk loader (stdlib only), stdin = one JSON source per line
import sys, json, hashlib, urllib.request, base64, os
url, index, dataset = sys.argv[1], sys.argv[2], sys.argv[3]       # e.g. http://es:9200 http_logs http_logs-191998
auth = os.environ.get("ES_AUTH")                                   # "user:pass" or unset
def send(buf):
    req = urllib.request.Request(f"{url}/_bulk", data=buf, method="POST",
                                 headers={"Content-Type": "application/x-ndjson"})
    if auth: req.add_header("Authorization", "Basic " + base64.b64encode(auth.encode()).decode())
    r = json.load(urllib.request.urlopen(req, timeout=300))
    if r.get("errors"): sys.exit("bulk errors: " + json.dumps([i for i in r["items"] if "error" in next(iter(i.values()))][:3]))
buf, n = [], 0
for i, line in enumerate(sys.stdin.buffer):
    line = line.strip()
    if not line: continue
    json.loads(line)                                                   # validate; never splice invalid JSON
    _id = hashlib.sha256(f"{dataset}:{i}".encode()).hexdigest()[:20]
    buf.append(json.dumps({"index": {"_index": index, "_id": _id}}).encode() + b"\n" + line + b"\n"); n += 1
    if len(buf) == 5000: send(b"".join(buf)); buf = []
if buf: send(b"".join(buf))
print(n)
```

Example: `pbzip2 -dc documents-191998.json.bz2 | python3 ref_load.py http://es-src:9200 http_logs http_logs-191998`. Run it inside a container on `edb-internal`. Create the index first with the track's mapping (from the rally-tracks repo, `http_logs/index.json`), using `number_of_replicas: 0`, and refresh it after loading.

## 8. Tasks for B

**Dependency gates** (A delivers a bundle at each one; see §3):

| Gate | A has finished | Contents you can rely on |
|---|---|---|
| **H0** | nothing new (current HEAD `958e9d5`) | Only for B0 setup and B1 downloads. **Do not run any test result that counts on H0** (it still has INJ-F1/F2) |
| **H1** | A1 security fixes + A3 harness | INJ-F1…F4 fixed; nextest tiers; router mock; `edverify`; metrics wrapper; resource guards; `edgen`; `datasets.toml`; converters; `scripts/load/run.sh` |
| **H2** | A5 | all fuzz targets and proptests, short-run clean, with seed corpora |
| **H3** | A4 | golden contracts per version; export-once/import-everywhere expectation table (API-071); Node interop suite |
| **H4** | A7 | egress-denied runner, strace profile, seccomp/Landlock wrapper, same-machine repro-build script |
| **Final** | A8 | L0/L1-lite baselines, which are your reference for L1 at 10M |

**Task summary:**

| Task | Requirements | Depends on | Machine time | Agent effort |
|---|---|---|---|---|
| B0 Environment | — | H0 | 1–2 h | 0.5 d |
| B1 Datasets | DATA-004/005 inputs, DATA-010 | H0 (download), H1 (convert/verify) | ~1–3 h download + 2 h convert | 0.5 d |
| B2 L1 load suite (10M) | LOAD-004…011 (10M parts), LOAD-014 (1,000 indices), LOAD-012 (runner limits) | H1 (+ A8 L0 baselines preferred) | 2–3 days | 2 d |
| B3 L2 scale (100M+, 100 GB+) | LOAD-005/008/009 at L2, LOAD-012 big runner | B2 | 3–4 days | 1.5 d |
| B4 Cross-engine at scale | LOAD-013 | B2, H3 | 1 day | 0.5 d |
| B5 Soak | LOAD-016, DATA-009 | B3 | 1–2 days per run | 0.5 d |
| B6 Baselines and corpus cache | LOAD-019, DATA-006 | B1 | 1 day | 0.75 d |
| B7 Long fuzz campaigns | API-084, INJ-015, SUP-020, SUP-022 (long parts) | H2 | 2–3 days (parallel on 16+ cores) | 0.75 d |
| B8 Multi-node and node restarts | BRK-NET-009, BRK-SRV-012 (multi-node parts) + new multi-node scenarios | H1 (H3 preferred) | 1 day | 2 d |
| B9 All-versions matrix in parallel | API-070 (parallel), API-072 (all pairs) | H3 | 4–6 h per full run | 1 d |
| B10 Authoritative perf curves | LOAD-010, LOAD-011 at 10M on dedicated cores | B2 | 1 day | 0.5 d |
| B11 Cross-machine reproducible builds | SUP-010 (cross-machine part) | H4 | 2 h | 0.5 d |
| B12 Privileged tracing at scale (optional) | SUP-002/003 extended with eBPF during L1 runs | H4, B2 | 0.5 day | 0.5 d |
| B13 AWS (blocked until approved) | GAPS §3/§4 live tests: S3, SigV4, Amazon OpenSearch Service, AOSS | A10 L-A04/L-A05 built + owner approval | 1 day | 1.5 d |

**Totals:** ≈ **13 agent-days**, ≈ **12–16 days of machine time**, ≈ **3–4 calendar weeks after H1**. B13 is extra once approved.

---

### B0. Environment
- **Steps:**
  1. Check hardware against §4.
  2. Install the tools in §5.
  3. Create `edb-internal`.
  4. Pull the images and record their digests.
  5. Clone the bundle (§3).
  6. Build in a container with `--network none` after `cargo fetch --locked`.
  7. Run `cargo nextest run --profile unit` (from H1 on), or `cargo test --lib` on H0.
- **Acceptance:**
  - all unit tests pass;
  - one OpenSearch 2 and one ES 8.19 single-node container are healthy on `edb-internal`;
  - from a container on that network, `curl https://example.com` fails (egress blocked).
- **Report:** `results/B0/result.json` with host facts (CPU model, cores, RAM, disk, kernel, cgroup mode, podman/docker version), tool versions, image digests and the bundle SHA.

### B1. Datasets
- **Steps:**
  1. Download the §7 files with `curl -fL --retry 5 -C -`, which can resume.
  2. Record `sha256sum` for each.
  3. From H1 on, run the bundle's converters (`scripts/data/convert.sh <corpus>`) to produce `elasticdump`-format NDJSON.zst with deterministic ids, and the source-only form for `ref_load.py`.
  4. Record doc counts.
- **Acceptance:**
  - every file's size matches §7;
  - doc counts after conversion match the track files (http_logs-191998 = 9,697,882; full http_logs = 247,249,096; big5-100 = 116M ±0.1% as reported by the OSB workload; nyc_taxis = 165,346,692 per the Rally track; eventdata = 20,000,000);
  - the converters are deterministic (two runs give equal SHA-256).
- **Report:** `results/B1/datasets.json` with a `{name, url, bytes, sha256, docs, licence}` entry per file.

### B2. L1 load suite (10M docs)
- **Setup:**
  - single node, heap 4 GB (`--memory 8g`), `number_of_replicas: 0`;
  - sources: `http_logs` documents-191998 (9.7M) loaded with `ref_load.py`, and `edgen --profile logs --seed 1 --count 10000000` (~10 GB);
  - variety runs: geonames, nested, pmc, eventdata.
- **Scenarios** (`scripts/load/run.sh L1 <scenario>`, from the requirements):
  - dump with scroll; PIT unsliced; slices ∈ {2, 4, 8, 16};
  - restore at concurrency ∈ {1, 4, 8, 16} × `--limit` ∈ {1000, 5000};
  - cluster→cluster copy;
  - gzip levels 1/6/9 and zstd 1/3/9/19;
  - `--fileSize 256mb` and `--maxRows 1000000` splits;
  - kill -9 / resume at 10 random points (LOAD-006);
  - fault injection (LOAD-007): 1 in 50 connections cut, a 429 storm (`thread_pool.write.queue_size: 1`), flood-stage watermark for 30 s, `podman restart` mid-run;
  - 1,000 indices (LOAD-014);
  - memory flatness (LOAD-005) with `/usr/bin/time -v` and `pidstat -r 1`;
  - Node comparison at 10M (LOAD-011): same flags; Node `elasticdump` 6.x in a container.
- **Acceptance (per run):**
  - `edverify` count and digest equal between source index, dump file(s) and destination;
  - 1,000 random ids byte-equal;
  - `Failed Writes` = 0 unless the scenario expects loss;
  - peak RSS at 10M within +10% of the 100k run with the same flags;
  - no part over the limit + 1 doc, and no doc spanning parts;
  - after kill -9: the final path is absent or complete (never partial), and a rerun gives the full digest;
  - **any mismatch is a finding**, not a retry.
- **Report:** one `results/B2/<scenario>/result.json` per cell (metrics per LOAD-003: wall time, docs/s, MB/s, CPU-s, peak RSS, retries by kind, failed writes, peak FDs, output bytes, compression ratio, cluster reject counts), plus `results/B2/SUMMARY.md` with a table.
- **Runtime:** ~2–3 days of machine time for the full matrix (each 10M dump/restore cell takes 5–40 min).

### B3. L2 scale (100M+ docs, 100 GB+)
- **Setup:** single node with a 8 GB heap (`--memory 16g`), 1 TB+ NVMe.
- **Sources:**
  - big5-100: 116M docs, loaded with `ref_load.py` in parallel streams;
  - full http_logs: 247M;
  - nyc_taxis: 165M.
  - Do them one at a time, and delete each before the next if disk is short.
- **Scenarios:**
  - dump PIT with slices 8/16 to zstd-3 split at 1 GB;
  - restore from the parts at concurrency 8/16;
  - cluster→cluster copy to a second node (if RAM allows, otherwise file round-trip);
  - multi-GB single gzip file;
  - kill -9 at 3 points during the big5 dump and restore.
- **Acceptance:** the same as B2, plus:
  - no unbounded growth in RSS, FDs or threads;
  - every server-side scroll/PIT context is closed after each run (LOAD-017);
  - staging leftovers are listed and cleaned.
- **Report:** `results/B3/<corpus>-<scenario>/result.json`, plus `SUMMARY.md` with throughput and memory charts as PNG or CSV.
- **Runtime:** loading big5 ~3–6 h; each dump/restore 1–4 h; 3–4 days in total.

### B4. Cross-engine at scale (LOAD-013)
- **Pairs:** ES 8.19 → OS 2.x and ES 9.1 → OS 3.x, with 10M docs, both via file and live cluster→cluster.
- **Acceptance:**
  - digest equal for data;
  - mapping/settings differences equal **exactly** the API-071 expectation table from H3 (field stripping, warnings);
  - no silent loss.
- **Report:** `results/B4/<pair>/result.json`.
- **Runtime:** ~1 day.

### B5. Soak (LOAD-016, DATA-009)
- **What:** 6–12 h of repeated dump → restore → copy cycles on the big5-60 or http_logs-241998 corpus, plus `flog -l -f json` streaming into a restore through stdin (`$`), digested on the fly.
- **Watch:** RSS drift, FD count, thread count, server-side open scroll/PIT contexts, staging leftovers, disk usage. Sample every 10 s into CSV.
- **Acceptance:**
  - RSS drift < 5% per hour after warm-up;
  - FDs and threads flat;
  - zero leaked contexts;
  - every cycle's digest equals the source.
- **Report:** `results/B5/soak-<n>/{result.json,samples.csv}`.

### B6. Baselines and corpus cache (LOAD-019, DATA-006)
- **Steps:**
  1. Keep the L1 corpora cached and checksummed (DATA-006: http_logs-191998, eventdata, geonames, nested, pmc).
  2. Run Rally (`esrally race --track=http_logs --challenge=append-no-conflicts-index-only --pipeline=benchmark-only --target-hosts=…`) and OSB equivalents against the same single node, so the tool's restore throughput can be compared with the cluster's ingest ceiling.
- **Acceptance:** the baseline docs/s per corpus is recorded, and the tool reaches it within ±x% (report x).
- **Report:** `results/B6/result.json`.

### B7. Long fuzz campaigns (API-084, INJ-015, SUP-020, SUP-022)
- **Targets:** every target in `fuzz/fuzz_targets/` from H2 (19 expected), including:
  - `fuzz_encode_doc`, `fuzz_file_to_bulk`, `fuzz_extract_batch`, `fuzz_url_segments`, `fuzz_rename_output`, `fuzz_meta_names`;
  - `zstd_in`, `gzip_in`;
  - `sonic_vs_serde`;
  - the API-084 targets.
- **Command:**
  ```
  cargo +nightly fuzz run <t> -s address -j <cores/targets> -- \
    -max_total_time=14400 -rss_limit_mb=4096 -timeout=10
  ```
  Use 4 h for `sonic_vs_serde` and `zstd_in`, 1 h for the others. Then minimise any crash with `cargo fuzz tmin` and save the corpus.
- **Acceptance:** zero crashes, OOMs, timeouts or oracle violations. Each finding gets a minimised reproducer committed under `fuzz/artifacts/<t>/` and a failing unit test (`#[ignore]` is fine; A fixes it).
- **Report:** `results/B7/<target>.json` with `{execs, cov, corpus_size, crashes:[…]}`; return the grown corpora as a tarball, not in git.
- **Runtime:** ~2–3 days on 16+ cores.

### B8. Multi-node and node restarts
- **Cluster:** 3-node clusters for OS 2.x and ES 8.19, with 1 replica.
- **Scenarios:**
  - rolling restart during a sliced PIT dump and during a restore (BRK-NET-009, multi-node);
  - kill the node holding a primary mid-dump (replica promotion);
  - real red cluster: stop 2 of 3 nodes with `number_of_replicas: 0` (BRK-SRV-012);
  - shard relocation during scroll;
  - a coordinating node that disappears behind a round-robin DNS name;
  - `--closeIndex` analyzer restore on a multi-node cluster.
- **Acceptance:** STD-ACC (no panic, no silent data loss, clear error, correct exit code, no secret leakage); a digest equal to the source when the run reports success, and a non-zero exit when it cannot guarantee completeness.
- **Report:** `results/B8/<scenario>/result.json`.
- **Runtime:** ~1 day.

### B9. All-versions matrix in parallel (API-070, API-072)
- **What:**
  - run the export-once/import-everywhere matrix (9 exports × 8 imports = 72 cells) with all 9 single nodes up at once;
  - run all live cluster→cluster pairs (API-072): ES 7.10.2 → OS 1.3.20, ES 7.17 → OS 2.x, ES 8.19 → OS 3.x, ES 9.1 → OS 3.x, OS 2.x → ES 8.19, OS 3.x → ES 9.1, ES 7.17 → ES 9.1, OS 1.3 → OS 3.x.
- **Acceptance:** every cell matches the API-071 expectation table from H3; any difference becomes a row in the report.
- **Report:** `results/B9/matrix.json` (cells with status and diff) and `matrix.md`.
- **Runtime:** 4–6 h per full run.

### B10. Authoritative performance curves (LOAD-010, LOAD-011)
- **What:** on dedicated cores (no other load; CPU governor set to performance), dump slices 1–16 and restore concurrency 1–16 at 10M, each 3 times; also Rust vs Node at 10M.
- **Acceptance:** coefficient of variation < 5% per point; the curves and Rust/Node ratios are compared with the README claims.
- **Report:** `results/B10/curves.csv` plus `SUMMARY.md`.

### B11. Cross-machine reproducible builds (SUP-010)
- **What:** run the H4 repro script on two different machines (or a VM plus a host). Use a different user, a different path, at a different time, with the same toolchain image by digest and `SOURCE_DATE_EPOCH`.
- **Acceptance:** SHA-256 identical for the binary (and the image layers, if the script builds images).
- **Report:** `results/B11/result.json` with both hashes and the environment of each machine; if they differ, include the diffoscope output.

### B12. Privileged tracing at scale (optional; SUP-002/003 extension)
- **What:** run `bpftrace`/eBPF (`tracepoint:syscalls:sys_enter_connect`, `openat`, `execve`) as root around B2 cells, and the H4 seccomp/Landlock profile under load.
- **Acceptance:**
  - `connect` only to cluster addresses;
  - `openat` only under the input/output paths, the CA files, `/proc/self`, `/etc/ssl` and `/etc/resolv.conf`;
  - no `execve` after start;
  - no throughput regression > 5% with the profile.
- **Report:** `results/B12/result.json` plus the trace summaries.

### B13. AWS (blocked until the owner approves and names a sandbox account)
- **Prerequisite:** A has built S3/MinIO support (L-A04) and SigV4/AOSS support (L-A05).
- **Tests:**
  - S3 dump/restore with SSE-KMS, storage class and multipart;
  - Amazon OpenSearch Service with IAM SigV4 (`--awsChain`, profiles, region);
  - OpenSearch Serverless (`aoss`: no scroll/refresh);
  - credential leakage canary.
- **Acceptance:** digests equal; no credentials in any log or result; every resource is deleted at the end (list them in the report).
- **Report:** `results/B13/result.json`.

## 9. How to return results

**Commits:** only on branch `b/results`. One commit per task or cell, with the message `B<n>[-cell]: <requirement IDs>: <summary>`. Allowed changes:
- `results/**`;
- harness and test fixes in `tests/`, `scripts/`, `tools/` and `fuzz/`, each in a separate commit labelled `harness:`;
- new failing tests that reproduce findings (`#[ignore]` with the finding ID).

Product code (`src/`), `Cargo.toml`/`Cargo.lock`, `build.rs` and `.github/` must stay unchanged. Propose changes in `results/PROPOSALS.md` instead.

**Return package:**

```bash
BASE=$(cat ../handoff-h1.BASE-SHA)                  # the base you started from (or the latest gate you rebased on)
git bundle create b-results-$(date -u +%Y%m%d).bundle "$BASE"..b/results
git format-patch "$BASE"..b/results -o b-results-patches/      # same content as patches (alternative)
tar -czf b-results-artefacts-$(date -u +%Y%m%d).tar.gz results/ fuzz-corpora/   # large artefacts, if not committed
sha256sum b-results-* > b-results.sha256
```

**Result files** (machine-readable, one per task or cell): `results/<B-task>/<cell>/result.json`:

```json
{
  "schema": "edrs-result/1",
  "task": "B2", "cell": "dump-pit-slices8-zstd3", "requirements": ["TST-LOAD-004", "TST-LOAD-005"],
  "status": "pass | fail | partial | blocked | skipped",
  "started_utc": "2026-11-02T14:00:00Z", "finished_utc": "2026-11-02T14:37:12Z",
  "host": {"cpu": "…", "cores": 32, "ram_gb": 128, "disk": "NVMe 2 TB", "kernel": "…", "container_runtime": "podman 5.x", "cgroup": "v2 delegated"},
  "git": {"bundle_base": "<sha>", "head": "<sha>"},
  "tool": {"version": "elasticdump-rs 0.1.0", "build": "release", "binary_sha256": "…"},
  "engine": {"image": "…@sha256:…", "heap": "4g", "nodes": 1},
  "dataset": {"name": "http_logs-191998", "sha256": "…", "docs": 9697882},
  "commands": ["scripts/load/run.sh L1 dump-pit-slices8-zstd3"],
  "metrics": {"wall_s": 0, "docs_per_s": 0, "mb_per_s_in": 0, "mb_per_s_out": 0, "cpu_s_user": 0, "cpu_s_sys": 0,
              "peak_rss_mb": 0, "peak_fds": 0, "retries": {"429": 0, "5xx": 0, "net": 0}, "failed_writes": 0,
              "output_bytes": 0, "compression_ratio": 0},
  "correctness": {"expected_count": 0, "actual_count": 0, "expected_digest": "…", "actual_digest": "…", "sampled_ids_equal": 1000},
  "findings": [{"id": "B2-F1", "severity": "high|medium|low|info", "title": "…", "requirement": "TST-…",
                "repro": "exact commands", "evidence": "results/B2/…/log.txt", "expected": "…", "actual": "…"}],
  "artefacts": ["results/B2/…/tool.log", "results/B2/…/pidstat.csv"],
  "notes": "free text"
}
```

**Summary:** `results/SUMMARY.md` with:
- one table per task (cell, status, key metric, finding IDs);
- a findings list ordered by severity;
- the things you could not do, with the reason;
- the total machine hours used.

**Times:** UTC in files. Wall-clock limits per cell are in the task text; if a cell exceeds 2× its expected runtime, stop it, record `partial` and move on.

## 10. How A merges the results back

```bash
cd /workspace/elasticdump-rs            # or the A work clone
sha256sum -c b-results.sha256
git bundle verify b-results-YYYYMMDD.bundle
git fetch b-results-YYYYMMDD.bundle b/results:b-results
git log --stat rewrite..b-results       # review: no src/, Cargo.*, build.rs or .github/ changes
git diff --stat rewrite...b-results -- src Cargo.toml Cargo.lock .github build.rs   # must be empty
```

A then validates every `result.json` against the schema, files the findings as tasks, and updates `TESTING-MATRIX.md`.

## 11. What B depends on from A (summary)

| B task | Must wait for | Can start early with |
|---|---|---|
| B0 | — | H0 |
| B1 | H1 for conversion | downloads on H0 |
| B2, B3, B5, B10 | **H1** (INJ-F1/F2 fixed; `edgen`, `edverify` and the runner exist); A8 L0 baselines preferred | — |
| B4, B9 | **H3** (expectation table API-071, goldens) | cluster provisioning |
| B6 | B1 | Rally/OSB install |
| B7 | **H2** (fuzz targets) | toolchain |
| B8 | H1 (H3 preferred) | 3-node compose files |
| B11, B12 | **H4** | — |
| B13 | A10 L-A04/L-A05 **and** owner approval | — |

## 12. Standard acceptance (STD-ACC) and terms

- **STD-ACC (from the requirements, applies to every chaos/robustness check):**
  - no panic (no `RUST_BACKTRACE` output, exit code ≠ 101);
  - no silent data loss (a digest mismatch is impossible with exit 0);
  - a clear one-line error naming the cause;
  - the documented exit code (0 = success, 1 = runtime failure, 2 = usage, 130 = interrupted);
  - no secret leakage (canary passwords and tokens are never in stdout, stderr, logs or result files).
- **L0 / L1 / L2:** 100k / 10M / 100M+ documents.
- **`edverify` digest:** an order-independent multiset digest over `(index, _id, routing, raw _source)`.
- **Findings from the security review** (fixed in A before H1; re-check them during B2 and B7):
  - **INJ-F1:** a pretty-printed `_source` split framing.
  - **INJ-F2:** wildcard or dot index names from files reached URLs.
  - **INJ-F3:** non-string `_id`/`routing` broke whole bulk requests.
  - **INJ-F4:** unescaped `--parseExtraFields`.

## Appendix A. Full text of the requirements B works on

- **TST-API-070** (must; context (A builds it; B uses it)): **Cross-version matrix, "export once, import everywhere":** each of the 9 images (plain) exports a standard fixture set (§3.3) to zstd files once per nightly run. Every other image then imports every export, one container at a time. 9 × 8 = 72 imports, each covering 11 types — Results match the expectation table (API-071) exactly — extend `cross.rs` + `IT_CROSS_DIR` per version
- **TST-API-071** (must; context (A builds it; B uses it)): **Committed expectation table** `tests/golden/cross-expectations.toml`: for each (source version, destination version, type) one of `pass`, `pass-with-warnings[list]`, `refused[message substring]`, `needs-flag[flag]` (e.g. `--convertIlmToIsm`, `--excludeSettings=`) — A diff between actual and expected fails CI; changes need a review — toml + asserter
- **TST-API-072** (should; A does the reduced run; B does the full scale): **Live cluster-to-cluster pairs** (two nodes at 256m heaps, when memory allows): ES 7.10.2 → OS 1.3.20 (the classic migration); ES 7.17 → OS 2.19; ES 8.19 → OS 3.x; ES 9.1 → OS 3.x; OS 2.19 → ES 8.19; OS 3.x → ES 9.1; ES 7.17 → ES 9.1 (skip-major); OS 1.3 → OS 3.x; downgrades ES 9.1 → ES 8.19 and OS 3.x → OS 2.19 — Digest equal for data; metadata per the expectation table — `run.sh cross PAIR=`
- **TST-API-084** (must; A does the reduced run; B does the full scale): **cargo-fuzz targets:** `json_split`, `codec_decode`, `bulk_encode_doc` (meta extraction), `extract_batch` (search response), `bulk_handle_response`, `parse_size`, `endpoint_parse`, `ilm_to_ism_convert`, `renamer_new_index`, `ini_auth_file`. Corpora are seeded from the goldens — 60 s per target on PR, 1 h nightly; crashes are minimised and committed as regression unit tests — `fuzz/` directory (needs `pub` test hooks or `#[cfg(fuzzing)]` entry points)
- **TST-LOAD-001** (must; context (A builds it; B uses it)): Load runner `scripts/load/run.sh <tier> <scenario>`: provisions the node with caps, seeds from the cache or `edgen`, runs the scenario under `measure.sh` (INF-006), verifies with `edverify`, tears down — One command per tier; results JSON per scenario; seeds are recorded — runner + scenario files (`scenarios/*.toml`)
- **TST-LOAD-002** (must; context (A builds it; B uses it)): **Correctness on every load run:** `count` and digest equal (source index vs dump file(s) vs destination index). Also 1,000 random `_id`s byte-compared with the source; `Failed Writes` = 0 unless the scenario expects loss — Any mismatch fails and names the first 10 differing ids — `edverify`
- **TST-LOAD-003** (must; context (A builds it; B uses it)): **Metrics per run:** wall time; docs/s; MB/s (in and out); CPU-seconds (user + sys); peak RSS; retries by kind (parsed from `retrying` warnings); `Failed Writes`; open FDs (peak); output bytes and compression ratio; cluster-side indexing/search rate and rejected count (`_nodes/stats/thread_pool`); open scroll/PIT contexts afterwards — All fields present in results JSON; schema-validated — `measure.sh`
- **TST-LOAD-004** (must; A does the reduced run; B does the full scale): **Scenario matrix** (each tier runs a documented subset): dump scroll; dump PIT unsliced and slices ∈ {2, 4, 8, 16}; restore at concurrency ∈ {1, 4, 8} and `--limit` ∈ {1000, 5000}; cluster→cluster copy (memory permitting); `--fsCompress`; `--compressionType zstd` (levels 1/3/9); `--fileSize`, `--maxRows`; `--sourceOnly`; `--searchBody` range filter — Matrix defined in `scenarios/*.toml`; each cell has a correctness gate and a perf baseline — scenarios
- **TST-LOAD-005** (must; A does the reduced run; B does the full scale): **Memory is bounded and flat:** peak RSS at 10M docs is within 10% of peak RSS at 100k for the same flags; the README formula `(bufferSize + slices + concurrency) × limit × avg_doc × 1.5 + 10 MB` holds within +25% for avg doc 200 B, 1 KB, 40 KB (pmc) and 1 MB (edgen `huge`) — Asserted per run; the formula is updated if it is wrong — `measure.sh`
- **TST-LOAD-006** (must; A does the reduced run; B does the full scale): **Resumability under load:** (a) dump `kill -9` at 10 random points → final path absent, staging leftovers listed (BRK-SIG-003), rerun with `--overwrite` gives the full digest; (b) restore `kill -9` then rerun the same file (explicit ids) → digest equal; (c) restore resume with `--offset N`, N taken from the last `Total Writes`; (d) scroll resume with `--scrollId` while the context is alive — Digest equal in (a)–(d); the documented resume recipe is tested verbatim — runner hooks (`KILL_AT_DOCS=`)
- **TST-LOAD-007** (must; A does the reduced run; B does the full scale): **Failure injection under load:** fault proxy cutting 1 in 50 connections; a 429 storm from `thread_pool.write.queue_size: 1` + `size: 1`; disk-watermark flood-stage block for 30 s; `podman restart` of the node mid-run — Run completes with digest equal, **or** exits non-zero with a clear message; never exit 0 with loss (see BRK-SRV-001) — runner + fault proxy
- **TST-LOAD-008** (must; A does the reduced run; B does the full scale): **Compression at scale:** gzip levels 1/6/9 and zstd 1/3/9/19 on 10M docs; multi-GB single files; `cat` of parts readable; throughput per core vs the README claims — Results recorded; README numbers within ±20% or updated — scenarios
- **TST-LOAD-009** (must; A does the reduced run; B does the full scale): **Splitting at scale:** 10M docs with `--fileSize 256mb` and `--maxRows 1000000`. Assertions: part count is as expected; no document spans parts; each part ≤ limit + 1 doc; restoring all parts gives the full digest; the staged parts are committed together (BRK-RES-002) — Asserted — scenarios
- **TST-LOAD-010** (must; A does the reduced run; B does the full scale): **Concurrency scaling curves:** dump slices 1–16 with workers = slices; restore concurrency 1–16; record docs/s and CPU-s; compare with the previous nightly — Regression > 10% on any point fails nightly — trend store (LOAD-020)
- **TST-LOAD-011** (must; A does the reduced run; B does the full scale): **Node comparison** at L0 and L1 (Node only up to 10M docs because of runtime): same flags for both tools; Node output digest equals Rust output digest (data); time and CPU ratios checked against PLAN.md §8 targets — Ratios recorded; digest equal — extend `quick-vs-node.py`
- **TST-LOAD-012** (must; context (A builds it; B uses it)): **Resource limits by place:** box = 1 node, heap ≤ 4 GB, container `--memory 6g`, tool ≤ 1 GB, ≤ 25 GB disk incl. cache, L2 forbidden; standard 4-vCPU/16-GB/~14-GB-disk GitHub runner = L0 plus a reduced L1 (5M docs, zstd only, no cache) [runner disk figure to confirm]; big runner = L1 + L2 — The runner refuses tiers that do not fit (INF-007) — guards
- **TST-LOAD-013** (should; B only): Cross-engine at scale: ES 8.19 → OS 2.19 and ES 9.1 → OS 3.x with 10M docs via file and live — Digest equal — runner
- **TST-LOAD-014** (should; A does the reduced run; B does the full scale): Many indices and many fields: 1,000 small indices (wildcard dump of `data`, `mapping`, `settings`, `index`, `alias`); a 5,000-field mapping (`index.mapping.total_fields.limit` raised); 100 aliases — Completes; meta types finish in under N minutes (baseline) — edgen profiles
- **TST-LOAD-016** (should; B only): **Soak:** 6–12 h of repeated dump/restore/copy cycles on L2 data; watch RSS drift, FD count, thread count, server-side open contexts, staging leftovers, disk usage — RSS drift < 5% per hour after warm-up; 0 leaked contexts and FDs — soak runner
- **TST-LOAD-017** (must; context (A builds it; B uses it)): **Server-side leak check after every load and IT run:** `_nodes/stats/indices/search` `scroll_current` = 0 and `point_in_time_current` = 0 (where the stat exists); no hidden staging indices; no `.part-` files — Asserted in the teardown of every load/IT run — harness teardown
- **TST-LOAD-019** (could; B only): Cluster-bound baseline: Rally/OSB ingest of the same corpus on the same node, to show how close the tool gets to the cluster's own limit — Recorded ratio — Rally/OSB optional
- **TST-DATA-006** (should; B only): Cached L1 corpora on the runner (`http_logs-191998`, `eventdata`, `geonames`, `nested`, `pmc`): about 8 GB compressed; zstd re-compressed copies — Cache hit on a second nightly — CI cache or volume
- **TST-DATA-009** (could; B only): flog streaming mode for unbounded soak input (`flog -l -f json`), digested on the fly — Soak runs ≥ 6 h — soak runner
- **TST-DATA-010** (must; context (A builds it; B uses it)): Deterministic `_id` policy for every external dataset (`hash(dataset, file, line)`) so dumps and restores are digest-comparable — Same id on every conversion — converters
- **TST-BRK-NET-009** (must; A does the reduced run; B does the full scale): Node restart mid-dump and mid-restore (`podman restart`, ~30–60 s down) — retry defaults: 4 × 5 s = 20 s — Restore: survives if `--retryAttempts` covers the downtime, otherwise fails clearly. Dump: scroll/PIT contexts are lost → clear "context lost, rerun" error — Default 20 s retry budget is shorter than a restart: restore aborts mid-file; the message should give the `--offset` to resume — resume recipe works (LOAD-006) — node.sh
- **TST-BRK-SRV-012** (should; A does the reduced run; B does the full scale): Cluster `red` / some shards unassigned during a dump — extract (`_shards.failed`) — Fails clearly naming the shards — — (verify) — | node.sh (`index.routing.allocation.require` to a missing node)
- **TST-SUP-002** (must; context (A builds it; B uses it)): **Syscall and file-access tracing:** `strace -f -e trace=%file,%network,%process -o …` around a representative command matrix (dump/restore/split/meta/TLS). Assertions: no `execve` beyond the binary; `connect` only to allow-listed addresses; `openat` only within the allow-list (input, output dir, staging, cert files, system CA dirs, `/proc/self`, `/etc/{hosts,resolv.conf,nsswitch.conf,localtime}`, dynamic-loader paths) — The allow-list diff is empty; a new path requires a reviewed allow-list change — `scripts/sup/trace.sh` + allow-list file
- **TST-SUP-003** (should; context (A builds it; B uses it)): **Enforcement profile:** ship an optional seccomp profile (no `execve` after start, no `ptrace`, `mount` etc.) and a Landlock wrapper (read-only everywhere except the output directory), and use them in CI runs of the image — Suite passes under the profile; a test binary that tries `execve` or writes outside is blocked — `contrib/seccomp.json`, `podman --security-opt`
- **TST-SUP-010** (must; A does the reduced run; B does the full scale): **Reproducible builds:** two builds in different directories, at different times and as different users, with `SOURCE_DATE_EPOCH`, `--remap-path-prefix`, `CARGO_INCREMENTAL=0`, the same toolchain image (by digest) → identical SHA-256 for the binary; images built with `--timestamp` must match too; embed the dependency list with `cargo auditable` — Bit-for-bit equal in CI — `scripts/sup/repro.sh`
- **TST-SUP-020** (must; A does the reduced run; B does the full scale): **Hostile compressed input:** cargo-fuzz targets for `.zst` and `.gz` decoding as used by `file_source` (truncated frames, huge declared window, skippable frames, concatenated frames, gzip multi-member, trailing garbage, decompression bombs at 1000:1). zstd decoding uses `window_log_max` ≤ 27 (or a configurable cap). The ruzstd decode path is benchmarked as an option — No crash, no out-of-memory beyond the cap, clear error and non-zero exit; 1 h nightly fuzz with no findings; ruzstd decision recorded — `fuzz/fuzz_targets/{zstd_in,gzip_in}.rs`
- **TST-SUP-022** (must; A does the reduced run; B does the full scale): **sonic-rs differential fuzz:** for arbitrary bytes, `sonic_rs::to_object_iter`/`get`/`to_array_iter` accept or reject as `serde_json` does, and return the same raw value spans (`LazyValue::as_raw_str`) on valid input. Run under ASan (`cargo +nightly fuzz -s address`) — 4 h nightly with no divergence or sanitizer report; any divergence is filed upstream and pinned — `fuzz/fuzz_targets/sonic_vs_serde.rs`
- **TST-INJ-015** (must; A does the reduced run; B does the full scale): **Fuzz targets** (cargo-fuzz, nightly 1 h each, seeds from the corpus): `fuzz_encode_doc` (oracle INJ-004); `fuzz_file_to_bulk` (bytes → `ValueSplitter` → `single_line` → `process_batch` → NDJSON oracle: lines == 2 × accepted docs and every target is in the allowed set); `fuzz_extract_batch` (search response with arbitrary whitespace and escapes inside `_source` → each output line is exactly one JSON value); `fuzz_url_segments`; `fuzz_rename_output` (every rename output either passes `validate_index_name` or is rejected); `fuzz_meta_names` — No crash and no oracle violation; findings become unit tests — `fuzz/`
