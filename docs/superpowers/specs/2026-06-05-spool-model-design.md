# agentbus v0.2: spool model design

- Date: 2026-06-05
- Status: draft, awaiting review
- Supersedes: the daemon architecture (`agentbusd`, fr:06 REST, fr:07 SSE, fr:11 daemon lifecycle)
- Downstream: reshapes `reedom/cmux-bellhop` (its daemon dissolves into a CLI; spec updated separately)

## 1. Summary

agentbus v0.2 removes the daemon. The bus becomes a protocol over shared
local storage -- a SQLite database plus per-instance inbox spool files --
implemented entirely by short-lived CLI invocations and in-process client
libraries. Think Maildir or git: no resident process, durable by default,
any participant can act on the bus by opening the store.

Every responsibility `agentbusd` held moves either into the store (state)
or into the sender's process (work):

- Registration is a database row, persistent or pid-scoped.
- Delivery is the sender appending to the recipient's inbox file.
- Reaction is the sender executing the recipient's registered
  `on_delivery` command, synchronously.
- Ask blocking is the asker polling the store for the reply.
- Event streaming is tailing an ordered table.

Zero daemons. An optional `agentbus sweep` run from launchd (a periodic
CLI, not a resident) handles crash-recovery edges.

## 2. Goals

- No resident process anywhere in the bus.
- Durable addressing: registrations survive reboots; envelopes addressed
  to an absent instance wait in its spool.
- Preserve the envelope wire format (fr:01) and the hook-inbox consume
  contract (fr:09: append-only file, rename-snapshot consume) unchanged.
- Keep the MCP tool surface and CLI verbs compatible where semantics
  allow (`register`, `send`, `ask`, `reply`, `await_message`,
  `check_inbox`, `publish_event`, `list_instances`, `unregister`).
- Single-user, single-machine trust model, `0700` storage.

## 3. Non-goals

- Remote or multi-host access. REST/SSE/WebSocket surfaces are deleted,
  not ported. If remote access returns later, it returns as a thin
  HTTP view over the same store; the spool stays the source of truth.
- Multi-user arbitration or authentication beyond filesystem ownership.
- v0.1 data migration. The v0.1 registry was in-memory; the v0.1 JSONL
  event log may be archived manually. No importer.
- Backward compatibility for v0.1 REST clients.

## 4. Decisions of record

| Decision | Choice | Why |
|---|---|---|
| Daemon relationship | Replace (v0.2, breaking) | v0.1 is experimental (31 downloads); dual transports double code and tests forever |
| Storage | SQLite (WAL) for registry/asks/events + JSONL inbox files for delivery | hook scripts keep the fr:09 rename-consume contract with shell + jq only |
| Ask wait | Polling, 50 ms start, 250 ms cap backoff | 10 lines, imperceptible against LLM turn latency; kqueue can replace it later without protocol change |
| Liveness | `pid` recorded at registration; readers verify with `kill(pid, 0)`; `persistent` rows exempt | same-machine assumption makes pid checks honest; no heartbeats, no TTL clocks |

## 5. Storage layout

```
~/.agentbus/                  (0700)
  bus.db                      SQLite, WAL mode, busy_timeout=5000
  inbox/
    <instance_id>.jsonl       append-only spool, consumed by rename
```

### 5.1 Schema

```sql
CREATE TABLE instances (
  id            TEXT PRIMARY KEY,           -- [A-Za-z0-9_.:-]{1,128}
  pid           INTEGER,                    -- NULL when persistent
  persistent    INTEGER NOT NULL DEFAULT 0, -- 1: survives owner exit
  on_delivery   TEXT,                       -- command run by senders, nullable
  registered_at TEXT NOT NULL               -- RFC3339 UTC
);

CREATE TABLE asks (
  request_id    TEXT PRIMARY KEY,           -- envelope id of the ask
  from_id       TEXT NOT NULL,
  to_id         TEXT NOT NULL,
  expires_at    TEXT NOT NULL,
  reply_payload TEXT,                       -- JSON; NULL until answered
  replied_at    TEXT
);

CREATE TABLE event_log (
  seq      INTEGER PRIMARY KEY AUTOINCREMENT,
  ts       TEXT NOT NULL,
  envelope TEXT NOT NULL                    -- full envelope JSON
);
```

Envelope `id` stays a ULID (`ids.rs`, unchanged). Global ordering and
replay cursors use `event_log.seq`, which is assigned transactionally and
therefore immune to wall-clock skew between writer processes.

### 5.2 Concurrency rules

- All `bus.db` mutations happen inside one `BEGIN IMMEDIATE` transaction
  per operation. WAL serializes writers; `busy_timeout` absorbs contention.
- Inbox appends take an exclusive `flock` on the inbox file around a
  single `O_APPEND` write of one complete line, then release. Consumers
  never need the lock: the rename snapshot (fr:09) is atomic.
- Only consumers (hook scripts, `await_message`) remove inbox files;
  writers only append. Unchanged from fr:09.

## 6. Operation semantics (who does the daemon's old job)

All operations are functions in `agentbus-core`'s new `store` module,
invoked by the CLI, the MCP shim, or any embedding program. "Sender"
below means the process performing the call.

### 6.1 register / unregister

- `register(id, {persistent, on_delivery})`: validate the id; insert.
  Collision handling: an existing row whose `pid` fails `kill(pid, 0)` is
  dead -- replace it. An existing live row owned by a different pid is
  `instance_id_taken`. Re-register from the same pid is idempotent.
  Non-persistent registrations record the caller's pid.
- `unregister(id)`: delete the row. The inbox file is left in place
  (undelivered mail is not destroyed; `sweep --purge-orphans` can clean).

### 6.2 send (kind: message)

In one transaction: recipient row must exist (else `unknown_instance`);
assign `id`/`ts`; append envelope to `event_log`. Then, outside the
transaction: append the line to `inbox/<to>.jsonl`; if the recipient row
has `on_delivery`, execute it (section 6.5).

### 6.3 ask / reply

- `ask(to, payload, timeout)`: as 6.2, plus an `asks` row with
  `expires_at`. The sender then polls `asks.reply_payload` (50 ms start,
  250 ms cap) until set or expired. Expiry returns a timeout error to the
  caller -- but the row stays. A late `reply` still lands in the row, so
  `agentbus ask-result <request_id>` retrieves it afterward. This is an
  intentional improvement over v0.1, where late replies were dropped.
- `reply(request_id, payload)`: update the `asks` row (idempotent: first
  write wins; later writes are recorded in `event_log` but do not
  overwrite), append the reply envelope to `event_log`. No inbox write:
  the asker reads the row.

### 6.4 events

- `publish_event(payload)`: append to `event_log` in a transaction.
- `events --follow [--since <seq|ts>] [--instance <id>] [--kind <k>]`:
  read rows where the cursor is below `seq`, print, advance, poll. The
  v0.1 snapshot-replay-then-live guarantee becomes trivial: there is only
  one ordered table, so no gap and no duplicate by construction.

### 6.5 on_delivery execution (sender-side, the daemon-killer)

After a successful inbox append, the sender executes the recipient's
`on_delivery` command via `sh -c`, with:

- env: `AGENTBUS_INSTANCE` (recipient id), `AGENTBUS_ENVELOPE_ID`,
  `AGENTBUS_KIND`, `AGENTBUS_FROM`.
- timeout: 15 s, kill on expiry.
- failure policy: non-fatal. The send/ask has already succeeded (the
  envelope is durably spooled); a hook failure emits a
  `bus.delivery_hook_failed` event and a warning on stderr.

This is how cmux-bellhop wakes agents without a daemon: it registers its
agents with `on_delivery = "bellhop dispatch <name>"`.

### 6.6 await_message / check_inbox

- `check_inbox()`: rename-snapshot own inbox file, parse, return 0..N.
- `await_message(timeout)`: poll own inbox file for nonzero size (same
  backoff as 6.3), then consume as `check_inbox`.

### 6.7 sweep (crash recovery, optional)

`agentbus sweep` is a periodic CLI (launchd interval, e.g. 60 s), not a
resident. It: removes dead non-persistent instance rows; re-runs
`on_delivery` for any inbox file that is non-empty and unmodified for one
grace period (covers "sender crashed between append and hook"); reports
expired unanswered asks as `bus.ask_expired` events; with `--purge-orphans`,
deletes inbox files whose instance row no longer exists. Running it is
optional -- without it the same recovery happens lazily at the next send.

## 7. Crate restructure

| Crate | v0.1 | v0.2 |
|---|---|---|
| `agentbus-core` | envelope, ids, registry, mailbox, router, eventlog (in-memory) | envelope, ids unchanged; new `store` module (rusqlite) absorbing registry/router/eventlog semantics; mailbox deleted (inbox files are the mailbox) |
| `agentbusd` | HTTP/SSE/UDS daemon | **deleted** |
| `agentbus-stdio` | MCP shim over UDS client | MCP shim over `store` directly; `uds_client.rs` deleted |
| `agentbus-cli` | REST client | thin wrapper over `store`; gains `ask-result`, `sweep`, `register --persistent --on-delivery` |

New dependency: `rusqlite` (bundled). Deleted: axum/hyper server stack.

## 8. Error model

| Code | When |
|---|---|
| `unknown_instance` | send/ask to an id with no row |
| `instance_id_taken` | register collision with a live owner pid |
| `timeout` | ask expired unanswered (reply may still arrive; see ask-result) |
| `store_locked` | busy_timeout exhausted (pathological contention) |
| `invalid_envelope` | fr:01 validation failure, unchanged |

## 9. Security

- `~/.agentbus` is `0700`; the trust boundary is the OS user, same as
  v0.1's loopback binding in practice.
- `on_delivery` executes arbitrary commands -- registered only by the same
  user who runs senders, so it grants nothing the user lacks. Documented
  loudly in the README regardless.
- No secrets in the store; envelopes are as sensitive as the user makes
  them, same as v0.1.

## 10. FR docs impact (the kusara graph after this change)

| FR | Fate |
|---|---|
| fr:01 envelope | unchanged |
| fr:02 instance-registry | rewritten: rows, pid liveness, persistent flag |
| fr:03 mailbox | folded into fr:09 (inbox files are the mailbox) |
| fr:04 router | rewritten: sender-executed delivery, asks table |
| fr:05 eventlog | rewritten: table + seq cursor, follow polling |
| fr:06 rest-api | deleted with a superseded-by note |
| fr:07 sse | deleted with a superseded-by note |
| fr:08 mcp-shim | rewritten: direct store access |
| fr:09 hook-inbox | minor edit: writer is now the sender, contract identical |
| fr:10 cli | extended: ask-result, sweep, register flags |
| fr:11 daemon-lifecycle | deleted; replaced by new fr: store layout and locking |
| new fr:12 | store layout, schema, concurrency rules (section 5) |
| new fr:13 | on_delivery execution contract (section 6.5) |
| new fr:14 | sweep (section 6.7) |

`docs/reference/protocol.md` is rewritten around the store operations.

## 11. Testing

- Unit: store operations against a tempdir SQLite (register collisions
  with live/dead pids, ask round trip, late reply retrieval, event
  cursor continuity, flock append interleaving).
- Concurrency: N processes (spawned test helpers) hammering send/ask on
  one store; assert no lost envelopes, no duplicate seq, intact JSONL
  lines in inbox files.
- CLI: golden-output tests for each verb against a temp store.
- Hook contract: the existing rename-consume shell tests keep passing
  verbatim against sender-written inbox files.

## 12. Open questions / verify during implementation

1. flock + O_APPEND interplay on APFS for lines above 4 KiB (large
   payloads): confirm no interleaving without the lock, keep the lock
   regardless.
2. rusqlite WAL behavior when the db lives on a path some tool syncs
   (Dropbox/iCloud): document "keep ~/.agentbus on a local volume".
3. Whether `kill(pid, 0)` false-positives (pid reuse) matter at this
   scale; mitigation if so: record process start time alongside pid.
4. MCP shim lifetime: shim sessions are non-persistent registrations;
   confirm shim exit paths always reach unregister or that pid liveness
   covers abrupt kills (it should).

## 13. Consequences for cmux-bellhop

- `bellhopd` (daemon) and `beacon.rs` (HTTP receiver) are deleted from
  that design. `bellhop` becomes a pure CLI + its own small state store;
  hooks write state transitions directly via `bellhop beacon <state>`.
- Wake-on-message becomes `on_delivery = "bellhop dispatch <name>"`.
- Registration permanence comes free via `persistent` rows.
- The cmux-bellhop spec and plan will be revised after this spec is
  approved; its supervisor state machine and hook scripts carry over
  nearly unchanged.
