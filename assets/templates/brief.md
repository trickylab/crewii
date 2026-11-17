## BRIEF: Definition of Ready

Provide these fields:

- Goal (required): desired outcome.
- Repo (required): repository to inspect or change.
- Branch/base (required): branch or revision to start from.
- Scope in (required): work included; name the files, packages, or areas when known.
- Scope out (required): work excluded.
- Acceptance criteria (required): observable conditions for completion, each with its check: a command and its expected result, or a manual step and the expected observation.
- Context (optional): relevant background and references.
- Constraints (optional): limits or rules beyond project rules.
- Deliverable (required): expected output.

Add the following when applicable:

- Reviewer: diff range or pull request.
- QA: target and environment.
- Writer: expected behavior and how to verify it.

If a required field or applicable role extra is missing, or an acceptance criterion has no check, reply once listing all missing items. Do not guess. Wait for the missing information before starting.

## Report format

Every result, from a worker or a lead, uses this format:

- One line per acceptance criterion: `met` or `not met`, the criterion, then the evidence: the command you ran and its exit code, or the manual check and what you observed.
- Changed files: each file changed, or "none".
- Out of scope: changed files outside Scope in, with the reason, or "none".
- Notes: non-blocking findings, or "none".
- Open questions, or "none".
