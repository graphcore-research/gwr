<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# GWR fuzz targets

## Prerequisites

Install the repository's [development dependencies]. This installs the nightly
Rust toolchain and the pinned `cargo-fuzz` release.

[development dependencies]: ../README.md#developing-gwr-packages

## Running fuzz targets

List the available targets:

```bash
cargo +nightly fuzz list
```

Run the default target for five minutes from the repository root:

```bash
cargo run -p gwr-terminus -- run --recipe fuzz/recipes/fuzz.yaml
```

Override the recipe defaults with named arguments:

```bash
cargo run -p gwr-terminus -- run --recipe fuzz/recipes/fuzz.yaml \
  --TARGET=engine_run_until \
  --MAX_TOTAL_TIME=1800 \
  --MAX_LEN=32 \
  --TIMEOUT=5
```

Run every target sequentially:

```bash
for fuzz_target in $(cargo +nightly fuzz list); do
  cargo +nightly fuzz run "$fuzz_target" -- \
    -max_total_time=300 \
    -max_len=32 \
    -timeout=5 || break
done
```

The time limit applies separately to each target. Run up to three targets in
parallel with `xargs`:

```bash
cargo +nightly fuzz list | \
  xargs -P 3 -I {} cargo +nightly fuzz run {} -- \
    -max_total_time=300 \
    -max_len=32 \
    -timeout=5
```

Parallel target output will be interleaved.

## Failures and coverage

Decode, reproduce, and minimize a failure with:

```bash
fuzz_target=engine_run_until
artifact_path=fuzz/artifacts/engine_run_until/crash-example
cargo +nightly fuzz fmt "$fuzz_target" "$artifact_path"
RUST_BACKTRACE=1 cargo +nightly fuzz run "$fuzz_target" "$artifact_path"
cargo +nightly fuzz tmin "$fuzz_target" "$artifact_path"
```

Install `llvm-tools-preview` and generate coverage after the target has built a
corpus:

```bash
rustup component add llvm-tools-preview --toolchain nightly
fuzz_target=engine_run_until
cargo +nightly fuzz coverage "$fuzz_target"
```

Convert confirmed failures into readable regression tests in the crate under
test rather than committing generated corpus or artifact directories.
