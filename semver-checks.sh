#!/bin/sh

# Copyright (c) 2026 Graphcore Ltd. All rights reserved.

set -eu

# Run from the script's directory (the workspace root) so the script works
# regardless of the caller's cwd.
cd -- "$(dirname -- "$0")"

REMOTE_URL=https://github.com/graphcore-research/gwr.git
REMOTE_NAME=origin
BRANCH=main

# Ensure gwr-onnx-sys/build.rs and gwr-perfetto-sys/build.rs populate their git
# submodules if missing, which is needed when building the baseline checkout
# (as it's created using `git init` rather than `git clone` so cannot pass
# `--recurse-submodules`).
export CARGO_SEMVER_CHECKS=1

# Required to use `-Z unstable-options --output-format json` with the stable
# toolchain set in rust-toolchain.toml.
export RUSTC_BOOTSTRAP=1

# Kill any background children (e.g. the parallel `cargo doc` invocations)
# if the script is interrupted, e.g. with Ctrl-C. POSIX sh sets SIGINT to ignore
# backgrounded children, so without this trap they would keep running as
# orphaned.
CHILD_PIDS=
trap '
    trap - INT TERM
    if [ -n "$CHILD_PIDS" ]; then
        kill $CHILD_PIDS 2>/dev/null
    fi
    exit 130
' INT TERM

# Resolve TARGET_DIR in two steps so a failure in `cargo metadata` aborts the
# script instead of silently leaving TARGET_DIR empty.
METADATA_JSON=$(cargo metadata --format-version 1 --no-deps)
TARGET_DIR=$(printf '%s' "$METADATA_JSON" | jq -r .target_directory)
unset METADATA_JSON

BASELINE_DIR="$TARGET_DIR/semver-checks/baseline-repo"
# Use a dedicated target dir for the current-workspace rustdoc build (which use
# `RUSTDOCFLAGS=-Z unstable-options --output-format json` and `RUSTC_BOOTSTRAP=1`)
# to avoid sharing fingerprints with regular `cargo doc` runs.
# Otherwise the two invocations will invalidate each other's rustdoc
# artifacts (as cargo's fingerprint includes `RUSTDOCFLAGS`), making both
# rebuild from scratch every time they are run alternately.
CURRENT_TARGET_DIR="$TARGET_DIR/semver-checks/current-target"
PACKAGE_LIST="$TARGET_DIR/semver-checks/packages.tsv"
mkdir -p "$TARGET_DIR/semver-checks"

# Set up the baseline checkout at $REMOTE_NAME/$BRANCH. Repeated runs of this
# script reuse the clone (which precludes the use of `git clone` directly).
setup_baseline_checkout() {
    mkdir -p "$BASELINE_DIR"

    if [ ! -d "$BASELINE_DIR/.git" ]; then
        git init "$BASELINE_DIR"
    fi

    if git -C "$BASELINE_DIR" remote get-url "$REMOTE_NAME" >/dev/null 2>&1; then
        git -C "$BASELINE_DIR" remote set-url "$REMOTE_NAME" "$REMOTE_URL"
    else
        git -C "$BASELINE_DIR" remote add "$REMOTE_NAME" "$REMOTE_URL"
    fi

    if ! git -C "$BASELINE_DIR" fetch --depth=1 "$REMOTE_NAME" "$BRANCH"; then
        # Fall back to a previously-fetched ref if there is one, otherwise
        # abort with a clearer message than the `git checkout` would give.
        if git -C "$BASELINE_DIR" rev-parse --verify --quiet \
            "$REMOTE_NAME/$BRANCH" >/dev/null; then
            echo "warning: failed to fetch $REMOTE_NAME/$BRANCH; using existing baseline checkout" >&2
        else
            echo "error: failed to fetch $REMOTE_NAME/$BRANCH and no cached ref is available in $BASELINE_DIR" >&2
            return 1
        fi
    fi
    # `-f` discards any local modifications left over from a previously
    # interrupted run, so a clean re-run isn't blocked by git reporting
    # `error: Your local changes would be overwritten`.
    # The baseline checkout is fully owned by this script, so it's safe to
    # throw away anything in it.
    git -C "$BASELINE_DIR" checkout -f -B "$BRANCH" "$REMOTE_NAME/$BRANCH"
}

# Emit TSV rows of "<package-name>\t<lib-target-name>" for each workspace
# member that exposes a library or proc-macro target. Captures the JSON in a
# variable first so a `cargo metadata` failure aborts the script via `set -e`
# instead of being masked by jq.
list_lib_crates() {
    metadata_json=$(cargo metadata --no-deps --format-version 1 \
        --manifest-path "$1/Cargo.toml")
    printf '%s' "$metadata_json" | jq -r '
        .workspace_members as $workspace_members
        | .packages[]
        | select(.id as $id | $workspace_members | index($id))
        | . as $package
        | $package.targets[]
        | select(.kind | index("lib") or index("proc-macro"))
        | [$package.name, .name] | @tsv
    '
    unset metadata_json
}

list_lib_crates . > "$PACKAGE_LIST"

# Build rustdoc JSON for every workspace lib in a single invocation.
# This is much faster than per-crate `cargo rustdoc` calls because cargo can
# share dependency compilation across the whole workspace, and only takes the
# target-dir lock once.
#
# RUSTDOCFLAGS injects the unstable JSON-output flags into every rustdoc
# invocation cargo spawns.
# `--lib` restricts rustdoc to library and proc-macro targets, matching what
# the semver-checks stage consumes; without it, `cargo doc` also documents
# binary targets.
# `--no-deps` skips rustdoc for dependency crates.
build_rustdoc_json() {
    RUSTDOCFLAGS="-Z unstable-options --output-format json" \
        cargo doc --locked --workspace --lib --all-features --no-deps
}

# Build the current and baseline rustdoc JSON in parallel. Each invocation
# uses its own target directory so they don't block on cargo's per-target-dir
# lock.
#
# They will briefly serialise on cargo's shared registry/package cache lock if
# either needs to fetch a new dependency, but otherwise run independently.
# The baseline child also runs its git setup so that IO latency is overlapped
# the the compute of the current-side rustdoc build.
#
# Both children set `CARGO_TARGET_DIR` explicitly so that a caller-provided
# `CARGO_TARGET_DIR` env var (which would otherwise be inherited by both
# invocations) doesn't cause them to share a target directory.
#
# stdout/stderr from the two builds will interleave.
echo "Building current and baseline rustdoc JSON for all workspace crates in parallel"
(
    CARGO_TARGET_DIR="$CURRENT_TARGET_DIR" build_rustdoc_json
) &
current_pid=$!
(
    setup_baseline_checkout
    cd "$BASELINE_DIR"
    CARGO_TARGET_DIR="$BASELINE_DIR/target" build_rustdoc_json
) &
baseline_pid=$!
CHILD_PIDS="$current_pid $baseline_pid"

# Use explicit `|| status=$?` so both children are always reaped (and any error
# message from the second is shown) before the script aborts.
current_status=0
baseline_status=0
wait "$current_pid" || current_status=$?
wait "$baseline_pid" || baseline_status=$?
CHILD_PIDS=
if [ "$current_status" -ne 0 ] || [ "$baseline_status" -ne 0 ]; then
    echo "rustdoc JSON build failed (current=$current_status baseline=$baseline_status)" >&2
    exit 1
fi

# Filter out packages absent from the baseline so the parallel stage doesn't
# fail when new crates are introduced.
# Only the package names (i.e. field 1) are needed from the baseline; the lib
# target name (second TSV column) is taken from the current workspace. The
# `list_lib_crates` call and the `cut` are kept in separate statements so a
# failure in the former isn't masked by `cut` succeeding on empty input.
BASELINE_PACKAGES_LIST=$(list_lib_crates "$BASELINE_DIR")
BASELINE_PACKAGES=$(printf '%s\n' "$BASELINE_PACKAGES_LIST" | cut -f1)
unset BASELINE_PACKAGES_LIST
CHECK_LIST="$TARGET_DIR/semver-checks/to-check.tsv"
: > "$CHECK_LIST"
while IFS="$(printf '\t')" read -r package crate; do
    if printf '%s\n' "$BASELINE_PACKAGES" | grep -qFx "$package"; then
        printf '%s\t%s\n' "$package" "$crate" >> "$CHECK_LIST"
    else
        echo "Skipping $package because it is not present in the baseline"
    fi
done < "$PACKAGE_LIST"

# Guard against the (unexpected) case where we enumerated current-workspace
# crates but ended up with nothing to check. Without this an empty CHECK_LIST
# would make xargs exit 0 and the script silently report success.
if [ -s "$PACKAGE_LIST" ] && [ ! -s "$CHECK_LIST" ]; then
    echo "error: no packages to semver-check (every workspace crate was filtered out)" >&2
    exit 1
fi

# Run the semver-checks in parallel across CPU cores.
# xargs reads each TSV line and splits on whitespace into positional args, so
# in the inner shell:
# - $1 is the package.
# - $2 is lib-target names.
# xargs exits non-zero if any child fails, so a semver violation fails the
# script (after all packages are checked).
echo "Checking packages for semver violations"
PARALLELISM=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)
CURRENT_TARGET_DIR="$CURRENT_TARGET_DIR" BASELINE_DIR="$BASELINE_DIR" \
    xargs -P "$PARALLELISM" -L 1 sh -c '
        set -e
        package=$1
        crate=$2
        cargo semver-checks \
            --package "$package" \
            --current-rustdoc "$CURRENT_TARGET_DIR/doc/$crate.json" \
            --baseline-rustdoc "$BASELINE_DIR/target/doc/$crate.json"
    ' sh < "$CHECK_LIST"
