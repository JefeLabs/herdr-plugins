#!/bin/sh
# Stand-in for an agent CLI in print mode. Logs argv, then streams FAKE_EVENTS
# one line per 20 ms. FAKE_MODE=hang never exits (after its result, as agy
# can); FAKE_MODE=midturn streams all but the result line and never exits (a
# turn still in progress); FAKE_MODE=garbage prints non-JSON lines first.
[ -n "$FAKE_PROVIDER_LOG" ] && printf '%s\n' "$*" >> "$FAKE_PROVIDER_LOG"
if [ "${FAKE_MODE:-}" = garbage ]; then
  echo "warning: model warming up"
  echo "not json at all"
fi
if [ -n "${FAKE_EVENTS:-}" ]; then
  while IFS= read -r line; do
    if [ "${FAKE_MODE:-}" = midturn ]; then
      case "$line" in *'"event": "result"'*) continue ;; esac
    fi
    printf '%s\n' "$line"
    sleep 0.02
  done < "$FAKE_EVENTS"
fi
if [ "${FAKE_MODE:-}" = hang ] || [ "${FAKE_MODE:-}" = midturn ]; then
  sleep 600
fi
if [ "${FAKE_MODE:-}" = selfkill ]; then
  kill -TERM $$
fi
exit 0
