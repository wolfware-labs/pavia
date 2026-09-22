---
applyTo: "spec/**"
---

Reviewing a change to the specification:

- Diff `spec/pavia-protocol.md`; the new file under `spec/versions/` must be byte-identical to it, its directory name must match the `**Version:**` line, and it must be listed in `spec/README.md`. Any change to an existing file under `spec/versions/` is a defect.
- The changelog entry at the end of the document must list every section touched.
- Normative words (MUST, SHOULD, MAY) follow RFC 2119; check that a new MUST has a matching vector or script under `vectors/`, or that the PR says why not.
- Registries in sections 5.6, 5.7 and 15 must stay consistent with the prose: a new header key needs a row, a removed one is marked unassigned, and unassigned keys below 1024 are never reused by extensions.
- Defaults in section 16 must agree with the sections they reference.
- Open questions in section 18 carry an explicit decision or deferral when they affect the milestones under way.
