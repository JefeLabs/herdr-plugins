#!/bin/sh
# Stand-in for open / osascript / pbcopy: logs each argument on its own line,
# then what came on stdin, then ---, to FAKE_ARGS_LOG.
{
  for a in "$@"; do printf '%s\n' "$a"; done
  printf 'stdin:'; cat; printf '\n---\n'
} >> "$FAKE_ARGS_LOG"
