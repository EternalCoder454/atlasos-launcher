pragma ComponentBehavior: Bound
import QtQuick
import org.kde.kirigami as Kirigami
import Atlas.Ui

// Start's footer, like Windows 11's: the user tile on the left; Settings,
// Files and the power menu on the right. The power menu's choices run at
// once (the search results' ones ask through Plasma's prompt instead).
FocusScope {
    id: footer

    required property var actions

    implicitHeight: Kirigami.Units.gridUnit * 3

    Rectangle {
        anchors.top: parent.top
        width: parent.width
        height: 1
        color: AtlasStyle.separator
    }

    // The user tile: avatar and name; opens Settings' Users page.
    AtlasButton {
        id: user
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        variant: AtlasButton.Ghost
        focus: true
        activeFocusOnTab: true
        text: footer.actions.userName
        Accessible.name: qsTr("%1, account settings").arg(footer.actions.userName)
        onClicked: footer.actions.openUserSettings()

        contentItem: Row {
            spacing: AtlasStyle.spacing
            AtlasAvatar {
                anchors.verticalCenter: parent.verticalCenter
                size: Kirigami.Units.iconSizes.medium
                name: footer.actions.userName
                source: footer.actions.userIcon.length > 0 ? "file://" + footer.actions.userIcon.split("/").map(encodeURIComponent).join("/") : ""
            }
            AtlasLabel {
                anchors.verticalCenter: parent.verticalCenter
                text: footer.actions.userName
                textFormat: Text.PlainText
            }
        }
    }

    Row {
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        spacing: AtlasStyle.spacingSmall

        ToolbarButton {
            focusable: true
            symbol: Symbols.Settings
            text: qsTr("Settings")
            onClicked: footer.actions.openSettingsApp()
        }
        ToolbarButton {
            focusable: true
            symbol: Symbols.Folder
            text: qsTr("Files")
            onClicked: footer.actions.openFiles()
        }
        ToolbarButton {
            id: power
            focusable: true
            symbol: Symbols.PowerSettingsNew
            text: qsTr("Power")
            Accessible.role: Accessible.ButtonMenu
            onClicked: powerMenu.popup(power, 0, -powerMenu.implicitHeight)
            Keys.onDownPressed: powerMenu.popup(power, 0, -powerMenu.implicitHeight)
        }
    }

    ContextMenu {
        id: powerMenu

        ContextMenuItem {
            text: qsTr("Lock")
            symbol: Symbols.Lock
            onTriggered: footer.actions.power("lock")
        }
        ContextMenuItem {
            visible: footer.actions.canSuspend
            text: qsTr("Sleep")
            symbol: Symbols.Bedtime
            onTriggered: footer.actions.power("sleep")
        }
        ContextMenuItem {
            visible: footer.actions.canHibernate
            text: qsTr("Hibernate")
            symbol: Symbols.ModeStandby
            onTriggered: footer.actions.power("hibernate")
        }
        ContextMenuItem {
            visible: footer.actions.canSwitchUser
            text: qsTr("Switch User")
            symbol: Symbols.SwitchAccount
            onTriggered: footer.actions.power("switch-user")
        }
        ContextMenuSeparator {}
        ContextMenuItem {
            text: qsTr("Log Out")
            symbol: Symbols.Logout
            onTriggered: footer.actions.power("log-out")
        }
        ContextMenuItem {
            text: qsTr("Restart")
            symbol: Symbols.RestartAlt
            onTriggered: footer.actions.power("restart")
        }
        ContextMenuItem {
            text: qsTr("Shut Down")
            symbol: Symbols.PowerSettingsNew
            onTriggered: footer.actions.power("shut-down")
        }
    }
}
