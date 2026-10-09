# Telamon Launcher: the Start menu and search of Telamon OS.

# No debuginfo subpackage: the Rust flags below keep symbols (debuginfo=2,
# strip=none) and the binary is shipped as built.
%global debug_package %{nil}

Name:           telamon-launcher
Version:        0.4.1
Release:        1%{?dist}
Summary:        Telamon Launcher, the Start menu and search of Telamon OS
License:        MIT
URL:            https://github.com/EternalCoder454/atlasos-launcher
Source0:        telamon-launcher-%{version}.tar.gz
# Renamed from atlas-launcher in 0.3.0.
Obsoletes:      atlas-launcher < 0.3.0
Provides:       atlas-launcher = %{version}-%{release}

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
# annocheck confirms scripts/check-hardening.sh on the finished program (%%check)
BuildRequires:  annobin-annocheck
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
# QML modules qmlcachegen resolves at build time (not linked). telamon-ui comes
# from atlas-framework, which is in no repository (the dev image has it).
BuildRequires:  kf6-kirigami-devel
BuildRequires:  telamon-ui >= 2.0.9

Requires:       kf6-kirigami
# Telamon.Ui, the shared look (atlas-framework)
Requires:       telamon-ui >= 2.0.9
Requires:       kf6-qqc2-desktop-style
Requires:       qt6-qtdeclarative
Requires:       qt6-qtsvg
Requires:       layer-shell-qt
# KRunner's plugins, the dock button's shell and its D-Bus QML module
Requires:       plasma-workspace

%description
Telamon Launcher is the Start menu and search of Telamon OS. Meta or the dock
button opens pinned apps, recent files and every app from A to Z; typing
searches apps, Settings, files, commands, the calculator and every KRunner
plugin in one ranked list. Alt+Space opens the same search on its own.

%prep
%autosetup -n telamon-launcher-%{version}

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
%global _vpath_srcdir apps/telamon-launcher
# TELAMON_LAUNCHER_BUILD_TESTS: the C++ checks of cpp/validate.h, run in %%check.
%cmake -G Ninja -DCMAKE_BUILD_TYPE=Release -DTELAMON_LAUNCHER_BUILD_TESTS=ON
%cmake_build

%install
%cmake_install
# The names before the rename, for this release (the image and the dock still
# say them): the binary, the user unit (`systemctl --user restart
# atlas-launcher`, `systemctl --global enable atlas-launcher.service`, both
# act on telamon-launcher.service) and the old pinned.list location.
ln -s telamon-launcher %{buildroot}%{_bindir}/atlas-launcher
ln -s telamon-launcher.service %{buildroot}%{_userunitdir}/atlas-launcher.service

%check
# The C++ checks on strings from outside (desktop file ids, file URIs, web URLs,
# command paths; docs/SECURITY.md, "Starting programs").
%ctest
# No path into the build tree (checked as well as set: see %%build).
rc=0
grep -qF "%{_builddir}" %{buildroot}%{_bindir}/telamon-launcher || rc=$?
if [ "$rc" != 1 ]; then
    echo "telamon-launcher holds the build path %{_builddir} (grep status $rc)" >&2
    exit 1
fi
desktop-file-validate %{buildroot}%{_datadir}/applications/net.eterneon.telamon.launcher.desktop
desktop-file-validate %{buildroot}%{_datadir}/applications/net.eterneon.atlas.launcher.desktop
test "$(readlink %{buildroot}%{_bindir}/atlas-launcher)" = telamon-launcher
test "$(readlink %{buildroot}%{_userunitdir}/atlas-launcher.service)" = telamon-launcher.service
# The program carries the hardening Fedora's build flags give it (docs/SECURITY.md,
# "Build hardening"): a position-independent executable with full RELRO and
# BIND_NOW, no executable stack, no RPATH, stack protectors; readelf says so, not
# the flags we meant. It holds no development-only switch.
scripts/check-hardening.sh --cxx %{buildroot}%{_bindir}/telamon-launcher
# annocheck agrees (PIE, BIND_NOW, RELRO, non-executable stack, CET marks, no
# writable GOT, stack protection, ...): it fails the build as well.
annocheck %{buildroot}%{_bindir}/telamon-launcher

%files
%license LICENSE
%{_bindir}/telamon-launcher
%{_bindir}/atlas-launcher
%{_datadir}/applications/net.eterneon.telamon.launcher.desktop
%{_datadir}/applications/net.eterneon.atlas.launcher.desktop
%{_datadir}/dbus-1/services/net.eterneon.telamon.launcher.service
%{_datadir}/dbus-1/services/net.eterneon.atlas.launcher.service
%{_userunitdir}/telamon-launcher.service
%{_userunitdir}/atlas-launcher.service
%{_datadir}/plasma/plasmoids/net.eterneon.telamon.launcher.button/
# Replaces the dock button of the old ID in existing panels, once per user.
%{_datadir}/plasma/shells/org.kde.plasma.desktop/contents/updates/telamon-20261007-launcher-button.js
%dir %{_sysconfdir}/xdg/telamon-launcher
%config(noreplace) %{_sysconfdir}/xdg/telamon-launcher/pinned.list

%changelog
* Fri Oct 09 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.4.1-1
- Fix: typing an app's name ("telamon g" for Telamon Gates) no longer offers to run it as a command. The
  "Run" rows for `telamon` and `atlas` are offered only when the word after the name is one of the recipes
  of /usr/share/telamon/telamon.just (read once, at the scan of PATH), and "Run" rows always rank below an
  app whose name the whole query matches, whatever the history says.

* Thu Oct 08 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.4.0-1
- Security release (docs/SECURITY.md has the threat model, each rule and the test that holds it).
- Rename App: the desktop file the launcher writes for a name is never written through a link: a link (or a
  pipe, or a folder) at its name is refused and a name that appeared since it looked is not replaced; the
  file it reads back to see whether it is its own is opened without following a link. The name written into
  the file is checked again where it is written (no line break, control, bidi or invisible character, at most
  64 characters), so no caller can add a key such as Exec to a desktop file the whole desktop reads. The
  launcher's own marker counts only in the desktop files of your own applications folder.
- The D-Bus interface is exactly the documented one: an internal method (queueClearHistory) and two internal
  signals (importPinsRequested, clearHistoryRequested) were exported on the bus by mistake and are not.
- Text handed to the panel by another process (D-Bus Show, --search) is cleaned like typed text, so the search
  field shows what is searched for (control, bidi and invisible characters out, at most 256 characters).
- Starting things: the desktop file id of an app to start must be a plain id (KService takes an absolute path
  as an id and would load any desktop file there), a typed command runs only by the absolute path the PATH scan
  found (a leading "/", no ".."), web addresses allow no user or port and long non-ASCII searches are no longer refused, file addresses no "." or ".." segment, and the Flatpak id check no
  longer lets a trailing line break through.
- Fix: a pipe could hang the launcher for good. An app whose Icon= names a pipe stopped the launcher when
  the Start page drew it; a pipe in the place of ~/.config/telamon-launcher/state.conf, or of KRunner's state
  file (~/.local/state/telamon-launcherstaterc), stopped it at start. Icon paths must now be plain files (not a
  pipe, a device, an empty or huge file, or /proc and /sys), and those two files are opened only if they are
  plain small files. File icons are drawn by an asynchronous image, so a file swapped for a pipe after the check
  cannot block the panel either.
- Smaller: a mime type starting with "-" or "." no longer makes an icon name starting with it, the folder shown
  under a command and the account's real name are cleaned like other shown text.
- Tests: property tests (cargo test -- props) for the cleaners, the desktop-entry catalogue, the rename writer,
  the file parsers, the calculator, the web URL and typed commands; C++ tests (ctest) for the checks above;
  a headless probe of the D-Bus surface; a lint of the QML that insists on plain text. Build hardening is
  checked on the finished program by the package build; CI runs cargo-deny and cargo-audit weekly.

* Thu Oct 08 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.3.4-1
- Fix: the app used about 8% of a core with its window idle. The icon layers added in the last release
  were redrawn on every frame with Qt Quick's software renderer; a layer is live now only for a moment
  after its icon changes (source, colour, size, state or theme).

* Thu Oct 08 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.3.3-1
- Fix: icons drawn over dialogs, popups and menus. With Qt Quick's software renderer a Kirigami.Icon was
  painted again over what sat in front of it whenever a repaint touched a part of it; the app's icons are
  layers on that renderer now.

* Wed Oct 07 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.3.2-1
- The panel is see-through like the other Telamon apps: with Transparency on (Settings, Appearance) and the
  compositor blurring, it is the floating surface's translucent tint over the blurred desktop, with a blur
  region that follows its rounded corners (and the slide-in); with it off, or with no blur, it is opaque.
  The switch and the compositor's blur are followed live. Before, a launcher started at login (before KWin's
  blur was up) never noticed the blur and stayed opaque: it asks again at each show.
- The page's soft top and bottom edges are only drawn on the opaque panel (over blur they showed as a denser
  band); the letter grid stays opaque.
- Rename App reaches the whole desktop: the name is also written to a desktop file of the same ID in
  ~/.local/share/applications (the original with only Name changed, marked X-Telamon-Renamed=true, like the
  menu editor does it), so the dock, the menus and KRunner show it too and a window still groups on its pinned
  icon. Reset Name (or deleting the line in names.conf) removes the file again. Only files the launcher made
  are ever changed or deleted; a file of your own with that ID is left alone and the name stays the launcher's
  (with a notice). Names from 0.3.1 are written once at the first start, and an app's file is copied again
  from the system's when the app updates (the name stays).

* Wed Oct 07 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.3.1-1
- New "Rename App…" in the right-click menu of an app (the grid, the pinned icons, the recent chips and the
  search results, the best match included). The name is edited in place: Enter saves, Escape cancels, an empty
  name gives the app's own back, and "Reset Name" appears once an app has a name of yours. The new name shows
  everywhere the launcher shows the app (the grid and its A-Z order, the pins' tooltips, the recent chips, the
  results), and search finds an app by either its name or the one it came with.
- The names are the launcher's own, in ~/.config/telamon-launcher/names.conf: the app's desktop file is never
  copied or edited, so the app keeps following its own updates. A name is cleaned (control and bidi
  characters removed, at most 64 characters) wherever it comes from, and shown as plain text.

* Wed Oct 07 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.3.0-1
- Renamed to Telamon Launcher (telamon-launcher, net.eterneon.telamon.launcher, the dock button
  net.eterneon.telamon.launcher.button), on telamon-ui 2.0.0. Obsoletes and provides atlas-launcher.
- For this release the old names still work: the D-Bus name net.eterneon.atlas.launcher with its interface
  (and its activation file), /usr/bin/atlas-launcher, atlas-launcher.service, and the hidden
  net.eterneon.atlas.launcher.desktop. A Plasma update script replaces the old dock button with the new one
  in existing panels, keeping its place, configuration and shortcut.
- The folders ~/.config/atlas-launcher and ~/.local/state/atlas-launcher (and atlas-launcherstaterc) move to
  the telamon-launcher names on the first run. Pins and history of the apps that were renamed
  (net.eterneon.atlas.<app>.desktop) carry over to their new desktop IDs.

* Tue Oct 06 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.2.3-1
- Lighter and faster, with nothing on screen changed: tiles out of view
  draw nothing, and menus, the letter grid and each tile's drag and tooltip
  parts are made when first used. Memory after use is about 44% lower, the
  first open about three times faster, and memory no longer grows with the
  number of installed apps.

* Tue Oct 06 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.2.2-1
- Start slides up out of the dock instead of popping in at the middle.
- The panel keeps one height while typing, and the first letter no longer
  flashes "No results" before the answers arrive.
- The search field uses Atlas's control shape; rows are never cut off
  half-way at the edges, which fade instead.
- Smoother wheel scrolling, for both notched and free-spinning wheels.

* Tue Oct 06 2026 EternalHell <77252745+EternalCoder454@users.noreply.github.com> - 0.2.1-1
- Start opens centred on the screen, just above the dock, and the dock stays
  shown while it is open.
- One web row: the web-shortcuts plugin's default search no longer doubles
  it (keyword shortcuts such as gg: still work).
- atlas-framework 1.6.0.

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
