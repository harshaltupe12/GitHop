#!/bin/sh
# GitHop hook: pre-push  (generated - do not edit; githop doctor verifies this file)
GITHOP="@@GITHOP@@"
HOOK=pre-push
PREV="@@PREVIOUS@@"
STRICT=@@STRICT@@

@@LOCATE@@
# pre-push gets the ref lines on stdin. Buffer them once only when another hook
# also needs them.
if [ -n "$REPO_HOOK" ] || [ -n "$PREV_HOOK" ]; then
  INPUT="$(cat)"
  BUFFERED=1
fi

@@CHECK@@

@@CHAIN@@
