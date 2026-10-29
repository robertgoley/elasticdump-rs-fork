#!/usr/bin/env bash
# SUP-006 (L-203): unsafe inventory with a baseline gate.
#   scripts/sup/geiger-gate.sh            # compare against the baseline
#   scripts/sup/geiger-gate.sh --update   # rewrite the baseline
# Needs cargo-geiger (0.14 builds with a nightly toolchain; L-002) and builds the
# graph in its own target dir. GEIGER_JSON=<file> reuses an existing report.
# Fails when: our crate stops forbidding unsafe, the total used-unsafe count
# rises, or any crate's used-unsafe count rises (new crates included).
set -euo pipefail
cd "$(dirname "$0")/../.."
BASE=supply-chain/geiger-baseline.json
TOOLCHAIN="${GEIGER_TOOLCHAIN:-+nightly}"
json="${GEIGER_JSON:-}"
if [[ -z "$json" ]]; then
  json=$(mktemp); trap 'rm -f "$json"' EXIT
  CARGO_TARGET_DIR="${GEIGER_TARGET_DIR:-${TMPDIR:-/tmp}/geiger-target}" \
    cargo "$TOOLCHAIN" geiger --output-format Json --target x86_64-unknown-linux-gnu >"$json"
fi
python3 - "$json" "$BASE" "${1:-}" <<'PY'
import json, sys
src, base, mode = sys.argv[1], sys.argv[2], sys.argv[3]
d = json.load(open(src))
cur, forbid = {}, None
for p in d["packages"]:
    pid = p["package"]["id"]; u = p["unsafety"]["used"]
    n = sum(v["unsafe_"] for v in u.values())
    name = pid["name"]
    cur[name] = cur.get(name, 0) + n
    if name == "elasticdump-rs":
        forbid = p["unsafety"]["forbids_unsafe"]
summary = {"total_used_unsafe": sum(cur.values()),
           "crates_with_unsafe": sum(1 for v in cur.values() if v),
           "elasticdump_rs_forbids_unsafe": forbid,
           "per_crate": dict(sorted(cur.items()))}
if mode == "--update":
    json.dump(summary, open(base, "w"), indent=1, sort_keys=False); open(base, "a").write("\n")
    print(f"baseline written: total={summary['total_used_unsafe']} crates={summary['crates_with_unsafe']}")
    sys.exit(0)
b = json.load(open(base))
bad = []
if forbid is not True:
    bad.append("elasticdump-rs no longer has #![forbid(unsafe_code)]")
for k, v in cur.items():
    old = b["per_crate"].get(k)
    if old is None and v:
        bad.append(f"new crate with unsafe: {k} ({v})")
    elif old is not None and v > old:
        bad.append(f"{k}: used unsafe {old} -> {v}")
t0, t1 = b["total_used_unsafe"], summary["total_used_unsafe"]
print(f"geiger: total used unsafe {t0} -> {t1}; crates with unsafe {b['crates_with_unsafe']} -> {summary['crates_with_unsafe']}")
if t1 > t0:
    bad.append(f"total used unsafe rose {t0} -> {t1}")
if bad:
    print("\n".join("FAIL " + x for x in bad)); sys.exit(1)
print("geiger gate: OK")
PY
