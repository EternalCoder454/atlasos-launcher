pragma ComponentBehavior: Bound
import QtQuick
import Atlas.Ui

// One group of the Start page's app grid: its heading (a kind, or a letter
// that opens the jump-to-letter grid) over the apps as labelled tiles.
// Up from the first row and Down from the last hand the keyboard to the
// sections around it.
FocusScope {
    id: section

    property string title: ""
    // [{ id, title, subtitle, icon }]
    property var items: []
    property int columns: 6
    property real cell: 96
    property bool letterHeader: false
    // The part of the Start page in view, from this section's top. Tiles
    // more than a row outside it draw nothing (LauncherAppTile `live`), so
    // a long app list costs only the rows on screen.
    property real viewTop: 0
    // False until the section is placed (LauncherStartPage).
    property bool placed: true
    property real viewHeight: Infinity
    readonly property real firstLiveRow: Math.floor((viewTop - grid.y) / grid.cellHeight) - 1
    readonly property real lastLiveRow: Math.floor((viewTop + viewHeight - grid.y) / grid.cellHeight) + 1

    signal activated(string id)
    signal menuRequested(string id, var item, real x, real y)
    signal headerClicked()
    signal focusedItem(var item)
    signal leaveUp()
    signal leaveDown()

    // The keyboard comes in from above (first row) or below (last row).
    function enter(top) {
        if (grid.count === 0) {
            return
        }
        grid.currentIndex = top ? 0 : Math.floor((grid.count - 1) / section.columns) * section.columns
        grid.forceActiveFocus(top ? Qt.TabFocusReason : Qt.BacktabFocusReason)
    }

    implicitHeight: header.height + AtlasStyle.spacingSmall + grid.height

    Item {
        id: header
        width: parent.width
        height: Math.max(label.implicitHeight, letterButton.implicitHeight)

        AtlasLabel {
            id: label
            anchors.verticalCenter: parent.verticalCenter
            visible: !section.letterHeader
            text: section.title
            textStyle: AtlasLabel.Caption
            font.weight: Font.DemiBold
        }
        AtlasButton {
            id: letterButton
            anchors.verticalCenter: parent.verticalCenter
            visible: section.letterHeader
            variant: AtlasButton.Ghost
            text: section.title
            Accessible.name: qsTr("%1, jump to letter").arg(section.title)
            onClicked: section.headerClicked()
        }
    }

    GridView {
        id: grid

        readonly property int rows: Math.ceil(count / section.columns)

        anchors.top: header.bottom
        anchors.topMargin: AtlasStyle.spacingSmall
        width: parent.width
        height: rows * cellHeight
        interactive: false
        model: section.items
        cellWidth: section.cell
        cellHeight: section.cell * 0.9
        currentIndex: -1
        keyNavigationEnabled: true
        highlightMoveDuration: 0
        activeFocusOnTab: true
        Accessible.role: Accessible.List
        Accessible.name: section.title

        onActiveFocusChanged: if (activeFocus && currentIndex < 0 && count > 0) currentIndex = 0
        onCurrentItemChanged: if (activeFocus && currentItem) section.focusedItem(currentItem)

        delegate: Item {
            id: cellItem

            required property int index
            required property var modelData

            width: GridView.view.cellWidth
            height: GridView.view.cellHeight

            LauncherAppTile {
                id: tile
                anchors.fill: parent
                index: cellItem.index
                model: cellItem.modelData
                reorderable: false
                live: {
                    const row = Math.floor(cellItem.index / section.columns)
                    return section.placed && row >= section.firstLiveRow && row <= section.lastLiveRow
                }
                current: cellItem.GridView.isCurrentItem && cellItem.GridView.view.activeFocus
                focus: cellItem.GridView.isCurrentItem
                onClicked: section.activated(rowId)
                onMenuRequested: (x, y) => section.menuRequested(rowId, tile, x, y)
            }
        }

        function currentId() {
            return currentIndex >= 0 && currentIndex < section.items.length ? section.items[currentIndex].id : ""
        }

        Keys.onReturnPressed: if (currentIndex >= 0) section.activated(currentId())
        Keys.onEnterPressed: if (currentIndex >= 0) section.activated(currentId())
        Keys.onMenuPressed: if (currentItem) section.menuRequested(currentId(), currentItem, currentItem.width / 2, currentItem.height / 2)
        Keys.onPressed: (event) => {
            if (event.key === Qt.Key_Up && currentIndex < section.columns) {
                section.leaveUp()
                event.accepted = true
            } else if (event.key === Qt.Key_Down && currentIndex + section.columns >= count) {
                // Past the last row (a shorter last row included).
                if (Math.floor(currentIndex / section.columns) === rows - 1) {
                    section.leaveDown()
                } else {
                    currentIndex = count - 1
                }
                event.accepted = true
            } else if (event.key === Qt.Key_F10 && (event.modifiers & Qt.ShiftModifier) && currentItem) {
                section.menuRequested(currentId(), currentItem, currentItem.width / 2, currentItem.height / 2)
                event.accepted = true
            }
        }
    }
}
