---
id: T-B0
plan_task: B0
lane: campaign
agent: runner
requirements: [B0]
goal: results/B0/result.json records host facts, tool versions, image digests, bundle SHA and the egress and memory-limit checks
depends_on: []
timeout_min: 30
campaign_timeout_min: 20
heavy_gb: 0
branch: card/T-B0
summary: results/cards/T-B0.md
files_allowed:
  - results/B0/result.json
  - results/B0/SUMMARY.md
files_readonly:
  - target/cards/T-B0/*
forbidden:
  - edit any file outside results/
  - rerun or change campaign commands; guess a value that is not in a log (write "unknown")
campaign:
  - mkdir -p "$ART" && { uname -a; nproc; lscpu | sed -n '1,20p'; free -m; df -h /; cat /sys/fs/cgroup/cgroup.controllers; } > "$ART/host.txt" 2>&1
  - { git rev-parse HEAD; git log -1 --format=%cI; } > "$ART/git.txt"
  - for t in "rustc --version" "cargo --version" "rustc +nightly --version" "cargo nextest --version" "cargo deny --version" "cargo vet --version" "cargo +nightly fuzz --version" "cargo geiger --version" "cargo +nightly udeps --version" "cargo machete --version" "gitleaks version" "podman --version" "jq --version" "zstd --version" "python3 --version" "opencode --version" "node --version"; do printf '%s => ' "$t"; $t 2>&1 | head -1; done > "$ART/tools.txt"
  - podman info --format '{{.Host.CgroupsVersion}} {{.Host.CgroupManager}} {{.Host.Security.Rootless}}' > "$ART/podman.txt" 2>&1
  - podman images --digests --format '{{.Repository}}:{{.Tag}} {{.Digest}}' > "$ART/images.txt" 2>&1
  - podman run --rm --memory 64m docker.io/library/alpine:3 cat /sys/fs/cgroup/memory.max > "$ART/memlimit.txt" 2>&1 || true
  - podman network exists edb-internal || podman network create --internal edb-internal
  - podman run --rm --network edb-internal docker.io/library/alpine:3 wget -T 5 -q -O- https://example.com > "$ART/egress.txt" 2>&1; echo "egress rc=$?" >> "$ART/egress.txt"
acceptance:
  - run: python3 -m json.tool results/B0/result.json >/dev/null
    exit: 0
    expect: valid JSON with schema edrs-result/1
  - run: jq -e '.task=="B0" and (.host.cores|type=="number") and (.git.head|length==40)' results/B0/result.json
    exit: 0
    expect: task, cores and head SHA filled from the logs
---
## Context
HANDOFF-TASKPLAN §6/B0: record host facts (CPU model, cores, RAM, disk, kernel, cgroup mode, podman version), tool
versions, image digests and the bundle SHA. Acceptance there: unit tests pass (the lead runs that), and from a container
on `edb-internal`, `curl/wget https://example.com` FAILS (egress blocked). Memory limits must be enforced
(`memory.max` = 67108864 for `--memory 64m`; on agent A's box it was `max`).

## Spec
1. Read every file in `target/cards/T-B0/` (`host.txt`, `git.txt`, `tools.txt`, `podman.txt`, `images.txt`,
   `memlimit.txt`, `egress.txt`, `campaign.log`).
2. Write `results/B0/result.json` (schema `edrs-result/1`, fields from HANDOFF-TASKPLAN §9): `task: "B0"`, `status`
   (`pass` only if egress rc ≠ 0 AND memlimit = 67108864; else `fail` with a finding), `host{cpu,cores,ram_gb,disk,
   kernel,container_runtime,cgroup}`, `git{bundle_base:"b4134af…" (full SHA from git.txt), head}`, `tools{name: version}`,
   `images[{name,digest}]`, `findings[]`, `notes`. Times in UTC.
3. Write `results/B0/SUMMARY.md`: a 10-line table (fact | value | ok?).
4. Unknown values are the string "unknown". Never invent a version.

## Output contract
One commit `T-B0: B0: environment facts` with the two results files and results/cards/T-B0.md.
