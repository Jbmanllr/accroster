# Architecture task pool

Work that would bring the code closer to `docs/ARCHITECTURE.md`. Each task names what is duplicated or misplaced, where, and what done looks like. Finished rounds are in `PROGRESS.md`. Take a task, do it as one commit with its tests green, then move it to `PROGRESS.md`.

## Open

| # | Task | Where | Done when |
|---|---|---|---|
| 8 | **One answer to "which kind of Session is this entry".** The Session index and Sidebar sync each decide SSH, WSL, scheduled-task and No-folder from an entry's fields. | `sessions.rs` (`EntryFile::skipped`), `sidebar_sync.rs` (`left_out`) | One function classifies an entry; both call it. Blocked on decision D1. |
| 9 | **One file stamp.** Three caches each define "this file changed since I read it" (size and mtime; size and optional mtime; inode, size, mtime, ctime). | `sessions.rs`, `discovered.rs`, `sidebar_sync.rs` (`Stamp`), used by `checks.rs` | One `Stamp` type in a low module; the caches use it. The Session index's saved `size`/`mtime` fields keep their meaning. |
| 11 | **Times shown to the user are formatted in four places.** | `monitor.rs` (`clock_time`, `resets_text`), `notifications.rs` (`reset_time`), `discovered.rs` (`local`), `limit_offers.rs` (`day`), `sessions.rs` (`age`) | One module turns a time into the words the menu shows; the wording of each is unchanged. |
| 12 | **`Core::view` builds the whole menu in one 260-line function** that also patches rows the registry and the CLI switcher built (inserting "Mute notifications" before "Remove…" by matching its title). | `core.rs` | The menu is assembled from named parts; no part finds another's item by its title. |
| 13 | **The registry file is `profiles.rs`, 1,600 lines:** the registry, sign-in, removal, names and rows. | `profiles.rs` | Naming (Profile name, Short label, `unique`) is its own module with its own tests; the rest stays. |

## Waiting for a decision

| # | Question | Why it matters |
|---|---|---|
| D1 | Should the Session index leave out a No-folder Session whose `originCwd` (not its `cwd`) is a scratch workspace, as Sidebar sync does? | Today the two disagree on that one case. Unifying them (task 8) changes which Sessions All Sessions lists, or which ones sync counts as No-folder. |
| D2 | Should the AppKit layer get automated checks (a snapshot of the menu built from a `View`, or UI tests)? | `src/` has no tests by design (`docs/TESTING.md`). Any change there is verified by hand. |
| D3 | Should the core become several crates, so the compiler enforces the layers? | ADR 0006 chose a test instead. Worth it only if the core grows several times over. |
