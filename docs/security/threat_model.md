# arc Threat Model

Status: living document

## Security Objectives

1. Preserve repository integrity for semantic history, graph state, and tags.
2. Prevent unauthorized code execution through repository content or tooling hooks.
3. Preserve availability under malformed, adversarial, or oversized inputs.
4. Protect user trust through deterministic, transparent remediation of security defects.

## Critical Assets

- Content-addressed objects and graph metadata under `.arc/store`.
- Author identity material and signature verification paths.
- Working directory safety boundaries enforced during materialization.
- Configuration values that can trigger side effects.

## Trust Boundaries

### Untrusted Inputs

- Remote repositories, peers, and network transports.
- Local repositories acquired from archives or third-party storage.
- Working tree content and files generated from untrusted history.
- Environment variables and external process resolution paths.

### Trusted Inputs

- Maintainer-reviewed source and release artifacts.
- Local policy files in trusted repository scope.
- Explicit allowlists in protected configuration scopes.

## STRIDE Summary

| Component                    | Threat                                          | STRIDE | Mitigation                                                                                |
| ---------------------------- | ----------------------------------------------- | ------ | ----------------------------------------------------------------------------------------- |
| CAS ingestion                | Malformed payload causes panic or invalid state | D, T   | strict decoding, bounded parsing, fail-closed checks                                      |
| Working tree materialization | Path traversal or reserved-path abuse           | T, E   | path canonicalization, deny traversal, platform-specific path guards                      |
| Identity and signatures      | signature bypass or key confusion               | S, T   | explicit key loading rules, mandatory verification, rejection on mismatch                 |
| External command integration | attacker-controlled executable resolution       | S, E   | explicit command allowlists, trusted-path execution, no implicit CWD execution            |
| Sync protocol surface        | replay or malformed message abuse               | T, D   | protocol validation, message size/resource bounds, authenticated channels where available |

## Operational Mitigations

1. Validate all untrusted inputs at boundaries.
2. Keep dangerous operations behind explicit user intent.
3. Use defense-in-depth checks for filesystem writes.
4. Maintain reproducible security release and advisory flow.
5. Capture platform-specific behavior differences in test harness known-failure registries.

## Review Triggers

Update this document when any of the following change:

- object encoding/decoding formats
- sync protocol framing or trust assumptions
- hook/tool execution model
- repository path handling rules
- key management and signature semantics
