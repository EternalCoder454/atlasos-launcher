import QtQuick
import org.kde.kirigami as Kirigami

// A row's icon: a theme icon name, or an absolute path made a file URL here
// (each path segment escaped, so '#' or '?' in a name can't cut the URL).
Kirigami.Icon {
    id: icon

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
