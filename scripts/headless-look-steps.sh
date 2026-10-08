# shellcheck shell=bash disable=SC2154  # variables and helpers come from headless-look.sh, which sources this
# The steps of headless-look.sh (sourced inside its session): $out, $scheme,
# $scale, $face, $sw, $sh and the helpers (call, shot, move, setconf, backdrop,
# lastblur, res, fail) are its.
measure=$here/headless-measure.py
tag=$scheme-$scale

# The panel's background, away from any text: just inside its left edge,
# halfway up the screen (the panel is centred, 648 logical pixels wide).
bgx=$(awk -v w="$sw" -v s="$scale" 'BEGIN { printf "%d", w / 2 - 316 * s }')
bgy=$((sh / 2))
text=$(if [ "$scheme" = dark ]; then echo 235,236,240; else echo 35,38,39; fi)

blur_is() { # <expected 0|1> <what>: the last blur request the launcher made
    local line
    line=$(lastblur)
    case $line in
        "blur enable=$1 "*) res "ok: $2 ($line)" ;;
        *) fail "$2: expected blur enable=$1, the last request was: ${line:-none}" ;;
    esac
}
shown() { # a shot of the panel over the backdrop that is up
    shot "$tag-$1"
}

# ---- 1. The compositor offers no blur yet (the launcher starts at login,
# ---- before KWin's effects): the panel is opaque and asks for no blur.
backdrop pattern
call Show start "" "{}"
shown 1-no-blur-opaque
blur_is 0 "blur not available: no blur asked"
call Hide

# ---- 2. KWin's blur comes on while the launcher runs; the next show finds
# ---- out (Appearance.refresh): translucent, with a blur region.
echo 1 >"$BLURSHIM_STATE"
call Show start "" "{}"
shown 2-late-blur-translucent
blur_is 1 "blur became available: blur asked at the next show"
rects=$(lastblur | sed -n 's/.* rects=\([0-9]*\).*/\1/p')
res "blur region: ${rects:-0} rectangles, $(lastblur | sed -n 's/.*window=\([0-9x]*\).*/\1/p') window"

# ---- 3. The switch in Settings turned off, then on again, while shown.
setconf false
shown 3-switch-off-live-opaque
blur_is 0 "Transparency off while shown: blur taken away"
setconf true
shown 4-switch-on-live-translucent
blur_is 1 "Transparency on while shown: blur asked again"
call Hide

# ---- 4. The panel's alpha and the text's contrast, from the panel over a
# ---- black, a white and the pattern backdrop; then the blur as it would look.
for b in black white; do
    backdrop $b
    call Show start "" "{}"; sleep 0.8; shot "e-$b"; call Hide
done
backdrop pattern
sleep 0.5
shot e-bd
call Show start "" "{}"; shot e-pattern
res "translucent, sampled at $bgx,$bgy:"
python3 "$measure" alpha "$out/e-black.png" "$out/e-white.png" "$bgx" "$bgy" | tee -a "$out/results.txt"
python3 "$measure" contrast "$out/e-black.png" "$out/e-white.png" "$bgx" "$bgy" "$text" 1.0 | sed 's/^/text  /' | tee -a "$out/results.txt"
python3 "$measure" contrast "$out/e-black.png" "$out/e-white.png" "$bgx" "$bgy" "$text" 0.65 | sed 's/^/muted /' | tee -a "$out/results.txt"
bash "$here/headless-emulate.sh" "$out" e "$scale" 12 >>"$out/steps.log" 2>&1 || fail "emulating the blur"
cp "$out/e-blurred.png" "$out/$tag-5-blurred-emulated.png"
cp "$out/e-pattern.png" "$out/$tag-5-sharp-qpainter.png"
cp "$out/e-mask.png" "$out/$tag-5-blur-region.png"
# Search mode, translucent.
call Show search "set" "{}"
shown 6-search-translucent
call Hide

# ---- 5. Opaque (Transparency off): no blur, alpha 1.
setconf false
for b in black white; do
    backdrop $b
    call Show start "" "{}"; sleep 0.8; shot "o-$b"; call Hide
done
backdrop pattern
call Show start "" "{}"; sleep 0.3
blur_is 0 "Transparency off: no blur asked"
res "opaque, sampled at $bgx,$bgy:"
python3 "$measure" alpha "$out/o-black.png" "$out/o-white.png" "$bgx" "$bgy" | tee -a "$out/results.txt"
python3 "$measure" contrast "$out/o-black.png" "$out/o-white.png" "$bgx" "$bgy" "$text" 1.0 | sed 's/^/text  /' | tee -a "$out/results.txt"
python3 "$measure" contrast "$out/o-black.png" "$out/o-white.png" "$bgx" "$bgy" "$text" 0.65 | sed 's/^/muted /' | tee -a "$out/results.txt"
shown 7-opaque
call Show search "set" "{}"
shown 8-search-opaque
call Hide

# ---- 6. The compositor's blur goes away again while the launcher runs.
setconf true
echo 0 >"$BLURSHIM_STATE"
call Show start "" "{}"
shown 9-blur-lost-opaque
blur_is 0 "blur lost: opaque, no blur asked"
call Hide
echo 1 >"$BLURSHIM_STATE"

# ---- 7. Hover highlights of the account and power buttons (opaque panel, so
# ---- the picture and the highlight are measured on one flat colour).
setconf false
backdrop black
call Show start "" "{}"
move 5 5
shot hover-none
av=$(python3 "$measure" avatar "$out/hover-none.png" "$face" | tee -a "$out/results.txt" | sed -n 's/^AVATAR //p')
read -r ax ay <<<"$av"
rad=$(awk -v s="$scale" 'BEGIN { printf "%d", 24 * s }')
grad=$(awk -v s="$scale" 'BEGIN { printf "%d", 17 * s }')
prad=$(awk -v s="$scale" 'BEGIN { printf "%d", 21 * s }')
res "hover avatar scale $scale"
move "${ax%.*}" "${ay%.*}"
shot hover-avatar
python3 "$measure" highlight "$out/hover-none.png" "$out/hover-avatar.png" "$ax" "$ay" "$rad" | tee -a "$out/results.txt"
python3 "$measure" centred "$out/hover-none.png" "$out/hover-avatar.png" "$ax" "$ay" "$rad" "$face" | tee -a "$out/results.txt"
# The power button, to the right of the avatar: its glyph and its highlight.
pwx=$(awk -v a="$ax" -v s="$scale" 'BEGIN { printf "%d", a + 39 * s }')
res "hover power scale $scale"
pgx=$(python3 "$measure" glyph "$out/hover-none.png" "$pwx" "${ay%.*}" "$grad" | tee -a "$out/results.txt" | sed -n 's/^GLYPH //p')
read -r gx gy <<<"$pgx"
move "$pwx" "${ay%.*}"
shot hover-power
python3 "$measure" highlight "$out/hover-none.png" "$out/hover-power.png" "$gx" "$gy" "$rad" | tee -a "$out/results.txt"
# The tooltip, after a longer hover.
move 5 5
move "${ax%.*}" "${ay%.*}"
sleep 1.5
shot hover-avatar-tip
# Pressed: the button held down on the avatar.
move "${ax%.*}" "${ay%.*}"
DISPLAY=$xdisplay xdotool mousedown 1; sleep 0.5
shot press-avatar
DISPLAY=$xdisplay xdotool mouseup 1
res "press avatar scale $scale"
python3 "$measure" centred "$out/hover-none.png" "$out/press-avatar.png" "$ax" "$ay" "$prad" "$face" | tee -a "$out/results.txt"
call Hide
