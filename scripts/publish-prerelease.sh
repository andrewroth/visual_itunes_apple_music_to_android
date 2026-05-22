#!/usr/bin/env bash
# Publish a GitHub pre-release from a CI run's artifacts.
#
# Picks up the Tauri installers + Android APK that the .github workflow
# uploads, attaches them to a new pre-release tagged at the run's commit,
# and writes platform-friendly download names. Anyone (logged-in or not)
# can then grab them from the Releases page.
#
# Usage:
#   scripts/publish-prerelease.sh              # latest successful main run
#   scripts/publish-prerelease.sh <run-id>     # specific GitHub Actions run id
#
# Requires: gh CLI (logged in), git, and write access to the repo.

set -euo pipefail

REPO="andrewroth/visual_itunes_apple_music_to_android"

usage() {
  echo "Usage: $0 [run-id]" >&2
  exit 64
}

# --- Resolve the run id -----------------------------------------------------
if [[ $# -gt 1 ]]; then usage; fi
RUN_ID="${1:-}"
if [[ -z "$RUN_ID" ]]; then
  # Most recent successful run of the "Build" workflow on main.
  RUN_ID=$(gh run list \
    --repo "$REPO" \
    --workflow Build \
    --branch main \
    --status success \
    --limit 1 \
    --json databaseId \
    --jq '.[0].databaseId')
  if [[ -z "$RUN_ID" || "$RUN_ID" == "null" ]]; then
    echo "Could not find a successful Build run on main." >&2
    exit 1
  fi
fi

echo "Using CI run: $RUN_ID"

# Commit the run was built from — used as the release target.
RUN_SHA=$(gh run view "$RUN_ID" --repo "$REPO" --json headSha --jq .headSha)
SHORT_SHA="${RUN_SHA:0:7}"
TAG="v0.2.0-pre-$SHORT_SHA"
TITLE="v0.2.0-pre ($SHORT_SHA)"
echo "Target commit: $RUN_SHA  tag: $TAG"

# Bail if a release already exists for this commit so we don't clobber it.
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "Release $TAG already exists; nothing to do." >&2
  exit 0
fi

# --- Download artifacts -----------------------------------------------------
TMP=$(mktemp -d -t musicsync-release-XXXXXX)
trap 'rm -rf "$TMP"' EXIT
echo "Downloading artifacts to $TMP"
gh run download "$RUN_ID" --repo "$REPO" --dir "$TMP"

# Resolve each installer path. We glob by extension/format because
# Tauri normalises the productName differently per platform (deb/rpm
# kebab-case it; dmg/AppImage/nsis preserve spaces), so hardcoding
# filenames is brittle. Errors out if a glob matches zero or >1 file
# so the script never partially-publishes a release.
find_one() {
  local label="$1"
  shift
  local matches=()
  while IFS= read -r -d '' f; do matches+=("$f"); done < <(find "$@" -print0 2>/dev/null)
  if [[ ${#matches[@]} -eq 0 ]]; then
    echo "Missing expected $label artifact under $* " >&2
    exit 1
  fi
  if [[ ${#matches[@]} -gt 1 ]]; then
    echo "Ambiguous $label artifact — multiple matches:" >&2
    printf '  %s\n' "${matches[@]}" >&2
    exit 1
  fi
  printf '%s' "${matches[0]}"
}

ANDROID_SRC=$(find_one "android apk" "$TMP/musicsync-android-debug-apk" -name '*.apk' -type f)
WIN_NSIS=$(find_one "windows nsis setup" "$TMP/musicsync-windows" -name '*-setup.exe' -type f)
WIN_MSI=$(find_one "windows msi"        "$TMP/musicsync-windows" -name '*.msi' -type f)
MAC_ARM=$(find_one "macOS arm64 dmg"    "$TMP/musicsync-macos-arm64" -name '*_aarch64.dmg' -type f)
MAC_X64=$(find_one "macOS x86_64 dmg"   "$TMP/musicsync-macos-x86_64" -name '*_x64.dmg' -type f)
LIN_APPIMAGE=$(find_one "linux AppImage" "$TMP/musicsync-linux" -name '*.AppImage' -type f)
LIN_DEB=$(find_one "linux deb"          "$TMP/musicsync-linux" -name '*.deb' -type f)
LIN_RPM=$(find_one "linux rpm"          "$TMP/musicsync-linux" -name '*.rpm' -type f)

# Rename the APK to a friendlier user-facing filename without touching
# the original (cp into the tmp dir so cleanup is automatic).
ANDROID_APK="$TMP/Viamta-Music-Sync-0.2.0-android.apk"
cp "$ANDROID_SRC" "$ANDROID_APK"

# --- Push the tag -----------------------------------------------------------
# `gh release create` accepts `--target <sha>` but only if the tag is a
# fresh tag *and* the SHA is on a default branch tip; in practice
# pushing the tag first is the most reliable path.
if git rev-parse --verify "$TAG" >/dev/null 2>&1; then
  echo "Local tag $TAG already exists, reusing."
else
  echo "Tagging $RUN_SHA as $TAG"
  git tag "$TAG" "$RUN_SHA"
fi
echo "Pushing tag"
git push origin "refs/tags/$TAG"

# --- Create the release -----------------------------------------------------
NOTES=$(cat <<EOF
Pre-release built from CI run [$RUN_ID](https://github.com/$REPO/actions/runs/$RUN_ID) at commit $SHORT_SHA.

## Downloads

| Platform | File suffix |
|---|---|
| Android | `*.apk` |
| Windows | `*-setup.exe` (NSIS) or `*.msi` |
| macOS (Apple Silicon) | `*_aarch64.dmg` |
| macOS (Intel) | `*_x64.dmg` |
| Linux | `*.AppImage`, `*.deb`, or `*.rpm` |
EOF
)

echo "Creating release $TAG"
gh release create "$TAG" \
  --repo "$REPO" \
  --prerelease \
  --title "$TITLE" \
  --notes "$NOTES" \
  "$ANDROID_APK" \
  "$WIN_NSIS" \
  "$WIN_MSI" \
  "$MAC_ARM" \
  "$MAC_X64" \
  "$LIN_APPIMAGE" \
  "$LIN_DEB" \
  "$LIN_RPM"

echo
echo "Done. Browse it:"
echo "  https://github.com/$REPO/releases/tag/$TAG"
