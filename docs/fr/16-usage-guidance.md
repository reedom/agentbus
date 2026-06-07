---
refs:
  id: fr:16-usage-guidance
  kind: fr
  title: "Usage guidance channels"
  related:
    - fr:08-mcp-shim
    - fr:10-cli
    - fr:12-store
  modules:
    - skills/agentbus/SKILL.md
    - crates/agentbus-stdio/src/instructions.rs
---

# FR 16: Usage guidance channels

> How clients learn to use the bus correctly, at the lowest possible
> context cost per session.

## Purpose

The knowledge that makes an agent use the bus correctly (ask/reply
request_id semantics, self-ask deadlock, batch returns, registration
lifetimes) must reach every client kind without taxing sessions that
never touch the bus. This FR is the design of record for the channel
hierarchy and the suppression contract. Design rationale:
`docs/superpowers/specs/2026-06-07-skill-first-guidance.md`.

## User-visible Behavior

Guidance reaches a client through the cheapest channel it supports,
in this order:

| Channel | Cost profile | Carrier |
|---|---|---|
| Skill (`skills/agentbus/SKILL.md`) | one description line per session; body loads on use | skill-capable harnesses; teaches the CLI |
| Error detail strings | zero until a mistake; arrives exactly when needed | `StoreError` Display (fr:12-store), all surfaces |
| MCP `instructions` | paid at initialize by shim sessions | `agentbus-stdio` (fr:08-mcp-shim) |
| MCP tool descriptions | paid at initialize by shim sessions | `agentbus-stdio` tool specs |

- Skill-capable clients (Claude Code, Codex) use the skill + CLI and do
  not load the shim at all.
- MCP-only clients load the shim and receive condensed guidance via the
  initialize `instructions` field, suppressible with
  `--instructions=none|minimal|full` (default `full`).
- The fork between the two is decided at install/packaging time by
  whoever configures the client — never sniffed at runtime. A
  configuration that installs the skill must not also register the shim.

## Capabilities

- `skills/agentbus/SKILL.md` is the single source of truth for usage
  prose. The shim's `instructions` text
  (`crates/agentbus-stdio/src/instructions.rs`) is a hand-maintained
  condensed subset of it.
- Error Display strings carry recovery hints (fr:12-store), so even an
  instructions-suppressed, skill-less client self-corrects in one
  roundtrip.
- Tool descriptions stay terse one-liners; correctness teaching lives in
  the channels above.

## Boundaries

- No runtime client detection: `clientInfo` is unauthenticated free-form
  prose and must not gate behavior.
- The shim's nine-tool surface is not collapsed into a meta-tool (spec
  open question 1; deferred).
- No second skill packaging for non-Claude hosts exists yet (spec open
  question 2); the SKILL.md body is written host-neutrally so one can be
  derived.

## Error Handling

None of its own; the recovery-hint wording is owned by fr:12-store.

## Traceability

- Reference docs: ref:protocol
- Related FR: fr:08-mcp-shim, fr:10-cli, fr:12-store
- Spec: docs/superpowers/specs/2026-06-07-skill-first-guidance.md

## When to update

- A guidance channel is added or removed (e.g. a meta-tool, a second
  skill packaging).
- The `--instructions` levels or default change.
- SKILL.md's mental model or gotchas change (the condensed shim text must
  be re-derived).
- The packaging fork (skill vs shim) gains any runtime detection.
