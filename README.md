# AccRoster

A menu bar app for Claude Code accounts: limits, sessions and one-click switching.

> AccRoster is an independent open-source project. It is not affiliated with, endorsed by, or sponsored by Anthropic. Claude and Claude Code are trademarks of Anthropic, PBC.

AccRoster is a lightweight native macOS menu bar app, written in Rust. It is pre-release: today it is the app skeleton (menu bar item, Settings, About, its own storage and logs); Profiles, limits, switching and the Unified session view are on the way.

## Privacy

AccRoster holds no token and makes no network request. Claude Code does every sign-in, refresh and usage read itself. There is no telemetry, no server and no crash upload: nothing leaves your Mac.

Everything the app owns lives in `~/Library/Application Support/io.github.jbmanllr.accroster/`. Its log (`logs/app.log`, Settings → Reveal logs in Finder) is redacted when written, with no tokens, prompts, titles, emails or project names, so you can attach it to a bug report as it is.

## CLI switching

Turn on CLI switching in Settings to make a click on a Profile row pick that Profile for new `claude` runs in your terminal. Running Sessions keep the Profile they started with.

How it works: AccRoster puts a small `claude` shell script (the shim) in `~/Library/Application Support/io.github.jbmanllr.accroster/bin/` and puts that folder first on your `PATH`. The shim reads `bin/cli-active-profile`, which holds the chosen Profile folder's path, sets `CLAUDE_CONFIG_DIR` to it and runs the real `claude` it finds next on your `PATH`. A click rewrites only that file. AccRoster never touches `~/.local/bin/claude`, which Claude Code's installer manages.

- **zsh** is set up for you. Turning CLI switching on backs up `~/.zshrc` and appends a block marked `# >>> accroster >>>` … `# <<< accroster <<<` at its end. Turning it off removes the block exactly as written, with the shim and the pointer. If you edited the block, AccRoster leaves it alone and lists the lines to remove yourself. Your Profiles stay either way.
- **bash**: add this at the end of `~/.bashrc` (and `~/.bash_profile` if your login shells read only that), after any line that puts `~/.local/bin` on `PATH`:

  ```sh
  export PATH="$HOME/Library/Application Support/io.github.jbmanllr.accroster/bin:$PATH"
  ```

- **fish**: add this at the end of `~/.config/fish/config.fish`:

  ```fish
  set -gx PATH "$HOME/Library/Application Support/io.github.jbmanllr.accroster/bin" $PATH
  ```

  Still turn CLI switching on in Settings: that installs the shim. It also adds the zsh block, which other shells ignore.

On every run the shim silently unsets the variables that would put another account's identity or credential under the Profile's name, including the token the Claude desktop app injects into its sessions: `CLAUDE_SECURESTORAGE_CONFIG_DIR`, `CLAUDE_CODE_ACCOUNT_UUID`, `CLAUDE_CODE_USER_EMAIL`, `CLAUDE_CODE_ORGANIZATION_UUID`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_OAUTH_REFRESH_TOKEN`, `CLAUDE_CODE_OAUTH_SCOPES`, `CLAUDE_CODE_SUBSCRIPTION_TYPE` and `CLAUDE_CODE_RATE_LIMIT_TIER`. API-key and provider variables (`ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_PROFILE`, `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_USE_*`) pass through.

- **One terminal on another Profile:** "Copy shell override" in a Profile's submenu copies an `export CLAUDE_CONFIG_DIR=…` line. A non-empty `CLAUDE_CONFIG_DIR` already set wins over the chosen Profile; that is also what keeps a `claude` started inside a Session on that Session's Profile.
- **Escape hatch:** there is no setting to turn the variable clearing off. To skip the shim entirely, run the real binary directly, for example `~/.local/bin/claude`.
- If the chosen Profile's folder is missing, the shim prints an error and exits with status 1. It never falls back to `~/.claude`.
- The IDE extensions, the Claude desktop app and anything not started from your shell don't go through the shim, so they use `~/.claude`.

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
