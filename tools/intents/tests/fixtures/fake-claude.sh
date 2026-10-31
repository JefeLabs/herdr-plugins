#!/bin/sh
# Stand-in for claude in stream-json print mode. `--help` names the flags a
# capture needs (FAKE_CLAUDE_OLD=1: an older claude without them). Otherwise
# logs its arguments (one per line, then ---), writes FAKE_CLAUDE_WRITES (the
# recording the browser tools would make), writes FAKE_CLAUDE_STEPS to the
# steps file the browser tools keep, prints FAKE_CLAUDE_EVENTS.
# FAKE_CLAUDE_PROOF=<n,n…> writes <n>.png into FAKE_CHECK_DIR (a check's proof).
# FAKE_CLAUDE_HANG=1 runs for 30 s.
if [ "$1" = --help ]; then
  if [ "${FAKE_CLAUDE_OLD:-0}" = 1 ]; then echo "--permission-mode (choices: default, plan)"; else echo "--restricted  --permission-mode (choices: dontAsk, plan)"; fi
  exit 0
fi
for a in "$@"; do printf '%s\n' "$a"; done >> "$FAKE_CLAUDE_LOG"
echo --- >> "$FAKE_CLAUDE_LOG"
[ -n "${FAKE_CLAUDE_WRITES:-}" ] && printf 'video' > "$FAKE_CLAUDE_WRITES"
[ -n "${FAKE_CLAUDE_STEPS:-}" ] && printf '%s\n' "$FAKE_CLAUDE_STEPS" > .steps-before.txt
for n in $(printf '%s' "${FAKE_CLAUDE_PROOF:-}" | tr , ' '); do printf png > "$FAKE_CHECK_DIR/$n.png"; done
[ "${FAKE_CLAUDE_HANG:-0}" = 1 ] && sleep 30
# a story's take (its agent has take_start): FAKE_CLAUDE_HANG_TAKE=1 runs for
# 30 s; FAKE_CLAUDE_CAPTIONS=<n,…> leaves those captions and a video in the
# story's folder (the agent's working folder), as the take tools would
# (take.mp4, and take.stopped unless FAKE_CLAUDE_NO_STOP=1)
case "$*" in *take_start*)
  [ "${FAKE_CLAUDE_HANG_TAKE:-0}" = 1 ] && { printf 'partial' > take.mp4; sleep 30; }
  if [ -n "${FAKE_CLAUDE_CAPTIONS+x}" ]; then
    printf '[%s]' "$FAKE_CLAUDE_CAPTIONS" > captions.json
    printf 'video' > take.mp4
    [ "${FAKE_CLAUDE_NO_STOP:-0}" = 1 ] || : > take.stopped
  fi
  ;;
esac
[ -n "${FAKE_CLAUDE_EVENTS:-}" ] && cat "$FAKE_CLAUDE_EVENTS"
exit 0
