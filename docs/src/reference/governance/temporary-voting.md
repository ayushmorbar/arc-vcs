# Temporary Community Approval Process for ARC Governance Proposals

## Introduction

This document defines a temporary process for how ARC will gather community
input and approve early governance proposals.

It is intentionally transitional. Once ARC adopts permanent governance
documentation, this process will be retired and replaced by that permanent
model.

> **Placeholder for future evolution:** ARC may replace, merge, or redesign this
> process once project scale, contributor structure, and decision needs are
> clearer.

## Why This Exists

ARC is still establishing formal governance. Early policy decisions should not
be made by a small group without clear community participation. This process
creates a transparent path for:

- Proposal drafting
- Public feedback
- Time-bounded voting
- Clear adoption or rejection outcomes

## Scope

This temporary process is for governance and project-process proposals, such as:

- `governance.md` and role definitions
- Decision-making and escalation rules
- Review and merge policy
- Maintainer selection or rotation policy
- Community standards and participation policy

This process is **not** intended for routine technical changes already covered by
normal engineering workflows.

## Participation

For this process, “community participants” include people actively contributing
to ARC, including:

- Code contributors and maintainers
- Reviewers and release contributors
- Documentation contributors
- Tooling and ecosystem maintainers
- Support and triage contributors
- Community members providing consistent, actionable feedback

If someone contributes meaningfully but does not fit a category above, they
should be treated as eligible by default unless there is a documented reason not
to.

## Process

### Stage 1: Early Notice

The proposal authors announce intent before publishing a final draft.

**Minimum timing:** at least 7 days before voting opens.

Authors should provide:

- The problem being solved
- Intended outcomes
- Known tradeoffs or constraints
- Link to a canonical discussion thread (GitHub)

Community is invited to provide:

- Additional goals
- Risks and edge cases
- Alternative approaches

### Stage 2: Draft Review

Authors publish the full proposal text as a pull request and link it to the
canonical discussion thread.

**Minimum timing:** at least 72 hours between draft publication and vote start.
**Typical duration:** 7+ days.

Authors should:

- Explain how the draft addresses Stage 1 goals
- Respond to substantive feedback
- Revise text where appropriate

Community should:

- Provide concrete, actionable edits
- Raise blocking concerns with rationale
- Identify ambiguity, enforceability, and operational risk

At the end of this stage, authors either:

- Move proposal to vote, or
- Withdraw/rework the proposal

### Stage 3: Voting

If authors determine the proposal is ready, they open a vote.

Voting rules:

- Voting is hosted on GitHub (poll or equivalent public mechanism)
- Voting window is fixed at start and cannot be shortened mid-vote
- Minimum open period: 7 days
- Recommended maximum: 14 days for broad participation
- Votes are **For** or **Against**
- “Against” votes should include required changes for support where possible

**Approval threshold:** proposal passes with at least **2/3 For** votes among
valid votes cast by eligible participants.

### Stage 4: Outcome and Implementation

After voting closes:

- **If passed:** proposal is merged and becomes active project policy.
- **If not passed:** proposal is either revised (return to Stage 2) or closed.

If implementation reveals major practical issues, ARC may open a follow-up
proposal under this same temporary process.

## Transparency and Records

For each governance proposal, ARC should maintain:

- Canonical discussion link
- Proposal PR link
- Vote window and result summary
- Final disposition (accepted/rejected/withdrawn)
- Implementation status

## Temporary Status

This document is a bridge, not a final constitution.

> **Placeholder:** ARC will define a permanent governance framework and a formal
> transition plan in a future revision.
