# Architecture task pool

Work that would bring the code closer to `docs/ARCHITECTURE.md`. Each task names what is duplicated or misplaced, where, and what done looks like. Finished rounds are in `PROGRESS.md`. Take a task, do it as one commit with its tests green, then move it to `PROGRESS.md`.

## Open

| # | Task | Where | Done when |
|---|---|---|---|
| 8 | **One answer to "which kind of Session is this entry".** The Session index and Sidebar sync each decide SSH, WSL, scheduled-task and No-folder from an entry's fields. | `sessions.rs` (`EntryFile::skipped`), `sidebar_sync.rs` (`left_out`) | One function classifies an entry; both call it. Blocked on decision D1. |

Tasks 9 and 11 to 18 are done or closed: see rounds 12 to 19 in `PROGRESS.md`.

## Waiting for a decision

| # | Question | Why it matters |
|---|---|---|
| D1 | Should the Session index leave out a No-folder Session whose `originCwd` (not its `cwd`) is a scratch workspace, as Sidebar sync does? | Today the two disagree on that one case. Unifying them (task 8) changes which Sessions All Sessions lists, or which ones sync counts as No-folder. |
| D2 | Should the AppKit layer get automated checks (a snapshot of the menu built from a `View`, or UI tests)? | `src/` has no tests by design (`docs/TESTING.md`). Any change there is verified by hand. |
| D3 | Should the core become several crates, so the compiler enforces the layers? | ADR 0006 chose a test instead. Worth it only if the core grows several times over. |
| D4 | Should a leftover Sidebar folder count for nothing when the app works out the desktop's account and confirms a Guided switch? | Today both take the folder Claude wrote last, leftover or not. If Claude writes an entry in a leftover folder of the target's account, the switch reports "signed in as another Profile" and the header reads "Desktop: another Profile" (`an_entry_in_a_leftover_folder_of_the_targets_account_is_flagged_as_a_mismatch` pins it). On the owner's Mac leftover folders held no Session of their own, so this has not been seen. |
