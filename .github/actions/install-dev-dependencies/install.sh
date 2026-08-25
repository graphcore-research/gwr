#!/usr/bin/env bash

# Copyright (c) 2025 Graphcore Ltd. All rights reserved.

# Assumes `install-build-dependencies/install.sh` has already run.
set -e
echo "Installing dev dependencies"

if [[ $GITHUB_ACTIONS != "true" ]]; then
  if [[ $OSTYPE == "linux"* ]]; then
    sudo apt-get update
    sudo apt install npm
  elif [[ $OSTYPE == "darwin"* ]]; then
    brew update
    brew install npm
  else
    echo "Installing dev dependencies on $OSTYPE is unsupported"
    exit 1
  fi
fi

npm install --no-save \
  @eslint/js@9.39.5   \
  eslint@9.39.5       \
  globals@17.11.0     \
  prettier@3.9.6

rustup toolchain install --profile minimal --component rustfmt nightly

cargo binstall --disable-telemetry --no-confirm --locked   \
  cargo-deny@0.20.2                                        \
  cargo-semver-checks@0.50.0                               \
  lychee@0.24.2                                            \
  prek@0.4.14                                              \
  release-plz@0.3.160                                      \
  taplo-cli@0.10.0
cargo binstall --disable-telemetry --no-confirm --locked --bin=cog cocogitto@7.0.0

# The license generation tools are also required for development, but installed
# via a separate script (to optimise certain CI workflows).
"$(dirname "$0")/../install-license-tools/install.sh"
