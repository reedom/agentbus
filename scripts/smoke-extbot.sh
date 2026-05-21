#!/usr/bin/env bash
# Stub MCP client smoke test exercising mcp-bus-cli against a running daemon.
# This mirrors smoke-curl.sh but routes through the Rust CLI to confirm the
# end-to-end CLI wiring (ls / send / ask / tail / rm) is functional.
#
# Requires:
#   - mcp-busd running at $MCP_BUS_URL (default http://127.0.0.1:8765)
#   - mcp-bus-cli binary on PATH (or set MCP_BUS_CLI=/path/to/mcp-bus)
set -euo pipefail

URL=${MCP_BUS_URL:-http://127.0.0.1:8765}
CLI=${MCP_BUS_CLI:-mcp-bus}

if ! command -v "$CLI" >/dev/null 2>&1; then
  echo "error: mcp-bus CLI not found ('$CLI'); set MCP_BUS_CLI or add to PATH" >&2
  exit 127
fi

echo "=> register extbot via REST (CLI does not expose register)"
TOKEN=$(curl -sS -X POST "$URL/v1/instances" \
  -H 'content-type: application/json' \
  -d '{"instance_id":"extbot"}' \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['owner_token'])")
echo "owner_token=$TOKEN"

echo "=> ls instances"
"$CLI" --url "$URL" ls

echo "=> tail events (extbot) for ~1s"
( "$CLI" --url "$URL" tail --instance extbot | head -n 3 ) &
TAIL_PID=$!
sleep 0.2

echo "=> send 2 messages via CLI"
for i in 1 2; do
  printf '{"n":%d,"from":"smoke-extbot"}' "$i" \
    | "$CLI" --url "$URL" send extbot --file -
done

wait "$TAIL_PID" || true

echo "=> rm extbot"
"$CLI" --url "$URL" rm extbot --owner "$TOKEN"
