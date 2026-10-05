# AtlasOS Launcher: design

What this file fixes: the form, the layout, the threading rule, what is
trusted, who owns what, the failure modes and the budgets. Change it together
with the code that changes them. The plan and roadmap are the Atlas Notes
notes "AtlasOS/Launcher/Plan" and "AtlasOS/Launcher/Roadmap".

App ID `net.eterneon.atlas.launcher`, repo `atlasos-launcher`, MIT.

## Scope

One launcher replaces two things on AtlasOS:

| Today | After |
|---|---|
| Andromeda Launcher (vendored plasmoid, the dock's first item, Meta) | Start mode of the launcher, opened by Meta or the dock button |
| KRunner (Alt+Space; already retired by `build.sh`, its search lives in Andromeda) | Search mode of the launcher, opened by Alt+Space, Alt+F2 or Meta+S |
| Baloo file search (indexer removed from AtlasOS) | AtlasOS Explorer's file index over D-Bus |

**Start mode**, the Windows 11 Start menu: a blurred, rounded Atlas.Ui panel
above the dock, with the search field focused. One scrolling page:

1. Pinned: a grid of 6 columns, 3 rows shown, "Show All" to expand.
   Drag to reorder, or reorder from the keyboard with Alt+Shift+arrows or
   the context menu (Move to Front, Move Left, Move Right).
2. Recent: up to 6 recent files and recently used or installed apps, in two
   columns, with "More" for the rest. Can be turned off (`ShowRecent`).
3. All Apps: A to Z with letter headers. A header opens the letter grid
   (A–Z, then #). Letters that have no apps are dimmed, and choosing a letter
   jumps to it.
4. Footer: the user tile (avatar and name, which opens Settings' Users page)
   on the left. On the right, Settings and Files buttons and the power
   button, whose menu has Lock, Sleep, Hibernate (when available), Switch
   User (when available), Log Out, Restart and Shut Down.

Typing turns the page into **one ranked list**: apps, AtlasOS Settings pages
and settings, files and folders, calculator and unit conversion, commands,
web search, and the results of every enabled KRunner plugin. Each row shows
its kind on the right ("App", "Setting", "Folder"...). The first row is the
top hit, drawn larger. Start mode also shows a detail pane on the right for
the selected row: icon, name, kind, path, and its actions.

**Search mode** (Alt+Space, Alt+F2, Meta+S): the same search, compact, at the
top third of the active screen like Spotlight: a field that grows downward
into at most 8 rows. No detail pane: actions are in the context menu.

**Everywhere:** Enter runs the highlighted row (the top hit unless the user
moved). The arrow keys move within a region and Tab moves between regions
(field, sections or results, detail actions, footer). Typing a printable
character anywhere sends it to the field. Esc clears the field, or closes
when it is empty. Menu, Shift+F10 or a right click opens the context menu:

| Item | For | How |
|---|---|---|
| Pin to Start / Unpin from Start | apps | `pinned.list` |
| Desktop file actions ("New Window"...) | apps | KIO ApplicationLauncherJob with the action |
| Open Containing Folder | files, folders | `org.freedesktop.FileManager1.ShowItems` (Explorer) |
| Copy Path | files, folders | clipboard |
| Remove from Recent | recent files | `KRecentDocument::removeFile` |
| App Settings | apps | Settings `ActivateAction("open-app", [id])` |
| Uninstall | Flatpak apps only | `atlas-store --remove <X-Flatpak id>` through CommandLauncherJob with an activation token; other apps show it disabled with "Part of AtlasOS" |
| Move to Front, Left, Right | pinned | `pinned.list` |
| Edit Applications… | panel menu | kmenuedit's desktop file through ApplicationLauncherJob |

## Form: a resident layer-shell process, with a QML button in the dock

Decided by how fast it opens. The two candidates:

1. **A Plasma applet** (Andromeda's form, with a Rust/C++ QML plugin) runs
   inside plasmashell. Its popup is a PlasmaQuick dialog: the full
   representation is built lazily on the first open. It runs on plasmashell's
   GUI thread, beside every panel widget and the desktop, so it waits whenever
   they are busy. KRunner plugins load on the first query, inside the shell:
   that first search is the 3–5 s freeze users report
   ([bug 352785](https://bugs.kde.org/show_bug.cgi?id=352785)). Meta takes two
   hops: KWin to plasmashell, then plasmashell to the applet.
2. **A resident process** (KRunner's own form before AtlasOS retired it)
   keeps its window created and its scene graph, glyphs and models warm while
   hidden. KWin's Meta binding calls its D-Bus method directly. Showing it
   maps a layer-shell surface and paints one frame: no QML to build, no
   plugin to load, and no other widget sharing its thread.

**Chosen: the resident process.** The panel is one `QQuickWindow` made a
layer-shell surface with LayerShellQt (layer Top, keyboard interactivity
OnDemand, activate on show, `wantsToBeOnActiveScreen`). It is created at
login, hidden and painted once while hidden; Start and Search modes change
its anchors, margins and size before it is shown. KWin blurs it through
`KWindowEffects::enableBlurBehind`, with a rounded region.

Other reasons for the choice:

- **Crash isolation.** KRunner plugins (including third-party ones) and our
  own code run outside plasmashell. A crash there costs the launcher, which
  systemd restarts in under a second, not the dock, menu bar and desktop.
- **Meta does not depend on plasmashell.** If the shell hangs, Meta and
  Alt+Space still open the launcher.

What it costs: a resident process's memory, the same class as the KRunner
daemon it replaces (see Budgets).

The P phase measures the claim against Andromeda on the same image, using
the coordinator's KWin-script method (map time from the D-Bus call to
`workspace.windowAdded`), cold and warm.

**The dock button** is a QML-only plasmoid, `net.eterneon.atlas.launcher.button`,
with no C++ in the shell:

- It shows the AtlasOS icon (with an accent underline while the launcher is
  open), and provides `org.kde.plasma.launchermenu`, so Plasma's own
  `activateLauncherMenu` reaches it.
- A click calls `ToggleStart(a{sv})` with its screen and global rect, so the
  panel opens centred above it.
- On load and on every geometry change it calls `SetDockAnchor(a{sv})`, so
  Meta opens the panel in the same place.
- On first load it hands the old launcher's favourites to `ImportPins(as)`.
  The coordinator's update script copies them into the button's
  `ImportPins` config key. The button sends them once, then clears the key.
- It calls D-Bus through `org.kde.plasma.workspace.dbus`. The F phase checks
  that this module exists in Plasma 6.7. If it doesn't, the fallback is a
  tiny QML plugin in the RPM.

Without a dock anchor (no dock, or before the button loads), Start mode opens
at the bottom centre of the active screen. Layer-shell keeps it clear of any
panel's exclusive zone; a 12 px margin applies when the dock has none.

## Interfaces

**D-Bus:** bus `net.eterneon.atlas.launcher`, path
`/net/eterneon/atlas/launcher`, interface `net.eterneon.atlas.Launcher1`.
It is D-Bus-activatable (`SystemdService=atlas-launcher.service`).

| Member | What it does |
|---|---|
| `ToggleStart()`, `ToggleSearch()` | KWin's modifier-only binding: no arguments |
| `ToggleStart(a{sv})` | The dock button: `screen` (s), `anchor` (x, y, w, h in screen coordinates, `(iiii)`) |
| `Show(s mode, s query, a{sv} platform_data)` | Open in `start` or `search` mode with the query typed in; `activation-token` in platform_data. Opening never runs anything |
| `Hide()` | Hide if shown |
| `SetDockAnchor(a{sv})` | The dock button's screen and rect |
| `ImportPins(as)` | Only when the user has no `pinned.list` yet; ids validated and resolved to installed apps |
| `ClearHistory()` | Removes `usage.tsv` and the in-memory ranking; returns when done |
| `Visible` (b, property, with PropertiesChanged) | For the dock button's indicator |

No method runs a result. Results run only from the user's own input in the
panel.

**CLI:** `atlas-launcher` starts the service (`--daemon`, used by the unit).
`atlas-launcher --start`, `--search [text]` and `--hide` forward to the
running instance through KDBusService.

**Global shortcuts:** registered at runtime with KGlobalAccel, under the
component `net.eterneon.atlas.launcher`:

- `toggle-search`: Alt+Space, with Alt+F2 as its alternate.
- `toggle-search-meta-s`: Meta+S.

The GlobalShortcuts portal (Atlas.Ui's AtlasGlobalShortcut) is not used: it
asks the user to approve the binding, which a system launcher must not do.

**Meta** is KWin's modifier-only shortcut, set by the image in `/etc/xdg/kwinrc`
(see "Image changes").

**Files:**

| File | What | Format |
|---|---|---|
| `$XDG_CONFIG_HOME/atlas-launcher/launcher.conf` | Options, written by Settings' "Launcher & Search" page with KConfig::Notify and followed live with KConfigWatcher | KConfig ini (below) |
| `$XDG_CONFIG_HOME/atlas-launcher/pinned.list` | Pinned apps in order | One desktop file id per line (`org.kde.dolphin.desktop`), or `preferred://browser`, `preferred://filemanager`, `preferred://terminal`; `#` comments |
| `/etc/xdg/atlas-launcher/pinned.list` | The image's default pins | Same |
| `$XDG_STATE_HOME/atlas-launcher/usage.tsv` | What the user ran, for ranking. Cleared by ClearHistory or by deleting it | `query-prefix TAB result-id TAB count TAB last-used-unix`, at most 2,000 lines |

`launcher.conf`:

```
[Search]
WebSearch=true            # "Search the web for …" as the last row
WebSearchEngine=duckduckgo  # duckduckgo google bing startpage ecosia brave kagi qwant
FileSearch=true
LearnFromUse=true         # false: nothing recorded, history ignored (not deleted)
[Start]
ShowRecent=true           # Windows' "Show recently opened items"
ShowRecentFiles=true
```

KRunner's `krunnerrc [Plugins] <id>Enabled` keys are honoured as Plasma
writes them, including those for the parts the launcher draws itself:

| Key | Launcher part |
|---|---|
| `servicesEnabled` | apps |
| `calculatorEnabled` | calculator |
| `unitconverterEnabled` | unit conversion |
| `shellEnabled` | commands |
| `krunner_systemsettingsEnabled` | Settings results |

Those five plugins are never run through RunnerManager, so nothing shows
twice.

**Other apps' interfaces, used:**

- AtlasOS Settings: `/usr/share/atlas-settings/search-index.json` (format
  v1: locale maps, falling back to "C"; unknown fields ignored; another major
  `version` refused). Deep links through `ActivateAction("open", [link])`
  and `("open-app", [id])` on `net.eterneon.atlas.settings`, with
  `activation-token` in platform_data. The user tile opens `users`.
- Explorer: the file index, bus `net.eterneon.atlas.explorer.Search`, path
  `/net/eterneon/atlas/explorer/Search`, interface
  `net.eterneon.atlas.explorer.Search1` (D-Bus-activated `atlas-explorer-indexd`).
  It offers `Search(s query, u limit, a{sv} options) -> a(sssssxtd)`, where
  each hit is (uri, name, kind, mime, icon, mtime, size, score), already
  sorted, and `Status() -> a{sv}`. The launcher asks for 20 hits, with
  `kind` and `root` only when the user types a filter, and treats the hits as
  untrusted. The interface also gives `org.freedesktop.FileManager1.ShowItems`.
- Store: `atlas-store --remove <flatpak id>` with `XDG_ACTIVATION_TOKEN`.
- Plasma: `org.kde.Shutdown` (log out, restart, shut down: chosen from the
  power menu, run at once like Windows), `org.kde.LogoutPrompt` (the same
  chosen from search results: Plasma's prompt with its countdown, so a stray
  Enter cannot restart the machine), `org.freedesktop.login1` (suspend,
  hibernate, `CanSuspend`/`CanHibernate`), `org.freedesktop.ScreenSaver.Lock`,
  `org.kde.KSMServerInterface` (switch user, when `canSwitchUser` allows it),
  AccountsService (`org.freedesktop.Accounts`) for the real name and avatar.

## Layout

- `crates/atlas-launcher-core`, no Qt:
  - the app catalogue (plain data handed over from KService);
  - the query engine and scoring;
  - the calculator and unit converter;
  - the Settings index parser;
  - the `recently-used.xbel` parser;
  - the usage store, the pins store and `PATH` command resolution;
  - web search URLs.
  It has unit tests and fixtures, and a bench of the query engine.
- `apps/atlas-launcher`:
  - `cpp/main.cpp`: Qt start, KDBusService, the D-Bus adaptor.
  - `cpp/panel.*`: LayerShellQt, blur, placement, show and hide.
  - `cpp/runners.*`: RunnerManager to plain matches, and running a match.
  - `cpp/launch.*`: KIO jobs and activation tokens.
  - `cpp/system.*`: power, session, user and KGlobalAccel.
  - `src/` (CXX-Qt): the results and start-page models, and the bridge to
    the core.
  - `qml/`.
- `plasmoid/net.eterneon.atlas.launcher.button/`: the dock button, QML only.
- `packaging/`: the RPM spec and `build-rpm.sh`, the user unit, the D-Bus
  service file and the default `pinned.list`.

## Search

Results arrive in two phases for each query, tagged with a serial:

1. **Instant**, computed in the core on the search worker within the same
   keystroke (budget ≤ 3 ms):
   - apps, matched on name, generic name, keywords, executable and desktop
     actions;
   - Settings pages and settings;
   - the calculator and units;
   - commands, both session commands and `PATH` executables;
   - recent files;
   - the web row.
2. **Late:**
   - KRunner plugins, launched on the GUI thread with
     `RunnerManager::launchQuery`; they run on its thread pool;
   - Explorer's file index, an async D-Bus call with a 150 ms timeout.
   Their matches are converted to plain data in C++ and merged by the core.

**Matching** is case- and diacritic-insensitive. Each candidate gets a match
quality `m`, from strongest to weakest:

| Match | m |
|---|---|
| exact name | 1.0 |
| name prefix | 0.9 |
| word prefix (spaces, `-`, `_`, `.`, camelCase) | 0.8 |
| acronym ("vsc" finds "Visual Studio Code") | 0.7 |
| keyword or generic-name prefix | 0.65 |
| substring | 0.5 |
| typo-tolerant subsequence (gap-penalised) | 0.3–0.5 |
| description | 0.3 |

**Score** = `m × prior(kind) + learned`.

- Priors:

  | Kind | Prior |
  |---|---|
  | calculator (only when the text is an expression or conversion) | 1.2 |
  | app | 1.0 |
  | setting | 0.85 |
  | command | 0.8 |
  | file or folder | 0.7 |
  | KRunner match | relevance × 0.75 |

  The web row is always last.
- `learned` comes from `usage.tsv`:
  - up to 0.5 for this exact query prefix having led to this result (the
    frecency `Σ 2^(−age/14 d)`, through `0.25·log2(1+f)`);
  - up to 0.15 for the result's overall use.
- Ties: the shorter name, then alphabetical.

**Stability.** These rules exist because KDE users report results jumping and
Enter running the wrong thing
([358252](https://bugs.kde.org/show_bug.cgi?id=358252),
[521001](https://bugs.kde.org/show_bug.cgi?id=521001)):

- Once the instant phase has shown a top hit for a query, late results never
  displace it or the selected row. They are inserted below.
- Enter runs the highlighted row of the current query. If the user types and
  presses Enter before that query's instant phase has arrived, Enter waits
  for it (at most 30 ms) instead of running a stale row.
- A result runs once per Enter: key auto-repeat is ignored.
- The model updates by diff (insert, remove, move), never by reset, so the
  selection and the screen reader's place survive.

**Learning.** Running a result records (the query's first 1–8 characters,
the result id) in `usage.tsv`, on the worker, atomically (temp file and
rename). Nothing is recorded when `LearnFromUse=false`. Clear it from
Settings' "Launcher & Search" page (`ClearHistory`) or by deleting the file.

**Calculator and units** are in the core, with no network:

- `+ − × ÷ ^ %`, parentheses, `sqrt`, `sin`/`cos`/`tan`, `ln`, `log`, `abs`,
  `pi`, `e`, and "15% of 80";
- conversions such as "5 km in mi" or "100 f to c", for length, mass,
  temperature, volume, area, speed, time, and data (kB/KiB);
- the locale's decimal separator.

There are no currencies: those need the network. Enter copies the result and
closes.

**Commands:**

- Session commands are results: Lock, Sleep, Restart, Shut Down, Log Out and
  Switch User.
- A typed command line whose first word is an executable on `PATH` offers
  "Run" and "Run in Terminal". Run splits the line with `KShell::splitArgs`
  and starts it through `KIO::CommandLauncherJob(executable, args)`. No
  shell is involved: a line with shell syntax (`|`, `;`, `&`, `$`, `>`,
  backquotes) offers only "Run in Terminal", which hands the user's own text
  to `KTerminalLauncherJob`.

**Web:** "Search the web for ‘text’" opens the chosen engine's https URL with
the query percent-encoded, through `KIO::OpenUrlJob` in the default browser.
The launcher sends nothing itself.

**Files:**

- From Explorer's index while `FileSearch=true`.
- Until that index exists, or when it does not answer: recent files from
  `recently-used.xbel` (which KDE apps write through KRecentDocument, and
  GTK apps too).

## Launching

- Apps start through `KIO::ApplicationLauncherJob` (with the desktop action
  when one was chosen).
- Files and URLs open through `KIO::OpenUrlJob`.
- Commands start as `KIO::CommandLauncherJob(executable, args)`.
- No shell command is ever built from a string.

Before starting a job, the panel asks KWin for an XDG activation token
against its own surface and last input serial
(`KWaylandExtras::xdgActivationToken`) and passes it as the job's startup ID,
so the app gets focus. It then hides. If no token arrives within 200 ms, it
launches without one. Launch errors after the panel hides are shown through
the job's notification UI delegate.

## Threads

The GUI thread never blocks:

- **GUI thread:** QML, the models, `RunnerManager` (its plugins run on its
  own pool), KIO jobs (asynchronous), and every D-Bus call (asynchronous,
  with a timeout).
- **Search worker** (one Rust thread): queries, merging, scoring, the
  usage store.
- **IO worker:**
  - `recently-used.xbel`;
  - the `PATH` scan;
  - the Settings index;
  - writes of `pinned.list`.
- The app catalogue comes from KService/KSycoca on the GUI thread (an mmap'd
  database read in a few ms). It is rebuilt when `KSycoca::databaseChanged`
  fires, and handed to the worker as one immutable snapshot.

Results come back with `qt_thread().queue`, each tagged with the query serial
so stale ones are dropped.

## Lifetime and idle

- **Start:** `atlas-launcher.service` (a user unit, `PartOf=graphical-session.target`)
  starts it at login.
- **Activation:** the D-Bus service file activates the same unit, so Meta
  works even before the unit has started, and again after a crash.
- **Restart:** `Restart=on-failure` with `RestartSec=500ms`.
- **While hidden:**
  - no timer and no polling;
  - the only watchers are KConfigWatcher, KSycoca's directory watch and the
    xbel file's mtime, which is checked when the panel opens (inotify, which
    wakes only on change);
  - one single-shot timer 60 s after a hide trims memory
    (`QQuickWindow::releaseResources`, `malloc_trim`).
- **On open:**
  - the recent files and the `PATH` cache refresh if their mtimes changed
    (one `stat` each);
  - nothing is rescanned otherwise.

## Trust (attack surface)

The launcher runs as the user with no privilege: no setuid, no polkit action
of its own, no root helper. Power and session go through logind and
ksmserver, under their own policies. There are no secrets. What it reads,
and how:

- **D-Bus methods (any session process):**
  - They can open or hide the panel and type a query, capped at 1 KiB with
    control and bidi characters stripped; they never run anything.
  - `ImportPins` takes at most 64 ids. Each must match
    `[A-Za-z0-9._-]{1,255}.desktop` or a `preferred://` name and resolve to
    an installed app, and is used only when no `pinned.list` exists.
  - `ClearHistory` deletes only the launcher's own file.
- **`recently-used.xbel`:** any app can write it.
  - Size cap 4 MiB, an element cap and XML depth limits.
  - Only `file:` URLs are kept.
  - Names are cleaned of control and bidi characters.
  - An entry is shown only if its file exists (checked on the IO worker).
- **The Settings index:**
  - Size cap 2 MiB.
  - Links must match `[a-z0-9-]+(/[a-z0-9-]+)*`, at most 128 bytes, and are
    only passed back to Settings.
  - Text is length-capped.
- **The user's own files** (`launcher.conf`, `pinned.list`, `usage.tsv`):
  read defensively; bad lines are skipped and logged; size caps of 256 KiB
  each.
- **KRunner plugins:**
  - In-process plugins are trusted as KRunner trusted them; D-Bus runners
    run out of process.
  - Their text is shown as plain text.
  - Their actions run through `RunnerManager::run`.
- **Explorer's file results:**
  - Paths must be absolute and normalised, under 4 KiB, with no NUL.
  - Names are cleaned.
  - Only `file:` URLs are opened.
- **Displayed text:** every `Text`/`Label` that shows app, file, plugin or
  query text sets `textFormat: Text.PlainText`.
- **Typed commands:**
  - The executable must be on `PATH` and be a regular executable file.
  - No shell is used, except "Run in Terminal" with the user's own line.
- **Icons:** theme names, or absolute paths from desktop files, loaded by
  Qt's icon loader.
- **Logs:** the journal never gets query text, file names or paths of what
  the user ran; only timings, counts and error kinds.
- **The service unit:** it cannot use `NoNewPrivileges`, seccomp or
  `RestrictSUIDSGID`. Apps started from it inherit them, which would break
  `sudo` and `pkexec` in a terminal launched from the launcher. It gets
  `Restart`, `MemoryHigh` and `OOMScoreAdjust=200` only, as plasmashell and
  krunner did.

## Failure modes (R)

| What fails | What the user sees |
|---|---|
| The launcher crashes | systemd restarts it (≤ 1 s); D-Bus activation starts it on the next Meta or dock click. A crash during a query records the running KRunner plugins; after 2 crashes in the same plugins within 10 min, that plugin is skipped until the next login and the log names it |
| A KRunner plugin is slow | The late phase stops waiting after 400 ms; matches that arrive later for the same query are still merged below the top hit, up to 2 s |
| Explorer's index is missing, slow or erroring | Recent files only; the file row group says "File search is unavailable" once per session |
| Settings index missing or invalid | No Settings results; one warning in the log |
| `usage.tsv`, `pinned.list` or `launcher.conf` corrupt | Bad lines skipped; the file is rewritten on the next change |
| Disk full or file unwritable | Changes kept in memory, one warning; nothing is lost from the old file (atomic writes) |
| A pinned app is uninstalled | Hidden, but kept in `pinned.list`, so a reinstall brings it back |
| A launch fails | A notification with the job's error |
| A power or session call fails | An inline message in the panel's footer |
| No layer-shell (not KWin, or X11) | A frameless always-on-top window, centred; logged |
| Screens change while open | The panel hides |
| The button plasmoid loads before the service | Its D-Bus call activates the service |

## Accessibility

- **Focus is real.** Each list, grid and button takes keyboard focus, and
  `activeFocus` follows the current item. This avoids Kickoff's
  [337267](https://bugs.kde.org/show_bug.cgi?id=337267).
- **Roles:** the field has `Accessible.name` "Search apps, settings and
  files". The results are `List` and `ListItem`, named "<name>, <kind>". The
  pinned and all-apps views are `List`. The power menu is a real menu.
- **Announcements:** after each query settles (500 ms), the result count and
  the top hit, through `Accessible.announce` ("12 results. Top hit: Firefox,
  app").
- **Context menu:** opens from the keyboard (Menu, Shift+F10).
- **Reduced motion and transparency:**
  - Reduced motion (AccessibilityState) turns the open animation off.
  - The shared transparency switch (`atlasrc [Appearance] Transparency`)
    turns blur off and makes the panel opaque.
- **Checks:** an AT-SPI test in a private bus checks the roles, names and
  focus order; Orca itself is checked in the VM.

## Budgets

| What | Budget | Measured how |
|---|---|---|
| Meta to the panel mapped (warm) | ≤ 40 ms (hard limit 80 ms) | KWin script, D-Bus call to `windowAdded`; plus in-process trace (D-Bus received to `frameSwapped`) |
| First open after login | ≤ 80 ms | Same, after a fresh start |
| Keystroke to first results painted | ≤ 30 ms (core query ≤ 3 ms at 1,000 apps + 200 settings) | Key event timestamp to `frameSwapped`; core bench |
| Late results (KRunner, files) | painted ≤ 150 ms after the keystroke, p95 | Trace |
| Idle CPU, hidden | 0 %: no wakeups over 60 s | `/proc/<pid>/status` context switches, `perf stat` |
| RSS hidden, after first use | ≤ 110 MB (PSS ≤ 70 MB) | `smaps_rollup` |
| Net idle memory for AtlasOS: (plasmashell without Andromeda + launcher) − (plasmashell with Andromeda), idle and after 20 opens | ≤ +40 MB; rarely used KRunner plugins load lazily if needed | Coordinator's VM run, `smaps_rollup` |
| RSS after 1,000 open/close cycles and 1,000 queries | ≤ +5 MB over the above | Same |
| Startup at login to ready (hidden, warm) | ≤ 400 ms | Trace |
| `usage.tsv` | ≤ 256 KiB | Line cap |
| RPM | ≤ 10 MB | `rpm -qp --qf %{size}` |

The memory budget is set against what the launcher removes: plasmashell's
growth with Andromeda open, plus the KRunner daemon that AtlasOS already
retired. The P phase reports the baseline from the coordinator's VM run.
Software rendering (the Atlas default) is measured against the GPU; the
cheaper of the two that meets the open budget wins.

## Image changes (handed to the coordinator; the launcher never edits the AtlasOS repo)

1. Install the `atlas-launcher` RPM:
   - `/usr/bin/atlas-launcher`;
   - the user unit `atlas-launcher.service`;
   - `/usr/share/dbus-1/services/net.eterneon.atlas.launcher.service`;
   - the plasmoid `/usr/share/plasma/plasmoids/net.eterneon.atlas.launcher.button/`;
   - `/etc/xdg/atlas-launcher/pinned.list`;
   - the desktop file `net.eterneon.atlas.launcher.desktop` (NoDisplay).
2. `systemctl --global enable atlas-launcher.service` in `build.sh`.
3. The dock layout: `net.eterneon.atlas.launcher.button` in place of
   `AndromedaLauncher` as the dock's first item.
4. Remove `system_files/usr/share/plasma/plasmoids/AndromedaLauncher/`, and
   `org.kde.plasma.simplekickoff/` if it is still there.
5. `/etc/xdg/kwinrc`:
   ```
   [ModifierOnlyShortcuts]
   Meta=net.eterneon.atlas.launcher,/net/eterneon/atlas/launcher,net.eterneon.atlas.Launcher1,ToggleStart
   ```
6. Alt+Space, Alt+F2 and Meta+S: no image change. The launcher registers
   them with KGlobalAccel. KRunner's shortcuts file stays deleted.
7. A Plasma update script, `atlasos-2026MMDD-launcher.js`. In every panel it
   replaces AndromedaLauncher (and kickoff, kicker, kickerdash or
   simplekickoff) with the button at the same position, and copies the old
   `favoriteApps` into the button's `ImportPins`.
8. The default pins in `/etc/xdg/atlas-launcher/pinned.list`, taken from
   Andromeda's `favoriteApps` default with the AtlasOS apps.
9. Keep the KRunner library and its plugins (already kept) and kmenuedit.
   Remove nothing else.

The exact text of each change goes to the coordinator at ship time, tested in
the VM.

## Parity with Andromeda (closed before ship)

| Andromeda | Launcher |
|---|---|
| Favourites grid, drag to reorder | Pinned |
| All apps, list or grid | All Apps A–Z with letter grid (no category view) |
| Recent apps and documents | Recent |
| KRunner search (all enabled plugins) | Search |
| Avatar, name, settings, lock, power | Footer |
| Right-click: desktop actions, Add to Favourites, Edit Application, Uninstall (Discover) | Context menu |
| Drag an app to the desktop or dock | Drag out as `text/uri-list` of its desktop file (F phase checks drag from a layer surface) |
| "Pin to Task Manager" | Through the dock button. The open question for the F phase: Plasma offers no API outside the shell, and `evaluateScript` with a validated id is the candidate |
| Edit Applications… | Panel context menu |
