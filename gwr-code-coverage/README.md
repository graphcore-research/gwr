<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# Code Coverage

Code coverage utilities for GWR.

## Generating Code Coverage

<!-- ANCHOR: gen_code_coverage -->

There is a recipe provided for generating code coverage for all the tests in the
test suite:

```bash
cargo run -p gwr-terminus -- run --recipe gwr-code-coverage/recipes/coverage.yaml
```

If needed, the recipe will tell you what tools to install and how. The recipe
also prints the location of the HTML coverage report it generates so that you
can open it in a browser of your choosing.

<!-- ANCHOR_END: gen_code_coverage -->

## diff-coverage

<!-- ANCHOR: diff_code_coverage -->

If you want to compare the coverage from two runs or worktrees then you can use
the `diff-coverage` tool. This is a utility for showing the differences between
coverage reports. This tool compares `llvm-cov` JSON output files.

Summary reports show just the overall summary and per-file summaries:

```bash
cargo run --bin diff-coverage -- [BEFORE_PATH]/summary.json [AFTER_PATH]/summary.json > diff.md
```

Full reports add details of how the line coverage has changed within each of the
source files with annotated code listings:

```bash
cargo run --bin diff-coverage -- [BEFORE_PATH]/details.json [AFTER_PATH]/details.json > full_diff.md
```

Within the full reports the line coverage changes are shown with a context of
unchanged lines. Use `-C`/`--context` to change the number of lines in the
context.

<!-- prettier-ignore-start -->

> [!Note]
> The `diff-coverage` tool returns non-zero if the coverage has degraded in
> any way. That is to say if any of the percentages have dropped.

<!-- prettier-ignore-end -->

<!-- ANCHOR_END: diff_code_coverage -->
