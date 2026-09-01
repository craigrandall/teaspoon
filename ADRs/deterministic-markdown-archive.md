---
status: "accepted"
date: 2026-08-29
decision-makers: Craig
consulted: Claude (Anthropic), ChatGPT
informed: n/a — single-developer project
---

# Deterministic Markdown archive

## Context and Problem Statement

Once `teaspoon` can extract a message's normalized representation, it needs
somewhere durable to put it. Markdown is human-readable and AI-friendly,
but Markdown alone can't represent everything a message carries (raw
properties, attachment bytes, extraction diagnostics). How should the
output be laid out on disk so that it is both durable/canonical and
Markdown-readable, and so that the same input always produces the same
archive?

## Decision Drivers

- Markdown is explicitly a *projection* of the normalized model, not the
  canonical data — the canonical data must survive even where Markdown
  can't represent it (see
  [format-independent-domain-model.md](format-independent-domain-model.md)).
- The archive needs to be reproducible: re-running `teaspoon` on the same
  input should not silently reshuffle or rename prior output.
- Attachments need a home that's addressable from `message.md` without
  embedding binary content inline in Markdown.

## Considered Options

* A deterministic, per-message folder containing `message.md`,
  `metadata.json`, and an `attachments/` subfolder.
* A single flat Markdown file per mailbox or folder, with attachments
  extracted alongside it under generated names.
* A database (e.g., SQLite) as the canonical store, with Markdown generated
  on demand as a view rather than stored as the archive itself.

## Decision Outcome

Chosen option: "A deterministic, per-message folder", because it keeps each
message's Markdown, metadata, and attachments co-located and addressable by
relative path, which a single flat file or a database-as-canonical-store
would not give as directly — the stated goal is a *file-based*, durable,
human-browsable archive, not a queryable store that happens to export
Markdown.

### Consequences

- Good, because each message's Markdown, metadata, and attachments are
  co-located and can be inspected, diffed, or moved as a unit.
- Good, because Markdown stays a projection: `metadata.json` can carry
  whatever the normalized model captured, even properties Markdown has no
  natural way to render.
- Bad, because message identity, filename sanitization, collision handling,
  and attachment relationship semantics must all be specified before
  production export — none of that is designed yet.
- Bad, because a per-message-folder layout produces many small files and
  directories for a large mailbox, which is less compact than a flat file
  or database, though this is judged acceptable given the durability and
  browsability goals.

## More Information

Message identity/provenance strategy (deterministic archive folder naming
when `PidTagInternetMessageId` is missing or duplicated) is explicitly
deferred and tracked as open research, not yet decided by this ADR. See
[loss-aware-normalized-representation.md](loss-aware-normalized-representation.md)
for how `metadata.json` is expected to carry extraction diagnostics.
