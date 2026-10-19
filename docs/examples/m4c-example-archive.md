# Example archive (M4c draft format, with the M4d envelope, the M4e-2b body, and the M4f-1 attachment records)

This is a small, hand-built illustration of what `tsp <dir> --out <out>` writes, following the accepted ADRs ([naming](../../ADRs/export-naming-collisions-and-path-budgets.md), [identity](../../ADRs/message-and-folder-identity-and-provenance.md), [output posture](../../ADRs/export-output-posture-consent-and-ownership.md), [file contract](../../ADRs/archive-file-contract-and-schema-evolution.md), [body selection and loss](../../ADRs/body-selection-original-bodies-and-formatting-loss.md)). It is synthetic: no real message content appears.

- `Hello World/message.md` and `Hello World/metadata.json` are exactly the committed golden files in `tests/golden/hello/` as of `v0.1.34` (the byte-for-byte output of the code for a synthetic message). `metadata.json` changed in M4e-2b: the `body` object now records the source, the converter, the preprocessing, the kept original files, and the loss, and `message.md` is unchanged for a plain-text message. In M4f-1 `metadata.json` gained an `attachments` array (empty here, because this message has no attachments).
- `Hello World/body.txt` is the plain-text body as the message stored it (`Line one`, CRLF, `Line two`, CRLF: 20 bytes). The kept body files are byte copies of source content and are exempt from the file contract's UTF-8-without-BOM and LF rules, which is why `body.txt` keeps its CRLF line endings while every generated file uses LF.
- The two `folder.json` files below were assembled by hand from the draft schema in `src/archive.rs`. Numbers, the tool version, and the hash are **illustrative placeholders**, not output of a run.
- Everything is the `0.1-draft` format: it may change without notice until the metadata stage freezes it.
- A message with a full envelope is shown in `tests/golden/envelope/message.md` (sender, recipients, date, importance, sensitivity); the example here has only a delivery time, so only a `Date` line appears.
- A message whose body is HTML (native, or recovered from an RTF) has the converted Markdown after the envelope list in `message.md`, and `body.html` (the HTML, as stored or as recovered) and, if the message has them, `body.rtf` and `body.txt` beside it; its `body` object has `source` `native_html` or `rtf_html`, the converter (`htmd`, `0.5.4`), the preprocessing steps that changed something, and the loss, if any. That case has no committed golden file because the Markdown depends on the converter; the export tests assert its structure.

## Layout

The input was a directory named `mail` holding `Hello World.msg` and a subdirectory `Projects` with two messages that share the subject `Status`.

```text
<out>/
  mail/                          archive root, named from the input directory
    folder.json                  root: marks the archive as created by tsp; source provenance; counts
    Hello World/
      body.txt                   the plain-text body, byte for byte as stored (only the forms that exist are kept)
      message.md
      metadata.json
    Projects/
      folder.json
      Status/
        body.txt
        message.md
        metadata.json
      Status (02)/               second message with the same name: position suffix, no "Copy"
        body.txt
        message.md
        metadata.json
```

A message that has them also gets `body.html` and `body.rtf` in its directory. A message with by-value attachments also gets an `attachments/` directory with one file per attachment (since v0.1.34; see the attachment example below); embedded messages, OLE objects, and by-reference attachments are recorded as not extracted.

## `mail/Hello World/message.md`

````text
# Hello World

- **Date:** 2022-06-18T04:26:40Z

```text
Line one
Line two
```
````

(The heading is the subject; the list under it holds the envelope fields the message has, here only the date, taken from the delivery time because there is no sent time; the plain-text body is inside a code fence so no Markdown in it is interpreted, because the message has no HTML to convert. A message with no envelope fields and no body would be the heading alone. For an HTML body the Markdown follows the list directly, as converted.)

## `mail/Hello World/metadata.json`

```json
{
  "schema_version": "0.1-draft",
  "kind": "message",
  "source_file": "m.msg",
  "subject": "Hello World",
  "internet_message_id": null,
  "time_filetime": 133000000000000000,
  "directory": {
    "original": "Hello World",
    "adjustments": []
  },
  "body": {
    "source": "plain_text",
    "converter": null,
    "preprocessing": [],
    "internet_codepage": null,
    "originals": [
      {
        "file": "body.txt",
        "bytes": 20,
        "sha256": "13187ebc90c47a525637071656826b946089b1806bc3c94555c6acb529ab0bf8",
        "note": null
      }
    ],
    "cid_references": {
      "total": 0,
      "distinct": 0
    },
    "loss": [],
    "kept_files_exempt_from_file_rules": true
  },
  "envelope": {
    "sender": null,
    "sent_representing": null,
    "recipients": [],
    "recipients_unlisted": 0,
    "submit_time_filetime": null,
    "delivery_time_filetime": 133000000000000000,
    "date_utc": "2022-06-18T04:26:40Z",
    "importance": null,
    "sensitivity": null,
    "conversation_topic": null,
    "conversation_index_hex": null,
    "transport_headers": null
  },
  "attachments_not_extracted": 0,
  "attachments": [],
  "status": "partial",
  "status_reasons": [
    "other_properties_not_preserved"
  ]
}
```

In an archive made from the directory above, `source_file` would be `Hello World.msg` (the file's path relative to the input directory); the golden file uses `m.msg` because it comes from a single-file test. `status` is `partial` because properties the model does not carry yet are not preserved (`other_properties_not_preserved`, always listed); if any of the message's attachments were not extracted the reasons would also list `attachments_not_extracted`, and `formatted_bodies_not_converted` is listed when an HTML or RTF form existed but the body used is plain text or none (a failed conversion, an RTF with no encapsulated HTML, or an RTF that could not be decompressed). A converted body does not carry it. (Before v0.1.25 the always-present reason was `envelope_not_extracted`.)

For an HTML message the `body` object differs like this (illustrative, not a committed file):

```json
  "body": {
    "source": "native_html",
    "converter": { "name": "htmd", "version": "0.5.4" },
    "preprocessing": ["style", "title"],
    "internet_codepage": 1252,
    "originals": [
      { "file": "body.html", "bytes": 98, "sha256": "<64 hex digits>", "note": "as_stored" },
      { "file": "body.txt", "bytes": 11, "sha256": "<64 hex digits>", "note": null }
    ],
    "cid_references": { "total": 0, "distinct": 0 },
    "loss": [],
    "kept_files_exempt_from_file_rules": true
  },
```

## With attachments (illustrative, not a committed file)

A message with two by-value attachments (one with a content ID) and one OLE object would have, beside its other files:

```text
Status/
  attachments/
    report.pdf
    logo.png
  message.md
  metadata.json
```

and, in `metadata.json` after the `envelope` object (shown compactly; the real file is pretty-printed with one field per line):

```json
  "attachments_not_extracted": 1,
  "attachments": [
    { "name": { "original": "report.pdf", "adjustments": [] }, "method": "by_value", "method_code": 1, "content_id": null, "status": "extracted", "reason": null, "file": "report.pdf", "bytes": 48213, "sha256": "<64 hex digits>" },
    { "name": { "original": "logo.png", "adjustments": [] }, "method": "by_value", "method_code": 1, "content_id": "logo@x", "status": "extracted", "reason": null, "file": "logo.png", "bytes": 2210, "sha256": "<64 hex digits>" },
    { "name": { "original": "chart.bin", "adjustments": [] }, "method": "ole", "method_code": 6, "content_id": null, "status": "not_extracted", "reason": "ole_object", "file": null, "bytes": null, "sha256": null }
  ],
```

The `status_reasons` of that message would also list `attachments_not_extracted`, because one attachment is not extracted. The records are in the order the attachments are stored in the `.msg`; `file`, `bytes`, and `sha256` describe what was written, and are null when nothing was.

## `mail/Projects/folder.json`

```json
{
  "schema_version": "0.1-draft",
  "kind": "folder",
  "directory": {
    "original": "Projects",
    "adjustments": []
  },
  "children": [
    {
      "kind": "message",
      "directory_name": "Status"
    },
    {
      "kind": "message",
      "directory_name": "Status (02)"
    }
  ]
}
```

## `mail/folder.json` (illustrative values)

```json
{
  "schema_version": "0.1-draft",
  "kind": "archive_root",
  "children": [
    {
      "kind": "message",
      "directory_name": "Hello World"
    },
    {
      "kind": "folder",
      "directory_name": "Projects"
    }
  ],
  "root": {
    "tool": "teaspoon",
    "tool_version": "0.1.31",
    "status": "complete",
    "source": {
      "kind": "msg_directory",
      "name": "mail",
      "size_bytes": 12345,
      "sha256": "<64 lowercase hex digits>",
      "sha256_scope": "listing_of_exported_msg_files"
    },
    "max_path_units_allowed": 259,
    "planned_longest_relative_path_units": 62,
    "counts": {
      "folders": 1,
      "messages": 3,
      "attachments_extracted": 0,
      "attachments_not_extracted": 0,
      "embedded_messages_not_extracted": 0
    }
  }
}
```

While an export is running (and if it is interrupted) the root `status` is `incomplete`; the `complete` version replaces it last.

## Checked by hand against the ADRs

| Invariant | Holds here? |
|---|---|
| Every name is a valid Windows component (no reserved characters, no trailing dot or space, no device name) | Yes: `Hello World`, `Projects`, `Status`, `Status (02)`, and the fixed `body.*` names. |
| Names in each directory are unique under `casefold(NFC(name))`, and `folder.json` is not used by a child | Yes. |
| The first of two same-named messages keeps the bare name and the second gets ` (02)`; no ` - Copy` | Yes. |
| Every path stays within the 259-unit budget (directories within 247) from the real output root | Depends on `<out>`; the root records `planned_longest_relative_path_units` so a reader can check. The `body.*` names are shorter than `metadata.json`, which already sets the reservation. |
| Each folder's `children` lists exactly the folder and message directories it contains | Yes. |
| Root `counts` equal what is on disk (1 folder, 3 messages) | Yes. |
| Every JSON file has `schema_version`; no absolute path; no export-time value; LF line endings and a trailing newline | Yes (the tool version is the writer's version, not a time; the envelope times are the source's). The kept `body.*` files are exempt, as `body.kept_files_exempt_from_file_rules` says. |
| Every gap is recorded (`status`, `status_reasons`, `attachments_not_extracted`, `attachments`, `body.loss`) | Yes. |
| Every kept body file's size and SHA-256 are in `body.originals` | Yes. |
| Every extracted attachment's size and SHA-256 are in its `attachments` record | Yes for an archive with attachments (this example has none). |
| Identity is in metadata, not in names | Partly: the export records `source_file` and the Internet message ID; folder and PST message identifiers and the content hash are decided in the identity ADR but not yet written. |
