#!/usr/bin/env bash
# Written by `stado release catalog adopt --kind npm`: the formatting gate
# `.wisent-release.json` declares and `stado quality`
# reads. With --check it lists every JavaScript and TypeScript source Biome
# would change and exits nonzero, writing nothing; without it, `stado quality
# format` runs it to write them. The formatter is npm's @biomejs/biome at the
# release BIOME_RELEASE names (https://www.npmjs.com/package/@biomejs/biome),
# run through npx so the exported tree `stado quality check` reads needs no
# node_modules; the style is Biome's own defaults, so no width or indent is
# chosen here. The sources are the paths {{PRODUCT}}'s package.json ships
# (`files`).
set -euo pipefail
cd "$(dirname "$0")/.."

BIOME_PACKAGE="@biomejs/biome"
BIOME_RELEASE="2.5.15"
SOURCES=({{SOURCES}})
BIOME=(npx --yes "$BIOME_PACKAGE@$BIOME_RELEASE" format --no-errors-on-unmatched)

case "${1:-}" in
  --check)
    exec "${BIOME[@]}" "${SOURCES[@]}"
    ;;
  '')
    exec "${BIOME[@]}" --write "${SOURCES[@]}"
    ;;
  *)
    printf 'usage: %s [--check]\n' "$0" >&2
    exit 64
    ;;
esac
