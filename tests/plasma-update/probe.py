import dbus, os, sys
bus = dbus.SessionBus()
o = bus.get_object('org.kde.plasmashell', '/PlasmaShell')
print(o.evaluateScript(open(sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.environ['HERE'], 'probe.js')).read(), dbus_interface='org.kde.PlasmaShell'))
