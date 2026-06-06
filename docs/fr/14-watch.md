---
refs:
  id: fr:14-watch
  kind: fr
  title: "watch: recipient-side notifier stream"
  related:
    - fr:05-eventlog
    - fr:09-hook-inbox
    - fr:10-cli
  modules:
    - crates/agentbus-cli/src/commands.rs
    - crates/agentbus-core/src/store/events.rs
---

# FR 14: watch: recipient-side notifier stream

> `agentbus watch <id>` tails the event log for envelopes addressed to one
> instance and prints them one per line, without ever consuming the inbox.

## Purpose

`on_delivery` (fr:13-on-delivery) is sender-executed and bounded to 15 s.
`await_message` blocks a tool-call slot. `check_inbox` is pull-only.
`agentbus watch` fills the remaining gap: a long-running recipient-side
reader for harnesses that can host a persistent monitor process and wake an
idle agent on its output (e.g. Claude Code's Monitor tool; prior art:
agmsg's `monitor` delivery mode).

## User-visible Behavior

### Invocation

```
agentbus watch <id> [--interval-ms <ms>]
```

`--interval-ms` defaults to 500. The command runs until killed.

### Stream behavior

1. On startup, the current `max_seq` is read. Watch starts from that
   position — **no history is replayed**. Only envelopes arriving after
   `watch` starts are printed.
2. The command polls `event_log` using `EventFilter { to: Some(id) }` —
   only envelopes explicitly addressed to the instance appear.
3. Each matching envelope is printed as one compact JSON line:

   ```
   {"id":"...","kind":"message","from":"alice","to":"bob","ts":"...","payload":{...}}
   ```

   `serde_json::to_string` escapes internal newlines, so one message is
   always exactly one output line. Harnesses can read line-by-line without
   a framing protocol.
4. The cursor advances past every scanned row (including filtered-out ones)
   so the poll loop never rescans.
5. After draining the current batch, the loop sleeps `interval_ms` and
   polls again. This is the same cursor-and-poll loop used by
   `agentbus events --follow`.

### watch never consumes the inbox

Watch is a notifier only. The envelope remains in the inbox spool
(`~/.agentbus/inbox/<id>.jsonl`) after watch prints it. The agent reacts to
the notification by calling `check_inbox` (or `await_message`), which
performs the rename-snapshot consume under the fr:09-hook-inbox contract.

A watcher dying mid-stream therefore loses nothing — the spool is the source
of truth.

### Output format: bare envelopes

Watch prints bare envelope JSON (no `seq` wrapper), unlike
`agentbus events --follow` which prints `{"seq":N,"envelope":{...}}`.
The `with_seq = false` flag in `stream_events` selects this shape.

## Capabilities

- Live-only stream: starts at `max_seq` at launch, no history replay.
- One compact JSON line per addressed envelope; safe for line-oriented
  monitor tools.
- Configurable poll interval (default 500 ms).
- Non-destructive: inbox is untouched; the agent decides when to consume.
- Uses the same `events_since` + `EventFilter` machinery as the events
  command; no additional storage read path.

## Boundaries

- Watch is a notifier, not a consumer. Inbox consumption is always a
  separate `check_inbox` or `await_message` call (fr:09-hook-inbox).
- Watcher lifecycle — launching from session-start hooks, deduplicating
  across session restarts, orphan cleanup — belongs to the integrating
  harness, not the bus. The integration pattern is documented in the watch
  integration reference doc (the watch integration reference, to be created
  in the next task).
- Watch does not filter by kind; it shows all envelope kinds addressed to
  the instance (messages, asks, replies, events).
- The 500 ms default poll means notification latency is up to 500 ms.
  Reduce `--interval-ms` if lower latency is needed (at higher CPU cost).
- `agentbus events --follow` covers bus-wide event tailing; watch is
  per-instance only.

## Error Handling

- Store open errors propagate as `StoreError` and exit non-zero.
- Poll errors (SQLite errors mid-stream) propagate up through `stream_events`
  and exit non-zero. There is no retry inside the watch loop.
- If the instance id does not exist in `instances`, watch still runs — it
  simply receives no envelopes until something is addressed to that id.

## Traceability

- Related FR: fr:05-eventlog, fr:09-hook-inbox, fr:10-cli
- Spec sections: 6.7 (watch notifier contract)

## When to update

- The default poll interval changes.
- The output line format changes (e.g. adding a `seq` field).
- Watch gains history-replay behavior (currently: starts at `max_seq`).
- Watch starts consuming the inbox (currently: notifier only).
- The `EventFilter.to` field semantics change.
- The `stream_events` `with_seq` flag behavior changes.
