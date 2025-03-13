#!/usr/bin/env bash

# Copyright (c) 2025 Graphcore Ltd. All rights reserved.

set -e
echo "Installing build dependencies"

if [[ $OSTYPE == "linux"* ]]; then
  sudo apt-get update
  sudo apt install asciidoctor capnproto
elif [[ $OSTYPE == "darwin"* ]]; then
  brew update
  brew install asciidoctor capnp
else
  echo "Installing build dependencies on $OSTYPE is unsupported"
  exit 1
fi

rustup update
rustup toolchain install --profile default stable

cargo install cargo-expand mdbook mdbook-cmdrun mdbook-keeper mdbook-linkcheck
