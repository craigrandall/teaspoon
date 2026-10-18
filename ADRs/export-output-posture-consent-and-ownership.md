---
status: "accepted"
date: 2026-10-04
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Export output posture: consent, ownership, and counts-only output

*Status history: proposed 2026-10-04; accepted 2026-10-04 by the owner, including the decisions that went beyond the owner's original instruction (listed under Confirmation). The behavior described here is implemented (M4c, tag v0.1.23.1) and was exercised on real output. Amended 2026-10-17 (M4h-0, tag v0.1.33): staging now happens as each file is rendered, before the consent decision, and a target that is not ours is refused before anything is rendered; see decisions 3 and 6, the Consequences, and the Confirmation. The amendment is proposed and counts as accepted when the owner commits it.*

## Context and Problem Statement

Until M4c `tsp` only read input and printed counts. Exporting writes message content, which is private, into a directory the user names, possibly one that already holds an earlier export, the user's own files, or another source's archive. Standard output and error are often pasted into bug reports and chats. How should `tsp` behave so that it never destroys what it did not create, never leaks content into its own output, and can be re-run safely?

## Decision Drivers

- Never delete or overwrite what `tsp` did not create.
- Re-running an unchanged export must be harmless and cheap; re-running after a change must be possible with consent.
- Consent must work interactively and in scripts.
- Output that can be shared must stay content-free at every stage ([Loss-aware normalized representation](loss-aware-normalized-representation.md) and the existing privacy rules).
- A half-written run must be recognizable.
- Exit codes should distinguish "refused, nothing written" from "error".

## Considered Options

* **A. A marker file that proves `tsp` created the archive; counts-only preflight; consent only when a file `tsp` generated would change; refuse everything else (chosen).**
* B. Always overwrite what is in the way.
* C. Refuse if the target exists at all (no re-export).
* D. A lock or manifest file outside the archive tree.
* E. Delete and recreate the archive on every run.

## Decision Outcome

Chosen option: "A", because it is the only option that allows safe re-export without ever touching foreign data.

1. **Invocation.** `tsp <input> --out <dir>` exports to `<dir>/<input name>/`; `--dry-run` plans and reports without writing; `--overwrite` gives consent in advance; `--no-source-hash` omits the source hash. `.pst` export stays unavailable until the PST stage.
2. **Ownership is the root `folder.json`.** A target directory may be written into only if it is absent, empty, or its root `folder.json` was written by `tsp` for the same source (same kind and name). A non-empty directory without that marker, or an archive of a different source, is refused, even with `--overwrite`.
3. **Counts-only preflight.** Before any file is moved into the archive, `tsp` compares the planned files with the target (by size and then SHA-256) and reports counts: files to create, files already identical, files to replace, planned paths something else occupies, and entries it would leave alone. Nothing is written if a planned path is occupied by something it cannot replace.
4. **Consent is needed only to replace.** Creating files, and re-running an unchanged export, need no consent. Replacing a file `tsp` generated earlier needs `--overwrite` or an interactive yes (asked on standard error). Not interactive and no `--overwrite` means refused. `tsp` never deletes, and files it did not generate are left alone and counted.
5. **Plan gates come first.** A plan that breaks a gate (see the [naming ADR](export-naming-collisions-and-path-budgets.md)) is refused before any preflight.
6. **Staging and read-back.** (Amended 2026-10-17, M4h-0.) A target that is not ours (decision 2) is refused before anything is rendered. Otherwise each file is written under a short numeric name in `<out>/.tsp-tmp/` the moment it is rendered, and only a manifest (path, size, SHA-256) is kept in memory; the output path must leave room for that. `<out>` and `.tsp-tmp` are therefore created before the consent decision, and message content is written to staging before it. Staged files are not part of the archive and replace nothing. A refused or failed run deletes the staged files and the staging directory, and removes `<out>` if this run created it and it is empty. After consent the staged files are renamed into place (those identical to the target are deleted from staging), and every file is read back by hash and compared with the manifest. The root `folder.json` is written first as `incomplete` and replaced by the `complete` version last, so an interrupted run is recognizable. Per-file replacement is atomic; the archive as a whole is not.
7. **Output is counts only.** Standard output carries bounded `key=value` counts and one hash over the tree (`export_tree_sha256`); refusal reasons go to standard error as counts and fixed text. Neither carries names, subjects, bodies, or paths. Archive content goes only to the directory the user named.
8. **Exit codes.** 0 completed, 2 refused with nothing written, 1 an error.

### Consequences

* Good, because nothing outside `tsp`'s own previous output is ever modified, and a repeat of an unchanged export is a verified no-op (all files compared, none written).
* Good, because the run is scriptable (exit codes, counts, one tree hash to compare runs).
* Bad, because ownership rests on one file: someone who deletes the root `folder.json` leaves an archive `tsp` will refuse to touch, and someone who copies it into a foreign directory makes that directory look owned.
* Good (since M4h-0), because memory no longer grows with the archive: one message and a small manifest are held. Measured on a synthetic corpus of messages of about 45 MB each, the streamed writer peaked at 521 MB for 20 messages and 523 MB for 300, where the earlier in-memory writer needed 1,668 MB for 20.
* Bad, because staged content is written before consent (inside `<out>/.tsp-tmp`, removed on a refusal), a repeat export re-renders and stages every file before finding it identical, and the archive is still not all-or-nothing: a crash leaves an `incomplete` root with a partial tree (and numeric leftovers that the next run removes).
* Bad, because there is no way yet to remove stale generated entries (a `--clean` with its own consent is deferred).

### Confirmation

* Synthetic tests cover: empty and absent targets, a repeat export writing nothing, changed files refused / declined / accepted / replaced with `--overwrite`, unrelated files left alone, a foreign directory refused even with `--overwrite`, an archive of another source refused, an over-budget plan writing nothing, an over-long output path being an error, leftover staging files being cleaned up, and the flag combinations the command line accepts.
* On the real `.msg` corpus (2026-10-03): first export 72 files written; repeat export 72 identical, 0 written; the same tree hash into a second directory; after a file was edited, the interactive prompt, the non-interactive refusal (exit code 2, nothing written), and `--overwrite` each behaved as described and restored the file with an unchanged tree hash.
* Decisions here that go beyond the owner's original instruction ("warn and ask permission, or exit"), **confirmed by the owner on acceptance (2026-10-04)**: refusing an archive of a *different source* even with `--overwrite`; defining source identity as kind plus name (not hash); refusing instead of adopting a non-empty directory without a marker; per-file rather than whole-archive atomicity for now.
* M4h-0 (streamed export, tag v0.1.33, 2026-10-17). Synthetic tests: the stager (staging, manifest order, the tree hash formula, commit, discard, read-back, leftover cleanup, chunked hashing); the manifest compared with the target by size and hash; the tree hash of the staged manifest equals the formula applied to the files on disk; the refusal tests also check that `.tsp-tmp` is gone. Real `.msg` corpus: 171 files written, 0 read-back mismatches; `export_tree_sha256=1daa76fc2bb7ff8e4077e8b54888e44362ef137c815d241ca11e81faf277d53f` for the streamed export, a second export into another directory, and builds of exactly `v0.1.31` and `v0.1.32` (the in-memory writers), all with `Cargo.toml` at 0.1.32 (the tagged `v0.1.33` has the version bumped and prints `4ab06cc0de4dc2ec55a583d1ae5bc04926b0941ad0b18ac5b9c664ab860ede56`, since the root `folder.json` records it); a repeat export found 171 identical, wrote 0, and left nothing in staging. By hand on real output, after editing one `message.md` and adding two files of the user's own: non-interactive, no `--overwrite`: refused, exit code 2, nothing written, staging absent; interactive no: the same; interactive yes and `--overwrite`: each replaced the edited file and the root marker (2 files written) and restored the original; the added files survived every case; a directory `tsp` did not create was refused even with `--overwrite`, before anything was staged (exit code 2, staging absent, its file untouched). Memory on the synthetic corpus: see the Consequences.
* Not yet confirmed (added by M4h-0): tens of thousands of files in the one flat staging directory (1,201 were tried); the manifest at millions of files; why the time per message rose from about 1.9 s (20 and 50 messages) to about 3.0 s (300 messages).
* Not yet confirmed: behavior on a read-only or network target; a crash mid-write (no fault-injection test yet); a sync client touching the output while it is being written.

## Pros and Cons of the Options

### A. Marker file, preflight, consent only to replace

* Good, because safe by construction and cheap to re-run.
* Bad, because it depends on one marker file and cannot clean up.

### B. Always overwrite

* Good, because simple.
* Bad, because it can destroy user files and another source's archive; unacceptable for a tool run on private archives.

### C. Refuse if the target exists

* Good, because it cannot destroy anything.
* Bad, because the first mistake in a long export forces manual cleanup, and re-export after a source change is impossible.

### D. Lock or manifest outside the tree

* Good, because it leaves the archive untouched.
* Bad, because it separates ownership from the data it describes; moving or copying the archive loses it.

### E. Delete and recreate

* Good, because the result is always exactly the plan.
* Bad, because it requires deleting, which this tool refuses to do to anything it cannot prove it created.

## More Information

* Lifecycle details and the original draft: [`docs/plans/m4a-export-rules.md`](../docs/plans/m4a-export-rules.md) sections 2 and 8.
* Evidence: [`docs/verification/m4-results.md`](../docs/verification/m4-results.md), M4c section.
* Revisit when: whole-archive atomicity is added (M4h; streaming was done in M4h-0), a `--clean` is proposed, or the marker file proves too fragile.
