# PST password protection: detection, verification, and refusal

- **Status:** proposed (decisions 1-9 are recommendations until the owner accepts them)
- **Date:** 2026-11-01
- **Deciders:** Craig Randall (owner)
- **Release:** v0.1.39 (this ADR records the decisions implemented there)

## Context and problem statement

Outlook lets a user protect a PST with a password. [MS-PST] describes that protection as a
UI-level convention, not encryption: the password is not a key to anything, and the data can be
read without it. What the file stores is a CRC-32 hash of the password text, in the property
PidTagPstPassword of the message store. When that property exists and is nonzero, the
specification says an implementation must enforce the check.

Up to v0.1.38 `tsp` ignored the property. A real PST of about 6 GB with a password was read
completely, with every error counter at zero, without any password being supplied. The owner's
requirement is that `tsp` supports password-protected PSTs. This ADR settles how a protected PST
is detected, how a supplied password is verified, how it is supplied, what happens when it is
missing or wrong, and what is reported.

## Facts established

From the specification ([MS-PST] 2.4.3.3 and 4.2):

- The hash is stored in PidTagPstPassword, in the property context of the message store.
- Absent or zero means unprotected.
- If the value is nonzero, an implementation should prompt for a password, hash it, and compare;
  it must enforce the check.
- The password gives no confidentiality: it is not used as a cipher key. CRC-32 is prone to
  collisions and weak against brute force.
- The pages do not state the CRC-32 variant, the text encoding, or the property tag number.

Measured (2026-10-31, on a real protected PST whose password the owner knows):

- **Tag.** Reading property 0x67FF from the store's properties through `outlook-pst` 1.2.0
  returns a 32-bit integer that equals the hash of the known password under the scheme below.
  The tag was first taken from third-party documentation and is confirmed by this measurement.
- **Scheme.** Reflected polynomial 0xEDB88320, initial value 0, no final XOR, over the password's
  ANSI bytes with no terminator. Sixteen candidates were tried (four text encodings times four
  seed and final-XOR combinations). Exactly one matched. The standard zlib CRC-32 (initial value
  FFFFFFFF, final XOR FFFFFFFF) did not. An independent PowerShell implementation gave the same
  value.
- The property survived a SCANPST repair (observed once).
- An empty password hashes to 0, which the format cannot tell apart from "no password".

## Considered options

| Option | Assessment |
|---|---|
| A. Ignore the property (v0.1.38 behavior) | Violates the specification's must-enforce rule and silently reads protected files. |
| B. Detect, warn, continue | Still reads without authorization, and a line in a content-free report is easy to miss. |
| C. Detect and refuse unless a verified password is supplied | **Chosen.** |
| D. As C, plus an explicit override flag to read without a password | Deferred: no use case yet, and under this format it would be a policy choice, not a technical one. |

How the password is supplied:

| Method | Assessment |
|---|---|
| Environment variable | **Chosen.** Works non-interactively and, unlike an argument, does not appear in process listings or shell history. It is visible to same-user processes and inherited by child processes. |
| Command-line option | Rejected: appears in process listings and shell history. |
| Interactive prompt | Deferred: breaks non-interactive use. |

## Decision outcome

Chosen option C, with these decisions:

1. **Detection.** Immediately after opening the store, before anything else is read, read
   PidTagPstPassword (0x67FF, 32-bit integer) from the store's properties. Absent or zero means
   unprotected. Any other value type is refused, because verification is impossible.
2. **Scheme.** Verify with a CRC-32 using reflected polynomial 0xEDB88320, initial value 0, and
   no final XOR, over the password's bytes. It is implemented in-house (about ten lines, no new
   dependency). Stock CRC-32 crates use the standard variant, which is the wrong one here, and
   must not be used.
3. **Policy.** If the property is nonzero and the password is missing or does not verify, refuse:
   the run ends with an error (exit status 1) and prints nothing on stdout. Both entry points
   (the diagnostic and the dry-run planner) enforce through one shared function.
4. **Supply.** The environment variable `TSP_PST_PASSWORD`. An empty value counts as unset, and a
   value that is not valid Unicode is refused. There is no command-line option.
5. **Secrets.** Error messages never contain the password or the stored hash, and the hash is
   never printed: a 32-bit hash of a password narrows guesses, and the reports are content-free.
   Tests use made-up passwords. Tests that need a real password run only when the variable is set.
6. **Text encoding.** Passwords are verified as ASCII only. A password containing a non-ASCII
   character is refused with a specific message, because the Windows code page Outlook used to
   encode it has not been verified. Revisit when `encoding_rs` code-page support lands (a
   Windows-1252 attempt, documented as unverified).
7. **Report.** A new stable key, `pst_password`, printed only when the check passes: `none`
   (unprotected) or `verified`, and `not_applicable` for `.msg` dry-run inputs. A refused run
   prints no report. The key adds one line to every PST output. `verify-split.ps1` ignores lines
   starting with `pst_password=` when comparing against its baseline tag.
8. **Unprotected PSTs.** A supplied password is ignored for an unprotected PST, and the run
   proceeds.
9. **Scope.** This is conformance with the format's rule, not security. The format has no
   encryption for the password to unlock, and any tool that skips the check can read a protected
   PST's data. `tsp` does not decrypt, recover, or bypass passwords.

## Consequences

Good:

- Conforms to the specification's must-enforce rule, so a protected PST is never read silently.
- No new dependency. The decision logic is a pure function that is unit-tested without a PST.
- The report vocabulary stays content-free, with one added key.

Accepted costs:

- A protected PST cannot be processed without setting an environment variable.
- Non-ASCII passwords are unsupported for now.
- A 32-bit hash can collide: a wrong password whose hash equals the stored one would verify, as
  with any reader that implements the specification's check.
- Every PST output gained one line, so golden outputs were re-baselined once.
- The PST header's separate block-encoding method is unrelated to this password and is still not
  reported by `tsp`.

## Verification

All figures are counts; no names, subjects, passwords, or hashes appear.

- Unit tests cover the CRC vectors and the decision logic. The quality gate passes (396 tests)
  and CI is green.
- A real protected PST (about 6.5 GB, 36 folders, 6,508 messages, 7,016 attachments):
  - no password: exit 1, empty stdout, in both the diagnostic and the dry-run;
  - wrong password: exit 1, empty stdout, in both; the error text does not contain the password;
  - correct password: exit 0, output identical to the earlier baseline plus
    `pst_password=verified`, in both.
- An unprotected PST (`tsp-tester.pst`): `pst_password=none`; output otherwise identical to a
  v0.1.38 build; a bogus password set in the environment is ignored.
- `verify-split.ps1`: default and `--verify` IDENTICAL; the PST check IDENTICAL once the new key's
  line is ignored (one line ignored).

## Not decided

- An explicit override flag to read a protected PST without its password (option D).
- An interactive password prompt, or a password file.
- Verification of non-ASCII passwords (decision 6).
- Reporting the PST header's block-encoding method.

## References

- [MS-PST]: 2.4.3.3 PST Password Security,
  <https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/48468b1e-cc81-4e2b-82a7-9bf61adc948e>
- [MS-PST]: 4.2 Strength of PST Password,
  <https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/a1b4b061-398d-4252-8593-1e42c0908ac6>
- `outlook-pst` 1.2.0, store properties lookup by property ID.
- The property tag value (0x67FF) is confirmed by measurement; the [MS-PST] pages above do not
  state it.
