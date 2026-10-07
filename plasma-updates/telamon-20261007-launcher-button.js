/*
    SPDX-FileCopyrightText: 2026 Telamon OS
    SPDX-License-Identifier: Apache-2.0

    Atlas Launcher became Telamon Launcher, and its dock button with it:
    net.eterneon.atlas.launcher.button is net.eterneon.telamon.launcher.button.
    In every panel (and desktop) each button of the old ID is replaced by one
    of the new ID at the same spot in the panel, with the old one's whole
    configuration (every group and key) and its global shortcut. A panel that
    already has the new button only loses the old one. A panel without the
    old button is left alone, and a second run finds nothing to change.

    Plasma runs each script of this folder once per user (plasmashellrc,
    [Updates] performed=); never rename this file. Install it as
    /usr/share/plasma/shells/org.kde.plasma.desktop/contents/updates/telamon-20261007-launcher-button.js
    (the telamon-launcher package does).

    The position goes through the panel's AppletOrder, as the scripts of
    the image do (atlasos-20261003-menu.js, atlasos-20261005-launcher.js):
    moving a widget with widget.index breaks the panel while Plasma starts.
*/

var OLD = "net.eterneon.atlas.launcher.button";
var NEW = "net.eterneon.telamon.launcher.button";

// Copies every key of the group `path` of `from`, and of the groups below it
// (to a depth of four), to `to`.
function copyGroup(from, to, path, depth) {
    from.currentConfigGroup = path;
    var keys = from.configKeys;
    var values = [];
    for (var i = 0; i < keys.length; i++) {
        values.push([keys[i], from.readConfig(keys[i], "")]);
    }
    var groups = depth > 0 ? from.configGroups : [];
    to.currentConfigGroup = path;
    for (var j = 0; j < values.length; j++) {
        to.writeConfig(values[j][0], values[j][1]);
    }
    for (var g = 0; g < groups.length; g++) {
        copyGroup(from, to, path.concat([groups[g]]), depth - 1);
    }
}

function swapIn(container) {
    var found = container.widgets(OLD);
    if (found.length === 0) {
        return;
    }
    var panel = container;
    panel.currentConfigGroup = ["General"];
    var order = String(panel.readConfig("AppletOrder", ""));
    var ids = order ? order.split(";") : [];
    if (ids.length === 0 && panel.widgetIds) {
        ids = panel.widgetIds.map(String);
    }
    // A panel that has the new button already only loses the old ones.
    var existing = container.widgets(NEW);
    found.forEach(function (old) {
        var oldId = String(old.id);
        var shortcut = "";
        try {
            shortcut = String(old.globalShortcut || "");
        } catch (e) {
            shortcut = "";
        }
        var created = null;
        if (existing.length === 0) {
            created = container.addWidget(NEW);
            if (created) {
                copyGroup(old, created, [], 4);
                old.currentConfigGroup = [];
                created.currentConfigGroup = [];
                existing = [created];
            }
        }
        old.remove();
        var at = ids.indexOf(oldId);
        if (created) {
            var newId = String(created.id);
            if (at >= 0) {
                ids[at] = newId;
            } else {
                ids.unshift(newId);
            }
            if (shortcut !== "") {
                try {
                    created.globalShortcut = shortcut;
                } catch (e2) {
                    // The shortcut is the user's to set again.
                }
            }
        } else if (at >= 0) {
            ids.splice(at, 1);
        }
    });
    panel.currentConfigGroup = ["General"];
    panel.writeConfig("AppletOrder", ids.join(";"));
}

panels().forEach(swapIn);
try {
  desktops().forEach(function (desktop) {
    // A button put on the desktop itself: swapped in place (no applet order).
    desktop.widgets(OLD).forEach(function (old) {
        if (desktop.widgets(NEW).length > 0) {
            old.remove();
            return;
        }
        var created = desktop.addWidget(NEW, old.geometry.x, old.geometry.y, old.geometry.width, old.geometry.height);
        if (created) {
            copyGroup(old, created, [], 4);
            old.remove();
        }
    });
  });
} catch (e) {
    // No desktop containment to look at: the panels were done above.
}
