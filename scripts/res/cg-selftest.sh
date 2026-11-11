#!/usr/bin/env bash
# L-003 acceptance + BRK-RES-001 smoke under a REAL cgroup v2 memory cap (needs sudo; exit 77 otherwise).
#  1. cg-run.sh 64M python3 -c 'bytearray(200<<20)'  -> OOM-killed (137, oom_kill=1), no stray cgroup
#  2. searchdump file->file with 4 x 40-50 MB docs under 48M -> OOM-killed; the final output
#     file must NOT exist (atomic commit); the hidden staging file is reported (SIGKILL cannot clean it)
#  3. same under 400M -> completes, output identical to input
set -uo pipefail
cd "$(dirname "$0")/../.."
CG_MODE=cgroup scripts/res/cg-run.sh 1M true 2>/dev/null; [[ $? == 77 ]] && { echo "SKIP: no cgroup v2/sudo"; exit 77; }
fail=0; ok() { echo "PASS $*"; }; bad() { echo "FAIL $*"; fail=1; }
CG_MODE=cgroup scripts/res/cg-run.sh 64M python3 -c 'bytearray(200<<20)' 2>/tmp/cgst.err; rc=$?
[[ $rc == 137 ]] && grep -q 'oom_kill=1' /tmp/cgst.err && ok "python 200 MiB under 64M OOM-killed" || bad "python rc=$rc $(tail -1 /tmp/cgst.err)"
[[ ! -e /sys/fs/cgroup/edrs-test ]] && ok "no stray cgroup" || bad "stray /sys/fs/cgroup/edrs-test"
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
target/release/edgen gen --profile huge --count 4 --huge-min-mb 40 --huge-max-mb 50 --out "$W/in.ndjson" 2>/dev/null
( cd "$W" && CG_MODE=cgroup "$OLDPWD/scripts/res/cg-run.sh" 48M "$OLDPWD/target/release/searchdump" --input in.ndjson --output out.ndjson ) >/dev/null 2>/tmp/cgst.err; rc=$?
[[ $rc == 137 ]] && ok "searchdump under 48M OOM-killed" || bad "searchdump 48M rc=$rc"
[[ ! -e "$W/out.ndjson" ]] && ok "no final output after OOM kill (atomic commit)" || bad "partial final output exists"
stale=$(cd "$W" && ls -A | grep -c '^\.out\.ndjson\.part-' || true)
echo "INFO stale staging files left by SIGKILL: $stale"
( cd "$W" && CG_MODE=cgroup "$OLDPWD/scripts/res/cg-run.sh" 400M "$OLDPWD/target/release/searchdump" --input in.ndjson --output out2.ndjson ) >/dev/null 2>/tmp/cgst.err; rc=$?
[[ $rc == 0 ]] && cmp -s "$W/in.ndjson" "$W/out2.ndjson" && ok "under 400M completes, output identical ($(grep -o 'memory.peak=[0-9]*' /tmp/cgst.err))" || bad "400M rc=$rc"
[[ ! -e /sys/fs/cgroup/edrs-test ]] && ok "no stray cgroup at end" || bad "stray cgroup at end"
rm -f /tmp/cgst.err
exit $fail
