#!/bin/sh

# Copyright (c) 2026 Graphcore Ltd. All rights reserved.

BASELINE_DIR=target/semver-checks/baseline-repo
REMOTE_URL=https://github.com/graphcore-research/gwr.git
REMOTE_NAME=origin
BRANCH=main

git init "$BASELINE_DIR"
git -C "$BASELINE_DIR" remote add "$REMOTE_NAME" "$REMOTE_URL"
git -C "$BASELINE_DIR" fetch --depth=1 "$REMOTE_NAME" "$BRANCH"
git -C "$BASELINE_DIR" checkout -B "$BRANCH" "$REMOTE_NAME/$BRANCH"

env CARGO_SEMVER_CHECKS=1 cargo semver-checks --baseline-root "$BASELINE_DIR" --exclude gwr-onnx-sys --exclude gwr-onnx
