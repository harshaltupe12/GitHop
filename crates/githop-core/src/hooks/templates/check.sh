# GitHop check. Exit 1 = wrong identity, so the @@WHAT@@ is blocked (githop
# also exits 1 for its own error under enforce = "block-strict"). Exit 3 =
# GitHop's own problem, already explained: let the @@WHAT@@ through (fail
# open). Any other exit, or no githop.exe: block only when settings.toml says
# block-strict, read now (not when the hooks were written), so a mode changed
# by hand applies at once. Those lines run only on failure: no extra cost.
if [ "$GITHOP_DISABLE" = "1" ]; then
@@DISABLED@@
elif [ -f "$GITHOP" ]; then
@@INVOKE@@
  rc=$?
  if [ $rc -eq 1 ]; then exit 1; fi
  if [ $rc -ne 0 ] && [ $rc -ne 3 ]; then
    githop_symbols
    if githop_strict; then
      echo "$GX GitHop couldn't check this @@WHAT@@, so it was blocked (block-strict mode). Run: githop doctor" >&2
      exit 1
    fi
    echo "$GW GitHop couldn't check this @@WHAT@@ (it went through). Run: githop doctor" >&2
  fi
else
  githop_symbols
  if githop_strict; then
    echo "$GX GitHop isn't installed correctly, so the @@WHAT@@ was blocked (block-strict mode). Run: githop doctor" >&2
    exit 1
  fi
  echo "$GW GitHop couldn't check this @@WHAT@@ (it went through). Run: githop doctor" >&2
fi
