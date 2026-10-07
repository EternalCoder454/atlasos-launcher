#!/bin/bash
# Checks the Plasma update script (plasma-updates/telamon-20261007-launcher-button.js)
# against a real plasmashell in the dev container:
#   scripts/dev.sh bash tests/plasma-update/run.sh
# It builds nothing: run the app build first (CLAUDE.md, "App build"). Output:
# /work/plasma-update/ (probe.txt shows the panel before/after).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
build=${BUILD:-/work/cmake/dev}
export STAGE=/work/stage OUT=/work/plasma-update HERE=$PWD/tests/plasma-update
rm -rf "$STAGE" "$OUT"
mkdir -p "$OUT"
cmake --install "$build" --prefix "$STAGE" >/dev/null
bash "$HERE/inner.sh" >"$OUT/run.log" 2>&1 || true
cat "$OUT/probe.txt"
# The old button is gone, the new one is in its place with the same neighbours.
grep -q 'order=2;[0-9]*;4: 2=org.kde.plasma.digitalclock, 4=org.kde.plasma.showdesktop, [0-9]*=net.eterneon.telamon.launcher.button' "$OUT/probe.txt" || { echo "FAIL: the button was not swapped in place" >&2; exit 1; }
! grep -q 'atlas.launcher.button' "$OUT/appletsrc.after" || { echo "FAIL: the old button is still in the panel" >&2; exit 1; }
grep -q 'KeepMe=yes' "$OUT/appletsrc.after" || { echo "FAIL: its configuration was not carried over" >&2; exit 1; }
grep -q 'telamon-20261007-launcher-button.js' "$OUT/plasmashellrc" || { echo "FAIL: Plasma did not run the script" >&2; exit 1; }
echo "PASS: plasma update script"
