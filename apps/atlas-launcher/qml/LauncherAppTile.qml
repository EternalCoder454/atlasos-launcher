pragma ComponentBehavior: Bound
import QtQuick
import org.kde.kirigami as Kirigami
import Atlas.Ui

// A pinned app: icon over its name. Drag one onto another to reorder.
Item {
    id: tile

    required property int index
    required property var model

    property bool current: false
    readonly property string rowId: model.id

    signal clicked()
    signal menuRequested(real x, real y)
    // Dropped onto the tile at `index`.
    signal dropped(string id, int index)

    Accessible.role: Accessible.ListItem
    Accessible.name: model.title
    Accessible.selected: current
    Accessible.focusable: true
    Accessible.onPressAction: tile.clicked()

    Rectangle {
        anchors.fill: parent
        anchors.margins: AtlasStyle.spacingXSmall
        radius: AtlasStyle.radius
        color: tile.current || drop.containsDrag ? AtlasStyle.selection
             : (drag.containsMouse ? AtlasStyle.hover : "transparent")

        AtlasFocusRing {
            anchors.fill: parent
            radius: parent.radius + gap
            shown: tile.activeFocus
        }
    }

    Column {
        anchors.centerIn: parent
        width: parent.width - AtlasStyle.spacingLarge * 2
        spacing: AtlasStyle.spacingSmall

        LauncherItemIcon {
            anchors.horizontalCenter: parent.horizontalCenter
            width: Kirigami.Units.iconSizes.large
            height: width
            name: tile.model.icon
        }
        AtlasLabel {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: tile.model.title
            textFormat: Text.PlainText
            elide: Text.ElideRight
            maximumLineCount: 1
        }
    }

    MouseArea {
        id: drag
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        drag.target: dragProxy
        drag.threshold: Kirigami.Units.gridUnit / 2
        onClicked: (event) => {
            if (event.button === Qt.RightButton) {
                tile.menuRequested(event.x, event.y)
            } else {
                tile.clicked()
            }
        }
        onReleased: {
            if (dragProxy.Drag.active) {
                dragProxy.Drag.drop()
            }
            dragProxy.x = 0
            dragProxy.y = 0
        }
    }

    // What moves under the pointer while dragging: a copy of the icon.
    Item {
        id: dragProxy
        width: tile.width
        height: tile.height
        Drag.active: drag.drag.active
        Drag.keys: ["atlas-launcher-pin"]
        Drag.hotSpot.x: width / 2
        Drag.hotSpot.y: height / 2
        Drag.source: tile

        LauncherItemIcon {
            anchors.centerIn: parent
            width: Kirigami.Units.iconSizes.large
            height: width
            name: tile.model.icon
            visible: dragProxy.Drag.active
            opacity: 0.8
        }
    }

    DropArea {
        id: drop
        anchors.fill: parent
        keys: ["atlas-launcher-pin"]
        onDropped: (event) => {
            // The proxy's Drag.source is the tile it came from.
            const source = event.source as LauncherAppTile
            if (source && source !== tile) {
                tile.dropped(source.rowId, tile.index)
                event.accept()
            }
        }
    }
}
