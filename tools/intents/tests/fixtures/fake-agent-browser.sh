#!/bin/sh
# Stand-in for agent-browser: logs each call as [arg][arg]… on one line to
# FAKE_AB_LOG (an argument with spaces stays one bracket), answers "ok".
{ for a in "$@"; do printf '[%s]' "$a"; done; echo; } >> "${FAKE_AB_LOG:-/dev/null}"
echo ok
