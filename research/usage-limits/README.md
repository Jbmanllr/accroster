# Where Session and Weekly limit figures come from

Answers [issue #4](https://github.com/Jbmanllr/claude-account-manager/issues/4) for the map in [issue #1](https://github.com/Jbmanllr/claude-account-manager/issues/1).

Researched 2026-10-24 on macOS against Claude Code CLI 2.1.296 (`~/.local/bin/claude` -> `~/.local/share/claude/versions/2.1.296`, a Bun single-file Mach-O whose bundled JS was searched with `strings`/Python), the code.claude.com docs, the `anthropics/claude-code` changelog and issues, and community trackers. Labels:

- **Local binary**: string or code found in the installed Claude Code 2.1.296 bundle (quoted minified JS, identifiers are minifier names).
- **Live call**: one `GET /api/oauth/usage` made on 2026-10-24 with this Mac's CLI token, read from the Keychain inside a Python process. Only key names, types and enum-like labels were printed; no token, percentage or timestamp value was printed or saved. No refresh was attempted.
- **Documented**: stated on code.claude.com or in the official changelog.
- **Issue report / community**: GitHub issue or third-party tool. Observed behavior, not confirmed by Anthropic.
- **Inference**: my reasoning from the above.

## Summary

- **One server source.** `/usage`, the VS Code usage panel, the SDK `get_usage` request and every serious community tracker read `GET https://api.anthropic.com/api/oauth/usage`, an **undocumented** endpoint authenticated with the claude.ai OAuth access token (`Authorization: Bearer`, `anthropic-beta: oauth-2025-04-20`). The token must carry the **`user:profile`** scope; `claude setup-token` tokens (inference only) cannot read it.
- **The response answers the ticket directly.** `five_hour` is the Session limit and `seven_day` the Weekly limit (`utilization` 0-100 float, `resets_at` ISO 8601). Model-specific weekly limits now arrive in a `limits[]` array (`kind: "weekly_scoped"`, `scope.model.display_name`, e.g. "Fable"). The legacy `seven_day_opus` / `seven_day_sonnet` keys are present but `null` on this account.
- **Scope is Login + Organization**, which matches our Profile: the endpoint answers for the token's account in its organization, and an issue report says quota is pooled per `organizationUuid`.
- **It is rate limited per credential, with an unstable history.** In March 2026 many trackers got persistent 429s (issues #30930, #31021, #31637). Claude Code itself backs off: on 429 or a plain 403 it does not ask again for `retry-after` (default 300 s, capped at 1 h), and on 401 or an Anthropic 403 for 1 h for that bearer. Community tools settle on a poll every 3-10 minutes with a backoff that survives restarts.
- **The documented alternative is the status line JSON** (`rate_limits.five_hour` / `seven_day`, since 2.1.80). It covers only the Active Profile, only during a running session, and has no model-specific weekly window. It is built from `anthropic-ratelimit-unified-*` response headers on `/v1/messages`.
- **Local token counting (ccusage) cannot produce these figures.** It estimates 5-hour blocks from transcripts and misses claude.ai chat, Cowork, other devices and server-side weighting.
- **Recommendation (inference):** poll `/api/oauth/usage` per Profile with a `user:profile` token, parse `limits[]` keyed on `kind` and fall back to `five_hour` / `seven_day`. Keep the last good sample, honour `retry-after` across restarts, and tolerate unknown keys and `null`s.

---

## 1. The usage endpoint Claude Code calls

### Request

| Item | Value | Evidence |
|---|---|---|
| URL | `https://api.anthropic.com/api/oauth/usage` | Local binary: `var Ww={plain:"/api/oauth/usage",at_wall:"/api/oauth/usage?at_wall=1&skip_spend=1",cedar_ember:"/api/oauth/usage?cedar_ember=1&skip_spend=1"}`; base URL `BASE_API_URL:"https://api.anthropic.com"` |
| Method | `GET`, 5 s timeout | Local binary: `zt.get(n,{timeout:5000,headers:{"Content-Type":"application/json"},refreshOAuth:!0,credentials:e,validateStatus:Bw})` in `fetchUtilization` |
| Auth headers | `Authorization: Bearer <accessToken>`, `anthropic-beta: oauth-2025-04-20` | Local binary: `{headers:{Authorization:\`Bearer ${r.accessToken}\`,"anthropic-beta":pp}}` with `pp="oauth-2025-04-20"` |
| Required scope | `user:profile` | Local binary: the fetch is gated on `wt()&&If()`, where `function If(){let e=Sn()?.scopes;return Array.isArray(e)&&e.includes(EY)}` and `EY="user:profile"`. The SDK schema says `rate_limits_available` is false for "API key, Bedrock, Vertex, or missing profile scope". |
| Query variants | `?at_wall=1&skip_spend=1` (read when a limit is hit) and `?cedar_ember=1&skip_spend=1` (adds saved "limit reset" grants) | Local binary map above. CodexBar documents `cedar_ember` as the reset-grant inventory ([CodexBar docs/claude.md](https://github.com/steipete/CodexBar/blob/main/docs/claude.md)). |

The interactive-login token on this Mac has scopes `user:file_upload user:inference user:mcp_servers user:plugins user:profile user:sessions:claude_code` (live call, scope names only). The bundle's default login scopes match: `n=[EY,kT,"user:sessions:claude_code","user:mcp_servers","user:file_upload"]` plus `user:plugins` when registered (local binary). Tokens from `claude setup-token` / `CLAUDE_CODE_OAUTH_TOKEN` are "limited to inference-only" ([community write-up quoting the CLI](https://github.com/AnotherSava/claude-code-common/blob/main/claude/learnings/anthropic-oauth-usage.md); issues [#79360](https://github.com/anthropics/claude-code/issues/79360) and [#99804](https://github.com/anthropics/claude-code/issues/99804) report usage data missing under that auth).

### Response shape (live call, 2026-10-24, Max plan, personal org)

HTTP 200, `Content-Type: application/json`, and no rate-limit headers on the success response. Values are replaced by their types:

```jsonc
{
  "five_hour":  { "utilization": float, "resets_at": "<ISO 8601>",
                  "limit_dollars": null, "used_dollars": null, "remaining_dollars": null, "locked_reason": null },
  "seven_day":  { /* same fields */ },
  "seven_day_oauth_apps": null, "seven_day_opus": null, "seven_day_sonnet": null,
  "seven_day_cowork": null, "seven_day_omelette": null,
  // codenamed buckets, all null on this account:
  "tangelo", "iguana_necktie", "omelette_promotional", "nimbus_quill", "cinder_cove", "copper_kite",
  "brass_thimble", "harbor_lantern", "wattle_ember", "amber_ladder", "granite_teacup", "amber_cistern",
  "amber_cistern_offer", "juniper_tide", "cedar_ember", "starry_waterfall", "amber_gauge", "pace",
  "extra_usage": { "is_enabled": bool, "monthly_limit", "used_credits", "utilization", "currency",
                   "decimal_places", "disabled_reason", "user_disabled": bool, "spend_limit_reached": bool,
                   "credits_ever_enabled": bool, "daily", "weekly" },
  "limits": [
    { "kind": "session",       "group": "session", "percent": int, "severity": "warning", "resets_at": "<ISO>", "scope": null, "is_active": bool },
    { "kind": "weekly_all",    "group": "weekly",  "percent": int, "severity": "warning", "resets_at": "<ISO>", "scope": null, "is_active": bool },
    { "kind": "weekly_scoped", "group": "weekly",  "percent": int, "severity": "normal",  "resets_at": "<ISO>",
      "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null }, "is_active": bool }
  ],
  "spend": { "used": { "amount_minor": int, "currency": "USD", "exponent": int }, "limit": null, "percent": int,
             "severity": "normal", "enabled": bool, "disabled_reason": null, "cap": null, "balance": null,
             "auto_reload": null, "disclaimer": "<string>", "can_purchase_credits": bool, "can_toggle": bool },
  "member_dashboard_available": bool,
  "seven_day_breakdown": { "as_of": "<ISO>", "window_started_at": "<ISO>",
                           "rows": [ { "key": "claude_code" | "chat" | "cowork" | "other", "display_name": "...", "percent": int } ] },
  "weekly_scoped_shares": [ { "limit_index": int, "used_percent_of_weekly": int, "allowance_percent_of_weekly": int } ]
}
```

How Claude Code reads it (local binary):

- **Units.** `utilization` is a percentage: `/usage` prints `` `${r}: ${Math.floor(l.utilization)}% used` ``. Community tools that once treated it as a 0-1 fraction were wrong (AnotherSava write-up above).
- **Rows shown in `/usage`.** "Current session" (`five_hour`), "Current week (all models)" (`seven_day`), "Current week (Sonnet only)" (`seven_day_sonnet`, Max/Team only), then one "Current week (<model>)" per `limits[]` entry with `kind==="weekly_scoped"` and a `scope.model.display_name`, filtered by a server-flag allowlist (`tengu_usage_overage_included_models`, default `["Fable","Fable 5","Fable 5.1"]`).
- **Classify on `kind`, not labels.** The SDK schema for `limits[].kind` says: "The server's meter kind, e.g. 'session', 'weekly_all' or 'weekly_scoped'. Classify a row on this, never on a label."
- **Shape check.** A body counts as a usage answer if it has any of `["five_hour","seven_day","seven_day_oauth_apps","seven_day_opus","seven_day_sonnet","cinder_cove","wattle_ember","extra_usage","limits"]` (`var Ow=[...]`). Every field is parsed as optional or nullable.

Mapping to the glossary (inference): **Session limit** = `five_hour`, or `limits[kind=session]`. **Weekly limit** = `seven_day`, or `limits[kind=weekly_all]`. **Model-specific weekly** = `limits[kind=weekly_scoped]` (today "Fable"), plus the legacy `seven_day_opus` / `seven_day_sonnet` when non-null. `seven_day_breakdown` gives a product split (Claude Code / chat / Cowork / other) that the tray could show for free.

### `/status`

`/status` does not show limit figures. The SDK `get_status` request "requests the rows of the terminal's /status screen (version, session, account, provider, model, servers, settings)" (local binary). Limits live under `/usage`, which merged `/cost` and `/stats` (changelog 2.1.118: "Merged `/cost` and `/stats` into `/usage`").

## 2. Rate limiting and Claude Code's own backoff

**What Claude Code does (local binary, `fetchUtilization`).**

- 401, or 403 with an Anthropic error body, counts as `auth_rejected`. That answer is remembered for 1 h (`Fw=3600000`) for that bearer.
- 429 or another 403 counts as `rate_limited`. It is remembered for `retry-after`, or 300 s if none (`Uw=300000`), capped at 1 h (`Tm=3600000`): `w=Math.min(P>0?P:Uw,Tm)`.
- On one 401 it refreshes the OAuth token and retries once ("401→refresh→retry").
- Reads are de-duplicated per org:account key (`` `${organizationUuid}:${accountUuid}` ``).

**What the changelog says (documented).**

- 2.1.116: the Settings Usage tab "no longer fails when the usage endpoint is rate-limited".
- 2.1.121: fixed "`/usage` returning 'rate limited' after a stale OAuth token".
- 2.1.208: `/usage` shows "last-known usage bars with an 'as of' note when the usage endpoint is rate-limited".
- 2.1.275: editor windows and non-interactive sessions on one machine share a read made in the last minute.
- 2.1.284: "Fixed repeated calls to the plan-usage endpoint after it rate-limits or rejects your login … now back off instead of re-asking".

Source: https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md. Anthropic is still tuning its own client against this endpoint's limits.

**What third parties hit (issue report / community).**

- **March 2026:** persistent 429s, at first with `retry-after: 0` or no header. See [#30930](https://github.com/anthropics/claude-code/issues/30930) (open, no Anthropic reply, opened 2026-03-05), [#31021](https://github.com/anthropics/claude-code/issues/31021), and [#31637](https://github.com/anthropics/claude-code/issues/31637) ("closed as not planned"). Comments on #30930 report a budget of about 5 requests per new access token before a 429.
- **User-Agent:** reports say a `User-Agent: claude-code/<version>` (or `claude-cli/<version> (external, cli)`) header gets a far more generous bucket ([#30930 comment](https://github.com/anthropics/claude-code/issues/30930), [Claude-Code-Usage-Monitor #202](https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor/issues/202), CodexBar). **Not confirmed here:** both live calls used a non-Claude User-Agent and returned 200. Impersonating Claude Code's User-Agent is a terms question for the terms ticket, not something to adopt by default.
- **`retry-after` values:** they do occur (832 s and 2449 s were observed), as do `0` and a missing header. Persist the deadline, cap it, and keep showing the last good sample ([AnotherSava write-up](https://github.com/AnotherSava/claude-code-common/blob/main/claude/learnings/anthropic-oauth-usage.md)).
- **Polling cadence in the wild:** every 10 minutes is "stable in practice" (AnotherSava). Every 180 s works with the Claude Code User-Agent (Usage-Monitor #202). ClaudeBar polls once a minute plus on menu open when stale ([ClaudeBar](https://gordonbeeming.com/projects/claude-bar)).
- **Shared token:** a VS Code tracker warns the limit is per OAuth token shared with Claude Code itself, so polling hard can starve Claude Code's own Account & usage panel ([CC Tracker](https://marketplace.visualstudio.com/items?itemName=MrN-studio.cc-tracker)).
- **Org pooling:** quota is pooled per `organizationUuid`. Accounts in one org showed identical figures; [#41886](https://github.com/anthropics/claude-code/issues/41886), closed as stale.

**Stability (inference from the above).** The endpoint is undocumented: none of the code.claude.com pages cite it, and an open feature request asks Anthropic to document it or ship a supported equivalent ([#99231](https://github.com/anthropics/claude-code/issues/99231), opened 2026-10-03). The path, auth headers and `five_hour` / `seven_day` fields have been stable since at least March 2026, judging by the issue reproductions. Additions are frequent and codenamed. Model-specific weekly data moved from fixed keys (`seven_day_opus`, `seven_day_sonnet`) to the generic `limits[]` array. Expect additive drift, and code for missing or `null` fields.

## 3. Alternatives

| Source | What it gives | Covers every Profile? | Model-specific weekly? | Fit with the safety bar |
|---|---|---|---|---|
| `GET /api/oauth/usage` (above) | Session, weekly, scoped weekly, extra usage, product split | Yes, one `user:profile` token per Profile | Yes (`limits[]`) | OK: token in Keychain, sent only to api.anthropic.com |
| Status line JSON `rate_limits` | `five_hour` / `seven_day` `used_percentage` (0-100) and `resets_at` (epoch s) | No: only the session's own login, only while a session runs, only after its first API response | No ([#91920](https://github.com/anthropics/claude-code/issues/91920), [#104823](https://github.com/anthropics/claude-code/issues/104823) open) | OK, but needs a status line hook to relay data out |
| `anthropic-ratelimit-unified-*` headers on `POST /v1/messages` | 5h / 7d / 7d_oi / overage utilization (0-1 fraction), reset (epoch s), status, representative claim | Yes, if a request is sent per Profile | No (only `representative-claim: seven_day_opus` as a hint) | Each probe is an inference request that counts against the limit |
| Agent SDK control request `get_usage` | Same data as `/usage`, incl. `model_scoped` | Only by spawning `claude` per Profile config | Yes | Experimental; heavy |
| Scraping `claude` `/usage` in a PTY | Text percentages | Only per spawned config | Yes | Fragile; CodexBar uses it as a last resort |
| Desktop app `~/Library/Application Support/Claude/plan-usage-history.json` | `{version: 2, samples: [{t, org, u: {fh, sd, xu}}]}` about every 15 min | Only orgs the desktop app has signed into | No ([#96716](https://github.com/anthropics/claude-code/issues/96716)) | Read-only use of an undocumented file; not secret storage |
| claude.ai web `GET /api/organizations/{org}/usage` | Same family of fields | Yes | Yes | **Excluded**: needs the `sessionKey` browser cookie (safety bar 2) |
| ccusage-style transcript counting | Token totals and estimated 5-hour blocks | Transcripts are shared, not split by Profile | No | Estimate only; not the server's percentage |

Evidence for each row:

- **Status line (documented).** `rate_limits` "appears only for claude.ai Pro and Max subscribers … and only after the first API response in the session. Each window … may be independently absent, and Claude Code drops a window once its `resets_at` time passes" (https://code.claude.com/docs/en/statusline). Added in 2.1.80 (changelog: "Added `rate_limits` field to statusline scripts"). In the local binary, the status line object is built from the header-derived state `u3()`: `five_hour:{used_percentage:fMt(ut.five_hour.utilization),resets_at:ut.five_hour.resets_at}`. An Enterprise login gets no `rate_limits` ([#96543](https://github.com/anthropics/claude-code/issues/96543)), and values drift across concurrent terminals ([#75408](https://github.com/anthropics/claude-code/issues/75408)).
- **Response headers (local binary).**
  - The parser reads `` `anthropic-ratelimit-unified-${s}-utilization` ``, `-reset` and `-surpassed-threshold` for `s` in `5h`, `7d`, `7d_oi`, `overage` (`var Tt=[["five_hour","5h"],["seven_day","7d"],["seven_day_overage_included","7d_oi"],["overage","overage"]]`), plus `anthropic-ratelimit-unified-status`, `-reset` and `-representative-claim`.
  - Claude Code has its own probe: `source:"quota_check"` sends `messages:[{role:"user",content:"quota"}]` with `max_tokens:1` and reads the headers off the response.
  - Community monitors use the same trick with a 1-token Haiku request when the usage endpoint is throttled (AnotherSava write-up).
- **`get_usage` (local binary).** "Requests the structured /usage data … Experimental — the response shape may change". It accepts `skip_behaviors` "for callers that need only the plan rate limits, such as a usage meter".
- **CLI PTY.** CodexBar's fallback order is "OAuth API (`oauth`) → CLI PTY (`claude`) → Web API (`web`)" ([providers.md](https://github.com/steipete/CodexBar/blob/main/docs/providers.md)).
- **Desktop history file (local).** The structure was read on this Mac: 552 samples, 4 distinct orgs, median gap 900 s, keys `fh` / `sd` / `xu` (values not printed).
- **claude.ai web endpoint (community).** Used with `Cookie: sessionKey=…` by CodexBar and, as a 429 workaround, by trackers in #30930.
- **ccusage (community).** "Block Start: Triggered by your first message … Lasts exactly 5 hours from start time" ([ccusage blocks](https://ccusage.com/guide/blocks-reports)). Users report hitting the limit while ccusage showed 72% ([ccusage #483](https://github.com/ccusage/ccusage/issues/483)). Claude Code's own transcript scan, the `/usage` "behaviors" section, is described as "Approximate, excludes other devices and claude.ai" (local binary).

## 4. Implications for the app (inference)

1. **Primary source:** `GET https://api.anthropic.com/api/oauth/usage` per Profile, with headers `Authorization: Bearer`, `anthropic-beta: oauth-2025-04-20` and `Content-Type: application/json`, and a 5-10 s timeout.
2. **Token:** an interactive-login token with `user:profile`. Never a `setup-token` token. How the app obtains and refreshes one token per Profile without disturbing the CLI's own Keychain item is the credentials ticket's question. Refreshing rotates the refresh token, so the app must not refresh the CLI's credential on its behalf.
3. **Parse:** `limits[]` by `kind` (`session`, `weekly_all`, `weekly_scoped` + `scope.model.display_name`), falling back to `five_hour` / `seven_day` / `seven_day_{opus,sonnet}`. Ignore unknown keys and treat every field as nullable. Use `utilization` for decimals and `percent` (int) when only `limits[]` is present. `resets_at` is ISO 8601.
4. **Poll and back off:** about every 5-10 minutes per Profile with jitter, plus on menu open when the sample is older than a minute or so. Mirror Claude Code: on 429 or a plain 403, wait for `retry-after`, or 300 s if none, capped at 1 h. On 401, mark the Profile "Sign in" and hold for an hour. Persist backoff deadlines and last good samples across restarts and show their age.
5. **Opportunistic freshness:** the Active Profile's status line JSON, or a desktop `plan-usage-history.json` sample, can update the tray between polls without extra requests. Both are optional, and neither covers model-specific weekly limits.
6. **Drift detection** (map: "Not yet specified"): flag a Profile when a 200 response lacks every key in Claude Code's own shape list.

## Open questions

- Is a non-Claude-Code User-Agent throttled harder today? Two calls succeeded, which is too few to say. Measuring it needs a deliberate, slow probe.
- Does the per-credential budget apply per access token or per org:account? Reports conflict (#30930 comments say per access token; #41886 says quota, not the endpoint budget, is per org).
- Whether polling this endpoint from a third-party app is within Anthropic's terms belongs to the terms ticket.
