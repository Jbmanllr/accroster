# Testing and gates

What runs, when, and what a green result proves. Tier names (T0–T4) follow the test-gating model: inner loop, quick, acceptance, full, release.

## Checks

| Check | Command | Proves | Size | Time |
|---|---|---|---|---|
| Format | `cargo fmt --all --check` | formatting | static | seconds |
| Lint | `cargo clippy --all-targets --locked -- -D warnings` | no lint, lockfile unchanged | static | ~1 min |
| Tests | `cargo test --locked` | every core module through its interface against the fake Mac (temp home, stub `claude`, fake clock and OS facts), the shim, FSEvents | one machine | ~2 min |
| Release dry run | `scripts/release.sh --dry-run` | universal build, hardened-runtime ad-hoc signature with the entitlements, dmg layout and version | one machine | ~10 min cold |
| Release | `scripts/release.sh --developer-id --version vX.Y.Z` | Developer ID signature, notarization, staples, Gatekeeper acceptance | one machine + Apple | per release |

The toolchain is pinned in `rust-toolchain.toml`, so local and hosted runs lint and build with the same Rust.

## Tiers and events

| Tier | What | Where and when |
|---|---|---|
| T0 | the test you're changing | while editing: `cargo test -p accroster-core --test <file>` |
| T1 | format, lint, all tests | before pushing, locally; then the `check` job on every pull request and push to `main` |
| T2 | release dry run | the `release-dry-run` job, after `check` passes, on every pull request and push to `main` |
| T3 | everything | every push to `main` (the same workflow, untargeted) |
| T4 | Developer ID release and the first-release checklist | the release workflow on a `v*.*.*` tag, plus the hand checks in `docs/RELEASING.md` |

## Selection

None: every event runs every check. T1 takes about 4 minutes and T2 about 10, both inside their budgets (10 and 30 minutes), and macOS minutes are free on this public repository, so skipping checks by changed path would save nothing and could miss a failure. Revisit when a measured run outgrows its budget.

## Wiring

- `check` runs cheapest first and stops at the first failure; `release-dry-run` waits for it, so a lint failure never pays for a release build.
- `gate` is the single result to require: it fails when either job failed or was cancelled. `main` has no branch protection yet; when it gets some, require `gate` only.
- A newer push to a pull request cancels its older run; `main`'s runs finish.
- Only `main` writes the build caches pull requests read.

## Not covered by any gate

- The AppKit layer (`src/ui.rs`): no automated tests by design. It is checked by hand against the `prototype/*` branches, and by the first-release checklist in `docs/RELEASING.md`.
- Anything that needs a real Claude login, a real desktop app switch or real Anthropic replies (429, 401, an expired Login).
