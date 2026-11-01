#!/bin/sh
# GitHop hook: prepare-commit-msg  (generated - do not edit; githop doctor verifies this file)
# git commit checks in pre-commit and exports GIT_AUTHOR_EMAIL to its hooks.
# git revert and git cherry-pick commit without pre-commit and without that
# variable, so GitHop checks those commits here (a failing prepare-commit-msg
# stops them). Their source is "message", or "merge" when a MERGE_HEAD was
# left over from a blocked merge (git merge and git pull, which set
# GIT_REFLOG_ACTION, were already checked in pre-merge-commit). Rebase is left
# to the push check: one check per replayed commit would make every rebase slow.
GITHOP="@@GITHOP@@"
HOOK=prepare-commit-msg
PREV="@@PREVIOUS@@"
SETTINGS="@@SETTINGS@@"

@@LOCATE@@
GITHOP_SEQ=""
case "$2" in
  message) GITHOP_SEQ=1 ;;
  merge) case "$GIT_REFLOG_ACTION" in merge*|pull*) ;; *) GITHOP_SEQ=1 ;; esac ;;
esac
if [ -n "$GITHOP_SEQ" ] && [ -z "${GIT_AUTHOR_EMAIL+x}" ] \
  && [ ! -d "${GIT_DIR:-.git}/rebase-merge" ] && [ ! -d "${GIT_DIR:-.git}/rebase-apply" ]; then
@@CHECK@@
fi

@@CHAIN@@
