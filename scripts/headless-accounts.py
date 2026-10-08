#!/usr/bin/env python3
"""A stand-in for AccountsService on the private system bus of a headless run
(scripts/headless-look.sh; DBUS_SYSTEM_BUS_ADDRESS names that bus): one user, with a real name and, when a file is
given, a picture.
  headless-accounts.py <real name> [picture file]
"""
import sys

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

NAME = sys.argv[1]
PICTURE = sys.argv[2] if len(sys.argv) > 2 else ""
USER = "/org/freedesktop/Accounts/User1000"


class Accounts(dbus.service.Object):
    @dbus.service.method("org.freedesktop.Accounts", in_signature="x", out_signature="o")
    def FindUserById(self, uid):
        return dbus.ObjectPath(USER)


class User(dbus.service.Object):
    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        return {"RealName": NAME, "IconFile": PICTURE}


DBusGMainLoop(set_as_default=True)
bus = dbus.SystemBus()
name = dbus.service.BusName("org.freedesktop.Accounts", bus)
Accounts(bus, "/org/freedesktop/Accounts")
User(bus, USER)
GLib.MainLoop().run()
