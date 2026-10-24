# 05 · Settings, plugins and skills across accounts

Rule of thumb: **configured on disk under `~/.claude` → shared by every account. Configured on claude.ai → per account.**

## Settings

| Scope | File | Shared across accounts |
|---|---|---|
| User | `~/.claude/settings.json` | yes |
| User, machine-local | `~/.claude/settings.local.json` | yes |
| Project | `<repo>/.claude/settings.json` | yes |
| Project, local | `<repo>/.claude/settings.local.json` | yes |
| Managed (admin) | `/Library/Application Support/ClaudeCode/managed-settings.json` | yes, if present |
| Desktop preferences | `~/Library/Application Support/Claude/claude_desktop_config.json` | yes, except keys ending in `ByAccount` |

User settings hold the model, permissions, hooks, env vars, output style, `enabledPlugins` and `extraKnownMarketplaces`. None of it is account-bound.

## Claude Code plugins

| Item | Location | Shared |
|---|---|---|
| Installed plugins | `~/.claude/plugins/installed_plugins.json` + `~/.claude/plugins/cache/` | yes |
| Marketplaces | `~/.claude/plugins/marketplaces/<name>/` | yes |
| Enabled state | `enabledPlugins` in user or project settings | yes |
| Plugins synced from claude.ai (`<name>@synced`) | `~/.claude/plugins/synced/<org>_<account>/` | **no**: one bucket per account |

This machine: 16 installed plugins from 6 marketplaces, the same for every account. `plugins/synced/` held a bucket for three of the four accounts, so even inside the shared `~/.claude` folder, claude.ai-synced plugins are kept apart per account ([docs](https://code.claude.com/docs/en/plugins/loading#plugins-synced-from-claude-ai)).

## Skills

| Source | Location | Shared |
|---|---|---|
| Personal skills | `~/.claude/skills/<name>/SKILL.md` | yes |
| Project skills | `<repo>/.claude/skills/` | yes |
| Plugin skills | inside each installed plugin | yes |
| claude.ai skills (Anthropic and org-provided) | desktop: `local-agent-mode-sessions/skills-plugin/<org>/<account>/`; CLI: `~/.claude/skills/synced/` | **no** |

The docs say skills synced by an earlier session "stay on disk" and load in later sessions "signed in to the same account" ([docs](https://code.claude.com/docs/en/skills#skills-synced-from-claude-ai)).

The claude.ai skill set is fetched per account and differs between accounts. Observed: 17 skills common to all four accounts, plus extras per account (B had algorithmic-art, brand-guidelines, learn, mcp-builder, theme-factory, web-artifacts-builder; C had setup-writing-style; `schedule` was missing from three of four). Org catalog plugins (`legal:*`, `sales:*`, …) follow the same rule: they come from the account's organization.

## Memory and instructions

`~/.claude/CLAUDE.md`, files it imports, and per-project memory under `~/.claude/projects/<cwd>/memory/` are on disk and shared by every account.
