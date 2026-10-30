# Contributing to teaspoon

`teaspoon` (binary `tsp`) mines Outlook `.pst` and `.msg` files into a Markdown archive. This file
defines the quality gate that CI runs and the rules every change follows.

## The gate

Rust 1.88 or later is needed (`rust-version` in `Cargo.toml`). Every change passes:

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
(`*.pst`, `*.msg`, `*.eml`, and `_NOTES/` are ignored). A passing `cargo test` does not show that a
fixture-gated test ran, because such a test skips when its fixture variable is unset; run it by
name and confirm that it ran.

## Rules

- **Privacy.** Standard output and standard error carry counts and bounded numbers with stable
  keys, never names, subjects, addresses, paths, or message content. An error message never
  includes the input path. Archive content goes only into the directory named with `--out`.
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
- **Deterministic output.** Two exports of the same input are byte-identical.
- **Dependencies** must be licensed compatibly with `MIT OR Apache-2.0` (no GPL). A new dependency
  needs an ADR or a note in one.
- **One definition per fact.** A name, a limit, or a number that the code or the archive format
  depends on is defined once and derived everywhere else (see `src/layout.rs`).

## Documentation

- Decisions are recorded in `ADRs/` (see `ADRs/_README.md`); verification results in
  `docs/verification/`; the plan in `docs/plans/`.
- A change that alters behavior, output keys, or the archive format updates the README and
  `docs/ARCHITECTURE.md` in the same commit.

## License

`teaspoon` is licensed under either of the MIT license (`LICENSE-MIT`) or the Apache License,
Version 2.0 (`LICENSE-APACHE`), at your option. Unless you state otherwise, a contribution you
submit is licensed under the same terms.
