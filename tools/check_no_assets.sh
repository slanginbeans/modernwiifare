#!/usr/bin/env bash
# Blocks commits that contain retail game data or converted output.
# Usage: tools/check_no_assets.sh            (checks staged files)
#        ln -s ../../tools/check_no_assets.sh .git/hooks/pre-commit
set -euo pipefail

banned_ext='\.(ff|iwd|xex|iso|xiso|god|gsc|csc|menu|str|bik|pak|tpl|wpk|dds|iwi|xpr)$'
banned_dirs='^(game_root|mw2|out|packs)/'
max_kb=512   # code and docs are never this large; data blobs usually are

fail=0
while IFS= read -r f; do
  [ -z "$f" ] && continue
  if [[ "$f" =~ $banned_ext ]] || [[ "$f" =~ $banned_dirs ]]; then
    echo "BLOCKED (game data type/path): $f"; fail=1; continue
  fi
  if [ -f "$f" ]; then
    kb=$(( $(wc -c < "$f") / 1024 ))
    if [ "$kb" -gt "$max_kb" ]; then
      echo "BLOCKED (>${max_kb}KB, looks like data): $f"; fail=1
    fi
    # IW4 fastfile magic inside a binary (non-text) file
    if ! grep -Iq . "$f" 2>/dev/null && grep -qa 'IWff' "$f" 2>/dev/null; then
      echo "BLOCKED (contains fastfile magic): $f"; fail=1
    fi
  fi
done < <(git diff --cached --name-only --diff-filter=ACM)

if [ "$fail" -ne 0 ]; then
  echo "Commit refused: retail or derived game data must stay out of the repo." >&2
  exit 1
fi
echo "asset guard: ok"
