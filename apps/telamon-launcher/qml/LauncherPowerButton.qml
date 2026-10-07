pragma ComponentBehavior: Bound
import QtQuick
import Telamon.Ui

// The power button beside the search field. Its menu's choices run at once
// (the search results' session commands ask through Plasma's prompt
// instead).
ToolbarButton {
    id: power

    required property var actions

    focusable: true
    symbol: Symbols.PowerSettingsNew
    text: qsTr("Power")
    Accessible.role: Accessible.ButtonMenu
    onClicked: openMenu()
    Keys.onDownPressed: openMenu()

    // The menu is made the first time it opens.
    function openMenu() {
        powerMenu.active = true
        const menu = powerMenu.item as ContextMenu
        menu.popup(power, 0, power.height)
    }

    Loader {
        id: powerMenu
        active: false
        sourceComponent: ContextMenu {
            parent: power

            ContextMenuItem {
                text: qsTr("Lock")
                symbol: Symbols.Lock
                onTriggered: power.actions.power("lock")
            }
            ContextMenuItem {
                visible: power.actions.canSuspend
                text: qsTr("Sleep")
                symbol: Symbols.Bedtime
                onTriggered: power.actions.power("sleep")
            }
            ContextMenuItem {
                visible: power.actions.canHibernate
                text: qsTr("Hibernate")
                symbol: Symbols.ModeStandby
                onTriggered: power.actions.power("hibernate")
            }
            ContextMenuItem {
                visible: power.actions.canSwitchUser
                text: qsTr("Switch User")
                symbol: Symbols.SwitchAccount
                onTriggered: power.actions.power("switch-user")
            }
            ContextMenuSeparator {}
            ContextMenuItem {
                text: qsTr("Log Out")
                symbol: Symbols.Logout
                onTriggered: power.actions.power("log-out")
            }
            ContextMenuItem {
                text: qsTr("Restart")
                symbol: Symbols.RestartAlt
                onTriggered: power.actions.power("restart")
            }
            ContextMenuItem {
                text: qsTr("Shut Down")
                symbol: Symbols.PowerSettingsNew
                onTriggered: power.actions.power("shut-down")
            }
        }
    }
}
