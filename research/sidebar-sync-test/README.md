# Synced Sidebar entry test (2026-10-24)

One run of [synctest.py](synctest.py) for ticket 18. Claude desktop 3.6556.2, bundled CLI 2.1.308, macOS.

## What was done

1. Created a disposable Session in the desktop app (`~/Dev/sidebar-sync-test`) under the Active Profile A (`dce123fe…/0e000f92…`), sent one message. Entry `local_41704151…`, status `idle`.
2. With Claude running and A still active, `synctest.py prep`: tarred the target folder of Profile B (`4bc338e1…/d0ae4c16…`), saved the source entry, wrote a copy into B through a temp file and rename. File name, `sessionId`, `cliSessionId`, title and archive state kept; `bridgeSessionIds` (1 id) and `remoteMcpServersConfig` (2 connectors, ~10 KB) set to `[]`; `enabledMcpTools` was absent and left absent.
3. Owner did the guided switch by hand: sign out, sign in as B, opened the entry, sent "say hi again", got a reply; signed back in as A.
4. `synctest.py verify` plus `~/Library/Logs/Claude/main.log`.

## Results

- **Listed and resumed under B.** The copy appeared in B's Sidebar with its earlier exchange; the new turn appended to the same shared Transcript (`~/.claude/projects/-Users-jb-Dev-sidebar-sync-test/<cliSessionId>.jsonl`).
- **No connector errors.** On focus the app logged `replaceEnabledMcpTools … toolCount=33` and `replaceRemoteMcpServers … serverCount=15`, then `warmed successfully` and `healthy cycle`. The connector snapshot is rebuilt from the signed-in account each time a Session is focused.
- **Fresh bridge id.** Sending enabled remote control and the app wrote a new `bridgeSessionIds` entry, different from A's.
- **Source untouched by the copy or by B.** Only A's `lastFocusedAt` changed, at 22:39:20, after the switch back, when A's entry was focused (`replaceRemoteMcpServers … serverCount=2`, A's two connectors).
- **Fields the app changed in B's copy:** `bridgeSessionIds`, `remoteMcpServersConfig`, `enabledMcpTools`, `toolSurfaceSnapshot`, `completedTurns`, `postTurnSummary`/`postTurnSummaryFor`, `lastAssistantUuid`, and the activity timestamps (`lastActivityAt`, `lastFocusedAt`, `lastTurnEdgeAt`, `latestUserFrameAt`). Nothing else.
- **Noise during the switch, unrelated to the copy:** `oauth failed (not_logged_in detail=account changed mid-flow)` while signing in and out, and `Cannot update session: local_… not found` for other Sessions at 22:39:15.

## Field list for Sidebar sync

| Field | On copy | Why |
|---|---|---|
| `bridgeSessionIds` | clear | Remote-control binding of the source account; the app issues a new one |
| `remoteMcpServersConfig` | clear | Source account's connector ids and URLs; rebuilt on focus anyway |
| `enabledMcpTools` | clear | Same, keyed by the source account's connector ids |
| `toolSurfaceSnapshot` | keep | The app refreshed it itself; no error from the stale copy |
| everything else | keep | No error; app-owned counters and timestamps update on use |
