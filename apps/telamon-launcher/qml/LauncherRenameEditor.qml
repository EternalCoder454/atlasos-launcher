pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Templates as T
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import Telamon.Ui

// Renaming an app in place (docs/DESIGN.md, "App names"): a small field laid
// over the name of the tile, chip or row that "Rename App…" was chosen for,
// filled with the name it shows and selected. Enter saves, Escape (or a click
// anywhere else, or the panel closing) cancels, and an empty name, or the
// app's own, gives the app's own name back. The name is the launcher's alone:
// the backend cleans and caps it and keeps it in names.conf.
//
// It is one popup for the whole panel, not part of the tiles and rows: those
// are remade whenever the lists change, and a rename changes the lists. Like
// the menu it is drawn in the overlay, and the keyboard goes back to where it
// was when it closes.
T.Popup {
    id: editor

    required property var backend

    // The row being renamed ("app:<desktop id>"), empty while closed.
    property string rowId: ""

    // Opens over `item`, a tile, chip or row, for the app behind row `id`.
    function openFor(id: string, item: var): void {
        const current = editor.backend.nameOf(id)
        if (current.length === 0 || !item) {
            return
        }
        const gap = TelamonStyle.spacingSmall
        const h = field.implicitHeight + gap * 2
        const at = item.mapToItem(editor.parent, 0, 0)
        let left = at.x
        let top = at.y + (item.height - h) / 2
        let w = item.width
        if (item.nameTop !== undefined) {
            // A tile: over its name, or under its icon when it has no label.
            w = Math.max(item.width, Kirigami.Units.gridUnit * 13)
            left = at.x + (item.width - w) / 2
            top = at.y + item.nameTop - gap
        } else if (item.large !== undefined) {
            // A result row: over its title, clear of the icon.
            const icon = item.large ? Kirigami.Units.iconSizes.large : Kirigami.Units.iconSizes.medium
            const lead = TelamonStyle.spacingLarge + icon + TelamonStyle.spacingLarge
            // The card's edge sits one padding left of the title, so the
            // text in the field starts where the title did.
            left = at.x + lead - gap
            w = item.width - lead + gap - TelamonStyle.spacingXSmall
            top = at.y + (Math.min(item.height, Kirigami.Units.gridUnit * 3.5) - h) / 2
        } else {
            // A chip is too small to type in: the field opens just under it,
            // so it never covers the icons of the chips beside it.
            w = Math.max(item.width, Kirigami.Units.gridUnit * 14)
            top = at.y + item.height + TelamonStyle.spacingXSmall
        }
        editor.width = w
        editor.x = Math.max(gap, Math.min(editor.parent.width - w - gap, left))
        editor.y = Math.max(gap, Math.min(editor.parent.height - h - gap, top))
        field.placeholderText = editor.backend.originalNameOf(id)
        field.text = current
        editor.rowId = id
        editor.open()
    }

    // Saves what is typed (the backend cleans it) and closes.
    function commit(): void {
        const id = editor.rowId
        const typed = field.text
        editor.rowId = ""
        editor.close()
        if (id.length > 0) {
            editor.backend.renameApp(id, typed)
        }
    }

    parent: QQC2.Overlay.overlay
    modal: false
    focus: true
    closePolicy: T.Popup.CloseOnEscape | T.Popup.CloseOnPressOutside
    padding: TelamonStyle.spacingSmall
    // A popup has no size of its own: the field's, and its padding.
    implicitWidth: contentWidth + leftPadding + rightPadding
    implicitHeight: contentHeight + topPadding + bottomPadding

    onOpened: {
        field.forceActiveFocus()
        field.selectAll()
    }
    // Closed any other way than by `commit`: nothing is saved.
    onClosed: editor.rowId = ""

    background: Rectangle {
        radius: TelamonStyle.radius
        color: TelamonStyle.surfaceRaised
        border.width: 1
        border.color: TelamonStyle.separator
    }

    contentItem: Item {
        implicitWidth: field.implicitWidth
        implicitHeight: field.implicitHeight

        TelamonTextField {
            id: field

            anchors.fill: parent
            maximumLength: 64
            // The app's own name, which an empty field brings back.
            Accessible.name: qsTr("Rename App")
            Accessible.description: qsTr("Leave empty to use %1.").arg(placeholderText)

            Keys.onReturnPressed: editor.commit()
            Keys.onEnterPressed: editor.commit()
        }
    }
}
