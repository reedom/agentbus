---
refs:
  id: fr:13-on-delivery
  kind: fr
  title: "on_delivery hook execution contract"
  related:
    - fr:02-instance-registry
    - fr:04-router
    - fr:15-sweep
  modules:
    - crates/agentbus-core/src/store/hook.rs
---

# FR 13: on_delivery hook execution contract

> The sender executes the recipient's registered shell command after a
> successful inbox append; failure is non-fatal and always logged.

## Purpose

agentbus v0.2 has no daemon that can push notifications. Instead, the sender
executes the recipient's registered `on_delivery` command synchronously after
the inbox append commits. This is how integrators (e.g. cmux-bellhop) wake
agents without a resident process: the agent registers with
`on_delivery = "bellhop dispatch <name>"` and the sender's process does the
knocking.

## User-visible Behavior

### Execution

After a successful inbox append (send or ask path — the envelope is already
durably spooled), the sender runs:

```sh
sh -c "<on_delivery command>"
```

The child process inherits the sender's full environment. Four
`AGENTBUS_*` variables are added (additive, not replacing):

| Variable | Value |
|---|---|
| `AGENTBUS_INSTANCE` | recipient instance id |
| `AGENTBUS_ENVELOPE_ID` | envelope id of the just-delivered message |
| `AGENTBUS_KIND` | envelope kind string: `message` or `ask` (replies and events do not write to inboxes and never fire delivery hooks) |
| `AGENTBUS_FROM` | sender instance id |

`stdin`, `stdout`, and `stderr` are all redirected to `/dev/null`. The hook
is not expected to produce output; any output is silently discarded.

### Timeout and kill

The hook has a hard 15-second wall-clock timeout. If the child is still
running after 15 seconds, it is killed (`SIGKILL`) and waited on before
`run_delivery_hook` returns. The hook outcome is `HookOutcome { ok: false,
detail: "timeout" }`.

### Failure policy

Hook failure is non-fatal. The envelope is already durably spooled before
the hook runs; the send/ask operation has already succeeded from the caller's
perspective. On failure:

1. A `bus.delivery_hook_failed` event is published to `event_log`:
   ```json
   {
     "event": "bus.delivery_hook_failed",
     "instance": "<recipient id>",
     "envelope_id": "<envelope id>",
     "detail": "<exit status or error string>"
   }
   ```
   The event is published `from = "bus"`.

2. A `tracing::warn!` is emitted with `instance` and `detail` fields.

The `HookOutcome` struct is returned to the caller (e.g. the CLI) so it can
surface a warning to the user. The caller decides whether to surface or
suppress.

### Sweep re-fire

`agentbus sweep` re-fires hooks for inbox files that are non-empty and
unmodified for the grace period (default 60 s), covering the case where a
sender crashed between the inbox append and the hook. Sweep re-fires using
`run_sweep_hook`, which sets only `AGENTBUS_INSTANCE` (no envelope-specific
vars — the exact envelope is unknown at sweep time).

## Capabilities

- Sender-executed, no daemon required.
- Env vars carry enough context for the hook to decide what to do without
  re-querying the bus.
- 15 s timeout prevents a misbehaving hook from blocking the sender.
- Non-fatal: a hook crash or timeout does not roll back or discard the
  envelope.
- `bus.delivery_hook_failed` in `event_log` creates an auditable record of
  every hook failure.
- Sweep covers "sender crashed between append and hook" without any
  persistent failure tracking.

## Boundaries

- The hook runs as the same OS user as the sender. Because `on_delivery` is
  registered by that same user, executing it grants nothing the user does not
  already have. **This must be documented loudly for integrators**: an
  `on_delivery` command is arbitrary code registered by the same user; it is
  not a sandbox.
- Hook stdout and stderr are discarded; there is no way to capture hook output
  from the bus side.
- The 15 s timeout is wall-clock, not CPU time.
- Sweep re-fire sets only `AGENTBUS_INSTANCE` — hooks must be idempotent with
  respect to receiving only that variable.
- Hook registration and storage are covered by fr:02-instance-registry. The
  send/ask flow that triggers the hook is covered by fr:04-router. Sweep
  scheduling is covered by fr:15-sweep.

## Error Handling

Hook errors do not propagate as `StoreError` to the caller. Instead:

- Spawn failure: `HookOutcome { ok: false, detail: "spawn: <io error>" }`.
- Non-zero exit: `HookOutcome { ok: false, detail: "exit: <ExitStatus>" }`.
- Timeout: `HookOutcome { ok: false, detail: "timeout" }`.
- `try_wait` error: child is killed; `HookOutcome { ok: false, detail: "wait: <error>" }`.

The `bus.delivery_hook_failed` event publication can itself fail (e.g. the
store is locked); that failure is logged via `tracing::warn!` and swallowed.

## Traceability

- Related FR: fr:02-instance-registry, fr:04-router, fr:15-sweep
- Spec sections: 6.5 (on_delivery execution), 9 (security)

## When to update

- The hook timeout changes (currently 15 s).
- The set of `AGENTBUS_*` env vars changes.
- The event payload shape for `bus.delivery_hook_failed` changes.
- stdin/stdout/stderr disposition changes (e.g. capturing stderr).
- The failure policy changes (e.g. hook failure becomes fatal, or triggers
  a retry).
- Sweep re-fire env vars change (currently only `AGENTBUS_INSTANCE`).
