import QtQuick
import org.kde.kirigami as Kirigami
import Atlas.Ui

// One row of a list: icon, title and subtitle, and its kind on the right.
// The top hit (`large`) is drawn taller. A command line is elided in the
// middle, with the whole line in the tooltip and the accessible description.
Item {
    id: row

    required property int index
    required property var model

    // Whether this is the list's current row (drawn highlighted).
    property bool current: false
    property bool large: false
    property bool showKind: true

    readonly property string rowId: model.id
    readonly property bool commandLine: rowId.startsWith("run:") || rowId.startsWith("term:")
    readonly property string kindLabel: kindText(model.kind)

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

    implicitHeight: large ? Kirigami.Units.gridUnit * 3.5 : Math.max(AtlasStyle.rowHeight, Kirigami.Units.gridUnit * 2.25)
    implicitWidth: ListView.view ? ListView.view.width : Kirigami.Units.gridUnit * 20

    Accessible.role: Accessible.ListItem
    Accessible.name: model.title + ", " + kindLabel
    Accessible.description: commandLine ? model.title : model.subtitle
    Accessible.selected: current
    Accessible.focusable: true
    Accessible.onPressAction: row.clicked()

    Rectangle {
        anchors.fill: parent
        anchors.leftMargin: AtlasStyle.spacingXSmall
        anchors.rightMargin: AtlasStyle.spacingXSmall
        radius: AtlasStyle.radius
        color: row.current ? AtlasStyle.selection : (mouse.containsMouse ? AtlasStyle.hover : "transparent")

        AtlasFocusRing {
            anchors.fill: parent
            radius: parent.radius + gap
            shown: row.activeFocus
        }
    }

    Row {
        anchors.fill: parent
        anchors.leftMargin: AtlasStyle.spacingLarge
        anchors.rightMargin: AtlasStyle.spacingLarge
        spacing: AtlasStyle.spacingLarge

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

            AtlasLabel {
                width: parent.width
                text: row.model.title
                textFormat: Text.PlainText
                textStyle: row.large ? AtlasLabel.Heading : AtlasLabel.Body
                elide: row.commandLine ? Text.ElideMiddle : Text.ElideRight
                maximumLineCount: 1
            }
            AtlasLabel {
                width: parent.width
                visible: text.length > 0
                text: row.model.subtitle
                textFormat: Text.PlainText
                textStyle: AtlasLabel.Caption
                elide: Text.ElideMiddle
                maximumLineCount: 1
            }
        }

        AtlasLabel {
            id: kind
            anchors.verticalCenter: parent.verticalCenter
            visible: row.showKind
            width: visible ? implicitWidth : 0
            text: row.kindLabel
            textFormat: Text.PlainText
            textStyle: AtlasLabel.Caption
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

    AtlasToolTip {
        text: row.model.title
        shown: row.commandLine && mouse.containsMouse
    }
}
