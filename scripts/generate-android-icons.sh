#!/usr/bin/env bash
# Generate Android launcher icons from the project-root logo-sq.png.
#
# Writes ic_launcher.png + ic_launcher_round.png into each mipmap-XXXdpi
# directory at the canonical sizes, plus a simple adaptive-icon XML so
# Android 8+ renders the logo cleanly inside whatever device-shape mask
# the launcher uses.
#
# Run from anywhere; this script resolves paths relative to its own
# location so it works whether you invoke it as `scripts/...` or with an
# absolute path.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
SRC="$ROOT/logo-sq.png"
RES="$ROOT/android/app/src/main/res"

if [[ ! -f "$SRC" ]]; then
  echo "Missing source: $SRC" >&2
  exit 1
fi
if ! command -v convert >/dev/null; then
  echo "ImageMagick 'convert' not found. Install with: sudo apt install imagemagick" >&2
  exit 1
fi

# Canonical Android launcher-icon sizes (px) keyed by density.
declare -A SIZES=(
  [mdpi]=48
  [hdpi]=72
  [xhdpi]=96
  [xxhdpi]=144
  [xxxhdpi]=192
)
# Adaptive-icon foreground is rendered inside a 108dp container with
# only the center 72dp visible (the rest is "safe zone" clipped by the
# launcher mask). At each density the foreground PNG is 108dp scaled to
# px. The logo itself goes in the central 72dp region so it never gets
# masked off.
declare -A FG_SIZES=(
  [mdpi]=108
  [hdpi]=162
  [xhdpi]=216
  [xxhdpi]=324
  [xxxhdpi]=432
)
SAFE_FRACTION=$(awk 'BEGIN{print 72/108}')   # 0.6667

# Pad the (possibly rectangular) logo to a square first so resize
# preserves the design without stretching. Store as an intermediate
# 1024x1024 PNG with a transparent letterbox — we then resize from that
# for every output size to maximise quality.
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Source: $(identify -format '%wx%h' "$SRC") $SRC"
convert "$SRC" \
  -background none -gravity center \
  -resize 1024x1024 \
  -extent 1024x1024 \
  "$TMP/logo_square.png"

# A version padded down so the logo sits inside the adaptive-icon safe
# zone. Used for foreground PNGs. We scale the logo to ~66% of the
# canvas and centre it; the outer ring is transparent so the launcher
# mask can clip it.
INNER=$(awk -v frac="$SAFE_FRACTION" 'BEGIN{printf "%d", 1024*frac}')
convert "$SRC" \
  -background none -gravity center \
  -resize "${INNER}x${INNER}" \
  -extent 1024x1024 \
  "$TMP/logo_safezone.png"

for density in "${!SIZES[@]}"; do
  px=${SIZES[$density]}
  out_dir="$RES/mipmap-$density"
  mkdir -p "$out_dir"
  for name in ic_launcher ic_launcher_round; do
    out="$out_dir/$name.png"
    convert "$TMP/logo_square.png" -resize "${px}x${px}" "$out"
    echo "  wrote $out (${px}x${px})"
  done
  # Adaptive-icon foreground layer at the same density.
  fg_px=${FG_SIZES[$density]}
  fg_out="$out_dir/ic_launcher_foreground.png"
  convert "$TMP/logo_safezone.png" -resize "${fg_px}x${fg_px}" "$fg_out"
  echo "  wrote $fg_out (${fg_px}x${fg_px}, safe-zone padded)"
done

# Adaptive-icon background (flat white) + foreground reference XMLs.
# Two of these (round + regular) so launchers that ask for either get
# the same shape. Existing mipmap-anydpi-v26 directory is fine to
# overwrite — it's pure XML.
mkdir -p "$RES/mipmap-anydpi-v26" "$RES/values"
cat > "$RES/mipmap-anydpi-v26/ic_launcher.xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
</adaptive-icon>
EOF
cat > "$RES/mipmap-anydpi-v26/ic_launcher_round.xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
</adaptive-icon>
EOF

# colors.xml — flat white background behind the logo for the adaptive
# icon's "background" layer. Create or update without trashing other
# colour entries the user may have added.
COLORS="$RES/values/colors.xml"
if [[ -f "$COLORS" ]] && grep -q 'name="ic_launcher_background"' "$COLORS"; then
  : # already present
elif [[ -f "$COLORS" ]]; then
  # Inject a new entry before </resources>.
  python3 - "$COLORS" <<'PY'
import sys, re
path = sys.argv[1]
text = open(path).read()
text = re.sub(r"</resources>", '    <color name="ic_launcher_background">#FFFFFFFF</color>\n</resources>', text)
open(path, "w").write(text)
PY
else
  cat > "$COLORS" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="ic_launcher_background">#FFFFFFFF</color>
</resources>
EOF
fi
echo "  wrote/updated $COLORS"

echo
echo "Done. Make sure AndroidManifest.xml has:"
echo "  android:icon=\"@mipmap/ic_launcher\""
echo "  android:roundIcon=\"@mipmap/ic_launcher_round\""
