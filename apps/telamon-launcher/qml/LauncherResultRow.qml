pragma ComponentBehavior: Bound
import QtQuick
import org.kde.kirigami as Kirigami
import Telamon.Ui

// One row of a list: icon, title and subtitle, and its kind on the right.
// The top hit (`large`) is the best-match card: taller, with its quick
// actions in a row under the name (Open, Pin, Show in Folder, Copy Path)
// when the list gives it `backend` and `actions`. A command line is elided in the
// middle, with the whole line in the tooltip and the accessible description.
Item {
    id: row

    required property int index
    required property var model

    // Whether this is the list's current row (drawn highlighted).
    property bool current: false
    property bool large: false
    property bool showKind: true
    // For the best-match card's actions; null elsewhere.
    property var backend: null
    property var actions: null
    // Bumped by the list when the pins change, so Pin/Unpin follows.
    property int pinsVersion: 0

    readonly property string rowId: model.id
    readonly property bool commandLine: rowId.startsWith("run:") || rowId.startsWith("term:")
    readonly property string kindLabel: kindText(model.kind)
    readonly property bool showActions: large && backend !== null && actions !== null
    readonly property string desktopId: showActions ? backend.desktopIdOf(rowId) : ""
    readonly property string uri: showActions ? backend.uriOf(rowId) : ""
    readonly property bool pinned: pinsVersion >= 0 && desktopId.length > 0 && backend.isPinned(rowId)
    readonly property bool pinnable: pinsVersion >= 0 && desktopId.length > 0 && backend.canPin(rowId)

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

    signal clicked()
    signal menuRequested(real x, real y)
    signal hoveredRow()

    function kindText(kind) {
        switch (kind) {
        case "app": return qsTr("App")
        case "setting": return qsTr("Setting")
        case "file": return qsTr("File")
        case "folder": return qsTr("Folder")
        case "calculator": return qsTr("Calculator")
        case "command": return qsTr("Command")
        case "session": return qsTr("System")
        case "web": return qsTr("Web")
        default: return qsTr("Result")
        }
    }

    // The card is taller while its actions show (Open is always one).
    implicitHeight: large ? Kirigami.Units.gridUnit * (quickActions.item !== null && quickActions.visible ? 5.5 : 3.5) : Math.max(TelamonStyle.rowHeight, Kirigami.Units.gridUnit * 2.25)
    implicitWidth: ListView.view ? ListView.view.width : Kirigami.Units.gridUnit * 20

    Accessible.role: Accessible.ListItem
    Accessible.name: model.title + ", " + kindLabel
    Accessible.description: commandLine ? model.title : model.subtitle
    Accessible.selected: current
    Accessible.focusable: true
    Accessible.onPressAction: row.clicked()

    Rectangle {
        anchors.fill: parent
        anchors.leftMargin: TelamonStyle.spacingXSmall
        anchors.rightMargin: TelamonStyle.spacingXSmall
        radius: row.large ? TelamonStyle.radiusLarge : TelamonStyle.radius
        color: row.current ? TelamonStyle.selection : (mouse.containsMouse ? TelamonStyle.hover : (row.large ? Qt.alpha(Kirigami.Theme.textColor, 0.04) : "transparent"))
        border.width: row.large ? 1 : 0
        border.color: TelamonStyle.separator

        TelamonFocusRing {
            anchors.fill: parent
            radius: parent.radius + gap
            shown: row.activeFocus
        }
    }

    Row {
        id: mainRow
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: row.large ? Kirigami.Units.gridUnit * 3.5 : parent.height
        anchors.leftMargin: TelamonStyle.spacingLarge
        anchors.rightMargin: TelamonStyle.spacingLarge
        spacing: TelamonStyle.spacingLarge

        LauncherItemIcon {
            id: icon
            anchors.verticalCenter: parent.verticalCenter
            width: row.large ? Kirigami.Units.iconSizes.large : Kirigami.Units.iconSizes.medium
            height: width
            name: row.model.icon
        }

        Column {
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width - icon.width - kind.width - parent.spacing * 2

            TelamonLabel {
                width: parent.width
                text: row.model.title
                textFormat: Text.PlainText
                textStyle: row.large ? TelamonLabel.Heading : TelamonLabel.Body
                elide: row.commandLine ? Text.ElideMiddle : Text.ElideRight
                maximumLineCount: 1
            }
            TelamonLabel {
                width: parent.width
                visible: text.length > 0
                text: row.model.subtitle
                textFormat: Text.PlainText
                textStyle: TelamonLabel.Caption
                elide: Text.ElideMiddle
                maximumLineCount: 1
            }
        }

        TelamonLabel {
            id: kind
            anchors.verticalCenter: parent.verticalCenter
            visible: row.showKind
            width: visible ? implicitWidth : 0
            text: row.kindLabel
            textFormat: Text.PlainText
            textStyle: TelamonLabel.Caption
        }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onContainsMouseChanged: if (containsMouse) row.hoveredRow()
        onClicked: (event) => {
            if (event.button === Qt.RightButton) {
                row.menuRequested(event.x, event.y)
            } else {
                row.clicked()
            }
        }
    }

    // The best match's quick actions, above the row's MouseArea. Made only
    // for the card: the other rows never show them, and rows are made anew
    // for each query.
    Loader {
        id: quickActions

        anchors.top: mainRow.bottom
        anchors.left: parent.left
        anchors.leftMargin: TelamonStyle.spacingLarge + Kirigami.Units.iconSizes.large + TelamonStyle.spacingLarge
        active: row.showActions

        sourceComponent: Row {
            spacing: TelamonStyle.spacingSmall

            TelamonButton {
                variant: TelamonButton.Prominent
                text: qsTr("Open")
                symbol: Symbols.OpenInNew
                onClicked: row.clicked()
            }
            TelamonButton {
                visible: row.pinned || row.pinnable
                text: row.pinned ? qsTr("Unpin") : qsTr("Pin")
                symbol: Symbols.PushPin
                onClicked: row.pinned ? row.backend.unpin(row.rowId) : row.backend.pin(row.rowId)
            }
            TelamonButton {
                visible: row.uri.length > 0
                text: qsTr("Show in Folder")
                symbol: Symbols.FolderOpen
                onClicked: row.actions.openContainingFolder(row.uri)
            }
            TelamonButton {
                visible: row.uri.length > 0
                text: qsTr("Copy Path")
                symbol: Symbols.ContentCopy
                onClicked: row.actions.copyText(row.localPath(row.uri))
            }
        }
    }

    // The whole command line, for a line elided in the middle.
    Loader {
        active: row.commandLine
        sourceComponent: TelamonToolTip {
            parent: row
            text: row.model.title
            shown: mouse.containsMouse
        }
    }
}
