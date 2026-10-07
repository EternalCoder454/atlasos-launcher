#!/bin/bash
# A headless run of the launcher, inside the dev container, never on the
# user's desktop: a private session bus, a private KWin (layer shell and
# focus need it) on its X11 backend inside a private Xvfb, which is what the
# screenshots capture, and a scratch home with every
# XDG dir under /work/xdg/<run>. It opens Start and Search through D-Bus,
# screenshots each state, and records timings, memory and idle CPU.
#   scripts/dev.sh bash scripts/headless-run.sh <run name> [binary] [trace]
# With "trace", the app runs under strace (process and signal calls only,
# into strace.log) to find out how it ended; timings are then not real.
# The binary defaults to /work/cmake/dev/telamon-launcher (the app build of
# CLAUDE.md). Output: /work/runs/<run>/ (shots, logs, metrics.txt).
set -euo pipefail

run=${1:?usage: headless-run.sh <run name> [binary]}
bin=${2:-/work/cmake/dev/telamon-launcher}
trace=${3:-}
case $trace in '' | trace) ;; *) echo "the third argument is trace or nothing" >&2; exit 2 ;; esac
# No leading dot: "." or ".." would make the rm below wipe /work.
case $run in .* | *[!A-Za-z0-9._-]* | '') echo "bad run name" >&2; exit 2 ;; esac
[ -x "$bin" ] || { echo "no launcher binary at $bin" >&2; exit 2; }

out=/work/runs/$run
root=/work/xdg/$run
rm -rf "$out" "$root"
mkdir -p "$out" "$root"/{home,config,data,cache,state} "$root/runtime"
chmod 700 "$root/runtime"
export HOME=$root/home XDG_CONFIG_HOME=$root/config XDG_DATA_HOME=$root/data \
    XDG_CACHE_HOME=$root/cache XDG_STATE_HOME=$root/state XDG_RUNTIME_DIR=$root/runtime

# A few files in the scratch home, opened "recently".
mkdir -p "$HOME/Documents"
for f in Budget.ods Notes.txt Trip.pdf; do : >"$HOME/Documents/$f"; done
now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
{
    echo '<?xml version="1.0" encoding="UTF-8"?>'
    echo '<xbel version="1.0" xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks" xmlns:mime="http://www.freedesktop.org/standards/shared-mime-info">'
    for f in Budget.ods Notes.txt Trip.pdf; do
        echo "<bookmark href=\"file://$HOME/Documents/$f\" added=\"$now\" modified=\"$now\" visited=\"$now\"><info><metadata owner=\"http://freedesktop.org\"><mime:mime-type type=\"application/octet-stream\"/><bookmark:applications><bookmark:application name=\"test\" exec=\"test\" modified=\"$now\" count=\"1\"/></bookmark:applications></metadata></info></bookmark>"
    done
    echo '</xbel>'
} >"$XDG_DATA_HOME/recently-used.xbel"

# What 0.2.x left (the names before the rename): a config folder with pins and
# options, a history, and the runners' state. The first run moves them.
mkdir -p "$XDG_CONFIG_HOME/atlas-launcher" "$XDG_STATE_HOME/atlas-launcher"
printf 'preferred://browser\nnet.eterneon.atlas.store.desktop\norg.kde.systemsettings.desktop\n' >"$XDG_CONFIG_HOME/atlas-launcher/pinned.list"
printf '[Search]\nWebSearch=false\n' >"$XDG_CONFIG_HOME/atlas-launcher/launcher.conf"
printf 'set\tapp:net.eterneon.atlas.settings.desktop\t5\t%s\n' "$(date +%s)" >"$XDG_STATE_HOME/atlas-launcher/usage.tsv"
printf '[General]\nx=1\n' >"$XDG_STATE_HOME/atlas-launcherstaterc"

# The inner part runs on a private bus that only this container sees.
# shellcheck disable=SC2016
exec dbus-run-session -- bash -c '
    set -uo pipefail
    out=$1 bin=$2 trace=$3 app= kwin= xvfb= failures=0
    log() { echo "$(date +%T.%3N) $*" >>"$out/steps.log"; }
    metric() { echo "$*" >>"$out/metrics.txt"; }
    # KWin --virtual composites with QPainter here (no render node), and
    # its ScreenShot2 cancels every capture; KWin windowed in Xvfb shows
    # the same scene in a window that `import` can read. Neither has GL, so
    # blur is not drawn headless.
    Xvfb -displayfd 3 -screen 0 1920x1080x24 -nolisten tcp 3>"$XDG_RUNTIME_DIR/display" >"$out/xvfb.log" 2>&1 &
    xvfb=$!
    # Everything this run started dies with it, on any exit. In trace mode
    # $app is strace, so its child (the launcher) goes first.
    cleanup() {
        [ -n "$app" ] && pkill -9 -P "$app" 2>/dev/null
        for p in $app $kwin $xvfb; do kill -9 "$p" 2>/dev/null; done
        wait 2>/dev/null
    }
    trap cleanup EXIT
    trap "exit 143" TERM INT HUP
    for _ in $(seq 50); do [ -s "$XDG_RUNTIME_DIR/display" ] && break; sleep 0.1; done
    [ -s "$XDG_RUNTIME_DIR/display" ] || { echo "Xvfb did not start" >&2; exit 1; }
    xdisplay=:$(head -1 "$XDG_RUNTIME_DIR/display")
    # kwin_wayland carries file capabilities (CAP_SYS_NICE), which a
    # rootless container refuses to exec; a plain copy has none. The copy
    # keeps its name, which the KWin Qt platform plugin checks.
    kwin_bin=$XDG_RUNTIME_DIR/kwin_wayland
    cp /usr/bin/kwin_wayland "$kwin_bin"
    DISPLAY=$xdisplay "$kwin_bin" --x11-display "$xdisplay" --no-lockscreen \
        --width 1920 --height 1080 --socket wl-test >"$out/kwin.log" 2>&1 &
    kwin=$!
    for _ in $(seq 100); do [ -S "$XDG_RUNTIME_DIR/wl-test" ] && break; sleep 0.1; done
    [ -S "$XDG_RUNTIME_DIR/wl-test" ] || { echo "kwin did not start" >&2; exit 1; }
    log "kwin up"

    export WAYLAND_DISPLAY=wl-test QT_QPA_PLATFORM=wayland
    export QT_LOGGING_RULES="telamon.launcher*.debug=true"
    export QT_FORCE_STDERR_LOGGING=1 RUST_BACKTRACE=1
    export QT_MESSAGE_PATTERN="%{time hh:mm:ss.zzz} %{category} %{type}: %{message}"
    t0=$(date +%s%N)
    if [ "$trace" = trace ]; then
        strace -f -tt -e trace=process,signal -o "$out/strace.log" "$bin" --daemon >"$out/app.log" 2>&1 &
    else
        "$bin" --daemon >"$out/app.log" 2>&1 &
    fi
    app=$!
    name=net.eterneon.telamon.launcher
    path=/net/eterneon/telamon/launcher
    iface=net.eterneon.telamon.Launcher1
    up=0
    for _ in $(seq 100); do
        if timeout 5 gdbus call --session -d org.freedesktop.DBus -o /org/freedesktop/DBus \
            -m org.freedesktop.DBus.NameHasOwner "$name" 2>/dev/null | grep -q true; then
            up=1; break
        fi
        kill -0 $app 2>/dev/null || break
        sleep 0.05
    done
    if [ $up != 1 ]; then
        echo "the launcher did not take its bus name; see app.log" >&2
        exit 1
    fi
    metric "startup_to_bus_ms $(( ($(date +%s%N) - t0) / 1000000 ))"
    # The files of the old name moved, once, and were read from the new place.
    mig=0
    [ -f "$XDG_CONFIG_HOME/telamon-launcher/pinned.list" ] && [ -f "$XDG_CONFIG_HOME/telamon-launcher/launcher.conf" ] \
        && [ -f "$XDG_STATE_HOME/telamon-launcher/usage.tsv" ] && [ -f "$XDG_STATE_HOME/telamon-launcherstaterc" ] \
        && [ ! -e "$XDG_CONFIG_HOME/atlas-launcher" ] && [ ! -e "$XDG_STATE_HOME/atlas-launcher" ] \
        && [ ! -e "$XDG_STATE_HOME/atlas-launcherstaterc" ] && mig=1
    metric "migration_moved_all $mig"
    [ $mig = 1 ] || { log "FAILED: the old files did not move"; failures=$((failures + 1)); }
    # The name before the rename answers too.
    # (it is taken a moment after the new one, once the panel exists)
    for n in net.eterneon.telamon.launcher net.eterneon.atlas.launcher; do
        owned=0
        for _ in $(seq 50); do
            v=$(timeout 5 gdbus call --session -d org.freedesktop.DBus -o /org/freedesktop/DBus -m org.freedesktop.DBus.NameHasOwner $n 2>&1 || true)
            case $v in *true*) owned=1; break;; esac
            sleep 0.1
        done
        if [ $owned = 1 ]; then metric "bus_name_owned $n"; else log "FAILED: $n not owned"; failures=$((failures + 1)); fi
    done
    sleep 3   # the catalogue, recent files and runner prewarm settle
    # The app runs in this container, so its PID is read in this /proc. A
    # dead app fails the run at the step that found it, with its status.
    check() {
        kill -0 "$app" 2>/dev/null && return 0
        wait "$app"; local st=$?
        log "app DIED (status $st) before: $1"
        metric "app_died_before $1 status $st"
        exit 1
    }
    # check() runs in this shell, not in $( ): exit and wait need it here.
    rss() { awk "/^Rss:/ {print \$2}" "/proc/$app/smaps_rollup"; }
    cpu() { local st; read -r -a st <"/proc/$app/stat"; echo $(( st[13] + st[14] )); }
    check rss_hidden_after_start; metric "rss_hidden_after_start_kb $(rss)"

    # Every call and wait has its own timeout: a hung app or KWin must not
    # hold the machine lock.
    fail() { log "FAILED: $*"; failures=$((failures + 1)); }
    call() { check "$1"; timeout 10 gdbus call --session -d $name -o $path -m "$iface.$1" "${@:2}" >>"$out/steps.log" 2>&1 || fail "call $1"; }
    shot() {
        check "shot $1"
        DISPLAY=$xdisplay timeout 20 import -window root "$out/$1.png" 2>>"$out/steps.log" || fail "shot $1"
        # What the app thinks, beside what KWin drew.
        log "$1 visible: $(timeout 5 gdbus call --session -d $name -o $path -m org.freedesktop.DBus.Properties.Get "$iface" Visible 2>&1)"
    }

    log "show start"
    t=$(date +%s%N); call Show start "" "{}"
    metric "show_call_ms $(( ($(date +%s%N) - t) / 1000000 ))"
    sleep 1; shot 01-start
    check rss_start_shown; metric "rss_start_shown_kb $(rss)"

    log "show search query"
    call Show search "set" "{}"; sleep 1; shot 02-search-set
    call Show start "fire" "{}"; sleep 1; shot 03-start-query-fire
    call Show search "2+2*3" "{}"; sleep 1; shot 04-search-calc
    call Show search "10 km in mi" "{}"; sleep 1; shot 05-search-units
    call Show search "Notes" "{}"; sleep 1; shot 06-search-recent-file
    call Show search "zzzzqqq" "{}"; sleep 1; shot 07-search-no-results
    check rss_after_queries; metric "rss_after_queries_kb $(rss)"

    # The old name, path and interface: the dock button of 0.2.x and old scripts.
    legacy() { check "legacy $1"; timeout 10 gdbus call --session -d net.eterneon.atlas.launcher -o /net/eterneon/atlas/launcher -m "net.eterneon.atlas.Launcher1.$1" "${@:2}" >>"$out/steps.log" 2>&1 || fail "legacy call $1"; }
    log "legacy name: Show start, Visible, Hide"
    legacy Show start "" "{}"; sleep 1; shot 07b-start-via-old-name
    v=$(timeout 5 gdbus call --session -d net.eterneon.atlas.launcher -o /net/eterneon/atlas/launcher -m org.freedesktop.DBus.Properties.Get net.eterneon.atlas.Launcher1 Visible 2>&1)
    log "legacy Visible: $v"; metric "legacy_visible_when_shown $v"
    case $v in *true*) ;; *) fail "legacy Visible is not true while shown";; esac
    legacy Hide; sleep 1
    v=$(timeout 5 gdbus call --session -d net.eterneon.atlas.launcher -o /net/eterneon/atlas/launcher -m org.freedesktop.DBus.Properties.Get net.eterneon.atlas.Launcher1 Visible 2>&1)
    case $v in *false*) metric "legacy_visible_when_hidden $v";; *) fail "legacy Visible is not false when hidden: $v";; esac
    legacy ClearHistory
    [ ! -e "$XDG_STATE_HOME/telamon-launcher/usage.tsv" ] && metric "legacy_clear_history ok" || fail "legacy ClearHistory left usage.tsv"
    # A bad mode: the old interface answers exactly as the new one does, and
    # opens nothing.
    v=$(timeout 5 gdbus call --session -d net.eterneon.atlas.launcher -o /net/eterneon/atlas/launcher -m net.eterneon.atlas.Launcher1.Show bogus "" "{}" 2>&1 || true)
    w=$(timeout 5 gdbus call --session -d $name -o $path -m "$iface.Show" bogus "" "{}" 2>&1 || true)
    log "Show with a bad mode: old [$v] new [$w]"
    [ "$v" = "$w" ] && metric "legacy_bad_mode same_as_new" || fail "the old interface answers a bad mode differently: [$v] [$w]"
    shot 07c-after-bad-mode

    log "hide"
    call Hide; sleep 1; shot 08-hidden
    timeout 10 gdbus call --session -d $name -o $path -m org.freedesktop.DBus.Properties.Get "$iface" Visible >>"$out/steps.log" 2>&1
    # Idle: 10 s hidden, no timers but the one memory trim.
    check idle_cpu; c0=$(cpu); sleep 10; check idle_cpu_end; c1=$(cpu)
    metric "idle_cpu_ticks_10s $(( c1 - c0 ))"
    metric "rss_hidden_idle_kb $(rss)"

    # Reopen after hide: the second open is the one users feel most.
    t=$(date +%s%N); call Show start "" "{}"
    metric "reshow_call_ms $(( ($(date +%s%N) - t) / 1000000 ))"
    sleep 1; shot 09-reshow-start
    call Hide
    check end; log "app alive at end"
    # TERM must reach the launcher itself (in trace mode $app is strace).
    target=$app
    [ "$trace" = trace ] && target=$(pgrep -P "$app" | head -1)
    [ -n "$target" ] || { fail "no launcher under strace"; target=$app; }
    kill "$target"
    if timeout 10 tail --pid="$app" -f /dev/null; then
        wait "$app"; st=$?
        metric "exit_code $st"
        [ "$st" = 0 ] || fail "exit status $st after TERM"
    else
        fail "app ignored TERM for 10 s"; metric "exit_code killed"
    fi
    metric "failures $failures"
    [ "$failures" = 0 ]
' _ "$out" "$bin" "$trace"
