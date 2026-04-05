---
title: Debugging
description: Documentation page for Debugging.
---

# Debugging and Hyper-Observability

Status: Stable
Audience: Developers, maintainers, and incident triage owners

Arc provides low-friction runtime observability so you can diagnose behavior without patching code first.

## Telemetry Modes

### Compact terminal trace

```sh
ARC_TRACE=1 arc pull origin main
```

Use this when reproducing failures interactively.

### Structured JSON event trace

```sh
ARC_TRACE_EVENT=./arc-trace.jsonl arc pull origin main
```

Use this when you need machine-parseable logs for issue reports or CI artifacts.

## What You Can Diagnose

### AST parse or semantic diff failures

Run the failing command with `ARC_TRACE=1`, then capture the exact file/path and operation where parser validation fails.

### CAS/blob integrity issues

Start with:

```sh
arc verify
ARC_TRACE=1 arc fetch origin main
```

`arc verify` currently validates change-signature integrity in the graph. Use trace output from `fetch`/`push`/`pull` to diagnose blob read/write path issues and missing object transfer symptoms.

### Network sync timeouts or transport instability

Use structured output to correlate retries and endpoint activity:

```sh
ARC_TRACE_EVENT=./arc-trace.jsonl arc push origin main
```

## Minimal Repro Template

Copy this into your issue:

```text
## Environment
- OS:
- Arc version:
- Repo state (new/existing, approximate size):

## Command
	<exact command>

## Expected
<what should have happened>

## Actual
<what happened>

## Trace
- `ARC_TRACE=1` excerpt:
- `ARC_TRACE_EVENT` file attached: yes/no

## Integrity Check
	arc verify
Output:
<paste output>
```

## Optional Bug Report Bundle

```sh
arc bug-report --output ./arc-bugreport.json
```

Attach `arc-bugreport.json` and the trace file when filing the issue.
