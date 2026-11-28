# Architecture progress

What each round changed, why, and how it was verified. Newest last. The target is `docs/ARCHITECTURE.md`.

This log is closed after round 21 (2026-11-28): no rounds are added. Later architecture work is a GitHub issue labelled `ready-for-agent`, and its record is that issue's pull request. Parallel tickets each appending a round here conflicted with one another.

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

This came from an architecture review of 2026-11-02 whose findings had not been acted on. Its other candidates went into the task pool: two were done by rounds 1 and 2 (one reader of the Sidebar folders' layout, one reader of live-process files), one is task 8, and four are tasks 15 to 18.

Rounds 1 to 11 were merged as pull request 82. Rounds 12 to 20 take the task pool's open tasks.

## Round 12: one file stamp (task 9)

The Session index, the Discovered Profiles, Sidebar sync and the self-check each defined "this file changed since I read it": size and modification time; size and an optional modification time; inode, size, modification and change time. They share `stamp::Stamp`, the strictest of the three, which Sidebar sync needs (ADR 0003). The Session index still saves a Transcript's size and modification time in `index/sessions.json`, taken from the stamp.

Two differences. The Discovered Profiles and Session index caches now also read an entry again when only its inode or change time differs. And a file whose modification time the file system can't give is skipped, where the Session index used to record 0 and the Discovered scan compared sizes only; macOS always gives one. A test of the Discovered cache relied on the old rule (it made an entry unreadable with `chmod`, which changes the change time); what it proved is now `a_listing_reads_only_new_and_rewritten_entries` in `discovered.rs`, and the integration test keeps the folder-level half.

## Round 13: time wording in one module (task 11)

The clock time, a coming day ("today", "Thu", "Nov 3"), the countdown to a reset and a Session's age were formatted in `monitor`, `notifications`, `discovered`, `limit_offers` and `sessions`. They are `times`, each wording unchanged and pinned by a unit test. A new architecture check keeps `strftime` out of the other modules (the machine formats in `backups`, `settings` and `logging` are exempt).

## Round 14: the menu from named parts, each client's Active Profile read once (tasks 12 and 16)

`Core::view` was one 160-line function. Its parts are functions in `core/menu.rs`: `status_lines`, `listed_profiles`, `default_login_part`, `add_desktop_items`, `header_line`, `discovered_parts` and `fixed_menu_items`; `view` puts them in order.

The CLI's pointer and the desktop app's account were read five to seven times per tick. `ActiveProfiles` holds both as read once; one view and one monitor step read it once and pass it down, and `cli_switcher::present` takes the pointer instead of reading it again. For the menu and the Limit monitor that is three reads per tick: once each for the monitor step, the view and the tick interval. Two readers are left as they were: the All Sessions detail of a selected Session, and Sidebar sync's watch, which reads the desktop's account on every tick while sync is on.

Not done as the task said: no test counts the reads of `config.json`. The files are read with `std::fs`, so counting would need a seam that exists only for the test. The existing menu tests cover the behaviour.

## Round 15: `profiles.rs` split (task 13)

`profiles.rs` was 1,600 lines. The browser sign-ins (Add Profile… and Sign in again…) are `profiles/sign_in.rs`, Remove… is `profiles/removal.rs`, and the registry file (about 1,000 lines) keeps which Profiles exist, each one's menu row and its Limit reading. A pure move: the same code lines, with two signatures rewrapped by rustfmt.

## Round 16: one file per window (task 14)

`src/ui.rs` was 2,000 lines. Settings, All Sessions, Diagnostics and the first-run window are each a file under `src/ui/` holding the window's struct, its builder and its render; `ui.rs` (960 lines) keeps `Ui`, `render`, `perform` and the menu. Verified as round 7 was: the same code lines, and the debug build launched against a throwaway home. The four windows were then opened in the installed build.

## Round 17: one answer to "what is this Sidebar folder" (task 15)

Whether a folder is a Profile's own, the same Login in another Organization, an account switch's leftover or a Discovered Profile's was worked out separately for the menu, for All Sessions and for Sidebar sync. All three ask `profiles::SidebarKinds`, which replaces round 11's `PersonalOrganizations`. Behaviour is unchanged: the leftover rule still comes first.

The Guided switch and the desktop's account don't apply the leftover rule. A test now pins what happens: an entry Claude writes in a leftover folder of the target's account reads as "another Profile". It should not: issue #85 changes it.

## Round 18: who goes first, and a Sync now bug (task 17)

Which of the Guided switch, Sidebar sync and Reopen Claude waits for which was spread over the tick, two command handlers and Settings. Three rules in `core/sidebar.rs` hold the whole order: `may_switch_desktop`, `may_reopen`, `may_sync`.

Writing them down showed two gaps. Automatic sync waited while a Guided switch waited for Claude to sign in, because its copies in the target's folder would read as Claude signing in; "Sync now" did not wait. `sync_now_waits_while_a_switch_waits_so_its_copies_confirm_nothing` failed before the change. "Sync now" is now off in Settings while a sync must wait.

The second gap came from the independent review of this round. "Switch Claude to X, then open…" in All Sessions started a Guided switch without asking whether Reopen Claude was under way, which the menu's switch does ask. With `may_sync` in place, such a switch would have sat at "Syncing sidebars into X…" with no sync running. It now asks `may_switch_desktop` like the menu's (`switch_then_open_does_not_start_while_claude_is_being_reopened` failed before).

## Round 19: All Sessions' join, measured and left alone (task 18)

`SessionIndex::sessions` joins the Transcripts and the Sidebar entries two or three times per input. The task said to measure first. With 1,400 Sessions and 5,600 Sidebar entries in a fake home (the owner's Mac has about 1,300 and 6,600), in a release build, one `Core::view` took 7 ms with the All Sessions window open or closed, and a menu open 9 ms. The machine was busy with other builds: a second run took 12 ms, and one taken at a load average near 85 took over 100 ms.

7 to 12 ms per input is not slow enough to earn a cache with its own invalidation, so the join stays as it is. The probe was a temporary ignored test and is not kept.

## Round 20: one rule for what kind of Session a Sidebar entry is (task 8, decision D1)

The Session index (`EntryFile::skipped`) and Sidebar sync (`left_out`, with its own lists of marker fields) each decided whether a Sidebar entry is an SSH, WSL, scheduled-task or No-folder Session. Both ask `sidebar_entry::kind`, a new module among Claude's formats, which keeps sync's rule: WSL, then scheduled-task, then SSH, then No-folder, a JSON `null` marker counting as absent. Sync's `LeftOut` is that kind under its old name; what it reports and logs is unchanged, and `claude_files::is_scratch_workspace` still says what a scratch workspace is.

One difference, decided as D1: an entry whose `originCwd` is a scratch workspace and whose `cwd` is not is now a No-folder Session for the Session index too, so it is left out there; its Session keeps its row when its Transcript exists, with the Transcript's own title. `an_entry_whose_origin_is_a_scratch_workspace_is_skipped_and_a_null_marker_is_not_one` failed before the change. None of the 11,403 entries on the owner's Mac was in that case on 2026-11-28. The Session index now parses an entry as JSON before reading its fields, as sync already did.

## Round 21: leftover folders count for nothing (issue #85, decision D4)

The desktop's Organization and the Guided switch's confirmation took the Sidebar folder Claude wrote last, leftover or not, so round 17 pinned an entry in a leftover folder of the target's account reading as "another Profile". Decision D4 said leftover folders count for nothing in both places. `desktop_account` and `confirmation` now take `SidebarKinds` from their callers and leave leftover folders out; the rule itself is not written again. Every caller passes the same classifier: the menu and the open flows (through `Core::desktop_account`), All Sessions' title precedence, Sidebar sync's run and its watch, and the Guided switch.

`an_entry_in_a_leftover_folder_of_the_targets_account_neither_confirms_nor_contradicts` replaces the pinning test and failed before the change: the switch notified a mismatch instead of waiting. `a_leftover_folder_written_last_does_not_hide_the_desktops_profile` failed too (the header read "Desktop: another Profile"). `a_folder_of_another_account_is_flagged_as_a_mismatch` passes unchanged.

Leaving leftovers out of the desktop's folder showed a third place: Sidebar sync's watch counted an entry Claude writes outside its folder as the dormant multi-account mode, so a write in a leftover folder would now have paused sync. The watch skips leftover folders, which sync never writes. `an_entry_claude_writes_in_a_leftover_folder_does_not_pause_sync` failed with that skip taken out. ADR 0003 has an amendment.

## Where it stands

- The gate is green on the branch: format, lint, and 33 test binaries (400 tests), nine of them architecture checks.
- Open: nothing. The questions left were answered: D2 in `docs/TESTING.md` (no automated AppKit tests for now), D3 in ADR 0006 (one core crate, layers checked by a test).
- Not checked by any test: the AppKit layer (round 16 says how it was checked by hand).
- Persisted files and settings are unchanged. What the user can see changed in three places: "Sync now" is off while a Guided switch waits or Claude restarts, All Sessions leaves out an entry whose `originCwd` is a scratch workspace (round 20; none on the owner's Mac), and a leftover folder no longer decides the desktop's Profile or a Guided switch's outcome (round 21; not seen on the owner's Mac).
