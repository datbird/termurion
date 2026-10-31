#!/bin/sh
# Tells Termurion's cursor effects that a prompt was sent (enter) or an answer came back (done).
# Port of `cursor-fx signal` (ghostty-cursor-fx, MIT): it sets the cursor color with an
# invisible low-bit change (OSC 12) that the shaders read, then resets it (OSC 112).
# Usage: fx-signal.sh enter|done [#rrggbb]   (the color is the effect's cursor_color)
# It prints nothing: Claude Code adds a UserPromptSubmit hook's output to the prompt.
kind=${1:-enter}
color=${2:-${TERMURION_FX_COLOR:-#9fd0fe}}
color=${color#\#}
tty=${TERMURION_FX_TTY:-/dev/tty}
[ -w "$tty" ] || exit 0
r=$((0x$(printf %s "$color" | cut -c1-2)))
g=$((0x$(printf %s "$color" | cut -c3-4)))
b=$((0x$(printf %s "$color" | cut -c5-6)))
if [ "$kind" = done ]; then g=$((g | 1)); b=$((b & ~1)); else g=$((g & ~1)); b=$((b | 1)); fi
# Each signal flips red's lowest bit, so a new signal is always a color change.
state="${TMPDIR:-/tmp}/termurion-fx-$(id -u)-$(basename "$(tty 2>/dev/null || echo none)")"
parity=$(( ( $(cat "$state" 2>/dev/null | cut -d: -f2 || echo 0) + 1 ) % 2 ))
r=$(( (r & ~1) | parity ))
token="$(date +%s)$$:$parity"
printf %s "$token" > "$state"
printf '\033]12;#%02x%02x%02x\007' "$r" "$g" "$b" > "$tty" 2>/dev/null
# Reset after 1.9 s unless a newer signal took over.
( sleep 1.9; if [ "$(cat "$state" 2>/dev/null)" = "$token" ]; then printf '\033]112\007' > "$tty" 2>/dev/null; rm -f "$state"; fi ) >/dev/null 2>&1 &
exit 0
