#!/usr/bin/env bash

set -euo pipefail

tags_output="$(git tag --points-at HEAD -- 'v*')"
count="$(printf '%s\n' "$tags_output" | sed '/^$/d' | wc -l | tr -d ' ')"

if [ "$count" -ne 1 ]; then
  echo "error: expected exactly one v* tag on HEAD, found $count" >&2
  if [ "$count" -gt 0 ]; then
    printf 'tags on HEAD:\n%s\n' "$tags_output" >&2
  fi
  exit 1
fi

printf '%s\n' "$tags_output" | sed '/^$/d' | head -n 1
