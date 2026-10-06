# AtlasOS Launcher: the Start menu and search of AtlasOS.

# No debuginfo subpackage: the Rust flags below keep symbols (debuginfo=2,
# strip=none) and the binary is shipped as built.
%global debug_package %{nil}

Name:           atlas-launcher
Version:        0.2.0
Release:        1%{?dist}
Summary:        AtlasOS Launcher, the Start menu and search of AtlasOS
License:        MIT
URL:            https://github.com/EternalCoder454/atlasos-launcher
Source0:        atlas-launcher-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
# %%build_rustflags
BuildRequires:  rust-srpm-macros
BuildRequires:  gcc
BuildRequires:  gcc-c++
BuildRequires:  cmake
BuildRequires:  ninja-build
BuildRequires:  corrosion
# Cargo fetches the atlas-framework crates from GitHub.
BuildRequires:  git-core
BuildRequires:  desktop-file-utils
BuildRequires:  systemd-rpm-macros
BuildRequires:  cmake(Qt6Core)
BuildRequires:  cmake(Qt6DBus)
BuildRequires:  cmake(Qt6Gui)
BuildRequires:  cmake(Qt6Qml)
BuildRequires:  cmake(Qt6Quick)
BuildRequires:  cmake(Qt6QuickControls2)
BuildRequires:  cmake(Qt6Widgets)
BuildRequires:  cmake(Qt6QmlTools)
BuildRequires:  qt6-qtbase-devel
BuildRequires:  qt6-qtbase-private-devel
BuildRequires:  cmake(KF6DBusAddons)
BuildRequires:  cmake(KF6WindowSystem)
BuildRequires:  cmake(KF6Runner)
BuildRequires:  cmake(KF6KIO)
BuildRequires:  cmake(KF6GlobalAccel)
BuildRequires:  cmake(KF6Config)
BuildRequires:  cmake(KF6Service)
BuildRequires:  cmake(KF6CoreAddons)
BuildRequires:  cmake(LayerShellQt)
# QML modules qmlcachegen resolves at build time (not linked). atlas-ui comes
# from atlas-framework, which is in no repository (the dev image has it).
BuildRequires:  kf6-kirigami-devel
BuildRequires:  atlas-ui >= 1.4.0

Requires:       kf6-kirigami
# Atlas.Ui, the shared look (atlas-framework)
Requires:       atlas-ui >= 1.4.0
Requires:       kf6-qqc2-desktop-style
Requires:       qt6-qtdeclarative
Requires:       qt6-qtsvg
Requires:       layer-shell-qt
# KRunner's plugins, the dock button's shell and its D-Bus QML module
Requires:       plasma-workspace

%description
AtlasOS Launcher is the Start menu and search of AtlasOS. Meta or the dock
button opens pinned apps, recent files and every app from A to Z; typing
searches apps, Settings, files, commands, the calculator and every KRunner
plugin in one ranked list. Alt+Space opens the same search on its own.

%prep
%autosetup -n atlas-launcher-%{version}

%build
# NETWORK: cargo (Corrosion runs it with --locked) fetches crates.io and the
# pinned atlas-framework crates during %%build. That works in podman and with
# `rpmbuild` on a networked machine, not in an offline mock/Koji build.
export CARGO_HOME=${CARGO_HOME:-%{_builddir}/cargo-home}
# Fedora's Rust flags, with build paths kept out of the package (see
# atlas-framework's DESIGN.md).
export RUSTFLAGS="%{build_rustflags} --remap-path-prefix=$PWD=. --remap-path-prefix=$CARGO_HOME=cargo"
export HOST_CXXFLAGS="-ffile-prefix-map=$PWD=. -ffile-prefix-map=$CARGO_HOME=cargo"
export CFLAGS="%{build_cflags} -ffile-prefix-map=$PWD=."
export CXXFLAGS="%{build_cxxflags} -ffile-prefix-map=$PWD=."
export CARGO_PROFILE_RELEASE_STRIP=none
# (%%cmake honours _vpath_srcdir, not __cmake_source_dir)
%global _vpath_srcdir apps/atlas-launcher
%cmake -G Ninja -DCMAKE_BUILD_TYPE=Release
%cmake_build

%install
%cmake_install

%check
# No path into the build tree (checked as well as set: see %%build).
rc=0
grep -qF "%{_builddir}" %{buildroot}%{_bindir}/atlas-launcher || rc=$?
if [ "$rc" != 1 ]; then
    echo "atlas-launcher holds the build path %{_builddir} (grep status $rc)" >&2
    exit 1
fi
desktop-file-validate %{buildroot}%{_datadir}/applications/net.eterneon.atlas.launcher.desktop

%files
%license LICENSE
%{_bindir}/atlas-launcher
%{_datadir}/applications/net.eterneon.atlas.launcher.desktop
%{_datadir}/dbus-1/services/net.eterneon.atlas.launcher.service
%{_userunitdir}/atlas-launcher.service
%{_datadir}/plasma/plasmoids/net.eterneon.atlas.launcher.button/
%dir %{_sysconfdir}/xdg/atlas-launcher
%config(noreplace) %{_sysconfdir}/xdg/atlas-launcher/pinned.list

%changelog
* Tue Oct 06 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.2.0-1
- Start is search first: a pill field with the account and power buttons, a
  row of pinned icons, recent chips and the apps by kind or A-Z; the top
  result is a best-match card with its actions; the panel fits its results.
- The dock button opens the launcher from a click and from Meta (through
  plasmashell), anchors the panel above it and shows when it is open.

* Mon Oct 05 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.1.1-1
- Install the default pins under /etc/xdg

* Mon Oct 05 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.1.0-1
- First package
