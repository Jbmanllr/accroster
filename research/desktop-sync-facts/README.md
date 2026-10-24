# Desktop app facts: cross-Profile writes, switching, mid-turn detection, entry fields

Claude desktop 3.6556.2, bundled CLI 2.1.308, macOS, 2026-10-24. Read-only: nothing launched, quit or written under `~/Library/Application Support/Claude`.

Method: extracted `app.asar` into `scratchpad/asar`, then pretty-printed chunks with `prettier@3.3.3 --parser babel` into `scratchpad/pretty`. Line numbers refer to that output (the same convention as the sidebar-entries research). Chunks used:
- `DNPOm2Zc`: LocalSessionManager
- `DBIg6EBr`: the session-manager base class and the entry serializer
- `jPKPzV-P`: main process and account state

Labels: **Confirmed** = cited code or observed file. **Inferred** = reasoning.

## 1. Writing `local_*.json` into another Profile's folder while the app runs

**Confirmed: the app holds no in-memory state for non-active Profiles.**
- `getStorageDir()` resolves only to `<currentAccountId>/<currentOrgId>` (`DNPOm2Zc` 79335–79344).
- The `sessions` map holds only the active Profile's entries. On switch it is cleared (`DBIg6EBr` 17177–17179, `this.sessions.clear()`).
- `writeSessionToDisk` writes only when `held(id) === session` (`DNPOm2Zc` 80451, 80460). `held` = `sessions.get ?? parked.get` (58364–58365), so a session from the previous Profile cannot be saved after the switch.

**Confirmed: the app does touch other Profiles' folders, in these ways.**
- **Read-only scans** (already known): the reap claim scan and the worktree orphan collector.
- **Tombstone removal across orgs.** Importing a CLI session removes `deleted_<id>` from every org folder of the *same account* (`FH`/`NH`, `DNPOm2Zc` 45740–45760). If Profile B is another org of the same account, its `deleted_*` files can change while A is active. `local_*.json` files are never touched by this.
- **Parked sessions (dormant).** `storageDirFor(id)` returns the *parked* session's own account folder (`DNPOm2Zc` 53650–53660), and `retireParked` flushes to it (79345–79351). So a Profile that is no longer active can keep receiving writes. Parking happens only when `knt(account) && Mnt() > 1` (79172–79195). Those are `gge`/`hge` (`jPKPzV-P` 16413–16418): the size of the multi-sign-in account set. That set holds more than the active account only when feature gate `96101707` is on (`jPKPzV-P` 144917–144935, `kNn` 144979+). The gate is off today, so no parking happens. If it ships, writes to a Profile that is still parked are unsafe.

**Confirmed: switching to B loads B fresh from disk.**
`doInitialize` runs, in order (`DBIg6EBr` 17163–17195):
1. `flushPendingSaves()` (A's debounced saves plus the `archived-sessions.idx` hint, `DNPOm2Zc` 80395–80400)
2. `onAccountOrgChanged`, which sets `sessionRecordsLoaded=false`, bumps `sessionLoadEpoch`, detaches the archived hint and clears file caches (`DNPOm2Zc` 79232–79241)
3. `sessions.clear()`
4. set the current ids to B
5. `loadSessions()`, which does a fresh `readdir` of B's folder (`DNPOm2Zc` 79487–79491) and drops results from a stale epoch (79619)

Entries written before the switch are therefore picked up.

**Inferred: a write that races the readdir during a switch can be missed.** Write atomically (temp file plus rename): the cross-Profile readers skip or fail-safe on unreadable files.

**Confirmed: sign-out alone does not flush A's entries.** The logout branch only sets `sawLogoutSinceInit` and calls `onAccountLoggedOut` (`DBIg6EBr` 17055–17067). That hook clears GitHub autofix state and proxied connector calls, nothing else (`DNPOm2Zc` 79155–79159). The current ids stay A, so sessions that are still running keep saving to A's folder while the app is signed out. The flush happens at the next sign-in that changes account or org (step 1 above) and at quit (77869).

**Inferred: A's folder is fully up to date once B's sign-in starts initializing, not at the moment of sign-out.**

**Verdict (inferred from the confirmed code above).** Writing into B's folder while A is active is safe for `local_*.json` with today's gate state, provided:
- each write is atomic;
- the write finishes before the user switches to B;
- `deleted_*` in B's folder is not relied on when B is another org of the same account.

Writing into the *active* Profile's folder is still unsafe.

## 2. Does sign-out or account switch stop sessions or interrupt a turn?

- **Sign-out only: no (confirmed).** No teardown is in the logout path (citations above). The account-signed-out token purge (`Rjn`/`dNn`, `jPKPzV-P` 143430, 144777, 144941) clears desktop token caches only. The `credential_revoked` teardown (`DBIg6EBr` 17287–17305) fires only from the third-party-inference `revokeInteractiveAuth` (`jPKPzV-P` 138931–138940), not from claude.ai sign-out.
- **Inferred:** a long turn could later fail if the CLI's OAuth token needs a refresh while the app is signed out.
- **Signing back in to the same account and org: no kill (confirmed).** The ids are unchanged, so the code takes the "reload" branch (`DBIg6EBr` 17196–17206). Already-loaded ids are skipped by the loader.
- **Switching to a different account or org: yes (confirmed).** `onAccountOrgChanged` calls `teardownSession(id, "account_switch")` for every session (`DNPOm2Zc` 79227–79230). For a live query that goes to `teardownQuery` and then `e.query?.close()` (25049), which ends the `claude` child process. A turn in flight is cut:
  - it is recorded as `lastCliExit.turnInFlight` (25041–25046);
  - the "stopped" event and analytics carry `had_pending_cycle` (84008–84030);
  - queued unsent input is kept across the loss for later (`keepInputAcrossLoss(e,"account_switch")`, 24978–24983, and `settleKeptInputForAccountSwitch`).
- **The only exception is the dormant multi-account mode** (gate `96101707`, more than one signed-in account). Sessions whose restart would lose work are *parked* and keep running (79170–79195).

## 3. Detecting that a desktop Code session is mid-turn

**Recommended signal: the CLI's own registry file `~/.claude/sessions/<pid>.json` (confirmed, observed plus CLI strings).**
- Every desktop child writes one, with these fields:
  - `pid`, `procStart`
  - `entrypoint: "claude-desktop"`
  - `hostSessionId` = the desktop `local_…` id (the app sets `CLAUDE_CODE_HOST_SESSION_ID`, `DNPOm2Zc` 91730)
  - `sessionId` = `cliSessionId` (6 of 6 matched entries on this Mac)
  - `status`, `statusUpdatedAt`
- In CLI 2.1.308, `status` is:
  - `"waiting"` with `waitingFor` (`"input needed"`, `"dialog open"`) when the CLI is blocked on a dialog or input;
  - otherwise `isLoading || delegatedActive ? "busy" : "idle"`.
  - Writer `Yze` stamps `statusUpdatedAt` on each status change.
- Observed live: 4 `busy`, 2 `idle`.
- How to read it:
  - **busy** = a running process (`kill -0 pid`, with `procStart` matching `ps -o lstart`) whose status is `busy`;
  - **stale** = a dead pid;
  - **not running** = no file for that `hostSessionId`.
- **Inferred caveats:**
  - The file is undocumented and its format can change, so fail closed: treat an unknown status as busy.
  - Desktop permission prompts are answered through the SDK, so the CLI may report `busy` rather than `waiting` while it waits for approval.

**Weaker signals:**
- **Process tree (confirmed).** The children are `disclaimer --pgroup -- …/Claude/claude-code/<ver>/…/claude --output-format stream-json …`, with the app as parent. A warm child exists whenever the session is open, even when idle (an idle timeout pauses it later). Presence means "running", not "mid-turn". CPU usage is a noisy heuristic.
- **Transcript `.jsonl` mtime.** It goes quiet during long tool calls or a slow model response. Inferred: false negatives.
- **Logs (`main.log`).** They have no per-turn start/end line. Observed `turn starts` 7 times against far more turns. Unreliable.
- **Entry JSON (confirmed).** It has no running or status field: the serializer `PM` (`DBIg6EBr` 21150–21330) persists no `isRunning`. `lastActivityAt`, `lastTurnEdgeAt` and `latestUserFrameAt` are timestamps only. The file is rewritten from memory, so it lags.

## 4. Title, archive state and timestamps in a `local_*.json` entry

**Confirmed: where title and archive state are stored.**
- Serializer fields (`DBIg6EBr` 21183–21290): `title`, `titleSource` (`user`/`tool`/`auto`), `titleTurn`, `titleOffers`, `previousTitles` (a list of strings, no times, `RO` 19318), `isArchived` (boolean). Also `lastFocusedAt`, `createdAt`, `lastActivityAt`, `lastTurnEdgeAt`, `indexedAt`, `errorAt`, `recapAt`, `interruptedByQuitAt`.
- `archived-sessions.idx` is only a load-order hint, rebuilt from memory.

**Confirmed: renaming changes no timestamp.** `updateSession` → `NJ` (`DNPOm2Zc` 94352–94389, 58714–58741) sets `title`, `titleSource`, `previousTitles` and `titleTurn = completedTurns`, then saves.
- `NJ` also calls `syncTitleToCli` → the SDK's `renameSession` (58665–58695).
- Inferred: that writes the title into the transcript too, which carries its own entry timestamp.

**Confirmed: archiving changes no timestamp.** `archiveSession` → `teardownSession("archive")` sets `isArchived = true` and saves (84052–84062). It does not touch `lastActivityAt` or `lastFocusedAt`.

**Confirmed: unarchiving sets `lastFocusedAt = now`** (89466–89473) and `autoArchiveExempt`. It does not set `lastActivityAt`.

**Confirmed: what bumps `lastActivityAt`.** Turn and message activity, rewinds, and stop/interrupt (for example 36887, 58054, 58177, 58339, 80725, 85432).

**Confirmed: no per-field modification time and no `archivedAt`/`titleUpdatedAt`/`updatedAt`** (field census of 566 entries on this Mac, plus the serializer).

**Inferred: how to pick the newest copy.**
- `lastActivityAt` orders conversation progress (and carries the newest `cliSessionId`). It cannot order rename or archive changes.
- `max(lastActivityAt, lastFocusedAt)` plus file mtime is the best available proxy. File mtime changes on every save, including the app's rewrites from memory, so it is not specific to a field.
- To merge title and archive correctly, keep a sync-side record of each field's last value per Profile and three-way diff it against the copy synced last time.
