#!/usr/bin/env bash
# Reference Claude Code SessionStart hook.
# Atomically drains $AGENTBUS_INBOX_DIR/<INSTANCE>.jsonl and emits each
# message's `payload` field as additionalContext lines on stdout.
set -euo pipefail

INSTANCE=${AGENTBUS_INSTANCE:?AGENTBUS_INSTANCE required}
INBOX_DIR=${AGENTBUS_INBOX_DIR:-${XDG_RUNTIME_DIR:-/tmp}/agentbus/inbox}
SRC="$INBOX_DIR/$INSTANCE.jsonl"
[ -f "$SRC" ] || exit 0

WORK="$INBOX_DIR/$INSTANCE.processing.$$"
mv "$SRC" "$WORK" 2>/dev/null || exit 0

while IFS= read -r line; do
  [ -z "$line" ] && continue
  payload=$(printf '%s' "$line" \
    | python3 -c "import sys,json;print(json.dumps(json.loads(sys.stdin.read()).get('payload', {})))")
  printf 'agentbus inbox: %s\n' "$payload"
done < "$WORK"

rm -f "$WORK"
