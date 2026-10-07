var o = [];
panels().forEach(function (p) {
    var ids = p.widgetIds;
    var parts = [];
    for (var i = 0; i < ids.length; i++) {
        var w = p.widgetById(ids[i]);
        parts.push(ids[i] + "=" + (w ? w.type : "?"));
    }
    p.currentConfigGroup = ["General"];
    o.push("panel " + p.id + " loc=" + p.location + " order=" + p.readConfig("AppletOrder", "") + ": " + parts.join(", "));
});
o.push("desktops " + desktops().length);
print(o.join("\n"));
