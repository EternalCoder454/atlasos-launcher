import QtQuick
import org.kde.kirigami as Kirigami
import Atlas.Ui

// A recent app or file on the Start page: a small pill with its icon and
// name (Spotlight's quiet suggestions rather than a "Recommended" block).
// The path or kind is in the tooltip.
FocusScope {
    id: chip

    required property int index
    required property var model

    readonly property string rowId: model.id

    signal clicked()
    signal menuRequested(real x, real y)

    // Flow places items by their size, not their implicit size.
    implicitHeight: Math.max(AtlasStyle.controlHeight, Kirigami.Units.gridUnit * 1.9)
    implicitWidth: Math.min(Kirigami.Units.gridUnit * 12, icon.width + content.spacing + name.implicitWidth + AtlasStyle.spacingLarge * 2)
    width: implicitWidth
    height: implicitHeight
    activeFocusOnTab: true

    Accessible.role: Accessible.Button
    Accessible.name: model.title
    Accessible.description: model.subtitle
    Accessible.onPressAction: chip.clicked()

    Keys.onReturnPressed: chip.clicked()
    Keys.onEnterPressed: chip.clicked()
    Keys.onSpacePressed: chip.clicked()
    Keys.onMenuPressed: chip.menuRequested(width / 2, height / 2)

    Rectangle {
        anchors.fill: parent
        radius: AtlasStyle.radiusPill
        color: mouse.pressed ? AtlasStyle.pressed : (mouse.containsMouse ? AtlasStyle.hover : Qt.alpha(Kirigami.Theme.textColor, 0.05))
        border.width: 1
        border.color: AtlasStyle.separator

        AtlasFocusRing {
            anchors.fill: parent
            radius: parent.radius
            shown: chip.activeFocus
        }
    }

    Row {
        id: content
        anchors.verticalCenter: parent.verticalCenter
        x: AtlasStyle.spacingLarge
        width: chip.width - AtlasStyle.spacingLarge * 2
        spacing: AtlasStyle.spacing

        LauncherItemIcon {
            id: icon
            anchors.verticalCenter: parent.verticalCenter
            width: Kirigami.Units.iconSizes.small
            height: width
            name: chip.model.icon
        }
        AtlasLabel {
            id: name
            anchors.verticalCenter: parent.verticalCenter
            width: Math.min(implicitWidth, content.width - icon.width - content.spacing)
            text: chip.model.title
            textFormat: Text.PlainText
            elide: Text.ElideRight
            maximumLineCount: 1
        }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: Qt.PointingHandCursor
        onClicked: (event) => {
            if (event.button === Qt.RightButton) {
                chip.menuRequested(event.x, event.y)
            } else {
                chip.clicked()
            }
        }
    }

    AtlasToolTip {
        text: chip.model.subtitle.length > 0 ? chip.model.subtitle : chip.model.title
        shown: mouse.containsMouse
    }
}
