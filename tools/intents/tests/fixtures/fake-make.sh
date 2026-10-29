#!/bin/sh
# Stand-in for make: logs "SLOT=<its SLOT> <args>"; FAKE_MAKE_FAIL=1 fails with
# exit 2; FAKE_MAKE_SLOW=1 takes 2 s.
printf 'SLOT=%s %s\n' "${SLOT:-}" "$*" >> "$FAKE_MAKE_LOG"
[ "${FAKE_MAKE_SLOW:-0}" = 1 ] && sleep 2
[ "${FAKE_MAKE_FAIL:-0}" = 1 ] && exit 2
exit 0
