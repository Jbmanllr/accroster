# Which environment variables can make Claude Code write a different identity into `oauthAccount`?

Answers [#26](https://github.com/Jbmanllr/claude-account-manager/issues/26) for the map in [#1](https://github.com/Jbmanllr/claude-account-manager/issues/1). Raised in [#20](https://github.com/Jbmanllr/claude-account-manager/issues/20) (item 13). Researched 2026-10-25 on macOS (Apple silicon). Builds on [#5 cli-credentials](https://github.com/Jbmanllr/claude-account-manager/blob/research/cli-credentials/research/cli-credentials/README.md), [#15 securestorage-config-dir](https://github.com/Jbmanllr/claude-account-manager/blob/research/securestorage-config-dir/research/securestorage-config-dir/README.md) and [ADR 0002](../../docs/adr/0002-cli-switching-by-path-shim.md).

**Method.** I searched the JavaScript bundled in the installed Claude Code binaries (`~/.local/share/claude/versions/2.1.310`, which `~/.local/bin/claude` points to, plus 2.1.295 and 2.1.296 kept beside it) for every `CLAUDE_CODE_*` and `ANTHROPIC_*` name, then read the code around each read of an identity variable and each write of `oauthAccount`. I read the official docs at code.claude.com (env-vars, authentication, settings-reference), fetched 2026-10-25. I did not run Claude Code, sign in or out, or read any Keychain item, token file or `.claude.json`. Identifiers below are placeholders.

**Labels.** **Code** = read in the 2.1.310 bundle (minified names in brackets, only to help find them again). **Docs** = code.claude.com. **Inference** = my reasoning from the above.

## Answer

1. **Only one path writes an identity taken from the environment: `CLAUDE_CODE_ACCOUNT_UUID` + `CLAUDE_CODE_USER_EMAIL` + `CLAUDE_CODE_ORGANIZATION_UUID`, all three set.** At every interactive or `-p` startup, if the config dir's `.claude.json` has no `oauthAccount`, Claude Code writes `{accountUuid, emailAddress, organizationUuid}` from these three variables. The next step, an OAuth profile fetch, replaces it with the real token's identity, but only when a claude.ai login with the `user:profile` scope is present and the fetch succeeds. Otherwise the made-up identity stays. **Code** (§1)
2. **Two more variables bring in a whole different account, and the identity follows the token:**
   - `CLAUDE_CODE_OAUTH_REFRESH_TOKEN` (with `CLAUDE_CODE_OAUTH_SCOPES`) makes `claude auth login` sign in with that token instead of opening the browser. The Profile directory then holds that token's Login and Organization, whatever `--email` says. **Code + Docs** (§2)
   - `CLAUDE_CODE_OAUTH_TOKEN` outranks the Profile's stored login for every request. It does not rewrite `oauthAccount` by default, so `claude auth status` keeps showing the Profile's identity while usage and billing go to the token's account. With `CLAUDE_CODE_OAUTH_SCOPES` including `user:profile`, the profile fetch writes the token's identity into `oauthAccount` too. **Code + Docs** (§2)
3. **`CLAUDE_CODE_ORGANIZATION_UUID` alone writes nothing, but it overrides the cached Organization** for cloud-session creation, the plugin and skill sync scope, and some org checks. **Code** (§3)
4. **Every other `ANTHROPIC_*` / `CLAUDE_CODE_*` credential variable replaces the credential and never writes `oauthAccount`.** These are `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_PROFILE`, the federation variables and `CLAUDE_CODE_USE_*`. While one of them is in use, Claude Code ignores `oauthAccount` as the identity. These are deliberate user setups, documented in the authentication precedence. **Code + Docs** (§4)
5. **None of the identity variables can come from a settings file.** The docs and the code both drop `CLAUDE_CODE_ACCOUNT_UUID`, `CLAUDE_CODE_ORGANIZATION_UUID` and `CLAUDE_CODE_USER_EMAIL` from every `env` block. Claude Code sets them only for its own cloud and bridge children, never for local tool subprocesses. They reach a local shell only if the user or some other tool exports them. `CLAUDE_CODE_OAUTH_TOKEN` and `ANTHROPIC_API_KEY` *can* be set in user settings `env`, which the shim cannot clear. **Code + Docs** (§5)
6. **Recommendation for the shim** (Inference, §6): when it applies a Profile, clear `CLAUDE_CODE_ACCOUNT_UUID`, `CLAUDE_CODE_USER_EMAIL`, `CLAUDE_CODE_ORGANIZATION_UUID`, `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_OAUTH_REFRESH_TOKEN` and `CLAUDE_CODE_OAUTH_SCOPES`, plus `CLAUDE_SECURESTORAGE_CONFIG_DIR` as already decided. Leave the provider and API-key variables alone.

## 1. The environment identity writer (Code, 2.1.310; same in 2.1.295 and 2.1.296)

Called once from startup init, after telemetry setup (`init_after_oauth_populate`) [`nqo`]:

```js
let s=env.CLAUDE_CODE_ACCOUNT_UUID, h=env.CLAUDE_CODE_USER_EMAIL, y=env.CLAUDE_CODE_ORGANIZATION_UUID,
    b = !diskless() && s && h && y ? {accountUuid:s, emailAddress:h, organizationUuid:y} : null;
if (b && !config().oauthAccount) storeOAuthAccountInfo(b, …);
… // then the profile fetch, see below
if (b) log("OAuth profile fetch succeeded, overriding env var account info");
```

- **All three must be non-empty.** `diskless()` is a host launch option, off for a normal CLI start.
- **It writes only when `oauthAccount` is absent.** That means a fresh Profile directory before its first login, or one signed out by `claude auth logout` (logout removes `oauthAccount`, per #20). It never overwrites an existing block.
- **The write is persistent:** a normal global-config update of that config dir's `.claude.json` [`__n` → `Re`].
- **The profile fetch that follows** overwrites the whole identity with `/api/oauth/profile` for the token in use. It runs only when both of these hold:
  - a claude.ai OAuth credential with `user:inference` is the active credential [`vt`], and its scopes include `user:profile` [`vp`];
  - the cached block lacks `billingType` / `accountCreatedAt` / `subscriptionCreatedAt` / `ccOnboardingFlags` (the env block always lacks them) or is older than 24 h (`Hb=86400000`).

  **So the made-up identity survives** when the directory has no login, when the login is expired and its refresh fails, when the token lacks `user:profile` (a `setup-token` token), when the fetch fails (offline, 401, 429), or when an API key, `ANTHROPIC_AUTH_TOKEN`, profile or cloud provider is the credential.

**What then reads the made-up block** (from #15 §3.1; same readers in 2.1.310):

- `claude auth status --json` (`email`, `orgId`, `orgName`);
- the `x-organization-uuid` on cloud and teleport calls;
- the usage cache key (`cachedUsageUtilization.accountUuid`, used by #20 item 8);
- the synced-skills bucket.

**For the app** (Inference): a signed-out Profile directory started through the shim with these three variables inherited gets an `oauthAccount`. #20 item 9 then reads that as "Login expired" under the made-up email, not as signed out. A later `claude auth login` replaces the block, because login clears and rewrites it (#15 §2).

The same three variables are read elsewhere without writing (Code):

| Reader | Effect |
|---|---|
| Analytics `user_id` metadata | `account_uuid` from the env only when `CLAUDE_CODE_REMOTE` is set |
| OTel attributes | `user.email` from `CLAUDE_CODE_USER_EMAIL` only in diskless or hosted mode |
| Browser-bridge user id | falls back to `CLAUDE_CODE_ACCOUNT_UUID` when `oauthAccount` has none, then prefers the token-derived account and warns on mismatch |
| Scope keys for env/fd tokens [`ee`, `Ktn`, `Eyo`] | Use the env account and org when the credential is an injected token (`env`/`fd`), not a stored login |

## 2. Variables that sign the directory in as someone else

### `CLAUDE_CODE_OAUTH_REFRESH_TOKEN` (+ `CLAUDE_CODE_OAUTH_SCOPES`, `CLAUDE_CODE_OAUTH_CLIENT_ID`)

- **Docs.** "When set, `claude auth login` exchanges this token directly instead of opening a browser. Requires `CLAUDE_CODE_OAUTH_SCOPES`." (env-vars)
- **Code** [`Ue`, the `auth login` handler]: if the variable is set, the handler skips the browser flow and ignores `--email` / `--claudeai`. It exchanges the token (client id from `CLAUDE_CODE_OAUTH_CLIENT_ID` if set), fetches the profile when the scopes include `user:profile`, and installs the result as this config dir's login. It exits 1 if `CLAUDE_CODE_OAUTH_SCOPES` is missing.
  - On a refused org/provider check, it restores the previous `oauthAccount` [`oqo`].
  - It prints that the refresh token "cannot be used again".
- **For the app** (Inference): #20 item 9's "Sign in again…" (`claude auth login --claudeai --email <email>`) would silently sign the Profile in as the token's account. It would also burn the user's single-use refresh token.

### `CLAUDE_CODE_OAUTH_TOKEN`

- **Docs.** It is rank 5 in authentication precedence, above `/login` credentials. "Takes precedence over keychain-stored credentials." A `setup-token` token "can only make model requests".
- **Code.**
  - The active credential [`Ln` → `k$`] is the env token whenever it is set, with scopes from `CLAUDE_CODE_OAUTH_SCOPES` or else `["user:inference"]`. `CLAUDE_CODE_SUBSCRIPTION_TYPE` and `CLAUDE_CODE_RATE_LIMIT_TIER` supply its plan fields.
  - With default scopes, `vp()` is false, so the profile fetch in §1 is skipped and `oauthAccount` is not rewritten.
  - Bootstrap may still send it, as "scopeless OAuth". Bootstrap merges org and email fields into `oauthAccount` only when the response's `account_uuid` equals the cached one [`X`]. So a token for a **different** Login writes nothing.
  - For a token of the **same** Login in another Organization, those org fields would be overwritten. That is Inference: whether bootstrap returns `oauth_account` for an inference-only token is server behaviour I did not observe.
  - With `CLAUDE_CODE_OAUTH_SCOPES` including `user:profile`, the §1 fetch runs against the env token and writes its account over a stale (>24 h) or incomplete block.
- **For the app** (Inference): the Profile row and `claude auth status` keep showing the Profile's Login while requests, limits and billing belong to the token's account. Both #5's identity source and #20's Limit readings would be wrong without any error.
- **Where it comes from**:
  - The desktop app injects it into its bundled engine (#5).
  - Claude Code's Bash tool passes the parent environment through unless `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` (Docs, env-vars "What the subprocess environment scrub removes"). So a `claude` typed into a desktop-app Session's terminal tool inherits the desktop's token (Inference).
  - Users export it for CI and scripts (Docs).

`CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR` and `CLAUDE_CODE_SESSION_ACCESS_TOKEN` are host-injected equivalents (Code: read only with a live fd or a host session). They are not useful in an interactive shell, and inheriting them there is unlikely.

## 3. `CLAUDE_CODE_ORGANIZATION_UUID` alone (Code, 2.1.310)

It never writes `oauthAccount` on its own. It takes precedence over `oauthAccount.organizationUuid` in:

- the org for cloud-session and teleport creation [`Boe` → `JEt`: env, then cache, then a profile fetch];
- the plugin and skill sync scope key [`kn`, `Pmn`];
- the OAuth principal memo key [`Fe`];
- the `X-Organization-Uuid` header sent with a session-key cookie [`dW`].

Several gates refuse work when it differs from the cached org (the GitHub web-connection cache, the API-credit link, the feedback survey [`X6`]). So an inherited value sends the wrong Organization or turns features off for that Session. It does not rewrite stored identity.

## 4. Credential variables that do not write `oauthAccount`

Claude Code lists its auth variables in one registry (Code, the `P` object). Its members are `AGENT_PROXY_AUTH_TOKEN`, `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_AWS_API_KEY`, `ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_FOUNDRY_*`, `ANTHROPIC_ORGANIZATION_ID`, `ANTHROPIC_PROFILE`, `ANTHROPIC_WORKSPACE_ID`, `AWS_*`, `CLAUDE_CODE_ACCOUNT_*`, `CLAUDE_CODE_*_FILE_DESCRIPTOR`, `CLAUDE_CODE_CUSTOM_OAUTH_URL`, `CLAUDE_CODE_OAUTH_*`, `CLAUDE_CODE_ORGANIZATION_UUID`, `CLAUDE_CODE_SESSION_ACCESS_TOKEN`, `CLAUDE_CODE_SKIP_*_AUTH`, `CLAUDE_CODE_SUBSCRIPTION_TYPE`, `CLAUDE_CODE_RATE_LIMIT_TIER`, `CLAUDE_CODE_USER_EMAIL`, `CLAUDE_TRUSTED_DEVICE_TOKEN`, `USE_LOCAL_OAUTH`, `USE_STAGING_OAUTH` and a few others. The ones not covered in §1–3:

| Variable(s) | What it changes | Writes `oauthAccount`? |
|---|---|---|
| `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `apiKeyHelper` | Become the credential (precedence 2–4, Docs); `nl()` is false, so `oauthAccount` is ignored as identity and the profile fetch is skipped (Code) | No |
| `ANTHROPIC_PROFILE`, `ANTHROPIC_FEDERATION_RULE_ID` + `ANTHROPIC_ORGANIZATION_ID` | Select an Anthropic profile / WIF credential above `/login` (Docs). A keyless Console sign-in writes the profile `ANTHROPIC_PROFILE` names and "signs you out of any claude.ai login stored on the machine" (Docs) | Not from the env; a Console sign-in changes login state |
| `CLAUDE_CODE_USE_BEDROCK` / `_VERTEX` / `_FOUNDRY` / `_ANTHROPIC_AWS` / `_MANTLE` | Third-party provider; bootstrap and profile fetch skipped (`Te()!=="firstParty"`) | No |
| `CLAUDE_CODE_CUSTOM_OAUTH_URL` | Must be an approved endpoint; changes the OAuth host and the Keychain suffix to `-custom-oauth`, so the directory reads a different credential item | Only via a login made under it |
| `USE_STAGING_OAUTH`, `USE_LOCAL_OAUTH` | Inert in release builds: the environment function returns the constant `"prod"` | No |
| `ANTHROPIC_BASE_URL` | Request endpoint; bootstrap uses the built-in API host except for WIF | No |

## 5. Where the variables can come from, and collision with `CLAUDE_CONFIG_DIR`

- **Settings files cannot set the identity variables.** Docs (settings-reference, "Variables Claude Code ignores in `env`"): "Identity variables that Claude Code's hosting environments own, such as `CLAUDE_CODE_REMOTE` and `CLAUDE_CODE_ACCOUNT_UUID`, are ignored from every file." Code: the same filter list [`Le`] holds `CLAUDE_CODE_ACCOUNT_UUID`, `CLAUDE_CODE_ORGANIZATION_UUID` and `CLAUDE_CODE_USER_EMAIL`.
- **Claude Code sets them only for hosted children.** These are the cloud runner's child env (from the session's `environment_variables`) and bridge children, where it explicitly sets all three to `undefined`. No `process.env.CLAUDE_CODE_ACCOUNT_UUID=` assignment exists in the bundle. A local Bash-tool child therefore inherits them only if Claude Code itself inherited them. **Code**
- **Settings files *can* set `CLAUDE_CODE_OAUTH_TOKEN` and `ANTHROPIC_API_KEY`** (Docs, authentication precedence 5 mentions "the `env` block of a settings file"). A settings `env` value overwrites the shell's. Per ADR 0002, `settings.json` is linked across Profiles, so such a value applies to every Profile, and no shim unset can reach it. **Docs + Inference**
- **Settings can also set `CLAUDE_CONFIG_DIR`** (user or managed settings only; Docs, env-vars). That is a second, file-based override beside the shim's per-shell rule. In a shared `settings.json` it would apply under every Profile. I did not trace when a settings-file `CLAUDE_CONFIG_DIR` takes effect. **Docs; effect not verified**
- **`CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` removes `CLAUDE_CONFIG_DIR`** from Bash-tool subprocesses (Docs). With it on, a `claude` started inside a Session falls back to the shim's pointer instead of staying on the Session's Profile. **Docs + Inference**
- **No variable found here collides with the precedence rule itself.** None of them changes which config dir is used; `CLAUDE_SECURESTORAGE_CONFIG_DIR` (#15) changes only the credential store. Clearing the variables in §6 is independent of whether the shim keeps an inherited `CLAUDE_CONFIG_DIR`. **Code + Inference**

## 6. What the shim should clear (Inference)

**Clear on every launch, including when an inherited `CLAUDE_CONFIG_DIR` wins:**

| Variable | Why |
|---|---|
| `CLAUDE_CODE_ACCOUNT_UUID`, `CLAUDE_CODE_USER_EMAIL`, `CLAUDE_CODE_ORGANIZATION_UUID` | §1 made-up `oauthAccount` in a signed-out Profile; §3 org override. Host-owned (Docs), never legitimately set in a local shell. |
| `CLAUDE_CODE_OAUTH_TOKEN` | §2: overrides the Profile's login for every request while the Profile's identity is still shown. Also blocks the desktop token leaking into a `claude` started from a desktop Session. |
| `CLAUDE_CODE_OAUTH_REFRESH_TOKEN`, `CLAUDE_CODE_OAUTH_SCOPES` | §2: `claude auth login` signs in as the token's account. The scopes also widen what an env token may rewrite. |
| `CLAUDE_SECURESTORAGE_CONFIG_DIR` | Already decided (#15, ADR 0002). |

- **Leave alone:** `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_PROFILE`, the WIF variables, `CLAUDE_CODE_USE_*` and `CLAUDE_CODE_CUSTOM_OAUTH_URL`. They are documented, deliberate setups (gateways, API billing, cloud providers). They do not touch `oauthAccount`, and `/status` already reports when one of them is the credential. Clearing them would break those setups behind the user's back. The app's own spawns already clear all `CLAUDE*`/`ANTHROPIC_*` (#20 item 13), so its readings are unaffected.
- **Not solvable in the shim:** `CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY` or `CLAUDE_CONFIG_DIR` in the shared `settings.json` `env`. Drift detection could flag them (the fog item in #13).

## Sources

- Claude Code 2.1.310 native binary, `~/.local/share/claude/versions/2.1.310` (bundled JS); spot checks in 2.1.295 and 2.1.296.
- https://code.claude.com/docs/en/env-vars (rows for `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_OAUTH_REFRESH_TOKEN`, `CLAUDE_CODE_OAUTH_SCOPES`, `ANTHROPIC_PROFILE`, `CLAUDE_CONFIG_DIR`; "Set variables in settings files"; "What the subprocess environment scrub removes"). `CLAUDE_CODE_ACCOUNT_UUID`, `CLAUDE_CODE_USER_EMAIL`, `CLAUDE_CODE_ORGANIZATION_UUID`, `CLAUDE_CODE_SUBSCRIPTION_TYPE` and `CLAUDE_CODE_RATE_LIMIT_TIER` have no row.
- https://code.claude.com/docs/en/authentication ("Authentication precedence", "Anthropic profiles and federation credentials", "Generate a long-lived token", "Log in with multiple accounts").
- https://code.claude.com/docs/en/settings-reference (`env`: "How `env` values interact with your shell", "Variables Claude Code ignores in `env`").
