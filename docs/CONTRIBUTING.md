# Contributing to teaspoon

`teaspoon` (binary `tsp`) mines Outlook `.pst` and `.msg` files into a Markdown archive. This file
defines the quality gate that CI runs and the rules every change follows.

## The gate

`rust-version` in `Cargo.toml` (1.88) is the declared minimum Rust version. CI builds with the
current stable toolchain only, so the minimum is declared, not tested; before relying on it, run
`cargo +1.88 check --locked`. Every change passes:

```text
cargo fmt --check
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

CI (`.github/workflows/ci.yml`) runs the same commands on Linux and Windows, with `--locked` so a
`Cargo.lock` that has drifted from `Cargo.toml` fails the build.

CI cannot run the fixture-dependent checks: real `.pst` and `.msg` fixtures are never committed
(`*.pst`, `*.ost`, `*.msg`, `*.eml`, and `_NOTES/` are ignored). A passing `cargo test` does not
show that a fixture-gated test ran, because such a test skips when its fixture variable is unset;
run it by name and confirm that it ran.

## Rules

- **Privacy.** Standard output and standard error carry counts and bounded numbers with stable
  keys, never names, subjects, addresses, paths, or message content. An error message never
  includes the input path. Archive content goes only into the directory named with `--out`. A
  credential the user supplies (a PST password, say), or anything derived from it, is never
  printed, logged, stored in the archive, or put in an error message.
- **Loss is explicit.** An item that cannot be read, recognized, or resolved increments its own
  counter. Nothing is skipped silently.
- **Never overwrite or delete what `tsp` did not generate.** Replacing its own files needs consent
  (`--overwrite` or a yes at the prompt).
- **Malformed input must not crash the tool.** No panic, and no recursion or allocation without a
  bound, on a hostile or corrupt file. Every such bound is a named constant, and what a bound cuts
  off is counted. Code that can panic on corrupt input (a dependency that indexes without
  checking, say) is called through `shared::run_contained`, and compressed RTF only through
  `shared::decompress_rtf_bounded`. Recursion on untrusted nesting is bounded before it starts,
  because a stack overflow cannot be caught.
- **Deterministic output.** Two exports of the same input by the same build are byte-identical.
  The root `folder.json` records the tool version, so a version change changes the archive's tree
  hash: measure the hash after the version bump, not before.
- **Dependencies** must be licensed under permissive terms compatible with `MIT OR Apache-2.0`
  (no GPL, LGPL, or AGPL). A licence with an `AND` term, such as `(Apache-2.0 OR MIT) AND
  BSD-3-Clause`, must be met in full, so check every term and not only the first option. A new
  dependency needs an ADR or a note in one, which records its version and licence, what it adds
  to `cargo tree`, and what it does on hostile input (read its source). A dependency whose output
  becomes archive content (the HTML converter, a text decoder) is pinned with `=`, because a new
  version can change the archive's bytes.
- **One definition per fact.** A name, a limit, or a number that the code or the archive format
  depends on is defined once and derived everywhere else (see `src/layout.rs`).

## Documentation

- Decisions are recorded in `ADRs/` (see `ADRs/_README.md`); verification results in
  `docs/verification/`; the plan in `docs/plans/`.
- A change that alters behavior, output keys, or the archive format updates the README and
  `docs/ARCHITECTURE.md` (and any ADR it amends) in the same commit.
- A verification result names what was run and what was not; a claim that was written but not run
  is marked as such.

## License

`teaspoon` is licensed under either of the MIT license (`LICENSE-MIT`) or the Apache License,
Version 2.0 (`LICENSE-APACHE`), at your option. Unless you state otherwise, a contribution you
submit is licensed under the same terms.
