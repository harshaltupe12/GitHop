#!/bin/sh
# GitHop hook: pre-merge-commit  (generated - do not edit; githop doctor verifies this file)
# git merge runs this hook, not pre-commit, before it makes a merge commit
# (githooks(5)), so GitHop runs the same commit check here.
GITHOP="@@GITHOP@@"
HOOK=pre-merge-commit
PREV="@@PREVIOUS@@"
STRICT=@@STRICT@@

@@LOCATE@@
@@CHECK@@

@@CHAIN@@
