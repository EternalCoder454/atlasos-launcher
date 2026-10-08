import QtQuick
import org.kde.kirigami as Kirigami

// A row's icon: a theme icon name, or an absolute path made a file URL here
// (each path segment escaped, so '#' or '?' in a name can't cut the URL).
// A layer with Quick's software renderer: a Kirigami.Icon is a render node
// there, and a repaint touching part of it paints the whole icon again over the
// popups and dialogs in front of it. No layer with OpenGL and the other backends.
Kirigami.Icon {
    id: icon

    layer.enabled: GraphicsInfo.api === GraphicsInfo.Software

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
