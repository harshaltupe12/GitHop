# Chain: the repo's own hook, then your previous global hook. A non-zero exit
# from either is passed on, so their checks still block as before.
if [ -n "$BUFFERED" ]; then
  if [ -n "$REPO_HOOK" ]; then
    { [ -z "$INPUT" ] || printf '%s\n' "$INPUT"; } | "$REPO_HOOK" "$@" || exit $?
  fi
  if [ -n "$PREV_HOOK" ]; then
    { [ -z "$INPUT" ] || printf '%s\n' "$INPUT"; } | "$PREV_HOOK" "$@" || exit $?
  fi
else
  if [ -n "$REPO_HOOK" ]; then
    "$REPO_HOOK" "$@" || exit $?
  fi
  if [ -n "$PREV_HOOK" ]; then
    "$PREV_HOOK" "$@" || exit $?
  fi
fi
exit 0
