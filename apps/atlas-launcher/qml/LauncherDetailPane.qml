import QtQuick
import org.kde.kirigami as Kirigami
import Atlas.Ui

// Start mode's pane beside the results: the highlighted row's icon, name,
// kind and path, and what can be done with it.
FocusScope {
    id: pane

    required property var backend
    required property var actions

    // The highlighted row (a LauncherResultRow), or null.
    property var row: null

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

    readonly property string rowId: row ? row.rowId : ""
    readonly property string desktopId: rowId.length > 0 ? backend.desktopIdOf(rowId) : ""
    readonly property string uri: rowId.length > 0 ? backend.uriOf(rowId) : ""
    // Re-read after a pin change (the backend's pins model tells).
    property int pinsVersion: 0
    readonly property bool pinned: pinsVersion >= 0 && rowId.length > 0 && backend.isPinned(rowId)
    readonly property bool pinnable: pinsVersion >= 0 && rowId.length > 0 && backend.canPin(rowId)

    signal open()

    Column {
        anchors.fill: parent
        anchors.margins: AtlasStyle.spacingLarge
        spacing: AtlasStyle.spacingLarge
        visible: pane.row !== null

        LauncherItemIcon {
            anchors.horizontalCenter: parent.horizontalCenter
            width: Kirigami.Units.iconSizes.enormous
            height: width
            name: pane.row ? pane.row.model.icon : ""
        }
        AtlasLabel {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: pane.row ? pane.row.model.title : ""
            textFormat: Text.PlainText
            textStyle: AtlasLabel.Heading
            wrapMode: Text.Wrap
            maximumLineCount: 3
            elide: Text.ElideRight
        }
        AtlasLabel {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            text: pane.row ? pane.row.kindLabel : ""
            textFormat: Text.PlainText
            textStyle: AtlasLabel.Caption
        }
        AtlasLabel {
            width: parent.width
            visible: text.length > 0
            horizontalAlignment: Text.AlignHCenter
            text: pane.row ? pane.row.model.subtitle : ""
            textFormat: Text.PlainText
            textStyle: AtlasLabel.Caption
            wrapMode: Text.WrapAnywhere
            maximumLineCount: 4
            elide: Text.ElideMiddle
        }

        Column {
            width: parent.width
            spacing: AtlasStyle.spacingSmall

            AtlasButton {
                width: parent.width
                text: qsTr("Open")
                variant: AtlasButton.Prominent
                onClicked: pane.open()
            }
            AtlasButton {
                width: parent.width
                visible: pane.desktopId.length > 0 && (pane.pinned || pane.pinnable)
                text: pane.pinned ? qsTr("Unpin from Start") : qsTr("Pin to Start")
                symbol: Symbols.PushPin
                onClicked: pane.pinned ? pane.backend.unpin(pane.rowId) : pane.backend.pin(pane.rowId)
            }
            AtlasButton {
                width: parent.width
                visible: pane.desktopId.length > 0
                text: qsTr("App Settings")
                symbol: Symbols.Tune
                onClicked: pane.actions.openAppSettings(pane.desktopId)
            }
            AtlasButton {
                width: parent.width
                visible: pane.uri.length > 0
                text: qsTr("Open Containing Folder")
                symbol: Symbols.Folder
                onClicked: pane.actions.openContainingFolder(pane.uri)
            }
            AtlasButton {
                width: parent.width
                visible: pane.uri.length > 0
                text: qsTr("Copy Path")
                onClicked: pane.actions.copyText(pane.localPath(pane.uri))
            }
        }
    }
}
