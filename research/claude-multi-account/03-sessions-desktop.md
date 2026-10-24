# 03 · Sessions in the desktop app

## Two layers

A desktop Code session is two things:

1. **The transcript**: `~/.claude/projects/<encoded-cwd>/<cliSessionId>.jsonl`. The desktop app runs Claude Code underneath, so this is the same file format and folder the CLI uses. Shared by every account.
2. **The sidebar entry**: `~/Library/Application Support/Claude/claude-code-sessions/<account>/<org>/local_<id>.json`. Per account.

The sidebar only lists entries in the signed-in account's folder. Another account's sessions vanish from the sidebar even though their transcripts are still on disk.

## What a sidebar entry holds

Fields seen across 557 entries (most common ones):

| Group | Fields |
|---|---|
| Identity | `sessionId` (`local_…`), `cliSessionId` (transcript id), `title`, `titleSource` |
| Location | `cwd`, `originCwd`, `gitAnchors`, `writtenBranches`, `prs` |
| State | `createdAt`, `lastActivityAt`, `completedTurns`, `isArchived`, `lastTurnReport`, `turnWrapUp` |
| Model and permissions | `model`, `effort`, `permissionMode`, `sessionPermissionUpdates`, `alwaysAllowedReasons` |
| Tools | `remoteMcpServersConfig` (claude.ai connectors injected into the session), `enabledMcpTools`, `toolSurfaceSnapshot` |
| Links | `spawnedFrom`, `bridgeSessionIds` (Remote Control), `startedFromEnvironmentId` |

`cliSessionId` is the join key to the transcript. Archiving, titles, PR links and per-session permissions live only in the entry, so they do not follow a session to another account.

## Retrieval flow

```
App start / account switch
  └─ read claude-code-sessions/<account>/<org>/*.json  → build sidebar
Open a session
  └─ cliSessionId → ~/.claude/projects/<cwd>/<cliSessionId>.jsonl → replay
  └─ remoteMcpServersConfig → reconnect that account's claude.ai connectors
```

## Observed counts

| Account | Entries | Transcript present | Transcript missing | Archived |
|---|---|---|---|---|
| A | 164 | 162 | 2 | 144 |
| B | 286 | 253 | 34 | 230 |
| C | 42 | 41 | 1 | 35 |
| D | 64 | 62 | 2 | 47 |

The entry can outlive its transcript. Of the 39 missing transcripts, 6 entries never got a `cliSessionId` and 2 were in removed worktrees; the other 31 point to folders that still exist, and none were newer than 2026-10-10. The cause is not determined: transcript retention (see [04](04-sessions-cli.md#retention)) does not explain it alone, since older transcripts from July are still present. Opening such an entry has nothing to replay. User reports describe the same symptom: `cliSessionId` reset to null and sessions flagged `transcriptUnavailable` (anthropics/claude-code#101545, #92825, #100364, #91017). None of this index is documented by Anthropic.

## Other per-account desktop data

- **Cowork sessions**: `local-agent-mode-sessions/<account>/<org>/`
- **"No folder" sessions**: `scratch-workspaces/<account>/<org>/scratch-<date>-<id>/`. Deleting the session deletes the folder.
- **Cloud sessions** (claude.ai/code): stored server side under the account; never in these folders.
