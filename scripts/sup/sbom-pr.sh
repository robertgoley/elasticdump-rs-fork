#!/usr/bin/env bash
# TST-SUP-011 / L-207: SBOM + build-time inventory diff of HEAD against a base ref.
#   scripts/sup/sbom-pr.sh <base-ref> [out-dir]     exit 0 = empty diff, 10 = non-empty (needs review)
# The base is checked out into a temporary git worktree (removed afterwards).
# Image packages: set IMAGE_SBOM_BASE / IMAGE_SBOM_HEAD (syft CycloneDX JSON) to include them.
set -uo pipefail
cd "$(dirname "$0")/../.."
BASE="${1:?usage: sbom-pr.sh <base-ref> [out-dir]}"; OUT="${2:-target/sbom-diff}"
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
WT="$(mktemp -d)"; trap 'git worktree remove --force "$WT" >/dev/null 2>&1; rm -rf "$WT"' EXIT
git worktree add --detach "$WT" "$BASE" >/dev/null 2>&1 || { echo "cannot check out $BASE"; exit 2; }
gen() {  # <dir> <name>
  ( cd "$1" && cargo cyclonedx --format json --target x86_64-unknown-linux-gnu --override-filename "sbom-$2" -q ) &&
    find "$1" -maxdepth 1 -name "sbom-$2*.json" -exec mv {} "$OUT/$2.cdx.json" \;
}
gen "$WT" base && gen "$PWD" head || exit 2
scripts/sup/build-inventory.py --dump "$PWD/Cargo.toml" > "$OUT/inv-head.json"
scripts/sup/build-inventory.py --dump "$WT/Cargo.toml" > "$OUT/inv-base.json"
args=(--inv-base "$OUT/inv-base.json" --inv-head "$OUT/inv-head.json")
[[ -n "${IMAGE_SBOM_BASE:-}" && -n "${IMAGE_SBOM_HEAD:-}" ]] && args+=(--image-base "$IMAGE_SBOM_BASE" --image-head "$IMAGE_SBOM_HEAD")
scripts/sup/sbom-diff.py "$OUT/base.cdx.json" "$OUT/head.cdx.json" "${args[@]}" --json "$OUT/diff.json" | tee "$OUT/diff.md"
exit "${PIPESTATUS[0]}"
