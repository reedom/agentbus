---
refs:
  id: ref:watch-integration
  kind: reference
  title: "Recipient-side watch integration for interactive harnesses"
  related:
    - fr:14-watch
    - fr:13-on-delivery
    - fr:09-hook-inbox
---

# Recipient-side watch integration for interactive harnesses

## Problem

An idle interactive agent session (e.g. Claude Code) has no passive way to
receive messages:

- `on_delivery` (fr:13-on-delivery) is sender-executed and bounded to 15 s.
  Once the sender's process exits, no one is knocking.
- `await_message` (fr:08-mcp-shim) burns a tool-call slot and holds the turn
  open for the entire block duration.
- `check_inbox` (fr:09-hook-inbox) is pull-only — the session must be
  active to ask.

`agentbus watch` fills this gap: a long-running per-recipient notifier that
the harness hosts outside the agent's turn cycle.

## Pattern: watch-plus-monitor

A session-start hook instructs the harness to launch
`agentbus watch <instance_id>` under its persistent monitor facility. Each
stdout line — one compact JSON envelope — re-invokes the idle agent. The agent
reacts by calling `check_inbox` (which performs the rename-snapshot consume
under the fr:09-hook-inbox contract) and then answers via `send` or `reply`.

```
session start
  └── harness launches: agentbus watch <id> --interval-ms 500
                              │ (runs in background, never exits)
                              │
      new message arrives     │
        sender appends to inbox/<id>.jsonl
        sender runs on_delivery hook (optional)
        event logged to event_log
              │
              ▼
        watch polls event_log, finds envelope addressed to <id>
        prints one compact JSON line on stdout
              │
              ▼
        harness receives line, re-invokes agent
              │
              ▼
        agent calls check_inbox(<id>)  → consumes envelope from inbox spool
        agent replies with send() / reply()
```

## Claude Code example

A `SessionStart` hook emits a directive to the agent instructing it to start
the watcher via the Monitor tool:

```
SYSTEM DIRECTIVE — run immediately before processing any user message:

Invoke the Monitor tool with these exact parameters:
  command:     "agentbus watch $AGENTBUS_INSTANCE --interval-ms 500"
  description: "agentbus inbox stream"
  persistent:  true

Each line the monitor emits is a compact JSON envelope addressed to this
session. When a line arrives, call check_inbox to consume pending messages,
then respond to them (reply to asks, acknowledge messages).
```

The hook sets `$AGENTBUS_INSTANCE` to the session's registered instance id
before emitting the directive.

## Division of responsibility

**agentbus ships only the verb.** `agentbus watch` is a bare event-log tail:
it polls, filters, and prints. That is the full extent of its contract.

Watcher lifecycle belongs entirely to the integrating harness package:

- **Deduplication across `/clear` and `--resume` re-fires**: the harness must
  detect that a watcher is already running for this instance before launching
  another one (e.g. check for an existing monitor by description).
- **Orphan cleanup**: if the session exits without killing the watcher, the
  harness (or the OS) must reclaim the process. Prior art: agmsg's monitor
  delivery mode used a pidfile for this; its pidfile management shows the
  operational cost of owning lifecycle inside the tool.
- **Exclusivity across sessions**: a given instance id should have at most one
  active watcher; two watchers double-fire notifications without harm to inbox
  integrity (watch never consumes), but waste resources and confuse the agent.

## Fallback for harnesses without a monitor facility

A harness that cannot host a persistent background process can approximate the
pattern with a stop-boundary hook:

```bash
# Hook: runs at Stop / turn boundary
agentbus check-inbox "$AGENTBUS_INSTANCE" | inject-into-next-prompt
```

This is pull-only: the agent sees messages only at the start of the next turn,
not while idle. It is simpler and covers the most common case where the user
re-engages the agent anyway.

## Watch is lossless to restart

`agentbus watch` never consumes the inbox. A dead watcher misses
notifications but not messages: the inbox spool file (`~/.agentbus/inbox/<id>.jsonl`)
accumulates envelopes regardless of whether watch is running. On restart,
`check_inbox` returns everything that arrived while the watcher was down.

Watch starting at a new `max_seq` after a restart will not replay past
envelopes (that is by design — no history replay). But those envelopes are
still in the inbox spool; the agent finds them on the next `check_inbox` call.
