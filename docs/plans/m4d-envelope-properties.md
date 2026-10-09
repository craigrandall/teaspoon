# M4d envelope properties: identifiers, sources, and extraction rules

Status: **built (tag `v0.1.25.1`, 2026-10-06; follow-ups in `v0.1.26.2`, 2026-10-08) and run on Windows: 203 tests pass, and `--verify-envelope --recursive` over all 34 `.msg` files reports 0 mismatches against `msg_parser`** (results: [`../verification/m4-results.md`](../verification/m4-results.md)). The M4 plan requires each candidate property ID to be confirmed against Microsoft's specifications before it is relied on. This document says what was and was not confirmed, so that nothing is presented as checked that was not. The tiers below record confirmation against Microsoft's text. The corpus run adds a separate kind of evidence, listed in the next section. On 2026-10-07 every property identifier below was checked against Microsoft's pages (the MAPI canonical property pages, and MS-OXPROPS for 0x0C1A) and is now tier A; what remains open is the meaning of two value sets and one recipient-type rule, listed under Open.

## Confirmation tiers

- **A — seen in a Microsoft document during this work** (MS-OXPROPS, MS-PST, or the MAPI canonical property pages).
- **B — seen only in a third-party library's property list.** Plausible, not authoritative.
- **C — not looked up in this session;** from the author's knowledge of the MAPI headers, or already in use by earlier stages.

The `--verify-envelope` differential check (below) tests, against `msg_parser`, every property whose value `msg_parser` exposes. It cannot test the others; they rest on the tier.

## Evidence from the corpus run (2026-10-06)

The differential check ran over the 29 top-level `.msg` files (not the 5 messages in the subdirectories). Every compared field matched (0 mismatches), on 29 senders and 36 recipients. What that adds to the tiers:

| Property | ID | Tier | Corpus evidence |
|---|---|---|---|
| PidTagSenderName | 0x0C1A | A (2026-10-07) | sender name matched `msg_parser` on 29 of 29 |
| PidTagSenderEmailAddress | 0x0C1F | A | equalled `msg_parser`'s sender email on 27 of 29 (the other 2 equalled the SMTP property) |
| PidTagSenderSmtpAddress | 0x5D01 | A | equalled `msg_parser`'s sender email on 3 of 29 (the one "both" case may be two empty values) |
| PidTagDisplayName (recipient) | 0x3001 | A (2026-10-07) | recipient name matched on 36 of 36 |
| PidTagEmailAddress (recipient) | 0x3003 | A (2026-10-07) | equalled `msg_parser`'s email on 33 of 36 |
| PidTagSmtpAddress (recipient) | 0x39FE | A (2026-10-07) | equalled `msg_parser`'s email on 3 of 36, where `PidTagEmailAddress` did not |
| PidTagRecipientType | 0x0C15 | A (2026-10-07) | To, Cc, and Bcc list lengths matched on 29 of 29 each |
| PidTagSubject (existing) | 0x0037 | already in use | matched on 29 of 29 |

This is agreement with an independent implementation on one corpus, not confirmation against Microsoft's text; the tiers stand. It shows the identifiers read the properties they are meant to read for these fields.

Not covered by any oracle, present in the corpus only as counts (of 29 messages): sent-representing 29, submit time 29, delivery time 29, importance 29, sensitivity 11, conversation topic 29, conversation index 29, transport headers 24. Their values were not checked against anything independent. The sent-representing properties (0x0042, 0x0064, 0x0065, 0x5D02) and the recipient address type (0x3002) are untested by the differential check; 0x0064 and 0x0065 are now tier A.

## Message-level properties

| Property | ID | Type | Tier | Used for |
|---|---|---|---|---|
| PidTagSenderName | 0x0C1A | string | A (2026-10-07) | sender display name |
| PidTagSenderAddressType | 0x0C1E | string | A | sender address type (`SMTP`, `EX`, ...) |
| PidTagSenderEmailAddress | 0x0C1F | string | A | sender address (an X.500 DN when the type is `EX`) |
| PidTagSenderSmtpAddress | 0x5D01 | string | A | sender SMTP address |
| PidTagSentRepresentingName | 0x0042 | string | A | "on behalf of" display name |
| PidTagSentRepresentingAddressType | 0x0064 | string | A (2026-10-07) | "on behalf of" address type |
| PidTagSentRepresentingEmailAddress | 0x0065 | string | A (2026-10-07) | "on behalf of" address |
| PidTagSentRepresentingSmtpAddress | 0x5D02 | string | A | "on behalf of" SMTP address |
| PidTagClientSubmitTime | 0x0039 | time | A | sent time (already used since M3) |
| PidTagMessageDeliveryTime | 0x0E06 | time | A (2026-10-07) | delivery time (already used since M3) |
| PidTagImportance | 0x0017 | long | A | importance |
| PidTagSensitivity | 0x0036 | long | A | sensitivity |
| PidTagConversationTopic | 0x0070 | string | A | conversation topic |
| PidTagConversationIndex | 0x0071 | binary | A | conversation index (kept as hex) |
| PidTagTransportMessageHeaders | 0x007D | string | A | Internet headers, verbatim |

## Recipient-row properties (one storage per recipient)

| Property | ID | Type | Tier | Used for |
|---|---|---|---|---|
| PidTagRecipientType | 0x0C15 | long | A (2026-10-07) | 1 To, 2 Cc, 3 Bcc (already used since M2 and verified against `msg_parser` counts) |
| PidTagDisplayName | 0x3001 | string | A (2026-10-07) | recipient display name |
| PidTagAddressType | 0x3002 | string | A (2026-10-07) | recipient address type |
| PidTagEmailAddress | 0x3003 | string | A (2026-10-07) | recipient address |
| PidTagSmtpAddress | 0x39FE | string | A (2026-10-07) | recipient SMTP address |

## Rules

1. **Nothing is derived or repaired.** An address keeps its display name, address type, address, and SMTP address as separate optional fields. An `EX` address with no SMTP address stays that way (the `exchange_without_smtp_total` count says how often; it is 0 on the corpus).
2. **Recipients** are the storages whose recipient type is 1, 2, or 3, in storage order. (Microsoft's PidTagRecipientType page says the value is one required type plus one optional flag, MAPI_P1 for a resend and MAPI_SUBMITTED; as written, a recipient carrying a flag is counted in `recipients_unlisted`, not listed. See Open.) The originator row (type 0), an unknown type, and a row whose type cannot be read are counted in `recipients_unlisted`, not listed (0 on the corpus).
3. **Sender and sent-representing** are each `None` when none of their four properties is present.
4. **Times** are kept as the stored FILETIME ticks; `message.md` and `metadata.json` also show one UTC string, from the submit time, else the delivery time, and omit it for a negative value or a year beyond 9999.
5. **Importance and sensitivity** are kept as the stored integers. `message.md` shows a label only for the standard values (importance 0 low, 1 normal, 2 high; sensitivity 0 none, 1 personal, 2 private, 3 company confidential) and shows nothing for the defaults (normal, none). Any other value is shown as `unknown (N)`. The sensitivity values 0 to 3 (none, personal, private, company confidential) were seen on Microsoft pages on 2026-10-07 (the PidTagRecipientReassignmentProhibited page lists all four, and MS-OXODLGT says private is 0x00000002); the importance names (low, normal, high) were seen on Microsoft's PidTagImportance page, and an Outlook VBA example on Microsoft's site uses 2 for high. The numbers 0 for low and 1 for normal were not seen in Microsoft text; they come from the MAPI headers.
6. **String decoding** uses the message's string code page chain already built for `PT_STRING8` (message code page, then Internet code page, then Windows-1252), as for every other string.
7. **`message.md`** shows From, On behalf of, To, Cc, Bcc, Date, Importance, and Sensitivity, each only when present. Every value is in a Markdown code span so nothing in a name or address is interpreted as Markdown. An address reads `Name <email>`, the name alone, or the email alone; the email shown is the SMTP address, else the address property only when the type is `SMTP` or absent, never an `EX` distinguished name. Transport headers and conversation fields appear only in `metadata.json`.

## What `--verify-envelope` compares

Against `msg_parser` (0.3.x, `Outlook::subject`, `sender`, `to`, `cc`, `bcc`; each person has `name` and `email`): the subject (after removing the subject-prefix marker from both); the sender's name and email; and the To, Cc, and Bcc lists, first by length, then name and email position by position. `msg_parser` exposes one email string per person, and its source suggests it prefers the SMTP property and, for an `EX` address, may look the address up in the transport headers. So the custom email is compared against both of its properties and the report says which one matched (`..._match_both`, `..._match_email_address_only`, `..._match_smtp_only`, `..._mismatch`); a `mismatch` is a finding to triage, not automatically an error in either path. An absent property is compared as an empty string, so `both` also counts two empty values, and `email_address_only` does not say whether the SMTP property is absent or present and different.

**Not compared** (nothing in `msg_parser` to compare against, or not used here): sent-representing, times, importance, sensitivity, conversation fields, and transport headers. They are reported only as presence counts across the files. The scan is non-recursive, like `--verify`: messages in subdirectories are not compared.

## Evidence from the recursive run (2026-10-08, 34 messages, 41 recipients)

0 mismatches on every compared field, as before, now including the 5 messages in the subdirectories. The email tally, split in `v0.1.26`:

| | both | `PidTagEmailAddress` only | SMTP property only |
|---|---|---|---|
| sender (34) | 1 | 31, SMTP absent in all 31 | 2, `PidTagEmailAddress` present and different in both |
| recipients (41) | 0 | 38, SMTP absent in all 38 | 3, `PidTagEmailAddress` present and different in all 3 |

So in all 5 cases where the two properties are both present and differ, `msg_parser` returned the SMTP address, and otherwise the SMTP property was absent and it returned `PidTagEmailAddress`. That matches the rule `message.md` uses; on this corpus the two never disagree. The recursive run added no `EX`-without-SMTP, unlisted, or flagged-type recipient (`recipients_unlisted_total=0`, `exchange_without_smtp_total=0`).

## Open

- Not seen in Microsoft text: the numbers 0 (low) and 1 (normal) for importance. The numeric values of the recipient-type flags MAPI_P1 (0x10000000) and MAPI_SUBMITTED (0x80000000) are from a third-party library's documentation; the code ignores exactly those two bits when classifying (fixed in `v0.1.26.2`, with tests; no corpus recipient carries a flag).
- Which `msg_parser` property feeds `Person.email` is established for this corpus only (above); the rule is inferred from 5 cases where the two properties differ, not read from `msg_parser`'s source.
- The `EX`-without-SMTP path, unlisted recipients, and flagged recipient types have no real instance in the corpus; they are covered by unit tests only.
- Sent-representing, times, importance, sensitivity, conversation fields, and transport headers have no oracle: the corpus shows them present (34 of 34, except sensitivity 11 and transport headers 29) but their values were not checked against anything independent.
- The corpus is one producer.
