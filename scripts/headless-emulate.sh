#!/bin/bash
# Draws what the compositor's blur would show, from shots of a headless run
# (scripts/headless-look.sh; ImageMagick only). KWin's QPainter compositing
# has no blur, so the panel's alpha is drawn over the sharp backdrop. With the
# panel over a black and over a white backdrop (premultiplied colour P, and
# P + (1 - alpha) * 255), each pixel's colour and transparency are known, and
#   blurred = P + (1 - alpha) * blur(backdrop),
# inside the region the launcher asked KWin to blur (blur.log), is what a
# blurring compositor shows (KWin's blur adds a little saturation and noise
# on top).
#   headless-emulate.sh <dir> <prefix> <scale> <sigma>
# Reads <dir>/<prefix>-black.png, -white.png, -bd.png (the backdrop alone) and
# -pattern.png (the panel over the backdrop, sharp), and blur.log; writes
# <prefix>-blurred.png, <prefix>-mask.png and <prefix>-geometry.txt.
set -euo pipefail

dir=${1:?dir}
prefix=${2:?prefix}
scale=${3:?scale}
sigma=${4:-12}
cd "$dir"

# The panel's box on the screen: what is not black over the black backdrop.
geo=$(convert "$prefix-black.png" -fuzz 1% -trim -format '%wx%h%X%Y' info:)
echo "$geo" >"$prefix-geometry.txt"
x0=$(convert "$prefix-black.png" -fuzz 1% -trim -format '%X' info:)
y0=$(convert "$prefix-black.png" -fuzz 1% -trim -format '%Y' info:)
x0=${x0#+}
y0=${y0#+}
size=$(identify -format '%wx%h' "$prefix-black.png")

# The mask: the last blur request's rectangles (logical pixels of the panel's
# surface) at the panel's place on the screen (physical pixels).
# The last one with the panel's own size (Search and Start differ).
win=$(echo "$geo" | awk -v sc="$scale" -F'[x+]' '{ printf "%dx%d", $1 / sc + 0.5, $2 / sc + 0.5 }')
line=$(grep "blur enable=1 window=$win " blur.log | tail -1)
[ -n "$line" ] || { echo "no blur request with blur on for a $win window in blur.log" >&2; exit 1; }
draw=$(awk -v sc="$scale" -v x0="$x0" -v y0="$y0" '{
    for (i = 5; i <= NF; i++) {
        split($i, r, ",")
        printf "rectangle %d,%d %d,%d ", x0 + r[1] * sc, y0 + r[2] * sc, x0 + (r[1] + r[3]) * sc - 1, y0 + (r[2] + r[4]) * sc - 1
    }
}' <<<"$line")
convert -size "$size" xc:black -fill white -draw "$draw" "$prefix-mask.png"

# The backdrop, blurred; the panel's colour and what it lets through.
convert "$prefix-bd.png" -gaussian-blur "0x$(awk -v s="$scale" -v g="$sigma" 'BEGIN { print g * s }')" "$prefix-bd-blur.png"
convert "$prefix-black.png" "$prefix-white.png" "$prefix-bd-blur.png" -fx 'u + (v - u) * u[2]' "$prefix-emu.png"
# Inside the region: the blurred version; outside it: the sharp composite.
convert "$prefix-pattern.png" "$prefix-emu.png" "$prefix-mask.png" -composite "$prefix-blurred.png"
