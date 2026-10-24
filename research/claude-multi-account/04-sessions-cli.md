# 04 · Sessions in the CLI

## Storage

| File | Content |
|---|---|
| `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl` | Full transcript, one JSON event per line. |
| `~/.claude/history.jsonl` | Prompt history: `display`, `pastedContents`, `timestamp`, `project`, `sessionId`. Powers up-arrow recall. |
| `~/.claude/sessions/<pid>.json` | Registry of running CLI processes (peer token, start time). Not history. |
| `~/.claude.json` → `projects[<path>].lastSessionId` | Last session per folder, used by `--continue`. |

`<encoded-cwd>` is the absolute working folder with `/` and `.` replaced by `-`, for example `/Users/me/Dev/app` → `-Users-me-Dev-app`.

Transcript events seen: `user`, `assistant`, `system`, `attachment`, `queue-operation`, `custom-title`, `last-prompt`, `bridge-session`. Each event carries `sessionId`, `cwd`, `gitBranch` and an `entrypoint` (CLI vs desktop) but **no account id**.

## Retrieval

| Command | Scope |
|---|---|
| `claude --continue` / `-c` | Most recent session in the current folder. |
| `claude --resume` / `-r` | Picker of sessions in the current worktree (`/resume` inside a session does the same). In the picker, `Ctrl+W` widens to all worktrees and `Ctrl+A` to every project on the machine. |
| `claude --resume <session-id>` | Opens that transcript directly; searches the current project first, then other projects. |
| `claude --desktop --resume <session-id>` / `/desktop` | Moves a CLI session into the desktop app. |

None of these filter by account. A session created in the desktop app under account A resumes in the CLI logged in as account B, because the transcript holds no account binding. New turns are billed to the account the CLI is currently logged into.

## Retention

CLI transcripts older than `cleanupPeriodDays` (default 30) are deleted at startup. Transcripts of sessions started or last continued in the desktop app or Cowork are kept at any age unless `desktopSessionCleanupPeriodDays` is set (v2.1.248+) or managed settings impose `cleanupPeriodDays` ([docs](https://code.claude.com/docs/en/claude-directory#cleaned-up-automatically)). Neither was set here. To keep CLI history longer, in `~/.claude/settings.json`:

```json
{ "cleanupPeriodDays": 365 }
```

## CLI vs desktop in one table

| | CLI | Desktop Code tab |
|---|---|---|
| Transcript location | `~/.claude/projects` | same |
| Session list source | transcripts in the current folder | per-account sidebar index |
| Filtered by account | no | yes |
| Titles, archive, PR links | transcript `custom-title` only | sidebar entry |
