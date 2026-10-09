//! Property tests and a corpus of hostile inputs for everything the core reads
//! from outside or turns into something the desktop acts on
//! (docs/SECURITY.md, "Tests").
//!
//! `cargo test -p telamon-launcher-core -- props` runs the properties with
//! proptest's default 256 cases; `PROPTEST_CASES=20000 cargo test --workspace
//! --locked -- props` is the long run CI makes. The `corpus` module holds the
//! named worst cases (a 10 MB string, a pipe, a link, NUL, "..", bidi) and runs
//! in every `cargo test`.
//!
//! The properties, per boundary:
//! - never panics, on any input (control characters, NUL, bidi, huge, unpaired
//!   surfaces of the encodings the parsers take);
//! - the output is bounded;
//! - whatever is *accepted* has the safe shape: no control or bidi characters
//!   in shown text, ids and links match their pattern, paths are absolute and
//!   clean, URLs are https with percent-encoded text, a command is an absolute
//!   path;
//! - what is written reads back the same.

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use proptest::prelude::*;
use telamon_launcher_core::calc;
use telamon_launcher_core::catalog::{
    AppActionEntry, AppEntry, Catalog, MAX_ACTIONS, MAX_APPS, MAX_KEYWORDS, valid_desktop_id,
    valid_flatpak_id,
};
use telamon_launcher_core::commands::{
    CommandPlan, PathCache, SessionAvailability, SessionCommands, parse_command_line,
};
use telamon_launcher_core::fsutil::{read_capped, write_atomic, write_atomic_nofollow};
use telamon_launcher_core::late::{
    ExplorerHit, RunnerMatch, explorer_item, explorer_items, runner_item, runner_items,
};
use telamon_launcher_core::legacy::{canonical_desktop_id, desktop_id_alias};
use telamon_launcher_core::names::{self, MAX_NAME_CHARS, Names, clean_name};
use telamon_launcher_core::overrides::{self, AppDirs};
use telamon_launcher_core::pins::{self, Pins, valid_pin};
use telamon_launcher_core::query::{self, Context, MAX_RESULTS, SearchOptions, Sources};
use telamon_launcher_core::recent::{self, RecentFiles};
use telamon_launcher_core::result::{Action, Kind, ResultItem};
use telamon_launcher_core::settings_index::{self, SettingsIndex, locale_chain, valid_link};
use telamon_launcher_core::text::{
    self, MAX_DISPLAY_CHARS, MAX_QUERY_CHARS, Prepared, Query, clean_display, clean_display_max,
    clean_query, fold, is_unsafe_char, valid_icon,
};
use telamon_launcher_core::usage::{self, UsageStore};
use telamon_launcher_core::web::{self, Engine};

/// A string nobody should be able to find in a log, a debug print or a file
/// that is not meant to hold it.
const CANARY: &str = "CANARY-7f3a9c-do-not-leak";

// ----------------------------------------------------------------- helpers

/// Runs the rest of a property body for at most `$n` cases of the process,
/// whatever `PROPTEST_CASES` says (proptest lets that variable override the
/// configured count, so a cap has to live in the body): the properties that
/// build whole catalogues or touch the file system cost milliseconds a case,
/// and the long run (20,000 cases) must stay under a minute each. The `corpus`
/// module covers their worst inputs.
macro_rules! cap_cases {
    ($n:expr) => {{
        static CASES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if CASES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= $n {
            return Ok(());
        }
    }};
}

/// Characters that have bitten parsers: control, NUL, bidi, zero-width,
/// invisible, separators, quotes, escapes, shell and URL syntax, combining
/// marks, and anything at all.
fn nasty_char() -> impl Strategy<Value = char> {
    let special: Vec<char> = vec![
        '\0',
        '\n',
        '\r',
        '\t',
        '\u{1b}',
        '\u{7f}',
        '\u{85}',
        '\u{a0}',
        '\u{ad}',
        '\u{2028}',
        '\u{2029}',
        '\u{202a}',
        '\u{202e}',
        '\u{2066}',
        '\u{2069}',
        '\u{200b}',
        '\u{200d}',
        '\u{200f}',
        '\u{feff}',
        '\u{3164}',
        '\u{e0001}',
        '\u{fe0f}',
        '\u{2800}',
        '\u{fffc}',
        '\u{301}',
        '"',
        '\'',
        '\\',
        '%',
        '/',
        '.',
        ' ',
        ';',
        '$',
        '`',
        '<',
        '>',
        '&',
        '#',
        '?',
        '=',
        ':',
        '[',
        ']',
        '{',
        '}',
        '*',
        '~',
        '!',
        '|',
        '(',
        ')',
        '-',
        '_',
        '+',
        ',',
        '@',
        '^',
    ];
    prop_oneof![
        6 => any::<char>(),
        5 => proptest::sample::select(special),
        4 => proptest::char::range('a', 'z'),
        1 => proptest::char::range('0', '9'),
    ]
}

fn nasty_string(max: usize) -> impl Strategy<Value = String> {
    proptest::collection::vec(nasty_char(), 0..max).prop_map(|v| v.into_iter().collect())
}

/// Mostly valid desktop ids, with the near misses mixed in.
fn desktop_id() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => "[A-Za-z0-9._-]{1,30}".prop_map(|s| format!("{s}.desktop")),
        2 => nasty_string(40).prop_map(|s| format!("{s}.desktop")),
        1 => nasty_string(40),
        1 => proptest::sample::select(vec![
            "../x.desktop", "a/b.desktop", "/etc/passwd.desktop", "..desktop", ".desktop",
            "-x.desktop", "a b.desktop", "x.desktop\n", "x\0.desktop", "org.kde.dolphin.desktop",
        ]).prop_map(str::to_owned),
    ]
}

fn icon_text() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "[a-z][a-z0-9._-]{0,20}",
        2 => "/[a-z./_-]{0,30}",
        3 => nasty_string(40),
        1 => proptest::sample::select(vec![
            "file:///etc/passwd", "https://evil/x.png", "//host/x", "/a/../b", "/dev/zero", "",
        ]).prop_map(str::to_owned),
    ]
}

fn app_entry() -> impl Strategy<Value = AppEntry> {
    // Small on purpose: generating strings is the slow part of a case. The
    // caps (512 characters, 64 keywords, 32 actions...) are held by
    // `catalog_caps_hold` and the `corpus` module with big, cheap inputs.
    let action = (nasty_string(8), nasty_string(20), icon_text())
        .prop_map(|(id, name, icon)| AppActionEntry { id, name, icon });
    (
        desktop_id(),
        nasty_string(60),
        nasty_string(20),
        nasty_string(30),
        nasty_string(60),
        proptest::collection::vec(nasty_string(20), 0..4),
        icon_text(),
        nasty_string(30),
        proptest::collection::vec(nasty_string(20), 0..3),
        proptest::collection::vec(action, 0..3),
        proptest::option::of(nasty_string(30)),
        any::<i64>(),
    )
        .prop_map(
            |(
                desktop_id,
                name,
                original_name,
                generic_name,
                comment,
                keywords,
                icon,
                exec_name,
                categories,
                actions,
                flatpak_id,
                installed_unix,
            )| AppEntry {
                desktop_id,
                name,
                original_name,
                generic_name,
                comment,
                keywords,
                icon,
                exec_name,
                categories,
                actions,
                flatpak_id,
                installed_unix,
            },
        )
}

fn no_unsafe(s: &str) -> bool {
    !s.chars().any(is_unsafe_char)
}

/// Shown text: no control or bidi characters, no edge or doubled spaces, and
/// no whitespace but a plain space.
fn clean_shown(s: &str) -> bool {
    no_unsafe(s)
        && !s.starts_with(' ')
        && !s.ends_with(' ')
        && !s.contains("  ")
        && !s.chars().any(|c| c.is_whitespace() && c != ' ')
}

fn valid_action_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// What a typed or listed row may carry out: the rules the C++ side checks
/// again before anything starts (docs/SECURITY.md, "Starting programs").
fn assert_safe_action(a: &Action) {
    match a {
        Action::LaunchApp { desktop_id, action } => {
            assert!(valid_desktop_id(desktop_id), "{desktop_id:?}");
            if let Some(act) = action {
                assert!(valid_action_id(act), "{act:?}");
            }
        }
        Action::OpenSettings { link } => assert!(valid_link(link), "{link:?}"),
        Action::OpenFile { uri } => {
            assert!(uri.starts_with("file:///"), "{uri:?}");
            assert!(uri.len() <= 4096 * 3 + 8);
            assert!(
                uri["file://".len()..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~/%".contains(&b)),
                "{uri:?}"
            );
            // Decodes to a clean absolute path.
            let path = pct_decode(&uri["file://".len()..]);
            assert!(path.starts_with('/') && !path.contains('\0'));
            assert!(
                !path[1..].split('/').any(|s| s == ".." || s == "."),
                "{uri:?}"
            );
        }
        Action::Copy { text } => assert!(text.len() <= 256, "{}", text.len()),
        Action::Run { executable, args } => {
            assert!(executable.starts_with('/'), "{executable:?}");
            assert!(!executable.contains('\0'));
            assert!(args.iter().all(|a| !a.contains('\0')));
            assert!(args.iter().map(String::len).sum::<usize>() <= MAX_QUERY_CHARS * 4);
        }
        Action::RunInTerminal { line } => {
            assert!(!line.chars().any(|c| c.is_control()), "{line:?}");
            assert!(no_unsafe(line));
            assert!(line.chars().count() < MAX_QUERY_CHARS);
        }
        Action::Session(_) => {}
        Action::OpenWeb { url } => {
            assert!(url.starts_with("https://"), "{url:?}");
            assert!(
                url.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~:/?=%&".contains(&b)),
                "{url:?}"
            );
            assert!(url.len() <= 64 + web::MAX_QUERY_BYTES * 3);
        }
        Action::Runner {
            runner_id,
            match_id,
        } => {
            assert!(
                !runner_id.is_empty()
                    && runner_id.len() <= 128
                    && runner_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            );
            assert!(!match_id.is_empty() && match_id.len() <= 1024);
            assert!(
                !match_id
                    .chars()
                    .any(|c| c.is_control() || is_unsafe_char(c))
            );
        }
    }
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(h, 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).expect("a rebuilt URI decodes to UTF-8")
}

fn assert_safe_item(it: &ResultItem) {
    if it.kind == Kind::Command {
        // A command row shows the line as typed, spacing included (a doubled
        // space is the user's); nothing invisible or odd is allowed.
        assert!(no_unsafe(&it.title), "title {:?}", it.title);
        assert!(!it.title.chars().any(|c| c.is_whitespace() && c != ' '));
    } else {
        assert!(clean_shown(&it.title), "title {:?}", it.title);
    }
    assert!(clean_shown(&it.subtitle), "subtitle {:?}", it.subtitle);
    assert!(no_unsafe(&it.id), "id {:?}", it.id);
    assert!(valid_icon(&it.icon), "icon {:?}", it.icon);
    assert!(it.score.is_finite());
    assert_safe_action(&it.action);
}

// ----------------------------------------------------------- the properties

mod props {
    use super::*;

    // ---- text ----

    proptest! {
        #[test]
        fn clean_display_is_safe_bounded_and_idempotent(s in nasty_string(260), max in 1usize..300) {
            let out = clean_display_max(&s, max);
            prop_assert!(clean_shown(&out), "{:?}", out);
            prop_assert!(out.chars().count() <= max.max(1));
            prop_assert_eq!(clean_display_max(&out, max), out);
            let d = clean_display(&s);
            prop_assert!(d.chars().count() <= MAX_DISPLAY_CHARS);
            prop_assert!(clean_shown(&d));
        }

        #[test]
        fn clean_name_is_safe_bounded_and_idempotent(s in nasty_string(120)) {
            let out = clean_name(&s);
            prop_assert!(out.chars().count() <= MAX_NAME_CHARS);
            prop_assert!(clean_shown(&out), "{:?}", out);
            prop_assert!(!out.chars().any(|c| c.is_control() || c.is_whitespace() && c != ' '));
            prop_assert_eq!(clean_name(&out), out);
        }

        #[test]
        fn clean_query_and_query_new(s in nasty_string(400)) {
            let out = clean_query(&s);
            prop_assert!(out.chars().count() <= MAX_QUERY_CHARS);
            prop_assert!(out.chars().all(|c| c == ' ' || !is_unsafe_char(c)));
            prop_assert_eq!(clean_query(&out), out.clone());
            let q = Query::new(&s, 7);
            prop_assert_eq!(&q.raw, &out);
            prop_assert!(q.words.iter().all(|w| !w.is_empty() && !w.contains(char::is_whitespace)));
            prop_assert_eq!(q.words.join(" "), q.folded.clone());
            // Never in a debug print.
            let q2 = Query::new(&format!("{CANARY}{s}"), 1);
            assert!(!format!("{q2:?}").contains(CANARY));
        }

        #[test]
        fn fold_and_prepared_do_not_panic(s in nasty_string(200), q in nasty_string(80)) {
            let _ = fold(&s);
            let p = Prepared::new(&s);
            let query = Query::new(&q, 0);
            let _ = p.matches(&query);
            let _ = p.matches_strict(&query);
            let _ = text::score_fields(&query, &p, std::slice::from_ref(&p), Some(&p));
        }

        #[test]
        fn icons_are_names_or_clean_absolute_paths(s in icon_text()) {
            if valid_icon(&s) {
                if s.starts_with('/') {
                    prop_assert!(!s.starts_with("//"));
                    prop_assert!(!s.split('/').any(|seg| seg == ".."));
                    prop_assert!(no_unsafe(&s));
                    prop_assert!(s.len() <= 4096);
                } else {
                    prop_assert!(!s.contains('/') && !s.contains(':') && !s.is_empty() && s.len() <= 128);
                    prop_assert!(!s.starts_with('.') && !s.starts_with('-'));
                }
            }
        }
    }

    // ---- the catalogue (hostile desktop entries) ----

    proptest! {

        #[test]
        fn catalog_entries_come_out_safe(entries in proptest::collection::vec(app_entry(), 0..3), q in nasty_string(16)) {
            cap_cases!(1000);
            let n = entries.len();
            let cat = Catalog::new(entries);
            prop_assert!(cat.len() <= n && cat.len() <= MAX_APPS);
            for e in cat.a_to_z() {
                prop_assert!(valid_desktop_id(&e.desktop_id));
                prop_assert!(!e.name.is_empty() && clean_shown(&e.name), "name {:?}", e.name);
                prop_assert!(e.name.chars().count() <= MAX_DISPLAY_CHARS);
                for s in [&e.original_name, &e.generic_name, &e.comment, &e.exec_name] {
                    prop_assert!(s.is_empty() || clean_shown(s), "{:?}", s);
                }
                prop_assert!(e.keywords.len() <= MAX_KEYWORDS && e.keywords.iter().all(|k| clean_shown(k) && !k.is_empty()));
                prop_assert!(e.categories.iter().all(|k| clean_shown(k)));
                prop_assert!(e.actions.len() <= MAX_ACTIONS);
                for a in &e.actions {
                    prop_assert!(valid_action_id(&a.id));
                    prop_assert!(!a.name.is_empty() && clean_shown(&a.name));
                    prop_assert!(a.icon.is_empty() || valid_icon(&a.icon));
                }
                prop_assert!(valid_icon(&e.icon), "icon {:?}", e.icon);
                if let Some(f) = &e.flatpak_id {
                    prop_assert!(valid_flatpak_id(f));
                }
            }
            let hits = cat.search(&Query::new(&q, 1));
            for it in &hits {
                assert_safe_item(it);
            }
            // Each desktop id appears once.
            let mut ids: Vec<&str> = cat.a_to_z().iter().map(|e| e.desktop_id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            prop_assert_eq!(ids.len(), cat.len());
        }

        /// The catalogue's caps, with big but cheap inputs: a name past 512
        /// characters, 100 keywords, 100 categories, 100 actions, an icon of
        /// 5,000 bytes, an id of 300 bytes, 12,000 apps.
        #[test]
        fn catalog_caps_hold(
            prefix in nasty_string(6),
            name_len in 480usize..700,
            kws in 60usize..100,
            acts in 30usize..60,
            apps in 0usize..3,
        ) {
            cap_cases!(40);
            let big = |c: char, n: usize| std::iter::repeat_n(c, n).collect::<String>();
            let entry = |i: usize| AppEntry {
                desktop_id: format!("big{i}.desktop"),
                name: format!("{prefix}{}", big('n', name_len)),
                comment: big('c', 3 * name_len),
                keywords: vec![big('k', 90); kws],
                categories: vec![big('g', 90); kws],
                icon: big('i', 5000),
                exec_name: big('e', 600),
                actions: (0..acts)
                    .map(|a| AppActionEntry { id: format!("a{a}"), name: big('x', 600), icon: String::new() })
                    .collect(),
                ..AppEntry::default()
            };
            let mut entries: Vec<AppEntry> = (0..apps + 1).map(entry).collect();
            // Past the cap of apps, and an id past 255 bytes.
            entries.extend((0..MAX_APPS + 50).map(|i| AppEntry {
                desktop_id: format!("many{i}.desktop"),
                name: "x".into(),
                ..AppEntry::default()
            }));
            entries.push(AppEntry { desktop_id: format!("{}.desktop", big('a', 300)), name: "long id".into(), ..AppEntry::default() });
            let cat = Catalog::new(entries);
            prop_assert!(cat.len() <= MAX_APPS);
            for e in cat.a_to_z() {
                prop_assert!(e.name.chars().count() <= MAX_DISPLAY_CHARS);
                prop_assert!(e.comment.chars().count() <= MAX_DISPLAY_CHARS);
                prop_assert!(e.keywords.len() <= MAX_KEYWORDS && e.keywords.iter().all(|k| k.chars().count() <= 64));
                prop_assert!(e.categories.len() <= 64 && e.categories.iter().all(|k| k.chars().count() <= 64));
                prop_assert!(e.actions.len() <= MAX_ACTIONS && e.actions.iter().all(|a| a.name.chars().count() <= MAX_DISPLAY_CHARS));
                prop_assert!(e.exec_name.chars().count() <= 255);
                prop_assert_eq!(&e.icon, "application-x-executable");
                prop_assert!(e.desktop_id.len() <= 255);
            }
        }

        #[test]
        fn names_rename_into_the_catalogue_safely(
            entries in proptest::collection::vec(app_entry(), 0..3),
            raw in nasty_string(60),
        ) {
            cap_cases!(1000);
            let mut names = Names::default();
            for e in &entries {
                names.set(&e.desktop_id, &raw);
            }
            let mut apps = entries.clone();
            names.apply(&mut apps);
            let cat = Catalog::new(apps);
            for e in cat.a_to_z() {
                prop_assert!(clean_shown(&e.name));
            }
        }
    }

    // ---- the rename override writer ----

    proptest! {
        #[test]
        fn render_never_adds_a_key(
            body in proptest::collection::vec(
                prop_oneof![
                    Just("Exec=files %U".to_owned()),
                    Just("Name[de]=Dateien".to_owned()),
                    Just("Icon=folder".to_owned()),
                    Just("X-Telamon-Renamed=false".to_owned()),
                    Just("[Desktop Action new]".to_owned()),
                    Just("[Other]".to_owned()),
                    nasty_string(30).prop_map(|s| s.replace(['\n', '\r'], " ")),
                ],
                0..12,
            ),
            name in nasty_string(100),
        ) {
            let original = format!("[Desktop Entry]\nType=Application\nName=Files\n{}\n", body.join("\n"));
            let out = overrides::render(&original, &name);
            if clean_name(&name) != name || name.is_empty() {
                prop_assert!(out.is_none(), "unclean name accepted: {:?}", name);
            }
            if let Some(out) = out {
                prop_assert!(overrides::is_ours(out.as_bytes()));
                // At most the original (with its Name echoed in the marker), the escaped name and the keys.
                prop_assert!(out.len() <= 2 * original.len() + 8 * MAX_NAME_CHARS + 128);
                // Every line of the output is a line of the original (CR-trimmed), or one
                // of ours: Name=, X-Telamon-Renamed=true, X-Telamon-Original-Name=.
                let orig_lines: std::collections::HashSet<&str> =
                    original.split('\n').map(|l| l.trim_end_matches('\r')).collect();
                let mut ours = 0;
                for line in out.split('\n') {
                    if orig_lines.contains(line) || line.is_empty() {
                        continue;
                    }
                    let known = line.starts_with("Name=")
                        || line == "X-Telamon-Renamed=true"
                        || line.starts_with("X-Telamon-Original-Name=");
                    prop_assert!(known, "new line {:?} in {:?}", line, out);
                    ours += 1;
                }
                prop_assert!(ours <= 3);
                // Exactly one Name= in the entry and it is the escaped name.
                let entry = out.split("\n[").next().unwrap();
                let names: Vec<&str> = entry.lines().filter(|l| l.starts_with("Name=")).collect();
                prop_assert_eq!(names.len(), 1);
                prop_assert_eq!(names[0], format!("Name={}", name.replace('\\', "\\\\")));
            }
        }
    }

    /// One user dir and two system dirs, reused across cases (a case
    /// resets the user dir).
    struct Fs {
        _dir: tempfile::TempDir,
        root: PathBuf,
        dirs: AppDirs,
    }

    fn fs_fixture() -> Fs {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let sys = root.join("usr/share/applications");
        fs::create_dir_all(&sys).unwrap();
        fs::create_dir_all(root.join("usr/share/applications/kde")).unwrap();
        let original = "[Desktop Entry]\nType=Application\nName=Files\nExec=files\n";
        for id in ["org.example.files.desktop", "a.desktop", "kde-foo.desktop"] {
            fs::write(sys.join(id), original).unwrap();
        }
        fs::write(sys.join("kde/foo.desktop"), original).unwrap();
        // Something outside every folder the writer may touch.
        fs::write(root.join("outside.desktop"), original).unwrap();
        Fs {
            dirs: AppDirs {
                user: root.join("home/applications"),
                system: vec![sys],
            },
            root,
            _dir: dir,
        }
    }

    fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                let ft = e.file_type().unwrap();
                if ft.is_dir() {
                    stack.push(p);
                } else if ft.is_file() {
                    let b = fs::read(&p).unwrap();
                    out.push((p, b));
                } else {
                    out.push((p, b"<non-file>".to_vec()));
                }
            }
        }
        out.sort();
        out
    }

    proptest! {

        /// Whatever ids and names reach the override writer, nothing outside
        /// the user's applications folder changes, and what it makes there is
        /// a plain file named by a valid desktop id.
        #[test]
        fn the_override_writer_stays_in_its_folder(
            ops in proptest::collection::vec((desktop_id(), nasty_string(90), any::<bool>()), 1..6),
        ) {
            cap_cases!(5000);
            thread_local! { static FX: Fs = fs_fixture(); }
            FX.with(|f| {
                let _ = fs::remove_dir_all(f.root.join("home"));
                let outside_before: Vec<_> = tree(&f.root)
                    .into_iter()
                    .filter(|(p, _)| !p.starts_with(f.root.join("home")))
                    .collect();
                for (id, name, remove) in &ops {
                    let ids = overrides::ids_of(id);
                    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                    let o = overrides::apply(&f.dirs, &refs, name);
                    if clean_name(name) != *name {
                        // An unclean name is never written.
                        prop_assert!(!matches!(o, overrides::Outcome::Written), "{:?}", name);
                    }
                    if *remove {
                        let _ = overrides::remove(&f.dirs, id);
                    }
                }
                let after = tree(&f.root);
                let outside_after: Vec<_> = after
                    .iter()
                    .filter(|(p, _)| !p.starts_with(f.root.join("home")))
                    .cloned()
                    .collect();
                prop_assert_eq!(&outside_before, &outside_after);
                for (p, bytes) in after.iter().filter(|(p, _)| p.starts_with(f.root.join("home"))) {
                    prop_assert_eq!(p.parent().unwrap(), f.dirs.user.as_path());
                    let file = p.file_name().unwrap().to_str().unwrap();
                    prop_assert!(valid_desktop_id(file), "{:?}", file);
                    prop_assert!(overrides::is_ours(bytes));
                    prop_assert!(bytes.len() <= overrides::MAX_DESKTOP_BYTES as usize);
                    let mode = fs::metadata(p).unwrap().permissions().mode() & 0o7777;
                    prop_assert_eq!(mode, 0o644);
                }
                // No temp file survives.
                prop_assert!(after.iter().all(|(p, _)| !p.to_string_lossy().contains(".tmp-")));
                Ok(())
            })?;
        }

        /// A sync against any names leaves only files it made, in its folder.
        #[test]
        fn sync_keeps_to_its_folder(
            pairs in proptest::collection::vec((desktop_id(), nasty_string(70)), 0..8),
            foreign in proptest::collection::vec("[a-z]{1,8}\\.desktop", 0..3),
        ) {
            cap_cases!(5000);
            thread_local! { static FX2: Fs = fs_fixture(); }
            FX2.with(|f| {
                let _ = fs::remove_dir_all(f.root.join("home"));
                fs::create_dir_all(&f.dirs.user).unwrap();
                // The user's own files in the folder, which sync must leave alone.
                for name in &foreign {
                    fs::write(f.dirs.user.join(name), "[Desktop Entry]\nType=Application\nName=Mine\n").unwrap();
                }
                let before_outside: Vec<_> = tree(&f.root)
                    .into_iter()
                    .filter(|(p, _)| !p.starts_with(f.root.join("home")))
                    .collect();
                let mut names = Names::default();
                for (id, name) in &pairs {
                    names.set(id, name);
                }
                let _ = overrides::sync(&f.dirs, &names);
                let after = tree(&f.root);
                let outside: Vec<_> = after
                    .iter()
                    .filter(|(p, _)| !p.starts_with(f.root.join("home")))
                    .cloned()
                    .collect();
                prop_assert_eq!(before_outside, outside);
                for name in &foreign {
                    let mine = fs::read_to_string(f.dirs.user.join(name)).unwrap();
                    prop_assert_eq!(mine, "[Desktop Entry]\nType=Application\nName=Mine\n");
                }
                for (p, bytes) in after.iter().filter(|(p, _)| p.starts_with(f.root.join("home"))) {
                    let file = p.file_name().unwrap().to_str().unwrap().to_owned();
                    if foreign.contains(&file) {
                        continue;
                    }
                    prop_assert!(valid_desktop_id(&file));
                    prop_assert!(overrides::is_ours(bytes));
                }
                Ok(())
            })?;
        }
    }

    // ---- the files the launcher reads and writes ----

    proptest! {

        #[test]
        fn pins_parse_is_safe_and_round_trips(bytes in proptest::collection::vec(any::<u8>(), 0..500), text in nasty_string(200)) {
            for input in [bytes, text.into_bytes()] {
                let p = Pins::parse(&input);
                prop_assert!(p.ids().len() <= pins::MAX_PINS);
                prop_assert!(p.ids().iter().all(|i| valid_pin(i)));
                let mut seen = std::collections::HashSet::new();
                prop_assert!(p.ids().iter().all(|i| seen.insert(i.clone())));
                let again = Pins::parse(&p.to_bytes());
                prop_assert_eq!(again.ids(), p.ids());
            }
        }

        #[test]
        fn pins_import_is_bounded_and_valid(list in proptest::collection::vec(desktop_id(), 0..40)) {
            cap_cases!(8000);
            let p = Pins::import(&list);
            prop_assert!(p.ids().len() <= pins::MAX_IMPORT);
            prop_assert!(p.ids().iter().all(|i| valid_pin(i)));
        }

        #[test]
        fn names_parse_is_safe_and_round_trips(bytes in proptest::collection::vec(any::<u8>(), 0..600), text in nasty_string(300)) {
            for input in [bytes, format!("[Names]\n{text}").into_bytes()] {
                let n = Names::parse(&input);
                prop_assert!(n.len() <= names::MAX_NAMES);
                for (id, name) in n.iter() {
                    prop_assert!(valid_desktop_id(id));
                    prop_assert!(!name.is_empty() && name == clean_name(name));
                }
                let again = Names::parse(&n.to_bytes());
                let a: Vec<_> = again.iter().collect();
                let b: Vec<_> = n.iter().collect();
                prop_assert_eq!(a, b, "the names file did not read back the same");
                // Debug shows a count only.
                let mut m = Names::default();
                m.set("org.x.y.desktop", &format!("{CANARY} app"));
                assert!(!format!("{m:?}").contains(CANARY));
            }
        }

        #[test]
        fn names_set_never_stores_an_unclean_name(id in desktop_id(), raw in nasty_string(200)) {
            let mut n = Names::default();
            if n.set(&id, &raw) {
                prop_assert!(valid_desktop_id(&id));
                let got = n.get(&id).unwrap().to_owned();
                prop_assert_eq!(&got, &clean_name(&raw));
            } else {
                prop_assert!(n.is_empty());
            }
        }

        #[test]
        fn usage_load_is_safe_bounded_and_stable(bytes in proptest::collection::vec(any::<u8>(), 0..600), lines in proptest::collection::vec(
            (nasty_string(14), prop_oneof![Just("app:org.x.desktop".to_owned()), Just("run:ls".to_owned()), nasty_string(30)], nasty_string(10), nasty_string(14)), 0..10)) {
            let text: String = lines.iter().map(|(a, b, c, d)| format!("{a}\t{b}\t{c}\t{d}\n")).collect();
            for input in [bytes, text.into_bytes()] {
                let s = UsageStore::load_at(&input, 1_700_000_000);
                let out = s.to_bytes();
                prop_assert!(out.len() as u64 <= usage::MAX_FILE_BYTES);
                prop_assert!(s.len() <= usage::MAX_ENTRIES);
                for line in std::str::from_utf8(&out).unwrap().lines() {
                    let f: Vec<&str> = line.split('\t').collect();
                    prop_assert_eq!(f.len(), 4, "{:?}", line);
                    prop_assert!(f[0].chars().count() <= usage::MAX_PREFIX_CHARS);
                    prop_assert!(!f[0].chars().any(char::is_control) && !f[1].chars().any(char::is_control));
                    prop_assert!(usage::learnable(f[1]), "{:?}", f[1]);
                    prop_assert!(f[1].len() <= usage::MAX_ID_BYTES);
                }
                let again = UsageStore::load_at(&out, 1_700_000_000).to_bytes();
                prop_assert_eq!(again, out);
            }
        }

        #[test]
        fn usage_records_only_what_carries_nothing_private(
            queries in proptest::collection::vec(nasty_string(60), 1..8),
            ids in proptest::collection::vec(prop_oneof![
                Just("app:org.x.desktop".to_owned()),
                Just("setting:users".to_owned()),
                Just("session:lock".to_owned()),
                Just("run:ls -la".to_owned()),
                Just("term:ls".to_owned()),
                Just("file:file:///home/u/secret.txt".to_owned()),
                Just("calc".to_owned()),
                Just("web".to_owned()),
                Just("runner:x:y".to_owned()),
                nasty_string(40),
            ], 1..8),
        ) {
            let mut s = UsageStore::default();
            for (i, q) in queries.iter().enumerate() {
                let q = Query::new(&format!("{q}{CANARY}"), 0);
                for id in &ids {
                    s.record(&q, id, 1_700_000_000 + i as i64);
                }
            }
            let out = String::from_utf8(s.to_bytes()).unwrap();
            // Only 8 characters of any query, never a typed command or a path.
            prop_assert!(!out.contains(CANARY));
            for line in out.lines() {
                let id = line.split('\t').nth(1).unwrap();
                prop_assert!(["app:", "setting:", "session:"].iter().any(|p| id.starts_with(p)), "{:?}", id);
            }
            prop_assert!(!out.contains("run:") && !out.contains("term:") && !out.contains("file:") && !out.contains("secret"));
            assert!(!format!("{s:?}").contains(CANARY));
        }

        #[test]
        fn legacy_ids_are_stable(id in nasty_string(60)) {
            let c = canonical_desktop_id(&id);
            prop_assert_eq!(canonical_desktop_id(&c), c.clone());
            if let Some(a) = desktop_id_alias(&id) {
                prop_assert_eq!(desktop_id_alias(&a), Some(id.clone()));
            }
        }

        #[test]
        fn locale_chains_are_short_and_plain(
            a in proptest::option::of(nasty_string(120)),
            b in proptest::option::of(nasty_string(60)),
            c in proptest::option::of(nasty_string(60)),
            d in proptest::option::of(nasty_string(60)),
        ) {
            let chain = locale_chain(a.as_deref(), b.as_deref(), c.as_deref(), d.as_deref());
            prop_assert!(chain.len() <= 18 && chain.last().map(String::as_str) == Some("C"));
            prop_assert!(chain.iter().all(|l| l.len() <= 32 && l.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))));
        }
    }

    // ---- recently-used.xbel ----

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect()
    }

    fn xbel(entries: &[(String, String, String)]) -> String {
        let mut doc = String::from(
            "<?xml version=\"1.0\"?>\n<xbel version=\"1.0\" xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\">\n",
        );
        for (href, date, mime) in entries {
            doc.push_str(&format!(
                "<bookmark href=\"{}\" added=\"{}\" modified=\"{}\" visited=\"{}\"><info><metadata><mime:mime-type type=\"{}\"/></metadata></info></bookmark>\n",
                xml_escape(href), xml_escape(date), xml_escape(date), xml_escape(date), xml_escape(mime)
            ));
        }
        doc.push_str("</xbel>\n");
        doc
    }

    fn href() -> impl Strategy<Value = String> {
        prop_oneof![
            4 => nasty_string(60).prop_map(|p| format!("file:///{p}")),
            3 => proptest::collection::vec(
                prop_oneof![
                    3 => "[a-z]{1,6}",
                    1 => Just("..".to_owned()),
                    1 => Just(".".to_owned()),
                    1 => Just("%2e%2e".to_owned()),
                    1 => Just("%00".to_owned()),
                    1 => Just("%0a".to_owned()),
                    1 => Just("%2f".to_owned()),
                    1 => Just("%e2%80%ae".to_owned()),
                    1 => Just("%".to_owned()),
                    1 => Just("%zz".to_owned()),
                    1 => Just("".to_owned()),
                    1 => nasty_string(8),
                ], 0..8).prop_map(|v| format!("file:///{}", v.join("/"))),
            2 => nasty_string(60).prop_map(|p| format!("file://{p}")),
            1 => nasty_string(60),
        ]
    }

    proptest! {
        #[test]
        fn recent_files_parse_is_safe_and_bounded(
            entries in proptest::collection::vec((href(), nasty_string(16), nasty_string(8)), 0..6),
            raw in proptest::collection::vec(any::<u8>(), 0..300),
        ) {
            for input in [xbel(&entries).into_bytes(), raw] {
                if let Ok(r) = RecentFiles::parse(&input) {
                    prop_assert!(r.items.len() <= recent::MAX_KEPT);
                    let mut seen = std::collections::HashSet::new();
                    for f in &r.items {
                        let path = f.path.to_str().expect("UTF-8");
                        prop_assert!(path.starts_with('/') && path.len() <= recent::MAX_PATH_BYTES && !path.contains('\0'));
                        prop_assert!(path == "/" || !path[1..].split('/').any(|s| s.is_empty() || s == "." || s == ".."), "{:?}", path);
                        prop_assert!(seen.insert(f.path.clone()), "duplicate path");
                        prop_assert!(clean_shown(&f.name) && !f.name.is_empty(), "{:?}", f.name);
                        prop_assert!(f.uri.starts_with("file:///"));
                        prop_assert!(f.uri["file://".len()..].bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~/%".contains(&b)), "{:?}", f.uri);
                        prop_assert_eq!(pct_decode(&f.uri["file://".len()..]), path.to_owned());
                        prop_assert!(f.when >= 0);
                        if let Some(m) = &f.mime {
                            prop_assert!(m.len() <= 255 && m.contains('/') && m.bytes().all(|b| b.is_ascii_graphic()));
                        }
                    }
                    // Searching and listing rows from it is safe, too.
                    let home = Path::new("/home/u");
                    for it in r.recent(10, home).iter().chain(&r.search(&Query::new("a", 0), home)) {
                        assert_safe_item(it);
                    }
                }
            }
        }

        #[test]
        fn xbel_dates_never_overflow_or_panic(s in nasty_string(60)) {
            let t = recent::parse_date(&s);
            prop_assert!(t >= 0);
        }
    }

    // ---- Explorer's hits and KRunner's matches ----

    proptest! {

        #[test]
        fn explorer_hits_are_rebuilt_from_a_clean_path(
            uri in href(), name in nasty_string(40), kind in nasty_string(10), mime in nasty_string(30),
            icon in icon_text(), mtime in any::<i64>(), size in any::<u64>(), score in any::<f64>(),
        ) {
            cap_cases!(5000);
            let hit = ExplorerHit { uri, name, kind, mime, icon, mtime, size, score };
            let dbg = format!("{:?}", ExplorerHit { uri: format!("file:///{CANARY}"), ..hit.clone() });
            prop_assert!(!dbg.contains(CANARY));
            if let Some(it) = explorer_item(hit.clone(), Path::new("/home/u")) {
                assert_safe_item(&it);
                prop_assert!(it.id.starts_with("file:file:///"));
                prop_assert!((0.0..=1.0).contains(&it.score));
                // The shown name is the path's own file name, never the hit's `name`.
                let Action::OpenFile { uri } = &it.action else { panic!() };
                let path = pct_decode(&uri["file://".len()..]);
                let file = Path::new(&path).file_name().unwrap().to_str().unwrap().to_owned();
                prop_assert_eq!(it.title.clone(), clean_display(&file));
            }
        }

        #[test]
        fn runner_matches_are_checked(
            runner in prop_oneof![ "[A-Za-z0-9._-]{1,40}", nasty_string(20)],
            mid in nasty_string(40), text in nasty_string(80), sub in nasty_string(100),
            icon in icon_text(), rel in any::<f64>(),
        ) {
            cap_cases!(5000);
            let m = RunnerMatch { runner_id: runner, match_id: mid, text, subtext: sub, icon, relevance: rel };
            let dbg = format!("{:?}", RunnerMatch { text: CANARY.into(), match_id: CANARY.into(), ..m.clone() });
            prop_assert!(!dbg.contains(CANARY));
            if let Some(it) = runner_item(m.clone()) {
                assert_safe_item(&it);
                prop_assert!((0.0..=1.0).contains(&it.score));
                prop_assert!(it.title.chars().count() <= 256 && it.subtitle.chars().count() <= 512);
            }
        }
    }

    // ---- the Settings index ----

    fn json_text() -> impl Strategy<Value = String> {
        nasty_string(24)
    }

    proptest! {
        #[test]
        fn settings_index_parse_is_safe(
            raw in proptest::collection::vec(any::<u8>(), 0..400),
            entries in proptest::collection::vec(
                (prop_oneof!["[a-z0-9/-]{1,20}", nasty_string(20)], json_text(), json_text(), icon_text(),
                 proptest::collection::vec(json_text(), 0..4), prop_oneof![Just("page"), Just("setting"), Just("x")]),
                0..6),
            version in prop_oneof![Just(1i64), Just(2i64), any::<i64>()],
        ) {
            let locales = vec!["C".to_owned()];
            let entries_json: Vec<serde_json::Value> = entries.iter().map(|(link, title, section, icon, kw, kind)| {
                serde_json::json!({
                    "link": link, "kind": kind, "icon": icon,
                    "title": { "C": title }, "section": { "C": section }, "parent": { "C": section },
                    "keywords": { "C": kw },
                })
            }).collect();
            let doc = serde_json::json!({ "version": version, "entries": entries_json }).to_string();
            for input in [doc.into_bytes(), raw] {
                if let Ok(idx) = SettingsIndex::parse(&input, &locales) {
                    prop_assert!(idx.len() <= settings_index::MAX_ENTRIES);
                    for e in idx.entries() {
                        prop_assert!(valid_link(&e.link), "{:?}", e.link);
                        prop_assert!(clean_shown(&e.title) && !e.title.is_empty());
                        prop_assert!(clean_shown(&e.subtitle));
                        prop_assert!(valid_icon(&e.icon));
                    }
                    for it in idx.search(&Query::new("a", 0)) {
                        assert_safe_item(&it);
                    }
                }
            }
        }

        #[test]
        fn settings_links_are_exactly_the_pattern(s in nasty_string(60)) {
            let pattern_ok = !s.is_empty() && s.len() <= 128 && s.split('/').all(|seg| {
                let b = seg.as_bytes();
                !b.is_empty() && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
                    && b.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            });
            prop_assert_eq!(valid_link(&s), pattern_ok);
        }
    }

    // ---- the calculator ----

    fn calc_text() -> impl Strategy<Value = String> {
        let alphabet: Vec<&'static str> = vec![
            "0",
            "1",
            "2",
            "7",
            "9",
            "99999999999999999999",
            ".",
            ",",
            "e",
            "E",
            "e999",
            "+",
            "-",
            "*",
            "/",
            "^",
            "%",
            "(",
            ")",
            " ",
            "pi",
            "π",
            "sqrt",
            "sin",
            "ln",
            "exp",
            "log",
            "abs",
            "floor",
            "round",
            "of",
            "km",
            "mi",
            "kg",
            "lb",
            "°C",
            "°F",
            "MB",
            "GiB",
            "in",
            "to",
            "→",
            "=",
            "−",
            "×",
            "÷",
            "!",
            "\u{202e}",
            "\n",
            "\0",
        ];
        prop_oneof![
            6 => proptest::collection::vec(proptest::sample::select(alphabet), 0..80)
                .prop_map(|v| v.concat()),
            2 => nasty_string(300),
        ]
    }

    proptest! {
        #[test]
        fn calculator_never_panics_and_answers_stay_small(s in calc_text(), comma in any::<bool>()) {
            if let Some(c) = calc::evaluate(&s, if comma { ',' } else { '.' }) {
                prop_assert!(c.value.is_finite());
                prop_assert!(c.display.len() <= 32, "{:?}", c.display);
                prop_assert!(c.display.bytes().all(|b| b.is_ascii_digit() || b"-.,e+".contains(&b)));
                prop_assert!(c.expression.chars().count() <= 1024);
                let row = c.to_result();
                prop_assert!(row.title.chars().count() <= 1100);
                assert!(matches!(row.action, Action::Copy { .. }));
                assert!(!format!("{c:?}").contains(&c.display) || c.display.len() < 3);
            }
        }
    }

    // ---- web search ----

    proptest! {
        #[test]
        fn web_urls_are_https_and_percent_encoded(s in prop_oneof![nasty_string(150), (nasty_string(20), 480usize..560).prop_map(|(a, n)| a + &"\u{e9}".repeat(n))], engine in proptest::sample::select(vec![
            Engine::DuckDuckGo, Engine::Google, Engine::Bing, Engine::Startpage, Engine::Ecosia, Engine::Brave, Engine::Kagi, Engine::Qwant,
        ])) {
            let url = engine.search_url(&s);
            assert_safe_action(&Action::OpenWeb { url: url.clone() });
            let prefix_end = url.find("q=").unwrap() + 2;
            let (prefix, q) = url.split_at(prefix_end);
            prop_assert!(prefix.starts_with("https://") && prefix.matches('?').count() == 1);
            prop_assert!(q.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~%".contains(&b)));
            prop_assert!(q.len() <= web::MAX_QUERY_BYTES * 3);
            // It decodes to the query, cut at the cap at a character boundary.
            let decoded = pct_decode(q);
            prop_assert!(s.starts_with(&decoded) && decoded.len() <= web::MAX_QUERY_BYTES);
            prop_assert!(s.len() <= web::MAX_QUERY_BYTES || decoded.len() > web::MAX_QUERY_BYTES - 4);
            // The row from a query.
            let row = web::web_row(engine, &Query::new(&s, 1));
            if let Some(r) = row {
                assert_safe_item(&r);
            }
            let enc = web::percent_encode(&s);
            prop_assert_eq!(pct_decode(&enc), s);
        }
    }

    // ---- typed commands ----

    fn path_cache() -> &'static Arc<PathCache> {
        use std::sync::OnceLock;
        static CACHE: OnceLock<(tempfile::TempDir, Arc<PathCache>)> = OnceLock::new();
        &CACHE
            .get_or_init(|| {
                let dir = tempfile::tempdir().unwrap();
                // A folder name with a doubled space and a bidi override: the subtitle
                // of a Run row shows the path, cleaned; the action keeps it as it is.
                let bin = dir.path().join("my  bin\u{202e}dir");
                fs::create_dir_all(&bin).unwrap();
                for name in [
                    "ls",
                    "echo",
                    "a b",
                    "x;y",
                    "--help",
                    "-rf",
                    "$(id)",
                    "tab\there",
                    "un\u{202e}safe",
                    "q'uote",
                    "nl\nname",
                ] {
                    let p = bin.join(name);
                    fs::write(&p, "#!/bin/sh\n").unwrap();
                    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
                }
                let cache = Arc::new(PathCache::scan(&format!(
                    "relative/dir:{}:/nonexistent",
                    bin.display()
                )));
                (dir, cache)
            })
            .1
    }

    proptest! {
        #[test]
        fn command_lines_split_without_a_shell(line in nasty_string(100)) {
            match parse_command_line(&line) {
                CommandPlan::Direct { executable, args } => {
                    prop_assert!(!line.contains('\n') && !line.contains('\r'));
                    prop_assert!(!executable.is_empty() || line.contains(['\'', '"']));
                    // No word is longer than the line it came from.
                    prop_assert!(args.iter().all(|a| a.len() <= line.len()));
                    // A line without quotes or escapes splits as split_whitespace does,
                    // and has no shell syntax in it.
                    if !line.contains(['\'', '"', '\\']) {
                        let words: Vec<&str> = line.split([' ', '\t']).filter(|w| !w.is_empty()).collect();
                        prop_assert_eq!(1 + args.len(), words.len());
                        assert!(!line.contains(['|', ';', '&', '$', '>', '<', '`', '(', ')', '{', '}', '*', '?', '~', '[']));
                        prop_assert!(!line.trim_start().starts_with('#'));
                    }
                }
                CommandPlan::Empty => prop_assert!(line.trim_matches([' ', '\t']).is_empty() || line.contains(['\'', '"'])),
                CommandPlan::TerminalOnly => {}
            }
        }

        #[test]
        fn a_typed_command_runs_a_path_executable_or_nothing(line in nasty_string(120), prefix in prop_oneof![Just(""), Just("ls "), Just("echo "), Just("x;y "), Just("-rf "), Just("'a b' "), Just("\"my bin\" ")]) {
            let q = Query::new(&format!("{prefix}{line}"), 1);
            let rows = path_cache().search(&q);
            prop_assert!(rows.len() <= 2);
            for it in &rows {
                assert_safe_item(it);
                match &it.action {
                    Action::Run { executable, .. } => {
                        // What runs is the file the scan found, by absolute path.
                        assert!(executable.contains("/my  bin\u{202e}dir/"));
                        prop_assert!(q.raw.split_whitespace().next().is_some());
                    }
                    Action::RunInTerminal { line } => prop_assert_eq!(line, q.raw.trim()),
                    other => prop_assert!(false, "unexpected {:?}", other),
                }
                // The shown line is what was typed (nothing invisible, nothing cut).
                prop_assert!(q.raw.chars().all(|c| c == ' ' || (!c.is_whitespace() && !is_unsafe_char(c))));
                prop_assert!(it.title.contains(q.raw.trim()));
            }
            // The canary typed after a command is only in the rows of that command.
            let c = Query::new(&format!("ls {CANARY}"), 1);
            for it in path_cache().search(&c) {
                assert!(!format!("{it:?}").contains(CANARY));
                assert!(!format!("{:?}", it.action).contains(CANARY));
            }
        }
    }

    // ---- the whole instant phase, over hostile data ----

    proptest! {

        #[test]
        fn every_row_of_a_query_is_safe_to_run(
            entries in proptest::collection::vec(app_entry(), 0..3),
            xbel_entries in proptest::collection::vec((href(), nasty_string(16), nasty_string(8)), 0..3),
            q in prop_oneof![nasty_string(24), calc_text(), Just("ls -la".to_owned()), Just("lock".to_owned()), Just("a".to_owned())],
            web in any::<bool>(),
        ) {
            cap_cases!(1000);
            let recent = RecentFiles::parse(xbel(&xbel_entries).as_bytes()).unwrap_or_default();
            let sources = Sources {
                catalog: Arc::new(Catalog::new(entries)),
                settings: None,
                recent: Arc::new(recent),
                path: Arc::clone(path_cache()),
                session: Arc::new(SessionCommands::new(SessionAvailability::default())),
                home: PathBuf::from("/home/u"),
            };
            let opts = SearchOptions { web: web.then_some(Engine::DuckDuckGo), ..SearchOptions::default() };
            let query = Query::new(&q, 3);
            let usage = UsageStore::default();
            let cx = Context { usage: &usage, opts: &opts, query: &query, now: 1_700_000_000 };
            let rows = query::instant(&sources, &cx);
            prop_assert!(rows.len() <= MAX_RESULTS);
            let mut ids = std::collections::HashSet::new();
            for it in &rows {
                assert_safe_item(it);
                prop_assert!(ids.insert(it.id.clone()), "duplicate row id");
                // Debug of a row shows no text.
                let dbg = format!("{it:?}");
                prop_assert!(!dbg.contains(&it.title) || it.title.len() < 4);
            }
        }
    }

    // ---- file helpers ----

    proptest! {
        #[test]
        fn write_then_read_returns_the_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..4096), cap in 0u64..5000) {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("f");
            write_atomic(&p, &bytes, 0o600).unwrap();
            prop_assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
            match read_capped(&p, cap) {
                Ok(Some(b)) => { prop_assert!(bytes.len() as u64 <= cap); prop_assert_eq!(b, bytes); }
                Err(e) => { prop_assert!(bytes.len() as u64 > cap); prop_assert_eq!(e.kind(), std::io::ErrorKind::InvalidData); }
                Ok(None) => prop_assert!(false, "the file exists"),
            }
            // The nofollow writer replaces it the same way and keeps the mode.
            write_atomic_nofollow(&p, b"new", 0o644).unwrap();
            prop_assert_eq!(fs::read(&p).unwrap(), b"new".to_vec());
            prop_assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }

        #[test]
        fn odd_names_in_the_writer_do_not_escape_the_folder(name in nasty_string(80)) {
            let d = tempfile::tempdir().unwrap();
            let sub = d.path().join("sub");
            fs::create_dir(&sub).unwrap();
            let target = sub.join(&name);
            // `join` can make the path leave `sub` ("..", "/x"); then the folder
            // is the writer's business, not the name's: only a plain file name
            // is a promise. Names without '/' and not "." / ".." stay inside.
            let plain = !name.is_empty() && !name.contains(['/', '\0']) && name != "." && name != "..";
            let r = write_atomic_nofollow(&target, b"x", 0o600);
            if plain {
                let r2 = r.is_ok();
                if r2 {
                    prop_assert!(sub.join(&name).is_file());
                    let n = fs::read_dir(&sub).unwrap().count();
                    prop_assert_eq!(n, 1);
                }
                let sibling = fs::read_dir(d.path()).unwrap().count();
                prop_assert_eq!(sibling, 1);
            }
        }
    }
}

// ---------------------------------------------------------------- the corpus

mod corpus {
    use super::*;

    /// The named worst cases, each of which broke (or could have broken)
    /// something in a parser like these.
    fn nasty_strings() -> Vec<String> {
        vec![
            String::new(),
            "\0".into(),
            "a\0b".into(),
            "\n".into(),
            "..".into(),
            "../../etc/passwd".into(),
            "-rf".into(),
            "--help".into(),
            "\u{202e}gpj.exe".into(),
            "\u{2066}\u{2069}".into(),
            "a\u{200b}b".into(),
            "line1\r\nExec=/bin/sh\r\n".into(),
            "$(id)".into(),
            "`id`".into(),
            "%00%2e%2e%2f".into(),
            "\u{1b}[2J\u{1b}[31m".into(),
            "\u{feff}BOM".into(),
            "x".repeat(10 * 1024 * 1024),
            "é".repeat(2 * 1024 * 1024),
            "a ".repeat(1024 * 1024),
            "\u{301}".repeat(1024 * 1024),
            "\n".repeat(1024 * 1024),
            "9".repeat(100_000),
            "(".repeat(100_000),
            "a".repeat(100_000) + "\n",
        ]
    }

    #[test]
    fn cleaners_survive_a_10_mb_string_in_bounded_time() {
        let start = Instant::now();
        for s in nasty_strings() {
            let d = clean_display(&s);
            assert!(d.chars().count() <= MAX_DISPLAY_CHARS && clean_shown(&d));
            let n = clean_name(&s);
            assert!(n.chars().count() <= MAX_NAME_CHARS && clean_shown(&n));
            let q = Query::new(&s, 0);
            assert!(q.raw.chars().count() <= MAX_QUERY_CHARS);
            let _ = fold(&q.raw);
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn calculator_worst_cases_are_refused_quickly() {
        let start = Instant::now();
        let cases = [
            "9^9^9^9^9^9^9".to_owned(),
            "9".repeat(256),
            format!("1{}", "0".repeat(250)),
            "(".repeat(255),
            format!("{}1{}", "(".repeat(100), ")".repeat(100)),
            "-".repeat(255),
            "sin ".repeat(60) + "1",
            "1e999999999999999999999".to_owned(),
            "10^400".to_owned(),
            "0/0".to_owned(),
            "1/0".to_owned(),
            "sqrt(-1)".to_owned(),
            "ln(0)".to_owned(),
            "2^0.5^1000".to_owned(),
            "1+".repeat(120) + "1",
            "100000000000 km in lightyear".to_owned(),
            "1e308 * 10".to_owned(),
            "1e308 GiB to B".to_owned(),
        ];
        for c in &cases {
            if let Some(r) = calc::evaluate(c, '.') {
                assert!(r.value.is_finite() && r.display.len() <= 32, "{c}");
            }
        }
        // Nothing refused is slow, nothing accepted is long.
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    fn hostile_apps(n: usize) -> Vec<AppEntry> {
        let long = "word ".repeat(100);
        (0..n)
            .map(|i| AppEntry {
                desktop_id: format!("app{i}.desktop"),
                name: long.clone(),
                original_name: long.clone(),
                generic_name: long.clone(),
                comment: "c ".repeat(300),
                keywords: vec!["k ".repeat(30); 64],
                icon: "x".repeat(200),
                exec_name: "e".repeat(300),
                categories: vec!["Utility".into(); 64],
                actions: (0..32)
                    .map(|a| AppActionEntry {
                        id: format!("a{a}"),
                        name: "act ".repeat(50),
                        icon: String::new(),
                    })
                    .collect(),
                flatpak_id: None,
                installed_unix: 0,
            })
            .collect()
    }

    /// Builds a catalogue of `n` apps with every field at its worst and
    /// searches it with the worst queries; the bounds are "does not run away"
    /// (debug builds on shared machines), the numbers are printed.
    fn hostile_catalogue_is_bounded(n: usize) {
        let build = Instant::now();
        let cat = Catalog::new(hostile_apps(n));
        assert_eq!(cat.len(), n);
        let built = build.elapsed();
        let worst = Query::new(&"word ".repeat(51), 1);
        let t = Instant::now();
        let hits = cat.search(&worst);
        let searched = t.elapsed();
        let t = Instant::now();
        let hits2 = cat.search(&Query::new("wrd", 2));
        let fuzzy = t.elapsed();
        eprintln!(
            "{n} hostile apps: built in {built:?}, a 51-word query {searched:?} ({} hits), a typo {fuzzy:?} ({} hits)",
            hits.len(),
            hits2.len()
        );
        assert!(built < Duration::from_secs(120), "{built:?}");
        assert!(searched < Duration::from_secs(60), "{searched:?}");
        assert!(fuzzy < Duration::from_secs(60), "{fuzzy:?}");
    }

    #[test]
    fn two_thousand_hostile_apps_search_in_bounded_time() {
        hostile_catalogue_is_bounded(2_000);
    }

    /// The catalogue's cap (10,000 apps, every field full): run with
    /// `cargo test --release -p telamon-launcher-core --test props -- --ignored --nocapture`.
    #[test]
    #[ignore = "slow in a debug build; the numbers are in docs/SECURITY.md"]
    fn ten_thousand_hostile_apps_search_in_bounded_time() {
        hostile_catalogue_is_bounded(10_000);
    }

    #[test]
    fn a_huge_xbel_is_refused_and_a_deep_one_cut() {
        let big = vec![b'a'; recent::MAX_BYTES + 1];
        assert!(RecentFiles::parse(&big).is_err());
        // Nesting past the cap keeps what came before.
        let deep = format!(
            "<xbel><bookmark href=\"file:///a/ok\"/>{}{}",
            "<x>".repeat(100),
            "</x>".repeat(100)
        );
        let r = RecentFiles::parse(deep.as_bytes()).unwrap();
        assert!(r.malformed);
        assert_eq!(r.items.len(), 1);
        // Entity tricks: custom entities are never expanded.
        let bomb = "<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY a \"aaaaaaaaaaaaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;\"><!ENTITY c \"&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;\"><!ENTITY d \"&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;\">]><xbel><bookmark href=\"file:///tmp/&d;\"/><bookmark href=\"file:///ok\"/></xbel>";
        let r = RecentFiles::parse(bomb.as_bytes()).unwrap();
        assert!(r.items.iter().all(|f| !f.uri.contains("aaaa")));
        // 10,000+ bookmarks: counted, cut.
        let many: String = (0..12_000)
            .map(|i| format!("<bookmark href=\"file:///f/{i}\"/>"))
            .collect();
        let r = RecentFiles::parse(format!("<xbel>{many}</xbel>").as_bytes()).unwrap();
        assert!(r.malformed && r.items.len() <= recent::MAX_KEPT);
    }

    #[test]
    fn files_that_are_not_files_are_not_read() {
        let d = tempfile::tempdir().unwrap();
        // A pipe: not waited for.
        let fifo = d.path().join("recently-used.xbel");
        let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: valid C string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let t = Instant::now();
        assert!(RecentFiles::load(&fifo).is_err());
        assert!(SettingsIndex::load(&fifo, &["C".into()]).is_err());
        assert!(read_capped(&fifo, 100).is_err());
        assert!(t.elapsed() < Duration::from_secs(5));
        // A link to a device that never ends is not a regular file.
        let dev = d.path().join("dev");
        std::os::unix::fs::symlink("/dev/zero", &dev).unwrap();
        assert!(read_capped(&dev, 1 << 20).is_err());
        assert!(RecentFiles::load(&dev).is_err());
        // A folder.
        assert!(read_capped(d.path(), 100).is_err());
        // A sparse file larger than the cap is refused by reading, not by trust.
        let big = d.path().join("big");
        let f = fs::File::create(&big).unwrap();
        f.set_len(8 * 1024 * 1024).unwrap();
        assert!(matches!(
            RecentFiles::load(&big),
            Err(recent::RecentError::TooLarge)
        ));
        assert!(matches!(
            SettingsIndex::load(&big, &["C".into()]),
            Err(settings_index::IndexError::TooLarge)
        ));
    }

    #[test]
    fn config_files_have_caps_and_modes() {
        // pinned.list / names.conf / usage.tsv larger than their caps: only the
        // first part is looked at; nothing panics.
        let mut pins = String::new();
        for i in 0..100_000 {
            pins.push_str(&format!("org.example.app{i}.desktop\n"));
        }
        let p = Pins::parse(pins.as_bytes());
        assert!(p.ids().len() <= pins::MAX_PINS);
        // An import is cut at 64, whatever the caller sends.
        let many: Vec<String> = (0..10_000)
            .map(|i| format!("org.example.app{i}.desktop"))
            .collect();
        assert_eq!(Pins::import(&many).ids().len(), pins::MAX_IMPORT);
        let mut names = String::from("[Names]\n");
        for i in 0..100_000 {
            names.push_str(&format!("org.example.app{i}.desktop=Name {i}\n"));
        }
        let n = Names::parse(names.as_bytes());
        assert!(n.len() <= names::MAX_NAMES);
        let mut usage_text = String::new();
        for i in 0..100_000 {
            usage_text.push_str(&format!(
                "ab\tapp:org.example.app{i}.desktop\t1\t1700000000\n"
            ));
        }
        let u = UsageStore::load_at(usage_text.as_bytes(), 1_700_000_000);
        assert!(
            u.len() <= usage::MAX_ENTRIES && u.to_bytes().len() as u64 <= usage::MAX_FILE_BYTES
        );
        // Written files: 0600 for the user's data.
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("cfg/telamon-launcher/names.conf");
        write_atomic(&f, &n.to_bytes(), 0o600).unwrap();
        assert_eq!(
            fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(f.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn the_override_writer_and_the_worst_ids() {
        let d = tempfile::tempdir().unwrap();
        let sys = d.path().join("sys");
        fs::create_dir_all(&sys).unwrap();
        fs::write(
            sys.join("a.desktop"),
            "[Desktop Entry]\nType=Application\nName=A\nExec=a\n",
        )
        .unwrap();
        let dirs = AppDirs {
            user: d.path().join("home/applications"),
            system: vec![sys],
        };
        for id in [
            "../a.desktop",
            "..\0.desktop",
            "/a.desktop",
            "a/../a.desktop",
            ".desktop",
            "a.desktop\n",
            "a.desktop/",
            "-a.desktop",
            "a\u{202e}.desktop",
            &format!("{}.desktop", "a".repeat(300)),
        ] {
            let r = overrides::apply(&dirs, &[id], "Name");
            assert!(
                matches!(r, overrides::Outcome::NoOriginal),
                "{id:?} gave {r:?}"
            );
            // Nothing there to remove: a bad id is refused, a good one that
            // has no file is simply unchanged.
            assert!(
                matches!(
                    overrides::remove(&dirs, id),
                    overrides::Outcome::NoOriginal | overrides::Outcome::Unchanged
                ),
                "{id:?}"
            );
        }
        assert!(!dirs.user.exists(), "nothing was created for a bad id");
        assert_eq!(
            overrides::apply(&dirs, &["a.desktop"], "Fine"),
            overrides::Outcome::Written
        );
        // The user's folder is the only new thing; its parent holds only it.
        assert_eq!(fs::read_dir(d.path().join("home")).unwrap().count(), 1);
    }

    #[test]
    fn hostile_desktop_entry_text_is_cleaned_wherever_it_lands() {
        let evil = "\u{202e}Fire\u{0}fox\u{200b}\n\r\tExec=/bin/sh\u{2028}";
        let e = AppEntry {
            desktop_id: "evil.desktop".into(),
            name: evil.repeat(100),
            original_name: evil.into(),
            generic_name: evil.into(),
            comment: evil.repeat(1000),
            keywords: vec![evil.into(); 500],
            icon: "file:///etc/passwd".into(),
            exec_name: evil.into(),
            categories: vec![evil.into(); 500],
            actions: vec![
                AppActionEntry {
                    id: "../x".into(),
                    name: evil.into(),
                    icon: "https://x/y.png".into()
                };
                100
            ],
            flatpak_id: Some("../../x".into()),
            installed_unix: -1,
        };
        let cat = Catalog::new(vec![e]);
        assert_eq!(cat.len(), 1);
        let got = &cat.a_to_z()[0];
        assert!(clean_shown(&got.name) && got.name.chars().count() <= MAX_DISPLAY_CHARS);
        assert_eq!(got.icon, "application-x-executable");
        assert!(got.actions.is_empty() && got.flatpak_id.is_none());
        assert!(got.keywords.len() <= MAX_KEYWORDS);
        for it in cat.search(&Query::new("fire", 1)) {
            assert_safe_item(&it);
        }
        // An id that is not an id never becomes an app.
        for bad in [
            "/tmp/x.desktop",
            "../x.desktop",
            "x",
            "x.desktop\n",
            "a b.desktop",
        ] {
            let c = Catalog::new(vec![AppEntry {
                desktop_id: bad.into(),
                name: "x".into(),
                ..AppEntry::default()
            }]);
            assert_eq!(c.len(), 0, "{bad:?}");
        }
    }

    #[test]
    fn late_batches_are_cut_before_they_are_ranked() {
        let m = RunnerMatch {
            runner_id: "krunner_x".into(),
            match_id: "id".into(),
            text: "Title".into(),
            subtext: "Sub".into(),
            icon: "x".into(),
            relevance: 0.5,
        };
        assert_eq!(runner_items(vec![m; 100_000]).len(), query::MAX_LATE_BATCH);
        let hit = ExplorerHit {
            uri: "file:///home/u/a".into(),
            ..ExplorerHit::default()
        };
        assert_eq!(
            explorer_items(vec![hit; 100_000], Path::new("/home/u")).len(),
            query::MAX_LATE_BATCH
        );
    }

    #[test]
    fn canary_text_never_reaches_a_debug_print() {
        let q = Query::new(CANARY, 1);
        let item = calc::evaluate("2+2", '.').unwrap().to_result();
        let items = [
            format!("{q:?}"),
            format!("{item:?}"),
            format!(
                "{:?}",
                Action::Run {
                    executable: CANARY.into(),
                    args: vec![CANARY.into()]
                }
            ),
            format!(
                "{:?}",
                Action::RunInTerminal {
                    line: CANARY.into()
                }
            ),
            format!("{:?}", Action::OpenFile { uri: CANARY.into() }),
            format!("{:?}", Action::OpenWeb { url: CANARY.into() }),
            format!(
                "{:?}",
                Action::Runner {
                    runner_id: CANARY.into(),
                    match_id: CANARY.into()
                }
            ),
            format!(
                "{:?}",
                Action::OpenSettings {
                    link: CANARY.into()
                }
            ),
            format!(
                "{:?}",
                Action::Copy {
                    text: CANARY.into()
                }
            ),
            format!("{:?}", parse_command_line(&format!("ls {CANARY}"))),
            format!("{:?}", Prepared::new(CANARY)),
        ];
        for s in items {
            assert!(!s.contains(CANARY), "{s}");
        }
    }
}
