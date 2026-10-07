//! The names before the rename to Telamon (`atlas-launcher`).
//!
//! Until 0.3.0 the launcher's files lived under `atlas-launcher` names. The
//! first run of the renamed app moves each one to its new name, once: one
//! `renameat2(RENAME_NOREPLACE)` per folder or file, which is atomic and
//! never replaces anything, so a name that exists already wins and the old
//! one is left alone (a downgrade and a second upgrade lose nothing). Nothing
//! is copied or read from the old name afterwards.
//!
//! Also here: what the launcher reads that other apps own and renamed (their
//! desktop file IDs, Settings' search index, the image's default pins).

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The user folders: `$XDG_CONFIG_HOME/<name>` (`launcher.conf`,
/// `pinned.list`, `state.conf`) and `$XDG_STATE_HOME/<name>` (`usage.tsv`).
pub const OLD_NAME: &str = "atlas-launcher";
pub const NAME: &str = "telamon-launcher";
/// KDE's state config of the runners, `$XDG_STATE_HOME/<name>staterc`.
pub const OLD_STATE_RC: &str = "atlas-launcherstaterc";
pub const NAME_STATE_RC: &str = "telamon-launcherstaterc";

/// What a call to [`migrate_user_files`] did, for the log (counts only: no
/// paths).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Migration {
    pub moved: usize,
    pub failed: usize,
}

/// Moves `base/old` to `base/new`: `Ok(true)` when it moved, `Ok(false)` when
/// there was nothing to move (no old name, or the new one exists already).
/// `want_dir`: the old name must be a folder, otherwise a file; a link to
/// either (a dotfiles manager's) moves as the link, never followed.
pub fn move_once(base: &Path, old: &str, new: &str, want_dir: bool) -> io::Result<bool> {
    let from = base.join(old);
    match std::fs::symlink_metadata(&from) {
        Ok(m) if m.is_symlink() || (want_dir && m.is_dir()) || (!want_dir && m.is_file()) => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(io::Error::other);
    let (a, b) = (c(&from)?, c(&base.join(new))?);
    // SAFETY: both are NUL-terminated paths that outlive the call.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            a.as_ptr(),
            libc::AT_FDCWD,
            b.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::EEXIST | libc::ENOTEMPTY) => Ok(false),
        _ => Err(e),
    }
}

/// Moves the user's launcher files from the old names to the new ones.
/// Never fails: what can't be moved stays where it is and is counted.
pub fn migrate_user_files(config_base: &Path, state_base: &Path) -> Migration {
    let mut m = Migration::default();
    let mut tally = |r: io::Result<bool>| match r {
        Ok(true) => m.moved += 1,
        Ok(false) => {}
        Err(_) => m.failed += 1,
    };
    tally(move_once(config_base, OLD_NAME, NAME, true));
    tally(move_once(state_base, OLD_NAME, NAME, true));
    tally(move_once(state_base, OLD_STATE_RC, NAME_STATE_RC, false));
    m
}

const OLD_ID_PREFIX: &str = "net.eterneon.atlas.";
const ID_PREFIX: &str = "net.eterneon.telamon.";

/// The same app under the other generation's desktop file ID:
/// `net.eterneon.atlas.store.desktop` and `net.eterneon.telamon.store.desktop`.
/// The apps moved to the Telamon names one by one, and a pin or a history
/// line may name either.
pub fn desktop_id_alias(id: &str) -> Option<String> {
    if let Some(rest) = id.strip_prefix(OLD_ID_PREFIX) {
        Some(format!("{ID_PREFIX}{rest}"))
    } else {
        id.strip_prefix(ID_PREFIX)
            .map(|rest| format!("{OLD_ID_PREFIX}{rest}"))
    }
}

/// An Eterneon desktop file ID in its current (Telamon) form. Anything else
/// is returned as it is.
pub fn canonical_desktop_id(id: &str) -> String {
    match id.strip_prefix(OLD_ID_PREFIX) {
        Some(rest) => format!("{ID_PREFIX}{rest}"),
        None => id.to_owned(),
    }
}

/// The first of `paths` that exists, else the first (so a missing file is
/// reported under its current name).
pub fn first_existing(paths: &[&str]) -> PathBuf {
    paths
        .iter()
        .find(|p| Path::new(p).exists())
        .or(paths.first())
        .map_or_else(PathBuf::new, PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn user_files_move_once_and_never_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let (cfg, st) = (d.path().join("config"), d.path().join("state"));
        fs::create_dir_all(cfg.join(OLD_NAME)).unwrap();
        fs::create_dir_all(st.join(OLD_NAME)).unwrap();
        fs::write(cfg.join(OLD_NAME).join("pinned.list"), "a.desktop\n").unwrap();
        fs::write(
            cfg.join(OLD_NAME).join("launcher.conf"),
            "[Search]\nWebSearch=false\n",
        )
        .unwrap();
        fs::write(
            st.join(OLD_NAME).join("usage.tsv"),
            "q\tapp:a.desktop\t3\t100\n",
        )
        .unwrap();
        fs::write(st.join(OLD_STATE_RC), "[General]\nx=1\n").unwrap();

        let m = migrate_user_files(&cfg, &st);
        assert_eq!(
            m,
            Migration {
                moved: 3,
                failed: 0
            }
        );
        assert!(
            !cfg.join(OLD_NAME).exists()
                && !st.join(OLD_NAME).exists()
                && !st.join(OLD_STATE_RC).exists()
        );
        assert_eq!(
            fs::read_to_string(cfg.join(NAME).join("pinned.list")).unwrap(),
            "a.desktop\n"
        );
        assert_eq!(
            fs::read_to_string(cfg.join(NAME).join("launcher.conf")).unwrap(),
            "[Search]\nWebSearch=false\n"
        );
        assert_eq!(
            fs::read_to_string(st.join(NAME).join("usage.tsv")).unwrap(),
            "q\tapp:a.desktop\t3\t100\n"
        );
        assert_eq!(
            fs::read_to_string(st.join(NAME_STATE_RC)).unwrap(),
            "[General]\nx=1\n"
        );

        // The second run has nothing to move.
        assert_eq!(migrate_user_files(&cfg, &st), Migration::default());
        // An old folder that comes back (a downgrade) does not replace the new one.
        fs::create_dir_all(cfg.join(OLD_NAME)).unwrap();
        fs::write(cfg.join(OLD_NAME).join("pinned.list"), "old.desktop\n").unwrap();
        assert_eq!(migrate_user_files(&cfg, &st), Migration::default());
        assert_eq!(
            fs::read_to_string(cfg.join(NAME).join("pinned.list")).unwrap(),
            "a.desktop\n"
        );
        assert!(cfg.join(OLD_NAME).join("pinned.list").exists());
    }

    #[test]
    fn nothing_there_is_not_an_error_and_a_link_moves_as_a_link() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            migrate_user_files(d.path(), &d.path().join("none")),
            Migration::default()
        );
        // A dotfiles manager's link: the link moves, its target is not touched.
        let target = d.path().join("dotfiles");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("pinned.list"), "a.desktop\n").unwrap();
        std::os::unix::fs::symlink(&target, d.path().join(OLD_NAME)).unwrap();
        assert_eq!(
            migrate_user_files(d.path(), &d.path().join("none")),
            Migration {
                moved: 1,
                failed: 0
            }
        );
        assert!(d.path().join(OLD_NAME).symlink_metadata().is_err());
        assert!(
            d.path()
                .join(NAME)
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(d.path().join(NAME).join("pinned.list")).unwrap(),
            "a.desktop\n"
        );
        assert!(target.join("pinned.list").exists());
    }

    #[test]
    fn desktop_ids_have_an_alias_each_way() {
        assert_eq!(
            desktop_id_alias("net.eterneon.atlas.store.desktop").as_deref(),
            Some("net.eterneon.telamon.store.desktop")
        );
        assert_eq!(
            desktop_id_alias("net.eterneon.telamon.store.desktop").as_deref(),
            Some("net.eterneon.atlas.store.desktop")
        );
        assert_eq!(desktop_id_alias("org.kde.dolphin.desktop"), None);
        assert_eq!(
            canonical_desktop_id("net.eterneon.atlas.settings.desktop"),
            "net.eterneon.telamon.settings.desktop"
        );
        assert_eq!(
            canonical_desktop_id("net.eterneon.telamon.settings.desktop"),
            "net.eterneon.telamon.settings.desktop"
        );
        assert_eq!(
            canonical_desktop_id("org.kde.dolphin.desktop"),
            "org.kde.dolphin.desktop"
        );
    }

    #[test]
    fn first_existing_prefers_the_first_that_is_there() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        let (sa, sb) = (a.to_str().unwrap(), b.to_str().unwrap());
        assert_eq!(
            first_existing(&[sa, sb]),
            a,
            "neither: the first, the current name"
        );
        fs::write(&b, "x").unwrap();
        assert_eq!(first_existing(&[sa, sb]), b);
        fs::write(&a, "x").unwrap();
        assert_eq!(first_existing(&[sa, sb]), a);
    }
}
