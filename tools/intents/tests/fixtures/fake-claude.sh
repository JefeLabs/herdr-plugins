#!/bin/sh
# Stand-in for claude in stream-json print mode. `--help` names the flags a
# capture needs (FAKE_CLAUDE_OLD=1: an older claude without them). Otherwise
# logs its arguments (one per line, then ---), writes FAKE_CLAUDE_WRITES (the
# recording the browser tools would make), writes FAKE_CLAUDE_STEPS to the
# steps file the browser tools keep, prints FAKE_CLAUDE_EVENTS.
# FAKE_CLAUDE_HANG=1 runs for 30 s.
if [ "$1" = --help ]; then
  if [ "${FAKE_CLAUDE_OLD:-0}" = 1 ]; then echo "--permission-mode (choices: default, plan)"; else echo "--restricted  --permission-mode (choices: dontAsk, plan)"; fi
  exit 0
fi
for a in "$@"; do printf '%s\n' "$a"; done >> "$FAKE_CLAUDE_LOG"
echo --- >> "$FAKE_CLAUDE_LOG"
[ -n "${FAKE_CLAUDE_WRITES:-}" ] && printf 'video' > "$FAKE_CLAUDE_WRITES"
[ -n "${FAKE_CLAUDE_STEPS:-}" ] && printf '%s\n' "$FAKE_CLAUDE_STEPS" > .steps-before.txt
[ "${FAKE_CLAUDE_HANG:-0}" = 1 ] && sleep 30
[ -n "${FAKE_CLAUDE_EVENTS:-}" ] && cat "$FAKE_CLAUDE_EVENTS"
exit 0
