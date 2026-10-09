//! Guards on the C++ sources (docs/SECURITY.md): rules the code keeps by how it
//! is written, which a refactor could break without a test noticing. They read
//! the source text, so a change that skips a rule fails here, in `cargo test`,
//! and not in a review three releases later. The checks that can run the code
//! (the strings it accepts) are in `tests/validate_test.cpp`, run by ctest; the
//! D-Bus surface is probed on a real bus by `scripts/headless-dbus-security.sh`.

use std::fs;
use std::path::PathBuf;

fn cpp(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("cpp")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn qml(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("qml")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The text of the function whose signature line contains `head`, up to the
/// closing brace at column 0.
fn function<'a>(src: &'a str, head: &str) -> &'a str {
    let start = src
        .find(head)
        .unwrap_or_else(|| panic!("no function `{head}`"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |e| e + 3);
    &rest[..end]
}

fn count(src: &str, needle: &str) -> usize {
    src.matches(needle).count()
}

const SOURCES: &[&str] = &[
    "actions.cpp",
    "catalog.cpp",
    "dbus.cpp",
    "executor.cpp",
    "main.cpp",
    "options.cpp",
    "panel.cpp",
    "runners.cpp",
];

#[test]
fn nothing_builds_a_shell_command_in_cpp() {
    // The one place a typed line goes to a shell is `startInTerminal`, through
    // KTerminalLauncherJob. Nothing else may start a process by itself.
    for name in SOURCES {
        let src = cpp(name);
        for bad in [
            "QProcess",
            "KProcess",
            "system(",
            "popen(",
            "execl",
            "execv",
            "posix_spawn",
            "fork(",
            "/bin/sh",
            "sh -c",
            "bash -c",
            "setShellCommand",
            "QDesktopServices",
            "QFile::link",
            "::symlink(",
        ] {
            assert!(!src.contains(bad), "{name} uses {bad}");
        }
    }
    let exec = cpp("executor.cpp");
    assert_eq!(count(&exec, "KTerminalLauncherJob("), 1);
    assert!(function(&exec, "bool Executor::startInTerminal").contains("KTerminalLauncherJob("));
    for name in SOURCES.iter().filter(|n| **n != "executor.cpp") {
        assert!(!cpp(name).contains("KTerminalLauncherJob"), "{name}");
    }
}

#[test]
fn every_launch_goes_through_a_kio_job_with_the_checks_before_it() {
    let exec = cpp("executor.cpp");
    // Apps: the id is a plain desktop file id before KService sees it (KService
    // takes an absolute path as an id and loads whatever desktop file is there).
    let launch = function(&exec, "bool Executor::launchApplication");
    let check = launch
        .find("Validate::desktopId(desktopId)")
        .expect("id check");
    let lookup = launch
        .find("serviceByStorageId(desktopId)")
        .expect("lookup");
    assert!(check < lookup, "the id is checked before the lookup");
    // Every other lookup is of a constant.
    for line in exec
        .lines()
        .chain(cpp("actions.cpp").lines())
        .chain(cpp("catalog.cpp").lines())
    {
        if line.contains("serviceByStorageId(") && !line.contains("serviceByStorageId(desktopId)") {
            assert!(
                line.contains("kSettingsDesktopId")
                    || line.contains("kLegacySettingsDesktopId")
                    || line.contains("QStringLiteral(")
                    || line.contains("terminal)")
                    || line.contains("storageId"),
                "a lookup of something that is not a constant or a checked id: {line}"
            );
        }
    }
    // Files and URLs: never run as programs, always checked first.
    assert_eq!(
        count(&exec, "KIO::OpenUrlJob("),
        count(&exec, "setRunExecutables(false)")
    );
    assert!(!exec.contains("setRunExecutables(true)"));
    assert!(function(&exec, "bool Executor::openLocalFile").contains("checkedFileUrl(uri)"));
    assert!(function(&exec, "bool Executor::openWebUrl").contains("Validate::webUrl("));
    assert!(function(&exec, "bool Executor::showInFolder").contains("checkedFileUrl(uri)"));
    // Commands: an absolute path and an argument vector, never a command line.
    let command = function(&exec, "bool Executor::startCommand");
    assert!(command.contains("Validate::commandPath(executable"));
    assert!(command.contains("KIO::CommandLauncherJob(executable, args, this)"));
    assert_eq!(count(&exec, "CommandLauncherJob("), 1);
    // Settings links.
    assert!(
        function(&exec, "void Executor::openSettings").contains("Validate::settingsLink(link)")
    );
    // The Store is run by an absolute path and a validated Flatpak id.
    let actions = cpp("actions.cpp");
    let uninstall = function(&actions, "void Actions::uninstall");
    assert!(uninstall.contains("Validate::flatpakId(flatpakId)"));
    assert!(uninstall.contains("QStringLiteral(\"/usr/bin\")"));
    assert!(uninstall.contains("{QStringLiteral(\"--remove\"), flatpakId}"));
    assert!(function(&actions, "void Actions::openAppSettings").contains("Validate::desktopId("));
}

#[test]
fn the_dbus_adaptors_export_methods_and_nothing_else() {
    // An adaptor's public slots are all D-Bus methods and its signals are all
    // sent on the bus: the adaptors have neither beyond the documented ones.
    let h = cpp("dbus.h");
    assert!(
        !h.contains("Q_SIGNALS"),
        "an adaptor signal is exported on the bus"
    );
    assert!(!h.contains("Q_EMIT"));
    let slots: Vec<&str> = h
        .lines()
        .filter(|l| l.trim_start().starts_with("void ") || l.trim_start().starts_with("bool "))
        .collect();
    let exported = h
        .split("public Q_SLOTS:")
        .skip(1)
        .map(|part| part.split(['}', ':']).next().unwrap_or(""))
        .collect::<String>();
    for method in [
        "ToggleStart()",
        "ToggleStart(const QVariantMap &options)",
        "ToggleSearch()",
        "Show(",
        "Hide()",
        "SetDockAnchor(",
        "ImportPins(",
        "ClearHistory()",
    ] {
        assert!(exported.contains(method), "{method}");
    }
    assert!(
        !exported.contains("queueClearHistory"),
        "internal, not a slot"
    );
    assert!(!exported.contains("setBackend"));
    assert!(slots.len() >= 8);
    // The legacy name is registered without queueing or allowing replacement
    // (the defaults of registerService): no flags are passed.
    let main = cpp("main.cpp");
    assert!(main.contains("bus.registerService(QStringLiteral(\"net.eterneon.atlas.launcher\"))"));
    assert!(main.contains("KDBusService service(KDBusService::Unique)"));
    assert!(!main.contains("ReplaceExistingService"));
    assert!(!main.contains("AllowReplacement"));
    // Neither adaptor reaches an object that runs something.
    let c = cpp("dbus.cpp");
    for bad in ["launchApp", "runCommand", "openFile", "Executor", "KIO::"] {
        assert!(!c.contains(bad), "dbus.cpp mentions {bad}");
    }
    // The backend's invokables are called by name, from the adaptor, only for
    // these two.
    assert_eq!(count(&c, "QMetaObject::invokeMethod(m_backend"), 2);
    assert!(c.contains("\"importPins\"") && c.contains("\"clearHistory\""));
}

#[test]
fn text_handed_to_the_panel_is_cleaned_before_the_field_shows_it() {
    let panel = qml("Panel.qml");
    assert!(panel.contains("root.backend.cleanQuery(query)"));
    assert!(panel.contains("field.text = cleaned"));
    assert!(!panel.contains("field.text = query"));
    // The D-Bus Show and the command line cap what they hand over.
    assert!(cpp("dbus.cpp").contains("query.left(kMaxQuery)"));
    assert!(cpp("main.cpp").contains(".left(256)"));
}

#[test]
fn catalogue_icons_and_the_real_name_are_vetted_before_use() {
    let catalog = cpp("catalog.cpp");
    assert!(catalog.contains("QString vettedIcon("));
    assert!(catalog.contains("telamon_launcher_icon_ok(QFile::encodeName(icon).constData())"));
    assert!(catalog.contains("vettedIcon(map.value(QStringLiteral(\"icon\")).toString())"));
    assert!(catalog.contains("vettedIcon(actionIcon)"));
    // The marker is honoured only in the user's own applications folder.
    assert!(catalog.contains(
        "writableLocation(QStandardPaths::GenericDataLocation) + QLatin1String(\"/applications/\")"
    ));
    // The Start page's saved view is read from a file only after C++ looked at it.
    assert!(qml("LauncherStartPage.qml").contains("location: page.options.stateConfig"));
    assert!(!qml("LauncherStartPage.qml").contains("state.conf\""));
    let options = cpp("options.cpp");
    let state = function(&options, "QUrl Options::stateConfig() const");
    assert!(state.contains("Validate::plainSmallFile(path, kMaxStateBytes)"));
    assert!(state.contains("/dev/null"));
    // KRunner's state file is opened only after C++ looked at it.
    let runners = cpp("runners.cpp");
    assert!(runners.contains("Validate::plainSmallFile(path, 64 * 1024)"));
    assert!(runners.contains("const KConfigGroup state(runnerStateConfig(),"));
    assert!(!runners.contains("openStateConfig(QStringLiteral(\"telamon-launcherstaterc\"))"));
    // The user's real name is cleaned before it is shown.
    let actions = cpp("actions.cpp");
    assert!(actions.contains(
        "Validate::displayText(values.value().value(QStringLiteral(\"RealName\")).toString(), 256)"
    ));
}

#[test]
fn the_launcher_logs_no_text_from_outside() {
    // Log lines carry timings, counts and error kinds. These are the only
    // places a string other than a literal is streamed into the log; each is a
    // D-Bus error *name*, a numeric code or a fixed set of kinds.
    for name in SOURCES {
        let src = cpp(name);
        for (n, line) in src.lines().enumerate() {
            if !(line.contains("qCDebug(")
                || line.contains("qCInfo(")
                || line.contains("qCWarning(")
                || line.contains("qCCritical(")
                || line.contains("qDebug(")
                || line.contains("qWarning("))
            {
                continue;
            }
            for streamed in [
                "query",
                "text",
                "path",
                "url",
                "uri",
                "desktopId",
                "executable",
                "args",
                "line",
                "name()",
                ".left(64)",
            ] {
                let ok = line.contains("kind.left(64)") // the backend's error kind, a fixed enum name
                    || line.contains("w->error().name()") // a D-Bus error name
                    || line.contains("action->objectName()"); // a fixed action id
                if line.contains(&format!("<< {streamed}"))
                    || line.contains(&format!("<< m_{streamed}"))
                {
                    assert!(ok, "{name}:{}: logs `{streamed}`: {line}", n + 1);
                }
            }
        }
    }
    // The Rust side: log macros take counts, kinds and timings.
    for dir in ["../../crates/telamon-launcher-core/src", "src"] {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(dir);
        for entry in fs::read_dir(&base).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = fs::read_to_string(&path).unwrap();
            let code = src.split("#[cfg(test)]").next().unwrap();
            for (n, line) in code.lines().enumerate() {
                if !(line.contains("log::warn!")
                    || line.contains("log::info!")
                    || line.contains("log::debug!")
                    || line.contains("log::error!")
                    || line.contains("log::trace!"))
                {
                    continue;
                }
                // The only values formatted: kinds, counts, errors' kinds.
                for bad in [
                    "{query", "{text", "{path", "{name", "{id", "{uri", "{url", "{line", "{title",
                ] {
                    assert!(
                        !line.contains(bad),
                        "{}:{}: logs a value that may be text from outside: {line}",
                        path.display(),
                        n + 1
                    );
                }
            }
        }
    }
}
