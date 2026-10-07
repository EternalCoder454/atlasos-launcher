pragma ComponentBehavior: Bound
import QtQuick
import org.kde.kirigami as Kirigami
import Telamon.Ui

// An app tile: icon over its name, or the icon alone (`showLabel: false`, the
// pinned row) with the name in a tooltip. Pinned tiles drag onto each other
// to reorder (`reorderable`).
Item {
    id: tile

    required property int index
    required property var model

    property bool current: false
    property bool showLabel: true
    property bool reorderable: true
    property real iconSize: Kirigami.Units.iconSizes.large
    // False while the tile is far out of view: it keeps its place, focus and
    // accessible name, and draws nothing (the app grid, LauncherAppSection).
    property bool live: true
    readonly property string rowId: model.id
    // Where the name is (or would be, for a tile without a label): the top of
    // the rename field laid over it (LauncherRenameEditor).
    readonly property real nameTop: showLabel
        ? (height - (iconSize + TelamonStyle.spacingSmall + Kirigami.Units.gridUnit * 1.3)) / 2 + iconSize + TelamonStyle.spacingSmall
        : height / 2 + iconSize / 2 + TelamonStyle.spacingSmall

    signal clicked()
    signal menuRequested(real x, real y)
    // Dropped onto the tile at `index`.
    signal dropped(string id, int index)

    Accessible.role: Accessible.ListItem
    Accessible.name: model.title
    Accessible.selected: current
    Accessible.focusable: true
    Accessible.onPressAction: tile.clicked()

    // What the tile draws and how it takes the pointer, made while `live`.
    Loader {
        anchors.fill: parent
        active: tile.live
        sourceComponent: Item {
            Rectangle {
                id: highlight
                anchors.fill: parent
                anchors.margins: TelamonStyle.spacingXSmall
                radius: TelamonStyle.radius
                color: tile.current || (dropTarget.item as DropArea)?.containsDrag ? TelamonStyle.selection
                     : (drag.containsMouse ? TelamonStyle.hover : "transparent")

                // The focus ring, made when the tile takes the focus and kept
                // until it has faded out.
                Loader {
                    anchors.fill: parent
                    active: tile.activeFocus || ((item as Item)?.visible ?? false)
                    sourceComponent: TelamonFocusRing {
                        radius: highlight.radius + gap
                        // Bound once made, so it fades in as it does when
                        // the focus comes to a tile that has its ring.
                        Component.onCompleted: shown = Qt.binding(() => tile.activeFocus)
                    }
                }
            }

            Column {
                anchors.centerIn: parent
                width: parent.width - TelamonStyle.spacingLarge * 2
                spacing: TelamonStyle.spacingSmall

                LauncherItemIcon {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: tile.iconSize
                    height: width
                    name: tile.model.icon
                }
                TelamonLabel {
                    width: parent.width
                    visible: tile.showLabel
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
                drag.target: dragProxy.item as Item
                drag.threshold: Kirigami.Units.gridUnit / 2
                onClicked: (event) => {
                    if (event.button === Qt.RightButton) {
                        tile.menuRequested(event.x, event.y)
                    } else {
                        tile.clicked()
                    }
                }
                onReleased: {
                    const proxy = dragProxy.item as Item
                    if (!proxy) {
                        return
                    }
                    if (proxy.Drag.active) {
                        proxy.Drag.drop()
                    }
                    proxy.x = 0
                    proxy.y = 0
                }
            }

            // The drag and drop parts exist only on tiles that reorder (the pinned
            // row), and the name's tooltip only on tiles without a label: the app
            // grid's many tiles never use them.

            // What moves under the pointer while dragging: a copy of the icon.
            Loader {
                id: dragProxy
                active: tile.reorderable
                sourceComponent: Item {
                    id: proxy
                    width: tile.width
                    height: tile.height
                    Drag.active: drag.drag.active
                    Drag.keys: ["telamon-launcher-pin"]
                    Drag.hotSpot.x: width / 2
                    Drag.hotSpot.y: height / 2
                    Drag.source: tile

                    LauncherItemIcon {
                        anchors.centerIn: parent
                        width: tile.iconSize
                        height: width
                        name: tile.model.icon
                        visible: proxy.Drag.active
                        opacity: 0.8
                    }
                }
            }

            Loader {
                id: dropTarget
                anchors.fill: parent
                active: tile.reorderable
                sourceComponent: DropArea {
                    keys: ["telamon-launcher-pin"]
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

            Loader {
                active: !tile.showLabel
                sourceComponent: TelamonToolTip {
                    parent: tile
                    text: tile.model.title
                    shown: drag.containsMouse || tile.activeFocus
                }
            }
        }
    }
}
