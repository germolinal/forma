#!/usr/bin/env bash
# Builds the website (Astro, into website/dist) with the documentation
# (mdBook) under dist/docs.
#
#   website/build.sh
#
# SKIP_REFERENCE_DOCS=1: do not regenerate the reference pages (`make docs`),
#   e.g. when they were generated already. They are git-ignored, so without
#   them every link under "Reference Documentation" is broken.
# SITE_URL, BASE_PATH: the origin and path the site is served from (default
#   https://germolinal.github.io and /forma). See astro.config.mjs.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
base=${BASE_PATH:-/forma}

if [ "${SKIP_REFERENCE_DOCS:-0}" != "1" ]; then
  (cd "$root/forma" && make docs)
fi

cd "$here"
[ -d node_modules ] || npm ci
npm run build

# mdBook's site-url: only its 404 page uses it.
MDBOOK_OUTPUT__HTML__SITE_URL="${base%/}/docs/" \
  mdbook build "$root/docs" --dest-dir "$here/dist/docs"

# GitHub Pages: serve files as they are (no Jekyll).
touch "$here/dist/.nojekyll"

echo "Site built in $here/dist"
