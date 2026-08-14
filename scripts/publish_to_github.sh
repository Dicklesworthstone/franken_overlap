#!/usr/bin/env bash
set -euo pipefail

REPO="${1:-Dicklesworthstone/franken_overlap}"
DESCRIPTION="Ultra-fast sparse-spectral textual overlap detection and approximate alignment in safe Rust"
GREEN='\033[0;32m'
CYAN='\033[0;36m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
RESET='\033[0m'

say() { printf "%b%s%b\n" "$CYAN" "$*" "$RESET"; }
ok() { printf "%b%s%b\n" "$GREEN" "$*" "$RESET"; }
warn() { printf "%b%s%b\n" "$YELLOW" "$*" "$RESET"; }
die() { printf "%b%s%b\n" "$RED" "$*" "$RESET" >&2; exit 1; }

command -v git >/dev/null 2>&1 || die "git is required"
command -v gh >/dev/null 2>&1 || die "GitHub CLI is required: https://cli.github.com/"
gh auth status >/dev/null 2>&1 || die "GitHub CLI is not authenticated; run: gh auth login"

git rev-parse --show-toplevel >/dev/null 2>&1 || die "run this script inside the FrankenOverlap git repository"
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

if [[ -n "$(git status --porcelain)" ]]; then
  warn "Working tree has uncommitted changes; committing the complete repository."
  git add -A
  git commit -m "Initial FrankenOverlap implementation"
fi

BRANCH="$(git branch --show-current)"
[[ -n "$BRANCH" ]] || die "the repository is in detached HEAD state"

if gh repo view "$REPO" >/dev/null 2>&1; then
  say "Repository $REPO already exists; reusing it."
else
  say "Creating public repository $REPO."
  gh repo create "$REPO" --public --description "$DESCRIPTION"
fi

REMOTE_URL="https://github.com/${REPO}.git"
if git remote get-url origin >/dev/null 2>&1; then
  CURRENT="$(git remote get-url origin)"
  if [[ "$CURRENT" != "$REMOTE_URL" && "$CURRENT" != "git@github.com:${REPO}.git" ]]; then
    warn "Updating origin from $CURRENT to $REMOTE_URL"
    git remote set-url origin "$REMOTE_URL"
  fi
else
  git remote add origin "$REMOTE_URL"
fi

say "Pushing $BRANCH to $REPO."
git push -u origin "$BRANCH"

gh repo edit "$REPO" \
  --description "$DESCRIPTION" \
  --add-topic rust \
  --add-topic text-processing \
  --add-topic approximate-matching \
  --add-topic cross-correlation \
  --add-topic edit-distance >/dev/null

ok "Published: https://github.com/$REPO"
