---
status: "proposed"
date: 2026-10-04
decision-makers: Craig
consulted: Claude (Anthropic)
informed: n/a — single-developer project
---

# Export output posture: consent, ownership, and counts-only output

*Status history: proposed 2026-10-04. Becomes accepted when the owner says so. The behavior described here is implemented (M4c, tag v0.1.23.1) and was exercised on real output; the points the owner has not yet confirmed are listed under Confirmation.*

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
3. **Counts-only preflight.** Before writing, `tsp` compares the plan with the target and reports counts: files to create, files already identical, files to replace, planned paths something else occupies, and entries it would leave alone. Nothing is written if a planned path is occupied by something it cannot replace.
4. **Consent is needed only to replace.** Creating files, and re-running an unchanged export, need no consent. Replacing a file `tsp` generated earlier needs `--overwrite` or an interactive yes (asked on standard error). Not interactive and no `--overwrite` means refused. `tsp` never deletes, and files it did not generate are left alone and counted.
5. **Plan gates come first.** A plan that breaks a gate (see the [naming ADR](export-naming-collisions-and-path-budgets.md)) is refused before any preflight.
6. **Staging and read-back.** Each file is written under a short numeric name in `<out>/.tsp-tmp/` and renamed into place; the output path must leave room for that. After writing, every file is read back and compared with what was planned. The root `folder.json` is written first as `incomplete` and replaced by the `complete` version last, so an interrupted run is recognizable. Per-file replacement is atomic; the archive as a whole is not.
7. **Output is counts only.** Standard output carries bounded `key=value` counts and one hash over the tree (`export_tree_sha256`); refusal reasons go to standard error as counts and fixed text. Neither carries names, subjects, bodies, or paths. Archive content goes only to the directory the user named.
8. **Exit codes.** 0 completed, 2 refused with nothing written, 1 an error.

### Consequences

* Good, because nothing outside `tsp`'s own previous output is ever modified, and a repeat of an unchanged export is a verified no-op (all files compared, none written).
* Good, because the run is scriptable (exit codes, counts, one tree hash to compare runs).
* Bad, because ownership rests on one file: someone who deletes the root `folder.json` leaves an archive `tsp` will refuse to touch, and someone who copies it into a foreign directory makes that directory look owned.
* Bad, because the archive is rendered in memory before writing and is not all-or-nothing; a crash leaves an `incomplete` root with a partial tree. Both are addressed in the writer-hardening stage (M4h).
* Bad, because there is no way yet to remove stale generated entries (a `--clean` with its own consent is deferred).

### Confirmation

* Synthetic tests cover: empty and absent targets, a repeat export writing nothing, changed files refused / declined / accepted / replaced with `--overwrite`, unrelated files left alone, a foreign directory refused even with `--overwrite`, an archive of another source refused, an over-budget plan writing nothing, an over-long output path being an error, leftover staging files being cleaned up, and the flag combinations the command line accepts.
* On the real `.msg` corpus (2026-10-03): first export 72 files written; repeat export 72 identical, 0 written; the same tree hash into a second directory; after a file was edited, the interactive prompt, the non-interactive refusal (exit code 2, nothing written), and `--overwrite` each behaved as described and restored the file with an unchanged tree hash.
* Decisions here that go beyond the owner's original instruction ("warn and ask permission, or exit") and need explicit confirmation: refusing an archive of a *different source* even with `--overwrite`; defining source identity as kind plus name (not hash); refusing instead of adopting a non-empty directory without a marker; per-file rather than whole-archive atomicity for now.
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
* Revisit when: whole-archive atomicity or streaming is added (M4h), a `--clean` is proposed, or the marker file proves too fragile.
