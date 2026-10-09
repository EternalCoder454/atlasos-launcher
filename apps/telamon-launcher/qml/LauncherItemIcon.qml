import QtQuick
import org.kde.kirigami as Kirigami

// A row's icon: a theme icon name, or an absolute path made a file URL here
// (each path segment escaped, so '#' or '?' in a name can't cut the URL).
//
// A theme name is a Kirigami.Icon. An absolute path is an Image with
// `asynchronous: true`: the file is opened by the image loader thread, never by
// the GUI thread. The paths are checked to be plain files when the catalogue is
// built (catalog.cpp, engine.rs), but a file can be swapped for a pipe after
// that, and open() on a pipe waits for a writer for good: on the loader thread
// that stalls the loading of other file icons (they keep their generic icon),
// not the panel. A Kirigami.Icon reads its file on the GUI thread, so it is
// never given a path. docs/SECURITY.md, "Icons".
//
// The Kirigami.Icon has a layer with Quick's software renderer: it is a render
// node there, and a repaint touching part of it paints the whole icon again over
// the popups and dialogs in front of it. No layer with OpenGL and the other
// backends. The layer is live only for a moment after the icon changes (its
// source, colour, size, state or theme; see _refresh()): a live layer on the
// software renderer is drawn again on every frame, which kept the daemon busy
// even with its window hidden. Same as Telamon.Ui's TelamonIcon.
Item {
    id: root

    // The model's `icon` role.
    property string name: ""

    readonly property bool fromFile: name.startsWith("/")

    function fileUrl(path) {
        return "file://" + path.split("/").map(encodeURIComponent).join("/")
    }

    // Decorative: the row around it carries the name.
    Accessible.ignored: true

    Kirigami.Icon {
        id: icon

        anchors.fill: parent
        // Until a file icon has loaded (or if it cannot be), the generic one.
        visible: !picture.ready

        layer.enabled: GraphicsInfo.api === GraphicsInfo.Software
        layer.live: false

        // Draws the layer again: live for a few frames, so the icon's own
        // changes (several in a row, an image that arrives later) are in it.
        function _refresh(): void {
            if (layer.enabled) {
                layer.live = true;
                refreshTimer.restart();
            }
        }

        onSourceChanged: _refresh()
        onColorChanged: _refresh()
        onWidthChanged: _refresh()
        onHeightChanged: _refresh()
        onIsMaskChanged: _refresh()
        onActiveChanged: _refresh()
        onSelectedChanged: _refresh()
        onEnabledChanged: _refresh()
        onVisibleChanged: _refresh()
        onValidChanged: _refresh()
        onStatusChanged: _refresh()
        onFallbackChanged: _refresh()
        onPlaceholderChanged: _refresh()
        onPaintedWidthChanged: _refresh()
        onPaintedHeightChanged: _refresh()
        onWindowChanged: _refresh()
        Kirigami.Theme.onColorsChanged: _refresh()
        Component.onCompleted: _refresh()

        Timer {
            id: refreshTimer
            interval: 100
            onTriggered: icon.layer.live = false
        }

        source: root.fromFile ? "application-x-executable" : (root.name.length > 0 ? root.name : "application-x-executable")
        fallback: "application-x-executable"
    }

    Image {
        id: picture

        readonly property bool ready: root.fromFile && status === Image.Ready

        anchors.fill: parent
        visible: ready
        asynchronous: true
        smooth: true
        fillMode: Image.PreserveAspectFit
        // Decoded no larger than it is drawn (at twice the size, for scaling).
        sourceSize.width: Math.max(1, Math.ceil(root.width * 2))
        sourceSize.height: Math.max(1, Math.ceil(root.height * 2))
        source: root.fromFile ? root.fileUrl(root.name) : ""
    }
}
