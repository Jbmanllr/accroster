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

Behaviour: every module that uses Sidebar entries now applies the strictest existing rule for a Sidebar folder (a real folder named by a uuid inside one named by a uuid). Claude makes no other kind. The self-check keeps looking into every real subfolder.

## Round 2: one reader for live-process files (`d3ffddb`)

New `live_processes`: one `LiveProcess`, one staleness check, one meaning of `status`, the field list the self-check reports. The busy guard, the running dots and the self-check use it.

Behaviour: a wrong-typed `name`, `hostSessionId` or `sessionId` no longer makes a file unreadable; unit tests cover it. A link named `<pid>.json` is skipped by the busy guard and the self-check, as the Session index already did (found by the review in round 10).

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

A new test (`mute_sits_right_above_remove_whatever_else_the_row_offers`) pins the submenu's order in three states; it passed before the change and after. The test doesn't look at detail lines, and one state differs there: while a Profile with a replaced link signs out, Mute now sits above the "… link replaced" line instead of below it, the same order as when it isn't signing out.

## Round 7: the AppKit layer's leaves leave `ui.rs` (`3ffbdb3`)

`src/ui.rs` was one 2,858-line file. Its parts that don't touch the `Ui` object moved out, unchanged: `src/ui/widgets.rs` (views built from view models), `src/ui/sidebar.rs` (the All Sessions sidebar's data source) and `src/ui/mac.rs` (Terminal, the desktop app, the Trash, alerts, the network path). `ui.rs` is 2,000 lines: `Ui` and the windows (task 14).

This layer has no tests, so the move was checked two other ways: the code lines of the four files are the same multiset as the old file's (apart from `use` lines, `pub(super)` and four signatures rustfmt rewrapped), and the debug build launched and ran against an empty throwaway home, logging `launch` and nothing else. No window was opened in that run.

## Round 8: how a Profile is called has one module (`1f812ae`)

The Profile name and the Short label are glossary terms, but their rules were in two places: for one Login in `claude_json` (the reader of `oauthAccount`), and across Profiles in `profiles` (`names`, `short_labels`, `unique`). Both now live in `names`; `claude_json` only reads the file, and the registry passes each Profile's nickname and Login. The personal default Organization's name (`<email>'s Organization`) was built twice and is built once.

Behaviour unchanged; `core/tests/profile_names.rs` covers it through the menu.

## Round 9: `Core::view` hands two parts to named functions (`d28add3`)

The menu's last items (Recent Sessions to Quit) are `Core::fixed_menu_items`, and the Settings window is `Core::settings_view`. `view` is 160 lines instead of 260 and reads as: status lines, header, Profile rows, Discovered rows, fixed items, then each window. Behaviour unchanged; task 12 keeps what is left.

## Round 10: an independent review of rounds 1 to 7, and what it changed (`53fffa1`)

A fresh reviewer compared old and new code over `e474feb..3ffbdb3`, read-only. It found no bug that shows on a real Mac, and confirmed the moves in rounds 3, 5 and 7 are identical code. It found four edge-case differences that weren't written down, documentation that claimed more than the checks do, and a blind spot in the architecture test. All are addressed:

- **Recorded in ADR 0006:** a link named `<pid>.json` is skipped by the busy guard and the self-check; the self-check takes an entry's id from its file name as sync does; a busy Session's title comes from the first Sidebar folder in sorted order. The Mute item's place in one transient state is recorded under round 6.
- **Docs made exact:** the self-check still looks into every Sidebar subfolder (ADR 0006 said every reader used the uuid rule); `docs/ARCHITECTURE.md` now lists what each check looks for and what it leaves to review.
- **Two more rules given their owner**, which the review pointed at: which files are Transcripts (`claude_files::transcript_files`, used by the Session index and the self-check, which now also skips `agent-*.jsonl`), and a Sidebar entry's file name (`sidebar_entry_file_name`, `sidebar_entry_id` in the two places that still cut `.json` by hand).
- **The architecture test** no longer stops reading a file at the first `#[cfg(test)]`: it skips test modules wherever they are, and trailing comments. It has a test of its own for that, and the planted-violation probe of round 4 was run again.

## Round 11: the leftover-folder rule has one owner

A Sidebar folder that pairs an account with another Profile's personal Organization is an account switch's leftover (ADR 0003). The rule was written in `core.rs` (to hide the folder) and again in `sidebar_sync::begin` (to never write into it), each from a map the core built. It is `PersonalOrganizations::is_leftover`, built by the Profile registry, which knows whose Organization is personal.

Behaviour unchanged; `a_leftover_folder_in_another_profiles_personal_organization_is_read_but_never_written` and `a_profiles_sidebars_in_other_organizations_follow_it_and_leftovers_are_hidden` cover both uses.

This came from an architecture review of 2026-11-02 whose findings had not been acted on. Its other candidates are in `TASKS.md`: two were done by rounds 1 and 2 (one reader of the Sidebar folders' layout, one reader of live-process files), one is task 8, and four are tasks 15 to 18.

## Where it stands

- The gate is green on the branch: format, lint, and 33 test binaries, eight of them architecture checks.
- Not run here: the release dry run (`scripts/release.sh --dry-run`); the hosted workflow runs it on a pull request.
- Not checked by any test: the AppKit layer beyond launching (round 7).
- Persisted files, settings and the menu's wording are unchanged. The edge-case differences are listed in ADR 0006.
