---
refs:
  id: fr:12-store
  kind: fr
  title: "Spool store layout, schema, and concurrency"
  related:
    - fr:01-envelope
    - fr:02-instance-registry
    - fr:09-hook-inbox
  modules:
    - crates/agentbus-core/src/store/mod.rs
    - crates/agentbus-core/src/store/paths.rs
    - crates/agentbus-core/src/store/error.rs
---

# FR 12: Spool store layout, schema, and concurrency

> The shared local store that all bus participants read and write: directory
> layout, SQLite schema, WAL concurrency rules, and the error model.

## Purpose

agentbus v0.2 has no daemon. All state lives in a single 0700 directory
(default `~/.agentbus`) that any participant on the same OS user opens
directly. This FR is the design of record for everything that defines
"what is on disk and why".

## User-visible Behavior

### Directory layout

```
~/.agentbus/                  (0700)
  bus.db                      SQLite, WAL mode, busy_timeout=5000
  inbox/                      (0700)
    <instance_id>.jsonl       append-only spool, consumed by rename
```

`$AGENTBUS_DIR` overrides the root when set. Absent `$HOME`, the
implementation falls back to the OS temp directory (see Boundaries).

`paths::ensure_layout` creates both directories and tightens them to `0700`
on every `Store::open` call, even when they already exist. This closes the
window where an external tool loosens permissions between runs.

### Schema

```sql
CREATE TABLE IF NOT EXISTS instances (
  id            TEXT PRIMARY KEY,
  pid           INTEGER,
  persistent    INTEGER NOT NULL DEFAULT 0,
  on_delivery   TEXT,
  registered_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS asks (
  request_id       TEXT PRIMARY KEY,
  from_id          TEXT NOT NULL,
  to_id            TEXT NOT NULL,
  expires_at       TEXT NOT NULL,
  reply_payload    TEXT,
  replied_at       TEXT,
  expired_notified INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS event_log (
  seq      INTEGER PRIMARY KEY AUTOINCREMENT,
  ts       TEXT NOT NULL,
  envelope TEXT NOT NULL
);
```

The `asks.expired_notified` column is not in the spec section 5.1 schema;
it was added during implementation so that `sweep` can report each expired
ask exactly once without re-scanning the whole table.

Envelope `id` is a ULID (`ids.rs`, unchanged). Global ordering and replay
cursors use `event_log.seq`, assigned transactionally and therefore immune to
wall-clock skew between writer processes.

### Pragmas

```sql
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA busy_timeout=5000;
```

These are applied via `execute_batch` so all three run atomically before any
DML. `execute_batch` bypasses the prepared-statement cache and is the correct
rusqlite API for multi-statement pragma sequences.

WAL mode enables concurrent readers alongside a writer. `synchronous=NORMAL`
commits without an fsync on every write (a crash can lose the last wal frame,
but the DB is never corrupted). `busy_timeout=5000` means SQLite retries for
up to 5 seconds before returning `SQLITE_BUSY` or `SQLITE_LOCKED`.

### Concurrency rules

- All `bus.db` mutations run inside one `BEGIN IMMEDIATE` transaction per
  operation (`Store::with_tx`). `BEGIN IMMEDIATE` acquires a write lock
  upfront; WAL serializes writers; `busy_timeout` absorbs contention.
- Inbox appends take an exclusive `flock` on the inbox file around a single
  `O_APPEND` write of one complete line, then release. See fr:09-hook-inbox
  for the full append-vs-consume protocol.
- Consumers rename the inbox file before reading it; writers only append.
  The rename is atomic on the local filesystem.

### Security posture

`~/.agentbus` at `0700` is the trust boundary. Only the OS user who created
the directory can read or write the store; no process running as another user
can reach the bus. No secrets belong in the store; envelope payloads are as
sensitive as the caller makes them. See the spec (section 9) for the full
security rationale. Keep `~/.agentbus` on a local volume: WAL mode is
unreliable on network or cloud-synced filesystems (Dropbox, iCloud).

## Capabilities

- Single-directory layout, overridable with `$AGENTBUS_DIR`.
- 0700 permissions enforced on every open, even when dirs already exist.
- Three-table schema (`instances`, `asks`, `event_log`) sufficient for all
  v0.2 bus semantics.
- `AUTOINCREMENT` seq on `event_log` guarantees monotone, gap-free ordering
  across all writer processes.
- `busy_timeout=5000` tolerates up to 5 s of write contention without
  returning an error.

## Boundaries

- rusqlite is pinned to 0.32 (not 0.40); the `cfg_select!` macro used in
  0.40 requires an unstable Rust feature and breaks MSRV 1.85. Upgrade when
  the feature stabilizes.
- When `$HOME` is unset and `$AGENTBUS_DIR` is not set, the store falls back
  to the OS temp directory. This is a convenience for test harnesses; it is
  not a supported production configuration.
- Per-inbox `flock` behavior on APFS for lines above 4 KiB has not been
  exhaustively verified (spec open question 1). The lock is kept regardless.
- WAL on cloud-synced folders (Dropbox, iCloud Drive) is unsupported (spec
  open question 2). Keep `~/.agentbus` on a local volume.
- Liveness checking via `kill(pid, 0)` is susceptible to pid reuse
  (spec open question 3). Mitigation (recording start time) is not yet
  implemented.
- This FR covers layout, schema, pragmas, and the error model. Specific
  operations (send, ask, reply, sweep, watch) are covered by fr:04-router,
  fr:13-on-delivery, fr:14-watch, and fr:15-sweep.

## Error Handling

| Code | Variant | When |
|---|---|---|
| `unknown_instance` | `StoreError::UnknownInstance(id)` | send/ask/reply finds no row for the target |
| `instance_id_taken` | `StoreError::InstanceIdTaken(id)` | register collides with a live owner pid |
| `invalid_instance_id` | `StoreError::InvalidInstanceId` | id fails `[A-Za-z0-9_.:-]{1,128}` validation |
| `timeout` | `StoreError::Timeout(request_id)` | ask polling deadline elapsed |
| `unknown_request_id` | `StoreError::UnknownRequestId(id)` | reply/ask-result finds no asks row |
| `store_locked` | `StoreError::StoreLocked` | `busy_timeout` exhausted (`SQLITE_BUSY` or `SQLITE_LOCKED`) |
| `invalid_envelope` | `StoreError::InvalidEnvelope(e)` | fr:01 validation failed |
| `io` | `StoreError::Io(e)` | filesystem I/O error (inbox, layout creation) |
| `sqlite` | `StoreError::Sqlite(e)` | SQLite error not mapped to a named code |

`StoreError::code()` returns the stable wire string for each variant.
`StoreLocked` is mapped from both `DatabaseBusy` and `DatabaseLocked` rusqlite
errors, so it covers both `SQLITE_BUSY` and `SQLITE_LOCKED`.

## Traceability

- Related FR: fr:01-envelope, fr:02-instance-registry, fr:09-hook-inbox
- Spec sections: 5 (storage layout), 8 (error model), 9 (security)

## When to update

- The directory layout changes (new subdirectory, new file type).
- Schema changes (new table, new column, column type change).
- Pragmas change (WAL settings, busy_timeout value).
- A new `StoreError` variant is added or its `code()` string changes.
- The `$AGENTBUS_DIR` / `$HOME` fallback logic changes.
- Permissions policy changes (0700 tightening logic).
- rusqlite is upgraded and the pinning rationale changes.
