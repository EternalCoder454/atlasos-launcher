//! The search engine: two worker threads and the messages between them and
//! the GUI, with no Qt. See docs/DESIGN.md, "Search" and "Threads".
//!
//! - **launcher-search** owns everything a query reads (the catalogue, the
//!   Settings index, the recent files, the `PATH` cache, the usage store, the
//!   options) and the rows of the current query. It computes the instant
//!   phase, merges late batches, and serialises the usage store for saving.
//! - **launcher-io** owns the files: it loads them at start and when the
//!   panel opens (only what changed), and does every write (`usage.tsv`,
//!   `pinned.list`) atomically. It also owns the pin list.
//!
//! Every [`Engine`] method only sends a message, so the GUI thread never
//! blocks. Both threads block on `recv` (no timers, no polling); when
//! several queries are waiting, only the newest is computed. Results leave
//! through the `on_update` callback, which runs on the worker thread that
//! produced them: the C++ side must queue them to the GUI thread.
//!
//! Nothing here logs what the user typed, ids, file names or paths: only
//! counts, kinds and timings. A panic while handling one message is caught,
//! counted and reported as [`ProblemKind::Panicked`]; the thread keeps going.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::catalog::{AppEntry, Catalog};
use crate::commands::{PathCache, SessionAvailability, SessionCommands};
use crate::fsutil::{read_capped, sweep_stale_temps, write_atomic};
use crate::pins::{self, Pins};
use crate::query::{self, Context, ResultSet, SearchOptions, Source, Sources};
use crate::recent::{RecentError, RecentFiles};
use crate::result::{Action, Kind, ResultItem, prior};
use crate::settings_index::{IndexError, SettingsIndex};
use crate::text::Query;
use crate::usage::{self, UsageStore, learnable};

/// Rows of each group on the Start page.
const RECENT_ROWS: usize = 8;
/// Temp files left by a crash are removed after this long.
const STALE_TEMP_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Most late rows taken from one batch.
const MAX_LATE_ITEMS: usize = 256;
/// Most history operations kept while `usage.tsv` is still loading.
const MAX_PENDING_OPS: usize = 64;
/// Longest wait at shutdown for the history to finish loading.
const LOAD_WAIT: Duration = Duration::from_secs(5);
/// Most entries taken from an import list.
const MAX_IMPORT_LIST: usize = 1024;

const USAGE_FILE: &str = "usage.tsv";
const PINS_FILE: &str = "pinned.list";

/// Where things are. Every path is the caller's; nothing is read from the
/// environment here.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub home: PathBuf,
    /// `$XDG_STATE_HOME/atlas-launcher`: `usage.tsv`.
    pub state_dir: PathBuf,
    /// `$XDG_CONFIG_HOME/atlas-launcher`: `pinned.list`.
    pub config_dir: PathBuf,
    /// `/etc/xdg/atlas-launcher/pinned.list`, used until the user has a list.
    pub system_pins: PathBuf,
    /// `recently-used.xbel`.
    pub xbel: PathBuf,
    /// AtlasOS Settings' `search-index.json`.
    pub settings_index: PathBuf,
    pub path_var: String,
    /// The locale chain from [`crate::settings_index::locale_chain`].
    pub locales: Vec<String>,
    pub session: SessionAvailability,
}

/// What went wrong, by kind only (never a path, id or text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// A worker caught a panic in one message; it keeps running.
    Panicked,
    /// A worker thread could not be started; the engine does nothing.
    ThreadStart,
    /// `usage.tsv` is too large or not readable; history starts empty.
    UsageUnreadable,
    /// `usage.tsv` has content but no line could be read; history starts empty.
    UsageCorrupt,
    /// `usage.tsv` could not be written; the history stays in memory.
    UsageWriteFailed,
    /// `pinned.list` is too large or not readable; the list starts empty.
    PinsUnreadable,
    /// `pinned.list` could not be written; the pins stay in memory.
    PinsWriteFailed,
    /// An import of Andromeda's favourites was ignored (the user already has
    /// a list, or nothing in it was valid).
    ImportSkipped,
    /// `recently-used.xbel` is too large, malformed or not readable.
    RecentUnreadable,
    /// The Settings index exists but is invalid or too large.
    SettingsIndexUnreadable,
}

/// What the engine reports. Sent from a worker thread.
#[derive(Clone, Debug)]
pub enum Update {
    /// The full list for query `serial` (instant rows, then each merge).
    Results {
        serial: u64,
        items: Vec<ResultItem>,
    },
    /// The pinned ids in order, as stored.
    Pins(Vec<String>),
    /// The Start page's recent apps and files.
    Recent {
        apps: Vec<ResultItem>,
        files: Vec<ResultItem>,
    },
    /// The answer to `clear_history`; the error is only the kind of failure.
    HistoryCleared(Result<(), String>),
    Problem(ProblemKind),
}

/// Counters since start, for logs and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Queries whose instant phase was computed.
    pub queries_computed: u64,
    /// Queries skipped because a newer one was already waiting.
    pub queries_coalesced: u64,
    /// Merges ignored because their serial was not the current one.
    pub merges_stale: u64,
    /// Problems reported (every `Update::Problem`).
    pub problems: u64,
}

#[derive(Default)]
struct Counters {
    computed: AtomicU64,
    coalesced: AtomicU64,
    stale: AtomicU64,
    problems: AtomicU64,
}

/// Sends updates through the callback, shielding the worker from a panic in
/// it.
#[derive(Clone)]
struct Emitter {
    cb: Arc<dyn Fn(Update) + Send + Sync>,
    counters: Arc<Counters>,
}

impl Emitter {
    fn send(&self, u: Update) {
        if catch_unwind(AssertUnwindSafe(|| (self.cb)(u))).is_err() {
            log::error!("the update callback panicked");
        }
    }

    fn problem(&self, kind: ProblemKind) {
        self.counters.problems.fetch_add(1, Ordering::Relaxed);
        log::warn!("problem: {kind:?}");
        self.send(Update::Problem(kind));
    }
}

enum SearchMsg {
    SetApps(Vec<AppEntry>),
    SetOptions(SearchOptions),
    Query {
        serial: u64,
        text: String,
    },
    Merge {
        serial: u64,
        source: Source,
        items: Vec<ResultItem>,
    },
    Select(Option<String>),
    Record {
        query: String,
        id: String,
    },
    ClearHistory,
    // From the IO worker.
    UsageLoaded(UsageStore),
    RecentLoaded(Arc<RecentFiles>),
    SettingsLoaded(Option<Arc<SettingsIndex>>),
    PathLoaded(Arc<PathCache>),
    /// The panel opened: report the Start page again.
    Opened,
    #[cfg(test)]
    TestPanic,
    Shutdown,
}

enum IoMsg {
    Refresh,
    Pin(String),
    Unpin(String),
    MovePin(String, usize),
    ImportPins(Vec<String>),
    WriteUsage(Vec<u8>),
    ClearUsage,
    Shutdown,
}

/// The engine's handle. Methods never block: each sends one message.
pub struct Engine {
    search_tx: Sender<SearchMsg>,
    io_tx: Sender<IoMsg>,
    search: Option<JoinHandle<()>>,
    io: Option<JoinHandle<()>>,
    counters: Arc<Counters>,
}

impl Engine {
    /// Starts the two worker threads. `on_update` is called from them.
    pub fn start(
        cfg: EngineConfig,
        opts: SearchOptions,
        on_update: Box<dyn Fn(Update) + Send + Sync>,
    ) -> Engine {
        let counters = Arc::new(Counters::default());
        let out = Emitter {
            cb: Arc::from(on_update),
            counters: Arc::clone(&counters),
        };
        let (search_tx, search_rx) = mpsc::channel();
        let (io_tx, io_rx) = mpsc::channel();

        let search_worker = Search::new(&cfg, opts, out.clone(), io_tx.clone());
        let search = thread::Builder::new()
            .name("launcher-search".into())
            .spawn(move || search_worker.run(search_rx))
            .map_err(|e| log::error!("search thread did not start: {:?}", e.kind()))
            .ok();
        let io_worker = Io::new(cfg, out.clone(), search_tx.clone());
        let io = thread::Builder::new()
            .name("launcher-io".into())
            .spawn(move || io_worker.run(io_rx))
            .map_err(|e| log::error!("io thread did not start: {:?}", e.kind()))
            .ok();
        if search.is_none() || io.is_none() {
            out.problem(ProblemKind::ThreadStart);
        }
        Engine {
            search_tx,
            io_tx,
            search,
            io,
            counters,
        }
    }

    fn to_search(&self, m: SearchMsg) {
        if self.search_tx.send(m).is_err() {
            log::debug!("search worker is gone; message dropped");
        }
    }

    fn to_io(&self, m: IoMsg) {
        if self.io_tx.send(m).is_err() {
            log::debug!("io worker is gone; message dropped");
        }
    }

    /// A new app catalogue (replaces the old one).
    pub fn set_apps(&self, apps: Vec<AppEntry>) {
        self.to_search(SearchMsg::SetApps(apps));
    }

    pub fn set_options(&self, opts: SearchOptions) {
        self.to_search(SearchMsg::SetOptions(opts));
    }

    /// Starts query `serial`; the answer is `Update::Results` with the same
    /// serial (unless a newer query arrives first). An empty text gives no
    /// rows.
    pub fn query(&self, serial: u64, text: String) {
        self.to_search(SearchMsg::Query { serial, text });
    }

    /// A late batch for query `serial` (ignored unless it is the current
    /// one). `Source::Instant` is never merged.
    pub fn merge(&self, serial: u64, source: Source, items: Vec<ResultItem>) {
        self.to_search(SearchMsg::Merge {
            serial,
            source,
            items,
        });
    }

    /// The id of the selected row, which late merges keep in place.
    pub fn select(&self, id: Option<String>) {
        self.to_search(SearchMsg::Select(id));
    }

    /// The user ran `id` after typing `query_text`. Learned only when the
    /// option is on and the id is learnable; then `usage.tsv` is saved.
    pub fn record(&self, query_text: String, id: String) {
        self.to_search(SearchMsg::Record {
            query: query_text,
            id,
        });
    }

    /// Forgets the history and deletes `usage.tsv`; answered with
    /// `Update::HistoryCleared`.
    pub fn clear_history(&self) {
        self.to_search(SearchMsg::ClearHistory);
    }

    pub fn pin(&self, id: String) {
        self.to_io(IoMsg::Pin(id));
    }

    pub fn unpin(&self, id: String) {
        self.to_io(IoMsg::Unpin(id));
    }

    pub fn move_pin(&self, id: String, index: usize) {
        self.to_io(IoMsg::MovePin(id, index));
    }

    /// Andromeda's favourites, taken over only while the user has no
    /// `pinned.list`.
    pub fn import_pins(&self, mut list: Vec<String>) {
        list.truncate(MAX_IMPORT_LIST);
        self.to_io(IoMsg::ImportPins(list));
    }

    /// The panel opened: reload what changed on disk, then report the Start
    /// page.
    pub fn refresh(&self) {
        self.to_io(IoMsg::Refresh);
    }

    pub fn stats(&self) -> Stats {
        Stats {
            queries_computed: self.counters.computed.load(Ordering::Relaxed),
            queries_coalesced: self.counters.coalesced.load(Ordering::Relaxed),
            merges_stale: self.counters.stale.load(Ordering::Relaxed),
            problems: self.counters.problems.load(Ordering::Relaxed),
        }
    }

    #[cfg(test)]
    fn inject_panic(&self) {
        self.to_search(SearchMsg::TestPanic);
    }

    /// Stops both threads and waits for them. Writes already requested are
    /// finished first. Safe to call twice; later calls do nothing.
    pub fn shutdown(&mut self) {
        // Search first: it may still queue a write for the IO worker.
        let _ = self.search_tx.send(SearchMsg::Shutdown);
        join(self.search.take());
        let _ = self.io_tx.send(IoMsg::Shutdown);
        join(self.io.take());
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn join(h: Option<JoinHandle<()>>) {
    let Some(h) = h else { return };
    // Never wait for ourselves (an Engine dropped inside the callback).
    if h.thread().id() == thread::current().id() {
        return;
    }
    if h.join().is_err() {
        log::error!("a worker thread ended with a panic");
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The one-line kind of an IO error, for logs (no path).
fn io_kind(e: &std::io::Error) -> String {
    e.kind().to_string()
}

/// A history change that arrived before `usage.tsv` was loaded.
enum Pending {
    Record(Query, String),
    Clear,
}

struct Search {
    out: Emitter,
    io: Sender<IoMsg>,
    opts: SearchOptions,
    sources: Sources,
    usage: UsageStore,
    usage_loaded: bool,
    pending: Vec<Pending>,
    set: Option<ResultSet>,
    /// The query `set` belongs to (for ranking late rows).
    query: Query,
    selected: Option<String>,
    recent_dirty: bool,
}

impl Search {
    fn new(cfg: &EngineConfig, opts: SearchOptions, out: Emitter, io: Sender<IoMsg>) -> Search {
        Search {
            out,
            io,
            opts,
            sources: Sources {
                catalog: Arc::new(Catalog::new(Vec::new())),
                settings: None,
                recent: Arc::new(RecentFiles::default()),
                path: Arc::new(PathCache::default()),
                session: Arc::new(SessionCommands::new(cfg.session)),
                home: cfg.home.clone(),
            },
            usage: UsageStore::default(),
            usage_loaded: false,
            pending: Vec::new(),
            set: None,
            query: Query::default(),
            selected: None,
            recent_dirty: false,
        }
    }

    fn run(mut self, rx: Receiver<SearchMsg>) {
        let mut batch: Vec<SearchMsg> = Vec::new();
        // Blocks; no timers. Ends when every sender is gone.
        let mut stopping = false;
        while !stopping && let Ok(first) = rx.recv() {
            batch.push(first);
            while let Ok(m) = rx.try_recv() {
                batch.push(m);
            }
            let newest = batch
                .iter()
                .rposition(|m| matches!(m, SearchMsg::Query { .. }));
            for (i, m) in batch.drain(..).enumerate() {
                match m {
                    SearchMsg::Shutdown => stopping = true,
                    // After Shutdown only the history's arrival matters.
                    m @ SearchMsg::UsageLoaded(_) if stopping => self.guarded(|s| s.handle(m)),
                    _ if stopping => {}
                    SearchMsg::Query { .. } if Some(i) != newest => {
                        self.out.counters.coalesced.fetch_add(1, Ordering::Relaxed);
                    }
                    m => self.guarded(|s| s.handle(m)),
                }
            }
            if self.recent_dirty {
                self.recent_dirty = false;
                self.guarded(Search::emit_recent);
            }
        }
        self.finish_pending(&rx);
        log::debug!("search worker stopped");
    }

    /// At shutdown: history changes still waiting for `usage.tsv` to load are
    /// applied and handed to the IO worker, which is about to deliver it.
    fn finish_pending(&mut self, rx: &Receiver<SearchMsg>) {
        let end = Instant::now() + LOAD_WAIT;
        while !self.usage_loaded && !self.pending.is_empty() {
            let left = end.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(m @ SearchMsg::UsageLoaded(_)) => self.guarded(|s| s.handle(m)),
                Ok(_) => {}
                Err(_) => {
                    log::warn!("history changes lost: the history never loaded");
                    break;
                }
            }
        }
    }

    /// Runs one unit of work; a panic is reported and the worker goes on.
    fn guarded(&mut self, f: impl FnOnce(&mut Search)) {
        if catch_unwind(AssertUnwindSafe(|| f(self))).is_err() {
            log::error!("search worker caught a panic");
            self.out.problem(ProblemKind::Panicked);
        }
    }

    fn handle(&mut self, m: SearchMsg) {
        match m {
            SearchMsg::SetApps(apps) => {
                let catalog = Catalog::new(apps);
                log::debug!(
                    "catalogue: {} apps, {} skipped",
                    catalog.len(),
                    catalog.skipped()
                );
                self.sources.catalog = Arc::new(catalog);
                self.recent_dirty = true;
            }
            SearchMsg::SetOptions(o) => {
                self.opts = o;
                self.recent_dirty = true;
            }
            SearchMsg::Query { serial, text } => self.query(serial, &text),
            SearchMsg::Merge {
                serial,
                source,
                items,
            } => self.merge(serial, source, items),
            SearchMsg::Select(id) => self.selected = id,
            SearchMsg::Record { query, id } => self.record(&query, &id),
            SearchMsg::ClearHistory => self.clear_history(),
            SearchMsg::UsageLoaded(store) => self.usage_loaded(store),
            SearchMsg::RecentLoaded(r) => {
                self.sources.recent = r;
                self.recent_dirty = true;
            }
            SearchMsg::SettingsLoaded(s) => self.sources.settings = s,
            SearchMsg::PathLoaded(p) => self.sources.path = p,
            SearchMsg::Opened => self.recent_dirty = true,
            #[cfg(test)]
            SearchMsg::TestPanic => panic!("injected"),
            SearchMsg::Shutdown => {}
        }
    }

    fn query(&mut self, serial: u64, text: &str) {
        let started = Instant::now();
        let q = Query::new(text, serial);
        let items = {
            let cx = Context {
                usage: &self.usage,
                opts: &self.opts,
                query: &q,
                now: now_unix(),
            };
            query::instant(&self.sources, &cx)
        };
        let set = ResultSet::new(serial, items);
        let items = set.items();
        self.set = Some(set);
        self.query = q;
        self.selected = None;
        self.out.counters.computed.fetch_add(1, Ordering::Relaxed);
        log::debug!(
            "query {serial}: {} rows in {} us",
            items.len(),
            started.elapsed().as_micros()
        );
        self.out.send(Update::Results { serial, items });
    }

    fn merge(&mut self, serial: u64, source: Source, mut items: Vec<ResultItem>) {
        if source == Source::Instant {
            return;
        }
        let Some(set) = self.set.as_mut().filter(|s| s.serial() == serial) else {
            self.out.counters.stale.fetch_add(1, Ordering::Relaxed);
            return;
        };
        items.truncate(MAX_LATE_ITEMS);
        let cx = Context {
            usage: &self.usage,
            opts: &self.opts,
            query: &self.query,
            now: now_unix(),
        };
        set.merge(source, items, self.selected.as_deref(), &cx);
        let items = set.items();
        self.out.send(Update::Results { serial, items });
    }

    fn record(&mut self, text: &str, id: &str) {
        if !self.opts.learn || !learnable(id) {
            return;
        }
        let q = Query::new(text, 0);
        if !self.usage_loaded {
            if self.pending.len() < MAX_PENDING_OPS {
                self.pending.push(Pending::Record(q, id.to_owned()));
            } else {
                log::warn!("history change dropped while the history was loading");
            }
            return;
        }
        self.usage.record(&q, id, now_unix());
        self.flush_usage();
    }

    /// Hands the IO worker a snapshot to write, if anything changed.
    fn flush_usage(&mut self) {
        if !self.usage.is_dirty() {
            return;
        }
        let bytes = self.usage.to_bytes();
        self.usage.mark_clean();
        if self.io.send(IoMsg::WriteUsage(bytes)).is_err() {
            log::debug!("io worker is gone; usage not saved");
        }
    }

    fn clear_history(&mut self) {
        if self.usage_loaded {
            self.usage.clear();
            self.usage.mark_clean();
        } else if self.pending.len() < MAX_PENDING_OPS {
            self.pending.push(Pending::Clear);
        } else {
            self.pending.clear();
            self.pending.push(Pending::Clear);
        }
        // The IO worker deletes the file and answers.
        if self.io.send(IoMsg::ClearUsage).is_err() {
            self.out
                .send(Update::HistoryCleared(Err("not running".into())));
        }
        self.recent_dirty = true;
    }

    fn usage_loaded(&mut self, store: UsageStore) {
        self.usage = store;
        self.usage_loaded = true;
        let now = now_unix();
        for op in std::mem::take(&mut self.pending) {
            match op {
                Pending::Record(q, id) => self.usage.record(&q, &id, now),
                Pending::Clear => {
                    // The IO worker has deleted (or will delete) the file.
                    self.usage.clear();
                    self.usage.mark_clean();
                }
            }
        }
        self.flush_usage();
        self.recent_dirty = true;
    }

    fn emit_recent(&mut self) {
        let mut apps = Vec::new();
        if self.opts.learn {
            for id in self.usage.recent_ids(usage::MAX_ENTRIES) {
                let Some(desktop_id) = id.strip_prefix("app:") else {
                    continue;
                };
                // Desktop actions ("app:x.desktop#new-window") are not apps.
                if let Some(app) = self.sources.catalog.get(desktop_id) {
                    apps.push(app_item(app, apps.len()));
                    if apps.len() == RECENT_ROWS {
                        break;
                    }
                }
            }
        }
        let files = self.sources.recent.recent(RECENT_ROWS, &self.sources.home);
        self.out.send(Update::Recent { apps, files });
    }
}

fn app_item(app: &AppEntry, rank: usize) -> ResultItem {
    let subtitle = if app.generic_name.is_empty() {
        &app.comment
    } else {
        &app.generic_name
    };
    ResultItem {
        id: format!("app:{}", app.desktop_id),
        kind: Kind::App,
        title: app.name.clone(),
        subtitle: subtitle.clone(),
        icon: app.icon.clone(),
        score: prior(Kind::App) / (1.0 + rank as f32),
        action: Action::LaunchApp {
            desktop_id: app.desktop_id.clone(),
            action: None,
        },
    }
}

/// A file's identity for "did it change": modification time and size
/// (`None` when it is missing).
type Stamp = Option<(SystemTime, u64)>;

fn stamp(p: &Path) -> Stamp {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

struct Io {
    cfg: EngineConfig,
    out: Emitter,
    search: Sender<SearchMsg>,
    pins: Pins,
    /// Whether the user has a `pinned.list` (or one that could not be read,
    /// which must not be replaced by an import).
    user_pins: bool,
    xbel_stamp: Stamp,
    settings_stamp: Stamp,
    path: Arc<PathCache>,
}

impl Io {
    fn new(cfg: EngineConfig, out: Emitter, search: Sender<SearchMsg>) -> Io {
        Io {
            cfg,
            out,
            search,
            pins: Pins::default(),
            user_pins: false,
            xbel_stamp: None,
            settings_stamp: None,
            path: Arc::new(PathCache::default()),
        }
    }

    fn to_search(&self, m: SearchMsg) {
        if self.search.send(m).is_err() {
            log::debug!("search worker is gone; message dropped");
        }
    }

    fn run(mut self, rx: Receiver<IoMsg>) {
        self.guarded(Io::load_all);
        // Blocks; no timers. Ends on Shutdown or when every sender is gone.
        while let Ok(m) = rx.recv() {
            if matches!(m, IoMsg::Shutdown) {
                break;
            }
            self.guarded(|io| io.handle(m));
        }
        log::debug!("io worker stopped");
    }

    fn guarded(&mut self, f: impl FnOnce(&mut Io)) {
        if catch_unwind(AssertUnwindSafe(|| f(self))).is_err() {
            log::error!("io worker caught a panic");
            self.out.problem(ProblemKind::Panicked);
        }
    }

    fn handle(&mut self, m: IoMsg) {
        match m {
            IoMsg::Refresh => self.refresh(),
            IoMsg::Pin(id) => self.change_pins(|p| p.pin(&id)),
            IoMsg::Unpin(id) => self.change_pins(|p| p.unpin(&id)),
            IoMsg::MovePin(id, i) => self.change_pins(|p| p.move_to(&id, i)),
            IoMsg::ImportPins(list) => self.import_pins(&list),
            IoMsg::WriteUsage(bytes) => {
                let path = self.cfg.state_dir.join(USAGE_FILE);
                if let Err(e) = write_atomic(&path, &bytes, 0o600) {
                    log::warn!("usage not saved: {}", io_kind(&e));
                    self.out.problem(ProblemKind::UsageWriteFailed);
                }
            }
            IoMsg::ClearUsage => {
                let path = self.cfg.state_dir.join(USAGE_FILE);
                let r = match std::fs::remove_file(&path) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => {
                        log::warn!("history not deleted: {}", io_kind(&e));
                        Err(io_kind(&e))
                    }
                };
                self.out.send(Update::HistoryCleared(r));
            }
            IoMsg::Shutdown => {}
        }
    }

    /// Everything read at start: history, pins, then the slower sources.
    fn load_all(&mut self) {
        let swept = sweep_stale_temps(&self.cfg.state_dir, USAGE_FILE, STALE_TEMP_AGE)
            + sweep_stale_temps(&self.cfg.config_dir, PINS_FILE, STALE_TEMP_AGE);
        if swept > 0 {
            log::info!("removed {swept} stale temp files");
        }
        let usage = self.load_usage();
        self.to_search(SearchMsg::UsageLoaded(usage));
        self.load_pins();
        self.out.send(Update::Pins(self.pins.ids().to_vec()));
        self.reload_recent();
        self.reload_settings();
        self.rescan_path();
    }

    fn load_usage(&mut self) -> UsageStore {
        let path = self.cfg.state_dir.join(USAGE_FILE);
        match read_capped(&path, usage::MAX_FILE_BYTES) {
            Ok(Some(bytes)) => {
                let store = UsageStore::load(&bytes);
                if store.is_empty() && !bytes.trim_ascii().is_empty() {
                    self.out.problem(ProblemKind::UsageCorrupt);
                }
                log::debug!("usage: {} entries", store.len());
                store
            }
            Ok(None) => UsageStore::default(),
            Err(e) => {
                log::warn!("usage not read: {}", io_kind(&e));
                self.out.problem(ProblemKind::UsageUnreadable);
                UsageStore::default()
            }
        }
    }

    fn load_pins(&mut self) {
        let user = self.cfg.config_dir.join(PINS_FILE);
        let read = match read_capped(&user, pins::MAX_FILE_BYTES) {
            Ok(Some(b)) => {
                self.user_pins = true;
                Ok(Some(b))
            }
            Ok(None) => read_capped(&self.cfg.system_pins, pins::MAX_FILE_BYTES),
            Err(e) => {
                // It exists; an import must not replace it.
                self.user_pins = true;
                Err(e)
            }
        };
        match read {
            Ok(Some(bytes)) => {
                self.pins = Pins::parse(&bytes);
                if self.pins.skipped() > 0 {
                    log::info!("pins: {} lines skipped", self.pins.skipped());
                }
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!("pins not read: {}", io_kind(&e));
                self.out.problem(ProblemKind::PinsUnreadable);
            }
        }
    }

    fn change_pins(&mut self, f: impl FnOnce(&mut Pins) -> bool) {
        if f(&mut self.pins) {
            self.save_pins();
        }
    }

    fn import_pins(&mut self, list: &[String]) {
        if self.user_pins {
            self.out.problem(ProblemKind::ImportSkipped);
            return;
        }
        let imported = Pins::import(list);
        if imported.ids().is_empty() {
            self.out.problem(ProblemKind::ImportSkipped);
            return;
        }
        self.pins = imported;
        self.save_pins();
    }

    fn save_pins(&mut self) {
        let path = self.cfg.config_dir.join(PINS_FILE);
        match write_atomic(&path, &self.pins.to_bytes(), 0o600) {
            Ok(()) => self.user_pins = true,
            Err(e) => {
                log::warn!("pins not saved: {}", io_kind(&e));
                self.out.problem(ProblemKind::PinsWriteFailed);
            }
        }
        self.out.send(Update::Pins(self.pins.ids().to_vec()));
    }

    fn reload_recent(&mut self) {
        // Stat before reading: a change during the read is seen next time.
        self.xbel_stamp = stamp(&self.cfg.xbel);
        let recent = match RecentFiles::load(&self.cfg.xbel) {
            Ok(mut r) => {
                r.retain_existing();
                if r.malformed {
                    log::info!("recent files: list was cut short (malformed)");
                }
                r
            }
            Err(RecentError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                RecentFiles::default()
            }
            Err(e) => {
                let kind = match e {
                    RecentError::TooLarge => "too large".to_owned(),
                    RecentError::Io(e) => io_kind(&e),
                };
                log::warn!("recent files not read: {kind}");
                self.out.problem(ProblemKind::RecentUnreadable);
                RecentFiles::default()
            }
        };
        log::debug!("recent files: {}", recent.items.len());
        self.to_search(SearchMsg::RecentLoaded(Arc::new(recent)));
    }

    fn reload_settings(&mut self) {
        self.settings_stamp = stamp(&self.cfg.settings_index);
        let index = match SettingsIndex::load(&self.cfg.settings_index, &self.cfg.locales) {
            Ok(i) => {
                log::debug!("settings index: {} entries, {} skipped", i.len(), i.skipped);
                Some(Arc::new(i))
            }
            Err(IndexError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                log::warn!("settings index is missing");
                None
            }
            Err(e) => {
                let kind = match e {
                    IndexError::TooLarge => "too large".to_owned(),
                    IndexError::Syntax(_) => "syntax".to_owned(),
                    IndexError::UnsupportedVersion(_) => "version".to_owned(),
                    IndexError::TooManyEntries => "too many entries".to_owned(),
                    IndexError::Io(e) => io_kind(&e),
                };
                log::warn!("settings index not used: {kind}");
                self.out.problem(ProblemKind::SettingsIndexUnreadable);
                None
            }
        };
        self.to_search(SearchMsg::SettingsLoaded(index));
    }

    fn rescan_path(&mut self) {
        let started = Instant::now();
        self.path = Arc::new(PathCache::scan(&self.cfg.path_var));
        log::debug!(
            "PATH scan: {} names in {} ms",
            self.path.len(),
            started.elapsed().as_millis()
        );
        self.to_search(SearchMsg::PathLoaded(Arc::clone(&self.path)));
    }

    /// The panel opened: one stat per source, reload only what changed.
    fn refresh(&mut self) {
        if stamp(&self.cfg.xbel) != self.xbel_stamp {
            self.reload_recent();
        }
        if stamp(&self.cfg.settings_index) != self.settings_stamp {
            self.reload_settings();
        }
        if self.path.is_stale() {
            self.rescan_path();
        }
        self.to_search(SearchMsg::Opened);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    const WAIT: Duration = Duration::from_secs(10);

    struct Fixture {
        dir: tempfile::TempDir,
        cfg: EngineConfig,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let home = p.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = EngineConfig {
            home,
            state_dir: p.join("state"),
            config_dir: p.join("config"),
            system_pins: p.join("etc-pinned.list"),
            xbel: p.join("recently-used.xbel"),
            settings_index: p.join("search-index.json"),
            path_var: String::new(),
            locales: vec!["en".into()],
            session: SessionAvailability::default(),
        };
        Fixture { dir, cfg }
    }

    fn start(f: &Fixture) -> (Engine, Receiver<Update>) {
        start_with(f, SearchOptions::default())
    }

    fn start_with(f: &Fixture, opts: SearchOptions) -> (Engine, Receiver<Update>) {
        let (tx, rx) = mpsc::channel();
        let e = Engine::start(
            f.cfg.clone(),
            opts,
            Box::new(move |u| {
                let _ = tx.send(u);
            }),
        );
        (e, rx)
    }

    /// The next update that satisfies `pick`, within the timeout.
    fn wait<T>(rx: &Receiver<Update>, mut pick: impl FnMut(Update) -> Option<T>) -> T {
        let end = Instant::now() + WAIT;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            let u = rx
                .recv_timeout(left)
                .expect("timed out waiting for an update");
            if let Some(t) = pick(u) {
                return t;
            }
        }
    }

    fn results(rx: &Receiver<Update>, serial: u64) -> Vec<ResultItem> {
        wait(rx, |u| match u {
            Update::Results { serial: s, items } if s == serial => Some(items),
            _ => None,
        })
    }

    fn pins_update(rx: &Receiver<Update>) -> Vec<String> {
        wait(rx, |u| match u {
            Update::Pins(p) => Some(p),
            _ => None,
        })
    }

    /// Shuts down on a helper thread so a hang fails the test.
    fn stop(mut e: Engine) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            e.shutdown();
            let _ = tx.send(());
        });
        rx.recv_timeout(WAIT).expect("shutdown hung");
    }

    fn app(id: &str, name: &str) -> AppEntry {
        AppEntry {
            desktop_id: id.into(),
            name: name.into(),
            icon: "application-x-executable".into(),
            ..AppEntry::default()
        }
    }

    fn two_apps() -> Vec<AppEntry> {
        vec![
            app("alpha-editor.desktop", "Alpha Editor"),
            app("alpha-viewer.desktop", "Alpha Viewer"),
        ]
    }

    fn runner_item(id: &str) -> ResultItem {
        ResultItem {
            id: id.into(),
            kind: Kind::Runner,
            title: format!("Runner {id}"),
            subtitle: String::new(),
            icon: String::new(),
            score: 0.5,
            action: Action::Runner {
                runner_id: "r".into(),
                match_id: id.into(),
            },
        }
    }

    #[test]
    fn starts_with_no_files() {
        let f = fixture();
        let (e, rx) = start(&f);
        assert!(pins_update(&rx).is_empty());
        e.query(1, String::new());
        assert!(results(&rx, 1).is_empty());
        e.set_apps(two_apps());
        e.refresh();
        let (apps, files) = wait(&rx, |u| match u {
            Update::Recent { apps, files } => Some((apps, files)),
            _ => None,
        });
        assert!(apps.is_empty() && files.is_empty());
        assert_eq!(e.stats().problems, 0);
        stop(e);
    }

    #[test]
    fn query_has_web_last() {
        let f = fixture();
        let (e, rx) = start(&f);
        e.set_apps(two_apps());
        e.query(1, "alpha".into());
        let items = results(&rx, 1);
        assert!(items.len() >= 3);
        assert_eq!(items.last().unwrap().kind, Kind::Web);
        assert_eq!(items[0].kind, Kind::App);
        stop(e);
    }

    #[test]
    fn newest_query_wins() {
        let f = fixture();
        // The callback holds the worker on the first result until the other
        // 99 queries are queued, so they must be coalesced.
        let (tx, rx) = mpsc::channel();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let gate = Mutex::new(gate_rx);
        let e = Engine::start(
            f.cfg.clone(),
            SearchOptions::default(),
            Box::new(move |u| {
                let first = matches!(u, Update::Results { serial: 1, .. });
                let _ = tx.send(u);
                if first {
                    let _ = gate.lock().unwrap().recv_timeout(WAIT);
                }
            }),
        );
        e.set_apps(two_apps());
        e.query(1, "alpha".into());
        results(&rx, 1);
        for serial in 2..=100 {
            e.query(serial, format!("alpha {serial}"));
        }
        gate_tx.send(()).unwrap();
        let last = results(&rx, 100);
        assert!(last.last().is_some());
        while let Ok(u) = rx.recv_timeout(Duration::from_millis(100)) {
            assert!(
                !matches!(u, Update::Results { .. }),
                "a coalesced query was answered"
            );
        }
        let s = e.stats();
        assert_eq!(s.queries_computed, 2);
        assert_eq!(s.queries_coalesced, 98);
        stop(e);
    }

    #[test]
    fn stale_merge_is_ignored() {
        let f = fixture();
        let (e, rx) = start(&f);
        e.set_apps(two_apps());
        e.query(1, "alpha".into());
        results(&rx, 1);
        e.query(2, "alpha".into());
        results(&rx, 2);
        e.merge(1, Source::Runner, vec![runner_item("stale")]);
        e.merge(2, Source::Runner, vec![runner_item("fresh")]);
        let items = results(&rx, 2);
        assert!(items.iter().any(|i| i.id == "fresh"));
        assert!(!items.iter().any(|i| i.id == "stale"));
        assert_eq!(items.last().unwrap().kind, Kind::Web);
        assert_eq!(e.stats().merges_stale, 1);
        // The instant source is never merged.
        e.merge(2, Source::Instant, vec![runner_item("x")]);
        e.select(Some("fresh".into()));
        e.query(3, "alpha".into());
        assert!(!results(&rx, 3).iter().any(|i| i.id == "x"));
        stop(e);
    }

    #[test]
    fn record_persists_and_reorders() {
        let f = fixture();
        let (e, rx) = start(&f);
        pins_update(&rx);
        e.set_apps(two_apps());
        e.query(1, "alpha".into());
        let before = results(&rx, 1);
        assert_eq!(before[0].id, "app:alpha-editor.desktop");
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        stop(e);

        let file = f.cfg.state_dir.join(USAGE_FILE);
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        let (e, rx) = start(&f);
        pins_update(&rx);
        e.set_apps(two_apps());
        e.query(1, "alpha".into());
        assert_eq!(results(&rx, 1)[0].id, "app:alpha-viewer.desktop");
        // Used apps show on the Start page.
        e.refresh();
        let apps = wait(&rx, |u| match u {
            Update::Recent { apps, .. } if !apps.is_empty() => Some(apps),
            _ => None,
        });
        assert_eq!(apps[0].id, "app:alpha-viewer.desktop");
        stop(e);
    }

    #[test]
    fn record_before_load_is_kept() {
        let f = fixture();
        let (e, _rx) = start(&f);
        // No wait for the load: the change is held until the file is read.
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        stop(e);
        let text = std::fs::read_to_string(f.cfg.state_dir.join(USAGE_FILE)).unwrap();
        assert!(text.contains("app:alpha-viewer.desktop"));
    }

    #[test]
    fn only_learnable_ids_are_written() {
        let f = fixture();
        let (e, rx) = start(&f);
        pins_update(&rx);
        e.record("a".into(), "file:file:///home/x/secret.txt".into());
        e.record("ls".into(), "cmd:ls".into());
        e.record("1+1".into(), "calc:2".into());
        stop(e);
        assert!(!f.cfg.state_dir.join(USAGE_FILE).exists());

        let off = SearchOptions {
            learn: false,
            ..SearchOptions::default()
        };
        let (e, rx) = start_with(&f, off);
        pins_update(&rx);
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        stop(e);
        assert!(!f.cfg.state_dir.join(USAGE_FILE).exists());
    }

    #[test]
    fn clear_history_deletes_the_file() {
        let f = fixture();
        let (e, rx) = start(&f);
        pins_update(&rx);
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        stop(e);
        let file = f.cfg.state_dir.join(USAGE_FILE);
        assert!(file.exists());

        let (e, rx) = start(&f);
        pins_update(&rx);
        e.clear_history();
        let r = wait(&rx, |u| match u {
            Update::HistoryCleared(r) => Some(r),
            _ => None,
        });
        assert_eq!(r, Ok(()));
        assert!(!file.exists());
        // Missing is fine too.
        e.clear_history();
        let r = wait(&rx, |u| match u {
            Update::HistoryCleared(r) => Some(r),
            _ => None,
        });
        assert_eq!(r, Ok(()));
        stop(e);
        assert!(!file.exists());
    }

    #[test]
    fn pins_persist_and_move() {
        let f = fixture();
        let (e, rx) = start(&f);
        assert!(pins_update(&rx).is_empty());
        e.pin("a.desktop".into());
        assert_eq!(pins_update(&rx), ["a.desktop"]);
        e.pin("b.desktop".into());
        assert_eq!(pins_update(&rx), ["a.desktop", "b.desktop"]);
        e.move_pin("b.desktop".into(), 0);
        assert_eq!(pins_update(&rx), ["b.desktop", "a.desktop"]);
        e.pin("not a valid id".into());
        e.unpin("a.desktop".into());
        assert_eq!(pins_update(&rx), ["b.desktop"]);
        e.pin("a.desktop".into());
        pins_update(&rx);
        stop(e);

        let file = f.cfg.config_dir.join(PINS_FILE);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let (e, rx) = start(&f);
        assert_eq!(pins_update(&rx), ["b.desktop", "a.desktop"]);
        stop(e);
    }

    #[test]
    fn import_only_without_a_user_list() {
        let f = fixture();
        let (e, rx) = start(&f);
        pins_update(&rx);
        e.import_pins(vec![
            "applications:x.desktop".into(),
            "bogus entry".into(),
            "preferred://browser".into(),
        ]);
        assert_eq!(pins_update(&rx), ["x.desktop", "preferred://browser"]);
        // Now the user has a list: a second import is refused.
        e.import_pins(vec!["y.desktop".into()]);
        wait(&rx, |u| {
            matches!(u, Update::Problem(ProblemKind::ImportSkipped)).then_some(())
        });
        stop(e);
        let text = std::fs::read_to_string(f.cfg.config_dir.join(PINS_FILE)).unwrap();
        assert!(!text.contains("y.desktop"));
    }

    #[test]
    fn system_pins_are_the_default() {
        let f = fixture();
        std::fs::write(&f.cfg.system_pins, "# system\nsys.desktop\n").unwrap();
        let (e, rx) = start(&f);
        assert_eq!(pins_update(&rx), ["sys.desktop"]);
        assert!(!f.cfg.config_dir.join(PINS_FILE).exists());
        // No user list yet, so an import may replace the system default.
        e.import_pins(vec!["mine.desktop".into()]);
        assert_eq!(pins_update(&rx), ["mine.desktop"]);
        stop(e);
    }

    fn write_xbel(f: &Fixture, names: &[&str]) {
        let mut x = String::from("<xbel version=\"1.0\">");
        for (i, n) in names.iter().enumerate() {
            let path = f.cfg.home.join(n);
            std::fs::write(&path, "x").unwrap();
            x.push_str(&format!(
                "<bookmark href=\"file://{}\" modified=\"2024-05-{:02}T10:00:00Z\"/>",
                path.display(),
                10 + i
            ));
        }
        x.push_str("</xbel>");
        std::fs::write(&f.cfg.xbel, x).unwrap();
    }

    fn recent_files(rx: &Receiver<Update>, n: usize) -> Vec<ResultItem> {
        wait(rx, |u| match u {
            Update::Recent { files, .. } if files.len() == n => Some(files),
            _ => None,
        })
    }

    #[test]
    fn refresh_picks_up_a_changed_xbel() {
        let f = fixture();
        write_xbel(&f, &["one.txt"]);
        let (e, rx) = start(&f);
        recent_files(&rx, 1);
        write_xbel(&f, &["one.txt", "two.txt"]);
        e.refresh();
        let files = recent_files(&rx, 2);
        assert!(files.iter().any(|i| i.title == "two.txt"));
        // Unchanged: still reported, not an error.
        e.refresh();
        recent_files(&rx, 2);
        assert_eq!(e.stats().problems, 0);
        stop(e);
    }

    #[test]
    fn oversized_usage_is_a_problem_not_a_crash() {
        let f = fixture();
        std::fs::create_dir_all(&f.cfg.state_dir).unwrap();
        let file = f.cfg.state_dir.join(USAGE_FILE);
        std::fs::write(&file, vec![b'x'; (usage::MAX_FILE_BYTES + 10) as usize]).unwrap();
        let (e, rx) = start(&f);
        wait(&rx, |u| {
            matches!(u, Update::Problem(ProblemKind::UsageUnreadable)).then_some(())
        });
        e.set_apps(two_apps());
        e.query(1, "alpha".into());
        assert_eq!(results(&rx, 1)[0].id, "app:alpha-editor.desktop");
        // A new use replaces the bad file.
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        stop(e);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("alpha-viewer"));
    }

    #[test]
    fn corrupt_usage_is_a_problem_and_empty() {
        let f = fixture();
        std::fs::create_dir_all(&f.cfg.state_dir).unwrap();
        std::fs::write(
            f.cfg.state_dir.join(USAGE_FILE),
            "garbage\n\u{0}\u{1}\nmore garbage\n",
        )
        .unwrap();
        let (e, rx) = start(&f);
        wait(&rx, |u| {
            matches!(u, Update::Problem(ProblemKind::UsageCorrupt)).then_some(())
        });
        e.set_apps(two_apps());
        e.query(1, "alpha".into());
        assert_eq!(results(&rx, 1)[0].id, "app:alpha-editor.desktop");
        stop(e);
    }

    #[test]
    fn a_panic_is_reported_and_the_engine_goes_on() {
        let f = fixture();
        let (e, rx) = start(&f);
        e.set_apps(two_apps());
        e.inject_panic();
        wait(&rx, |u| {
            matches!(u, Update::Problem(ProblemKind::Panicked)).then_some(())
        });
        e.query(1, "alpha".into());
        assert!(!results(&rx, 1).is_empty());
        stop(e);
    }

    #[test]
    fn a_panicking_callback_does_not_kill_the_engine() {
        let f = fixture();
        let (tx, rx) = mpsc::channel();
        let e = Engine::start(
            f.cfg.clone(),
            SearchOptions::default(),
            Box::new(move |u| {
                if matches!(u, Update::Results { serial: 1, .. }) {
                    panic!("callback");
                }
                let _ = tx.send(u);
            }),
        );
        e.set_apps(two_apps());
        e.query(1, "alpha".into());
        e.query(2, "alpha".into());
        assert!(!results(&rx, 2).is_empty());
        stop(e);
    }

    #[test]
    fn stale_temp_files_are_swept() {
        let f = fixture();
        std::fs::create_dir_all(&f.cfg.state_dir).unwrap();
        let tmp = f.cfg.state_dir.join(".usage.tsv.tmp-1-0");
        std::fs::write(&tmp, "x").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&tmp)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let fresh = f.cfg.state_dir.join(".usage.tsv.tmp-2-0");
        std::fs::write(&fresh, "x").unwrap();
        let (e, rx) = start(&f);
        pins_update(&rx);
        stop(e);
        assert!(!tmp.exists());
        assert!(fresh.exists());
        let _ = &f.dir;
    }

    #[test]
    fn drop_joins_the_threads() {
        let f = fixture();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (e, _rx) = start(&f);
            e.set_apps(two_apps());
            e.query(1, "alpha".into());
            drop(e);
            let _ = tx.send(());
        });
        rx.recv_timeout(WAIT).expect("drop hung");
    }
}
