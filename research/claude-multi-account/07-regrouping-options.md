# 07 · Regrouping options and tools

## Status

Anthropic ships no feature to merge accounts or show sessions from several accounts in one sidebar. Open requests:

- [Desktop app: session history lost when switching accounts (anthropics/claude-code#48511)](https://github.com/anthropics/claude-code/issues/48511)
- [Allow opening/resuming local sessions not present in the account's sidebar](https://claudeissues.com/issue/74662-claude-desktop-allow-opening-resuming-local-sessions-not-present-in-the-accounts)
- [See sessions across the Claude accounts I own on one machine](https://claudeissues.com/issue/90188-feature-see-sessions-across-the-claude-accounts-i-own-on-one-machine)
- [Multi-account / multi-profile support in Claude Desktop](https://claudeissues.com/issue/60607-feature-request-multi-account-multi-profile-support-in-claude-desktop)
- [Account switch does not isolate local state](https://claudeissues.com/issue/85294-bug-desktop-account-switch-does-not-isolate-local-state-app-storage-is-not-accou)

## Options, best first

| # | Option | Effort | Risk | Gets you |
|---|---|---|---|---|
| 1 | Pick one main account; use others only for extra quota | none | none | Shared config already follows you |
| 2 | `claude --resume` in the CLI | none | none | Any session in a folder, any account |
| 3 | Community session browser over `~/.claude/projects` | install | low (read-only) | Search and resume across all projects and accounts |
| 4 | Copy `local_*.json` entries into the main account's folder | script | medium (unsupported; app may overwrite or ignore) | Old sessions in the main sidebar |
| 5 | CLI account switcher with one `CLAUDE_CONFIG_DIR` per account | install | medium (splits config unless it symlinks back) | Several CLI accounts in parallel |

## Community tools (not verified for maintenance)

**Session browsers** (read `~/.claude/projects`):
[ccsesh](https://awesome.ecosyste.ms/projects/github.com%2Fyedhink%2Fccsesh) ·
[ccrider](https://pkg.go.dev/github.com/neilberkman/ccrider) ·
[cchb](https://docs.rs/crate/cchb/0.9.5) ·
[search-sessions](https://github.com/sinzin91/search-sessions) ·
[session-browser](https://awesome.ecosyste.ms/projects/github.com%2Fgiannimassi%2Fsession-browser) ·
[claude-code-chat-browser](https://github.com/CppDigest/claude-code-chat-browser)

**CLI account switchers** (`CLAUDE_CONFIG_DIR` per account):
[clauth](https://docs.rs/crate/clauth/0.5.4) (symlinks back to `~/.claude`) ·
[ccsw](https://docs.rs/crate/ccsw/0.1.1) ·
[claude-switch](https://github.com/hoangvu12/claude-switch) (notes OAuth tokens live in the Keychain, so macOS OAuth profiles are hard to isolate) ·
[claude-account-switch](https://github.com/ftery0/claude-account-switch) ·
[swapdex](https://docs.rs/crate/swapdex/0.95.0)

## Option 4 procedure (if attempted)

1. Quit the desktop app.
2. Back up `~/Library/Application Support/Claude/claude-code-sessions/`.
3. Copy `local_*.json` from `<other-account>/<other-org>/` into `<main-account>/<main-org>/`.
4. Restart and sign into the main account.
5. If entries do not appear or get removed, restore the backup.

Per-session connector snapshots (`remoteMcpServersConfig`) will reference the other account's connectors and may fail to reconnect.
