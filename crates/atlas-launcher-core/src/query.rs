//! The query engine (docs/DESIGN.md, "Search"): the instant phase over the
//! core's own providers, the merge of late results under the stability
//! rules, and the diff that updates the list model without a reset.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::calc::{self, CalcKind};
use crate::catalog::Catalog;
use crate::commands::{PathCache, SessionCommands};
use crate::recent::RecentFiles;
use crate::result::{Kind, ResultItem};
use crate::settings_index::SettingsIndex;
use crate::text::Query;
use crate::usage::UsageStore;
use crate::web::{Engine, web_row};

/// The most rows a query shows, the web row included.
pub const MAX_RESULTS: usize = 50;

/// What `launcher.conf` and `krunnerrc` turn on (DESIGN.md, "Interfaces").
#[derive(Clone, Debug, PartialEq)]
pub struct SearchOptions {
    /// `servicesEnabled`
    pub apps: bool,
    /// `krunner_systemsettingsEnabled`
    pub settings: bool,
    /// `calculatorEnabled`
    pub calculator: bool,
    /// `unitconverterEnabled`
    pub units: bool,
    /// `shellEnabled`: session commands and `PATH` executables.
    pub commands: bool,
    /// `FileSearch`: recent files now, Explorer's hits late.
    pub files: bool,
    /// `WebSearch` and `WebSearchEngine`; None hides the row.
    pub web: Option<Engine>,
    /// `LearnFromUse`: false ignores the history (it is not deleted).
    pub learn: bool,
    /// The locale's decimal separator.
    pub decimal: char,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            apps: true,
            settings: true,
            calculator: true,
            units: true,
            commands: true,
            files: true,
            web: Some(Engine::DuckDuckGo),
            learn: true,
            decimal: '.',
        }
    }
}

/// The instant providers' data: immutable snapshots, replaced whole when
/// their source changes (KSycoca, the xbel's mtime, `PATH`'s mtimes).
#[derive(Clone)]
pub struct Sources {
    pub catalog: Arc<Catalog>,
    pub settings: Option<Arc<SettingsIndex>>,
    pub recent: Arc<RecentFiles>,
    pub path: Arc<PathCache>,
    pub session: Arc<SessionCommands>,
    /// Shown as "~" in file rows.
    pub home: PathBuf,
}

/// What ranking needs besides the rows: the history, the options, the query
/// and the time.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub usage: &'a UsageStore,
    pub opts: &'a SearchOptions,
    pub query: &'a Query,
    /// Unix seconds, for the frecency decay.
    pub now: i64,
}

/// Adds the learned boost, keeps the best row per id and sorts best first.
/// The web row is left out; callers append it last.
fn rank(items: &mut Vec<ResultItem>, cx: &Context) {
    items.retain(|it| it.kind != Kind::Web);
    if cx.opts.learn {
        for it in items.iter_mut() {
            it.score += cx.usage.boost(cx.query, &it.id, cx.now);
        }
    }
    items.sort_by(ResultItem::rank_cmp);
    let mut seen = HashSet::with_capacity(items.len());
    items.retain(|it| seen.insert(it.id.clone()));
}

/// The instant phase: every core provider the options allow, learned
/// boosts applied, best first, capped, and the web row last.
pub fn instant(src: &Sources, cx: &Context) -> Vec<ResultItem> {
    let (opts, q) = (cx.opts, cx.query);
    if q.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(64);
    if (opts.calculator || opts.units)
        && let Some(c) = calc::evaluate(&q.raw, opts.decimal)
        && match c.kind {
            CalcKind::Math => opts.calculator,
            CalcKind::Conversion => opts.units,
        }
    {
        out.push(c.to_result());
    }
    if opts.apps {
        out.extend(src.catalog.search(q));
    }
    if opts.settings
        && let Some(settings) = &src.settings
    {
        out.extend(settings.search(q));
    }
    if opts.commands {
        out.extend(src.session.search(q));
        out.extend(src.path.search(q));
    }
    if opts.files {
        out.extend(src.recent.search(q, &src.home));
    }
    rank(&mut out, cx);
    let web = opts.web.and_then(|engine| web_row(engine, q));
    out.truncate(MAX_RESULTS - usize::from(web.is_some()));
    out.extend(web);
    out
}

/// Where a row came from, so a later batch from the same source replaces
/// that source's earlier rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Instant,
    /// KRunner plugins (RunnerManager sends its whole current list each time).
    Runner,
    /// Explorer's index.
    Files,
}

/// The rows of one query, as shown. Late batches merge in under the
/// stability rules: the top row and the selected row never move.
#[derive(Clone, Debug)]
pub struct ResultSet {
    serial: u64,
    rows: Vec<(Source, ResultItem)>,
    web: Option<ResultItem>,
}

impl ResultSet {
    /// The set for query `serial`, from its instant phase (the web row, if
    /// any, is kept apart and always shown last).
    pub fn new(serial: u64, instant: Vec<ResultItem>) -> ResultSet {
        let mut web = None;
        let mut rows = Vec::with_capacity(instant.len());
        for it in instant {
            if it.kind == Kind::Web {
                web = Some(it);
            } else {
                rows.push((Source::Instant, it));
            }
        }
        ResultSet { serial, rows, web }
    }

    pub fn serial(&self) -> u64 {
        self.serial
    }

    pub fn len(&self) -> usize {
        self.rows.len() + usize::from(self.web.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The rows in order, web row last.
    pub fn items(&self) -> Vec<ResultItem> {
        self.rows
            .iter()
            .map(|(_, it)| it.clone())
            .chain(self.web.clone())
            .collect()
    }

    pub fn get(&self, index: usize) -> Option<&ResultItem> {
        if index < self.rows.len() {
            Some(&self.rows[index].1)
        } else if index == self.rows.len() {
            self.web.as_ref()
        } else {
            None
        }
    }

    /// Merges a late batch from `source`, replacing that source's earlier
    /// rows. The top row and the row with id `selected` keep their places;
    /// everything else is re-ranked around them. A late row whose id another
    /// source already shows is dropped (the instant row wins).
    pub fn merge(
        &mut self,
        source: Source,
        late: Vec<ResultItem>,
        selected: Option<&str>,
        cx: &Context,
    ) {
        debug_assert!(
            source != Source::Instant,
            "the instant phase makes a new set"
        );
        let mut late = late;
        rank(&mut late, cx);

        // Frozen rows: the top hit, and the selected row, by position.
        let selected_pos = selected.and_then(|id| self.rows.iter().position(|(_, it)| it.id == id));
        let mut frozen: Vec<(usize, (Source, ResultItem))> = Vec::with_capacity(2);
        let mut rest: Vec<(Source, ResultItem)> = Vec::with_capacity(self.rows.len() + late.len());
        for (i, row) in std::mem::take(&mut self.rows).into_iter().enumerate() {
            if i == 0 || Some(i) == selected_pos {
                frozen.push((i, row));
            } else if row.0 != source {
                rest.push(row);
            }
        }

        let shown: HashSet<&str> = frozen
            .iter()
            .map(|(_, (_, it))| it.id.as_str())
            .chain(rest.iter().map(|(_, it)| it.id.as_str()))
            .collect();
        let fresh: Vec<(Source, ResultItem)> = late
            .into_iter()
            .filter(|it| !shown.contains(it.id.as_str()))
            .map(|it| (source, it))
            .collect();
        rest.extend(fresh);
        rest.sort_by(|a, b| ResultItem::rank_cmp(&a.1, &b.1));

        for (pos, row) in frozen {
            let at = pos.min(rest.len());
            rest.insert(at, row);
        }
        rest.truncate(MAX_RESULTS - usize::from(self.web.is_some()));
        self.rows = rest;
    }
}

/// One step that turns the shown list into the next one. Applied in order;
/// indexes refer to the list as it is after the previous steps.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelOp {
    Remove {
        index: usize,
    },
    Insert {
        index: usize,
        item: ResultItem,
    },
    /// The row at `from` moves up to `to` (`to < from` always).
    Move {
        from: usize,
        to: usize,
    },
    /// Same id, new content.
    Update {
        index: usize,
        item: ResultItem,
    },
}

/// The steps from `old` to `new`, both with unique ids: removals first
/// (from the end), then each position of `new` in order is matched by
/// keeping, moving up or inserting a row.
pub fn diff(old: &[ResultItem], new: &[ResultItem]) -> Vec<ModelOp> {
    let mut ops = Vec::new();
    let new_ids: HashSet<&str> = new.iter().map(|it| it.id.as_str()).collect();
    let old_by_id: HashMap<&str, &ResultItem> = old.iter().map(|it| (it.id.as_str(), it)).collect();
    let mut cur: Vec<&str> = old.iter().map(|it| it.id.as_str()).collect();

    for i in (0..cur.len()).rev() {
        if !new_ids.contains(cur[i]) {
            ops.push(ModelOp::Remove { index: i });
            cur.remove(i);
        }
    }
    for (i, item) in new.iter().enumerate() {
        let id = item.id.as_str();
        if cur.get(i) != Some(&id) {
            if let Some(off) = cur
                .get(i + 1..)
                .and_then(|tail| tail.iter().position(|c| *c == id))
            {
                let from = i + 1 + off;
                ops.push(ModelOp::Move { from, to: i });
                cur.remove(from);
                cur.insert(i, id);
            } else {
                ops.push(ModelOp::Insert {
                    index: i,
                    item: item.clone(),
                });
                cur.insert(i, id);
                continue;
            }
        }
        if old_by_id.get(id).is_some_and(|before| *before != item) {
            ops.push(ModelOp::Update {
                index: i,
                item: item.clone(),
            });
        }
    }
    debug_assert_eq!(cur.len(), new.len());
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::AppEntry;
    use crate::commands::SessionAvailability;
    use crate::result::{Action, prior};

    const NOW: i64 = 1_800_000_000;

    fn app(id: &str, name: &str) -> AppEntry {
        AppEntry {
            desktop_id: id.into(),
            name: name.into(),
            icon: "icon".into(),
            ..Default::default()
        }
    }

    fn sources() -> Sources {
        let settings = SettingsIndex::parse(
            br#"{"version":1,"app":"net.eterneon.atlas.settings","entries":[
                {"id":"displays","kind":"page","page":"displays","title":{"C":"Displays"},"section":{"C":"Devices"},"icon":"preferences-desktop-display","keywords":{"C":["monitor","screen"]},"link":"displays"},
                {"id":"firewall","kind":"page","page":"firewall","title":{"C":"Firewall"},"section":{"C":"Network"},"icon":"security-high","keywords":{"C":[]},"link":"firewall"}]}"#,
            &["C".to_owned()],
        )
        .unwrap();
        let recent = RecentFiles::parse(
            br#"<xbel version="1.0"><bookmark href="file:///home/u/firefox-notes.txt" added="2026-01-01T00:00:00Z" modified="2026-01-01T00:00:00Z" visited="2026-01-01T00:00:00Z"/></xbel>"#,
        )
        .unwrap();
        Sources {
            catalog: Arc::new(Catalog::new(vec![
                app("org.mozilla.firefox.desktop", "Firefox"),
                app("org.kde.dolphin.desktop", "Dolphin"),
                app("org.kde.kate.desktop", "Kate"),
                app("org.kde.kcalc.desktop", "KCalc"),
            ])),
            settings: Some(Arc::new(settings)),
            recent: Arc::new(recent),
            path: Arc::new(PathCache::scan("")),
            session: Arc::new(SessionCommands::new(SessionAvailability::default())),
            home: PathBuf::from("/home/u"),
        }
    }

    fn ids(items: &[ResultItem]) -> Vec<&str> {
        items.iter().map(|it| it.id.as_str()).collect()
    }

    fn q(s: &str) -> Query {
        Query::new(s, 1)
    }

    fn run(src: &Sources, usage: &UsageStore, opts: &SearchOptions, text: &str) -> Vec<ResultItem> {
        let query = q(text);
        instant(
            src,
            &Context {
                usage,
                opts,
                query: &query,
                now: NOW,
            },
        )
    }

    #[test]
    fn instant_composes_ranks_and_ends_with_web() {
        let src = sources();
        let usage = UsageStore::default();
        let opts = SearchOptions::default();

        let r = run(&src, &usage, &opts, "fire");
        assert_eq!(r[0].id, "app:org.mozilla.firefox.desktop");
        assert!(ids(&r).contains(&"setting:firewall"));
        assert!(ids(&r).contains(&"file:file:///home/u/firefox-notes.txt"));
        assert_eq!(r.last().unwrap().kind, Kind::Web);
        // App beats setting beats file at equal match quality.
        let pos = |id: &str| ids(&r).iter().position(|x| *x == id).unwrap();
        assert!(pos("setting:firewall") < pos("file:file:///home/u/firefox-notes.txt"));

        // The calculator tops everything when the text is an expression.
        let r = run(&src, &usage, &opts, "2+3");
        assert_eq!(r[0].id, "calc");
        assert_eq!(r[0].score, prior(Kind::Calculator));

        // Nothing for an empty query, not even the web row.
        assert!(run(&src, &usage, &opts, "   ").is_empty());
    }

    #[test]
    fn options_turn_providers_off() {
        let src = sources();
        let usage = UsageStore::default();
        let off = SearchOptions {
            apps: false,
            settings: false,
            files: false,
            web: None,
            ..SearchOptions::default()
        };
        assert!(run(&src, &usage, &off, "fire").is_empty());

        let no_calc = SearchOptions {
            calculator: false,
            ..SearchOptions::default()
        };
        assert!(!ids(&run(&src, &usage, &no_calc, "2+3")).contains(&"calc"));
        // Units stay on when only the calculator is off.
        assert!(ids(&run(&src, &usage, &no_calc, "5 km in mi")).contains(&"calc"));
        let no_units = SearchOptions {
            units: false,
            ..SearchOptions::default()
        };
        assert!(!ids(&run(&src, &usage, &no_units, "5 km in mi")).contains(&"calc"));
        assert!(ids(&run(&src, &usage, &no_units, "2+3")).contains(&"calc"));
    }

    #[test]
    fn learning_reorders_and_can_be_ignored() {
        let src = sources();
        let mut usage = UsageStore::default();
        let opts = SearchOptions::default();
        // "k" matches Kate and KCalc; KCalc has been picked for "k" often.
        let before = run(&src, &usage, &opts, "k");
        assert_eq!(before[0].id, "app:org.kde.kate.desktop");
        for _ in 0..5 {
            usage.record(&q("k"), "app:org.kde.kcalc.desktop", NOW - 60);
        }
        let after = run(&src, &usage, &opts, "k");
        assert_eq!(after[0].id, "app:org.kde.kcalc.desktop");

        let ignore = SearchOptions {
            learn: false,
            ..SearchOptions::default()
        };
        assert_eq!(
            run(&src, &usage, &ignore, "k")[0].id,
            "app:org.kde.kate.desktop"
        );
    }

    fn row(id: &str, score: f32) -> ResultItem {
        ResultItem {
            id: id.into(),
            kind: Kind::Runner,
            title: id.into(),
            subtitle: String::new(),
            icon: "x".into(),
            score,
            action: Action::Copy {
                text: String::new(),
            },
        }
    }

    fn set(rows: &[(&str, f32)], web: bool) -> ResultSet {
        let mut items: Vec<ResultItem> = rows.iter().map(|(id, s)| row(id, *s)).collect();
        if web {
            items.push(ResultItem {
                kind: Kind::Web,
                ..row("web", 0.0)
            });
        }
        ResultSet::new(7, items)
    }

    fn merge(s: &mut ResultSet, source: Source, late: &[(&str, f32)], selected: Option<&str>) {
        let late = late.iter().map(|(id, sc)| row(id, *sc)).collect();
        let (usage, opts, query) = (UsageStore::default(), SearchOptions::default(), q("x"));
        let cx = Context {
            usage: &usage,
            opts: &opts,
            query: &query,
            now: NOW,
        };
        s.merge(source, late, selected, &cx);
    }

    #[test]
    fn late_rows_never_displace_top_or_selection() {
        let mut s = set(&[("a", 0.9), ("b", 0.6), ("c", 0.5)], true);
        // A stronger late row lands right under the top, not above it.
        merge(&mut s, Source::Runner, &[("r1", 0.95)], None);
        assert_eq!(ids(&s.items()), ["a", "r1", "b", "c", "web"]);

        // With "c" selected (index 3), a new batch keeps it at index 3.
        merge(
            &mut s,
            Source::Files,
            &[("f1", 0.99), ("f2", 0.98)],
            Some("c"),
        );
        let items = s.items();
        assert_eq!(items[0].id, "a");
        assert_eq!(items[3].id, "c");
        assert_eq!(items.last().unwrap().id, "web");
        assert_eq!(items.len(), 7);
    }

    #[test]
    fn a_source_replaces_its_own_rows_and_dedupes() {
        let mut s = set(&[("a", 0.9), ("file:x", 0.5)], false);
        merge(&mut s, Source::Runner, &[("r1", 0.4), ("r2", 0.3)], None);
        assert_eq!(ids(&s.items()), ["a", "file:x", "r1", "r2"]);
        // The runners' next list replaces their earlier one.
        merge(&mut s, Source::Runner, &[("r3", 0.45)], None);
        assert_eq!(ids(&s.items()), ["a", "file:x", "r3"]);
        // A late row the instant phase already shows is dropped.
        merge(
            &mut s,
            Source::Files,
            &[("file:x", 0.99), ("file:y", 0.1)],
            None,
        );
        assert_eq!(ids(&s.items()), ["a", "file:x", "r3", "file:y"]);
        // A selected late row stays even when its source drops it.
        merge(&mut s, Source::Runner, &[], Some("r3"));
        assert_eq!(ids(&s.items()), ["a", "file:x", "r3", "file:y"]);
        merge(&mut s, Source::Runner, &[], None);
        assert_eq!(ids(&s.items()), ["a", "file:x", "file:y"]);
    }

    #[test]
    fn empty_instant_lets_late_choose_the_top_and_caps_hold() {
        let mut s = set(&[], true);
        let many: Vec<(String, f32)> = (0..80)
            .map(|i| (format!("r{i:02}"), 1.0 - i as f32 / 100.0))
            .collect();
        let many: Vec<(&str, f32)> = many.iter().map(|(id, sc)| (id.as_str(), *sc)).collect();
        merge(&mut s, Source::Runner, &many, None);
        let items = s.items();
        assert_eq!(items.len(), MAX_RESULTS);
        assert_eq!(items[0].id, "r00");
        assert_eq!(items.last().unwrap().id, "web");
        assert_eq!(s.get(MAX_RESULTS - 1).unwrap().id, "web");
        assert!(s.get(MAX_RESULTS).is_none());
    }

    fn apply(old: &[ResultItem], ops: &[ModelOp]) -> Vec<ResultItem> {
        let mut cur = old.to_vec();
        for op in ops {
            match op {
                ModelOp::Remove { index } => {
                    cur.remove(*index);
                }
                ModelOp::Insert { index, item } => cur.insert(*index, item.clone()),
                ModelOp::Move { from, to } => {
                    assert!(to < from);
                    let it = cur.remove(*from);
                    cur.insert(*to, it);
                }
                ModelOp::Update { index, item } => {
                    assert_eq!(cur[*index].id, item.id);
                    cur[*index] = item.clone();
                }
            }
        }
        cur
    }

    #[test]
    fn diff_reproduces_the_new_list() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2000 {
            let pick = |n: u64, salt: u64| -> Vec<ResultItem> {
                let mut ids: Vec<u64> = (0..12).filter(|i| (n >> (i + salt)) & 1 == 1).collect();
                // Shuffle a bit, deterministically.
                let len = ids.len().max(1);
                ids.rotate_left((n % 5) as usize % len);
                ids.into_iter()
                    .map(|i| row(&format!("i{i}"), ((n >> (i * 2)) & 3) as f32))
                    .collect()
            };
            let (a, b) = (next(), next());
            let old = pick(a, 0);
            let new = pick(b, 1);
            let ops = diff(&old, &new);
            assert_eq!(apply(&old, &ops), new);
        }
        // Unchanged lists need nothing.
        let same = vec![row("a", 1.0), row("b", 0.5)];
        assert!(diff(&same, &same).is_empty());
        // A pure insertion below keeps the rows above untouched.
        let more = vec![row("a", 1.0), row("c", 0.7), row("b", 0.5)];
        assert_eq!(
            diff(&same, &more),
            vec![ModelOp::Insert {
                index: 1,
                item: row("c", 0.7)
            }]
        );
    }
}
