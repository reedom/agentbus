# Example: Slack bridge for human-in-the-loop `ask`

This walkthrough shows how to bridge agentbus `ask` calls to a Slack channel
where a human picks an answer via interactive buttons. The human's choice is
returned as the `reply` payload, unblocking the AI's `ask`.

This is the concrete realization of spec section 7.3.

## 1. Topology

```
AI (Claude)              agentbusd                 Slack bridge          Slack
   |                       |                          |                    |
   |--- ask("slack-ask") ->|                          |                    |
   |                       |--- envelope on inbox --->|                    |
   |                       |                          |--- chat.postMessage with buttons -->|
   |                       |                          |                    |
   |                       |                          |<-- interaction payload (button click) --|
   |                       |<-- POST /replies --------|                    |
   |<------ reply ---------|                          |                    |
```

The bridge is a small program (Node, Go, Rust — anything that speaks HTTP and
SSE) that:

1. Registers as `slack-ask` against the daemon.
2. Subscribes to its own inbox via SSE.
3. Posts an interactive Slack message for every inbound `ask`.
4. Posts a `reply` to the daemon when the human clicks a button.

## 2. Register and subscribe

The bridge registers via REST. The HTTP connection's keep-alive holds the
registration.

```bash
curl -N -X POST http://127.0.0.1:PORT/v1/instances \
     -H 'content-type: application/json' \
     -d '{"instance_id": "slack-ask", "mailbox_size": 64}'
```

While that connection stays open, the bridge also opens an inbox SSE stream on
a second connection owned by the same process:

```bash
curl -N http://127.0.0.1:PORT/v1/instances/slack-ask/inbox
```

Each line of the SSE stream is a JSON envelope addressed to `slack-ask`.

## 3. AI side: ask the human

The Claude orchestrator calls the `ask` MCP tool:

```jsonc
{
  "to": "slack-ask",
  "timeout_secs": 1800,
  "payload": {
    "question": "Deploy build 4321 to production?",
    "options": ["deploy", "hold"]
  }
}
```

The daemon enqueues the envelope to `slack-ask`'s inbox and arms a 30-minute
timeout.

## 4. Bridge side: post to Slack

The bridge receives an envelope like:

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
`reply` to the daemon:

```bash
curl -X POST http://127.0.0.1:PORT/v1/instances/slack-ask/replies \
     -H 'content-type: application/json' \
     -d '{
       "request_id": "msg_01HXY_ASK",
       "payload": {
         "choice": "deploy",
         "by":     "alice"
       }
     }'
```

[slack-interactions]: https://api.slack.com/interactivity/handling

## 6. AI side: receives the choice

The orchestrator's blocked `ask` returns:

```json
{ "choice": "deploy", "by": "alice" }
```

## 7. Failure modes

- **Bridge crashes mid-flight.** The HTTP connection holding the registration
  drops, the daemon auto-unregisters `slack-ask`, and any pending asks
  targeting it are cancelled with `instance_disconnected`. The AI's `ask`
  returns immediately rather than waiting for the timeout.
- **Human never clicks.** The daemon times out after `timeout_ms`. The AI sees
  `{"error": "timeout", "request_id": "msg_01HXY_ASK"}`. The bridge should
  update the Slack message to a "expired" state when it sees the same envelope
  pass `timeout_ms` since `ts`.
- **Late click.** A `reply` arriving after timeout is rejected with
  `unknown_request_id`. The bridge should log and surface a "too late" note in
  Slack.

## 8. Production checklist

- Verify Slack request signatures before trusting interaction payloads.
- Rate-limit unknown `from` instances if the bridge ever exposes posting back
  into agentbus from Slack slash commands.
- Persist a short-lived map of `envelope_id -> slack_ts` so the bridge can edit
  the original message on reply (showing the choice and who made it).
- Treat the registration connection as load-bearing: monitor it, restart with
  backoff, and re-subscribe to the inbox on reconnect.
