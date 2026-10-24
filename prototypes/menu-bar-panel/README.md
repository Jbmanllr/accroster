# PROTOTYPE: menu bar panel (throwaway)

Answers the wayfinder ticket [Prototype the menu bar panel](https://github.com/Jbmanllr/claude-account-manager/issues/11): what the menu bar item and its panel look like and do, and whether the objc2 stack delivers the look (native `NSMenu` vs `NSPopover`).

This branch is never merged. Fake data only; nothing is read from or written to disk. Every action is a stub that prints the in-memory state to stdout.

## Run

```bash
cargo run --manifest-path prototypes/menu-bar-panel/Cargo.toml -- --variant a
```

`--variant a|b|c` picks the starting variant; the bottom row of every panel cycles A → B → C. `--open` opens the panel 0.8 s after launch (used for screenshots).

## Variants

| | Shape | Primary affordance | Menu bar label |
|---|---|---|---|
| A | Native `NSMenu`, one custom-view row per Profile | Click a row = use it for the CLI; hover = submenu (desktop switch, details, rename) | `Perso 62%` |
| B | `NSPopover`, one card per Profile | Explicit `Use in CLI` / `Switch desktop…` buttons per card; Session search field | `✳ Perso · 5h 62%` |
| C | `NSPopover`, organised by client | CLI and Desktop slots with a Profile picker each; compact table of all Profiles; recent Sessions inline with Terminal / Desktop buttons | `62% · 92%` (CLI · Desktop) |

Fake Profiles: Perso (CLI, 62 % session), Work (Desktop, Opus weekly 92 %, stale and rate-limited), `jb@gmail.com · Side Lab` (no nickname, most headroom), Freelance (seen on disk, not signed in).

## Stack findings (measured 2026-10-24, macOS 27.0.1, objc2 0.6.5 / objc2-app-kit 0.3.2)

- Both shapes work with objc2. Custom-view `NSMenuItem` rows render inside a native menu with vibrancy, and an `NSPopover` anchored to the status item renders cards, pop-up buttons, a search field and level bars.
- `NSProgressIndicator` inside a menu renders washed out (grey track, faint fill). `NSLevelIndicator` (continuous capacity, warning 70 / critical 90) keeps its green / yellow / red in both menus and popovers. Use level indicators.
- Custom-view menu rows need their own code for two things a plain item does for free: hover highlight (`drawRect:` checking `enclosingMenuItem().isHighlighted()`, confirmed working) and click (`mouseUp:` → `cancelTracking` → `performSelector:afterDelay:0`). The submenu arrow isn't drawn either; the row draws its own `›`.
- A long menu bar label gets hidden behind the notch's overflow on a crowded menu bar: `Perso 62% │ D 18%` disappeared entirely, while `62% · 92%` shows. Keep the label short.
- Two Profiles can share one email, so a Profile without a nickname needs the `email · Organization` name.
- Debug build 1.6 MB; builds in seconds from a warm registry.
