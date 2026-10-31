#!/bin/sh
# GitHop hook: pre-push  (generated - do not edit; githop doctor verifies this file)
GITHOP="@@GITHOP@@"
HOOK=pre-push
PREV="@@PREVIOUS@@"

@@LOCATE@@
# pre-push gets the ref lines on stdin. Buffer them once only when another hook
# also needs them.
if [ -n "$REPO_HOOK" ] || [ -n "$PREV_HOOK" ]; then
  INPUT="$(cat)"
  BUFFERED=1
fi

# GitHop check. Exit 1 = wrong identity, so the push is blocked. Any other
# error is GitHop's own problem: warn and let the push through (fail open).
if [ "$GITHOP_DISABLE" != "1" ]; then
  if [ -f "$GITHOP" ]; then
    if [ -n "$BUFFERED" ]; then
      { [ -z "$INPUT" ] || printf '%s\n' "$INPUT"; } | "$GITHOP" check --stage push -- "$@"
    else
      "$GITHOP" check --stage push -- "$@"
    fi
    rc=$?
    if [ $rc -eq 1 ]; then exit 1; fi
    if [ $rc -ne 0 ] && [ $rc -ne 3 ]; then
      echo "! GitHop couldn't check this push (it went through). Run: githop doctor" >&2
    fi
  else
    echo "! GitHop is not installed correctly (it went through). Run: githop doctor" >&2
  fi
fi

@@CHAIN@@
