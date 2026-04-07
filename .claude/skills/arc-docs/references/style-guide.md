# arc-vcs Documentation Style Guide

## Voice and tone

- Second-person active: "Run `arc snap`" not "You should run" or "The user runs".
- Present tense: "arc stores" not "arc will store".
- Confident but not boastful: describe what arc does, not how amazing it is.
- Plain language first. Technical precision second.

## Terminology

Always use these terms consistently:

| Use | Avoid |
|---|---|
| change | commit, patch, diff |
| snapshot | commit, save |
| semantic diff | text diff, line diff |
| causal graph / DAG | history, log (alone) |
| sponsor / approve | merge, accept |
| ghost node | draft, WIP |
| content-addressed store | CAS (first use must spell out) |
| BLAKE3 hash | SHA, hash (generic) |

## Code blocks

- Always specify the language: ` ```rust ` not ` ``` `.
- Arc CLI examples use ` ```sh `.
- Blocks that are conceptual and not runnable are labeled with `# conceptual`.
- Long expected output can be truncated with `# ... (truncated)`.

## Admonitions (mdBook)

Use admonitions sparingly and consistently:

```
> **Note:** Supplementary information a reader can skip.
> **Warning:** Something that could cause data loss or confusion.
> **Enterprise:** Enterprise-specific context or requirement.
> **Planned:** Feature not yet implemented. Links to tracking issue.
```

## Diagrams

- Use Mermaid for all architecture and flow diagrams.
- Every diagram has a text-alternative summary immediately below it.
- Diagram titles use sentence case.
- Keep diagrams small and focused; split complex flows across two diagrams.

## Headings

- H1: page title only, sentence case.
- H2: major sections, sentence case.
- H3: subsections, sentence case.
- Never skip levels.
- Do not use bold text as a substitute for a heading.

## Links

- Use descriptive link text: [semantic diff concept](../concepts/semantic-changes.md)
  not [click here](../concepts/semantic-changes.md).
- All internal links are relative paths, not absolute URLs.
- External links open in the same tab (mdBook default).
