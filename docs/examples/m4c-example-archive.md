# Example archive (M4c draft format)

This is a small, hand-built illustration of what `tsp <dir> --out <out>` writes, following the accepted ADRs ([naming](../../ADRs/export-naming-collisions-and-path-budgets.md), [identity](../../ADRs/message-and-folder-identity-and-provenance.md), [output posture](../../ADRs/export-output-posture-consent-and-ownership.md), [file contract](../../ADRs/archive-file-contract-and-schema-evolution.md)). It is synthetic: no real message content appears.

- `Hello World/message.md` and `Hello World/metadata.json` are exactly the committed golden files in `tests/golden/hello/` (the byte-for-byte output of the code for a synthetic message).
- The two `folder.json` files below were assembled by hand from the draft schema in `src/archive.rs`. Numbers, the tool version, and the hash are **illustrative placeholders**, not output of a run.
- Everything is the `0.1-draft` format: it may change without notice until the metadata stage freezes it.

## Layout

The input was a directory named `mail` holding `Hello World.msg` and a subdirectory `Projects` with two messages that share the subject `Status`.

```text
<out>/
  mail/                          archive root, named from the input directory
    folder.json                  root: marks the archive as created by tsp; source provenance; counts
    Hello World/
      message.md
      metadata.json
    Projects/
      folder.json
      Status/
        message.md
        metadata.json
      Status (02)/               second message with the same name: position suffix, no "Copy"
        message.md
        metadata.json
```

Attachments, if the messages had any, would be counted and recorded as not extracted in M4c; no `attachments/` directory is written yet.

## `mail/Hello World/message.md`

````text
# Hello World

```text
Line one
Line two
```
````

(The heading is the subject; the plain-text body is inside a code fence so no Markdown in it is interpreted. A message with no plain-text body would be the heading alone.)

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
    "plain_text": "present",
    "markdown": "fenced_text_provisional",
    "html_native": false,
    "html_via_rtf": false,
    "rtf": false
  },
  "attachments_not_extracted": 0,
  "status": "partial",
  "status_reasons": [
    "envelope_not_extracted"
  ]
}
```

In an archive made from the directory above, `source_file` would be `Hello World.msg` (the file's path relative to the input directory); the golden file uses `m.msg` because it comes from a single-file test. `status` is `partial` because the sender, recipients, and headers are not extracted yet; if the message had attachments or an HTML-in-RTF body the reasons would also list `attachments_not_extracted` and `formatted_bodies_not_converted`.

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
    "tool_version": "0.1.23",
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
| Every name is a valid Windows component (no reserved characters, no trailing dot or space, no device name) | Yes: `Hello World`, `Projects`, `Status`, `Status (02)`. |
| Names in each directory are unique under `casefold(NFC(name))`, and `folder.json` is not used by a child | Yes. |
| The first of two same-named messages keeps the bare name and the second gets ` (02)`; no ` - Copy` | Yes. |
| Every path stays within the 259-unit budget (directories within 247) from the real output root | Depends on `<out>`; the root records `planned_longest_relative_path_units` so a reader can check. |
| Each folder's `children` lists exactly the folder and message directories it contains | Yes. |
| Root `counts` equal what is on disk (1 folder, 3 messages) | Yes. |
| Every JSON file has `schema_version`; no absolute path; no export-time value; LF line endings and a trailing newline | Yes (the tool version is the writer's version, not a time). |
| Every gap is recorded (`status`, `status_reasons`, `attachments_not_extracted`) | Yes. |
| Identity is in metadata, not in names | Partly: M4c records `source_file` and the Internet message ID; folder and PST message identifiers and the content hash are decided in the identity ADR but not yet written. |
