<!-- Copyright (c) 2026 Graphcore Ltd. All rights reserved. -->

# GWR fuzz targets

## Prerequisites

Install the pinned `cargo-fuzz` release:

```console
cargo install cargo-fuzz --version 0.13.2 --locked
```

## Running fuzz targets

List the available targets:

```console
cargo +nightly fuzz list
```

Run one target from the repository root:

```console
fuzz_target=engine_run_until
cargo +nightly fuzz run "$fuzz_target" -- \
  -max_total_time=300 \
  -max_len=32 \
  -timeout=5
```

Run every target sequentially:

```console
for fuzz_target in $(cargo +nightly fuzz list); do
  cargo +nightly fuzz run "$fuzz_target" -- \
    -max_total_time=300 \
    -max_len=32 \
    -timeout=5 || break
done
```

The time limit applies separately to each target. Run up to three targets in
parallel with `xargs`:

```console
cargo +nightly fuzz list | \
  xargs -P 3 -I {} cargo +nightly fuzz run {} -- \
    -max_total_time=300 \
    -max_len=32 \
    -timeout=5
```

Parallel target output will be interleaved.

## Failures and coverage

Decode, reproduce, and minimize a failure with:

```console
fuzz_target=engine_run_until
artifact_path=fuzz/artifacts/engine_run_until/crash-example
cargo +nightly fuzz fmt "$fuzz_target" "$artifact_path"
RUST_BACKTRACE=1 cargo +nightly fuzz run "$fuzz_target" "$artifact_path"
cargo +nightly fuzz tmin "$fuzz_target" "$artifact_path"
```

Install `llvm-tools-preview` and generate coverage after the target has built a
corpus:

```console
rustup component add llvm-tools-preview --toolchain nightly
fuzz_target=engine_run_until
cargo +nightly fuzz coverage "$fuzz_target"
```

Convert confirmed failures into readable regression tests in the crate under
test rather than committing generated corpus or artifact directories.
