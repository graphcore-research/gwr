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

npm install --no-save prettier

rustup update
rustup toolchain install --profile default nightly

cargo install --locked cargo-about cargo-deny cargo-semver-checks release-plz
cargo install --locked --bin cog cocogitto
