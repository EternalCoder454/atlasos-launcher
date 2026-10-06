//! The installed apps, handed over from KService as plain data, validated
//! once, prepared for matching once, and searched on every keystroke.

use std::collections::HashMap;
use std::ops::Range;

use crate::result::{Action, Kind, ResultItem, prior};
use crate::text::{
    Prepared, Query, clean_display, clean_display_max, fold, score_fields, valid_icon,
};

pub const MAX_APPS: usize = 10_000;
pub const MAX_KEYWORDS: usize = 64;
pub const MAX_ACTIONS: usize = 32;
const MAX_CATEGORIES: usize = 64;
/// Longest id of a desktop file or a flatpak app.
const MAX_ID_BYTES: usize = 255;
/// Desktop actions score a little below the app itself.
const ACTION_WEIGHT: f32 = 0.9;
/// Shown for an app whose icon is missing or fails the icon check.
const FALLBACK_ICON: &str = "application-x-executable";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppActionEntry {
    pub id: String,
    pub name: String,
    pub icon: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AppEntry {
    /// `org.kde.dolphin.desktop`
    pub desktop_id: String,
    pub name: String,
    pub generic_name: String,
    pub comment: String,
    pub keywords: Vec<String>,
    pub icon: String,
    /// The binary's file name, e.g. `dolphin`.
    pub exec_name: String,
    pub categories: Vec<String>,
    pub actions: Vec<AppActionEntry>,
    pub flatpak_id: Option<String>,
    /// The desktop file's mtime, for "recently installed".
    pub installed_unix: i64,
}

/// A desktop file id: `[A-Za-z0-9._-]{1,255}` ending in `.desktop`.
pub fn valid_desktop_id(id: &str) -> bool {
    id.len() <= MAX_ID_BYTES
        && id.len() > ".desktop".len()
        && id.ends_with(".desktop")
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Reverse-DNS: `[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+){2,}`, at most 255 bytes.
pub fn valid_flatpak_id(id: &str) -> bool {
    id.len() <= MAX_ID_BYTES
        && id.split('.').count() >= 3
        && id.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        })
}

fn valid_action_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// One entry made safe: ids checked, text cleaned and capped. `None` when the
/// desktop id is not valid.
fn sanitize(mut e: AppEntry) -> Option<AppEntry> {
    if !valid_desktop_id(&e.desktop_id) {
        return None;
    }
    e.name = clean_display(&e.name);
    if e.name.is_empty() {
        // Show something rather than hide an app that can still be launched.
        e.name = clean_display(e.desktop_id.trim_end_matches(".desktop"));
    }
    e.generic_name = clean_display(&e.generic_name);
    e.comment = clean_display(&e.comment);
    // Icons are names or absolute paths: checked, never cleaned (cleaning
    // would mangle a path).
    if !valid_icon(&e.icon) {
        e.icon = FALLBACK_ICON.to_owned();
    }
    e.exec_name = clean_display_max(&e.exec_name, 255);
    e.keywords = e
        .keywords
        .iter()
        .map(|k| clean_display_max(k, 64))
        .filter(|k| !k.is_empty())
        .take(MAX_KEYWORDS)
        .collect();
    e.categories = e
        .categories
        .iter()
        .map(|c| clean_display_max(c, 64))
        .filter(|c| !c.is_empty())
        .take(MAX_CATEGORIES)
        .collect();
    e.actions = e
        .actions
        .into_iter()
        .filter(|a| valid_action_id(&a.id))
        .map(|a| AppActionEntry {
            name: clean_display(&a.name),
            icon: if valid_icon(&a.icon) {
                a.icon
            } else {
                String::new()
            },
            id: a.id,
        })
        .filter(|a| !a.name.is_empty())
        .take(MAX_ACTIONS)
        .collect();
    e.flatpak_id = e.flatpak_id.filter(|f| valid_flatpak_id(f));
    Some(e)
}

/// What matching needs of one app, prepared once.
struct Prep {
    name: Prepared,
    /// Generic name, exec name, then the keywords.
    secondary: Vec<Prepared>,
    comment: Prepared,
    actions: Vec<Prepared>,
}

#[derive(Default)]
pub struct Catalog {
    /// In A–Z order: non-letters ("#") first, then by folded name, then id.
    apps: Vec<AppEntry>,
    /// Entries dropped (invalid, duplicate or over the cap).
    skipped: usize,
    prep: Vec<Prep>,
    by_id: HashMap<String, usize>,
    /// The letter groups, as ranges of `apps`.
    groups: Vec<(char, Range<usize>)>,
}

/// The A–Z group of a name: its first letter in upper case when that is a
/// Latin letter (after folding, so "É" is E and "ß" is S), `#` for anything
/// else: digits, symbols and other scripts.
/// The Start page's app groups, in the order they are shown. The panel
/// translates the keys.
pub const GROUPS: [&str; 10] = [
    "internet",
    "office",
    "media",
    "graphics",
    "development",
    "games",
    "education",
    "system",
    "utilities",
    "other",
];

/// The group an app is shown in, from its desktop file's `Categories`
/// (freedesktop's main categories, first match in GROUPS order wins, so a
/// browser that is also a "Utility" stays under Internet).
pub fn group_of(categories: &[String]) -> &'static str {
    let has = |names: &[&str]| categories.iter().any(|c| names.contains(&c.as_str()));
    if has(&["Network", "WebBrowser", "Email", "Chat", "InstantMessaging"]) {
        "internet"
    } else if has(&["Office", "WordProcessor", "Spreadsheet", "Presentation", "Calendar", "ContactManagement"]) {
        "office"
    } else if has(&["AudioVideo", "Audio", "Video", "Music", "Player", "Recorder", "TV"]) {
        "media"
    } else if has(&["Graphics", "Photography", "2DGraphics", "3DGraphics", "RasterGraphics", "VectorGraphics", "Viewer"]) {
        "graphics"
    } else if has(&["Development", "IDE", "TextEditor", "Debugger", "RevisionControl"]) {
        "development"
    } else if has(&["Game"]) {
        "games"
    } else if has(&["Education", "Science", "Math"]) {
        "education"
    } else if has(&["System", "Settings", "Monitor", "PackageManager", "TerminalEmulator", "FileManager", "Security"]) {
        "system"
    } else if has(&["Utility", "Accessibility", "Archiving", "Compression", "Calculator", "Clock"]) {
        "utilities"
    } else {
        "other"
    }
}

pub fn letter_of(name: &str) -> char {
    match fold(name).chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        Some('ß') => 'S',
        Some('æ') => 'A',
        Some('ø' | 'œ') => 'O',
        Some('đ' | 'ð') => 'D',
        Some('ł') => 'L',
        Some('ħ') => 'H',
        Some('ı') => 'I',
        _ => '#',
    }
}

/// An app's row: `app:<desktop id>`, its name, and its generic name (or
/// comment) below. Used for search hits, pins, A–Z and recent apps alike.
pub fn app_item(app: &AppEntry, score: f32) -> ResultItem {
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
        score,
        action: Action::LaunchApp {
            desktop_id: app.desktop_id.clone(),
            action: None,
        },
    }
}

impl Catalog {
    pub fn new(entries: Vec<AppEntry>) -> Catalog {
        let total = entries.len();
        let mut seen = HashMap::new();
        let mut apps: Vec<(char, String, AppEntry)> = Vec::new();
        for e in entries {
            if apps.len() >= MAX_APPS {
                break;
            }
            if seen.contains_key(&e.desktop_id) {
                continue;
            }
            let Some(e) = sanitize(e) else { continue };
            seen.insert(e.desktop_id.clone(), ());
            let folded = fold(&e.name);
            apps.push((letter_of(&e.name), folded, e));
        }
        // '#' sorts before 'A', and each letter's apps stay contiguous.
        apps.sort_by(|a, b| (a.0, &a.1, &a.2.desktop_id).cmp(&(b.0, &b.1, &b.2.desktop_id)));
        let apps: Vec<AppEntry> = apps.into_iter().map(|t| t.2).collect();

        let prep = apps
            .iter()
            .map(|a| {
                let mut secondary =
                    vec![Prepared::new(&a.generic_name), Prepared::new(&a.exec_name)];
                secondary.extend(a.keywords.iter().map(|k| Prepared::new(k)));
                Prep {
                    name: Prepared::new(&a.name),
                    secondary,
                    comment: Prepared::new(&a.comment),
                    actions: a.actions.iter().map(|x| Prepared::new(&x.name)).collect(),
                }
            })
            .collect();
        let by_id = apps
            .iter()
            .enumerate()
            .map(|(i, a)| (a.desktop_id.clone(), i))
            .collect();
        let mut groups: Vec<(char, Range<usize>)> = Vec::new();
        for (i, a) in apps.iter().enumerate() {
            let l = letter_of(&a.name);
            match groups.last_mut() {
                Some((c, r)) if *c == l => r.end = i + 1,
                _ => groups.push((l, i..i + 1)),
            }
        }
        Catalog {
            skipped: total - apps.len(),
            apps,
            prep,
            by_id,
            groups,
        }
    }

    pub fn len(&self) -> usize {
        self.apps.len()
    }

    /// How many entries were dropped (invalid, duplicate or over the cap),
    /// for the caller's log.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }

    pub fn get(&self, desktop_id: &str) -> Option<&AppEntry> {
        self.by_id.get(desktop_id).map(|&i| &self.apps[i])
    }

    /// Every app in A–Z order ("#" group first).
    pub fn a_to_z(&self) -> &[AppEntry] {
        &self.apps
    }

    /// The letters that have at least one app, in order ('#' first).
    pub fn letters(&self) -> Vec<char> {
        self.groups.iter().map(|g| g.0).collect()
    }

    /// The apps under one letter ('#' for non-letters), in order.
    pub fn apps_for_letter(&self, letter: char) -> &[AppEntry] {
        let l = letter_of(letter.encode_utf8(&mut [0; 4]));
        self.groups
            .iter()
            .find(|g| g.0 == l)
            .map_or(&[], |g| &self.apps[g.1.clone()])
    }

    /// Apps whose desktop file is at least as new as `since_unix`, newest
    /// first, at most `n`.
    pub fn recently_installed(&self, since_unix: i64, n: usize) -> Vec<&AppEntry> {
        let mut v: Vec<&AppEntry> = self
            .apps
            .iter()
            .filter(|a| a.installed_unix >= since_unix)
            .collect();
        v.sort_by(|a, b| {
            b.installed_unix
                .cmp(&a.installed_unix)
                .then_with(|| a.desktop_id.cmp(&b.desktop_id))
        });
        v.truncate(n);
        v
    }

    /// Apps (and, for "app word" queries, their desktop actions) matching the
    /// query, unsorted; the caller merges and ranks.
    pub fn search(&self, q: &Query) -> Vec<ResultItem> {
        let mut out = Vec::new();
        if q.is_empty() {
            return out;
        }
        let app_prior = prior(Kind::App);
        // Desktop actions: "firefox private" → the part after the first word.
        let action_query = (q.words.len() >= 2).then(|| {
            let rest = q.words[1..].join(" ");
            (q.words[0].as_str(), Query::new(&rest, q.serial))
        });
        for (app, p) in self.apps.iter().zip(&self.prep) {
            let desc = (!app.comment.is_empty()).then_some(&p.comment);
            if let Some(m) = score_fields(q, &p.name, &p.secondary, desc) {
                out.push(app_item(app, m * app_prior));
            }
            let Some((first, rest)) = &action_query else {
                continue;
            };
            if rest.is_empty() || !p.name.folded.starts_with(first) {
                continue;
            }
            for (act, ap) in app.actions.iter().zip(&p.actions) {
                if let Some(m) = score_fields(rest, ap, &[], None) {
                    out.push(ResultItem {
                        id: format!("app:{}#{}", app.desktop_id, act.id),
                        kind: Kind::App,
                        title: format!("{} \u{203a} {}", app.name, act.name),
                        subtitle: app.generic_name.clone(),
                        icon: if act.icon.is_empty() {
                            app.icon.clone()
                        } else {
                            act.icon.clone()
                        },
                        score: ACTION_WEIGHT * m * app_prior,
                        action: Action::LaunchApp {
                            desktop_id: app.desktop_id.clone(),
                            action: Some(act.id.clone()),
                        },
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn app(id: &str, name: &str) -> AppEntry {
        AppEntry {
            desktop_id: id.into(),
            name: name.into(),
            icon: "icon".into(),
            ..Default::default()
        }
    }

    #[test]
    fn groups_from_categories() {
        let g = |cats: &[&str]| group_of(&cats.iter().map(|c| c.to_string()).collect::<Vec<_>>());
        assert_eq!(g(&["Network", "WebBrowser", "Utility"]), "internet");
        assert_eq!(g(&["Qt", "KDE", "Office", "WordProcessor"]), "office");
        assert_eq!(g(&["AudioVideo", "Player"]), "media");
        assert_eq!(g(&["Graphics", "Viewer"]), "graphics");
        assert_eq!(g(&["Utility", "TextEditor"]), "development");
        assert_eq!(g(&["Game", "ArcadeGame"]), "games");
        assert_eq!(g(&["System", "TerminalEmulator"]), "system");
        assert_eq!(g(&["Utility", "Archiving"]), "utilities");
        assert_eq!(g(&[]), "other");
        assert_eq!(g(&["X-Unknown"]), "other");
        assert!(GROUPS.contains(&g(&["Science"])));
    }

    fn search(c: &Catalog, q: &str) -> Vec<ResultItem> {
        let mut r = c.search(&Query::new(q, 1));
        r.sort_by(ResultItem::rank_cmp);
        r
    }

    fn firefox() -> AppEntry {
        AppEntry {
            generic_name: "Web Browser".into(),
            comment: "Browse the World Wide Web".into(),
            keywords: vec!["internet".into(), "www".into()],
            exec_name: "firefox".into(),
            actions: vec![
                AppActionEntry {
                    id: "new-private-window".into(),
                    name: "New Private Window".into(),
                    icon: String::new(),
                },
                AppActionEntry {
                    id: "new-window".into(),
                    name: "New Window".into(),
                    icon: "window-new".into(),
                },
            ],
            ..app("org.mozilla.firefox.desktop", "Firefox")
        }
    }

    #[test]
    fn validates_entries() {
        let mut long = app("x.desktop", "Long");
        long.keywords = (0..100).map(|i| format!("k{i}")).collect();
        long.actions = (0..40)
            .map(|i| AppActionEntry {
                id: format!("a{i}"),
                name: format!("A {i}"),
                icon: String::new(),
            })
            .collect();
        long.flatpak_id = Some("not-reverse-dns".into());
        let mut fp = app("fp.desktop", "Fp");
        fp.flatpak_id = Some("org.example.App".into());
        let mut ctl = app("ctl.desktop", "Bad\u{202e}\nName  here");
        ctl.actions.push(AppActionEntry {
            id: "bad id".into(),
            name: "X".into(),
            icon: String::new(),
        });
        let c = Catalog::new(vec![
            app("../evil.desktop", "Evil"),
            app("noext", "No"),
            app("sp ace.desktop", "Space"),
            app(".desktop", "Empty"),
            long,
            fp,
            ctl,
            app("dup.desktop", "First"),
            app("dup.desktop", "Second"),
        ]);
        assert_eq!(c.len(), 4);
        assert_eq!(c.get("x.desktop").unwrap().keywords.len(), MAX_KEYWORDS);
        assert_eq!(c.get("x.desktop").unwrap().actions.len(), MAX_ACTIONS);
        assert_eq!(c.get("x.desktop").unwrap().flatpak_id, None);
        assert_eq!(
            c.get("fp.desktop").unwrap().flatpak_id.as_deref(),
            Some("org.example.App")
        );
        let b = c.get("ctl.desktop").unwrap();
        assert_eq!(b.name, "Bad Name here");
        assert!(b.actions.is_empty());
        assert_eq!(c.get("dup.desktop").unwrap().name, "First");
    }

    #[test]
    fn icons_are_checked_not_cleaned() {
        let mut a = app("a.desktop", "A");
        a.icon = "/usr/share/icons/a b/x.svg".into();
        a.actions = vec![
            AppActionEntry {
                id: "p".into(),
                name: "P".into(),
                icon: "/usr/share/x.png".into(),
            },
            AppActionEntry {
                id: "q".into(),
                name: "Q".into(),
                icon: "data:image/png;base64,AA".into(),
            },
        ];
        let mut b = app("b.desktop", "B");
        b.icon = "../../etc/passwd".into();
        let mut c = app("c.desktop", "C");
        c.icon = String::new();
        let cat = Catalog::new(vec![a, b, c, app("bad", "X")]);
        let a = cat.get("a.desktop").unwrap();
        assert_eq!(a.icon, "/usr/share/icons/a b/x.svg");
        assert_eq!(a.actions[0].icon, "/usr/share/x.png");
        assert_eq!(a.actions[1].icon, "");
        assert_eq!(cat.get("b.desktop").unwrap().icon, FALLBACK_ICON);
        assert_eq!(cat.get("c.desktop").unwrap().icon, FALLBACK_ICON);
        assert_eq!(cat.skipped(), 1);
    }

    #[test]
    fn caps_app_count() {
        let v: Vec<_> = (0..MAX_APPS + 50)
            .map(|i| app(&format!("a{i}.desktop"), &format!("App {i}")))
            .collect();
        let c = Catalog::new(v);
        assert_eq!(c.len(), MAX_APPS);
        assert_eq!(c.skipped(), 50);
    }

    #[test]
    fn id_validators() {
        assert!(valid_flatpak_id("org.kde.kate"));
        assert!(!valid_flatpak_id("org.kde"));
        assert!(!valid_flatpak_id("org..kate"));
        assert!(!valid_flatpak_id("org.kde.ka te"));
        assert!(valid_desktop_id("org.kde.dolphin.desktop"));
        assert!(!valid_desktop_id(&format!("{}.desktop", "a".repeat(260))));
    }

    #[test]
    fn name_beats_secondary_and_description() {
        let c = Catalog::new(vec![firefox(), app("org.kde.konsole.desktop", "Konsole")]);
        let r = search(&c, "firefox");
        assert_eq!(r[0].id, "app:org.mozilla.firefox.desktop");
        assert_eq!(r[0].score, 1.0);
        assert_eq!(r[0].subtitle, "Web Browser");
        assert_eq!(
            r[0].action,
            Action::LaunchApp {
                desktop_id: "org.mozilla.firefox.desktop".into(),
                action: None
            }
        );
        // Keyword and description matches are weaker.
        let r = search(&c, "internet");
        assert_eq!(r.len(), 1);
        assert!(r[0].score <= 0.7);
        let r = search(&c, "world wide");
        assert_eq!(r.len(), 1);
        assert!(r[0].score <= 0.3 + 1e-6);
        assert!(search(&c, "").is_empty());
        assert!(search(&c, "zzzzqq").is_empty());
    }

    #[test]
    fn desktop_actions() {
        let c = Catalog::new(vec![firefox()]);
        let r = search(&c, "firefox private");
        let a = r
            .iter()
            .find(|i| i.id == "app:org.mozilla.firefox.desktop#new-private-window")
            .expect("action result");
        assert_eq!(a.title, "Firefox \u{203a} New Private Window");
        assert!(a.score <= 0.9 + 1e-6 && a.score > 0.0);
        assert_eq!(
            a.action,
            Action::LaunchApp {
                desktop_id: "org.mozilla.firefox.desktop".into(),
                action: Some("new-private-window".into())
            }
        );
        // Not for one word, nor when the first word isn't the app's name.
        assert!(search(&c, "private").iter().all(|i| !i.id.contains('#')));
        assert!(search(&c, "chrome private").is_empty());
        assert!(
            search(&c, "fire win")
                .iter()
                .any(|i| i.id.ends_with("#new-window"))
        );
    }

    #[test]
    fn a_to_z_groups() {
        let c = Catalog::new(vec![
            app("z.desktop", "Zed"),
            app("e.desktop", "\u{c9}cran"),
            app("d.desktop", "Dolphin"),
            app("n.desktop", "7-Zip"),
            app("s.desktop", "_hidden"),
            app("e2.desktop", "Editor"),
        ]);
        assert_eq!(c.letters(), vec!['#', 'D', 'E', 'Z']);
        let e: Vec<_> = c
            .apps_for_letter('E')
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(e, vec!["\u{c9}cran", "Editor"]); // "ecran" < "editor"
        assert_eq!(c.apps_for_letter('#').len(), 2);
        assert_eq!(c.apps_for_letter('e').len(), 2);
        assert!(c.apps_for_letter('Q').is_empty());
        assert_eq!(c.a_to_z().len(), 6);
        assert_eq!(c.a_to_z()[5].name, "Zed");
    }

    #[test]
    fn latin_letters_group_under_base_letter() {
        let c = Catalog::new(vec![
            app("a.desktop", "\u{df}tuff"),
            app("b.desktop", "Sun"),
            app("c.desktop", "\u{d8}rsted"),
            app("d.desktop", "Opera"),
            app("e.desktop", "\u{416}uk"),
            app("f.desktop", "\u{65e5}\u{672c}"),
            app("g.desktop", "3D Tool"),
            app("h.desktop", "Zed"),
            app("i.desktop", "e\u{301}cran"),
        ]);
        assert_eq!(c.letters(), vec!['#', 'E', 'O', 'S', 'Z']);
        assert_eq!(c.apps_for_letter('#').len(), 3);
        assert_eq!(c.apps_for_letter('S').len(), 2);
        assert_eq!(c.apps_for_letter('O').len(), 2);
        assert_eq!(c.apps_for_letter('\u{c9}').len(), 1);
        assert!(c.apps_for_letter('\u{416}').len() == 3); // not a letter group: '#'
    }

    #[test]
    fn recently_installed_orders_and_limits() {
        let mk = |id: &str, t: i64| AppEntry {
            installed_unix: t,
            ..app(id, id)
        };
        let c = Catalog::new(vec![
            mk("a.desktop", 100),
            mk("b.desktop", 300),
            mk("c.desktop", 200),
            mk("d.desktop", 50),
        ]);
        let r: Vec<_> = c
            .recently_installed(100, 2)
            .iter()
            .map(|a| a.desktop_id.as_str())
            .collect();
        assert_eq!(r, vec!["b.desktop", "c.desktop"]);
        assert_eq!(c.recently_installed(1000, 5).len(), 0);
    }

    #[test]
    fn empty_catalog_is_fine() {
        let c = Catalog::new(Vec::new());
        assert!(c.is_empty());
        assert!(c.letters().is_empty());
        assert!(search(&c, "x").is_empty());
    }

    fn apps_1000() -> Catalog {
        let v: Vec<_> = (0..1000)
            .map(|i| AppEntry {
                generic_name: format!("Tool number {i}"),
                comment: format!("Does thing {i} with files and documents"),
                keywords: vec![format!("kw{i}"), "utility".into()],
                exec_name: format!("tool{i}"),
                ..app(
                    &format!("org.example.app{i}.desktop"),
                    &format!("Sample Application {i}"),
                )
            })
            .collect();
        Catalog::new(v)
    }

    #[test]
    fn search_of_1000_apps_is_fast() {
        let c = apps_1000();
        for text in ["sample app 5", "zqxv"] {
            let q = Query::new(text, 1);
            let _ = c.search(&q);
            let t = Instant::now();
            let n = 20;
            for _ in 0..n {
                std::hint::black_box(c.search(&q));
            }
            let per = t.elapsed() / n;
            println!("search {text:?} over 1000 apps: {per:?}");
            assert!(per.as_millis() < 20, "{text}: {per:?}");
        }
        assert!(search(&c, "zqxv").is_empty());
    }

    /// `cargo test --release -p atlas-launcher-core -- --ignored --nocapture bench`
    #[test]
    #[ignore = "benchmark: run in release mode"]
    fn bench_instant_1000_apps() {
        let c = apps_1000();
        for text in [
            "s",
            "sa",
            "samp",
            "sample app 5",
            "tool99",
            "zqxv",
            "smpl",
            "does thing",
        ] {
            let q = Query::new(text, 1);
            for _ in 0..50 {
                std::hint::black_box(c.search(&q));
            }
            let n = 500;
            let t = Instant::now();
            for _ in 0..n {
                std::hint::black_box(c.search(&q));
            }
            let us = t.elapsed().as_secs_f64() * 1e6 / f64::from(n);
            println!("bench {text:?}: {us:.1} us per query over 1000 apps");
        }
    }
}
