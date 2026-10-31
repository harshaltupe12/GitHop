#!/bin/sh
# GitHop hook: pre-commit  (generated - do not edit; githop doctor verifies this file)
GITHOP="@@GITHOP@@"
HOOK=pre-commit
PREV="@@PREVIOUS@@"

@@LOCATE@@
# GitHop check. Exit 1 = wrong identity, so the commit is blocked. Any other
# error is GitHop's own problem: warn and let the commit through (fail open).
if [ "$GITHOP_DISABLE" != "1" ]; then
  if [ -f "$GITHOP" ]; then
    "$GITHOP" check --stage commit -- "$@"
    rc=$?
    if [ $rc -eq 1 ]; then exit 1; fi
    if [ $rc -ne 0 ] && [ $rc -ne 3 ]; then
      echo "! GitHop couldn't check this commit (it went through). Run: githop doctor" >&2
    fi
  else
    echo "! GitHop is not installed correctly (it went through). Run: githop doctor" >&2
  fi
fi

@@CHAIN@@
