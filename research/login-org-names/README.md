# Local files that name a Login and Organization before sign-in

Claude desktop 3.6556.2, Claude Code CLI 2.1.296, macOS, 2026-10-24. Read-only: nothing launched, quit, signed into or written under `~/Library/Application Support/Claude`, `~/.claude` or the Keychain. No token, cookie or Keychain value was read.

Question ([#19](https://github.com/Jbmanllr/claude-account-manager/issues/19)): for each account id / organization id the app finds on disk, which local files give the Login (email, account id) and the Organization (name, id, plan), without a token or a cookie?

Method:
- The account and org ids came from the Sidebar folders `claude-code-sessions/<accountUuid>/<orgUuid>/`. This Mac has 5 account folders, 5 distinct org ids and 11 account/org pairs.
- A scratch script labelled each id (`A1..A5`, `O1..O5`), then walked every candidate JSON file. It printed key paths, labels and the names of sibling keys, never values.
- Binary stores (logs, Local Storage, Session Storage, IndexedDB) were scanned only for counts of ids, email-shaped strings and the one known email / org name, plus the JSON key names next to a hit.
- Desktop code citations use the prettified `app.asar` chunk `jPKPzV-P` from the desktop-sync-facts research (same app version; that research describes the method). Line numbers refer to that output.

Labels: **Confirmed** = observed file or cited code. **Inferred** = reasoning.

Paths: `D/` = `~/Library/Application Support/Claude/`.

## Sources

| File | Field path | Maps | Coverage of other Logins on this Mac | Documented? | Safety bar |
|---|---|---|---|---|---|
| `~/.claude.json` (or `$CLAUDE_CONFIG_DIR/.claude.json`) | `oauthAccount.{accountUuid, emailAddress, displayName, fullName, organizationUuid, organizationName, organizationType, billingType, organizationRateLimitTier, userRateLimitTier, seatTier, organizationRole, workspaceRole, profileFetchedAt}` | account uuid → email, names; org uuid → org name, org type (e.g. `claude_max`), billing type, rate-limit tier | **Only the Login currently signed in to that config dir** (1 of 5). Replaced on `/login`, deleted on logout. | File location documented; `oauthAccount` fields not documented | OK: plain JSON, no secrets in this object |
| same file | `groveConfigCache.<accountUuid>`, `passesEligibilityCache.<orgUuid>`, `overageCreditGrantCache.<orgUuid>`, `s1mAccessCache.<orgUuid>`, `clientDataCacheSlots.*.org`, `cachedUsageUtilization.accountUuid`, `cachedArtifactRoster.{accountUuid, organizationUuid}`, `githubWebConnectionStatusCache.{accountUuid, orgUuid}` | ids only, no names. Keys left by earlier logins | 4 of 5 account ids, 5 of 5 org ids | No | OK |
| `~/.claude/backups/.claude.json.backup.<ms>` | `oauthAccount.*` | same as `oauthAccount` | 5 rolling backups, all minutes old, all the **current** Login | No | OK |
| `~/.claude.json.backup`, plus hand-made copies (`*.bak-<date>`, `*.rea.backup`, `*.before-*`) | `oauthAccount.*` | same as `oauthAccount` | The hand-made copies hold 2 **other** Logins (A1/O2 and A4/O4). They are not app-managed and exist only because someone copied the file. | No | OK, but incidental |
| `D/local-agent-mode-sessions/<acct>/<org>/local_<uuid>.json` (Cowork session record) | `accountName`, `emailAddress` | session's account → name, email | Only Logins that ran a Cowork session (1 pair here, the current CLI Login) | No | OK |
| `D/local-agent-mode-sessions/<acct>/<org>/local_<uuid>/.claude/.claude.json` | `oauthAccount.*` (no `fullName`, `organizationRole`, `workspaceRole`) | same as `oauthAccount` | Same single pair as the line above | No | OK |
| `D/local-agent-mode-sessions/skills-plugin/<orgUuid>/<accountUuid>/` | folder names | org ↔ account pair | 5 pairs (one per account) | No | OK |
| `D/config.json` | `lastKnownAccountUuid`, `bootSideSessions.accountUuid`, `dxt:allowlist*:<orgUuid>` | desktop's last account id; org ids. No email or name key exists in the file. | Last account only; 5 of 5 org ids as key suffixes | No | Read only these keys. The file also holds `oauth:tokenCache` and `oauth:tokenCacheV2`; do not read them. |
| `D/plan-usage-history.json` | `samples[].org` (+ `t`, `u.fh`, `u.sd`) | org uuid → usage samples. No name, no plan. | 5 of 5 orgs, 503 samples | No | OK |
| `D/remote-control-state.json` | `identities."<orgUuid>:<accountUuid>"` | org ↔ account pair, no names | 2 pairs | No | OK |
| `D/claude_desktop_config.json` | `preferences.*ByAccount.<accountUuid>`, `remoteChatFolder*.{accountUuid, orgUuid}` | ids only | 5 of 5 accounts | No | OK |
| `D/ccd-session-presets.json` | `byAccount.<accountUuid>` | ids only | 5 of 5 | No | OK |
| `D/ant-device-registry.json` | top-level keys = account uuids, string values | ids only | 5 of 5 | No | Key names only; values not read |
| `D/cowork-enabled-cli-ops.json`, `D/mcp-user-tool-toggles.json` | `ownerAccountId`, `owners.<accountUuid>` | ids only | 1 account each | No | OK |
| `D/git-worktrees.json` | none | contains no account or org id | none | No | OK |
| `D/buddy-tokens.json` | top-level key `tokens-today` | nothing identity-related seen | none | No | Holds counters or secrets; key names only |
| Sidebar entries `D/claude-code-sessions/<acct>/<org>/local_*.json` | no identity field among the 120+ top-level keys | the folder path is the only id link | all 11 pairs (by path) | No | OK |
| `~/Library/Logs/Claude/main*.log`, `remote-control.log`, `claude.ai-web*.log` | log text | account and org uuids appear thousands of times (all 5 / all 5 across the rotated files). **0 email-shaped strings; no `email`, `orgName` or `organization_name` keys; the known org name does not appear.** | all, but ids only | No | OK |
| `D/Local Storage/leveldb` | `antalytics_queue_v1` events: `accountUuid`, `organizationUuid`, `billing_type` | pair → billing type, no names | whichever pair was active when events queued (here only the desktop's current one) | No | Borderline: Chromium store, compressed blocks, format not meant for readers |
| `D/Local Storage/leveldb/000005.ldb` (oldest table) | an analytics "traits" record: `email`, `organization…uuid`, `billing_type`, a `…_plan` key, `created_at` | one account → email, org id, billing type, plan | 1 Login (the current CLI one), stale | No | Borderline, as above; may be compacted away at any time |
| `D/Session Storage`, `D/IndexedDB` | uuids inside per-account preference keys (`cmdk-navigation-history:<acct>:<org>`, …) | ids only; no names | partial | No | Borderline, as above |
| `~/.claude/telemetry/*` | `account_uuid`, `organization_uuid`, `user_type` | ids only | 3 accounts | No | OK |
| `~/.claude/cache/tok-*-ccd.json` | names suggest a token cache, keyed by org | — | — | No | **Do not read.** Existence noted only. |
| `D/Partitions/launch-preview-cowork-shared/Local Storage` | contains the current Login's email once | — | 1 Login | No | Out of bar: a web partition |

## Confirmed findings

1. **The only app-managed local source of names is `oauthAccount`, and it names only the Login currently signed in to that config dir.**
   - Fields: `accountUuid`, `emailAddress`, `displayName`, `fullName`, `organizationUuid`, `organizationName`, `organizationType`, `billingType`, `organizationRateLimitTier`, `seatTier`, `organizationRole`, `profileFetchedAt` (observed in `~/.claude.json`).
   - Its copies are the same Login: the 5 rolling backups in `~/.claude/backups/`, `~/.claude.json.backup`, and the Cowork VM's `.claude.json`.
   - Logout deletes `oauthAccount`, and a token blanked by `invalid_grant` keeps it (cc-usage-auth research, Q3/Q4).
2. **Names reach 3 of the 5 Logins and 3 of the 5 Organizations on this Mac, and 2 of those 3 only through hand-made copies of `.claude.json`.** No local file names the other 2 Logins, including the desktop app's current Login (`D/config.json` `lastKnownAccountUuid`), or their 2 Organizations.
3. **The desktop app keeps no account or org display info on disk.**
   - `D/config.json` holds only the account id: `lastKnownAccountUuid` is written by `wo.set(ASe, e.accountUuid)` (`jPKPzV-P` 145280–145283, with `ASe` at 28917). Observed keys hold no email or name.
   - The account object's schema has `displayName`, `fullName`, `emailAddress` and `accountUuid` (`jPKPzV-P` 43431–43435). It lives in a `DATAPROVIDER` in memory (43437+).
   - The browser's account list (`/api/auth/accounts`, giving `account_uuid`, `email_address`, `full_name`) is fetched with the session cookie and kept in memory (`jNn`, 144998–145017). It runs only when feature gate `96101707` is on (144985), and that gate is off today (desktop-sync-facts research).
   - The claude.ai import flow reads `organization.name` and `account.email_address` from a token response, also in memory (299998–300010).
4. **Logs never name a Login or an Organization.** `main*.log` and `remote-control.log` carry account and org uuids (pairs as `identity=<org>:<account>` in `remote-control.log`). They hold no email-shaped string and no name key. The one hostname hit is the machine name, not identity data.
5. **Org ↔ account pairing is available without names from several files:** the Sidebar folders (11 pairs), `skills-plugin/<org>/<account>` (5), `remote-control-state.json` (2), Local Storage analytics events, and logs.
6. **Plan without a token.**
   - `oauthAccount.organizationType` (`claude_max` on every copy here), `billingType` and `organizationRateLimitTier` (e.g. `default_claude_max_20x`) give the plan, but only for Logins that have an `oauthAccount` copy.
   - `subscriptionType` lives only in the Keychain credential.
   - `plan-usage-history.json` covers every org but carries no plan field.
7. **`claude auth status --json` gives `email`, `orgId` and `orgName` but no account id.** The three fields come from `oauthAccount`. `subscriptionType` comes from the credential (cc-usage-auth Q3, securestorage-config-dir §3). To join it to a folder's account uuid, read `oauthAccount.accountUuid` from the same config dir, or match on `orgId` plus the email.
8. **Personal-plan Organization names follow a default pattern.** All 3 `organizationName` values seen end in "Organization" (e.g. `<name>'s Organization`), so an org name alone may not tell two personal Logins apart.

## Inferred

- **Leftover id-keyed caches show an earlier CLI sign-in, not a name.** Keys such as `groveConfigCache.<account>` and `passesEligibilityCache.<org>` in the CLI's global `.claude.json` cover ids beyond the current Login. They suggest those Logins once signed in to that config dir.
- **`passesEligibilityCache.<org>` probably implies a paid plan for that org.** Guest passes are a paid-plan feature. This was not verified.
- **Each per-Profile `CLAUDE_CONFIG_DIR` the app creates will keep its own `oauthAccount`.** That would make names available for every Profile the app signs in, until a logout there deletes them. This follows from findings 1 and 7.
- **Local Storage records may be compacted away at any time.** The analytics traits sit in the oldest table (`000005.ldb`), and the queue is transient. Neither is a stable source.

## What this means for a greyed Sign in row (facts, not decisions)

- For a Profile found only in Sidebar folders, local files give the account uuid, the org uuid and the pairing. In most cases they give no email, org name or plan.
- A name exists without a token only where a `.claude.json` with `oauthAccount` for that account uuid exists: the current CLI Login, a Cowork VM copy, or an incidental copy. Every such copy carries `profileFetchedAt` as an "as of" time.
- Usage history (`plan-usage-history.json`) exists for every org, with no names.
- The account uuid is the join key that `oauthAccount` and the Sidebar folders share. `claude auth status --json` lacks it, so a merge with a signed-in identity goes through `oauthAccount.accountUuid` in that Profile's config dir, or through `orgId` plus the email.
- Readable without touching secrets: `.claude.json` files and their backups, `D/config.json` (named keys only), `plan-usage-history.json`, `remote-control-state.json`, and folder names. Local Storage, Session Storage and IndexedDB are Chromium internals and give no names beyond one stale record.
