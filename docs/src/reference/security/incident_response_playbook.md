# Incident Response Playbook

Status: active

## Principles

- Stay calm and methodical.
- Prioritize user safety and data integrity.
- Preserve evidence and timeline fidelity.
- Communicate clearly with affected stakeholders.

## Phase 1: Intake and Triage

1. Acknowledge report within 72 hours.
2. Classify report scope and potential impact.
3. Reproduce with minimal proof-of-concept.
4. Open private tracking thread and assign incident owner.
5. Choose disclosure strategy:
    - patch and advisory together
    - advisory first, patch later (active exploitation or unavoidable early disclosure)

## Phase 2: Investigation

1. Identify affected crates, versions, and platforms.
2. Estimate confidentiality, integrity, and availability impact.
3. Determine whether issue generalizes to related components.
4. Capture exploitation preconditions and likely attacker capability.
5. Draft remediation plan with test coverage requirements.

## Phase 3: Remediation and Release

1. Implement minimal-risk fix.
2. Add regression tests and negative tests.
3. Validate with workspace checks and targeted scenario tests.
4. Prepare advisory metadata and impacted version ranges.
5. Release fixed versions and publish advisory.

## Phase 4: Post-Release Follow-up

1. Monitor issue trackers for regressions.
2. Verify published advisory accuracy and consistency.
3. Update follow-up guidance if exploitability assumptions change.

## Phase 5: Retrospective

1. Write root-cause analysis.
2. Record process/tooling improvements.
3. Feed lessons into threat model and secure coding guidance.

## Roles

- Incident owner: coordinates timeline and decisions.
- Fix lead: implements and validates patch.
- Reviewer: verifies correctness and security posture.
- Communications lead: manages disclosure updates.
