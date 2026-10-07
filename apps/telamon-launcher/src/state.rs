//! What the panel shows and what a click does, without Qt: the lists QML
//! sees (results, pins, A–Z, recent apps and files) and the lookup from a
//! row's id to its action. `backend.rs` wraps this in QObjects; keeping it
//! here lets it be tested without a display.

use std::collections::HashMap;

use telamon_launcher_core::catalog::{AppEntry, Catalog, app_item, letter_of};
use telamon_launcher_core::result::{Action, Kind, ResultItem};

/// The lists the panel shows, each an `ItemModel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum List {
    Results,
    Pins,
    Apps,
    RecentApps,
    RecentFiles,
}

impl List {
    pub const ALL: [List; 5] = [
        List::Results,
        List::Pins,
        List::Apps,
        List::RecentApps,
        List::RecentFiles,
    ];

    pub fn from_index(i: i32) -> Option<List> {
        usize::try_from(i)
            .ok()
            .and_then(|i| List::ALL.get(i).copied())
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// The name QML gets in the `kind` role.
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::App => "app",
        Kind::Setting => "setting",
        Kind::File => "file",
        Kind::Folder => "folder",
        Kind::Calculator => "calculator",
        Kind::Command => "command",
        Kind::Session => "session",
        Kind::Web => "web",
        Kind::Runner => "runner",
    }
}

/// The A–Z section of a row (apps only; empty for anything else).
pub fn section_of(item: &ResultItem) -> String {
    if item.kind == Kind::App {
        letter_of(&item.title).to_string()
    } else {
        String::new()
    }
}

/// The panel's state, on the GUI thread.
#[derive(Default)]
pub struct State {
    catalog: Catalog,
    /// `preferred://browser` and the like, resolved by C++ (KApplicationTrader).
    preferred: HashMap<String, String>,
    /// The pinned ids as stored (desktop ids or `preferred://` names).
    pin_ids: Vec<String>,
    lists: [Vec<ResultItem>; 5],
    /// The query whose answer the results list holds, and its text.
    results_serial: u64,
    results_query: String,
}

impl State {
    pub fn list(&self, list: List) -> &[ResultItem] {
        &self.lists[list.index()]
    }

    /// The installed apps, A–Z.
    pub fn apps(&self) -> &[AppEntry] {
        self.catalog.a_to_z()
    }

    /// A new catalogue: the A–Z list is rebuilt and the pins re-resolved.
    /// Returns the lists that changed.
    pub fn set_apps(
        &mut self,
        apps: Vec<AppEntry>,
        preferred: HashMap<String, String>,
    ) -> Vec<List> {
        self.catalog = Catalog::new(apps);
        self.preferred = preferred;
        self.lists[List::Apps.index()] = self
            .catalog
            .a_to_z()
            .iter()
            .map(|a| app_item(a, 0.0))
            .collect();
        self.resolve_pins();
        vec![List::Apps, List::Pins]
    }

    /// The answer to query `serial`, whose text was `query`.
    pub fn set_results(&mut self, serial: u64, query: String, items: Vec<ResultItem>) {
        self.lists[List::Results.index()] = items;
        self.results_serial = serial;
        self.results_query = query;
    }

    /// The query the results list answers.
    pub fn results_serial(&self) -> u64 {
        self.results_serial
    }

    /// The text of that query, recorded with what runs from it.
    pub fn results_query(&self) -> &str {
        &self.results_query
    }

    pub fn set_recent(&mut self, apps: Vec<ResultItem>, files: Vec<ResultItem>) {
        self.lists[List::RecentApps.index()] = apps;
        self.lists[List::RecentFiles.index()] = files;
    }

    pub fn set_pins(&mut self, ids: Vec<String>) {
        self.pin_ids = ids;
        self.resolve_pins();
    }

    /// The desktop id a pin stands for, if that app is installed.
    fn resolve(&self, pin: &str) -> Option<&AppEntry> {
        let id = self.preferred.get(pin).map_or(pin, String::as_str);
        self.catalog.get(id)
    }

    /// Pins of apps that are not installed are hidden but kept in the
    /// list, so a reinstall brings them back (DESIGN "Failure modes").
    fn resolve_pins(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let items: Vec<ResultItem> = self
            .pin_ids
            .iter()
            .filter_map(|p| self.resolve(p))
            .filter(|a| seen.insert(a.desktop_id.clone()))
            .map(|a| app_item(a, 0.0))
            .collect();
        self.lists[List::Pins.index()] = items;
    }

    /// The stored pins behind a row (its app, whichever action), first
    /// as stored: the `preferred://` name when the pin came from one. More
    /// than one when the app was pinned both ways.
    pub fn pin_keys(&self, row_id: &str) -> Vec<String> {
        let Some(desktop_id) = row_id
            .strip_prefix("app:")
            .and_then(|id| id.split('#').next())
        else {
            return Vec::new();
        };
        self.pin_ids
            .iter()
            .filter(|p| self.resolve(p).is_some_and(|a| a.desktop_id == desktop_id))
            .cloned()
            .collect()
    }

    /// The first stored pin behind a row, for moves.
    pub fn pin_key(&self, row_id: &str) -> Option<String> {
        self.pin_keys(row_id).into_iter().next()
    }

    /// Whether the app behind a row id is pinned.
    pub fn is_pinned(&self, row_id: &str) -> bool {
        self.pin_key(row_id).is_some()
    }

    /// The desktop id to pin for a row id: apps only, installed only.
    pub fn pinnable(&self, row_id: &str) -> Option<String> {
        let desktop_id = row_id.strip_prefix("app:")?;
        let desktop_id = desktop_id.split('#').next()?;
        self.catalog.get(desktop_id).map(|a| a.desktop_id.clone())
    }

    /// The index in the stored list where a shown pin row sits, for moves:
    /// shown rows skip uninstalled pins, the stored list does not.
    pub fn stored_index(&self, shown: usize) -> usize {
        let shown_ids: Vec<&str> = self
            .list(List::Pins)
            .iter()
            .map(|i| i.id.as_str())
            .collect();
        let Some(target) = shown_ids.get(shown) else {
            return self.pin_ids.len();
        };
        self.pin_key(target)
            .and_then(|k| self.pin_ids.iter().position(|p| *p == k))
            .unwrap_or(self.pin_ids.len())
    }

    /// The row with this id in `list`, if it is still shown.
    pub fn find(&self, list: List, id: &str) -> Option<&ResultItem> {
        self.list(list).iter().find(|i| i.id == id)
    }

    /// The URI behind a shown file row (results or recent files).
    pub fn file_uri_of(&self, row_id: &str) -> Option<&str> {
        [List::Results, List::RecentFiles]
            .into_iter()
            .find_map(|list| self.find(list, row_id))
            .and_then(|item| match &item.action {
                Action::OpenFile { uri } => Some(uri.as_str()),
                _ => None,
            })
    }

    /// The app behind a row, for its context menu.
    pub fn app_of(&self, row_id: &str) -> Option<&AppEntry> {
        let desktop_id = row_id.strip_prefix("app:")?.split('#').next()?;
        self.catalog.get(desktop_id)
    }

    /// The index of the first app under `letter` in the A–Z list, or -1.
    pub fn letter_index(&self, letter: &str) -> i32 {
        let Some(l) = letter.chars().next() else {
            return -1;
        };
        let l = letter_of(l.encode_utf8(&mut [0; 4]));
        self.list(List::Apps)
            .iter()
            .position(|i| letter_of(&i.title) == l)
            .map_or(-1, |p| i32::try_from(p).unwrap_or(-1))
    }

    /// The letters that have apps, in A–Z order ('#' first).
    pub fn letters(&self) -> Vec<String> {
        self.catalog
            .letters()
            .into_iter()
            .map(String::from)
            .collect()
    }
}

/// What the C++ side carries out for an action: the signal to emit and its
/// arguments. One variant per signal of `Backend`.
#[derive(Clone, PartialEq)]
pub enum Exec {
    LaunchApp {
        desktop_id: String,
        action: String,
    },
    OpenSettings {
        link: String,
    },
    OpenFile {
        uri: String,
    },
    OpenWeb {
        url: String,
    },
    Copy {
        text: String,
    },
    Run {
        executable: String,
        args: Vec<String>,
    },
    RunInTerminal {
        line: String,
    },
    Session {
        name: &'static str,
    },
    Runner {
        runner_id: String,
        match_id: String,
    },
}

impl std::fmt::Debug for Exec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redacted like Action: never a path, line or id in a log.
        let name = match self {
            Exec::LaunchApp { .. } => "LaunchApp",
            Exec::OpenSettings { .. } => "OpenSettings",
            Exec::OpenFile { .. } => "OpenFile",
            Exec::OpenWeb { .. } => "OpenWeb",
            Exec::Copy { .. } => "Copy",
            Exec::Run { .. } => "Run",
            Exec::RunInTerminal { .. } => "RunInTerminal",
            Exec::Session { name } => return write!(f, "Exec::Session({name})"),
            Exec::Runner { .. } => "Runner",
        };
        write!(f, "Exec::{name}")
    }
}

/// The session commands' names as C++ knows them.
pub fn session_name(a: telamon_launcher_core::result::SessionAction) -> &'static str {
    use telamon_launcher_core::result::SessionAction as S;
    match a {
        S::Lock => "lock",
        S::Sleep => "sleep",
        S::Hibernate => "hibernate",
        S::SwitchUser => "switch-user",
        S::LogOut => "log-out",
        S::Restart => "restart",
        S::ShutDown => "shut-down",
    }
}

pub fn exec_for(action: &Action) -> Exec {
    match action {
        Action::LaunchApp { desktop_id, action } => Exec::LaunchApp {
            desktop_id: desktop_id.clone(),
            action: action.clone().unwrap_or_default(),
        },
        Action::OpenSettings { link } => Exec::OpenSettings { link: link.clone() },
        Action::OpenFile { uri } => Exec::OpenFile { uri: uri.clone() },
        Action::OpenWeb { url } => Exec::OpenWeb { url: url.clone() },
        Action::Copy { text } => Exec::Copy { text: text.clone() },
        Action::Run { executable, args } => Exec::Run {
            executable: executable.clone(),
            args: args.clone(),
        },
        Action::RunInTerminal { line } => Exec::RunInTerminal { line: line.clone() },
        Action::Session(a) => Exec::Session {
            name: session_name(*a),
        },
        Action::Runner {
            runner_id,
            match_id,
        } => Exec::Runner {
            runner_id: runner_id.clone(),
            match_id: match_id.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> AppEntry {
        AppEntry {
            desktop_id: id.into(),
            name: name.into(),
            icon: "x".into(),
            ..AppEntry::default()
        }
    }

    fn state() -> State {
        let mut s = State::default();
        let mut preferred = HashMap::new();
        preferred.insert(
            "preferred://browser".to_owned(),
            "firefox.desktop".to_owned(),
        );
        s.set_apps(
            vec![
                app("firefox.desktop", "Firefox"),
                app("org.kde.dolphin.desktop", "Dolphin"),
                app("7zip.desktop", "7-Zip"),
            ],
            preferred,
        );
        s
    }

    #[test]
    fn a_to_z_and_letters() {
        let s = state();
        let titles: Vec<&str> = s
            .list(List::Apps)
            .iter()
            .map(|i| i.title.as_str())
            .collect();
        assert_eq!(titles, ["7-Zip", "Dolphin", "Firefox"]);
        assert_eq!(s.letters(), ["#", "D", "F"]);
        assert_eq!(s.letter_index("f"), 2);
        assert_eq!(s.letter_index("#"), 0);
        assert_eq!(s.letter_index("Q"), -1);
        assert_eq!(s.letter_index(""), -1);
        assert_eq!(section_of(&s.list(List::Apps)[0]), "#");
    }

    #[test]
    fn pins_resolve_preferred_and_hide_missing() {
        let mut s = state();
        s.set_pins(vec![
            "preferred://browser".into(),
            "gone.desktop".into(),
            "org.kde.dolphin.desktop".into(),
            "firefox.desktop".into(),
        ]);
        let ids: Vec<&str> = s.list(List::Pins).iter().map(|i| i.id.as_str()).collect();
        // Firefox shows once, through its first pin.
        assert_eq!(ids, ["app:firefox.desktop", "app:org.kde.dolphin.desktop"]);
        assert_eq!(
            s.pin_key("app:firefox.desktop").as_deref(),
            Some("preferred://browser")
        );
        assert!(s.is_pinned("app:org.kde.dolphin.desktop"));
        assert!(!s.is_pinned("app:7zip.desktop"));
        // An action's row is its app's; both ways Firefox was pinned unpin.
        assert!(s.is_pinned("app:firefox.desktop#private"));
        assert_eq!(
            s.pin_keys("app:firefox.desktop#private"),
            ["preferred://browser", "firefox.desktop"]
        );
        // Shown row 1 (Dolphin) is stored at 2, past the missing app.
        assert_eq!(s.stored_index(1), 2);
        assert_eq!(s.stored_index(9), 4);
    }

    #[test]
    fn pinnable_only_installed_apps() {
        let s = state();
        assert_eq!(
            s.pinnable("app:firefox.desktop#private").as_deref(),
            Some("firefox.desktop")
        );
        assert_eq!(s.pinnable("app:gone.desktop"), None);
        assert_eq!(s.pinnable("file:///etc/passwd"), None);
        assert_eq!(s.pinnable("calc"), None);
    }

    #[test]
    fn list_indexes_round_trip() {
        for l in List::ALL {
            assert_eq!(List::from_index(l.index() as i32), Some(l));
        }
        assert_eq!(List::from_index(-1), None);
        assert_eq!(List::from_index(5), None);
    }

    #[test]
    fn file_uri_only_for_shown_file_rows() {
        let mut s = state();
        let file = ResultItem {
            id: "file:file:///home/u/a.txt".into(),
            kind: Kind::File,
            title: "a.txt".into(),
            subtitle: String::new(),
            icon: String::new(),
            score: 1.0,
            action: Action::OpenFile {
                uri: "file:///home/u/a.txt".into(),
            },
        };
        s.set_recent(Vec::new(), vec![file]);
        assert_eq!(
            s.file_uri_of("file:file:///home/u/a.txt"),
            Some("file:///home/u/a.txt")
        );
        // Not shown, or not a file: nothing.
        assert_eq!(s.file_uri_of("file:file:///etc/passwd"), None);
        assert_eq!(s.file_uri_of("app:firefox.desktop"), None);
    }

    #[test]
    fn exec_debug_is_redacted() {
        let e = exec_for(&Action::Run {
            executable: "/usr/bin/secret".into(),
            args: vec!["token".into()],
        });
        let d = format!("{e:?}");
        assert!(!d.contains("secret") && !d.contains("token"), "{d}");
    }
}
