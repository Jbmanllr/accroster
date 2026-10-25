# Claude Code holds every credential; the app reads limits through it

The app never holds an OAuth token. Each Profile is its own `CLAUDE_CONFIG_DIR`, signed in with `claude auth login`, and its credential is the Keychain item Claude Code creates for that directory. To read a Profile's limits, the app spawns Claude Code in that directory and sends it the `get_usage` control request. Refresh tokens are single-use, so any second refresher logs the CLI out, and Anthropic's terms forbid third parties from collecting, storing or intermediating claude.ai tokens. With this design, Claude Code is the only process that reads, refreshes or sends a token.

## Considered Options

- **Read the access token from the Keychain and poll `/api/oauth/usage` directly.** Rejected: the app would handle tokens, which the terms lean against.
- **Status-line `rate_limits` only.** Documented, but it only carries figures for a Profile with a running Session, so idle Profiles would never show live limits.
- **`claude -p "/usage"`.** Kept as the fallback if `get_usage` changes. It also makes no model request, but it scans the last 7 days of Transcripts on every call and returns fewer fields.

## Consequences

- `get_usage` still calls an undocumented endpoint, which is grey under the terms, and the SDK typings mark the method experimental. The app tolerates both: every field is optional, and drift falls back to `/usage`.
- Each usage read is a short Claude Code process, about 1.5 s from spawn to exit. It writes `cachedUsageUtilization` to that Profile's `.claude.json`.
- The usage spawn passes `--settings '{"disableAllHooks":true}'` and runs from a neutral working directory. Checked against 2.1.310: without the flag the spawn runs the user's, the project's and every enabled plugin's `SessionStart` hooks and the `SessionEnd` hooks; with it, none of them run and the reply is unchanged. Hooks from managed settings were not tested and may still run.
- A Profile directory's path string is part of its credential's identity, because the Keychain slot is a hash of the literal path. Directories are named by a fixed id and never moved or renamed.
- Adding and removing a Profile go through `claude auth login` and `claude auth logout`, so a sign-in always needs a person in the browser.
