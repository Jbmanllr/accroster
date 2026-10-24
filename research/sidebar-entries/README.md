# How the desktop app treats Sidebar entries it did not create

Answers [#8](https://github.com/Jbmanllr/claude-account-manager/issues/8). Researched 2026-10-24 on macOS with Claude desktop 3.6556.2 and Claude Code CLI 2.1.296. Builds on [03-sessions-desktop.md](../claude-multi-account/03-sessions-desktop.md) and [07-regrouping-options.md](../claude-multi-account/07-regrouping-options.md).

The question: if a `local_*.json` Sidebar entry from Profile A is placed in Profile B's `claude-code-sessions/<account>/<org>/` folder, what does the app do with it, and can Sidebar sync by file writes meet the safety bar?

## Answer

- **The app lists the entry.** On load it reads every `local_*.json` in the signed-in Profile's folder, and nothing in the file ties it to an account. The Profile comes from the folder path. The app does not delete or rename an entry it did not create, and it does not check it against a server.
- **The app rewrites entries from memory.** It does not watch the folder. It reads entries when it loads a Profile (at launch or on account switch), then saves its in-memory copy whenever session state changes. A write made while the app is running is either never seen or overwritten.
- **The app already reads other Profiles' entries, read-only.** It scans every `<account>/<org>` folder to see which transcripts other Profiles still claim. A transcript claimed elsewhere is never deleted, and the app's own CLI-session import picker refuses it. So a copy in B keeps A's transcript safe when the session is deleted in A, and the reverse holds too.
- **Nothing is mirrored server-side, apart from Remote Control.** The Sidebar is built only from these files. `bridgeSessionIds` points at claude.ai Remote Control copies owned by the source Profile's account, and archiving or deleting the entry calls claude.ai with the signed-in Profile's token. Pins and stars live in the shared `claude_desktop_config.json`, keyed by `sessionId`.
- **Verdict: Sidebar sync by file writes can meet the safety bar under strict conditions.** The desktop app must not be running, a backup must be taken first, and the copy must be a whole new file. Keep `sessionId`, `cliSessionId` and the file name. Clear `bridgeSessionIds`. Skip scratch, SSH/WSL and scheduled-task entries. Respect `deleted_*` tombstones. Copies drift apart once both Profiles use them, so sync has to be repeated, with newest-wins.
- **An official route exists for one session at a time.** `claude --desktop --resume <cliSessionId>` (or `/desktop` inside a CLI session) makes the signed-in Profile create its own fresh entry for an existing transcript. It works even when another Profile already has an entry for it. Titles, archive state, PR links and permissions do not carry over.

Labels: **Bundle** = read in the app's bundled JavaScript (cited as chunk file and line; see [Method](#method)). **Observed** = read-only inspection of this Mac's files. **Documented** = Anthropic docs. **Report** = a GitHub issue or community tool. **Inference** = my reasoning.

## 1. Loading: what the app reads and checks

**Bundle.** `loadSessionRecords` (`DNPOm2Zc` 79475–79720):

1. `getStorageDir()` = `userData/claude-code-sessions/<currentAccountId>/<currentOrgId>`. If the folder is missing, it loads zero sessions and does not create the folder.
2. It lists the folder and keeps names that start with `local_` and end with `.json`. Anything else in the folder (`deleted_*`, `archived-sessions.idx`, `scheduled-tasks.json`, `waiting-input/`, `backlog/`) is handled separately.
3. For each file:
   - skips it if it is over 10 MB (counted as skipped) or empty;
   - `JSON.parse`s it, and a parse error only logs `Failed to load session from …`;
   - skips it if its `sessionId` is being deleted or is already loaded.
4. It normalizes the record. There is no schema check, no signature and no account check.
5. It sets `homeTenancy = { accountId: currentAccountId, orgId: currentOrgId }` **in memory** (79662) and stores the session under the JSON's own `sessionId`, not the file name.
6. Some loads rewrite the file straight away. Example: a side session that never started is re-saved as archived (79646–79679).

**Observed.** Across the 558 entries on this Mac:

- none persists `homeTenancy`;
- every `sessionId` equals its file name stem;
- no entry contains another Profile's account or org id.

The only own-id mentions are in `cwd`, `originCwd` and `gitAnchorsFolderRealpath` of the 20 "No folder" scratch sessions, whose working folder is `scratch-workspaces/<account>/<org>/…`.

**Inference.** A copied entry is indistinguishable from a native one. It must keep `sessionId` equal to its file name, otherwise the next save writes a second file named after the JSON's `sessionId`.

**Archived hint.** `archived-sessions.idx` (`{"v":1,"archived":["local_…"]}`, ids must match `^local_[A-Za-z0-9_-]+$`) only defers archived entries to a second load pass (`DNPOm2Zc` 73368–73420, 79571–79576). If it is unreadable, the app logs `archived-sessions hint unreadable … loading without it`. After loading, the app rebuilds the hint from memory (79774–79787). A copied entry missing from the hint simply loads in the first pass, so the hint does not need to be edited.

## 2. Writes, deletes and tombstones

**Bundle.**

- **Saves overwrite the file.** `saveSession` writes the in-memory record to `<storageDir>/<sessionId>.json` (`getSessionFilePath`, 79362). The app has no watcher on this folder: the only `.watch(` hits in the chunk are unrelated. Changing a file while the app runs is therefore unsafe. [claude-transplant](https://github.com/vitaliyhayda/claude-transplant) says the same from its own testing: "Do it only while Desktop is closed, it rewrites records it has open from memory."
- **The `cliSessionId` pointer moves.** It changes on `/clear` and on rewind or fork (`DNPOm2Zc` 58312–58316, 53799–53830). It is set to `undefined` when a resume finds no transcript (`clearStaleResumeHandle`). The previous ids go to `priorCliSessionIds`. A session marked missing gets `transcriptUnavailable: true` and is re-saved (52234–52250, 63655–63673).
  - Inference: a copy can point at an older transcript after the other Profile rewinds or clears.
- **Delete writes a tombstone.** It writes `deleted_<transcript id>` into the Profile's org folder (`MH`, 45730–45739, called at 84510).
  - The CLI-session list hides tombstoned ids, but only for the signed-in account (`zH`, 45792–45836).
  - Importing a session removes its tombstones in every org folder of the current account (`FH`, 45758–45760, used at 45929 and 46118).
- **Delete can also remove the transcript ("reap").** Before it does, it calls `cliSessionIdsClaimedOnDisk()` (90171–90219). That function reads the `local_*.json` of **every** `<account>/<org>` folder, plus sibling app profiles, and refuses to release or reap a transcript that any of them claims: `is claimed by a session of another account or profile on this computer` (90156–90160, 84522–84523).
  - If those records cannot be read, it leaves the transcript on disk (90211–90213).
- **Worktree cleanup checks every Profile too.** The worktree orphan collector reads every Profile's entries before removing anything, and discards nothing if one is unreadable (`aU`, 46607–46691).

**Inference.** Deleting one Profile's copy does not destroy the transcript the other Profile's copy uses. This is the strongest reason file-based sync is not inherently destructive.

## 3. Ownership across Profiles: the app expects one owner

**Bundle.** `getPersistedMarkers` (`zH`) collects every `cliSessionId`, `unarchivedCliSessionId`, `priorCliSessionIds` entry and file stem across **all** account and org folders (`recorded`). The desktop's CLI-session discovery (`Bs1Y7Yn0` 343–395) uses it:

- the Sidebar's CLI list and the `/resume` picker leave out every recorded id;
- `resolveForResume` refuses a recorded id with `Failed` (`Bs1Y7Yn0` 419–423).

**Inference.** The in-app import cannot be used to bring Profile A's sessions into Profile B. The design assumes one Sidebar entry per transcript, but the code tolerates several (Section 2). Copies are outside the intended model, so the app has no logic to merge or deduplicate them. That is why community tools report that copies "drift apart" ([claude-transplant's comparison table](https://github.com/vitaliyhayda/claude-transplant)).

## 4. Fields that are bound to an account

| Field | Seen on this Mac | Why it matters in another Profile | Sync action |
|---|---|---|---|
| file name, `sessionId` | 558/558, always equal | Pins and stars in shared `claude_desktop_config.json` (`preferences/epitaxyPrefs/starred-local-code-sessions`, `pinnedOrder`), renderer Local Storage, worktree leases in `git-worktrees.json` (`leasedBy`) and `spawnedFrom`/`forkedFromSessionId` links are keyed by it | **Keep** |
| `cliSessionId`, `priorCliSessionIds` | 552 have one | Join key to the shared transcript; also what protects the transcript from reaping (Section 2) | **Keep**; refresh from the newest copy |
| `bridgeSessionIds` | 554/558 non-empty | Remote Control copies on claude.ai owned by the source account. Archiving or deleting the entry calls claude.ai with the **signed-in** Profile's OAuth token for these ids (`cleanupRemoteBridgeSessions`, `DNPOm2Zc` 84555–84613, called at 84115–84120) | **Clear** to `[]` (claude-transplant does this, `transplant.js` line 2079) |
| `remoteMcpServersConfig`, `enabledMcpTools` | 558/558 non-empty (`uuid`, `name`, `url`, `tools`) | Snapshot of the source account's claude.ai connectors. Used to create remote MCP servers when the session starts (`DNPOm2Zc` 37380–37394); the renderer then pushes the signed-in account's connectors (`applyRemoteMcpServers`, 95347–95370), and connectors are reconciled at spawn (53339–53348) | Keep or clear (see [Open questions](#open-questions)). Clearing is safer: it avoids naming another account's connectors under this Profile |
| `cwd`, `originCwd`, `gitAnchorsFolderRealpath` | 20 contain `scratch-workspaces/<account>/<org>/` | Scratch folders are per Profile and out of scope | **Skip** these entries |
| `sshConfig`, `wslConfig`, `sshRemote*` | 12 | The app treats remote mirrors specially when scanning claims | **Skip** |
| `scheduledTaskId`, `notifySessionId` | 0 here | Tied to the Profile's `scheduled-tasks.json` (claude-transplant moves task families as a whole) | **Skip** |
| `homeTenancy` | never persisted | Set from the folder at load | Nothing to do |

**Observed.** Every other field (title, archive state, PRs, permissions, model, effort, …) is local data with no account id in it. Keeping it is what makes a synced entry useful.

## 5. Is the index mirrored anywhere else?

- **No server copy of the Sidebar.** **Bundle:** the list comes only from the folder scan in Section 1. Server calls tied to entries go to Remote Control bridge sessions (Section 4). **Report:** [restore-desktop-sessions](https://github.com/arunmauryaaa/restore-desktop-sessions) found the list "is *not* stored in the `claude.ai` IndexedDB / LevelDB".
- **Observed, ids found elsewhere (read-only scan, counts only).** Outside `claude-code-sessions`, `local_<uuid>` ids appear in:
  - `claude_desktop_config.json`: pins, stars, last-open session (shared by all Profiles);
  - `Local Storage` and `Partitions`: renderer UI state, already holding ids from 4 different Profiles;
  - `git-worktrees.json`: worktree leases;
  - `side-session-notes/`;
  - `IndexedDB`: 1 id.

  None of these is the index. All of them are already shared across Profiles. Keeping the same `sessionId` in copies means stars, pins and worktree leases follow the session.

## 6. What users who copied entries report

- **[restore-desktop-sessions](https://github.com/arunmauryaaa/restore-desktop-sessions)** (linked from [anthropics/claude-code#48511](https://github.com/anthropics/claude-code/issues/48511)). It copies `local_*.json` unchanged with `cp -n` into the signed-in Profile's folder, after a backup and a full quit. It reports that the sessions are listed and that resume works.
- **[claude-transplant](https://github.com/vitaliyhayda/claude-transplant)** (#48511). It **moves** instead of copying, so that only one Profile lists each session.
  - It keeps ids and `cliSessionId` and clears `bridgeSessionIds`.
  - It refuses sessions owned by a running worker or tied to scheduled tasks.
  - It only writes while Desktop is closed, or it restarts Desktop with approval.
  - It verifies transcripts before and after, and offers undo.
  - It reports that continued sessions work under the new account.
- **[ccdejavu](https://github.com/wjames111/ccdejavu)** (#48511). It runs a background two-way sync of `local_*.json` and `deleted_*` markers across linked Profiles, newest copy wins, with a backup and atomic renames.
  - It syncs about 2 s after a change while the app may be running, and asks for a Cmd+Q to see changes.
  - Given Section 2, its writes to the signed-in Profile's folder can be overwritten by the running app.
- **No report found of the app deleting, rejecting or flagging copied entries.** The related reports are about pointers decaying in place, not copies:
  - [#92825](https://github.com/anthropics/claude-code/issues/92825): `cliSessionId` nulled and `transcriptUnavailable`, with similar rates on both accounts of a two-account Mac;
  - [#100364](https://github.com/anthropics/claude-code/issues/100364): old entries without a `cliSessionId` show "Session not found on disk";
  - [#91017](https://github.com/anthropics/claude-code/issues/91017): the index is not rebuilt when the folder is removed, which matches the "folder missing → load nothing" branch in Section 1;
  - [#101545](https://github.com/anthropics/claude-code/issues/101545): `local_` id versus `cliSessionId` in "Copy link".

## 7. Official route that creates entries

- **Documented.** `/desktop` (alias `/app`) in a CLI session, or `claude --desktop --resume <session-id>` (`--continue` for the latest in the folder), opens that session in Desktop. "Desktop continues the same session rather than a copy, so `claude --resume` in the terminal still finds it afterwards." `--resume` here takes only an id. Both need a Claude subscription on macOS or x64 Windows, and the `--desktop` flag needs Claude Code v2.1.285 or later. Sources: [Desktop: coming from the CLI](https://code.claude.com/docs/en/desktop#coming-from-the-cli), [CLI reference](https://code.claude.com/docs/en/cli-usage), [Commands](https://code.claude.com/docs/en/commands).
- **Bundle.** The CLI hands off through the deep link `claude://claude.ai/resume?session=<uuid>` (`jPKPzV-P` 162880–162916, route enum at 44055). That link calls `importCliSession` with no options. `adoptCliSession` (`DNPOm2Zc` 45850–46124):
  - creates a new in-memory entry `local_<cliSessionId>` under the **signed-in** Profile;
  - fills it with `cwd` from the transcript, a title from the transcript tail and default permissions;
  - removes this Profile's tombstones for that id.

  It checks only this Profile's loaded sessions, deletes in flight and Cowork ids. It does **not** check `recorded`, unlike the in-app picker (Section 3).

  **Observed:** 4 entries here have `sessionId == local_<cliSessionId>`, the shape this path creates.
- **Inference.** For one session at a time, this is a supported way to give Profile B its own entry for a Profile A transcript, with no undocumented write. It does not carry over title, archive state, PRs, stars or permissions. It needs the CLI and the desktop app signed in, and the session closed in the terminal. It cannot bulk-sync.

## 8. Conditions for file-based Sidebar sync to meet the safety bar

Safety bar item 3 says every write to an undocumented file is preceded by a backup and an app-not-running check, and is reversible. Sidebar sync meets it if all of these hold:

1. **Desktop is not running** (no `Claude` process) for the whole write. The app rewrites from memory and does not reread (Section 2). The new entries appear on the next launch or account switch.
2. **Back up every target `<account>/<org>` folder** before writing. Record each file the sync creates, so undo removes exactly those and restores overwritten ones.
3. **Write each file to a temp name, then rename it into place.** Never edit a file in place. The loader treats a half-written file as a failed load (Section 1).
4. **Copy whole entries, changing only `bridgeSessionIds` → `[]`** (and optionally `remoteMcpServersConfig`/`enabledMcpTools`). Keep the file name, `sessionId`, `cliSessionId` and lineage fields.
5. **Skip entries** that are scratch (`cwd` under `scratch-workspaces/`), SSH/WSL, tied to scheduled tasks, oversized (>10 MB) or unparsable. Skip any `sessionId` already present in the target with different content unless newest-wins is applied deliberately.
6. **Respect deletes.** Do not copy an entry into a Profile whose folder has `deleted_<stem or cliSessionId>`. If deletes are propagated, move files to a trash folder rather than unlinking them.
7. **Resolve drift on every run.** Pick the copy with the larger `lastActivityAt`, which carries the latest `cliSessionId` after rewind or `/clear` (Section 2). Never overwrite a newer copy with an older one.
8. **Fail safe on format drift.** Abort if a target folder holds files the sync does not recognise, or if an entry lacks `sessionId`/`cwd`. ccdejavu does the same. The format is undocumented and the bundle chunk names change every release.

Things this cannot guarantee, to carry into the spec:

- Archive and delete state stays per Profile unless the sync propagates it.
- A session can be continued from two Profiles in turn. Each continuation is billed to and sent to the signed-in account; claude-transplant's FAQ notes that personal history moved into a Team org becomes that org's data. This is a terms and data-ownership choice, not a file-safety issue.

## Open questions

- Whether keeping another account's `remoteMcpServersConfig` causes visible connector errors on first resume before the renderer replaces it. The code path suggests a brief attempt with stale connector uuids. Needs one controlled test with a disposable session, which this research did not run (no writes to real folders).
- Whether `/desktop` refuses when the session is open in Desktop under another Profile. The docs only say it "doesn't move a session that is open in another terminal or still running in the background."

## Method

- Copied `/Applications/Claude.app/Contents/Resources/app.asar` (version 3.6556.2) to a scratch directory and extracted it with `npx @electron/asar extract`. The native module did not extract, which is harmless.
- Pretty-printed `.vite/build/*.js` with `prettier@3.3.3 --parser babel`. Line numbers above refer to that output.
- Chunks:
  - `index.chunk-DNPOm2Zc.js`: local session manager;
  - `index.chunk-Bs1Y7Yn0.js`: CLI session discovery;
  - `index.chunk-jPKPzV-P.js`: main process, constants `claude-code-sessions`, `local_`, `archived-sessions.idx`, `imported-staging` at line 49851, deep links.

  Chunk hashes change with every app release.
- Read the CLI binary's strings for `--desktop`.
- Inspected `claude-code-sessions` with read-only Python scripts that printed only key names, types and counts. They never printed ids, titles, paths or values. Counted `local_<uuid>` occurrences in other userData stores, skipping cookies, password and token files.
- Nothing was written to Claude state. No extracted app code or session data is committed.
- Exa search was out of credits, so web search used the Parallel search tool. GitHub issues and repos were read with `gh`.
