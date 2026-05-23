#!/usr/bin/env bash
# Propagate /VERSION into every place that hardcodes a version string.
#
# Single source of truth: the /VERSION file at the repo root. Edit
# that, then run this script before committing — every place that
# needs the version (Cargo workspace, Tauri config, Gradle, frontend
# build tag) is updated in lock-step.
#
# Android `versionCode` is bumped by 1 on every run since the Play
# Store rejects re-using a code. If you only want to update the
# display version without bumping code, pass --no-bumpcode.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
VERSION_FILE="$ROOT/VERSION"

if [[ ! -f "$VERSION_FILE" ]]; then
  echo "Missing $VERSION_FILE" >&2
  exit 1
fi

V=$(tr -d '[:space:]' < "$VERSION_FILE")
if [[ -z "$V" ]]; then
  echo "$VERSION_FILE is empty" >&2
  exit 1
fi

BUMP_CODE=1
for arg in "$@"; do
  case "$arg" in
    --no-bumpcode) BUMP_CODE=0 ;;
    *) echo "Unknown arg: $arg" >&2; exit 64 ;;
  esac
done

echo "Syncing version $V into project files…"

# --- Cargo workspace ----------------------------------------------------
# version = "X.Y.Z" line under [workspace.package] in tauri/Cargo.toml
CARGO="$ROOT/tauri/Cargo.toml"
sed -i.bak -E 's/^version = "[^"]+"/version = "'"$V"'"/' "$CARGO"
rm -f "$CARGO.bak"
echo "  $CARGO -> $V"

# --- Tauri config -------------------------------------------------------
TAURI_CONF="$ROOT/tauri/src-tauri/tauri.conf.json"
sed -i.bak -E 's/("version": )"[^"]+"/\1"'"$V"'"/' "$TAURI_CONF"
rm -f "$TAURI_CONF.bak"
echo "  $TAURI_CONF -> $V"

# --- Android Gradle -----------------------------------------------------
GRADLE="$ROOT/android/app/build.gradle.kts"
sed -i.bak -E 's/(versionName = )"[^"]+"/\1"'"$V"'"/' "$GRADLE"
if [[ $BUMP_CODE -eq 1 ]]; then
  # Bump versionCode by 1. Play Store requires monotonic codes.
  CUR=$(grep -E '^\s*versionCode\s*=\s*[0-9]+' "$GRADLE" | grep -oE '[0-9]+' | head -1)
  NEW=$((CUR + 1))
  sed -i.bak -E "s/^(\s*versionCode\s*=\s*)[0-9]+/\1$NEW/" "$GRADLE"
  echo "  $GRADLE -> $V (versionCode $CUR -> $NEW)"
else
  echo "  $GRADLE -> $V (versionCode unchanged)"
fi
rm -f "$GRADLE.bak"

# --- Frontend build tag -------------------------------------------------
# Build tag is "X.Y.Z-pre-YYYY-MM-DD" so the user can see at a glance
# which build is loaded. Date refreshes on every sync.
MAIN_JS="$ROOT/tauri/src/main.js"
TODAY=$(date +%Y-%m-%d)
sed -i.bak -E "s/(const MUSICSYNC_BUILD = )\"[^\"]+\";/\1\"$V-pre-$TODAY\";/" "$MAIN_JS"
rm -f "$MAIN_JS.bak"
echo "  $MAIN_JS -> $V-pre-$TODAY"

echo
echo "Done. Review the changes with:  git diff"
echo "Then commit and push. publish-prerelease.sh reads VERSION too,"
echo "so the GitHub release tag will match."
