import QtQuick
import org.kde.kirigami as Kirigami
import Atlas.Ui

// The launcher's panel: one window, created hidden at login and shown by
// Meta, the dock button or Alt+Space (docs/DESIGN.md, "Form").
Window {
    id: root

    required property var backend

    title: AtlasApp.name
    width: Kirigami.Units.gridUnit * 40
    height: Kirigami.Units.gridUnit * 45
    visible: false
    color: "transparent"
    flags: Qt.FramelessWindowHint
}
