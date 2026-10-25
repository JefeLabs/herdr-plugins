#!/bin/sh
# Stand-in for an agent CLI in print mode. Logs argv, then streams FAKE_EVENTS
# one line per 20 ms. FAKE_MODE=hang never exits; FAKE_MODE=garbage prints
# non-JSON lines first.
[ -n "$FAKE_PROVIDER_LOG" ] && printf '%s\n' "$*" >> "$FAKE_PROVIDER_LOG"
if [ "${FAKE_MODE:-}" = garbage ]; then
  echo "warning: model warming up"
  echo "not json at all"
fi
if [ -n "${FAKE_EVENTS:-}" ]; then
  while IFS= read -r line; do
    printf '%s\n' "$line"
    sleep 0.02
  done < "$FAKE_EVENTS"
fi
if [ "${FAKE_MODE:-}" = hang ]; then
  sleep 600
fi
exit 0
