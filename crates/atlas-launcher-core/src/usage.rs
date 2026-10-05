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

/// Largest count kept (also the largest the file may hold).
const MAX_COUNT: f64 = 1e6;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    /// Decayed use count as of `last` (a decimal, 3 places in the file).
    count: f64,
    last: i64,
}

/// `2^(−Δ / 14 days)`; a future `last` counts as "now".
fn decay(from: i64, to: i64) -> f64 {
    let age_days = to.saturating_sub(from).max(0) as f64 / 86_400.0;
    (-age_days / HALF_LIFE_DAYS).exp2()
}

/// Rounds to the 3 decimals the file keeps, within `0..=MAX_COUNT`.
fn norm_count(c: f64) -> f64 {
    (c.clamp(0.0, MAX_COUNT) * 1000.0).round() / 1000.0
}

impl Entry {
    /// Frecency at `now`: `count × 2^(−age_days / 14)`, the age measured
    /// from the last use. With the decay applied on every update this
    /// approximates DESIGN's Σ 2^(−age/14 d) over all uses.
    fn frecency(&self, now: i64) -> f64 {
        self.count * decay(self.last, now)
    }
}

/// The count as written: an integer, or up to 3 decimals without trailing
/// zeros.
fn fmt_count(c: f64) -> String {
    let s = format!("{c:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A non-negative decimal of digits and at most one `.` with 1..=3 decimals
/// (no sign, exponent, NaN or inf), in `(0, 1e6]`.
fn parse_count(s: &str) -> Option<f64> {
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    if int.is_empty()
        || int.len() > 7
        || frac.len() > 3
        || (s.contains('.') && frac.is_empty())
        || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let v: f64 = s.parse().ok()?;
    (v.is_finite() && v > 0.0 && v <= MAX_COUNT).then_some(v)
}

/// Only these results may be learned: apps, settings and session commands.
/// Commands typed, files, calculator and web rows would put what the user
/// typed or opened into the file.
pub fn learnable(id: &str) -> bool {
    id.starts_with("app:") || id.starts_with("setting:") || id.starts_with("session:")
}

/// A query's key into the store, computed once per query.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct BoostKey {
    prefix: String,
}

/// Redacted: the prefix is what the user typed.
impl std::fmt::Debug for BoostKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoostKey")
            .field("prefix_chars", &self.prefix.chars().count())
            .finish()
    }
}

#[derive(Clone, Default)]
pub struct UsageStore {
    /// result id → query prefix → entry. One map serves both the exact-prefix
    /// lookup and the sum over a result's prefixes.
    by_id: HashMap<String, HashMap<String, Entry>>,
    entries: usize,
    dirty: bool,
    /// Lines `load` skipped as invalid.
    skipped: usize,
}

/// Redacted: counts only, never ids or prefixes.
impl std::fmt::Debug for UsageStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageStore")
            .field("entries", &self.entries)
            .field("dirty", &self.dirty)
            .field("skipped", &self.skipped)
            .finish()
    }
}

/// How far ahead of now a `last` may be (clock changes, forged files).
const MAX_FUTURE_SECS: i64 = 86_400;

/// A prefix and id the file format can hold and `load` accepts.
fn key_ok(prefix: &str, id: &str) -> bool {
    field_ok(prefix)
        && field_ok(id)
        && !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && learnable(id)
        && prefix.chars().count() <= MAX_PREFIX_CHARS
}

fn field_ok(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

/// The key under which a query is stored: its first 8 folded characters.
fn prefix_of(q: &Query) -> String {
    q.folded.chars().take(MAX_PREFIX_CHARS).collect()
}

/// Bytes one row takes in the file.
fn line_len(prefix: &str, id: &str, e: &Entry) -> usize {
    prefix.len() + id.len() + fmt_count(e.count).len() + digits(e.last) + 4
}

/// Characters of an `i64` in decimal, with the sign.
fn digits(n: i64) -> usize {
    let sign = n < 0;
    let mut n = n.unsigned_abs();
    let mut d = usize::from(n == 0) + usize::from(sign);
    while n > 0 {
        d += 1;
        n /= 10;
    }
    d
}

impl UsageStore {
    /// Reads the file's bytes, skipping every line that isn't valid (wrong
    /// field count, bad count, control characters, prefix over 8 characters,
    /// empty, over-long or not learnable id, bad UTF-8), and looking at no
    /// more than 256 KiB and 2,000 lines. A last line cut by the size cap is
    /// dropped. Never fails.
    pub fn load(bytes: &[u8]) -> UsageStore {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        Self::load_at(bytes, now)
    }

    /// [`UsageStore::load`] with the current time given: a `last` more than
    /// a day after `now` is set to `now` plus a day.
    pub fn load_at(bytes: &[u8], now: i64) -> UsageStore {
        let max_last = now.saturating_add(MAX_FUTURE_SECS);
        let mut s = UsageStore::default();
        let cap = MAX_FILE_BYTES as usize;
        let mut b = &bytes[..bytes.len().min(cap)];
        if bytes.len() >= cap && !b.ends_with(b"\n") {
            // The last line may be cut short: drop it.
            b = match b.iter().rposition(|&c| c == b'\n') {
                Some(i) => &b[..=i],
                None => &[],
            };
        }
        for line in b.split(|&c| c == b'\n').take(MAX_ENTRIES) {
            if line.is_empty() {
                continue;
            }
            let Ok(line) = std::str::from_utf8(line) else {
                s.skipped += 1;
                continue;
            };
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut f = line.split('\t');
            let (Some(prefix), Some(id), Some(count), Some(last), None) =
                (f.next(), f.next(), f.next(), f.next(), f.next())
            else {
                s.skipped += 1;
                continue;
            };
            if !key_ok(prefix, id) {
                s.skipped += 1;
                continue;
            }
            let (Some(count), Ok(last)) = (parse_count(count), last.parse::<i64>()) else {
                s.skipped += 1;
                continue;
            };
            s.merge(prefix, id, count, last.min(max_last));
        }
        s.dirty = false;
        s
    }

    /// How many lines `load` skipped as invalid (empty lines don't count).
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// Adds to an entry, or creates it (duplicate lines add up, each decayed
    /// to the later time).
    fn merge(&mut self, prefix: &str, id: &str, count: f64, last: i64) {
        let map = self.by_id.entry(id.to_owned()).or_default();
        match map.get_mut(prefix) {
            Some(e) => {
                let at = e.last.max(last);
                e.count = norm_count(e.count * decay(e.last, at) + count * decay(last, at));
                e.last = at;
            }
            None => {
                map.insert(
                    prefix.to_owned(),
                    Entry {
                        count: norm_count(count),
                        last,
                    },
                );
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
            out.push_str(&format!("{p}\t{id}\t{}\t{}\n", fmt_count(e.count), e.last));
        }
        out.into_bytes()
    }

    /// Records that `id` was run after typing `q` (an empty query is the
    /// Start page). Only apps, settings and session commands are learned;
    /// anything else, and ids that couldn't be read back, are ignored. The
    /// old count decays to now, then 1 is added. Keeps the store within
    /// 2,000 entries and 256 KiB by dropping the weakest.
    pub fn record(&mut self, q: &Query, id: &str, now: i64) {
        let prefix = prefix_of(q);
        if !key_ok(&prefix, id) {
            return;
        }
        let max_last = now.saturating_add(MAX_FUTURE_SECS);
        let map = self.by_id.entry(id.to_owned()).or_default();
        match map.get_mut(&prefix) {
            Some(e) => {
                e.count = norm_count(e.count * decay(e.last, now) + 1.0);
                // The clock may have gone back: never move `last` backwards.
                e.last = e.last.max(now).min(max_last);
            }
            None => {
                map.insert(
                    prefix,
                    Entry {
                        count: 1.0,
                        last: now.min(max_last),
                    },
                );
                self.entries += 1;
            }
        }
        self.dirty = true;
        self.prune(now);
    }

    /// Drops the entries with the lowest frecency until at most 2,000 remain
    /// and the file would fit in 256 KiB (ties go to the older one, then by
    /// key, so it is deterministic).
    fn prune(&mut self, now: i64) {
        let mut total: usize = self
            .by_id
            .iter()
            .flat_map(|(id, m)| m.iter().map(move |(p, e)| line_len(p, id, e)))
            .sum();
        if self.entries <= MAX_ENTRIES && total <= MAX_FILE_BYTES as usize {
            return;
        }
        let mut rows: Vec<(f64, i64, String, String, usize)> = self
            .by_id
            .iter()
            .flat_map(|(id, m)| {
                m.iter().map(move |(p, e)| {
                    (
                        e.frecency(now),
                        e.last,
                        p.clone(),
                        id.clone(),
                        line_len(p, id, e),
                    )
                })
            })
            .collect();
        rows.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| (&a.2, &a.3).cmp(&(&b.2, &b.3)))
        });
        for (_, _, p, id, len) in rows {
            if self.entries <= MAX_ENTRIES && total <= MAX_FILE_BYTES as usize {
                break;
            }
            if let Some(m) = self.by_id.get_mut(&id) {
                m.remove(&p);
                self.entries -= 1;
                total -= len;
                if m.is_empty() {
                    self.by_id.remove(&id);
                }
            }
        }
    }

    /// The key for [`UsageStore::boost`], once per query.
    pub fn key(&self, q: &Query) -> BoostKey {
        BoostKey {
            prefix: prefix_of(q),
        }
    }

    /// The learned part of a result's score at `now`:
    /// `min(0.5, 0.25·log2(1+f))` for the entry whose prefix is the key's,
    /// plus `min(0.15, 0.075·log2(1+F))` with `F` the frecency summed over all
    /// of the id's prefixes. A pure function of the store. An id never
    /// recorded costs one hash lookup and no allocation.
    pub fn boost(&self, key: &BoostKey, id: &str, now: i64) -> f32 {
        let Some(map) = self.by_id.get(id) else {
            return 0.0;
        };
        let exact = map
            .get(key.prefix.as_str())
            .map_or(0.0, |e| e.frecency(now));
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

    fn bo(s: &UsageStore, query: &str, id: &str, now: i64) -> f32 {
        s.boost(&s.key(&q(query)), id, now)
    }

    #[test]
    fn tolerant_load() {
        let mut data = Vec::new();
        data.extend_from_slice(b"fire\tapp:a.desktop\t3\t100\n"); // good
        data.extend_from_slice(b"fire\tapp:a.desktop\t3\n"); // too few fields
        data.extend_from_slice(b"fire\tapp:a.desktop\t3\t100\textra\n"); // too many
        data.extend_from_slice(b"fire\tapp:a.desktop\tapp:x\t100\n"); // not a number
        data.extend_from_slice(b"fire\tapp:a.desktop\t-1\t100\n"); // negative
        data.extend_from_slice(b"fire\tapp:a.desktop\t0\t100\n"); // zero count
        data.extend_from_slice(b"123456789\tapp:b.desktop\t1\t1\n"); // prefix too long
        data.extend_from_slice(b"fi\x01re\tapp:c.desktop\t1\t1\n"); // control char
        data.extend_from_slice(b"fire\t\t1\t1\n"); // empty id
        data.extend_from_slice(b"fire\t\xff\xfe\t1\t1\n"); // bad utf-8
        data.extend_from_slice(format!("x\tapp:{}\t1\t1\n", "i".repeat(510)).as_bytes());
        data.extend_from_slice(b"\tapp:\xc3\xa9cran\t2\t50\r\n"); // empty prefix, CRLF
        data.extend_from_slice(b"fire\trun:ls\t1\t1\n"); // not learnable
        for bad in [
            "NaN", "inf", "1e3", "1.2345", "1000001", "1.", "+1", "-1", "0.000",
        ] {
            data.extend_from_slice(format!("fire\tapp:d\t{bad}\t1\n").as_bytes());
        }
        data.extend_from_slice(b"\n\nnot a line");
        let s = UsageStore::load(&data);
        assert_eq!(s.len(), 2);
        assert_eq!(s.skipped(), 21);
        assert!(!s.is_dirty());
        assert!(bo(&s, "fire", "app:a.desktop", 100) > 0.0);
        assert!(bo(&s, "", "app:\u{e9}cran", 50) > 0.0);
    }

    #[test]
    fn digits_counts_the_sign() {
        assert_eq!(digits(0), 1);
        assert_eq!(digits(1_234), 4);
        assert_eq!(digits(-1_234), 5);
        assert_eq!(digits(i64::MIN), i64::MIN.to_string().len());
    }

    #[test]
    fn future_and_backwards_clocks() {
        // A forged far-future `last` is clamped to now + 1 day on load.
        let s = UsageStore::load_at(b"a\tapp:x\t1\t99999999999\n", NOW);
        let text = String::from_utf8(s.to_bytes()).unwrap();
        assert_eq!(text, format!("a\tapp:x\t1\t{}\n", NOW + DAY));

        // `record` with the clock gone backwards keeps `last`, and a record
        // far in the future is clamped as well.
        let mut s = UsageStore::default();
        s.record(&q("a"), "app:x", NOW);
        s.record(&q("a"), "app:x", NOW - 1_000);
        let text = String::from_utf8(s.to_bytes()).unwrap();
        assert!(text.ends_with(&format!("\t{NOW}\n")), "{text}");
        let mut s = UsageStore::default();
        s.record(&q("a"), "app:y", NOW + 100 * DAY);
        s.record(&q("a"), "app:y", NOW);
        let text = String::from_utf8(s.to_bytes()).unwrap();
        assert!(text.ends_with(&format!("\t{}\n", NOW + DAY)), "{text}");
        let s = UsageStore::load_at(b"a\tapp:x\t1\t1\n", i64::MAX);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn record_checks_the_prefix_like_load() {
        let mut s = UsageStore::default();
        // A query can't carry control characters; build the prefix by hand
        // through the same check `record` uses.
        assert!(!key_ok("a\tb", "app:x"));
        assert!(!key_ok("a\nb", "app:x"));
        assert!(key_ok("abc", "app:x"));
        s.record(&q("abcdefghijkl"), "app:x", NOW);
        assert_eq!(s.len(), 1);
        // What `record` wrote loads back whole.
        let back = UsageStore::load_at(&s.to_bytes(), NOW);
        assert_eq!((back.len(), back.skipped()), (1, 0));
    }

    #[test]
    fn debug_is_redacted() {
        let mut s = UsageStore::default();
        s.record(&q("secretq"), "app:secret.desktop", NOW);
        let d = format!("{s:?} {:?}", s.key(&q("secretq")));
        assert!(!d.contains("secret"), "{d}");
    }

    #[test]
    fn load_caps_lines_and_bytes() {
        let mut data = String::new();
        for i in 0..2_500 {
            data.push_str(&format!("a\tapp:id{i}\t1\t10\n"));
        }
        assert_eq!(UsageStore::load(data.as_bytes()).len(), MAX_ENTRIES);
        let big = vec![b'x'; 1_000_000];
        assert!(UsageStore::load(&big).is_empty());
        assert!(UsageStore::load(b"").is_empty());
    }

    #[test]
    fn duplicate_lines_merge() {
        let s = UsageStore::load(b"a\tapp:x\t2\t20\na\tapp:x\t3\t20\n");
        assert_eq!(s.len(), 1);
        let again = UsageStore::load(&s.to_bytes());
        assert_eq!(again.to_bytes(), b"a\tapp:x\t5\t20\n");
    }

    #[test]
    fn round_trip() {
        let mut s = UsageStore::default();
        s.record(&q("Firefox is long"), "app:f.desktop", NOW);
        s.record(&q("fire"), "app:f.desktop", NOW + 5);
        s.record(&q(""), "setting:x", NOW + 10);
        let b = s.to_bytes();
        let s2 = UsageStore::load_at(&b, NOW + 10);
        assert_eq!(s2.to_bytes(), b);
        assert_eq!(s2.len(), 3);
        // The prefix is the first 8 folded chars.
        assert!(String::from_utf8_lossy(&b).contains("firefox \tapp:f.desktop\t1"));
    }

    #[test]
    fn record_bumps_and_saturates() {
        let mut s = UsageStore::default();
        assert!(!s.is_dirty());
        s.record(&q("ab"), "app:x", 10);
        assert!(s.is_dirty());
        s.record(&q("AB"), "app:x", 20);
        assert_eq!(s.to_bytes(), b"ab\tapp:x\t2\t20\n");
        let mut s = UsageStore::load(b"ab\tapp:x\t1000000\t1\n");
        s.record(&q("ab"), "app:x", 2);
        assert_eq!(s.to_bytes(), b"ab\tapp:x\t1000000\t2\n");
        // Unwritable ids are ignored.
        s.record(&q("ab"), "bad\tid", 3);
        s.record(&q("ab"), "", 3);
        s.record(&q("ab"), &format!("app:{}", "i".repeat(509)), 3);
        s.record(&q("ab"), "run:ls", 3);
        s.record(&q("ab"), "file:/home/u/x", 3);
        s.record(&q("ab"), "calc", 3);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn boost_grows_with_use_and_shrinks_with_age() {
        let mut s = UsageStore::default();
        let mut last = 0.0;
        for i in 0..2 {
            s.record(&q("fire"), "app:x", NOW);
            let b = bo(&s, "fire", "app:x", NOW);
            assert!(b > last || i == 0 && b > 0.0, "{b} <= {last}");
            last = b;
        }
        let aged = bo(&s, "fire", "app:x", NOW + 14 * DAY);
        let older = bo(&s, "fire", "app:x", NOW + 60 * DAY);
        assert!(aged < last && older < aged);
        // Half a life: f = 2 -> 1.
        // Two uses at the same instant: count 2, so after 14 days f = 1.
        let f = 1.0_f32;
        let want = 0.25 * (1.0 + f).log2() + 0.075 * (1.0 + f).log2();
        assert!((aged - want).abs() < 1e-3, "{aged} vs {want}");
        // Future timestamps don't boost beyond "now".
        assert!((bo(&s, "fire", "app:x", NOW - 10 * DAY) - last).abs() < 1e-6);
    }

    #[test]
    fn only_apps_settings_sessions_are_learned() {
        let mut s = UsageStore::default();
        for id in [
            "run:ls", "term:ls", "file:/x", "runner:a", "calc", "web", "x", "app", "apps:x",
        ] {
            s.record(&q("a"), id, 5);
        }
        assert!(s.is_empty() && !s.is_dirty());
        for id in ["app:a.desktop", "setting:x", "session:lock"] {
            assert!(learnable(id));
            s.record(&q("a"), id, 5);
        }
        assert_eq!(s.len(), 3);
        // Old files with other ids lose those rows on load.
        let old = b"a\trun:ls\t3\t1\na\tfile:/h/secret\t3\t1\na\tcalc\t3\t1\na\tapp:z\t3\t1\n";
        assert_eq!(UsageStore::load(old).to_bytes(), b"a\tapp:z\t3\t1\n");
    }

    #[test]
    fn decay_on_record_approximates_the_sum() {
        let mut s = UsageStore::default();
        s.record(&q("a"), "app:x", 0);
        s.record(&q("a"), "app:x", 14 * DAY);
        // 1 x 0.5 + 1 = 1.5, last = now.
        assert_eq!(
            s.to_bytes(),
            format!("a\tapp:x\t1.5\t{}\n", 14 * DAY).as_bytes()
        );
        let back = UsageStore::load(&s.to_bytes());
        assert_eq!(back.to_bytes(), s.to_bytes());
        let e = back.by_id["app:x"]["a"];
        assert!((e.frecency(14 * DAY + 28 * DAY) - 0.375).abs() < 1e-9);
        // A clock going backwards doesn't blow the count up, and `last`
        // never moves back (it is only cut to now + 1 day).
        s.record(&q("a"), "app:x", 0);
        assert_eq!(s.to_bytes(), format!("a\tapp:x\t2.5\t{DAY}\n").as_bytes());
    }

    #[test]
    fn decimal_counts_parse() {
        let s = UsageStore::load(b"a\tapp:x\t2.5\t9\nb\tapp:x\t0.001\t9\nc\tapp:x\t7\t9\n");
        assert_eq!(s.len(), 3);
        assert!(s.to_bytes().ends_with(b"\t7\t9\n"));
        assert_eq!(parse_count("1000000"), Some(1e6));
        assert_eq!(parse_count(".5"), None);
        assert_eq!(parse_count(""), None);
    }

    #[test]
    fn file_stays_within_the_byte_cap() {
        let mut s = UsageStore::default();
        let long = format!("app:{}", "i".repeat(500));
        for i in 0..1_000 {
            s.record(&q("a"), &format!("{long}{i}"), NOW + i);
        }
        let bytes = s.to_bytes();
        assert!(bytes.len() as u64 <= MAX_FILE_BYTES, "{}", bytes.len());
        assert!(s.len() < 1_000 && s.len() > 400);
        assert!(s.recent_ids(1)[0].ends_with("999"));
        assert_eq!(UsageStore::load(&bytes).len(), s.len());
    }

    #[test]
    fn load_drops_a_line_cut_by_the_cap() {
        let cap = MAX_FILE_BYTES as usize;
        for pad in [25, 26] {
            // pad 25: one byte past the cap; pad 26: exactly at it.
            let mut data = String::from("a\tapp:x\t1\t10\n");
            data.push_str(&"z".repeat(cap - pad));
            data.push_str("\na\tapp:y\t1\t10");
            assert_eq!(data.len(), cap + 26 - pad);
            let s = UsageStore::load(data.as_bytes());
            assert_eq!(s.len(), 1, "pad {pad}");
            assert!(!s.by_id.contains_key("app:y"));
        }
    }

    #[test]
    fn boost_caps_hold() {
        let s =
            UsageStore::load(b"fire\tapp:x\t1000000\t1800000000\nfi\tapp:x\t1000000\t1800000000\n");
        let b = bo(&s, "fire", "app:x", NOW);
        assert!((b - (0.5 + 0.15)).abs() < 1e-6, "{b}");
    }

    #[test]
    fn boost_exact_prefix_vs_item_total() {
        let mut s = UsageStore::default();
        s.record(&q("fire"), "app:x", NOW);
        // Another query prefix only gets the item-wide part.
        let other = bo(&s, "web", "app:x", NOW);
        let exact = bo(&s, "fire", "app:x", NOW);
        assert!(other > 0.0 && other <= 0.15);
        assert!(exact > other);
        // Longer queries use the first 8 chars only.
        s.record(&q("fireworks"), "app:y", NOW);
        assert_eq!(
            bo(&s, "fireworks", "app:y", NOW),
            bo(&s, "fireworkz!", "app:y", NOW)
        );
        // "firewall" has another prefix: only the item-wide part.
        let (a, b) = (
            bo(&s, "fireworks", "app:y", NOW),
            bo(&s, "firewall", "app:y", NOW),
        );
        assert!(a > b && b > 0.0, "{a} {b}");
        assert_eq!(bo(&s, "fire", "app:unknown", NOW), 0.0);
        assert_eq!(bo(&UsageStore::default(), "a", "app:x", NOW), 0.0);
    }

    #[test]
    fn prune_keeps_the_strongest() {
        let mut s = UsageStore::default();
        for i in 0..MAX_ENTRIES {
            s.record(&q("a"), &format!("app:old{i}"), NOW - 100 * DAY);
        }
        // A strong old entry and a fresh weak one survive over stale ones.
        for _ in 0..100 {
            s.record(&q("a"), "app:strong", NOW - 100 * DAY);
        }
        s.record(&q("a"), "app:fresh", NOW);
        assert_eq!(s.len(), MAX_ENTRIES);
        let ids = s.recent_ids(MAX_ENTRIES);
        assert!(ids.contains(&"app:strong".to_owned()));
        assert!(ids.contains(&"app:fresh".to_owned()));
        assert_eq!(UsageStore::load(&s.to_bytes()).len(), MAX_ENTRIES);
    }

    #[test]
    fn recent_ids_newest_first() {
        let mut s = UsageStore::default();
        s.record(&q("a"), "app:one", 10);
        s.record(&q("b"), "app:two", 30);
        s.record(&q("c"), "app:three", 20);
        s.record(&q("d"), "app:one", 5); // older use of "app:one" doesn't lower it
        assert_eq!(s.recent_ids(2), vec!["app:two", "app:three"]);
        assert_eq!(s.recent_ids(10), vec!["app:two", "app:three", "app:one"]);
    }

    #[test]
    fn clear_empties_and_marks_dirty() {
        let mut s = UsageStore::default();
        s.record(&q("a"), "app:x", 1);
        s.mark_clean();
        s.clear();
        assert!(s.is_empty() && s.is_dirty());
        assert!(s.to_bytes().is_empty());
        assert_eq!(bo(&s, "a", "app:x", 1), 0.0);
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
        s.record(&q("ab"), "app:x", 7);
        write_atomic(&link, &s.to_bytes(), 0o600).unwrap();
        let back = UsageStore::load(&read_capped(&link, MAX_FILE_BYTES).unwrap().unwrap());
        assert_eq!(back.to_bytes(), s.to_bytes());
        assert!(real.exists());
    }
}
