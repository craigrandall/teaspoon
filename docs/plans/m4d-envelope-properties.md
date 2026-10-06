# M4d envelope properties: identifiers, sources, and extraction rules

Status: **drafted 2026-10-05 with the M4d code; not yet verified on Windows.** The M4 plan requires each candidate property ID to be confirmed against Microsoft's specifications before it is relied on. This document says what was and was not confirmed, so that nothing is presented as checked that was not.

## Confirmation tiers

- **A — seen in a Microsoft document during this work** (MS-OXPROPS, MS-PST, or the MAPI canonical property pages).
- **B — seen only in a third-party library's property list.** Plausible, not authoritative.
- **C — not looked up in this session;** from the author's knowledge of the MAPI headers, or already in use by earlier stages.

The `--verify-envelope` differential check (below) tests, against `msg_parser`, every property whose value `msg_parser` exposes. It cannot test the others; they rest on the tier.

## Message-level properties

| Property | ID | Type | Tier | Used for |
|---|---|---|---|---|
| PidTagSenderName | 0x0C1A | string | B | sender display name |
| PidTagSenderAddressType | 0x0C1E | string | A | sender address type (`SMTP`, `EX`, ...) |
| PidTagSenderEmailAddress | 0x0C1F | string | A | sender address (an X.500 DN when the type is `EX`) |
| PidTagSenderSmtpAddress | 0x5D01 | string | A | sender SMTP address |
| PidTagSentRepresentingName | 0x0042 | string | A | "on behalf of" display name |
| PidTagSentRepresentingAddressType | 0x0064 | string | C | "on behalf of" address type |
| PidTagSentRepresentingEmailAddress | 0x0065 | string | C | "on behalf of" address |
| PidTagSentRepresentingSmtpAddress | 0x5D02 | string | A | "on behalf of" SMTP address |
| PidTagClientSubmitTime | 0x0039 | time | A | sent time (already used since M3) |
| PidTagMessageDeliveryTime | 0x0E06 | time | C | delivery time (already used since M3) |
| PidTagImportance | 0x0017 | long | A | importance |
| PidTagSensitivity | 0x0036 | long | A | sensitivity |
| PidTagConversationTopic | 0x0070 | string | A | conversation topic |
| PidTagConversationIndex | 0x0071 | binary | A | conversation index (kept as hex) |
| PidTagTransportMessageHeaders | 0x007D | string | A | Internet headers, verbatim |

## Recipient-row properties (one storage per recipient)

| Property | ID | Type | Tier | Used for |
|---|---|---|---|---|
| PidTagRecipientType | 0x0C15 | long | C | 1 To, 2 Cc, 3 Bcc (already used since M2 and verified against `msg_parser` counts) |
| PidTagDisplayName | 0x3001 | string | C | recipient display name |
| PidTagAddressType | 0x3002 | string | C | recipient address type |
| PidTagEmailAddress | 0x3003 | string | C | recipient address |
| PidTagSmtpAddress | 0x39FE | string | B | recipient SMTP address |

## Rules

1. **Nothing is derived or repaired.** An address keeps its display name, address type, address, and SMTP address as separate optional fields. An `EX` address with no SMTP address stays that way (the `exchange_without_smtp_total` count says how often).
2. **Recipients** are the storages whose recipient type is 1, 2, or 3, in storage order. The originator row (type 0), an unknown type, and a row whose type cannot be read are counted in `recipients_unlisted`, not listed.
3. **Sender and sent-representing** are each `None` when none of their four properties is present.
4. **Times** are kept as the stored FILETIME ticks; `message.md` and `metadata.json` also show one UTC string, from the submit time, else the delivery time, and omit it for a negative value or a year beyond 9999.
5. **Importance and sensitivity** are kept as the stored integers. `message.md` shows a label only for the standard values (importance 0 low, 1 normal, 2 high; sensitivity 0 none, 1 personal, 2 private, 3 company confidential) and shows nothing for the defaults (normal, none). Any other value is shown as `unknown (N)`. The standard value meanings come from the MAPI headers, not from a page fetched in this session (tier C for the meanings).
6. **String decoding** uses the message's string code page chain already built for `PT_STRING8` (message code page, then Internet code page, then Windows-1252), as for every other string.
7. **`message.md`** shows From, On behalf of, To, Cc, Bcc, Date, Importance, and Sensitivity, each only when present. Every value is in a Markdown code span so nothing in a name or address is interpreted as Markdown. An address reads `Name <email>`, the name alone, or the email alone; the email shown is the SMTP address, else the address property only when the type is `SMTP` or absent, never an `EX` distinguished name. Transport headers and conversation fields appear only in `metadata.json`.

## What `--verify-envelope` compares

Against `msg_parser` (0.3.x, `Outlook::subject`, `sender`, `to`, `cc`, `bcc`; each person has `name` and `email`): the subject (after removing the subject-prefix marker from both); the sender's name and email; and the To, Cc, and Bcc lists, first by length, then name and email position by position. `msg_parser` exposes one email string per person, and its source suggests it prefers the SMTP property and, for an `EX` address, may look the address up in the transport headers. So the custom email is compared against both of its properties and the report says which one matched (`..._match_both`, `..._match_email_address_only`, `..._match_smtp_only`, `..._mismatch`); a `mismatch` is a finding to triage, not automatically an error in either path.

**Not compared** (nothing in `msg_parser` to compare against, or not used here): sent-representing, times, importance, sensitivity, conversation fields, and transport headers. They are reported only as presence counts across the files.

## Open

- Tier B and C identifiers have not been confirmed against Microsoft's text in this session. The differential check covers 0x0C1A, 0x0C1F, 0x5D01, 0x3001, 0x3003, and 0x39FE through `msg_parser`; it does not cover 0x0064, 0x0065, 0x3002, importance and sensitivity values, or the conversation fields.
- Which `msg_parser` property feeds `Person.email` has not been established by test; the report's breakdown is meant to show it.
