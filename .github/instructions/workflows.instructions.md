---
applyTo: ".github/workflows/**,.github/dependabot.yml,scripts/**"
---

Reviewing CI and scripts:

- Third-party actions are pinned to a full commit SHA with the version in a comment.
- `permissions` is the minimum the job needs, declared per job; the workflow default is empty or read-only.
- No job that checks out pull request code has write permissions or secrets beyond what it needs; workflows triggered by `pull_request_target` or `issue_comment` never execute code from the pull request.
- Tools installed in CI are pinned to a version.
- Required checks (`rust`, `typescript`, `vectors`) keep their job names, since the branch ruleset refers to them by name.
- Concurrency groups do not let a job's own side effects (comments, pushes) cancel the run that produced them.
- Scripts run with `set -euo pipefail` (or the equivalent) and fail loudly.
