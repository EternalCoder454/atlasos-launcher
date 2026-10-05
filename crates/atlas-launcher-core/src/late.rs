//! Late results, checked where they enter (docs/DESIGN.md, "Trust"): KRunner
//! matches, which C++ hands over as plain data, and Explorer's file hits.
//! Anything that fails a check is dropped, never shown half-cleaned.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::query::MAX_LATE_BATCH;
use crate::recent::{RecentFile, file_uri, href_to_path, icon_for, parent_of, tilde, valid_mime};
use crate::result::{Action, Kind, ResultItem, prior};
use crate::text::{clean_display, clean_display_max, is_unsafe_char, valid_icon};

/// Longest runner id (a plugin id such as `krunner_bookmarksrunner`).
pub const MAX_RUNNER_ID_BYTES: usize = 128;
/// Longest match id; C++ uses it only to find the match it kept.
pub const MAX_MATCH_ID_BYTES: usize = 1024;
/// Shown text caps.
pub const MAX_TITLE_CHARS: usize = 256;
pub const MAX_SUBTITLE_CHARS: usize = 512;
pub use crate::text::MAX_ICON_PATH_BYTES;

const RUNNER_FALLBACK_ICON: &str = "application-x-executable";

/// One KRunner match, as C++ read it from `KRunner::QueryMatch`.
#[derive(Clone, Default, PartialEq)]
pub struct RunnerMatch {
    pub runner_id: String,
    pub match_id: String,
    pub text: String,
    pub subtext: String,
    /// A theme icon name, an absolute path, or empty.
    pub icon: String,
    /// The plugin's own relevance, nominally 0–1.
    pub relevance: f64,
}

/// One hit of Explorer's `Search1.Search`: (uri, name, kind, mime, icon,
/// mtime, size, score).
#[derive(Clone, Default, PartialEq)]
pub struct ExplorerHit {
    pub uri: String,
    pub name: String,
    pub kind: String,
    pub mime: String,
    pub icon: String,
    pub mtime: i64,
    pub size: u64,
    pub score: f64,
}

/// Redacted: lengths only, never text, ids or URIs.
impl fmt::Debug for RunnerMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunnerMatch")
            .field("runner_id_len", &self.runner_id.len())
            .field("match_id_len", &self.match_id.len())
            .field("text_len", &self.text.len())
            .field("subtext_len", &self.subtext.len())
            .finish_non_exhaustive()
    }
}

/// Redacted: lengths and the kind class only, never names, paths or URIs.
impl fmt::Debug for ExplorerHit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExplorerHit")
            .field("uri_len", &self.uri.len())
            .field("name_len", &self.name.len())
            .field(
                "is_dir",
                &matches!(self.kind.as_str(), "folder" | "directory" | "dir"),
            )
            .finish_non_exhaustive()
    }
}

fn clamp_unit(x: f64) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

fn valid_runner_id(id: &str) -> bool {
    (1..=MAX_RUNNER_ID_BYTES).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn valid_match_id(id: &str) -> bool {
    (1..=MAX_MATCH_ID_BYTES).contains(&id.len())
        && !id.chars().any(|c| c.is_control() || is_unsafe_char(c))
}

/// A KRunner match as a result row, or None when it fails a check.
/// Score = relevance × the runner prior.
pub fn runner_item(m: RunnerMatch) -> Option<ResultItem> {
    if !valid_runner_id(&m.runner_id) || !valid_match_id(&m.match_id) {
        return None;
    }
    let title = clean_display_max(&m.text, MAX_TITLE_CHARS);
    if title.is_empty() {
        return None;
    }
    let icon = if valid_icon(&m.icon) {
        m.icon
    } else {
        RUNNER_FALLBACK_ICON.to_owned()
    };
    Some(ResultItem {
        id: format!("runner:{}:{}", m.runner_id, m.match_id),
        kind: Kind::Runner,
        title,
        subtitle: clean_display_max(&m.subtext, MAX_SUBTITLE_CHARS),
        icon,
        score: clamp_unit(m.relevance) * prior(Kind::Runner),
        action: Action::Runner {
            runner_id: m.runner_id,
            match_id: m.match_id,
        },
    })
}

/// An Explorer hit as a result row, or None when its URI is not a clean
/// local `file:` path. The shown name comes from the path itself, so a hit
/// cannot show one name and open another file; the URI is rebuilt from the
/// path. Its id matches the recent-files row for the same path, so the two
/// merge.
pub fn explorer_item(hit: ExplorerHit, home: &Path) -> Option<ResultItem> {
    let (path, slash) = href_to_path(&hit.uri)?;
    let is_dir = slash || matches!(hit.kind.as_str(), "folder" | "directory" | "dir");
    let path = PathBuf::from(path);
    let name = clean_display(path.file_name()?.to_str()?);
    if name.is_empty() {
        return None;
    }
    let uri = file_uri(path.to_str()?);
    let file = RecentFile {
        mime: valid_mime(&hit.mime).then_some(hit.mime),
        uri,
        name,
        when: hit.mtime,
        is_dir,
        path,
    };
    // The hit's own icon is ignored: the icon comes from the path and mime.
    let icon = icon_for(&file);
    let kind = if is_dir { Kind::Folder } else { Kind::File };
    Some(ResultItem {
        id: format!("file:{}", file.uri),
        kind,
        subtitle: clean_display(&tilde(parent_of(&file.path), home)),
        title: file.name,
        icon,
        score: clamp_unit(hit.score) * prior(kind),
        action: Action::OpenFile { uri: file.uri },
    })
}

/// The checked rows of a KRunner batch; at most [`MAX_LATE_BATCH`] matches
/// are looked at.
pub fn runner_items(matches: Vec<RunnerMatch>) -> Vec<ResultItem> {
    matches
        .into_iter()
        .take(MAX_LATE_BATCH)
        .filter_map(runner_item)
        .collect()
}

/// The checked rows of an Explorer batch; at most [`MAX_LATE_BATCH`] hits
/// are looked at.
pub fn explorer_items(hits: Vec<ExplorerHit>, home: &Path) -> Vec<ResultItem> {
    hits.into_iter()
        .take(MAX_LATE_BATCH)
        .filter_map(|h| explorer_item(h, home))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rm(runner: &str, id: &str, text: &str, rel: f64) -> RunnerMatch {
        RunnerMatch {
            runner_id: runner.into(),
            match_id: id.into(),
            text: text.into(),
            subtext: "sub".into(),
            icon: "bookmarks".into(),
            relevance: rel,
        }
    }

    fn hit(uri: &str) -> ExplorerHit {
        ExplorerHit {
            uri: uri.into(),
            name: "whatever".into(),
            kind: "file".into(),
            mime: "text/plain".into(),
            icon: "text-x-generic".into(),
            mtime: 1,
            size: 2,
            score: 0.8,
        }
    }

    #[test]
    fn runner_matches_are_checked() {
        let item =
            runner_item(rm("krunner_bookmarks", "b1", "  Rust\u{202E}  docs ", 0.5)).unwrap();
        assert_eq!(item.id, "runner:krunner_bookmarks:b1");
        assert_eq!(item.title, "Rust docs");
        assert!((item.score - 0.375).abs() < 1e-6);
        assert_eq!(
            item.action,
            Action::Runner {
                runner_id: "krunner_bookmarks".into(),
                match_id: "b1".into()
            }
        );

        assert!(runner_item(rm("bad id", "x", "t", 1.0)).is_none());
        assert!(runner_item(rm("", "x", "t", 1.0)).is_none());
        assert!(runner_item(rm("r", "", "t", 1.0)).is_none());
        assert!(runner_item(rm("r", "a\nb", "t", 1.0)).is_none());
        assert!(runner_item(rm("r", &"x".repeat(MAX_MATCH_ID_BYTES + 1), "t", 1.0)).is_none());
        assert!(runner_item(rm("r", "x", "  \u{200B} ", 1.0)).is_none());

        // Relevance is clamped, NaN is zero.
        assert_eq!(
            runner_item(rm("r", "x", "t", 7.0)).unwrap().score,
            prior(Kind::Runner)
        );
        assert_eq!(runner_item(rm("r", "x", "t", -1.0)).unwrap().score, 0.0);
        assert_eq!(runner_item(rm("r", "x", "t", f64::NAN)).unwrap().score, 0.0);

        // Icons: names and absolute paths pass, anything else falls back.
        let mut m = rm("r", "x", "t", 1.0);
        m.icon = "/usr/share/icons/x.svg".into();
        assert_eq!(
            runner_item(m.clone()).unwrap().icon,
            "/usr/share/icons/x.svg"
        );
        m.icon = "/usr/../etc/x".into();
        assert_eq!(runner_item(m.clone()).unwrap().icon, RUNNER_FALLBACK_ICON);
        m.icon = "data:image/png;base64,AAAA".into();
        assert_eq!(runner_item(m).unwrap().icon, RUNNER_FALLBACK_ICON);

        let long = runner_item(rm("r", "x", &"a".repeat(5000), 1.0)).unwrap();
        assert!(long.title.chars().count() <= MAX_TITLE_CHARS);
    }

    #[test]
    fn explorer_hits_are_checked() {
        let home = Path::new("/home/u");
        let item = explorer_item(hit("file:///home/u/Docs/caf%C3%A9%20notes.txt"), home).unwrap();
        assert_eq!(item.title, "café notes.txt");
        assert_eq!(item.subtitle, "~/Docs");
        assert_eq!(item.kind, Kind::File);
        assert_eq!(item.id, "file:file:///home/u/Docs/caf%C3%A9%20notes.txt");
        assert!((item.score - 0.8 * prior(Kind::File)).abs() < 1e-6);
        assert_eq!(
            item.action,
            Action::OpenFile {
                uri: "file:///home/u/Docs/caf%C3%A9%20notes.txt".into()
            }
        );

        // The shown name is the path's, not the hit's.
        let mut h = hit("file:///home/u/evil.sh");
        h.name = "holiday.jpg".into();
        assert_eq!(explorer_item(h, home).unwrap().title, "evil.sh");

        for bad in [
            "https://example.org/x",
            "file://otherhost/etc/passwd",
            "file:///home/u/../../etc/passwd",
            "file:///home/u/./x",
            "file:relative",
            "file:///home/u/a%00b",
            "smb://server/share",
            "",
        ] {
            assert!(explorer_item(hit(bad), home).is_none(), "{bad}");
        }

        let mut dir = hit("file:///home/u/Projects");
        dir.kind = "folder".into();
        dir.icon = "evil-icon".into();
        let d = explorer_item(dir, home).unwrap();
        assert_eq!(d.kind, Kind::Folder);
        assert_eq!(d.icon, "folder");

        let mut weird = hit("file:///home/u/x");
        weird.score = f64::INFINITY;
        weird.mime = "not a mime".into();
        let w = explorer_item(weird, home).unwrap();
        assert_eq!(w.score, 0.0);
    }

    #[test]
    fn explorer_icon_comes_from_the_path() {
        let home = Path::new("/home/u");
        let mut h = hit("file:///home/u/a.txt");
        h.icon = "/etc/passwd".into();
        let it = explorer_item(h.clone(), home).unwrap();
        assert_ne!(it.icon, "/etc/passwd");
        h.icon = "text-x-generic".into();
        assert_eq!(explorer_item(h, home).unwrap().icon, it.icon);
    }

    #[test]
    fn debug_is_redacted() {
        let r = format!(
            "{:?}",
            rm("krunner_secret", "id-secret", "text-secret", 1.0)
        );
        assert!(!r.contains("secret"), "{r}");
        let e = format!("{:?}", hit("file:///home/u/secret.txt"));
        assert!(!e.contains("secret") && !e.contains("home"), "{e}");
    }

    #[test]
    fn batches_are_capped() {
        let ms: Vec<_> = (0..MAX_LATE_BATCH + 50)
            .map(|i| rm("r", &format!("m{i}"), "t", 0.5))
            .collect();
        let items = runner_items(ms);
        assert_eq!(items.len(), MAX_LATE_BATCH);
        assert_eq!(items[0].id, "runner:r:m0");
        let hs: Vec<_> = (0..MAX_LATE_BATCH + 50)
            .map(|i| hit(&format!("file:///home/u/f{i}")))
            .collect();
        assert_eq!(
            explorer_items(hs, Path::new("/home/u")).len(),
            MAX_LATE_BATCH
        );
        // Invalid entries inside the first 200 are dropped, not replaced.
        let mut ms: Vec<_> = (0..MAX_LATE_BATCH + 5)
            .map(|i| rm("r", &format!("m{i}"), "t", 0.5))
            .collect();
        ms[0].runner_id = "bad id".into();
        assert_eq!(runner_items(ms).len(), MAX_LATE_BATCH - 1);
    }
}
