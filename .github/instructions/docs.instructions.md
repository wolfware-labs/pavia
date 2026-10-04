---
applyTo: "docs/**,ROADMAP.md,CONTRIBUTING.md,README.md"
---

Reviewing design records and project documents:

- A decision record states the problem, the rule adopted and the rejected options with a reason each. Changing an accepted decision needs a new entry or an explicit amendment, not a silent rewrite.
- `ROADMAP.md` capability lines and tests agree with the current spec draft and with the decision records; a stale rule (one a later draft changed) is a finding.
- Section references point at sections that exist in the current draft and say what the section actually says.
- Crate names, dependency rules and ownership agree with `docs/design/sans-io.md` and `scripts/check-deps.sh`.
