# Can `CLAUDE_SECURESTORAGE_CONFIG_DIR` give each Profile its own login inside one shared config?

Answers [#15](https://github.com/Jbmanllr/claude-account-manager/issues/15) for the map in [#1](https://github.com/Jbmanllr/claude-account-manager/issues/1). Researched 2026-10-24 on macOS (Apple silicon). Builds on [#5 cli-credentials](https://github.com/Jbmanllr/claude-account-manager/blob/research/cli-credentials/research/cli-credentials/README.md) and [#6 cli-switching](https://github.com/Jbmanllr/claude-account-manager/blob/research/cli-switching/research/cli-switching/README.md), which named this variable as option E / H and asked for this check.

**Method.** I read the JavaScript bundled in these Claude Code builds by searching their bytes: the installed CLI 2.1.296 (`~/.local/share/claude/versions/2.1.296`, plus 2.1.250 and 2.1.295 kept beside it), the VS Code extension's 2.1.181 binary, and the desktop app's bundled 2.1.304 and 2.1.308. To date the variable I fetched older releases from the npm registry into a scratch folder and searched them without running them: `@anthropic-ai/claude-code` 2.0.0, 2.1.56 and 2.1.112 (`cli.js`), and `@anthropic-ai/claude-code-darwin-arm64` 2.1.143 and 2.1.144 (native binary). I did not run Claude Code. A live check of identity display would need a signed-in second store, and any login, refresh or 401 handling could write a Keychain item. Code reading plus third-party live reports below was enough. No Keychain item, `~/.claude` file or secret was read or changed.

**Labels.** **Code** = read in the bundled JavaScript of the version named. Names are minified, so I quote string literals and give the 2.1.296 minified name in brackets. **Docs** = code.claude.com, fetched 2026-10-24. **Issue report** = a user report in `anthropics/claude-code`; primary evidence of behavior, not confirmed by Anthropic. **Inference** = my reasoning from the above.

## Answer

1. **Yes, it keeps everything but credentials shared.** The variable changes only the credential store: the Keychain item name, the `.credentials.json` fallback, and the lock files next to it. `.claude.json`, `settings.json`, plugins, `projects/`, `sessions/` and every other config path still come from `CLAUDE_CONFIG_DIR` or the default. **Code**
2. **The shared `oauthAccount` then mislabels Profiles, and the label is not cosmetic.** `oauthAccount` is one unkeyed block in the shared `.claude.json`. It is written by whichever Profile last logged in, by a profile refresh that runs at most once every 24 h, and, for the organization fields only, by every startup's bootstrap when the account matches. Claude Code then uses this block, not the credential, for several things:
   - the `email`, `orgId` and `orgName` that `claude auth status --json` prints;
   - the `x-organization-uuid` header on cloud-session, remote-environment, teleport and GitHub-sync calls;
   - the account key of the cached usage figures;
   - the folder that synced claude.ai skills are written to.

   The model requests, the billing and `subscriptionType` follow the credential. **Code + issue reports** (#79222, #84915, #98526, #87702)
3. **Stability: undocumented, but stable so far.** The variable first appears in 2.1.144 (published 2026-05-18). Its two functions have the same logic in every build checked from 2.1.144 to 2.1.308. It is absent from the docs and the changelog, and a request to document it (#79223) has had no maintainer reply since 2026-07-19. **Code + Docs**
4. **Recommendation: keep the per-Profile `CLAUDE_CONFIG_DIR` design (B in #6). Do not use this variable for switching.** It would break the identity source #5 picked (`claude auth status --json`). It would make one Profile's `/login` rewrite identity and clear account caches for every other Profile. Running Sessions would pick that up (§3.3). It would also add unkeyed shared caches that are already reported to corrupt each other (§3.4). Its one gain is a single `.claude.json` and plugin index, and B gets most of that with links. One side effect for B: the PATH shim must **unset** `CLAUDE_SECURESTORAGE_CONFIG_DIR`. An inherited value outranks `CLAUDE_CONFIG_DIR` for the credential store, and an *empty* value pins the bare default item. **Inference**

## 1. Every read of the variable (Code, 2.1.296)

Three property reads in the bundle; 13 occurrences of the name in all (others are allow-lists and strings).

| Where | What it does |
|---|---|
| Secure-storage directory [`Lw`] | `CLAUDE_SECURESTORAGE_CONFIG_DIR` if defined (empty → `~/.claude`), NFC-normalized; else the config dir [`be`] = `CLAUDE_CONFIG_DIR ?? ~/.claude`. |
| Keychain service name [`j$`] | `"Claude Code" + OAUTH_FILE_SUFFIX + kind + suffix`. Suffix is empty when the variable is defined and empty, or when it is undefined and `CLAUDE_CONFIG_DIR` is unset; otherwise `-` + first 8 hex of `sha256(value)`. Precedence over `CLAUDE_CONFIG_DIR` is total: if the variable is defined, `CLAUDE_CONFIG_DIR` does not affect the name. |
| Teammate spawn command [`W`] | Passes the parent's value on (`CLAUDE_SECURESTORAGE_CONFIG_DIR=<value>`) whenever it is defined, so agent-team teammates share the parent's store. |

The variable is also on three allow-lists:

- Env vars kept for background sessions [`q6e`]. The saved env in a job's `state.json` is checked before reuse: an empty value is allowed, and a path must be absolute and outside `jobs/`, `projects/`, `plans/`, `agent-memory/` and Claude's temp folder [`Wrs`, `T`].
- When that env set is captured [`NNt`], it is the only variable kept even when empty.
- A list of env vars that point at config locations.

### What `Lw()` and `j$()` move

| Item | Path or name | Code |
|---|---|---|
| claude.ai login, MCP OAuth logins, design OAuth, trusted-device token, enterprise gateway, plugin sensitive values | Keychain `Claude Code-credentials[-hash]` (the logout code deletes `claudeAiOauth`, `organizationUuid`, `trustedDeviceToken`, `enterpriseGateway` and `designOauth` from this blob) | `j$(Lbe)`, logout `mutate` |
| Console API key (`/login` managed key) | Keychain `Claude Code[-hash]` | `j$()` with no kind |
| Plaintext fallback, and the mtime watch that triggers credential reloads | `<dir>/.credentials.json` | `u()`, `f()`, `jk` |
| Write lock | `<dir>/.storage-write` | `Gwo` |
| Refresh locks | `<dir>/.oauth_refresh.lock`, `<dir>/.design_oauth_refresh.lock`, `<dir>/mcp-refresh-<server>.lock`, `<dir>/.gcp_auth_refresh-*`, `<dir>/.aws_auth_refresh-*` | `jyo`, `C`, `refreshAuthorization`, `Zf` |
| Protected-path list (the sandbox and permission checks treat these as sensitive) | the dir is added beside `CLAUDE_CONFIG_DIR` and `~/.claude` | `xL`, `za`, `Uqe` |

### What it does not move

`.claude.json` stays `$CLAUDE_CONFIG_DIR/.claude.json` or `~/.claude.json`, with `.config.json` as a legacy override [`txr`, `Tt`]. `claude auth status --json` reports `configDirectory: be()`, the config dir, not the store. `mcp-needs-auth-cache.json` is `join(be(), …)` [`x2e`]. Synced skills, `projects/`, `plans/`, `jobs/` and plugins all hang off `be()`.

So under this design, two Profiles differ only in their Keychain items and lock files. **Code**

## 2. `oauthAccount`: one block, last writer wins (Code, 2.1.296)

`oauthAccount` is a single top-level key of the global config, read through `ce().oauthAccount` (accessors `Hn`, `qf`). It is not keyed by credential item, account or organization. It is written in five places:

| Writer | When | What it writes |
|---|---|---|
| Login (`installOAuthTokens`) | `claude auth login` or `/login` | First runs the logout-style cleanup with preserve flags. That clears `oauthAccount` and the account caches listed in §3. Then it writes the new account's full block (`storeOAuthAccountInfo` [`NQt`]). |
| Profile refresh [`Iho`] | At startup, only when a required field is missing or `profileFetchedAt` is older than 24 h (`SG=86400000`) | The full block from `/api/oauth/profile` for the token **in use**, overwriting identity |
| Token refresh [`RSe`] | Only when cached billing fields are missing | Display and billing fields merged onto the **existing** identity. It does not write `accountUuid`, `emailAddress` or `organizationUuid`, and it stamps a fresh `profileFetchedAt`. |
| Bootstrap [`uDs` → `q`] | Each startup (`/api/claude_cli/bootstrap`) | If the bootstrap `account_uuid` differs from the cached one: **nothing**. If it matches, it overwrites `emailAddress`, `organizationUuid`, `organizationName`, `organizationType` and the rate-limit and seat tiers with the token's values. |
| Roles fetch, trial and extra-usage updates | Various | Single fields merged in place |

Two guards keep a stale block in place:

- If a write would drop `oauthAccount` while the in-memory copy has it, Claude Code treats that as auth loss. It repairs or refuses the write (`Yt`, `tengu_config_auth_loss_prevented`, `tengu_config_auto_repaired`, "See GH #3117").
- The global-config store has a freshness watcher (`freshnessWatcherStarted`, `pendingExternalRefresh`), so running processes pick up another process's write.

**Consequences for two Profiles A and B sharing one `.claude.json`** (Inference from the table):

- **Different Logins.** After B's login, every A Session shows B's identity until a 24 h profile refresh in some A process rewrites it to A. Then B is mislabeled. The flip follows whichever Profile happens to start after the TTL.
- **Same Login, different Organizations** (two Profiles in the map's sense). Each startup's bootstrap rewrites the organization fields to the starting Profile's organization. Running Sessions of the other Profile then read the wrong `organizationUuid`.
- **Stripping the block (clauth's approach) does not stick.** The auth-loss guard restores it from memory, and the next startup refetches it (field missing → profile fetch).

## 3. What reads the cached identity, and what else in the shared files is account-scoped

### 3.1 Readers of `oauthAccount` (Code, 2.1.296)

| Reader | Uses | Effect when the block names the other Profile |
|---|---|---|
| `claude auth status --json` [`me`] | `email`, `orgId` and `orgName` come from `oauthAccount`; `subscriptionType` comes from the credential [`Sr`] | Reports the wrong Profile with the right plan. This is the identity source #5 recommended. |
| `/status`, startup banner | Same accessor (#79222, re-checked on 2.1.268 in a comment there) | Wrong email and organization |
| Organization for API calls [`S$`] | `CLAUDE_CODE_ORGANIZATION_UUID`, else `oauthAccount.organizationUuid`, else a profile fetch | Sent as `x-organization-uuid` with the other Profile's token: cloud sessions, remote environments, self-hosted runner pools, teleport resume, GitHub token sync |
| Usage cache [`ySs`, `SSr`] | `cachedUsageUtilization` is keyed by `Hn().accountUuid`, the cached identity | Usage figures stored under the wrong account |
| Synced skills bucket [`ARt`] | `~/.claude/skills/synced/<org>_<account>` from `oauthAccount` | All Profiles share one bucket and churn it (#98526) |
| Analytics / feature-flag identity, Chrome bridge user id, startup prefetch | `oauthAccount.accountUuid` / `organizationUuid` | Wrong account attributes. The Chrome bridge warns: "The OAuth token in use resolves to a different claude.ai account than the persisted Claude Code login." |

### 3.2 Other account-scoped keys in the shared `.claude.json`

Taken from the list that login and logout clear [`performLogout`]:

- `additionalModelOptionsCache` / `…AnsweredAt`, `additionalModelCostsCache`, `modelAccessCache`, `orgModelDefaultCache`, `lastSeenOrgDefaultUpdatedAt`, `autoCompactWindowsCache`
- `clientDataCache`, `clientDataCacheSlots` (slots are keyed by organization)
- `cachedUsageUtilization`, `metricsStatusCache`, `metricsStatusCacheByPrincipal`
- `cachedArtifactRoster`, `artifactRosterDenied`, `githubWebConnectionStatusCache`, `startupPrefetchedAt`

Also global and unkeyed: `cachedGrowthBookFeatures` and the other feature caches, and the onboarding and subscription notice counters. Bootstrap rewrites the model caches at startup whenever they differ, so the last Profile to start sets the model list that a running Session of another Profile reads after the freshness watcher reloads. **Code**

### 3.3 Login and logout reach across Profiles

A `/login` or `claude auth login` under one store first runs the logout cleanup on the shared file. It also emits account-change events. A `claude auth logout` under one store wipes `oauthAccount` and all the caches above for everyone. The other Profiles' credentials are not touched, so their requests still use their own login. **Code**

### 3.4 Shared files outside `.claude.json`

- `~/.claude/mcp-needs-auth-cache.json` is keyed by server name only. One Profile's MCP auth failure makes the other skip that server for the 15-minute TTL (#87702, 2.1.234, closed by the stale bot unfixed; path confirmed in 2.1.296 code).
- `~/.claude/skills/synced/<org>_<account>/` is named from the cached identity. Sessions on two accounts remove each other's skills every sync and fill `~/.claude/skills/.trash` (#98526, open, 2.1.285).

### 3.5 Not split, in both designs

MCP OAuth logins live in the credential blob, so they are per Profile under this variable too. #6 listed that split as a cost of B; it is not a difference between the designs. **Code**

## 4. History and stability

| Build | Occurrences | Logic of `Lw` / `j$` |
|---|---|---|
| 2.0.0, 2.1.56, 2.1.112 (npm `cli.js`) | 0 | Service suffix from `CLAUDE_CONFIG_DIR` only |
| 2.1.143 (darwin-arm64, 2026-05-15) | 0 | Same |
| **2.1.144** (darwin-arm64, 2026-05-18) | 8 | Variable introduced, logic as today |
| 2.1.181 (VS Code), 2.1.250, 2.1.295, 2.1.296 (CLI), 2.1.304, 2.1.308 (desktop bundle) | 10 → 14 | Unchanged; the extra occurrences are new allow-lists (background-session env, teammates) |

- 2.1.144's changelog entry includes background-session and agent-view changes, but does not mention the variable. Neither does any other changelog entry (CHANGELOG.md through 2.1.310). #79223 dates it to "roughly v2.1.144 (per community release-diffing)", which matches.
- Docs: none of `env-vars`, `authentication` or `claude-directory` mentions it. The authentication page says only that `CLAUDE_CONFIG_DIR` "keys the macOS Keychain entry to that directory".
- Signals of intent (Inference). Its internal uses are teammates and background sessions inheriting the parent's store, and the store path is validated before reuse. It is maintained infrastructure, not a leftover, but nothing promises its semantics to users. A fix for #79222 (identity from the credential) would change the trade-off. Nothing suggests one is planned: #79222 and #84915 were closed by the stale bot.
- Third-party reliance. #79223 lists tools that set or pin it (claudex-switch, agent-fleet, claude-pod, switchroom and others). A live test on 2.1.257 in its comments confirms that requests bill the selected store's account, and that `/status` and `auth status --json` show the shared `oauthAccount` email.

## 5. Against the per-Profile `CLAUDE_CONFIG_DIR` design (B in #6)

| Criterion | B: per-Profile `CLAUDE_CONFIG_DIR` + links | E: `CLAUDE_SECURESTORAGE_CONFIG_DIR` only |
|---|---|---|
| Global for new runs via PATH shim | ✓ | ✓ |
| Running Sessions keep their login | ✓ (env fixed at exec) | ✓ for credentials. **✗ for identity, organization and caches**, which follow the last writer through the freshness watcher. |
| Transcripts shared | ✓ with `projects/` linked | ✓ natively |
| `.claude.json` (user MCP servers, project trust, global keys) | Split, needs seeding or sync | Shared |
| Plugin indexes | Split, absolute paths need rewriting | Shared |
| MCP OAuth logins | Per Profile | Per Profile |
| Identity per Profile (`auth status --json`) | Correct | **Wrong** for all but the last writer |
| Org-scoped calls (`x-organization-uuid`) | Correct | Can name the wrong organization |
| Known cross-Profile bugs | #88855 (Remote Control drop; reported for config dirs) | #79222, #84915, #98526, #87702 |
| Documented | Yes (`CLAUDE_CONFIG_DIR`, multiple-accounts section) | No |

**Recommendation.** Keep B. Each Profile keeps its own `.claude.json`, so `oauthAccount`, the org-scoped caches and the usage cache stay correct. The menu bar app can trust `claude auth status --json` per Profile.

What E would save, B can approximate:

- Link `projects/`, `settings.json`, skills, agents, commands and the other files #6 lists.
- Seed each `.claude.json` without `oauthAccount`.
- Optionally mirror `mcpServers` and project trust.

Never link `.claude.json` itself. That brings back E's shared identity.

**Two follow-ups for B** (Inference from §1):

1. The shim must `unset CLAUDE_SECURESTORAGE_CONFIG_DIR` before `exec`, rather than set it to empty. A value inherited from another tool or a teammate environment would otherwise pick a different Keychain item from the Profile's `CLAUDE_CONFIG_DIR`. An empty value would pin the bare `Claude Code-credentials` item that Desktop's bundled engine also writes (#101990). #79223 also reports a `/login` under a stale value overwriting the other account's stored login.
2. Drift detection should watch for this variable in the user's environment and in Claude's saved background-job env (`jobs/*/state.json`). A background session resumed with a saved value uses that store, not the Profile's.

## Sources

**Claude Code code** (strings searched in the binaries; minified names from 2.1.296):
- `~/.local/share/claude/versions/2.1.296`: `Lw`, `j$`, `RR`, keychain backend `O` (`find-generic-password`, `add-generic-password -U`, `delete-generic-password`), `Gwo` (`.storage-write`), plaintext store `u`/`f`, `jk` (credential mtime watch), `jyo` (`.oauth_refresh.lock`), `Zf`/`oK` (GCP/AWS refresh locks), `W` (teammate env), `q6e`/`Wrs`/`T` (background-job env), `NNt`, `xL`/`za`/`Uqe` (protected paths), `txr`/`Tt`/`jl` (global config path), `x2e` (`mcp-needs-auth-cache.json`), `Hn`/`qf`/`ce().oauthAccount`, `Iho` + `NQt` (`storeOAuthAccountInfo`, 24 h `SG`), `RSe` (refresh merge), bootstrap `uDs`/`q`/`nt`, `installOAuthTokens` (login), `performLogout` clear list, `Yt` (auth-loss guard), config store class (`freshnessWatcherStarted`), `me` (`auth status`), `Sr`, `S$`, `ySs`/`SSr` (`cachedUsageUtilization`), `ARt`/`syr` (synced skills bucket).
- Same functions checked in `~/.local/share/claude/versions/2.1.250` and `2.1.295`, `~/.vscode/extensions/anthropic.claude-code-2.1.181-darwin-arm64/resources/native-binary/claude`, and `~/Library/Application Support/Claude/claude-code/2.1.304/…` and `2.1.308/…`.
- npm registry tarballs (not executed): `@anthropic-ai/claude-code@2.0.0`, `@2.1.56`, `@2.1.112`; `@anthropic-ai/claude-code-darwin-arm64@2.1.143`, `@2.1.144`. Release dates from `npm view @anthropic-ai/claude-code time`.

**Docs** (fetched 2026-10-24): [env-vars](https://code.claude.com/docs/en/env-vars), [authentication: credential management](https://code.claude.com/docs/en/authentication#credential-management), [claude-directory](https://code.claude.com/docs/en/claude-directory); [CHANGELOG.md](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md) (2.1.144 entry; no mention of the variable through 2.1.310).

**Issues** (`anthropics/claude-code`, state on 2026-10-24):
- [#79223](https://github.com/anthropics/claude-code/issues/79223) docs request, open; comments include a live test on 2.1.257
- [#79222](https://github.com/anthropics/claude-code/issues/79222) `/status` shows cached identity, closed by stale bot; root cause and repro on 2.1.215, static re-check on 2.1.268
- [#84915](https://github.com/anthropics/claude-code/issues/84915) same symptom, 2.1.224, closed by stale bot
- [#98526](https://github.com/anthropics/claude-code/issues/98526) synced skills churn across stores, open
- [#87702](https://github.com/anthropics/claude-code/issues/87702) shared `mcp-needs-auth-cache.json`, closed by stale bot
- [#88855](https://github.com/anthropics/claude-code/issues/88855) Remote Control drop across config dirs, closed by stale bot
- [#101990](https://github.com/anthropics/claude-code/issues/101990) Desktop engine overwrites the bare item, open
- [#104767](https://github.com/anthropics/claude-code/issues/104767) request to decouple config from credential slot (the opposite direction), open

**Project:** [#5](https://github.com/Jbmanllr/claude-account-manager/issues/5), [#6](https://github.com/Jbmanllr/claude-account-manager/issues/6).
