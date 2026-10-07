//! The launcher's QObjects: `Backend` (the engine, the panel's state, and
//! the signals C++ carries out) and `ItemModel` (one per list the panel
//! shows). C++ sees them as plain QObjects and calls them by name, like the
//! other Telamon apps. Never blocks the GUI thread: work runs on the engine's
//! workers and comes back with `qt_thread().queue(..)`.

#[cxx_qt::bridge(namespace = "telamon_launcher")]
pub mod qobject {
    // Qt and cxx-qt-lib types live in the global namespace.
    #[namespace = ""]
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
        include!(<QtCore/QAbstractListModel>);
        type QAbstractListModel;
    }

    extern "RustQt" {
        /// One list of the panel (results, pins, A–Z, recent apps, recent
        /// files). Changed by row diffs, so the view keeps its place.
        #[qobject]
        #[base = QAbstractListModel]
        #[qproperty(i32, count)]
        /// The query the rows answer (results only).
        #[qproperty(i32, serial)]
        type ItemModel = super::ItemModelRust;

        #[qobject]
        /// The serial of the latest query typed.
        #[qproperty(i32, serial)]
        /// The A–Z letters that have apps ('#' first).
        #[qproperty(QStringList, letters)]
        type Backend = super::BackendRust;
    }

    unsafe extern "RustQt" {
        /// The id of the row at `row`, or "" past the end.
        #[qinvokable]
        #[cxx_name = "idAt"]
        fn id_at(self: &ItemModel, row: i32) -> QString;

        #[inherit]
        #[cxx_name = "beginInsertRows"]
        fn begin_insert_rows(
            self: Pin<&mut ItemModel>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        #[inherit]
        #[cxx_name = "endInsertRows"]
        fn end_insert_rows(self: Pin<&mut ItemModel>);
        #[inherit]
        #[cxx_name = "beginRemoveRows"]
        fn begin_remove_rows(
            self: Pin<&mut ItemModel>,
            parent: &QModelIndex,
            first: i32,
            last: i32,
        );
        #[inherit]
        #[cxx_name = "endRemoveRows"]
        fn end_remove_rows(self: Pin<&mut ItemModel>);
        #[inherit]
        #[cxx_name = "beginResetModel"]
        fn begin_reset_model(self: Pin<&mut ItemModel>);
        #[inherit]
        #[cxx_name = "endResetModel"]
        fn end_reset_model(self: Pin<&mut ItemModel>);
        #[inherit]
        #[cxx_name = "beginMoveRows"]
        fn begin_move_rows(
            self: Pin<&mut ItemModel>,
            source_parent: &QModelIndex,
            source_first: i32,
            source_last: i32,
            destination_parent: &QModelIndex,
            destination_child: i32,
        ) -> bool;
        #[inherit]
        #[cxx_name = "endMoveRows"]
        fn end_move_rows(self: Pin<&mut ItemModel>);
        #[inherit]
        fn index(self: &ItemModel, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;
        #[inherit]
        #[qsignal]
        #[cxx_name = "dataChanged"]
        fn data_changed(
            self: Pin<&mut ItemModel>,
            top_left: &QModelIndex,
            bottom_right: &QModelIndex,
            roles: &QList_i32,
        );

        #[cxx_override]
        fn data(self: &ItemModel, index: &QModelIndex, role: i32) -> QVariant;
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &ItemModel) -> QHash_i32_QByteArray;
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &ItemModel, parent: &QModelIndex) -> i32;
    }

    unsafe extern "RustQt" {
        // --- From C++ (main.cpp and the providers), by name ---
        // Only QVariant, QString, QStringList and bool cross to C++: it calls
        // and connects these by name, without the generated header, where
        // names like `QList_QVariant` or `::std::int32_t` would not match.

        /// Starts the engine, once, after `setOptions`.
        #[qinvokable]
        fn start(self: Pin<&mut Backend>, can_hibernate: bool, can_switch_user: bool);
        /// logind or the seat answered after `start`.
        #[qinvokable]
        #[cxx_name = "setSession"]
        fn set_session(self: Pin<&mut Backend>, can_hibernate: bool, can_switch_user: bool);
        /// A new app catalogue: maps with `desktopId`, `name`, `genericName`,
        /// `comment`, `icon`, `execName`, `flatpakId` (strings), `keywords`,
        /// `categories`, `actionIds`, `actionNames`, `actionIcons` (string
        /// lists) and `installed` (seconds, qint64). `preferred` maps
        /// `preferred://browser` and the like to desktop ids. Both come
        /// wrapped in a QVariant (a QVariantList and a QVariantMap).
        #[qinvokable]
        #[cxx_name = "setApps"]
        fn set_apps(self: Pin<&mut Backend>, apps: &QVariant, preferred: &QVariant);
        /// `launcher.conf` and `krunnerrc`: bools `apps`, `settings`,
        /// `calculator`, `units`, `commands`, `files`, `web`, `learn`, and
        /// strings `webEngine`, `decimal` (a QVariantMap).
        #[qinvokable]
        #[cxx_name = "setOptions"]
        fn set_options(self: Pin<&mut Backend>, options: &QVariant);
        /// KRunner's current matches for query `serial` (an int, the
        /// `serial` property when the query started), its whole list: a
        /// QVariantList of maps with `runnerId`, `matchId`, `text`,
        /// `subtext`, `icon` and `relevance` (a double).
        #[qinvokable]
        #[cxx_name = "mergeRunner"]
        fn merge_runner(self: Pin<&mut Backend>, serial: &QVariant, matches: &QVariant);
        /// D-Bus `ImportPins` (already capped at 64).
        #[qinvokable]
        #[cxx_name = "importPins"]
        fn import_pins(self: Pin<&mut Backend>, ids: &QStringList);
        /// D-Bus `ClearHistory` or Settings; answered by `historyCleared`.
        #[qinvokable]
        #[cxx_name = "clearHistory"]
        fn clear_history(self: Pin<&mut Backend>);

        // --- From QML ---

        /// The search field's text changed: starts a query.
        #[qinvokable]
        fn search(self: Pin<&mut Backend>, text: &QString);
        /// The panel opened: reload what changed and report the Start page.
        #[qinvokable]
        fn refresh(self: Pin<&mut Backend>);
        /// The highlighted result, which late results keep in place.
        #[qinvokable]
        fn select(self: Pin<&mut Backend>, id: &QString);
        /// Runs the row `id` of list `list` (state.rs `List`). False when
        /// that row is no longer shown.
        #[qinvokable]
        fn activate(self: Pin<&mut Backend>, list: i32, id: &QString) -> bool;
        #[qinvokable]
        #[cxx_name = "canPin"]
        fn can_pin(self: &Backend, id: &QString) -> bool;
        #[qinvokable]
        #[cxx_name = "isPinned"]
        fn is_pinned(self: &Backend, id: &QString) -> bool;
        #[qinvokable]
        fn pin(self: Pin<&mut Backend>, id: &QString);
        #[qinvokable]
        fn unpin(self: Pin<&mut Backend>, id: &QString);
        /// Moves the pin row `id` to the place of shown pin row `index`.
        #[qinvokable]
        #[cxx_name = "movePin"]
        fn move_pin(self: Pin<&mut Backend>, id: &QString, index: i32);
        /// The A–Z row where `letter` starts, or -1.
        #[qinvokable]
        #[cxx_name = "letterIndex"]
        fn letter_index(self: &Backend, letter: &QString) -> i32;
        /// The desktop id behind an app row ("" for anything else), for its
        /// context menu.
        #[qinvokable]
        #[cxx_name = "desktopIdOf"]
        fn desktop_id_of(self: &Backend, id: &QString) -> QString;
        /// The Flatpak id behind an app row, or "" (Uninstall is offered
        /// only for Flatpaks, through Telamon Store).
        #[qinvokable]
        #[cxx_name = "flatpakIdOf"]
        fn flatpak_id_of(self: &Backend, id: &QString) -> QString;
        /// The `file:` URI behind a shown file row, or "" (Open Containing
        /// Folder, Copy Path, Remove from Recent).
        #[qinvokable]
        #[cxx_name = "uriOf"]
        fn uri_of(self: &Backend, id: &QString) -> QString;
        /// The Start page group of an app row (a key of
        /// `catalog::GROUPS`: "internet", "office"...), or "" for anything else.
        #[qinvokable]
        #[cxx_name = "categoryOf"]
        fn category_of(self: &Backend, id: &QString) -> QString;
        /// An app row's desktop actions: a list of maps with `id`, `name`
        /// and `icon`, empty for anything else.
        #[qinvokable]
        #[cxx_name = "actionsOf"]
        fn actions_of(self: &Backend, id: &QString) -> QVariant;

        // --- To C++, which carries them out ---

        #[qsignal]
        #[cxx_name = "launchApp"]
        fn launch_app(self: Pin<&mut Backend>, desktop_id: QString, action: QString);
        #[qsignal]
        #[cxx_name = "openSettings"]
        fn open_settings(self: Pin<&mut Backend>, link: QString);
        #[qsignal]
        #[cxx_name = "openFile"]
        fn open_file(self: Pin<&mut Backend>, uri: QString);
        #[qsignal]
        #[cxx_name = "openWeb"]
        fn open_web(self: Pin<&mut Backend>, url: QString);
        #[qsignal]
        #[cxx_name = "copyText"]
        fn copy_text(self: Pin<&mut Backend>, text: QString);
        #[qsignal]
        #[cxx_name = "runCommand"]
        fn run_command(self: Pin<&mut Backend>, executable: QString, args: QStringList);
        #[qsignal]
        #[cxx_name = "runInTerminal"]
        fn run_in_terminal(self: Pin<&mut Backend>, line: QString);
        #[qsignal]
        #[cxx_name = "sessionAction"]
        fn session_action(self: Pin<&mut Backend>, name: QString);
        #[qsignal]
        #[cxx_name = "runRunner"]
        fn run_runner(self: Pin<&mut Backend>, runner_id: QString, match_id: QString);
        /// A row ran; the panel hides.
        #[qsignal]
        fn activated(self: Pin<&mut Backend>);
        /// A query started: KRunner (and later Explorer) search it too. Its
        /// serial is the `serial` property, already set.
        #[qsignal]
        #[cxx_name = "searchStarted"]
        fn search_started(self: Pin<&mut Backend>, text: QString);
        #[qsignal]
        #[cxx_name = "historyCleared"]
        fn history_cleared(self: Pin<&mut Backend>, ok: bool);
        /// Something went wrong, by kind only (`ProblemKind`'s name).
        #[qsignal]
        fn problem(self: Pin<&mut Backend>, kind: QString);
    }

    impl cxx_qt::Threading for ItemModel {}
    impl cxx_qt::Threading for Backend {}

    #[namespace = "rust::cxxqtlib1"]
    unsafe extern "C++" {
        include!("cxx-qt-lib/common.h");

        #[cxx_name = "make_unique"]
        fn backend_make_unique() -> UniquePtr<Backend>;
        #[cxx_name = "make_unique"]
        fn item_model_make_unique() -> UniquePtr<ItemModel>;
    }
}

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::{
    QByteArray, QHash, QHashPair_i32_QByteArray, QList, QMap, QMapPair_QString_QVariant,
    QModelIndex, QString, QStringList, QVariant,
};
use telamon_launcher_core::catalog::{AppActionEntry, AppEntry, MAX_APPS, valid_desktop_id};
use telamon_launcher_core::commands::SessionAvailability;
use telamon_launcher_core::engine::{Engine, EngineConfig, Update};
use telamon_launcher_core::late::{RunnerMatch, runner_items};
use telamon_launcher_core::legacy;
use telamon_launcher_core::pins::valid_pin;
use telamon_launcher_core::query::{MAX_LATE_BATCH, ModelOp, SearchOptions, Source, diff};
use telamon_launcher_core::result::ResultItem;
use telamon_launcher_core::settings_index::{self, locale_chain};
use telamon_launcher_core::text::MAX_QUERY_CHARS;
use telamon_launcher_core::web;

use crate::state::{Exec, List, State, exec_for, kind_name, section_of};

// ---------------------------------------------------------------- ItemModel

/// Qt::UserRole: roles below it are Qt's own.
const FIRST_ROLE: i32 = 0x0100;
const ROLES: [&str; 6] = ["id", "kind", "title", "subtitle", "icon", "section"];

#[derive(Default)]
pub struct ItemModelRust {
    count: i32,
    serial: i32,
    rows: Vec<ResultItem>,
}

fn int(v: usize) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// The low 31 bits of a serial, for QML (which only compares them).
///
/// It wraps after 2^31 queries, far past any session; only equality is
/// asked of it.
fn qml_serial(serial: u64) -> i32 {
    (serial & 0x7fff_ffff) as i32
}

impl qobject::ItemModel {
    pub fn id_at(&self, row: i32) -> QString {
        usize::try_from(row)
            .ok()
            .and_then(|r| self.rows.get(r))
            .map_or_else(QString::default, |i| QString::from(i.id.as_str()))
    }

    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(item) = usize::try_from(index.row())
            .ok()
            .and_then(|r| self.rows.get(r))
        else {
            return QVariant::default();
        };
        let text = match role - FIRST_ROLE {
            0 => item.id.clone(),
            1 => kind_name(item.kind).to_owned(),
            2 => item.title.clone(),
            3 => item.subtitle.clone(),
            4 => item.icon.clone(),
            5 => section_of(item),
            _ => return QVariant::default(),
        };
        QVariant::from(&QString::from(text.as_str()))
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        for (i, name) in ROLES.iter().enumerate() {
            roles.insert(FIRST_ROLE + int(i), QByteArray::from(*name));
        }
        roles
    }

    pub fn row_count(&self, parent: &QModelIndex) -> i32 {
        if parent.is_valid() {
            0
        } else {
            int(self.rows.len())
        }
    }

    /// Brings the rows to `items` by diff (removals, moves up, inserts and
    /// updates), so the view keeps its selection and the screen reader its
    /// place.
    fn apply(mut self: Pin<&mut Self>, serial: u64, mut items: Vec<ResultItem>) {
        // `diff` needs unique ids; the engine gives them, but a duplicate
        // must cost a row, not the model.
        let mut seen = HashSet::new();
        items.retain(|i| seen.insert(i.id.clone()));
        // A new query's answer replaces the list (the view goes back to the
        // top hit anyway): a diff of many moves left stale rows drawn in the
        // ListView. Late merges into the same answer, pins and apps diff.
        if qml_serial(serial) != *self.serial() {
            self.as_mut().begin_reset_model();
            self.as_mut().rust_mut().rows = items;
            self.as_mut().end_reset_model();
            let count = int(self.rows.len());
            self.as_mut().set_count(count);
            self.as_mut().set_serial(qml_serial(serial));
            return;
        }
        let root = QModelIndex::default();
        for op in diff(&self.rows, &items) {
            match op {
                ModelOp::Remove { index } => {
                    let i = int(index);
                    self.as_mut().begin_remove_rows(&root, i, i);
                    self.as_mut().rust_mut().rows.remove(index);
                    self.as_mut().end_remove_rows();
                }
                ModelOp::Insert { index, item } => {
                    let i = int(index);
                    self.as_mut().begin_insert_rows(&root, i, i);
                    self.as_mut().rust_mut().rows.insert(index, item);
                    self.as_mut().end_insert_rows();
                }
                ModelOp::Move { from, to } => {
                    // Moving up: the destination is the row it goes before.
                    let (f, t) = (int(from), int(to));
                    if self.as_mut().begin_move_rows(&root, f, f, &root, t) {
                        {
                            let mut rust = self.as_mut().rust_mut();
                            let row = rust.rows.remove(from);
                            rust.rows.insert(to, row);
                        }
                        self.as_mut().end_move_rows();
                    }
                }
                ModelOp::Update { index, item } => {
                    self.as_mut().rust_mut().rows[index] = item;
                    let at = self.index(int(index), 0, &root);
                    self.as_mut().data_changed(&at, &at, &QList::default());
                }
            }
        }
        // A move Qt refused (it never should) would leave the order off;
        // the rows must end up exactly as asked.
        if self.rows != items {
            log::warn!("list diff did not converge; replacing {} rows", items.len());
            let old = int(self.rows.len());
            if old > 0 {
                self.as_mut().begin_remove_rows(&root, 0, old - 1);
                self.as_mut().rust_mut().rows.clear();
                self.as_mut().end_remove_rows();
            }
            if !items.is_empty() {
                self.as_mut()
                    .begin_insert_rows(&root, 0, int(items.len()) - 1);
                self.as_mut().rust_mut().rows = items;
                self.as_mut().end_insert_rows();
            }
        }
        let count = int(self.rows.len());
        self.as_mut().set_count(count);
        self.as_mut().set_serial(qml_serial(serial));
    }
}

// ------------------------------------------------------------------ Backend

#[derive(Default)]
pub struct BackendRust {
    serial: i32,
    letters: QStringList,
    state: State,
    engine: Option<Engine>,
    models: [Option<Box<CxxQtThread<qobject::ItemModel>>>; 5],
    /// The latest query's serial, read on the workers' side too.
    current: Arc<AtomicU64>,
    /// The text of the latest query; the results list keeps its own.
    query: String,
    options: SearchOptions,
}

// Runs at exit only: the backend lives as long as the process.
impl Drop for BackendRust {
    fn drop(&mut self) {
        if let Some(mut e) = self.engine.take() {
            e.shutdown();
        }
    }
}

type VariantMap = QMap<QMapPair_QString_QVariant>;

/// `preferred://` names C++ resolves: a handful in practice.
const MAX_PREFERRED: usize = 64;
/// UTF-16 units of typed text kept: four times the engine's cap, so a
/// command row still sees the line is too long.
const MAX_SEARCH_UNITS: isize = 4 * MAX_QUERY_CHARS as isize;

fn qstrings(list: &QStringList) -> Vec<String> {
    QList::<QString>::from(list)
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn to_qstrings(list: &[String]) -> QStringList {
    list.iter().map(|s| QString::from(s.as_str())).collect()
}

fn field(m: &VariantMap, key: &str) -> Option<QVariant> {
    m.get(&QString::from(key))
}

fn text(m: &VariantMap, key: &str) -> String {
    field(m, key)
        .and_then(|v| v.value::<QString>())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn texts(m: &VariantMap, key: &str) -> Vec<String> {
    field(m, key)
        .and_then(|v| v.value::<QStringList>())
        .map(|l| qstrings(&l))
        .unwrap_or_default()
}

fn flag(m: &VariantMap, key: &str, default: bool) -> bool {
    field(m, key)
        .and_then(|v| v.value::<bool>())
        .unwrap_or(default)
}

fn maps(list: &QList<QVariant>) -> impl Iterator<Item = VariantMap> + '_ {
    list.iter().filter_map(|v| v.value::<VariantMap>())
}

fn app_entry(m: &VariantMap) -> AppEntry {
    let icons = texts(m, "actionIcons");
    let actions = texts(m, "actionIds")
        .into_iter()
        .zip(texts(m, "actionNames"))
        .enumerate()
        .map(|(i, (id, name))| AppActionEntry {
            id,
            name,
            icon: icons.get(i).cloned().unwrap_or_default(),
        })
        .collect();
    let flatpak = text(m, "flatpakId");
    AppEntry {
        desktop_id: text(m, "desktopId"),
        name: text(m, "name"),
        generic_name: text(m, "genericName"),
        comment: text(m, "comment"),
        keywords: texts(m, "keywords"),
        icon: text(m, "icon"),
        exec_name: text(m, "execName"),
        categories: texts(m, "categories"),
        actions,
        flatpak_id: (!flatpak.is_empty()).then_some(flatpak),
        installed_unix: field(m, "installed")
            .and_then(|v| v.value::<i64>())
            .unwrap_or(0),
    }
}

/// An XDG base directory: the variable when it is an absolute path (the
/// spec ignores relative ones), else `$HOME/<fallback>`.
fn xdg_dir(var: &str, home: &Path, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
}

fn engine_config(can_hibernate: bool, can_switch_user: bool) -> EngineConfig {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"));
    let env = |k: &str| std::env::var(k).ok();
    EngineConfig {
        state_dir: xdg_dir("XDG_STATE_HOME", &home, ".local/state").join("telamon-launcher"),
        config_dir: xdg_dir("XDG_CONFIG_HOME", &home, ".config").join("telamon-launcher"),
        // The package's default pins, else the image's own file of the old
        // name (until the image has moved).
        system_pins: legacy::first_existing(&[
            "/etc/xdg/telamon-launcher/pinned.list",
            "/etc/xdg/atlas-launcher/pinned.list",
        ]),
        xbel: xdg_dir("XDG_DATA_HOME", &home, ".local/share").join("recently-used.xbel"),
        settings_index: legacy::first_existing(&[
            settings_index::DEFAULT_PATH,
            settings_index::LEGACY_PATH,
        ]),
        path_var: env("PATH").unwrap_or_default(),
        locales: locale_chain(
            env("LANGUAGE").as_deref(),
            env("LC_ALL").as_deref(),
            env("LC_MESSAGES").as_deref(),
            env("LANG").as_deref(),
        ),
        session: SessionAvailability {
            hibernate: can_hibernate,
            switch_user: can_switch_user,
        },
        home,
    }
}

impl qobject::Backend {
    /// Gives the backend the model of `list`; done once, in `lib.rs`.
    pub fn attach_model(mut self: Pin<&mut Self>, list: List, model: &qobject::ItemModel) {
        self.as_mut().rust_mut().models[list.index()] = Some(Box::new(model.qt_thread()));
    }

    pub fn start(mut self: Pin<&mut Self>, can_hibernate: bool, can_switch_user: bool) {
        if self.engine.is_some() {
            return;
        }
        let qt = self.qt_thread();
        let current = self.current.clone();
        let on_update = move |u: Update| {
            // A stale answer is dropped before it costs the GUI thread.
            if let Update::Results { serial, .. } = &u
                && current.load(Ordering::Acquire) != *serial
            {
                return;
            }
            // Refused once the backend is gone (at exit): nothing to do then.
            let _ = qt.queue(move |b| b.on_update(u));
        };
        let engine = Engine::start(
            engine_config(can_hibernate, can_switch_user),
            self.options.clone(),
            Box::new(on_update),
        );
        // A catalogue given before the start.
        if !self.state.apps().is_empty() {
            engine.set_apps(self.state.apps().to_vec());
        }
        self.as_mut().rust_mut().engine = Some(engine);
    }

    fn on_update(mut self: Pin<&mut Self>, update: Update) {
        match update {
            Update::Results { serial, items } => {
                // State and model change together, from here only, so the
                // row a user presses is the row `activate` finds. The
                // model's update is posted from this handler, so Qt runs it
                // in the same pass over posted events, before any input.
                if serial == self.current.load(Ordering::Acquire) {
                    let query = self.query.clone();
                    self.as_mut()
                        .rust_mut()
                        .state
                        .set_results(serial, query, items);
                    self.push(List::Results);
                }
            }
            Update::Pins(ids) => {
                self.as_mut().rust_mut().state.set_pins(ids);
                self.push(List::Pins);
            }
            Update::Recent { apps, files } => {
                self.as_mut().rust_mut().state.set_recent(apps, files);
                self.push(List::RecentApps);
                self.push(List::RecentFiles);
            }
            Update::HistoryCleared(r) => {
                if let Err(kind) = &r {
                    log::warn!("history not cleared: {kind}");
                }
                self.as_mut().history_cleared(r.is_ok());
            }
            Update::Problem(kind) => {
                log::warn!("engine problem: {kind:?}");
                self.as_mut()
                    .problem(QString::from(format!("{kind:?}").as_str()));
            }
        }
    }

    /// Sends a list's current rows to its model.
    fn push(&self, list: List) {
        let Some(model) = &self.models[list.index()] else {
            return;
        };
        let items = self.state.list(list).to_vec();
        let serial = if list == List::Results {
            self.state.results_serial()
        } else {
            0
        };
        let _ = model.queue(move |m| m.apply(serial, items));
    }

    pub fn set_apps(mut self: Pin<&mut Self>, apps: &QVariant, preferred: &QVariant) {
        let apps = apps.value::<QList<QVariant>>().unwrap_or_default();
        let preferred = preferred.value::<VariantMap>().unwrap_or_default();
        let entries: Vec<AppEntry> = maps(&apps).take(MAX_APPS).map(|m| app_entry(&m)).collect();
        let preferred: HashMap<String, String> = preferred
            .iter()
            .take(MAX_PREFERRED)
            .map(|(k, v)| {
                let id = v.value::<QString>().map(|s| s.to_string());
                (k.to_string(), id.unwrap_or_default())
            })
            .filter(|(k, v)| valid_pin(k) && valid_desktop_id(v))
            .collect();
        let changed = self
            .as_mut()
            .rust_mut()
            .state
            .set_apps(entries.clone(), preferred);
        for list in changed {
            self.push(list);
        }
        let letters = to_qstrings(&self.state.letters());
        self.as_mut().set_letters(letters);
        if let Some(e) = &self.engine {
            e.set_apps(entries);
        }
    }

    pub fn set_options(mut self: Pin<&mut Self>, options: &QVariant) {
        let options = options.value::<VariantMap>().unwrap_or_default();
        let o = &options;
        let decimal = text(o, "decimal")
            .chars()
            .next()
            .filter(|c| matches!(c, '.' | ','))
            .unwrap_or('.');
        let opts = SearchOptions {
            apps: flag(o, "apps", true),
            settings: flag(o, "settings", true),
            calculator: flag(o, "calculator", true),
            units: flag(o, "units", true),
            commands: flag(o, "commands", true),
            files: flag(o, "files", true),
            web: flag(o, "web", true).then(|| web::Engine::from_config(&text(o, "webEngine"))),
            learn: flag(o, "learn", true),
            decimal,
        };
        if let Some(e) = &self.engine {
            e.set_options(opts.clone());
        }
        self.as_mut().rust_mut().options = opts;
    }

    pub fn merge_runner(self: Pin<&mut Self>, serial: &QVariant, matches: &QVariant) {
        let current = self.current.load(Ordering::Acquire);
        if serial.value::<i32>() != Some(qml_serial(current)) {
            return;
        }
        let matches = matches.value::<QList<QVariant>>().unwrap_or_default();
        let matches: Vec<RunnerMatch> = maps(&matches)
            .take(MAX_LATE_BATCH)
            .map(|m| RunnerMatch {
                runner_id: text(&m, "runnerId"),
                match_id: text(&m, "matchId"),
                text: text(&m, "text"),
                subtext: text(&m, "subtext"),
                icon: text(&m, "icon"),
                relevance: field(&m, "relevance")
                    .and_then(|v| v.value::<f64>())
                    .unwrap_or(0.0),
            })
            .collect();
        if let Some(e) = &self.engine {
            e.merge(current, Source::Runner, runner_items(matches));
        }
    }

    pub fn set_session(self: Pin<&mut Self>, can_hibernate: bool, can_switch_user: bool) {
        if let Some(e) = &self.engine {
            e.set_session(SessionAvailability {
                hibernate: can_hibernate,
                switch_user: can_switch_user,
            });
        }
    }

    pub fn import_pins(self: Pin<&mut Self>, ids: &QStringList) {
        if let Some(e) = &self.engine {
            e.import_pins(qstrings(ids));
        }
    }

    pub fn clear_history(self: Pin<&mut Self>) {
        if let Some(e) = &self.engine {
            e.clear_history();
        }
    }

    pub fn search(mut self: Pin<&mut Self>, text: &QString) {
        // A huge paste is cut before it is copied around; past the engine's
        // own cap it only tells a command row the line is too long.
        let text = if text.len() > MAX_SEARCH_UNITS {
            text.left(MAX_SEARCH_UNITS).to_string()
        } else {
            text.to_string()
        };
        let serial = self.current.load(Ordering::Acquire) + 1;
        self.current.store(serial, Ordering::Release);
        self.as_mut().rust_mut().query = text.clone();
        self.as_mut().set_serial(qml_serial(serial));
        if let Some(e) = &self.engine {
            e.query(serial, text.clone());
        }
        self.as_mut().search_started(QString::from(text.as_str()));
    }

    pub fn refresh(self: Pin<&mut Self>) {
        if let Some(e) = &self.engine {
            e.refresh();
        }
    }

    pub fn select(self: Pin<&mut Self>, id: &QString) {
        if let Some(e) = &self.engine {
            let id = id.to_string();
            e.select((!id.is_empty()).then_some(id));
        }
    }

    pub fn activate(mut self: Pin<&mut Self>, list: i32, id: &QString) -> bool {
        let Some(list) = List::from_index(list) else {
            return false;
        };
        let id = id.to_string();
        // A results row runs only while the list answers what is typed: an
        // Enter that beats the new answer runs nothing.
        if list == List::Results
            && self.state.results_serial() != self.current.load(Ordering::Acquire)
        {
            return false;
        }
        let Some(item) = self.state.find(list, &id) else {
            return false;
        };
        let exec = exec_for(&item.action);
        if let Some(e) = &self.engine {
            // Only results carry the query that found them; the Start page's
            // rows count as use with no query.
            let query = if list == List::Results {
                self.state.results_query().to_owned()
            } else {
                String::new()
            };
            e.record(query, id);
        }
        let q = |s: &str| QString::from(s);
        match exec {
            Exec::LaunchApp { desktop_id, action } => {
                self.as_mut().launch_app(q(&desktop_id), q(&action));
            }
            Exec::OpenSettings { link } => self.as_mut().open_settings(q(&link)),
            Exec::OpenFile { uri } => self.as_mut().open_file(q(&uri)),
            Exec::OpenWeb { url } => self.as_mut().open_web(q(&url)),
            Exec::Copy { text } => self.as_mut().copy_text(q(&text)),
            Exec::Run { executable, args } => {
                self.as_mut()
                    .run_command(q(&executable), to_qstrings(&args));
            }
            Exec::RunInTerminal { line } => self.as_mut().run_in_terminal(q(&line)),
            Exec::Session { name } => self.as_mut().session_action(q(name)),
            Exec::Runner {
                runner_id,
                match_id,
            } => self.as_mut().run_runner(q(&runner_id), q(&match_id)),
        }
        self.as_mut().activated();
        true
    }

    pub fn can_pin(&self, id: &QString) -> bool {
        let id = id.to_string();
        self.state.pinnable(&id).is_some() && !self.state.is_pinned(&id)
    }

    pub fn is_pinned(&self, id: &QString) -> bool {
        self.state.is_pinned(&id.to_string())
    }

    pub fn pin(self: Pin<&mut Self>, id: &QString) {
        if let (Some(desktop_id), Some(e)) = (self.state.pinnable(&id.to_string()), &self.engine) {
            e.pin(desktop_id);
        }
    }

    pub fn unpin(self: Pin<&mut Self>, id: &QString) {
        if let Some(e) = &self.engine {
            for key in self.state.pin_keys(&id.to_string()) {
                e.unpin(key);
            }
        }
    }

    pub fn move_pin(self: Pin<&mut Self>, id: &QString, index: i32) {
        let Ok(shown) = usize::try_from(index) else {
            return;
        };
        if let (Some(key), Some(e)) = (self.state.pin_key(&id.to_string()), &self.engine) {
            e.move_pin(key, self.state.stored_index(shown));
        }
    }

    pub fn letter_index(&self, letter: &QString) -> i32 {
        self.state.letter_index(&letter.to_string())
    }

    pub fn desktop_id_of(&self, id: &QString) -> QString {
        self.state
            .app_of(&id.to_string())
            .map_or_else(QString::default, |a| QString::from(a.desktop_id.as_str()))
    }

    pub fn category_of(&self, id: &QString) -> QString {
        self.state
            .app_of(&id.to_string())
            .map_or_else(QString::default, |a| {
                QString::from(telamon_launcher_core::catalog::group_of(&a.categories))
            })
    }

    pub fn flatpak_id_of(&self, id: &QString) -> QString {
        self.state
            .app_of(&id.to_string())
            .and_then(|a| a.flatpak_id.as_deref())
            .map_or_else(QString::default, QString::from)
    }

    pub fn uri_of(&self, id: &QString) -> QString {
        self.state
            .file_uri_of(&id.to_string())
            .map_or_else(QString::default, QString::from)
    }

    pub fn actions_of(&self, id: &QString) -> QVariant {
        let mut list = QList::<QVariant>::default();
        if let Some(app) = self.state.app_of(&id.to_string()) {
            for a in &app.actions {
                let mut m = VariantMap::default();
                m.insert(
                    QString::from("id"),
                    QVariant::from(&QString::from(a.id.as_str())),
                );
                m.insert(
                    QString::from("name"),
                    QVariant::from(&QString::from(a.name.as_str())),
                );
                m.insert(
                    QString::from("icon"),
                    QVariant::from(&QString::from(a.icon.as_str())),
                );
                list.append(QVariant::from(&m));
            }
        }
        QVariant::from(&list)
    }
}

/// The QObjects C++ hands to QML: the backend and one model per `List`, in
/// `List::ALL` order. The caller owns them; delete the backend first (it
/// stops the engine), then the models.
pub fn make_objects() -> (*mut qobject::Backend, [*mut qobject::ItemModel; 5]) {
    let mut backend = qobject::backend_make_unique();
    let models = List::ALL.map(|list| {
        let model = qobject::item_model_make_unique();
        if let (Some(b), Some(m)) = (backend.as_mut(), model.as_ref()) {
            b.attach_model(list, m);
        }
        model.into_raw()
    });
    (backend.into_raw(), models)
}
