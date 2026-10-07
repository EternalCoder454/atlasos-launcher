var o = [];
function walk(w, path, depth) {
    w.currentConfigGroup = path;
    var keys = w.configKeys;
    for (var i = 0; i < keys.length; i++) {
        o.push("  " + (path.join("/") || "(root)") + " " + keys[i] + "=" + w.readConfig(keys[i], ""));
    }
    if (depth > 0) {
        var gs = w.configGroups;
        for (var g = 0; g < gs.length; g++) { walk(w, path.concat([gs[g]]), depth - 1); }
    }
}
panels().forEach(function (p) {
    for (var i = 0; i < p.widgetIds.length; i++) {
        var w = p.widgetById(p.widgetIds[i]);
        if (w.type.indexOf("launcher") >= 0) {
            o.push("widget " + w.id + " " + w.type + " index=" + w.index + " shortcut=" + w.globalShortcut);
            walk(w, [], 3);
        }
    }
});
print(o.join("\n"));
