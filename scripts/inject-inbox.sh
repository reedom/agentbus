#!/usr/bin/env bash
# Reference Claude Code SessionStart hook.
# Atomically drains $AGENTBUS_INBOX_DIR/<INSTANCE>.jsonl and emits each
# message's `payload` field as additionalContext lines on stdout.
set -euo pipefail

INSTANCE=${AGENTBUS_INSTANCE:?AGENTBUS_INSTANCE required}
INBOX_DIR=${AGENTBUS_INBOX_DIR:-$HOME/.agentbus/inbox}
SRC="$INBOX_DIR/$INSTANCE.jsonl"

# Fail before the rename when the decoder is unavailable: the spool stays in
# place untouched instead of being stranded mid-drain.
command -v python3 >/dev/null || { echo "inject-inbox.sh: python3 required" >&2; exit 1; }
[ -f "$SRC" ] || exit 0

WORK="$INBOX_DIR/$INSTANCE.processing.$$"
mv "$SRC" "$WORK" 2>/dev/null || exit 0

while IFS= read -r line; do
  [ -z "$line" ] && continue
  # A corrupt line must not abort the drain (matching check_inbox): skip it
  # and keep delivering. Aborting here would strand the rest of the batch in
  # $WORK until a sweep recovers it.
  if ! payload=$(printf '%s' "$line" \
    | python3 -c "import sys,json;print(json.dumps(json.loads(sys.stdin.read()).get('payload', {})))" 2>/dev/null); then
    echo "inject-inbox.sh: skipping corrupt inbox line" >&2
    continue
  fi
  printf 'agentbus inbox: %s\n' "$payload"
done < "$WORK"

rm -f "$WORK"
