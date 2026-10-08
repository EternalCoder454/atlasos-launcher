# shellcheck shell=bash
# Steps of headless-look.sh (LOOK_STEPS=headless-look-latency.sh): the time
# from the start of a show to the first frame handed to KWin, 25 shows, each
# after a hide (the panel's own "first frame" debug line, in app.log).
backdrop black
for _ in $(seq 25); do
    call Show start "" "{}"
    sleep 0.6
    call Hide
    sleep 0.4
done
grep -o 'first frame [0-9]* us' "$out/app.log" | awk '{print $3}' | sort -n >"$out/first-frame-us.txt"
n=$(wc -l <"$out/first-frame-us.txt")
med=$(sed -n "$(( (n + 1) / 2 ))p" "$out/first-frame-us.txt")
res "first-frame samples $n median_us $med min_us $(head -1 "$out/first-frame-us.txt") max_us $(tail -1 "$out/first-frame-us.txt")"
