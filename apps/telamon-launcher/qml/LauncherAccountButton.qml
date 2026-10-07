import QtQuick
import org.kde.kirigami as Kirigami
import Telamon.Ui

// The account button beside the search field: the user's picture (or
// initials) in a circle; opens Settings' Users page.
FocusScope {
    id: account

    required property var actions

    implicitWidth: Kirigami.Units.iconSizes.medium + TelamonStyle.spacing
    implicitHeight: implicitWidth
    activeFocusOnTab: true

    Accessible.role: Accessible.Button
    Accessible.name: qsTr("%1, account settings").arg(actions.userName)
    Accessible.onPressAction: account.actions.openUserSettings()
    Keys.onReturnPressed: account.actions.openUserSettings()
    Keys.onEnterPressed: account.actions.openUserSettings()
    Keys.onSpacePressed: account.actions.openUserSettings()

    Rectangle {
        anchors.fill: parent
        radius: TelamonStyle.radiusPill
        color: mouse.pressed ? TelamonStyle.pressed : (mouse.containsMouse ? TelamonStyle.hover : "transparent")
        TelamonFocusRing {
            anchors.fill: parent
            radius: parent.radius
            shown: account.activeFocus
        }
    }

    TelamonAvatar {
        anchors.centerIn: parent
        size: Kirigami.Units.iconSizes.medium
        name: account.actions.userName
        source: account.actions.userIcon.length > 0 ? "file://" + account.actions.userIcon.split("/").map(encodeURIComponent).join("/") : ""
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: account.actions.openUserSettings()
    }

    TelamonToolTip {
        text: account.actions.userName
        shown: mouse.containsMouse
    }
}
