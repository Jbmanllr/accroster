# claude-account-manager

Notes and tooling for using several Claude accounts on one Mac with the Claude desktop app and Claude Code CLI.

Full report: [research/claude-multi-account](research/claude-multi-account/README.md).

## How Claude stores data on macOS

There is one desktop app (`/Applications/Claude.app`) and one CLI (`~/.local/bin/claude`). Switching accounts in the app does not create another install; it changes which per-account index the app reads.

### Shared by every account

| What | Where |
|---|---|
| Session transcripts | `~/.claude/projects/<project>/<session-id>.jsonl` |
| Claude Code plugins and marketplaces | `~/.claude/plugins/` |
| Global settings, hooks, permissions | `~/.claude/settings.json` |
| User skills, `CLAUDE.md`, memory | `~/.claude/` |
| Desktop extensions (local MCP servers) | `~/Library/Application Support/Claude/Claude Extensions/` |
| Desktop app preferences | `~/Library/Application Support/Claude/claude_desktop_config.json` |

### Separate per account

| What | Where |
|---|---|
| Code tab sidebar index | `~/Library/Application Support/Claude/claude-code-sessions/<account-id>/<org-id>/local_*.json` |
| Cowork sessions | `~/Library/Application Support/Claude/local-agent-mode-sessions/<account-id>/` |
| "No folder" scratch workspaces | `~/Library/Application Support/Claude/scratch-workspaces/<account-id>/` |
| claude.ai skills and plugins (synced copy) | `~/Library/Application Support/Claude/local-agent-mode-sessions/skills-plugin/<org-id>/` |
| Connectors, usage limits, billing | claude.ai account (server side) |

Each `local_*.json` sidebar entry points to a transcript through its `cliSessionId`. The transcript is shared; only the entry is per account. That is why another account's sessions disappear from the sidebar while their history is still on disk.

The CLI signs in to one account at a time (`oauthAccount` in `~/.claude.json`), independently of the desktop app.

## Reaching sessions from any account

`claude --resume` lists sessions by project folder from `~/.claude/projects`, regardless of the account that created them:

```bash
cd ~/Dev/some-project
claude --resume
```

Community session browsers read the same folder across all projects: ccsesh, ccrider, cchb, search-sessions.

## Switching accounts in the CLI

Community switchers give each account its own `CLAUDE_CONFIG_DIR`: clauth, ccsw, claude-switch, claude-account-switch, swapdex. Prefer one that links back to `~/.claude` so plugins, settings and history stay shared.

## Status

Anthropic has no official multi-account or session-merge feature yet. Related requests: anthropics/claude-code#48511 and the multi-account issues tracked on claudeissues.com.
