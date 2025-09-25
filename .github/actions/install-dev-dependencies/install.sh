#!/usr/bin/env bash

# Copyright (c) 2025 Graphcore Ltd. All rights reserved.

set -e
echo "Installing dev dependencies"

if [[ $OSTYPE == "linux"* ]]; then
  sudo apt-get update
  sudo apt install pre-commit
elif [[ $OSTYPE == "darwin"* ]]; then
  brew update
  brew install pre-commit
else
  echo "Installing dev dependencies on $OSTYPE is unsupported"
  exit 1
fi

npm install --no-save prettier@3.6.2

rustup toolchain install --profile minimal --component rustfmt nightly

cargo binstall --disable-telemetry --no-confirm --locked   \
  cargo-about@0.8.2                                        \
  cargo-deny@0.18.5                                        \
  cargo-semver-checks@0.44.0                               \
  release-plz@0.3.147
cargo binstall --disable-telemetry --no-confirm --locked --bin=cog cocogitto@6.3.0
