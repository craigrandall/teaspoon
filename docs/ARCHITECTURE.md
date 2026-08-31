# Architecture

## Intent

`teaspoon` is a format-independent Outlook item mining tool.

The key boundary is:

```text
PST / MSG serialization
        |
        v
format adapter
        |
        v
normalized Outlook item
        |
        +--> Markdown
        +--> metadata
        +--> attachment archive
        +--> diagnostics
```

PST and MSG must not leak into the renderer or archive writer.

## Domain boundary

The eventual normalized model should represent:

- source/provenance
- folder location where applicable
- message class
- message properties
- recipients
- plain/HTML/RTF bodies
- attachments
- embedded messages
- named properties
- extraction diagnostics

The raw property bag is intentional: Outlook's property model is larger than the subset required by Markdown.

## M1 boundary

M1 intentionally stops at:

```text
PST
 |
 +-- open store
 |
 +-- IPM subtree
 |     |
 |     +-- folders
 |           |
 |           +-- hierarchy
 |           +-- contents
 |                 |
 |                 +-- message entry IDs
 |                 +-- message properties
 |
 +-- deterministic inventory
```

M1 does not create the final normalized domain model. That prevents a spike implementation from prematurely becoming the architecture.
