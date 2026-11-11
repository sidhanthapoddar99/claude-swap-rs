#!/usr/bin/env bash
# Make the README's pictures of the dashboard.
#
# The dashboard draws itself with made-up accounts (the `readme_shots` test),
# a headless browser turns each page into a PNG, and ImageMagick trims it and
# writes the webp. Needs: cargo, a Chromium-family browser, and `magick`.
#
#   scripts/shots.sh            # every page, and the herdr layout's preview
#   BROWSER_BIN=chromium scripts/shots.sh
set -euo pipefail
cd "$(dirname "$0")/.."

browser="${BROWSER_BIN:-}"
if [ -z "$browser" ]; then
  for b in chromium chromium-browser google-chrome microsoft-edge; do
    if command -v "$b" >/dev/null 2>&1; then browser="$b"; break; fi
  done
fi
[ -n "$browser" ] || { echo "no Chromium-family browser found; set BROWSER_BIN" >&2; exit 1; }
command -v magick >/dev/null 2>&1 || { echo "ImageMagick (magick) is needed" >&2; exit 1; }

COLORTERM=truecolor cargo test --quiet readme_shots -- --ignored >/dev/null

out=docs/img
mkdir -p "$out"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for page in target/shots/*.html; do
  name="$(basename "$page" .html)"
  "$browser" --headless=new --disable-gpu --hide-scrollbars \
    --force-device-scale-factor=2 --window-size=1500,900 \
    --screenshot="$tmp/$name.png" "file://$PWD/$page" >/dev/null 2>&1
  # The page's own background is a colour nothing else has, so trim finds the screen.
  magick "$tmp/$name.png" -trim +repage -quality 90 -define webp:method=6 "$out/$name.webp"
  echo "$out/$name.webp"
done

# The herdr layout is a drawing with made-up spaces and accounts: a real
# screenshot would show whose machine it was taken on.
"$browser" --headless=new --disable-gpu --hide-scrollbars \
  --force-device-scale-factor=1 --window-size=1500,960 \
  --screenshot="$tmp/herdr.png" "file://$PWD/scripts/herdr-preview.html" >/dev/null 2>&1
magick "$tmp/herdr.png" -trim +repage integrations/herdr/preview.png
magick integrations/herdr/preview.png -quality 90 -define webp:method=6 "$out/herdr.webp"
echo "integrations/herdr/preview.png"
echo "$out/herdr.webp"
