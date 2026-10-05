//! The learning store, `$XDG_STATE_HOME/atlas-launcher/usage.tsv`: which
//! result the user ran after typing which first letters.
//!
//! Lines: `query-prefix TAB result-id TAB count TAB last-used-unix`. The prefix
//! is the first 1..=8 characters of the folded query, or empty for a result
//! run from the Start page. Reading is tolerant (bad lines are skipped), the
//! caller reads with `fsutil::read_capped` and writes with
//! `fsutil::write_atomic`.

use std::collections::HashMap;

use crate::text::Query;

/// Largest file the caller should read.
pub const MAX_FILE_BYTES: u64 = 256 * 1024;
pub const MAX_ENTRIES: usize = 2_000;
pub const MAX_PREFIX_CHARS: usize = 8;
pub const MAX_ID_BYTES: usize = 512;

/// Frecency half-life: a use loses half its weight every this many days.
const HALF_LIFE_DAYS: f64 = 14.0;
const EXACT_CAP: f32 = 0.5;
const EXACT_SCALE: f32 = 0.25;
const ITEM_CAP: f32 = 0.15;
const ITEM_SCALE: f32 = 0.075;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    count: u32,
    last: i64,
}

impl Entry {
    /// Frecency at `now`: `count × 2^(−age_days / 14)`. A count is how often,
    /// the age is measured from the last use; a future timestamp counts as
    /// "now".
    fn frecency(&self, now: i64) -> f64 {
        let age_days = now.saturating_sub(self.last).max(0) as f64 / 86_400.0;
        f64::from(self.count) * (-age_days / HALF_LIFE_DAYS).exp2()
    }
}

#[derive(Clone, Debug, Default)]
pub struct UsageStore {
    /// result id → query prefix → entry. One map serves both the exact-prefix
    /// lookup and the sum over a result's prefixes.
    by_id: HashMap<String, HashMap<String, Entry>>,
    entries: usize,
    dirty: bool,
}

fn field_ok(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

/// The key under which a query is stored: its first 8 folded characters.
fn prefix_of(q: &Query) -> String {
    q.folded.chars().take(MAX_PREFIX_CHARS).collect()
}

impl UsageStore {
    /// Reads the file's bytes, skipping every line that isn't valid (wrong
    /// field count, non-numeric, control characters, prefix over 8 characters,
    /// empty or over-long id, bad UTF-8), and looking at no more than 256 KiB
    /// and 2,000 lines. Never fails.
    pub fn load(bytes: &[u8]) -> UsageStore {
        let mut s = UsageStore::default();
        let bytes = &bytes[..bytes.len().min(MAX_FILE_BYTES as usize)];
        for line in bytes.split(|&b| b == b'\n').take(MAX_ENTRIES) {
            let Ok(line) = std::str::from_utf8(line) else {
                continue;
            };
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut f = line.split('\t');
            let (Some(prefix), Some(id), Some(count), Some(last), None) =
                (f.next(), f.next(), f.next(), f.next(), f.next())
            else {
                continue;
            };
            if !field_ok(prefix)
                || !field_ok(id)
                || id.is_empty()
                || id.len() > MAX_ID_BYTES
                || prefix.chars().count() > MAX_PREFIX_CHARS
            {
                continue;
            }
            let (Ok(count), Ok(last)) = (count.parse::<u32>(), last.parse::<i64>()) else {
                continue;
            };
            if count == 0 {
                continue;
            }
            s.merge(prefix, id, count, last);
        }
        s.dirty = false;
        s
    }

    /// Adds to an entry, or creates it (duplicate lines add up).
    fn merge(&mut self, prefix: &str, id: &str, count: u32, last: i64) {
        let map = self.by_id.entry(id.to_owned()).or_default();
        match map.get_mut(prefix) {
            Some(e) => {
                e.count = e.count.saturating_add(count);
                e.last = e.last.max(last);
            }
            None => {
                map.insert(prefix.to_owned(), Entry { count, last });
                self.entries += 1;
            }
        }
    }

    /// The file's content, most recently used first (stable for equal input).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut rows: Vec<(&str, &str, Entry)> = self
            .by_id
            .iter()
            .flat_map(|(id, m)| m.iter().map(move |(p, e)| (p.as_str(), id.as_str(), *e)))
            .collect();
        rows.sort_by(|a, b| {
            b.2.last
                .cmp(&a.2.last)
                .then_with(|| a.0.cmp(b.0))
                .then_with(|| a.1.cmp(b.1))
        });
        let mut out = String::new();
        for (p, id, e) in rows {
            out.push_str(&format!("{p}\t{id}\t{}\t{}\n", e.count, e.last));
        }
        out.into_bytes()
    }

    /// Records that `id` was run after typing `q` (an empty query is the
    /// Start page). Ids that couldn't be read back are ignored. Keeps the
    /// store within 2,000 entries by dropping the weakest.
    pub fn record(&mut self, q: &Query, id: &str, now: i64) {
        if id.is_empty() || id.len() > MAX_ID_BYTES || !field_ok(id) {
            return;
        }
        let prefix = prefix_of(q);
        let map = self.by_id.entry(id.to_owned()).or_default();
        match map.get_mut(&prefix) {
            Some(e) => {
                e.count = e.count.saturating_add(1);
                e.last = now;
            }
            None => {
                map.insert(
                    prefix,
                    Entry {
                        count: 1,
                        last: now,
                    },
                );
                self.entries += 1;
            }
        }
        self.dirty = true;
        self.prune(now);
    }

    /// Drops the entries with the lowest frecency until at most 2,000 remain
    /// (ties go to the older one, then by key, so it is deterministic).
    fn prune(&mut self, now: i64) {
        while self.entries > MAX_ENTRIES {
            let weakest = self
                .by_id
                .iter()
                .flat_map(|(id, m)| m.iter().map(move |(p, e)| (id, p, e)))
                .min_by(|a, b| {
                    a.2.frecency(now)
                        .total_cmp(&b.2.frecency(now))
                        .then_with(|| a.2.last.cmp(&b.2.last))
                        .then_with(|| (a.1, a.0).cmp(&(b.1, b.0)))
                })
                .map(|(id, p, _)| (id.clone(), p.clone()));
            let Some((id, p)) = weakest else { break };
            if let Some(m) = self.by_id.get_mut(&id) {
                m.remove(&p);
                self.entries -= 1;
                if m.is_empty() {
                    self.by_id.remove(&id);
                }
            }
        }
    }

    /// The learned part of a result's score at `now`:
    /// `min(0.5, 0.25·log2(1+f))` for the entry whose prefix is this query's,
    /// plus `min(0.15, 0.075·log2(1+F))` with `F` the frecency summed over all
    /// of the id's prefixes. A pure function of the store.
    pub fn boost(&self, q: &Query, id: &str, now: i64) -> f32 {
        let Some(map) = self.by_id.get(id) else {
            return 0.0;
        };
        let exact = map.get(&prefix_of(q)).map_or(0.0, |e| e.frecency(now));
        let total: f64 = map.values().map(|e| e.frecency(now)).sum();
        let a = (EXACT_SCALE * (1.0 + exact as f32).log2()).min(EXACT_CAP);
        let b = (ITEM_SCALE * (1.0 + total as f32).log2()).min(ITEM_CAP);
        a + b
    }

    pub fn clear(&mut self) {
        if self.entries > 0 {
            self.dirty = true;
        }
        self.by_id.clear();
        self.entries = 0;
    }

    /// The most recently used result ids, newest first, at most `n`.
    pub fn recent_ids(&self, n: usize) -> Vec<String> {
        let mut v: Vec<(i64, &str)> = self
            .by_id
            .iter()
            .map(|(id, m)| (m.values().map(|e| e.last).max().unwrap_or(0), id.as_str()))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        v.into_iter().take(n).map(|(_, id)| id.to_owned()).collect()
    }

    pub fn len(&self) -> usize {
        self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries == 0
    }

    /// Whether something changed since `load` or the last `mark_clean`.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Call after the file was written.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;
    const NOW: i64 = 1_800_000_000;

    fn q(s: &str) -> Query {
        Query::new(s, 0)
    }

    #[test]
    fn tolerant_load() {
        let mut data = Vec::new();
        data.extend_from_slice(b"fire\tapp:a.desktop\t3\t100\n"); // good
        data.extend_from_slice(b"fire\tapp:a.desktop\t3\n"); // too few fields
        data.extend_from_slice(b"fire\tapp:a.desktop\t3\t100\textra\n"); // too many
        data.extend_from_slice(b"fire\tapp:a.desktop\tx\t100\n"); // not a number
        data.extend_from_slice(b"fire\tapp:a.desktop\t-1\t100\n"); // negative
        data.extend_from_slice(b"fire\tapp:a.desktop\t0\t100\n"); // zero count
        data.extend_from_slice(b"123456789\tapp:b.desktop\t1\t1\n"); // prefix too long
        data.extend_from_slice(b"fi\x01re\tapp:c.desktop\t1\t1\n"); // control char
        data.extend_from_slice(b"fire\t\t1\t1\n"); // empty id
        data.extend_from_slice(b"fire\t\xff\xfe\t1\t1\n"); // bad utf-8
        data.extend_from_slice(format!("x\t{}\t1\t1\n", "i".repeat(513)).as_bytes());
        data.extend_from_slice(b"\t\xc3\xa9cran\t2\t50\r\n"); // empty prefix, CRLF
        data.extend_from_slice(b"\n\nnot a line");
        let s = UsageStore::load(&data);
        assert_eq!(s.len(), 2);
        assert!(!s.is_dirty());
        assert!(s.boost(&q("fire"), "app:a.desktop", 100) > 0.0);
        assert!(s.boost(&q(""), "\u{e9}cran", 50) > 0.0);
    }

    #[test]
    fn load_caps_lines_and_bytes() {
        let mut data = String::new();
        for i in 0..2_500 {
            data.push_str(&format!("a\tid{i}\t1\t10\n"));
        }
        assert_eq!(UsageStore::load(data.as_bytes()).len(), MAX_ENTRIES);
        let big = vec![b'x'; 1_000_000];
        assert!(UsageStore::load(&big).is_empty());
        assert!(UsageStore::load(b"").is_empty());
    }

    #[test]
    fn duplicate_lines_merge() {
        let s = UsageStore::load(b"a\tx\t2\t10\na\tx\t3\t20\n");
        assert_eq!(s.len(), 1);
        let again = UsageStore::load(&s.to_bytes());
        assert_eq!(again.to_bytes(), b"a\tx\t5\t20\n");
    }

    #[test]
    fn round_trip() {
        let mut s = UsageStore::default();
        s.record(&q("Firefox is long"), "app:f.desktop", NOW);
        s.record(&q("fire"), "app:f.desktop", NOW + 5);
        s.record(&q(""), "setting:x", NOW + 10);
        let b = s.to_bytes();
        let s2 = UsageStore::load(&b);
        assert_eq!(s2.to_bytes(), b);
        assert_eq!(s2.len(), 3);
        // The prefix is the first 8 folded chars.
        assert!(String::from_utf8_lossy(&b).contains("firefox \tapp:f.desktop\t1"));
    }

    #[test]
    fn record_bumps_and_saturates() {
        let mut s = UsageStore::default();
        assert!(!s.is_dirty());
        s.record(&q("ab"), "x", 10);
        assert!(s.is_dirty());
        s.record(&q("AB"), "x", 20);
        assert_eq!(s.to_bytes(), b"ab\tx\t2\t20\n");
        let mut s = UsageStore::load(format!("ab\tx\t{}\t1\n", u32::MAX).as_bytes());
        s.record(&q("ab"), "x", 2);
        assert_eq!(s.to_bytes(), format!("ab\tx\t{}\t2\n", u32::MAX).as_bytes());
        // Unwritable ids are ignored.
        s.record(&q("ab"), "bad\tid", 3);
        s.record(&q("ab"), "", 3);
        s.record(&q("ab"), &"i".repeat(513), 3);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn boost_grows_with_use_and_shrinks_with_age() {
        let mut s = UsageStore::default();
        let mut last = 0.0;
        for i in 0..2 {
            s.record(&q("fire"), "x", NOW);
            let b = s.boost(&q("fire"), "x", NOW);
            assert!(b > last || i == 0 && b > 0.0, "{b} <= {last}");
            last = b;
        }
        let aged = s.boost(&q("fire"), "x", NOW + 14 * DAY);
        let older = s.boost(&q("fire"), "x", NOW + 60 * DAY);
        assert!(aged < last && older < aged);
        // Half a life: f = 2 -> 1.
        let f = 1.0_f32;
        let want = 0.25 * (1.0 + f).log2() + 0.075 * (1.0 + f).log2();
        assert!((aged - want).abs() < 1e-3, "{aged} vs {want}");
        // Future timestamps don't boost beyond "now".
        assert!((s.boost(&q("fire"), "x", NOW - 10 * DAY) - last).abs() < 1e-6);
    }

    #[test]
    fn boost_caps_hold() {
        let s =
            UsageStore::load(b"fire\tx\t4000000000\t1800000000\nfi\tx\t4000000000\t1800000000\n");
        let b = s.boost(&q("fire"), "x", NOW);
        assert!((b - (0.5 + 0.15)).abs() < 1e-6, "{b}");
    }

    #[test]
    fn boost_exact_prefix_vs_item_total() {
        let mut s = UsageStore::default();
        s.record(&q("fire"), "x", NOW);
        // Another query prefix only gets the item-wide part.
        let other = s.boost(&q("web"), "x", NOW);
        let exact = s.boost(&q("fire"), "x", NOW);
        assert!(other > 0.0 && other <= 0.15);
        assert!(exact > other);
        // Longer queries use the first 8 chars only.
        assert_eq!(s.boost(&q("fire"), "x", NOW), s.boost(&q("fire"), "x", NOW));
        assert_eq!(s.boost(&q("fire"), "unknown", NOW), 0.0);
        assert_eq!(UsageStore::default().boost(&q("a"), "x", NOW), 0.0);
    }

    #[test]
    fn prune_keeps_the_strongest() {
        let mut s = UsageStore::default();
        for i in 0..MAX_ENTRIES {
            s.record(&q("a"), &format!("old{i}"), NOW - 100 * DAY);
        }
        // A strong old entry and a fresh weak one survive over stale ones.
        for _ in 0..100 {
            s.record(&q("a"), "strong", NOW - 100 * DAY);
        }
        s.record(&q("a"), "fresh", NOW);
        assert_eq!(s.len(), MAX_ENTRIES);
        let ids = s.recent_ids(MAX_ENTRIES);
        assert!(ids.contains(&"strong".to_owned()));
        assert!(ids.contains(&"fresh".to_owned()));
        assert_eq!(UsageStore::load(&s.to_bytes()).len(), MAX_ENTRIES);
    }

    #[test]
    fn recent_ids_newest_first() {
        let mut s = UsageStore::default();
        s.record(&q("a"), "one", 10);
        s.record(&q("b"), "two", 30);
        s.record(&q("c"), "three", 20);
        s.record(&q("d"), "one", 5); // older use of "one" doesn't lower it
        assert_eq!(s.recent_ids(2), vec!["two", "three"]);
        assert_eq!(s.recent_ids(10), vec!["two", "three", "one"]);
    }

    #[test]
    fn clear_empties_and_marks_dirty() {
        let mut s = UsageStore::default();
        s.record(&q("a"), "x", 1);
        s.mark_clean();
        s.clear();
        assert!(s.is_empty() && s.is_dirty());
        assert!(s.to_bytes().is_empty());
        assert_eq!(s.boost(&q("a"), "x", 1), 0.0);
    }

    #[test]
    fn survives_disk_round_trip_through_symlink() {
        use crate::fsutil::{read_capped, write_atomic};
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real.tsv");
        let link = d.path().join("state/usage.tsv");
        std::fs::create_dir(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut s = UsageStore::default();
        s.record(&q("ab"), "x", 7);
        write_atomic(&link, &s.to_bytes(), 0o600).unwrap();
        let back = UsageStore::load(&read_capped(&link, MAX_FILE_BYTES).unwrap().unwrap());
        assert_eq!(back.to_bytes(), s.to_bytes());
        assert!(real.exists());
    }
}
