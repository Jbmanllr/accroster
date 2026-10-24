# How Claude handles multiple accounts on macOS: official sources

Researched 2026-10-24. Sources are limited to code.claude.com (Claude Code docs, including the generated changelog), support.claude.com, and the `anthropics/claude-code` GitHub repository and its issues. Labels used below:

- **Documented**: stated in official docs or help-center articles.
- **Issue report**: stated in a GitHub issue by a user. It is primary evidence of observed behavior, but Anthropic has not confirmed it.
- **Inference**: my reasoning from documented facts.
- **UNVERIFIED**: no primary source found.

## Summary

- **The one supported multi-account method is `CLAUDE_CONFIG_DIR`, one directory per account.** The docs say each directory has its own settings, session history, plugins and login. They also say the macOS Keychain entry is keyed to the directory. A maintainer closed the main feature request (#20131) by pointing to this. Feature requests for a native switcher in the CLI (#30031), the Desktop app (#18435) and VS Code (#55621) are still open.
- **Transcripts live in `<config dir>/projects/<project>/<session-id>.jsonl`.** CLI sessions and Desktop Code-tab sessions both write there. The CLI `/resume` picker is scoped to the current worktree by default and can widen to every project. Retention is controlled by `cleanupPeriodDays` (default 30) and, for Desktop/Cowork transcripts, `desktopSessionCleanupPeriodDays`.
- **Desktop and the CLI share configuration** (`~/.claude.json`, `~/.claude/settings.json`, CLAUDE.md, MCP servers, user-scope plugins) **but keep separate session lists.** Desktop can open a CLI session in place with `/resume` or `claude --desktop --resume <id>`. The docs do not describe Desktop's per-account session index.
- **Several things are account-scoped and arrive with the claude.ai login:** claude.ai connectors, synced skills (`~/.claude/skills/synced/`), synced plugins (`~/.claude/plugins/synced/`), server-managed settings and cloud sessions. Everything a user creates on disk, such as personal skills, user settings and user/local MCP servers, is keyed to the config directory, not to the account.
- **Credential isolation is uneven in practice.** Issues report the Keychain item `Claude Code-credentials` being shared across `CLAUDE_CONFIG_DIR` profiles (#20553 open, #70697) and being overwritten by Desktop's bundled Claude Code (#101990, open). Current docs say the entry is keyed per directory. Treat this as a real risk area.

---

## 1. Claude Code CLI authentication

**Credential storage on macOS (documented).** "On macOS, credentials are stored in the encrypted macOS Keychain." If the Keychain rejects the write, for example when it is locked in an SSH session, Claude Code falls back to `~/.claude/.credentials.json` with mode `0600`. Source: https://code.claude.com/docs/en/authentication#credential-management

**Effect of `CLAUDE_CONFIG_DIR` on credentials (documented).** With `CLAUDE_CONFIG_DIR` set, Claude Code keeps `.credentials.json` under that directory and "keys the macOS Keychain entry to that directory too, so a session with a different `CLAUDE_CONFIG_DIR` reads a different entry." Source: https://code.claude.com/docs/en/authentication#credential-management

**Keychain service name (UNVERIFIED in docs; issue report).** The docs do not name the Keychain item. Several issues call it `Claude Code-credentials`, for example #101990, #94491 and #87348. No source documents the suffix or derivation for non-default config directories. Issue #104155 reports that MCP OAuth tokens are stored in the same Keychain credential payload. The docs confirm that `/logout` also removes "saved MCP server logins, and plugin sensitive values" (https://code.claude.com/docs/en/troubleshoot-install). Sources: https://github.com/anthropics/claude-code/issues/101990, https://github.com/anthropics/claude-code/issues/104155

**Conflicting issue reports on isolation (issue report).**
- #70697 (June 2026, closed as a duplicate) reported that all `CLAUDE_CONFIG_DIR` profiles share the same `Claude Code-credentials` entry. It was closed as a duplicate of #20553, which is still **open**: "OAuth credentials shared across CLAUDE_CONFIG_DIR profiles".
- The current docs state per-directory keying.
- I found no changelog entry that announces a fix.
- Status: documented intent is per-directory isolation; whether all versions actually isolate is UNVERIFIED.

Sources: https://github.com/anthropics/claude-code/issues/70697, https://github.com/anthropics/claude-code/issues/20553

**Desktop and CLI share the default Keychain item (issue report).** #101990 (open, October 2026) reports that the Claude Code bundled in Claude Desktop writes the same `Claude Code-credentials` item as the standalone CLI. When Desktop spawns a Code session, it overwrites the CLI's login. Source: https://github.com/anthropics/claude-code/issues/101990

**`/login`, `/logout` and `claude auth` (documented).**
- `/login` means "Sign in to your Anthropic account"; `/logout` means "Sign out from your Anthropic account". Source: https://code.claude.com/docs/en/commands
- `/logout` "also resets your first-launch setup state". Source: https://code.claude.com/docs/en/authentication
- `claude auth login` accepts `--email`, `--sso` and `--console`. Source: https://code.claude.com/docs/en/cli-reference
- `claude auth status` prints JSON that includes `configDirectory` (v2.1.268+) and `authMethod`. Source: https://code.claude.com/docs/en/cli-reference
- `/status` shows the active account. When both a login and an API key exist, it marks the one not in use. Source: https://code.claude.com/docs/en/authentication#authentication-precedence

**Switching accounts (documented).** The docs give no in-place account switcher. There are two documented routes:
1. `/logout` followed by `/login`.
2. One config directory per account. The "Log in with multiple accounts" section says each directory "has its own settings, session history, and claude.ai login or API key", with the example `alias claude-work='CLAUDE_CONFIG_DIR=~/.claude-work claude'`. Caveat: separate directories "don't keep two Claude Console sign-ins without an API key apart", because that kind of sign-in is stored outside the config directory as an Anthropic profile.

Source: https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts

**What `CLAUDE_CONFIG_DIR` relocates (documented).**
- The env-vars reference says it overrides the configuration directory (default `~/.claude`): "All settings, session history, and plugins are stored under this path". It is "Useful for running multiple accounts side by side". It is ignored in project and local settings. Source: https://code.claude.com/docs/en/env-vars
- "every `~/.claude` path on this page lives under that directory instead". This covers `projects/` transcripts, `history.jsonl`, `plugins/`, `skills/` and the caches. Source: https://code.claude.com/docs/en/claude-directory
- "the global config file lives under that directory instead of your home directory". In other words, `~/.claude.json` moves into the config directory. Source: https://code.claude.com/docs/en/troubleshoot-install
- The plugins root can be moved separately with `CLAUDE_CODE_PLUGIN_CACHE_DIR`. Source: https://code.claude.com/docs/en/plugins/loading#find-plugins-on-disk
- `CLAUDE_CODE_PROJECT_DIR_NAME` takes effect only together with `CLAUDE_CONFIG_DIR`. Source: https://code.claude.com/docs/en/env-vars

**Role of `~/.claude.json` (documented).** It holds "App state, account details, UI toggles, personal MCP servers" (https://code.claude.com/docs/en/claude-directory). The settings page says it "holds your account details, MCP server configurations, per-project state such as trust decisions, and the global config keys" (https://code.claude.com/docs/en/settings). Global config keys are honored only there (https://code.claude.com/docs/en/settings-reference#global-config-settings). Earlier versions are backed up to `~/.claude/backups/`, keeping the five newest (https://code.claude.com/docs/en/claude-directory).

**`apiKeyHelper` (documented).** A setting that runs a shell script returning an API key. It applies to the CLI, the VS Code extension, the Agent SDK and GitHub Actions. "Claude Desktop and cloud sessions do not call `apiKeyHelper` or read these environment variables". Source: https://code.claude.com/docs/en/authentication#credential-management

**Authentication precedence (documented).** From highest to lowest:
1. Cloud provider env (Bedrock, Vertex, Foundry)
2. `ANTHROPIC_AUTH_TOKEN`
3. `ANTHROPIC_API_KEY`, used once you approve it in interactive mode and always in `-p` mode
4. `apiKeyHelper`
5. `CLAUDE_CODE_OAUTH_TOKEN`
6. Anthropic profile or federation credentials
7. Subscription OAuth from `/login`

A signed-in Claude apps gateway session outranks all of these. Source: https://code.claude.com/docs/en/authentication#authentication-precedence

The docs also warn: "If you have an active Claude subscription but also have `ANTHROPIC_API_KEY` set ... Claude Code uses the API key once you approve it." Source: same page.

## 2. Session storage and retrieval in the CLI

**Transcript location (documented).** Transcripts are stored as JSONL at `~/.claude/projects/<project>/<session-id>.jsonl`. `<project>` is the working-directory path with non-alphanumerics replaced by `-`; names over 200 characters are truncated and hashed. The format is internal and "changes between versions". Subagent transcripts live in `projects/<project>/<session>/subagents/`. Sources: https://code.claude.com/docs/en/sessions#where-transcripts-are-stored, https://code.claude.com/docs/en/claude-directory

**Resume flags (documented).**
- `claude --continue` "Reopens the most recent session in the current directory".
- `claude --resume` opens the picker. It also accepts a name, a session ID or an absolute `.jsonl` path.
- `/resume` switches sessions from inside a session.
- `--resume` "reopens a conversation from this machine's local history and doesn't list cloud sessions". Cloud sessions use `--teleport`.

Sources: https://code.claude.com/docs/en/sessions#resume-a-session, https://code.claude.com/docs/en/claude-code-on-the-web

**Picker scope (documented).** "Claude Code stores sessions per project directory." By default the picker shows sessions from the current worktree, plus sessions that added this directory with `/add-dir`. `Ctrl+W` widens to all worktrees and `Ctrl+A` widens "to every project on this machine". `claude -p`, Agent SDK and `/loop`-first sessions are excluded from the picker. `--resume <id>` searches the current project first, then other projects. Source: https://code.claude.com/docs/en/sessions#where-the-session-picker-looks

**Picker and accounts (inference).** The picker reads `projects/` under the active config directory. The docs mention no account filter. With a single config directory, sessions from every account you have logged into appear together. With one config directory per account, each account sees only its own sessions.

**Retention (documented).**
- `cleanupPeriodDays` defaults to 30, minimum 1. The sweep deletes old transcripts silently, so they disappear from `/resume`. Source: https://code.claude.com/docs/en/settings-reference
- Transcripts of sessions started or last continued in Claude Desktop or Cowork are kept at any age unless `desktopSessionCleanupPeriodDays` is set (v2.1.248+), or managed settings or the HIPAA configuration impose `cleanupPeriodDays`. Source: https://code.claude.com/docs/en/claude-directory#cleaned-up-automatically
- `history.jsonl`, the prompt history, is kept until deleted. Source: same page.

## 3. Claude desktop app (Code tab)

**Switching accounts in the app (documented, help center).** The help center describes switching between an individual account and a Team/Enterprise account "tied to the same email address": "Click your initials in the bottom left corner", select the account, and look for the check mark. Each account gives access to "its separate conversations and projects". Neither article names the desktop app specifically or covers accounts with different emails. Sources: https://support.claude.com/en/articles/13189465-log-in-to-your-claude-account, https://support.claude.com/en/articles/9267400-move-your-personal-claude-account-to-a-team-or-enterprise-organization

**Multi-account switching with different emails in Desktop (documented as not shipped).** Feature request #18435, "manage multiple Claude accounts within the Claude Desktop app", is **open**. #30565 was closed as its duplicate. Sources: https://github.com/anthropics/claude-code/issues/18435, https://github.com/anthropics/claude-code/issues/30565

**Relation to CLI sessions (documented).**
- Desktop "runs the same underlying engine". Desktop and the CLI each keep "its own session list".
- `/desktop` or `claude --desktop --resume <session-id>` moves a CLI session into Desktop.
- Desktop's `/resume` "lists the sessions you started from the CLI on this computer". Desktop "continues the same session rather than a copy, so `claude --resume` in the terminal still finds it afterwards".
- Claude Code keeps Desktop and Cowork transcripts under its own retention rules (section 2). This places Desktop session transcripts in the same `projects/` store.

Sources: https://code.claude.com/docs/en/desktop#coming-from-the-cli, https://code.claude.com/docs/en/sessions, https://code.claude.com/docs/en/claude-directory

**Shared configuration (documented).** Desktop and the CLI share CLAUDE.md, MCP servers in `~/.claude.json` and `.mcp.json`, hooks, skills, and "Settings in `~/.claude.json` and `~/.claude/settings.json`". Source: https://code.claude.com/docs/en/desktop#shared-configuration

**`CLAUDE_CONFIG_DIR` in Desktop (documented).** You can set it for Desktop local sessions in one of the places you would set a dev-server variable, "then quit and reopen Desktop". This implies one config directory per Desktop process, not one per account. Source: https://code.claude.com/docs/en/desktop#environment-variables-in-local-sessions

**Desktop's per-account local session index (UNVERIFIED in docs; issue report).** No doc describes where Desktop stores its sidebar list. Issues describe the location:
- `~/Library/Application Support/Claude/claude-code-sessions/<org>/<user>/local_<uuid>.json`, with a `cliSessionId` field that points to the CLI transcript (#101545, open).
- Index entries whose `cliSessionId` was nulled and flagged `transcriptUnavailable` (#92825, #100364).
- A stale index leaving the sidebar empty of CLI sessions (#91017).

Sources: https://github.com/anthropics/claude-code/issues/101545, https://github.com/anthropics/claude-code/issues/92825, https://github.com/anthropics/claude-code/issues/100364, https://github.com/anthropics/claude-code/issues/91017

**Cloud sessions are per account (documented).**
- Teleport requires that "You must be authenticated to the same claude.ai account used in the cloud session".
- CLI workflows for cloud sessions "require the Claude Code CLI signed in to the same claude.ai account".
- Cloud sessions are listed at claude.ai/code. A local session appears there only once Remote Control connects it, and then on "devices signed in to your claude.ai account".

Sources: https://code.claude.com/docs/en/claude-code-on-the-web, https://code.claude.com/docs/en/desktop#control-which-sessions-appear-on-your-other-devices

**Desktop auth failures (documented).** The troubleshooting step for 403 errors in the Code tab is "Sign out and back in from the app menu". Source: https://code.claude.com/docs/en/desktop#403-or-authentication-errors-in-the-code-tab

## 4. Settings scopes

**The four files plus managed settings (documented).**

| Scope | File | Applies to |
| - | - | - |
| User | `~/.claude/settings.json` | You, in every project on this machine |
| Shared project | `.claude/settings.json` | Everyone working in the folder |
| Project local | `.claude/settings.local.json` | You, in this project only |
| Managed | `managed-settings.json` and other managed sources | Everyone the organization deploys it to |

`~/.claude.json` is a fifth file that Claude Code writes for itself. Source: https://code.claude.com/docs/en/settings#settings-files-and-who-they-affect

**File-based managed settings are system-wide (documented).** On macOS they live at `/Library/Application Support/ClaudeCode/managed-settings.json`, alongside `managed-settings.d/` and `managed-mcp.json`. Source: https://code.claude.com/docs/en/managed-settings

**Which settings depend on the account.**
- Server-managed settings (documented) reach machines "that you sign in to with your organization account". They are cached in `remote-settings.json`, which Claude Code "deletes ... when you log out". `policy-limits.json` carries a sidecar that "records which account or API key the cache belongs to". Sources: https://code.claude.com/docs/en/settings, https://code.claude.com/docs/en/claude-directory
- User settings and `~/.claude.json` (inference from the docs) depend on the config directory, not the account. They move with `CLAUDE_CONFIG_DIR` and are shared by every account that uses that directory.
- Project files and system managed files (inference from the docs) do not depend on the account at all.

## 5. Plugins and marketplaces

**Install location (documented).** `~/.claude/plugins` holds cloned marketplaces, installed versions, `installed_plugins.json`, `known_marketplaces.json` and per-plugin data. Plugins synced from claude.ai go to `~/.claude/plugins/synced/`. Sources: https://code.claude.com/docs/en/claude-directory, https://code.claude.com/docs/en/plugins/loading#find-plugins-on-disk

**Install scopes (documented).**
- User scope writes `enabledPlugins` in `~/.claude/settings.json`.
- Project scope writes `.claude/settings.json`.
- Local scope writes `.claude/settings.local.json`.
- Precedence is local over project over user.
- "The terminal, the desktop app's local sessions, and the VS Code extension on one computer read the same settings files".

Source: https://code.claude.com/docs/en/plugins/install#choose-an-install-scope

**`enabledPlugins` (documented).** A map keyed by `plugin-name@marketplace-name`, valid in any settings file. Source: https://code.claude.com/docs/en/settings-reference#enabledplugins

**Plugins from claude.ai and the organization (documented).**
- Plugins you or your organization turn on for your claude.ai account load as `<name>@synced`.
- They sync in the background at each start, only in "terminal sessions where you sign in with your claude.ai account".
- They do not sync with `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN` or `apiKeyHelper`.
- Organization-required plugins cannot be disabled.
- Turn sync off with `syncClaudeAiPlugins: false`.
- Locally installed plugins "aren't added to your claude.ai account".

Sources: https://code.claude.com/docs/en/plugins/loading#plugins-synced-from-claude-ai, https://code.claude.com/docs/en/plugins/install#plugins-from-your-claude-ai-account

**Cloud sessions and plugins (documented).** Cloud sessions do not load locally installed plugins. Source: https://code.claude.com/docs/en/plugins/install

**What happens to synced plugins on an account switch (UNVERIFIED).** `synced/` is a single directory per config directory. The docs don't say what happens to it when you log into a different account in the same directory. Inference: the next startup sync removes plugins "that you or your organization turned off". The docs do not confirm that this applies on an account change.

## 6. Skills

**Locations (documented).**

| Location | Path | Where it loads |
| - | - | - |
| Enterprise | `.claude/skills/` in the managed settings directory | Machines where the organization deploys it |
| Personal | `~/.claude/skills/<name>/SKILL.md` | "All your projects on this machine", but not Cowork or cloud sessions |
| Project | `.claude/skills/` | Sessions in that repository |
| Plugin | `<plugin>/skills/` | Wherever the plugin is enabled, invoked as `/plugin-name:skill-name` |
| claude.ai account | Skills enabled for the account | Cowork, cloud, and "terminal sessions where you sign in with that account" |

Source: https://code.claude.com/docs/en/skills

**Synced skills (documented).**
- In the terminal they download into `~/.claude/skills/synced/`.
- Claude Code re-checks about every 10 minutes while active and every 40 minutes while idle.
- They are download-only.
- "Skills that an earlier session synced stay on disk. Claude Code loads them in later sessions signed in to the same account".
- Turn sync off with `syncClaudeAiSkills: false`.
- Removed skills go to `~/.claude/skills/.trash/`.
- The full name is `/anthropic-skills:<name>`.

Source: https://code.claude.com/docs/en/skills#skills-synced-from-claude-ai

**Desktop (documented).** Local sessions load `~/.claude/skills/` and the claude.ai account's skills. Cloud sessions load the account's skills instead of `~/.claude/skills/`. Source: https://code.claude.com/docs/en/desktop#use-skills

**Cowork (documented).** Cowork takes skills, plugins and connectors from the Customize configuration, "which syncs through your claude.ai account, not from the CLI's `~/.claude` directory". Source: https://code.claude.com/docs/en/desktop#extend-claude-code

## 7. Connectors and MCP

**Claude Code MCP scopes (documented).**

| Scope | Stored in | Loads in |
| - | - | - |
| Local (default) | `~/.claude.json` under the project path | That project only |
| Project | `.mcp.json` | That project, shared through version control |
| User | `~/.claude.json` | All projects |

Precedence is local, then project, then user, then plugin servers, then claude.ai connectors. Source: https://code.claude.com/docs/en/mcp#mcp-installation-scopes

**claude.ai connectors in Claude Code (documented).**
- "If you've logged into Claude Code with a claude.ai account, MCP servers you've added in claude.ai ... are automatically available".
- Connectors are managed at claude.ai/customize/connectors. On Team and Enterprise plans, only admins can add them.
- Terminal, VS Code, JetBrains and SDK sessions fetch connectors from claude.ai.
- Cloud sessions receive them from the host.
- Desktop local and SSH sessions get them in-process as `type: "sdk"` servers, which "no MCP setting or `managed-mcp.json` reaches".

Sources: https://code.claude.com/docs/en/mcp#use-mcp-servers-from-claude-ai, https://code.claude.com/docs/en/mcp#how-connectors-reach-claude-code

**Connectors are per account and per organization (documented, help center).**
- On Team and Enterprise plans, owners add connectors, and "users individually connect to and enable that connector".
- Remote connectors connect "from Anthropic's cloud infrastructure".
- They work across "claude.ai, Claude Desktop, Cowork, and the mobile apps". The article doesn't mention Claude Code; the Claude Code docs above cover it.

Source: https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp

**`claude_desktop_config.json` (documented).**
- Desktop loads its servers into local Code-tab sessions. On a name clash the Desktop definition wins.
- "The standalone CLI does not read `claude_desktop_config.json`". You can import its servers with `claude mcp add-from-claude-desktop` (macOS/WSL).

Sources: https://code.claude.com/docs/en/desktop#mcp-servers-from-the-claude-desktop-chat-app, https://code.claude.com/docs/en/mcp#import-mcp-servers-from-claude-desktop

**Location of `claude_desktop_config.json` (UNVERIFIED in the required sources).** It is commonly given as `~/Library/Application Support/Claude/claude_desktop_config.json`. That path appears on modelcontextprotocol.io, which is outside this source set.

**Desktop extensions (documented, help center).**
- They are "single-click installable packages" for local MCP servers.
- Install from Settings, or install a `.mcpb` file through Advanced settings > Extension Developer > "Install Extension…".
- Sensitive values go into the OS secure storage (Keychain).
- Team and Enterprise owners manage access and the allowlist.
- The article says nothing about `.dxt`, about on-disk location, or about whether extensions are tied to an account. Those points are UNVERIFIED.

Source: https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop

## 8. Official stance and feature requests

**Stance.** The docs present `CLAUDE_CONFIG_DIR` per account as the supported method (https://code.claude.com/docs/en/authentication#log-in-with-multiple-accounts). On 2026-08-17 a maintainer closed #20131 with: "You can already keep separate accounts side by side today by giving each one its own config directory". The comment carried a "Generated with Claude Code" footer.

Issue status as of 2026-10-24:

| Issue | Title (short) | Status |
| - | - | - |
| [#20131](https://github.com/anthropics/claude-code/issues/20131) | Multi-account profile support | Closed (completed; pointed to `CLAUDE_CONFIG_DIR`) |
| [#30031](https://github.com/anthropics/claude-code/issues/30031) | Multiple account login and switching, like `gh auth switch` | **Open** |
| [#18435](https://github.com/anthropics/claude-code/issues/18435) | Multiple accounts in the Claude Desktop app | **Open** |
| [#30565](https://github.com/anthropics/claude-code/issues/30565) | Desktop multi-account switching | Closed (duplicate of #18435) |
| [#55621](https://github.com/anthropics/claude-code/issues/55621) | Multi-account in the VS Code extension | **Open** |
| [#36151](https://github.com/anthropics/claude-code/issues/36151) | Mobile multi-account without a shared email | Open (labeled invalid) |
| [#20549](https://github.com/anthropics/claude-code/issues/20549), [#41048](https://github.com/anthropics/claude-code/issues/41048), [#34341](https://github.com/anthropics/claude-code/issues/34341) | Profiles, switching, rate-limit failover | Closed (duplicates) |
| [#69906](https://github.com/anthropics/claude-code/issues/69906), [#56232](https://github.com/anthropics/claude-code/issues/56232), [#64376](https://github.com/anthropics/claude-code/issues/64376) | Email-based switching, per-project accounts, multiple subscriptions | Closed (not planned; stale) |
| [#20553](https://github.com/anthropics/claude-code/issues/20553) | OAuth credentials shared across `CLAUDE_CONFIG_DIR` profiles | **Open** (bug) |
| [#70697](https://github.com/anthropics/claude-code/issues/70697) | `CLAUDE_CONFIG_DIR` doesn't isolate the macOS Keychain | Closed (duplicate of #20553) |
| [#101990](https://github.com/anthropics/claude-code/issues/101990) | Desktop's bundled Claude Code overwrites the CLI Keychain item | **Open** (bug) |

**VS Code (documented in the changelog).** Several releases fixed `CLAUDE_CONFIG_DIR` handling in the extension's `claudeCode.environmentVariables` setting, including the session list and sign-in state after a change. Source: https://code.claude.com/docs/en/changelog

## Sources

Claude Code docs:
- https://code.claude.com/docs/en/authentication
- https://code.claude.com/docs/en/env-vars
- https://code.claude.com/docs/en/claude-directory
- https://code.claude.com/docs/en/settings
- https://code.claude.com/docs/en/settings-reference
- https://code.claude.com/docs/en/managed-settings
- https://code.claude.com/docs/en/sessions
- https://code.claude.com/docs/en/desktop
- https://code.claude.com/docs/en/claude-code-on-the-web
- https://code.claude.com/docs/en/mcp
- https://code.claude.com/docs/en/skills
- https://code.claude.com/docs/en/plugins/install
- https://code.claude.com/docs/en/plugins/loading
- https://code.claude.com/docs/en/commands
- https://code.claude.com/docs/en/cli-reference
- https://code.claude.com/docs/en/troubleshoot-install
- https://code.claude.com/docs/en/changelog (generated from https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)

Help center:
- https://support.claude.com/en/articles/13189465-log-in-to-your-claude-account
- https://support.claude.com/en/articles/9267400-move-your-personal-claude-account-to-a-team-or-enterprise-organization
- https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp
- https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop

GitHub issues:
- #18435, #20131, #20549, #20553, #30031, #30565, #34341, #36151, #41048, #55621, #56232, #64376, #69906, #70697, #91017, #92825, #100364, #101545, #101990, #104155
- All at https://github.com/anthropics/claude-code/issues/<number>
