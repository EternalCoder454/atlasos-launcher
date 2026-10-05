//! Rust side of the launcher: the QObjects QML sees, over
//! `atlas-launcher-core`. The C++ in `cpp/` holds what only KDE's C++
//! libraries offer (layer shell, KRunner, KIO, KGlobalAccel).

mod backend;
mod state;

atlas_framework_ui::app! {
    name: "AtlasOS Launcher",
    id: "net.eterneon.atlas.launcher",
    repo: "atlasos-launcher",
    ui: "1.4.0",
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
pub extern "C" fn atlas_launcher_objects_new() -> LauncherObjects {
    let (backend, models) = backend::make_objects();
    LauncherObjects {
        backend: backend.cast(),
        models: models.map(|m| m.cast()),
    }
}
