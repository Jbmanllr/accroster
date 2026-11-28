#!/bin/sh
# Installs a local build as /Applications/AccRoster.app on this Mac and checks
# that it starts: quits the running app, moves the installed one to
# ~/.agent-backups/ (never deleted), copies the new one in, relaunches it and
# waits for its launch line in the app log.
#
# Usage: scripts/install-local.sh [<app>]
#   <app>  the bundle to install; default target/release/AccRoster.app,
#          which scripts/bundle.sh builds.
#
# Exit 0: the new app runs and logged its launch with no ERROR line.
# Exit 1: nothing was replaced (the bundle is unsigned, or the app wouldn't
# quit), or the new app didn't start; the backup path is printed either way.
#
# System Events sometimes answers 0 menu bars for a healthy AccRoster, so its
# answer is reported but never decides the result.
set -eu

repo="$(cd "$(dirname "$0")/.." && pwd)"
new="${1:-$repo/target/release/AccRoster.app}"
installed=/Applications/AccRoster.app
bundle_id=io.github.jbmanllr.accroster
binary="$installed/Contents/MacOS/accroster"
log="$HOME/Library/Application Support/$bundle_id/logs/app.log"

[ -d "$new" ] || { echo "no app at $new: build it with scripts/bundle.sh" >&2; exit 1; }
codesign --verify --strict "$new"

running() { pgrep -f "AccRoster.app/Contents/MacOS/accroster" > /dev/null; }

if running; then
  osascript -e "tell application id \"$bundle_id\" to quit" > /dev/null 2>&1 || true
  i=0
  while running; do
    i=$((i + 1))
    [ "$i" -le 40 ] || { echo "AccRoster did not quit within 20 s; nothing replaced" >&2; exit 1; }
    sleep 0.5
  done
fi

if [ -d "$installed" ]; then
  backup="$HOME/.agent-backups/$(date +%Y-%m-%d)-accroster-app-$(date +%H%M%S)"
  mkdir -p "$backup"
  mv "$installed" "$backup/AccRoster.app"
  echo "previous app: $backup/AccRoster.app"
fi
ditto "$new" "$installed"
codesign --verify --strict "$installed"

offset=$( [ -f "$log" ] && wc -c < "$log" | tr -d ' ' || echo 0)
open "$installed"

# The launch line is written once the core has started.
launched=""
i=0
while [ "$i" -lt 60 ]; do
  if [ -f "$log" ]; then
    size=$(wc -c < "$log" | tr -d ' ')
    [ "$size" -ge "$offset" ] || offset=0
    launched=$(tail -c +"$((offset + 1))" "$log" | grep ' INFO launch ' | tail -n 1 || true)
    [ -n "$launched" ] && break
  fi
  i=$((i + 1))
  sleep 0.5
done

pid=$(pgrep -f "$binary" | head -n 1 || true)
if [ -z "$pid" ]; then
  echo "AccRoster is not running after launch; see $log" >&2
  exit 1
fi
if [ -z "$launched" ]; then
  echo "AccRoster (pid $pid) logged no launch line within 30 s; see $log" >&2
  exit 1
fi
echo "running: pid $pid, $launched"

# Give the first reads a moment, then report what the log says since launch.
sleep 5
kill -0 "$pid" 2> /dev/null || { echo "AccRoster (pid $pid) exited after launch; see $log" >&2; exit 1; }
errors=$(tail -c +"$((offset + 1))" "$log" | grep ' ERROR ' || true)

# On a loaded Mac, System Events can take 40 s to see the item.
menu="not seen within 60 s"
i=0
while [ "$i" -lt 30 ]; do
  count=$(osascript -e "tell application \"System Events\" to count of menu bars of (first process whose unix id is $pid)" 2> /dev/null || echo 0)
  [ "${count:-0}" -ge 1 ] 2> /dev/null && { menu="seen after $((i * 2)) s"; break; }
  i=$((i + 1))
  sleep 2
done
echo "menu bar item through System Events: $menu"

if [ -n "$errors" ]; then
  echo "ERROR lines since launch:" >&2
  echo "$errors" >&2
  exit 1
fi
echo "installed $installed"
