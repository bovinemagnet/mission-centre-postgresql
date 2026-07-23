#!/bin/sh
# Meson -> Cargo shim. custom_target() cannot chain commands, so the build
# and the copy of the resulting binary happen here.
set -eu

SOURCE_ROOT="$1"
BUILD_ROOT="$2"
OUTPUT="$3"
PROFILE="$4"

CARGO_TARGET_DIR="$BUILD_ROOT/cargo"
export CARGO_TARGET_DIR

if [ "$PROFILE" = "release" ]; then
    cargo build --manifest-path "$SOURCE_ROOT/Cargo.toml" --release
    cp "$CARGO_TARGET_DIR/release/mission-centre-pg" "$OUTPUT"
else
    cargo build --manifest-path "$SOURCE_ROOT/Cargo.toml"
    cp "$CARGO_TARGET_DIR/debug/mission-centre-pg" "$OUTPUT"
fi
