# AccRoster

A menu bar app for Claude Code accounts: limits, sessions and one-click switching.

> AccRoster is an independent open-source project. It is not affiliated with, endorsed by, or sponsored by Anthropic. Claude and Claude Code are trademarks of Anthropic, PBC.

AccRoster is a lightweight native macOS menu bar app, written in Rust. It is pre-release: today it is the app skeleton (menu bar item, Settings, About, its own storage and logs); Profiles, limits, switching and the Unified session view are on the way.

## Privacy

AccRoster holds no token and makes no network request. Claude Code does every sign-in, refresh and usage read itself. There is no telemetry, no server and no crash upload: nothing leaves your Mac.

Everything the app owns lives in `~/Library/Application Support/io.github.jbmanllr.accroster/`. Its log (`logs/app.log`, Settings → Reveal logs in Finder) is redacted when written, with no tokens, prompts, titles, emails or project names, so you can attach it to a bug report as it is.

## Build

Requires macOS 14 or later and Rust 1.93 or later.

```sh
scripts/create-signing-identity.sh   # once: a stable self-signed identity in your login keychain
scripts/bundle.sh                    # builds and signs target/release/AccRoster.app
open target/release/AccRoster.app
```

The stable identity keeps Keychain approvals and notification permission across rebuilds; `scripts/bundle.sh --ad-hoc` builds without it. This is local signing only; the app is not notarized.

Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

### Layout

- `core/` (`accroster-core`): all behaviour. Commands and events in, view models and effects out; everything outside goes through one environment boundary (home folder, process spawner, clock, OS facts).
- `src/` (`accroster`): the AppKit UI layer, which only renders view models and forwards clicks.
- `fake-mac/` (`accroster-fake-mac`): the test harness, a fake Mac with a temporary home, a stub `claude`, a fake clock and fake OS facts.

## Prior art

AccRoster is written from scratch, with no copied code. These projects shaped its design:

- ClaudeBar: the "In use" pointer.
- ccsw: the shared and per-Profile file list, and heal-on-launch.
- claude-swap: the lock discipline before touching `.claude.json`.
- nerlichman/claudebar: poll-and-backoff and the live-process liveness check.
- SnlperStripes: the Sidebar copy rules (tombstones, backups).
- CodexBar and Claude-Usage-Tracker: usage-tracker references.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

## Background: Claude on one Mac with several accounts

Full report: [research/claude-multi-account](research/claude-multi-account/README.md).

### How Claude stores data on macOS

There is one desktop app (`/Applications/Claude.app`) and one CLI (`~/.local/bin/claude`). Switching accounts in the app does not create another install; it changes which per-account index the app reads.

#### Shared by every account

| What | Where |
|---|---|
| Session transcripts | `~/.claude/projects/<project>/<session-id>.jsonl` |
| Claude Code plugins and marketplaces | `~/.claude/plugins/` |
| Global settings, hooks, permissions | `~/.claude/settings.json` |
| User skills, `CLAUDE.md`, memory | `~/.claude/` |
| Desktop extensions (local MCP servers) | `~/Library/Application Support/Claude/Claude Extensions/` |
| Desktop app preferences | `~/Library/Application Support/Claude/claude_desktop_config.json` |

#### Separate per account

| What | Where |
|---|---|
| Code tab sidebar index | `~/Library/Application Support/Claude/claude-code-sessions/<account-id>/<org-id>/local_*.json` |
| Cowork sessions | `~/Library/Application Support/Claude/local-agent-mode-sessions/<account-id>/` |
| "No folder" scratch workspaces | `~/Library/Application Support/Claude/scratch-workspaces/<account-id>/` |
| claude.ai skills and plugins (synced copy) | `~/Library/Application Support/Claude/local-agent-mode-sessions/skills-plugin/<org-id>/` |
| Connectors, usage limits, billing | claude.ai account (server side) |

Each `local_*.json` sidebar entry points to a transcript through its `cliSessionId`. The transcript is shared; only the entry is per account. That is why another account's sessions disappear from the sidebar while their history is still on disk.

The CLI signs in to one account at a time (`oauthAccount` in `~/.claude.json`), independently of the desktop app.

### Reaching sessions from any account

`claude --resume` lists sessions by project folder from `~/.claude/projects`, regardless of the account that created them:

```bash
cd ~/Dev/some-project
claude --resume
```

Community session browsers read the same folder across all projects: ccsesh, ccrider, cchb, search-sessions.

### Switching accounts in the CLI

Community switchers give each account its own `CLAUDE_CONFIG_DIR`: clauth, ccsw, claude-switch, claude-account-switch, swapdex. Prefer one that links back to `~/.claude` so plugins, settings and history stay shared.

### Status

Anthropic has no official multi-account or session-merge feature yet. Related requests: anthropics/claude-code#48511 and the multi-account issues tracked on claudeissues.com.
