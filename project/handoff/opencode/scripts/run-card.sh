#!/usr/bin/env bash
# run-card.sh — run one task card with OpenCode + Grok 4.7 in its own git worktree, then gate it.
#
#   run-card.sh <card.md> [--retry N --feedback FILE] [--no-gate] [--dry-run] [--force]
#
# 1. checks the card (card.py check), the plan task is not already done (PROGRESS_FILE or an 'L-NNN:' commit on BASE),
#    and every depends_on ID has a commit "<ID>: ..." on $BASE;
# 2. takes one of PAR_MAX slots (flock), creates worktree $WT_ROOT/<id> on branch card/<id>
#    from $BASE (a retry keeps the rejected attempt as card/<id>-rejected-<N-1>);
# 3. lane=campaign: runs the card's campaign commands first (under scripts/res/heavy.sh when
#    heavy_gb > 0), logs in target/cards/<id>/ inside the worktree (git-ignored);
# 4. runs `opencode run --agent <agent> --format json` with the card (+ feedback) as the prompt;
#    events → target/cards/<id>/events.jsonl, session export → session.json;
# 5. runs gate.sh --card (unless --no-gate) → target/cards/<id>/gate.txt.
# Exit: 0 gate pass · 3 already done · 4 dependency missing · 10 gate fail · 11 no commit ·
#       5 lead card (not run headless) · 12 campaign failed (runner still summarises) · 124 timeout.
#
# Env (defaults): REPO=$HOME/src/elasticdump-rs  BASE=rewrite  WT_ROOT=$REPO-wt
#   OC_MODEL=xai/grok-4.7  PAR_MAX=2  CARGO_BUILD_JOBS=4
#   HANDOFF=<dir above opencode/>  PROGRESS_FILE=$HANDOFF/plan/PROGRESS.md
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
HANDOFF="${HANDOFF:-$(cd "$HERE/../.." && pwd)}"
OC_DIR="$HANDOFF/opencode"
REPO="${REPO:-$HOME/src/elasticdump-rs}"
BASE="${BASE:-rewrite}"
WT_ROOT="${WT_ROOT:-$REPO-wt}"
OC_MODEL="${OC_MODEL:-xai/grok-4.7}"
PAR_MAX="${PAR_MAX:-2}"
PROGRESS_FILE="${PROGRESS_FILE:-$HANDOFF/plan/PROGRESS.md}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"

CARD="" RETRY=0 FEEDBACK="" GATE=1 DRY=0 FORCE=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --retry) RETRY="$2"; shift 2 ;;
    --feedback) FEEDBACK="$(realpath "$2")"; shift 2 ;;
    --no-gate) GATE=0; shift ;;
    --dry-run) DRY=1; shift ;;
    --force) FORCE=1; shift ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) CARD="$(realpath "$1")"; shift ;;
  esac
done
[[ -f "$CARD" ]] || { echo "usage: run-card.sh <card.md> [...]" >&2; exit 2; }
C() { "$HERE/card.py" "$@"; }
C check "$CARD"
ID="$(C get "$CARD" id)"; AGENT="$(C get "$CARD" agent)"; LANE="$(C get "$CARD" lane)"
TMO="$(C get "$CARD" timeout_min)"; PLAN="$(C get "$CARD" plan_task)"
HEAVY="$(C get "$CARD" heavy_gb)"; CTMO="$(C get "$CARD" campaign_timeout_min)"
BR="card/$ID"; WT="$WT_ROOT/$ID"; ART_REL="target/cards/$ID"

# 1. already done on A? (an L-task row in the owner's latest PROGRESS.md)
if [[ "$PLAN" =~ ^L-[0-9A-Z]+$ && -f "$PROGRESS_FILE" ]] && grep -qE "^\| *${PLAN}[ /|]" "$PROGRESS_FILE"; then
  echo "run-card: $PLAN already has a row in $PROGRESS_FILE (done on A); not running $ID" >&2
  [[ $FORCE == 1 ]] || exit 3
fi
if [[ "$PLAN" =~ ^L-[0-9A-Z]+$ ]] && [[ -n "$(git -C "$REPO" log "$BASE" -n1 --format=%h -E --grep="^${PLAN}:")" ]]; then
  echo "run-card: $BASE already has a commit '${PLAN}: ...' (done on A); not running $ID" >&2
  [[ $FORCE == 1 ]] || exit 3
fi
for dep in $(C list "$CARD" depends_on); do
  if [[ -z "$(git -C "$REPO" log "$BASE" -n1 --format=%h -E --grep="^$dep(:| )")" ]]; then
    echo "run-card: dependency $dep has no commit '$dep: ...' on $BASE" >&2
    [[ $FORCE == 1 ]] || exit 4
  fi
done
if [[ "$AGENT" == lead ]]; then echo "run-card: $ID is a lead card; do it in the interactive lead session (opencode --agent lead)" >&2; exit 5; fi
if [[ $DRY == 1 ]]; then echo "dry-run ok: $ID agent=$AGENT lane=$LANE branch=$BR worktree=$WT"; exit 0; fi

# 2. slot (max PAR_MAX opencode instances at once), worktree, branch
SLOT=""
while [[ -z "$SLOT" ]]; do
  for i in $(seq 1 "$PAR_MAX"); do
    exec {fd}>"/tmp/edrs-oc-slot-$i.lock"
    if flock -n "$fd"; then SLOT=$i; break; fi
    exec {fd}>&-
  done
  [[ -n "$SLOT" ]] || { echo "run-card: all $PAR_MAX slots busy; waiting" >&2; sleep 30; }
done
echo "run-card: $ID on slot $SLOT"
mkdir -p "$WT_ROOT"
if git -C "$REPO" rev-parse -q --verify "refs/heads/$BR" >/dev/null; then
  [[ "$RETRY" -gt 0 ]] || { echo "run-card: branch $BR exists; use --retry N --feedback FILE" >&2; exit 2; }
  [[ -d "$WT" ]] && git -C "$REPO" worktree remove --force "$WT"
  git -C "$REPO" branch -m "$BR" "$BR-rejected-$((RETRY - 1))"
fi
git -C "$REPO" worktree add -q -b "$BR" "$WT" "$BASE"
EXCL="$(git -C "$REPO" rev-parse --git-common-dir)/info/exclude"
mkdir -p "$(dirname "$EXCL")"; grep -qxF '/AGENTS.md' "$EXCL" 2>/dev/null || echo '/AGENTS.md' >>"$EXCL"
cp "$OC_DIR/AGENTS.md" "$WT/AGENTS.md"
ART="$WT/$ART_REL"; mkdir -p "$ART"
cd "$WT"

# 3. campaign commands (deterministic; not run by the model)
CAMP_RC=0
if [[ "$LANE" == campaign ]]; then
  : > "$ART/campaign.log"
  while IFS= read -r cmd; do
    [[ -z "$cmd" ]] && continue
    echo "+ $cmd" | tee -a "$ART/campaign.log"
    wrap=(bash -o pipefail -c "$cmd")
    [[ -n "$HEAVY" && "$HEAVY" != 0 ]] && wrap=(scripts/res/heavy.sh "$HEAVY" "${wrap[@]}")
    if ! ART="$ART_REL" timeout --kill-after=120 "${CTMO:-600}m" "${wrap[@]}" </dev/null >>"$ART/campaign.log" 2>&1; then
      CAMP_RC=1; echo "campaign step failed: $cmd" | tee -a "$ART/campaign.log"
    fi
  done < <(C list "$CARD" campaign)
  echo "$CAMP_RC" > "$ART/campaign.rc"
fi

# 4. the model run
PROMPT="$ART/prompt.md"
{
  cat "$CARD"
  echo; echo "---"; echo "Runtime facts: worktree=$WT branch=$BR base=$BASE artefacts=$ART_REL (git-ignored; read-only logs)."
  [[ "$LANE" == campaign ]] && echo "Campaign exit status: $CAMP_RC (0 = every step exited 0). Log: $ART_REL/campaign.log"
  if [[ -n "$FEEDBACK" ]]; then echo; echo "## Reviewer feedback from attempt $RETRY (fix these; same card)"; cat "$FEEDBACK"; fi
} > "$PROMPT"
# per-card edit allow-list for impl (relative and absolute forms)
INLINE="$(python3 - "$CARD" "$ID" "$HERE" <<'PY'
import json, subprocess, sys
card, cid, here = sys.argv[1:4]
files = subprocess.run([here + "/card.py", "list", card, "files_allowed"], capture_output=True, text=True).stdout.split()
rules = {"*": "deny"}
for f in files + [f"results/cards/{cid}.md"]:
    rules[f] = "allow"; rules["*/" + f] = "allow"
print(json.dumps({"agent": {"impl": {"permission": {"edit": rules}},
                            "runner": {"permission": {"edit": rules}}}}))
PY
)"
mkdir -p "$WT_ROOT/.opencode-db"
export OPENCODE_CONFIG="$OC_DIR/opencode.json"
export OPENCODE_CONFIG_CONTENT="$INLINE"
export OPENCODE_DB="$WT_ROOT/.opencode-db/$ID.db"
export OPENCODE_DISABLE_AUTOUPDATE=1 OPENCODE_DISABLE_CLAUDE_CODE=1
RC=0
timeout --kill-after=60 "${TMO}m" opencode run --agent "$AGENT" -m "$OC_MODEL" --format json \
  --title "$ID" --dir "$WT" "$(cat "$PROMPT")" >"$ART/events.jsonl" 2>"$ART/opencode.stderr" || RC=$?
SID="$(jq -r 'select(.sessionID != null) | .sessionID' "$ART/events.jsonl" 2>/dev/null | head -1 || true)"
if [[ -n "$SID" ]]; then opencode export "$SID" >"$ART/session.json" 2>/dev/null || true; fi
jq -s '[.[] | select(.type=="step_finish") | .part.tokens] | {steps: length,
        input: (map(.input // 0) | add), output: (map(.output // 0) | add)}' "$ART/events.jsonl" \
  >"$ART/tokens.json" 2>/dev/null || true
jq -r 'select(.type=="error") | .error | tostring' "$ART/events.jsonl" 2>/dev/null >"$ART/errors.txt" || true
[[ $RC == 124 ]] && { echo "run-card: $ID timed out after ${TMO}m (artefacts: $ART)"; exit 124; }
N="$(git rev-list --count "$BASE"..HEAD)"
[[ "$N" -ge 1 ]] || { echo "run-card: $ID produced no commit (opencode rc $RC; see $ART)"; exit 11; }

# 5. gate
if [[ $GATE == 1 ]]; then
  if "$HERE/gate.sh" --card "$CARD" --base "$BASE" | tee "$ART/gate.txt"; then G=0; else G=10; fi
else G=0; fi
echo "run-card: $ID done: branch $BR, $N commit(s), opencode rc $RC, campaign rc $CAMP_RC, gate $([[ $G == 0 ]] && echo pass || echo FAIL)"
echo "  review (fresh session): cd $WT && opencode run --agent review \"Review git diff rewrite...$BR per HANDOFF §7\""
echo "  inspect: git -C $WT show --stat HEAD; cat $ART/gate.txt $WT/results/cards/$ID.md"
[[ $G == 0 && $CAMP_RC == 1 ]] && exit 12
exit $G
