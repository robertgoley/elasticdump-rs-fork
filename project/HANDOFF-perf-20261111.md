# Handoff: v1.0.0-beta1 release + full perf suite (2026-11-11)

See the index in [HANDOFF.md](HANDOFF.md).

## State
- main: release fixes L-810g (e1a0f8e), DATA_FLAGS override (f38af3f), L-810h (94e4afb).
- Tag v1.0.0-beta1 -> 94e4afb. release-build run 43077974616 green; DRAFT pre-release with
  4 archives (+.sha256), CycloneDX SBOM (+.sha256), SHA256SUMS. ghcr.io/robertgoley/searchdump-rs tags:
  1.0.0-beta1-{dhi-debian,dhi-static,dhi-alpine,public,alpine}, beta. Anonymous pull works (public).
- Perf suite driver: /workspace/perf-b1/run-all.sh (resumable via steps/<engine>.done; `touch STOP`
  stops it between engines). Env /workspace/perf-b1/env.sh:
  - podman wrapper /workspace/perf-b1/bin/podman forces the ISOLATED store
    (~/.local/share/edrs-odt-podman). CONTAINERS_STORAGE_CONF is NOT honoured on this box.
  - XDG_RUNTIME_DIR=/tmp/xdg-runtime-perf-b1; PERF_ENGINE_PORT=19232 (19222 is held by the
    uid 100999 OpenSearch java: do not touch it).
  - PERF_BIN_DIR=/workspace/perf-bin-beta1: release x86_64-gnu binaries from run 43077974616 plus
    locally built edgen/edverify; hashes in /workspace/perf-b1/bin-sha256.txt.
  - PERF_HOST_ID=grokbox-beta1 (old box baselines have another identity, so cases show NO_BASELINE;
    aggregate.py compares against them instead).
- Progress: /workspace/perf-b1/summary.txt; per-engine logs in /workspace/perf-b1/logs/.

## Resume
    cd /workspace/perf-b1 && setsid nohup ./run-all.sh > driver.out 2>&1 < /dev/null &
    # when done:
    cd /workspace/fix-wt && python3 scripts/perf/aggregate.py --title "v1.0.0-beta1 L0 perf suite" \
      --old-baselines 'scripts/perf/baselines/grok-bot-vm-417583633-box-L0-*.json' \
      $(sed -n 's/.*dir=//p' /workspace/perf-b1/steps/*.done) > scripts/perf/results/v1.0.0-beta1-L0.md

## Findings so far
- Old "rerun4 OS 2/3 4g" failures: every case failed in about 1 s because port 19222 was taken by a
  leftover engine. suite.py now fails fast with a clear message and supports PERF_ENGINE_PORT
  (1f064e9). os2-plain at 4g heap now passes all 37 cases.
- **Port leak during the beta1 suite (2026-11-11).** After the ES 7.17 plain run, its container did not
  shut down and kept holding port 19232. The remaining 13 engines each failed in about 1 s (status
  `FAIL: setup/seed interrupted`). The hourly watch cleared the leftover and restarted from ES 7.17 TLS.
  A later leftover from elasticsearch8-plain / elasticsearch8-secure caused the same fast-fail; the
  watch cleared it and restarted elasticsearch8-secure. Do not treat a run of `.done` files as proof
  the cases actually ran (see below).
- searchdump-multi `--` extras now replace same-named DATA_FLAGS defaults (f38af3f).
- ES 7.10: sliced scroll s2 is slower than unsliced (engine-side slicing on _id); informational.

## Verify the suite really ran

`.done` files and a `rc=0` line in `/workspace/perf-b1/summary.txt` are not enough: a leftover
engine on the test port produces a result directory and a failed `combined.json` in about a second,
and a later successful retry is what should be counted. Before aggregating or declaring an engine
done:

1. In that engine's `summary.md` / `combined.json`, **count PASS rows** (and SKIP), not `.done` files.
   A setup failure marks every case `FAIL: setup/seed interrupted` with no wall times — that is not a
   completed run.
2. Confirm the result directory in `summary.txt` is the successful retry, not the one-second
   fast-fail (the successful ES 7.17 TLS and ES 8 TLS dirs are the later timestamps).
3. Then run `scripts/perf/aggregate.py` on those result dirs only.
