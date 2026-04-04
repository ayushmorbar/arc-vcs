# Troubleshoot Sync

Use this guide when `fetch`, `pull`, `push`, or `sync` behaves unexpectedly.

## Quick Checks

1. Confirm repository integrity:

```sh
arc verify
```

2. Confirm current view and recent operations:

```sh
arc status
arc op log
```

3. Confirm remote aliases:

```sh
arc remote list
```

## Native Sync Issues (`arc fetch`, `arc pull`, `arc sync`)

### Symptom: remote path or view not found

- Verify the path exists and points to an arc repository.
- Verify the view exists on remote side.
- Retry with explicit path and view names.

### Symptom: missing blob conflict during sync

- This indicates metadata arrived before one or more blob payloads.
- Retry operation; uploader should send required blobs before final apply.
- If persistent, inspect transport logs with telemetry enabled.

### Symptom: signature verification failure

- Run `arc verify` locally and on remote peer.
- Ensure no partial/corrupted transfer artifacts remain.
- Re-run fetch from a clean transport session.

## Git Remote Push Issues (`arc push` over HTTP/HTTPS)

### Symptom: receive-pack rejects update

- Check remote branch protection and fast-forward policy.
- Validate credentials and remote URL.
- Confirm destination ref name and permissions.

### Symptom: refs discovery fails

- Confirm endpoint is a Git Smart HTTP remote.
- Check proxy or TLS interception settings.
- Retry with a direct URL and network diagnostics.

## Observability

Enable trace output during reproduction:

```sh
ARC_TRACE=1 arc pull <remote> <view>
```

Structured JSON events:

```sh
ARC_TRACE_EVENT=./arc-trace.jsonl arc push <remote>
```

## Escalation

If issue persists, generate a bug report package:

```sh
arc bug-report --output ./arc-bugreport.json
```

Attach command used, trace snippets, and exact error message in your issue report.
