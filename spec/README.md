# Pavia specification

This directory holds the protocol specification and every published version of it.

## Layout

```
spec/
  README.md                          this file: policy and version index
  pavia-protocol.md                  editor's draft (the current text, stable path for links)
  versions/
    draft-0.2/pavia-protocol.md      frozen snapshot
    draft-0.3/pavia-protocol.md      frozen snapshot
    1.0/pavia-protocol.md            frozen snapshot, first stable release
```

`pavia-protocol.md` is the file you read and link to. `versions/` is the record: one directory per published version, never edited after it lands.

## How versions work

The model is the one W3C uses for its technical reports and that the OpenAPI and LSP specs use in git: a mutable editor's draft plus dated, immutable snapshots. IETF drafts work the same way (`draft-foo-00`, `-01`, ...), and each draft stays available forever.

Rules:

1. **Every change to the spec is a new version.** A pull request that edits `spec/pavia-protocol.md` must also add `spec/versions/<version>/pavia-protocol.md` with byte-identical content. There is no "small fix" exception; a typo fix is a new draft number.
2. **Snapshots are immutable.** Nothing under `spec/versions/` is modified, renamed or deleted once merged. CI rejects the PR otherwise (`scripts/spec-guard.sh`).
3. **The editor's draft always equals the newest snapshot.** Reviewers diff `spec/pavia-protocol.md` in the PR; the snapshot is a copy.
4. **Version names** come from the `**Version:**` line at the top of the document. While protocol version 1 is unfrozen, snapshots are `draft-0.N`. When the roadmap reaches M18 the text is frozen as `1.0`. Later compatible revisions are `1.1`, `1.2`, and so on.
5. **The changelog lives inside the document** (last section). Each new version adds an entry there, so a snapshot carries its own history.
6. **Test vectors move with the spec.** A change to normative text lands in the same PR as the matching changes under `vectors/` (roadmap, spec-first rule). Vectors are not snapshotted per version; the git tag is the link.
7. **Tags.** Each merged snapshot gets an annotated, signed tag `spec/<version>` on the merge commit (for example `spec/draft-0.3`). The tag is how an implementation states which spec text it was tested against.

Why not the GitHub wiki: wiki pages are outside pull requests and CI, cannot be protected, and cannot change together with `vectors/` in one commit. The spec-first rule needs all three.

## Reviewing a spec change

- Read the diff of `spec/pavia-protocol.md`.
- Check the new snapshot directory name matches the `**Version:**` line.
- Check the changelog entry lists every section touched.
- If the change is normative (a MUST, SHOULD, value, registry entry or wire layout), confirm `vectors/` changed too, or the PR says why not.

## Versions

| Version | Date | Status | Notes |
|---|---|---|---|
| [draft-0.2](versions/draft-0.2/pavia-protocol.md) | 2026-09-21 | Working draft | First version tracked in this repository. |
| [draft-0.3](versions/draft-0.3/pavia-protocol.md) | 2026-09-21 | Working draft | Consistency pass from the C0.1 spec review (#229 to #248). |
| [draft-0.4](versions/draft-0.4/pavia-protocol.md) | 2026-09-21 | Working draft | Integer keys in `limits` and `client` maps. |
| [draft-0.5](versions/draft-0.5/pavia-protocol.md) | 2026-09-21 | Working draft | Replay buffer eviction order and per-lane gap reporting (`lost`). |
| [draft-0.6](versions/draft-0.6/pavia-protocol.md) | 2026-09-21 | Working draft | Resume routing hint; multi-node ownership, takeover and backplane outage (13.8). |
