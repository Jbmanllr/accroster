# Architecture task pool

Work that would bring the code closer to `docs/ARCHITECTURE.md`. Open work is a GitHub issue labelled `ready-for-agent` (see `docs/agents/issue-tracker.md`); this file lists those issues and the questions already answered. Finished rounds are in `PROGRESS.md`.

To add work: write it as an issue that says what is duplicated or misplaced and what done looks like, then list it here.

## Open

None. Tasks 8, 9 and 11 to 18 are done or closed, and decisions D1 and D4 are carried out: see rounds 12 to 21 in `PROGRESS.md`.

## Answered

| Question | Answer | Where it is recorded |
|---|---|---|
| D2: should the AppKit layer get automated checks? | Not now. It has no automated tests by design and is checked by hand. | `docs/TESTING.md` |
| D3: should the core become several crates, so the compiler enforces the layers? | No. A test enforces them; worth another look only if the core grows several times over. | ADR 0006, Considered Options |
