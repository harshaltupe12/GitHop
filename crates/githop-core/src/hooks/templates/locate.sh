# Find the hooks to chain to. core.hooksPath hides .git/hooks, so GitHop runs
# them itself. The common git dir is used so linked worktrees reach the main
# repo's hooks. Cheap checks first; spawn git only when they can't tell.
if [ -z "$GIT_DIR" ] && [ -d .git ] && [ ! -f .git/commondir ]; then
  COMMON=.git
elif [ -n "$GIT_DIR" ] && [ -d "$GIT_DIR" ] && [ ! -f "$GIT_DIR/commondir" ]; then
  COMMON="$GIT_DIR"
else
  COMMON="$(git rev-parse --git-common-dir 2>/dev/null)"
fi
REPO_HOOK=""
if [ -n "$COMMON" ] && [ -f "$COMMON/hooks/$HOOK" ] && [ -x "$COMMON/hooks/$HOOK" ]; then
  REPO_HOOK="$COMMON/hooks/$HOOK"
fi
PREV_HOOK=""
if [ -n "$PREV" ] && [ -f "$PREV/$HOOK" ] && [ -x "$PREV/$HOOK" ]; then
  PREV_HOOK="$PREV/$HOOK"
fi
BUFFERED=""
INPUT=""
