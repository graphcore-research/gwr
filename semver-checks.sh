#!/bin/sh

# Copyright (c) 2026 Graphcore Ltd. All rights reserved.

set -eu

ROOT_DIR=$(pwd)
BASELINE_DIR=target/semver-checks/baseline-repo
REMOTE_URL=https://github.com/graphcore-research/gwr.git
REMOTE_NAME=origin
BRANCH=main
PACKAGE_LIST=target/semver-checks/packages.tsv
ORIGINAL_HOME=${HOME:-}
ORIGINAL_CARGO_HOME=${CARGO_HOME:-}

export CARGO_SEMVER_CHECKS=1
export RUSTC_BOOTSTRAP=1
export XDG_CACHE_HOME="$ROOT_DIR/target/semver-checks/cache"
export HOME="$ROOT_DIR/target/semver-checks/home"

if [ -z "$ORIGINAL_CARGO_HOME" ] && [ -n "$ORIGINAL_HOME" ]; then
    ORIGINAL_CARGO_HOME="$ORIGINAL_HOME/.cargo"
fi

if [ -n "$ORIGINAL_CARGO_HOME" ]; then
    export CARGO_HOME="$ORIGINAL_CARGO_HOME"
fi

if [ -z "${RUSTUP_HOME:-}" ] && [ -n "$ORIGINAL_HOME" ]; then
    export RUSTUP_HOME="$ORIGINAL_HOME/.rustup"
fi

mkdir -p "$BASELINE_DIR" "$XDG_CACHE_HOME" "$HOME"

if [ ! -d "$BASELINE_DIR/.git" ]; then
    git init "$BASELINE_DIR"
fi

git -C "$BASELINE_DIR" remote get-url "$REMOTE_NAME" >/dev/null 2>&1 \
    && git -C "$BASELINE_DIR" remote set-url "$REMOTE_NAME" "$REMOTE_URL" \
    || git -C "$BASELINE_DIR" remote add "$REMOTE_NAME" "$REMOTE_URL"

if ! git -C "$BASELINE_DIR" fetch --depth=1 "$REMOTE_NAME" "$BRANCH"; then
    echo "warning: failed to fetch $REMOTE_NAME/$BRANCH; using existing baseline checkout" >&2
fi
git -C "$BASELINE_DIR" checkout -B "$BRANCH" "$REMOTE_NAME/$BRANCH"

cargo metadata --no-deps --format-version 1 \
    | jq -r '
        .workspace_members as $workspace_members
        |
        .packages[]
        | select(.id as $id | $workspace_members | index($id))
        | select(.name != "gwr-tools")
        | . as $package
        | $package.targets[]
        | select(.kind | index("lib") or index("proc-macro"))
        | [$package.name, .name] | @tsv
    ' > "$PACKAGE_LIST"

while IFS="$(printf '\t')" read -r package crate; do
    current_json="$ROOT_DIR/target/doc/$crate.json"
    baseline_json="$ROOT_DIR/$BASELINE_DIR/target/doc/$crate.json"

    echo "Building current rustdoc JSON for $package"
    cargo rustdoc --locked -p "$package" --lib --all-features -- \
        -Z unstable-options --output-format json

    if ! git -C "$BASELINE_DIR" grep -q "^name = \"$package\"$" -- '*/Cargo.toml'; then
        echo "Skipping $package because it is not present in the baseline"
        continue
    fi

    echo "Building baseline rustdoc JSON for $package"
    (
        cd "$BASELINE_DIR"
        cargo rustdoc --locked -p "$package" --lib --all-features -- \
            -Z unstable-options --output-format json
    )

    echo "Checking $package for semver violations"
    cargo semver-checks \
        --current-rustdoc "$current_json" \
        --baseline-rustdoc "$baseline_json"
done < "$PACKAGE_LIST"
