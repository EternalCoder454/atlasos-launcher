#!/bin/bash
# Refuse to publish an image that holds a secret. Run before every push: an
# image pushed from a public repo is public at once.
#   ci/secret-check.sh <image>
# Checks the image's environment and build history, and every file in it that
# no RPM owns (packages carry test keys of their own; what we add is what
# matters), for private keys and the token formats of GitHub, GitLab, AWS,
# Slack, Google and npm.
set -euo pipefail
image=${1:?usage: secret-check.sh <image>}
pattern='-----BEGIN ([A-Z]+ )?PRIVATE KEY-----|gh[pousr]_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{60,}|glpat-[A-Za-z0-9_-]{20}|AKIA[0-9A-Z]{16}|xox[abpr]-[A-Za-z0-9-]{10,}|AIza[0-9A-Za-z_-]{35}|npm_[A-Za-z0-9]{36}'
found=0

if podman image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$image" |
    grep -Ei '(TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|API_?KEY)[A-Z_]*=' ; then
    echo "secret-check: a secret-looking variable in the image's environment" >&2
    found=1
fi
if podman history --no-trunc --format '{{.CreatedBy}}' "$image" | grep -E -- "$pattern"; then
    echo "secret-check: a secret in the image's build history" >&2
    found=1
fi
# Files no package owns, scanned inside the image itself.
if ! podman run --rm --network none --entrypoint /bin/bash "$image" -c '
    set -euo pipefail
    comm -13 <(rpm -qal | sort -u) <(find / -xdev -type f ! -path "/proc/*" ! -path "/sys/*" 2>/dev/null | sort -u) >/tmp/unowned
    echo "secret-check: $(wc -l </tmp/unowned) files no package owns" >&2
    if tr "\n" "\0" </tmp/unowned | xargs -0 -r grep -lIE -- "$1" 2>/dev/null; then
        exit 1
    fi' bash "$pattern"; then
    echo "secret-check: a secret in a file of the image" >&2
    found=1
fi
if [ "$found" != 0 ]; then
    echo "secret-check: refusing to publish $image" >&2
    exit 1
fi
echo "secret-check: $image is clean"
