# M1 PST Feasibility Spike

## Status

**P2 privacy-safe diagnostics implemented; behavioral execution requires a real PST fixture.**

## What M1 establishes

The spike uses the Microsoft Rust PST implementation rather than reimplementing PST parsing.

The intended path is:

1. open PST;
2. obtain the IPM subtree entry ID;
3. open the IPM subtree;
4. traverse folder hierarchy;
5. traverse folder contents;
6. open each message;
7. inspect its raw property collection;
8. report deterministic inventory totals.

## Privacy-safe diagnostics

The M1 CLI deliberately avoids emitting source-content or mailbox-identifying values. It does not print the input path, PST display name, folder names/paths, message subjects, entry IDs, property values, addresses, bodies, attachment names, attachment contents, or detailed parser errors.

It reports structural totals only, including folder/message counts, message-open failures, folder-open failures, and property-value counts.

## What remains unproven

No claim is made that M1 has proven:
- all PST variants;
- complete property fidelity;
- body extraction;
- attachment extraction;
- embedded messages;
- named-property semantic normalization;
- corrupt-PST behavior;
- differential equivalence with another implementation.

Those require fixtures and explicit tests.
