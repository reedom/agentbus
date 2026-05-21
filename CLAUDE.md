# agentbus

An MCP-native message bus. See `docs/README.md` for the product overview.

## Documentation model

Product features are documented as Functional Requirements (FRs) under
`docs/fr/`, tracked by the `kusara` cross-reference graph. Reference docs
(protocol, integration examples) live under `docs/reference/`.

- `docs/kinds.md` — kusara doc-kind manifest (`fr`, `reference`).
- `.claude/rules/refs.md` — authoritative `refs:` frontmatter schema.
- `docs/fr/README.md` — how to read and add FRs.

FR docs are the design of record for already-implemented code: each FR's
`modules:` frontmatter lists the source paths it governs. When code and an FR
disagree, the FR documents the actual code behavior, with a Boundaries note
where the original design intent is not yet implemented.

## Keeping docs fresh

- The `.claude/hooks/refs-postedit.sh` PostToolUse hook runs automatically:
  `kusara validate` after any `.md` edit, `kusara touched` after any
  `crates/` edit (reporting which FR owns the changed module).
- When you change behavior under `crates/`, update the FR that lists that
  file in its `modules:` frontmatter.
- `kusara index` regenerates `docs/fr/index.md` and `docs/reference/index.md`.
- `/kusara:check` audits the graph; `/kusara:sync` syncs docs after a batch
  of changes. Install the kusara binary with `/kusara:setup` or
  `cargo install kusara`.
