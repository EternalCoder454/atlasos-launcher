//! Telamon Settings' search index (`/usr/share/telamon-settings/search-index.json`,
//! format v1): parsed defensively and searched in the core. The file is
//! untrusted: size, entry count, link syntax and text lengths are capped where
//! it enters (docs/DESIGN.md, "Trust").

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::fsutil::read_capped;
use crate::result::{Action, Kind, ResultItem, prior};
use crate::text::{Prepared, Query, clean_display_max, score_fields, valid_icon};

/// Where Settings installs its index.
pub const DEFAULT_PATH: &str = "/usr/share/telamon-settings/search-index.json";
/// Where AtlasOS Settings (before it was renamed) installed it; read when the
/// new one is not there, for this release.
pub const LEGACY_PATH: &str = "/usr/share/atlas-settings/search-index.json";
/// Largest index read, in bytes.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Most entries accepted.
pub const MAX_ENTRIES: usize = 5_000;
/// Longest link, in bytes.
pub const MAX_LINK_BYTES: usize = 128;
pub const MAX_TITLE_CHARS: usize = 128;
pub const MAX_KEYWORD_CHARS: usize = 64;
pub const MAX_KEYWORDS: usize = 64;

const FALLBACK_ICON: &str = "preferences-system";

#[derive(Debug)]
pub enum IndexError {
    /// Bigger than [`MAX_BYTES`].
    TooLarge,
    /// Not JSON, or not the shape of an index.
    Syntax(String),
    /// A `version` other than 1 (None when missing or not a number).
    UnsupportedVersion(Option<i64>),
    /// More than [`MAX_ENTRIES`] entries.
    TooManyEntries,
    Io(std::io::Error),
}

impl fmt::Display for IndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexError::TooLarge => write!(f, "index is larger than {MAX_BYTES} bytes"),
            IndexError::Syntax(e) => write!(f, "index is not valid: {e}"),
            IndexError::UnsupportedVersion(Some(v)) => write!(f, "index version {v} is not 1"),
            IndexError::UnsupportedVersion(None) => write!(f, "index has no version"),
            IndexError::TooManyEntries => write!(f, "index has more than {MAX_ENTRIES} entries"),
            IndexError::Io(e) => write!(f, "index could not be read: {e}"),
        }
    }
}

impl std::error::Error for IndexError {}

#[derive(Deserialize, Default)]
struct RawEntry {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    title: Option<HashMap<String, String>>,
    #[serde(default)]
    section: Option<HashMap<String, String>>,
    #[serde(default)]
    parent: Option<HashMap<String, String>>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    keywords: Option<HashMap<String, Vec<String>>>,
    #[serde(default)]
    link: Option<String>,
}

/// One page or setting, ready to match.
#[derive(Clone, Debug)]
pub struct IndexEntry {
    pub link: String,
    pub is_page: bool,
    pub title: String,
    /// "Settings › <parent or section>".
    pub subtitle: String,
    pub icon: String,
    title_prepared: Prepared,
    secondary: Vec<Prepared>,
}

/// The parsed index, immutable; build it off the GUI thread.
#[derive(Clone, Debug, Default)]
pub struct SettingsIndex {
    entries: Vec<IndexEntry>,
    /// Entries dropped for a bad link, missing title or wrong shape.
    pub skipped: usize,
}

/// Whether `link` matches `[a-z0-9][a-z0-9-]*(/[a-z0-9][a-z0-9-]*)*` (no
/// segment starts with `-`) and fits 128 bytes.
pub fn valid_link(link: &str) -> bool {
    !link.is_empty()
        && link.len() <= MAX_LINK_BYTES
        && link.split('/').all(|seg| {
            let b = seg.as_bytes();
            b.first()
                .is_some_and(|f| f.is_ascii_lowercase() || f.is_ascii_digit())
                && b.iter()
                    .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

/// The locale list to pick translations with, in gettext's order: the
/// `LANGUAGE` colon list first, then the first set of `LC_ALL`,
/// `LC_MESSAGES` and `LANG`. "de_DE.UTF-8@euro" gives ["de_DE", "de", "C"],
/// and "de_DE:fr" gives ["de_DE", "de", "fr", "C"]. "C" and "POSIX" add
/// nothing. When the first non-empty of the three is exactly "C" or
/// "POSIX", `LANGUAGE` is ignored and the result is ["C"], as glibc does;
/// "C.UTF-8" keeps `LANGUAGE`.
/// Always ends with "C".
pub fn locale_chain(
    language: Option<&str>,
    lc_all: Option<&str>,
    lc_messages: Option<&str>,
    lang: Option<&str>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        let name = name.split(['.', '@']).next().unwrap_or("");
        if name.is_empty()
            || name == "C"
            || name == "POSIX"
            || name.len() > 32
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return;
        }
        let mut push = |s: &str| {
            if out.len() < 16 && !out.iter().any(|o| o == s) {
                out.push(s.to_owned());
            }
        };
        push(name);
        if let Some((lang, _)) = name.split_once(['_', '-']) {
            push(lang);
        }
    };
    let category = [lc_all, lc_messages, lang]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty());
    // glibc's gettext ignores LANGUAGE only for the exact "C" locale
    // (setlocale names POSIX "C"); C.UTF-8 keeps it.
    let is_c = category.is_some_and(|v| v == "C" || v == "POSIX");
    if is_c {
        return vec!["C".to_owned()];
    }
    for src in [language, category].into_iter().flatten() {
        for part in src.split(':').take(16) {
            add(part);
        }
    }
    out.push("C".to_owned());
    out
}

fn pick<'a>(map: &'a HashMap<String, String>, locales: &[String]) -> Option<&'a str> {
    locales
        .iter()
        .map(String::as_str)
        .chain(std::iter::once("C"))
        .find_map(|l| map.get(l))
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
}

impl SettingsIndex {
    /// Parses an index. `locales` is the chain from [`locale_chain`].
    pub fn parse(bytes: &[u8], locales: &[String]) -> Result<SettingsIndex, IndexError> {
        if bytes.len() > MAX_BYTES {
            return Err(IndexError::TooLarge);
        }
        let root: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| IndexError::Syntax(e.to_string()))?;
        let obj = root
            .as_object()
            .ok_or_else(|| IndexError::Syntax("not an object".into()))?;
        match obj.get("version").and_then(serde_json::Value::as_i64) {
            Some(1) => {}
            other => return Err(IndexError::UnsupportedVersion(other)),
        }
        let list = obj
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| IndexError::Syntax("no entries".into()))?;
        if list.len() > MAX_ENTRIES {
            return Err(IndexError::TooManyEntries);
        }
        let mut entries = Vec::with_capacity(list.len());
        let mut skipped = 0;
        let mut seen = std::collections::HashSet::new();
        for v in list {
            match serde_json::from_value::<RawEntry>(v.clone())
                .ok()
                .and_then(|raw| build(raw, locales))
            {
                Some(e) if seen.insert(e.link.clone()) => entries.push(e),
                _ => skipped += 1,
            }
        }
        Ok(SettingsIndex { entries, skipped })
    }

    /// Reads and parses the index at `path`, reading at most the size cap
    /// plus one byte. This is file IO: call it off the GUI thread.
    pub fn load(path: &Path, locales: &[String]) -> Result<SettingsIndex, IndexError> {
        let buf = match read_capped(path, MAX_BYTES as u64) {
            Ok(Some(b)) => b,
            Ok(None) => {
                return Err(IndexError::Io(std::io::ErrorKind::NotFound.into()));
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                return Err(IndexError::TooLarge);
            }
            Err(e) => return Err(IndexError::Io(e)),
        };
        Self::parse(&buf, locales)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[IndexEntry] {
        &self.entries
    }

    /// Pages and settings matching the query, unsorted, scored
    /// `m × prior(Setting)`.
    pub fn search(&self, q: &Query) -> Vec<ResultItem> {
        if q.is_empty() {
            return Vec::new();
        }
        let p = prior(Kind::Setting);
        self.entries
            .iter()
            .filter_map(|e| {
                let m = score_fields(q, &e.title_prepared, &e.secondary, None)?;
                Some(ResultItem {
                    id: format!("setting:{}", e.link),
                    kind: Kind::Setting,
                    title: e.title.clone(),
                    subtitle: e.subtitle.clone(),
                    icon: e.icon.clone(),
                    score: m * p,
                    action: Action::OpenSettings {
                        link: e.link.clone(),
                    },
                })
            })
            .collect()
    }
}

fn build(raw: RawEntry, locales: &[String]) -> Option<IndexEntry> {
    let link = raw.link?;
    if !valid_link(&link) {
        return None;
    }
    let title = clean_display_max(pick(raw.title.as_ref()?, locales)?, MAX_TITLE_CHARS);
    if title.is_empty() {
        return None;
    }
    let section = raw
        .section
        .as_ref()
        .and_then(|m| pick(m, locales))
        .map(|s| clean_display_max(s, MAX_TITLE_CHARS))
        .unwrap_or_default();
    let parent = raw
        .parent
        .as_ref()
        .and_then(|m| pick(m, locales))
        .map(|s| clean_display_max(s, MAX_TITLE_CHARS))
        .unwrap_or_default();
    let is_page = raw.kind.as_deref() == Some("page");
    let shown = if !is_page && !parent.is_empty() {
        &parent
    } else {
        &section
    };
    let subtitle = if shown.is_empty() {
        "Settings".to_owned()
    } else {
        format!("Settings › {shown}")
    };
    let icon = match raw.icon {
        Some(i) if valid_icon(&i) => i,
        _ => FALLBACK_ICON.to_owned(),
    };
    let mut secondary = Vec::new();
    if let Some(kw) = &raw.keywords {
        let mut seen_langs: Vec<&str> = Vec::new();
        for l in locales.iter().map(String::as_str).chain(["C"]) {
            if let Some(words) = kw.get(l)
                && !seen_langs.contains(&l)
                && (seen_langs.is_empty() || l == "C")
            {
                seen_langs.push(l);
                for w in words {
                    if secondary.len() >= MAX_KEYWORDS {
                        break;
                    }
                    let w = clean_display_max(w, MAX_KEYWORD_CHARS);
                    if !w.is_empty() {
                        secondary.push(Prepared::new(&w));
                    }
                }
            }
        }
    }
    for extra in [&section, &parent] {
        if !extra.is_empty() {
            secondary.push(Prepared::new(extra));
        }
    }
    Some(IndexEntry {
        link,
        is_page,
        title_prepared: Prepared::new(&title),
        title,
        subtitle,
        icon,
        secondary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"version":1,"app":"net.eterneon.telamon.settings","extra":true,"entries":[
{"id":"displays","kind":"page","page":"displays","item":null,"title":{"C":"Displays","de":"Anzeigen"},"section":{"C":"Devices","de":"Geräte"},"parent":null,"symbol":"Monitor","icon":"preferences-desktop-display","keywords":{"C":["monitor","screen","resolution","scaling"],"de":["bildschirm"]},"link":"displays","unknown":1},
{"id":"displays/night-light","kind":"setting","page":"displays","item":"night-light","title":{"C":"Night Light"},"section":{"C":"Devices"},"parent":{"C":"Displays"},"symbol":"x","icon":"weather-clear-night","keywords":{"C":["warm","blue light"]},"link":"displays/night-light"}]}"#;

    fn c() -> Vec<String> {
        vec!["C".into()]
    }

    fn q(s: &str) -> Query {
        Query::new(s, 0)
    }

    #[test]
    fn parses_and_searches() {
        let ix = SettingsIndex::parse(SAMPLE.as_bytes(), &c()).unwrap();
        assert_eq!(ix.len(), 2);
        assert_eq!(ix.skipped, 0);
        let r = ix.search(&q("night"));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, "setting:displays/night-light");
        assert_eq!(r[0].subtitle, "Settings › Displays");
        assert_eq!(r[0].icon, "weather-clear-night");
        assert_eq!(r[0].kind, Kind::Setting);
        assert!((r[0].score - 0.9 * 0.85).abs() < 1e-6);
        assert_eq!(
            r[0].action,
            Action::OpenSettings {
                link: "displays/night-light".into()
            }
        );
        // A page shows its section; a keyword finds it.
        let r = ix.search(&q("resolution"));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].subtitle, "Settings › Devices");
        assert!(r[0].score < 0.7);
        assert!(ix.search(&q("")).is_empty());
        assert!(ix.search(&q("zzzzqq")).is_empty());
    }

    #[test]
    fn locale_fallback() {
        let de = locale_chain(None, None, None, Some("de_DE.UTF-8"));
        let ix = SettingsIndex::parse(SAMPLE.as_bytes(), &de).unwrap();
        let r = ix.search(&q("anzeigen"));
        assert_eq!(r[0].title, "Anzeigen");
        assert_eq!(r[0].subtitle, "Settings › Geräte");
        // Title missing in "de" falls back to C; C keywords still match.
        assert_eq!(ix.search(&q("night light"))[0].title, "Night Light");
        assert!(!ix.search(&q("bildschirm")).is_empty());
        assert!(!ix.search(&q("resolution")).is_empty());
    }

    #[test]
    fn locale_chains() {
        let lc = |l, a, m, g| locale_chain(l, a, m, g);
        assert_eq!(
            lc(Some("de_DE.UTF-8@euro"), None, None, None),
            ["de_DE", "de", "C"]
        );
        assert_eq!(
            lc(Some("de_DE:fr"), None, None, None),
            ["de_DE", "de", "fr", "C"]
        );
        assert_eq!(lc(None, None, None, None), ["C"]);
        assert_eq!(
            lc(None, None, None, Some("de_DE.UTF-8")),
            ["de_DE", "de", "C"]
        );
        assert_eq!(lc(Some(""), None, Some("C.UTF-8"), None), ["C"]);
        // The first set of LC_ALL, LC_MESSAGES, LANG wins; LANGUAGE goes first.
        assert_eq!(
            lc(Some("fr"), Some("de_DE.UTF-8"), Some("es"), Some("it")),
            ["fr", "de_DE", "de", "C"]
        );
        assert_eq!(lc(None, Some(""), Some("es"), Some("it")), ["es", "C"]);
        assert_eq!(lc(None, None, None, Some("POSIX")), ["C"]);
        // A C locale makes gettext ignore LANGUAGE.
        assert_eq!(lc(Some("de:fr"), None, None, Some("C")), ["C"]);
        assert_eq!(
            lc(Some("de"), Some("C.UTF-8"), None, Some("fr")),
            ["de", "C"]
        );
        assert_eq!(lc(Some("de"), None, Some("POSIX"), None), ["C"]);
        // An empty LC_ALL is skipped, so LANG decides; a real locale keeps LANGUAGE.
        assert_eq!(lc(Some("de"), Some(""), None, Some("C")), ["C"]);
        assert_eq!(lc(Some("de"), None, None, Some("fr")), ["de", "fr", "C"]);
        assert_eq!(lc(Some("../x:a b"), None, None, None), ["C"]);
    }

    #[test]
    fn rejects_bad_files() {
        assert!(matches!(
            SettingsIndex::parse(b"", &c()),
            Err(IndexError::Syntax(_))
        ));
        assert!(matches!(
            SettingsIndex::parse(b"[1]", &c()),
            Err(IndexError::Syntax(_))
        ));
        assert!(matches!(
            SettingsIndex::parse(br#"{"version":2,"entries":[]}"#, &c()),
            Err(IndexError::UnsupportedVersion(Some(2)))
        ));
        assert!(matches!(
            SettingsIndex::parse(br#"{"entries":[]}"#, &c()),
            Err(IndexError::UnsupportedVersion(None))
        ));
        assert!(matches!(
            SettingsIndex::parse(br#"{"version":1}"#, &c()),
            Err(IndexError::Syntax(_))
        ));
        let big = vec![b' '; MAX_BYTES + 1];
        assert!(matches!(
            SettingsIndex::parse(&big, &c()),
            Err(IndexError::TooLarge)
        ));
        let many = format!(
            r#"{{"version":1,"entries":[{}]}}"#,
            vec!["{}"; MAX_ENTRIES + 1].join(",")
        );
        assert!(matches!(
            SettingsIndex::parse(many.as_bytes(), &c()),
            Err(IndexError::TooManyEntries)
        ));
    }

    #[test]
    fn bad_entries_are_skipped() {
        let links = [
            "Bad",
            "a//b",
            "/a",
            "a/",
            "a b",
            "../x",
            "",
            "a_b",
            "-a",
            "a/-b",
            &"a".repeat(129),
        ];
        let mut entries: Vec<String> = links
            .iter()
            .map(|l| format!(r#"{{"kind":"page","title":{{"C":"T"}},"link":{l:?}}}"#))
            .collect();
        entries.push(r#"{"kind":"page","title":{"C":"No link"}}"#.into());
        entries.push(r#"{"kind":"page","title":{"C":"  "},"link":"ok"}"#.into());
        entries.push(r#"{"kind":"page","title":5,"link":"ok2"}"#.into());
        entries.push(r#"7"#.into());
        entries.push(
            r#"{"kind":"page","title":{"C":"Good"},"link":"a/b-c/d9","icon":"../evil"}"#.into(),
        );
        entries.push(r#"{"kind":"page","title":{"C":"Dup"},"link":"a/b-c/d9"}"#.into());
        let json = format!(r#"{{"version":1,"entries":[{}]}}"#, entries.join(","));
        let ix = SettingsIndex::parse(json.as_bytes(), &c()).unwrap();
        assert_eq!(ix.len(), 1);
        assert_eq!(ix.skipped, entries.len() - 1);
        assert_eq!(ix.entries()[0].icon, "preferences-system");
        assert!(valid_link("a-1/b/c"));
        assert!(!valid_link("-x") && !valid_link("a/-b") && !valid_link("a/b/-"));
        assert!(valid_link(&"a".repeat(128)));
    }

    #[test]
    fn text_is_capped_and_cleaned() {
        let kws: Vec<String> = (0..100).map(|i| format!("kw{i}")).collect();
        let json = serde_json::json!({"version":1,"entries":[{
            "kind":"page","link":"x",
            "title":{"C": format!("A\u{202e}B{}", "z".repeat(500))},
            "keywords":{"C": kws}}]});
        let ix = SettingsIndex::parse(json.to_string().as_bytes(), &c()).unwrap();
        let e = &ix.entries()[0];
        assert_eq!(e.title.chars().count(), MAX_TITLE_CHARS);
        assert!(!e.title.contains('\u{202e}'));
        assert_eq!(e.secondary.len(), MAX_KEYWORDS);
    }

    #[test]
    fn load_checks_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("i.json");
        std::fs::write(&p, SAMPLE).unwrap();
        assert_eq!(SettingsIndex::load(&p, &c()).unwrap().len(), 2);
        std::fs::write(&p, vec![b' '; MAX_BYTES + 10]).unwrap();
        assert!(matches!(
            SettingsIndex::load(&p, &c()),
            Err(IndexError::TooLarge)
        ));
        assert!(matches!(
            SettingsIndex::load(&dir.path().join("none"), &c()),
            Err(IndexError::Io(_))
        ));
        // A directory or FIFO is refused, not read (or waited on).
        assert!(matches!(
            SettingsIndex::load(dir.path(), &c()),
            Err(IndexError::Io(_))
        ));
    }

    #[test]
    fn fuzz_no_panic() {
        let mut s = 99_u64;
        let base = SAMPLE.as_bytes();
        for _ in 0..400 {
            let mut b = base.to_vec();
            for _ in 0..(1 + s % 8) {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                if b.is_empty() {
                    break;
                }
                let i = (s >> 33) as usize % b.len();
                match (s >> 20) % 3 {
                    0 => b[i] = (s >> 8) as u8,
                    1 => b.truncate(i.max(1)),
                    _ => {
                        b.remove(i);
                    }
                }
            }
            if let Ok(ix) = SettingsIndex::parse(&b, &c()) {
                let _ = ix.search(&q("night"));
            }
        }
    }
}
