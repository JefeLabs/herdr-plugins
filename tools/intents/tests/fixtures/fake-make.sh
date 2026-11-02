#!/bin/sh
# Stand-in for make: logs "SLOT=<its SLOT> <args>"; FAKE_MAKE_FAIL=1 fails with
# exit 2; FAKE_MAKE_SLOW=1 takes 2 s. Prints a line on stdout and one on
# stderr, as the e2e runner does, for the run's log.
printf 'SLOT=%s %s\n' "${SLOT:-}" "$*" >> "$FAKE_MAKE_LOG"
printf 'make said on stdout: %s\n' "$*"
printf 'make said on stderr: %s\n' "$*" >&2
[ "${FAKE_MAKE_SLOW:-0}" = 1 ] && sleep 2
[ "${FAKE_MAKE_FAIL:-0}" = 1 ] && exit 2
exit 0
