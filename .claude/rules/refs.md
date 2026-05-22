---
description: "Doc cross-reference graph: judgment calls the post-edit hook cannot make. Trigger-specific excerpts of this rule are auto-injected by .claude/hooks/refs-postedit.sh."
paths: **/*.md
---

# Cross-reference rule

agentbus documentation is a `kusara` cross-reference graph. Two doc kinds are
tracked (see `docs/kinds.md`):

- `fr` — functional requirements under `docs/fr/[0-9]*.md`, ids `fr:NN-<slug>`.
- `reference` — protocol and integration docs under `docs/reference/[a-z]*.md`,
  ids `ref:<slug>`.

The post-edit hook (`.claude/hooks/refs-postedit.sh`) auto-runs `kusara validate`
after every `.md` edit and `kusara touched <file>` after every source edit under
`crates/`. When an edit matches one of the triggers below, the hook also injects
the matching excerpt of this rule into context.

For the schema and CLI behavior of `kusara` itself, invoke the
`kusara:refs-schema` and `kusara:kinds-manifest` skills.

**This rule covers only what the hook cannot decide.** Do not re-run `validate`
or `touched` manually unless investigating a specific failure.

## refs: frontmatter

Every graph-tracked `.md` begins with `---` delimited YAML:

```yaml
---
refs:
  id: fr:NN-<slug>          # required, globally unique, prefix = kind
  kind: fr                  # required, must match a kind in docs/kinds.md
  title: "..."              # optional
  related:                  # optional, weak see-also links (bidirectional)
    - fr:XX-other
    - ref:protocol
  modules:                  # optional (fr docs): source paths this doc is
    - crates/.../foo.rs     #   the design of record for; file or dir prefix
---
```

## Trigger: creating a new `.md`

Pick the kind by matching the file path against `docs/kinds.md` `path_globs`:

| Situation | Action |
|---|---|
| Path matches an existing kind's `path_globs` | Add `refs:` of that kind. Mirror the closest sibling's frontmatter shape. |
| New instance of an existing kind, in a new location | Extend that kind's `path_globs` in `docs/kinds.md`, then add `refs:`. |
| New category of doc | Add a new kind entry to `docs/kinds.md` (name + `path_globs` + `id_pattern`; add `index.output` only if a generated index is wanted), then add `refs:`. |
| Deliberately outside the graph (README / template / generated) | No `refs:`. Tighten the kind's glob if validate now complains. |

## Trigger: source change under any `modules:` path

The hook prints the docs of record via `kusara touched`. Decide per change type:

| Change type | Doc update? |
|---|---|
| Internal: refactor, bugfix, perf, dependency bump | none |
| New / changed REST endpoint, MCP tool, CLI subcommand, wire field, or config var | the relevant `reference` doc (`ref:protocol`) |
| New / removed end-to-end behavior | the owning `fr:NN-*` |

If unsure whether a change is "public surface", err on the side of updating the
doc.

## Trigger: rename / delete of a doc

Every reference to the old ID elsewhere in the graph becomes dangling. Update
the references; the hook surfaces leftovers on the next edit.

## Trigger: editing the body of a graph-linked `.md`

The hook surfaces the doc's immediate `related` / `modules` plus reverse direct
impact via `kusara show`. Treat the surfaced list as a **content-drift
checklist**: if this edit changed observable behavior (not a typo or prose-only
tweak), each linked doc may need a matching wording update. The validator cannot
detect prose drift — only the reader of the diff can. Skip the sweep for typos,
formatting, or pure clarification.

## Trigger: editing `docs/kinds.md`

| Edit type | Risk |
|---|---|
| Tightening a `path_globs` | safe |
| Loosening or adding new globs | every newly-matched file must have `refs:` (validator enforces) |
| Renaming a kind | invalidates every `kind: <old-name>` in existing front matter; audit + rewrite |
| Adding `index.output` | run `kusara index` once to materialize the file |

## What the hook handles (do NOT re-run unless debugging)

- `kusara validate` after every `.md` edit.
- `kusara touched <file>` after every `crates/` or `docs/kinds.md` edit.
- All graph-integrity errors (dangling refs, dup IDs, unknown kinds,
  missing-frontmatter under a declared glob, missing `modules:` paths).

## Schema pointers

- Kind manifest: `docs/kinds.md`
- Tool schema: the `kusara:refs-schema` and `kusara:kinds-manifest` skills
- FR conventions: `docs/fr/README.md`
