//! Rename App beyond the launcher: the user's name for an app, written where
//! the rest of the desktop looks for it.
//!
//! The dock, the menus, KRunner and Settings read an app's name from its
//! desktop file. A desktop file of the same id in `$XDG_DATA_HOME/applications`
//! takes the place of the system one (that is how KDE's menu editor changes an
//! entry), so a rename writes one: the system file's content, with only `Name`
//! changed (every localized `Name[..]` is dropped, so the name is the same in
//! every language), and two keys that say the launcher made it:
//!
//! ```text
//! X-Telamon-Renamed=true
//! X-Telamon-Original-Name=<the system file's Name>
//! ```
//!
//! Only a file with that marker is ever changed or deleted; a file of the
//! user's own (or of another tool) at that path is left alone and the name
//! stays the launcher's alone ([`Outcome::NotOurs`]). The id is the app's real
//! desktop file id, so a pin in the dock, a window's app id and the launch
//! (`KService::serviceByStorageId`) all still find the one app.
//!
//! `names.conf` stays the record of what the user chose; [`sync`] makes the
//! files agree with it at start: an override is (re)written when it is
//! missing or the system file changed since (an update), and one the launcher
//! made that `names.conf` no longer asks for, or whose app is gone, is
//! deleted. Names set before 0.3.2 reach the desktop the same way, once.
//!
//! Nothing here writes outside the user's `applications` folder, follows a
//! link there, or reads more than [`MAX_DESKTOP_BYTES`] of a file; ids are
//! validated by the caller ([`crate::catalog::valid_desktop_id`]) and again
//! here. Nothing logs ids or names.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::catalog::valid_desktop_id;
use crate::fsutil::{
    create_atomic_nofollow, read_capped, read_capped_nofollow, write_atomic_nofollow,
};
use crate::legacy::desktop_id_alias;
use crate::names::{Names, clean_name};

/// `true` in an override the launcher made.
pub const RENAMED_KEY: &str = "X-Telamon-Renamed";
/// The system file's own `Name`, as written in it.
pub const ORIGINAL_KEY: &str = "X-Telamon-Original-Name";
/// Largest desktop file read.
pub const MAX_DESKTOP_BYTES: u64 = 256 * 1024;
/// Most files looked at in the user's folder when sweeping.
const MAX_SCAN: usize = 8192;

/// Where desktop files are.
#[derive(Clone, Debug)]
pub struct AppDirs {
    /// `$XDG_DATA_HOME/applications`: the only place anything is written.
    pub user: PathBuf,
    /// `<dir>/applications` of `$XDG_DATA_DIRS` (and Flatpak's exports), in
    /// order: the first with the id is the one the desktop uses.
    pub system: Vec<PathBuf>,
}

/// What a call did to one app's override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Written (new, or the content changed).
    Written,
    /// Already as it should be.
    Unchanged,
    /// Deleted (ours, no longer wanted).
    Removed,
    /// There is a file of the user's own or another tool's at that path (or a
    /// link): it is not touched and the name stays the launcher's.
    NotOurs,
    /// No system desktop file for the app to copy (a file only in the user's
    /// folder, a bad id, a file too large or not text).
    NoOriginal,
    /// The write or delete failed.
    Failed,
}

/// What a [`sync`] did: counts, never ids or names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub written: usize,
    pub removed: usize,
    /// Names the desktop does not get (see [`Outcome::NotOurs`], `NoOriginal`).
    pub not_shared: usize,
    pub failed: usize,
}

impl SyncReport {
    /// Whether any file in the user's folder changed.
    pub fn changed(&self) -> bool {
        self.written + self.removed > 0
    }
}

/// The value of `key` in the `[Desktop Entry]` group, as written (first one).
fn entry_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut in_entry = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_entry {
                return None;
            }
            in_entry = t == "[Desktop Entry]";
            continue;
        }
        if in_entry
            && let Some((k, v)) = t.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim());
        }
    }
    None
}

/// Whether `bytes` is an override the launcher made.
pub fn is_ours(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|t| entry_value(t, RENAMED_KEY))
        == Some("true")
}

/// A desktop file value for `name`: backslash is the escape character.
fn escape(name: &str) -> String {
    name.replace('\\', "\\\\")
}

/// The override for `original` (a system desktop file) under `name` (already
/// cleaned: one line, no control characters). `None` when `original` is not
/// an application's desktop file.
pub fn render(original: &str, name: &str) -> Option<String> {
    if original.contains('\0') || entry_value(original, "Type") != Some("Application") {
        return None;
    }
    // A name that `clean_name` would change (a line break, a control, bidi or
    // invisible character, too long, empty) is refused here too: this is the
    // line that is written into a file the whole desktop reads, so it does not
    // rely on the caller having cleaned it.
    if name.is_empty() || clean_name(name) != name {
        return None;
    }
    let own = entry_value(original, "Name").unwrap_or("");
    let markers = format!(
        "{RENAMED_KEY}=true\n{ORIGINAL_KEY}={own}\n",
        own = own.replace(['\r', '\n'], " ")
    );
    let name_line = format!("Name={}\n", escape(name));
    let mut out = String::with_capacity(original.len() + 128);
    let mut in_entry = false;
    let mut name_done = false;
    let mut markers_done = false;
    let finish_entry = |out: &mut String, name_done: &mut bool, markers_done: &mut bool| {
        if !*name_done {
            out.push_str(&name_line);
            *name_done = true;
        }
        if !*markers_done {
            out.push_str(&markers);
            *markers_done = true;
        }
    };
    for raw in original.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        let t = line.trim();
        if t.starts_with('[') {
            if in_entry {
                finish_entry(&mut out, &mut name_done, &mut markers_done);
            }
            in_entry = t == "[Desktop Entry]" && !markers_done;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_entry && let Some((k, _)) = t.split_once('=') {
            let k = k.trim();
            if k == "Name" {
                if !name_done {
                    out.push_str(&name_line);
                    name_done = true;
                }
                continue;
            }
            if k.starts_with("Name[")
                || k == RENAMED_KEY
                || k == ORIGINAL_KEY
                || k.starts_with(&format!("{ORIGINAL_KEY}["))
            {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if in_entry {
        finish_entry(&mut out, &mut name_done, &mut markers_done);
    }
    Some(out)
}

/// The system desktop file of `id` and its content: the first dir that has
/// the id, directly or as the menu folder it names (`kde-foo.desktop` may be
/// `kde/foo.desktop`), is the one the desktop uses; when that one cannot be
/// used (too large, not text, not an application, one of ours) there is none,
/// and a lower dir's file is not put in its place.
fn find_original(dirs: &AppDirs, id: &str) -> Option<(PathBuf, String, i64)> {
    if !valid_desktop_id(id) {
        return None;
    }
    let mut names = vec![id.to_owned()];
    for (i, b) in id.bytes().enumerate() {
        // A folder name is never empty, `.` or `..`.
        if b == b'-'
            && i > 0
            && i + 1 < id.len() - ".desktop".len()
            && !matches!(&id[..i], "." | "..")
        {
            names.push(format!("{}/{}", &id[..i], &id[i + 1..]));
        }
    }
    for dir in &dirs.system {
        if *dir == dirs.user {
            continue;
        }
        for n in &names {
            let path = dir.join(n);
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if meta.len() == 0 || meta.len() > MAX_DESKTOP_BYTES {
                return None;
            }
            let Ok(Some(bytes)) = read_capped(&path, MAX_DESKTOP_BYTES) else {
                return None;
            };
            let text = String::from_utf8(bytes).ok()?;
            if is_ours(text.as_bytes()) {
                return None;
            }
            return Some((path, text, meta.mtime()));
        }
    }
    None
}

/// Test hooks between the steps of [`apply`], to put something in the way at
/// the moment a race would: after `existing` has looked at the path, and just
/// before the file is written.
#[cfg(test)]
mod hooks {
    use std::cell::RefCell;
    use std::path::Path;

    type Hook = Box<dyn Fn(&Path)>;

    thread_local! {
        pub static AFTER_LOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
        pub static BEFORE_WRITE: RefCell<Option<Hook>> = const { RefCell::new(None) };
    }

    pub fn after_look(path: &Path) {
        AFTER_LOOK.with(|h| {
            if let Some(f) = h.borrow().as_ref() {
                f(path);
            }
        });
    }

    pub fn before_write(path: &Path) {
        BEFORE_WRITE.with(|h| {
            if let Some(f) = h.borrow().as_ref() {
                f(path);
            }
        });
    }
}

enum Existing {
    Absent,
    Ours(Vec<u8>),
    /// Something else, or not safe to look at.
    Other,
}

/// What is at the user's path for `id`: looked at with `lstat`, so a link is
/// never followed.
fn existing(dirs: &AppDirs, id: &str) -> Existing {
    let path = dirs.user.join(id);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Existing::Absent,
        Err(_) => Existing::Other,
        Ok(m) if !m.file_type().is_file() || m.len() > MAX_DESKTOP_BYTES => Existing::Other,
        // Opened without following a link: one swapped in since the lstat is
        // an error here, not a file to read.
        Ok(_) => {
            #[cfg(test)]
            hooks::after_look(&path);
            match read_capped_nofollow(&path, MAX_DESKTOP_BYTES) {
                Ok(Some(bytes)) if is_ours(&bytes) => Existing::Ours(bytes),
                Ok(None) => Existing::Absent,
                _ => Existing::Other,
            }
        }
    }
}

fn set_mtime(path: &Path, secs: i64) {
    let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    let t = libc::timespec {
        tv_sec: secs,
        tv_nsec: 0,
    };
    // SAFETY: `c` is a NUL-terminated path that outlives the call, `times`
    // points at two valid timespecs; the flag makes a link itself the target
    // rather than what it points to.
    unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c.as_ptr(),
            [t, t].as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        );
    }
}

/// Makes the override of `ids` (the desktop file ids the app may have: its
/// own and the other generation's, see [`desktop_id_alias`]) say `name`.
pub fn apply(dirs: &AppDirs, ids: &[&str], name: &str) -> Outcome {
    for id in ids {
        let Some((_, text, mtime)) = find_original(dirs, id) else {
            continue;
        };
        let Some(wanted) = render(&text, name).filter(|w| w.len() as u64 <= MAX_DESKTOP_BYTES)
        else {
            return Outcome::NoOriginal;
        };
        let state = existing(dirs, id);
        let outcome = match state {
            Existing::Other => return Outcome::NotOurs,
            Existing::Ours(ref have) if have == wanted.as_bytes() => Outcome::Unchanged,
            _ => {
                let path = dirs.user.join(id);
                let made = fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o755)
                    .create(&dirs.user);
                #[cfg(test)]
                hooks::before_write(&path);
                // A name that was absent when looked at is made only if it is
                // still free; a file of ours is replaced.
                let written = if matches!(state, Existing::Absent) {
                    create_atomic_nofollow(&path, wanted.as_bytes(), 0o644)
                } else {
                    write_atomic_nofollow(&path, wanted.as_bytes(), 0o644)
                };
                if made.is_err() || written.is_err() {
                    return Outcome::Failed;
                }
                // The app's age is the system file's, not this write's
                // ("recently installed" must not list a renamed app).
                if mtime > 0 {
                    set_mtime(&path, mtime);
                }
                Outcome::Written
            }
        };
        // The app moved from one generation's id to the other's (an image
        // update): what was made under the id it left goes.
        for other in ids.iter().filter(|o| *o != id) {
            if matches!(existing(dirs, other), Existing::Ours(_)) {
                remove(dirs, other);
            }
        }
        return outcome;
    }
    // No system file under any id. A leftover of ours has nothing to follow,
    // unless the folders themselves cannot be seen (then nothing is known).
    if dirs.system.iter().all(|d| fs::read_dir(d).is_err()) {
        return Outcome::NoOriginal;
    }
    for id in ids {
        if matches!(existing(dirs, id), Existing::Ours(_)) {
            return remove(dirs, id);
        }
    }
    Outcome::NoOriginal
}

/// Deletes the override of `id` if the launcher made it.
pub fn remove(dirs: &AppDirs, id: &str) -> Outcome {
    if !valid_desktop_id(id) {
        return Outcome::NoOriginal;
    }
    match existing(dirs, id) {
        Existing::Absent => Outcome::Unchanged,
        Existing::Other => Outcome::NotOurs,
        Existing::Ours(_) => match fs::remove_file(dirs.user.join(id)) {
            Ok(()) => Outcome::Removed,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Outcome::Unchanged,
            Err(_) => Outcome::Failed,
        },
    }
}

/// The ids an app of `id` may have as a file name.
pub fn ids_of(id: &str) -> Vec<String> {
    let mut v = vec![id.to_owned()];
    if let Some(a) = desktop_id_alias(id) {
        v.push(a);
    }
    v
}

/// Makes the user's folder agree with `names`: one override per name, none
/// the launcher made that is not asked for.
pub fn sync(dirs: &AppDirs, names: &Names) -> SyncReport {
    let mut r = SyncReport::default();
    let tally = |o: Outcome, r: &mut SyncReport| match o {
        Outcome::Written => r.written += 1,
        Outcome::Removed => r.removed += 1,
        Outcome::NotOurs | Outcome::NoOriginal => r.not_shared += 1,
        Outcome::Failed => r.failed += 1,
        Outcome::Unchanged => {}
    };
    for (id, name) in names.iter() {
        let ids = ids_of(id);
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        tally(apply(dirs, &refs, name), &mut r);
    }
    // What the launcher made and is not asked for any more.
    let Ok(rd) = fs::read_dir(&dirs.user) else {
        return r;
    };
    for entry in rd.take(MAX_SCAN).flatten() {
        let file = entry.file_name();
        let Some(id) = file.to_str().filter(|n| valid_desktop_id(n)) else {
            continue;
        };
        if names.get(id).is_some() {
            continue;
        }
        if matches!(existing(dirs, id), Existing::Ours(_)) {
            tally(remove(dirs, id), &mut r);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    const ORIGINAL: &str = "[Desktop Entry]\nType=Application\nName=Files\nName[de]=Dateien\nName[fr]=Fichiers\nExec=files %U\nIcon=folder\nStartupWMClass=files\nX-Flatpak=org.example.Files\nActions=new;\n\n[Desktop Action new]\nName=New Window\nName[de]=Neues Fenster\nExec=files --new\n";

    struct Fx {
        _dir: tempfile::TempDir,
        dirs: AppDirs,
    }

    fn fx() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("home/applications");
        let sys = dir.path().join("usr/share/applications");
        let flat = dir.path().join("flatpak/applications");
        fs::create_dir_all(&sys).unwrap();
        fs::create_dir_all(&flat).unwrap();
        fs::write(sys.join("org.example.files.desktop"), ORIGINAL).unwrap();
        Fx {
            dirs: AppDirs {
                user,
                system: vec![sys, flat],
            },
            _dir: dir,
        }
    }

    fn read(f: &Fx, id: &str) -> String {
        fs::read_to_string(f.dirs.user.join(id)).unwrap()
    }

    #[test]
    fn render_changes_only_the_name() {
        let out = render(ORIGINAL, "My Files").unwrap();
        assert!(out.contains("\nName=My Files\n"));
        assert!(!out.contains("Dateien") && !out.contains("Fichiers"));
        // Everything else of the entry is there, in place.
        for keep in [
            "Exec=files %U",
            "Icon=folder",
            "StartupWMClass=files",
            "X-Flatpak=org.example.Files",
            "Actions=new;",
        ] {
            assert!(out.contains(keep), "{keep}");
        }
        // The action's own names are not the app's.
        assert!(out.contains("[Desktop Action new]\nName=New Window\nName[de]=Neues Fenster\n"));
        assert!(is_ours(out.as_bytes()));
        assert_eq!(entry_value(&out, ORIGINAL_KEY), Some("Files"));
        assert_eq!(entry_value(&out, "Name"), Some("My Files"));
        // The markers sit in the entry, before the action group.
        assert!(out.find(RENAMED_KEY).unwrap() < out.find("[Desktop Action").unwrap());
    }

    #[test]
    fn render_escapes_and_refuses() {
        let out = render(ORIGINAL, "Back\\slash").unwrap();
        assert!(out.contains("\nName=Back\\\\slash\n"));
        assert!(render("[Desktop Entry]\nType=Link\nName=x\n", "y").is_none());
        assert!(render("garbage", "y").is_none());
        assert!(render("[Desktop Entry]\nType=Application\0\nName=x\n", "y").is_none());
        // No Name in the original: one is added.
        let n = render("[Desktop Entry]\nType=Application\nExec=x\n", "Zed").unwrap();
        assert!(n.contains("Name=Zed\n") && n.contains("X-Telamon-Original-Name=\n"));
        // Our own keys in the input are dropped, not doubled.
        let twice = render(&render(ORIGINAL, "A").unwrap(), "B").unwrap();
        assert_eq!(twice.matches(RENAMED_KEY).count(), 1);
        assert_eq!(twice.matches(ORIGINAL_KEY).count(), 1);
    }

    #[test]
    fn render_keeps_crlf_files_readable() {
        let crlf = ORIGINAL.replace('\n', "\r\n");
        let out = render(&crlf, "Mine").unwrap();
        assert_eq!(entry_value(&out, "Name"), Some("Mine"));
        assert_eq!(entry_value(&out, "Exec"), Some("files %U"));
    }

    #[test]
    fn apply_writes_updates_and_removes() {
        let f = fx();
        let id = "org.example.files.desktop";
        assert_eq!(apply(&f.dirs, &[id], "My Files"), Outcome::Written);
        let path = f.dirs.user.join(id);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(read(&f, id).contains("Name=My Files"));
        // The age is the system file's.
        let sys = f.dirs.system[0].join(id);
        assert_eq!(
            fs::metadata(&path).unwrap().mtime(),
            fs::metadata(&sys).unwrap().mtime()
        );
        assert_eq!(apply(&f.dirs, &[id], "My Files"), Outcome::Unchanged);
        // A new name rewrites it.
        assert_eq!(apply(&f.dirs, &[id], "Other"), Outcome::Written);
        assert!(read(&f, id).contains("Name=Other"));
        // The system file changed (an update): the override follows it and
        // keeps the name.
        fs::write(&sys, ORIGINAL.replace("Exec=files %U", "Exec=files2 %U")).unwrap();
        assert_eq!(apply(&f.dirs, &[id], "Other"), Outcome::Written);
        let now = read(&f, id);
        assert!(now.contains("Exec=files2 %U") && now.contains("Name=Other"));
        assert_eq!(remove(&f.dirs, id), Outcome::Removed);
        assert!(!path.exists());
        assert_eq!(remove(&f.dirs, id), Outcome::Unchanged);
        // No stray temp files.
        assert_eq!(fs::read_dir(&f.dirs.user).unwrap().count(), 0);
    }

    #[test]
    fn a_file_that_is_not_ours_is_never_touched() {
        let f = fx();
        let id = "org.example.files.desktop";
        fs::create_dir_all(&f.dirs.user).unwrap();
        let mine = "[Desktop Entry]\nType=Application\nName=Edited by hand\nExec=x\n";
        fs::write(f.dirs.user.join(id), mine).unwrap();
        assert_eq!(apply(&f.dirs, &[id], "My Files"), Outcome::NotOurs);
        assert_eq!(remove(&f.dirs, id), Outcome::NotOurs);
        assert_eq!(read(&f, id), mine);
        // A link, even to a file of ours, is not followed.
        fs::remove_file(f.dirs.user.join(id)).unwrap();
        let target = f.dirs.user.join("target");
        fs::write(&target, render(ORIGINAL, "X").unwrap()).unwrap();
        symlink(&target, f.dirs.user.join(id)).unwrap();
        assert_eq!(apply(&f.dirs, &[id], "My Files"), Outcome::NotOurs);
        assert_eq!(remove(&f.dirs, id), Outcome::NotOurs);
        assert!(
            fs::symlink_metadata(f.dirs.user.join(id))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn no_original_means_no_override() {
        let f = fx();
        // Only in the user's folder: not ours to copy.
        fs::create_dir_all(&f.dirs.user).unwrap();
        fs::write(f.dirs.user.join("mine.desktop"), ORIGINAL).unwrap();
        assert_eq!(apply(&f.dirs, &["mine.desktop"], "X"), Outcome::NoOriginal);
        // Unknown, invalid and path-like ids.
        assert_eq!(apply(&f.dirs, &["gone.desktop"], "X"), Outcome::NoOriginal);
        assert_eq!(
            apply(&f.dirs, &["../etc/x.desktop"], "X"),
            Outcome::NoOriginal
        );
        assert_eq!(apply(&f.dirs, &["a/b.desktop"], "X"), Outcome::NoOriginal);
        assert_eq!(remove(&f.dirs, "../x.desktop"), Outcome::NoOriginal);
        // A system file that is not an application, or too large, or not text.
        fs::write(
            f.dirs.system[0].join("link.desktop"),
            "[Desktop Entry]\nType=Link\nName=l\n",
        )
        .unwrap();
        assert_eq!(apply(&f.dirs, &["link.desktop"], "X"), Outcome::NoOriginal);
        fs::write(f.dirs.system[0].join("bin.desktop"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(apply(&f.dirs, &["bin.desktop"], "X"), Outcome::NoOriginal);
        let big = format!("{ORIGINAL}# {}\n", "x".repeat(MAX_DESKTOP_BYTES as usize));
        fs::write(f.dirs.system[0].join("big.desktop"), big).unwrap();
        assert_eq!(apply(&f.dirs, &["big.desktop"], "X"), Outcome::NoOriginal);
        // Nothing was written for any of them.
        assert_eq!(fs::read_dir(&f.dirs.user).unwrap().count(), 1);
    }

    #[test]
    fn flatpak_exports_and_menu_folders_are_found() {
        let f = fx();
        let flat = &f.dirs.system[1];
        let exported = ORIGINAL.replace(
            "Exec=files %U",
            "Exec=/usr/bin/flatpak run --command=files org.example.Files @@u %U @@",
        );
        fs::write(flat.join("org.example.Files.desktop"), &exported).unwrap();
        assert_eq!(
            apply(&f.dirs, &["org.example.Files.desktop"], "Mine"),
            Outcome::Written
        );
        let out = read(&f, "org.example.Files.desktop");
        assert!(
            out.contains("flatpak run --command=files")
                && out.contains("X-Flatpak=org.example.Files")
        );
        // kde-foo.desktop lives at kde/foo.desktop.
        fs::create_dir_all(f.dirs.system[0].join("kde")).unwrap();
        fs::write(f.dirs.system[0].join("kde/foo.desktop"), ORIGINAL).unwrap();
        assert_eq!(
            apply(&f.dirs, &["kde-foo.desktop"], "Foo"),
            Outcome::Written
        );
        assert!(f.dirs.user.join("kde-foo.desktop").exists());
        assert!(!f.dirs.user.join("kde").exists());
    }

    #[test]
    fn a_dash_never_makes_a_path_out_of_the_folder() {
        let f = fx();
        // `<system dir>/../x.desktop` is not read.
        fs::write(
            f.dirs.system[0].parent().unwrap().join("x.desktop"),
            ORIGINAL,
        )
        .unwrap();
        assert_eq!(apply(&f.dirs, &["..-x.desktop"], "X"), Outcome::NoOriginal);
        assert_eq!(apply(&f.dirs, &[".-x.desktop"], "X"), Outcome::NoOriginal);
        assert!(!f.dirs.user.exists());
    }

    #[test]
    fn an_unusable_first_file_is_not_replaced_by_a_lower_one() {
        let f = fx();
        let id = "org.example.files.desktop";
        fs::write(f.dirs.system[0].join(id), [0xff, 0xfe]).unwrap();
        fs::write(f.dirs.system[1].join(id), ORIGINAL).unwrap();
        assert_eq!(apply(&f.dirs, &[id], "N"), Outcome::NoOriginal);
        assert!(!f.dirs.user.join(id).exists());
    }

    #[test]
    fn an_override_under_the_other_generations_id_goes_when_the_app_moves() {
        let f = fx();
        let (old, new) = (
            "net.eterneon.atlas.x.desktop",
            "net.eterneon.telamon.x.desktop",
        );
        fs::write(f.dirs.system[0].join(old), ORIGINAL).unwrap();
        let mut names = Names::default();
        names.set(new, "Mine");
        sync(&f.dirs, &names);
        assert!(f.dirs.user.join(old).exists());
        // The image now ships the new id only.
        fs::remove_file(f.dirs.system[0].join(old)).unwrap();
        fs::write(f.dirs.system[0].join(new), ORIGINAL).unwrap();
        sync(&f.dirs, &names);
        assert!(f.dirs.user.join(new).exists());
        assert!(!f.dirs.user.join(old).exists());
    }

    #[test]
    fn nothing_is_removed_when_no_system_folder_can_be_seen() {
        let f = fx();
        let id = "org.example.files.desktop";
        assert_eq!(apply(&f.dirs, &[id], "N"), Outcome::Written);
        let blind = AppDirs {
            user: f.dirs.user.clone(),
            system: vec![f.dirs.user.join("nowhere")],
        };
        assert_eq!(apply(&blind, &[id], "N"), Outcome::NoOriginal);
        assert!(f.dirs.user.join(id).exists());
    }

    #[test]
    fn the_first_system_dir_wins_as_in_the_desktop() {
        let f = fx();
        let id = "org.example.files.desktop";
        fs::write(
            f.dirs.system[1].join(id),
            ORIGINAL.replace("Exec=files %U", "Exec=second"),
        )
        .unwrap();
        assert_eq!(apply(&f.dirs, &[id], "N"), Outcome::Written);
        assert!(read(&f, id).contains("Exec=files %U"));
    }

    #[test]
    fn sync_migrates_follows_and_cleans_up() {
        let f = fx();
        let id = "org.example.files.desktop";
        let mut names = Names::default();
        names.set(id, "My Files");
        // An app that is not installed keeps its name, with no file.
        names.set("gone.desktop", "Ghost");
        // The user's own file at another id stays.
        fs::create_dir_all(&f.dirs.user).unwrap();
        fs::write(
            f.dirs.user.join("mine.desktop"),
            "[Desktop Entry]\nType=Application\nName=m\n",
        )
        .unwrap();
        let r = sync(&f.dirs, &names);
        assert_eq!((r.written, r.removed, r.not_shared, r.failed), (1, 0, 1, 0));
        assert!(r.changed());
        assert!(read(&f, id).contains("Name=My Files"));
        // Again: nothing to do.
        assert!(!sync(&f.dirs, &names).changed());
        // The line was deleted from names.conf: the override goes.
        names.reset(id);
        let r = sync(&f.dirs, &names);
        assert_eq!(r.removed, 1);
        assert!(!f.dirs.user.join(id).exists());
        assert!(f.dirs.user.join("mine.desktop").exists());
        // The app was uninstalled: its override does not outlive it.
        names.set(id, "My Files");
        sync(&f.dirs, &names);
        fs::remove_file(f.dirs.system[0].join(id)).unwrap();
        let r = sync(&f.dirs, &names);
        assert!(r.removed == 1 && r.not_shared >= 1);
        assert!(!f.dirs.user.join(id).exists());
    }

    #[test]
    fn the_other_generations_id_is_found() {
        let f = fx();
        // The system still ships the Atlas id; the name is kept under the
        // Telamon one.
        fs::write(
            f.dirs.system[0].join("net.eterneon.atlas.settings.desktop"),
            ORIGINAL,
        )
        .unwrap();
        let mut names = Names::default();
        names.set("net.eterneon.telamon.settings.desktop", "Prefs");
        let r = sync(&f.dirs, &names);
        assert_eq!(r.written, 1);
        assert!(
            f.dirs
                .user
                .join("net.eterneon.atlas.settings.desktop")
                .exists()
        );
        assert!(
            !f.dirs
                .user
                .join("net.eterneon.telamon.settings.desktop")
                .exists()
        );
        // And it is recognised as asked for, not swept.
        assert!(!sync(&f.dirs, &names).changed());
    }

    #[test]
    fn a_name_cannot_add_a_key_to_the_desktop_file() {
        // The name goes into a line of a file the whole desktop reads: a
        // line break, a carriage return or any control character would add
        // a key (an Exec of its own).
        for bad in [
            "x\nExec=/bin/sh",
            "x\r\nExec=/bin/sh",
            "x\rExec=/bin/sh",
            "x\u{85}Exec=/bin/sh",
            "x\u{2028}Exec=/bin/sh",
            "x\u{2029}Exec=/bin/sh",
            "x\0y",
            "x\u{1b}[31m",
            "a\u{202e}b",
            "a\u{200b}b",
            "  padded ",
            "two  spaces",
            "tab\there",
            "",
        ] {
            assert!(render(ORIGINAL, bad).is_none(), "{bad:?}");
        }
        assert!(render(ORIGINAL, &"x".repeat(65)).is_none());
        assert!(render(ORIGINAL, &"x".repeat(64)).is_some());
        // Whatever is accepted is one line with one Name and no new Exec.
        let out = render(ORIGINAL, "Fine Name").unwrap();
        assert_eq!(out.matches("\nExec=").count(), 2); // the entry's and the action's
        assert_eq!(out.matches("\nName=").count(), 2);
    }

    #[test]
    fn apply_with_an_unclean_name_writes_nothing() {
        let f = fx();
        let id = "org.example.files.desktop";
        assert_eq!(
            apply(&f.dirs, &[id], "x\nExec=/bin/sh"),
            Outcome::NoOriginal
        );
        assert!(!f.dirs.user.join(id).exists());
    }

    #[test]
    fn a_link_in_the_way_is_neither_read_nor_written_through() {
        let f = fx();
        let id = "org.example.files.desktop";
        fs::create_dir_all(&f.dirs.user).unwrap();
        // A link to a file the user cares about, named like the app's file.
        let precious = f.dirs.user.join("precious.txt");
        fs::write(&precious, "do not touch").unwrap();
        symlink(&precious, f.dirs.user.join(id)).unwrap();
        assert_eq!(apply(&f.dirs, &[id], "N"), Outcome::NotOurs);
        assert_eq!(remove(&f.dirs, id), Outcome::NotOurs);
        assert_eq!(fs::read_to_string(&precious).unwrap(), "do not touch");
        // A dangling link: nothing is created where it points.
        fs::remove_file(f.dirs.user.join(id)).unwrap();
        let out = f.dirs.user.join("created-through-the-link");
        symlink(&out, f.dirs.user.join(id)).unwrap();
        assert_eq!(apply(&f.dirs, &[id], "N"), Outcome::NotOurs);
        assert!(!out.exists());
        // A pipe is not read (it would block) and not replaced.
        fs::remove_file(f.dirs.user.join(id)).unwrap();
        let fifo = std::ffi::CString::new(f.dirs.user.join(id).as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo` is a valid C string.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert_eq!(apply(&f.dirs, &[id], "N"), Outcome::NotOurs);
        assert_eq!(remove(&f.dirs, id), Outcome::NotOurs);
    }

    #[test]
    fn the_marker_alone_in_a_foreign_group_or_file_is_not_ours() {
        // Markers count in [Desktop Entry] only, and as exactly "true".
        let other = "[Desktop Entry]\nType=Application\nName=x\nX-Telamon-Renamed=false\n";
        assert!(!is_ours(other.as_bytes()));
        let group = "[Desktop Entry]\nType=Application\n[Other]\nX-Telamon-Renamed=true\n";
        assert!(!is_ours(group.as_bytes()));
        assert!(!is_ours(&[0xff, 0xfe]));
        assert!(!is_ours(b""));
    }

    /// Runs `body` with `hook` installed in `slot`, removing it afterwards.
    type HookSlot = &'static std::thread::LocalKey<std::cell::RefCell<Option<Box<dyn Fn(&Path)>>>>;

    fn with_hook(slot: HookSlot, hook: impl Fn(&Path) + 'static, body: impl FnOnce()) {
        slot.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
        body();
        slot.with(|h| *h.borrow_mut() = None);
    }

    #[test]
    fn a_link_swapped_in_after_the_look_is_not_read() {
        // The file is ours when `existing` looks (lstat: a regular file); then
        // a link to another file of ours-looking content takes its place. The
        // read must refuse the link (O_NOFOLLOW): NotOurs, nothing written.
        let f = fx();
        let id = "org.example.files.desktop";
        assert_eq!(apply(&f.dirs, &[id], "Mine"), Outcome::Written);
        let other = f.dirs.user.join("other.txt");
        fs::write(&other, render(ORIGINAL, "Someone else").unwrap()).unwrap();
        let (path, target) = (f.dirs.user.join(id), other.clone());
        with_hook(
            &hooks::AFTER_LOOK,
            move |p| {
                if p == path {
                    fs::remove_file(p).unwrap();
                    symlink(&target, p).unwrap();
                }
            },
            || assert_eq!(apply(&f.dirs, &[id], "New name"), Outcome::NotOurs),
        );
        assert!(read_link_is_symlink(&f.dirs.user.join(id)));
        assert!(
            fs::read_to_string(&other)
                .unwrap()
                .contains("Name=Someone else")
        );
    }

    fn read_link_is_symlink(p: &Path) -> bool {
        fs::symlink_metadata(p).unwrap().file_type().is_symlink()
    }

    #[test]
    fn a_link_swapped_in_before_the_write_is_not_written_through() {
        let f = fx();
        let id = "org.example.files.desktop";
        assert_eq!(apply(&f.dirs, &[id], "Mine"), Outcome::Written);
        let precious = f.dirs.user.join("precious.txt");
        fs::write(&precious, "do not touch").unwrap();
        let (path, target) = (f.dirs.user.join(id), precious.clone());
        with_hook(
            &hooks::BEFORE_WRITE,
            move |p| {
                if p == path {
                    fs::remove_file(p).unwrap();
                    symlink(&target, p).unwrap();
                }
            },
            || assert_eq!(apply(&f.dirs, &[id], "New name"), Outcome::Failed),
        );
        assert_eq!(fs::read_to_string(&precious).unwrap(), "do not touch");
        assert!(
            read_link_is_symlink(&f.dirs.user.join(id)),
            "the link is still there"
        );
    }

    #[test]
    fn a_file_that_appears_before_the_write_is_not_replaced() {
        // No override yet: `existing` says Absent; before the write the user's
        // own file appears at the name. RENAME_NOREPLACE keeps it.
        let f = fx();
        let id = "org.example.files.desktop";
        let path = f.dirs.user.join(id);
        let mine = "[Desktop Entry]\nType=Application\nName=Made meanwhile\nExec=x\n";
        with_hook(
            &hooks::BEFORE_WRITE,
            move |p| {
                if p == path {
                    fs::write(p, mine).unwrap();
                }
            },
            || assert_eq!(apply(&f.dirs, &[id], "N"), Outcome::Failed),
        );
        assert_eq!(read(&f, id), mine);
    }
}
