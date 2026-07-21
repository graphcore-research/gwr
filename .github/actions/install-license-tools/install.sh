#!/usr/bin/env bash

# Copyright (c) 2026 Graphcore Ltd. All rights reserved.

# Assumes `install-build-dependencies/install.sh` has already run.

set -e
echo "Installing license tools"

cargo binstall --disable-telemetry --no-confirm --locked   \
  cargo-about@0.8.4
