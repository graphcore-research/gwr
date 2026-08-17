#!/usr/bin/env bash

# Copyright (c) 2026 Graphcore Ltd. All rights reserved.

# Assumes `install-build-dependencies/install.sh` has already run.

set -e
echo "Installing nightly tools"

rustup toolchain install --profile minimal --component rustfmt nightly

cargo install cargo-fuzz --version 0.13.2 --locked
