#!/bin/sh
# GitHop hook: @@HOOK@@  (generated - do not edit; githop doctor verifies this file)
# GitHop checks nothing here. This file only makes sure the repo's own hook and
# your previous global hook still run while core.hooksPath points at GitHop.
HOOK=@@HOOK@@
PREV="@@PREVIOUS@@"

@@LOCATE@@
if [ -z "$REPO_HOOK" ] && [ -z "$PREV_HOOK" ]; then exit 0; fi
@@BUFFER@@
@@CHAIN@@
