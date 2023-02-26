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
if ! gi