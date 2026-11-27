# Architecture

How AccRoster's code is organised, which module owns what, and the rules that keep it that way. The vocabulary is `GLOSSARY.md`'s; the decisions are in `docs/adr/`. The rules in "Rules the tests check" are enforced by `core/tests/architecture.rs` (ADR 0006).

## Two parts, one process

| Part | Crate | What it does |
|---|---|---|
| The core | `accroster-core` (`core/`) | All behaviour. Commands and events in, a `View` and `Effect`s out. No AppKit. |
| The AppKit layer | `accroster` (`src/`) | Renders the `View`, forwards clicks as `Command`s and OS notifications as `Event`s, performs each `Effect`. Decides nothing. |
| The fake Mac | `accroster-fake-mac` (`fake-mac/`) | Test support: a temp home, a stub `claude`, a fake clock and fake OS facts. |

The contract between the two parts is `core/src/interface.rs`. The driver (`core/src/driver.rs`) runs the core on its own thread, so the UI never waits on it. Everything the core touches outside itself comes through one `Environment` (`core/src/env.rs`), which the tests replace with the fake Mac.

## The AppKit layer's files

| File | Holds |
|---|---|
| `src/main.rs` | Builds the real `Environment`, launches the core, starts the UI. |
| `src/ui.rs` | `Ui`: the target of every click and the owner of every window; rendering a `View`, performing an `Effect`, and building the windows. |
| `src/ui/widgets.rs` | Views built from one view model each (a Profile row, a limit bar, a badge, a Session row, menu items). They don't know `Ui`. |
| `src/ui/sidebar.rs` | The All Sessions sidebar's data source and delegate. |
| `src/ui/mac.rs` | What the UI does to the rest of the Mac: Terminal, the Claude desktop app, bringing windows forward, the Trash, alerts, the network path. |
| `src/notifications.rs` | Posting user notifications and receiving their clicks. |

## The core's modules

Lowest layer first. A module uses only the modules above it in this table. The order is `LAYERS` in `core/tests/architecture.rs`, which is the source of truth.

### Foundations

They know none of the app's features.

| Module | Owns |
|---|---|
| `interface` | The contract with the UI: `Command`, `Event`, `Effect`, `View` and its parts. |
| `claude_files` | Where Claude Code and the desktop app keep their files, and how they name them. Opens nothing. |
| `storage` | The app's own folder (`AppPaths`) and the one way a file is written: temp file, then rename. Versioned JSON files. |
| `env` | The boundary with the rest of the Mac: home folder, inherited variables, spawning, the clock, OS facts, the login item. |
| `logging` | `app.log`: redacted when written, rotated. The panic hook. |
| `shell` | Quoting a path or an id as one `sh` word. |

### Claude's formats

One reader per undocumented format. Each checks the fields it uses every time it reads them (ADR 0004).

| Module | Owns |
|---|---|
| `claude_json` | `oauthAccount` in a `.claude.json`: a Login, its names, its Organization. |
| `limit_offers` | Banked resets and Session refills in a usage reply. |
| `usage` | The `get_usage` reply and the `/usage` fallback: a Limit reading. |
| `live_processes` | `sessions/<pid>.json`: which Claude Code processes run, for which Session, busy or idle. |

### Services

| Module | Owns |
|---|---|
| `backups` | `backups/`: a copy before every write to a file the app doesn't own; pruning. |
| `settings` | `settings.json`: the user's choices. |
| `runner` | The only module that spawns Claude Code: the cleared environment, the neutral working folder, timeouts, the job queue. |
| `claude_code` | Finding `claude` on this Mac (and remembering it in `state/claude-path`); the installed Claude versions. |
| `context` | `Context`: the environment, paths, log and Safety pauses the core lends a feature for one call. |
| `safety` | Format check results and Safety pauses (`state/safety.json`); the settings `env` check; Diagnostics. |

### Features

Each owns its state and its own file under `state/`.

| Module | Owns |
|---|---|
| `profile_folder` | A Profile folder's links back to `~/.claude` and its `.claude.json` seed (ADR 0002). |
| `monitor` | The Limit monitor: when each Profile's limits are read, backoff (`state/backoff.json`), the limit bars. |
| `profiles` | The Profile registry (`state/profiles.json`, `profiles/<id>/`): sign-in, sign-out, removal, names and Short labels. |
| `notifications` | Which limit notifications fired (`state/notifications.json`) and which are due. |
| `cli_switcher` | CLI switching: the shim, the pointer, the `~/.zshrc` block, and its part of the menu (ADR 0002). |
| `desktop_account` | The desktop's Active Profile, as far as its files tell. |
| `discovered` | Discovered Profiles: the evidence on the Mac and their rows. |
| `sidebar_sync` | Sidebar sync: reading, planning and writing copies (`state/sidebar-sync.json`), and its watch (ADR 0003). |
| `sessions` | The Session index (`index/sessions.json`) and the All Sessions window's state. |
| `opening` | What opening a Session depends on, and the choices offered (ADR 0005). |
| `desktop` | The Desktop bridge: the busy guard, the Guided switch, Reopen Claude. |
| `checks` | The self-check's format checks that read files (ADR 0004). |

### The core and what runs it

| Module | Owns |
|---|---|
| `core` | `Core`: composes the features, handles every `Command` and `Event`, builds the `View`. Its submodules (`core/cli.rs`, `open.rs`, `self_check.rs`, `sidebar.rs`, `uninstaller.rs`) are the flows that span several features. |
| `driver` | The core's thread and its ticks. |
| `watcher` | The real FSEvents watch behind the watch effects. |

## Who writes which file

A file has one writer. To change what a file holds, change its owner.

| File | Owner |
|---|---|
| `settings.json` | `settings` (the core saves it) |
| `state/profiles.json`, `profiles/<id>/` | `profiles` |
| `profiles/<id>/` links and `.claude.json` seed | `profile_folder` |
| `state/backoff.json` | `monitor` |
| `state/notifications.json` | `notifications` |
| `state/safety.json` | `safety` |
| `state/sidebar-sync.json`, copies in `claude-code-sessions/` | `sidebar_sync` |
| `state/claude-path` | `claude_code` |
| `index/sessions.json` | `sessions` |
| `bin/claude`, `bin/cli-active-profile`, the `~/.zshrc` block | `cli_switcher` |
| `backups/` | `backups` |
| `logs/app.log` | `logging` |
| `spawn-cwd/` | `runner` |

## Rules the tests check

`cargo test -p accroster-core --test architecture` runs them; a failure prints the file, the line and what to do instead.

1. **Layering.** A module of the core uses only modules listed before it in `LAYERS`. No cycles.
2. **Named by module.** Inside the core, write `crate::core::Core`, not `crate::Core`.
3. **Claude's layout has one owner.** No module but `claude_files` writes one of Claude's paths or file-name rules as a literal.
4. **The environment boundary.** No module but `env` reads the clock, the process environment or the process table, or spawns a program.
5. **One way to write a file.** No module but `storage` writes a whole file in place.
6. **The AppKit layer decides nothing.** `src/` reads and writes no files and parses nothing.

## Rules the compiler checks

- Every module of the core is private except `interface`, `env`, `driver` and `watcher`; the AppKit layer can only reach what `core/src/lib.rs` re-exports.
- `Command`, `Event` and `Effect` are matched exhaustively: a new variant doesn't compile until the core and the UI handle it.
- App-owned JSON files implement `VersionedFile`; `read_versioned` rejects another version.

## Rules review still decides

- **One canonical rule.** Before writing a rule about a Claude format or a domain concept, look for its owner in the tables above and call it.
- **A feature owns its state.** The core calls a feature's methods; it doesn't reach into its file or its fields.
- **Text shown to the user** uses the glossary's words.

## Adding something

- **A new fact about Claude's files:** a path or name goes in `claude_files`; a format gets a reader in the formats layer; `checks` gets a format check when a write depends on it (ADR 0004).
- **A new feature:** a module in the features layer that owns its state file, takes a `Context`, and returns `Effect`s and view parts. The core wires it to `Command`s and `Event`s in `core.rs`, or in a `core/` submodule when the flow spans features.
- **A new thing the user can click:** a `Command` variant, handled in the core. The UI only sends it.
- **A new outside fact** (a new OS query): a method on a trait in `env`, with its fake in `fake-mac`.
