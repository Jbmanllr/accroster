# Architecture task pool

Work that would bring the code closer to `docs/ARCHITECTURE.md`. Open work is a GitHub issue labelled `ready-for-agent` (see `docs/agents/issue-tracker.md`); this file lists those issues and the questions already answered. Finished rounds are in `PROGRESS.md`.

To add work: write it as an issue that says what is duplicated or misplaced and what done looks like, then list it here.

## Open

| Issue | Work |
|---|---|
| [#84](https://github.com/Jbmanllr/accroster/issues/84) | One rule for what kind of Session a Sidebar entry is (SSH, WSL, scheduled-task, No-folder), used by the Session index and Sidebar sync. Was task 8. |
| [#85](https://github.com/Jbmanllr/accroster/issues/85) | Leftover Sidebar folders are ignored when telling the desktop's Profile and confirming a Guided switch. |

Tasks 9 and 11 to 18 are done or closed: see rounds 12 to 19 in `PROGRESS.md`.

## Answered

| Question | Answer | Where it is recorded |
|---|---|---|
| D1: should the Session index leave out a No-folder Session whose `originCwd`, not its `cwd`, is a scratch workspace, as Sidebar sync does? | Yes: one rule, Sidebar sync's. None of the 11,403 entries on the owner's Mac was in that case on 2026-11-28. | Issue #84 |
| D2: should the AppKit layer get automated checks? | Not now. It has no automated tests by design and is checked by hand. | `docs/TESTING.md` |
| D3: should the core become several crates, so the compiler enforces the layers? | No. A test enforces them; worth another look only if the core grows several times over. | ADR 0006, Considered Options |
| D4: should a leftover Sidebar folder count for nothing when the app works out the desktop's account and confirms a Guided switch? | Yes. | Issue #85 |
