#!/usr/bin/env python3
"""Probes the launcher's D-Bus surface on a private session bus
(docs/SECURITY.md, "The D-Bus surface"). Run it through
scripts/headless-dbus-security.sh, inside the dev container: it starts the
launcher itself (QT_QPA_PLATFORM=offscreen, every XDG dir in a scratch folder)
and never touches the user's bus.

  1. the interfaces export exactly the documented methods, none of which takes
     a path, a command or a result id;
  2. the bus names are held without allow-replacement: another connection
     cannot take them over, with or without queueing for them;
  3. a name squatter that owns the name first only ever sees the activation a
     second start forwards (the squatter case is reported, not asserted: a
     process of the same user can also kill the launcher);
  4. hostile arguments (wrong types, huge strings and arrays, control
     characters, huge lists) are refused or bounded, and the launcher is
     still alive and answering afterwards, with no child process started and
     its memory bounded;
  5. the calls org.freedesktop.Application and org.kde.KDBusService forward
     (Open with a file URI, ActivateAction, CommandLine) start nothing.

Exit status 0 when every assertion held.
"""

import os
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

import dbus
import dbus.mainloop.glib
from gi.repository import GLib

dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)

NAME = "net.eterneon.telamon.launcher"
PATH = "/net/eterneon/telamon/launcher"
IFACE = "net.eterneon.telamon.Launcher1"
OLD_NAME = "net.eterneon.atlas.launcher"
OLD_PATH = "/net/eterneon/atlas/launcher"
OLD_IFACE = "net.eterneon.atlas.Launcher1"

# What the interface may offer (docs/DESIGN.md, "Interfaces").
ALLOWED = {
    "ToggleStart",
    "ToggleSearch",
    "Show",
    "Hide",
    "SetDockAnchor",
    "ImportPins",
    "ClearHistory",
}

failures = 0


def check(ok, what):
    global failures
    print(("PASS " if ok else "FAIL ") + what, flush=True)
    if not ok:
        failures += 1


def note(what):
    print("NOTE " + what, flush=True)


def pump(seconds):
    ctx = GLib.MainContext.default()
    end = time.time() + seconds
    while time.time() < end:
        while ctx.iteration(False):
            pass
        time.sleep(0.01)


def private_bus():
    # A connection of its own; libdbus must not exit() the probe when the bus
    # drops it, the probe says so instead.
    c = dbus.SessionBus(private=True)
    c.set_exit_on_disconnect(False)
    return c


def start_launcher(env_extra=None):
    env = dict(os.environ, QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software")
    env["QT_LOGGING_RULES"] = "telamon.launcher*.debug=true"
    env["QT_FORCE_STDERR_LOGGING"] = "1"
    env.update(env_extra or {})
    log = open(os.path.join(os.environ["OUT"], "launcher.log"), "ab")
    return subprocess.Popen([BIN, "--daemon"], env=env, stdout=log, stderr=log)


def wait_for_name(bus, name, seconds=30):
    end = time.time() + seconds
    while time.time() < end:
        if bus.name_has_owner(name):
            return True
        time.sleep(0.1)
    return False


def rss_kib(pid):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1])
    except OSError:
        pass
    return -1


def children(pid):
    out = subprocess.run(
        ["pgrep", "-P", str(pid)], capture_output=True, text=True
    ).stdout.split()
    return out


def iface_of(bus, name, path, iface):
    return dbus.Interface(bus.get_object(name, path), iface)


def introspect(bus, name, path):
    xml = bus.get_object(name, path).Introspect(
        dbus_interface="org.freedesktop.DBus.Introspectable"
    )
    return ET.fromstring(str(xml))


CANARY = "CANARY-7f3a9c-do-not-log"


def seed_user_data():
    """User data that must never reach a log: an app with a canary name and
    comment, a recent file and a pin with a canary in their names, and a name
    the user gave an app."""
    data = os.environ["XDG_DATA_HOME"]
    home = os.environ["HOME"]
    os.makedirs(os.path.join(data, "applications"), exist_ok=True)
    with open(os.path.join(data, "applications", "canary-app.desktop"), "w") as f:
        f.write(
            "[Desktop Entry]\nType=Application\n"
            f"Name={CANARY}-app\nGenericName={CANARY}-generic\nComment={CANARY}-comment\n"
            f"Keywords={CANARY}-keyword;\nExec=true\n"
        )
    os.makedirs(os.path.join(home, "Documents"), exist_ok=True)
    doc = os.path.join(home, "Documents", f"{CANARY}-file.txt")
    open(doc, "w").close()
    xbel = os.path.join(data, "recently-used.xbel")
    with open(xbel, "w") as f:
        f.write(
            '<?xml version="1.0"?><xbel version="1.0"><bookmark href="file://%s" '
            'added="2026-01-01T00:00:00Z" modified="2026-01-01T00:00:00Z" visited="2026-01-01T00:00:00Z"/></xbel>' % doc
        )
    # Apps whose icons are things an image loader must never be given: a pipe
    # (its open blocks until someone writes, for good), a link to a device
    # that never ends, a huge file. Each has a desktop file in the folder
    # a sandboxed app can write to.
    # And desktop files that are not files: KDE's database builder must skip
    # them and the launcher stay up.
    apps = os.path.join(data, "applications")
    os.mkfifo(os.path.join(apps, "evil-pipe.desktop"))
    os.symlink("/dev/zero", os.path.join(apps, "evil-zero.desktop"))
    with open(os.path.join(apps, "evil-huge.desktop"), "wb") as f:
        f.truncate(2 * 1024 * 1024 * 1024)
    icons = os.path.join(home, "icons")
    os.makedirs(icons, exist_ok=True)
    os.mkfifo(os.path.join(icons, "pipe.png"))
    os.symlink("/dev/zero", os.path.join(icons, "zero.svg"))
    with open(os.path.join(icons, "big.svg"), "wb") as f:
        f.truncate(900 * 1024 * 1024)
    # An icon that is a plain file now and is swapped for a pipe once the
    # launcher has built its catalogue (the check is then already past).
    with open(os.path.join(icons, "swap.svg"), "w") as f:
        f.write('<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="red"/></svg>')
    for n, icon in enumerate(["pipe.png", "zero.svg", "big.svg", "swap.svg"], 1):
        with open(os.path.join(data, "applications", f"icontest{n}.desktop"), "w") as f:
            f.write(
                f"[Desktop Entry]\nType=Application\nName=Icon test {n}\nExec=true\n"
                f"Icon={os.path.join(icons, icon)}\n"
            )
    cfg = os.path.join(os.environ["XDG_CONFIG_HOME"], "telamon-launcher")
    os.makedirs(cfg, exist_ok=True)
    with open(os.path.join(cfg, "names.conf"), "w") as f:
        f.write(f"[Names]\ncanary-app.desktop={CANARY}-renamed\n")
    # No pinned.list: ImportPins is accepted only while there is none, and the
    # probe checks that it reaches the backend. A history to clear, too.
    state = os.path.join(os.environ["XDG_STATE_HOME"], "telamon-launcher")
    os.makedirs(state, exist_ok=True)
    with open(os.path.join(state, "usage.tsv"), "w") as f:
        f.write("ca\tapp:canary-app.desktop\t3\t1790000000\n")
    # launcher.conf, krunnerrc and kdeglobals are pipes: KConfig does not wait
    # for those (the start must not hang; checked by the launcher answering).
    os.mkfifo(os.path.join(cfg, "launcher.conf"))
    for rc in ("krunnerrc", "kdeglobals"):
        os.mkfifo(os.path.join(os.environ["XDG_CONFIG_HOME"], rc))
    # state.conf (the Start page's view, Qt's Settings) too, and KRunner's
    # state file: both are opened as they are on the GUI thread, and a pipe
    # there made the launcher wait in open() for good, before the fix.
    os.mkfifo(os.path.join(cfg, "state.conf"))
    os.makedirs(os.environ["XDG_STATE_HOME"], exist_ok=True)
    os.mkfifo(os.path.join(os.environ["XDG_STATE_HOME"], "telamon-launcherstaterc"))
    subprocess.run(["kbuildsycoca6", "--noincremental"], capture_output=True, timeout=120)


def main():
    global BIN
    BIN = sys.argv[1]
    bus = private_bus()

    # ---- the squatter that owns the name before the launcher starts ----
    squat = private_bus()
    seen = []

    def spy(_bus, message):
        if message.get_type() == 1:  # a method call
            seen.append((message.get_interface(), message.get_member()))

    squat.add_message_filter(spy)
    got = squat.request_name(NAME, 0)
    check(got == 1, "a squatter can take the name before the launcher starts (the premise)")
    p = start_launcher()
    pump(4)
    code = p.poll()
    note(
        f"squatter owns {NAME}: the launcher process {'exited ' + str(code) if code is not None else 'is still running'};"
        f" the squatter saw {sorted(set(seen))}"
    )
    # Whatever it did, it must not have forwarded anything but an activation
    # (show / hide): the calls it can make are the interface's own.
    forwarded = {m for i, m in seen if i not in (None,)}
    check(
        forwarded <= {"Activate", "CommandLine", "ActivateAction", "Open", "Ping", "GetAll", "Get"},
        "a squatter is sent nothing but an activation request",
    )
    if code is None:
        p.terminate()
        p.wait(10)
    squat.close()
    time.sleep(0.5)

    # ---- the real run ----
    seed_user_data()
    # The launcher's own categories at debug level (what a bug report asks
    # for), the libraries' at their defaults. With every category at debug
    # level (QT_LOGGING_RULES="*.debug=true") KRunner and KIO's URI filters
    # log the text of a query themselves; that is theirs, and off by default.
    p = start_launcher()
    check(wait_for_name(bus, NAME), "the launcher takes its bus name")
    check(wait_for_name(bus, OLD_NAME, 5), "the launcher takes the old bus name too")
    pid = p.pid

    # 1. the exported surface
    for name, path, iface in ((NAME, PATH, IFACE), (OLD_NAME, OLD_PATH, OLD_IFACE)):
        root = introspect(bus, name, path)
        ours = [i for i in root.findall("interface") if i.get("name") == iface]
        check(len(ours) == 1, f"{iface} is exported at {path}")
        methods = {m.get("name") for m in ours[0].findall("method")}
        check(methods == ALLOWED, f"{iface} has exactly the documented methods (got {sorted(methods)})")
        for m in ours[0].findall("method"):
            sigs = "".join(a.get("type") for a in m.findall("arg") if a.get("direction", "in") == "in")
            # Strings, string lists and a{sv} only; no object path or file descriptor.
            check(
                set(sigs) <= set("sias{v}(")|set(")"),
                f"{iface}.{m.get('name')} takes only strings, ints and dictionaries ({sigs or 'nothing'})",
            )
        check(not ours[0].findall("signal"), f"{iface} has no signals of its own")

    # 2. the names cannot be taken over
    thief = private_bus()
    for name in (NAME, OLD_NAME):
        r = thief.request_name(name, 2 | 4)  # REPLACE_EXISTING | DO_NOT_QUEUE
        check(r == 3, f"{name} cannot be replaced (request answered {r}, 3 = exists)")
    r = thief.request_name(NAME, 0)  # queue behind the owner
    note(
        f"a connection that queues for {NAME} is answered {r} (2 = queued): the session bus lets anyone queue;"
        " it only takes the name if the launcher exits (docs/SECURITY.md)"
    )
    thief.close()

    # 3. hostile arguments
    launcher = iface_of(bus, NAME, PATH, IFACE)
    old = iface_of(bus, OLD_NAME, OLD_PATH, OLD_IFACE)
    base_rss = rss_kib(pid)

    def visible():
        return bool(bus.get_object(NAME, PATH).Get(IFACE, "Visible", dbus_interface="org.freedesktop.DBus.Properties"))

    def refused(call, what):
        try:
            call()
        except dbus.exceptions.DBusException as e:
            check(True, f"{what}: refused ({e.get_dbus_name().rsplit('.', 1)[-1]})")
            return
        check(False, f"{what}: was not refused")

    def accepted(call, what):
        try:
            call()
        except dbus.exceptions.DBusException as e:
            check(False, f"{what}: refused unexpectedly ({e.get_dbus_name()})")
            return
        check(True, f"{what}: answered")

    # The standard PropertiesChanged for Visible is the only signal the
    # launcher may send: not one that repeats what a caller handed over (an
    # adaptor's signals are exported on the bus). A small call first, with a
    # listener; the big ones below would only disconnect a listener.
    signals = []
    receivers = []
    for n in (NAME, OLD_NAME):
        receivers.append(
            bus.add_signal_receiver(
                lambda *a, **k: signals.append(k.get("member")),
                bus_name=n,
                member_keyword="member",
            )
        )
    # The catalogue is built by now and swap.svg passed its check as a plain
    # file: swap a pipe over it, as a hostile app could.
    icons = os.path.join(os.environ["HOME"], "icons")
    os.mkfifo(os.path.join(icons, "swap.fifo"))
    pump(3.0)
    os.rename(os.path.join(icons, "swap.fifo"), os.path.join(icons, "swap.svg"))
    # The engine starts within a second of the name (logind and the seat answer,
    # or a timer): ImportPins and ClearHistory before that have nothing to reach.
    pump(3.0)
    launcher.ImportPins(["canary-signal.desktop"])
    launcher.ClearHistory(timeout=20)
    pump(1.0)
    pins_file = os.path.join(os.environ["XDG_CONFIG_HOME"], "telamon-launcher", "pinned.list")
    usage_file = os.path.join(os.environ["XDG_STATE_HOME"], "telamon-launcher", "usage.tsv")
    for _ in range(50):
        if os.path.exists(pins_file) and not os.path.exists(usage_file):
            break
        time.sleep(0.1)
    check(
        os.path.exists(pins_file) and "canary-signal.desktop" in open(pins_file).read(),
        "ImportPins reaches the backend and writes pinned.list (the direct call replaced the signal)",
    )
    check(not os.path.exists(usage_file), "ClearHistory reaches the backend and deletes usage.tsv")
    own = [m for m in signals if m not in (None, "PropertiesChanged")]
    check(not own, f"the launcher broadcasts no signal of its own (saw {sorted(set(own))})")
    for r in receivers:
        r.remove()

    for target, label in ((launcher, "new"), (old, "old")):
        # The adaptor's QDBusContext is inert (Qt sets the context on an
        # adaptor's parent, which is KDBusService), so a bad mode is not
        # answered with an error: it is ignored. What matters is that it does
        # nothing.
        target.Show("run", "x", {})
        check(visible() is False, f"[{label}] Show with a mode that is neither start nor search does nothing")
        name, path, iface = (NAME, PATH, IFACE) if label == "new" else (OLD_NAME, OLD_PATH, OLD_IFACE)

        def raw(method, sig, *args):
            return lambda: bus.call_blocking(name, path, iface, method, sig, args, timeout=20)

        refused(raw("Show", "ss", "start", "x"), f"[{label}] Show with a missing argument")
        refused(raw("Show", "iii", 1, 2, 3), f"[{label}] Show with wrong types")
        refused(raw("ToggleStart", "s", "not a dict"), f"[{label}] ToggleStart with a string")
        refused(raw("SetDockAnchor", "ai", [1, 2]), f"[{label}] SetDockAnchor with an array")
        refused(raw("ImportPins", "s", "one string"), f"[{label}] ImportPins with a string")
        refused(raw("Hide", "s", "x"), f"[{label}] Hide with an argument")
        accepted(lambda: target.Show("search", "", {}), f"[{label}] Show with an empty query")
        accepted(lambda: target.Hide(), f"[{label}] Hide")

    big = "A" * (8 * 1024 * 1024)
    accepted(lambda: launcher.Show("search", big, {}), "Show with an 8 MiB query")
    accepted(lambda: launcher.Show("search", "a\u202e\x01\x1b[31mb\u200bc\nd", {}), "Show with control and bidi characters")
    accepted(lambda: launcher.Show("start", "x", {"activation-token": "t" * 100000, "other": dbus.Array(range(1000), signature="i")}), "Show with odd platform data")
    accepted(lambda: launcher.Hide(), "Hide after the big Show")
    # Typed text, app names, file names and the user's own names must not be
    # logged: a query with a canary, then the Start page, both searched.
    accepted(lambda: launcher.Show("search", CANARY + "-query", {}), "Show with a canary query")
    pump(2.0)
    accepted(lambda: launcher.Show("search", "2+2 " + CANARY, {}), "Show with a canary calculation")
    pump(1.0)
    accepted(lambda: launcher.Show("search", "ls " + CANARY, {}), "Show with a canary command")
    pump(1.0)
    accepted(lambda: launcher.Show("start", "", {}), "Show the Start page")
    pump(4.0)
    # The Start page draws every app's icon: with a pipe as an icon the GUI
    # thread used to wait in open() for good and the launcher stopped
    # answering (the vetted icon is a generic one instead).
    try:
        bus.call_blocking(NAME, PATH, "org.freedesktop.DBus.Properties", "Get", "ss", (IFACE, "Visible"), timeout=8)
        check(True, "the launcher answers with a pipe, a device link, a huge file and a file swapped for a pipe as app icons on the Start page")
    except dbus.exceptions.DBusException as e:
        check(False, f"the launcher stopped answering with a pipe as an app icon ({e.get_dbus_name()})")
    threads_in_open = []
    for t in os.listdir(f"/proc/{pid}/task"):
        try:
            with open(f"/proc/{pid}/task/{t}/wchan") as f:
                w = f.read()
        except OSError:
            continue
        if "wait_for_partner" in w or "fifo_open" in w:
            threads_in_open.append(t)
    # The image loader thread may wait on a pipe swapped in late (that is what
    # the asynchronous Image is for); the GUI thread, the process's first, never.
    check(str(pid) not in threads_in_open, f"the GUI thread does not wait in open() on a pipe (threads waiting: {threads_in_open})")
    accepted(lambda: launcher.Hide(), "Hide after the canary queries")
    huge = dbus.Array([dbus.Int32(i) for i in range(2_000_000)], signature="i")
    accepted(
        lambda: launcher.SetDockAnchor({"screen": "eDP-1", "anchor": huge}),
        "SetDockAnchor with a 2,000,000-element anchor",
    )
    accepted(
        lambda: launcher.SetDockAnchor({"screen": "S" * 100000, "anchor": dbus.Struct((0, 0, 10, 10), signature="iiii")}),
        "SetDockAnchor with a 100,000-character screen name",
    )
    accepted(
        lambda: launcher.SetDockAnchor({"screen": "eDP-1", "anchor": dbus.Struct((-2**31, 2**31 - 1, 2**31 - 1, 0), signature="iiii")}),
        "SetDockAnchor with extreme coordinates",
    )
    accepted(
        lambda: launcher.ToggleStart({"screen": "eDP-1", "anchor": dbus.Struct((1, 2, 3, 4), signature="iiii")}),
        "ToggleStart with a well-formed anchor",
    )
    accepted(lambda: launcher.Hide(), "Hide")
    accepted(
        lambda: launcher.ImportPins(dbus.Array(["x" * 4096 + ".desktop"] * 2000, signature="s")),
        "ImportPins with 2,000 long ids",
    )
    accepted(
        lambda: launcher.ImportPins(["../../etc/passwd", "/bin/sh", "a b.desktop", "preferred://evil", "a\nb.desktop"]),
        "ImportPins with path-like ids",
    )
    accepted(lambda: launcher.ClearHistory(timeout=20), "ClearHistory")

    # 4. what the standard interfaces forward starts nothing
    app = iface_of(bus, NAME, PATH, "org.freedesktop.Application")
    accepted(lambda: app.Open(["file:///etc/passwd", "file:///bin/sh"], {}), "Application.Open with file URIs")
    try:
        app.ActivateAction("quit", [], {})
        check(True, "Application.ActivateAction answered")
    except dbus.exceptions.DBusException as e:
        check(True, f"Application.ActivateAction: {e.get_dbus_name().rsplit('.', 1)[-1]}")
    kde = iface_of(bus, NAME, PATH, "org.kde.KDBusService")
    accepted(lambda: kde.CommandLine(["telamon-launcher", "--search", "xterm"], "/tmp", {}), "KDBusService.CommandLine --search")
    accepted(lambda: kde.CommandLine(["telamon-launcher", "--bogus", "--hide", "--daemon"], "/tmp", {}), "KDBusService.CommandLine with unknown options")
    accepted(lambda: kde.CommandLine(["x"] * 100000, "/tmp", {}), "KDBusService.CommandLine with 100,000 arguments")
    accepted(lambda: kde.CommandLine(["telamon-launcher", "--search", "y" * 3_000_000], "/tmp", {}), "KDBusService.CommandLine --search with 3 MB")

    pump(1.5)
    check(p.poll() is None, "the launcher is still running after all of it")
    check(not children(pid), f"the launcher started no child process (children: {children(pid)})")
    try:
        vis = bus.get_object(NAME, PATH).Get(IFACE, "Visible", dbus_interface="org.freedesktop.DBus.Properties")
        check(vis in (True, False), "the Visible property still answers")
    except dbus.exceptions.DBusException as e:
        check(False, f"Visible: {e}")
    grown = rss_kib(pid) - base_rss
    check(grown < 400 * 1024, f"memory stayed bounded (grew {grown // 1024} MiB over the start)")

    p.terminate()
    try:
        p.wait(15)
    except subprocess.TimeoutExpired:
        p.kill()
    with open(os.path.join(os.environ["OUT"], "launcher.log"), "rb") as f:
        log = f.read()
    check(len(log) > 1000, f"the log is there to be read ({len(log)} bytes, the launcher's debug categories on)")
    leaked = [line for line in log.splitlines() if b"CANARY" in line]
    check(
        not leaked,
        "no query, app name, file name or user-given name reached the log"
        + (f" (found in {len(leaked)} lines, first: {leaked[0][:160]!r})" if leaked else ""),
    )
    print(f"{failures} failure(s)")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
