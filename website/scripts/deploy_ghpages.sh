#!/usr/bin/env bash
# Deploy the Verve website to GitHub Pages (aios-pub/verve:gh-pages).
#
# Pipeline (CLAUDE.md §4):
#   1. trunk build --release  -> dist/
#   2. ensure dist/.nojekyll  (Jekyll otherwise drops *_bg.wasm filenames)
#   3. temp-clone an orphan gh-pages, copy dist into it, push (work repo clean)
#   4. verify Pages status + live URLs
#
# Usage:
#   ./scripts/deploy_ghpages.sh ["deploy message"]
#
# Notes:
#   - Force-pushes gh-pages: every deploy is a fresh orphan history by design.
#   - Never touches aios-pub/verve:main, lijingrs/verve, or any build artifact
#     in the work repo. dist/ and public/tailwind.css stay gitignored.
#   - Requires: trunk, wasm32-unknown-unknown target, npx, gh (logged in),
#     wasm-opt (`brew install binaryen` if missing).

set -euo pipefail

DEPLOY_REPO="aios-pub/verve"          # owner/repo for Pages
DEPLOY_BRANCH="gh-pages"
GIT_NAME="Jing.Li"
GIT_EMAIL="lijingrs@users.noreply.github.com"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEBSITE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"     # this script lives in website/scripts/
DIST_DIR="$WEBSITE_DIR/dist"
MSG="${1:-Deploy website}"

c() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
e() { printf '\033[1;31m!!\033[0m %s\n' "$*" >&2; }

# --- 0. preflight ----------------------------------------------------------
command -v trunk >/dev/null  || { e "trunk not found"; exit 1; }
command -v npx   >/dev/null  || { e "npx not found";  exit 1; }
command -v gh    >/dev/null  || { e "gh not found";   exit 1; }
gh auth token >/dev/null 2>&1 || { e "gh not logged in (run: gh auth login)"; exit 1; }
command -v wasm-opt >/dev/null || e "warning: wasm-opt missing — trunk may fail to fetch binaryen"

# --- 1. build --------------------------------------------------------------
c "Building (trunk build --release) in $WEBSITE_DIR"
cd "$WEBSITE_DIR"
trunk build --release

# --- 2. .nojekyll + sanity -------------------------------------------------
if [ ! -f "$DIST_DIR/.nojekyll" ]; then touch "$DIST_DIR/.nojekyll"; c "Added dist/.nojekyll"; fi
ls "$DIST_DIR"/*.wasm >/dev/null 2>&1 || { e "no .wasm in dist/ — build failed?"; ls -la "$DIST_DIR"; exit 1; }
[ -f "$DIST_DIR/index.html" ] || { e "no index.html in dist/"; exit 1; }
c "dist contents:"; ls -1 "$DIST_DIR" | sed 's/^/    /'

# --- 3. push via temp clone (work repo stays clean) ------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
c "Staging orphan $DEPLOY_BRANCH in temp $TMP"
cd "$TMP"
git init -q
git config user.name  "$GIT_NAME"
git config user.email "$GIT_EMAIL"
git checkout -q -b "$DEPLOY_BRANCH"
cp -R "$DIST_DIR/." .

TOKEN="$(gh auth token)"
git remote add deploy "https://x-access-token:${TOKEN}@github.com/${DEPLOY_REPO}.git"
git add -A
git commit -q -m "Deploy website: $MSG"
c "Pushing (force, orphan) to github.com/${DEPLOY_REPO}:${DEPLOY_BRANCH}"
git push --force -u deploy "$DEPLOY_BRANCH"
c "Push complete."

# --- 4. verify -------------------------------------------------------------
c "Pages status:"
gh api "repos/${DEPLOY_REPO}/pages" \
  --jq '"    status=\(.status) branch=\(.source.branch)/\(.source.path)"' 2>/dev/null \
  || e "could not read Pages status (may need admin scope on gh)"
c "Live URL probe (Pages may take ~1 min to rebuild):"
curl -sL -o /dev/null -w '    https://aios-pub.github.io/verve/ -> %{http_code}\n' https://aios-pub.github.io/verve/ || true
curl -sL -o /dev/null -w '    https://aios.pub/verve/         -> %{http_code}\n' https://aios.pub/verve/ || true

c "Done. See https://aios.pub/verve/"
