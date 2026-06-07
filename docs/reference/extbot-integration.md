---
refs:
  id: ref:extbot-integration
  kind: reference
  title: "External orchestrator (extbot) integration example"
  related:
    - ref:protocol
---

# Example: extbot orchestrator integration

This walkthrough shows how the [extbot][extbot] orchestrator can drive a Claude
Code instance via agentbus, using the CLI from the extbot side.

The Claude instance registers as `extbot-<ticket>` through the MCP shim;
extbot sends it work and observes results via the `agentbus` CLI or by
embedding `agentbus-core` directly.

[extbot]: https://example.invalid/extbot

## 1. Topology

```
extbot (CLI / embedded)         store (~/.agentbus)         Claude (MCP shim)
       |                               |                           |
       |                               |<-- register("extbot-ENG-123") --|
       |                               |                           |
       |--- agentbus send ------------>|--- inbox append --------->|
       |                               |--- on_delivery hook ----->|
       |                               |                           |
       |--- agentbus ask (blocks) ---->|--- inbox append --------->|
       |                               |<-- reply written to asks -|
       |<--- reply payload ------------|                           |
       |                               |                           |
       |--- agentbus publish --------->|--- event_log append ----->|
```

extbot is a store writer. It does not register an instance — it sends as
`ext:extbot` (the CLI default) or as a registered id of its own.

## 2. Claude side: register

When extbot launches a Claude session for ticket `ENG-123`, Claude's MCP client
loads the `agentbus` server and calls `register`:

```jsonc
{ "instance_id": "extbot-ENG-123", "persistent": false }
```

The shim tracks this as a non-persistent registration tied to the shim process.
When Claude exits, the shim exits and the registration becomes a dead row;
`agentbus sweep` reclaims it.

Claude then enters a loop driven by `check_inbox` at each turn, or blocks via
`await_message`:

```jsonc
// tool: check_inbox
{ "instance_id": "extbot-ENG-123" }
// returns: {"envelopes": [...]}
```

## 3. extbot side: inject context as a `message`

When extbot wants to feed in extra context — say, the latest CI failure log —
it sends a one-way `message`:

```bash
agentbus send extbot-ENG-123 --from ext:extbot <<'EOF'
{
  "type": "ci_failure",
  "build_id": 4321,
  "log_tail": "...",
  "hint": "look at module X"
}
EOF
# {"id": "msg_01HXY_CI"}
```

Claude's `check_inbox` returns the envelope in the next batch:

```json
{
  "envelopes": [
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
  ]
}
```

Claude incorporates the `payload` into its working context and continues.

## 4. extbot side: ask Claude a question synchronously

When extbot needs an answer before proceeding (for example, "is this PR ready
to merge?"), it uses `ask`. The CLI call blocks until Claude replies or the
timeout expires.

```bash
agentbus ask extbot-ENG-123 --from ext:extbot --timeout-ms 600000 <<'EOF'
{
  "question": "Ready to merge PR #4321?",
  "checks": ["tests", "lint", "review"]
}
EOF
# On success, prints the reply payload as pretty JSON:
# {
#   "ready": true,
#   "notes": "All checks green, one nit on naming addressed."
# }
```

Claude side (MCP tool calls):

```jsonc
// check_inbox returns kind=ask envelope
// tool: reply
{
  "from": "extbot-ENG-123",
  "request_id": "msg_01HXY_ASK_EXTBOT",
  "payload": {
    "ready": true,
    "notes": "All checks green, one nit on naming addressed."
  }
}
```

On timeout (exit 2), stderr contains the request_id hint:

```
error[timeout]: no reply within 600000 ms; retrieve a late reply with: agentbus ask-result msg_01HXY_ASK_EXTBOT
```

## 5. extbot side: broadcast events for observers

For status events that any sibling agent or dashboard may want, extbot uses
broadcast `event`s appended to the event log:

```bash
agentbus publish --from ext:extbot <<'EOF'
{
  "type": "ticket_state_changed",
  "ticket": "ENG-123",
  "from": "in_progress",
  "to": "in_review"
}
EOF
# {"id": "msg_01HEVT..."}
```

Any process can read the event log:

```bash
agentbus events --instance extbot-ENG-123
# {"seq":1,"envelope":{"id":"msg_01HEVT...","kind":"event","from":"ext:extbot","ts":"...","payload":{...}}}

agentbus events --follow --since 42
# streams from seq 42 indefinitely
```

## 6. Lifetime considerations

- The Claude registration is a non-persistent row tied to the shim pid. extbot
  does not need to manage its lifetime — when the Claude session exits, the
  shim exits, the row becomes a dead entry, and `agentbus sweep` reclaims it.
- `unknown_instance` from `send` or `ask` means Claude is not registered.
  extbot should restart the Claude session or fail the ticket.
- `timeout` from `ask` means Claude is registered but did not answer in time.
  Use `agentbus ask-result <id>` to retrieve a late reply if Claude answers
  after the deadline.
- The instance ID `extbot-<ticket>` is exclusive for live (pid-alive) rows.
  If extbot tries to spawn a second Claude for the same ticket while the first
  is still alive, registration fails with `instance_id_taken`. Use that as a
  cheap lock.
