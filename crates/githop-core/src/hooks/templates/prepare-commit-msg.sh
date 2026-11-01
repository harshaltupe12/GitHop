#!/bin/sh
# GitHop hook: prepare-commit-msg  (generated - do not edit; githop doctor verifies this file)
# git commit checks in pre-commit and exports GIT_AUTHOR_EMAIL to its hooks.
# git revert and git cherry-pick commit without pre-commit and without that
# variable, so GitHop checks those commits here (a failing prepare-commit-msg
# stops them). Rebase is left to the push check: one check per replayed commit
# would make every rebase slow.
GITHOP="@@GITHOP@@"
HOOK=prepare-commit-msg
PREV="@@PREVIOUS@@"
STRICT=@@STRICT@@

@@LOCATE@@
if [ "$2" = "message" ] && [ -z "${GIT_AUTHOR_EMAIL+x}" ] \
  && [ ! -d "${GIT_DIR:-.git}/rebase-merge" ] && [ ! -d "${GIT_DIR:-.git}/rebase-apply" ]; then
@@CHECK@@
fi

@@CHAIN@@
