# The CLI switches Profile through a PATH shim, not by swapping credentials

A `claude` shell script in the app's own `bin/` directory, put first on PATH by a marked block at the end of `~/.zshrc`, reads a pointer file holding the CLI's Active Profile directory. It then `exec`s the real Claude Code with `CLAUDE_CONFIG_DIR` set to that directory. A switch rewrites only the pointer. A running Session read its environment once at launch, and its credential is that directory's own Keychain item, so no switch can reach it. Each Profile directory links back to `~/.claude` for Transcripts, the data `--resume` needs, and the user's own configuration, so a Session started under one Profile resumes under any other.

## Considered Options

- **Swap the shared Keychain item and `oauthAccount`** (claude-swap, CCSwitcher, caam). Rejected: Claude Code re-reads the credential on a 30 s cache expiry, on refresh and on 401, so running Sessions move accounts mid-turn.
- **`CLAUDE_SECURESTORAGE_CONFIG_DIR` inside one shared config.** Rejected: the single shared `oauthAccount` mislabels every Profile but the last one written.
- **A zsh function or `launchctl setenv`.** Rejected: the function reaches only interactive shells, and `launchctl` reaches only GUI apps launched later.
- **Replace `~/.local/bin/claude`.** Rejected: the native updater owns that symlink; replacing it disables version cleanup and makes `claude doctor` warn.

## Consequences

- A `CLAUDE_CONFIG_DIR` already in the environment wins over the pointer. That is the per-shell override, and it keeps a `claude` started inside a Session on that Session's Profile. The shim always unsets `CLAUDE_SECURESTORAGE_CONFIG_DIR`.
- The IDE extensions, the desktop app and anything not started from a zsh shell bypass the shim and use `~/.claude`.
- Profiles are linked only where Claude Code's writes keep the link, checked against the 2.1.296 bundle:
  - **Directories** (every writer is safe): `projects`, `file-history`, `tasks`, `plans`, `session-env`, `skills`, `agents`, `commands`, `hooks`, `output-styles`, `plugins`.
  - **Files** (written through the link, or never written by Claude Code): `settings.json`, `CLAUDE.md`, `keybindings.json`.
- Kept per Profile:
  - `.claude.json`: written by temp file and rename, and holds the identity.
  - `history.jsonl`: pruning skips a symlink.
  - Caches, logs, the live-process registry, shell snapshots and the MCP needs-auth cache.
- Plugin index files store the config directory as it was spelled when written, so a shared `plugins/` holds whichever Profile wrote last. Before relying on it, an implementation spike must confirm that installing and updating plugins works across two Profiles. If it doesn't, `plugins/` becomes per Profile.
