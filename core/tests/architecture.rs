//! The architecture as checks (ADR 0006, `docs/ARCHITECTURE.md`): which
//! modules of the core may use which, and which module owns each piece of
//! outside knowledge. Each check reads the source text, so a failure names
//! the file and line and says what to do instead.
//!
//! These rules are decisions. When a change needs to break one, change the
//! rule here in the same pull request and say why in an ADR; don't work
//! around the check.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// The core's modules, lowest layer first. A module may use the modules
/// listed before it, never one listed after it: the list is a reading order,
/// and it keeps the dependency graph free of cycles. A new module goes where
/// its dependencies allow, as low as possible.
const LAYERS: &[(&str, &[&str])] = &[
    (
        "Foundations: the contract with the UI, where files are, and the \
         boundary with the rest of the Mac. They know none of the app's \
         features.",
        &[
            "interface",
            "claude_files",
            "stamp",
            "storage",
            "env",
            "logging",
            "shell",
        ],
    ),
    (
        "The domain's own rules, which need no file.",
        &["names", "times"],
    ),
    (
        "Claude's formats: one reader per undocumented format (ADR 0004).",
        &[
            "claude_json",
            "limit_offers",
            "usage",
            "live_processes",
            "sidebar_entry",
        ],
    ),
    (
        "Services the features share.",
        &[
            "backups",
            "settings",
            "runner",
            "claude_code",
            "context",
            "safety",
        ],
    ),
    (
        "Features: each owns its state and its file under `state/`.",
        &[
            "profile_folder",
            "monitor",
            "profiles",
            "notifications",
            "cli_switcher",
            "desktop_account",
            "discovered",
            "sidebar_sync",
            "sessions",
            "opening",
            "desktop",
            "checks",
        ],
    ),
    (
        "The core, which composes the features, and what runs it.",
        &["core", "driver", "watcher", "lib"],
    ),
];

/// One source file's code: comment lines and the trailing `#[cfg(test)]`
/// module left out.
struct Source {
    /// The top-level module the file belongs to (`core` for `core/cli.rs`).
    module: String,
    /// Relative to the repository, for messages.
    file: String,
    /// `(line number, text)`.
    lines: Vec<(usize, String)>,
}

fn sources_under(folder: &Path, shown_as: &str) -> Vec<Source> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("a source folder").flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let inside = path.strip_prefix(folder).expect("under the folder");
            let module = inside
                .components()
                .next()
                .expect("a file name")
                .as_os_str()
                .to_string_lossy()
                .trim_end_matches(".rs")
                .to_owned();
            let text = fs::read_to_string(&path).expect("a readable source file");
            Source {
                module,
                file: format!("{shown_as}/{}", inside.display()),
                lines: code_lines(&text),
            }
        })
        .collect()
}

/// The lines of `text` that are code, numbered from 1: comments (whole
/// lines and trailing ones) and `#[cfg(test)] mod … { … }` blocks are left
/// out, wherever in the file the block is.
fn code_lines(text: &str) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    let mut in_test_module = false;
    let mut after_cfg_test = false;
    for (index, line) in text.lines().enumerate() {
        if in_test_module {
            // rustfmt puts a top-level module's closing brace in column one.
            in_test_module = line != "}";
            continue;
        }
        if line == "#[cfg(test)]" {
            after_cfg_test = true;
            continue;
        }
        if std::mem::take(&mut after_cfg_test) && line.starts_with("mod ") {
            in_test_module = line.ends_with('{');
            continue;
        }
        let code = without_trailing_comment(line);
        if !code.trim().is_empty() {
            lines.push((index + 1, code.to_owned()));
        }
    }
    lines
}

/// `line` up to a `//` that isn't inside a string literal (told by the
/// quotes before it; an escaped quote in a string can fool this, which only
/// ever hides the rest of that one line).
fn without_trailing_comment(line: &str) -> &str {
    let mut from = 0;
    while let Some(found) = line[from..].find("//") {
        let at = from + found;
        if line[..at].matches('"').count().is_multiple_of(2) {
            return &line[..at];
        }
        from = at + 2;
    }
    line
}

#[test]
fn the_checks_read_code_and_skip_comments_and_test_modules() {
    let text = [
        "use crate::storage; // crate::sessions in a comment",
        "/// A doc line naming `crate::core`.",
        "const URL: &str = \"https://example.com//x\"; // trailing",
        "#[cfg(test)]",
        "mod tests {",
        "    use crate::desktop;",
        "}",
        "fn after() { crate::env::now(); }",
    ]
    .join("\n");
    let code: Vec<String> = code_lines(&text).into_iter().map(|(_, l)| l).collect();
    assert_eq!(
        code,
        [
            "use crate::storage; ",
            "const URL: &str = \"https://example.com//x\"; ",
            "fn after() { crate::env::now(); }",
        ]
    );
}

/// The core's source files.
fn core_sources() -> Vec<Source> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    sources_under(&src, "core/src")
}

/// The AppKit layer's source files.
fn ui_sources() -> Vec<Source> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src");
    sources_under(&src, "src")
}

/// Every line of `sources` (outside `allowed`, by module) matching
/// `pattern`, as `file:line: text`.
fn matches(sources: &[Source], pattern: &str, allowed: &[&str]) -> Vec<String> {
    let pattern = Regex::new(pattern).expect("a valid pattern");
    sources
        .iter()
        .filter(|source| !allowed.contains(&source.module.as_str()))
        .flat_map(|source| {
            source
                .lines
                .iter()
                .filter(|(_, line)| pattern.is_match(line))
                .map(|(number, line)| format!("{}:{number}: {}", source.file, line.trim()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn assert_none(found: Vec<String>, rule: &str) {
    assert!(found.is_empty(), "\n{rule}\n\n{}\n", found.join("\n"));
}

fn position(module: &str) -> Option<usize> {
    LAYERS
        .iter()
        .flat_map(|(_, modules)| modules.iter())
        .position(|listed| *listed == module)
}

#[test]
fn a_module_uses_only_the_modules_below_it() {
    let uses = Regex::new(r"\bcrate::([a-z_]+)").expect("a valid pattern");
    let mut found = Vec::new();
    for source in core_sources() {
        let Some(own) = position(&source.module) else {
            found.push(format!(
                "{}: module `{}` has no place in LAYERS (core/tests/architecture.rs)",
                source.file, source.module
            ));
            continue;
        };
        for (number, line) in &source.lines {
            for used in uses.captures_iter(line).map(|found| found[1].to_owned()) {
                match position(&used) {
                    Some(other) if other > own => found.push(format!(
                        "{}:{number}: `{}` uses `{used}`, which is above it",
                        source.file, source.module
                    )),
                    _ => {}
                }
            }
        }
    }
    assert_none(
        found,
        "A module of the core may use only the modules listed before it in LAYERS \
         (core/tests/architecture.rs). Move what both need into the lower module, or pass it in \
         from the caller; reorder LAYERS only when the new order still has no cycle.",
    );
}

#[test]
fn the_layers_list_exactly_the_modules_that_exist() {
    let existing: Vec<String> = core_sources().into_iter().map(|s| s.module).collect();
    let gone: Vec<String> = LAYERS
        .iter()
        .flat_map(|(_, modules)| modules.iter())
        .filter(|listed| !existing.iter().any(|module| module == *listed))
        .map(|listed| format!("`{listed}` is in LAYERS but core/src has no such module"))
        .collect();
    assert_none(
        gone,
        "LAYERS names every module of the core, and only those.",
    );
}

#[test]
fn inside_the_crate_an_item_is_named_by_its_module() {
    // `crate::Core` hides which module is used from the check above.
    let sources: Vec<Source> = core_sources();
    assert_none(
        matches(&sources, r"\bcrate::(\{|[A-Z])", &["lib"]),
        "Inside the core, name an item by its module (`crate::core::Core`), not through the \
         crate root's re-exports (`crate::Core`) or a grouped `crate::{…}` import: the layering \
         check reads module names.",
    );
}

#[test]
fn claudes_file_layout_is_named_only_in_claude_files() {
    let layout = [
        r#""\.claude(\.json)?[/"]"#,
        r"claude-code-sessions",
        r"Application Support/Claude",
        r"Logs/Claude",
        r"Claude\.app",
        r"claude-desktop",
        r"scratch-workspaces",
        r#""local_"#,
        r#""deleted_"#,
    ]
    .join("|");
    assert_none(
        matches(&core_sources(), &layout, &["claude_files"]),
        "Where Claude keeps its files, and how it names them, is written once, in \
         core/src/claude_files.rs. Ask it for the path or the naming rule (add one there if it \
         is missing) instead of writing the literal here.",
    );
}

#[test]
fn the_core_reaches_the_rest_of_the_mac_only_through_env() {
    let outside = [
        r"std::env::(var|vars|home_dir|current_dir)",
        r"\benv::(var|vars)(_os)?\(",
        r"Timestamp::now\(\)",
        r"Zoned::now\(\)",
        r"SystemTime::now\(\)",
        r"TimeZone::system\(\)",
        r"process::Command",
        r"\bCommand::new\(",
    ]
    .join("|");
    let found: Vec<String> = matches(&core_sources(), &outside, &["env"])
        .into_iter()
        // The panic hook runs with no `Environment` to ask.
        .filter(|hit| !(hit.starts_with("core/src/logging.rs") && hit.contains("Timestamp::now()")))
        .collect();
    assert_none(
        found,
        "The core reads the clock, the process environment and other processes only through \
         its `Environment` (core/src/env.rs), so every test can run it against the fake Mac. \
         Take the value from `env` (or add it to `Environment`) instead.",
    );
}

#[test]
fn a_whole_file_is_written_only_by_storage() {
    assert_none(
        matches(&core_sources(), r"fs::write\(|File::create\(", &["storage"]),
        "Files are written temp-file-then-rename, by `storage::write_atomically`, \
         `write_new` or `write_versioned` (core/src/storage.rs): a crash must never leave half \
         a file. Use one of those instead of writing in place.",
    );
}

#[test]
fn a_time_the_user_reads_is_worded_only_in_times() {
    assert_none(
        matches(
            &core_sources(),
            r"strftime\(",
            // The machine formats: backup and file names, and the log.
            &["times", "backups", "settings", "logging"],
        ),
        "A time shown to the user is worded by `times` (core/src/times.rs), so the menu, the \
         windows and the notifications name a day or a time the same way. Use one of its \
         functions, or add one there.",
    );
}

#[test]
fn the_ui_layer_keeps_no_files_and_reads_no_formats() {
    assert_none(
        matches(&ui_sources(), r"std::fs|\bfs::|serde|std::env::var", &[]),
        "The AppKit layer (src/) only renders the core's `View`, forwards clicks as `Command`s \
         and performs `Effect`s. Anything that reads or writes a file, reads a format or the \
         environment, or decides, belongs in the core, behind a `Command`, an `Event` or a \
         field of the `View`.",
    );
}
