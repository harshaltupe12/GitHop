# Status symbols for the lines this script prints itself (GITHOP_ASCII=1 gives
# [blocked] / [warn], like githop.exe).
githop_symbols() {
  if [ -n "$GITHOP_ASCII" ] && [ "$GITHOP_ASCII" != "0" ]; then
    GX="[blocked]"; GW="[warn]"
  else
    GX="✗"; GW="!"
  fi
}
# True when settings.toml says enforce = "block-strict". Shell built-ins only
# (no process start); stops at the first [table].
githop_strict() {
  [ -n "$SETTINGS" ] && [ -f "$SETTINGS" ] || return 1
  while IFS= read -r l || [ -n "$l" ]; do
    l="${l#"${l%%[!	 ]*}"}"
    case "$l" in
      "["*) return 1 ;;
      enforce*block-strict*) return 0 ;;
    esac
  done < "$SETTINGS"
  return 1
}
