pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Templates as T
import org.kde.kirigami as Kirigami
import Telamon.Ui

// The ranked list while the user types. The first row is the top hit, drawn
// larger. The highlighted row stays put when late results (KRunner, files)
// arrive: rows change by diff and the backend keeps the selection's place.
ListView {
    id: list

    required property var backend
    required property var menu
    // For the best-match card's quick actions (LauncherResultRow).
    property var actions: null
    // Bumped when the pins change, so the card's Pin/Unpin follows.
    property int pinsVersion: 0

    // The user moved the highlight in this query (else Enter runs the top hit).
    property bool moved: false
    // When a late answer (KRunner, files) last replaced the top hit of the
    // same query: an Enter just after it would run a row the user never saw.
    property string topId: ""
    property double topChangedAt: 0
    // Enter came before the answer to what is typed: it runs that answer's
    // top hit when it lands, unless the user types or moves first.
    property bool pendingEnter: false
    // The query came with a D-Bus Show (Panel's `needsInteraction`): Enter
    // runs nothing until the user edits it, and for half a second after the
    // show, neither does a click (one meant for the window below).
    property bool locked: false
    property double clickAllowedAt: 0

    signal backToField()

    function noteTop() {
        const id = model.idAt(0)
        if (id !== topId) {
            topId = id
            topChangedAt = Date.now()
        }
    }

    // `byEnter`: from the keyboard, where the top hit may have just changed;
    // else a click.
    function activateCurrent(byEnter) {
        if (byEnter ? locked : !clickAllowed()) {
            return
        }
        if (byEnter && !moved && model.serial !== backend.serial) {
            pendingEnter = true
            return
        }
        if (byEnter && !moved && Date.now() - topChangedAt < 150) {
            return
        }
        const id = model.idAt(Math.max(0, currentIndex))
        if (id.length > 0) {
            backend.activate(0, id)
        }
    }

    function clickAllowed() {
        return !locked || Date.now() >= clickAllowedAt
    }

    function moveBy(step) {
        if (count === 0) {
            return
        }
        pendingEnter = false
        const next = Math.max(0, Math.min(count - 1, currentIndex + step))
        currentIndex = next
        moved = true
        backend.select(model.idAt(next))
    }

    function openMenu() {
        if (currentItem) {
            menu.openFor(0, model.idAt(currentIndex), currentItem, currentItem.width / 2, currentItem.height / 2)
        }
    }

    clip: true
    currentIndex: 0
    keyNavigationEnabled: false
    highlightMoveDuration: 0
    boundsBehavior: Flickable.StopAtBounds
    activeFocusOnTab: true
    // No reuseItems: with the diff's row moves, a pooled row stayed drawn
    // under the new one (headless F0), and a result list is short anyway.

    Accessible.role: Accessible.List
    Accessible.name: qsTr("Results")

    T.ScrollBar.vertical: TelamonScrollBar {}

    // As on the Start page: even wheel steps, pixel scrolling for smooth wheels.
    Kirigami.WheelHandler {
        target: list
        filterMouseEvents: true
    }

    // A new query's first answer: back to the top hit.
    Connections {
        target: list.model
        function onRowsInserted() { list.noteTop() }
        function onRowsRemoved() { list.noteTop() }
        function onRowsMoved() { list.noteTop() }
        function onDataChanged() { list.noteTop() }
        function onModelReset() { list.noteTop() }
        // Fires after the rows of a new query's answer: that top hit is the
        // answer to what was typed, not a late change.
        function onSerialChanged() {
            list.topId = list.model.idAt(0)
            list.topChangedAt = 0
            list.moved = false
            list.currentIndex = 0
            list.positionViewAtBeginning()
            if (list.pendingEnter && list.model.serial === list.backend.serial) {
                list.pendingEnter = false
                list.activateCurrent(false)
            }
        }
    }

    delegate: LauncherResultRow {
        id: row
        width: ListView.view.width
        current: ListView.isCurrentItem
        large: index === 0
        backend: list.backend
        actions: list.actions
        pinsVersion: list.pinsVersion
        focus: ListView.isCurrentItem
        onHoveredRow: {
            list.currentIndex = index
        }
        onClicked: {
            list.currentIndex = index
            if (list.clickAllowed()) {
                list.backend.activate(0, rowId)
            }
        }
        onMenuRequested: (x, y) => list.menu.openFor(0, rowId, row, x, y)
    }

    Keys.onUpPressed: (event) => {
        if (currentIndex <= 0) {
            list.backToField()
        } else {
            moveBy(-1)
        }
    }
    Keys.onDownPressed: moveBy(1)
    Keys.onReturnPressed: activateCurrent(true)
    Keys.onEnterPressed: activateCurrent(true)
    Keys.onMenuPressed: openMenu()
    Keys.onPressed: (event) => {
        if (event.key === Qt.Key_PageDown) {
            moveBy(8)
            event.accepted = true
        } else if (event.key === Qt.Key_PageUp) {
            moveBy(-8)
            event.accepted = true
        } else if (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier)) {
            openMenu()
            event.accepted = true
        }
    }
}
