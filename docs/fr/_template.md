# FR template

Copy the sections below into `NN-<slug>.md`. Start the new file with real
`refs:` frontmatter of this shape (delete the example fence — the frontmatter
must be the first thing in the file, `---` delimited):

```yaml
---
refs:
  id: fr:NN-<slug>
  kind: fr
  title: "<title>"
  related:
    - fr:XX-other
  modules:
    - crates/agentbus-core/src/foo.rs
---
```

---

# FR NN: <feature name>

> One-line summary.

## Purpose
Why this exists; the operator/caller contract.

## User-visible Behavior
What a caller observes — surfaces, semantics, ordering.

## Capabilities
What the feature guarantees, as bullets.

## Boundaries
Explicit non-responsibilities — prevents AI inference errors.

## Error Handling
The error cases owned by this FR.

## Traceability
- Reference docs: ref:protocol
- Related FR: fr:XX-*

## When to update
Concrete triggers that make this doc stale.
