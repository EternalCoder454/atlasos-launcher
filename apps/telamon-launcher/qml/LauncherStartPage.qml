pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Templates as T
import QtCore
import org.kde.kirigami as Kirigami
import Telamon.Ui

// Start's page before anything is typed (docs/DESIGN.md, "Start mode"):
// search first, then quiet suggestions, then browsing. One scrolling page:
//
// 1. Pinned: one row of icons without labels (the name is in the tooltip),
//    Spotlight's quick row rather than Windows' labelled grid.
// 2. Recent: small chips for the last apps and files, never a block.
// 3. Apps: every app as a grid, grouped by kind (Internet, Office, Media...)
//    or A–Z, chosen with the switch beside the heading and remembered.
//
// Tab moves between the regions, the arrows within; Up from the first row
// goes back to the search field.
FocusScope {
    id: page

    required property var backend
    required property var actions
    required property var options
    required property var menu
    required property var pins
    required property var apps
    required property var recentApps
    required property var recentFiles

    readonly property int columns: 6
    readonly property real cell: Math.floor(width / columns)
    readonly property real pinCell: Math.floor(width / 8)
    readonly property bool showRecent: options.showRecent && (recentApps.count > 0 || showRecentFiles)
    readonly property bool showRecentFiles: options.showRecentFiles && recentFiles.count > 0
    readonly property int recentShown: 3

    // 0: grouped by kind, 1: A–Z. Remembered across opens and logins.
    readonly property int appView: viewState.appView
    // [{ key, title, items: [{ id, title, subtitle, icon }] }], in display order.
    property var sections: []
    // False while new sections are made and not yet placed: their tiles
    // draw nothing until each section knows where it is.
    property bool placed: true

    // Up from the first region: back to the search field.
    signal backToField()

    function groupTitle(key) {
        switch (key) {
        case "internet": return qsTr("Internet")
        case "office": return qsTr("Office")
        case "media": return qsTr("Music & Video")
        case "graphics": return qsTr("Graphics")
        case "development": return qsTr("Development")
        case "games": return qsTr("Games")
        case "education": return qsTr("Education")
        case "system": return qsTr("System")
        case "utilities": return qsTr("Utilities")
        default: return qsTr("Other")
        }
    }

    // The order the groups are shown in (catalog::GROUPS).
    readonly property var groupOrder: ["internet", "office", "media", "graphics", "development", "games", "education", "system", "utilities", "other"]

    // The sections, from the A–Z app list (kept in its order inside each).
    function rebuild() {
        const byKey = ({})
        const keys = []
        for (let i = 0; i < appRows.count; ++i) {
            const o = appRows.objectAt(i) as AppRow
            if (!o) {
                continue
            }
            const m = o.model
            const item = { id: m.id, title: m.title, subtitle: m.subtitle, icon: m.icon }
            let key
            if (page.appView === 1) {
                key = m.section.length > 0 ? m.section : "#"
            } else {
                key = page.backend.categoryOf(m.id)
                if (key.length === 0) {
                    key = "other"
                }
            }
            if (!byKey[key]) {
                byKey[key] = []
                keys.push(key)
            }
            byKey[key].push(item)
        }
        const order = page.appView === 1 ? keys : page.groupOrder.filter(k => byKey[k] !== undefined)
        page.placed = false
        page.sections = order.map(k => ({
            key: k,
            title: page.appView === 1 ? k : page.groupTitle(k),
            items: byKey[k]
        }))
        // Place the new sections now, not at the next frame: each one's
        // place decides which of its tiles draw (LauncherAppSection).
        appsColumn.forceLayout()
        column.forceLayout()
        page.placed = true
    }

    // A fresh page each time the panel opens.
    function reset() {
        if (letters.item) {
            letters.item.visible = false
        }
        flick.contentY = 0
        pinList.currentIndex = -1
    }

    // The first recent chip shown, or null. The Flow's children include its
    // two Repeaters, which can't take focus, so chips are told by rowId.
    function firstChip(): Item {
        if (!page.showRecent)
            return null
        for (const c of chips.children) {
            if (c.visible && c.rowId !== undefined)
                return c
        }
        return null
    }

    // Down from the field: the first region that has something.
    function focusFirst() {
        if (pinList.count > 0) {
            pinList.currentIndex = Math.max(0, pinList.currentIndex)
            pinList.forceActiveFocus(Qt.TabFocusReason)
        } else if (page.firstChip()) {
            page.firstChip().forceActiveFocus(Qt.TabFocusReason)
        } else {
            page.focusSection(0, true)
        }
    }

    // Moves the keyboard into section `i`, at its first or last row.
    function focusSection(i, top) {
        const grid = sectionRepeater.itemAt(i) as LauncherAppSection
        if (!grid) {
            return false
        }
        grid.enter(top)
        return true
    }

    function jumpTo(letter) {
        const grid = letters.item as LauncherLetterGrid
        grid.close()
        for (let i = 0; i < page.sections.length; ++i) {
            if (page.sections[i].key === letter) {
                const section = sectionRepeater.itemAt(i)
                if (section) {
                    flick.contentY = Math.min(section.y + appsColumn.y, Math.max(0, flick.contentHeight - flick.height))
                    page.focusSection(i, true)
                }
                return
            }
        }
    }

    // One row of the app list, read into plain data for the grids.
    component AppRow: QtObject {
        required property var model
    }
    Instantiator {
        id: appRows
        model: page.apps
        delegate: AppRow {}
        onObjectAdded: Qt.callLater(page.rebuild)
        onObjectRemoved: Qt.callLater(page.rebuild)
    }
    // A renamed app keeps its row but not its place: the titles and the
    // order of the sections follow the model.
    Connections {
        target: page.apps
        function onDataChanged() { Qt.callLater(page.rebuild) }
        function onRowsMoved() { Qt.callLater(page.rebuild) }
    }
    onAppViewChanged: rebuild()

    Settings {
        id: viewState
        location: StandardPaths.writableLocation(StandardPaths.GenericConfigLocation) + "/telamon-launcher/state.conf"
        category: "Start"
        property int appView: 0
    }

    Flickable {
        id: flick

        anchors.fill: parent
        clip: true
        contentHeight: column.implicitHeight
        boundsBehavior: Flickable.StopAtBounds
        T.ScrollBar.vertical: TelamonScrollBar {}

        // Kirigami's wheel handling: even steps for a notched wheel, pixel
        // scrolling for a free-spinning or high-resolution one (both of the
        // G502's modes), and no flick inertia fighting the wheel.
        Kirigami.WheelHandler {
            target: flick
            filterMouseEvents: true
        }

        // Keeps the focused item in view.
        function show(item) {
            if (!item) {
                return
            }
            const p = item.mapToItem(column, 0, 0)
            if (p.y < contentY) {
                contentY = Math.max(0, p.y - TelamonStyle.spacing)
            } else if (p.y + item.height > contentY + height) {
                contentY = Math.min(contentHeight - height, p.y + item.height - height + TelamonStyle.spacing)
            }
        }

        Column {
            id: column
            width: flick.width
            spacing: TelamonStyle.spacingLarge
            bottomPadding: TelamonStyle.spacingLarge

            // --- Pinned: one quiet row of icons ---
            ListView {
                id: pinList

                width: parent.width
                height: page.pinCell
                visible: count > 0
                orientation: ListView.Horizontal
                model: page.pins
                currentIndex: -1
                keyNavigationEnabled: true
                highlightMoveDuration: 0
                boundsBehavior: Flickable.StopAtBounds
                clip: true
                activeFocusOnTab: true
                Accessible.role: Accessible.List
                Accessible.name: qsTr("Pinned")
                // Centred while the pins fit; scrolls when they don't.
                leftMargin: Math.max(0, (width - count * page.pinCell) / 2)

                onActiveFocusChanged: if (activeFocus) {
                    if (currentIndex < 0 && count > 0) {
                        currentIndex = 0
                    }
                    flick.show(pinList)
                }

                delegate: LauncherAppTile {
                    id: pin
                    width: page.pinCell
                    height: page.pinCell
                    showLabel: false
                    iconSize: Kirigami.Units.iconSizes.large
                    current: ListView.isCurrentItem && ListView.view.activeFocus
                    focus: ListView.isCurrentItem
                    onClicked: page.backend.activate(1, rowId)
                    onMenuRequested: (x, y) => page.menu.openFor(1, rowId, pin, x, y, index, page.pins.count)
                    onDropped: (id, at) => page.backend.movePin(id, at)
                }

                function currentId() {
                    return currentIndex >= 0 ? model.idAt(currentIndex) : ""
                }
                Keys.onReturnPressed: if (currentIndex >= 0) page.backend.activate(1, currentId())
                Keys.onEnterPressed: if (currentIndex >= 0) page.backend.activate(1, currentId())
                Keys.onUpPressed: page.backToField()
                Keys.onDownPressed: {
                    if (page.firstChip()) {
                        page.firstChip().forceActiveFocus(Qt.TabFocusReason)
                    } else {
                        page.focusSection(0, true)
                    }
                }
                Keys.onMenuPressed: if (currentItem) page.menu.openFor(1, currentId(), currentItem, currentItem.width / 2, currentItem.height / 2, currentIndex, count)
                Keys.onPressed: (event) => {
                    const alt = (event.modifiers & Qt.AltModifier) && (event.modifiers & Qt.ShiftModifier)
                    if (alt && event.key === Qt.Key_Left && currentIndex > 0) {
                        page.backend.movePin(currentId(), currentIndex - 1)
                        currentIndex -= 1
                        event.accepted = true
                    } else if (alt && event.key === Qt.Key_Right && currentIndex < count - 1) {
                        page.backend.movePin(currentId(), currentIndex + 1)
                        currentIndex += 1
                        event.accepted = true
                    } else if (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier) && currentItem) {
                        page.menu.openFor(1, currentId(), currentItem, currentItem.width / 2, currentItem.height / 2, currentIndex, count)
                        event.accepted = true
                    }
                }
            }

            // --- Recent: small chips ---
            Flow {
                id: chips

                width: parent.width
                visible: page.showRecent
                spacing: TelamonStyle.spacing
                Accessible.role: Accessible.Grouping
                Accessible.name: qsTr("Recent")

                Repeater {
                    model: page.recentApps
                    delegate: LauncherChip {
                        id: recentApp
                        visible: index < page.recentShown
                        onClicked: page.backend.activate(3, rowId)
                        onMenuRequested: (x, y) => page.menu.openFor(3, rowId, recentApp, x, y)
                        onActiveFocusChanged: if (activeFocus) flick.show(recentApp)
                        Keys.onUpPressed: pinList.count > 0 ? pinList.forceActiveFocus(Qt.TabFocusReason) : page.backToField()
                        Keys.onDownPressed: page.focusSection(0, true)
                    }
                }
                Repeater {
                    model: page.showRecentFiles ? page.recentFiles : null
                    delegate: LauncherChip {
                        id: recentFile
                        visible: index < page.recentShown
                        onClicked: page.backend.activate(4, rowId)
                        onMenuRequested: (x, y) => page.menu.openFor(4, rowId, recentFile, x, y)
                        onActiveFocusChanged: if (activeFocus) flick.show(recentFile)
                        Keys.onUpPressed: pinList.count > 0 ? pinList.forceActiveFocus(Qt.TabFocusReason) : page.backToField()
                        Keys.onDownPressed: page.focusSection(0, true)
                    }
                }
            }

            // --- Apps: grouped grid, or A–Z ---
            Item {
                width: parent.width
                height: Math.max(appsTitle.implicitHeight, viewSwitch.implicitHeight)

                TelamonLabel {
                    id: appsTitle
                    anchors.verticalCenter: parent.verticalCenter
                    text: qsTr("Apps")
                    textStyle: TelamonLabel.Heading
                }
                TelamonSegmentedControl {
                    id: viewSwitch
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    model: [
                        { text: qsTr("Categories"), symbol: Symbols.Category },
                        { text: qsTr("A–Z"), symbol: Symbols.SortByAlpha }
                    ]
                    currentIndex: page.appView
                    onActivated: (index) => viewState.appView = index
                    Accessible.name: qsTr("Show apps")
                }
            }

            Column {
                id: appsColumn
                width: parent.width
                spacing: TelamonStyle.spacingLarge

                Repeater {
                    id: sectionRepeater
                    model: page.sections

                    delegate: LauncherAppSection {
                        id: section
                        required property int index
                        required property var modelData

                        width: appsColumn.width
                        title: modelData.title
                        items: modelData.items
                        columns: page.columns
                        cell: page.cell
                        letterHeader: page.appView === 1
                        placed: page.placed
                        viewTop: flick.contentY - appsColumn.y - section.y
                        viewHeight: flick.height
                        onHeaderClicked: {
                            letters.active = true
                            const grid = letters.item as LauncherLetterGrid
                            grid.open(modelData.key)
                        }
                        onActivated: (id) => page.backend.activate(2, id)
                        onMenuRequested: (id, item, x, y) => page.menu.openFor(2, id, item, x, y)
                        onFocusedItem: (item) => flick.show(item)
                        onLeaveUp: {
                            if (!page.focusSection(index - 1, false)) {
                                if (page.firstChip()) {
                                    page.firstChip().forceActiveFocus(Qt.BacktabFocusReason)
                                } else if (pinList.count > 0) {
                                    pinList.forceActiveFocus(Qt.BacktabFocusReason)
                                } else {
                                    page.backToField()
                                }
                            }
                        }
                        onLeaveDown: page.focusSection(index + 1, true)
                    }
                }
            }
        }
    }

    // Soft edges where the page scrolls on, so a row cut by the panel's edge
    // fades out instead of peeking half-drawn. Only on the opaque panel: the
    // fade lays the panel's colour over the page, and over the translucent
    // panel (blur on) that colour, itself translucent, would show as a denser
    // band; there the page is cut at its edge.
    component EdgeFade: Rectangle {
        property bool atTop: false
        anchors.left: parent.left
        anchors.right: parent.right
        height: Kirigami.Units.gridUnit * 2
        gradient: Gradient {
            GradientStop {
                position: 0
                color: Qt.alpha(TelamonStyle.floatingBackground, 0)
            }
            GradientStop {
                position: 1
                color: TelamonStyle.floatingBackground
            }
        }
        rotation: atTop ? 180 : 0
    }
    EdgeFade {
        anchors.bottom: flick.bottom
        visible: !Appearance.effective && flick.contentY + flick.height < flick.contentHeight - 1
    }
    EdgeFade {
        anchors.top: flick.top
        atTop: true
        visible: !Appearance.effective && flick.contentY > 1
    }

    // Made the first time a letter heading is clicked.
    Loader {
        id: letters
        anchors.fill: parent
        active: false
        sourceComponent: LauncherLetterGrid {
            letters: page.backend.letters
            onChosen: (letter) => page.jumpTo(letter)
            onClosed: page.focusSection(0, true)
        }
    }
}
