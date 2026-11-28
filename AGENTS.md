# AGENTS.md

Release status: pre-release.

## Real Mac

- Run AccRoster on the real home folder only as the one installed `/Applications/AccRoster.app`. A second copy (`cargo run`, a bundle started from `target/`) spawns its own usage reads, and refresh tokens are single-use (ADR 0001), so two copies can log a Profile out. To see a change work, drive the core through the fake Mac (`fake-mac/`) in a test.

## Architecture

Read `docs/ARCHITECTURE.md` before adding a module, a dependency between modules, or anything that reads Claude's files: it says which module owns what. `cargo test -p accroster-core --test architecture` checks its rules; when a check fails, follow its message instead of working around it. Open architecture work is in `docs/architecture/TASKS.md`.

## Verification

`docs/TESTING.md` holds the gate: T0 while editing, T1 (format, lint, tests) before pushing, and what no gate covers. A change in `src/ui.rs` or `src/ui/` has no automated test and is checked by hand (its "Not covered by any gate").

## Agent skills

### Issue tracker

Issues live in GitHub Issues on Jbmanllr/accroster (gh CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: GLOSSARY.md and docs/adr/ at the repo root. See `docs/agents/domain.md`.
