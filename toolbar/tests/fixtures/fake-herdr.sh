#!/bin/sh
# Stand-in for the `herdr` CLI in tests. Logs every call, answers a few.
printf '%s\n' "$*" >> "$FAKE_HERDR_LOG"
case "$1 $2" in
  "pane get")
    if [ "${FAKE_PANE_EXISTS:-0}" = 1 ]; then
      # A real non-plugin pane has no plugin_id field; only the label tells it apart.
      printf '{"id":"cli:pane","result":{"pane":{"pane_id":"%s","label":"%s"}}}\n' "$3" "${FAKE_PANE_LABEL:-Toolbar}"
      exit 0
    fi
    exit 1 ;;
  "pane layout")
    # tab 200 wide: agent 110, toolbar 90 (45%)
    printf '{"id":"cli:pane:layout","result":{"layout":{"area":{"x":0,"y":0,"width":200,"height":50},"panes":[{"pane_id":"w1:p3","rect":{"x":0,"y":0,"width":110,"height":50}},{"pane_id":"w1:p9","rect":{"x":110,"y":0,"width":90,"height":50}}]}}}\n' ;;
  "pane neighbor")
    # real shape: neighbor_pane_id is absent when there is no neighbour
    if [ -n "${FAKE_NEIGHBOR:-}" ]; then
      printf '{"id":"cli:pane:neighbor","result":{"neighbor":{"direction":"left","neighbor_pane_id":"%s","pane_id":"w1:p9"}}}\n' "$FAKE_NEIGHBOR"
    else
      printf '{"id":"cli:pane:neighbor","result":{"neighbor":{"direction":"left","pane_id":"w1:p9"}}}\n'
    fi ;;
  "pane split")
    printf '{"id":"cli:pane:split","result":{"pane":{"pane_id":"w1:p12","tab_id":"w1:t1"}}}\n' ;;
  "plugin pane")
    # Real 0.9.5 shape: result.plugin_pane.pane.pane_id
    printf '{"id":"cli:plugin","result":{"plugin_pane":{"entrypoint":"toolbar","pane":{"pane_id":"w1:p9","tab_id":"w1:t1"},"plugin_id":"jefelabs.toolbar"},"type":"plugin_pane_opened"}}\n' ;;
esac
exit 0
