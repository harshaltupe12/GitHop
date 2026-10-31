#!/bin/sh
# GitHop hook: post-checkout  (generated - do not edit; githop doctor verifies this file)
GITHOP="@@GITHOP@@"
HOOK=post-checkout
PREV="@@PREVIOUS@@"

@@LOCATE@@
# GitHop check, only for a fresh clone (previous HEAD is the null id). It pins
# the new repo to its profile. It never blocks: its exit code is ignored.
case "$1" in
  0000000000000000000000000000000000000000|0000000000000000000000000000000000000000000000000000000000000000)
    if [ "$GITHOP_DISABLE" != "1" ]; then
      if [ -f "$GITHOP" ]; then
        "$GITHOP" check --stage clone -- "$@"
        rc=$?
        if [ $rc -ne 0 ] && [ $rc -ne 1 ] && [ $rc -ne 3 ]; then
          echo "! GitHop couldn't check this clone (it went through). Run: githop doctor" >&2
        fi
      else
        echo "! GitHop is not installed correctly (it went through). Run: githop doctor" >&2
      fi
    fi
    ;;
esac

@@CHAIN@@
