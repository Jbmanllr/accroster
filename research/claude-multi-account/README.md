# Claude multi-account on macOS

How the Claude desktop app and the Claude Code CLI store accounts, sessions, settings, plugins, skills and connectors when several claude.ai accounts are used on one Mac, and how to bring sessions back together.

Researched 2026-10-24 on macOS with the Claude desktop app (Code tab) and Claude Code CLI 2.1.296. Account and organization ids are replaced with letters.

## Summary

- **One install.** One desktop app and one CLI. Switching accounts changes which account-scoped folders the app reads; nothing is reinstalled.
- **Transcripts are shared, the sidebar is not.** Every session's transcript lives in `~/.claude/projects`, whatever the account. The desktop sidebar is built from a per-account index (`claude-code-sessions/<account>/<org>/local_*.json`), so another account's sessions disappear from view while staying on disk.
- **The CLI ignores accounts when resuming.** `claude --resume` lists transcripts by folder (`Ctrl+A` in the picker widens to every project), so it reaches sessions from every account.
- **Official multi-account route is `CLAUDE_CONFIG_DIR`.** One config directory per account, with open bugs around Keychain sharing and the desktop app overwriting the CLI login.
- **Disk config follows you; claude.ai config does not.** Settings, hooks, Claude Code plugins, personal skills, `CLAUDE.md`, local MCP servers and desktop extensions are shared. claude.ai connectors, claude.ai skills and org plugins, cloud sessions, usage limits and billing are per account.
- **CLI and app log in separately.** The CLI's account (`Claude Code-credentials` in the Keychain, `oauthAccount` in `~/.claude.json`) is independent of the app's.
- **No official merge.** Regrouping today means one main account plus `claude --resume` or a community session browser.

## Contents

| File | Topic |
|---|---|
| [01-installs-and-folders.md](01-installs-and-folders.md) | What is installed, full folder map, Keychain items |
| [02-accounts-and-identity.md](02-accounts-and-identity.md) | Account vs organization, how switching works, per-account keys in shared files |
| [03-sessions-desktop.md](03-sessions-desktop.md) | Sidebar index format, retrieval flow, counts per account |
| [04-sessions-cli.md](04-sessions-cli.md) | Transcript storage, `--resume` / `--continue`, retention, CLI vs desktop |
| [05-settings-plugins-skills.md](05-settings-plugins-skills.md) | Settings scopes, plugins, skills: shared vs per account |
| [06-connectors-and-mcp.md](06-connectors-and-mcp.md) | claude.ai connectors vs Claude Code MCP vs desktop extensions |
| [07-regrouping-options.md](07-regrouping-options.md) | Open issues, options ranked, community tools |
| [90-official-sources.md](90-official-sources.md) | What Anthropic's docs and repo say, with citations |

## Shared vs per account at a glance

| Item | Shared | Per account |
|---|:-:|:-:|
| Session transcripts (`~/.claude/projects`) | ✓ | |
| Desktop Code sidebar, titles, archive state | | ✓ |
| Cowork sessions, "No folder" workspaces | | ✓ |
| Cloud sessions (claude.ai/code) | | ✓ |
| `~/.claude/settings.json`, hooks, permissions | ✓ | |
| Claude Code plugins and marketplaces | ✓ | |
| Personal and project skills | ✓ | |
| claude.ai skills and org plugins (incl. `~/.claude/plugins/synced/<org>_<account>`) | | ✓ |
| `CLAUDE.md` and memory | ✓ | |
| Claude Code MCP servers (`~/.claude.json`, `.mcp.json`) | ✓ | |
| Desktop extensions | ✓ | |
| claude.ai connectors | | ✓ |
| CLI login | one at a time per config dir | |
| Usage limits and billing | | ✓ |

## Method

Local evidence comes from reading the files listed in each section on one Mac (structure and field names only; no credentials were read). Official behavior is checked against Anthropic documentation and the anthropics/claude-code repository in [90-official-sources.md](90-official-sources.md). Where the two disagree, the section notes it.
