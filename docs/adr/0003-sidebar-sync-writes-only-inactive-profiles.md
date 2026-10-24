# Sidebar sync copies entries into inactive Profiles only, and never deletes

Sidebar sync copies each existing Sidebar entry (`local_*.json`) into every other Profile folder under `claude-code-sessions/`. While the desktop app runs, it writes only to Profiles that are not the desktop's Active Profile: the app keeps no memory of those folders and reads the target folder fresh from disk on an account switch, so a copy written before the switch is picked up. The active Profile's own folder is written only by the app, and by sync only when the app is not running. Automatic mode mirrors the active Profile's folder shortly after each change; Manual mode is a "Sync now" button. Sync never needs to quit the desktop app. The setting defaults to Off.

## Considered Options

- **The official route only** (`claude --desktop --resume <id>`). Kept for opening a Session that has no entry in the Active Profile, but rejected as the sync mechanism: one Session at a time, a fresh entry with no title or archive state.
- **Sync with the app quit** ("Quit Claude, sync, reopen", or on quit). Rejected once the code showed inactive folders are safe to write: quitting interrupts any desktop turn in progress.
- **Creating entries for CLI-only Sessions.** Rejected: it means guessing at fields the app may later depend on; copying a real entry is what existing tools already do without reported problems.
- **Propagating deletes.** Rejected: the app refuses to reap a Transcript while another Profile holds an entry for it, so a propagated delete would strip that protection and let one click destroy the shared Transcript.

## Consequences

- A copy keeps the file name, `sessionId` and `cliSessionId`, and clears `bridgeSessionIds` and the connector snapshot (`remoteMcpServersConfig`, `enabledMcpTools`). Scratch, SSH/WSL and scheduled-task entries are skipped.
- A `deleted_*` tombstone only stops sync re-adding that Session to that Profile.
- Entries carry no per-field timestamps, so sync keeps its own record of each copy's title and archive state as of the last run; a copy that differs from the record changed and wins. If two changed, the active Profile's copy wins, else the newest file.
- If the desktop app's dormant multi-account mode is ever enabled, inactive Profiles may be written in the background: sync detects it, stops and warns.
- Every write follows the safety bar: backup, temp file then rename, abort on unknown files.
