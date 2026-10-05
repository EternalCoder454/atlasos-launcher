import QtQuick
import org.kde.plasma.plasmoid
import org.kde.kirigami as Kirigami

// The dock's launcher button. All of the launcher runs in its own process
// (atlas-launcher); this only asks it over D-Bus to open (docs/DESIGN.md,
// "Form"). F14 adds the D-Bus calls, the anchor and the open indicator.
PlasmoidItem {
    id: root

    preferredRepresentation: compactRepresentation
    compactRepresentation: Kirigami.Icon {
        source: "atlasos"
        active: mouse.containsMouse
        MouseArea {
            id: mouse
            anchors.fill: parent
            hoverEnabled: true
        }
    }
}
