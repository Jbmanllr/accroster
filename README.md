# AccRoster

A menu bar app for Claude Code accounts: limits, sessions and one-click switching.

> AccRoster is an independent open-source project. It is not affiliated with, endorsed by, or sponsored by Anthropic. Claude and Claude Code are trademarks of Anthropic, PBC.

AccRoster is a lightweight native macOS menu bar app, written in Rust. It is pre-release: today it is the app skeleton (menu bar item, Settings, About, its own storage and logs); Profiles, limits, switching and the Unified session view are on the way.

## Install

1. Open the [Releases page](https://github.com/Jbmanllr/accroster/releases) (the first release is not published yet) and download the latest `AccRoster-<version>.dmg`. Each release lists its SHA-256 so you can check the download.
2. Open the dmg and drag AccRoster onto the Applications folder next to it.
3. Open AccRoster from Applications. It appears in the menu bar.

Releases are signed with a Developer ID and notarized by Apple, so macOS opens them without a security warning. One download runs on Apple Silicon and Intel Macs. It needs macOS 14 or later.

Install [Claude Code](https://code.claude.com/docs/en/overview) first: AccRoster drives the `claude` it finds on your Mac and holds no sign-in of its own. Opened from Finder or the Dock, an app gets no shell `PATH`, so AccRoster also looks in `~/.local/bin`, `~/.claude/local`, `/opt/homebrew/bin` and `/usr/local/bin`, then asks your login shell once; it never uses the desktop app's bundled copy. If it still finds none, the menu says so: install Claude Code, or open AccRoster from a terminal once (it remembers where `claude` was).

macOS asks for permissions when a feature first needs them:

- **Notifications**, when a limit threshold is reached.
- **Automation (Terminal)**, the first time you use Resume in Terminal.
- **Launch at login** is offered on first run and in Settings.

To update, install the new release over the old one; the permissions stay. To remove AccRoster, see [Uninstall](#uninstall).

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

## Uninstall

Use Settings → Uninstall…. It lists what it removes and what stays, then runs these steps in order. Each step is skipped when it is already done, so if it stops or is interrupted, run it again to finish; `claude` keeps working at every point.

1. Unhook the CLI, as turning CLI switching off does: the `~/.zshrc` block goes first, then the shim and the pointer. New and open terminals then run the real `claude` with `~/.claude`.
2. Remove each Profile: sign it out with Claude Code, then delete its folder. That deletes the Profile's own prompt history, and the MCP servers and folder trust added after it was set up. Signing out asks Anthropic to revoke the login; offline, that revoke may not have happened, though the login is deleted from your Mac. If a sign-out fails, that folder is kept so its login can still be revoked, and Uninstall stops with Retry.
3. Unregister launch at login.
4. Delete `~/Library/Application Support/io.github.jbmanllr.accroster/` permanently.
5. Move the app to the Trash.

Sidebar entries that Sidebar sync copied into other Profiles' sidebars, folders AccRoster created in `~/.claude`, and the notification permission (System Settings → Notifications) stay.

By hand, the same steps:

1. Remove the `# >>> accroster >>>` … `# <<< accroster <<<` block from `~/.zshrc` (and any `PATH` line you added for bash or fish), then open a new terminal.
2. For each folder in `~/Library/Application Support/io.github.jbmanllr.accroster/profiles/`, sign it out, then delete it:

   ```sh
   cd ~/Library/Application\ Support/io.github.jbmanllr.accroster/profiles
   for dir in "$PWD"/*/; do
     dir="${dir%/}"
     CLAUDE_CONFIG_DIR="$dir" ~/.local/bin/claude auth logout && rm -rf "$dir"
   done
   ```

   Pass the folder's path exactly as above, with no trailing slash: Claude Code names each Profile's Keychain item after that exact string.
3. Remove AccRoster in System Settings → General → Login Items.
4. Delete `~/Library/Application Support/io.github.jbmanllr.accroster/`.
5. Move `AccRoster.app` to the Trash.

Trashing the app without uninstalling is harmless: the shim and the `~/.zshrc` block stay, and `claude` keeps routing through the shim to the Profile it last pointed at, because the shim is a plain shell script that doesn't need the app. Run steps 1–4 above whenever you want the rest gone.

## Build from source

Requires macOS 14 or later and Rust 1.93 or later.

```sh
scripts/create-signing-identity.sh   # once: a stable self-signed identity in your login keychain
scripts/bundle.sh                    # builds and signs target/release/AccRoster.app
open target/release/AccRoster.app
```

The stable identity keeps Keychain approvals and notification permission across rebuilds; `scripts/bundle.sh --ad-hoc` builds without it. This is local signing only; the app is not notarized, and launch at login is not available in a local build. Releases are built by `scripts/release.sh`; see [Releasing](#releasing).

Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. CI (`.github/workflows/ci.yml`) runs the same three on `macos-latest` for every pull request and push to `main`, then a release dry run (see [Releasing](#releasing)); Developer ID signing and notarization run only for releases.

### Layout

- `core/` (`accroster-core`): all behaviour. Commands and events in, view models and effects out; everything outside goes through one environment boundary (home folder, process spawner, clock, OS facts).
- `src/` (`accroster`): the AppKit UI layer, which only renders view models and forwards clicks.
- `fake-mac/` (`accroster-fake-mac`): the test harness, a fake Mac with a temporary home, a stub `claude`, a fake clock and fake OS facts.

## Releasing

Maintainers: [docs/RELEASING.md](docs/RELEASING.md) covers the one-time Apple and GitHub setup, how to cut a release and how to recover from a failed one.

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
