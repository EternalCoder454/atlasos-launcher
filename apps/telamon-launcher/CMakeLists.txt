cmake_minimum_required(VERSION 3.24)
project(telamon-launcher VERSION 0.3.1 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

if(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT)
    set(CMAKE_INSTALL_PREFIX /usr CACHE PATH "" FORCE)
endif()
include(GNUInstallDirs)
find_package(Qt6 6.11 REQUIRED COMPONENTS Core DBus Gui Qml Quick QuickControls2 Widgets)
qt_policy(SET QTP0001 NEW)
qt_policy(SET QTP0004 NEW)
# telamon-framework-ui's startup code (single instance, window activation), the
# panel (layer shell, blur), KRunner's plugins, KIO's launch jobs, the global
# shortcuts and the app catalogue (KService).
# One find_package per framework: there is no KF6 umbrella package without ECM.
foreach(framework DBusAddons WindowSystem Runner KIO GlobalAccel Config Service CoreAddons)
    find_package(KF6${framework} 6.20 REQUIRED)
endforeach()
find_package(LayerShellQt 6.4 REQUIRED)
find_package(Corrosion REQUIRED)

set(WORKSPACE_DIR ${CMAKE_CURRENT_SOURCE_DIR}/../..)

# Rust library (CXX-Qt QObjects, atlas-framework, telamon-launcher-core).
# cxx-qt-build finds Qt via $QMAKE.
find_program(QMAKE6 NAMES qmake6 qmake REQUIRED)
corrosion_import_crate(MANIFEST_PATH ${WORKSPACE_DIR}/Cargo.toml
    CRATES telamon-launcher LOCKED)
corrosion_set_env_vars(telamon_launcher QMAKE=${QMAKE6})

# The framework's C header (telamon/app.h) comes with the crate's sources,
# wherever cargo put them (a git checkout in ~/.cargo).
find_program(CARGO NAMES cargo REQUIRED)
execute_process(
    COMMAND ${CARGO} metadata --format-version 1 --locked --manifest-path ${WORKSPACE_DIR}/Cargo.toml
    OUTPUT_VARIABLE CARGO_METADATA
    RESULT_VARIABLE CARGO_METADATA_RESULT
    ERROR_VARIABLE CARGO_METADATA_ERROR
    TIMEOUT 300)
if(NOT CARGO_METADATA_RESULT EQUAL 0)
    message(FATAL_ERROR "cargo metadata failed (${CARGO_METADATA_RESULT}): ${CARGO_METADATA_ERROR}\n"
        "Run `cargo fetch --locked` first, or check the network.")
endif()
string(JSON NPACKAGES LENGTH "${CARGO_METADATA}" packages)
math(EXPR LAST_PACKAGE "${NPACKAGES} - 1")
set(TELAMON_UI_CRATE_DIR "")
foreach(i RANGE ${LAST_PACKAGE})
    string(JSON PKG_NAME GET "${CARGO_METADATA}" packages ${i} name)
    if(PKG_NAME STREQUAL "telamon-framework-ui")
        string(JSON PKG_MANIFEST GET "${CARGO_METADATA}" packages ${i} manifest_path)
        get_filename_component(TELAMON_UI_CRATE_DIR "${PKG_MANIFEST}" DIRECTORY)
    endif()
endforeach()
if(NOT EXISTS "${TELAMON_UI_CRATE_DIR}/include/telamon/app.h")
    message(FATAL_ERROR "telamon-framework-ui's include/telamon/app.h not found (looked in '${TELAMON_UI_CRATE_DIR}')")
endif()

# Telamon.Ui, the shared Telamon look, is installed like Kirigami (telamon-ui, from
# atlas-framework): QML imports it and nothing links it.
if(NOT EXISTS "${QT6_INSTALL_PREFIX}/${QT6_INSTALL_QML}/Telamon/Ui/qmldir")
    message(FATAL_ERROR "Telamon.Ui is not installed: install telamon-ui (atlas-framework)")
endif()

qt_add_executable(telamon-launcher-app
    cpp/main.cpp
    cpp/panel.cpp cpp/panel.h
    cpp/dbus.cpp cpp/dbus.h
    cpp/catalog.cpp cpp/catalog.h
    cpp/options.cpp cpp/options.h
    cpp/runners.cpp cpp/runners.h
    cpp/executor.cpp cpp/executor.h
    cpp/actions.cpp cpp/actions.h)
set_target_properties(telamon-launcher-app PROPERTIES OUTPUT_NAME telamon-launcher)
target_include_directories(telamon-launcher-app PRIVATE "${TELAMON_UI_CRATE_DIR}/include")

# qmlcachegen compiles every QML file to C++ at build time; the app reads no
# .qml from disk at run time.
qt_add_qml_module(telamon-launcher-app
    URI net.eterneon.telamon.launcher
    VERSION 1.0
    QML_FILES
        qml/Panel.qml
        qml/LauncherResultList.qml
        qml/LauncherResultRow.qml
        qml/LauncherStartPage.qml
        qml/LauncherAppTile.qml
        qml/LauncherItemMenu.qml
        qml/LauncherRenameEditor.qml
        qml/LauncherChip.qml
        qml/LauncherAppSection.qml
        qml/LauncherPowerButton.qml
        qml/LauncherAccountButton.qml
        qml/LauncherLetterGrid.qml
        qml/LauncherItemIcon.qml
)

target_link_libraries(telamon-launcher-app PRIVATE
    telamon_launcher
    Qt6::Core Qt6::DBus Qt6::Gui Qt6::Qml Qt6::Quick Qt6::QuickControls2 Qt6::Widgets
    KF6::DBusAddons KF6::WindowSystem KF6::Runner KF6::KIOGui KF6::GlobalAccel
    KF6::ConfigCore KF6::ConfigGui KF6::Service KF6::CoreAddons KF6::KIOCore
    LayerShellQt::Interface)

install(TARGETS telamon-launcher-app RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
install(FILES data/net.eterneon.telamon.launcher.desktop
        DESTINATION ${CMAKE_INSTALL_DATADIR}/applications)
install(FILES data/net.eterneon.telamon.launcher.service
        DESTINATION ${CMAKE_INSTALL_DATADIR}/dbus-1/services)
# The names before the rename (Atlas Launcher), for this release: a hidden
# desktop file, the old D-Bus name's activation file, and, in the RPM
# (packaging/telamon-launcher.spec), links for the binary and the user unit.
install(FILES data/net.eterneon.atlas.launcher.desktop
        DESTINATION ${CMAKE_INSTALL_DATADIR}/applications)
install(FILES data/net.eterneon.atlas.launcher.service
        DESTINATION ${CMAKE_INSTALL_DATADIR}/dbus-1/services)
install(FILES data/telamon-launcher.service
        DESTINATION ${CMAKE_INSTALL_PREFIX}/lib/systemd/user)
install(FILES data/pinned.list
        DESTINATION ${CMAKE_INSTALL_FULL_SYSCONFDIR}/xdg/telamon-launcher)
install(DIRECTORY ${WORKSPACE_DIR}/plasmoid/net.eterneon.telamon.launcher.button
        DESTINATION ${CMAKE_INSTALL_DATADIR}/plasma/plasmoids)
# Swaps the dock button of the old ID for this one in existing panels, once per
# user (Plasma's update scripts; docs/DESIGN.md, "Names before the rename").
install(FILES ${WORKSPACE_DIR}/plasma-updates/telamon-20261007-launcher-button.js
        DESTINATION ${CMAKE_INSTALL_DATADIR}/plasma/shells/org.kde.plasma.desktop/contents/updates)
