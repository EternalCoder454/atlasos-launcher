//! The user's own names for apps, `names.conf`: what the launcher shows
//! instead of an app's name, in the grid, the pins, the recent chips and the
//! search results. This file is the record of them; [`crate::overrides`]
//! writes each one to a desktop file of the same id, so the dock and menus
//! show it too (the system file is never edited, so an app keeps following
//! its own updates).
//!
//! The file is a small ini, written by the launcher (and readable by hand):
//!
//! ```text
//! [Names]
//! org.kde.dolphin.desktop=My Files
//! ```
//!
//! Reading is tolerant and capped, like `pinned.list`: the caller reads with
//! `fsutil::read_capped` and writes with `fsutil::write_atomic`. A name is
//! untrusted text wherever it enters (the panel's editor, a hand-edited file),
//! so every one goes through [`clean_name`]. A name for an app that is not
//! installed is kept, ignored, and shows again when the app comes back (as
//! pins do); nothing here logs ids or names.

use std::collections::BTreeMap;
use std::fmt;

use crate::catalog::{AppEntry, valid_desktop_id};
use crate::legacy::{canonical_desktop_id, desktop_id_alias};
use crate::text::is_unsafe_char;

/// Largest file the caller should read.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Most names kept. With the longest id and name, that still fits the file cap.
pub const MAX_NAMES: usize = 400;
/// Longest name, in characters.
pub const MAX_NAME_CHARS: usize = 64;

const GROUP: &str = "[Names]";

/// A name made safe to show: control, bidi and invisible characters removed
/// (tabs and line breaks count as spaces), runs of spaces made one, the ends
/// trimmed, and at most [`MAX_NAME_CHARS`] characters kept. Empty when
/// nothing is left, which means "no custom name".
pub fn clean_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_NAME_CHARS * 4));
    let mut count = 0;
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if is_unsafe_char(c) {
            continue;
        }
        let needed = if pending_space { 2 } else { 1 };
        if count + needed > MAX_NAME_CHARS {
            break;
        }
        if pending_space {
            out.push(' ');
            count += 1;
            pending_space = false;
        }
        out.push(c);
        count += 1;
    }
    out
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Names {
    map: BTreeMap<String, String>,
    skipped: usize,
}

/// Redacted: a count, never an id or a name.
impl fmt::Debug for Names {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Names")
            .field("len", &self.map.len())
            .finish_non_exhaustive()
    }
}

impl Names {
    /// Reads the file: the `[Names]` group only; comments, blank lines and
    /// other groups are ignored; lines that are not `id=name` with a valid
    /// desktop id and a name with something left after [`clean_name`], and
    /// those beyond [`MAX_NAMES`] or the first [`MAX_FILE_BYTES`], are skipped
    /// and counted. The last line for an id wins. Never fails.
    pub fn parse(bytes: &[u8]) -> Names {
        let mut n = Names::default();
        let bytes = &bytes[..bytes.len().min(MAX_FILE_BYTES as usize)];
        let mut in_names = false;
        for line in bytes.split(|&b| b == b'\n') {
            let Ok(line) = std::str::from_utf8(line) else {
                n.skipped += 1;
                continue;
            };
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_names = line == GROUP;
                continue;
            }
            if !in_names {
                continue;
            }
            let Some((id, name)) = line.split_once('=') else {
                n.skipped += 1;
                continue;
            };
            if !n.insert(id.trim(), name) {
                n.skipped += 1;
            }
        }
        n
    }

    /// The file's content, with a header comment.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut s = String::from(
            "# Telamon Launcher: your own names for apps. The launcher also writes each one\n\
             # to a desktop file of the same id in ~/.local/share/applications (marked\n\
             # X-Telamon-Renamed) so the dock and menus show it. One desktop file id and\n\
             # its name per line. Delete a line to get the app's own name back.\n",
        );
        s.push_str(GROUP);
        s.push('\n');
        for (id, name) in &self.map {
            s.push_str(id);
            s.push('=');
            s.push_str(name);
            s.push('\n');
        }
        s.into_bytes()
    }

    fn insert(&mut self, id: &str, name: &str) -> bool {
        if !valid_desktop_id(id) {
            return false;
        }
        let name = clean_name(name);
        if name.is_empty() {
            return false;
        }
        // An app of the Atlas names (net.eterneon.atlas.x.desktop) is kept
        // under its Telamon one, as pins are.
        let id = canonical_desktop_id(id);
        if !self.map.contains_key(&id) && self.map.len() >= MAX_NAMES {
            return false;
        }
        self.map.insert(id, name);
        true
    }

    /// Gives `id` the name `raw`, cleaned. True when the list changed (so the
    /// caller saves); false when nothing did: the id is not a valid desktop id,
    /// nothing is left of the name, the list is full, or the app already had
    /// that name.
    pub fn set(&mut self, id: &str, raw: &str) -> bool {
        let before = self.map.get(&canonical_desktop_id(id)).cloned();
        if !self.insert(id, raw) {
            return false;
        }
        self.map.get(&canonical_desktop_id(id)) != before.as_ref()
    }

    /// Removes the name of `id`. False when it had none.
    pub fn reset(&mut self, id: &str) -> bool {
        self.map.remove(&canonical_desktop_id(id)).is_some()
    }

    /// The name set for `id` (under either generation's id).
    pub fn get(&self, id: &str) -> Option<&str> {
        self.map
            .get(id)
            .or_else(|| desktop_id_alias(id).and_then(|a| self.map.get(&a)))
            .map(String::as_str)
    }

    /// Every id with its name, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.map.iter().map(|(i, n)| (i.as_str(), n.as_str()))
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Lines the last `parse` had to skip.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// Puts the names on `apps`: an app with a name set shows it, and keeps
    /// its own in `original_name` (so searching either finds it); one without
    /// shows its own again. Safe to run again on entries it has already
    /// changed, with the same names or others.
    pub fn apply(&self, apps: &mut [AppEntry]) {
        for app in apps {
            let own = if app.original_name.is_empty() {
                std::mem::take(&mut app.name)
            } else {
                std::mem::take(&mut app.original_name)
            };
            match self.get(&app.desktop_id) {
                Some(custom) if custom != own => {
                    app.name = custom.to_owned();
                    app.original_name = own;
                }
                _ => {
                    app.name = own;
                    app.original_name.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> AppEntry {
        AppEntry {
            desktop_id: id.into(),
            name: name.into(),
            ..AppEntry::default()
        }
    }

    #[test]
    fn clean_trims_collapses_and_strips() {
        assert_eq!(clean_name("  My   Files \t"), "My Files");
        assert_eq!(clean_name("a\nb"), "a b");
        assert_eq!(clean_name(""), "");
        assert_eq!(clean_name(" \t\n "), "");
        // Control, bidi override, zero-width and Hangul filler vanish.
        assert_eq!(
            clean_name("My\u{202e}Fi\u{0}les\u{200b}\u{3164}"),
            "MyFiles"
        );
        assert_eq!(clean_name("\u{202e}\u{2066}"), "");
        // Non-Latin text and emoji are kept.
        assert_eq!(clean_name("  Écran 画面 🙂 "), "Écran 画面 🙂");
        // Markup is only text; it is shown as plain text.
        assert_eq!(clean_name("<b>x</b>"), "<b>x</b>");
    }

    #[test]
    fn clean_caps_the_length() {
        let long = "x".repeat(200);
        assert_eq!(clean_name(&long).chars().count(), MAX_NAME_CHARS);
        let words = "ab ".repeat(100);
        let c = clean_name(&words);
        assert!(c.chars().count() <= MAX_NAME_CHARS);
        assert!(!c.ends_with(' '));
        // Exactly at the cap stays whole.
        let exact = "y".repeat(MAX_NAME_CHARS);
        assert_eq!(clean_name(&exact), exact);
        // Multi-byte characters count once.
        let wide = "画".repeat(100);
        assert_eq!(clean_name(&wide).chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn parse_tolerates_junk() {
        let data = "# header\n[Other]\nx.desktop=ignored\n[Names]\n\
                    org.kde.dolphin.desktop = My Files \n\
                    no equals sign\n\
                    ../x.desktop=Bad\n\
                    bad id.desktop=Bad\n\
                    kate.desktop=\n\
                    kate.desktop=\u{202e}\n\
                    org.kde.kate.desktop=Text\u{0} Pad\n\
                    org.kde.kate.desktop=Pad\n\
                    plain=No suffix\n";
        let mut bytes = data.as_bytes().to_vec();
        bytes.extend_from_slice(b"\xff\xfe\n");
        let n = Names::parse(&bytes);
        assert_eq!(n.get("org.kde.dolphin.desktop"), Some("My Files"));
        // The last line for an id wins.
        assert_eq!(n.get("org.kde.kate.desktop"), Some("Pad"));
        assert_eq!(n.get("kate.desktop"), None);
        assert_eq!(n.get("x.desktop"), None);
        assert_eq!(n.len(), 2);
        assert_eq!(n.skipped(), 7);
    }

    #[test]
    fn parse_caps() {
        let mut s = String::from("[Names]\n");
        for i in 0..(MAX_NAMES + 20) {
            s.push_str(&format!("a{i}.desktop=Name {i}\n"));
        }
        let n = Names::parse(s.as_bytes());
        assert_eq!(n.len(), MAX_NAMES);
        assert_eq!(n.skipped(), 20);
        // Nothing past the size cap is read.
        let mut big = b"[Names]\n".to_vec();
        big.extend(std::iter::repeat_n(b'#', MAX_FILE_BYTES as usize));
        big.extend_from_slice(b"\nlate.desktop=Late\n");
        assert!(Names::parse(&big).is_empty());
        assert!(Names::parse(b"").is_empty());
        // The worst file the launcher writes fits the cap it reads with.
        let mut full = Names::default();
        for i in 0..MAX_NAMES {
            let id = format!("{i:0>247}.desktop");
            assert!(full.set(&id, &"画".repeat(MAX_NAME_CHARS)));
        }
        assert!(full.to_bytes().len() as u64 <= MAX_FILE_BYTES);
    }

    #[test]
    fn round_trip_has_header() {
        let mut n = Names::default();
        assert!(n.set("org.kde.dolphin.desktop", "My Files"));
        assert!(n.set("a.desktop", "Zed = Ä"));
        let b = n.to_bytes();
        assert!(b.starts_with(b"# "));
        let back = Names::parse(&b);
        assert_eq!(back.get("a.desktop"), Some("Zed = Ä"));
        assert_eq!(back, n);
        assert_eq!(back.skipped(), 0);
    }

    #[test]
    fn set_and_reset() {
        let mut n = Names::default();
        assert!(n.set("a.desktop", "  One  "));
        assert_eq!(n.get("a.desktop"), Some("One"));
        // Same name again: valid, but no change to save.
        assert!(!n.set("a.desktop", "One"));
        assert!(n.set("a.desktop", "Two"));
        // Invalid id, or nothing left of the name: refused, nothing changes.
        assert!(!n.set("not valid", "X"));
        assert!(!n.set("a.desktop", " \u{202e} "));
        assert_eq!(n.get("a.desktop"), Some("Two"));
        assert!(n.reset("a.desktop"));
        assert!(!n.reset("a.desktop"));
        assert!(n.is_empty());
        for i in 0..MAX_NAMES {
            assert!(n.set(&format!("p{i}.desktop"), "x"));
        }
        assert!(!n.set("one-more.desktop", "x"));
        // A name already there can still change when the list is full.
        assert!(n.set("p0.desktop", "y"));
    }

    #[test]
    fn atlas_ids_are_kept_under_their_telamon_ones() {
        let n = Names::parse(b"[Names]\nnet.eterneon.atlas.store.desktop=Shop\n");
        assert_eq!(n.get("net.eterneon.telamon.store.desktop"), Some("Shop"));
        // The old id finds it too, for an image that has not moved yet.
        assert_eq!(n.get("net.eterneon.atlas.store.desktop"), Some("Shop"));
        let mut m = Names::default();
        assert!(m.set("net.eterneon.atlas.store.desktop", "Shop"));
        assert!(m.reset("net.eterneon.telamon.store.desktop"));
    }

    #[test]
    fn apply_renames_and_keeps_the_original() {
        let mut n = Names::default();
        n.set("b.desktop", "Zebra");
        let mut apps = vec![app("a.desktop", "Alpha"), app("b.desktop", "Beta")];
        n.apply(&mut apps);
        assert_eq!(apps[0].name, "Alpha");
        assert_eq!(apps[0].original_name, "");
        assert_eq!(apps[1].name, "Zebra");
        assert_eq!(apps[1].original_name, "Beta");
        // Again, with the same names: unchanged.
        let once = apps.clone();
        n.apply(&mut apps);
        assert_eq!(apps, once);
        // With another name, then none: the app's own is never lost.
        n.set("b.desktop", "Yak");
        n.set("a.desktop", "Ant");
        n.apply(&mut apps);
        assert_eq!(apps[0].name, "Ant");
        assert_eq!(apps[0].original_name, "Alpha");
        assert_eq!(apps[1].name, "Yak");
        assert_eq!(apps[1].original_name, "Beta");
        Names::default().apply(&mut apps);
        assert_eq!(apps[0].name, "Alpha");
        assert_eq!(apps[1].name, "Beta");
        assert!(apps.iter().all(|a| a.original_name.is_empty()));
    }

    #[test]
    fn a_name_equal_to_the_apps_own_is_no_name() {
        let mut n = Names::default();
        n.set("a.desktop", "Alpha");
        let mut apps = vec![app("a.desktop", "Alpha")];
        n.apply(&mut apps);
        assert_eq!(apps[0].name, "Alpha");
        assert_eq!(apps[0].original_name, "");
    }

    #[test]
    fn debug_is_redacted() {
        let mut n = Names::default();
        n.set("secret.desktop", "My Secret Project");
        let d = format!("{n:?}");
        assert!(!d.contains("ecret") && !d.contains("Project"), "{d}");
    }

    #[test]
    fn disk_round_trip_into_missing_dir() {
        use crate::fsutil::{read_capped, write_atomic};
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("cfg/telamon-launcher/names.conf");
        let mut n = Names::default();
        n.set("a.desktop", "Mine");
        write_atomic(&f, &n.to_bytes(), 0o600).unwrap();
        let back = Names::parse(&read_capped(&f, MAX_FILE_BYTES).unwrap().unwrap());
        assert_eq!(back, n);
    }
}
