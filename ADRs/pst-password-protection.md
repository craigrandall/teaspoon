---
status: "accepted"
date: 2026-11-02
decision-makers: Craig Randall (project owner)
consulted: none
informed: none
---

# Refuse a password-protected PST unless its password verifies

Status history: proposed 2026-11-01; accepted 2026-11-02, except the exit status in
item 3, which the owner ruled on (2026-11-02): exit status 2, amended into item 3 and built in
`v0.1.40`.

## Context and Problem Statement

Outlook can protect a PST with a password. MS-PST treats that password as advisory, not as
encryption. A CRC-32 of the password is stored in `PidTagPstPassword`, in the property context of
the message store; the value is zero or absent when there is no password; the password is not used
as a cipher key, and the data can be read without it. The specification nevertheless says that an
implementation must enforce the check when the value is nonzero (section 2.4.3.3, PST Password
Security; section 4.2, Strength of PST Password, says the protection is weak).

Up to `v0.1.38` `tsp` ignored the property. A real PST of about 6 GB with a known password was read
completely by the diagnostic and by `--dry-run`, with no password supplied: 36 folders, 6,473
messages, 7,016 attachments, every error counter 0 (2026-10-31). On 2026-10-30 the owner set as a
requirement that `tsp` support password-protected PST files. `outlook-pst` exposes the property
through `StoreProperties::get`. This ADR records how a protected PST is detected, how a supplied
password is verified, how it is supplied, what happens when it is missing or wrong, and what is
reported. It is expensive to reverse because it fixes a stable report key and a refusal behavior
that scripts will depend on, it constrains both PST entry points and the later PST export, and it
had real alternatives.

## Decision Drivers

- Conformance with the specification's rule that the check is enforced.
- Loss and access are explicit: a protected file is never read silently.
- The password, and anything derived from it, never appears in output.
- Deterministic, content-free output with a stable vocabulary.
- No new dependency, and testable without a protected PST in CI.
- Cheap to extend later (an override, a prompt, other encodings).

## Considered Options

1. Ignore the property (the `v0.1.38` behavior).
2. Detect it, warn, and continue.
3. Detect it and refuse unless a verified password is supplied, the password coming from an
   environment variable.
4. As option 3, plus an explicit option to read a protected PST without its password.
5. As option 3, with the password from a command-line option or an interactive prompt.

## Decision Outcome

Chosen option: 3, because it is the only option that meets the specification's must-enforce rule
without a way to leak the password (a command-line option appears in process listings and shell
history, and a prompt breaks non-interactive use), and because option 4 or a prompt can be added
later without changing anything decided here. Items 1 to 9 were accepted by the owner on 2026-11-02 and describe what `v0.1.40` builds;
`v0.1.39` differs only in the exit status of item 3 (1 there).

1. **Detection.** Right after the store is opened, before anything else is read, read
   `PidTagPstPassword` (0x67FF, a 32-bit integer) from the store's properties. Absent or zero means
   unprotected. Any other value type is an error (exit status 1), because verification is impossible.
2. **Scheme.** Verify with a CRC-32 using the reflected polynomial 0xEDB88320, initial value 0, and
   no final XOR, over the password's bytes. It is written in-house (about ten lines, no new
   dependency). Stock CRC-32 crates use the standard variant (initial value and final XOR
   0xFFFFFFFF), which is the wrong one here and must not be used.
3. **Policy.** If the property is nonzero and the password is missing or does not verify, refuse:
   the run ends with an error, exit status 2, and prints nothing on standard output. Exit status 2
   is the status `tsp` already gives a run that declined to proceed (an export that needs consent):
   it understood the input, the caller can fix the cause, and a script can tell it from a failure,
   which exits 1 (a PST that does not open, a store property of an unexpected type, a
   `TSP_PST_PASSWORD` that is not valid Unicode). Three outcomes are refusals: the password missing,
   wrong, or containing a non-ASCII character (item 6). Both entry points (the diagnostic and the
   dry-run planner) enforce through one shared function, which returns a typed error
   (`PasswordRefusal`) with the same message text as before; `main` maps it to the status. `v0.1.39`
   exited 1 for these refusals; the owner ruled for 2 on 2026-11-02 (the plan, v4.26, decisions still
   needed, item 10, had recommended it), and `v0.1.40` builds it.
4. **Supply.** The environment variable `TSP_PST_PASSWORD`. An empty value counts as unset, and a
   value that is not valid Unicode is an error (exit status 1). There is no command-line option.
5. **Secrets.** Error messages never contain the password or the stored hash, and the hash is never
   printed: a 32-bit hash of a password narrows guesses, and the reports are content-free. Tests use
   made-up passwords, and any test that needs a real password runs only when the variable is set.
6. **Text encoding.** Passwords are verified as ASCII only. A password containing a non-ASCII
   character is refused with its own message, because the Windows code page Outlook used to encode
   it has not been verified. Revisit when `codepage.rs` exists (a Windows-1252 attempt, documented
   as unverified).
7. **Report.** A new stable key, `pst_password`, printed only when the check passes: `none`
   (unprotected) or `verified`, and `not_applicable` for a `.msg` dry run. A refused run prints no
   report. The key adds one line to every PST output (the diagnostic prints it first, the dry run
   after `input_kind`). `verify-split.ps1` ignores lines starting with `pst_password=` when it
   compares against its baseline tag.
8. **Unprotected PSTs.** A supplied password is ignored for an unprotected PST, and the run proceeds.
9. **Scope.** This is conformance with the format's rule, not security. The format has no encryption
   for the password to unlock, and any tool that skips the check can read a protected PST's data.
   `tsp` does not decrypt, recover, or bypass passwords. The PST header's block-encoding method is
   a separate matter and is not reported.

### Consequences

* Good, because it meets the specification's must-enforce rule, so a protected PST is never read
  silently.
* Good, because it adds no dependency, and the decision logic is a pure function that is unit-tested
  without a PST.
* Good, because the report vocabulary stays content-free, with one added key, and the password
  never reaches a process list, a shell history, or an output.
* Good, because the PST export (M4i) will call the same function, so the two readers cannot
  disagree.
* Neutral, because every PST output gains one line, so golden outputs were re-baselined once and
  the comparison script ignores that line.
* Bad, because a protected PST cannot be processed until an environment variable is set.
* Bad, because non-ASCII passwords are unsupported for now.
* Bad, because a 32-bit hash can collide: a wrong password whose hash equals the stored one would
  verify, as with any reader that implements the specification's check.
* Bad, because the refusal is policy and not protection, and a reader might take it for security;
  item 9 and the README say so.
* Good, because exit status 2 separates a refusal the caller can fix from a failure, and matches
  the status a consent refusal already has.
* Neutral, because `v0.1.39` exited 1 for these refusals, so a script written against that tag
  sees a changed status from `v0.1.40` (none is known to exist).
* Bad, because status 2 is also what the argument parser (clap) uses for a usage error, so the
  status alone does not tell a refusal from a bad option; the message does.

### Confirmation

1. Done (2026-11-01): unit tests for the CRC vectors, the decision table, and the report keys; the
   quality gate passes with 396 tests; CI passed for the push.
2. Done (2026-11-01), on a real protected PST (the repaired copy, about 6.5 GB, 36 folders, 6,508
   messages, 7,016 attachments): no password and a wrong password are refused by both modes with
   exit status 1 (`v0.1.39`; `v0.1.40` exits 2, see item 7) and 0 bytes on standard output; the
   right password gives the earlier output plus
   `pst_password=verified`; the wrong-password outputs contain no trace of the password.
3. Done (2026-11-01), on the unprotected `tsp-tester.pst`: `pst_password=none`, otherwise identical
   to a build of `v0.1.38`; a wrong password in the environment changes nothing.
4. Done (2026-11-01): `verify-split.ps1` default and `--verify` IDENTICAL; the PST comparison
   IDENTICAL once the new line is ignored.
5. Done (2026-10-31): the scheme was found by comparing 16 candidates with the stored value of the
   real file (exactly one matched), and an independent PowerShell implementation agreed.
6. Open: a second protected PST or producer; a password with a non-ASCII character; one PST per
   block-encoding method; Outlook's item and attachment counts for the protected PST (Outlook's
   call to add a data file failed on copies of it, cause undetermined).
7. Done (2026-11-02, `v0.1.40`; details in `docs/verification/m4-results.md`): on the same
   protected PST, a missing, a wrong, and a non-ASCII password are each refused by both modes with
   exit status 2, 0 bytes on standard output, and the same message as before on standard error; the
   right password gives the earlier output sizes (1,149 and 1,298 bytes); the unprotected
   `tsp-tester.pst` is unchanged (1,110 bytes for the diagnostic, with or without a wrong password
   set); a path that does not exist exits 1.

## Pros and Cons of the Options

### 1. Ignore the property

* Good, because nothing changes.
* Bad, because it violates the specification's must-enforce rule and reads protected files
  silently, which is the owner's stated gap.

### 2. Detect, warn, and continue

* Good, because it reads every file and still tells the user.
* Bad, because it still reads without authorization, and a line in a content-free report is easy to
  miss.

### 3. Refuse unless a verified password is supplied, from an environment variable (chosen)

* Good, because it meets the rule, keeps the password out of process listings and shell history, and
  works non-interactively.
* Bad, because the variable is visible to same-user processes and inherited by child processes, and
  it must be set before every run on a protected file.

### 4. As 3, plus an explicit override

* Good, because it would let a user read a protected file they cannot unlock.
* Bad, because there is no use case yet, and under this format it is a policy choice and not a
  technical need. Deferred; it can be added without changing items 1 to 9.

### 5. A command-line option or an interactive prompt

* Good, because an option is easy to use and a prompt avoids a stored variable.
* Bad, because an option appears in process listings and shell history, and a prompt breaks
  scripts and CI. A prompt is deferred.

## More Information

Verified on 2026-11-01 from the MS-PST pages: the hash is stored in `PidTagPstPassword` in the
property context of the message store; zero or absent means no password; an implementation must
enforce the check when the value is nonzero; the password is not a cipher key and the protection
is weak. The pages do not state the CRC-32 variant, the text encoding, or the property tag number.
Verified by measurement on 2026-10-31: the tag (0x67FF, read through `outlook-pst` 1.2.0 as a
32-bit integer), the scheme of item 2 (one of 16 candidates matched; the standard CRC-32 did not),
and that the property survived a repair with Outlook's Inbox Repair tool.

Not verified: any other protected PST or producer; non-ASCII passwords; the block-encoding method
of the tested file (not reported); that Outlook's behavior with a copy of the file matches item 3 (a
password prompt was seen for the original, not confirmed for a copy); Outlook's item and attachment counts; that `outlook-pst` has no
password handling of its own beyond exposing the property.

Related: [Microsoft specifications as normative authority](microsoft-specifications-as-normative-authority.md)
(the must-enforce rule comes from the specification), [Code page decoding with `encoding_rs`](code-page-decoding-with-encoding-rs.md)
(the encoding of a non-ASCII password waits for `codepage.rs`), and
[MS-PST PST Password Security](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/48468b1e-cc81-4e2b-82a7-9bf61adc948e)
and [Strength of PST Password](https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/a1b4b061-398d-4252-8593-1e42c0908ac6).
