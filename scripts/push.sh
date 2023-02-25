#!/bin/bash
set -e
if ! git diff-index --quiet HEAD --; then
  echo "Please commit all changes first" >&2
  exit 1
fi
MIN_RUST_VER=$(grep ^rust-version evcxr/Cargo.toml | cut -d'"' -f2)
if [ -z "$MIN_RUST_VER" ]; then
  echo "Failed to determine 