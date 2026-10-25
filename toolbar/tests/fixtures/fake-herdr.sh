#!/bin/sh
# Stand-in for the `herdr` CLI in tests. Logs every call, answers a few.
printf '%s\n' "$*" >> "$FAKE_HERDR_LOG"
case "$1 $2" in
  "pane get")
    [ "${FAKE_PANE_EXISTS:-0}" = 1 ] && exit 0 || exit 1 ;;
  "plugin pane")
    printf '{"result":{"pane":{"pane_id":"w1:p9"}}}\n' ;;
esac
exit 0
