#!/bin/sh
# Usage: scripts/notes.sh <tag>
# Prints the CHANGELOG section for the tag, failing when it is missing or empty.
set -eu
version=${1#v}
notes=$(awk -v v="$version" '
  index($0, "## [" v "]") == 1 { on = 1; next }
  on && /^## \[/ { exit }
  on { print }
' CHANGELOG.md | sed '/./,$!d')
[ -n "$notes" ] || { echo "CHANGELOG.md has no entries for $version" >&2; exit 1; }
printf '%s\n\n**Full changelog:** https://github.com/trickylab/crewii/blob/v%s/CHANGELOG.md\n' "$notes" "$version"
