//! Opening a Session: the choices the All Sessions detail pane offers, what
//! each one costs (which Profile pays, what carries over), and which client a
//! double-click or a Recent Sessions item uses.
//!
//! - **Terminal:** `claude --resume <id>` in the Session's folder, in a new
//!   Terminal window; new turns count against the CLI's Active Profile (the
//!   shim picks it). A Session already running outside the desktop app is
//!   brought forward instead of started twice.
//! - **Claude, entry in its Active Profile:** `claude://code/continue?session=
//!   local_<id>`, which opens that entry and writes nothing. The link skips
//!   archived entries, so an archived entry doesn't count as one.
//! - **Claude, no entry there:** `claude --desktop --resume <id>`: Claude adds
//!   a new entry under its Active Profile (or unarchives `local_<id>` when that
//!   is the archived entry). The other way is a Guided switch to a Profile that
//!   has an entry, then the link above.

use std::path::Path;

use crate::env::OsFacts;
use crate::interface::{Command, Effect, OpenChoice, ProfileKey, RunningDot};
use crate::sessions::{EntryFacts, SessionFacts};

/// The desktop app's `entrypoint` in Transcripts.
const DESKTOP_ENTRYPOINT: &str = "claude-desktop";

/// The clients that could open a Session, as of now.
pub(crate) struct Clients {
    /// The CLI's Active Profile, as the menu header names it ("Perso",
    /// "Default").
    pub cli: String,
    /// The desktop app, when it is installed.
    pub desktop: Option<DesktopClient>,
    /// Signed-in Profiles, in the user's order.
    pub profiles: Vec<NamedProfile>,
    /// The busy guard found a desktop turn running or waiting: a Guided
    /// switch now would end it.
    pub desktop_turn_running: bool,
    /// A signed-in Profile folder exists to run `claude --desktop --resume`
    /// in (never `~/.claude`).
    pub can_run_claude_code: bool,
}

/// The desktop app's side.
pub(crate) struct DesktopClient {
    /// Its Active Profile's account and Organization, when its files tell.
    pub pair: Option<ProfileKey>,
    /// That Profile's name ("Work"), when it is one AccRoster knows.
    pub name: Option<String>,
}

impl DesktopClient {
    fn sidebar(&self) -> String {
        match &self.name {
            Some(name) => format!("{name}'s sidebar"),
            None => "Claude's sidebar".to_owned(),
        }
    }
}

/// A signed-in Profile with the name the Open section shows.
pub(crate) struct NamedProfile {
    pub id: String,
    pub pair: ProfileKey,
    pub name: String,
}

/// How "Open in Claude" opens a Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InClaude {
    /// The link to its entry (`local_…`) in the desktop's Active Profile.
    Entry(String),
    /// `claude --desktop --resume <id>`.
    Resume,
}

/// Which client a double-click (or a Recent Sessions item) opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DefaultOpen {
    Terminal,
    Claude,
    /// Last used in Claude, but its Active Profile has no entry: opening
    /// there would lose the title and archive state, so the choices are
    /// shown instead.
    Choose,
}

/// `claude://code/continue?session=<entry>`.
pub(crate) fn continue_url(entry_id: &str) -> String {
    format!("claude://code/continue?session={entry_id}")
}

/// The client the Session was last used in, by the Transcript's last
/// `entrypoint`; the Terminal when Claude isn't installed.
pub(crate) fn default_open(facts: &SessionFacts, clients: &Clients) -> Option<DefaultOpen> {
    if facts.deleted {
        return None;
    }
    if facts.entrypoint.as_deref() != Some(DESKTOP_ENTRYPOINT) {
        return Some(DefaultOpen::Terminal);
    }
    Some(match in_claude(facts, clients) {
        None => DefaultOpen::Terminal,
        Some(InClaude::Entry(_)) => DefaultOpen::Claude,
        Some(InClaude::Resume) => DefaultOpen::Choose,
    })
}

/// "Resume in Terminal" (`RunInTerminal`) or, when the Session already runs
/// outside the desktop app, "Show in Terminal" (`ShowInTerminal`). `None`
/// when there is nothing to resume or no folder to resume it in.
pub(crate) fn terminal(facts: &SessionFacts, os: &dyn OsFacts) -> Option<Effect> {
    if facts.deleted {
        return None;
    }
    if let Some(running) = facts.running.filter(|r| r.dot == RunningDot::Terminal) {
        return Some(Effect::ShowInTerminal {
            tty: os.terminal_of(running.pid),
        });
    }
    let cwd = facts.cwd.as_deref().filter(|cwd| cwd.is_dir())?;
    Some(Effect::RunInTerminal(format!(
        "cd {} && claude --resume {}",
        shell_quoted(&cwd.to_string_lossy()),
        shell_quoted(&facts.id)
    )))
}

/// How "Open in Claude" would open the Session; `None` when Claude isn't
/// installed or the Transcript is gone.
pub(crate) fn in_claude(facts: &SessionFacts, clients: &Clients) -> Option<InClaude> {
    let desktop = clients.desktop.as_ref()?;
    if facts.deleted {
        return None;
    }
    Some(match open_entry(facts, desktop.pair.as_ref()) {
        Some(entry) => InClaude::Entry(entry.entry_id.clone()),
        None => InClaude::Resume,
    })
}

/// Why `claude --desktop --resume` can't run for this Session now, if it
/// can't.
pub(crate) fn resume_blocked(facts: &SessionFacts, clients: &Clients) -> Option<&'static str> {
    if facts.running.is_some_and(|r| r.dot == RunningDot::Terminal) {
        Some("It runs in Terminal, and Claude Code won't move a running Session")
    } else if !clients.can_run_claude_code {
        Some("Needs a signed-in Profile to run Claude Code in")
    } else {
        None
    }
}

/// The entry a Guided switch to `pair` would open afterwards.
pub(crate) fn switch_entry<'a>(
    facts: &'a SessionFacts,
    pair: &ProfileKey,
) -> Option<&'a EntryFacts> {
    open_entry(facts, Some(pair))
}

/// The detail pane's Open section.
pub(crate) fn choices(facts: &SessionFacts, clients: &Clients) -> Vec<OpenChoice> {
    if facts.deleted {
        return Vec::new();
    }
    let mut choices = Vec::new();
    let running_in_terminal = facts
        .running
        .is_some_and(|running| running.dot == RunningDot::Terminal);
    let folder_ok = facts.cwd.as_deref().is_some_and(Path::is_dir);
    choices.push(OpenChoice {
        title: if running_in_terminal {
            "Show in Terminal"
        } else {
            "Resume in Terminal"
        }
        .into(),
        note: Some(if running_in_terminal || folder_ok {
            format!("New turns count against {} (CLI)", clients.cli)
        } else {
            "Its folder no longer exists".to_owned()
        }),
        command: Command::ResumeInTerminal(facts.id.clone()),
        enabled: running_in_terminal || folder_ok,
    });
    let Some(desktop) = &clients.desktop else {
        return choices;
    };
    let open_in_claude = Command::OpenInClaude(facts.id.clone());
    if in_claude(facts, clients) != Some(InClaude::Resume) {
        choices.push(OpenChoice {
            title: "Open in Claude".into(),
            note: Some(format!("Opens its entry in {}", desktop.sidebar())),
            command: open_in_claude,
            enabled: true,
        });
        return choices;
    }
    let blocked = resume_blocked(facts, clients);
    choices.push(OpenChoice {
        title: match &desktop.name {
            Some(name) => format!("Open in Claude as {name}"),
            None => "Open in Claude".to_owned(),
        },
        note: Some(match blocked {
            Some(why) => why.to_owned(),
            None if archived_resumable(facts, desktop) => {
                format!("Unarchives its entry in {}", desktop.sidebar())
            }
            None => format!(
                "Adds a new entry to {}; title and archive state don't carry over",
                desktop.sidebar()
            ),
        }),
        command: open_in_claude,
        enabled: blocked.is_none(),
    });
    for profile in &clients.profiles {
        if desktop.pair.as_ref() == Some(&profile.pair)
            || switch_entry(facts, &profile.pair).is_none()
        {
            continue;
        }
        choices.push(OpenChoice {
            title: format!("Switch Claude to {}, then open…", profile.name),
            note: clients
                .desktop_turn_running
                .then(|| "A desktop turn is running; switching now would end it".to_owned()),
            command: Command::SwitchClaudeThenOpen {
                profile: profile.id.clone(),
                session: facts.id.clone(),
            },
            enabled: !clients.desktop_turn_running,
        });
    }
    choices
}

/// The desktop's own entry is the archived `local_<id>`, which
/// `claude --desktop --resume` unarchives instead of adding another.
fn archived_resumable(facts: &SessionFacts, desktop: &DesktopClient) -> bool {
    let own = format!("local_{}", facts.id);
    facts
        .entries
        .iter()
        .any(|entry| Some(&entry.pair) == desktop.pair.as_ref() && entry.entry_id == own)
}

/// The newest entry of `pair` the continue link can open: not archived (the
/// link skips archived entries) and with an id it accepts.
fn open_entry<'a>(facts: &'a SessionFacts, pair: Option<&ProfileKey>) -> Option<&'a EntryFacts> {
    let pair = pair?;
    facts
        .entries
        .iter()
        .filter(|entry| entry.pair == *pair && !entry.archived && linkable(&entry.entry_id))
        .max_by_key(|entry| entry.last_activity_at)
}

/// The ids `claude://code/continue` accepts: `^local_[A-Za-z0-9-]{1,64}$`.
fn linkable(entry_id: &str) -> bool {
    entry_id.strip_prefix("local_").is_some_and(|rest| {
        (1..=64).contains(&rest.len())
            && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// One `sh` word: single-quoted, with `'` written as `'\''`.
fn shell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
