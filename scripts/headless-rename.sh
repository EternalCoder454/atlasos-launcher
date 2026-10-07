#!/bin/bash
# Drives "Rename App…" in a headless launcher, inside the dev container,
# never on the user's desktop: a private session bus, a private KWin in a
# private Xvfb (which is what the screenshots capture), a scratch home with
# every XDG dir under /work/xdg/<run>, and a handful of made-up apps.
#   scripts/dev.sh bash scripts/headless-rename.sh <run name> [binary] [light|dark]
# xdotool clicks and types into the Xvfb screen KWin shows, so the panel gets
# real pointer and key events. Output: /work/runs/<run>/ (shots, logs, steps.log);
# the exit status is the number of failed checks (0: all passed).
set -euo pipefail

run=${1:?usage: headless-rename.sh <run name> [binary] [light|dark]}
bin=${2:-/work/cmake/dev/telamon-launcher}
scheme=${3:-light}
case $run in .* | *[!A-Za-z0-9._-]* | '') echo "bad run name" >&2; exit 2 ;; esac
case $scheme in light | dark) ;; *) echo "the scheme is light or dark" >&2; exit 2 ;; esac
[ -x "$bin" ] || { echo "no launcher binary at $bin" >&2; exit 2; }

out=/work/runs/$run
root=/work/xdg/$run
rm -rf "$out" "$root"
mkdir -p "$out" "$root"/{home,config,data,cache,state} "$root/runtime" "$root/data/applications"
chmod 700 "$root/runtime"
export HOME=$root/home XDG_CONFIG_HOME=$root/config XDG_DATA_HOME=$root/data \
    XDG_CACHE_HOME=$root/cache XDG_STATE_HOME=$root/state XDG_RUNTIME_DIR=$root/runtime

# Made-up apps (Exec=true: nothing ever runs from here).
mkapp() { # id name icon categories [keywords]
    cat >"$XDG_DATA_HOME/applications/$1.desktop" <<DESK
[Desktop Entry]
Type=Application
Name=$2
Icon=$3
Exec=true
Categories=$4
Keywords=${5:-}
DESK
}
mkapp org.example.files "Dolphin" system-file-manager "System;FileManager;"
mkapp org.example.browser "Firefox" internet-web-browser "Network;WebBrowser;" "internet;www;"
mkapp org.example.editor "Kate" accessories-text-editor "Utility;TextEditor;"
mkapp org.example.terminal "Konsole" utilities-terminal "System;TerminalEmulator;"
mkapp org.example.music "Elisa" multimedia-audio-player "AudioVideo;Player;"
mkapp org.example.paint "Krita" applications-graphics "Graphics;"
mkapp org.example.calc "Calculator" accessories-calculator "Utility;Calculator;"
mkapp org.example.chat "Telegram" internet-chat "Network;Chat;"
mkdir -p "$XDG_CONFIG_HOME/telamon-launcher" "$XDG_STATE_HOME/telamon-launcher"
printf 'org.example.files.desktop\norg.example.browser.desktop\norg.example.editor.desktop\norg.example.terminal.desktop\n' >"$XDG_CONFIG_HOME/telamon-launcher/pinned.list"
printf '[Search]\nWebSearch=false\nFileSearch=false\n' >"$XDG_CONFIG_HOME/telamon-launcher/launcher.conf"
now=$(date +%s)
printf 'x\tapp:org.example.music.desktop\t3\t%s\nx\tapp:org.example.paint.desktop\t2\t%s\n' "$now" "$((now - 60))" >"$XDG_STATE_HOME/telamon-launcher/usage.tsv"
# Names set before the launcher starts, by hand: shown from the first frame
# when they are good, cleaned or dropped when they are not (a bidi override
# and a control character inside a name, a name past 64 characters, an id
# that is no desktop file id) and kept, unused, for an app that is not here.
{
    printf '[Names]\norg.example.calc.desktop=Quick Maths\n'
    printf 'org.example.terminal.desktop=Con\xe2\x80\xaesole\x01 Window\n'
    printf 'org.example.paint.desktop=%s\n' "$(printf 'P%.0s' $(seq 100))"
    printf '../evil.desktop=Evil\norg.example.gone.desktop=Ghost\n'
} >"$XDG_CONFIG_HOME/telamon-launcher/names.conf"
names=$XDG_CONFIG_HOME/telamon-launcher/names.conf

# The inner part runs on a private bus that only this container sees.
# shellcheck disable=SC2016
exec dbus-run-session -- bash -c '
    set -uo pipefail
    out=$1 bin=$2 scheme=$3 app= kwin= xvfb= failures=0
    log() { echo "$(date +%T.%3N) $*" >>"$out/steps.log"; }
    fail() { log "FAILED: $*"; failures=$((failures + 1)); }
    Xvfb -displayfd 3 -screen 0 1920x1080x24 -nolisten tcp 3>"$XDG_RUNTIME_DIR/display" >"$out/xvfb.log" 2>&1 &
    xvfb=$!
    cleanup() {
        for p in $app $kwin $xvfb; do kill -9 "$p" 2>/dev/null; done
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
        --width 1920 --height 1080 --socket wl-test >"$out/kwin.log" 2>&1 &
    kwin=$!
    for _ in $(seq 100); do [ -S "$XDG_RUNTIME_DIR/wl-test" ] && break; sleep 0.1; done
    [ -S "$XDG_RUNTIME_DIR/wl-test" ] || { echo "kwin did not start" >&2; exit 1; }

    # The colour scheme the panel is drawn in (Kirigami follows the palette).
    export WAYLAND_DISPLAY=wl-test QT_QPA_PLATFORM=wayland RUST_BACKTRACE=1
    export QT_QPA_PLATFORMTHEME= QT_FORCE_STDERR_LOGGING=1
    export QT_LOGGING_RULES="telamon.launcher*.debug=true"
    export QT_MESSAGE_PATTERN="%{time hh:mm:ss.zzz} %{category} %{type}: %{message}"
    if [ "$scheme" = dark ]; then
        mkdir -p "$XDG_CONFIG_HOME"
        printf "[Colors:Window]\nBackgroundNormal=32,34,38\nForegroundNormal=235,236,240\n[Colors:View]\nBackgroundNormal=24,26,30\nForegroundNormal=235,236,240\n[General]\nColorScheme=BreezeDark\n" >"$XDG_CONFIG_HOME/kdeglobals"
    fi
    "$bin" --daemon >"$out/app.log" 2>&1 &
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
    shot() { alive "shot $1"; sleep 0.6; DISPLAY=$xdisplay timeout 20 import -window root "$out/$1.png" 2>>"$out/steps.log" || fail "shot $1"; log "shot $1"; }
    click() { DISPLAY=$xdisplay xdotool mousemove "$1" "$2"; sleep 0.25; DISPLAY=$xdisplay xdotool click "${3:-1}"; sleep 0.6; }
    key() { DISPLAY=$xdisplay xdotool key --delay 80 "$@"; sleep 0.5; }
    typeit() { DISPLAY=$xdisplay xdotool type --delay 60 -- "$1"; sleep 0.5; }
    names_has() { grep -qF -- "$1" "$names_file"; }
    names_file=$4
    has() { grep -qxF -- "$1" "$names_file" || fail "names.conf lacks: $1"; }
    lacks() { ! grep -qF -- "$1" "$names_file" || fail "names.conf still has: $1"; }
    shotname() { echo "$scheme-$1"; }

    log "tile: Rename App… on the Firefox tile, typed over the selected name"
    call Show start "" "{}"; sleep 1; shot "$(shotname 01-start)"
    # What was set by hand is cleaned: the pin tooltip has no bidi override.
    DISPLAY=$xdisplay xdotool mousemove 1076 534; sleep 1.2; shot "$(shotname 02-pin-tooltip-cleaned)"
    click 703 740 3; click 786 792; shot "$(shotname 03-editor-tile)"
    typeit "Web"; shot "$(shotname 04-typed)"
    key Return; shot "$(shotname 05-tile-renamed)"
    has "org.example.browser.desktop=Web"
    # The rewrite drops the bad lines and keeps the cleaned ones.
    lacks evil; has "org.example.terminal.desktop=Console Window"; has "org.example.gone.desktop=Ghost"
    [ "$(awk -F= "/paint/{print length(\$2)}" "$names_file")" = 64 ] || fail "the long name was not cut to 64"

    log "pinned icon: the tooltip has the new name; rename a pin"
    DISPLAY=$xdisplay xdotool mousemove 922 534; sleep 1.2; shot "$(shotname 06-pin-tooltip)"
    click 998 534 3; click 1081 682; shot "$(shotname 07-editor-pin)"
    typeit "Notes"; key Return
    has "org.example.editor.desktop=Notes"
    DISPLAY=$xdisplay xdotool mousemove 998 534; sleep 1.2; shot "$(shotname 08-pin-renamed)"

    log "recent chip"
    click 690 601 3; click 773 653; shot "$(shotname 09-editor-chip)"
    typeit "Music"; key Return; shot "$(shotname 10-chip-renamed)"
    has "org.example.music.desktop=Music"

    log "search: the new name, the old name, a name set before the start"
    call Show start "web" "{}"; sleep 1; shot "$(shotname 11-search-new-name)"
    call Show start "firefox" "{}"; sleep 1; shot "$(shotname 12-search-old-name)"
    call Show start "calculator" "{}"; sleep 1; shot "$(shotname 13-search-old-name-preset)"

    log "search: rename the best match, and a row in the list, in place"
    call Show start "tele" "{}"; sleep 1
    click 900 560 3; click 983 617; shot "$(shotname 14-editor-card)"
    typeit "Chat"; key Return; shot "$(shotname 15-card-renamed)"
    has "org.example.chat.desktop=Chat"
    call Show start "k" "{}"; sleep 1
    click 900 616 3; click 983 668; shot "$(shotname 16-editor-row)"
    typeit "Writer"; key Return; shot "$(shotname 17-row-renamed)"
    has "org.example.editor.desktop=Writer"

    log "Reset Name"
    call Show start "" "{}"; sleep 1
    click 805 740 3; shot "$(shotname 18-menu-with-reset)"
    click 888 824; shot "$(shotname 19-after-reset)"
    lacks "org.example.browser.desktop"

    log "Escape, a click elsewhere, an empty name and a blank one change nothing, or restore"
    click 703 876 3; click 786 928; typeit "ZZZ"; key Escape; shot "$(shotname 20-after-escape)"
    has "org.example.music.desktop=Music"
    click 703 876 3; click 786 928; shot "$(shotname 21-editor-empty-soon)"
    key ctrl+a BackSpace; shot "$(shotname 22-editor-empty)"; key Return
    lacks "org.example.music.desktop"
    click 998 534 3; click 1081 682; key ctrl+a; typeit "   "; key Return
    lacks "org.example.editor.desktop"
    click 703 740 3; click 786 792; typeit "Zzz"; click 1150 700 1
    has "org.example.chat.desktop=Chat"
    shot "$(shotname 23-after-click-away)"

    log "a long name is cut at 64 characters"
    click 703 740 3; click 786 792; key ctrl+a
    typeit "$(printf "L%.0s" $(seq 100))"; key Return
    [ "$(awk -F= "/chat/{print length(\$2)}" "$names_file")" = 64 ] || fail "the typed name was not cut to 64"

    log "A–Z, sorted by the names shown"
    click 1215 644; sleep 1; shot "$(shotname 24-a-z)"
    click 1108 644; sleep 1

    log "the names survive a restart; nothing the user named reaches the log"
    kill "$app"; wait "$app" 2>/dev/null
    "$bin" --daemon >"$out/app2.log" 2>&1 &
    app=$!
    for _ in $(seq 100); do
        timeout 5 gdbus call --session -d org.freedesktop.DBus -o /org/freedesktop/DBus \
            -m org.freedesktop.DBus.NameHasOwner "$name" 2>/dev/null | grep -q true && break
        sleep 0.05
    done
    sleep 3
    call Show start "" "{}"; sleep 1; shot "$(shotname 25-after-restart)"
    if grep -hE "Quick Maths|Console|Writer|Notes|Music|Ghost|LLLL|PPPP|Chat|Web|evil" "$out/app.log" "$out/app2.log"; then
        fail "a name reached the log"
    fi

    log "failures $failures"
    echo "failures $failures" >>"$out/metrics.txt"
    kill "$app" 2>/dev/null
    exit $failures
' _ "$out" "$bin" "$scheme" "$names"
