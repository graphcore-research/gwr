<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Repository Instructions

- Use `cargo +nightly fmt` when formatting Rust code. The repository
  `rustfmt.toml` uses nightly-only options, so plain `cargo fmt` will warn and
  ignore parts of the formatting configuration.

## Reviews

Review the diff between the current branch and `origin/main` as a strict PR
reviewer. Focus on ensuring that all of the Design and Maintenance guidance
below is respected.

## Design and Maintenance

- Prefer simple, maintainable designs over preserving unnecessary structure.
- Minimize code wherever practical: remove duplication, collapse thin wrappers,
  and refactor existing code when that makes behavior easier to understand.
- Do not preserve awkward APIs solely for compatibility. This repository uses
  semantic versioning, so API-breaking changes are acceptable when they improve
  clarity, correctness, or maintainability.
- Keep code easy to read first: clear names, direct control flow, and small
  focused helpers are preferred over clever abstractions.
- Update existing code confidently when the current design is no longer the
  simplest expression of the behavior.
- Back refactors and behavior changes with focused tests so the resulting code
  remains high quality, understandable, and easy to maintain.
- Order code for easy reading top to bottom. So keep code that depends on other
  code first.
- Minimise the CRAP score by following Clean Code Fundamentals guidelines and
  writing simple functions and well tested code.
- Public APIs should be well documented.
