#!/usr/bin/env bash

# Copyright (c) 2025 Graphcore Ltd. All rights reserved.

set -e
echo "Installing build dependencies"

if [[ $OSTYPE == "linux"* ]]; then
  sudo apt-get update
  sudo apt install asciidoctor capnproto protobuf-compiler
elif [[ $OSTYPE == "darwin"* ]]; then
  brew update
  brew install asciidoctor capnp protobuf
else
  echo "Installing build dependencies on $OSTYPE is unsupported"
  exit 1
fi

rustup show  # Cause toolchain specified in rust-toolchain.toml to be installed

cargo install --locked    \
  cargo-expand@1.0.114    \
  mdbook@0.4.52           \
  mdbook-cmdrun@0.7.1     \
  mdbook-keeper@0.5.0     \
  mdbook-linkcheck@0.7.7
