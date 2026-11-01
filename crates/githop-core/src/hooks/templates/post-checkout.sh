#!/bin/sh
# GitHop hook: post-checkout  (generated - do not edit; githop doctor verifies this file)
GITHOP="@@GITHOP@@"
HOOK=post-checkout
PREV="@@PREVIOUS@@"
SETTINGS="@@SETTINGS@@"

@@LOCATE@@
@@HELPERS@@
# GitHop check, only for a fresh clone (previous HEAD is the null id) or a
# submodule checkout (its git dir is under .git/modules/; git checks a new
# submodule out with a non-null previous HEAD). It pins the repo to its
# profile when an owner or folder rule matches. It never blocks: its exit
# code is ignored.
GITHOP_FRESH=""
case "$1" in
  0000000000000000000000000000000000000000|0000000000000000000000000000000000000000000000000000000000000000)
    GITHOP_FRESH=1 ;;
esac
case "$COMMON" in
  */.git/modules/*|.git/modules/*) GITHOP_FRESH=1; GITHOP_SUBMODULE=1; export GITHOP_SUBMODULE ;;
esac
if [ -n "$GITHOP_FRESH" ] && [ "$GITHOP_DISABLE" != "1" ]; then
  if [ -f "$GITHOP" ]; then
    "$GITHOP" check --stage clone -- "$@"
    rc=$?
    if [ $rc -ne 0 ] && [ $rc -ne 1 ] && [ $rc -ne 3 ]; then
      githop_symbols
      echo "$GW GitHop couldn't check this clone (it went through). Run: githop doctor" >&2
    fi
  else
    githop_symbols
    echo "$GW GitHop couldn't check this clone (it went through). Run: githop doctor" >&2
  fi
fi

@@CHAIN@@
