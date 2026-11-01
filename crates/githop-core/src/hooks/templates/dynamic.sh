# A file your global git config includes sets core.hooksPath too (company
# hooks, often through includeIf). Chain to the folder git would use here
# without GitHop: the last core.hooksPath value that isn't GitHop's own.
OURS="@@OURS@@"
DYN=""
while IFS= read -r v; do
  case "$v" in "~/"*) v="$HOME/${v#??}" ;; esac
  v="${v%/}"
  if [ -n "$v" ] && [ "$v" != "$OURS" ] && [ ! "$v" -ef "$OURS" ]; then DYN="$v"; fi
done <<GITHOP_EOF
$(git config --get-all core.hooksPath 2>/dev/null)
GITHOP_EOF
if [ -n "$DYN" ]; then PREV="$DYN"; fi
