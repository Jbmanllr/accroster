#!/bin/sh
# Stub `claude` for the fake Mac. Written next to its state by
# accroster-fake-mac; never installed anywhere else.
#
# 1. Classifies the call into a reply key (auth-login, auth-status,
#    auth-logout, get-usage, slash-usage, other).
# 2. Records the call under calls/<n>/: args and env (NUL-separated), the
#    stdin it read, the key and its pid.
# 3. Answers from replies/<key>/<seq>/: copies files/ into $CLAUDE_CONFIG_DIR,
#    prints stdout and stderr, exits with exit_code. Replies are consumed in
#    order; the last one repeats until another is queued after it. With no
#    reply, exits 97. A reply marked
#    hang never answers: it waits until killed.
set -u
state="$(cd "$(dirname "$0")/.." && pwd)"

key=other
case "${1:-} ${2:-}" in
  "auth login") key=auth-login ;;
  "auth status") key=auth-status ;;
  "auth logout") key=auth-logout ;;
esac

stdin_line=""
for arg in "$@"; do
  # The usage read sends one control request line on stdin and keeps the
  # pipe open, so read exactly one line rather than to EOF.
  if [ "$arg" = "--input-format" ]; then
    IFS= read -r stdin_line || true
    break
  fi
done

if [ "$key" = other ]; then
  case "$stdin_line" in
    *get_usage*) key=get-usage ;;
  esac
fi
if [ "$key" = other ]; then
  for arg in "$@"; do
    [ "$arg" = "/usage" ] && key=slash-usage
  done
fi

mkdir -p "$state/calls"
# Claim the next call number with mkdir, which fails if another stub running
# at the same time took it first.
n=$(( $(ls "$state/calls" | wc -l) + 1 ))
until mkdir "$state/calls/$(printf '%04d' "$n")" 2>/dev/null; do
  n=$((n + 1))
done
call="$state/calls/$(printf '%04d' "$n")"
printf '%s\0' "$@" > "$call/args"
/usr/bin/env -0 > "$call/env"
printf '%s' "$stdin_line" > "$call/stdin"
printf %s "$key" > "$call/key"
# Written then renamed: a reader never sees an empty pid file.
printf %s "$$" > "$call/pid.tmp"
mv "$call/pid.tmp" "$call/pid"

replies="$state/replies/$key"
first=""
count=0
if [ -d "$replies" ]; then
  for seq in $(ls "$replies" | sort -n); do
    count=$((count + 1))
    [ -z "$first" ] && first="$seq"
  done
fi
# A last reply that already answered (and so repeated) gives way to replies
# queued after it.
if [ "$count" -gt 1 ] && [ -f "$replies/$first/answered" ]; then
  mkdir -p "$state/used/$key"
  mv "$replies/$first" "$state/used/$key/$first"
  count=$((count - 1))
  first=$(ls "$replies" | sort -n | head -n 1)
fi
if [ -z "$first" ]; then
  echo "stub claude: no canned reply for $key" >&2
  exit 97
fi
reply="$replies/$first"
[ "$count" -eq 1 ] && : > "$reply/answered"

# A hanging reply waits, like a sign-in nobody finishes, until killed. exec
# keeps the pid, so killing the recorded pid kills the wait. It is consumed
# first, like any other reply, unless it is the last one.
if [ -f "$reply/hang" ]; then
  if [ "$count" -gt 1 ]; then
    mkdir -p "$state/used/$key"
    mv "$reply" "$state/used/$key/$first"
  fi
  exec /bin/sleep 600
fi

if [ -d "$reply/files" ]; then
  if [ -z "${CLAUDE_CONFIG_DIR:-}" ]; then
    echo "stub claude: reply writes files but CLAUDE_CONFIG_DIR is unset" >&2
    exit 98
  fi
  mkdir -p "$CLAUDE_CONFIG_DIR"
  cp -R "$reply/files/." "$CLAUDE_CONFIG_DIR/"
fi
cat "$reply/stdout"
cat "$reply/stderr" >&2
code=$(cat "$reply/exit_code")

if [ "$count" -gt 1 ]; then
  mkdir -p "$state/used/$key"
  mv "$reply" "$state/used/$key/$first"
fi
exit "$code"
