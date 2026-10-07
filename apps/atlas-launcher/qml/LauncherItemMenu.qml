pragma ComponentBehavior: Bound
import QtQuick
import QtQml.Models
import Telamon.Ui

// The context menu of a row (docs/DESIGN.md, the table under "Everywhere").
// `openFor` reads what the row is from the backend each time it opens.
ContextMenu {
    id: menu

    required property var backend
    required property var actions

    // The list the row is in (state.rs `List`: 0 results, 1 pins, 2 apps,
    // 3 recent apps, 4 recent files) and the row's id.
    property int list: 0
    property string rowId: ""
    property int pinIndex: -1
    property int pinCount: 0

    property string desktopId: ""
    property string flatpakId: ""
    property string uri: ""
    property bool pinnable: false
    property bool pinned: false
    property var appActions: []

    readonly property bool isApp: desktopId.length > 0
    readonly property bool isFile: uri.length > 0

    // Opens for row `id` of `list` at (x, y) in `item`.
    function openFor(list, id, item, x, y, pinIndex, pinCount) {
        menu.list = list
        menu.rowId = id
        menu.pinIndex = pinIndex === undefined ? -1 : pinIndex
        menu.pinCount = pinCount === undefined ? 0 : pinCount
        desktopId = backend.desktopIdOf(id)
        flatpakId = backend.flatpakIdOf(id)
        uri = backend.uriOf(id)
        pinnable = backend.canPin(id)
        pinned = backend.isPinned(id)
        appActions = backend.actionsOf(id)
        if (!isApp && !isFile) {
            return
        }
        popup(item, x, y)
    }

    // The path for Copy Path, without NULs; the URI as is if its escapes
    // are malformed.
    function localPath(fileUri) {
        const path = fileUri.replace(/^file:\/\//, "")
        try {
            return decodeURIComponent(path).replace(/\u0000/g, "")
        } catch (e) {
            return path
        }
    }

    ContextMenuItem {
        visible: menu.isApp && menu.pinnable && !menu.pinned
        text: qsTr("Pin to Start")
        symbol: Symbols.PushPin
        onTriggered: menu.backend.pin(menu.rowId)
    }
    ContextMenuItem {
        visible: menu.isApp && menu.pinned
        text: qsTr("Unpin from Start")
        symbol: Symbols.PushPin
        onTriggered: menu.backend.unpin(menu.rowId)
    }
    ContextMenuItem {
        visible: menu.list === 1 && menu.pinIndex > 0
        text: qsTr("Move to Front")
        onTriggered: menu.backend.movePin(menu.rowId, 0)
    }
    ContextMenuItem {
        visible: menu.list === 1 && menu.pinIndex > 0
        text: qsTr("Move Left")
        onTriggered: menu.backend.movePin(menu.rowId, menu.pinIndex - 1)
    }
    ContextMenuItem {
        visible: menu.list === 1 && menu.pinIndex >= 0 && menu.pinIndex < menu.pinCount - 1
        text: qsTr("Move Right")
        onTriggered: menu.backend.movePin(menu.rowId, menu.pinIndex + 1)
    }

    ContextMenuSeparator {
        id: actionsStart
        visible: menu.appActions.length > 0
    }
    Instantiator {
        model: menu.appActions
        delegate: ContextMenuItem {
            required property var modelData
            text: modelData.name
            icon.name: modelData.icon
            onTriggered: menu.actions.launchAction(menu.desktopId, modelData.id)
        }
        onObjectAdded: (index, object) => {
            let at = 0
            for (let i = 0; i < menu.count; ++i) {
                if (menu.itemAt(i) === actionsStart) {
                    at = i + 1
                }
            }
            menu.insertItem(at + index, object)
        }
        onObjectRemoved: (index, object) => menu.removeItem(object)
    }

    ContextMenuSeparator {
        visible: menu.isApp
    }
    ContextMenuItem {
        visible: menu.isApp
        text: qsTr("App Settings")
        symbol: Symbols.Tune
        onTriggered: menu.actions.openAppSettings(menu.desktopId)
    }
    ContextMenuItem {
        visible: menu.isApp
        enabled: menu.flatpakId.length > 0
        text: menu.flatpakId.length > 0 ? qsTr("Uninstall") : qsTr("Uninstall (Part of AtlasOS)")
        destructive: enabled
        onTriggered: menu.actions.uninstall(menu.flatpakId)
    }

    ContextMenuItem {
        visible: menu.isFile
        text: qsTr("Open Containing Folder")
        symbol: Symbols.Folder
        onTriggered: menu.actions.openContainingFolder(menu.uri)
    }
    ContextMenuItem {
        visible: menu.isFile
        text: qsTr("Copy Path")
        onTriggered: menu.actions.copyText(menu.localPath(menu.uri))
    }
    ContextMenuItem {
        visible: menu.isFile && menu.list === 4
        text: qsTr("Remove from Recent")
        symbol: Symbols.History
        onTriggered: menu.actions.removeRecent(menu.uri)
    }
}
