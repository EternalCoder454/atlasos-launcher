import QtQuick
import org.kde.kirigami as Kirigami

// A row's icon: a theme icon name, or an absolute path made a file URL here
// (each path segment escaped, so '#' or '?' in a name can't cut the URL).
// A layer with Quick's software renderer: a Kirigami.Icon is a render node
// there, and a repaint touching part of it paints the whole icon again over the
// popups and dialogs in front of it. No layer with OpenGL and the other backends.
//
// The layer is live only for a moment after the icon changes (its source,
// colour, size, state or theme; see _refresh()): a live layer on the software
// renderer is drawn again on every frame, which kept the daemon busy even
// with its window hidden. Same as Telamon.Ui's TelamonIcon.
Kirigami.Icon {
    id: icon

    layer.enabled: GraphicsInfo.api === GraphicsInfo.Software
    layer.live: false

    // Draws the layer again: live for a few frames, so the icon's own changes
    // (several in a row, an image that arrives later) are in it.
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

    // The model's `icon` role.
    property string name: ""

    function fileUrl(path) {
        return "file://" + path.split("/").map(encodeURIComponent).join("/")
    }

    source: name.startsWith("/") ? fileUrl(name) : (name.length > 0 ? name : "application-x-executable")
    fallback: "application-x-executable"
    // Decorative: the row around it carries the name.
    Accessible.ignored: true
}
