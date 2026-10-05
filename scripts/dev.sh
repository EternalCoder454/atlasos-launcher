#!/bin/bash
# Run a command in the dev container (the image CI uses, plus tools for
# headless runs), with the repo at /src, the build output at /work and the
# cargo cache in named podman volumes.
#   scripts/dev.sh <command...>     e.g. scripts/dev.sh cargo test --workspace
#   scripts/dev.sh                  an interactive shell
# /work is $ATLAS_LAUNCHER_WORK, by default ~/.cache/claude-builds/atlas-launcher:
# on disk, outside the repo (cargo's target dirs, CMake build dirs, test
# XDG dirs). Set CARGO_TARGET_DIR to /work/target/<name> to keep one target
# dir per task.
# The image is built from ci/Containerfile on first use, and again whenever the
# Containerfile, the spec's BuildRequires or the atlas-framework tag in
# Cargo.toml change (ci/image-tag.sh). It needs network access: it builds
# atlas-ui from atlas-framework's tag. ATLAS_FRAMEWORK_REF=<tag or branch>
# builds from another ref (the image is rebuilt when it changes).
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
image=localhost/atlas-launcher-dev:44
work=${ATLAS_LAUNCHER_WORK:-$HOME/.cache/claude-builds/atlas-launcher}
mkdir -p "$work"
pinned=$("$repo/ci/framework-ref.sh")
ref=${ATLAS_FRAMEWORK_REF:-$pinned}
want=$("$repo/ci/image-tag.sh")
[ "$ref" = "$pinned" ] || want=$want-$ref

have=$(podman image inspect --format '{{ index .Labels "net.eterneon.atlas.launcher.image-tag" }}' "$image" 2>/dev/null || true)
if [ "$have" != "$want" ]; then
    podman build -f "$repo/ci/Containerfile" --target dev \
        --build-arg ATLAS_FRAMEWORK_REF="$ref" --build-arg IMAGE_TAG="$want" \
        -t "$image" "$repo" >&2
fi

tty=()
[ -t 0 ] && tty=(-it)
# No SELinux relabelling (:z/:Z) of host folders: it would lock other
# containers and tools out of them. Labels are off for the container.
# --init: a real PID 1, so timeout, dbus-run-session and the headless
# runs signal and reap their children as they do outside a container.
exec podman run --rm --init --security-opt label=disable "${tty[@]}" \
    -v "$repo":/src -w /src \
    -v "$work":/work \
    -v atlas-cargo:/root/.cargo/registry \
    -v atlas-cargo-git:/root/.cargo/git \
    -e CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/work/target/dev}" \
    "$image" "${@:-bash}"
