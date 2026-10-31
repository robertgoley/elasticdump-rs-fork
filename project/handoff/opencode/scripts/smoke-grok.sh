#!/usr/bin/env bash
# smoke-grok.sh — check OpenCode + xAI Grok with one tiny run in a scratch dir (no repo changes).
#   OC_MODEL=xai/grok-4.7 smoke-grok.sh
# Never prints the API key: it only reports whether XAI_API_KEY is set and lists stored providers.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
MODEL="${OC_MODEL:-xai/grok-4.7}"
command -v opencode >/dev/null || { echo "smoke: opencode not installed (https://opencode.ai/docs)" >&2; exit 4; }
echo "== opencode $(opencode --version 2>/dev/null || echo '?')"
echo "== auth: XAI_API_KEY $([[ -n "${XAI_API_KEY:-}" ]] && echo set || echo unset); stored providers:"
opencode auth list 2>/dev/null | grep -i -o 'xai[^ ]*\|grok[^ ]*' | sort -u | sed 's/^/   /' || echo "   (none listed; run: opencode auth login, pick xAI)"
echo "== models from xai"
if opencode models xai 2>/dev/null | grep -qxF "$MODEL"; then echo "   $MODEL listed"
else
  echo "   $MODEL NOT listed. Available xai models:" >&2
  opencode models xai 2>/dev/null | sed 's/^/   /' >&2 || true
  echo "   set OC_MODEL and the model ids in opencode.json to one of these" >&2; exit 4
fi
echo "== opencode run (runner agent, temp dir)"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
( cd "$T" && git init -q && echo hi > a.txt && git add a.txt && git -c user.email=x@y -c user.name=x commit -qm init )
OPENCODE_CONFIG="$HERE/../opencode.json" OPENCODE_DISABLE_AUTOUPDATE=1 \
  OPENCODE_DB="$T/oc.db" timeout 300 opencode run --agent runner -m "$MODEL" \
  --format json --dir "$T" "Read a.txt and reply with its exact content only." \
  > "$T/events.jsonl" 2> "$T/stderr" || { echo "smoke: opencode run failed (stderr tail below)" >&2; tail -20 "$T/stderr" | sed -E 's/(xai-|sk-)[A-Za-z0-9_-]{8,}/\1<redacted>/g' >&2; exit 1; }
jq -r 'select(.type=="text") | .part.text' "$T/events.jsonl"
jq -r 'select(.type=="tool_use") | "tool used: \(.part.tool)"' "$T/events.jsonl"
echo "smoke: done (expect the text 'hi' and a 'read' tool use)"
