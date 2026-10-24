# Where banked limit resets and their expiry come from

Answers [issue #17](https://github.com/Jbmanllr/claude-account-manager/issues/17) for the map in [issue #1](https://github.com/Jbmanllr/claude-account-manager/issues/1).

Researched 2026-10-24 on macOS. Sources:

- **Claude Code CLI 2.1.296**, at `~/.local/bin/claude` -> `~/.local/share/claude/versions/2.1.296`. This is a Bun single-file binary; its bundled JS was searched with `strings` and Python.
- **Claude Code 2.1.308**, the copy the desktop app bundles at `~/Library/Application Support/Claude/claude-code/2.1.308/…/claude`. Used to check that nothing moved.
- **Claude desktop app 3.6556.2**: `/Applications/Claude.app/Contents/Resources/app.asar`, plus the file names and key shapes of non-secret JSON under `~/Library/Application Support/Claude`.
- **CodexBar** at `main` `d34fbb47` (2026-10-24).
- **The Claude Help Center.**

No network call was made with a token, and no token, cookie or value was printed. The only usage data read locally was the key list and the `null`/non-null type of two keys in `~/.claude.json` → `cachedUsageUtilization`.

Labels: **Local binary** (code in the installed bundle; identifiers are minifier names), **Local file**, **Documented** (support.claude.com), **Community** (CodexBar), **Inference**.

## Summary

- **The server feature exists.** Anthropic's help article "What is a limit reset?" (updated 2026-09-22) describes resets "given occasionally to eligible plans". One refills the five-hour session limit or the weekly limit at once, without moving the weekly reset day. It is redeemed with **Reset for free** in Settings > Usage on the web or desktop, and an unused one expires at the time shown on the offer. Our glossary calls it a **Banked reset**.
- **The data is the `cedar_ember` block of `GET /api/oauth/usage`.** Claude Code and CodexBar parse the same schema: `eligible`, then `grants[]` with `resets_left`, `starts_at` / `ends_at` (ISO 8601, `null` = unbounded), `clears` (the limit types a grant refills), `paused` and `usable_now`, plus `next_grant_id`, `weekly_resets_at` and `cooldown_until`.
- **The block is filled on demand.** Claude Code asks for it only with `?cedar_ember=1&skip_spend=1` (the `/limit-reset` command, used away from a limit) or `?at_wall=1&skip_spend=1` (when a limit is hit). CodexBar always adds `?cedar_ember=1`. The plain read carries the key too: on this Mac it is `"cedar_ember": null`.
- **`get_usage` does not ask for it.** The SDK `get_usage` control request always reads the plain variant and has no option to choose another (2.1.296 and 2.1.308). It does pass the whole body through as `rate_limits`, so `rate_limits.cedar_ember` reaches the caller, but only as whatever the plain read holds (`null` here).
- **Desktop app local files hold nothing.** `plan-usage-history.json` stores only utilization (`fh`, `sd`, `xu`, …). The Electron shell has no `cedar_ember` code. The Reset for free UI is claude.ai web content served remotely.
- **Within the safety bar, a Banked reset is not reliably reachable today.** The only sources that carry it are a direct `?cedar_ember=1` call with the Profile's OAuth token, which ADR 0001 rules out, and the claude.ai cookie endpoint, which safety bar 2 excludes. `/limit-reset` is interactive, flag-gated, and can *spend* the reset.
- **Which limit:** per grant, from `clears`. `five_hour` is the Session limit. `seven_day` / `seven_day_overage_included` are the Weekly limit. `seven_day_opus` / `seven_day_sonnet` are model-specific weekly limits. CodexBar's fixture uses `["five_hour","seven_day"]`. A reset never changes the weekly reset day.
- **There is a second, different program, `juniper_tide`.** It is a recurring "reset your session limit now · uses weekly limit · 1/week". It has no expiry and is not banked: it reports `available` and `next_available_at`. It is a separate concept, not a Banked reset.

---

## 1. What a limit reset is (documented)

[support.claude.com/en/articles/17007452-about-limit-resets](https://support.claude.com/en/articles/17007452-about-limit-resets), "What is a limit reset?", last updated 2026-09-22:

- **What it does:** it sets usage limits back to full when used. "Depending on the reset", it refills the five-hour session limit *or* the weekly limit. Weekly limits still reset on their usual day and time.
- **Where:** Settings > Usage on the web or in Claude Desktop, under Resets, with a **Reset for free** button. The same button appears on the message shown at a limit. Use is not limited to being at a limit, and it can't be undone.
- **Not on mobile or in Claude Code:** the button is "not currently available" on Claude Mobile or in Claude Code (terminal or IDE). Limits are shared, so a reset applies there too.
- **Expiry:** an unused reset expires on the day and time listed on the offer, and the date is shown in Settings > Usage.
- **Loss:** downgrading or cancelling before use forfeits it.

The Claude Code changelog up to 2.1.310 does not mention `/limit-reset` or saved resets ([CHANGELOG.md](https://raw.githubusercontent.com/anthropics/claude-code/main/CHANGELOG.md)).

## 2. The `cedar_ember` block (local binary, Claude Code 2.1.296)

### How it is fetched

```js
var Ww={plain:"/api/oauth/usage",
        at_wall:"/api/oauth/usage?at_wall=1&skip_spend=1",
        cedar_ember:"/api/oauth/usage?cedar_ember=1&skip_spend=1"};
```

These go to the same `api.anthropic.com` host with the same `Bearer` + `anthropic-beta: oauth-2025-04-20` request as the plain read (see [usage-limits research](https://github.com/Jbmanllr/claude-account-manager/blob/research/usage-limits/research/usage-limits/README.md)). The status reader is:

```js
async function Be(a,{read:s="at_wall"}={}){ … let l=await eD(a,{read:s}); …
  return{kind:"answered",status:Qn(l?.cedar_ember)} }
```

- At a limit, Claude Code reads `at_wall`.
- The `/limit-reset` command's "anytime" path reads `cedar_ember`: `lr` → `Be(s,{read:"cedar_ember"})`.
- No other caller uses the `cedar_ember` variant (`read:"cedar_ember"` occurs once).
- The whole feature is gated by the server-side flag `tengu_cedar_ember` (`hle(){return Wfe().enabled===!0}`). On this Mac the flag is absent from `cachedGrowthBookFeatures` in `~/.claude.json`, so it is off.

### Schema (Zod, as Claude Code parses it)

Status block (`Xn`):

| Field | Type | Notes |
|---|---|---|
| `eligible` | bool, **required** | A block without it is "malformed" and ignored |
| `ineligible_reason` | enum, nullable, optional | `config_off`, `tier`, `seat`, `mobile`, `surface`, `cli_version`, `no_grant`, `tenure`, `other_experiment`, `unavailable`, `unknown` |
| `at_limit` | bool, optional | |
| `exhausted` | limit-type[], optional | Limits currently at 100 % |
| `grants` | object[], optional | Each parsed separately; a malformed grant is dropped |
| `next_grant_id` | string `^[a-z0-9_-]{1,40}$`, nullable | Kept only if it matches a grant's `id` |
| `weekly_resets_at` | string, nullable | ISO 8601; shown as "your weekly reset day stays {week}" |
| `cooldown_until` | string, nullable | ISO 8601 |
| `event_props` | object, nullable | Analytics: `surface`, `tier` (`claude_pro`, `claude_max_5x`, `claude_max_20x`, `claude_team`), `tenure_bucket`, `billing_path`, `billing_period`, `extra_usage_state` |

Grant (`Bn`):

| Field | Type | Notes |
|---|---|---|
| `id` | string `^[a-z0-9_-]{1,40}$`, required | The redemption handle. CodexBar never decodes or stores it |
| `label` | string (default `""`) | |
| `resets_total` | int ≥ 0 (default 0) | |
| `resets_left` | int ≥ 0, **required** | |
| `starts_at` | string, nullable, optional | ISO 8601 (`Date.parse`) |
| `ends_at` | string, nullable, optional | **The expiry.** ISO 8601, `null` = no expiry |
| `clears` | limit-type[] | Which limits it refills |
| `paused` | bool (default false) | |
| `usable_now` | bool (default false) | |
| `use_requires_limit` | bool (default **true**) | When true, it can only be used at a limit it clears |
| `percent_used` | `{limit-type: int 0-100}` | Used for "You still have {percent}% of your {limit} left — use your reset anyway?" |
| `blocking` | limit-type[] | Copy: "Your reset doesn't refill your …, so it can't be used until that resets" |

Limit types (`Le`): `five_hour`, `seven_day`, `seven_day_overage_included`, `seven_day_opus`, `seven_day_sonnet`, `seven_day_cowork`, `seven_day_omelette`, `seven_day_oauth_apps`. Unknown values are filtered out.

### How Claude Code reads it

- **Available.** `zLo` treats a reset as available when it is `eligible` and some grant has `resetsLeft>0 && !paused && (ends_at ?? ∞) > now`.
- **Count.** `Ue` gives the count shown as "{resets} left": the sum of `resets_left` over grants.
- **Expiry.** It shows the expiry as "use by {date}", formatted from `ends_at`.
- **Offered grant.** It offers `next_grant_id` only when that grant is `usable_now`, not paused and not expired, and, at a limit, only when its `clears` includes the limit that was hit.
- **Limit names.** It maps `clears` to names (`ar`): `five_hour` → "session"; `seven_day` and `seven_day_overage_included` → "weekly"; `seven_day_opus` / `seven_day_sonnet` → the Opus/Sonnet limit name. The other three are not named.

Copy strings (`Bh`):

- "Use your reset now ({left} left, until {date})"
- "/limit-reset to refill your limits · {resets} left · use by {date}"
- "Refills your {limits} now · your weekly reset day stays {week}"

### Redeeming (write path: the app must never call it)

```js
zt.post(`/api/organizations/${org}/reset_rate_limits`,
        {program:"cedar_ember", grant_id, request_id})
```

The response has `result` (`reset`, `already_used`, `not_limited`, `cooldown`, `ineligible`, `unavailable`), plus `reason`, `resets_left`, `cleared[]`, `weekly_resets_at` and `cooldown_until`. `/limit-reset` (`type:"local-jsx"`, `isHidden:!0`, enabled only when `tengu_cedar_ember` or `tengu_nifty_lemur` is on) asks "Use your reset?" before it posts. Driving it from the app risks spending a user's reset, and it is interactive only.

## 3. What `get_usage` returns (local binary)

- **The request.** The schema is `{subtype:"get_usage", skip_behaviors?: bool}`, with no variant selector. This is unchanged in 2.1.308.
- **The read.** The handler (`Ghn` → `bXn` → `R`) calls `eD(n)` with the default `read:"plain"`.
- **The answer.** It returns `rate_limits: {...body, model_scoped}`. The typed schema lists only `five_hour`, `seven_day`, `seven_day_oauth_apps`, `seven_day_opus`, `seven_day_sonnet`, `model_scoped` and `extra_usage`, but the implementation spreads the whole plain body. That is why the [cc-usage-auth research](https://github.com/Jbmanllr/claude-account-manager/blob/research/cc-usage-auth/research/cc-usage-auth/README.md) saw the full body.
- **Cache.** The same plain body is cached in `.claude.json` → `cachedUsageUtilization` (Zod `.passthrough()`, so every key is kept).

**Local file.** On this Mac, `~/.claude.json` → `cachedUsageUtilization` was fetched 2026-10-24T18:24Z. It has 31 top-level keys, including `cedar_ember` and `juniper_tide`, and **both are `null`**.

**Inference.** Two readings fit this:

1. the server fills `cedar_ember` only when asked (`?cedar_ember=1` or `?at_wall=1`);
2. or this account is outside the program (flag off, no grants).

The opt-in query parameter, Claude Code's own split, and CodexBar's always adding `?cedar_ember=1` favour reading 1. It is **unverified**: telling them apart needs a plain read from an account that has a banked reset.

## 4. Desktop app (local binary and local files)

- **`app.asar` 3.6556.2:** 0 matches for `cedar_ember`, `reset_rate_limits`, `juniper_tide` or `Reset for free`. The shell's `[plan-usage]` reader calls `${host}/api/organizations/${org}/usage` (optionally `?skip_spend=1`) with the web session. `[ccdPlanUsage]` calls the plain `/api/oauth/usage`. Neither asks for `cedar_ember`.
- **`plan-usage-history.json`:** `{version: 2, samples: [{t, org, u}]}`, 552 samples, with `u` keys `fh`, `sd`, `xu`. The shell's key map (`five_hour:"fh", seven_day:"sd", seven_day_opus:"so", seven_day_oauth_apps:"oa", seven_day_cowork:"cw", …`) has no reset field.
- **Other JSON files:** the other non-secret JSON files under `~/Library/Application Support/Claude` (`ccd-session-presets.json`, `remote-control-state.json`, `window-state.json`, `ccd-ids.json`, `vm-support-probe.json`, `sentry/scope_v3.json`) have 0 matches for `cedar|reset_grant|limit_reset|reset_rate|juniper`. `config.json`, cookies, Local Storage and IndexedDB were not opened (secret storage).
- **Where the UI lives:** the Reset for free UI is part of claude.ai's Settings > Usage page, which the desktop app loads remotely, so nothing is persisted locally.

## 5. CodexBar (community)

- **Docs:** [docs/claude.md](https://github.com/steipete/CodexBar/blob/main/docs/claude.md) ("Limit Reset Credits").
- **OAuth source:** `GET https://api.anthropic.com/api/oauth/usage?cedar_ember=1` returns "usage and saved reset inventory". On 400/403 it retries once without the parameter.
- **Web source:** `GET https://claude.ai/api/organizations/{orgId}/usage?cedar_ember=1` with `Cookie: sessionKey`.
- **Counting:** the count is the sum of `resets_left` over grants that are not paused, have started and have not expired, and requires `eligible: true`. `usable_now` is not consulted.
- **Verification:** observed on personal Pro/Max accounts; "Team and Enterprise organizations are not verified".
- **Redeeming:** CodexBar never redeems a reset.
- **Parser** ([ClaudeRateLimitResetCredits.swift](https://github.com/steipete/CodexBar/blob/main/Sources/CodexBarCore/Providers/Claude/ClaudeRateLimitResetCredits.swift), added 2026-09-27 for the Web source and 2026-10-03 for OAuth):
  - `starts_at` / `ends_at` are ISO 8601. "An absent or null bound is open; a supplied but unreadable bound is malformed."
  - At most 50 resets and 200 grant records are accepted.
  - The fixtures use `"clears": ["five_hour", "seven_day"]` and `"ends_at":"2099-01-01T00:00:00Z"`.
- **Not the CLI:** CodexBar's CLI source does not report saved resets.

## 6. The other program: `juniper_tide` (local binary)

This is a separate status block, also read from `at_wall`, and a separate flag, `tengu_nifty_lemur` (`{enabled: false}` on this Mac). Its fields:

- `eligible`, `ineligible_reason`
- `in_experiment`, `arm` (`control` | `reset`), `available`
- `next_available_at`, `weekly_resets_at` (ISO 8601)
- `resets_per_week` (default 1)

It is claimed with `POST …/reset_rate_limits {program:"juniper_tide"}`. Its copy is "Reset your session limit now · uses weekly limit · 1/week", and "Session limit reset · next reset available {date} · your weekly limit still applies". It has no grant and no expiry: it is a weekly allowance, not a saved reset. If the app ever shows it, it needs its own glossary term.

## 7. Options for the app (inference)

| Option | Reaches Banked resets? | Safety bar |
|---|---|---|
| `get_usage` per Profile (ADR 0001) | Only if the plain read ever carries a non-null `cedar_ember`. Unverified, and `null` on this Mac | OK |
| Spawn `claude` and drive `/limit-reset` | Reads it, but interactive only, flag-gated, and one keypress from spending it | Unacceptable |
| App calls `/api/oauth/usage?cedar_ember=1` with the Profile's token | Yes | **Breaks ADR 0001** (the app would handle tokens) |
| claude.ai `/api/organizations/{org}/usage?cedar_ember=1` | Yes | **Excluded** (session cookie) |
| Desktop app local files | No | — |
| Upstream: ask Anthropic to add a variant or `cedar_ember` to `get_usage` | Would make it reachable | OK |

Recommendation:

1. **Parse defensively for free.** Parse `rate_limits.cedar_ember` from `get_usage` with Claude Code's schema, and show "N banked resets · use by {soonest `ends_at`}" plus the limits named by `clears` when it is non-null and `eligible`.
2. **Otherwise show nothing.** Do not say "none": `null` means "unknown", not "zero".
3. **Never redeem.** Link to claude.ai Settings > Usage instead.
4. **Track upstream.** Treat reliable display as blocked on Claude Code exposing the `cedar_ember` read through `get_usage`.

## Open questions

- Does the plain `/api/oauth/usage` read fill `cedar_ember` for an account that holds a banked reset? Verifying it needs such an account, and a `get_usage` call on it that prints only the block's type.
- How do Team and Enterprise organizations behave? CodexBar has verified only personal Pro/Max accounts.
- Should the map accept "Banked resets: shown only opportunistically", or treat it as a feature request to Anthropic (a `get_usage` option)?
