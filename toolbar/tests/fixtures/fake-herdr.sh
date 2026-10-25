#!/bin/sh
# Stand-in for the `herdr` CLI in tests. Logs every call, answers a few.
printf '%s\n' "$*" >> "$FAKE_HERDR_LOG"
case "$1 $2" in
  "pane get")
    [ "${FAKE_PANE_EXISTS:-0}" = 1 ] && exit 0 || exit 1 ;;
  "plugin pane")
    # Real 0.9.5 shape: result.plugin_pane.pane.pane_id
    printf '{"id":"cli:plugin","result":{"plugin_pane":{"entrypoint":"toolbar","pane":{"pane_id":"w1:p9","tab_id":"w1:t1"},"plugin_id":"jefelabs.toolbar"},"type":"plugin_pane_opened"}}\n' ;;
esac
exit 0
