# The core is layered, each fact about Claude's files has one owner, and tests check both

The core's modules are listed in one order, lowest layer first, and a module uses only the modules listed before it. Each piece of knowledge about the world outside the core has one owning module: where Claude keeps its files and how it names them (`claude_files`), each undocumented format (`claude_json`, `live_processes`, `usage`), the clock, the environment and other processes (`env`), and how a file is written (`storage`). `core/tests/architecture.rs` checks these rules on every test run, from the source text, so a change that breaks one fails with the file, the line and what to do instead.

Before this, the layout of Claude's files was joined from literals in more than ten modules. Six functions walked the Sidebar folders, and they disagreed on folders not named by a uuid and on links. Four constants held the 10 MB entry limit. The busy guard and the running dots each parsed live-process files their own way. Five dependency cycles tied the feature modules together, so reading one meant reading several. Nothing stopped the next change from adding a sixth copy.

## Considered Options

- **Convention only**, written in a document. Rejected: the duplication above grew while `env` and `storage` already documented their boundaries. A rule nothing checks is followed until the day it is inconvenient.
- **A crate per layer**, so the compiler enforces the order. Rejected for now: five crates for 16,000 lines, slower builds, and `pub` on every item a higher layer needs. Worth revisiting if the core grows several times over.
- **A lint tool** (`cargo-modules`, `cargo-deny` bans, a custom Clippy lint). Rejected: a new tool to install and pin, for rules a 300-line test expresses with the `regex` crate the core already uses.
- **Allowing cycles inside a "feature group".** Rejected: every cycle found was a misplaced item (a shared type defined by one feature, a second responsibility inside a module), and moving it made both modules simpler.

## Consequences

- A new module is placed in `LAYERS` in `core/tests/architecture.rs`, as low as its dependencies allow. A new dependency on a higher module fails the test: move what both need into the lower module, or have the caller pass it in.
- Inside the core, an item is named by its module (`crate::core::Core`), never through the crate root's re-exports, so the layering check sees every dependency.
- A new path or naming rule of Claude's goes into `claude_files`; no other module of the core writes `.claude`, `claude-code-sessions`, `local_`, `deleted_` or the desktop app's folders as a literal.
- Every module that uses Sidebar entries agrees on what a Sidebar folder is: a real folder named by a uuid, inside one named by a uuid. The Session index, the Discovered scan and the Guided switch's snapshot used to accept other names or follow links. Claude makes neither, so nothing changes on a real Mac. The self-check's Sidebar entries check still looks into every real subfolder, whatever its name: its job is to notice what the app doesn't expect (ADR 0004).
- In a live-process file, the fields that only name a Session (`name`, `hostSessionId`, `sessionId`) are absent when they have the wrong type, instead of making the file unreadable. The busy guard then reads `status` where it used to answer "can't tell". `pid`, `procStart`, `entrypoint` and `status` still make the file unreadable, and the self-check still reports all of them (ADR 0004).
- A live-process file is a regular file named `<pid>.json`. The Session index already skipped a link with that name; the busy guard and the self-check used to read through one and now skip it too. A desktop child whose file is skipped counts as "can't tell", as before for any child without a file.
- Three more places now follow the rule their neighbours already used, in cases Claude doesn't produce:
  - The self-check takes a Sidebar entry's id from its file name by removing one `.json`, as Sidebar sync does (it removed every trailing `.json`).
  - The self-check samples the Transcripts the Session index reads, so not a subagent's `agent-*.jsonl` kept next to a Session's own.
  - A busy Session's title, when its entry exists in several Sidebar folders with different titles, comes from the first folder in sorted order (it came from whichever the file system listed first).
- Every cache of Claude's files tells a changed file by one stamp (`stamp::Stamp`: inode, size, modification and change time). The Session index and the Discovered scan compared size and modification time only; they now also read an entry again when only its inode or change time differs.
- A time shown to the user is worded in `times`; no other module of the core calls `strftime`, except for file names and the log.
- The checks read source text, not types. They can miss a dependency reached through a re-export or a `super::` path, and they match literals, not meaning. They are a tripwire for the common mistake; review still decides.
- The AppKit layer (`src/`) is checked for one thing: it uses no `std::fs`, no `serde` and no `std::env::var`. That it decides nothing is left to review, and its internal structure has no tests (see `docs/TESTING.md`).
