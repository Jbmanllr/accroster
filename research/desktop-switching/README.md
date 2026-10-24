# Desktop app: switching accounts and opening Sessions

Answers issue #7: what the Claude desktop app (macOS, Electron) allows for (a) changing the signed-in Login, (b) running more than one instance or profile, and (c) opening a given Session from outside, ranked against the map's safety bar (#1).

Researched 2026-10-24 on macOS with Claude desktop 3.6556.2 (`CFBundleVersion` in `/Applications/Claude.app/Contents/Info.plist`) and Claude Code CLI 2.1.296. Bundle evidence comes from an extracted copy of `Contents/Resources/app.asar`; minified file names (`index.chunk-*.js`) change on every release. Nothing was launched, no Login state was changed, no Session was created, and no cookie, token or secret-storage file was opened.

## Answer in brief

- **Login switching stays manual.** The only switch path in the app is sign-out then sign-in inside the claude.ai web view. The app learns the Active Profile from the web app over IPC and from the `lastActiveOrg` cookie. There is no URL parameter, deep link or login hint that picks a claude.ai account. The best a tray app can do safely is bring the app forward and tell the user which Profile to pick.
- **A native multi-account switcher is in the bundle, switched off.** It is gated by a server feature gate (`96101707`) and an admin policy `disableMultiAccount`, and it lists the browser's signed-in accounts from `claude.ai/api/auth/accounts`. Nothing outside the app can turn it on.
- **Second instances are possible but unsupported.** `open -n -a Claude --args --user-data-dir=<dir>` is a community workaround. Signed builds deliberately strip `CLAUDE_USER_DATA_DIR`, take no single-instance lock per profile, and route every `claude://` callback to one instance. Users report sign-ins landing in the wrong instance, and a Windows release stopped honoring `--user-data-dir`.
- **Opening a Session from outside works, and it writes to the Active Profile.** `claude --desktop --resume <id>` (documented) leads to the app's `claude://resume?session=<uuid>` handler. That handler writes a new Sidebar entry `local_<id>` stamped with the *Active Profile's* account and organization. It does not check whether another Profile already lists the Session. A Session from Profile B therefore gets a second Sidebar entry under Profile A.
- **For a Session already in the Active Profile's Sidebar, there is a read-only opener**, `claude://code/continue?session=local_<id>` (undocumented).

## (a) Changing the signed-in Login

### How the app knows who is signed in

| Mechanism | Evidence |
|---|---|
| The claude.ai web app reports the account to the main process over IPC. | Desktop log lines `[account] Account details received via IPC` and `[account] IPC did not provide accountUuid, falling back to bootstrap API` (`~/Library/Logs/Claude/main*.log`). |
| The Active Organization comes from the `lastActiveOrg` cookie in the app's own browser session. The main process watches it for changes. | `index.chunk-jPKPzV-P.js`: `defaultSession.cookies.on("changed",…t.name==="lastActiveOrg"…)` and `cookies.get({url:eA(),name:"lastActiveOrg"})` |
| Sign-out happens when the web view navigates to `/logout`. The main process mirrors that. | `index.chunk-jPKPzV-P.js`: `if(e==="/logout"\|\|e==="/logout/all-sessions"){… N.info("[account] Navigated to /logout, synthesizing logged-out")`. A second listener stops the Cowork VM on the same navigation (`[logout] User navigated to ${e}, stopping VM`). |
| Every switch on this Mac went through a logged-out state. | `main*.log`: one `Login-state transition (loggedOut: false → true …)` followed by `(loggedOut: true → false …)` per switch. Account ids are omitted here. |

Official guidance matches: the help center switches organizations of one email from the initials menu, and the desktop troubleshooting step is "Sign out and back in from the app menu" ([desktop docs](https://code.claude.com/docs/en/desktop#403-or-authentication-errors-in-the-code-tab); help center citations in [../claude-multi-account/90-official-sources.md](../claude-multi-account/90-official-sources.md#3-claude-desktop-app-code-tab)).

### Login hints and URL parameters

- `login_hint` and `domain_hint` exist only in the third-party (Vertex, Entra, OIDC) sign-in code, not in the claude.ai login. `index.chunk-jPKPzV-P.js`: `e.loginHint&&n.searchParams.set("login_hint",e.loginHint)` inside the generic OAuth helper. The `loginHint` setting is scoped `scopes:["3p"]` (`inferenceVertexOAuthLoginHint`).
- The claude.ai sign-in runs inside the web view. Desktop accepts sign-in callbacks only for flows it started. Google: `claude://login/google-auth?code=…` must match a pending sign-in, or the log says `"Google sign-in code does not answer a sign-in this app started; ignoring"`. Magic link: `claude://claude.ai/magic-link#<a>:<b>`. SSO: `claude://claude.ai/sso-callback`. Repeat deliveries are dropped (`ignoring repeat delivery of login callback`). A tray app cannot forge or pre-fill these, and must not try.
- No deep-link parameter selects an organization. The query keys passed through to the web app are connector OAuth results (`authed_mcp_server_id`, `oauth_error`, …). SSO callbacks get `code, state, error, error_description, organization_id`. All of them are callback data, not a switch command.

### `claude://` deep links (registered in Info.plist)

`CFBundleURLTypes` registers two schemes: `claude` and `msauth.com.anthropic.claudefordesktop` (Microsoft sign-in). The handler (`$z` in `index.chunk-jPKPzV-P.js`) dispatches on the URL host:

| Host | Paths handled | Relevance |
|---|---|---|
| `login` | `/google-auth` | Sign-in callback only |
| `claude.ai` | `magic-link`, `sso-callback`, `mcp-auth-callback`, `new`, `chat/<uuid>`, `project/<uuid>`, `settings`, `customize`, `code/…`, `epitaxy`, `cowork`, `local_sessions`, …; anything else is passed to the web app's router (`dispatchHandleDeepLink`) | No account switch. A link to `claude://claude.ai/logout` would reach the web router; what it does there is not verifiable from the bundle (unverified, and it would change Login state). |
| `code` | `/new` (prompt, folder, file, SSH), `/continue?session=last\|local_<id>`, `/needs-input`, `/search`, project and cloud-session links | Opening Sessions, see (c) |
| `resume` | `?session=<uuid>` | Imports a CLI Transcript, see (c) |
| `cowork`, `hotkey`, `sable-otter`, `preview`, `debug-handoff` | — | Out of scope |

Admin policy `disableDeepLinkRegistration` drops every link except sign-in callbacks (`claudeURLHandler: dropping deep link (disableDeepLinkRegistration)`). Its support entry is scoped to third-party installs.

### The dormant in-app multi-account switcher

`index.chunk-jPKPzV-P.js` contains:

- Admin policy `disableMultiAccount` ("Single account only"): `scopes:["1p"]`, `visibility:"public-undocumented"`, default `false`, available since `1.46388.1`.
- Capability `multiAccount:!K().authentication.disableMultiAccount&&Pb("96101707")?…:{status:"unavailable"}`. The feature turns on only when the server-side gate `96101707` is on.
- `jNn()`: `a.net.fetch(\`${eA()}/api/auth/accounts\`)`, which reads `account_uuid`, `email_address` and `full_name` for every account signed in to "this browser" (log text `could not learn this browser's accounts`). There is also a telemetry event `desktop_account_switch` with `previous_accounts_signed_in` and `code_sessions_running`.

So claude.ai is building multiple simultaneous sign-ins with an in-app switch. A user comment on [#18435](https://github.com/anthropics/claude-code/issues/18435) found the same flag and policy. The gate is server controlled. Flipping it locally would mean tampering with the app, which is outside the safety bar and Anthropic's terms. When it ships, the switch still happens inside the app's own UI. No external API for it was found.

## (b) More than one instance or profile

| Mechanism | What the bundle and reports show | Verdict |
|---|---|---|
| `CLAUDE_USER_DATA_DIR` env var | Signed builds delete it before use unless a developer-signed `CLAUDE_CDP_AUTH` token checks out. `index.pre.js`: `E.app.isPackaged&&!J9&&xZ(process.env)`, with `xZ` running `delete e.CLAUDE_USER_DATA_DIR`. Elsewhere: `"[user-data-dir] CLAUDE_USER_DATA_DIR is set and signed builds ignore it … this launch uses the default data folder"`. | Not available |
| `--user-data-dir=<dir>` (Chromium switch) | It is not on the app's list of refused switches (`remote-debugging-port`, `host-rules`, `ssl-key-log-file`, …), and the app checks `commandLine.hasSwitch("user-data-dir")`. When `userData` is not the default, the app sets `localPairingDisabledReason = "userData relocated"`. `requestSingleInstanceLock()` is called only in the Reset App Data path, so no per-profile single-instance lock exists. Users run `open -n -a "Claude.app" --args --user-data-dir=…` on macOS ([#20131](https://github.com/anthropics/claude-code/issues/20131), [Stracker 2026](https://philippstracker.com/multiple-claude-instances)). Reported problems: both instances share bundle id `com.anthropic.claudefordesktop` and one `claude://` callback, so a sign-in can land in the wrong instance ([#18435 comment](https://github.com/anthropics/claude-code/issues/18435), [#98549](https://github.com/anthropics/claude-code/issues/98549)). Windows MSIX 2.42304.0 stopped honoring the switch ([#103068](https://github.com/anthropics/claude-code/issues/103068), open). Not tested here. | Works today on macOS per users, undocumented, fragile |
| `open -n` without a new data dir | With no single-instance lock, a second process starts on the *same* profile folder (#18435 comment; consistent with the code above). Two writers on one Electron profile and one Sidebar index. | Unsafe |
| `CLAUDE_CONFIG_DIR` in the launch environment | Documented for Desktop local sessions: set it, then "quit and reopen Desktop" ([desktop docs](https://code.claude.com/docs/en/desktop#environment-variables-in-local-sessions)). That gives one config directory per running app, not per account. It also moves Transcripts away from `~/.claude/projects`, which breaks the map's standing decision that Transcripts stay shared. | Conflicts with the map |

A second instance also splits the Sidebar further: each data dir has its own `claude-code-sessions/<account>/<org>/` tree.

## (c) Opening a Session from outside

### `claude --desktop --resume <id>` and `/desktop` (documented)

[Desktop docs, "Coming from the CLI?"](https://code.claude.com/docs/en/desktop#coming-from-the-cli):

- `/desktop` in the CLI "saves your session and opens it in the desktop app, then exits the CLI". It needs a Claude subscription sign-in in the CLI (no API key, Bedrock, Vertex or Foundry).
- `claude --desktop --resume <session-id>` needs CLI v2.1.285 or later. It prints `Opening session <session-id> in Claude Desktop`. A session name does not work in place of the ID. It "doesn't move a session that is open in another terminal or still running in the background".
- "Desktop continues the same session rather than a copy."

CLI 2.1.296 contains the flag help ("Open in the Claude Desktop app instead of the terminal (with --continue or --resume <id> to pick the session)"). It also contains a deep-link opener that checks for `/Applications/Claude.app` and a minimum Desktop version `1.1.9669`, then calls AppleScript `open location`. The exact URL string is not readable in the compiled CLI. The only Desktop handler that imports a CLI Transcript is `claude://resume?session=<uuid>`, so the CLI almost certainly sends that URL (inference).

### What `claude://resume?session=<uuid>` does (bundle)

Handler, `index.chunk-jPKPzV-P.js`: `case uu.Resume:{let e=a.searchParams.get("session");return e&&Hz.test(e)?(N.info(\`Resume deep link: importing CLI session ${e}\`), … t.importCliSession(e) …`. `Hz` is a UUID regex. The import is `adoptCliSession` in `index.chunk-DNPOm2Zc.js`:

1. It needs a signed-in account. Otherwise it fails with "Unable to import session: account information is unavailable because your sign-in has expired. Please sign in to the desktop app again." The user-facing string is "Couldn't open that session. Sign in to the desktop app and try again."
2. If `local_<id>` already exists **in the Active Profile's loaded sessions**, it unarchives that entry and opens it: `CLI session ${e} already imported as ${c}`.
3. Otherwise it finds the Transcript, reads its `cwd` and **trusts that folder without a dialog** if it is not trusted yet (`saveWorkspaceTrust(_,{withoutDialog:!0})`). It touches the Transcript's mtime and registers a git worktree if the folder is one.
4. It creates the Sidebar entry `{sessionId:"local_<id>", homeTenancy:{accountId:currentAccountId, orgId:currentOrgId}, cliSessionId:<id>, …, adoptedFromOtherSurface:!0}`. The log says `Imported CLI session ${e} as Desktop session ${c}`. It aborts if the account changes mid-import (`the account changed during the import`).

So **yes: it creates a Sidebar entry under the desktop app's Active Profile**, whichever Profile the Session came from. No step looks at other Profiles' `claude-code-sessions/<account>/<org>/` folders. A Session that Profile B already lists gets a second entry under Profile A. The new entry has the history and the mode carried in the Transcript, but not B's title edits, archive state, permission grants or connector config. The desktop string for recovered sessions says the same: "They keep their history and permission mode (except bypass permissions), but not archive status or permission grants."

### Other openers

| Opener | Effect | Evidence |
|---|---|---|
| `claude://code/continue?session=local_<id>` or `session=last` | Opens an existing, non-archived entry of the Active Profile. Writes nothing. Ignored when signed out or when entry points are off. | `Orr=/^local_[A-Za-z0-9-]{1,64}$/`; `Lrr` filters `getAllSessions()` by `!e.isArchived` and `sessionId===e`; `lnr` requires `accountUuid` and not `isLoggedOut`. Undocumented. |
| `claude://code/new?folder=<path>&q=<prompt>` | New-session page in the Active Profile | Documented for SSH links ([desktop docs](https://code.claude.com/docs/en/desktop#open-an-ssh-session-from-a-link)) |
| Desktop `/resume` picker (user action) | Lists CLI Transcripts not claimed by any Profile's index. Desktop-created Transcripts are skipped unless they predate the index. Another Profile's desktop Sessions are not offered. | `scanRecoverableCliSessions` → `cU(…, ()=>this.getAccountIds())`. For records outside the Active Profile, `XH` adds both `lineage` and `live` ids to the known set, and the scan log reads "desktop-stamped transcripts re-admitted only if started before …". |
| `claude://claude.ai/code/<cse_…\|session_…>` | Cloud or Remote Control sessions | Out of scope (map) |

## Ranking against the safety bar

Safety bar: no cookie or secret-storage access; undocumented writes need a backup, an app-not-running check and a way to undo them.

**Switching the desktop Active Profile**

1. **Guided manual switch (recommended).** The tray brings Claude forward (`open -b com.anthropic.claudefordesktop`) and names the Profile to choose. The user signs out and in, or picks the organization from the initials menu. Reads no secrets and writes nothing. To detect the result without cookies, watch which `claude-code-sessions/<account>/<org>/` folder gets new writes, or read `Login-state transition` lines in `main.log`. Both are heuristics and need drift detection.
2. **Adopt the native multi-account switcher when Anthropic ships it.** It is the supported path, but it is server-gated and its UI lives in the app.
3. **`--user-data-dir` second instance.** Touches no secrets directly, since each instance keeps its own cookies. It is undocumented and already regressed on Windows. Callbacks go to the wrong instance, local pairing is disabled, and the Sidebar splits further. At most an opt-in power-user feature with clear warnings.
4. **Rejected:** setting `lastActiveOrg` or other cookies; swapping or copying `~/Library/Application Support/Claude` (Cookies, Local Storage, `config.json` token cache); flipping gate `96101707` or `disableMultiAccount`; forging sign-in callbacks; `CLAUDE_CONFIG_DIR` relaunch (splits Transcripts); bare `open -n` (two writers on one profile).

**Opening any Session in the desktop app**

1. **Session already in the Active Profile's Sidebar:** `claude://code/continue?session=local_<id>`. Read-only, but undocumented. If it stops working, fall back to (2).
2. **CLI Session, or one from another Profile:** `claude --desktop --resume <cliSessionId>`. Documented, and the app does the write itself. Side effects to warn about: a duplicate Sidebar entry under the Active Profile, the folder trusted without a dialog, and refusal while the Session runs elsewhere. It also needs the CLI signed in with a subscription. Prefer it over calling `claude://resume` directly, which is the same import without the CLI's running-session check.
3. **Writing Sidebar entry files directly** (Sidebar sync) belongs to the Sidebar ticket. Even with backups it duplicates what `adoptCliSession` already does safely while the app runs.

## Open points

- The exact URL the CLI opens for `--desktop --resume` (inferred to be `claude://resume?session=<id>`). This could be confirmed without side effects from the CLI's debug log line `Opening deep link:` the next time the user runs it.
- What the claude.ai web router does with `claude://claude.ai/logout` and other unknown paths (remote code, not in the bundle).
- `--user-data-dir` on macOS 3.6556.2 was not tested, because a test would create a second signed-out profile.
