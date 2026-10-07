#!/bin/bash
# Print the tag of the dev image for this checkout: 44-<hash>, where the hash
# covers everything that changes the image's content: ci/Containerfile, the
# spec's BuildRequires and the atlas-framework tag. A changed Containerfile in
# a pull request therefore gets its own image; CI never runs in a stale one.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
hash=$({
    cat ci/Containerfile
    grep -E '^BuildRequires:' packaging/telamon-launcher.spec
    ci/framework-ref.sh
} | sha256sum | cut -c1-16)
printf '44-%s\n' "$hash"
