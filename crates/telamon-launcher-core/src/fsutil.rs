//! File helpers for the user's own files: capped reads and atomic writes.
//! Nothing here logs paths (they can carry names the user typed).

use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// Reads a whole file, but never more than `cap` bytes. `Ok(None)` when it
/// doesn't exist; `InvalidData` when it is larger than `cap` (decided by
/// reading at most `cap + 1` bytes, not by trusting the size in the metadata);
/// anything that isn't a regular file is `InvalidInput`. Follows symlinks.
pub fn read_capped(path: &Path, cap: u64) -> io::Result<Option<Vec<u8>>> {
    read_capped_with(path, cap, 0)
}

/// [`read_capped`], but a symlink at `path` itself is refused (`InvalidInput`,
/// opened with `O_NOFOLLOW`) instead of followed: for files in folders other
/// programs may write to (the user's `applications` folder), where a link
/// planted at a name must not make the launcher read, or act on, what it
/// points at. Links in the folders above `path` are still followed.
pub fn read_capped_nofollow(path: &Path, cap: u64) -> io::Result<Option<Vec<u8>>> {
    read_capped_with(path, cap, libc::O_NOFOLLOW)
}

fn read_capped_with(path: &Path, cap: u64, flags: libc::c_int) -> io::Result<Option<Vec<u8>>> {
    // O_NONBLOCK so that opening a FIFO put there by mistake can't hang us.
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | flags)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        // O_NOFOLLOW on a link: ELOOP (a link to a directory may say ENOTDIR
        // on other systems; both are "not a plain file").
        Err(e) if flags != 0 && e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file (a link)",
            ));
        }
        Err(e) => return Err(e),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let mut buf = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > cap {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file is larger than the allowed size",
        ));
    }
    Ok(Some(buf))
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Most symlink hops followed before giving up (the kernel's own limit is 40).
const MAX_LINK_HOPS: usize = 16;

/// Follows `path` while it is a symlink, so a dotfile manager's link is
/// written through, never replaced. A missing final target is fine (it will
/// be created where the link points).
fn resolve_link(path: &Path) -> io::Result<PathBuf> {
    let mut cur = path.to_path_buf();
    for _ in 0..MAX_LINK_HOPS {
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                let target = fs::read_link(&cur)?;
                cur = if target.is_absolute() {
                    target
                } else {
                    cur.parent().unwrap_or(Path::new(".")).join(target)
                };
            }
            Ok(_) => return Ok(cur),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(cur),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("too many levels of symbolic links"))
}

fn cstr(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))
}

/// The part of the target's name a temp name keeps: well under NAME_MAX
/// (255) whatever the target's name is.
fn temp_stem(name: &[u8]) -> Vec<u8> {
    name.iter().copied().take(120).collect()
}

/// Whether `s` is `<digits>-<digits>`, the tail `write_atomic` puts after
/// `.tmp-`.
fn is_temp_tail(s: &[u8]) -> bool {
    let Some(i) = s.iter().position(|&b| b == b'-') else {
        return false;
    };
    let (a, b) = (&s[..i], &s[i + 1..]);
    !a.is_empty()
        && !b.is_empty()
        && a.iter().all(u8::is_ascii_digit)
        && b.iter().all(u8::is_ascii_digit)
}

/// Writes `bytes` to `path` so that a crash leaves either the old file or the
/// new one: a temp file in the same directory (`O_CREAT|O_EXCL`), fsync,
/// rename over the target, fsync of the directory. An existing target keeps
/// its mode; `mode` is for a new file. A failed directory fsync after the
/// rename is only logged (the data is in place). Missing parent
/// directories are created with mode 0700. A symlink at `path` is written
/// through. On any error the temp file is removed.
///
/// The directory is opened once and the temp file, the rename and the unlink
/// all go through that descriptor (`openat`, `renameat`, `unlinkat`), so
/// swapping a path component for a symlink mid-way can't redirect them.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let target = resolve_link(path)?;
    write_in_dir(&target, bytes, mode, false)
}

/// [`write_atomic`] for a name in a folder other programs may write to (the
/// user's `applications` folder): a symlink at `path` is **not** followed. The
/// target must be absent or a regular file (`InvalidInput` for a link, a
/// folder, a pipe...); the new file replaces it by rename, so a link is never
/// written through. A name that is absent is taken with
/// `renameat2(RENAME_NOREPLACE)`, so a file that appeared since the caller
/// looked is not clobbered (`AlreadyExists`). Links in the folders above
/// `path` are followed, as a dotfiles manager may use them.
pub fn write_atomic_nofollow(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    write_in_dir(path, bytes, mode, true)
}

fn write_in_dir(target: &Path, bytes: &[u8], mode: u32, strict: bool) -> io::Result<()> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_owned();
    let dir: &Path = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let dir_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(dir)?;
    let dfd = dir_file.as_raw_fd();

    let final_name = cstr(&name)?;
    // An existing regular target keeps its permissions (the user may have
    // loosened or tightened them); `mode` only applies to a new file.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dfd` is a live directory fd, `final_name` a valid C string and
    // `st` a writable stat buffer. A strict write looks at the name itself
    // (a link is seen as a link), the other at what it points to.
    let seen = unsafe {
        libc::fstatat(
            dfd,
            final_name.as_ptr(),
            &mut st,
            if strict { libc::AT_SYMLINK_NOFOLLOW } else { 0 },
        )
    } == 0;
    let existing = seen && (st.st_mode & libc::S_IFMT) == libc::S_IFREG;
    if strict && seen && !existing {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let noreplace = strict && !seen;
    let mode = if existing {
        // Never setuid, setgid or sticky, whatever the old file had.
        (st.st_mode & 0o777) as u32
    } else {
        mode & 0o777
    };
    let stem = temp_stem(name.as_bytes());
    let mut last_err = None;
    for _ in 0..8 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut tmp = b".".to_vec();
        tmp.extend_from_slice(&stem);
        tmp.extend_from_slice(format!(".tmp-{}-{}", std::process::id(), n).as_bytes());
        let tmp_name = cstr(&OsString::from_vec(tmp))?;
        match write_via_temp(dfd, &tmp_name, &final_name, bytes, mode, noreplace) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last_err = Some(e),
            Err(e) => return Err(e),
            Ok(()) => {
                // Make the rename itself durable. The data is already in
                // place, so a failure here is a warning, not an error.
                if let Err(e) = dir_file.sync_all() {
                    log::warn!("could not sync the folder after a write: {:?}", e.kind());
                }
                return Ok(());
            }
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("could not create a temp file")))
}

/// Removes `.{name}.tmp-<pid>-<n>` files in `dir` last modified more than
/// `older_than` ago: what a crash between creating and renaming a temp file
/// leaves behind. Only regular files with exactly that name shape are removed
/// (so `.{name}.tmp-backup` stays), listed and unlinked through the same
/// folder descriptor (a symlink is never followed). Returns how many were removed;
/// errors only skip the entry.
pub fn sweep_stale_temps(dir: &Path, name: &str, older_than: Duration) -> usize {
    // The prefix is built from the same cut stem `write_atomic` uses.
    let mut prefix = b".".to_vec();
    prefix.extend_from_slice(&temp_stem(name.as_bytes()));
    prefix.extend_from_slice(b".tmp-");
    let Ok(dir_file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(dir)
    else {
        return 0;
    };
    let dfd = dir_file.as_raw_fd();
    // List the directory behind the very descriptor the unlinks use.
    // SAFETY: `dfd` is a live descriptor; the duplicate is owned by the DIR
    // below, which closes it (or is closed here when fdopendir fails).
    let dup = unsafe { libc::fcntl(dfd, libc::F_DUPFD_CLOEXEC, 0) };
    if dup < 0 {
        return 0;
    }
    // SAFETY: `dup` is a valid directory descriptor nobody else uses.
    let dirp = unsafe { libc::fdopendir(dup) };
    if dirp.is_null() {
        // SAFETY: fdopendir failed, so `dup` is still ours to close.
        unsafe { libc::close(dup) };
        return 0;
    }
    // SAFETY: `dirp` is a valid open DIR.
    unsafe { libc::rewinddir(dirp) };
    let now = SystemTime::now();
    let mut names: Vec<CString> = Vec::new();
    loop {
        // SAFETY: `dirp` is valid; the entry is copied before the next call.
        let ent = unsafe { libc::readdir(dirp) };
        if ent.is_null() {
            break;
        }
        // SAFETY: d_name is a NUL-terminated string inside the entry.
        let fname = unsafe { std::ffi::CStr::from_ptr((*ent).d_name.as_ptr()) };
        let bytes = fname.to_bytes();
        if let Some(tail) = bytes.strip_prefix(prefix.as_slice())
            && is_temp_tail(tail)
        {
            names.push(fname.to_owned());
        }
    }
    // SAFETY: `dirp` is valid and closed exactly once (this closes `dup`).
    unsafe { libc::closedir(dirp) };
    let mut removed = 0;
    for c in names {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid dir fd, C string and stat buffer; AT_SYMLINK_NOFOLLOW
        // so a link is seen as a link.
        let ok = unsafe { libc::fstatat(dfd, c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0;
        if !ok || (st.st_mode & libc::S_IFMT) != libc::S_IFREG {
            continue;
        }
        let mtime = SystemTime::UNIX_EPOCH
            + Duration::new(
                st.st_mtime.max(0) as u64,
                st.st_mtime_nsec.clamp(0, 999_999_999) as u32,
            );
        // A future mtime is not "older".
        if now.duration_since(mtime).is_ok_and(|age| age > older_than)
            // SAFETY: as above; unlinkat on a name, never a followed link.
            && unsafe { libc::unlinkat(dfd, c.as_ptr(), 0) } == 0
        {
            removed += 1;
        }
    }
    removed
}

/// Renames `from` to `to` in the folder `dfd`; with `noreplace` the name `to`
/// must not exist (`RENAME_NOREPLACE`; a file system that cannot do that
/// falls back to a plain rename).
fn rename_in(dfd: RawFd, from: &CString, to: &CString, noreplace: bool) -> io::Result<()> {
    if noreplace {
        // SAFETY: both names are valid C strings relative to `dfd`.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                dfd,
                from.as_ptr(),
                dfd,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if !matches!(
            e.raw_os_error(),
            Some(libc::EINVAL | libc::ENOSYS | libc::ENOTSUP)
        ) {
            return Err(e);
        }
    }
    // SAFETY: both names are valid C strings relative to `dfd`.
    if unsafe { libc::renameat(dfd, from.as_ptr(), dfd, to.as_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn write_via_temp(
    dfd: RawFd,
    tmp: &CString,
    final_name: &CString,
    bytes: &[u8],
    mode: u32,
    noreplace: bool,
) -> io::Result<()> {
    // SAFETY: `dfd` is a live directory descriptor and `tmp` a valid C string.
    let fd = unsafe {
        libc::openat(
            dfd,
            tmp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just opened and nothing else owns it.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let result = (|| {
        // The umask may have cut the mode at creation; set it exactly.
        // SAFETY: `file` owns a valid descriptor.
        if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        rename_in(dfd, tmp, final_name, noreplace)
    })();
    drop(file);
    if result.is_err() {
        // SAFETY: as above; best effort, the original error is what matters.
        unsafe { libc::unlinkat(dfd, tmp.as_ptr(), 0) };
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn read_missing_is_none() {
        let d = tempfile::tempdir().unwrap();
        assert!(read_capped(&d.path().join("nope"), 10).unwrap().is_none());
    }

    #[test]
    fn read_cap_is_exact() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        fs::write(&p, b"12345").unwrap();
        assert_eq!(read_capped(&p, 5).unwrap().unwrap(), b"12345");
        let e = read_capped(&p, 4).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        fs::write(&p, b"").unwrap();
        assert_eq!(read_capped(&p, 0).unwrap().unwrap(), b"");
    }

    #[test]
    fn read_follows_symlink_and_rejects_dirs() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        fs::write(&p, b"x").unwrap();
        let l = d.path().join("l");
        symlink(&p, &l).unwrap();
        assert_eq!(read_capped(&l, 10).unwrap().unwrap(), b"x");
        let e = read_capped(d.path(), 10).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn write_creates_dirs_and_mode() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a/b/file");
        write_atomic(&p, b"hello", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"hello");
        let m = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600);
        let dm = fs::metadata(d.path().join("a"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dm, 0o700);
        write_atomic(&p, b"bye", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"bye");
        // No temp files left.
        let n = fs::read_dir(p.parent().unwrap()).unwrap().count();
        assert_eq!(n, 1);
    }

    #[test]
    fn write_goes_through_symlink() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join("dots")).unwrap();
        let real = d.path().join("dots/real");
        fs::write(&real, b"old").unwrap();
        let link = d.path().join("link");
        symlink("dots/real", &link).unwrap();
        write_atomic(&link, b"new", 0o644).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&real).unwrap(), b"new");
        // A dangling link gets its target created.
        let l2 = d.path().join("l2");
        symlink("dots/created", &l2).unwrap();
        write_atomic(&l2, b"c", 0o644).unwrap();
        assert_eq!(fs::read(d.path().join("dots/created")).unwrap(), b"c");
    }

    #[test]
    fn symlink_loop_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        symlink(&b, &a).unwrap();
        symlink(&a, &b).unwrap();
        assert!(write_atomic(&a, b"x", 0o600).is_err());
    }

    #[test]
    fn failure_keeps_old_file_and_leaves_no_temp() {
        let d = tempfile::tempdir().unwrap();
        // Parent is a file: can't create the directory.
        let f = d.path().join("plain");
        fs::write(&f, b"keep").unwrap();
        assert!(write_atomic(&f.join("child"), b"x", 0o600).is_err());
        assert_eq!(fs::read(&f).unwrap(), b"keep");
        // Target is a non-empty directory: rename fails, temp is removed.
        let t = d.path().join("dir");
        fs::create_dir(&t).unwrap();
        fs::write(t.join("inner"), b"i").unwrap();
        assert!(write_atomic(&t, b"x", 0o600).is_err());
        assert!(t.join("inner").exists());
        let names: Vec<_> = fs::read_dir(d.path()).unwrap().collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn existing_target_keeps_its_mode() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        fs::write(&p, b"old").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&p, b"new", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o640
        );
        // A new file gets the asked mode.
        let n = d.path().join("g");
        write_atomic(&n, b"x", 0o600).unwrap();
        assert_eq!(
            fs::metadata(&n).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn sweep_removes_only_old_regular_temps() {
        let d = tempfile::tempdir().unwrap();
        let old = d.path().join(".usage.tsv.tmp-1-0");
        let fresh = d.path().join(".usage.tsv.tmp-1-1");
        let other = d.path().join(".other.tmp-1-0");
        let keep = d.path().join("usage.tsv");
        for f in [&old, &fresh, &other, &keep] {
            fs::write(f, b"x").unwrap();
        }
        let long_ago = SystemTime::now() - Duration::from_secs(7200);
        for f in [&old, &other, &keep] {
            fs::File::options()
                .write(true)
                .open(f)
                .unwrap()
                .set_modified(long_ago)
                .unwrap();
        }
        // A symlink with a matching name is left alone, and so is its target.
        let target = d.path().join("victim");
        fs::write(&target, b"v").unwrap();
        symlink(&target, d.path().join(".usage.tsv.tmp-2-0")).unwrap();
        // So is a directory.
        fs::create_dir(d.path().join(".usage.tsv.tmp-3-0")).unwrap();
        assert_eq!(
            sweep_stale_temps(d.path(), "usage.tsv", Duration::from_secs(3600)),
            1
        );
        assert!(!old.exists() && fresh.exists() && other.exists() && keep.exists());
        assert!(target.exists());
        assert!(d.path().join(".usage.tsv.tmp-3-0").is_dir());
        assert_eq!(
            sweep_stale_temps(&d.path().join("missing"), "x", Duration::ZERO),
            0
        );
    }

    #[test]
    fn long_names_work() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("n".repeat(250));
        write_atomic(&p, b"x", 0o600).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"x");
    }

    #[test]
    fn sweep_needs_the_exact_temp_shape() {
        let d = tempfile::tempdir().unwrap();
        let long_ago = SystemTime::now() - Duration::from_secs(7200);
        let keep_names = [
            ".usage.tsv.tmp-backup",
            ".usage.tsv.tmp-",
            ".usage.tsv.tmp-1",
            ".usage.tsv.tmp-1-",
            ".usage.tsv.tmp--1",
            ".usage.tsv.tmp-1-2-3",
            ".usage.tsv.tmp-a-2",
        ];
        for n in keep_names {
            let f = d.path().join(n);
            fs::write(&f, b"x").unwrap();
            fs::File::options()
                .write(true)
                .open(&f)
                .unwrap()
                .set_modified(long_ago)
                .unwrap();
        }
        let gone = d.path().join(".usage.tsv.tmp-12-34");
        fs::write(&gone, b"x").unwrap();
        fs::File::options()
            .write(true)
            .open(&gone)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        assert_eq!(
            sweep_stale_temps(d.path(), "usage.tsv", Duration::from_secs(3600)),
            1
        );
        assert!(!gone.exists());
        for n in keep_names {
            assert!(d.path().join(n).exists(), "{n}");
        }
    }

    #[test]
    fn sweep_finds_temps_of_long_names() {
        let d = tempfile::tempdir().unwrap();
        let name = "n".repeat(250);
        let tmp = d.path().join(format!(".{}.tmp-5-6", "n".repeat(120)));
        fs::write(&tmp, b"x").unwrap();
        assert_eq!(
            sweep_stale_temps(d.path(), &name, Duration::ZERO.max(Duration::from_nanos(1))),
            1
        );
        assert!(!tmp.exists());
    }

    #[test]
    fn special_mode_bits_are_never_kept_or_set() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        fs::write(&p, b"a").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o4755)).unwrap();
        write_atomic(&p, b"b", 0o600).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o7777,
            0o755
        );
        let n = d.path().join("new");
        write_atomic(&n, b"b", 0o6600).unwrap();
        assert_eq!(
            fs::metadata(&n).unwrap().permissions().mode() & 0o7777,
            0o600
        );
    }

    #[test]
    fn nofollow_read_refuses_a_link_and_reads_a_file() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f");
        fs::write(&f, b"x").unwrap();
        let l = d.path().join("l");
        symlink(&f, &l).unwrap();
        assert_eq!(read_capped_nofollow(&f, 10).unwrap().unwrap(), b"x");
        // The plain read follows the link; the other one does not.
        assert_eq!(read_capped(&l, 10).unwrap().unwrap(), b"x");
        let e = read_capped_nofollow(&l, 10).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        // A dangling link, a link to a folder, a folder: refused, not "absent".
        let dangling = d.path().join("dangling");
        symlink(d.path().join("nowhere"), &dangling).unwrap();
        assert!(read_capped_nofollow(&dangling, 10).is_err());
        let dl = d.path().join("dl");
        symlink(d.path(), &dl).unwrap();
        assert!(read_capped_nofollow(&dl, 10).is_err());
        assert!(read_capped_nofollow(d.path(), 10).is_err());
        assert!(
            read_capped_nofollow(&d.path().join("absent"), 10)
                .unwrap()
                .is_none()
        );
        // Over the cap is still an error.
        assert_eq!(
            read_capped_nofollow(&f, 0).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn nofollow_write_never_writes_through_a_link() {
        let d = tempfile::tempdir().unwrap();
        let victim = d.path().join("victim");
        fs::write(&victim, b"precious").unwrap();
        let l = d.path().join("l.desktop");
        symlink(&victim, &l).unwrap();
        let e = write_atomic_nofollow(&l, b"overwritten", 0o644).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read(&victim).unwrap(), b"precious");
        assert!(fs::symlink_metadata(&l).unwrap().file_type().is_symlink());
        // The ordinary write does follow it (a dotfiles manager's link).
        write_atomic(&l, b"through", 0o600).unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"through");
        // A dangling link is refused too: nothing is created where it points.
        let d2 = d.path().join("d2.desktop");
        let target = d.path().join("created-by-the-link");
        symlink(&target, &d2).unwrap();
        assert!(write_atomic_nofollow(&d2, b"x", 0o644).is_err());
        assert!(!target.exists());
        // A folder, and a pipe, in the way.
        let dir = d.path().join("dir.desktop");
        fs::create_dir(&dir).unwrap();
        assert!(write_atomic_nofollow(&dir, b"x", 0o644).is_err());
        let fifo = d.path().join("fifo.desktop");
        let c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c` is a valid C string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(write_atomic_nofollow(&fifo, b"x", 0o644).is_err());
        // No temp file is left behind by any of it.
        let left: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn nofollow_write_creates_and_replaces_regular_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("new.desktop");
        write_atomic_nofollow(&p, b"one", 0o644).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o644
        );
        // An existing file keeps its mode and is replaced.
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic_nofollow(&p, b"two", 0o644).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o640
        );
        // Setuid, setgid and sticky never come through.
        fs::set_permissions(&p, fs::Permissions::from_mode(0o4755)).unwrap();
        write_atomic_nofollow(&p, b"three", 0o644).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o7777,
            0o755
        );
    }

    #[test]
    fn a_name_that_appeared_is_not_clobbered_by_a_new_file() {
        // The write looked, saw nothing, and made a temp file; then someone
        // made the name. RENAME_NOREPLACE keeps theirs.
        let d = tempfile::tempdir().unwrap();
        let dir_file = File::open(d.path()).unwrap();
        let dfd = dir_file.as_raw_fd();
        fs::write(d.path().join(".tmp"), b"ours").unwrap();
        fs::write(d.path().join("final"), b"theirs").unwrap();
        let (tmp, fin) = (
            CString::new(".tmp").unwrap(),
            CString::new("final").unwrap(),
        );
        let e = rename_in(dfd, &tmp, &fin, true).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(d.path().join("final")).unwrap(), b"theirs");
        // Without the flag the rename replaces, as write_atomic always did.
        rename_in(dfd, &tmp, &fin, false).unwrap();
        assert_eq!(fs::read(d.path().join("final")).unwrap(), b"ours");
    }
}
