//! One search result, whatever produced it, and what running it does. The
//! C++ side carries out the `Action`; nothing here runs anything.

/// What a result is; the panel shows it on the right of the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    App,
    Setting,
    File,
    Folder,
    Calculator,
    Command,
    /// Lock, Sleep, Restart... (asked through Plasma's prompt from search).
    Session,
    Web,
    /// A KRunner plugin's match.
    Runner,
}

/// The session commands of DESIGN.md "Commands".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionAction {
    Lock,
    Sleep,
    Hibernate,
    SwitchUser,
    LogOut,
    Restart,
    ShutDown,
}

/// What running a result does. Paths and links in here were validated where
/// they entered.
#[derive(Clone, PartialEq)]
pub enum Action {
    /// Start an app from its desktop file id, optionally one of its
    /// desktop actions (KIO::ApplicationLauncherJob).
    LaunchApp {
        desktop_id: String,
        action: Option<String>,
    },
    /// AtlasOS Settings' `ActivateAction("open", [link])`.
    OpenSettings {
        link: String,
    },
    /// A `file:` URI (KIO::OpenUrlJob).
    OpenFile {
        uri: String,
    },
    /// Copy text to the clipboard (the calculator).
    Copy {
        text: String,
    },
    /// An executable on PATH with its arguments, no shell
    /// (KIO::CommandLauncherJob).
    Run {
        executable: String,
        args: Vec<String>,
    },
    /// The user's own command line, in their terminal (KTerminalLauncherJob).
    RunInTerminal {
        line: String,
    },
    Session(SessionAction),
    /// An https URL in the default browser (KIO::OpenUrlJob).
    OpenWeb {
        url: String,
    },
    /// A KRunner match, run through RunnerManager by its id.
    Runner {
        runner_id: String,
        match_id: String,
    },
}

/// Redacted: the variant only (a session action by name); ids, links, URIs,
/// command lines and text never reach a log.
impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::LaunchApp { .. } => f.write_str("Action::LaunchApp"),
            Action::OpenSettings { .. } => f.write_str("Action::OpenSettings"),
            Action::OpenFile { .. } => f.write_str("Action::OpenFile"),
            Action::Copy { .. } => f.write_str("Action::Copy"),
            Action::Run { args, .. } => write!(f, "Action::Run(args: {})", args.len()),
            Action::RunInTerminal { .. } => f.write_str("Action::RunInTerminal"),
            Action::Session(a) => write!(f, "Action::Session({a:?})"),
            Action::OpenWeb { .. } => f.write_str("Action::OpenWeb"),
            Action::Runner { .. } => f.write_str("Action::Runner"),
        }
    }
}

/// One row of the result list.
#[derive(Clone, PartialEq)]
pub struct ResultItem {
    /// Stable across queries, so learning and the list's diff can follow it:
    /// `app:<desktop id>`, `app:<desktop id>#<action>`, `setting:<link>`,
    /// `file:<uri>`, `calc`, `run:<line>`, `term:<line>`, `session:<name>`,
    /// `web`, `runner:<runner id>:<match id>`.
    pub id: String,
    pub kind: Kind,
    /// Shown text, already cleaned for display.
    pub title: String,
    pub subtitle: String,
    /// A theme icon name or an absolute path.
    pub icon: String,
    /// `m × prior + learned`; higher is better.
    pub score: f32,
    pub action: Action,
}

/// Redacted: titles, ids and payloads can carry file names and typed text,
/// and must never reach a log.
impl std::fmt::Debug for ResultItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultItem")
            .field("kind", &self.kind)
            .field("score", &self.score)
            .field("id_len", &self.id.len())
            .finish_non_exhaustive()
    }
}

impl ResultItem {
    /// Orders results best first: score, then the shorter title, then
    /// alphabetically, then the id (so equal inputs always sort alike). The
    /// web row always comes last.
    pub fn rank_cmp(a: &ResultItem, b: &ResultItem) -> std::cmp::Ordering {
        (a.kind == Kind::Web)
            .cmp(&(b.kind == Kind::Web))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.title.chars().count().cmp(&b.title.chars().count()))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.id.cmp(&b.id))
    }
}

/// The priors of DESIGN.md, by kind.
pub fn prior(kind: Kind) -> f32 {
    match kind {
        Kind::Calculator => 1.2,
        Kind::App => 1.0,
        Kind::Setting => 0.85,
        Kind::Command | Kind::Session => 0.8,
        Kind::File | Kind::Folder => 0.7,
        Kind::Runner => 0.75,
        Kind::Web => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, kind: Kind, score: f32) -> ResultItem {
        ResultItem {
            id: id.into(),
            kind,
            title: "Secret Title".into(),
            subtitle: "secret sub".into(),
            icon: String::new(),
            score,
            action: Action::Copy {
                text: "secret payload".into(),
            },
        }
    }

    #[test]
    fn debug_is_redacted() {
        let d = format!("{:?}", item("file:/home/u/secret", Kind::File, 0.5));
        assert!(!d.contains("ecret"), "{d}");
        assert!(
            d.contains("File") && d.contains("0.5") && d.contains("19"),
            "{d}"
        );
    }

    #[test]
    fn action_debug_is_redacted() {
        let all = [
            Action::LaunchApp {
                desktop_id: "secret".into(),
                action: Some("secret".into()),
            },
            Action::OpenSettings {
                link: "secret".into(),
            },
            Action::OpenFile {
                uri: "secret".into(),
            },
            Action::Copy {
                text: "secret".into(),
            },
            Action::Run {
                executable: "secret".into(),
                args: vec!["secret".into()],
            },
            Action::RunInTerminal {
                line: "secret".into(),
            },
            Action::Session(SessionAction::Lock),
            Action::OpenWeb {
                url: "secret".into(),
            },
            Action::Runner {
                runner_id: "secret".into(),
                match_id: "secret".into(),
            },
        ];
        for a in all {
            assert!(!format!("{a:?}").contains("secret"), "{a:?}");
        }
    }

    #[test]
    fn web_row_sorts_last() {
        let mut v = [
            item("web", Kind::Web, 99.0),
            item("a", Kind::App, 0.1),
            item("b", Kind::File, 0.0),
        ];
        v.sort_by(ResultItem::rank_cmp);
        assert_eq!(v[2].kind, Kind::Web);
        assert_eq!(v[0].id, "a");
        v.reverse();
        v.sort_by(ResultItem::rank_cmp);
        assert_eq!(v[2].id, "web");
    }
}
