//! The app log: `logs/app.log`, redacted when written, so it is safe to attach
//! to a bug report as it is.
//!
//! Redaction is enforced by construction, then checked again per line:
//!
//! 1. A log line is a `&'static str` message plus typed [`Field`]s. Free text
//!    from the outside world can only enter as [`Field::Text`]; Profiles,
//!    project folders and paths each have their own field that renders them
//!    safely (`<Profile 3f2a>`, `<project 9c1e2b4a>`, `~/…`).
//! 2. Every finished line goes through [`scrub`]: the home folder becomes `~`,
//!    emails become `<email>` and token-shaped strings become `<token>`.
//!
//! Never log Transcript content, prompts, titles or raw Claude Code output:
//! log parsed numbers and exit codes instead. Session ids are allowed.
//!
//! The log is at info level; Verbose logging adds debug lines. It rotates at
//! 1 MiB, keeping `app.log.1` to `app.log.3`.

use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use jiff::Timestamp;
use regex::Regex;

/// `app.log` rotates once a write would take it past this size.
pub(crate) const ROTATE_AT_BYTES: u64 = 1024 * 1024;
/// How many rotated files (`app.log.1` …) are kept.
pub(crate) const KEPT_ROTATIONS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    /// Written only while Verbose logging is on.
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// A value in a log line, rendered so it reveals nothing private.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Profile, Project and SessionId are for the Profile and Session tickets.
pub(crate) enum Field<'a> {
    /// Outside text (an error message, a version string). Scrubbed.
    Text(&'a str),
    /// A path: the home folder is written `~`, and a folder under
    /// `.claude/projects/` is hashed like [`Field::Project`].
    Path(&'a Path),
    /// A Profile id, written `<Profile 3f2a>`.
    Profile(&'a str),
    /// A project folder name, written `<project 9c1e2b4a>`.
    Project(&'a str),
    /// A Session id; allowed verbatim.
    SessionId(&'a str),
    /// A number: a count, an exit code, a percentage.
    Number(i64),
    /// A fixed word chosen by the code (`"on"`, `"tick"`).
    Word(&'static str),
}

/// Appends redacted lines to `app.log`.
#[derive(Debug, Clone)]
pub(crate) struct Logger {
    path: PathBuf,
    home: PathBuf,
}

impl Logger {
    pub fn new(path: PathBuf, home: PathBuf) -> Logger {
        Logger { path, home }
    }

    /// Writes one line, unless it is a debug line and `verbose` is off.
    /// Logging never fails the caller: a write error is dropped.
    pub fn write(
        &self,
        now: Timestamp,
        verbose: bool,
        level: Level,
        message: &'static str,
        fields: &[(&'static str, Field<'_>)],
    ) {
        if level == Level::Debug && !verbose {
            return;
        }
        let line = format_line(&self.home, now, level, message, fields);
        let _ = append_line(&self.path, &line);
    }
}

/// One finished, scrubbed line (with its newline).
pub(crate) fn format_line(
    home: &Path,
    now: Timestamp,
    level: Level,
    message: &str,
    fields: &[(&'static str, Field<'_>)],
) -> String {
    let mut line = format!(
        "{} {} {message}",
        now.strftime("%Y-%m-%dT%H:%M:%SZ"),
        level.label()
    );
    for (name, field) in fields {
        let _ = write!(line, " {name}={}", render(home, field));
    }
    let mut line = scrub(home, &line).replace('\n', " | ");
    line.push('\n');
    line
}

fn render(home: &Path, field: &Field<'_>) -> String {
    match *field {
        Field::Text(text) => format!("{text:?}"),
        Field::Path(path) => format!("{:?}", render_path(home, path)),
        Field::Profile(id) => format!("<Profile {}>", &short_hash(id)[..4]),
        Field::Project(name) => format!("<project {}>", short_hash(name)),
        Field::SessionId(id) => id.to_owned(),
        Field::Number(n) => n.to_string(),
        Field::Word(word) => word.to_owned(),
    }
}

/// `~/…` for paths under home, with any `.claude/projects/<name>` hashed.
fn render_path(home: &Path, path: &Path) -> String {
    let (prefix, rest) = match path.strip_prefix(home) {
        Ok(rest) => ("~".to_owned(), rest),
        Err(_) => (String::new(), path),
    };
    let mut out = prefix;
    let mut previous: [Option<&std::ffi::OsStr>; 2] = [None, None];
    for component in rest.components() {
        let text = match component {
            Component::RootDir => {
                out.push('/');
                continue;
            }
            Component::Normal(part) => {
                let hashed = previous[0] == Some(".claude".as_ref())
                    && previous[1] == Some("projects".as_ref());
                previous = [previous[1], Some(part)];
                if hashed {
                    format!("<project {}>", short_hash(&part.to_string_lossy()))
                } else {
                    part.to_string_lossy().into_owned()
                }
            }
            other => other.as_os_str().to_string_lossy().into_owned(),
        };
        if !out.is_empty() && !out.ends_with('/') {
            out.push('/');
        }
        out.push_str(&text);
    }
    out
}

/// A stable 8-hex-digit FNV-1a hash: the same name always gives the same
/// label, across runs and builds, so log lines can be correlated.
pub(crate) fn short_hash(text: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}")
        .expect("valid email pattern")
});
/// Anthropic keys and tokens, JWTs, and any other long unbroken run of
/// token characters.
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"sk-ant-[A-Za-z0-9_\-]+|eyJ[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-.]+|[A-Za-z0-9_\-]{32,}")
        .expect("valid token pattern")
});
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
        .expect("valid uuid pattern")
});

/// The last line of defence, applied to every line: home folder → `~`,
/// emails → `<email>`, token-shaped strings → `<token>`. UUIDs (Session ids)
/// are kept.
pub(crate) fn scrub(home: &Path, text: &str) -> String {
    let home = home.to_string_lossy();
    let mut text = if home.len() > 1 {
        text.replace(&format!("{home}/"), "~/")
            .replace(home.as_ref(), "~")
    } else {
        text.to_owned()
    };
    text = EMAIL.replace_all(&text, "<email>").into_owned();
    TOKEN
        .replace_all(&text, |found: &regex::Captures<'_>| {
            let found = &found[0];
            if UUID.is_match(found) {
                found.to_owned()
            } else {
                "<token>".to_owned()
            }
        })
        .into_owned()
}

/// Appends `line` to the log at `path`, rotating first if it would pass
/// [`ROTATE_AT_BYTES`].
pub(crate) fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    let size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if size > 0 && size + line.len() as u64 > ROTATE_AT_BYTES {
        rotate(path)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())
}

/// `app.log.2` → `.3`, `.1` → `.2`, `app.log` → `.1`; the oldest is dropped.
fn rotate(path: &Path) -> std::io::Result<()> {
    let numbered = |n: u32| {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    };
    for n in (1..KEPT_ROTATIONS).rev() {
        let from = numbered(n);
        if from.exists() {
            fs::rename(&from, numbered(n + 1))?;
        }
    }
    fs::rename(path, numbered(1))
}

/// Installs a panic hook that writes the panic (redacted) to `app.log` before
/// the process exits, then runs the previously installed hook.
///
/// The app calls this once at startup with [`crate::Environment::home`]. It is
/// process-wide, so the core never installs it itself.
pub fn install_panic_hook(home: &Path) {
    let path = crate::AppPaths::new(home).app_log();
    let home = home.to_path_buf();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-text panic payload".to_owned());
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_default();
        let line = format_line(
            &home,
            Timestamp::now(),
            Level::Error,
            "panic",
            &[
                ("message", Field::Text(&payload)),
                ("at", Field::Text(&location)),
            ],
        );
        let _ = append_line(&path, &line);
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    //! The typed fields later tickets rely on. Behaviour reachable through the
    //! core's interface is tested in `tests/logs.rs`.
    use super::*;

    fn line(fields: &[(&'static str, Field<'_>)]) -> String {
        format_line(
            Path::new("/Users/tester"),
            "2026-10-25T14:02:00Z".parse().unwrap(),
            Level::Info,
            "example",
            fields,
        )
    }

    #[test]
    fn a_profile_is_written_as_a_short_stable_label() {
        let written = line(&[("profile", Field::Profile("perso-7f3a"))]);
        let again = line(&[("profile", Field::Profile("perso-7f3a"))]);
        let label = Regex::new(r"profile=<Profile [0-9a-f]{4}>").unwrap();
        assert!(label.is_match(&written), "{written}");
        assert!(!written.contains("perso-7f3a"), "{written}");
        assert_eq!(written, again);
    }

    #[test]
    fn project_folders_are_hashed_alone_and_inside_transcript_paths() {
        let written = line(&[
            ("project", Field::Project("secret-client")),
            (
                "transcript",
                Field::Path(Path::new(
                    "/Users/tester/.claude/projects/-Users-tester-secret-client/0b9c.jsonl",
                )),
            ),
        ]);
        assert!(!written.contains("secret-client"), "{written}");
        assert!(
            written.contains("transcript=\"~/.claude/projects/<project "),
            "{written}"
        );
        assert!(written.contains("/0b9c.jsonl\""), "{written}");
    }

    #[test]
    fn session_ids_are_kept_and_numbers_are_plain() {
        let written = line(&[
            (
                "session",
                Field::SessionId("4f1d2c3b-1a2b-4c3d-8e9f-0a1b2c3d4e5f"),
            ),
            ("exit", Field::Number(2)),
        ]);
        assert!(
            written.ends_with("session=4f1d2c3b-1a2b-4c3d-8e9f-0a1b2c3d4e5f exit=2\n"),
            "{written}"
        );
    }
}
