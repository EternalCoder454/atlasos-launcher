pragma ComponentBehavior: Bound
import QtQuick
import Atlas.Ui

// The jump-to-letter grid, opened from an A–Z header: # and A to Z, with the
// letters that have no apps dimmed. Arrow keys move, Enter jumps, Esc closes.
FocusScope {
    id: grid

    // The letters that have apps (the backend's `letters`).
    property var letters: []

    signal chosen(string letter)
    signal closed()

    readonly property var all: ["#", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
                                "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"]
    readonly property int columns: 7

    function open(letter) {
        visible = true
        let at = all.indexOf(letter)
        if (at < 0 || !repeater.itemAt(at).enabled) {
            at = all.findIndex(l => letters.indexOf(l) >= 0)
        }
        if (at >= 0) {
            repeater.itemAt(at).forceActiveFocus(Qt.TabFocusReason)
        } else {
            forceActiveFocus()
        }
    }

    function close() {
        visible = false
        closed()
    }

    visible: false
    Accessible.role: Accessible.List
    Accessible.name: qsTr("Jump to Letter")

    Keys.onEscapePressed: close()

    Rectangle {
        anchors.fill: parent
        radius: AtlasStyle.radiusLarge
        color: AtlasStyle.floatingBackground
    }

    Grid {
        anchors.centerIn: parent
        columns: grid.columns
        spacing: AtlasStyle.spacingSmall

        Repeater {
            id: repeater
            model: grid.all

            AtlasButton {
                required property int index
                required property string modelData
                readonly property bool has: grid.letters.indexOf(modelData) >= 0

                text: modelData
                variant: AtlasButton.Ghost
                enabled: has
                width: height * 1.4
                Accessible.name: modelData === "#" ? qsTr("Numbers and symbols") : modelData
                onClicked: grid.chosen(modelData)

                // The next letter with apps, `step` at a time (dimmed ones
                // can't take focus).
                function moveBy(step) {
                    for (let i = index + step; i >= 0 && i < repeater.count; i += step) {
                        if (repeater.itemAt(i).enabled) {
                            repeater.itemAt(i).forceActiveFocus(Qt.TabFocusReason)
                            return
                        }
                    }
                }
                Keys.onLeftPressed: moveBy(-1)
                Keys.onRightPressed: moveBy(1)
                Keys.onUpPressed: moveBy(-grid.columns)
                Keys.onDownPressed: moveBy(grid.columns)
                Keys.onReturnPressed: if (has) grid.chosen(modelData)
                Keys.onEnterPressed: if (has) grid.chosen(modelData)
            }
        }
    }
}
