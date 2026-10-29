#!/bin/sh
# Stand-in for `gh` in progress tests. Logs every call; `pr view <n>` answers
# with the state stored in $FAKE_GH_DIR/<n> (and the title in <n>.title, if
# any), and fails when there is none.
printf '%s\n' "$*" >> "${FAKE_GH_LOG:-/dev/null}"
if [ "$1 $2" = "pr list" ]; then
  q=""
  while [ $# -gt 0 ]; do
    [ "$1" = "--search" ] && { q="$2"; break; }
    shift
  done
  q=$(printf '%s' "$q" | tr -d '"')
  if [ -f "$FAKE_GH_DIR/search-$q.json" ]; then cat "$FAKE_GH_DIR/search-$q.json"; else echo '[]'; fi
  exit 0
fi
if [ "$1 $2" = "issue view" ]; then
  if [ -f "$FAKE_GH_DIR/issue-$3.json" ]; then cat "$FAKE_GH_DIR/issue-$3.json"; exit 0; fi
  echo "no issue" >&2
  exit 1
fi
n="$3"
case "$n" in
  */pull/*) n="${n##*/pull/}"; n="${n%%/*}" ;;
esac
if [ "$1 $2" = "pr view" ] && [ -f "$FAKE_GH_DIR/$n" ]; then
  if [ -f "$FAKE_GH_DIR/$n.title" ]; then
    printf '{"state":"%s","title":"%s"}\n' "$(cat "$FAKE_GH_DIR/$n")" "$(cat "$FAKE_GH_DIR/$n.title")"
  else
    printf '{"state":"%s"}\n' "$(cat "$FAKE_GH_DIR/$n")"
  fi
  exit 0
fi
echo "no pull requests found" >&2
exit 1
