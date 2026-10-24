# 01 · Installs and folders

Observed on macOS (Darwin 27), Claude desktop app with the Code tab, Claude Code CLI 2.1.296, October 2026.

## What is installed

| Component | Path | Notes |
|---|---|---|
| Desktop app | `/Applications/Claude.app` | Single install. Bundle id `com.anthropic.claudefordesktop`. |
| CLI launcher | `~/.local/bin/claude` | Symlink to the active native build. |
| CLI builds | `~/.local/share/claude/versions/<x.y.z>` | The native installer keeps the last few versions for rollback. |
| URL handler | `~/Applications/Claude Code URL Handler.app` | Opens `claude-cli://` links in the CLI. Not a second install. |
| Third-party mode config | `~/Library/Application Support/Claude-3p/claude_desktop_config.json` | Used when the app runs against a third-party provider (Bedrock, Vertex). Unused on a claude.ai login. |

Switching accounts never installs another copy. It changes which account-scoped folders the same app reads.

## Folder map

```
~/.claude/                                   ← shared by every account (CLI + desktop Code tab)
├── projects/<encoded-cwd>/<session-id>.jsonl   transcripts
├── history.jsonl                               prompt history (display, project, sessionId)
├── sessions/<pid>.json                         live CLI process registry
├── settings.json, settings.local.json          user settings
├── CLAUDE.md                                   user memory / instructions
├── skills/  plugins/  hooks/  plans/  tasks/
└── file-history/  shell-snapshots/  todos …

~/.claude.json                               ← CLI state: oauthAccount, per-project trust, user MCP servers

~/Library/Application Support/Claude/        ← desktop app (Electron profile)
├── claude-code-sessions/<account>/<org>/local_<id>.json    per-account Code sidebar index
├── local-agent-mode-sessions/<account>/<org>/…             per-account Cowork sessions
├── local-agent-mode-sessions/skills-plugin/<org>/<account>/ per-account synced claude.ai skills
├── scratch-workspaces/<account>/<org>/scratch-*            per-account "No folder" sessions
├── Claude Extensions/                                      desktop extensions (local MCP), shared
├── claude_desktop_config.json                              app preferences, shared (some keys per account)
├── config.json                                             locale, theme, OAuth token cache
└── Cookies, Local Storage, IndexedDB …                     web session for claude.ai
```

## Credentials (macOS Keychain)

| Keychain service | Used by |
|---|---|
| `Claude Code-credentials` | CLI OAuth token for the account it is logged into. |
| `Claude Safe Storage` | Electron safe-storage key that encrypts the desktop app's cookies and token cache. |

The CLI and the desktop app hold separate logins. Signing into another account in the app does not change the CLI's account (`oauthAccount` in `~/.claude.json`), and the reverse.
