#!/usr/bin/env bash
# Convert a cached dataset to elasticdump format with deterministic ids
# (TST-DATA-005/010):  scripts/data/convert.sh <name> [--index NAME] [--max-docs N]
# Output: $ED_DATA_CACHE/converted/<name>.ndjson.zst (prints its SHA-256).
set -euo pipefail
cd "$(dirname "$0")/../.."
name="$1"; shift
index="$name"; max=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --index) index="$2"; shift 2;;
    --max-docs) max=(--max-docs "$2"); shift 2;;
    *) echo "unknown option $1" >&2; exit 2;;
  esac
done
EDGEN="${EDGEN:-target/release/edgen}"
[[ -x "$EDGEN" ]] || CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build -q --release -p edgen
cache="${ED_DATA_CACHE:-/var/tmp/searchdump-data}"
read -r conv comp < <(python3 - "$name" <<'PY'
import os, sys, tomllib
d = tomllib.load(open(os.environ.get("ED_DATASETS", "datasets.toml"), "rb"))["datasets"][sys.argv[1]]
print(d["converter"], d["compression"])
PY
)
ext=""; case "$comp" in bz2) ext=.bz2;; gz) ext=.gz;; zst) ext=.zst;; xz) ext=.xz;; zip) echo "$name: unpack the zip first" >&2; exit 2;; esac
src="$cache/$name$ext"
[[ -f "$src" ]] || { echo "$name: not cached; run scripts/data/fetch.sh $name" >&2; exit 1; }
mkdir -p "$cache/converted"
out="$cache/converted/$name.ndjson.zst"
file="$(basename "$src")"
if [[ "$comp" == bz2 || "$comp" == xz ]]; then
  python3 -c "import bz2,lzma,shutil,sys; f=(bz2.open if sys.argv[2]=='bz2' else lzma.open)(sys.argv[1],'rb'); shutil.copyfileobj(f, sys.stdout.buffer, 1<<20)" "$src" "$comp" \
    | "$EDGEN" convert --kind "$conv" --dataset "$name" --file "$file" --index "$index" "${max[@]}" --input - --out "$out" --compress zstd
else
  "$EDGEN" convert --kind "$conv" --dataset "$name" --file "$file" --index "$index" "${max[@]}" --input "$src" --out "$out" --compress zstd
fi
sha256sum "$out"
