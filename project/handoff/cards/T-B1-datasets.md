---
id: T-B1
plan_task: B1
lane: campaign
agent: runner
requirements: [TST-DATA-004, TST-DATA-005, TST-DATA-010]
goal: Every licensed dataset in datasets.toml is cached and checksummed, converted twice with equal SHA-256, and reported in results/B1/datasets.json
depends_on: [T-B0]
timeout_min: 40
campaign_timeout_min: 600
heavy_gb: 0
branch: card/T-B1
summary: results/cards/T-B1.md
files_allowed:
  - results/B1/datasets.json
  - results/B1/SUMMARY.md
  - results/B1/PIN-PROPOSAL.md
files_readonly:
  - datasets.toml
  - target/cards/T-B1/*
forbidden:
  - edit datasets.toml (the lead pins hashes after review)
  - fetch loghub_bgl (licence question Q9 open), ait_log_v2 (CC-BY-NC-SA, excluded), kaggle_web_access (needs an account)
campaign:
  - scripts/data/fetch.sh --list > "$ART/list.txt" 2>&1
  - scripts/data/fetch.sh http_logs_1k http_logs_181998 http_logs_191998 geonames nested noaa nginx_access_0 nasa_jul95 nasa_aug95 apache_logs_10k secrepo_2017_01_01 > "$ART/fetch-pinned.txt" 2>&1
  - scripts/data/fetch.sh --allow-unpinned http_logs_181998_osb_zst eventdata pmc nyc_taxis big5_60 > "$ART/fetch-unpinned.txt" 2>&1
  - scripts/data/fetch.sh --verify > "$ART/verify.txt" 2>&1
  - for n in http_logs_191998 geonames nested noaa pmc nyc_taxis big5_60 eventdata; do for r in 1 2; do echo "== $n run $r"; scripts/data/convert.sh "$n" 2>&1 | tail -3; done; done > "$ART/convert.txt" 2>&1
acceptance:
  - run: python3 -m json.tool results/B1/datasets.json >/dev/null
    exit: 0
    expect: valid JSON list
  - run: jq -e 'all(.[]; has("name") and has("url") and has("bytes") and has("sha256") and has("docs") and has("licence"))' results/B1/datasets.json
    exit: 0
    expect: one complete entry per dataset fetched
---
## Context
HANDOFF-TASKPLAN B1: download with resume, record sha256, convert with `scripts/data/convert.sh <name>` (deterministic
ids → `$ED_DATA_CACHE/converted/<name>.ndjson.zst`, prints its SHA-256), record doc counts. Expected counts:
http_logs-191998 = 9,697,882; nyc_taxis = 165,346,692; eventdata = 20,000,000. Converters must be deterministic
(two runs → equal SHA-256). Entries with `sha256 = ""` in datasets.toml are unpinned (trust on first use).
Set `ED_DATA_CACHE` to a disk with ≥ 150 GB free before the run (default /var/tmp/elasticdump-rs-data).

## Spec
1. From the logs, build `results/B1/datasets.json`: `[{name, url, bytes, sha256, docs, licence, pinned, convert_sha256_run1,
   convert_sha256_run2, deterministic, status}]`. url/licence come from datasets.toml.
2. `deterministic=false` or a count that differs from the expected numbers above is a finding (`status: "fail"`).
3. `results/B1/PIN-PROPOSAL.md`: for each unpinned entry that downloaded completely, the exact
   `sha256 = "<hash>"` line the lead should put in datasets.toml, with bytes. Do not edit datasets.toml.
4. Note in SUMMARY.md which B3 corpora are missing from the manifest (big5-100, full http_logs): the lead adds them.

## Output contract
One commit `T-B1: TST-DATA-004 TST-DATA-005 TST-DATA-010: datasets cached and converted` with the three results files
and results/cards/T-B1.md.
