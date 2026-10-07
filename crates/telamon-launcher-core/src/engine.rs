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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::catalog::{self, AppEntry, Catalog};
use crate::commands::{PathCache, SessionAvailability, SessionCommands};
use crate::fsutil::{read_capped, sweep_stale_temps, write_atomic};
use crate::pins::{self, Pins};
use crate::query::{self, Context, ResultSet, SearchOptions, Source, Sources};
use crate::recent::{RecentError, RecentFiles};
use crate::result::{Kind, ResultItem, prior};
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
const LOAD_WAIT: Duration = Duration::from_secs(1);
/// Longest `shutdown` waits for the workers in all; a worker stuck in a slow
/// file system call is left to finish on its own.
/// What the IO worker is always given at shutdown, for the final write.
const IO_FLOOR: Duration = Duration::from_millis(500);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);
/// Most entries taken from an import list.
const MAX_IMPORT_LIST: usize = 1024;

const USAGE_FILE: &str = "usage.tsv";
const PINS_FILE: &str = "pinned.list";

/// Where things are. Every path is the caller's; nothing is read from the
/// environment here.
#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub home: PathBuf,
    /// `$XDG_STATE_HOME/telamon-launcher`: `usage.tsv`.
    pub state_dir: PathBuf,
    /// `$XDG_CONFIG_HOME/telamon-launcher`: `pinned.list`.
    pub config_dir: PathBuf,
    /// `/etc/xdg/telamon-launcher/pinned.list`, used until the user has a list.
    pub system_pins: PathBuf,
    /// `recently-used.xbel`.
    pub xbel: PathBuf,
    /// Telamon Settings' `search-index.json`.
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
    /// Set by `shutdown`: a worker left running never calls back after it.
    closed: Arc<AtomicBool>,
}

impl Emitter {
    fn send(&self, u: Update) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        self.deliver(u);
    }

    fn deliver(&self, u: Update) {
        if catch_unwind(AssertUnwindSafe(|| (self.cb)(u))).is_err() {
            log::error!("the update callback panicked");
        }
    }

    /// Reports a problem from a one-off thread, for when the caller is still
    /// inside `Engine::start`.
    fn problem_deferred(&self, kind: ProblemKind) {
        self.counters.problems.fetch_add(1, Ordering::Relaxed);
        log::warn!("problem: {kind:?}");
        let me = self.clone();
        if thread::Builder::new()
            .name("launcher-report".into())
            .spawn(move || me.deliver(Update::Problem(kind)))
            .is_err()
        {
            log::error!("the problem could not be reported");
        }
    }

    fn problem(&self, kind: ProblemKind) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        self.counters.problems.fetch_add(1, Ordering::Relaxed);
        log::warn!("problem: {kind:?}");
        self.send(Update::Problem(kind));
    }
}

enum SearchMsg {
    SetApps(Vec<AppEntry>),
    SetOptions(SearchOptions),
    SetSession(SessionAvailability),
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
    // From the IO and scan workers.
    UsageLoaded(UsageStore),
    /// `usage.tsv` could not be written: send the history again later.
    UsageWriteFailed,
    /// Sent only when the list changed; the Start page is reported again.
    RecentLoaded(Arc<RecentFiles>),
    SettingsLoaded(Option<Arc<SettingsIndex>>),
    PathLoaded(Arc<PathCache>),
    /// The panel opened: report the Start page again and retry a failed
    /// save. The scan worker follows with what changed on disk.
    Retry,
    #[cfg(test)]
    TestPanic,
    Shutdown,
}

enum IoMsg {
    Pin(String),
    Unpin(String),
    MovePin(String, usize),
    ImportPins(Vec<String>),
    WriteUsage(Vec<u8>),
    ClearUsage,
    Shutdown,
}

enum ScanMsg {
    /// The panel opened.
    Refresh,
    Shutdown,
}

/// Switches for tests to hold or break a worker at a chosen point.
#[derive(Default)]
struct Hooks {
    /// The IO worker waits for this before reading `usage.tsv`.
    #[cfg(test)]
    load_gate: Option<Receiver<()>>,
    /// The scan worker waits for this before its first stage.
    #[cfg(test)]
    scan_gate: Option<Receiver<()>>,
    /// While set, every `usage.tsv` write fails.
    #[cfg(test)]
    write_fail: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Reading `usage.tsv` panics.
    #[cfg(test)]
    panic_usage: bool,
    /// The scan thread "fails to start".
    #[cfg(test)]
    fail_scan_spawn: bool,
}

/// The engine's handle. Methods never block: each sends one message.
pub struct Engine {
    search_tx: Sender<SearchMsg>,
    io_tx: Sender<IoMsg>,
    scan_tx: Sender<ScanMsg>,
    search: Option<JoinHandle<()>>,
    io: Option<JoinHandle<()>>,
    scan: Option<JoinHandle<()>>,
    counters: Arc<Counters>,
    closed: Arc<AtomicBool>,
}

impl Engine {
    /// Starts the worker threads: search, IO (reads and writes of the user's
    /// files) and scan (the slow reads: recent files, Settings index, `PATH`),
    /// so a slow read never delays a write. `on_update` is called from them.
    ///
    /// All start or none do. If a thread cannot be started the engine is
    /// inert ([`Engine::is_running`] is false, every method does nothing) and
    /// `Update::Problem(ThreadStart)` follows from a short-lived thread, never
    /// from inside this call.
    pub fn start(
        cfg: EngineConfig,
        opts: SearchOptions,
        on_update: Box<dyn Fn(Update) + Send + Sync>,
    ) -> Engine {
        Self::start_with(cfg, opts, on_update, Hooks::default())
    }

    fn start_with(
        cfg: EngineConfig,
        opts: SearchOptions,
        on_update: Box<dyn Fn(Update) + Send + Sync>,
        mut hooks: Hooks,
    ) -> Engine {
        let counters = Arc::new(Counters::default());
        let out = Emitter {
            cb: Arc::from(on_update),
            counters: Arc::clone(&counters),
            closed: Arc::new(AtomicBool::new(false)),
        };
        let (search_tx, search_rx) = mpsc::channel();
        let (io_tx, io_rx) = mpsc::channel();
        let (scan_tx, scan_rx) = mpsc::channel();

        let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| {
            thread::Builder::new()
                .name(name.into())
                .spawn(f)
                .map_err(|e| log::error!("{name} thread did not start: {:?}", e.kind()))
                .ok()
        };
        let search_worker = Search::new(&cfg, opts, out.clone(), io_tx.clone());
        let search = spawn(
            "launcher-search",
            Box::new(move || search_worker.run(search_rx)),
        );
        let io_worker = Io::new(&cfg, out.clone(), search_tx.clone(), &mut hooks);
        let io = spawn("launcher-io", Box::new(move || io_worker.run(io_rx)));
        let scan_worker = Scan::new(cfg, out.clone(), search_tx.clone(), &mut hooks);
        #[cfg(test)]
        let fail_scan = hooks.fail_scan_spawn;
        #[cfg(not(test))]
        let fail_scan = false;
        let scan = if fail_scan {
            None
        } else {
            spawn("launcher-scan", Box::new(move || scan_worker.run(scan_rx)))
        };

        let mut e = Engine {
            search_tx,
            io_tx,
            scan_tx,
            search,
            io,
            scan,
            counters,
            closed: Arc::clone(&out.closed),
        };
        if e.search.is_none() || e.io.is_none() || e.scan.is_none() {
            e.shutdown();
            out.problem_deferred(ProblemKind::ThreadStart);
        }
        e
    }

    /// False when the threads did not start or `shutdown` has run.
    pub fn is_running(&self) -> bool {
        self.search.is_some() && self.io.is_some() && self.scan.is_some()
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

    /// Which session commands exist, when logind or the seat answer after
    /// the start.
    pub fn set_session(&self, session: SessionAvailability) {
        self.to_search(SearchMsg::SetSession(session));
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

    /// The panel opened: report the Start page at once, retry a failed save
    /// of the history, then reload what changed on disk (and check again
    /// that the recent files exist), reporting the Start page again if the
    /// recent files changed.
    pub fn refresh(&self) {
        self.to_search(SearchMsg::Retry);
        if self.scan_tx.send(ScanMsg::Refresh).is_err() {
            log::debug!("scan worker is gone; message dropped");
        }
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

    /// Stops the workers and waits for them, but for 2 s at most: a worker
    /// stuck in a slow file system call (a dead network mount) is left to
    /// finish by itself and the engine is dropped anyway. Writes already
    /// requested are finished first, by the workers that are not stuck. Safe
    /// to call twice; later calls do nothing.
    pub fn shutdown(&mut self) {
        // No callback after this point, from any worker.
        self.closed.store(true, Ordering::SeqCst);
        let end = Instant::now() + SHUTDOWN_WAIT;
        // Search first: it may still queue a write for the IO worker.
        let _ = self.search_tx.send(SearchMsg::Shutdown);
        join_until(self.search.take(), end);
        let _ = self.io_tx.send(IoMsg::Shutdown);
        let _ = self.scan_tx.send(ScanMsg::Shutdown);
        // The last usage write gets time even if the search worker used it up.
        join_until(self.io.take(), end.max(Instant::now() + IO_FLOOR));
        join_until(self.scan.take(), end);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Waits for a thread until `end`; past that the thread is detached (it has
/// been told to stop and ends on its own) and a warning is logged.
fn join_until(h: Option<JoinHandle<()>>, end: Instant) {
    let Some(h) = h else { return };
    // Never wait for ourselves (an Engine dropped inside the callback).
    if h.thread().id() == thread::current().id() {
        return;
    }
    while !h.is_finished() {
        if Instant::now() >= end {
            log::warn!(
                "{} did not stop in time; left running",
                h.thread().name().unwrap_or("worker")
            );
            return;
        }
        thread::sleep(Duration::from_millis(2));
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
    /// A write of `usage.tsv` failed: send the history again.
    unsaved: bool,
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
            unsaved: false,
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
        // Failures reported while stopping still count: try the write again.
        while let Ok(m) = rx.try_recv() {
            if matches!(m, SearchMsg::UsageWriteFailed)
                && (!self.usage.is_empty() || self.usage.is_dirty())
            {
                self.unsaved = true;
            }
        }
        if self.usage_loaded {
            self.guarded(Search::flush_usage);
        }
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
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    log::warn!("history changes lost: the history did not load in time");
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    log::warn!("history changes lost: the IO worker is gone");
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
            SearchMsg::SetSession(s) => {
                self.sources.session = Arc::new(SessionCommands::new(s));
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
            SearchMsg::UsageWriteFailed => {
                // Not after a clear: an empty, clean store has nothing to send.
                if !self.usage.is_empty() || self.usage.is_dirty() {
                    self.unsaved = true;
                }
            }
            SearchMsg::Retry => {
                self.recent_dirty = true;
                // Retry a write that failed.
                if self.usage_loaded {
                    self.flush_usage();
                }
            }
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
        if !self.usage.is_dirty() && !self.unsaved {
            return;
        }
        let bytes = self.usage.to_bytes();
        self.usage.mark_clean();
        self.unsaved = false;
        if self.io.send(IoMsg::WriteUsage(bytes)).is_err() {
            log::debug!("io worker is gone; usage not saved");
        }
    }

    fn clear_history(&mut self) {
        if self.usage_loaded {
            self.usage.clear();
            self.usage.mark_clean();
            self.unsaved = false;
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
                    self.unsaved = false;
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
                    apps.push(catalog::app_item(
                        app,
                        prior(Kind::App) / (1.0 + apps.len() as f32),
                    ));
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

/// A file's identity for "did it change": modification time and size
/// (`None` when it is missing).
type Stamp = Option<(SystemTime, u64)>;

fn stamp(p: &Path) -> Stamp {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Reads of the user's small files at start, and every write. Nothing slow
/// runs here, so a write never waits behind a scan.
struct Io {
    state_dir: PathBuf,
    config_dir: PathBuf,
    system_pins: PathBuf,
    out: Emitter,
    search: Sender<SearchMsg>,
    pins: Pins,
    /// Whether the user has a `pinned.list` (or one that could not be read,
    /// which must not be replaced by an import).
    user_pins: bool,
    #[cfg(test)]
    load_gate: Option<Receiver<()>>,
    #[cfg(test)]
    write_fail: Option<Arc<std::sync::atomic::AtomicBool>>,
    #[cfg(test)]
    panic_usage: bool,
}

impl Io {
    fn new(cfg: &EngineConfig, out: Emitter, search: Sender<SearchMsg>, hooks: &mut Hooks) -> Io {
        #[cfg(not(test))]
        let _ = hooks;
        Io {
            state_dir: cfg.state_dir.clone(),
            config_dir: cfg.config_dir.clone(),
            system_pins: cfg.system_pins.clone(),
            out,
            search,
            pins: Pins::default(),
            user_pins: false,
            #[cfg(test)]
            load_gate: hooks.load_gate.take(),
            #[cfg(test)]
            write_fail: hooks.write_fail.take(),
            #[cfg(test)]
            panic_usage: hooks.panic_usage,
        }
    }

    fn to_search(&self, m: SearchMsg) {
        if self.search.send(m).is_err() {
            log::debug!("search worker is gone; message dropped");
        }
    }

    fn run(mut self, rx: Receiver<IoMsg>) {
        self.load_all();
        // Blocks; no timers. Ends on Shutdown or when every sender is gone.
        while let Ok(m) = rx.recv() {
            if matches!(m, IoMsg::Shutdown) {
                break;
            }
            self.guarded(|io| io.handle(m));
        }
        // A search worker that was left running may have queued a last write
        // behind Shutdown; do it, and say what is dropped.
        let mut dropped = 0;
        while let Ok(m) = rx.try_recv() {
            match m {
                IoMsg::WriteUsage(_) => self.guarded(|io| io.handle(m)),
                _ => dropped += 1,
            }
        }
        if dropped > 0 {
            log::warn!("{dropped} io messages dropped at shutdown");
        }
        log::debug!("io worker stopped");
    }

    /// Runs one unit of work; a panic is reported and the worker goes on.
    fn guarded(&mut self, f: impl FnOnce(&mut Io)) {
        if catch_unwind(AssertUnwindSafe(|| f(self))).is_err() {
            log::error!("io worker caught a panic");
            self.out.problem(ProblemKind::Panicked);
        }
    }

    fn handle(&mut self, m: IoMsg) {
        match m {
            IoMsg::Pin(id) => self.change_pins(|p| p.pin(&id)),
            IoMsg::Unpin(id) => self.change_pins(|p| p.unpin(&id)),
            IoMsg::MovePin(id, i) => self.change_pins(|p| p.move_to(&id, i)),
            IoMsg::ImportPins(list) => self.import_pins(&list),
            IoMsg::WriteUsage(bytes) => {
                if let Err(e) = self.write_usage(&bytes) {
                    log::warn!("usage not saved: {}", io_kind(&e));
                    // The search worker keeps the history and sends it again.
                    self.to_search(SearchMsg::UsageWriteFailed);
                    self.out.problem(ProblemKind::UsageWriteFailed);
                }
            }
            IoMsg::ClearUsage => {
                let path = self.state_dir.join(USAGE_FILE);
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

    fn write_usage(&self, bytes: &[u8]) -> std::io::Result<()> {
        #[cfg(test)]
        if self
            .write_fail
            .as_ref()
            .is_some_and(|f| f.load(Ordering::SeqCst))
        {
            return Err(std::io::Error::other("injected"));
        }
        write_atomic(&self.state_dir.join(USAGE_FILE), bytes, 0o600)
    }

    /// What is read at start: stale temp files, history, pins. Each stage is
    /// guarded on its own, and the search worker always gets a history (empty
    /// when the stage failed), so changes waiting for it are never stuck.
    fn load_all(&mut self) {
        #[cfg(test)]
        if let Some(gate) = self.load_gate.take() {
            let _ = gate.recv_timeout(Duration::from_secs(10));
        }
        let (state, config) = (self.state_dir.clone(), self.config_dir.clone());
        if let Some(swept) = self.stage(|_| {
            sweep_stale_temps(&state, USAGE_FILE, STALE_TEMP_AGE)
                + sweep_stale_temps(&config, PINS_FILE, STALE_TEMP_AGE)
        }) && swept > 0
        {
            log::info!("removed {swept} stale temp files");
        }
        let usage = self.stage(Io::load_usage).unwrap_or_default();
        self.to_search(SearchMsg::UsageLoaded(usage));
        self.guarded(Io::load_pins);
        self.out.send(Update::Pins(self.pins.ids().to_vec()));
    }

    /// Like `guarded`, but returns the result (None after a panic).
    fn stage<T>(&mut self, f: impl FnOnce(&mut Io) -> T) -> Option<T> {
        match catch_unwind(AssertUnwindSafe(|| f(self))) {
            Ok(v) => Some(v),
            Err(_) => {
                log::error!("io worker caught a panic");
                self.out.problem(ProblemKind::Panicked);
                None
            }
        }
    }

    fn load_usage(&mut self) -> UsageStore {
        #[cfg(test)]
        if self.panic_usage {
            panic!("injected");
        }
        let path = self.state_dir.join(USAGE_FILE);
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
        let user = self.config_dir.join(PINS_FILE);
        let read = match read_capped(&user, pins::MAX_FILE_BYTES) {
            Ok(Some(b)) => {
                self.user_pins = true;
                Ok(Some(b))
            }
            Ok(None) => read_capped(&self.system_pins, pins::MAX_FILE_BYTES),
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
        let path = self.config_dir.join(PINS_FILE);
        match write_atomic(&path, &self.pins.to_bytes(), 0o600) {
            Ok(()) => self.user_pins = true,
            Err(e) => {
                log::warn!("pins not saved: {}", io_kind(&e));
                self.out.problem(ProblemKind::PinsWriteFailed);
            }
        }
        self.out.send(Update::Pins(self.pins.ids().to_vec()));
    }
}

/// The slow reads: recent files (with one `stat` per entry), the Settings
/// index and the `PATH` scan. On its own thread so that nothing here can hold
/// up a write.
struct Scan {
    xbel: PathBuf,
    settings_index: PathBuf,
    path_var: String,
    locales: Vec<String>,
    out: Emitter,
    search: Sender<SearchMsg>,
    xbel_stamp: Stamp,
    settings_stamp: Stamp,
    recent: Arc<RecentFiles>,
    path: Arc<PathCache>,
    #[cfg(test)]
    gate: Option<Receiver<()>>,
}

impl Scan {
    fn new(cfg: EngineConfig, out: Emitter, search: Sender<SearchMsg>, hooks: &mut Hooks) -> Scan {
        #[cfg(not(test))]
        let _ = hooks;
        Scan {
            xbel: cfg.xbel,
            settings_index: cfg.settings_index,
            path_var: cfg.path_var,
            locales: cfg.locales,
            out,
            search,
            xbel_stamp: None,
            settings_stamp: None,
            recent: Arc::new(RecentFiles::default()),
            path: Arc::new(PathCache::default()),
            #[cfg(test)]
            gate: hooks.scan_gate.take(),
        }
    }

    fn to_search(&self, m: SearchMsg) {
        if self.search.send(m).is_err() {
            log::debug!("search worker is gone; message dropped");
        }
    }

    fn run(mut self, rx: Receiver<ScanMsg>) {
        #[cfg(test)]
        if let Some(gate) = self.gate.take() {
            let _ = gate.recv_timeout(Duration::from_secs(5));
        }
        self.guarded(Scan::reload_recent);
        self.guarded(Scan::reload_settings);
        self.guarded(Scan::rescan_path);
        // Blocks; no timers. Ends on Shutdown or when every sender is gone.
        'run: while let Ok(m) = rx.recv() {
            let mut next = Some(m);
            // Opens that queued up while a refresh ran count as one.
            while let Some(m) = next {
                if matches!(m, ScanMsg::Shutdown) {
                    break 'run;
                }
                next = rx.try_recv().ok();
            }
            self.refresh();
        }
        log::debug!("scan worker stopped");
    }

    /// Runs one stage; a panic is reported and the worker goes on.
    fn guarded(&mut self, f: impl FnOnce(&mut Scan)) {
        if catch_unwind(AssertUnwindSafe(|| f(self))).is_err() {
            log::error!("scan worker caught a panic");
            self.out.problem(ProblemKind::Panicked);
        }
    }

    fn reload_recent(&mut self) {
        // Stat before reading: a change during the read is seen next time.
        self.xbel_stamp = stamp(&self.xbel);
        let recent = match RecentFiles::load(&self.xbel) {
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
        self.recent = Arc::new(recent);
        self.to_search(SearchMsg::RecentLoaded(Arc::clone(&self.recent)));
    }

    /// The list is unchanged, but files may have been deleted since: filter
    /// again.
    fn recheck_recent(&mut self) {
        let mut r = (*self.recent).clone();
        r.retain_existing();
        if r.items == self.recent.items {
            return;
        }
        self.recent = Arc::new(r);
        self.to_search(SearchMsg::RecentLoaded(Arc::clone(&self.recent)));
    }

    fn reload_settings(&mut self) {
        self.settings_stamp = stamp(&self.settings_index);
        let index = match SettingsIndex::load(&self.settings_index, &self.locales) {
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
        self.path = Arc::new(PathCache::scan(&self.path_var));
        log::debug!(
            "PATH scan: {} names in {} ms",
            self.path.len(),
            started.elapsed().as_millis()
        );
        self.to_search(SearchMsg::PathLoaded(Arc::clone(&self.path)));
    }

    /// The panel opened: one stat per source, reload only what changed. The
    /// recent files are checked for existence every time.
    fn refresh(&mut self) {
        if stamp(&self.xbel) != self.xbel_stamp {
            self.guarded(Scan::reload_recent);
        } else {
            self.guarded(Scan::recheck_recent);
        }
        if stamp(&self.settings_index) != self.settings_stamp {
            self.guarded(Scan::reload_settings);
        }
        if self.path.is_stale() {
            self.guarded(Scan::rescan_path);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::Action;
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
    fn stop(e: Engine) {
        stop_after(e, || {});
    }

    /// Starts the shutdown, then runs `release` (which lets a held worker go)
    /// 100 ms later; returns how long the shutdown took.
    fn stop_after(mut e: Engine, release: impl FnOnce()) -> Duration {
        let (tx, rx) = mpsc::channel();
        let began = Instant::now();
        thread::spawn(move || {
            e.shutdown();
            let _ = tx.send(());
        });
        thread::sleep(Duration::from_millis(100));
        release();
        rx.recv_timeout(WAIT).expect("shutdown hung");
        began.elapsed()
    }

    fn start_hooked(f: &Fixture, hooks: Hooks) -> (Engine, Receiver<Update>) {
        let (tx, rx) = mpsc::channel();
        let e = Engine::start_with(
            f.cfg.clone(),
            SearchOptions::default(),
            Box::new(move |u| {
                let _ = tx.send(u);
            }),
            hooks,
        );
        (e, rx)
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
    fn session_availability_updates_late() {
        let f = fixture();
        let (e, rx) = start(&f);
        let has = |items: &[ResultItem]| items.iter().any(|i| i.id == "session:hibernate");
        e.query(1, "hibernate".into());
        assert!(has(&results(&rx, 1)));
        e.set_session(SessionAvailability {
            hibernate: false,
            switch_user: true,
        });
        e.query(2, "hibernate".into());
        assert!(!has(&results(&rx, 2)));
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
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (e, _rx) = start_hooked(
            &f,
            Hooks {
                load_gate: Some(gate_rx),
                ..Hooks::default()
            },
        );
        // The history cannot load yet: the change is held, then applied and
        // written once the load is released during shutdown.
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        stop_after(e, || {
            let _ = gate_tx.send(());
        });
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
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (e, rx) = start_hooked(
            &f,
            Hooks {
                load_gate: Some(gate_rx),
                ..Hooks::default()
            },
        );
        // A use recorded while the bad file is still loading replaces it.
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        stop_after(e, || {
            let _ = gate_tx.send(());
        });
        // (No Problem arrives: the load ran during shutdown, after which
        // nothing calls back.)
        assert!(rx.try_iter().all(|u| !matches!(u, Update::Results { .. })));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("alpha-viewer"));
    }

    #[test]
    fn oversized_usage_starts_empty() {
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
        stop(e);
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
        let (seen_tx, seen_rx) = mpsc::channel::<()>();
        let e = Engine::start(
            f.cfg.clone(),
            SearchOptions::default(),
            Box::new(move |u| {
                if matches!(u, Update::Results { serial: 1, .. }) {
                    let _ = seen_tx.send(());
                    panic!("callback");
                }
                let _ = tx.send(u);
            }),
        );
        e.set_apps(two_apps());
        e.query(1, "alpha".into());
        seen_rx
            .recv_timeout(WAIT)
            .expect("the first result never came");
        e.query(2, "alpha".into());
        assert!(!results(&rx, 2).is_empty());
        assert!(e.stats().queries_computed >= 2);
        stop(e);
    }

    #[test]
    fn a_failed_usage_stage_still_loads_an_empty_history() {
        let f = fixture();
        let (e, rx) = start_hooked(
            &f,
            Hooks {
                panic_usage: true,
                ..Hooks::default()
            },
        );
        wait(&rx, |u| {
            matches!(u, Update::Problem(ProblemKind::Panicked)).then_some(())
        });
        // The pins stage still ran.
        pins_update(&rx);
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        let took = stop_after(e, || {});
        assert!(
            took < Duration::from_millis(1500),
            "shutdown waited: {took:?}"
        );
        let text = std::fs::read_to_string(f.cfg.state_dir.join(USAGE_FILE)).unwrap();
        assert!(text.contains("alpha-viewer"));
    }

    #[test]
    fn a_failed_write_is_retried() {
        let f = fixture();
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (e, rx) = start_hooked(
            &f,
            Hooks {
                write_fail: Some(Arc::clone(&fail)),
                ..Hooks::default()
            },
        );
        pins_update(&rx);
        let file = f.cfg.state_dir.join(USAGE_FILE);
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        wait(&rx, |u| {
            matches!(u, Update::Problem(ProblemKind::UsageWriteFailed)).then_some(())
        });
        assert!(!file.exists());
        // The panel opening sends the history again.
        fail.store(false, Ordering::SeqCst);
        e.refresh();
        let end = Instant::now() + WAIT;
        while !file.exists() {
            assert!(Instant::now() < end, "the retry never wrote the file");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("alpha-viewer")
        );
        // A failure followed by shutdown is retried at shutdown.
        fail.store(true, Ordering::SeqCst);
        e.record("alpha".into(), "app:alpha-editor.desktop".into());
        wait(&rx, |u| {
            matches!(u, Update::Problem(ProblemKind::UsageWriteFailed)).then_some(())
        });
        fail.store(false, Ordering::SeqCst);
        stop(e);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("alpha-editor") && text.contains("alpha-viewer"));
    }

    #[test]
    fn shutdown_is_bounded_when_a_scan_is_stuck() {
        let f = fixture();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let (e, rx) = start_hooked(
            &f,
            Hooks {
                scan_gate: Some(gate_rx),
                ..Hooks::default()
            },
        );
        // Writes do not wait behind the stuck scan.
        pins_update(&rx);
        e.pin("a.desktop".into());
        assert_eq!(pins_update(&rx), ["a.desktop"]);
        e.record("alpha".into(), "app:alpha-viewer.desktop".into());
        let took = stop_after(e, || {});
        assert!(took < Duration::from_secs(4), "shutdown took {took:?}");
        assert!(f.cfg.config_dir.join(PINS_FILE).exists());
        assert!(f.cfg.state_dir.join(USAGE_FILE).exists());
        drop(gate_tx);
    }

    #[test]
    fn a_thread_that_cannot_start_leaves_an_inert_engine() {
        let f = fixture();
        let (e, rx) = start_hooked(
            &f,
            Hooks {
                fail_scan_spawn: true,
                ..Hooks::default()
            },
        );
        wait(&rx, |u| {
            matches!(u, Update::Problem(ProblemKind::ThreadStart)).then_some(())
        });
        assert!(!e.is_running());
        assert_eq!(e.stats().problems, 1);
        e.query(1, "alpha".into());
        e.pin("a.desktop".into());
        e.refresh();
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        stop(e);
    }

    #[test]
    fn refresh_drops_recent_files_deleted_since() {
        let f = fixture();
        write_xbel(&f, &["one.txt", "two.txt"]);
        let (e, rx) = start(&f);
        recent_files(&rx, 2);
        std::fs::remove_file(f.cfg.home.join("two.txt")).unwrap();
        e.refresh();
        let files = recent_files(&rx, 1);
        assert_eq!(files[0].title, "one.txt");
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
