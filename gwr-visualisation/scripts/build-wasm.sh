#!/usr/bin/env bash

# Copyright (c) 2026 Graphcore Ltd. All rights reserved.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$script_dir/../.." && pwd)"
generated_dir="$repo_dir/gwr-visualisation/assets/generated"
temporary_dir="$(mktemp -d)"
trap 'rm -rf "$temporary_dir"' EXIT

cd "$repo_dir"

expected_version="wasm-bindgen 0.2.126"
actual_version="$(wasm-bindgen --version)"
if [[ "$actual_version" != "$expected_version" ]]; then
  echo "Expected $expected_version, found $actual_version" >&2
  exit 1
fi

rustflags="${RUSTFLAGS:-}"
rustflags+=" --remap-path-prefix=$repo_dir=/workspace"
rust_commit="$(rustc --version --verbose | sed -n 's/^commit-hash: //p')"
if [[ -z "$rust_commit" ]]; then
  echo "Unable to determine the Rust commit" >&2
  exit 1
fi
rust_source="$(rustc --print sysroot)/lib/rustlib/src/rust"
rustflags+=" --remap-path-prefix=$rust_source=/rustc/$rust_commit"
rustflags+=" --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo"

RUSTFLAGS="$rustflags" cargo build \
  --release \
  --package gwr-visualisation \
  --target wasm32-unknown-unknown \
  --no-default-features \
  --features web \
  --lib

metadata_json="$(cargo metadata --format-version 1 --no-deps)"
target_dir="$(printf '%s' "$metadata_json" | jq -r .target_directory)"
if [[ -z "$target_dir" ]]; then
  echo "Unable to determine Cargo target directory" >&2
  exit 1
fi

wasm-bindgen \
  --target no-modules \
  --no-typescript \
  --omit-default-module-path \
  --remove-name-section \
  --remove-producers-section \
  --out-dir "$temporary_dir" \
  --out-name gwr_visualisation \
  "$target_dir/wasm32-unknown-unknown/release/gwr_visualisation.wasm"

if [[ "${1:-}" == "--check" ]]; then
  cmp "$temporary_dir/gwr_visualisation.js" "$generated_dir/gwr_visualisation.js"
  cmp "$temporary_dir/gwr_visualisation_bg.wasm" "$generated_dir/gwr_visualisation_bg.wasm"
  exit 0
fi

mkdir -p "$generated_dir"
cp "$temporary_dir/gwr_visualisation.js" "$generated_dir/gwr_visualisation.js"
cp "$temporary_dir/gwr_visualisation_bg.wasm" "$generated_dir/gwr_visualisation_bg.wasm"
