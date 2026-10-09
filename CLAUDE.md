# Telamon Launcher

The Start menu and search of Telamon OS, a Fedora Kinoite 44 bootc image (repo
`~/Documents/Projects/AtlasOS/AtlasOS`, read-only from here). It replaces the
vendored Andromeda Launcher (Meta, the dock's first item) and KRunner
(Alt+Space). Rust + CXX-Qt + Qt 6.11 Quick + Kirigami on Atlas Framework
(`v1.4.0`), plus the KDE C++ libraries only C++ can reach (LayerShellQt,
KRunner, KIO, KGlobalAccel, KService).

Read `docs/DESIGN.md` first, and `docs/SECURITY.md` (the threat model, the rule
for each entry point and the test that keeps it true) before touching anything
that reads, writes, parses, launches or exposes something. DESIGN.md fixes:
- the form (a resident layer-shell process plus a QML-only dock button);
- the D-Bus interface and files;
- the search phases and stability rules;
- the threads, the trust model, the failure modes and the budgets.

Change it only together with the code that changes what it says. The plan and
checklist are the Atlas Notes notes "AtlasOS/Launcher/Plan" and
"AtlasOS/Launcher/Roadmap"; tick roadmap items as they're built and tested.

The stack, build and look follow AtlasOS Store and Wizard
(`~/Documents/Projects/AtlasOS/AtlasOS Store`, `... Wizard`). The framework
is `~/Documents/Atlas Framework` (read-only from here; its reference is
`docs/reference/`).

## Hard rules

- **Build and test inside the dev container**, never on the host:
  `scripts/dev.sh <command>`. Output goes to `/work`
  (`~/.cache/claude-builds/telamon-launcher`), never into the repo or `/tmp`.
  Use a separate target dir per agent or task
  (`CARGO_TARGET_DIR=/work/target/<name> scripts/dev.sh ...`).
- **Never run the launcher, plasmashell or KWin on the user's display or
  session bus.** Headless runs use `dbus-run-session` plus
  `kwin_wayland --virtual` (layer shell, blur and focus need KWin) or
  `QT_QPA_PLATFORM=offscreen`, in the container, with every `XDG_*_HOME`
  under `/work/xdg/<run>`. Never read or write the user's real
  `~/.config/telamon-launcher`, `krunnerrc`, `recently-used.xbel` or KActivities data.
- **No shell strings.** Apps start through `KIO::ApplicationLauncherJob`,
  files through `KIO::OpenUrlJob`, and commands through
  `KIO::CommandLauncherJob(executable, args)`. The one exception is "Run in
  Terminal", which hands the user's own typed line to `KTerminalLauncherJob`.
- **No D-Bus method runs a result.** Only the user's own input in the panel
  runs anything.
- **No privilege.** No setuid, no polkit action of its own, no root helper.
  Power and session go through logind and ksmserver. Never call
  `atlas-system-helper`.
- **No network**, except that the web-search row opens the user's browser.
- **Untrusted input** is checked where it enters, in `telamon-launcher-core`
  where it can be, with the caps in DESIGN.md "Trust":
  - `recently-used.xbel`;
  - Explorer's file hits;
  - KRunner matches;
  - the Settings index;
  - the D-Bus arguments;
  - the user's own `launcher.conf`, `pinned.list` and `usage.tsv`.
  Every `Text`/`Label` showing app, file, plugin or query text sets
  `textFormat: Text.PlainText`.
- **Files other programs can touch are never opened blindly.** Rust reads go
  through `fsutil::read_capped` (non-blocking, regular files only, a size cap);
  a path handed to Qt or KDE (which open it as it is, on the asking thread) goes
  through `Validate::plainSmallFile` first, an absolute icon path through
  `text::icon_file_ok`: a pipe in the place of a file blocked the GUI thread for
  good. Writes into the user's `applications` folder use `write_atomic_nofollow`.
  docs/SECURITY.md has the rest.
- **Never log what the user typed or ran**: no query text, file names or
  paths. Only timings, counts and error kinds.
- **The GUI thread never blocks.** Queries, merging and file work run on the
  search and IO workers; every D-Bus call is asynchronous with a timeout.
  Results come back with `qt_thread().queue`, tagged with the query serial.
- **Idle means idle.** While hidden there are no timers and no polling (one
  single-shot memory trim 60 s after a hide is the only exception).
- **Telamon.Ui is the installed `telamon-ui` package.** Use its controls, never
  stock QQC2 or Kirigami buttons. Ask the "AtlasOS Framework" session for
  what's missing. A local stand-in is named `Launcher<Name>`, so
  `check-app-names.sh` never sees a clash.
- **Other sessions' repos are read-only.** Interfaces with Explorer,
  Settings and Store are agreed over SendMessage and recorded in DESIGN.md.
  Image changes go to the "AtlasOS" coordinator as text; never edit the
  AtlasOS repo.
- **Commits:** author them as
  `EternalHell <77252745+EternalCoder454@users.noreply.github.com>`, and
  commit only the paths you own (`git commit -- <paths>`).
- **Pushes:** the coordinator approves them once the change has passed its
  four gates.
- **Style:** MIT. Wording follows KDE: Title Case buttons and titles, US
  spelling, `qsTr()` for every string a user reads.

## Commands

| Task | Command (from the repo root on the host) |
|---|---|
| Format | `scripts/dev.sh cargo fmt --all --check` |
| Lint | `scripts/dev.sh cargo clippy --workspace --all-targets --locked -- -D warnings` |
| Tests | `scripts/dev.sh cargo test --workspace --locked` |
| App build | `scripts/dev.sh bash -c 'cmake -S apps/telamon-launcher -B /work/cmake/dev -G Ninja && cmake --build /work/cmake/dev'` |
| QML lint | `scripts/dev.sh qmllint-qt6 apps/telamon-launcher/qml/*.qml plasmoid/*/contents/ui/*.qml` |
| Telamon checks | `scripts/dev.sh bash -c '$TELAMON_FRAMEWORK/tools/lint-app.sh . && $TELAMON_FRAMEWORK/tools/check-app-names.sh .'` |
| RPM | `scripts/dev.sh packaging/build-rpm.sh /work/out` (`%check` runs ctest, `scripts/check-hardening.sh` and `annocheck`) |
| Property tests, long run | `scripts/dev.sh env PROPTEST_CASES=20000 cargo test --workspace --locked -- props` |
| C++ tests | `scripts/dev.sh bash -c 'cmake -S apps/telamon-launcher -B /work/cmake/dev -G Ninja -DTELAMON_LAUNCHER_BUILD_TESTS=ON && cmake --build /work/cmake/dev && ctest --test-dir /work/cmake/dev --output-on-failure'` |
| D-Bus and special-file probe | `scripts/dev.sh bash scripts/headless-dbus-security.sh /work/cmake/dev/telamon-launcher` |
| Hardening check's own test | `scripts/dev.sh scripts/test-check-hardening.sh` |
| Supply chain | `cargo deny check` and `cargo audit` (config: `deny.toml`; CI: `security.yml`) |

`scripts/dev.sh` builds `localhost/telamon-launcher-dev:44` from
`ci/Containerfile`'s `dev` target:
- That file is the one list of packages.
- CI runs in its `ci` target, published as
  `ghcr.io/eternalcoder454/atlas-launcher-dev` by `dev-image.yml`, after
  `ci/secret-check.sh`.
- It rebuilds by itself when the Containerfile, the spec's BuildRequires or
  the framework tag change (`ci/image-tag.sh`).

The image build is intensive: on this machine, the "AtlasOS" coordinator runs
it, as it runs full suites, RPMs and UI runs. Small iterative compiles go
through `~/.claude/heavy/run.sh` with `-j 8` or less.

## Moving the atlas-framework pin

Change `tag` in `Cargo.toml`, then
`scripts/dev.sh cargo update -p telamon-framework-ui`. CI and the dev image
follow the tag by themselves (`ci/framework-ref.sh`). When the app uses
something new in Telamon.Ui, raise `ui:` in `apps/telamon-launcher/src/lib.rs`
and `telamon-ui >=` in the spec (Requires and BuildRequires) to match.
