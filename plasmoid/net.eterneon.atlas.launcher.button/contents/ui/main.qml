pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import QtQuick.Window
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasmoid
import org.kde.plasma.workspace.dbus as DBus
import org.kde.kirigami as Kirigami

// The dock's launcher button. All of the launcher runs in its own process
// (atlas-launcher); this only asks it over D-Bus to open (docs/DESIGN.md,
// "Form"): a click, or Meta through plasmashell's "Activate Application
// Launcher" (this applet provides org.kde.plasma.launchermenu), calls
// ToggleStart(a{sv}) with the screen and this button's rect, so the panel
// opens centred above the dock; and while it is open the dock stays shown.
PlasmoidItem {
    id: root

    readonly property string service: "net.eterneon.atlas.launcher"
    readonly property string objectPath: "/net/eterneon/atlas/launcher"
    readonly property string iface: "net.eterneon.atlas.Launcher1"
    readonly property bool vertical: Plasmoid.formFactor === PlasmaCore.Types.Vertical
    readonly property bool launcherOpen: launcherProps.properties.Visible === true

    // The button itself, shown in place, as the other AtlasOS panel widgets
    // do: a compact-only applet never gets its compact item in this panel.
    preferredRepresentation: fullRepresentation
    Plasmoid.backgroundHints: PlasmaCore.Types.NoBackground
    Plasmoid.icon: "atlasos"
    // The dock auto-hides; while the launcher is open it stays up, as the
    // taskbar does under Windows' Start: an applet that needs attention keeps
    // its panel shown.
    Plasmoid.status: root.launcherOpen ? PlasmaCore.Types.NeedsAttentionStatus : PlasmaCore.Types.ActiveStatus
    toolTipMainText: i18nc("@info:tooltip", "AtlasOS Launcher")
    toolTipSubText: ""

    // The screen and the button's rect in global coordinates, as the
    // launcher wants them: anchor is (x, y, w, h), sent as an array (the
    // QML module has no struct type; the launcher accepts av).
    function anchorOptions() {
        const item = root.fullRepresentationItem as LauncherButton;
        // Attached properties (Window.window) only resolve in the item's own
        // scope, so the button exposes its window as a plain property.
        if (!item || !item.panelWindow || item.width <= 0 || item.height <= 0) {
            return null;
        }
        const pos = item.mapToGlobal(0, 0);
        return {
            "screen": item.screenName,
            "anchor": [Math.round(pos.x), Math.round(pos.y), Math.round(item.width), Math.round(item.height)]
        };
    }

    // Plasma's D-Bus module only sends its own typed values: a JS object or
    // array passed as is is dropped. This holds the a{sv} argument.
    property DBus.dict optionsArg

    function call(member, args) {
        DBus.SessionBus.asyncCall({
            "service": root.service,
            "path": root.objectPath,
            "iface": root.iface,
            "member": member,
            "arguments": args
        }, () => {}, error => console.warn("atlas-launcher button:", member, "failed:", error.message));
    }

    function toggleStart() {
        const options = anchorOptions();
        // Without a rect, the launcher opens at the bottom centre.
        if (options) {
            root.optionsArg = options;
            call("ToggleStart", [root.optionsArg]);
        } else {
            call("ToggleStart", []);
        }
    }

    function sendAnchor() {
        const options = anchorOptions();
        if (options && options.screen !== "") {
            root.optionsArg = options;
            call("SetDockAnchor", [root.optionsArg]);
        }
    }

    // The old launcher's favourites, handed over once (DESIGN.md, "Form").
    function importPins() {
        const raw = String(Plasmoid.configuration.ImportPins || "").trim();
        if (raw === "") {
            return;
        }
        const ids = raw.split(",").map(s => s.trim()).filter(s => s !== "").slice(0, 64);
        Plasmoid.configuration.ImportPins = "";
        if (ids.length > 0) {
            call("ImportPins", [ids]);
        }
    }

    Connections {
        target: Plasmoid
        function onActivated() {
            root.toggleStart();
        }
    }

    // Debounced, so a panel relayout sends one anchor.
    Timer {
        id: anchorTimer
        interval: 250
        onTriggered: root.sendAnchor()
    }

    DBus.Properties {
        id: launcherProps
        busType: DBus.BusType.Session
        service: root.service
        path: root.objectPath
        iface: root.iface
    }

    Component.onCompleted: Qt.callLater(root.importPins)

    // The button, a named type so anchorOptions() can read its window.
    component LauncherButton: MouseArea {
        readonly property var panelWindow: Window.window
        readonly property string screenName: Screen.name
    }

    fullRepresentation: LauncherButton {
        id: button

        Layout.minimumWidth: root.vertical ? -1 : button.height
        Layout.preferredWidth: root.vertical ? -1 : button.height
        Layout.minimumHeight: root.vertical ? button.width : -1
        Layout.preferredHeight: root.vertical ? button.width : -1

        hoverEnabled: true
        acceptedButtons: Qt.LeftButton
        onClicked: root.toggleStart()

        Accessible.role: Accessible.Button
        Accessible.name: i18nc("@action:button", "AtlasOS Launcher")
        Accessible.onPressAction: root.toggleStart()

        onXChanged: anchorTimer.restart()
        onYChanged: anchorTimer.restart()
        onWidthChanged: anchorTimer.restart()
        onHeightChanged: anchorTimer.restart()
        onPanelWindowChanged: anchorTimer.restart()
        Component.onCompleted: anchorTimer.restart()

        Kirigami.Icon {
            anchors.fill: parent
            anchors.margins: Math.round(Math.min(parent.width, parent.height) * 0.1)
            source: Plasmoid.icon
            active: button.containsMouse
        }

        // The open indicator, like the dock's own: a short accent underline.
        Rectangle {
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.bottom: parent.bottom
            width: Math.round(parent.width * 0.3)
            height: 3
            radius: height / 2
            color: Kirigami.Theme.highlightColor
            visible: root.launcherOpen
        }
    }
}
