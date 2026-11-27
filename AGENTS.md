# AGENTS.md

## Agent skills

### Issue tracker

Issues live in GitHub Issues on Jbmanllr/accroster (gh CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: GLOSSARY.md and docs/adr/ at the repo root. See `docs/agents/domain.md`.

## Architecture

Read `docs/ARCHITECTURE.md` before adding a module, a dependency between modules, or anything that reads Claude's files: it says which module owns what. `cargo test -p accroster-core --test architecture` checks its rules; when a check fails, follow its message instead of working around it. Open architecture work is in `docs/architecture/TASKS.md`.
