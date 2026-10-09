//! The recent files list: `~/.local/share/recently-used.xbel` (freedesktop
//! bookmark spec, as written by GTK and KDE's KRecentDocument). Any app can
//! write that file, so it is parsed defensively (docs/DESIGN.md, "Trust"):
//! size, bookmark count and XML depth are capped, only `file:` hrefs with
//! clean absolute paths are kept, and text is cleaned for display.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::fsutil::read_capped;
use crate::result::{Action, Kind, ResultItem, prior};
use crate::text::{Prepared, Query, clean_display, score_fields};

/// Largest file read, in bytes.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
/// Most `<bookmark>` elements read.
pub const MAX_BOOKMARKS: usize = 10_000;
/// Deepest XML nesting accepted.
pub const MAX_DEPTH: usize = 32;
/// Longest path kept, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;
/// Most entries kept (the newest): only these can be shown, and they bound
/// both the search cost and the stats of `retain_existing`.
pub const MAX_KEPT: usize = 500;
/// Most characters of the parent folder used for matching.
const MAX_PARENT_MATCH_CHARS: usize = 256;

#[derive(Debug)]
pub enum RecentError {
    /// Bigger than [`MAX_BYTES`].
    TooLarge,
    Io(std::io::Error),
}

impl fmt::Display for RecentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecentError::TooLarge => {
                write!(f, "recent files list is larger than {MAX_BYTES} bytes")
            }
            RecentError::Io(e) => write!(f, "recent files list could not be read: {e}"),
        }
    }
}

impl std::error::Error for RecentError {}

#[derive(Clone, PartialEq)]
pub struct RecentFile {
    /// Canonical `file:` URI (the path percent-encoded again).
    pub uri: String,
    /// Absolute, normalised, valid UTF-8, no NUL, at most 4096 bytes.
    pub path: PathBuf,
    /// The file name, cleaned for display.
    pub name: String,
    pub mime: Option<String>,
    /// Unix seconds of the latest of modified, visited and added; 0 if none
    /// could be read.
    pub when: i64,
    pub is_dir: bool,
}

/// Redacted: no URI, path, name or mime, only the kind.
impl fmt::Debug for RecentFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecentFile")
            .field("is_dir", &self.is_dir)
            .finish_non_exhaustive()
    }
}

/// The parsed list, newest first, one entry per path.
#[derive(Clone, Default)]
pub struct RecentFiles {
    /// Don't change this directly: `search` keeps a prepared copy of the
    /// names, rebuilt by [`RecentFiles::retain_existing`].
    pub items: Vec<RecentFile>,
    /// The XML was malformed or over a cap; `items` holds what was read
    /// before that.
    pub malformed: bool,
    /// (name, parent folder) prepared for matching, parallel to `items`.
    prepared: Vec<(Prepared, Prepared)>,
}

/// Redacted: counts only.
impl fmt::Debug for RecentFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecentFiles")
            .field("items", &self.items.len())
            .field("malformed", &self.malformed)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct Partial {
    href: Option<String>,
    added: i64,
    modified: i64,
    visited: i64,
    mime: Option<String>,
}

fn attr_value(e: &BytesStart<'_>, name: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == name {
            return a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned());
        }
    }
    None
}

pub(crate) fn valid_mime(m: &str) -> bool {
    if m.len() > 255 {
        return false;
    }
    match m.split_once('/') {
        Some((a, b)) => {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'+' | b'-')
                })
                && b.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'+' | b'-'))
        }
        None => false,
    }
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Percent-decodes into a string; None for a bad escape or invalid UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hi = hex(*b.get(i + 1)?)?;
            let lo = hex(*b.get(i + 2)?)?;
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A path as a `file:` URI: unreserved bytes and `/` stay, the rest is
/// percent-encoded.
pub(crate) fn file_uri(path: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(path.len() + 8);
    out.push_str("file://");
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 15) as usize] as char);
        }
    }
    out
}

/// A `file:` href as a clean absolute path and whether it named a folder
/// (trailing slash); None for anything else.
pub(crate) fn href_to_path(href: &str) -> Option<(String, bool)> {
    if href.len() > MAX_PATH_BYTES * 3 || href.len() < 6 {
        return None;
    }
    if !href.get(..5)?.eq_ignore_ascii_case("file:") {
        return None;
    }
    let rest = &href[5..];
    let encoded = if let Some(auth) = rest.strip_prefix("//") {
        let slash = auth.find('/')?;
        let host = &auth[..slash];
        if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
            return None;
        }
        &auth[slash..]
    } else {
        rest
    };
    let path = percent_decode(encoded)?;
    if !path.starts_with('/') || path.len() > MAX_PATH_BYTES || path.contains('\0') {
        return None;
    }
    let dir_slash = path.len() > 1 && path.ends_with('/');
    let trimmed = if dir_slash {
        &path[..path.len() - 1]
    } else {
        &path[..]
    };
    if trimmed != "/" {
        for seg in trimmed[1..].split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return None;
            }
        }
    }
    Some((trimmed.to_owned(), dir_slash))
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn num(b: &[u8], i: &mut usize, digits: usize) -> Option<i64> {
    let s = b.get(*i..*i + digits)?;
    if !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    *i += digits;
    Some(s.iter().fold(0, |a, &c| a * 10 + i64::from(c - b'0')))
}

/// ISO 8601 ("2024-05-17T10:20:30.5Z", "+02:00" offsets, no zone = UTC,
/// date only) as unix seconds; 0 for anything unreadable.
pub fn parse_date(s: &str) -> i64 {
    parse_date_opt(s.trim()).unwrap_or(0).max(0)
}

fn parse_date_opt(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() > 40 {
        return None;
    }
    let mut i = 0;
    let y = num(b, &mut i, 4)?;
    if b.get(i) != Some(&b'-') {
        return None;
    }
    i += 1;
    let mo = num(b, &mut i, 2)?;
    if b.get(i) != Some(&b'-') {
        return None;
    }
    i += 1;
    let d = num(b, &mut i, 2)?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let (mut h, mut mi, mut sec, mut off) = (0, 0, 0, 0);
    if i < b.len() {
        if !matches!(b[i], b'T' | b't' | b' ') {
            return None;
        }
        i += 1;
        h = num(b, &mut i, 2)?;
        if b.get(i) == Some(&b':') {
            i += 1;
        }
        mi = num(b, &mut i, 2)?;
        if b.get(i) == Some(&b':') {
            i += 1;
            sec = num(b, &mut i, 2)?;
        }
        if matches!(b.get(i), Some(b'.' | b',')) {
            i += 1;
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        match b.get(i) {
            None => {}
            Some(b'Z' | b'z') => i += 1,
            Some(&sign @ (b'+' | b'-')) => {
                i += 1;
                let oh = num(b, &mut i, 2)?;
                if b.get(i) == Some(&b':') {
                    i += 1;
                }
                let om = if i < b.len() { num(b, &mut i, 2)? } else { 0 };
                off = (oh * 3600 + om * 60) * if sign == b'-' { -1 } else { 1 };
            }
            Some(_) => return None,
        }
        if i != b.len() || h > 23 || mi > 59 || sec > 60 {
            return None;
        }
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - off)
}

fn finish(p: Partial) -> Option<RecentFile> {
    let (path, dir_slash) = href_to_path(p.href.as_deref()?)?;
    if path == "/" {
        return None;
    }
    let name = clean_display(path.rsplit('/').next()?);
    if name.is_empty() {
        return None;
    }
    let is_dir = dir_slash || p.mime.as_deref() == Some("inode/directory");
    Some(RecentFile {
        uri: file_uri(&path),
        path: PathBuf::from(path),
        name,
        mime: p.mime,
        when: p.added.max(p.modified).max(p.visited),
        is_dir,
    })
}

fn bookmark_start(e: &BytesStart<'_>) -> Partial {
    let mut p = Partial {
        href: attr_value(e, "href"),
        ..Partial::default()
    };
    let date = |n: &str| attr_value(e, n).map_or(0, |v| parse_date(&v));
    p.added = date("added");
    p.modified = date("modified");
    p.visited = date("visited");
    p
}

pub(crate) fn icon_for(f: &RecentFile) -> String {
    if f.is_dir {
        return "folder".into();
    }
    let Some(m) = f.mime.as_deref() else {
        return "text-x-generic".into();
    };
    let kind = m.split('/').next().unwrap_or("");
    match kind {
        "text" => "text-x-generic".into(),
        "image" => "image-x-generic".into(),
        "audio" => "audio-x-generic".into(),
        "video" => "video-x-generic".into(),
        _ if m == "application/pdf" => "application-pdf".into(),
        _ => {
            let dashed = m.replace('/', "-");
            // A theme icon name like any other: it starts with a letter or a
            // digit (a mime type may start with "-" or "."), so it is never
            // taken for an option or a hidden name.
            if crate::text::valid_icon_name(&dashed)
                && dashed
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
            {
                dashed
            } else {
                "text-x-generic".into()
            }
        }
    }
}

pub(crate) fn parent_of(path: &Path) -> &str {
    path.parent().and_then(Path::to_str).unwrap_or("/")
}

/// The parent folder as matched: its last [`MAX_PARENT_MATCH_CHARS`]
/// characters (the folders nearest the file).
fn parent_for_match(path: &Path) -> &str {
    let p = parent_of(path);
    match p.char_indices().rev().nth(MAX_PARENT_MATCH_CHARS - 1) {
        Some((i, _)) => &p[i..],
        None => p,
    }
}

/// `dir` with a leading `home` shown as "~".
pub(crate) fn tilde(dir: &str, home: &Path) -> String {
    if let Some(h) = home.to_str()
        && h.starts_with('/')
        && h != "/"
    {
        let h = h.trim_end_matches('/');
        if dir == h {
            return "~".into();
        }
        if let Some(rest) = dir.strip_prefix(h)
            && rest.starts_with('/')
        {
            return format!("~{rest}");
        }
    }
    dir.to_owned()
}

impl RecentFiles {
    /// Parses an `recently-used.xbel`. Malformed XML or an over-cap file
    /// keeps what came before and sets `malformed`; only a file over
    /// [`MAX_BYTES`] is refused.
    pub fn parse(bytes: &[u8]) -> Result<RecentFiles, RecentError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        Self::parse_at(bytes, now)
    }

    /// [`RecentFiles::parse`] with the clock given: a date up to a day after
    /// `now` counts as `now` (clock skew); a later one is forged and counts
    /// as 0 (unknown), so it sorts last, never to the top.
    fn parse_at(bytes: &[u8], now: i64) -> Result<RecentFiles, RecentError> {
        if bytes.len() > MAX_BYTES {
            return Err(RecentError::TooLarge);
        }
        let mut reader = Reader::from_reader(bytes);
        let mut items: Vec<RecentFile> = Vec::new();
        let mut cur: Option<Partial> = None;
        let (mut depth, mut count, mut malformed) = (0usize, 0usize, false);
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    depth += 1;
                    if depth > MAX_DEPTH {
                        malformed = true;
                        break;
                    }
                    match e.local_name().as_ref() {
                        "bookmark" => {
                            count += 1;
                            if count > MAX_BOOKMARKS {
                                malformed = true;
                                break;
                            }
                            cur = Some(bookmark_start(&e));
                        }
                        "mime-type" => set_mime(&mut cur, &e),
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                    "bookmark" => {
                        count += 1;
                        if count > MAX_BOOKMARKS {
                            malformed = true;
                            break;
                        }
                        items.extend(finish(bookmark_start(&e)));
                    }
                    "mime-type" => set_mime(&mut cur, &e),
                    _ => {}
                },
                Ok(Event::End(e)) => {
                    depth = depth.saturating_sub(1);
                    if e.local_name().as_ref() == "bookmark"
                        && let Some(p) = cur.take()
                    {
                        items.extend(finish(p));
                    }
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(_) => {
                    malformed = true;
                    break;
                }
            }
        }
        for i in &mut items {
            i.when = if i.when > now.saturating_add(86_400) {
                0
            } else {
                i.when.min(now)
            };
        }
        // Newest first; the path breaks ties so equal inputs sort alike.
        items.sort_by(|a, b| b.when.cmp(&a.when).then_with(|| a.path.cmp(&b.path)));
        let mut seen = HashSet::with_capacity(items.len());
        items.retain(|i| seen.insert(i.path.clone()));
        items.truncate(MAX_KEPT);
        let mut r = RecentFiles {
            items,
            malformed,
            prepared: Vec::new(),
        };
        r.prepare();
        Ok(r)
    }

    /// Reads and parses the file at `path`, reading at most the size cap
    /// plus one byte. File IO: call it off the GUI thread.
    pub fn load(path: &Path) -> Result<RecentFiles, RecentError> {
        let buf = match read_capped(path, MAX_BYTES as u64) {
            Ok(Some(b)) => b,
            Ok(None) => return Err(RecentError::Io(std::io::ErrorKind::NotFound.into())),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                return Err(RecentError::TooLarge);
            }
            Err(e) => return Err(RecentError::Io(e)),
        };
        Self::parse(&buf)
    }

    fn prepare(&mut self) {
        self.prepared = self
            .items
            .iter()
            .map(|i| {
                (
                    Prepared::new(&i.name),
                    Prepared::new(parent_for_match(&i.path)),
                )
            })
            .collect();
    }

    /// Drops entries whose file is gone and refreshes `is_dir` from the
    /// file system. This is file IO (one `stat` per entry): call it on the
    /// IO worker. Entries that can't be checked for another reason
    /// (permissions) stay.
    pub fn retain_existing(&mut self) {
        self.items.retain_mut(|i| match std::fs::metadata(&i.path) {
            Ok(md) => {
                i.is_dir = md.is_dir();
                true
            }
            Err(e) => !matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ),
        });
        self.prepare();
    }

    fn item(&self, f: &RecentFile, m: f32, home: &Path) -> ResultItem {
        let kind = if f.is_dir { Kind::Folder } else { Kind::File };
        ResultItem {
            id: format!("file:{}", f.uri),
            kind,
            title: f.name.clone(),
            subtitle: clean_display(&tilde(parent_of(&f.path), home)),
            icon: icon_for(f),
            score: m * prior(kind),
            action: Action::OpenFile { uri: f.uri.clone() },
        }
    }

    /// Files and folders whose name (in full) or parent folder (secondary)
    /// matches the query, unsorted. `home` is shown as "~".
    pub fn search(&self, q: &Query, home: &Path) -> Vec<ResultItem> {
        if q.is_empty() {
            return Vec::new();
        }
        let in_sync = self.prepared.len() == self.items.len();
        self.items
            .iter()
            .enumerate()
            .filter_map(|(n, f)| {
                let m = if in_sync {
                    let (name, parent) = &self.prepared[n];
                    score_fields(q, name, std::slice::from_ref(parent), None)?
                } else {
                    let parent = Prepared::new(parent_for_match(&f.path));
                    score_fields(q, &Prepared::new(&f.name), &[parent], None)?
                };
                Some(self.item(f, m, home))
            })
            .collect()
    }

    /// The `n` newest entries for the Start page, scored in that order.
    pub fn recent(&self, n: usize, home: &Path) -> Vec<ResultItem> {
        self.items
            .iter()
            .take(n)
            .enumerate()
            .map(|(rank, f)| self.item(f, 1.0 / (1.0 + rank as f32), home))
            .collect()
    }
}

fn set_mime(cur: &mut Option<Partial>, e: &BytesStart<'_>) {
    if let Some(p) = cur
        && let Some(t) = attr_value(e, "type")
        && valid_mime(&t)
    {
        p.mime = Some(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GTK: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0"
      xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks"
      xmlns:mime="http://www.freedesktop.org/standards/shared-mime-info">
  <bookmark href="file:///home/u/My%20Docs/caf%C3%A9.txt" added="2024-05-17T10:20:30Z" modified="2024-05-17T10:20:30Z" visited="2024-05-18T08:00:00Z">
    <info>
      <metadata owner="http://freedesktop.org">
        <mime:mime-type type="text/plain"/>
        <bookmark:applications>
          <bookmark:application name="Kate" exec="&apos;kate %u&apos;" modified="2024-05-17T10:20:30Z" count="1"/>
        </bookmark:applications>
      </metadata>
    </info>
  </bookmark>
  <bookmark href="file:///home/u/Pictures/" added="2024-01-01T00:00:00Z" modified="2024-01-01T00:00:00Z" visited="2024-01-01T00:00:00Z">
    <info><metadata owner="http://freedesktop.org"><mime:mime-type type="inode/directory"/></metadata></info>
  </bookmark>
  <bookmark href="https://example.org/x" added="2025-01-01T00:00:00Z" modified="2025-01-01T00:00:00Z" visited="2025-01-01T00:00:00Z"/>
</xbel>"#;

    const KDE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks" xmlns:mime="http://www.freedesktop.org/standards/shared-mime-info" xmlns:kdepriv="http://www.kde.org/kdepriv" version="1.0">
 <bookmark href="file:///home/u/doc.pdf" added="2023-03-01T12:00:00.123456Z" modified="2023-03-01T12:00:00Z" visited="2023-03-02T12:00:00+01:00">
  <title>doc.pdf</title>
  <info>
   <metadata owner="http://freedesktop.org">
    <mime:mime-type type="application/pdf"/>
    <bookmark:applications>
     <bookmark:application name="okular" exec="okular %u" modified="2023-03-01T12:00:00Z" count="1"/>
    </bookmark:applications>
   </metadata>
  </info>
 </bookmark>
</xbel>"#;

    fn p(s: &str) -> RecentFiles {
        RecentFiles::parse(s.as_bytes()).unwrap()
    }

    fn q(s: &str) -> Query {
        Query::new(s, 0)
    }

    #[test]
    fn gtk_sample() {
        let r = p(GTK);
        assert!(!r.malformed);
        assert_eq!(r.items.len(), 2);
        let f = &r.items[0];
        assert_eq!(f.path, PathBuf::from("/home/u/My Docs/café.txt"));
        assert_eq!(f.uri, "file:///home/u/My%20Docs/caf%C3%A9.txt");
        assert_eq!(f.name, "café.txt");
        assert_eq!(f.mime.as_deref(), Some("text/plain"));
        assert_eq!(f.when, 1_716_019_200); // 2024-05-18T08:00:00Z
        assert!(!f.is_dir);
        let d = &r.items[1];
        assert!(d.is_dir);
        assert_eq!(d.path, PathBuf::from("/home/u/Pictures"));
        assert_eq!(d.name, "Pictures");
    }

    #[test]
    fn kde_sample() {
        let r = p(KDE);
        assert_eq!(r.items.len(), 1);
        // visited 2023-03-02T12:00+01:00 = 11:00Z is the latest.
        assert_eq!(r.items[0].when, parse_date("2023-03-02T11:00:00Z"));
        assert_eq!(r.items[0].mime.as_deref(), Some("application/pdf"));
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("1970-01-01T00:00:00Z"), 0);
        assert_eq!(parse_date("2000-03-01T00:00:00Z"), 951_868_800);
        assert_eq!(parse_date("2024-02-29T23:59:59Z"), 1_709_251_199);
        assert_eq!(
            parse_date("2024-05-17T12:00:00+02:00"),
            parse_date("2024-05-17T10:00:00Z")
        );
        assert_eq!(
            parse_date("2024-05-17T12:00:00-0130"),
            parse_date("2024-05-17T13:30:00Z")
        );
        assert_eq!(
            parse_date("2024-05-17T10:00:00"),
            parse_date("2024-05-17T10:00:00Z")
        );
        assert_eq!(parse_date("2024-05-17"), 1_715_904_000);
        assert_eq!(
            parse_date("2024-05-17T10:00:00.25Z"),
            parse_date("2024-05-17T10:00:00Z")
        );
        for bad in [
            "",
            "x",
            "2024",
            "2024-13-01",
            "2024-05-17Tzz",
            "2024-05-17T25:00:00Z",
            "1969-01-01T00:00:00Z",
            "2024-05-17T10:00:00Zjunk",
        ] {
            assert_eq!(parse_date(bad), 0, "{bad}");
        }
    }

    #[test]
    fn malformed_tail_keeps_head() {
        let cut = &GTK[..GTK.find("<bookmark href=\"https").unwrap()];
        let r = p(&format!(
            "{cut}<bookmark href=\"file:///a/b\"><info></bookmar"
        ));
        assert!(r.malformed);
        assert_eq!(r.items.len(), 2);
        let r = p("<xbel><bookmark href=\"file:///a/b\"/><bookmark></xbel>");
        assert!(r.malformed);
        assert_eq!(r.items.len(), 1);
        assert!(p("").items.is_empty());
        assert!(!p(GTK).malformed);
    }

    #[test]
    fn hrefs() {
        let ok = |h: &str| {
            let x = format!("<xbel><bookmark href=\"{h}\"/></xbel>");
            p(&x).items.into_iter().next().map(|f| f.path)
        };
        assert_eq!(ok("file:///a/b%20c"), Some("/a/b c".into()));
        assert_eq!(ok("file://localhost/a/b"), Some("/a/b".into()));
        assert_eq!(ok("file:/a/b"), Some("/a/b".into()));
        assert_eq!(ok("FILE:///a/b"), Some("/a/b".into()));
        assert_eq!(ok("file:///a/b&amp;c"), Some("/a/b&c".into()));
        for bad in [
            "http://x/y",
            "smb://h/s/f",
            "file://host/a/b",
            "file:///a/../b",
            "file:///a/./b",
            "file:///a/%2e%2e/b",
            "file:///a//b",
            "file:///a%00b",
            "file:///a%zz",
            "file:///a%",
            "file:///%FF%FE",
            "file://",
            "file:///",
            "relative/path",
            "",
            "file:a/b",
        ] {
            assert_eq!(ok(bad), None, "{bad}");
        }
        let long = format!("file:///{}", "a".repeat(4096));
        assert_eq!(ok(&long), None);
        assert!(ok(&format!("file:///{}", "a".repeat(4000))).is_some());
    }

    #[test]
    fn mime_validation() {
        let m = |t: &str| {
            let x = format!(
                "<xbel><bookmark href=\"file:///a\"><mime:mime-type type=\"{t}\"/></bookmark></xbel>"
            );
            p(&x).items[0].mime.clone()
        };
        assert_eq!(m("image/svg+xml").as_deref(), Some("image/svg+xml"));
        assert_eq!(m("Text/plain"), None);
        assert_eq!(m("text"), None);
        assert_eq!(m("text/pl ain"), None);
        assert_eq!(m("../../x"), None);
    }

    #[test]
    fn dedupe_newest_first() {
        let r = p(r#"<xbel>
<bookmark href="file:///a/old" modified="2020-01-01T00:00:00Z"/>
<bookmark href="file:///a/new" modified="2024-01-01T00:00:00Z"/>
<bookmark href="file:///a/old" modified="2022-01-01T00:00:00Z"/>
<bookmark href="file:///a/none"/></xbel>"#);
        let names: Vec<_> = r.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["new", "old", "none"]);
        assert_eq!(r.items[1].when, parse_date("2022-01-01T00:00:00Z"));
    }

    #[test]
    fn caps() {
        assert!(matches!(
            RecentFiles::parse(&vec![b' '; MAX_BYTES + 1]),
            Err(RecentError::TooLarge)
        ));
        let deep = format!("{}{}", "<a>".repeat(40), "</a>".repeat(40));
        let r = p(&format!(
            "<xbel><bookmark href=\"file:///x\"/>{deep}</xbel>"
        ));
        assert!(r.malformed);
        assert_eq!(r.items.len(), 1);
        let many: String = (0..MAX_BOOKMARKS + 5)
            .map(|i| format!("<bookmark href=\"file:///f{i}\"/>"))
            .collect();
        let r = p(&format!("<xbel>{many}</xbel>"));
        assert!(r.malformed);
        assert_eq!(r.items.len(), MAX_KEPT); // MAX_BOOKMARKS read, newest MAX_KEPT kept
    }

    #[test]
    fn names_are_cleaned() {
        let r = p("<xbel><bookmark href=\"file:///a/x%0A%E2%80%AEy.txt\"/></xbel>");
        assert_eq!(r.items[0].name, "x y.txt");
    }

    #[test]
    fn search_and_start_page() {
        let r = p(GTK);
        let home = Path::new("/home/u");
        let hits = r.search(&q("café"), home);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "file:file:///home/u/My%20Docs/caf%C3%A9.txt");
        assert_eq!(hits[0].kind, Kind::File);
        assert_eq!(hits[0].subtitle, "~/My Docs");
        assert_eq!(hits[0].icon, "text-x-generic");
        assert!(hits[0].score > 0.0 && hits[0].score <= 0.7);
        let dirs = r.search(&q("pictures"), home);
        assert_eq!(dirs[0].kind, Kind::Folder);
        assert_eq!(dirs[0].icon, "folder");
        assert_eq!(dirs[0].subtitle, "~");
        // Parent folder as secondary.
        assert_eq!(r.search(&q("docs"), home).len(), 1);
        assert!(r.search(&q(""), home).is_empty());
        let rec = r.recent(1, home);
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].title, "café.txt");
        assert_eq!(r.recent(10, home).len(), 2);
        assert_eq!(p(KDE).recent(5, home)[0].icon, "application-pdf");
        assert_eq!(tilde("/home/user2/x", home), "/home/user2/x");
        assert_eq!(tilde("/x", Path::new("/")), "/x");
    }

    #[test]
    fn retain_existing_drops_missing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("here.txt");
        std::fs::write(&file, "x").unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let x = format!(
            "<xbel><bookmark href=\"{}\"/><bookmark href=\"{}\"/><bookmark href=\"{}\"/></xbel>",
            file_uri(file.to_str().unwrap()),
            file_uri(dir.path().join("gone").to_str().unwrap()),
            file_uri(sub.to_str().unwrap()),
        );
        let mut r = p(&x);
        assert_eq!(r.items.len(), 3);
        r.retain_existing();
        assert_eq!(r.items.len(), 2);
        assert!(r.items.iter().any(|i| i.is_dir));
        assert_eq!(r.search(&q("here"), dir.path()).len(), 1);
    }

    fn bm(path: &str, date: &str) -> String {
        format!(r#"<bookmark href="file://{path}" modified="{date}"/>"#)
    }

    #[test]
    fn keeps_newest_500() {
        let mut x = String::from("<xbel>");
        for i in 0..700 {
            let d = format!("2020-01-01T00:{:02}:{:02}Z", (i / 60) % 60, i % 60);
            x.push_str(&bm(&format!("/home/u/f{i}"), &d));
        }
        x.push_str("</xbel>");
        let now = parse_date("2024-01-01T00:00:00Z");
        let r = RecentFiles::parse_at(x.as_bytes(), now).unwrap();
        assert_eq!(r.items.len(), MAX_KEPT);
        assert_eq!(r.items[0].path, PathBuf::from("/home/u/f699"));
        assert_eq!(r.prepared.len(), MAX_KEPT);
    }

    #[test]
    fn future_dates_are_skew_or_forged() {
        let now = parse_date("2024-01-01T00:00:00Z");
        let x = format!(
            "<xbel>{}{}{}</xbel>",
            bm("/home/u/old", "2020-01-01T00:00:00Z"),
            bm("/home/u/skew", "2024-01-01T12:00:00Z"), // within a day: now
            bm("/home/u/forged", "2999-01-01T00:00:00Z"), // unknown: 0
        );
        let r = RecentFiles::parse_at(x.as_bytes(), now).unwrap();
        let by = |n: &str| r.items.iter().find(|i| i.path.ends_with(n)).unwrap().when;
        assert_eq!(by("skew"), now);
        assert_eq!(by("forged"), 0);
        assert_eq!(r.items[0].path, PathBuf::from("/home/u/skew"));
        assert_eq!(r.items[2].path, PathBuf::from("/home/u/forged"));
    }

    #[test]
    fn debug_is_redacted() {
        let r = p(&format!(
            "<xbel>{}</xbel>",
            bm("/home/u/secret.txt", "2020-01-01T00:00:00Z")
        ));
        let d = format!("{r:?} {:?}", r.items[0]);
        assert!(!d.contains("secret") && !d.contains("home"), "{d}");
    }

    #[test]
    fn parent_match_text_is_capped() {
        let long = format!("/{}/file", "d".repeat(1000));
        let p = PathBuf::from(&long);
        assert_eq!(parent_for_match(&p).chars().count(), MAX_PARENT_MATCH_CHARS);
        let multi = PathBuf::from(format!("/{}/f", "é".repeat(300)));
        assert_eq!(
            parent_for_match(&multi).chars().count(),
            MAX_PARENT_MATCH_CHARS
        );
        assert_eq!(parent_for_match(Path::new("/a/b")), "/a");
    }

    #[test]
    fn load_refuses_fifo_and_big_files() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("big.xbel");
        std::fs::write(&f, vec![b' '; MAX_BYTES + 1]).unwrap();
        assert!(matches!(RecentFiles::load(&f), Err(RecentError::TooLarge)));
        // A directory (like a FIFO) is not a regular file.
        assert!(matches!(
            RecentFiles::load(dir.path()),
            Err(RecentError::Io(_))
        ));
    }

    #[test]
    fn load_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("r.xbel");
        std::fs::write(&f, GTK).unwrap();
        assert_eq!(RecentFiles::load(&f).unwrap().items.len(), 2);
        assert!(matches!(
            RecentFiles::load(&dir.path().join("nope")),
            Err(RecentError::Io(_))
        ));
    }

    #[test]
    fn fuzz_no_panic() {
        let mut s = 7_u64;
        let base = GTK.as_bytes();
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
            if let Ok(r) = RecentFiles::parse(&b) {
                let _ = r.search(&q("caf"), Path::new("/home/u"));
            }
        }
        for _ in 0..300 {
            let mut t = String::new();
            for _ in 0..(s % 30) {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                t.push(char::from_u32(((s >> 33) % 0x200) as u32).unwrap_or('?'));
            }
            let _ = parse_date(&t);
            let _ = href_to_path(&t);
            let _ = href_to_path(&format!("file://{t}"));
        }
    }

    #[test]
    fn a_mime_type_never_makes_a_bad_icon_name() {
        // A mime type may start with "-" or "."; its icon name must not.
        let f = |m: &str| RecentFile {
            uri: "file:///a".into(),
            path: PathBuf::from("/a"),
            name: "a".into(),
            mime: Some(m.to_owned()),
            when: 0,
            is_dir: false,
        };
        for bad in ["-w/q", ".x/y", "-/-", "a/-b-", "x/.hidden"] {
            let icon = icon_for(&f(bad));
            assert!(
                crate::text::valid_icon_name(&icon),
                "{bad} gave the icon {icon}"
            );
        }
        assert_eq!(icon_for(&f("application/zip")), "application-zip");
        assert_eq!(icon_for(&f("-w/q")), "text-x-generic");
    }
}
