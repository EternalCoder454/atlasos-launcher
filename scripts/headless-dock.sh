#!/bin/bash
# The dock against the launcher, in a headless Plasma, inside the dev container,
# never on the user's desktop: a private session bus, a private KWin in a
# private Xvfb (which is what the screenshots capture), plasmashell with a
# bottom panel holding Icons-only Task Manager with one pinned app, the
# launcher, and a scratch home with every XDG dir under /work/xdg/<run>.
#   scripts/dev.sh bash scripts/headless-dock.sh <run> <launcher binary> <testapp> <plain|renamed|override> [light|dark]
# plain: the app has no name of the user's. renamed: names.conf holds one
# (0.3.1). override: a user override desktop file with the same id and the
# new Name (what 0.3.2 writes), made by hand here.
# It starts the app from the launcher (typed query, Enter) and looks at the
# dock and at the windows KWin knows (app id, caption). Output:
# /work/runs/<run>/ (dock shots, windows.txt, steps.log, results.txt).
set -euo pipefail

run=${1:?usage: headless-dock.sh <run> <binary> <testapp> <plain|renamed|override>}
bin=${2:?binary}
testapp=${3:?testapp}
mode=${4:-plain}
case $run in .* | *[!A-Za-z0-9._-]* | '') echo "bad run name" >&2; exit 2 ;; esac
case $mode in plain | renamed | override | explore | uirename) ;; *) echo "mode: plain, renamed or override" >&2; exit 2 ;; esac
[ -x "$bin" ] && [ -x "$testapp" ] || { echo "no launcher or test app" >&2; exit 2; }
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

out=/work/runs/$run
root=/work/xdg/$run
[ ! -e "$out" ] && [ ! -e "$root" ] || { echo "the run $run exists; use another name" >&2; exit 2; }
mkdir -p "$out" "$root"/{home,config,data,cache,state} "$root/runtime" "$root/data/applications" "$root/usr/share/applications"
chmod 700 "$root/runtime"
# The made-up apps are a system folder's, as an installed app's file is.
export XDG_DATA_DIRS=$root/usr/share:/usr/local/share:/usr/share
export HOME=$root/home XDG_CONFIG_HOME=$root/config XDG_DATA_HOME=$root/data \
    XDG_CACHE_HOME=$root/cache XDG_STATE_HOME=$root/state XDG_RUNTIME_DIR=$root/runtime

# Two made-up apps; the test app is a window with the id it is given.
mkapp() { # file id name
    cat >"$2" <<DESK
[Desktop Entry]
Type=Application
Name=$3
Icon=system-file-manager
Exec=$testapp $1 "$3 window"
Categories=Utility;
StartupNotify=true
DESK
}
mkapp org.example.files "$root/usr/share/applications/org.example.files.desktop" "Files"
mkapp org.example.other "$root/usr/share/applications/org.example.other.desktop" "Other"
mkdir -p "$XDG_CONFIG_HOME/telamon-launcher" "$XDG_STATE_HOME/telamon-launcher"
printf 'org.example.other.desktop\n' >"$XDG_CONFIG_HOME/telamon-launcher/pinned.list"
printf '[Search]\nWebSearch=false\nFileSearch=false\n' >"$XDG_CONFIG_HOME/telamon-launcher/launcher.conf"
printf '[Appearance]\nTransparency=false\n' >"$XDG_CONFIG_HOME/telamonrc"
case $mode in
    renamed) printf '[Names]\norg.example.files.desktop=My Files\n' >"$XDG_CONFIG_HOME/telamon-launcher/names.conf" ;;
    override)
        # What the menu editor and 0.3.2 write: the same id, the new Name.
        mkapp org.example.files "$XDG_DATA_HOME/applications/org.example.files.desktop" "My Files"
        ;;
esac

cat >"$XDG_CONFIG_HOME/plasma-org.kde.plasma.desktop-appletsrc" <<RC
[Containments][1]
activityId=
formfactor=2
immutability=1
lastScreen=0
location=4
plugin=org.kde.panel
wallpaperplugin=org.kde.image

[Containments][1][Applets][2]
immutability=1
plugin=org.kde.plasma.icontasks

[Containments][1][Applets][2][Configuration][General]
launchers=applications:org.example.files.desktop
iconSpacing=1

[Containments][1][General]
AppletOrder=2

[Containments][5]
activityId=
formfactor=0
immutability=1
lastScreen=0
location=0
plugin=org.kde.plasma.folder
wallpaperplugin=org.kde.image
RC

# shellcheck disable=SC2016
exec dbus-run-session -- bash -c '
    set -uo pipefail
    out=$1 bin=$2 testapp=$3 mode=$4 here=$5 app= kwin= xvfb= ps= failures=0
    log() { echo "$(date +%T.%3N) $*" >>"$out/steps.log"; }
    fail() { log "FAILED: $*"; failures=$((failures + 1)); echo "FAILED: $*" >>"$out/results.txt"; }
    res() { echo "$*" >>"$out/results.txt"; }
    Xvfb -displayfd 3 -screen 0 1920x1080x24 -nolisten tcp 3>"$XDG_RUNTIME_DIR/display" >"$out/xvfb.log" 2>&1 &
    xvfb=$!
    cleanup() {
        for p in $app $ps $kwin $xvfb; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null; done
        pkill -9 -f "^$testapp " 2>/dev/null
        wait 2>/dev/null
    }
    trap cleanup EXIT
    trap "exit 143" TERM INT HUP
    for _ in $(seq 50); do [ -s "$XDG_RUNTIME_DIR/display" ] && break; sleep 0.1; done
    xdisplay=:$(head -1 "$XDG_RUNTIME_DIR/display")
    kwin_bin=$XDG_RUNTIME_DIR/kwin_wayland
    cp /usr/bin/kwin_wayland "$kwin_bin"
    DISPLAY=$xdisplay QT_FORCE_STDERR_LOGGING=1 QT_LOGGING_RULES="js=true;kwin_scripting=true" "$kwin_bin" --x11-display "$xdisplay" --no-lockscreen --width 1920 --height 1080 --socket wl-test >"$out/kwin.log" 2>&1 &
    kwin=$!
    for _ in $(seq 100); do [ -S "$XDG_RUNTIME_DIR/wl-test" ] && break; sleep 0.1; done
    export WAYLAND_DISPLAY=wl-test QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software QT_QPA_PLATFORMTHEME= QT_FORCE_STDERR_LOGGING=1
    dbus-update-activation-environment XDG_DATA_DIRS WAYLAND_DISPLAY QT_QPA_PLATFORM QT_QUICK_BACKEND HOME XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_STATE_HOME XDG_RUNTIME_DIR 2>/dev/null
    plasmashell --no-respawn >"$out/plasmashell.log" 2>&1 &
    ps=$!
    sleep 14
    QT_LOGGING_RULES="telamon.launcher*.debug=true" "$bin" --daemon >"$out/app.log" 2>&1 &
    app=$!
    name=net.eterneon.telamon.launcher
    for _ in $(seq 100); do
        timeout 5 gdbus call --session -d org.freedesktop.DBus -o /org/freedesktop/DBus -m org.freedesktop.DBus.NameHasOwner "$name" 2>/dev/null | grep -q true && break
        sleep 0.1
    done
    sleep 3
    shot() { sleep 0.8; DISPLAY=$xdisplay timeout 20 import -window root "$out/$1.png" 2>>"$out/steps.log" || fail "shot $1"; convert "$out/$1.png" -crop 1920x120+0+960 +repage "$out/$1-dock.png" 2>/dev/null; log "shot $1"; }
    # The windows KWin knows: app id, caption and startup id, by a KWin script.
    windows() {
        cat >"$XDG_RUNTIME_DIR/dump.js" <<JS
var out = [];
workspace.windowList().forEach(function (w) { if (w.normalWindow) out.push(w.desktopFileName + "|" + w.resourceClass + "|" + w.caption); });
print("WINDOWS " + out.join(" ;; "));
JS
        local id
        id=$(timeout 10 gdbus call --session -d org.kde.KWin -o /Scripting -m org.kde.kwin.Scripting.loadScript "$XDG_RUNTIME_DIR/dump.js" "dump$RANDOM" 2>/dev/null | sed -n "s/.*(\([0-9-]*\),).*/\1/p")
        timeout 10 gdbus call --session -d org.kde.KWin -o /Scripting/Script$id -m org.kde.kwin.Script.run >/dev/null 2>&1
        sleep 1
        timeout 10 gdbus call --session -d org.kde.KWin -o /Scripting/Script$id -m org.kde.kwin.Script.stop >/dev/null 2>&1
    }
    call() { timeout 10 gdbus call --session -d $name -o /net/eterneon/telamon/launcher -m "net.eterneon.telamon.Launcher1.$1" "${@:2}" >>"$out/steps.log" 2>&1 || fail "call $1"; }

    shot 0-before
    ov=$XDG_DATA_HOME/applications/org.example.files.desktop
    if [ "$mode" = uirename ]; then
        # Rename App... from the context menu of the tile.
        call Show start "" "{}"; sleep 1.5
        DISPLAY=$xdisplay xdotool mousemove 805 800; sleep 0.4; DISPLAY=$xdisplay xdotool click 3; sleep 1
        DISPLAY=$xdisplay xdotool mousemove 888 852; sleep 0.4; DISPLAY=$xdisplay xdotool click 1; sleep 1
        DISPLAY=$xdisplay xdotool type --delay 80 -- "My Files"; sleep 0.5
        shot 1-editor
        DISPLAY=$xdisplay xdotool key Return; sleep 4
        shot 1b-renamed
        if [ -f "$ov" ]; then res "override written:"; sed "s/^/    /" "$ov" | tee -a "$out/results.txt"; else fail "no override file"; fi
        call Hide; sleep 1
        # The dock: hover the pinned icon for its tooltip.
        DISPLAY=$xdisplay xdotool mousemove 31 1057; sleep 2.5
        shot 1c-dock-tooltip
        DISPLAY=$xdisplay xdotool mousemove 960 300; sleep 0.5
    fi
    if [ "$mode" = renamed ]; then
        # A name from 0.3.1 (names.conf only): written to a desktop file at start.
        if grep -q "X-Telamon-Renamed=true" "$ov" 2>/dev/null; then res "migrated: the override was written at start"; else fail "names.conf was not carried to a desktop file"; fi
        DISPLAY=$xdisplay xdotool mousemove 31 1057; sleep 2.5
        shot 1c-dock-tooltip
        DISPLAY=$xdisplay xdotool mousemove 960 300; sleep 0.5
    fi
    if [ "$mode" = explore ]; then call Show start "" "{}"; sleep 1.5; shot explore; exit 0; fi
    if [ "$mode" = menuonly ]; then exit 0; fi
    # Starts the app from the panel (typed query, Enter), looks at the dock and
    # at the windows KWin knows, then ends the app.
    launch() { # <label>
        call Show start "" "{}"; sleep 1.2
        DISPLAY=$xdisplay xdotool type --delay 80 -- "${QUERY:-Files}"; sleep 1.5
        DISPLAY=$xdisplay xdotool key Return
        sleep 6
        shot "$1-launched"
        windows
        grep "WINDOWS" "$out/kwin.log" | tail -1 | sed "s/^/$1 /" | tee -a "$out/results.txt"
        pkill -f "^$testapp" 2>/dev/null; sleep 2
        shot "$1-after-close"
    }
    restart_launcher() {
        kill "$app"; wait "$app" 2>/dev/null
        QT_LOGGING_RULES="telamon.launcher*.debug=true" "$bin" --daemon >>"$out/app.log" 2>&1 &
        app=$!
        for _ in $(seq 100); do
            timeout 5 gdbus call --session -d org.freedesktop.DBus -o /org/freedesktop/DBus -m org.freedesktop.DBus.NameHasOwner "$name" 2>/dev/null | grep -q true && break
            sleep 0.1
        done
        sleep 3
    }
    if [ -z "${SKIP_LAUNCH:-}" ]; then
        launch L1
        launch L2
        restart_launcher
        launch L3
    fi
    if [ "$mode" = uirename ]; then
        # Reset Name from the context menu (the tile of the app is where the
        # recent row pushed it to: found by the typed query instead).
        call Show start "" "{}"; sleep 1.2
        DISPLAY=$xdisplay xdotool type --delay 80 -- "My Files"; sleep 1.5
        shot 5-search
        DISPLAY=$xdisplay xdotool mousemove 820 495; sleep 0.4; DISPLAY=$xdisplay xdotool click 3; sleep 1
        shot 5b-search-menu
        DISPLAY=$xdisplay xdotool mousemove 896 579; sleep 0.4; DISPLAY=$xdisplay xdotool click 1; sleep 4
        shot 6-after-reset
        if [ -e "$ov" ]; then fail "the override is still there after Reset Name"; else res "Reset Name removed the override"; fi
        call Hide; sleep 1
        DISPLAY=$xdisplay xdotool mousemove 31 1057; sleep 2.5
        shot 6b-dock-tooltip-after-reset
    fi
    res "failures $failures"
    [ "$failures" = 0 ]
' _ "$out" "$bin" "$testapp" "$mode" "$here"
