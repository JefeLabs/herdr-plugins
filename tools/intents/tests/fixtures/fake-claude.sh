#!/bin/sh
# Stand-in for claude in stream-json print mode: logs its arguments (one per
# line, then ---), writes FAKE_CLAUDE_WRITES (the recording agent-browser would
# make), prints FAKE_CLAUDE_EVENTS. FAKE_CLAUDE_HANG=1 never finishes.
for a in "$@"; do printf '%s\n' "$a"; done >> "$FAKE_CLAUDE_LOG"
echo --- >> "$FAKE_CLAUDE_LOG"
[ -n "${FAKE_CLAUDE_WRITES:-}" ] && printf 'video' > "$FAKE_CLAUDE_WRITES"
[ "${FAKE_CLAUDE_HANG:-0}" = 1 ] && sleep 600
[ -n "${FAKE_CLAUDE_EVENTS:-}" ] && cat "$FAKE_CLAUDE_EVENTS"
exit 0
