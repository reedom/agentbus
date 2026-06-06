---
refs:
  id: ref:slack-bridge
  kind: reference
  title: "Slack bridge integration example"
  related:
    - ref:protocol
---

# Example: Slack bridge for human-in-the-loop `ask`

This walkthrough shows how to bridge agentbus `ask` calls to a Slack channel
where a human picks an answer via interactive buttons. The human's choice is
returned as the `reply` payload, unblocking the AI's `ask`.

This is the concrete realization of spec section 7.3.

## 1. Topology

```
AI (Claude)           store (~/.agentbus)        Slack bridge          Slack
   |                        |                         |                   |
   |--- ask("slack-ask") -->|                         |                   |
   |                        |--- inbox append -------->|                   |
   |                        |--- on_delivery hook ---->|                   |
   |                        |                         |--- chat.postMessage with buttons -->|
   |                        |                         |                   |
   |                        |                         |<-- interaction payload (button click) --|
   |                        |<-- reply written --------|                   |
   |<------ reply ----------|                         |                   |
```

The bridge is a small program (Node, Go, Rust — anything that can call the
`agentbus` CLI or embed `agentbus-core`) that:

1. Registers as `slack-ask` with `on_delivery` set to the bridge's wakeup command.
2. On delivery wake, calls `agentbus check-inbox slack-ask` to drain the inbox.
3. Posts an interactive Slack message for every inbound `ask` envelope.
4. Calls `agentbus reply` when the human clicks a button.

## 2. Register with on_delivery

The bridge registers once at startup. `on_delivery` tells the sender's process
to run the bridge's wakeup command after each inbox append.

```bash
agentbus register slack-ask \
  --persistent \
  --on-delivery "bellhop dispatch slack-ask"
# {"ok": true}
```

**Security note**: `on_delivery` is arbitrary code run by the sender's OS user.
Register it only with commands you trust. See [fr:13-on-delivery](../fr/13-on-delivery.md).

On each delivery the wakeup command fires (15 s cap), then the bridge calls:

```bash
agentbus check-inbox slack-ask
# {"envelopes": [...]}
```

## 3. AI side: ask the human

The Claude orchestrator calls the `ask` MCP tool:

```jsonc
{
  "from": "orch",
  "to": "slack-ask",
  "timeout_ms": 1800000,
  "payload": {
    "question": "Deploy build 4321 to production?",
    "options": ["deploy", "hold"]
  }
}
```

The store appends the envelope to `slack-ask`'s inbox spool and fires the
`on_delivery` hook. The asker polls for a reply in the `asks` table.

## 4. Bridge side: post to Slack

The bridge drains its inbox and finds an envelope like:

```json
{
  "id": "msg_01HXY_ASK",
  "kind": "ask",
  "from": "orch",
  "to": "slack-ask",
  "timeout_ms": 1800000,
  "ts": "2026-05-21T08:12:34Z",
  "payload": {
    "question": "Deploy build 4321 to production?",
    "options": ["deploy", "hold"]
  }
}
```

It posts the question to Slack via [`chat.postMessage`][slack-post] with one
button per option. The envelope `id` is carried in `block_id` so the
interaction webhook can correlate the click back to the original ask:

```json
POST https://slack.com/api/chat.postMessage
{
  "channel": "C0123456",
  "text": "Deploy build 4321 to production?",
  "blocks": [
    {
      "type": "section",
      "text": { "type": "mrkdwn", "text": "*Deploy build 4321 to production?*" }
    },
    {
      "type": "actions",
      "block_id": "msg_01HXY_ASK",
      "elements": [
        { "type": "button", "action_id": "choose", "value": "deploy", "text": { "type": "plain_text", "text": "deploy" } },
        { "type": "button", "action_id": "choose", "value": "hold",   "text": { "type": "plain_text", "text": "hold"   } }
      ]
    }
  ]
}
```

[slack-post]: https://api.slack.com/methods/chat.postMessage

## 5. Bridge side: handle the click

Slack POSTs an [interaction payload][slack-interactions] to the bridge's
webhook URL when the user clicks a button. The relevant fields:

```json
{
  "type": "block_actions",
  "actions": [
    {
      "action_id": "choose",
      "block_id": "msg_01HXY_ASK",
      "value": "deploy"
    }
  ],
  "user": { "id": "U0123", "name": "alice" }
}
```

The bridge extracts `block_id` (the original `ask` envelope `id`) and posts a
`reply` via the CLI:

```bash
agentbus reply msg_01HXY_ASK slack-ask <<'EOF'
{
  "choice": "deploy",
  "by": "alice"
}
EOF
# {"ok": true}
```

[slack-interactions]: https://api.slack.com/interactivity/handling

## 6. AI side: receives the choice

The orchestrator's blocked `ask` returns:

```json
{ "request_id": "msg_01HXY_ASK", "payload": { "choice": "deploy", "by": "alice" } }
```

## 7. Failure modes

- **Bridge crashes mid-flight.** The registration row stays (it is persistent).
  The inbox spool retains any unread envelopes. When the bridge restarts, it
  calls `check-inbox` and processes whatever accumulated. If `on_delivery` was
  set, `agentbus sweep` will re-fire the hook for any inbox that has been
  non-empty for more than the grace period (default 60 s).
- **Human never clicks.** The asker's `ask` times out after `timeout_ms` (exit
  2). Use `agentbus ask-result <id>` to retrieve a reply if the human clicks
  late.
- **Late click.** A `reply` to an already-timed-out ask is written to the
  `asks` row and retrievable via `agentbus ask-result`. `unknown_request_id`
  is returned only if the request_id is entirely unknown (never existed).

## 8. Production checklist

- Verify Slack request signatures before trusting interaction payloads.
- Rate-limit if the bridge ever exposes posting back into agentbus from Slack
  slash commands.
- Persist a short-lived map of `envelope_id -> slack_ts` so the bridge can
  edit the original message on reply (showing the choice and who made it).
- The `agentbus register --persistent` row survives bridge restarts; no
  re-registration needed unless the instance id changes.
