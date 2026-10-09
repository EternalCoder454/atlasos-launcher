//! Commands in search: the session commands (Lock, Sleep, Restart...) and
//! executables on `PATH` typed with arguments. Nothing here runs anything;
//! each result carries the `Action` the C++ side performs (docs/DESIGN.md,
//! "Commands" and "Trust").

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::result::{Action, Kind, ResultItem, SessionAction, prior};
use crate::text::{MAX_QUERY_CHARS, Prepared, Query, clean_display, is_unsafe_char, score_fields};

/// Most `PATH` folders scanned.
pub const MAX_PATH_DIRS: usize = 64;
/// Most executable names kept.
pub const MAX_NAMES: usize = 20_000;
/// Longest executable name, in bytes.
pub const MAX_NAME_BYTES: usize = 255;
/// Most directory entries looked at in one `PATH` folder.
pub const MAX_ENTRIES_PER_DIR: usize = 10_000;
/// Most directory entries looked at in all `PATH` folders together.
pub const MAX_ENTRIES_TOTAL: usize = 100_000;
/// Longest command line shown in a row title, in characters.
const MAX_SHOWN_CHARS: usize = 512;
/// Telamon OS's command menu, the justfile `telamon` (and `atlas`) runs.
pub const TELAMON_JUSTFILE: &str = "/usr/share/telamon/telamon.just";
/// The executables that are that command menu.
const TELAMON_MENU_NAMES: &[&str] = &["telamon", "atlas"];
/// Largest justfile read, in bytes.
const MAX_JUSTFILE_BYTES: u64 = 256 * 1024;
/// Most recipe names kept.
const MAX_RECIPES: usize = 512;

// ---------------------------------------------------------------------------
// Session commands

/// Which optional session commands this system offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionAvailability {
    pub hibernate: bool,
    pub switch_user: bool,
}

impl Default for SessionAvailability {
    fn default() -> Self {
        SessionAvailability {
            hibernate: true,
            switch_user: true,
        }
    }
}

struct SessionDef {
    name: &'static str,
    title: &'static str,
    subtitle: &'static str,
    icon: &'static str,
    aliases: &'static [&'static str],
    action: SessionAction,
}

const SESSION_DEFS: &[SessionDef] = &[
    SessionDef {
        name: "lock",
        title: "Lock",
        subtitle: "Lock the screen",
        icon: "system-lock-screen",
        aliases: &["lock screen", "lock session", "screen lock"],
        action: SessionAction::Lock,
    },
    SessionDef {
        name: "sleep",
        title: "Sleep",
        subtitle: "Suspend to memory",
        icon: "system-suspend",
        aliases: &["suspend", "standby"],
        action: SessionAction::Sleep,
    },
    SessionDef {
        name: "hibernate",
        title: "Hibernate",
        subtitle: "Suspend to disk",
        icon: "system-suspend-hibernate",
        aliases: &["suspend to disk"],
        action: SessionAction::Hibernate,
    },
    SessionDef {
        name: "switch-user",
        title: "Switch User",
        subtitle: "Start or switch to another session",
        icon: "system-switch-user",
        aliases: &["change user", "switch account", "new session"],
        action: SessionAction::SwitchUser,
    },
    SessionDef {
        name: "logout",
        title: "Log Out",
        subtitle: "End this session",
        icon: "system-log-out",
        aliases: &[
            "logout",
            "sign out",
            "signout",
            "log off",
            "logoff",
            "exit session",
        ],
        action: SessionAction::LogOut,
    },
    SessionDef {
        name: "restart",
        title: "Restart",
        subtitle: "Restart the computer",
        icon: "system-reboot",
        aliases: &["reboot"],
        action: SessionAction::Restart,
    },
    SessionDef {
        name: "shutdown",
        title: "Shut Down",
        subtitle: "Turn the computer off",
        icon: "system-shutdown",
        aliases: &["shutdown", "power off", "poweroff", "turn off", "halt"],
        action: SessionAction::ShutDown,
    },
];

struct SessionEntry {
    def: &'static SessionDef,
    title: Prepared,
    aliases: Vec<Prepared>,
}

/// The session commands offered, prepared once.
pub struct SessionCommands {
    entries: Vec<SessionEntry>,
}

impl SessionCommands {
    pub fn new(avail: SessionAvailability) -> Self {
        let entries = SESSION_DEFS
            .iter()
            .filter(|d| match d.action {
                SessionAction::Hibernate => avail.hibernate,
                SessionAction::SwitchUser => avail.switch_user,
                _ => true,
            })
            .map(|def| SessionEntry {
                def,
                title: Prepared::new(def.title),
                aliases: def.aliases.iter().map(|a| Prepared::new(a)).collect(),
            })
            .collect();
        SessionCommands { entries }
    }

    /// Commands matching a query of at least 2 characters, unsorted. Asking
    /// for confirmation is the C++ side's job (Plasma's prompt).
    pub fn search(&self, q: &Query) -> Vec<ResultItem> {
        if q.folded.chars().count() < 2 {
            return Vec::new();
        }
        let p = prior(Kind::Session);
        self.entries
            .iter()
            .filter_map(|e| {
                let m = score_fields(q, &e.title, &e.aliases, None)?;
                Some(ResultItem {
                    id: format!("session:{}", e.def.name),
                    kind: Kind::Session,
                    title: e.def.title.to_owned(),
                    subtitle: e.def.subtitle.to_owned(),
                    icon: e.def.icon.to_owned(),
                    score: m * p,
                    action: Action::Session(e.def.action),
                })
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Command lines

/// What a typed command line can become.
#[derive(Clone, PartialEq, Eq)]
pub enum CommandPlan {
    /// No words at all.
    Empty,
    /// Plain words, run without a shell.
    Direct {
        executable: String,
        args: Vec<String>,
    },
    /// Uses shell syntax (or has unbalanced quotes): only the terminal's
    /// shell may run it.
    TerminalOnly,
}

/// Redacted: the kind and word count only, never the command text.
impl std::fmt::Debug for CommandPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandPlan::Empty => f.write_str("CommandPlan::Empty"),
            CommandPlan::Direct { args, .. } => f
                .debug_struct("CommandPlan::Direct")
                .field("args", &args.len())
                .finish_non_exhaustive(),
            CommandPlan::TerminalOnly => f.write_str("CommandPlan::TerminalOnly"),
        }
    }
}

/// Characters that mean shell syntax outside quotes. `[` is here because a
/// shell would glob it.
const SHELL_META: &[char] = &[
    '|', ';', '&', '$', '>', '<', '`', '(', ')', '{', '}', '*', '?', '~', '\n', '\r', '[',
];

/// Splits a line like a POSIX shell's word splitting, with single and double
/// quotes and backslash escapes, but no expansion of any kind. A line with
/// shell syntax or unbalanced quotes is [`CommandPlan::TerminalOnly`].
pub fn parse_command_line(line: &str) -> CommandPlan {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' | '\r' => return CommandPlan::TerminalOnly,
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return CommandPlan::TerminalOnly,
                        Some('\'') => break,
                        Some('\n' | '\r') => return CommandPlan::TerminalOnly,
                        Some(c) => cur.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        None => return CommandPlan::TerminalOnly,
                        Some('"') => break,
                        Some('$' | '`' | '\n' | '\r') => return CommandPlan::TerminalOnly,
                        Some('\\') => match chars.next() {
                            None => return CommandPlan::TerminalOnly,
                            Some(e @ ('"' | '\\' | '$' | '`')) => cur.push(e),
                            Some('\n' | '\r') => return CommandPlan::TerminalOnly,
                            Some(other) => {
                                cur.push('\\');
                                cur.push(other);
                            }
                        },
                        Some(c) => cur.push(c),
                    }
                }
            }
            '\\' => match chars.next() {
                None | Some('\n' | '\r') => return CommandPlan::TerminalOnly,
                Some(e) => {
                    in_word = true;
                    cur.push(e);
                }
            },
            '#' if !in_word => return CommandPlan::TerminalOnly, // a comment
            c if SHELL_META.contains(&c) => return CommandPlan::TerminalOnly,
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    let mut it = words.into_iter();
    match it.next() {
        None => CommandPlan::Empty,
        Some(executable) => CommandPlan::Direct {
            executable,
            args: it.collect(),
        },
    }
}

// ---------------------------------------------------------------------------
// PATH executables

/// Whether the current user may execute `path` (`faccessat` with
/// `AT_EACCESS`, so the effective ids count; follows symlinks).
fn can_execute(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` is a valid NUL-terminated string that outlives the call.
    unsafe { libc::faccessat(libc::AT_FDCWD, c.as_ptr(), libc::X_OK, libc::AT_EACCESS) == 0 }
}

/// The line as typed with unsafe characters removed (spacing kept), cut at
/// 512 characters with "…".
fn shown_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len().min(MAX_SHOWN_CHARS * 4));
    for (n, c) in line.chars().filter(|c| !is_unsafe_char(*c)).enumerate() {
        if n >= MAX_SHOWN_CHARS {
            out.pop();
            out.push('…');
            break;
        }
        out.push(c);
    }
    out
}

/// Whether a `PATH` entry is an absolute path the launcher will run things from:
/// a leading `/` (not `//`) and no `..` segment. The C++ side requires the same
/// of the program it starts (`Validate::commandPath`), so a row is never offered
/// for a path it would refuse.
fn plain_absolute_dir(dir: &str) -> bool {
    dir.starts_with('/') && !dir.starts_with("//") && !dir.split('/').any(|seg| seg == "..")
}

/// The executables on `PATH`, listed once (off the GUI thread). Typed
/// commands must name one of these exactly.
#[derive(Debug, Default)]
pub struct PathCache {
    /// Name to absolute path; the first `PATH` folder wins.
    exes: HashMap<String, String>,
    /// Each scanned folder with its modification time at scan time.
    dirs: Vec<(PathBuf, Option<SystemTime>)>,
    /// Executables that are a command menu, with the commands it has. The
    /// first word after the name must be one of these (or a flag), or no
    /// "Run" row is offered: `telamon g` is the start of an app's name, not
    /// a command (every Telamon app's name starts with "Telamon").
    menus: HashMap<String, HashSet<String>>,
}

/// The names of the public recipes and aliases in a justfile's text, read
/// like `just --summary` would list them: private recipes (`[private]` or a
/// leading `_`) are left out. Nothing is run; this is a plain line reading
/// of the top level, which is all the Telamon justfile uses.
pub fn justfile_recipes(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut private = false;
    for line in text.lines() {
        if out.len() >= MAX_RECIPES {
            break;
        }
        if line.starts_with(char::is_whitespace) || line.is_empty() {
            continue; // a recipe body (or a blank line)
        }
        if line.starts_with('#') {
            continue; // a doc comment sits between attributes and the recipe
        }
        if line.starts_with('[') {
            if line.contains("private") {
                private = true;
            }
            continue;
        }
        let is_private = std::mem::take(&mut private);
        let line = line.strip_prefix('@').unwrap_or(line);
        let end = line
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(line.len());
        let (word, rest) = line.split_at(end);
        if word.is_empty() || !word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            continue;
        }
        let rest = rest.trim_start();
        if word == "alias" {
            let Some(name) = rest
                .split(":=")
                .next()
                .map(str::trim)
                .filter(|n| !n.is_empty() && !n.contains(char::is_whitespace))
            else {
                continue;
            };
            if !name.starts_with('_') {
                out.insert(name.to_owned());
            }
            continue;
        }
        // `name := value`, `set x`, `import 'f'`, `export x := y`: not recipes.
        if rest.starts_with(":=") || rest.starts_with('=') {
            continue;
        }
        if matches!(word, "set" | "import" | "mod" | "export" | "unexport") {
            continue;
        }
        if !rest.contains(':') || is_private || word.starts_with('_') {
            continue;
        }
        out.insert(word.to_owned());
    }
    out
}

impl PathCache {
    /// Lists the regular files the user may execute in each absolute folder
    /// of `path_var` (at most 64 folders, 20,000 names, 10,000 entries looked
    /// at per folder and 100,000 in all; names of at most 255 bytes and valid
    /// UTF-8). Symlinks count by what they point to. File
    /// IO: call it off the GUI thread.
    pub fn scan(path_var: &str) -> PathCache {
        Self::scan_with_limits(path_var, MAX_ENTRIES_PER_DIR, MAX_ENTRIES_TOTAL)
    }

    /// [`PathCache::scan`] with the entry caps as parameters (for tests).
    fn scan_with_limits(path_var: &str, per_dir: usize, total: usize) -> PathCache {
        let mut cache = PathCache::default();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut visited_total = 0usize;
        for dir in path_var.split(':') {
            if cache.dirs.len() >= MAX_PATH_DIRS {
                break;
            }
            if !plain_absolute_dir(dir) || !seen.insert(dir) {
                continue;
            }
            let dir_path = Path::new(dir);
            let mtime = std::fs::metadata(dir_path).and_then(|m| m.modified()).ok();
            cache.dirs.push((dir_path.to_path_buf(), mtime));
            let Ok(rd) = std::fs::read_dir(dir_path) else {
                continue;
            };
            for (n, entry) in rd.flatten().enumerate() {
                if cache.exes.len() >= MAX_NAMES || n >= per_dir || visited_total >= total {
                    break;
                }
                visited_total += 1;
                let name_os = entry.file_name();
                let Some(name) = name_os.to_str() else {
                    continue;
                };
                if name.is_empty() || name.len() > MAX_NAME_BYTES || cache.exes.contains_key(name) {
                    continue;
                }
                let full = entry.path();
                let Ok(md) = std::fs::metadata(&full) else {
                    continue;
                };
                if !md.is_file() || md.permissions().mode() & 0o111 == 0 || !can_execute(&full) {
                    continue;
                }
                let Some(full) = full.to_str() else { continue };
                cache.exes.insert(name.to_owned(), full.to_owned());
            }
        }
        cache.load_menus(Path::new(TELAMON_JUSTFILE));
        cache
    }

    /// Reads Telamon's command menu once, here at scan time, so a typed
    /// `telamon <word>` is only offered as a command when the word is one. A
    /// missing or unreadable justfile leaves `telamon` as any other program.
    fn load_menus(&mut self, justfile: &Path) {
        if !TELAMON_MENU_NAMES
            .iter()
            .any(|n| self.exes.contains_key(*n))
        {
            return;
        }
        let Ok(Some(bytes)) = crate::fsutil::read_capped(justfile, MAX_JUSTFILE_BYTES) else {
            return;
        };
        let recipes = justfile_recipes(&String::from_utf8_lossy(&bytes));
        for name in TELAMON_MENU_NAMES {
            if self.exes.contains_key(*name) {
                self.menus.insert((*name).to_owned(), recipes.clone());
            }
        }
    }

    /// Makes `name` a command menu with these commands (tests).
    #[cfg(test)]
    pub(crate) fn set_menu(&mut self, name: &str, recipes: &[&str]) {
        self.menus.insert(
            name.to_owned(),
            recipes.iter().map(|r| (*r).to_owned()).collect(),
        );
    }

    /// A cache of these executables, name to path (tests).
    #[cfg(test)]
    pub(crate) fn with_exes(names: &[&str]) -> PathCache {
        let mut c = PathCache::default();
        for n in names {
            c.exes.insert((*n).to_owned(), format!("/usr/bin/{n}"));
        }
        c
    }

    pub fn len(&self) -> usize {
        self.exes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.exes.is_empty()
    }

    /// The absolute path of the executable called exactly `name`.
    pub fn lookup(&self, name: &str) -> Option<&str> {
        self.exes.get(name).map(String::as_str)
    }

    /// Whether any scanned folder changed since the scan (one `stat` per
    /// folder, at most 64). Call it off the GUI thread.
    pub fn is_stale(&self) -> bool {
        self.dirs
            .iter()
            .any(|(dir, then)| std::fs::metadata(dir).and_then(|m| m.modified()).ok() != *then)
    }

    /// "Run" rows for a query whose first word is exactly an executable on
    /// `PATH`: a direct run when the line has no shell syntax (0.9 × prior)
    /// and always one in the terminal (0.85 × prior).
    pub fn search(&self, q: &Query) -> Vec<ResultItem> {
        // A query cut at the cap may not be what the user typed: run nothing.
        if q.raw.chars().take(MAX_QUERY_CHARS).count() >= MAX_QUERY_CHARS {
            return Vec::new();
        }
        // A line whose shown text could differ from what runs (odd spaces,
        // invisible or bidi characters) is offered nothing.
        if q.raw
            .chars()
            .any(|c| (c.is_whitespace() && c != ' ') || is_unsafe_char(c))
        {
            return Vec::new();
        }
        let line = q.raw.trim();
        let Some(first) = line.split_whitespace().next() else {
            return Vec::new();
        };
        // The first word as the shell would read it, for a quoted name too.
        let plan = parse_command_line(line);
        let name = match &plan {
            CommandPlan::Direct { executable, .. } => executable.as_str(),
            _ => first,
        };
        let Some(path) = self.exes.get(name) else {
            return Vec::new();
        };
        if let Some(recipes) = self.menus.get(name) {
            let ok = match &plan {
                CommandPlan::Direct { args, .. } => args
                    .first()
                    .is_none_or(|a| a.starts_with('-') || recipes.contains(a)),
                _ => false,
            };
            if !ok {
                return Vec::new();
            }
        }
        let p = prior(Kind::Command);
        let shown = shown_line(line);
        let mut out = Vec::with_capacity(2);
        if let CommandPlan::Direct { args, .. } = plan {
            out.push(ResultItem {
                id: format!("run:{line}"),
                kind: Kind::Command,
                title: format!("Run ‘{shown}’"),
                // Shown cleaned; the action keeps the path as it is.
                subtitle: clean_display(path),
                icon: "system-run".to_owned(),
                score: 0.9 * p,
                action: Action::Run {
                    executable: path.clone(),
                    args,
                },
            });
        }
        out.push(ResultItem {
            id: format!("term:{line}"),
            kind: Kind::Command,
            title: format!("Run ‘{shown}’ in Terminal"),
            subtitle: "Terminal".to_owned(),
            icon: "utilities-terminal".to_owned(),
            score: 0.85 * p,
            action: Action::RunInTerminal {
                line: line.to_owned(),
            },
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    fn q(s: &str) -> Query {
        Query::new(s, 0)
    }

    fn direct(exe: &str, args: &[&str]) -> CommandPlan {
        CommandPlan::Direct {
            executable: exe.into(),
            args: args.iter().map(|s| (*s).into()).collect(),
        }
    }

    const SAMPLE_JUSTFILE: &str = "# comment\nset shell := [\"bash\"]\nname := \"x\"\nalias up := update\n\n[private]\ndefault:\n    @echo hi\n\n# Which OS\ninfo:\n    echo a: b\n\n# Follow a channel\nchannel name:\n    echo {{ name }}\n\n_hidden:\n    true\n\n[private]\n# doc after attribute\nsecret:\n    true\n\n@update force=\"no\":\n    true\n\nbuild target=\"a:b\": dep\n    true\n";

    #[test]
    fn reads_public_recipe_names() {
        let r = justfile_recipes(SAMPLE_JUSTFILE);
        let mut names: Vec<&str> = r.iter().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["build", "channel", "info", "up", "update"]);
        assert!(justfile_recipes("").is_empty());
        assert!(justfile_recipes("\u{0}\u{1}garbage ::: \n  x: y").is_empty());
    }

    #[test]
    fn a_command_menu_only_offers_its_commands() {
        let mut c = PathCache::with_exes(&["telamon", "htop"]);
        c.set_menu("telamon", &["update", "info"]);
        let ids =
            |line: &str| -> Vec<String> { c.search(&q(line)).into_iter().map(|r| r.id).collect() };
        assert_eq!(
            ids("telamon update"),
            ["run:telamon update", "term:telamon update"]
        );
        assert_eq!(ids("telamon"), ["run:telamon", "term:telamon"]);
        assert_eq!(ids("telamon --list").len(), 2);
        assert_eq!(ids("telamon update now").len(), 2);
        assert!(ids("telamon g").is_empty());
        assert!(ids("telamon upd").is_empty());
        assert!(ids("telamon set").is_empty());
        assert!(ids("telamon info | cat").is_empty());
        // Other programs are not affected.
        assert_eq!(ids("htop g").len(), 2);
    }

    #[test]
    fn scan_reads_the_command_menu_next_to_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for n in ["telamon", "htop"] {
            let f = bin.join(n);
            std::fs::write(&f, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let jf = dir.path().join("t.just");
        std::fs::write(&jf, SAMPLE_JUSTFILE).unwrap();
        let mut c = PathCache::scan(bin.to_str().unwrap());
        c.load_menus(&jf);
        assert!(!c.search(&q("telamon update")).is_empty());
        assert!(c.search(&q("telamon g")).is_empty());
        assert!(!c.search(&q("htop g")).is_empty());
        // No justfile: telamon is any other program.
        let mut c = PathCache::scan(bin.to_str().unwrap());
        c.load_menus(&dir.path().join("absent.just"));
        assert!(!c.search(&q("telamon g")).is_empty());
    }

    #[test]
    fn splits_words() {
        assert_eq!(parse_command_line(""), CommandPlan::Empty);
        assert_eq!(parse_command_line("   \t "), CommandPlan::Empty);
        assert_eq!(parse_command_line("ls"), direct("ls", &[]));
        assert_eq!(
            parse_command_line("  ls  -la\t/tmp "),
            direct("ls", &["-la", "/tmp"])
        );
        assert_eq!(
            parse_command_line(r#"echo "a b" 'c d' e\ f"#),
            direct("echo", &["a b", "c d", "e f"])
        );
        assert_eq!(parse_command_line(r#"x "" ''"#), direct("x", &["", ""]));
        assert_eq!(parse_command_line(r#"x a"b c"d"#), direct("x", &["ab cd"]));
        assert_eq!(
            parse_command_line(r#"x 'a\b' "a\b" "a\"b" "a\\b""#),
            direct("x", &["a\\b", "a\\b", "a\"b", "a\\b"])
        );
        assert_eq!(
            parse_command_line(r"x \| \$HOME \* \~"),
            direct("x", &["|", "$HOME", "*", "~"])
        );
        assert_eq!(
            parse_command_line("x '|;&$><`(){}*?~'"),
            direct("x", &["|;&$><`(){}*?~"])
        );
        assert_eq!(
            parse_command_line(r#"x "|;&><(){}*?~""#),
            direct("x", &["|;&><(){}*?~"])
        );
        assert_eq!(parse_command_line(r#"x "a\$b""#), direct("x", &["a$b"]));
        assert_eq!(parse_command_line("x a#b"), direct("x", &["a#b"]));
        assert_eq!(parse_command_line("'my prog' a"), direct("my prog", &["a"]));
        assert_eq!(parse_command_line("café ü"), direct("café", &["ü"]));
    }

    #[test]
    fn shell_syntax_is_terminal_only() {
        for c in [
            "|", ";", "&", "$", ">", "<", "`", "(", ")", "{", "}", "*", "?", "~", "\n", "[",
        ] {
            for line in [format!("ls {c}"), format!("ls a{c}b"), format!("ls{c}")] {
                assert_eq!(
                    parse_command_line(&line),
                    CommandPlan::TerminalOnly,
                    "{line:?}"
                );
            }
        }
        for line in [
            "ls 'a",
            "ls \"a",
            "ls a\\",
            "ls 'a\nb'",
            "echo \"$HOME\"",
            "echo \"`x`\"",
            "ls && ls",
            "ls > f",
            "# c",
            "ls \\\nx",
            "ls \"a\\\nb\"",
            "ls\rx",
        ] {
            assert_eq!(
                parse_command_line(line),
                CommandPlan::TerminalOnly,
                "{line:?}"
            );
        }
    }

    #[test]
    fn session_commands() {
        let all = SessionCommands::new(SessionAvailability::default());
        let ids = |s: &str| -> Vec<String> {
            let mut r = all.search(&q(s));
            r.sort_by(ResultItem::rank_cmp);
            r.into_iter().map(|i| i.id).collect()
        };
        assert_eq!(ids("lock")[0], "session:lock");
        assert_eq!(ids("lock screen")[0], "session:lock");
        assert_eq!(ids("suspend")[0], "session:sleep");
        assert_eq!(ids("reboot")[0], "session:restart");
        assert_eq!(ids("shutdown")[0], "session:shutdown");
        assert_eq!(ids("shut down")[0], "session:shutdown");
        assert_eq!(ids("power off")[0], "session:shutdown");
        assert_eq!(ids("logout")[0], "session:logout");
        assert_eq!(ids("sign out")[0], "session:logout");
        assert_eq!(ids("hibernate")[0], "session:hibernate");
        assert_eq!(ids("switch user")[0], "session:switch-user");
        assert!(ids("l").is_empty());
        assert!(ids("").is_empty());
        assert!(ids("zzqx").is_empty());
        let r = all.search(&q("restart"));
        assert_eq!(r[0].kind, Kind::Session);
        assert_eq!(r[0].icon, "system-reboot");
        assert_eq!(r[0].action, Action::Session(SessionAction::Restart));
        assert!((r[0].score - 0.8).abs() < 1e-6);

        let some = SessionCommands::new(SessionAvailability {
            hibernate: false,
            switch_user: false,
        });
        assert!(
            some.search(&q("hibernate"))
                .iter()
                .all(|i| i.id != "session:hibernate")
        );
        assert!(
            some.search(&q("switch user"))
                .iter()
                .all(|i| i.id != "session:switch-user")
        );
        assert!(!some.search(&q("lock")).is_empty());
    }

    fn make_exe(dir: &Path, name: &str, mode: u32) {
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn path_scan() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        make_exe(&a, "tool", 0o755);
        make_exe(&a, "plain", 0o644);
        make_exe(&a, "groupx", 0o010);
        make_exe(&b, "tool", 0o755);
        make_exe(&b, "other", 0o755);
        std::os::unix::fs::symlink(a.join("tool"), a.join("link")).unwrap();
        std::os::unix::fs::symlink(a.join("plain"), a.join("link-plain")).unwrap();
        std::os::unix::fs::symlink(a.join("missing"), a.join("dangling")).unwrap();
        std::fs::create_dir(a.join("adir")).unwrap();
        std::fs::set_permissions(a.join("adir"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(a.join(std::ffi::OsStr::from_bytes(b"bad\xff")), "x").unwrap();
        std::fs::set_permissions(
            a.join(std::ffi::OsStr::from_bytes(b"bad\xff")),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let var = format!(
            ":relative/dir:{}::{}:{}:/nonexistent-dir",
            a.display(),
            b.display(),
            a.display()
        );
        let c = PathCache::scan(&var);
        assert_eq!(c.lookup("tool"), Some(a.join("tool").to_str().unwrap()));
        assert_eq!(c.lookup("other"), Some(b.join("other").to_str().unwrap()));
        assert!(c.lookup("link").is_some());
        assert!(c.lookup("groupx").is_some());
        for no in ["plain", "link-plain", "dangling", "adir", "bad\u{fffd}"] {
            assert!(c.lookup(no).is_none(), "{no}");
        }
        assert_eq!(c.len(), 4);
        assert!(!c.is_stale());
        assert!(PathCache::scan("").is_empty());
        assert!(PathCache::scan("rel:also/rel").is_empty());
    }

    #[test]
    fn path_scan_caps_and_staleness() {
        let t = tempfile::tempdir().unwrap();
        let dirs: Vec<String> = (0..70)
            .map(|i| {
                let d = t.path().join(format!("d{i}"));
                std::fs::create_dir(&d).unwrap();
                d.to_str().unwrap().to_owned()
            })
            .collect();
        make_exe(Path::new(&dirs[0]), "first", 0o755);
        make_exe(Path::new(&dirs[69]), "last", 0o755);
        let c = PathCache::scan(&dirs.join(":"));
        assert!(c.lookup("first").is_some());
        assert!(c.lookup("last").is_none());
        assert_eq!(c.dirs.len(), MAX_PATH_DIRS);

        let c = PathCache::scan(&dirs[0]);
        assert!(!c.is_stale());
        make_exe(Path::new(&dirs[0]), "new", 0o755);
        let f = std::fs::File::open(&dirs[0]).unwrap();
        f.set_modified(SystemTime::now() + std::time::Duration::from_secs(30))
            .unwrap();
        assert!(c.is_stale());
        std::fs::remove_dir_all(&dirs[0]).unwrap();
        assert!(c.is_stale());
    }

    #[test]
    fn run_rows() {
        let t = tempfile::tempdir().unwrap();
        make_exe(t.path(), "mytool", 0o755);
        let c = PathCache::scan(t.path().to_str().unwrap());
        let exe = t.path().join("mytool").to_str().unwrap().to_owned();

        let r = c.search(&q("mytool -v 'a b'"));
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].id, "run:mytool -v 'a b'");
        assert_eq!(r[0].title, "Run ‘mytool -v 'a b'’");
        assert_eq!(
            r[0].action,
            Action::Run {
                executable: exe.clone(),
                args: vec!["-v".into(), "a b".into()]
            }
        );
        assert!((r[0].score - 0.72).abs() < 1e-6);
        assert_eq!(r[1].id, "term:mytool -v 'a b'");
        assert_eq!(r[1].title, "Run ‘mytool -v 'a b'’ in Terminal");
        assert_eq!(
            r[1].action,
            Action::RunInTerminal {
                line: "mytool -v 'a b'".into()
            }
        );
        assert!((r[1].score - 0.68).abs() < 1e-6);
        assert_eq!(r[0].kind, Kind::Command);

        // Shell syntax: the terminal row only.
        let r = c.search(&q("mytool | cat"));
        assert_eq!(r.len(), 1);
        assert!(r[0].id.starts_with("term:"));
        let r = c.search(&q("  mytool "));
        assert_eq!(r[0].id, "run:mytool");
        // Not exactly an executable, or empty.
        assert!(c.search(&q("mytoo")).is_empty());
        assert!(c.search(&q("MYTOOL")).is_empty());
        assert!(c.search(&q("")).is_empty());
        assert!(c.search(&q("/bin/mytool")).is_empty());
        assert!(c.search(&q("nothing here")).is_empty());
    }

    #[test]
    fn titles_keep_spacing_and_cap_length() {
        let t = tempfile::tempdir().unwrap();
        make_exe(t.path(), "mytool", 0o755);
        let c = PathCache::scan(t.path().to_str().unwrap());
        let r = c.search(&q("mytool  a   b"));
        assert_eq!(r[0].title, "Run ‘mytool  a   b’");
        // Under the query cap nothing is cut; the title cap is separate.
        assert_eq!(shown_line("a\u{202e}b\u{200b}c"), "abc");
        let long = "x".repeat(600);
        let s = shown_line(&long);
        assert_eq!(s.chars().count(), MAX_SHOWN_CHARS);
        assert!(s.ends_with('…'));
        assert_eq!(shown_line(&"y".repeat(512)), "y".repeat(512));
    }

    #[test]
    fn cut_queries_offer_no_run_rows() {
        let t = tempfile::tempdir().unwrap();
        make_exe(t.path(), "mytool", 0o755);
        let c = PathCache::scan(t.path().to_str().unwrap());
        let ok = format!("mytool {}", "a".repeat(MAX_QUERY_CHARS - 8));
        assert_eq!(ok.chars().count(), MAX_QUERY_CHARS - 1);
        assert_eq!(c.search(&q(&ok)).len(), 2);
        let cut = format!("mytool {}", "a".repeat(MAX_QUERY_CHARS));
        assert!(c.search(&q(&cut)).is_empty());
        let exact = format!("mytool {}", "a".repeat(MAX_QUERY_CHARS - 7));
        assert!(c.search(&q(&exact)).is_empty());
    }

    #[test]
    fn path_scan_needs_real_execute_access() {
        let t = tempfile::tempdir().unwrap();
        make_exe(t.path(), "mine", 0o755);
        // Execute bit for others only: the owner (us) can't run it.
        make_exe(t.path(), "others", 0o001);
        let c = PathCache::scan(t.path().to_str().unwrap());
        assert!(c.lookup("mine").is_some());
        // Root passes every access check, so only assert it as a normal user.
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            assert!(c.lookup("others").is_none());
        }
    }

    #[test]
    fn path_scan_caps_entries_per_folder() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..(MAX_ENTRIES_PER_DIR + 50) {
            std::fs::write(t.path().join(format!("f{i}")), "").unwrap();
        }
        let c = PathCache::scan(t.path().to_str().unwrap());
        assert!(c.is_empty()); // none executable, and the scan ended
    }

    #[test]
    fn scan_caps_are_enforced() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        for i in 0..5 {
            make_exe(&a, &format!("a{i}"), 0o755);
            make_exe(&b, &format!("b{i}"), 0o755);
        }
        let var = format!("{}:{}", a.display(), b.display());
        let kept = |c: &PathCache, p: char| {
            (0..5)
                .filter(|i| c.lookup(&format!("{p}{i}")).is_some())
                .count()
        };
        // Per-dir cap: at most 2 of each folder.
        let c = PathCache::scan_with_limits(&var, 2, 100);
        assert_eq!((kept(&c, 'a'), kept(&c, 'b')), (2, 2));
        // Total cap: 7 entries looked at in all.
        let c = PathCache::scan_with_limits(&var, 100, 7);
        assert_eq!(c.len(), 7);
        // No cap hit: all.
        assert_eq!(PathCache::scan_with_limits(&var, 100, 100).len(), 10);
    }

    #[test]
    fn odd_whitespace_and_invisible_offer_no_rows() {
        let t = tempfile::tempdir().unwrap();
        make_exe(t.path(), "mytool", 0o755);
        let c = PathCache::scan(t.path().to_str().unwrap());
        assert_eq!(c.search(&q("mytool a")).len(), 2);
        for bad in ["mytool\u{a0}a", "mytool\u{3000}a", "mytool a\u{2003}"] {
            assert!(c.search(&q(bad)).is_empty(), "{bad:?}");
        }
        // Query::new already strips U+2800 and the like; a hand-built raw
        // query keeps them and is refused.
        let mut raw = q("mytool a");
        raw.raw = "mytool \u{2800}".to_owned();
        assert!(c.search(&raw).is_empty());
        // Raw queries built by hand (not through Query::new) too.
        let mut raw = q("mytool a");
        raw.raw = "mytool\u{200b}a".to_owned();
        assert!(c.search(&raw).is_empty());
        // Tabs become plain spaces in the query, so they show as they run.
        assert_eq!(c.search(&q("mytool\ta")).len(), 2);
    }

    #[test]
    fn plan_debug_is_redacted() {
        let d = format!("{:?}", direct("secret", &["pw"]));
        assert!(!d.contains("secret") && !d.contains("pw"));
    }

    #[test]
    fn fuzz_no_panic() {
        const ALPHA: &[char] = &[
            'a', 'b', ' ', '\t', '\'', '"', '\\', '|', ';', '&', '$', '>', '<', '`', '(', ')', '{',
            '}', '*', '?', '~', '\n', '#', '[', 'é', '\u{202e}',
        ];
        let mut s = 42_u64;
        let c = PathCache::default();
        for _ in 0..3000 {
            let mut line = String::new();
            for _ in 0..(s % 24) {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                line.push(ALPHA[(s >> 33) as usize % ALPHA.len()]);
            }
            let plan = parse_command_line(&line);
            if let CommandPlan::Direct { .. } = plan
                && !line.contains(['\'', '"', '\\'])
            {
                assert!(!line.contains(SHELL_META), "{line:?}");
            }
            let _ = c.search(&q(&line));
        }
    }

    #[test]
    fn path_entries_the_cpp_side_would_refuse_are_not_scanned() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = bin.join("sectool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plain = bin.to_str().unwrap().to_owned();
        assert!(PathCache::scan(&plain).lookup("sectool").is_some());
        // The same folder by a path with "..", with a leading "//", or relative.
        let dotdot = format!("{}/../bin", bin.to_str().unwrap());
        assert!(PathCache::scan(&dotdot).lookup("sectool").is_none());
        let double = format!("/{plain}");
        assert!(PathCache::scan(&double).lookup("sectool").is_none());
        assert!(PathCache::scan("bin").lookup("sectool").is_none());
        // One bad entry does not hide a good one.
        let mixed = format!("{dotdot}:{plain}");
        assert!(PathCache::scan(&mixed).lookup("sectool").is_some());
    }
}
