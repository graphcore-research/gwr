#!/usr/bin/env bash

# Copyright (c) 2025 Graphcore Ltd. All rights reserved.

set -e
echo "Installing build dependencies"

show_exit_message=false
if [[ $GITHUB_ACTIONS != "true" ]]; then
  if [[ $OSTYPE == "linux"* ]]; then
    sudo apt-get update
    sudo apt install    \
      build-essential   \
      curl              \
      git               \
      python3.12-venv
  elif [[ $OSTYPE == "darwin"* ]]; then
    if [ ! -f /opt/homebrew/bin/brew ]; then
      /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
      echo >> $HOME/.zprofile
      echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> $HOME/.zprofile
      eval "$(/opt/homebrew/bin/brew shellenv)"
      show_exit_message=true
    fi
  else
    echo "Installing build dependencies on $OSTYPE is unsupported"
    exit 1
  fi

  if ! command -v rustup >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    show_exit_message=true
  fi

  cargo_bin_dir="$HOME/.cargo/bin"
  case ":$PATH:" in
    *":$cargo_bin_dir:"*)
      echo "Cargo bin directory already found on PATH"
      ;;
    *)
      echo "Adding Cargo bin directory to PATH"
      export PATH="$cargo_bin_dir:$PATH"
      show_exit_message=true
      ;;
  esac
fi

if [[ $OSTYPE == "linux"* ]]; then
  sudo apt-get update
  sudo apt install   \
    asciidoctor      \
    capnproto        \
    protobuf-compiler
elif [[ $OSTYPE == "darwin"* ]]; then
  brew update
  brew install    \
    asciidoctor   \
    capnp         \
    protobuf
else
  echo "Installing build dependencies on $OSTYPE is unsupported"
  exit 1
fi

rustup show  # Cause toolchain specified in rust-toolchain.toml to be installed

export BINSTALL_VERSION=1.17.3
curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash

cargo binstall --disable-telemetry --no-confirm --locked   \
  cargo-expand@1.0.119                                     \
  mdbook@0.4.52                                            \
  mdbook-alerts@0.8.0                                      \
  mdbook-cmdrun@0.7.3                                      \
  mdbook-keeper@0.5.0                                      \
  mdbook-linkcheck@0.7.7                                   \
  typst-cli@0.14.0

if $show_exit_message; then
  echo "Please start a new shell to pickup changes to PATH"
fi
