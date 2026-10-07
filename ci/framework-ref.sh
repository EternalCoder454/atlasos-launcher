#!/bin/bash
# Print the atlas-framework tag the crates are pinned to (Cargo.toml).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
ref=$(sed -n 's/^telamon-framework-ui = .*tag = "\([^"]*\)".*/\1/p' Cargo.toml)
# The tag ends up in a git command and an image tag: only a plain tag name.
[[ $ref =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.]+)?$ ]] || {
    echo "framework-ref: no valid atlas-framework tag in Cargo.toml (got '$ref')" >&2
    exit 1
}
printf '%s\n' "$ref"
