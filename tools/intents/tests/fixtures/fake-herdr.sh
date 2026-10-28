#!/bin/sh
# Stand-in for the `herdr` CLI in send tests. Logs every call, answers a few.
printf '%s\n' "$*" >> "$FAKE_HERDR_LOG"
case "$1 $2" in
  "workspace create")
    printf '{"id":"cli:workspace","result":{"workspace":{"workspace_id":"w9","label":"x"},"tab":{"tab_id":"w9:t1"},"root_pane":{"pane_id":"w9:p1"}}}\n' ;;
  "agent start")
    [ "${FAKE_AGENT_START_FAILS:-0}" = 1 ] && { echo '{"error":{"code":"agent_not_ready","message":"agent did not become ready"}}' >&2; exit 1; }
    printf '{"id":"cli:agent","result":{"agent":{"name":"%s","kind":"claude","pane_id":"w9:p1"}}}\n' "$3" ;;
  "agent list")
    [ -n "$FAKE_AGENT_LIST" ] || exit 1
    cat "$FAKE_AGENT_LIST" ;;
  "agent focus")
    printf '{"id":"cli:agent","result":{"focused":true}}\n' ;;
  "agent prompt")
    printf '{"id":"cli:agent","result":{"submitted":true}}\n' ;;
esac
exit 0
