# PROTOTYPE: Unified session view window (throwaway)

Answers the wayfinder ticket [Prototype the Unified session view window](https://github.com/Jbmanllr/claude-account-manager/issues/16): what the window opened from the menu's All Sessions… shows and does, and how it presents opening a Session whose Sidebar entry isn't in the desktop's Active Profile.

This branch is never merged. Fake data only; nothing is read from or written to disk. Every action is a stub that shows an alert saying what it would do, and prints the in-memory state to stdout.

## Run

```bash
cargo run --manifest-path prototypes/unified-session-view/Cargo.toml -- --variant a
```

`--variant a|b|c` picks the starting variant; the purple bar at the bottom cycles A → B → C. `--select N` preselects Session N (for screenshots).

## Variants

| | Shape | Grouping / filter | How a client is picked |
|---|---|---|---|
| A: Table | One dense list with columns (Session, Project · branch, In sidebar of, Last activity) | Search field, Group by None / Project / Profile / Day, Show archived | Per-row `Terminal` and `Claude` buttons; `Claude…` (ellipsis) when the desktop's Active Profile has no entry, which asks. Double-click opens where it was last used |
| B: Browser | Three panes: sidebar \| list \| detail | Sidebar: All, Running, Projects, Sidebars (per Profile, plus CLI only) | Detail pane spells out each choice before clicking: Resume in Terminal (and which Profile pays), Open in Claude as `<desktop Profile>` (new entry), Switch Claude to `<Profile>`, then open… |
| C: Launcher | Spotlight-like: big search field, flat recency list | Search only | Keyboard: ↑↓, ↩ opens where last used, ⌘↩ in the other client; the "not in this sidebar" choice comes as an alert |

Fake state: CLI Active Profile Perso, desktop Active Profile Work. 13 Sessions cover: running in Terminal (green dot; Terminal open brings that window forward), a running desktop turn (orange; blocks the guided switch), untitled (first prompt shown), entries in several Profiles (synced), entry only in a not-signed-in Profile (Freelance), CLI only, archived, and Transcript deleted with the entry left behind (greyed, cannot open).

## Stack findings (objc2 0.6.5 / objc2-app-kit 0.3.2)

- Stack-view rows in a scroll view are enough for a few hundred rows of prototype; the real window should use `NSTableView` (view-based) for many Sessions.
- The scroll view's document view must be flipped (`isFlipped` → YES) or rows pin to the bottom.
- A row that selects on click but lets its buttons click needs `hitTest:` to return the button when hit, else itself.
- Return / ⌘Return / arrows in the search field come through the delegate's `control:textView:doCommandBySelector:` (`insertNewline:`, `noop:` with Command held, `moveUp:`/`moveDown:`); objc2 needs the `Bool` return type there.
- `CGColor` can't be messaged as an object; tint backgrounds with an `NSBox` of type Custom instead.
