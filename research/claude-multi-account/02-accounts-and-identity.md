# 02 · Accounts and identity

## Account vs organization

Every claude.ai login has an **account id** and an **organization id** (a personal plan gets its own one-person organization). The desktop app nests per-account data two levels deep:

```
<folder>/<account-id>/<org-id>/…
```

so one account that belongs to several organizations (for example a personal plan and a Team plan) gets one sub-folder per organization.

Account first, organization second was confirmed here by matching the CLI's `oauthAccount.accountUuid` and `organizationUuid` against the folder names. A user report (anthropics/claude-code#101545) gives the order as `<org>/<user>`; the layout may differ between app versions. `plugins/synced/` and `skills-plugin/` use the opposite order (`<org>_<account>`, `<org>/<account>`).

## What this machine shows

Four accounts have been used in the desktop app. Ids are replaced with letters here.

| Account | Code sessions indexed | Last used | CLI logged in |
|---|---|---|---|
| A | 164 | 2026-09-23 | no |
| B | 286 | 2026-10-18 | **yes** |
| C | 42 | 2026-10-14 | no |
| D | 64 | 2026-10-21 | no |

## How switching works

1. **Desktop app:** the account menu signs out of claude.ai and signs into another. The app then reads the new account's folders under `claude-code-sessions/`, `local-agent-mode-sessions/` and `scratch-workspaces/`, and fetches that account's connectors, skills and plan limits from claude.ai.
2. **CLI:** `/logout` then `/login` replaces the token in the `Claude Code-credentials` Keychain item and the `oauthAccount` block in `~/.claude.json`. Only one CLI account is active per config directory.
3. **Running both at once:** the CLI can run under a different account than the app. To run several CLI accounts in parallel you need a separate `CLAUDE_CONFIG_DIR` per account (see [07](07-regrouping-options.md)).

## The official multi-account answer: `CLAUDE_CONFIG_DIR`

Anthropic's documented approach is one config directory per account. Each directory gets its own settings, transcripts, plugins, `.claude.json` and, per the docs, its own Keychain entry; a maintainer closed the main feature request (#20131) on that basis. Caveats from open reports:

- Keychain separation per config directory may not hold in practice (#20553).
- The desktop app's built-in Claude Code can overwrite the terminal CLI's login (#101990).
- The desktop app honors `CLAUDE_CONFIG_DIR` only from its launch environment, after a restart: one config directory per running app, not per account.

Details and citations: [90-official-sources.md](90-official-sources.md).

## Settings stored per account inside shared files

`claude_desktop_config.json` is one file, but some preferences are keyed by account id:

- `bypassPermissionsOptInByAccount`, `bypassPermissionsGateByAccount`
- `coworkModelAutoFallbackByAccount`
- `remoteChatFolderGrants` (folder access granted to claude.ai chats, keyed by account and org)

`config.json` keys the extension allowlist by organization (`dxt:allowlist…:<org-id>`), and `mcp-user-tool-toggles.json` keys per-tool on/off switches by account (`owners`).
