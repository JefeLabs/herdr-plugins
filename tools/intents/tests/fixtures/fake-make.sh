#!/bin/sh
# Stand-in for make: logs its arguments; FAKE_MAKE_FAIL=1 fails with exit 2.
printf '%s\n' "$*" >> "$FAKE_MAKE_LOG"
[ "${FAKE_MAKE_FAIL:-0}" = 1 ] && exit 2
exit 0
