# How Claude Code stores and refreshes OAuth credentials on macOS

Research for [#5](https://github.com/Jbmanllr/claude-account-manager/issues/5), part of the map [#1](https://github.com/Jbmanllr/claude-account-manager/issues/1). Checked 2026-10-24 on macOS (Darwin 27, Apple silicon) with:

- Claude Code CLI 2.1.296, native install (`~/.local/bin/claude` → `~/.local/share/claude/versions/2.1.296`)
- Claude desktop app 3.6556.2, with bundled Claude Code 2.1.308 (`~/Library/Application Support/Claude/claude-code/2.1.308/<hash>/claude.app`)

**Method.** I read the JavaScript bundled inside both Claude Code binaries and the desktop app's `app.asar` as plain text, by searching their bytes for identifiers. I listed Keychain attributes and ACLs with `security dump-keychain` and `security dump-keychain -a`, which print no secret data. I never used `-w` or `-g`. From `~/.claude.json` I printed key names only. I did not log in, log out, refresh or run any Claude command. No secret value was printed, copied or saved.

**Labels.**

- **Code**: read in the bundled JavaScript of the version named. The names are minified, so I quote string literals and describe the logic.
- **Observed**: seen on this Mac.
- **Documented**: stated on code.claude.com.
- **Issue report**: stated by a user in `anthropics/claude-code`. It is primary evidence of behavior, but Anthropic has not confirmed it.
- **Inference**: my reasoning from the above.
- **UNVERIFIED**: no primary source found.

## Answer

1. **Keychain item.** Claude Code stores the login in one generic password per credential slot. The service is `Claude Code-credentials`, plus `-<first 8 hex chars of sha256(config dir)>` when `CLAUDE_CONFIG_DIR` is set. The account is `$USER`. The hash input is the literal `CLAUDE_CONFIG_DIR` string after NFC normalization: no tilde expansion, no realpath, trailing slashes kept. An undocumented `CLAUDE_SECURESTORAGE_CONFIG_DIR` overrides which slot is used. **Code**
2. **What the item holds.** A JSON object. Its `claudeAiOauth` holds `accessToken`, `refreshToken`, `expiresAt`, `refreshTokenExpiresAt`, `scopes`, `subscriptionType`, `rateLimitTier` and `clientId`. The same blob also holds MCP OAuth logins (`mcpOAuth`) and other state. Identity (account, organization, plan details) is not in the item; it lives in `oauthAccount` in `.claude.json`. **Code**
3. **Refresh tokens rotate, and only one holder survives.** Claude Code refreshes 5 minutes before expiry, at `https://platform.claude.com/v1/oauth/token`, with the public client `9d1c250a-e61b-44d9-88ed-5944d1962f5e`. If a refresh fails with `invalid_grant`, Claude Code blanks the stored tokens, and the user sees `Login expired` / `Not logged in`. Issue reports agree that a refresh makes the previous refresh token unusable. So **any second holder that refreshes a copy of the CLI's refresh token logs the CLI out** (#80585, #103325, #94464). **Code + issue reports**
4. **The desktop app passes its own token** to its bundled Claude Code through `CLAUDE_CODE_OAUTH_TOKEN`. But the bundled engine uses the same `~/.claude` and the same Keychain slot as the terminal CLI. Two open bugs show it rewriting the CLI's item (#101990) or rotating the CLI's refresh token without persisting the result (#94464). Desktop 3.6556.2 itself only reads the item, through `security find-generic-password -w`. **Code + issue reports**
5. **What fits the safety bar.** Only one option breaks neither login and stays inside the terms research ([#3](https://github.com/Jbmanllr/claude-account-manager/issues/3)): **one `CLAUDE_CONFIG_DIR` per Profile, signed in with Claude Code's own `claude auth login`, and Claude Code as the only refresher.** The app should never refresh or copy a refresh token. Every way of holding a *live, independently refreshed* credential per Profile either forks the CLI's token family (and logs it out) or mints tokens under Claude Code's client id (forbidden by #3). Read-only use of each slot's access token is technically safe for the CLI login, but #3 rates it grey, leaning forbidden. See [section 7](#7-options-for-a-separate-app).

---

## 1. Keychain item names

### Service and account (Code, 2.1.296 and 2.1.308)

The service name is built as follows (minified function `j$` in 2.1.296, `QB` in 2.1.308; same logic):

```
service = "Claude Code" + OAUTH_FILE_SUFFIX + "-credentials" + slotSuffix
slotSuffix =
  ""                                         if CLAUDE_SECURESTORAGE_CONFIG_DIR is unset and CLAUDE_CONFIG_DIR is unset/empty
  "-" + sha256(dir).hex[0:8]                 otherwise, where
      dir = CLAUDE_SECURESTORAGE_CONFIG_DIR.normalize("NFC")   if that is set (an empty value means "no suffix")
          = (CLAUDE_CONFIG_DIR ?? ~/.claude).normalize("NFC")  otherwise
account = $USER, else os.userInfo().username; "claude-code-user" if it has characters outside [A-Za-z0-9._-]
```

- `OAUTH_FILE_SUFFIX` is `""` in production. It is `-local-oauth` for local development, and `-custom-oauth` when `CLAUDE_CODE_CUSTOM_OAUTH_URL` is set (only approved FedStart and staging hosts are allowed).
- Example: `CLAUDE_CONFIG_DIR=/Users/you/.claude-work` gives `Claude Code-credentials-f7aef36f`, from `printf '%s' /Users/you/.claude-work | shasum -a 256 | cut -c1-8`.
- **Gotchas (Inference from the code).**
  - Setting `CLAUDE_CONFIG_DIR=$HOME/.claude` explicitly selects a *different* slot from leaving it unset, because the unset case gets no suffix.
  - `/x/.claude-work` and `/x/.claude-work/` hash differently.
  - A symlinked path and its target hash differently.
  - The docs warn that `~` is not expanded in settings-file `env` values (https://code.claude.com/docs/en/env-vars). A literal `~/.claude-work` therefore hashes as typed.
- **Documented.** "If you've set the `CLAUDE_CONFIG_DIR` environment variable, Claude Code ... keys the macOS Keychain entry to that directory too, so a session with a different `CLAUDE_CONFIG_DIR` reads a different entry" (https://code.claude.com/docs/en/authentication#credential-management). The docs do not give the item's name or its derivation.
- **Issue report.** #20553 (still open) reported one shared item for all directories in v2.1.19. A comment there confirms per-directory `Claude Code-credentials-<sha256[:8]>` items from v2.1.56 on (https://github.com/anthropics/claude-code/issues/20553#issuecomment-3967317782, 2026-02-26). Inference: the isolation half of #20553 is fixed in current versions, and the issue was simply never closed.
- **`CLAUDE_SECURESTORAGE_CONFIG_DIR` (Code; undocumented).** It is not on the env-vars page. Claude Code sets it for the subprocesses it spawns, such as agent-team teammates, so that they share the parent's slot. It moves all of the following at once: the Keychain slot, the plaintext fallback `<dir>/.credentials.json`, the write lock `<dir>/.storage-write` and the refresh lock `<dir>/.oauth_refresh.lock`. Open request #104767 (2026-10-24) asks for a *supported* way to decouple config location from credential location. That suggests this variable is internal.

### What exists on this Mac (Observed)

| Keychain service | Account | Notes |
|---|---|---|
| `Claude Code-credentials` | the login user (`$USER`) | Created 2026-04-06, modified 2026-10-24T12:59:42Z. No suffixed items exist, because no `CLAUDE_CONFIG_DIR` has been used to sign in. |
| `Claude Safe Storage` | `Claude Key` | The desktop app's Electron safe-storage key. Out of bounds by safety bar (2). |

There is no `~/.claude/.credentials.json`, so the Keychain is the live store.

### Access control (Observed)

The item's ACL allows `decrypt` without a password **only for `/usr/bin/security`**, with partition `apple-tool:`. That is how Claude Code (Code: it shells out to `security find-generic-password -a <user> -w -s <service>`) and the desktop app (Code: `execFile("security", ["find-generic-password", …, "-w", …])`) both read it silently.

Implications (Inference, consistent with the ACL dump quoted in #20553):

- Any process running as the user can read the secret silently by shelling out to `/usr/bin/security`.
- A Rust app that uses Security.framework directly is not on the ACL. It would get a Keychain access prompt, or a denial.
- An app should not rely on the `security` CLI to read Claude Code's secrets, because that is exactly the "intermediating tokens" that #3 flags.

### Read and write mechanics (Code, 2.1.296)

- **Write.** `security -i` with `add-generic-password -U -a <user> -s <service> -X <hex of JSON>`. It falls back to argv when the payload exceeds 4032 bytes. It runs under a process-wide lock file `<storage dir>/.storage-write`.
- **Delete.** `security delete-generic-password -a <user> -s <service>`.
- **Read cache.** A successful read is cached for 30 s, and later reads within that window are served from the cache.
- **Plaintext fallback.** If the Keychain write fails and the failure is not transient, the login goes to `<storage dir>/.credentials.json`, mode `0600`, and Claude Code tries to delete the Keychain item to avoid holding two copies (documented on the authentication page above). Issue #94464 shows this "split brain" happening inside the desktop-spawned engine.

## 2. What the stored JSON holds (field names only)

Top-level keys seen in the code: `claudeAiOauth`, `mcpOAuth` (per-server MCP OAuth logins; also #94464 and #104155), `enterpriseGateway` (gateway logins) and `slackTag`. The docs confirm that `/logout` also removes "saved MCP server logins, and plugin sensitive values" (https://code.claude.com/docs/en/troubleshoot-install). **Writing this item from outside Claude Code would therefore risk MCP logins as well as the claude.ai login.**

`claudeAiOauth` (Code: the save path `Fk(...)` and the refresh result):

| Field | Meaning |
|---|---|
| `accessToken` | Bearer token. Lifetime is about 8 h, per #101990 and #94464 ("8 h access token"). The lifetime comes from the server's `expires_in`. |
| `refreshToken` | Used at the token endpoint. Set to `""` when Claude Code declares it dead. |
| `expiresAt` | Epoch milliseconds: `Date.now() + expires_in*1000`. Set to `0` when dead. |
| `refreshTokenExpiresAt` | From `refresh_token_expires_in` when the server sends it. Kept from the previous value otherwise. |
| `scopes` | Array of strings. |
| `subscriptionType` | `pro`, `max`, `team` or `enterprise`, mapped from the profile's `organization_type` (`claude_pro`, `claude_max`, `claude_team`, `claude_enterprise`). |
| `rateLimitTier` | From the profile's `organization.rate_limit_tier`. |
| `clientId` | Set only for a non-default OAuth client. |

Tokens are saved to storage only when they carry claude.ai scopes **and** a refresh token **and** an expiry. Tokens from `CLAUDE_CODE_OAUTH_TOKEN` or `claude setup-token` are used in memory with `refreshToken: null` and are never written (Code: `tengu_oauth_tokens_inference_only`).

## 3. `oauthAccount` in `.claude.json`

- **Location (Code).** `.claude.json` in `CLAUDE_CONFIG_DIR` when that is set, otherwise in `$HOME`. The docs say the same thing (https://code.claude.com/docs/en/troubleshoot-install).
- **Keys on this Mac (Observed; values not read):** `accountUuid`, `emailAddress`, `organizationUuid`, `organizationName`, `organizationType`, `organizationRole`, `workspaceRole`, `organizationRateLimitTier`, `userRateLimitTier`, `seatTier`, `billingType`, `displayName`, `fullName`, `hasExtraUsageEnabled`, `accountCreatedAt`, `subscriptionCreatedAt`, `claudeCodeTrialEndsAt`, `claudeCodeTrialDurationDays`, `ccOnboardingFlags`, `profileFetchedAt`. No other top-level key looks like an auth key.
- **Updates (Code).** On refresh, Claude Code may fetch the profile and merge display and billing fields into `oauthAccount`. The refresh response can also carry `account.uuid`, `account.email_address` and `organization.uuid`.
- **`oauthAccount` is the Profile key.** The pair `accountUuid` + `organizationUuid` is exactly the map's Profile (Login + Organization). Inference: the credential is bound to one organization. The authorize URL takes `orgUUID`, and the refresh response names one organization.
- **Identity without touching tokens (Code).** `claude auth status --json` prints `loggedIn`, `authMethod`, `apiProvider`, `configDirectory`, `projectsDirectory`, plus `email`, `orgId`, `orgName` and `subscriptionType` for a claude.ai login (documented in https://code.claude.com/docs/en/cli-reference). The handler itself does not call refresh. UNVERIFIED: whether CLI startup can refresh before the handler runs.

## 4. OAuth client, endpoints and refresh

### Constants (Code, 2.1.296 production config)

| Item | Value |
|---|---|
| Client id (public, PKCE, no secret) | `9d1c250a-e61b-44d9-88ed-5944d1962f5e`. The desktop app's production config uses the same id (Code, `app.asar`). It can be overridden with `CLAUDE_CODE_OAUTH_CLIENT_ID`. |
| Authorize (claude.ai login) | `https://claude.com/cai/oauth/authorize` |
| Authorize (Console login) | `https://platform.claude.com/oauth/authorize` |
| Token | `https://platform.claude.com/v1/oauth/token` (revocation: `…/v1/oauth/token/revoke`) |
| Redirect | `http://localhost:<port>/callback`, or `https://platform.claude.com/oauth/code/callback` for manual paste |
| Flow | Authorization code + PKCE `S256`, with `state`. Optional `orgUUID`, `login_hint` and `login_method`. |
| Default claude.ai scopes | `user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload`, plus `user:plugins` (and `org:create_api_key` on the login request) |
| `claude setup-token` | `inferenceOnly`, so the scope is `user:inference` only. `expires_in` is 31,536,000 s (1 year). Not stored. Documented: "It can only make model requests" (https://code.claude.com/docs/en/authentication#generate-a-long-lived-token) |

### When and how Claude Code refreshes (Code, 2.1.296)

1. **When.** A token counts as expiring when `now + 300000 ms >= expiresAt` (5 minutes before expiry), or when a request returns 401.
2. **Lock first.** Claude Code takes `<storage dir>/.oauth_refresh.lock` (stale after 60 s), retries up to 5 times with backoff, then re-reads storage under the lock. If the stored `accessToken` differs from the one it started with, another process has already refreshed, and Claude Code adopts that result instead of refreshing (`tengu_oauth_token_refresh_race_resolved`).
3. **Request.** A JSON `POST` to the token URL with `grant_type: "refresh_token"`, `refresh_token`, `client_id` and `scope`.
4. **Response.** `access_token`, `expires_in`, and optionally `refresh_token`, `refresh_token_expires_in`, `scope`, `account` and `organization`. When the response omits `refresh_token`, the old one is kept (`refresh_token: z = e`).
5. **Failure with `invalid_grant`.** Claude Code re-reads storage once more, in case a sibling rotated the token in the meantime. If nothing changed, it marks the refresh token dead in memory and **rewrites storage with `refreshToken: ""`, `accessToken: ""` and `expiresAt: 0`** (`tengu_oauth_refresh_token_marked_dead_invalid_grant`, `…cleared_on_disk`). Every later start then reports `Login expired · Please run /login` (documented error: https://code.claude.com/docs/en/authentication).

### Do refresh tokens rotate, and what happens to other holders?

- **The server side is not documented.** No code.claude.com page states the token endpoint's rotation policy. UNVERIFIED as an official statement.
- **The client is written for rotation (Code).** It defaults to keeping the old refresh token when the response omits one, it uses a cross-process lock, it adopts a sibling's newer write, and it re-reads before declaring a token dead.
- **Users observe single-use rotation (Issue reports).**
  - #80585 (closed by the stale bot): "Anthropic's refresh grant appears to be single-use (it rotates the refresh token), so the first refresher succeeds and the others fail with an `invalid_grant`-class error", and the failure "clears the keychain credential".
  - #103325 (open, 2.1.296–2.1.303): the same race under one config directory.
  - #94464 (open) is the clearest case. Desktop's embedded engine refreshed and saved the *new* pair to a file. Hours later a process that read the Keychain's *old* refresh token got `authentication_error` at once: "the Keychain refresh token is already dead".
  - Related: #80085, #91009, #88947, #76561, #91571.
- **Conclusion (Inference, strong).** A successful refresh issues a new refresh token and invalidates the one presented. Any other holder of the old refresh token (another config directory sharing a copy, a desktop engine, or a third-party app) gets `invalid_grant` on its next refresh, and if that holder is Claude Code it wipes its stored login. UNVERIFIED: whether the *old access token* stays valid until its `expiresAt`. The reports above only show refresh failures.

## 5. The desktop app's bundled Claude Code

- **Location (Observed).** `~/Library/Application Support/Claude/claude-code/<version>/<hash>/claude.app/Contents/MacOS/claude`. Versions 2.1.304 and 2.1.308 are present.
- **Same storage code (Code, 2.1.308).** The service-name function, the `$USER` account and the `CLAUDE_SECURESTORAGE_CONFIG_DIR` handling are identical to the CLI's. Unless Desktop sets `CLAUDE_CONFIG_DIR` (documented: one value per Desktop launch, https://code.claude.com/docs/en/desktop#environment-variables-in-local-sessions), the engine uses `~/.claude` and the item `Claude Code-credentials`.
- **Desktop's own login travels by environment (Code, `app.asar` 3.6556.2).**
  - Desktop spawns sessions with `CLAUDE_CODE_OAUTH_TOKEN` set to its own token, plus `CLAUDE_CODE_SUBSCRIPTION_TYPE` and `CLAUDE_CODE_RATE_LIMIT_TIER`.
  - It pushes rotated tokens into live sessions with an `update_environment_variables` control message.
  - Its claude.ai login uses the same client id with scopes `user:inference user:file_upload user:profile` (+`user:plugins`). It is held in Desktop's own encrypted storage (see [01](../claude-multi-account/01-installs-and-folders.md)), which safety bar (2) puts out of bounds.
- **Desktop only reads the CLI's item (Code, 3.6556.2).** When it resumes a session into a temporary config directory, it reads `Claude Code-credentials` through `security find-generic-password -w`, deletes `claudeAiOauth.refreshToken` from the copy, and writes the copy to `<tmp>/.credentials.json`. `app.asar` contains no `add-generic-password` call.
- **But the bundled engine still collides (Issue reports).**
  - #101990 (open, 2026-10-14; Desktop 2.31226.1 with engine 2.1.295): about 1–2 s after Desktop auto-spawns a scratch Code session, the item is recreated, with JSON that lacks `claudeAiOauth`. The terminal CLI then shows `Not logged in`. Workaround: open Desktop on the Chat tab.
  - #94464 (open; engine 2.1.270): the engine rotated the CLI's refresh token and could persist the new pair only to `.credentials.json`. The Keychain was left holding a dead refresh token, and the result was "Login expired" daily.
  - Inference: the env token keeps model requests on Desktop's login, but the engine's own storage writes (MCP logins, migration, refresh of a stored login) still land in the CLI's slot.
- **Side note for [#4](https://github.com/Jbmanllr/claude-account-manager/issues/4) (Code).** Desktop reads plan usage by spawning the bundled CLI with its own `CLAUDE_CODE_OAUTH_TOKEN` and `CLAUDE_CODE_TAGS=usage_probe`, then calling an SDK method named `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET()` that returns `day` and `week` buckets.

## 6. Open issues (status 2026-10-24)

| Issue | Topic | Status |
|---|---|---|
| [#20553](https://github.com/anthropics/claude-code/issues/20553) | Credentials shared across `CLAUDE_CONFIG_DIR` profiles | Open. A comment reports it fixed since v2.1.56 (hashed suffix); the code above confirms per-directory slots. |
| [#101990](https://github.com/anthropics/claude-code/issues/101990) | Desktop's bundled engine overwrites the CLI's `Claude Code-credentials` item | Open, no replies |
| [#94464](https://github.com/anthropics/claude-code/issues/94464) | Desktop's engine rotates the CLI's refresh token and persists only to a file | Open (`has repro`, `area:desktop`) |
| [#103325](https://github.com/anthropics/claude-code/issues/103325) | Refresh rotation race in one config dir; the loser gets `invalid_grant` and Claude Code clears the login | Open (re-report of #24317, #43392, #48786, #80585) |
| [#80085](https://github.com/anthropics/claude-code/issues/80085) | Mid-session logouts with concurrent instances on one Keychain item | Open |
| [#104767](https://github.com/anthropics/claude-code/issues/104767) | Request: decouple config location from credential slot | Open |
| [#80585](https://github.com/anthropics/claude-code/issues/80585) | Concurrent sessions race on rotation | Closed by the stale bot, not fixed |
| [#70697](https://github.com/anthropics/claude-code/issues/70697) | `CLAUDE_CONFIG_DIR` does not isolate the Keychain | Closed as a duplicate of #20553 |

## 7. Options for a separate app

The safety bar from #1:

- (1) Tokens only in the Keychain, sent only to Anthropic.
- (2) No browser cookies, no desktop secret storage.
- (3) Backup, not-running check and reversibility before any write to an undocumented Claude file.
- (4) Nothing leaves the machine.

The **terms** column uses #3's verdicts:

- Third-party reuse of Claude Code OAuth tokens is forbidden for a sign-in of the app's own.
- Read-only reuse for metadata is grey, leaning forbidden.
- The undocumented usage endpoint is grey, leaning forbidden.
- The status-line `rate_limits` feed is documented.

| # | Option | Risk to CLI / Desktop login | Safety bar | Terms (#3) | Live limits per Profile? | Verdict |
|---|---|---|---|---|---|---|
| A | **Piggyback, read-only:** read each slot's `accessToken` (default and hashed items) via `/usr/bin/security`; never refresh | **None to the login chain.** Reads add a 30-second cache miss at worst. | (1) ok. Needs `security -w`, a silent read of a secret the app does not own. | Grey, leaning forbidden | Only while Claude Code keeps that slot fresh. Expires about 8 h after the Profile's last use. | Possible fallback, off by default |
| B | **Piggyback + refresh:** refresh the CLI's token and write the new pair back into Claude Code's item | **High.** Races Claude Code's lock-free paths. A crash between refresh and write leaves Claude Code with a dead token, which it then wipes. Clobbers `mcpOAuth` if the blob is rewritten wrongly. | Breaks (3) in spirit: a live write to an undocumented secret store. | Forbidden (intermediating tokens) | Yes | **Reject** |
| C | **Fork the chain:** copy the refresh token into the app's own Keychain item and refresh independently | **Certain.** The first refresh by either side invalidates the other's refresh token (§4), and Claude Code then clears its login. | (1) ok, but this is a copy of a Claude Code secret. | Forbidden | Yes, until the CLI refreshes | **Reject** |
| D | **Per-Profile `CLAUDE_CONFIG_DIR`, Claude Code is the only refresher:** add a Profile with `CLAUDE_CONFIG_DIR=<profile dir> claude auth login` (the browser flow is Anthropic's own). Identity from `claude auth status --json` or `oauthAccount`. Limits from the status-line feed. | **None.** One token family per slot, refreshed only under Claude Code's own lock. The default slot (CLI) and Desktop's env token are untouched. | All four hold. The app stores no token. | Allowed (documented multi-account method) | Live for Profiles with a running session. Last-known, with an "as of" time, for the others. | **Recommended baseline** |
| E | D + **read-only usage polling** with the slot's access token (= A scoped to Profile slots) | None to the chain, same as A | Same as A | Grey, leaning forbidden (token reuse + undocumented endpoint) | While the token is fresh | Separate ADR, off by default |
| F | **Own PKCE sign-in** per Profile with Claude Code's client id; tokens in the app's Keychain | Low for the chain: a separate token family. UNVERIFIED: any server-side cap on concurrent refresh tokens per user and client. | (1) ok | **Forbidden** (the app's own sign-in reusing Claude Code's OAuth client / impersonation) | Yes | **Reject** on terms |
| G | **`claude setup-token` per Profile**, stored in the app's Keychain | None: no refresh token, separate grant | (1) ok, but the app now holds a one-year bearer token | Forbidden as token reuse by a third-party tool. It is also scoped `user:inference` only, "can only make model requests", and the app makes none (#3). | UNVERIFIED whether `user:inference` can read usage | **Reject** |
| H | **`CLAUDE_SECURESTORAGE_CONFIG_DIR`** per Profile with a shared `CLAUDE_CONFIG_DIR`, to keep Transcripts shared | None to the chain, but `oauthAccount` in the shared `.claude.json` would describe only the last Profile to log in, and could mismatch the slot | (3) risk: undocumented internal variable, may change without notice | Not addressed | Same as D | Lead for [#13](https://github.com/Jbmanllr/claude-account-manager/issues/13). Needs a test in a scratch home before it is relied on. |

**Notes for the downstream tickets.**

- **[#12](https://github.com/Jbmanllr/claude-account-manager/issues/12), how the app holds a credential per Profile.**
  - The standing decision "one live credential per Profile" cannot be met by the app *holding and refreshing* a token without either C (it breaks the CLI) or F/G (forbidden by #3).
  - The safe reading of that decision is option D: the credential lives in that Profile's own Claude Code slot, and Claude Code keeps it.
  - An idle Profile's access token therefore goes stale after about 8 h. Its limits show as last-known until the user runs a session in that Profile.
  - Whether to also adopt E is the owner's decision. I recommend not adopting it by default, because it reuses tokens the terms research rates grey, leaning forbidden.
- **[#13](https://github.com/Jbmanllr/claude-account-manager/issues/13), switching the CLI's Active Profile.**
  - Never switch by copying a Profile's item into the default slot. Two slots would then hold the same refresh token, and the first refresh would kill the other copy.
  - Switch by pointing new `claude` runs at the Profile's config directory, or test H.
  - Mind the hash gotchas in §1. The app must set `CLAUDE_CONFIG_DIR` to a byte-identical absolute path every time, or Claude Code picks a different slot.
- **Desktop.** Leave the default slot alone while #101990 and #94464 are open. A Profile whose CLI slot is the default `Claude Code-credentials` is exposed to the desktop engine. Hashed per-Profile slots are not, unless Desktop is launched with that same `CLAUDE_CONFIG_DIR`.
- **Drift detection.**
  - The service-name formula, the field names and the lock files are all undocumented except the fact that the slot is "keyed to that directory".
  - 2.1.296 already carries a `storageV5` code path whose meaning I did not determine (UNVERIFIED). Treat the item format as unstable.

## Sources

Local code and state (versions above): the Claude Code native binaries `~/.local/share/claude/versions/2.1.296` and the bundled 2.1.308; `/Applications/Claude.app/Contents/Resources/app.asar` (3.6556.2); `security dump-keychain` attributes and ACLs; key names of `~/.claude.json`.

Docs:
- https://code.claude.com/docs/en/authentication (credential management, multiple accounts, `setup-token`, precedence)
- https://code.claude.com/docs/en/env-vars (`CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_OAUTH_REFRESH_TOKEN`, `CLAUDE_CODE_OAUTH_SCOPES`)
- https://code.claude.com/docs/en/cli-reference (`claude auth status`)
- https://code.claude.com/docs/en/troubleshoot-install (`.claude.json` location, `/logout` scope)
- https://code.claude.com/docs/en/desktop#environment-variables-in-local-sessions

Issues (https://github.com/anthropics/claude-code/issues/N): #20553, #70697, #76561, #80085, #80585, #88947, #91009, #91571, #94464, #101990, #103325, #104155, #104767.

Project: [#3 terms research](https://github.com/Jbmanllr/claude-account-manager/issues/3); [research/claude-multi-account](../claude-multi-account/).
