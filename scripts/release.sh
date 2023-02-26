#!/bin/bash
set -e
VERSION=$(perl -e \
  'while (<>) { if (/^# Version (\d+\.\d+\.\d+) \(unreleased\)/) {print "$1"}}' \
  RELEASE_NOTES.md \
)
if [ -z "$VERSION" ]; then
  echo "RELEASE_NOTES.md doesn't contain an unreleased version" >&2
  exit 1
fi
if ! git diff-index --quiet HEAD --; then
  echo "Please commit all changes first" >&2
  exit 1
fi
MIN_RUST_VER=$(grep ^rust-version evcxr/Cargo.toml | cut -d'"' -f2)
if [ -z "$MIN_RUST_VER" ]; then
  echo "Failed to determine minimum rust version" >&2
  exit 1
fi
echo "Releasing $VERSION"
git pull --rebase
perl -pi -e 's/(^# .*) \(unreleased\)$/$1/' RELEASE_NOTES.md
perl 