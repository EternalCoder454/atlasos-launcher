//! Rust side of the launcher: the QObjects QML sees, over
//! `atlas-launcher-core`. The C++ in `cpp/` holds what only KDE's C++
//! libraries offer (layer shell, KRunner, KIO, KGlobalAccel).

mod backend;

atlas_framework_ui::app! {
    name: "AtlasOS Launcher",
    id: "net.eterneon.atlas.launcher",
    repo: "atlasos-launcher",
    ui: "1.4.0",
}

use std::ffi::c_void;

/// Called once from `main.cpp`. Returns the `Backend` QObject, which C++ hands
/// to the QML engine. Ownership passes to the caller (a QObject with no parent).
#[unsafe(no_mangle)]
pub extern "C" fn atlas_backend_new() -> *mut c_void {
    backend::qobject::backend_make_unique().into_raw().cast()
}
