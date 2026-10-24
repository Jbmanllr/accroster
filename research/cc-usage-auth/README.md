# Claude Code usage and auth facts for the account manager

Checked 2026-10-24 against the installed Claude Code **2.1.296** (`/Users/jb/.local/bin/claude` -> `/Users/jb/.local/share/claude/versions/2.1.296`, a Bun-compiled Mach-O; the JS was read with `strings -n 8` into `scratchpad/cc/bundle.txt`), plus the Agent SDK npm tarball `@anthropic-ai/claude-agent-sdk@0.3.310` (`sdk.d.ts`) and the docs at code.claude.com (`/docs/en/*.md`, fetched with curl). Minified identifiers are from the 2.1.296 build and change between builds; the string literals next to them don't.

Commands actually run: `claude --help`, `claude auth {,login,status,logout} --help`, `claude --debug-file … auth status --json` (key names and non-identifying values only), one dry `mcp_status` control request and **one** live `get_usage` control request, both against the default config (key structure and types only). No login, logout, credential read or secret print.

---

## Q1. Can Claude Code fetch usage for a config dir itself?

**Yes.** The best route is the `get_usage` control request in stream-json print mode. It needs no user message and makes no model request. It refreshes tokens itself, under the same lock every Claude Code process uses. There is no `claude usage` subcommand.

### 1a. `get_usage` control request (recommended)

**Invocation (verified live):**

```sh
CLAUDE_CONFIG_DIR=<profile dir> claude -p --input-format stream-json --output-format stream-json --verbose \
  --no-session-persistence --strict-mcp-config
# stdin, one line:
{"type":"control_request","request_id":"probe-1","request":{"subtype":"get_usage","skip_behaviors":true}}
# then close stdin; the process exits 0
```

- **No `initialize` and no user message needed.** The reply came back as `{"type":"control_response","response":{"subtype":"success","request_id":"probe-1","response":{…}}}`. Before it, stdout carried only `system/hook_started` and `system/hook_response` lines from SessionStart hooks. No `system/init`, no `assistant` and no `result` message (command: `scratchpad/tools/ctlprobe.py`).
- **No model request.** The handler is `We=async(e,o)=>{… return{...await Ghn({storageV5,credentials,includeBehaviors:!o.redactsPersistedLane()&&e.skip_behaviors!==!0})}}`, registered in the shared control table `var xe={mcp_status:Be,…,get_usage:We,…}` (bundle @47737071, @47768462). `Ghn` only calls `bXn` (usage fetch) and, unless skipped, `pjt` (local transcript scan). The debug log of the live run shows `fetchUtilization: GET /api/oauth/usage (attempt 1)` and `fetchUtilization: 200 after 1 attempt(s)`, and no `/v1/messages` (`scratchpad/probe-usage.log`).
- **`skip_behaviors: true`** skips "the scan of local transcripts … the scan reads every transcript touched in the last seven days" (bundle schema `No=p(()=>u({subtype:C("get_usage"),skip_behaviors:H().optional()…`; same text in `sdk.d.ts` `SDKControlGetUsageRequest`). A non-boolean value gets the error `get_usage: skip_behaviors must be a boolean`.
- **Fetch path:** `bXn` -> `R` -> `eD` -> `km`. `km` calls `zt.get("/api/oauth/usage",{timeout:5000,…,refreshOAuth:!0,credentials:e,…})` and only fetches when `wt()&&If()` (claude.ai login with the `user:profile` scope). Otherwise `rate_limits_available:false, rate_limits:null` (bundle @17079628, @34781116).
- **Token refresh is Claude Code's own, under `.oauth_refresh.lock`:**
  - Before the request, the HTTP client's `RS(e){if(!e.refreshOAuth)return;try{await Yi({credentials,storageV5})}catch…"firstPartyApi: refreshOAuth failed (…); proceeding with cached token"}` runs (bundle @15768471).
  - `Yi` -> `Koe` -> `Kl`, which returns `"not_needed"` unless `rH(expiresAt)` (near expiry). It then takes the lock `V1n(e)={lockfilePath:vp(e,".oauth_refresh.lock"),stale:60000,update:5000,…}` in `Lw()` (the config dir), plus a legacy lock (`xks`).
  - Under the lock it **re-reads the Keychain** and returns `"refreshed"` without a network call if another process already rotated the token (`if(te.accessToken!==N)return … "tengu_oauth_token_refresh_race_resolved"`). Up to 5 lock retries at 1-2 s each, with dead-holder takeover (bundle @16012796, @16015141).
  - On a 401, `xS` and the 401-recovery path first re-read the Keychain, then force-refresh (bundle @15768471, @16012796).
  - **Inference:** this is the same double-checked locking any second `claude` process on that config dir uses, so it is as safe alongside running Sessions as opening a second terminal. Not stress-tested.
- **Caching, which makes polling cheap:**
  - On success it writes `cachedUsageUtilization:{fetchedAtMs,accountUuid,utilization}` into that config dir's `.claude.json` (`ySs`; skipped if the cache is under 60 s old).
  - A later read within `Hm=60000` ms is answered from that snapshot without calling the endpoint (`_Ss`; log line "Usage read answered from a snapshot …s old; endpoint not asked").
  - On errors or 429s it seeds from that cache if it is under `cS=3600000` ms old (1 h), or from rate-limit headers (status `"seeded"`) (`SSr`, `Whn`, `f`; bundle @17098029, @34778541).
  - Logout clears `cachedUsageUtilization` (Q4).
  - Side effect: **the probe writes `.claude.json`** in the profile dir. The cache is keyed by `oauthAccount.accountUuid`, the identity block that issue #15 says can mismatch the Keychain slot when `CLAUDE_SECURESTORAGE_CONFIG_DIR` is set.
- **Response shape (live, types only):**
  - Top level: `{session:{total_cost_usd,total_api_duration_ms,total_duration_ms,total_lines_added,total_lines_removed,model_usage:{}}, subscription_type:str, rate_limits_available:bool, rate_limits:{…}, behaviors:null}`.
  - `rate_limits` is the raw `/api/oauth/usage` body plus `model_scoped`:
    - `five_hour` and `seven_day`: `{utilization:number, resets_at:str, limit_dollars, used_dollars, remaining_dollars, locked_reason}`.
    - `seven_day_oauth_apps|opus|sonnet|cowork|omelette` and about 20 codenamed keys (all `null`).
    - `extra_usage{is_enabled,monthly_limit,used_credits,utilization,currency,decimal_places,disabled_reason,user_disabled,spend_limit_reached,credits_ever_enabled,daily,weekly}`.
    - `limits[3]{kind,group,percent,severity,resets_at,scope,is_active}`.
    - `spend{used{amount_minor,currency,exponent},limit,percent,severity,enabled,…}`, `member_dashboard_available`.
    - `seven_day_breakdown{as_of,window_started_at,rows[4]{key,display_name,percent}}`.
    - `weekly_scoped_shares[1]{limit_index,used_percent_of_weekly,allowance_percent_of_weekly}`.
    - `model_scoped[1]{display_name,utilization,resets_at}`.
  - The typed shape is in `sdk.d.ts` `SDKControlGetUsageResponse`.
- **Documented?** Only in the SDK typings, as `Query.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET(opts?:{skipBehaviors?})`. The docs comment reads "EXPERIMENTAL: this API is unstable and may change or be removed in any release without notice … The method name will change when the API is stabilized." (`sdk.d.ts` ~3118). It is absent from the TypeScript reference page's `Query` method table (https://code.claude.com/docs/en/agent-sdk/typescript). The raw stream-json `control_request` wire protocol is not documented for CLI callers either.
- **Cold spawn timing (this Mac, default config, `--strict-mcp-config`):**
  - `get_usage`: reply at **0.73 s**, process exit 1.46 s after spawn.
  - Dry `mcp_status`: 0.61 s and 1.32 s.
  - The endpoint call itself took about 0.2 s (debug log timestamps).
- **Other side effects of a spawn (from `probe-mcp.log`):**
  - The user's **SessionStart hooks and plugin hooks run** (ponytail, i-have-adhd).
  - `[Bootstrap] Fetching`, `[claudeai-mcp] Fetching from https://api.anthropic.com/v1/mcp_servers…` (even with `--strict-mcp-config`), and a telemetry `POST …/api/event_logging/v2/batch`.
  - `--bare` would skip hooks but **can't be used**: it makes "Anthropic auth … strictly ANTHROPIC_API_KEY or apiKeyHelper … (OAuth and keychain are never read)" (`claude --help` text in the bundle).
  - Unverified mitigation: pass `--settings '{"disableAllHooks":true}'` and spawn with a neutral cwd, so project hooks and CLAUDE.md of an arbitrary cwd don't load.

### 1b. `claude -p "/usage"` (works, but weaker)

- `/usage` is registered for print mode as `{type:"local",name:"usage",aliases:["cost","stats"],supportsNonInteractive:!0,…}` (bundle @22817451).
- Local commands return `{messages:[…],shouldQuery:!1,…}` unless the command returns `type:"query"`. `/usage` returns `{type:"text",value,usageReport}`, so **no model request** (bundle @42299440, `case"local":{…}`).
- Documented: with stream-json output the assistant message carries `usage_report` (`SDKUsageReport`) when the session uses a claude.ai credential with a known plan or `user:profile`, and is not on usage-based billing (https://code.claude.com/docs/en/agent-sdk/typescript, `SDKAssistantMessage` and `SDKUsageReport`; "experimental: its shape may change").
- Weaknesses:
  - `usage_report.rate_limits` carries only `{limits, extra_usage}` (`rate_limits:e===null?null:{limits:e.limits??null,extra_usage:e.extra_usage}`, bundle @34798058). No `five_hour`/`seven_day`, breakdown or `model_scoped`.
  - It always runs the 7-day transcript scan (`includeBehaviors:i&&!lo(e.session)&&r.allowed`). `skip_behaviors` is not available.
  - It creates a session transcript unless `--no-session-persistence`.
- Same fetch and refresh path (`Ghn`). **Not run live** (one-fetch budget used on 1a).

### 1c. Subcommands and flags

None. `claude --help` lists no `usage` command (agents, attach, auth, auto-mode, doctor, gateway, import, install, logs, mcp, plugin, purge, respawn, rm, setup-token, stop, ultrareview, update). `claude usage …` without `--help` would be taken as a prompt.

---

## Q2. `claude auth login` from a GUI with no TTY

Source: auth chunk `export{ge as authLogin,_e as authLogout,me as authStatus}` (bundle @39071574–39077300), class `$U` (@35290581) and the callback listener `u` (@35286829).

- **Flags:** `--claudeai` (default), `--console`, `--email <email>` (sent as `loginHint`), `--sso` (`loginMethod:"sso"`) (`claude auth login --help`; docs https://code.claude.com/docs/en/cli-reference). `--console` together with `--claudeai` gives "Error: --console and --claudeai cannot be used together." and exit 1.
- **No org flag.** The org is chosen in the browser. `forceLoginOrgUUID` from settings is passed as `orgUUID` when the method matches (`P=typeof w.forceLoginOrgUUID==="string"&&!I?w.forceLoginOrgUUID:void 0`). **Inference:** a Profile could pin its org through `settings.json` in its config dir, which would also enforce it in Sessions.
- **Flow:** both. `startOAuthFlow` starts an HTTP server on `127.0.0.1:<random port>` with path `/callback`, and builds a manual URL and an automatic URL (`Wwr({...n,isManual:!0})` and `Wwr({...n,isManual:!1})`).
  - The CLI callback prints three lines to **stdout**:
    - `Opening browser to sign in…`
    - `If the browser didn't open, visit: <manual URL>`
    - `Paste code here if prompted > ` (no newline)
  - It then opens the **automatic (localhost-redirect) URL** with `$s(p)`. On macOS that runs `open <url>`, or `$BROWSER` or the `browser` setting if set (chunk exporting `$s`).
  - The printed URL is wrapped in an OSC-8 hyperlink only when `process.stdout.isTTY` (`dw(o,void 0,{assumeSupport:!0})`). Piped stdout gets the plain URL.
  - Paste-a-code fallback: stdin is read with readline. A line `code#state` completes the flow (`handleManualAuthCodeInput`); otherwise it prints "Invalid code. Please make sure the full code was copied." to stderr. Docs: "If your browser shows a login code instead of redirecting back … paste it into the terminal at the `Paste code here if prompted` prompt" (https://code.claude.com/docs/en/authentication).
- **Success:**
  - Saves tokens to that config dir's slot (`aje`).
  - Validates org and provider policy (`w$`).
  - Sets `hasCompletedOnboarding:true`, prints `Login successful.` to stdout and exits **0**.
- **Failure:** prints `Login failed: <msg>` to stderr and exits **1**. Account on hold prints the on-hold message and exits 1. A policy rejection (`provider_not_allowed`, org not allowed) prints the message to stderr, runs a cleanup (`tBe`, not inspected), and exits 1.
- **Cancel:** the listener has **no timeout** in this path (only the separate Claude Design login has a 5-minute one).
  - If the user closes the tab, the process waits forever, so the app must kill it (signal exit). Nothing is written before the code exchange.
  - A browser-side denial redirects to `/callback` with `error`. It ends in `Login failed …` and exit 1, either through `respondToErrorCallback` (behind a flag, `T0t()`) or through "No authorization code received".
- **Non-interactive?** It needs no TTY: piped stdout and stdin work by construction (readline on `process.stdin`). Debug mode logs `Could not open /dev/tty for stdin override` with no TTY, which seen with `auth status` is harmless. But a human must finish the browser step.
  - The only fully headless path is `CLAUDE_CODE_OAUTH_REFRESH_TOKEN` plus `CLAUDE_CODE_OAUTH_SCOPES` (login from an existing refresh token). That conflicts with #5 (never copy refresh tokens), so it is out.
  - Not run (forbidden by the brief).

## Q3. `claude auth status --json`

Source: `me` in the auth chunk, `vl()` (bundle @15973788), and the live command.

- **Fields (live run, default config):** `loggedIn:bool, authMethod:str, apiProvider:str, analyticsDisabled:bool, projectsDirectory:str, configDirectory:str, email, orgId, orgName, subscriptionType`. The code also adds `forcedLoginMethod`, `allowedProviders` and `apiKeySource` when relevant.
- `email`, `orgId` and `orgName` come from `oauthAccount` in `.claude.json` (`Hn()`), and **only when `authMethod==="claude.ai"`**. `subscriptionType` comes from the credential (`Sr()`).
- `authMethod` is one of `none|claude.ai|oauth_token|api_key|api_key_helper|third_party`. Exit code is 0 if logged in, 1 if not. `configDirectory` needs 2.1.268 or later (docs: https://code.claude.com/docs/en/cli-reference).
- **Network:** none in the code path. It is local reads only: Keychain via the CLI's own `security` call, `.claude.json` and settings. The `--debug-file` log of the live run shows no HTTP. It took **0.34 s**, exit 0, empty stderr. It may enqueue a telemetry event (`g("cli_auth_status")`); not observed in the log.
- **Expired access token, valid refresh token:** still `loggedIn:true`. No expiry check and no refresh happen here (`vl()` checks only `pW(scopes)&&accessToken`).
- **"Login expired" (blanked) state:** on `invalid_grant`, `QEr` rewrites the slot to `claudeAiOauth:{...y,refreshToken:"",accessToken:"",expiresAt:0}` (bundle @16001278).
  - `vl()` then returns `{source:"none",hasToken:!1}`, so status reports `loggedIn:false, authMethod:"none"`, with no email or org fields, and exits 1.
  - That **looks exactly like never-logged-in or logged-out.**
  - **Inference:** to tell them apart, the app can check whether `oauthAccount` is still present in that dir's `.claude.json`. Blanking keeps it; logout deletes it (Q4).
- Text mode (`--text`) prints "Not logged in. Run claude auth login to authenticate." when logged out.

## Q4. `claude auth logout` under a `CLAUDE_CONFIG_DIR`

Source: `_e` in the auth chunk -> `Jie({clearOnboarding:!1,…})` (bundle @35148266) and the macOS store `delete()` (@15075675). Not run.

- **Server-side revoke:** for first-party logins it re-reads the slot and POSTs `${TOKEN_URL}/revoke` with `{token:<refreshToken>,token_type_hint:"refresh_token",client_id}` for `claudeAiOauth`, and for `designOauth` if present (`rT`, bundle @16048385). A revoke failure is logged ("OAuth token revoke failed … continuing with local logout.") and doesn't block. Unverified: whether the server also kills the live access token.
- **Keychain:**
  - Deletes the **whole** slot item: `security delete-generic-password -a $USER -s <service>`, where the service is `Claude Code-credentials[-<hash>]` for this config dir. It runs inside a store-write lock wrapper (`Gwo`).
  - That loses `mcpOAuth` and `designOauth` too.
  - It then re-creates the item only if `coworkRemoteDevice` existed (`X`/`J` "device identity is carried across the wipe").
  - Also removes the Console API key item and `primaryApiKey` (`Lyo` -> `Nk` -> "Failed to remove API key from macOS keychain").
- **Anthropic profile credentials (`N`):** if an Anthropic profile config exists under `~/.config/anthropic/configs/<profile>.json`, it revokes and deletes that profile's credential file. That directory is **not per `CLAUDE_CONFIG_DIR`**. Inference: it only matters if a Profile ever signed in through a keyless Console or `ant` profile.
- **`.claude.json` in that config dir:** removes `oauthAccount` and the account caches, including `cachedUsageUtilization`, `modelAccessCache`, `clientDataCache`, `metricsStatusCache` and `githubWebConnectionStatusCache`. Onboarding is kept (`clearOnboarding:false`). Unsets `CLAUDE_CODE_OAUTH_TOKEN` in-process and clears in-memory caches (`nyn`).
- Prints "Successfully logged out from your Anthropic account." On failure it prints `Logout failed: …` and exits 1.

---

## Not verified

- `claude -p "/usage"` was not run live (code and docs only).
- Concurrent refresh under load with a running Session on the same dir: reasoned from the lock code, not tested.
- `auth login`'s exact stdout under a real GUI spawn, behaviour on browser denial with flag `T0t` on or off, and whether `tBe` wipes the slot on a policy rejection.
- Whether logout's refresh-token revoke also invalidates outstanding access tokens.
- Whether `disableAllHooks` via `--settings` suppresses the SessionStart hooks in the `get_usage` spawn.
- Long-term stability: `get_usage` is explicitly experimental, and the SDK method name is set to change.
