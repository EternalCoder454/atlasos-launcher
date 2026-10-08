#!/bin/bash
# The panel's look in a headless launcher, inside the dev container, never on
# the user's desktop: a private session bus, a private KWin in a private Xvfb
# (which is what the screenshots capture), a scratch home with every XDG dir
# under /work/xdg/<run>, and a few made-up apps. It checks the see-through
# panel (Telamon.Ui's Transparency setting and the compositor's blur) and the
# hover highlights of the account and power buttons.
#   scripts/dev.sh bash scripts/headless-look.sh <run> <binary> <shim.so> <light|dark> <scale> [initials|picture]
# The avatar shows initials (no AccountsService on the private bus), or with
# "picture" a flat orange picture served by scripts/headless-accounts.py.
# The private KWin draws with QPainter and offers no blur, so
# scripts/headless-blurshim.cpp stands in for the two KWindowEffects calls
# (preloaded): it says blur is available when $BLURSHIM_STATE holds "1", and
# logs each blur request with its region. A backdrop window
# (scripts/headless-backdrop.qml.in) sits behind the panel. The see-through
# shots come twice: as KWin's QPainter composites them (the panel's alpha over
# the sharp backdrop), and as the blur would (scripts/headless-emulate.sh: the
# backdrop, Gaussian-blurred inside the logged region, under the panel).
# The steps are in headless-look-steps.sh. Output: /work/runs/<run>/ (shots,
# steps.log, blur.log, results.txt); the exit status is the number of failed
# checks. A run name is used once.
set -euo pipefail

run=${1:?usage: headless-look.sh <run> <binary> <shim.so> <light|dark> <scale> [initials|picture]}
bin=${2:?binary}
shim=${3:?shim}
scheme=${4:-light}
scale=${5:-1}
face=${6:-initials}
case $run in .* | *[!A-Za-z0-9._-]* | '') echo "bad run name" >&2; exit 2 ;; esac
case $scheme in light | dark) ;; *) echo "the scheme is light or dark" >&2; exit 2 ;; esac
case $face in initials | picture) ;; *) echo "the avatar is initials or picture" >&2; exit 2 ;; esac
case $scale in 1 | 1.25 | 1.5 | 1.7 | 2) ;; *) echo "scale: 1, 1.25, 1.5, 1.7 or 2" >&2; exit 2 ;; esac
[ -x "$bin" ] || { echo "no launcher binary at $bin" >&2; exit 2; }
[ -f "$shim" ] || { echo "no blur shim at $shim" >&2; exit 2; }
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

out=/work/runs/$run
root=/work/xdg/$run
[ ! -e "$out" ] && [ ! -e "$root" ] || { echo "the run $run exists; use another name" >&2; exit 2; }
mkdir -p "$out" "$root"/{home,config,data,cache,state} "$root/runtime" "$root/data/applications"
chmod 700 "$root/runtime"
export HOME=$root/home XDG_CONFIG_HOME=$root/config XDG_DATA_HOME=$root/data \
    XDG_CACHE_HOME=$root/cache XDG_STATE_HOME=$root/state XDG_RUNTIME_DIR=$root/runtime

mkapp() { # id name icon categories
    cat >"$XDG_DATA_HOME/applications/$1.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=$2
Icon=$3
Exec=true
Categories=$4
DESK
}
mkapp org.example.files "Dolphin" system-file-manager "System;FileManager;"
mkapp org.example.browser "Firefox" internet-web-browser "Network;WebBrowser;"
mkapp org.example.editor "Kate" accessories-text-editor "Utility;TextEditor;"
mkapp org.example.terminal "Konsole" utilities-terminal "System;TerminalEmulator;"
mkapp org.example.music "Elisa" multimedia-audio-player "AudioVideo;Player;"
mkapp org.example.paint "Krita" applications-graphics "Graphics;"
mkapp org.example.calc "Calculator" accessories-calculator "Utility;Calculator;"
mkapp org.example.chat "Telegram" internet-chat "Network;Chat;"
mkdir -p "$XDG_CONFIG_HOME/telamon-launcher" "$XDG_STATE_HOME/telamon-launcher"
printf 'org.example.files.desktop\norg.example.browser.desktop\norg.example.editor.desktop\norg.example.terminal.desktop\n' >"$XDG_CONFIG_HOME/telamon-launcher/pinned.list"
printf '[Search]\nWebSearch=false\nFileSearch=false\n' >"$XDG_CONFIG_HOME/telamon-launcher/launcher.conf"
# The Transparency switch of Telamon.Ui (Settings, Appearance): on.
printf '[Appearance]\nTransparency=true\n' >"$XDG_CONFIG_HOME/telamonrc"

# shellcheck disable=SC2016
exec dbus-run-session -- bash -c '
    set -uo pipefail
    out=$1 bin=$2 shim=$3 scheme=$4 scale=$5 here=$6 face=$7 app= kwin= xvfb= bd= failures=0
    log() { echo "$(date +%T.%3N) $*" >>"$out/steps.log"; }
    fail() { log "FAILED: $*"; failures=$((failures + 1)); echo "FAILED: $*" >>"$out/results.txt"; }
    res() { echo "$*" >>"$out/results.txt"; }
    # The screen is 1920x1080 logical pixels at any scale (LOOK_LOGICAL=WxH
    # for another, as 1506x847 is a 2560x1440 screen at 1.7).
    lw=${LOOK_LOGICAL:-}; lh=${lw#*x}; lw=${lw%x*}
    sw=$(awk -v s="$scale" -v w="${lw:-1920}" "BEGIN { printf \"%d\", w * s + 0.5 }")
    sh=$(awk -v s="$scale" -v w="${lh:-1080}" "BEGIN { printf \"%d\", w * s + 0.5 }")
    Xvfb -displayfd 3 -screen 0 ${sw}x${sh}x24 -nolisten tcp 3>"$XDG_RUNTIME_DIR/display" >"$out/xvfb.log" 2>&1 &
    xvfb=$!
    cleanup() {
        for p in $bd $app ${accounts:-} $kwin $xvfb; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null; done
        wait 2>/dev/null
    }
    trap cleanup EXIT
    trap "exit 143" TERM INT HUP
    for _ in $(seq 50); do [ -s "$XDG_RUNTIME_DIR/display" ] && break; sleep 0.1; done
    [ -s "$XDG_RUNTIME_DIR/display" ] || { echo "Xvfb did not start" >&2; exit 1; }
    xdisplay=:$(head -1 "$XDG_RUNTIME_DIR/display")
    kwin_bin=$XDG_RUNTIME_DIR/kwin_wayland
    cp /usr/bin/kwin_wayland "$kwin_bin"
    DISPLAY=$xdisplay "$kwin_bin" --x11-display "$xdisplay" --no-lockscreen \
        --width "$sw" --height "$sh" --socket wl-test >"$out/kwin.log" 2>&1 &
    kwin=$!
    for _ in $(seq 100); do [ -S "$XDG_RUNTIME_DIR/wl-test" ] && break; sleep 0.1; done
    [ -S "$XDG_RUNTIME_DIR/wl-test" ] || { echo "kwin did not start" >&2; exit 1; }

    export WAYLAND_DISPLAY=wl-test QT_QPA_PLATFORM=wayland RUST_BACKTRACE=1
    export QT_QPA_PLATFORMTHEME= QT_FORCE_STDERR_LOGGING=1
    export QT_LOGGING_RULES="telamon.launcher*.debug=true"
    export QT_MESSAGE_PATTERN="%{time hh:mm:ss.zzz} %{category} %{type}: %{message}"
    # The scale is a Qt factor (the windowed KWin has no fractional
    # output scale that Qt follows), so the panel is laid out as at that scale.
    if [ "$scale" != 1 ]; then export QT_ENABLE_HIGHDPI_SCALING=1 QT_SCALE_FACTOR=$scale; fi
    if [ "$scheme" = dark ]; then
        printf "[Colors:Window]\nBackgroundNormal=32,34,38\nForegroundNormal=235,236,240\n[Colors:View]\nBackgroundNormal=24,26,30\nForegroundNormal=235,236,240\n[General]\nColorScheme=BreezeDark\n" >"$XDG_CONFIG_HOME/kdeglobals"
    fi

    # The compositor starts without blur, as at login before its effects load.
    export BLURSHIM_STATE=$XDG_RUNTIME_DIR/blur-state BLURSHIM_LOG=$out/blur.log
    echo 0 >"$BLURSHIM_STATE"
    if [ "$face" = picture ]; then
        convert -size 128x128 xc:"#e8590c" "$HOME/face.png"
        # The launcher asks AccountsService on the system bus: a private one.
        export DBUS_SYSTEM_BUS_ADDRESS=$(dbus-daemon --session --fork --print-address)
        python3 "$here/headless-accounts.py" Riley "$HOME/face.png" >"$out/accounts.log" 2>&1 &
        accounts=$!
        sleep 1
    fi
    LD_PRELOAD=$shim "$bin" --daemon >"$out/app.log" 2>&1 &
    app=$!
    name=net.eterneon.telamon.launcher
    path=/net/eterneon/telamon/launcher
    iface=net.eterneon.telamon.Launcher1
    for _ in $(seq 100); do
        timeout 5 gdbus call --session -d org.freedesktop.DBus -o /org/freedesktop/DBus \
            -m org.freedesktop.DBus.NameHasOwner "$name" 2>/dev/null | grep -q true && break
        kill -0 $app 2>/dev/null || { echo "the launcher died" >&2; exit 1; }
        sleep 0.05
    done
    sleep 3
    alive() { kill -0 "$app" 2>/dev/null || { log "app DIED before: $1"; exit 1; }; }
    call() { alive "$1"; timeout 10 gdbus call --session -d $name -o $path -m "$iface.$1" "${@:2}" >>"$out/steps.log" 2>&1 || fail "call $1"; }
    shot() { alive "shot $1"; sleep 0.7; DISPLAY=$xdisplay timeout 20 import -window root "$out/$1.png" 2>>"$out/steps.log" || fail "shot $1"; log "shot $1"; }
    move() { DISPLAY=$xdisplay xdotool mousemove "$1" "$2"; sleep 0.5; }
    setconf() { kwriteconfig6 --notify --file telamonrc --group Appearance --key Transparency "$1"; sleep 0.8; }
    lastblur() { tail -1 "$out/blur.log" 2>/dev/null; }
    backdrop() { # pattern|black|white|none
        if [ -n "$bd" ]; then kill "$bd" 2>/dev/null; wait "$bd" 2>/dev/null; fi
        bd=
        [ "$1" = none ] && return 0
        sed "s/@KIND@/$1/" "$here/headless-backdrop.qml.in" >"$XDG_RUNTIME_DIR/backdrop.qml"
        /usr/lib64/qt6/bin/qml "$XDG_RUNTIME_DIR/backdrop.qml" >>"$out/backdrop.log" 2>&1 &
        bd=$!
        sleep 2.5
    }

    . "$here/${LOOK_STEPS:-headless-look-steps.sh}"

    res "failures $failures"
    [ "$failures" = 0 ]
' _ "$out" "$bin" "$shim" "$scheme" "$scale" "$here" "$face"
