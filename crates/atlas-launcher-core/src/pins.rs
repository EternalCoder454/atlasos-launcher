//! The pinned apps, `pinned.list`: one desktop file id per line, or a
//! `preferred://` token the C++ side resolves; `#` comments and blank lines
//! are ignored. Reading is tolerant, the caller reads with
//! `fsutil::read_capped` and writes with `fsutil::write_atomic`.

use crate::catalog::valid_desktop_id;

/// Largest file the caller should read.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
pub const MAX_PINS: usize = 256;
/// How many of Andromeda's favourites are taken over.
pub const MAX_IMPORT: usize = 64;

const PREFERRED: [&str; 3] = [
    "preferred://browser",
    "preferred://filemanager",
    "preferred://terminal",
];

/// A desktop file id or one of the `preferred://` tokens.
pub fn valid_pin(id: &str) -> bool {
    valid_desktop_id(id) || PREFERRED.contains(&id)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pins {
    ids: Vec<String>,
    skipped: usize,
}

impl Pins {
    /// Reads the list: comments and blank lines are ignored, lines that are
    /// not a valid pin (or are beyond 256 entries, or beyond the first
    /// 64 KiB) are skipped and counted, repeats are dropped. Never fails.
    pub fn parse(bytes: &[u8]) -> Pins {
        let mut p = Pins::default();
        let bytes = &bytes[..bytes.len().min(MAX_FILE_BYTES as usize)];
        for line in bytes.split(|&b| b == b'\n') {
            let Ok(line) = std::str::from_utf8(line) else {
                p.skipped += 1;
                continue;
            };
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if !valid_pin(line) || p.ids.len() >= MAX_PINS {
                p.skipped += 1;
                continue;
            }
            if !p.ids.iter().any(|i| i == line) {
                p.ids.push(line.to_owned());
            }
        }
        p
    }

    /// The file's content, with a header comment.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut s = String::from(
            "# AtlasOS Launcher: pinned apps, in order.\n\
             # One desktop file id per line, or preferred://browser,\n\
             # preferred://filemanager, preferred://terminal.\n",
        );
        for id in &self.ids {
            s.push_str(id);
            s.push('\n');
        }
        s.into_bytes()
    }

    /// Andromeda's `favoriteApps`: `preferred://` tokens, desktop ids, and
    /// `applications:<id>` (prefix removed); anything else is dropped; at most
    /// 64 are taken.
    pub fn import(list: &[String]) -> Pins {
        let mut p = Pins::default();
        for entry in list {
            let id = entry.strip_prefix("applications:").unwrap_or(entry);
            if !valid_pin(id) || p.ids.iter().any(|i| i == id) {
                p.skipped += 1;
                continue;
            }
            if p.ids.len() >= MAX_IMPORT {
                p.skipped += 1;
                continue;
            }
            p.ids.push(id.to_owned());
        }
        p
    }

    /// Adds `id` at the end. False when invalid, already pinned, or full.
    pub fn pin(&mut self, id: &str) -> bool {
        if !valid_pin(id) || self.is_pinned(id) || self.ids.len() >= MAX_PINS {
            return false;
        }
        self.ids.push(id.to_owned());
        true
    }

    pub fn unpin(&mut self, id: &str) -> bool {
        match self.position(id) {
            Some(i) => {
                self.ids.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.position(id).is_some()
    }

    /// Moves `id` to `index` (past the end means the end). False when it
    /// isn't pinned.
    pub fn move_to(&mut self, id: &str, index: usize) -> bool {
        let Some(from) = self.position(id) else {
            return false;
        };
        let item = self.ids.remove(from);
        self.ids.insert(index.min(self.ids.len()), item);
        true
    }

    pub fn move_left(&mut self, id: &str) -> bool {
        match self.position(id) {
            Some(i) => self.move_to(id, i.saturating_sub(1)),
            None => false,
        }
    }

    pub fn move_right(&mut self, id: &str) -> bool {
        match self.position(id) {
            Some(i) => self.move_to(id, i + 1),
            None => false,
        }
    }

    pub fn move_to_front(&mut self, id: &str) -> bool {
        self.move_to(id, 0)
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    /// Lines the last `parse` or `import` had to skip.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|i| i == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pins(ids: &[&str]) -> Pins {
        let mut p = Pins::default();
        for i in ids {
            assert!(p.pin(i));
        }
        p
    }

    #[test]
    fn parse_tolerates_junk() {
        let data = b"# header\n\norg.kde.dolphin.desktop\n  preferred://browser  \r\nbad line\n\
                     ../x.desktop\norg.kde.dolphin.desktop\npreferred://nothing\n\xff\xfe\nkate.desktop";
        let p = Pins::parse(data);
        assert_eq!(
            p.ids(),
            [
                "org.kde.dolphin.desktop",
                "preferred://browser",
                "kate.desktop"
            ]
        );
        assert_eq!(p.skipped(), 4);
    }

    #[test]
    fn parse_caps() {
        let mut s = String::new();
        for i in 0..300 {
            s.push_str(&format!("a{i}.desktop\n"));
        }
        let p = Pins::parse(s.as_bytes());
        assert_eq!(p.ids().len(), MAX_PINS);
        assert_eq!(p.skipped(), 44);
        // Nothing past 64 KiB is read.
        let mut big = vec![b'#'; MAX_FILE_BYTES as usize];
        big.extend_from_slice(b"\nlate.desktop\n");
        assert!(Pins::parse(&big).ids().is_empty());
        assert!(Pins::parse(b"").ids().is_empty());
    }

    #[test]
    fn round_trip_has_header() {
        let p = pins(&["a.desktop", "preferred://terminal", "b.desktop"]);
        let b = p.to_bytes();
        assert!(b.starts_with(b"# "));
        let back = Pins::parse(&b);
        assert_eq!(back, p);
        assert_eq!(back.skipped(), 0);
    }

    #[test]
    fn pin_unpin() {
        let mut p = Pins::default();
        assert!(p.pin("a.desktop"));
        assert!(!p.pin("a.desktop"));
        assert!(!p.pin("not valid"));
        assert!(!p.pin("preferred://other"));
        assert!(p.is_pinned("a.desktop"));
        assert!(p.unpin("a.desktop"));
        assert!(!p.unpin("a.desktop"));
        assert!(!p.is_pinned("a.desktop"));
        for i in 0..MAX_PINS {
            assert!(p.pin(&format!("p{i}.desktop")));
        }
        assert!(!p.pin("one-more.desktop"));
    }

    #[test]
    fn moves() {
        let mut p = pins(&["a.desktop", "b.desktop", "c.desktop", "d.desktop"]);
        assert!(p.move_to("a.desktop", 2));
        assert_eq!(
            p.ids(),
            ["b.desktop", "c.desktop", "a.desktop", "d.desktop"]
        );
        assert!(p.move_left("a.desktop"));
        assert!(p.move_left("b.desktop")); // already first: stays
        assert_eq!(
            p.ids(),
            ["b.desktop", "a.desktop", "c.desktop", "d.desktop"]
        );
        assert!(p.move_right("d.desktop")); // already last: stays
        assert!(p.move_right("b.desktop"));
        assert_eq!(
            p.ids(),
            ["a.desktop", "b.desktop", "c.desktop", "d.desktop"]
        );
        assert!(p.move_to_front("d.desktop"));
        assert_eq!(p.ids()[0], "d.desktop");
        assert!(p.move_to("d.desktop", 99));
        assert_eq!(p.ids()[3], "d.desktop");
        assert!(!p.move_to("zz.desktop", 0));
        assert!(!p.move_left("zz.desktop"));
        assert!(!p.move_right("zz.desktop"));
        assert_eq!(p.ids().len(), 4);
    }

    #[test]
    fn import_andromeda() {
        let list: Vec<String> = [
            "preferred://browser",
            "org.kde.dolphin.desktop",
            "applications:org.kde.kate.desktop",
            "applications:org.kde.kate.desktop",
            "file:///usr/share/applications/x.desktop",
            "preferred://unknown",
            "menu:/x",
            "",
        ]
        .map(String::from)
        .into();
        let p = Pins::import(&list);
        assert_eq!(
            p.ids(),
            [
                "preferred://browser",
                "org.kde.dolphin.desktop",
                "org.kde.kate.desktop"
            ]
        );
        let many: Vec<String> = (0..100).map(|i| format!("a{i}.desktop")).collect();
        assert_eq!(Pins::import(&many).ids().len(), MAX_IMPORT);
        assert!(Pins::import(&[]).ids().is_empty());
    }

    #[test]
    fn disk_round_trip_into_missing_dir() {
        use crate::fsutil::{read_capped, write_atomic};
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("cfg/atlas-launcher/pinned.list");
        let p = pins(&["a.desktop"]);
        write_atomic(&f, &p.to_bytes(), 0o644).unwrap();
        let back = Pins::parse(&read_capped(&f, MAX_FILE_BYTES).unwrap().unwrap());
        assert_eq!(back, p);
    }
}
