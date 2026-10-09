# Launcher: security

The threat model of Telamon Launcher: what it protects, who it defends against,
the rule each entry point follows, where the rule is enforced and the test that
keeps it true. `docs/DESIGN.md` ("Trust") says what is trusted in one list; this
file says why, and what is left. Change it together with the code it describes:
some of the tests below read the QML and the C++ and fail when a rule here is
skipped.

Report a vulnerability privately to the maintainer through GitHub's "Report a
vulnerability" on the repository (Security tab), not in a public issue.

## What the launcher is, security-wise

- It runs as the signed-in user, in the session, and has **no privilege of its
  own**: no setuid file, no polkit action, no root helper, no service of its
  own on the system bus. Power and session go through logind and ksmserver,
  which ask polkit themselves. A bug in the launcher can therefore not do more
  than the user, or the user plus a prompt they accept, could do.
- It is **resident**: one process per session (`telamon-launcher.service`),
  with its panel created and hidden, so a key press only has to show it. It
  owns two session bus names and is D-Bus-activatable.
- It **starts programs on the user's behalf**: apps, files, typed commands,
  Settings pages, power actions. It does so only from the user's own input in
  the panel (Enter, a click), never from a D-Bus call, a file it read or a
  search result arriving by itself.
- It **shows text it did not write**: app names, comments and actions from
  desktop files, file names, KRunner matches, Settings page titles, the
  account's real name, and the query itself.
- It **reads and writes the user's files**: its own (`pinned.list`,
  `names.conf`, `usage.tsv`, `launcher.conf`) and other programs' (the
  recently-used list, the desktop files of the apps, Settings' search index).
  It writes desktop files into `~/.local/share/applications` (Rename App).
- It holds **no secrets**. What it keeps about the user is what they ran
  (`usage.tsv`: ids of apps, Settings pages and session commands, with the
  first 8 characters of the query that found them) and the names they gave
  apps.
- It uses **no network**. The web-search row hands an https URL to the user's
  browser. The typed text goes to the KRunner plugins the user has switched
  on, as it does in KRunner itself (see "What is left").

## Who we defend against

| Attacker | What they can reach | Defended? |
|---|---|---|
| **A hostile program in the session**, typically a sandboxed (Flatpak) app that was given a folder or a D-Bus name to talk to, or a compromised app | The launcher's D-Bus names, the folders it was given (`~/.local/share/applications`, `~/.local/share/recently-used.xbel`, an icon path), the session bus | Yes: this is the main model. What arrives from it is validated (1 to 6) |
| **A planted file** in the folders the launcher reads (desktop files, the xbel list, icons, its config) | Names, comments, icons, paths, huge or special files (pipes, links, devices) | Yes: sizes, file types and link targets are checked before use; text is cleaned (3, 5) |
| **Another local user** | Their full name (AccountsService), files they own that a path names | Yes: shown as plain text, cleaned |
| **Hostile text from a program the user trusts**: a KRunner plugin, a file name, a Settings title | Anything that is shown or becomes an action | Yes: cleaned and capped where it enters, drawn as plain text, an action is rebuilt from checked parts (6, 8) |
| **The supply chain** | crates.io, the pinned git dependency, the build image, CI actions | Partly: locked and pinned builds, `cargo-deny`, `cargo-audit`, weekly (11, 12) |

Out of scope, because it is not the launcher's to stop:

- code running as the same user outside any sandbox. It can already do
  everything the launcher can, read the user's files, kill the launcher and
  take its bus name;
- a malicious or compromised OS image, the kernel, or physical access;
- bugs in the services the launcher calls (logind, ksmserver, KIO, KRunner and
  its plugins, AccountsService, polkit, the file manager, Settings, the Store);
- what the pinned `atlas-framework` crates do inside their own boundary (we
  review how the launcher calls them, below).

## 1. The D-Bus surface

Two session bus names, both on the same process:

| Name | Path | Interface |
|---|---|---|
| `net.eterneon.telamon.launcher` | `/net/eterneon/telamon/launcher` | `net.eterneon.telamon.Launcher1`, and KDBusService's `org.freedesktop.Application` and `org.kde.KDBusService` |
| `net.eterneon.atlas.launcher` (the name before the rename, for one more release) | `/net/eterneon/atlas/launcher` | `net.eterneon.atlas.Launcher1`: the same methods, forwarded to the new adaptor |

**Who can call what.** The session bus is per user: every process of the user
can call every method, and the bus gives no per-method policy or caller
identity worth checking (a same-user caller can do anything the launcher can).
So the rule is about *what a call can make the launcher do*:

- `ToggleStart`, `ToggleSearch`, `Show(mode, query, platform_data)`, `Hide`,
  `SetDockAnchor` open, place and hide the panel. They take no path, no
  command and no result id. `Show` types a query into the field and runs
  nothing: after a `Show` that typed a query, Enter does nothing until the
  user has edited the text, and a click on a result waits 500 ms
  (`Panel.qml`: `needsInteraction`, `clickAllowedAt`), so another process
  cannot line a query up under the user's Enter or click.
- `ImportPins(as)` takes at most 64 ids; the backend accepts each only as a
  desktop file id or a `preferred://` name, and only while the user has no
  `pinned.list`. `ClearHistory()` deletes the launcher's own `usage.tsv`.
- KDBusService's `Activate` shows Start; `CommandLine(args)` accepts the four
  flags of the program (`--start`, `--search text`, `--hide`, `--daemon`) and
  parses the rest away without exiting; `Open(uris)` and `ActivateAction` are
  not connected to anything. None of them starts a program.
- **The text of `Show` and of `--search`** is cut at 256 characters on arrival
  and cleaned of control, bidi and invisible characters by the Rust core
  (`Backend::cleanQuery`, called by `Panel.qml` before the field gets it), so
  the field shows what is searched for.
- Bad arguments (wrong types) are refused by Qt's D-Bus layer (`UnknownMethod`).
  A `Show` with a mode that is neither `start` nor `search` is **ignored**
  (nothing opens); it is not answered with an error: Qt sets a `QDBusContext`
  on an adaptor's *parent*, not on the adaptor, so `sendErrorReply` in the
  adaptors does nothing and a `ClearHistory` call may return before the file is
  gone. That is a reliability gap, not a security one (see "What is left").
- `SetDockAnchor` and `ToggleStart(a{sv})`: the screen name at most 64
  characters, the rectangle four integers within ±65,536 with a positive size;
  anything else is ignored. A caller can send a huge array (the bus limits the
  message): it is read once and dropped (tested with 2,000,000 elements).

**The surface is exactly the documented one.** Before 0.4.0 the adaptor also
exported an internal method (`queueClearHistory`) and two internal signals
(`importPinsRequested`, `clearHistoryRequested`, which broadcast whatever a
caller handed `ImportPins`): an adaptor's public slots and signals are all
exported by Qt. They are plain methods and direct calls now. *Test:*
`scripts/headless-dbus-security.sh` introspects both names on a private bus and
requires exactly the documented methods, no signal of its own and no argument
that is a path or a descriptor; `cpp_guards.rs`
(`the_dbus_adaptors_export_methods_and_nothing_else`) reads `dbus.h` and
`dbus.cpp`.

**Name ownership.** `KDBusService::Unique` takes `net.eterneon.telamon.launcher`
without queueing and without allowing replacement; the old name is taken with
Qt's defaults (do not queue, do not allow replacement). *Test (probe):* another
connection's `RequestName(..., REPLACE_EXISTING | DO_NOT_QUEUE)` is answered
"exists" for both names. Two things a bus cannot prevent, both needing a
process of the same user:

- **A squatter that owns the name before the launcher starts** receives the
  launcher's forwarded start (`org.kde.KDBusService.CommandLine`) and, from
  then on, the dock button's calls (`ToggleStart`, with the screen and the
  button's rectangle; `SetDockAnchor`; `ImportPins` from the plasmoid's
  configuration). It never receives a typed query: those are typed into the
  panel, not sent. The launcher then does not get the name: it forwards its
  start to the squatter, as `KDBusService::Unique` does for a second start.
  *Probe:* reports the forwarded call.
- **A process that queues for the name** (`RequestName` without `DO_NOT_QUEUE`
  is answered "in queue") gets it if the launcher exits, ahead of the restart
  500 ms later. The same limit applies.

Neither gives more than a stolen "open Start" notification, which a process
of the user can also get by sending the key press itself. The plasmoid does not
check who owns the name: there is nothing it could do that a squatter could
not also do by being the owner.

**The plasmoid** (`plasmoid/`, the dock's button) is QML only. It calls the one
bus name, path and interface above, with `ToggleStart`, `SetDockAnchor` and
`ImportPins` (at most 64 ids from its own configuration), through one helper;
it has no `Executable` data source, no `Process`, no file or network access of
its own. *Test:* `qml_text.rs` (`the_dock_button_talks_to_the_launcher_only`).

## 2. Starting things: no shell, no argument injection

Everything runs from `Executor` (C++), after the user's Enter or click, and only
through KDE's launch jobs with an argument vector:

| What | How | Checked before |
|---|---|---|
| An app, or one of its desktop actions | `KIO::ApplicationLauncherJob` (service, or service action) | The id is `[A-Za-z0-9._-]{1,255}` ending in `.desktop` (`Validate::desktopId`). The action is found by name among the service's own actions. |
| A file or folder | `KIO::OpenUrlJob`, `setRunExecutables(false)` | `Validate::fileUrl`: a `file:` URL, no host (or `localhost`), no query, fragment, user, port, NUL, `.` or `..` segment, at most 2,048 bytes |
| A web search | `KIO::OpenUrlJob`, `setRunExecutables(false)` | `Validate::webUrl`: `https`, a host, no user, no port, no control character |
| A typed command | `KIO::CommandLauncherJob(executable, args)` | `Validate::commandPath`: an absolute path (the one the `PATH` scan found) and at most 256 arguments |
| "Run in Terminal" | `KTerminalLauncherJob(line)` | A non-empty line of at most 4,096 characters. **The one place a line goes to a shell**, because that is what the row says |
| A Settings page | D-Bus `ActivateAction("open", [link])` | `Validate::settingsLink`: `[a-z0-9][a-z0-9-]*(/...)`, at most 128 bytes |
| Uninstall (Flatpak) | `/usr/bin/telamon-store --remove <id>` (then `/usr/local/bin`) | `Validate::flatpakId`: reverse-DNS, no leading dash (`\A...\z`, so a trailing line break is refused) |
| Power and session | One fixed table of D-Bus calls (logind, ksmserver, `org.kde.Shutdown`, `org.kde.LogoutPrompt`); the name picks a row, nothing is spliced into a call | The name is one of seven |
| A KRunner match | `RunnerManager::run(match)`, for the match the launcher kept for that id | The id must still be one of the kept matches |

- **The desktop file id check is the second line** (the Rust core only hands
  on ids it validated). It exists because `KService::serviceByStorageId`
  accepts an *absolute path* as a storage id and loads any desktop file there:
  measured in the build container, `serviceByStorageId("/tmp/x.desktop")`
  returns a service whose `Exec` would run, `"../../tmp/x.desktop"` does not.
  Before 0.4.0 `launchApplication` only checked the id was non-empty and short;
  no caller could reach it with a path, but one would have run it.
- **No shell is built from text.** `CommandLauncherJob(executable, args)` was
  checked on a path containing spaces and `$(touch ...)` with no arguments: it
  runs that file, and nothing from the name is interpreted. A line with shell
  syntax or odd quoting (`| ; & $ > < \` ( ) { } * ? ~ [`, a newline, a
  comment) is offered only as "Run in Terminal".
- **Exec field codes, `TryExec`, `Terminal`, `Path`, `DBusActivatable`** are
  handled by KIO's `ApplicationLauncherJob` and KService, not by the launcher;
  we splice nothing into an `Exec` line. Measured with KSycoca in the build
  container: `Hidden=true`, a missing `TryExec` and `Type=Link` are dropped by
  the builder; `NoDisplay`, `OnlyShowIn` and `NotShowIn` are filtered by the
  launcher's own query (`isApplication && !noDisplay && showInCurrentDesktop &&
  showOnCurrentPlatform`); a `DBusActivatable` entry without `Exec` is listed
  and started by D-Bus activation.
- **A desktop file is a program.** Whoever can write a desktop file the
  desktop reads (the user's `applications` folder, an exported Flatpak) can
  make the launcher *list* it and, if the user picks it, run its `Exec`. That
  is what a desktop file is; it is out of scope. What the launcher does is
  make sure the file cannot run anything *else*, show anything misleading
  beyond its name (3), or hurt the launcher (3, 5).
- **No D-Bus method runs a result.** See 1; `cpp_guards.rs` fails if
  `dbus.cpp` mentions the executor or a KIO job.
- *Tests:* `apps/telamon-launcher/tests/validate_test.cpp` (ctest, run by the
  package's `%check`): ids, links, Flatpak ids, file URLs, web URLs and command
  paths, with NUL, `..`, encoded dots, other hosts, users, ports, trailing line
  breaks and 10 MB strings; `cpp_guards.rs`
  (`every_launch_goes_through_a_kio_job_with_the_checks_before_it`,
  `nothing_builds_a_shell_command_in_cpp`): the checks come before the lookups,
  every `OpenUrlJob` sets `setRunExecutables(false)`, there is one
  `CommandLauncherJob` and one `KTerminalLauncherJob`, and no other source
  starts a process.

## 3. Hostile desktop files, names and keywords

A desktop file reaches the launcher as KService data: `Name`, `GenericName`,
`Comment`, `Keywords`, `Categories`, `Icon`, the program of `Exec`,
`X-Flatpak`, the actions. The C++ catalogue (`catalog.cpp`) hands them to the
Rust core as plain data (at most 20,000 apps); `Catalog::new` validates every
field (`sanitize`) and drops what fails:

| Field | Rule |
|---|---|
| Desktop id | `valid_desktop_id`, else the entry is dropped; duplicates dropped; at most 10,000 apps kept |
| Name, generic name, comment | `clean_display`: control, bidi, zero-width, variation-selector, tag, filler, Braille-blank and object-replacement characters become spaces, runs of white space one space, ends trimmed, at most 512 characters, cut with "…"; an empty name falls back to the id |
| Keywords, categories | At most 64 each, each cleaned and cut at 64 characters |
| Executable name | The first word of `Exec` past `env` and `VAR=value`, its file name, cleaned, at most 255 characters; used only for matching, never launched |
| Icon | A theme name `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`, or an absolute path (no scheme, no `//`, no `..`, no control character, at most 4 KiB); else a generic icon. In C++ an absolute path is kept only if the core says it is a **plain file** (`text::icon_file_ok` through `telamon_launcher_icon_ok`: non-empty, regular, at most 16 MiB, links followed as icon themes use them, not on `/proc`, `/sys`, cgroup, debugfs...; two `stat`s on a pool thread, no `open`). Measured: with `Icon=` naming a pipe the image loader stood in `open()` on the GUI thread (`wait_for_partner`) when the Start page drew the app, and the launcher never answered again |
| Actions | At most 32; the id `[A-Za-z0-9._-]{1,64}`; the name cleaned and non-empty |
| `X-Flatpak` | `valid_flatpak_id`, else none (Uninstall is not offered) |

- **Special files named `*.desktop`.** A pipe, a link to `/dev/zero` and a
  2 GiB sparse file in `~/.local/share/applications` were measured: KDE's
  database builder skips them and the launcher stays responsive (the probe
  seeds all three).
- **What reaches QML is plain text.** Names land in `TelamonLabel` (plain by
  default, and every label that shows one also says `textFormat:
  Text.PlainText`), `TelamonToolTip` (plain) and accessible names; there is no
  `Text`, `Label` or `TextEdit` without `Text.PlainText` anywhere, and
  `ToolTip.text` (which the style draws in a format of its own) is not used.
  A desktop action's name is the text of a menu item: Qt reads `&` in it as a
  mnemonic marker (an underlined letter), nothing more. *Test:* `qml_text.rs`.
- **Names that look alike.** Control, bidi and invisible characters are removed,
  so two names cannot look the same while differing in what is not shown. A
  hostile desktop file can still be *named* "Firefox": the panel shows names
  and comments, not paths (see "What is left").
- **The rename marker** (`X-Telamon-Renamed=true`, `X-Telamon-Original-Name`)
  is honoured only for desktop files in `$XDG_DATA_HOME/applications`, where the
  launcher writes (`catalog.cpp`). In any other file it is just a key, so a
  system file cannot make the launcher show a different name from the rest of
  the desktop. *Guard:* `cpp_guards.rs`
  (`catalogue_icons_and_option_files_are_vetted_before_use`).
- **Thousands of entries.** The catalogue is built and searched on the search
  worker, never on the GUI thread; it is capped at 10,000 apps, and the cost is
  linear in the fields' caps. Measured with the hostile worst case (10,000 apps
  with every field at its cap, 64 keywords of 60 characters, 32 actions): the
  catalogue is built in 2.5 s and a 51-word query that matches everything
  takes 0.5 s, a typo query 0.1 s (release build; `cargo test --release -p
  telamon-launcher-core --test props -- --ignored --nocapture`).
- *Tests:* `props.rs` (`catalog_entries_come_out_safe`,
  `names_rename_into_the_catalogue_safely`, the `corpus` tests with 10 MB
  strings and hostile entries), `catalog.rs` unit tests.

## 4. Renaming apps: the override writer

Rename App writes a desktop file of the app's id into
`$XDG_DATA_HOME/applications`: the system file with `Name` changed and two
marker keys (`overrides.rs`; DESIGN.md, "App names"). The folder is one that
other programs, sandboxed ones with `xdg-data/applications` access among them,
can write to, so every step assumes the entry may have been tampered with:

| Concern | Rule | Test |
|---|---|---|
| **Path traversal in the id** | The id must be `valid_desktop_id` (`[A-Za-z0-9._-]`, `.desktop`, 255 bytes) in `apply`, `remove`, `sync` and `find_original`; a menu-folder form (`kde-foo.desktop` as `kde/foo.desktop`) never uses an empty, `.` or `..` folder | `a_dash_never_makes_a_path_out_of_the_folder`, `props::the_override_writer_stays_in_its_folder`, `corpus::the_override_writer_and_the_worst_ids` |
| **Writing outside the folder** | The only path written is `<applications>/<id>`; the property test applies random ids and names and requires every file outside the user folder to be byte-identical afterwards, and every new file to have a valid id | `props::the_override_writer_stays_in_its_folder`, `props::sync_keeps_to_its_folder` |
| **A link at the target** | `existing` looks with `lstat`, reads with `O_NOFOLLOW` (`read_capped_nofollow`) and the write is `write_atomic_nofollow`: the final name is never followed, the target must be absent or a regular file, and the new file replaces a name by `rename`, never writes through a link. A link, a pipe or a folder at the name is `NotOurs`: not read, not replaced, not removed | `a_file_that_is_not_ours_is_never_touched`, `a_link_in_the_way_is_neither_read_nor_written_through`, `fsutil::nofollow_*` |
| **A race: a link, or a file, appears between the look and the write** | A name that was absent is taken with `renameat2(RENAME_NOREPLACE)`: a file that appeared in between is not clobbered. A swap of the file *of ours* for another between the look and the `rename` still replaces it (that is a file the same user made); the rename replaces the name and never follows a link | `fsutil::a_name_that_appeared_is_not_clobbered_by_a_new_file` (the primitive; the race itself is reasoned, not tested) |
| **A collision with a system id** | That is the point: a user file of the same id replaces the system one for the whole desktop, exactly as KDE's menu editor does. It copies the system file the desktop would have used (the first folder that has the id, also as a menu subfolder; if that file is too large, not text or not an application, there is none and a lower folder's file is not used) | `an_unusable_first_file_is_not_replaced_by_a_lower_one`, `the_first_system_dir_wins_as_in_the_desktop` |
| **Marker spoofing** | Only a file with `X-Telamon-Renamed=true` in `[Desktop Entry]` is ever changed or deleted (a user's own file, or another tool's, never is). The marker is not authenticated: any process of the user can write a file with it, and the launcher will then overwrite or remove *that file*, which that process could do itself. The marker alone in another group, as `false`, or in a non-UTF-8 file is not the marker | `the_marker_alone_in_a_foreign_group_or_file_is_not_ours` |
| **Injection through the name** | `clean_name` (control, bidi, invisible characters out; tabs and line breaks are spaces; at most 64 characters) runs where the name enters, **and `render` refuses any name that `clean_name` would change**, so no caller can put a line break, a carriage return, NEL, U+2028/2029, an escape, a NUL or bidi controls into the `Name=` line and add a key such as `Exec` to a file the desktop reads. A backslash is doubled (the desktop-file escape). The original name copied into `X-Telamon-Original-Name` has its line breaks replaced | `a_name_cannot_add_a_key_to_the_desktop_file`, `apply_with_an_unclean_name_writes_nothing`, `props::render_never_adds_a_key` (every output line is a line of the original or one of three of ours) |
| **Modes** | New file `0644` (it is a copy of a world-readable system file and holds nothing secret); an existing one keeps its permission bits, never setuid, setgid or sticky; the temp file is created `O_CREAT|O_EXCL|O_NOFOLLOW` in the same folder through a directory descriptor and `fchmod`-ed before the rename | `fsutil::nofollow_write_creates_and_replaces_regular_files`, `apply_writes_updates_and_removes` |
| **Atomic** | temp file, `fsync`, `renameat`, `fsync` of the folder; a crash leaves the old file or the new one; stale temp files (`.<name>.tmp-<pid>-<n>`, regular files, older than a day) are swept at start | `fsutil` tests, `stale_temp_files_are_swept` |
| **Size** | A desktop file read is at most 256 KiB of UTF-8 text with `Type=Application` (an override is refused if it would be larger); `sync` looks at the first 8,192 entries of the folder | `props` |

Not in the table: the sweep in `sync` reads each file of the user's folder whose
name is a valid desktop id (to see whether it is ours), up to 8,192 files of
256 KiB: 2 GiB of reads at the worst, on the IO worker, once per catalogue
change. See "What is left".

## 5. Files the launcher reads and writes

**Reading.** Every file of the launcher's own code goes through `read_capped`:
opened `O_NONBLOCK` (a pipe put there cannot hang the worker), refused unless
it is a regular file, read at most `cap + 1` bytes (the size is not trusted),
`InvalidData` over the cap. All of it is off the GUI thread, on the IO and scan
workers. The files that Qt and KDE's libraries open for the launcher are the
hard case: they open the file as it is, on whichever thread asks (the last two
rows).

| File | Owner of the risk | Cap and rule |
|---|---|---|
| `recently-used.xbel` | Any app can write it | 4 MiB; at most 10,000 `<bookmark>` elements; XML depth 32; custom entities are never expanded (quick-xml has no DTD support: a billion-laughs document yields nothing); only `file:` URLs with no host (or `localhost`), absolute, no `.` or `..` or empty segment, no NUL, at most 4,096 bytes, percent-decoded to valid UTF-8; the URI is **rebuilt from the checked path**; dates more than a day ahead count as unknown; the 500 newest are kept; names cleaned; an entry is shown only if the file exists. A pipe, a link to `/dev/zero` or a folder in its place is an error, not a hang |
| Settings' `search-index.json` | Root-owned, but another app's | 2 MiB; at most 5,000 entries; `version` must be 1; links `[a-z0-9][a-z0-9-]*(/...)`, at most 128 bytes, used only as an argument to Settings' `ActivateAction("open")`; titles, sections and keywords cleaned and capped; `serde_json` limits the nesting |
| `pinned.list` | The user's | 64 KiB; 256 pins; each `valid_pin` (a desktop id or `preferred://browser/filemanager/terminal`); bad lines skipped and counted |
| `names.conf` | The user's | 256 KiB; the `[Names]` group only; 400 names; ids valid desktop ids; names through `clean_name` |
| `usage.tsv` | The user's | 256 KiB; 2,000 entries; prefix at most 8 characters; ids only `app:`, `setting:`, `session:`; counts clamped; a time more than a day ahead is clamped |
| `launcher.conf`, `krunnerrc`, `kdeglobals` (KConfig) | Settings and Plasma write them | Read by KConfig as it does everywhere in KDE. Measured: a pipe in the place of any of them, or a 1.5 GB sparse `launcher.conf`, does not stop the launcher (the probe seeds the pipes) |
| `state.conf` (Qt `Settings`: the Start page's view) | The panel | **Measured: a pipe in its place blocked the GUI thread in `open()` while the panel loaded, and the launcher never finished starting.** Now `Options::stateConfig` hands the QML the file only if it is absent or a regular file of at most 64 KiB; otherwise `/dev/null` (nothing is remembered) |
| `$XDG_STATE_HOME/telamon-launcherstaterc` (KRunner's state, `KSharedConfig::openStateConfig`) | KRunner | **Measured: the same hang with a pipe here**, in `Runners::manager`. Now `runnerStateConfig` opens it only if absent or a regular file of at most 64 KiB, else the state goes to `/dev/null` |

**Writing.** `write_atomic`: the directory is opened once and the temp file
(`O_CREAT|O_EXCL|O_NOFOLLOW`), the `fchmod`, the `rename` and the unlink all go
through that descriptor; `fsync` of the file and of the folder; **new files are
`0600` in folders created `0700`**; an existing file keeps its permission bits
(setuid, setgid and sticky are never kept); a symlink at the final name is
**written through**, on purpose, so a dotfiles manager's link keeps working
(the target is replaced by rename in its own folder; the link is not).

That last rule is a known trade: a process that can make a link at
`~/.config/telamon-launcher/pinned.list` can make the launcher replace the
link's target with a pin list (header, then desktop ids, so it can destroy a
file but not choose what it says beyond that). It needs write access to the
launcher's own config folder, which a sandboxed app is not given by any portal;
and an unsandboxed one can already edit the target itself. The one folder that
sandboxed apps *are* commonly given, `applications`, uses the no-follow writer
(4).

*Tests:* `fsutil.rs` unit tests (caps, links, modes, directory modes, stale
temp files), `props.rs` (`pins_parse_is_safe_and_round_trips`,
`names_parse_is_safe_and_round_trips`, `usage_load_is_safe_bounded_and_stable`,
`recent_files_parse_is_safe_and_bounded`, `settings_index_parse_is_safe`,
`write_then_read_returns_the_bytes`, and the corpus: `files_that_are_not_files_are_not_read`
(a FIFO, a link to `/dev/zero`, a folder, an 8 MiB sparse file),
`a_huge_xbel_is_refused_and_a_deep_one_cut` (a 4 MiB+1 file, 100 levels of
nesting, an entity bomb, 12,000 bookmarks), `config_files_have_caps_and_modes`).

## 6. Results from other programs

- **KRunner matches** (plugins in process, D-Bus runners out of process; the
  user's `krunnerrc` decides which): the C++ side takes at most 200 matches a
  batch and hands the Rust core plain data; `late::runner_item` accepts a
  match only with a runner id `[A-Za-z0-9._-]{1,128}`, a match id of at most
  1,024 bytes without control or invisible characters and a non-empty cleaned
  title (256 characters; subtitle 512); icons pass the same icon check as any
  other and, when it is an absolute path, must be a plain file (the search
  worker `stat`s it, as for apps: `vetted_icon`); relevance is clamped to 0..1.
  The action is **never a command from the
  match**: it is `RunnerManager::run` on the match the launcher kept for that
  `(runner id, match id)`. Plugins in process are trusted as KRunner trusts them.
  *Test:* `props::runner_matches_are_checked`, `corpus::late_batches_are_cut_before_they_are_ranked`.
- **Explorer's file index** (`net.eterneon.telamon.explorer.Search`,
  `Search1.Search(...) -> a(sssssxtd)`): **the launcher has no client for it in
  0.4.0**. DESIGN.md describes the interface, and the core has the checks
  (`late::explorer_item`: a clean absolute `file:` path rebuilt from the hit, the
  shown name taken from the *path* and not from the hit's `name`, the icon from
  the path and mime, at most 200 hits) with tests, but nothing in the C++
  calls the bus name. When the client is written it must also: (1) take the
  reply only from the owner of the name, checked by `GetNameOwner` and the
  owner's executable (`GetConnectionUnixProcessID` and `/proc/<pid>/exe` is
  the index daemon), since any same-user process can take the name while the
  daemon is not running; (2) expect exactly `a(sssssxtd)` and treat any other
  type as an empty answer; (3) call asynchronously with a timeout; (4) cap the
  rows read before any copy. Until then there is no name to squat and no reply
  to mistype, and the file rows come from the recently-used list (5) only.
  *Test:* `props::explorer_hits_are_rebuilt_from_a_clean_path`.
- **The Settings index**: 5 (it is a file, not a call).
- **A name squatter on `org.freedesktop.FileManager1` or Settings' names**
  receives the file URI of "Show in Folder" or a Settings link and the
  activation token. Both are things the user just chose; the token is a
  one-time focus permission. It is what every KDE app does.

## 7. The calculator, the web row and typed commands

- **Calculator** (`calc.rs`): at most 256 characters and 256 tokens; recursive
  descent with every recursion through one function that stops at depth 64;
  numbers are `f64` and every result must be finite (an overflow is "no
  calculation", never `inf`); no factorial, no loops, no big numbers, so no
  work that grows with the *value* (`9^9^9^9` is `inf`, refused at once); the
  answer is at most 12 significant digits, at most 32 bytes. Units: a fixed
  table. *Tests:* `props::calculator_never_panics_and_answers_stay_small`,
  `corpus::calculator_worst_cases_are_refused_quickly` (nesting 255 deep, 256
  digits, `1e999999999999999999999`, `0/0`, `10^400`, all together under 2 s
  in a debug build).
- **Web search** (`web.rs`): the URL is one of eight fixed `https://` prefixes
  plus the query percent-encoded (RFC 3986 unreserved bytes stay, every other
  byte is `%XX`) and cut at 1,024 bytes at a character boundary. The query
  therefore cannot add a parameter, a scheme or a host. The browser is started
  by `OpenUrlJob` with `setRunExecutables(false)` after `Validate::webUrl`.
  *Test:* `props::web_urls_are_https_and_percent_encoded`.
- **Typed commands** (`commands.rs`): a "Run" row exists only when the first
  word, as the shell would read it, **is exactly the name of an executable**
  found by the `PATH` scan (absolute `PATH` entries only, at most 64 folders,
  20,000 names, 10,000 entries a folder, 100,000 in all; a regular file the
  user may execute). The row **shows the line as typed**; a line with any white
  space but a plain space, or any invisible, control or bidi character, or one
  that the 256-character cap cut, offers no row at all. The direct run is
  `CommandLauncherJob(<absolute path>, args)` of a split with no expansion of
  any kind; shell syntax means "Run in Terminal" only. *Tests:*
  `props::command_lines_split_without_a_shell`,
  `props::a_typed_command_runs_a_path_executable_or_nothing` (against a
  `PATH` with `a b`, `x;y`, `--help`, `-rf`, `$(id)`, tabs and bidi in the file
  names, and a relative `PATH` entry), `props::every_row_of_a_query_is_safe_to_run`
  (every action of every row of a whole query has the safe shape of 2).

## 8. Displayed text and icons

Everything from outside is drawn as **plain text** (3). The checks:

- `apps/telamon-launcher/tests/qml_text.rs` reads every QML file of the app and
  of the dock button: no `Text`, `Label`, `Heading`, `TextEdit` or `TextArea`
  without `textFormat: Text.PlainText`; `TelamonLabel`, `TelamonTextField` and
  `TelamonTextArea` never switched away from plain; no `RichText`,
  `StyledText`, `MarkdownText` or `AutoText`; no `ToolTip.text`; no `eval`,
  `Function`, `Qt.include`, `XMLHttpRequest`, `WebSocket`, web view,
  `Qt.createQmlObject`, `setSource`, `Loader { source }`, `Qt.openUrlExternally`
  or `Qt.openUrl`; images and avatars only where listed, with the source they
  are listed with (a theme name, a vetted path, the user's picture). The checker
  is tested on snippets that must pass and must fail.
- **The user's picture** (`Actions::trustedIconFile`) is a path AccountsService
  stores: it must be an absolute regular file (`lstat`, not a link) owned by
  the user or root, 1 byte to 16 MiB, with a header that claims at most 4,096
  by 4,096 pixels (read without decoding).
- **The real name** from AccountsService is cleaned (`Validate::displayText`)
  and capped at 256 characters.
- **Icons** are theme names or absolute paths turned into `file://` URLs with
  each segment escaped (`LauncherItemIcon.qml`); `https:` and `file:` URLs from
  data fall back to a generic icon, so no icon reaches the network. An absolute
  path is also a plain file of at most 16 MiB, checked off the GUI thread
  (3; late KRunner icons in `engine.rs`, tested by
  `a_late_icon_that_is_not_a_plain_file_never_reaches_the_loaders`).
- The Rust core cleans every string at the boundary (`clean_display`,
  `clean_name`, `clean_query`); the properties require that nothing with a
  control, bidi or invisible character comes out, whatever goes in
  (`props::clean_*`, `catalog_entries_come_out_safe`).

## 9. Logging and privacy

- **Never logged: what the user typed, file names, paths, ids, app names, the
  names the user gave.** The logs carry timings, counts and error kinds.
  Every core type that holds such text prints redacted under `{:?}`
  (queries, results, actions, runner matches, file hits, recent files, the
  usage store and its keys, calculations, command plans, prepared fields,
  names), so a stray debug print cannot leak. *Tests:* `corpus::canary_text_never_reaches_a_debug_print`,
  `props` (a canary in a query, a name, a match, a hit, a command is never in a
  `Debug` string or in `usage.tsv` beyond 8 characters), `cpp_guards.rs`
  (`the_launcher_logs_no_text_from_outside`: no log macro of the C++ streams a
  query, text, path, URL, id or command, and none of the Rust formats one).
- **Measured on a real process:** the probe seeds an app, a recent file and a
  user-given name with a canary string, starts the launcher with the
  launcher's own categories at debug level, types canary queries (a plain one,
  a calculation, a command) through `Show`, and requires the canary to be in
  none of the log (`scripts/headless-dbus-security.sh`).
- **Library logs.** With *every* Qt and KDE category at debug level
  (`QT_LOGGING_RULES="*.debug=true"`) KRunner (`kf.runner`) and KIO's URI
  filters (`kf.kio.urifilters.*`) log the text of a query themselves. They are
  off by default and not the launcher's code; the launcher cannot redact them.
- **What is kept on disk about the user:** `usage.tsv` (`0600`, readable only by
  them): only ids that carry nothing private (`app:`, `setting:`, `session:`;
  typed commands, file paths, KRunner ids, calculations and the web row are
  never recorded) with at most the first 8 characters of the query that ran
  them, 2,000 lines. "Learn from use" can be switched off in Settings and the
  file cleared (`ClearHistory`, or deleting it).
- **Where the typed text goes:** to the in-process search, and to every
  enabled KRunner plugin, as KRunner does (`krunnerrc` switches them; the
  web-shortcuts plugin is limited to keyword shortcuts). Out-of-process D-Bus
  runners receive it too.

## 10. The daemon

- **Single instance:** `KDBusService::Unique`; a second `telamon-launcher
  --search` forwards its arguments and exits.
- **Crash behaviour:** `Restart=on-failure`, `RestartSec=500ms`, D-Bus
  activation on the next Meta; worker threads catch panics per message and keep
  running; the panel is created hidden at start, so a crash shows nothing. A
  crash during a query records the running KRunner plugins and skips a plugin
  that crashed it twice (DESIGN.md, "Failure modes").
- **SIGTERM, SIGINT, SIGHUP** end the event loop like a normal quit so the
  files are saved; shutdown waits at most 2 s for the workers.
- **The unit** (`data/telamon-launcher.service`) gets `Restart`, `MemoryHigh`
  and `OOMScoreAdjust=200` only. It cannot use `NoNewPrivileges`,
  `RestrictSUIDSGID`, a seccomp filter or `MemoryDenyWriteExecute`: apps the
  launcher starts directly (not through systemd's transient units) inherit
  them, which would break `sudo` and `pkexec` in a terminal started from it;
  Qt Quick's JIT needs writable-executable memory.
- **Core dumps:** the launcher does not turn them off. A crash is collected by
  systemd-coredump (readable by the user and root, as every crash of the
  user's apps is) and may hold recent text in memory; the framework's crash
  reporter (opt-in, redacted) depends on that collection. Turning dumps off
  would also turn the crash reports off: left for the owner (see "What is
  left"). The development container runs with `--ulimit core=0`, so a test run
  leaves none.

## 11. Build hardening and supply chain

- **RPM flags.** The spec builds with Fedora's `%build_cflags`,
  `%build_cxxflags`, `%build_ldflags` and `%build_rustflags` (stack protector
  strong, `_FORTIFY_SOURCE=3`, `_GLIBCXX_ASSERTIONS`, stack clash protection,
  `-fcf-protection`, PIE, full RELRO and `BIND_NOW`, `-Werror=format-security`,
  Rust's own hardening). The flags are **confirmed on the result**:
  `scripts/check-hardening.sh`, run by `%check` on the installed program, reads
  the ELF back with `readelf` and fails the package build without PIE,
  `GNU_RELRO` with `BIND_NOW`, a non-executable stack, no RPATH, RUNPATH or
  TEXTREL, and stack protectors; `annocheck` (a `BuildRequires`) must agree.
  CI tests the check itself (`scripts/test-check-hardening.sh`) against
  programs built with and without each protection. Measured on the 0.4.0
  package: `check-hardening: 1 file(s) ok`, `Hardened: telamon-launcher: PASS`.
  *Not met:* Intel CET's IBT marking: the C++ is built with
  `-fcf-protection`, but rustc has no stable switch for it and the linker marks
  a program only when every object is marked, so the program carries the
  shadow-stack mark (`SHSTK`) and not `IBT`; the check reports it as a note.
  Linux does not enforce userspace IBT yet.
- **`unsafe`.** Rust `unsafe` is `libc` calls with `SAFETY` comments in
  `fsutil.rs` (descriptor-relative open, rename, unlink, stat, directory
  listing), `commands.rs` (`faccessat`), `overrides.rs` (`utimensat`),
  `legacy.rs` (`renameat2`), and CXX-Qt's bridge and the two C entry points in
  `lib.rs`. Everything that parses is safe Rust.
- **Locked and pinned.** Builds use `--locked`; `atlas-framework` is a git
  dependency pinned by tag (`Cargo.toml`, checked by `ci/framework-ref.sh`; the
  build image is built from the same tag); every GitHub Action is pinned by
  commit; the Fedora base image is `fedora:44`, rebuilt when its inputs change.
  `ci/secret-check.sh` runs before every image push.
- **`cargo-deny`** (`deny.toml`: advisories, licences, bans, sources) and
  **`cargo-audit`** run in CI on every change to the dependencies and every
  Monday (`.github/workflows/security.yml`), so a new advisory against an
  unchanged lock file is seen without a commit. Measured for 0.4.0 with
  cargo-deny 0.20.2 and cargo-audit 0.22.2 against the RustSec database
  (1,295 advisories, 94 crates): `advisories ok, bans ok, licenses ok, sources
  ok`; no vulnerability. Nothing is ignored. Dependabot moves the Actions and
  the crates weekly (`.github/dependabot.yml`); the framework pin is moved by
  hand (CLAUDE.md).
- **Property tests** (`proptest`; every one in a module named `props`) run in CI
  with `PROPTEST_CASES=20000 cargo test --workspace --locked -- props`; a local
  `cargo test` does 256 cases each. The properties that build whole catalogues
  or touch the file system stop at 1,000 to 8,000 cases (a counter in the test
  body: proptest lets `PROPTEST_CASES` override a configured count) so each
  stays under a minute; the `corpus` tests cover their worst inputs. They are
  not coverage-guided fuzzing.
- Releases are tagged by hand and built in the Fedora 44 container; nothing is
  fetched at build time except crates and the pinned git dependency.

## 12. GitHub workflows

`scripts/check-workflows.py` (tests in `scripts/test_check_workflows.py`; run
by `security.yml` on every change to a workflow and weekly) fails the build when
a workflow:

- is triggered by `pull_request_target` or `workflow_run`;
- has no top-level `permissions:`, a writable one, or a job that asks for write
  access without an entry (with its reason) in the checker's list: today the
  image publish (`packages: write`, on main only, after `ci/secret-check.sh`),
  the `ci.yml` job that calls it, and the audit job's `checks: write`;
- uses an action or reusable workflow not pinned to a full commit sha with its
  version in a comment, or a `docker://` action;
- checks out without `persist-credentials: false`;
- puts an attacker-influenced expression (inputs, ref and branch names, step
  outputs, matrix values, event fields) in a `run:` or `script:`: it goes
  through `env:`;
- uses a secret other than `GITHUB_TOKEN`, puts one in a script or says
  `secrets: inherit`;
- pipes a download into a shell, or saves a cache from a step a pull request
  can reach (only a `push` to `main` saves).

The job container image is our own build image (`atlas-launcher-dev:44`, tag
only, listed in the checker with the reason: a digest would have to be
committed after every rebuild). Audit findings fixed in this release: the
cache-save conditions did not name the `push` event, and the audit job and the
checker did not exist.

## 13. Tests, in one place

| Boundary | Where |
|---|---|
| Cleaning, caps, ids, icons, names | `crates/telamon-launcher-core/tests/props.rs` (`props::`), unit tests in `text.rs`, `names.rs`, `catalog.rs` |
| The override writer | `overrides.rs` and `fsutil.rs` unit tests, `props.rs` (`render_never_adds_a_key`, `the_override_writer_stays_in_its_folder`, `sync_keeps_to_its_folder`) |
| Parsers: xbel, Settings index, pins, names, usage | `props.rs`, `corpus::*`, unit tests |
| Calculator, web row, typed commands | `props.rs`, `calc.rs`, `web.rs`, `commands.rs` unit tests |
| Whole queries over hostile data | `props::every_row_of_a_query_is_safe_to_run` |
| What C++ accepts before it starts anything | `apps/telamon-launcher/tests/validate_test.cpp` (ctest, in `%check`) |
| How the C++ is written | `apps/telamon-launcher/tests/cpp_guards.rs` |
| How the QML is written | `apps/telamon-launcher/tests/qml_text.rs` |
| D-Bus names, surface, hostile arguments, logging on a real process | `scripts/headless-dbus-security.sh` (in the dev container, a private bus; not run by CI) |
| Build hardening | `scripts/check-hardening.sh` in `%check`; `scripts/test-check-hardening.sh` in CI |
| Dependencies and workflows | `deny.toml`, `security.yml`, `scripts/check-workflows.py` |

Each fix above was checked by reverting it alone and running its test: the
override name guard, the no-follow read and write, the `RENAME_NOREPLACE` rename,
the mime-type icon name, the plain-file icon check (apps' and late), the
adaptor surface, the desktop id check, the cleaned `Show` text, the QML text
lint, the logging guard, the state-file checks, the marker's folder and the
command row's cleaned subtitle each made a named test fail. The runtime ones
(a pipe as an icon, as `state.conf`, as KRunner's state; the exported signals;
the squatter and the log canaries) are held by the probe, which fails against
the 0.3.4 binary.

## 14. Considered, not an issue

- **`KService::serviceByStorageId` with a path** was reachable by nothing
  (every caller passed an id the core had validated); it is checked anyway (2).
- **A calculator DoS** (`9^9^9^9`, deep parentheses, `1e999999999`): there is no
  loop that grows with a value; refused at once (7).
- **Entity expansion in `recently-used.xbel`**: quick-xml never expands custom
  entities; tested with a four-level bomb.
- **Regex blow-up**: the core uses no regular expressions; the C++ uses two
  anchored, linear patterns (Flatpak ids).
- **Huge queries**: the query is cut at 256 characters (the backend looks at
  1,024 UTF-16 units of a paste before copying it), so the matcher's cost is
  bounded by the catalogue's caps (3).
- **`ImportPins` and `ClearHistory`** as write primitives: the first only when
  no `pinned.list` exists, with validated ids; the second deletes only the
  launcher's own history.
- **A hostile desktop file named like a real app**: it can only be *that name*;
  it can run only its own `Exec` (2, 3).
- **Reading `/proc` or devices through an icon path**: an absolute icon path is
  stat-ed as a regular file of at most 16 MiB before it is used (3).
- **Exec field codes in the typed command**: arguments are literal; the
  executor never expands `%` codes (they are an `ApplicationLauncherJob`
  feature of desktop files).
- **`launcher.conf` as a source of commands**: it holds booleans and one engine
  name, matched against a list of eight.
- **Clipboard**: the calculator's answer, at most 1 MiB, written on Enter.
- **Icons from KRunner matches**: the same name-or-clean-path check, and for a
  path the plain-file check on the search worker (6).

## 15. What is left

- **No Explorer client yet**; the contract in 6 binds the one to come.
- **Other special files in the config folders.** The two files the launcher's
  own code opens on the GUI thread through Qt (`state.conf`, KRunner's state)
  are pre-checked (5); `launcher.conf`, `krunnerrc` and `kdeglobals` were
  measured as not hanging. A pipe planted in a place the probe does not seed
  (a file KDE's libraries read at start that we did not think of) would be
  KDE's to fix, and would stall Plasma's own processes too.
- **`sync_overrides` reads up to 2 GiB** (8,192 files of 256 KiB in
  `applications`) per catalogue change, on the IO worker. A process that fills
  that folder with large files named like desktop ids can make each change slow;
  it is bounded, off the GUI thread, and visible in the folder.
- **Core dumps** may contain recent typed text (10). Turning them off is a
  product decision (it turns off the crash reports that depend on them) and
  needs the framework crash reporter's agreement.
- **The adaptors' `QDBusContext` is inert** (1): no error replies and an
  early `ClearHistory` reply. Fixing it means moving the context to the
  adaptor's parent object; reliability, not security.
- **Same-name spoofing** in the list: the panel shows names and comments, not
  `Exec` or the file's folder, so two entries called "Firefox" look alike.
- **Squatting and queueing for the bus name** (1): needs the same user; the
  launcher cannot prevent it and a system bus policy does not apply.
- **The typed text goes to out-of-process KRunner plugins**, and KRunner and
  KIO log it at debug level when asked to (9).
- **The unit has no sandboxing** beyond limits (10); see the reason there.
- **The dock button does not check who owns the launcher's name** (1).
- **Fuzzing is property testing** (stable Rust, no coverage guidance);
  `cargo-fuzz` targets for the xbel and desktop-entry paths are a possible next
  step. The C++ is covered by ctest on the validators and by source guards,
  not by a fuzzer.

## 16. Found and fixed in 0.4.0

| Finding | Severity | Fix |
|---|---|---|
| An app whose `Icon=` names a pipe stopped the launcher for good (the GUI thread waits in `open()` when the Start page draws it); the same for a late KRunner icon | medium (a denial of service of the Start menu by a process that can write a desktop file) | Icon paths must be plain files, checked off the GUI thread (3, 6) |
| A pipe in the place of `state.conf` or of KRunner's state file stopped the launcher at start | low (needs write access to the user's config or state folder) | Both are opened only when plain and small (5) |
| The adaptors exported an internal method and two internal signals on the bus; the signals broadcast what `ImportPins` was given | low | Plain methods; the probe pins the surface (1) |
| `Show` and `--search` text reached the field with control and bidi characters although DESIGN.md said they were stripped | low | `Backend::cleanQuery` (1) |
| `render` trusted its caller to have cleaned the name written into a file the whole desktop reads | low (no caller passed an unclean one) | `render` refuses a name `clean_name` would change (4) |
| The override writer followed a link that appeared between its `lstat` and its read or write, and could replace a file that appeared meanwhile | low (a race, same user) | `O_NOFOLLOW` reads, `write_atomic_nofollow`, `RENAME_NOREPLACE` (4) |
| `KService::serviceByStorageId` loads an absolute path; the executor checked only that the id was short | low (not reachable) | `Validate::desktopId` (2) |
| The Flatpak id check in C++ used `$`, which accepts a trailing line break | low (the core had already refused it) | `\A...\z` (2) |
| A mime type starting with `-` or `.` made an icon name starting with it | info | The name must pass `valid_icon_name` |
| The marker of a rename was honoured in any desktop file | info | Only in the user's `applications` folder (3) |
| A command's subtitle (a `PATH` entry) was shown uncleaned | info | `clean_display` |
| The real name from AccountsService was shown uncleaned | info | `Validate::displayText` (8) |
| CI cache saves did not name the `push` event; no advisory, licence or workflow checks existed; no hardening was checked on the result | info | `security.yml`, `deny.toml`, `check-workflows.py`, `check-hardening.sh` (11, 12) |
