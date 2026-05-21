#!/usr/bin/env bash
# Smoke test for agentbusd REST API using curl.
# Requires the daemon to be running (default http://127.0.0.1:8765).
set -euo pipefail

URL=${AGENTBUS_URL:-http://127.0.0.1:8765}

echo "=> register bob"
TOKEN=$(curl -sS -X POST "$URL/v1/instances" \
  -H 'content-type: application/json' \
  -d '{"instance_id":"bob"}' \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['owner_token'])")
echo "owner_token=$TOKEN"

echo "=> tail bob's inbox in background"
# Subshell disables pipefail so curl exiting 23 on head's SIGPIPE
# (after 3 SSE lines) does not fail the whole script.
( set +o pipefail; curl -sN "$URL/v1/instances/bob/inbox" 2>/dev/null | head -n 3 ) &
TAIL_PID=$!
sleep 0.2

echo "=> send 3 messages"
for i in 1 2 3; do
  curl -sS -X POST "$URL/v1/instances/bob/messages" \
    -H 'content-type: application/json' \
    -d "{\"payload\":{\"n\":$i}}"
  echo
done

wait "$TAIL_PID" || true

echo "=> unregister bob"
curl -sS -X DELETE "$URL/v1/instances/bob" \
  -H "x-agentbus-owner: $TOKEN" \
  -w "%{http_code}\n"
