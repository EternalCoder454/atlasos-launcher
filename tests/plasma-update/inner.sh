#!/bin/bash
# Runs inside the dev container, never on the desktop (tests/plasma-update/run.sh).
# A private dbus session, a virtual KWin, and plasmashell with a seeded panel
# holding the OLD launcher button (net.eterneon.atlas.launcher.button, whose
# plasmoid is not installed) between two other widgets, with the staged
# package (cmake --install into $STAGE: the new plasmoid and the update
# script) last on XDG_DATA_DIRS. Plasma runs the update script at start; the
# result goes to $OUT (probe.txt, appletsrc.before/after, plasmashellrc).
# shellcheck disable=SC2034,SC2016
set -uo pipefail
export STAGE OUT HERE
out=$OUT; root=/tmp/pt; rm -rf $root; mkdir -p $root/{home,config,data,cache,state,runtime}; chmod 700 $root/runtime
export HOME=$root/home XDG_CONFIG_HOME=$root/config XDG_DATA_HOME=$root/data XDG_CACHE_HOME=$root/cache XDG_STATE_HOME=$root/state XDG_RUNTIME_DIR=$root/runtime
export XDG_DATA_DIRS=/usr/local/share:/usr/share:$STAGE/share
exec dbus-run-session -- bash -c '
set -uo pipefail
out=$OUT
cat > $XDG_CONFIG_HOME/plasma-org.kde.plasma.desktop-appletsrc <<RC
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
plugin=org.kde.plasma.digitalclock

[Containments][1][Applets][3]
immutability=1
plugin=net.eterneon.atlas.launcher.button

[Containments][1][Applets][3][Configuration]
PreloadWeight=42

[Containments][1][Applets][3][Configuration][General]
ImportPins=
KeepMe=yes

[Containments][1][Applets][3][Configuration][Custom]
Foo=bar

[Containments][1][Applets][3][Configuration][Custom][Deep]
Baz=1

[Containments][1][Applets][4]
immutability=1
plugin=org.kde.plasma.showdesktop

[Containments][1][General]
AppletOrder=2;3;4

[Containments][5]
activityId=
formfactor=0
immutability=1
lastScreen=0
location=0
plugin=org.kde.plasma.folder
wallpaperplugin=org.kde.image
RC
cp $XDG_CONFIG_HOME/plasma-org.kde.plasma.desktop-appletsrc $out/appletsrc.before
kwin_bin=$XDG_RUNTIME_DIR/kwin_wayland; cp /usr/bin/kwin_wayland $kwin_bin
$kwin_bin --virtual --width 1280 --height 800 --socket wl-test --no-lockscreen >$out/kwin.log 2>&1 &
kwin=$!
for _ in $(seq 100); do [ -S $XDG_RUNTIME_DIR/wl-test ] && break; sleep 0.1; done
export WAYLAND_DISPLAY=wl-test QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software KWIN_COMPOSE=Q
dbus-update-activation-environment WAYLAND_DISPLAY QT_QPA_PLATFORM QT_QUICK_BACKEND KWIN_COMPOSE XDG_DATA_DIRS HOME XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_STATE_HOME XDG_RUNTIME_DIR 2>/dev/null
QT_FORCE_STDERR_LOGGING=1 QT_LOGGING_RULES="*.debug=true;qt.*=false;kf.*=false;kde.*=true;plasma*=true;org.kde.plasma*=true" plasmashell --no-respawn >$out/plasmashell.log 2>&1 &
ps=$!
Q=x
for t in 8 12 20; do sleep $t; echo "== t+$t"; timeout 15 python3 $HERE/probe.py 2>&1 | tee -a $out/probe.txt; done
echo "== plasmashellrc now:"; cat $XDG_CONFIG_HOME/plasmashellrc 2>&1 | tee -a $out/probe.txt
echo "== launcher buttons left, with their configuration" | tee -a $out/probe.txt
timeout 15 python3 $HERE/probe.py $HERE/dump.js 2>&1 | tee -a $out/probe.txt
sleep ${WAIT:-5}
kill -0 $ps 2>/dev/null && echo "plasmashell alive" || echo "plasmashell DIED"
timeout 15 gdbus call --session -d org.kde.plasmashell -o /PlasmaShell -m org.kde.PlasmaShell.dumpCurrentLayoutJson > $out/layout.json 2>&1
python3 -c "import dbus;dbus.SessionBus().get_object('org.kde.plasmashell','/MainApplication').quit(dbus_interface='org.qtproject.Qt.QCoreApplication')" 2>&1; for _ in $(seq 40); do kill -0 $ps 2>/dev/null || break; sleep 0.25; done; kill -TERM $ps 2>/dev/null; for _ in $(seq 50); do kill -0 $ps 2>/dev/null || break; sleep 0.2; done
kill $kwin 2>/dev/null
cp $XDG_CONFIG_HOME/plasma-org.kde.plasma.desktop-appletsrc $out/appletsrc.after 2>/dev/null
ls -la $XDG_CONFIG_HOME > $out/confdir.txt; cp $XDG_CONFIG_HOME/plasmashellrc $out/plasmashellrc 2>/dev/null
true
'
