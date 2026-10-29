#!/bin/sh
# Stand-in for workbrokr's `work`. Logs "<WORK_SESSION> <args>" to FAKE_WORK_LOG.
# FAKE_WORK_REFUSE=<area>:<code> makes `start` for that area refuse the way
# work does with --json: a {"code","message"} line on stderr, exit 1.
printf '%s %s\n' "$WORK_SESSION" "$*" >> "$FAKE_WORK_LOG"
case "$1" in
  start)
    area=$3
    case "${FAKE_WORK_REFUSE:-}" in
      "$area":*)
        echo "(node:1) a warning on stderr first" >&2
        printf '{"code":"%s","message":"refused"}\n' "${FAKE_WORK_REFUSE#*:}" >&2
        exit 1 ;;
    esac
    echo '{"ok":true}' ;;
  env) printf '{"SLOT":"4","WORKTREE":"%s","PORT_FRONTEND":"3004"}\n' "$FAKE_WORK_TREE" ;;
  finish) echo '{"ok":true}' ;;
esac
exit 0
