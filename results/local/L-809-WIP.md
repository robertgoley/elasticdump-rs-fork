# L-809 WIP: design only (no code yet)

Not started: usage was 89.4% at 2026-11-01 after L-808e; generous L-809 estimate 6 points,
89.4 + 1.5 × 6 = 98.4 is not under 98, so the task was not started (user usage rule).

Spec: TASKS-LOCAL L-809 / TEST-REQUIREMENTS LOAD-014, L1-lite, place box (tiers.toml).

Plan for `scripts/load/manyidx.py` (model on scripts/load/split.py; es717-plain via matrix.sh,
scripts/it/up.sh, lock /tmp/elasticdump-rs-it.lock, teardown `podman rm -f it-es-plain-$RUN_ID`
and `podman unshare rm -rf work`):
1. Create 300 indices `many-000`..`many-299` (1 shard, 0 replicas, edgen logs mapping), 100 docs each
   (`edgen gen --profile logs --seed N --count 100 --format bulk --index many-NNN`).
2. 100 aliases `a-000`..`a-099`, each over 3 indices, some with filters (one atomic POST /_aliases).
3. One wide index: `edgen mapping --profile wide --fields 5000` (total_fields.limit 5100), a few docs.
4. Wildcard dump (`$URL/many-*`, and the wide index) of types data, mapping, settings, index, alias;
   time each and sample peak RSS with allocbench.run_sampled (VmHWM). Baseline = time per meta type.
5. Verify data with `edverify diff --a URL/index --b file --out report.json`.
6. Delete, restore via host-only `--output` (settings → mapping → data → alias, and index type),
   edverify diff again, re-dump meta and compare JSON; round-trip the 5000-field mapping.
7. Leak check: `_nodes/stats/indices/search` scroll_current = 0, no `.part` files left.
8. Results: results/load/<run-id>/manyidx.json and results/local/L-809.md.
