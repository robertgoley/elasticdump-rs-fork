#!/usr/bin/env bash
# INF-010: gather failure artefacts into artifacts/<name>/ for upload:
# tool logs ($IT_DIR/*.log), container state + logs, metrics JSON, load digests
# and golden diffs. Never fails the job itself.
set -uo pipefail
name="${1:-run}"
out="artifacts/$name"
mkdir -p "$out"
IT_DIR="${IT_DIR:-$HOME/.cache/edrs-it}"
cp "$IT_DIR"/*.log "$out/" 2>/dev/null
podman ps -a --format '{{.Names}} {{.Image}} {{.Status}}' > "$out/containers.txt" 2>&1
for c in $(podman ps -aq 2>/dev/null); do
  n=$(podman inspect --format '{{.Name}}' "$c" 2>/dev/null || echo "$c")
  podman logs --tail 500 "$c" > "$out/podman-$n.log" 2>&1
done
# metrics (L-306), load digests and per-step metrics (L-314), golden diffs (L-303)
[[ -d results ]] && find results -maxdepth 3 -name '*.json' -newer Cargo.toml -exec cp --parents {} "$out/" \; 2>/dev/null
[[ -d target/golden-diff ]] && cp -r target/golden-diff "$out/" 2>/dev/null
[[ -d target/nextest ]] && find target/nextest -name junit.xml -exec cp --parents {} "$out/" \; 2>/dev/null
echo "collected $(find "$out" -type f | wc -l) files into $out"
exit 0
