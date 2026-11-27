# Architecture progress

What each round changed, why, and how it was verified. Newest last. Open work is in `TASKS.md`; the target is `docs/ARCHITECTURE.md`.

Every round ended with the same gate green on the worktree: `cargo fmt --all --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`.

## Baseline (2026-11-27)

`main` at `e474feb`: gate green, 30 test binaries. Survey findings:

- Claude's file layout joined from literals in more than ten modules; six walkers of the Sidebar folders that disagreed on names that aren't uuids and on links; four copies of the 10 MB entry limit; three copies of the `<pid>.json` rule.
- Two parsers of live-process files with different structs, staleness checks and readings of `status`.
- Five dependency cycles: `profiles` ↔ `monitor`, `profiles` ↔ `cli_switcher`, `claude_code` ↔ `runner`, `desktop` → `opening` → `sessions` → `desktop`, `core` ↔ `driver`.
- Good already: the UI/core split behind `interface`, the `Environment` boundary (two harmless leaks), atomic writes in `storage`, tests through the core's interface against the fake Mac.

## Round 1: one owner for Claude's file layout (`f96b858`)

New `claude_files`: `ClaudePaths`, the Sidebar folder walk, the names of entries, tombstones and live-process files, the entry size limit, the desktop entrypoint. `DesktopPaths` removed. Nineteen files changed; ten modules lost their literals.

Behaviour: every reader now uses the strictest existing rule for a Sidebar folder (a real folder named by a uuid inside one named by a uuid). Claude makes no other kind.

## Round 2: one reader for live-process files (`d3ffddb`)

New `live_processes`: one `LiveProcess`, one staleness check, one meaning of `status`, the field list the self-check reports. The busy guard, the running dots and the self-check use it.

Behaviour: a wrong-typed `name`, `hostSessionId` or `sessionId` no longer makes a file unreadable; unit tests cover it.

## Round 3: no dependency cycles (`b769e1b`)

`Context` → `context`; the Profile-folder half of `cli_switcher` → `profile_folder`; the desktop's account facts → `desktop_account`; the login-shell `PATH` marker into `runner`; `TICK_INTERVAL` into `interface`. The module graph is acyclic. Behaviour unchanged.

## Round 4: the rules become checks (`f314237`)

`core/tests/architecture.rs` (seven checks), ADR 0006, `docs/ARCHITECTURE.md`. Each behavioural check was shown to fail on a planted violation, with the file and line.

## Round 5: small rules with two copies (`7ae4781`)

- Quoting a `sh` word: `cli_switcher::sh_quote` and `opening::shell_quoted` → `shell::quote` / `quote_path`, with a unit test.
- A row's tightest limit: computed in `core` and again in `cli_switcher` → `ProfileRow::tightest_limit`.
- "Is this file executable": `env` and `claude_code` → `env::is_executable`.
- Deleting a file that may be gone: twice in `cli_switcher` → one private helper.
- The installed Claude versions moved from the Desktop bridge to `claude_code::installed_versions`: `desktop` is now only the busy guard, the Guided switch and Reopen Claude.

Behaviour unchanged.

## Round 6: a row's items are placed by the module that builds the row (`ae06c7f`)

The core added "Mute notifications" to each Profile row by searching the row's submenu for an item titled "Remove…", which the registry builds and the CLI switcher sometimes replaces with a submenu. Renaming that item would have silently moved Mute to the end. The registry now places Mute itself, from the settings it already receives.

A new test (`mute_sits_right_above_remove_whatever_else_the_row_offers`) pins the submenu's order in three states; it passed before the change and after.

## Round 7: the AppKit layer's leaves leave `ui.rs` (`3ffbdb3`)

`src/ui.rs` was one 2,858-line file. Its parts that don't touch the `Ui` object moved out, unchanged: `src/ui/widgets.rs` (views built from view models), `src/ui/sidebar.rs` (the All Sessions sidebar's data source) and `src/ui/mac.rs` (Terminal, the desktop app, the Trash, alerts, the network path). `ui.rs` is 2,000 lines: `Ui` and the windows (task 14).

This layer has no tests, so the move was checked two other ways: the code lines of the four files are the same multiset as the old file's (apart from `use` lines, `pub(super)` and four signatures rustfmt rewrapped), and the debug build launched and ran against an empty throwaway home, logging `launch` and nothing else. No window was opened in that run.

## Round 8: how a Profile is called has one module

The Profile name and the Short label are glossary terms, but their rules were in two places: for one Login in `claude_json` (the reader of `oauthAccount`), and across Profiles in `profiles` (`names`, `short_labels`, `unique`). Both now live in `names`; `claude_json` only reads the file, and the registry passes each Profile's nickname and Login. The personal default Organization's name (`<email>'s Organization`) was built twice and is built once.

Behaviour unchanged; `core/tests/profile_names.rs` covers it through the menu.
