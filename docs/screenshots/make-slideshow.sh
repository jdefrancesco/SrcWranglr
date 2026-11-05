#!/usr/bin/env bash
# Builds docs/screenshots/slideshow.webp, the README's feature tour, from the
# screenshots here: each one fitted on a same-size frame with a caption bar.
# Needs ImageMagick (magick) and libwebp (img2webp): brew install imagemagick webp
set -euo pipefail

cd "$(dirname "$0")"

readonly WIDTH=1280
readonly HEIGHT=860
readonly CAPTION_HEIGHT=88
readonly MARGIN=20
readonly SECONDS_PER_SLIDE_MS=3500
readonly BACKGROUND='#24262b'
readonly BAR='#17181b'
readonly FONT='/System/Library/Fonts/Supplemental/Arial.ttf'
readonly BOLD='/System/Library/Fonts/Supplemental/Arial Bold.ttf'

# screenshot | title | description
readonly SLIDES=(
  "method-trail.png|Trail view|Callers on the left, the symbol in the middle, what it uses on the right"
  "class-view.png|Class view|A class with its members, who uses it, and what it depends on"
  "ss-src.png|Linked source and notes|Every name in the code is clickable; notes stay with the code they're about"
  "ss-open-ed.png|Open in Editor|The same file and line in your \$VISUAL / \$EDITOR, VS Code, or vim in a terminal"
  "ss-risky-calls.png|Findings|Risky calls judged by their arguments, plus semgrep and cppcheck, by severity"
  "ss-triage-1.png|Project Triage|Languages and size, repository links, binaries and their hardening"
  "ss-ipc.png|IPC & Capabilities|Which IPC mechanisms the code uses, and every place it calls them"
  "ss-fuzz.png|Fuzz Targets|Functions worth fuzzing, with their reasons; Harness… drafts a libFuzzer harness"
)

frames=$(mktemp -d)
trap 'rm -rf "$frames"' EXIT

image_height=$((HEIGHT - CAPTION_HEIGHT - 2 * MARGIN))
image_width=$((WIDTH - 2 * MARGIN))
count=${#SLIDES[@]}
index=0
for slide in "${SLIDES[@]}"; do
  IFS='|' read -r file title description <<<"$slide"
  index=$((index + 1))
  magick -size "${WIDTH}x${HEIGHT}" "xc:${BACKGROUND}" \
    \( "$file" -resize "${image_width}x${image_height}" \) \
    -gravity center -geometry "+0-$((CAPTION_HEIGHT / 2))" -composite \
    -fill "$BAR" -draw "rectangle 0,$((HEIGHT - CAPTION_HEIGHT)) ${WIDTH},${HEIGHT}" \
    -gravity southwest \
    -font "$BOLD" -pointsize 26 -fill white -annotate "+32+46" "$title" \
    -font "$FONT" -pointsize 19 -fill '#c9ccd1' -annotate "+32+18" "$description" \
    -gravity southeast -pointsize 17 -fill '#8a8f98' -annotate "+32+46" "${index} / ${count}" \
    "$frames/$(printf '%02d' "$index").png"
done

img2webp -loop 0 -d "$SECONDS_PER_SLIDE_MS" -lossy -q 82 -m 6 "$frames"/*.png -o slideshow.webp
echo "wrote $(pwd)/slideshow.webp"
