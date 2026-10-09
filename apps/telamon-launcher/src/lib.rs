//! Rust side of the launcher: the QObjects QML sees, over
//! `telamon-launcher-core`. The C++ in `cpp/` holds what only KDE's C++
//! libraries offer (layer shell, KRunner, KIO, KGlobalAccel).

mod backend;
mod state;

telamon_framework_ui::app! {
    name: "Telamon Launcher",
    id: "net.eterneon.telamon.launcher",
    repo: "atlasos-launcher",
    ui: "2.0.0",
}

use std::ffi::c_void;

// cxx-qt-build's generated initializer calls into cxx-qt-lib; keep the crate
// linked.
extern crate cxx_qt_lib;

/// The QObjects QML sees, handed to the engine as `Panel.qml`'s initial
/// properties. `models` are in `state::List::ALL` order: results, pins,
/// apps, recent apps, recent files.
#[repr(C)]
pub struct LauncherObjects {
    pub backend: *mut c_void,
    pub models: [*mut c_void; 5],
}

/// Called once from `main.cpp`. The caller owns every object (QObjects with
/// no parent): delete the backend first, which stops the engine, then the
/// models.
#[unsafe(no_mangle)]
pub extern "C" fn telamon_launcher_objects_new() -> LauncherObjects {
    let (backend, models) = backend::make_objects();
    LauncherObjects {
        backend: backend.cast(),
        models: models.map(|m| m.cast()),
    }
}

/// Whether an absolute icon path may be handed to the image loaders: a
/// non-empty regular file of at most 16 MiB, not on a pseudo file system
/// (`telamon_launcher_core::text::icon_file_ok`). Called from `catalog.cpp`
/// on a pool thread, never on the GUI thread (it does `stat`s).
///
/// # Safety
/// `path` must be null or point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn telamon_launcher_icon_ok(path: *const std::ffi::c_char) -> bool {
    if path.is_null() {
        return false;
    }
    // SAFETY: the caller promises a NUL-terminated string.
    let c = unsafe { std::ffi::CStr::from_ptr(path) };
    c.to_str()
        .is_ok_and(telamon_launcher_core::text::icon_file_ok)
}

/// Called from `main.cpp` first thing, before anything reads the user's
/// files: moves the folders of the old name (`~/.config/atlas-launcher`,
/// `~/.local/state/atlas-launcher`, `atlas-launcherstaterc`) to the new ones,
/// once (`telamon_launcher_core::legacy`). Returns how many moved, or -1
/// when something could not be moved.
#[unsafe(no_mangle)]
pub extern "C" fn telamon_launcher_migrate_user_files() -> i32 {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute());
    let dir = |var: &str, fallback: &str| {
        std::env::var_os(var)
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.as_ref().map(|h| h.join(fallback)))
    };
    let (Some(config), Some(state)) = (
        dir("XDG_CONFIG_HOME", ".config"),
        dir("XDG_STATE_HOME", ".local/state"),
    ) else {
        return -1;
    };
    let m = telamon_launcher_core::legacy::migrate_user_files(&config, &state);
    if m.moved > 0 || m.failed > 0 {
        log::info!(
            "moved {} file(s) of the old name, {} could not be moved",
            m.moved,
            m.failed
        );
    }
    if m.failed > 0 { -1 } else { m.moved as i32 }
}
