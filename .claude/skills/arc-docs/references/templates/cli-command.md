# CLI Command Page Template

Use this template for each command under `docs/src/user-guide/`.

---

```markdown
# `arc <command>`

[One sentence: what this command does and when you use it.]

## Syntax

```sh
arc <command> [OPTIONS] [ARGS]
```

## Arguments

| Argument | Description | Required |
|---|---|---|
| `<ARG>` | [What it is] | Yes / No |

## Options

| Flag | Short | Default | Description |
|---|---|---|---|
| `--flag` | `-f` | `false` | [What it does] |

## Examples

### [Most common use case title]

```sh
arc <command> example-value
```

Expected output:

```
[Realistic expected output]
```

### [Second example: edge case or power-user usage]

```sh
arc <command> --flag value
```

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | [Specific failure condition] |

## Related commands

- [`arc <other>`](<other>.md) — [how they relate]
```
