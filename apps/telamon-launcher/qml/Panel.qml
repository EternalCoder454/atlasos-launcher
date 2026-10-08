pragma ComponentBehavior: Bound
import QtQuick
import org.kde.kirigami as Kirigami
import Telamon.Ui

// The launcher's panel: one window, created hidden at login and shown by
// Meta, the dock button or Alt+Space (docs/DESIGN.md, "Form"). Panel (C++)
// places it and maps it; this file draws it.
//
// Start mode: search first. A pill-shaped field with the account and power
// buttons beside it, over the Start page (a pinned row, recent chips, the
// apps by kind or A–Z) until something is typed, then the ranked results,
// the best match a card with its actions. Search mode: the field and the
// results only.
Window {
    id: root

    // The Rust backend and its lists (src/backend.rs), and the C++ helpers
    // (cpp/actions.h, cpp/options.h), set by main.cpp.
    required property var backend
    required property var results
    required property var pins
    required property var apps
    required property var recentApps
    required property var recentFiles
    required property var actions
    required property var options
    // Set by main.cpp right after loading (cpp/panel.h).
    property var panel: null

    // Read by Panel before each show.
    readonly property size startSize: Qt.size(Kirigami.Units.gridUnit * 36, Kirigami.Units.gridUnit * 36)
    readonly property size searchSize: Qt.size(Kirigami.Units.gridUnit * 38, Kirigami.Units.gridUnit * 26)
    readonly property real cornerRadius: TelamonStyle.radiusLarge * 2
    // The transparency switch (Settings, Appearance) with the compositor's
    // blur, as the other Telamon apps follow it: on, the panel is the
    // floating surface's translucent tint (TelamonStyle.floatingBackground,
    // as Telamon.Ui's menus and popovers have it) over what KWin blurs
    // behind it; off, or with no blur to be had, it is opaque. Panel (C++)
    // reads `blurEnabled`, `cornerRadius` and `blurOffset` to set the blur's
    // region.
    readonly property bool blurEnabled: Appearance.effective
    // How far the sheet is still below its place while it slides in; the
    // blur follows it, so nothing is blurred above the sheet.
    readonly property real blurOffset: slide.y

    // The panel keeps its size while typing: a panel that grew and shrank
    // with each keystroke's results looked like it was flickering.

    // The results on screen answer what is typed now (a keystroke's first
    // answer has arrived). Until then the page that was there stays, so the
    // first letter doesn't flash "No results" and an empty list.
    readonly property bool answered: root.results.serial === root.backend.serial

    readonly property bool startMode: panel !== null && panel.mode === "start"
    readonly property bool searching: field.text.length > 0
    readonly property bool shown: panel !== null && panel.shown

    // A query that came with a D-Bus Show was typed by someone else, so
    // another program can't line up a command for a stray key or click:
    // Enter runs nothing until the user has edited it (a move is not enough:
    // Down then Enter is how a terminal's history is used), and clicks wait
    // half a second (LauncherResultList `locked`).
    property bool needsInteraction: false

    function runTopOrCurrent() {
        if (searching) {
            resultList.activateCurrent(true)
        }
    }

    function showFailure(kind) {
        // The kinds of cpp/executor.cpp and cpp/actions.cpp.
        switch (kind) {
        case "UnknownApp":
        case "UnknownAction":
        case "StaleResult":
            failure.text = qsTr("That item is no longer available.")
            break
        case "Busy":
            failure.text = qsTr("Still opening the last item.")
            break
        case "StoreMissing":
            failure.text = qsTr("Telamon Store is not installed.")
            break
        case "NoClipboard":
            failure.text = qsTr("Could not copy to the clipboard.")
            break
        case "NameRefused":
        case "NamesWriteFailed":
            failure.text = qsTr("Could not save that name.")
            break
        case "NameNotShared":
            failure.text = qsTr("Renamed here only: the dock and menus keep the app's own name.")
            break
        case "SessionCall":
            failure.text = qsTr("The session did not respond.")
            break
        default:
            failure.text = qsTr("Could not open that item.")
        }
        failure.visible = true
        failureTimer.restart()
    }

    // A printable key typed anywhere goes to the field.
    function typeIntoField(text) {
        needsInteraction = false
        field.forceActiveFocus()
        field.insert(field.cursorPosition, text)
    }

    function backToField() {
        field.forceActiveFocus(Qt.BacktabFocusReason)
    }

    title: TelamonApp.name
    visible: false
    color: "transparent"
    flags: Qt.FramelessWindowHint

    // Blur on or off, live: the Transparency switch changed, or the
    // compositor's blur effect did (found out at the next show).
    onBlurEnabledChanged: if (panel) panel.refreshBlur()
    onBlurOffsetChanged: if (panel && blurEnabled) panel.refreshBlur()

    Connections {
        target: root.panel
        function onAboutToShow(mode, query) {
            // Nothing tells us when KWin's blur effect is switched on or
            // off, and this process outlives it: ask again at each show,
            // before the first frame, so that frame has the right tint.
            Appearance.refresh()
            itemMenu.close()
            renamer.cancel()
            failure.visible = false
            failureTimer.stop()
            resultList.pendingEnter = false
            startPage.reset()
            root.backend.refresh()
            // Setting the text starts the query (an empty one clears the list).
            field.text = query
            root.needsInteraction = query.length > 0
            resultList.clickAllowedAt = Date.now() + 500
            resultList.moved = false
            field.forceActiveFocus()
            field.selectAll()
            sheet.enter()
        }
        function onShownChanged() {
            if (!root.panel.shown) {
                itemMenu.close()
                renamer.cancel()
                announce.stop()
                failureTimer.stop()
                // The next show starts at the top anyway: going there now
                // makes the Start page's top rows before that show, not in it.
                startPage.reset()
            }
        }
    }

    // When a row runs, the C++ side hides the panel once it has the
    // activation token; nothing else to do here.

    Connections {
        target: root.actions
        function onFailed(kind) {
            if (root.shown) {
                root.showFailure(kind)
            }
        }
    }

    // The rows' context menu, made the first time one opens: most opens of
    // the panel never right-click. The lists call `itemMenu`.
    QtObject {
        id: itemMenu
        function openFor(list, id, item, x, y, pinIndex, pinCount) {
            itemMenuLoader.active = true
            const menu = itemMenuLoader.item as LauncherItemMenu
            menu.openFor(list, id, item, x, y, pinIndex, pinCount)
        }
        function close() {
            const menu = itemMenuLoader.item as LauncherItemMenu
            if (menu) {
                menu.close()
            }
        }
    }
    Loader {
        id: itemMenuLoader
        active: false
        sourceComponent: LauncherItemMenu {
            parent: root.contentItem
            backend: root.backend
            actions: root.actions
            onRenameRequested: (id, item) => renamer.openFor(id, item)
        }
    }

    // The inline rename field (LauncherRenameEditor, a popup), made the first
    // time "Rename App…" is chosen.
    Loader {
        id: renameLoader
        active: false
        sourceComponent: LauncherRenameEditor {
            backend: root.backend
        }
    }

    QtObject {
        id: renamer
        function openFor(id, item) {
            renameLoader.active = true
            const editor = renameLoader.item as LauncherRenameEditor
            editor.openFor(id, item)
        }
        function cancel() {
            const editor = renameLoader.item as LauncherRenameEditor
            if (editor) {
                editor.close()
            }
        }
    }

    // Everything the panel draws, in one sheet that slides up out of the
    // dock as the panel opens (Start; Search fades in place). Reduced motion
    // (TelamonStyle durations of 0) shows it at once.
    Item {
        id: sheet
        width: parent.width
        height: parent.height
        transform: Translate { id: slide }

        function enter() {
            slideIn.stop()
            slide.y = root.startMode ? Kirigami.Units.gridUnit * 3 : 0
            sheet.opacity = 0
            slideIn.start()
        }

        ParallelAnimation {
            id: slideIn
            NumberAnimation {
                target: slide
                property: "y"
                to: 0
                duration: TelamonStyle.durationLong
                easing.type: Easing.OutCubic
            }
            NumberAnimation {
                target: sheet
                property: "opacity"
                to: 1
                duration: TelamonStyle.duration
                easing.type: Easing.OutCubic
            }
        }

        Rectangle {
            anchors.fill: parent
            radius: root.cornerRadius
            color: TelamonStyle.floatingBackground
            border.width: 1
            border.color: Qt.alpha(Kirigami.Theme.textColor, 0.12)
        }

        // The panel's own menu, from a right click on its background; made
        // on its first use.
        Loader {
            id: panelMenu
            active: false
            sourceComponent: ContextMenu {
                parent: sheet
                ContextMenuItem {
                    text: qsTr("Edit Applications…")
                    onTriggered: root.actions.editApplications()
                }
            }
        }
        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.RightButton
            enabled: root.startMode
            onClicked: {
                panelMenu.active = true
                const menu = panelMenu.item as ContextMenu
                menu.popup()
            }
        }

        FocusScope {
            id: content

            anchors.fill: parent
            anchors.margins: TelamonStyle.spacingXLarge
            focus: true

            Keys.onEscapePressed: {
                if (root.searching) {
                    field.clear()
                    root.backToField()
                } else if (root.panel) {
                    root.panel.hide()
                }
            }
            Keys.onPressed: (event) => {
                const mods = event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier)
                if (field.activeFocus || mods !== 0) {
                    return
                }
                if (event.text.length > 0 && event.text.charCodeAt(0) >= 0x20 && event.text !== "\u007f") {
                    root.typeIntoField(event.text)
                    event.accepted = true
                } else if (event.key === Qt.Key_Backspace && root.searching) {
                    root.needsInteraction = false
                    field.forceActiveFocus()
                    field.remove(field.text.length - 1, field.text.length)
                    event.accepted = true
                }
            }

            // The search pill, and in Start mode the account and power buttons.
            Item {
                id: header
                anchors.top: parent.top
                width: parent.width
                height: field.height

                Row {
                    id: headerButtons
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    visible: root.startMode
                    spacing: TelamonStyle.spacingSmall

                    LauncherAccountButton {
                        anchors.verticalCenter: parent.verticalCenter
                        actions: root.actions
                    }
                    LauncherPowerButton {
                        anchors.verticalCenter: parent.verticalCenter
                        actions: root.actions
                    }
                }
            }

            SearchField {
                id: field

                anchors.top: parent.top
                anchors.left: parent.left
                width: root.startMode ? parent.width - headerButtons.width - TelamonStyle.spacing : parent.width
                implicitHeight: Kirigami.Units.gridUnit * 2.6
                font.pointSize: TelamonStyle.fontSizeHeading
                focus: true
                background: Rectangle {
                    radius: TelamonStyle.radiusSmall
                    color: Qt.alpha(Kirigami.Theme.textColor, 0.06)
                    border.width: 1
                    border.color: field.activeFocus ? TelamonStyle.focus : TelamonStyle.separator
                }
                placeholderText: qsTr("Search apps, settings and files")
                // A huge paste is cut here; the engine reads far less.
                maximumLength: 1024
                Accessible.name: qsTr("Search apps, settings and files")

                // Every keystroke, not the debounced `query`: the engine answers
                // within a frame and stale queries are dropped by serial.
                // Any new text, typed here or routed from elsewhere, drops an
                // Enter still waiting for the old text's answer.
                onTextChanged: {
                    resultList.pendingEnter = false
                    root.backend.search(text)
                }
                onTextEdited: root.needsInteraction = false

                Keys.onReturnPressed: root.runTopOrCurrent()
                Keys.onEnterPressed: root.runTopOrCurrent()
                Keys.onDownPressed: {
                    if (root.searching) {
                        if (resultList.count > 0) {
                            resultList.currentIndex = Math.min(1, resultList.count - 1)
                            resultList.moved = true
                            root.backend.select(resultList.model.idAt(resultList.currentIndex))
                            resultList.forceActiveFocus(Qt.TabFocusReason)
                        }
                    } else if (root.startMode) {
                        startPage.focusFirst()
                    }
                }
            }

            Item {
                id: body

                anchors.top: field.bottom
                anchors.topMargin: TelamonStyle.spacingLarge
                anchors.bottom: parent.bottom
                width: parent.width

                LauncherStartPage {
                    id: startPage
                    anchors.fill: parent
                    visible: root.startMode && (!root.searching || !root.answered)
                    backend: root.backend
                    actions: root.actions
                    options: root.options
                    menu: itemMenu
                    pins: root.pins
                    apps: root.apps
                    recentApps: root.recentApps
                    recentFiles: root.recentFiles
                    onBackToField: root.backToField()
                }

                LauncherResultList {
                    id: resultList
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    anchors.left: parent.left
                    width: parent.width
                    visible: root.searching && root.answered
                    locked: root.needsInteraction
                    model: root.results
                    backend: root.backend
                    actions: root.actions
                    menu: itemMenu
                    onBackToField: root.backToField()
                    onCountChanged: if (root.searching && root.shown) announce.restart()
                }

                Connections {
                    target: root.pins
                    function onCountChanged() { resultList.pinsVersion += 1 }
                }

                TelamonLabel {
                    anchors.centerIn: parent
                    visible: root.searching && root.answered && resultList.count === 0
                    text: qsTr("No results")
                    textStyle: TelamonLabel.Caption
                }

                // Why the last row did nothing; fades with the next show.
                TelamonLabel {
                    id: failure
                    anchors.bottom: parent.bottom
                    anchors.horizontalCenter: parent.horizontalCenter
                    visible: false
                    textStyle: TelamonLabel.Caption
                    Accessible.role: Accessible.AlertMessage
                    onVisibleChanged: if (visible) Accessible.announce(text)
                }
            }
        }
    }

    Timer {
        id: failureTimer
        interval: 4000
        onTriggered: failure.visible = false
    }

    // After the results settle, a screen reader hears the count and the top
    // hit. Runs only while the panel is shown and the user types.
    Timer {
        id: announce
        interval: 500
        onTriggered: {
            if (!root.searching || !root.shown) {
                return
            }
            const top = resultList.itemAtIndex(0) as LauncherResultRow
            const count = resultList.count
            const text = count === 0 ? qsTr("No results")
                : top ? qsTr("%n result(s). Top hit: %1, %2", "", count).arg(top.model.title).arg(top.kindLabel)
                : qsTr("%n result(s)", "", count)
            resultList.Accessible.announce(text)
        }
    }
}
