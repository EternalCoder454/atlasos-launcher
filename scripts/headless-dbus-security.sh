#!/bin/bash
# Probes the launcher's D-Bus surface (name ownership, exported methods,
# hostile arguments) on a private session bus, inside the dev container. The
# launcher runs with QT_QPA_PLATFORM=offscreen and every XDG dir under
# /work/xdg/<run>; the user's session is never touched.
#   scripts/dev.sh bash scripts/headless-dbus-security.sh [binary]
# The binary defaults to /work/cmake/dev/telamon-launcher (the app build of
# CLAUDE.md). Output: /work/runs/dbus-security/ (launcher.log), and the
# PASS/FAIL lines on stdout. docs/SECURITY.md, "The D-Bus surface".
set -euo pipefail

bin=${1:-/work/cmake/dev/telamon-launcher}
[ -x "$bin" ] || { echo "no launcher binary at $bin" >&2; exit 2; }
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

out=/work/runs/dbus-security
root=/work/xdg/dbus-security
rm -rf "$out" "$root"
mkdir -p "$out" "$root"/{home,config,data,cache,state} "$root/runtime"
chmod 700 "$root/runtime"
export HOME=$root/home XDG_CONFIG_HOME=$root/config XDG_DATA_HOME=$root/data \
    XDG_CACHE_HOME=$root/cache XDG_STATE_HOME=$root/state XDG_RUNTIME_DIR=$root/runtime
export OUT=$out

# The bus, the portals and Qt are noisy on stderr; the results are the lines
# the probe prints on stdout.
status=0
dbus-run-session -- timeout 300 python3 -I "$here/headless-dbus-security.py" "$bin" \
    >"$out/results.txt" 2>"$out/noise.log" || status=$?
cat "$out/results.txt"
[ "$status" = 0 ] || echo "headless-dbus-security: failed (status $status); see $out/noise.log and launcher.log" >&2
exit "$status"
