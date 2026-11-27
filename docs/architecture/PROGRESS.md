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

## Round 4: the rules become checks

`core/tests/architecture.rs` (seven checks), ADR 0006, `docs/ARCHITECTURE.md`. Each behavioural check was shown to fail on a planted violation, with the file and line.
