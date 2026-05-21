# Example: extbot orchestrator integration

This walkthrough shows how the [extbot][extbot] orchestrator can drive a Claude
Code instance via mcp-bus, using only the REST surface from the extbot side.

The Claude instance registers as `extbot-<ticket>` through the MCP shim;
extbot sends it work and observes results over HTTP.

[extbot]: https://example.invalid/extbot

## 1. Topology

```
extbot                       mcp-busd                  Claude (MCP)
 |                            |                          |
 |                            |<-- register("extbot-ENG-123") --|
 |                            |                          |
 |--- POST /messages -------->|--- envelope --> await_message -->|
 |                            |                          |
 |--- POST /ask (blocks) ---->|--- envelope --> await_message -->|
 |                            |<-- reply ----------------|
 |<--- 200 with reply --------|                          |
 |                            |                          |
 |--- POST /events ---------->|--- broadcast SSE -->     |
```

extbot is purely an HTTP client. It does not register an instance — it talks as
an unregistered external using `from: "ext:extbot"` automatically attached by
the daemon at ingress.

## 2. Claude side: register

When extbot launches a Claude session for ticket `ENG-123`, Claude's MCP client
loads the `mcp-bus` server and calls `register`:

```jsonc
{ "instance_id": "extbot-ENG-123", "mailbox_size": 256 }
```

The shim binds the registration to its Unix socket connection. When Claude
exits, the shim exits, the socket closes, and the daemon auto-unregisters.

Claude then enters a loop, typically driven by an agent or hook:

```jsonc
// tool: await_message
{ "timeout_secs": 60 }
```

## 3. extbot side: inject context as a `message`

When extbot wants to feed in extra context — say, the latest CI failure log —
it sends a one-way `message`:

```bash
curl -X POST http://127.0.0.1:PORT/v1/instances/extbot-ENG-123/messages \
     -H 'content-type: application/json' \
     -d '{
       "payload": {
         "type": "ci_failure",
         "build_id": 4321,
         "log_tail": "...",
         "hint": "look at module X"
       }
     }'
```

Claude's `await_message` returns with:

```json
{
  "id": "msg_01HXY_CI",
  "kind": "message",
  "from": "ext:extbot",
  "to": "extbot-ENG-123",
  "ts": "2026-05-21T08:12:34Z",
  "payload": {
    "type": "ci_failure",
    "build_id": 4321,
    "log_tail": "...",
    "hint": "look at module X"
  }
}
```

Claude can incorporate the `payload` into its working context and continue.

## 4. extbot side: ask Claude a question synchronously

When extbot needs an answer before proceeding (for example, "is this PR ready
to merge?"), it uses `ask`. The HTTP call blocks until Claude `reply`s or the
timeout expires.

```bash
curl -X POST 'http://127.0.0.1:PORT/v1/instances/extbot-ENG-123/ask?timeout_ms=600000' \
     -H 'content-type: application/json' \
     -d '{
       "payload": {
         "question": "Ready to merge PR #4321?",
         "checks":   ["tests","lint","review"]
       }
     }'
```

Claude side:

```jsonc
// tool: await_message — returns kind=ask envelope
// tool: reply
{
  "request_id": "msg_01HXY_ASK_EXTBOT",
  "payload": {
    "ready": true,
    "notes": "All checks green, one nit on naming addressed."
  }
}
```

extbot's HTTP response is `200` with body:

```json
{
  "ready": true,
  "notes": "All checks green, one nit on naming addressed."
}
```

If Claude never replies within `timeout_ms`, extbot gets `504` with body
`{"error": "timeout", "request_id": "msg_01HXY_ASK_EXTBOT"}`.

## 5. extbot side: broadcast events for observers

For status events that any subscribed dashboard or sibling agent may want,
extbot uses broadcast `event`s:

```bash
curl -X POST http://127.0.0.1:PORT/v1/events \
     -H 'content-type: application/json' \
     -d '{
       "kind": "event",
       "payload": {
         "type":     "ticket_state_changed",
         "ticket":   "ENG-123",
         "from":     "in_progress",
         "to":       "in_review"
       }
     }'
```

Any client subscribed to `GET /v1/events` (or `GET
/v1/events?instance=extbot-ENG-123`) sees the envelope. The daemon stamps
`from: "ext:extbot"` (or whatever label extbot supplies) and writes it to the
JSONL log for replay.

## 6. extbot side: replay missed events

If a extbot dashboard process restarts, it can resume from its last seen
timestamp:

```
GET /v1/events?since=2026-05-21T08:00:00Z
```

The daemon replays matching envelopes from the JSONL log up to the snapshot
offset, then attaches the subscriber to live broadcasts. Dedup by envelope
`id` on the client to defend against reconnect overlap.

## 7. Lifetime considerations

- The Claude registration is bound to the MCP shim's Unix socket. extbot does
  not need to manage its lifetime — when the Claude session exits, the
  instance is gone, and any blocked extbot `ask` against it returns
  `instance_disconnected` immediately.
- extbot should treat `instance_disconnected` and `timeout` differently:
  - `instance_disconnected` — Claude crashed or exited. Restart it, or fail
    the ticket.
  - `timeout` — Claude is alive but didn't answer in time. Decide based on
    the question (escalate to a human, retry with smaller scope, etc.).
- The instance ID `extbot-<ticket>` is exclusive. If extbot tries to spawn a
  second Claude for the same ticket while the first is still alive,
  registration fails with `instance_id_taken`. Use that as a cheap lock.
