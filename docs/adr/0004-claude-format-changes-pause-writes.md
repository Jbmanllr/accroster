# Claude format changes pause writes and degrade reads

The app relies on undocumented Claude formats: Sidebar entries and their folder layout, live-process files, Transcript records, the `get_usage` response, `.claude.json`'s `oauthAccount` and `cachedUsageUtilization`, the Profile directory links, and Claude Code honouring `CLAUDE_CONFIG_DIR`. Each parser checks the shape of the fields it uses every time it reads them. A self-check covers the same formats at launch, and again when the Claude Code or desktop version changes. When a check fails, a feature that writes (Sidebar sync, CLI switching, creating or repairing the Profile links, seeding a Profile's `.claude.json`) gets a Safety pause: it stops completely before any write, warns once, and resumes only when the user clicks after the checks pass again. Read paths degrade per item instead ("unknown", the dimmed last reading), so one moved field never blanks the menu.

## Considered Options

- **A known-good version allowlist.** Rejected: Claude Code ships several releases a week, so it would pause features after almost every update and still miss a format change made within a version.
- **Best-effort copying in Sidebar sync**, stopping only when a field the app uses moves. Rejected: a new top-level field the app doesn't know may be bound to an account, as `bridgeSessionIds` is, and copying it would spread that value to every Profile. Any field the app doesn't know pauses sync instead.
- **Copying only the fields the app knows.** Rejected: it drops data the desktop app may need.
- **Resuming automatically once checks pass.** Rejected: the first write after a format change is the risky one, so a person confirms it. There is no "sync anyway" override.

## Consequences

- Sync pauses after some desktop updates until the app learns the new field. This is the accepted cost of the policy.
- The desktop app's dormant multi-account mode has no on-disk signal; its gate and account set live in memory. Two proxies stand in for it, and either one triggers a Safety pause of sync:
  - a Sidebar entry written outside the desktop's Active Profile folder while Claude runs, not by sync and more than 30 s after the last account switch or after the watch started (a switch just before launch flushes the same way);
  - a pre-write check finding that a target folder changed since sync's own record of it.
- Unknown values fail toward caution:
  - an unknown live-process `status`, or a desktop Claude Code child process with no live-process file, means "busy" or "can't tell" for the Guided switch guard. "Can't tell" is looked at again for a few seconds (a child just started writes its file shortly after), then asks the user in the menu and in a notification. A live-process file's `procStart` is UTC (Claude Code runs `TZ=UTC ps -o lstart`); read in the Mac's time zone, every file looked stale on a Mac east of UTC and the guard could never tell;
  - an unknown limit `kind` is shown and counts toward the tightest limit.
- If a Profile directory's `auth status` identity disagrees with its `oauthAccount`, or every Profile reports the same identity, CLI switching gets a Safety pause: the pointer is frozen and the menu bar label shows `?`. While a self-check's `auth status` answers are still coming in (up to 30 s per Profile after launch), CLI switching waits the same way, with "checking…" in the menu header.
- The self-check also reads the `env` block of the shared `settings.json` and of managed settings, again whenever the shared file's mtime changes (checked on menu open and before each pointer write). The shim can't clear these values, and they apply under every Profile:
  - `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_OAUTH_TOKEN` or `CLAUDE_CODE_OAUTH_REFRESH_TOKEN` gives CLI switching a Safety pause: the first overrides the pointer, the others put one account behind every Profile's name;
  - `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN` or a provider variable only warns once that those Sessions don't use the Profile's subscription;
  - `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1` only warns once that a nested `claude` follows the CLI's Active Profile instead of its Session's.
- Only a replaced `projects/`, `file-history/` or `session-env/` link pauses the Profile links (no link is made or repaired in any Profile), and its Profile's row shows "Transcripts not shared". Other replaced links only warn in that Profile's details.
- A malformed `oauthAccount` pauses CLI switching and seeding `.claude.json`. The app's only other write to a Profile's `.claude.json`, putting its `oauthAccount` back after another Login's sign-in there was undone, waits on the same pause, and is never made over a file that isn't a JSON object.

## Amendment: new Sidebar entry fields no longer pause sync

A desktop update added a top-level entry field (`peerInbound`) and paused the owner's sync until a release learned it, which made sync unusable in practice. Unknown top-level fields no longer stop or pause Sidebar sync: entries are copied as they are, every field kept except the account-bound ones sync already clears. Diagnostics' Sidebar entries check names them in a passing note ("new fields seen: …") so the app can still learn them in a release. This takes the "best-effort copying" option rejected above, accepting that a new account-bound field could spread to other Profiles. Entries that aren't an object, are named unlike their `sessionId`, or can't be read are still skipped one by one, and the check still fails (and pauses sync while it's on) for the folder-layout problems. A pause left from an earlier version for this reason can be resumed at once, since the check now passes.
