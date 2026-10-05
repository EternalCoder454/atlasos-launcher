cmake_minimum_required(VERSION 3.24)
project(atlas-launcher VERSION 0.1.1 LANGUAGES CXX)

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
# atlas-framework-ui's startup code (single instance, window activation), the
# panel (layer shell, blur), KRunner's plugins, KIO's launch jobs, the global
# shortcuts and the app catalogue (KService).
# One find_package per framework: there is no KF6 umbrella package without ECM.
foreach(framework DBusAddons WindowSystem Runner KIO GlobalAccel Config Service CoreAddons)
    find_package(KF6${framework} 6.20 REQUIRED)
endforeach()
find_package(LayerShellQt 6.4 REQUIRED)
find_package(Corrosion REQUIRED)

set(WORKSPACE_DIR ${CMAKE_CURRENT_SOURCE_DIR}/../..)

# Rust library (CXX-Qt QObjects, atlas-framework, atlas-launcher-core).
# cxx-qt-build finds Qt via $QMAKE.
find_program(QMAKE6 NAMES qmake6 qmake REQUIRED)
corrosion_import_crate(MANIFEST_PATH ${WORKSPACE_DIR}/Cargo.toml
    CRATES atlas-launcher LOCKED)
corrosion_set_env_vars(atlas_launcher QMAKE=${QMAKE6})

# The framework's C header (atlas/app.h) comes with the crate's sources,
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
set(ATLAS_UI_CRATE_DIR "")
foreach(i RANGE ${LAST_PACKAGE})
    string(JSON PKG_NAME GET "${CARGO_METADATA}" packages ${i} name)
    if(PKG_NAME STREQUAL "atlas-framework-ui")
        string(JSON PKG_MANIFEST GET "${CARGO_METADATA}" packages ${i} manifest_path)
        get_filename_component(ATLAS_UI_CRATE_DIR "${PKG_MANIFEST}" DIRECTORY)
    endif()
endforeach()
if(NOT EXISTS "${ATLAS_UI_CRATE_DIR}/include/atlas/app.h")
    message(FATAL_ERROR "atlas-framework-ui's include/atlas/app.h not found (looked in '${ATLAS_UI_CRATE_DIR}')")
endif()

# Atlas.Ui, the shared Atlas look, is installed like Kirigami (atlas-ui, from
# atlas-framework): QML imports it and nothing links it.
if(NOT EXISTS "${QT6_INSTALL_PREFIX}/${QT6_INSTALL_QML}/Atlas/Ui/qmldir")
    message(FATAL_ERROR "Atlas.Ui is not installed: install atlas-ui (atlas-framework)")
endif()

qt_add_executable(atlas-launcher-app
    cpp/main.cpp
    cpp/panel.cpp cpp/panel.h
    cpp/dbus.cpp cpp/dbus.h
    cpp/catalog.cpp cpp/catalog.h
    cpp/options.cpp cpp/options.h
    cpp/runners.cpp cpp/runners.h
    cpp/executor.cpp cpp/executor.h
    cpp/actions.cpp cpp/actions.h)
set_target_properties(atlas-launcher-app PROPERTIES OUTPUT_NAME atlas-launcher)
target_include_directories(atlas-launcher-app PRIVATE "${ATLAS_UI_CRATE_DIR}/include")

# qmlcachegen compiles every QML file to C++ at build time; the app reads no
# .qml from disk at run time.
qt_add_qml_module(atlas-launcher-app
    URI net.eterneon.atlas.launcher
    VERSION 1.0
    QML_FILES
        qml/Panel.qml
        qml/LauncherResultList.qml
        qml/LauncherResultRow.qml
        qml/LauncherStartPage.qml
        qml/LauncherAppTile.qml
        qml/LauncherItemMenu.qml
        qml/LauncherFooter.qml
        qml/LauncherLetterGrid.qml
        qml/LauncherItemIcon.qml
        qml/LauncherDetailPane.qml
)

target_link_libraries(atlas-launcher-app PRIVATE
    atlas_launcher
    Qt6::Core Qt6::DBus Qt6::Gui Qt6::Qml Qt6::Quick Qt6::QuickControls2 Qt6::Widgets
    KF6::DBusAddons KF6::WindowSystem KF6::Runner KF6::KIOGui KF6::GlobalAccel
    KF6::ConfigCore KF6::ConfigGui KF6::Service KF6::CoreAddons KF6::KIOCore
    LayerShellQt::Interface)

install(TARGETS atlas-launcher-app RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
install(FILES data/net.eterneon.atlas.launcher.desktop
        DESTINATION ${CMAKE_INSTALL_DATADIR}/applications)
install(FILES data/net.eterneon.atlas.launcher.service
        DESTINATION ${CMAKE_INSTALL_DATADIR}/dbus-1/services)
install(FILES data/atlas-launcher.service
        DESTINATION ${CMAKE_INSTALL_PREFIX}/lib/systemd/user)
install(FILES data/pinned.list
        DESTINATION ${CMAKE_INSTALL_FULL_SYSCONFDIR}/xdg/atlas-launcher)
install(DIRECTORY ${WORKSPACE_DIR}/plasmoid/net.eterneon.atlas.launcher.button
        DESTINATION ${CMAKE_INSTALL_DATADIR}/plasma/plasmoids)
