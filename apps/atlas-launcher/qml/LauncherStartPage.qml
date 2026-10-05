pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Templates as T
import Atlas.Ui

// Start's page before anything is typed, one scrolling list: Pinned (6
// columns, 3 rows until Show All), Recent (apps on the left, files on the
// right, 3 rows until More), then All Apps A–Z with letter headers that open
// the jump-to-letter grid. Tab moves between the regions, arrows within.
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

    // The list's header (Pinned and Recent), untyped: it is an inline Column.
    readonly property var header: list.headerItem

    property bool pinsExpanded: false
    property bool recentExpanded: false

    readonly property int columns: 6
    readonly property real tileSize: Math.floor(width / columns)
    readonly property bool showRecent: options.showRecent && (recentApps.count > 0 || showRecentFiles)
    readonly property bool showRecentFiles: options.showRecentFiles && recentFiles.count > 0

    // Up from the first region: back to the search field.
    signal backToField()

    // A fresh page each time the panel opens.
    function reset() {
        pinsExpanded = false
        recentExpanded = false
        // Not close(): that hands the focus to the list, and the field has it.
        letters.visible = false
        list.currentIndex = -1
        list.positionViewAtBeginning()
    }

    // Down from the field: the first region that has something.
    function focusFirst() {
        const h = page.header
        if (h && page.pins.count > 0) {
            h.pinGrid.currentIndex = Math.max(0, h.pinGrid.currentIndex)
            h.pinGrid.forceActiveFocus(Qt.TabFocusReason)
        } else if (h && page.showRecent && page.recentApps.count > 0) {
            h.recentAppList.currentIndex = 0
            h.recentAppList.forceActiveFocus(Qt.TabFocusReason)
        } else {
            list.currentIndex = 0
            list.forceActiveFocus(Qt.TabFocusReason)
        }
    }

    function jumpTo(letter) {
        letters.close()
        const at = backend.letterIndex(letter)
        if (at >= 0) {
            list.currentIndex = at
            list.positionViewAtIndex(at, ListView.Beginning)
            list.forceActiveFocus(Qt.TabFocusReason)
        }
    }

    ListView {
        id: list

        anchors.fill: parent
        clip: true
        model: page.apps
        currentIndex: -1
        keyNavigationEnabled: true
        highlightMoveDuration: 0
        boundsBehavior: Flickable.StopAtBounds
        activeFocusOnTab: true
        reuseItems: true
        Accessible.role: Accessible.List
        Accessible.name: qsTr("All Apps")
        T.ScrollBar.vertical: AtlasScrollBar {}

        onActiveFocusChanged: if (activeFocus && currentIndex < 0 && count > 0) currentIndex = 0

        header: Column {
            id: header

            property alias pinGrid: pinGrid
            property alias recentAppList: recentAppList
            property alias recentFileList: recentFileList

            width: list.width
            spacing: AtlasStyle.spacing
            bottomPadding: AtlasStyle.spacingLarge

            // --- Pinned ---
            Item {
                width: parent.width
                height: pinnedTitle.implicitHeight
                visible: page.pins.count > 0
                AtlasLabel {
                    id: pinnedTitle
                    text: qsTr("Pinned")
                    textStyle: AtlasLabel.Heading
                }
                TextButton {
                    anchors.right: parent.right
                    visible: page.pins.count > page.columns * 3
                    text: page.pinsExpanded ? qsTr("Show Less") : qsTr("Show All")
                    onClicked: page.pinsExpanded = !page.pinsExpanded
                }
            }

            GridView {
                id: pinGrid

                readonly property int rows: Math.ceil(count / page.columns)

                width: parent.width
                height: page.tileSize * 0.85 * Math.min(rows, page.pinsExpanded ? rows : 3)
                visible: count > 0
                interactive: false
                clip: true
                model: page.pins
                cellWidth: page.tileSize
                cellHeight: page.tileSize * 0.85
                currentIndex: -1
                keyNavigationEnabled: true
                highlightMoveDuration: 0
                activeFocusOnTab: true
                Accessible.role: Accessible.List
                Accessible.name: qsTr("Pinned")

                onActiveFocusChanged: if (activeFocus && currentIndex < 0 && count > 0) currentIndex = 0
                // Keep the current tile shown when it is past the third row.
                onCurrentIndexChanged: if (currentIndex >= page.columns * 3) page.pinsExpanded = true

                delegate: LauncherAppTile {
                    id: tile
                    width: GridView.view.cellWidth
                    height: GridView.view.cellHeight
                    current: GridView.isCurrentItem && GridView.view.activeFocus
                    focus: GridView.isCurrentItem
                    onClicked: page.backend.activate(1, rowId)
                    onMenuRequested: (x, y) => page.menu.openFor(1, rowId, tile, x, y, index, page.pins.count)
                    onDropped: (id, at) => page.backend.movePin(id, at)
                }

                function currentId() {
                    return currentIndex >= 0 ? model.idAt(currentIndex) : ""
                }
                Keys.onReturnPressed: if (currentIndex >= 0) page.backend.activate(1, currentId())
                Keys.onEnterPressed: if (currentIndex >= 0) page.backend.activate(1, currentId())
                Keys.onMenuPressed: if (currentItem) page.menu.openFor(1, currentId(), currentItem, currentItem.width / 2, currentItem.height / 2, currentIndex, count)
                Keys.onPressed: (event) => {
                    const alt = (event.modifiers & Qt.AltModifier) && (event.modifiers & Qt.ShiftModifier)
                    if (alt && event.key === Qt.Key_Left && currentIndex > 0) {
                        const id = currentId()
                        page.backend.movePin(id, currentIndex - 1)
                        currentIndex -= 1
                        event.accepted = true
                    } else if (alt && event.key === Qt.Key_Right && currentIndex < count - 1) {
                        const id = currentId()
                        page.backend.movePin(id, currentIndex + 1)
                        currentIndex += 1
                        event.accepted = true
                    } else if (event.key === Qt.Key_Up && currentIndex < page.columns) {
                        page.backToField()
                        event.accepted = true
                    } else if (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier) && currentItem) {
                        page.menu.openFor(1, currentId(), currentItem, currentItem.width / 2, currentItem.height / 2, currentIndex, count)
                        event.accepted = true
                    }
                }
            }

            // --- Recent ---
            Item {
                width: parent.width
                height: recentTitle.implicitHeight
                visible: page.showRecent
                AtlasLabel {
                    id: recentTitle
                    text: qsTr("Recent")
                    textStyle: AtlasLabel.Heading
                }
                TextButton {
                    anchors.right: parent.right
                    visible: page.recentApps.count > 3 || (page.showRecentFiles && page.recentFiles.count > 3)
                    text: page.recentExpanded ? qsTr("Less") : qsTr("More")
                    onClicked: page.recentExpanded = !page.recentExpanded
                }
            }

            Row {
                width: parent.width
                visible: page.showRecent

                ListView {
                    id: recentAppList
                    width: page.showRecentFiles ? parent.width / 2 : parent.width
                    height: contentHeight
                    visible: count > 0
                    interactive: false
                    model: page.recentApps
                    currentIndex: -1
                    keyNavigationEnabled: true
                    highlightMoveDuration: 0
                    activeFocusOnTab: true
                    Accessible.role: Accessible.List
                    Accessible.name: qsTr("Recent Apps")
                    onActiveFocusChanged: if (activeFocus && currentIndex < 0 && count > 0) currentIndex = 0

                    delegate: LauncherResultRow {
                        id: recentApp
                        width: ListView.view.width
                        visible: page.recentExpanded || index < 3
                        height: visible ? implicitHeight : 0
                        showKind: false
                        current: ListView.isCurrentItem && ListView.view.activeFocus
                        focus: ListView.isCurrentItem
                        onClicked: page.backend.activate(3, rowId)
                        onMenuRequested: (x, y) => page.menu.openFor(3, rowId, recentApp, x, y)
                    }
                    Keys.onReturnPressed: page.backend.activate(3, model.idAt(currentIndex))
                    Keys.onEnterPressed: page.backend.activate(3, model.idAt(currentIndex))
                    Keys.onRightPressed: if (recentFileList.visible) recentFileList.forceActiveFocus(Qt.TabFocusReason)
                    Keys.onMenuPressed: if (currentItem) page.menu.openFor(3, model.idAt(currentIndex), currentItem, 0, 0)
                    // Past the third row while collapsed: show the rest.
                    onCurrentIndexChanged: if (currentIndex >= 3) page.recentExpanded = true
                }

                ListView {
                    id: recentFileList
                    width: recentAppList.visible ? parent.width / 2 : parent.width
                    height: contentHeight
                    visible: page.showRecentFiles
                    interactive: false
                    model: page.recentFiles
                    currentIndex: -1
                    keyNavigationEnabled: true
                    highlightMoveDuration: 0
                    activeFocusOnTab: true
                    Accessible.role: Accessible.List
                    Accessible.name: qsTr("Recent Files")
                    onActiveFocusChanged: if (activeFocus && currentIndex < 0 && count > 0) currentIndex = 0

                    delegate: LauncherResultRow {
                        id: recentFile
                        width: ListView.view.width
                        visible: page.recentExpanded || index < 3
                        height: visible ? implicitHeight : 0
                        showKind: false
                        current: ListView.isCurrentItem && ListView.view.activeFocus
                        focus: ListView.isCurrentItem
                        onClicked: page.backend.activate(4, rowId)
                        onMenuRequested: (x, y) => page.menu.openFor(4, rowId, recentFile, x, y)
                    }
                    Keys.onReturnPressed: page.backend.activate(4, model.idAt(currentIndex))
                    Keys.onEnterPressed: page.backend.activate(4, model.idAt(currentIndex))
                    Keys.onLeftPressed: if (recentAppList.visible) recentAppList.forceActiveFocus(Qt.TabFocusReason)
                    Keys.onMenuPressed: if (currentItem) page.menu.openFor(4, model.idAt(currentIndex), currentItem, 0, 0)
                    onCurrentIndexChanged: if (currentIndex >= 3) page.recentExpanded = true
                }
            }

            AtlasLabel {
                topPadding: AtlasStyle.spacingLarge
                text: qsTr("All Apps")
                textStyle: AtlasLabel.Heading
            }
        }

        section.property: "section"
        section.criteria: ViewSection.FullString
        section.delegate: AtlasButton {
            required property string section
            variant: AtlasButton.Ghost
            text: section
            Accessible.name: qsTr("%1, jump to letter").arg(section)
            onClicked: letters.open(section)
        }

        delegate: LauncherResultRow {
            id: appRow
            width: ListView.view.width
            showKind: false
            current: ListView.isCurrentItem && ListView.view.activeFocus
            focus: ListView.isCurrentItem
            onClicked: page.backend.activate(2, rowId)
            onMenuRequested: (x, y) => page.menu.openFor(2, rowId, appRow, x, y)
        }

        Keys.onReturnPressed: if (currentIndex >= 0) page.backend.activate(2, model.idAt(currentIndex))
        Keys.onEnterPressed: if (currentIndex >= 0) page.backend.activate(2, model.idAt(currentIndex))
        Keys.onMenuPressed: if (currentItem) page.menu.openFor(2, model.idAt(currentIndex), currentItem, 0, 0)
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Up && currentIndex <= 0) {
                page.backToField()
                event.accepted = true
            } else if (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier) && currentItem) {
                page.menu.openFor(2, model.idAt(currentIndex), currentItem, 0, 0)
                event.accepted = true
            }
        }
    }

    LauncherLetterGrid {
        id: letters
        anchors.fill: parent
        letters: page.backend.letters
        onChosen: (letter) => page.jumpTo(letter)
        onClosed: list.forceActiveFocus(Qt.TabFocusReason)
    }
}
