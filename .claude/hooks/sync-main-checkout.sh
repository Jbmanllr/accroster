#!/bin/sh
# Stop hook: when a session in a worktree ends a turn, fast-forward the main
# checkout's `main` to `origin/main`, so a pull request the session merged on
# GitHub is in the owner's folder without a manual pull. A session in a
# worktree can't run git there itself.
#
# It does nothing unless the main checkout is on `main`, has no changes to
# tracked files, and can fast-forward. It never fails the turn.

dir=${CLAUDE_PROJECT_DIR:-$PWD}
here=$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null) || exit 0
main=$(git -C "$dir" worktree list --porcelain 2>/dev/null | sed -n '1s/^worktree //p')
[ -n "$main" ] && [ "$main" != "$here" ] || exit 0
[ "$(git -C "$main" symbolic-ref --quiet --short HEAD 2>/dev/null)" = main ] || exit 0
[ -z "$(git -C "$main" status --porcelain --untracked-files=no 2>/dev/null)" ] || exit 0
git -C "$main" fetch --quiet origin main 2>/dev/null || exit 0
git -C "$main" merge --ff-only --quiet origin/main >/dev/null 2>&1
exit 0
