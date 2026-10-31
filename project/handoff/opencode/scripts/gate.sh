#!/usr/bin/env bash
# gate.sh — the lead's merge gate for one card branch (or any branch), run inside its worktree.
#
#   gate.sh [--card CARD.md] [--base rewrite] [--quick]
#
# Steps (hard = blocks merge):
#   policy    hard  one commit ahead of base, clean tree, files ⊆ card's files_allowed,
#                   no forbidden paths, summary file present
#   scan      hard  no added `unsafe`, `#[allow`, `#![allow`, `#[ignore`; no new [dependencies]
#             warn  new URLs / sockets / Command in src/, removed asserts, removed #[test]
#   fmt       hard  cargo fmt --all -- --check
#   build     hard  cargo build -q (plain default features; catches test-only feature unification)
#   clippy    hard  clippy --workspace --lib --bins --tests --features load,it -D warnings
#   unit      hard  cargo nextest run --profile unit
#   contract  hard  cargo nextest run --profile contract
#   chaos     hard  cargo nextest run --profile chaos   (only if tests/chaos* changed, or GATE_CHAOS=1)
#   accept    hard  the card's acceptance commands, expected exit codes
#   deny      hard  cargo deny check advisories bans licenses sources  (CARGO_DENY_OFFLINE=1 → --disable-fetch)
#   geiger    hard  scripts/sup/geiger-gate.sh   (skipped when no .rs/Cargo.* changed; --quick skips)
#   secrets   hard  scripts/sup/secret-scan.sh <base>..HEAD   (gitleaks + canary suite)
#   cleantree hard  scripts/check-clean-tree.sh
#   hygiene   hard  scripts/lint-hygiene.sh
# A branch that changes only results/** skips fmt..geiger (GATE_FULL=1 runs them anyway).
# Output: one line per step on stdout; full log in target/cards/<id>/gate.log; exit 0 only if all
# hard steps pass. Never modifies tracked files.
# shellcheck disable=SC2317  # step functions are invoked indirectly through step()
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
CARD="" BASE="${BASE:-rewrite}" QUICK=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --card) CARD="$(realpath "$2")"; shift 2 ;;
    --base) BASE="$2"; shift 2 ;;
    --quick) QUICK=1; shift ;;
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) echo "gate: unknown arg $1" >&2; exit 2 ;;
  esac
done
cd "$(git rev-parse --show-toplevel)" || exit 2
# shellcheck disable=SC1090,SC1091
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
ID="$(git rev-parse --abbrev-ref HEAD | sed 's#^card/##; s#/#_#g')"
[[ -n "$CARD" ]] && ID="$("$HERE/card.py" get "$CARD" id)"
ART="target/cards/$ID"; mkdir -p "$ART"; LOG="$ART/gate.log"; : > "$LOG"
FAIL=0; WARN=0
step() {  # step <name> <hard|warn> <cmd...>
  local name="$1" kind="$2"; shift 2
  local t0=$SECONDS rc
  { echo "=== $name: $*"; } >>"$LOG"
  "$@" >>"$LOG" 2>&1; rc=$?
  if [[ $rc -eq 0 ]]; then printf 'PASS %-9s %4ss\n' "$name" $((SECONDS - t0))
  elif [[ $kind == warn ]]; then printf 'WARN %-9s %4ss (rc %s)\n' "$name" $((SECONDS - t0)) "$rc"; WARN=1
  else printf 'FAIL %-9s %4ss (rc %s, see %s)\n' "$name" $((SECONDS - t0)) "$rc" "$LOG"; FAIL=1
  fi
  return 0
}
CHANGED="$(git diff --name-only "$BASE"...HEAD)"

policy() {
  local rc=0 n f
  [[ -z "$(git status --porcelain --untracked-files=no)" ]] || { echo "uncommitted tracked changes"; rc=1; }
  n=$(git rev-list --count "$BASE"..HEAD)
  if [[ "$(git rev-parse --abbrev-ref HEAD)" == card/* && "$n" -ne 1 ]]; then
    echo "expected exactly 1 commit ahead of $BASE, found $n"; rc=1
  fi
  local forbidden='^(Cargo\.toml|Cargo\.lock|deny\.toml|supply-chain/|\.github/|\.config/|build\.rs|.*/build\.rs|tests/golden/|images\.lock|scripts/it/images\.lock|oom|test_query_elasticdump_rs_test_857109_16\.json)$'
  while IFS= read -r f; do
    [[ -z "$f" ]] && continue
    if [[ "$f" =~ $forbidden ]] || [[ "$f" == supply-chain/* || "$f" == .github/* || "$f" == .config/* || "$f" == tests/golden/* ]]; then
      if ! grep -qxF "$f" <(card_allowed); then echo "forbidden path changed: $f"; rc=1; fi
    fi
  done <<<"$CHANGED"
  if [[ -n "$CARD" ]]; then
    while IFS= read -r f; do
      [[ -z "$f" ]] && continue
      allowed_match "$f" || { echo "not in files_allowed: $f"; rc=1; }
    done <<<"$CHANGED"
    grep -qxF "results/cards/$ID.md" <<<"$CHANGED" || { echo "missing results/cards/$ID.md"; rc=1; }
  fi
  return "$rc"
}
card_allowed() { [[ -n "$CARD" ]] && "$HERE/card.py" list "$CARD" files_allowed; echo "results/cards/$ID.md"; }
allowed_match() {  # exact path, or a glob pattern from files_allowed (e.g. results/B1/*)
  local f="$1" p
  while IFS= read -r p; do
    [[ -z "$p" ]] && continue
    # shellcheck disable=SC2053
    [[ "$f" == "$p" || "$f" == $p ]] && return 0
  done < <(card_allowed)
  return 1
}

scan() {
  local rc=0 added removed
  added="$(git diff -U0 "$BASE"...HEAD -- '*.rs' | grep -E '^\+[^+]' || true)"
  removed="$(git diff -U0 "$BASE"...HEAD -- '*.rs' | grep -E '^-[^-]' || true)"
  if grep -nE '\bunsafe\b' <<<"$added" | grep -vE '^\S*\+\s*//' ; then echo "HARD: added unsafe"; rc=1; fi
  if grep -nE '#!?\[allow\(' <<<"$added"; then echo "HARD: added #[allow(...)]"; rc=1; fi
  if grep -nE '#\[ignore' <<<"$added"; then echo "HARD: added #[ignore] (only allowed when the card says so)"; [[ "${GATE_ALLOW_IGNORE:-0}" == 1 ]] || rc=1; fi
  if [[ -n "$(git diff --name-only "$BASE"...HEAD -- '*Cargo.toml' '*Cargo.lock')" ]]; then echo "HARD: Cargo.toml changed"; rc=1; fi
  # warnings (lead + review agent check by hand)
  local srcadd
  srcadd="$(git diff -U0 "$BASE"...HEAD -- 'src/*.rs' 'src/**/*.rs' | grep -E '^\+[^+]' || true)"
  grep -nE 'https?://' <<<"$srcadd" && echo "WARN: URL literal added in src/"
  grep -nE 'TcpStream|UdpSocket|TcpListener|reqwest::Client(Builder)?::new|std::process::Command|std::env::set_var' <<<"$srcadd" && echo "WARN: network/process/env API added in src/"
  grep -nE '(println|eprintln|info|debug|warn|error|trace)!\(.*(password|passwd|token|secret|api_key|authorization)' -i <<<"$added" && echo "WARN: log line mentions a secret-like name"
  local a r
  a=$(grep -cE 'assert' <<<"$added" || true); r=$(grep -cE 'assert' <<<"$removed" || true)
  (( r > a )) && echo "WARN: net assertions removed ($r removed, $a added)"
  grep -nE '#\[(tokio::)?test' <<<"$removed" && echo "WARN: a test attribute was removed"
  return "$rc"
}
scan_warn() { local out; out="$(scan 2>&1)"; if grep '^WARN' <<<"$out"; then return 1; fi; return 0; }

accept() {
  local rc=0 exp cmd got
  [[ -z "$CARD" ]] && { echo "no card: skipped"; return 0; }
  while IFS=$'\t' read -r exp cmd; do
    [[ -z "$cmd" ]] && continue
    echo "--- accept: $cmd (expect exit $exp)"
    bash -c "$cmd"; got=$?
    if [[ "$got" != "$exp" ]]; then echo "accept FAIL: exit $got != $exp: $cmd"; rc=1; fi
  done < <("$HERE/card.py" acceptance "$CARD")
  return "$rc"
}

rs_changed() { grep -qE '\.rs$|(^|/)Cargo\.(toml|lock)$' <<<"$CHANGED"; }
deny_cmd() { if [[ "${CARGO_DENY_OFFLINE:-0}" == 1 ]]; then cargo deny check --disable-fetch advisories bans licenses sources; else cargo deny check advisories bans licenses sources; fi; }

echo "gate: $(git rev-parse --short HEAD) on $(git rev-parse --abbrev-ref HEAD) vs $BASE (card: ${CARD:-none})"
step policy   hard policy
step scan     hard scan
step scanwarn warn scan_warn
RESULTS_ONLY=0
if [[ -n "$CHANGED" ]] && ! grep -qvE '^results/' <<<"$CHANGED"; then RESULTS_ONLY=1; fi
if [[ $RESULTS_ONLY == 1 && "${GATE_FULL:-0}" != 1 ]]; then
  echo "SKIP fmt/build/clippy/unit/contract/deny/geiger (only results/ changed; GATE_FULL=1 forces them)"
else
step fmt      hard cargo fmt --all -- --check
step build    hard cargo build -q
step clippy   hard cargo clippy -q --workspace --lib --bins --tests --features elasticdump-rs/load,elasticdump-rs/it -- -D warnings
step unit     hard cargo nextest run --profile unit
step contract hard cargo nextest run --profile contract
if [[ "${GATE_CHAOS:-0}" == 1 ]] || grep -qE '^tests/chaos' <<<"$CHANGED"; then
  step chaos  hard cargo nextest run --profile chaos
fi
step deny     hard deny_cmd
if [[ $QUICK == 0 ]] && rs_changed; then step geiger hard scripts/sup/geiger-gate.sh; else echo "SKIP geiger    (quick or no Rust/Cargo change)"; fi
fi
step accept   hard accept
step secrets  hard scripts/sup/secret-scan.sh "$BASE..HEAD"
step cleantree hard scripts/check-clean-tree.sh
step hygiene  hard scripts/lint-hygiene.sh
if [[ $FAIL == 0 ]]; then echo "GATE PASS ($ID)$([[ $WARN == 1 ]] && echo ', with warnings: review them')"; else echo "GATE FAIL ($ID): $LOG"; fi
exit $FAIL
