# agentbus: skill-first client guidance

- Date: 2026-06-07
- Status: draft, awaiting review
- Revises: fr:08 (shim role), fr:10 (CLI register), fr:02 (explicit-pid
  registration path), and decision 5 ("MCP shim") of
  `2026-06-05-spool-model-design.md`
- Adds: fr:16-usage-guidance (channel hierarchy for teaching clients the bus)

## 1. Summary

The MCP shim is a per-session token tax: every session whose client config
loads `agentbus-stdio` pays the nine tool schemas and descriptions
(~600-1500 tokens) at initialize, whether or not the session ever touches
the bus. For an always-installed coordination tool, idle sessions dominate,
so the steady-state cost is almost entirely waste. Meanwhile the knowledge
that makes an agent use the bus *correctly* (ask/reply `request_id`
semantics, self-ask deadlock, batch returns, registration lifetimes) ships
only in the Claude Code plugin skill — clients on other harnesses (Codex
et al.) see nine terse one-liners and operate blind.

This design inverts the integration: **the skill becomes the primary AI
surface and teaches the CLI; the MCP shim becomes a compatibility fallback**
for clients that cannot load skills or run shell commands. Skill-capable
clients install no MCP server at all — their idle cost drops to one skill
description line, and the full usage guidance loads only when coordination
actually happens.

## 2. Goals

- Zero MCP context cost in sessions that do not use the bus, for any
  skill-capable client (Claude Code, Codex, and other Agent Skills
  adopters).
- Self-teaching fallback for MCP-only clients: the shim alone must carry
  enough guidance for correct use, without a sidecar doc.
- One source of truth for usage knowledge; the condensed forms (shim
  instructions, error hints) derive from it.
- Session-scoped (non-persistent) registration must work without the shim
  process as the liveness anchor.

## 3. Non-goals

- Deleting the shim. MCP remains a supported surface; it is repositioned,
  not removed.
- Branching *behavior* on `clientInfo`. Client identity is unauthenticated
  free-form prose; at most it may flavor wording.
- Changing the envelope wire format, store schema, or CLI verb semantics
  beyond the `register --pid` flag.
- Cross-machine or multi-user concerns (unchanged from the spool model).

## 4. Decisions of record

| Decision | Choice | Why |
|---|---|---|
| Primary AI surface | Skill that teaches the `agentbus` CLI via shell; no MCP server installed for skill-capable clients | Skill costs one description line at boot and loads its body on use; CLI invocations carry zero schema overhead. The MCP surface costs its full schema+description payload in every session, used or not |
| Session liveness anchor without the shim | `agentbus register <id> --pid <pid>`; the skill passes the harness session pid | Core already supports an explicit pid (`RegisterOpts.pid`, fr:02); only CLI exposure is missing. Pid-liveness, collision matrix, and sweep semantics are unchanged — the row is anchored to the long-lived harness process instead of the shim |
| Loss of EOF cleanup | Accept: session end leaves a dead row, reclaimed lazily on next `register` or by `agentbus sweep` | This is already the abrupt-kill path today (fr:08 Capabilities); making it the normal path adds no new failure mode |
| Shim role | Compatibility fallback for clients with MCP but without skills+shell | Those clients have no cheaper channel; they pay the tax because nothing else reaches them |
| Shim guidance | `initialize` result gains `instructions` (condensed usage: mental model, ask/reply rule, deadlock warning, error table); new flag `--instructions=none\|minimal\|full`, default `full` | `instructions` is the MCP-native guidance channel. The flag lets a packaging that ships a skill suppress the duplicate deterministically — configuration, not client sniffing |
| Just-in-time teaching | Store error `data` strings gain recovery hints (e.g. `unknown_request_id` → "request_id must be the ask envelope's id"; `unknown_instance` → "recipient must register first; see list_instances") | Errors cost zero tokens until a mistake happens and arrive exactly when needed. Both CLI and shim inherit them from the store layer. Precedent: the `timeout` error already embeds the `ask-result` hint |
| Source of truth | `skills/agentbus/SKILL.md`; the shim instructions text is a hand-maintained condensed subset, with the relationship documented in fr:16 | Build-time generation from SKILL.md is cleaner but adds a build step for ~600 tokens of prose; revisit if drift becomes a problem |
| Plugin packaging | The Claude Code plugin ships the skill and drops `.mcp.json` (no shim registration) | The plugin is exactly the skill-capable case; shipping both channels double-pays |

### Revision of spool-model decision 5

The spool spec kept the shim because "a CLI `register` records the
invocation's pid, which exits immediately — the row is instantly dead",
concluding that dropping the shim "would require redesigning fr:02
liveness". The premise was that the CLI can only anchor to its own pid.
With `--pid`, the row anchors to any long-lived process the caller names —
for an AI session, the harness process. No liveness redesign: `kill(pid, 0)`
checks, the collision matrix, and persistent-row exemptions are untouched.
The shim remains valid as *one* way to obtain a session-scoped pid anchor;
it is no longer the only way.

## 5. Client tiers

| Client capability | Channel | Idle cost per session |
|---|---|---|
| Skills + shell (Claude Code, Codex) | skill → `agentbus` CLI | one skill description line |
| MCP only (no skills or no shell) | `agentbus-stdio` shim; tool descriptions + `instructions` + error hints | tool schemas, plus instructions unless `--instructions=none` |
| Plain processes, scripts, humans | `agentbus` CLI directly | zero |

The fork between tiers is decided at install/packaging time by whoever
configures the client — never sniffed at runtime. A configuration that
installs the skill must not also register the shim; a configuration that
registers the shim assumes no skill is present and leaves instructions on.

## 6. Skill changes

`skills/agentbus/SKILL.md` is rewritten to teach CLI invocations instead of
`mcp__agentbus__*` tools:

- Quickstart and patterns use `agentbus register/send/ask/reply/check-inbox/
  await/publish/events` (Bash), keeping the same mental model, gotchas, and
  error table.
- Registration pattern becomes
  `agentbus register <id> --pid <session-pid>`, with harness-specific
  guidance for obtaining the pid (e.g. the parent pid of a spawned shell).
  `--persistent` remains the alternative for durable addresses.
- Blocking calls (`ask`, `await`) must advise a `--timeout-ms` below the
  harness's shell command timeout, looping for longer waits.
- The Claude-specific patterns (SessionStart hook-injected inbox, `watch`
  under a session monitor) stay, marked as harness-dependent.
- The skill body is the portable artifact: the same content ships to other
  Agent Skills hosts (e.g. Codex) modulo those harness-dependent sections.

## 7. FR deltas (applied with the implementation, per repo convention)

| FR | Delta |
|---|---|
| fr:02-instance-registry | Document the CLI as a second explicit-pid caller. No schema or semantics change; the "caller supplies an explicit pid via `RegisterOpts`" path is now user-reachable |
| fr:08-mcp-shim | Purpose reframed: fallback surface for MCP-only clients, no longer the canonical session anchor. Add: `instructions` in initialize result, `--instructions` flag. When-to-update gains the instructions text and flag |
| fr:10-cli | `register` gains `--pid <pid>`. The Boundaries note "meaningful only with `--persistent`; long-lived processes should register through the shim" is rewritten: session-scoped registration via `--pid` is the primary AI path |
| fr:12-store | Error `data` strings gain recovery hints; stable `message` codes unchanged |
| fr:16-usage-guidance (new) | Design of record for the channel hierarchy (skill > errors > instructions > descriptions), the suppression flag contract, and the SKILL.md-as-source-of-truth relationship. `modules:` lists `skills/agentbus/SKILL.md` and the shim instructions source file |
| ref:protocol | `--instructions` flag, `register --pid`, error-hint wording |

Also outside the graph: `README.md` repositions the shim section;
`.claude-plugin` packaging drops the shim registration; the stale
`commands/install.md` (still daemon-era) is updated or removed in passing.

## 8. Open questions

1. **Meta-tool collapse.** Folding nine tools into one `agentbus(op, ...)`
   tool would cut the fallback tier's schema payload to ~150 tokens, but
   breaks every existing MCP client config and makes per-op schemas
   unenforceable. Deferred — the fallback tier's tax is paid only by
   clients with no alternative, so the win is small.
2. **Codex skill distribution.** The skill content is portable, but the
   repo's plugin layout is Claude-specific. Decide whether to publish a
   second packaging (Codex skills directory layout) from the same
   SKILL.md, and where it lives in-repo.
3. **Session pid discovery.** `$PPID` from a spawned shell reaches the
   harness process on the harnesses checked, but intermediary wrapper
   processes would break it. The skill documents it as a heuristic; the
   contract is only "the row binds to the pid you name". Is a
   `--pid auto` (walk up to the nearest long-lived ancestor) worth it?
4. **Pid reuse window** (fr:02 open question): binding rows to harness
   pids slightly widens exposure to pid reuse since harness processes are
   not bus-aware. The existing mitigation plan (record process start time)
   covers it; unchanged priority.

## 9. Migration

No store, envelope, or wire changes — existing registrations, spools, and
the event log are untouched. Existing shim users keep working (default
`--instructions=full` only adds guidance). The plugin update swaps the
shim registration for the rewritten skill in one release; sessions that
still have a stale `.mcp.json` entry merely pay the old tax until they
remove it.
