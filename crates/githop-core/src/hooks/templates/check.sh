# GitHop check. Exit 1 = wrong identity, so the @@WHAT@@ is blocked. Any other
# error is GitHop's own problem: warn and let the @@WHAT@@ through (fail open).
# With enforce = block-strict (STRICT=1) a GitHop problem blocks it instead.
if [ "$GITHOP_DISABLE" != "1" ]; then
  if [ -f "$GITHOP" ]; then
@@INVOKE@@
    rc=$?
    if [ $rc -eq 1 ]; then exit 1; fi
    if [ $rc -ne 0 ]; then
      if [ "$STRICT" = "1" ]; then
        echo "✗ GitHop couldn't check this @@WHAT@@, so it was blocked (block-strict mode). Run: githop doctor" >&2
        exit 1
      fi
      if [ $rc -ne 3 ]; then
        echo "! GitHop couldn't check this @@WHAT@@ (it went through). Run: githop doctor" >&2
      fi
    fi
  elif [ "$STRICT" = "1" ]; then
    echo "✗ GitHop is not installed correctly, so the @@WHAT@@ was blocked (block-strict mode). Run: githop doctor" >&2
    exit 1
  else
    echo "! GitHop is not installed correctly (it went through). Run: githop doctor" >&2
  fi
fi
