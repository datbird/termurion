// Teleport cursor for Ghostty (`custom-shader`).
// Jump the cursor along its row (delete-all, Home; JUMP_EFFECTS) and it
// beams across: the old cursor breaks
// into glowing pixels inside a sparkling beam, the pixels stream along a curve
// to the new spot, and a second beam sparkles there as they arrive. Typing gives
// a faint shimmer on the cursor. At rest the cursor is left clean. Enter (a
// prompt sent in Claude Code, a command run in zsh, via `cursor-fx signal`)
// makes the lines of Claude Code's input box sparkle, out from the cursor, then
// beams pixels of them up out of the window.
// Its cursor is a matter stream: twinkling pixels with faint transporter bands
// drifting up through it, a shimmering edge and a soft teal glow. The prompt
// mark (❯) on its line takes the same style.
// The input box's lines glow teal with particles twinkling along them.

// macOS: true. On Linux, change it to false (install.sh does this for you);
// otherwise the effect plays upside down.
#define Y_DOWN true

// --- settings: the ones worth a tweak (edit and save, or use `cursor-fx edit`; Ghostty reloads on Command+Shift+,) ---
// -- Typing
const bool  TYPE_EFFECT    = true;  // a faint sparkle shimmers over the cursor as you type
const bool  PHRASE_EFFECT  = true;  // picking a character or phrase from your input method plays the typing effect too (like one keystroke)
// -- Enter & done
const bool  ENTER_EFFECT = true;  // beam the input box lines up on Enter (`cursor-fx enter off` pauses every effect's Enter)
const bool  DONE_EFFECT = true;  // play it again, from the edges inward, when Claude finishes answering (Stop hook) or a long zsh command ends
const float ENTER_PIXELS = 16.0;  // pixels beamed up from the box lines (at most 32)
const float ENTER_TIME   = 1.2;  // seconds until the last pixel is gone
// -- Delete-all
const bool  JUMP_EFFECTS = true;  // beam when the cursor jumps along its row (`cursor-fx jumps on|off`)
const float TRAVEL_TIME  = 0.28;  // seconds a pixel takes to cross a jump
// -- Cursor
const bool  CURSOR_EFFECT  = true;  // the cursor's own look: a matter stream: twinkling pixels, drifting bands, a shimmering edge
const bool  IDLE_EFFECTS   = true;  // the cursor's motion while idle (`cursor-fx idle on|off`; off keeps the screen calm)
// -- Prompt mark
const bool  PROMPT_EFFECT  = true;  // the prompt mark (❯) at the start of the cursor's line: teal matter with twinkling pixels
// -- Box lines
const bool  LINE_TINT      = false;  // the input box lines take the cursor's color (a plain color, no glow)
const float LINE_COLOR     = 0.42;  // how much of the cursor's color the lines take, at their own brightness
const bool  LINE_EFFECT    = false;  // the input box lines, always: a teal glow with particles twinkling along
// -- Look
const float PIXEL_SIZE   = 0.11;  // the pixels' size, in character heights

// --- fine-tuning ---
// -- Typing
const float SHIMMER_TIME = 0.15;  // seconds the shimmer lasts
// -- Enter & done
const float ENTER_SPREAD = 0.2;  // seconds the sparkle takes to run from the cursor to the edges
// -- Delete-all
const float TELEPORT_MIN = 3.0;  // moves this long or longer teleport (in characters; a row counts as two)
const float BEAM_TIME    = 0.22;  // seconds each end of the beam sparkles
const float STAGGER      = 0.12;  // seconds over which the pixels leave
const float ARC          = 0.25;  // how far the stream curves, as a share of the distance
const float MAX_ARC      = 3.0;  // the most the stream curves, in character heights
// -- Cursor
const float CURSOR_STRENGTH = 1.0;  // how strong the cursor's own look is
// -- Box lines
const float LINE_STRENGTH  = 1.0;   // how strong the box lines' own look is
// -- Look
const float FLICKER_RATE = 30.0;  // sparkle changes per second
// -- Timing
const float SETTLE       = 0.04;  // seconds the cursor must stay put before an effect starts
const float JUMP_SETTLE  = 0.12;  // the same after a jump (gives the Enter signal time to arrive)

// Ghostty hands shaders y-down pixels on macOS (Metal) and y-up ones on Linux
// (OpenGL). The effects are written y-up (embers rise, curtains hang upward),
// so with Y_DOWN (top of the file) mainImage turns its pixel and the cursors
// over and screenAt() turns every read of the screen back.

// The terminal's color at uv (0 to 1, y-up).
vec4 screenAt(vec2 uv) {
    return texture(iChannel0, Y_DOWN ? vec2(uv.x, 1.0 - uv.y) : uv);
}

// A cursor rectangle with .xy its top-left corner in y-up pixels. Ghostty gives
// the "-X, +Y corner": bottom-left on macOS, top-left on Linux.
vec4 upCursor(vec4 c) {
    return Y_DOWN ? vec4(c.x, iResolution.y - c.y + c.w, c.zw) : c;
}

// One cell's width. On a Chinese character the block cursor covers two cells (it is
// then about as wide as it is tall; a single cell is about twice as tall as wide).
float cellWOf(vec4 c) {
    return iCurrentCursorStyle == CURSORSTYLE_BLOCK && c.w < 1.4 * c.z ? 0.5 * c.z : c.z;
}

float hash11(float p) {
    p = fract(p * 0.1031);
    p *= p + 33.33;
    p *= p + p;
    return fract(p);
}

float hash21(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Ghostty passes colors in sRGB, but the shader works in linear light.
vec3 sRGBToLinear(vec3 c) {
    return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), c));
}

// Distance from a screen pixel to a uniform color, in whichever form (sRGB or
// linear) the screen texture turns out to use.
float colorDist(vec3 s, vec3 c) {
    return min(distance(s, c), distance(s, sRGBToLinear(c)));
}

float segDist(vec2 p, vec2 a, vec2 b) {
    vec2 pa = p - a, ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

// A sparkling transporter beam over a cell: a soft column of light with
// twinkling specks. strength 0..1.
vec3 beam(vec2 p, vec2 lo, vec2 hi, float cellH, float strength, vec3 tint) {
    vec2 c = (lo + hi) * 0.5;
    vec2 hs = (hi - lo) * 0.5 + vec2(0.15, 0.4) * cellH;
    vec2 d = abs(p - c);
    if (strength <= 0.0 || d.x > hs.x + 0.6 * cellH || d.y > hs.y + 0.3 * cellH) return vec3(0.0);
    float column = exp(-d.x / (0.18 * cellH)) * (1.0 - smoothstep(hs.y - 0.2 * cellH, hs.y + 0.3 * cellH, d.y));
    vec2 speck = floor(p / (0.07 * cellH));
    float twinkle = step(0.82, hash21(speck + floor(iTime * FLICKER_RATE) * 7.3));
    float inside = step(d.x, hs.x) * step(d.y, hs.y);
    return (tint * column * 0.45 + vec3(1.0) * twinkle * inside * 0.8) * strength;
}

// Enter (a prompt sent in Claude Code, a command run in zsh): `cursor-fx signal`
// sets the lowest bit of the cursor color's blue value for a moment, and flips the
// red value's lowest bit with every signal, so a new one is always a change of
// color. True while this frame belongs to that moment: the signal just started
// (blue turned odd, or red flipped while it was), or `back`: the cursor jumped while
// it was on in a way that means the same Enter (Claude Code clearing its input box
// after the signal arrived).
bool oddByte(float v) {
    return mod(floor(v * 255.0 + 0.5), 2.0) > 0.5;
}

bool signalFlipped() {
    return oddByte(iCurrentCursorColor.r) != oddByte(iPreviousCursorColor.r);
}

bool enterSignal(bool back) {
    return oddByte(iCurrentCursorColor.b) && (!oddByte(iPreviousCursorColor.b) || signalFlipped() || back);
}

// Done (Claude Code finished answering, via its Stop hook; a long zsh command
// finished): `cursor-fx signal done` sets the green value's lowest bit instead.
bool doneSignal(bool back) {
    return oddByte(iCurrentCursorColor.g) && !oddByte(iCurrentCursorColor.b)
        && (!oddByte(iPreviousCursorColor.g) || signalFlipped() || back);
}

// The height of the line that was just sent. Claude Code shows a sent prompt in
// the chat above the input box on a lighter bar across the whole width, so look
// up to 12 rows above the cursor for a row whose right end is off the background
// near its top and bottom (a border line or a letter there doesn't qualify).
// Without one (a shell), it's the cursor's row, or the row above when the cursor
// is at the left edge (after Enter in a shell).
float sentRowY(vec4 cur, float cellW, vec2 res) {
    float x = res.x - max(0.06 * res.x, 3.0 * cellW);
    for (int k = 1; k <= 12; k++) {
        float rowLo = cur.y + float(k - 1) * cur.w;
        if (rowLo + cur.w > res.y) break;
        vec3 a = screenAt(vec2(x, rowLo + 0.2 * cur.w) / res).rgb;
        vec3 b = screenAt(vec2(x, rowLo + 0.8 * cur.w) / res).rgb;
        if (colorDist(a, iBackgroundColor) > 0.012 && colorDist(b, iBackgroundColor) > 0.012) return rowLo + 0.5 * cur.w;
    }
    return cur.x <= 1.5 * cellW ? cur.y + 0.5 * cur.w : cur.y - 0.5 * cur.w;
}

// Claude Code draws its input box with a thin grey line across the whole width
// above and below the typed text. Looks for the nearest such line within
// `reach` of the pixel: ink at three points along the width with background a
// few pixels above and below (a letter or the sent-message bar doesn't
// qualify), outside the cursor's own row, at most 4 rows above its center
// and 1.6 rows below it (the status lines never count). Returns its height, or -1.
float boxLineY(vec2 p, vec2 center, vec4 cur, vec2 res, float reach) {
    float stepPx = max(1.0, reach / 40.0);
    vec3 xs = vec3(0.45, 0.66, 0.88) * res.x;
    for (int i = 0; i < 80; i++) {
        float dy = floor(float(i) * 0.5) * stepPx * (mod(float(i), 2.0) < 0.5 ? 1.0 : -1.0);
        if (abs(dy) > reach) break;
        float y = p.y + dy;
        if (y < 0.0 || y > res.y || y - center.y > 4.0 * cur.w || center.y - y > 1.6 * cur.w
                || (y > cur.y - cur.w && y < cur.y)) continue;
        if (colorDist(screenAt(vec2(xs.x, y) / res).rgb, iBackgroundColor) < 0.03
                || colorDist(screenAt(vec2(xs.y, y) / res).rgb, iBackgroundColor) < 0.03
                || colorDist(screenAt(vec2(xs.z, y) / res).rgb, iBackgroundColor) < 0.03) continue;
        // Claude Code's box lines run across the whole window; a shorter line (zsh's
        // underline under a path, a rule in some output) is not one.
        if (colorDist(screenAt(vec2(0.05 * res.x, y) / res).rgb, iBackgroundColor) < 0.03
                || colorDist(screenAt(vec2(0.95 * res.x, y) / res).rgb, iBackgroundColor) < 0.03) continue;
        if (colorDist(screenAt(vec2(xs.x, y + 4.0) / res).rgb, iBackgroundColor) > 0.03
                || colorDist(screenAt(vec2(xs.x, y - 4.0) / res).rgb, iBackgroundColor) > 0.03) continue;
        return y;
    }
    return -1.0;
}

// The box lines one row above and one row below the cursor (above, below; -1
// when missing). With no box at all (a shell), "below" is a line just under
// the row that was sent.
vec2 boxLines(vec4 cur, vec2 center, float cellW, float cellH, vec2 res) {
    vec2 lines = vec2(boxLineY(center + vec2(0.0, cellH), center, cur, res, 0.35 * cellH),
                      boxLineY(center - vec2(0.0, cellH), center, cur, res, 0.35 * cellH));
    if (lines.x < 0.0 && lines.y < 0.0) lines.y = sentRowY(cur, cellW, res) - 0.5 * cur.w + 2.0;
    return lines;
}

// How far the pixel is from where the effect starts: the cursor (Enter), or the
// window's edges when the answer arrived (done: it runs inward instead).
float spreadDist(vec2 p, vec2 center, vec2 res, bool inward) {
    return inward ? min(p.x, res.x - p.x) : abs(p.x - center.x);
}

// Enter: the input box lines sparkle as if being scanned, out from the cursor,
// then glowing pixels lift off along them and stream up out of the top of the
// window, speeding up.
vec3 enterEffect(vec2 p, vec4 cur, vec2 center, float cellW, float cellH, vec2 res, float t, vec3 col, bool inward) {
    if (p.y < center.y - 1.8 * cellH) return col;
    vec2 lines = boxLines(cur, center, cellW, cellH, res);
    float share = lines.x >= 0.0 && lines.y >= 0.0 ? 0.5 : 1.0;  // pixels per line
    vec3 tint = sRGBToLinear(iCurrentCursorColor.rgb);
    vec3 light = vec3(0.0);
    float size = PIXEL_SIZE * cellH;
    for (int k = 0; k < 2; k++) {
        float lineY = k == 0 ? lines.x : lines.y;
        if (lineY < 0.0) continue;
        float dy = abs(p.y - lineY);
        if (dy < 0.3 * cellH) {
            float local = clamp((t - spreadDist(p, center, res, inward) / res.x * ENTER_SPREAD) / 0.35, 0.0, 1.0);
            float twinkle = step(0.86, hash21(floor(p / (0.07 * cellH)) + floor(iTime * FLICKER_RATE) * 7.3));
            light += (tint * 0.35 * (1.0 - smoothstep(0.1 * cellH, 0.3 * cellH, dy)) + vec3(1.0) * twinkle * 0.8)
                * sin(3.14159 * local);
        }
        float climb = res.y - lineY + cellH;
        for (float i = 0.0; i < 32.0; i += 1.0) {
            if (i >= ENTER_PIXELS * share) break;
            float j = i + float(k) * 32.0;
            vec2 start = vec2(mix(0.03, 0.97, hash11(j * 3.17 + 0.5)) * res.x, lineY);
            // Each leaves a little after the sparkle has reached it.
            float u = t - spreadDist(start, center, res, inward) / res.x * ENTER_SPREAD - 0.15 - 0.2 * hash11(j * 7.1 + 1.3);
            if (u < 0.0 || u > 0.6) continue;
            float rise = climb * u * u / 0.36;  // out of the window by 0.6s
            float d = segDist(p, start + vec2(0.0, 0.75 * rise), start + vec2(0.0, rise));
            light += mix(tint, vec3(1.0), 0.5) * (1.0 - smoothstep(size * 0.4, size * 0.6, d))
                + tint * exp(-d / (size * 1.2)) * 0.5;
        }
    }
    return min(col + light, vec3(1.0));
}

// --- The cursor's own look (CURSOR_EFFECT) ---

// Pixel hash and smooth value noise for the cursor's look.
float clHash(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float clNoise(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(clHash(i), clHash(i + vec2(1.0, 0.0)), f.x),
               mix(clHash(i + vec2(0.0, 1.0)), clHash(i + vec2(1.0, 1.0)), f.x), f.y);
}

// Signed distance to a box of half-size b centered at the origin (negative inside).
float clBox(vec2 p, vec2 b) {
    vec2 q = abs(p) - b;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
}

// How much a pixel belongs to the cursor block itself: closer to the cursor
// color than to the background and to the letter color. Relative, because
// Ghostty's block isn't exactly the uniform's color; a letter under the cursor
// stays as it is.
float clOnBlock(vec3 col) {
    return smoothstep(0.0, 0.05, min(colorDist(col, iBackgroundColor), colorDist(col, iCursorText))
                                - colorDist(col, iCurrentCursorColor.rgb));
}

// Is the cursor drawn in this frame, or blinked off? Checks three points along
// its left edge: each counts if it looks more like the cursor color than like
// the background or the text color. Two of three must match.
bool cursorDrawn(vec2 lo, vec4 cur, vec2 res) {
    // A drawn cursor fills its rectangle up to the corners, where letters don't reach:
    // both top corners must be its color (and one more point lower down). While an input
    // method composes, Ghostty hides the cursor and the first pinyin letter sits in its
    // cell; with a light cursor color that letter alone used to pass for the cursor.
    vec2 ins = vec2(min(1.5, 0.5 * cur.z), min(1.5, 0.5 * cur.w));
    float top = 0.0, low = 0.0;
    for (int k = 0; k < 5; k++) {
        vec2 at = k == 0 ? vec2(lo.x + ins.x, lo.y + cur.w - ins.y) : k == 1 ? vec2(lo.x + cur.z - ins.x, lo.y + cur.w - ins.y)
                : k == 2 ? vec2(lo.x + ins.x, lo.y + ins.y) : k == 3 ? vec2(lo.x + cur.z - ins.x, lo.y + ins.y) : vec2(lo.x + ins.x, lo.y + 0.5 * cur.w);
        vec3 s = screenAt(at / res).rgb;
        float dc = colorDist(s, iCurrentCursorColor.rgb);
        float hit = step(dc, colorDist(s, iBackgroundColor)) * step(dc, colorDist(s, iForegroundColor));
        if (k < 2) top += hit; else low += hit;
    }
    return top >= 2.0 && low >= 1.0;
}

// Teleport's cursor: a matter stream. The block is made of twinkling pixels
// with faint transporter bands drifting up through it, its edge shimmers, and a
// soft teal glow surrounds it.
vec3 cursorLook(vec2 p, vec3 col) {
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w), hi = vec2(cur.x + cur.z, cur.y);
    vec2 c = 0.5 * (lo + hi), hs = 0.5 * (hi - lo);
    float cellH = max(cur.w, cellWOf(cur) * 2.0);
    vec2 q = abs(p - c) - hs;
    if (max(q.x, q.y) > 0.4 * cellH) return col;
    vec2 res = iResolution.xy;
    if (!cursorDrawn(lo, cur, res)) return col;
    float t = IDLE_EFFECTS ? iTime : 0.0, live = IDLE_EFFECTS ? 1.0 : 0.0;
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    float d = clBox(p - c, hs);
    float on = clamp(clOnBlock(col) * CURSOR_STRENGTH, 0.0, 1.0);
    float tw = smoothstep(0.92, 1.0, clHash(floor((p - lo) / 2.0) + floor(t * 3.0) * 7.31)) * live;
    float bands = 0.5 + 0.5 * sin((p.y - lo.y) / cur.w * 28.27 - t * 4.0);
    col = mix(col, col * (0.88 + 0.12 * bands) + mix(k, vec3(1.0), 0.7) * tw * 0.6, on);
    vec3 light = k * exp(-max(d, 0.0) / (0.09 * cellH)) * 0.22 * step(0.0, d);
    if (d > 0.0 && d < 2.5) light += k * smoothstep(0.85, 1.0, clHash(floor(p) + floor(t * 8.0) * 3.3)) * 0.5 * (1.0 - d / 2.5) * live;
    return col + light * CURSOR_STRENGTH;
}

// --- The prompt mark's look (PROMPT_EFFECT) ---

// Ink of the prompt mark's glyph at a point, only inside its cell (m is the
// cell's top-left corner, w and h its size).
float clMarkInk(vec2 at, vec2 m, float w, float h, vec2 res) {
    if (at.x < m.x || at.x > m.x + w || at.y > m.y || at.y < m.y - h) return 0.0;
    return smoothstep(0.04, 0.14, colorDist(screenAt(at / res).rgb, iBackgroundColor));
}

// Is the glyph in the cell at x shaped like a ❯ (or >)? Read in 17 rows of 12 points,
// in one pass: its ink reaches furthest right in the middle of its height (the tip),
// the top and bottom rows end at the same place well left of it, and at the tip the
// ink starts further right too. Letters, digits, Chinese characters, "-", "*", "$"
// don't pass (closing brackets do), so a one-letter word starting a continued or
// wrapped line ("a ", "I ") isn't taken for the prompt mark.
bool clChevron(float x, float w, vec4 cur, vec2 res) {
    float first = -1.0, last = -1.0, fL = 0.0, fR = 0.0, lL = 0.0, lR = 0.0;
    float most = -1.0, a = -1.0, b = -1.0, tipL = 0.0;
    for (int r = 0; r < 17; r++) {
        float y = cur.y - (0.1 + 0.05 * float(r)) * cur.w;
        float lo = 1.0, hi = 0.0;
        for (int i = 0; i < 12; i++) {
            float f = (float(i) + 0.5) / 12.0;
            if (colorDist(screenAt(vec2(x + f * w, y) / res).rgb, iBackgroundColor) > 0.08) { lo = min(lo, f); hi = max(hi, f); }
        }
        if (hi <= 0.0) continue;
        if (first < 0.0) { first = float(r); fL = lo; fR = hi; }
        last = float(r); lL = lo; lR = hi;
        if (hi > most + 0.01) { most = hi; a = float(r); b = a; tipL = lo; }
        else if (hi >= most - 0.01) { b = float(r); tipL = max(tipL, lo); }
    }
    if (first < 0.0 || last - first < 4.0) return false;
    float at = (0.5 * (a + b) - first) / (last - first);
    return at > 0.3 && at < 0.7 && most - max(fR, lR) > 0.15 && abs(fR - lR) < 0.15 && tipL - max(fL, lL) > 0.12;
}

// The prompt mark (the ❯ of Claude Code and of the shell): the first character
// on the cursor's row, when a blank cell follows it and both sit left of the
// cursor. Cells line up with the cursor's, so only the first few cells from the
// window's left edge are checked. Returns the mark cell's left edge, or -1.
float clMarkLeft(vec4 cur, vec2 res) {
    float w = cellWOf(cur), h = cur.w;
    if (w < 2.0) return -1.0;
    float x = mod(cur.x, w);
    for (int j = 0; j < 8; j++) {
        if (x + 2.0 * w > cur.x + 0.5) return -1.0;
        float hits = 0.0;
        for (int i = 0; i < 5; i++) {
            vec2 f = i == 0 ? vec2(0.3, 0.5) : i == 1 ? vec2(0.5, 0.36) : i == 2 ? vec2(0.5, 0.64) : i == 3 ? vec2(0.72, 0.5) : vec2(0.4, 0.42);
            hits += step(0.08, colorDist(screenAt(vec2(x + f.x * w, cur.y - f.y * h) / res).rgb, iBackgroundColor));
        }
        if (hits > 0.5) {
            float next = 0.0;
            for (int i = 0; i < 3; i++)
                next += step(0.08, colorDist(screenAt(vec2(x + (1.3 + 0.2 * float(i)) * w, cur.y - (0.35 + 0.15 * float(i)) * h) / res).rgb, iBackgroundColor));
            return next < 0.5 ? x : -1.0;
        }
        x += w;
    }
    return -1.0;
}

// The prompt mark of a prompt that wraps over several rows, the cursor on a later row:
// in Claude Code's input box, the first character under the box's top line, in the
// box's first column. Rows can be taller than the cursor, so the mark's cell is placed
// from its own glyph, which sits mid-cell. Callers check it is shaped like a mark.
// Returns the cell's top-left corner, or x < 0.
float clLineNear(float y0, float reach, vec2 res);  // further down, with the box lines' look
vec2 clMarkAbove(vec4 cur, vec2 res) {
    float w = cellWOf(cur), h = cur.w;
    if (w < 2.0) return vec2(-1.0);
    // The input box's top line: the first line across the window above the cursor's row.
    float stepY = max(1.0, floor(h / 16.0)), lineY = -1.0;
    for (int i = 0; i < 400; i++) {
        float y = cur.y + 1.0 + float(i) * stepY;
        if (y > cur.y + 10.0 * h || y > res.y) break;
        if (colorDist(screenAt(vec2(0.95 * res.x, y) / res).rgb, iBackgroundColor) < 0.03) continue;
        lineY = clLineNear(y, 1.0, res);
        if (lineY >= 0.0) break;
    }
    if (lineY < 0.0) return vec2(-1.0);
    // The box's first column: where that line starts (the window's padding can be wider
    // than a cell, so it is looked for along the cell grid).
    float x = -1.0, x0 = mod(cur.x, w);
    for (int k = 0; k < 10; k++) {
        float cx = x0 + float(k) * w;
        if (colorDist(screenAt(vec2(cx + 0.5 * w, lineY) / res).rgb, iBackgroundColor) > 0.03) { x = cx; break; }
    }
    if (x < 0.0) return vec2(-1.0);
    // The first glyph under the line in the first two cells: the mark (the rows after the
    // first are indented, so nothing else is there). Its cell from its middle.
    float top = -1.0, bot = -1.0, cell = x, gap = 0.0;
    for (int s = 0; s < 200; s++) {
        float y = lineY - 5.0 - float(s);
        if (y < cur.y - 0.5 * h || float(s) > 2.5 * h) break;
        bool any = false;
        for (int k = 0; k < 6; k++) {
            float cx = x + float(k / 3) * w;
            if ((top < 0.0 || cx == cell) && colorDist(screenAt(vec2(cx + (0.3 + 0.2 * float(k % 3)) * w, y) / res).rgb, iBackgroundColor) > 0.08) {
                if (top < 0.0) cell = cx;
                any = true;
            }
        }
        if (any) { if (top < 0.0) top = y; bot = y; gap = 0.0; }
        else if (top >= 0.0) { gap += 1.0; if (gap > 2.0) break; }
    }
    if (top < 0.0) return vec2(-1.0);
    return vec2(cell, 0.5 * (top + bot) + 0.5 * h);
}

// The prompt mark as teal matter: twinkling pixels through it and a soft
// shimmering glow.
vec3 markLook(vec2 p, vec3 col, float ink, float halo, vec2 m, float w, float h, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    vec2 res = iResolution.xy;
    float tw = live * smoothstep(0.9, 1.0, clHash(floor(p / 2.0) + floor(t * 4.0) * 3.1));
    float shimmer = 0.7 + 0.3 * sin(t * 3.0 + p.y * 0.5) * live;
    col = mix(col, mix(k, vec3(1.0), 0.15 + 0.6 * tw), ink);
    return col + k * halo * (1.0 - ink) * 0.45 * shimmer;
}

// Finds the prompt mark on the cursor's row and lets markLook() restyle its
// glyph (ink: how much the pixel is the glyph) and light the space around it
// (halo: the glyph blurred over a disc).
vec3 promptLook(vec2 p, vec3 col) {
    vec4 cur = upCursor(iCurrentCursor);
    float w = cellWOf(cur), h = cur.w;
    if (p.x > 9.0 * w + 0.4 * h) return col;
    vec2 res = iResolution.xy;
    vec2 m;
    if (abs(p.y - (cur.y - 0.5 * h)) <= 0.9 * h) {
        if (p.x > cur.x - 1.0) return col;
        float x0 = clMarkLeft(cur, res);
        if (x0 < 0.0 || p.x < x0 - 0.4 * h || p.x > x0 + max(w + 0.4 * h, 2.0 * w) || !clChevron(x0, w, cur, res)) return col;
        m = vec2(x0, cur.y);
    } else {
        // A prompt that wraps, the cursor on a later row: the mark on the prompt's first row.
        if (p.y < cur.y || p.y > cur.y + 10.5 * h || p.x > mod(cur.x, w) + 7.0 * w) return col;
        if (clMarkLeft(cur, res) >= 0.0) return col;  // the cursor's own row starts with the mark (or with text)
        // Only the box's first row (a line across the window at most a row and a half above) can hold the mark:
        // cheap, before the search.
        bool under = false;
        float stepY = max(1.0, floor(h / 16.0));
        for (int i = 0; i < 100; i++) {
            float y = p.y + float(i) * stepY;
            if (y > p.y + 2.2 * h || y > res.y) break;
            if (colorDist(screenAt(vec2(0.95 * res.x, y) / res).rgb, iBackgroundColor) > 0.03
                    && colorDist(screenAt(vec2(0.05 * res.x, y) / res).rgb, iBackgroundColor) > 0.03
                    && colorDist(screenAt(vec2(0.5 * res.x, y) / res).rgb, iBackgroundColor) > 0.03) { under = true; break; }
        }
        if (!under) return col;
        m = clMarkAbove(cur, res);
        if (m.x < 0.0 || abs(p.y - (m.y - 0.5 * h)) > 0.9 * h || p.x < m.x - 0.4 * h || p.x > m.x + max(w + 0.4 * h, 2.0 * w)
                || !clChevron(m.x, w, vec4(cur.x, m.y, cur.zw), res)) return col;
    }
    // The glow: the glyph blurred over a disc, 64 samples on a golden-angle spiral with a
    // soft falloff (two rings of 8 showed as separate ghost copies of the mark).
    float ink = clMarkInk(p, m, w, h, res), halo = 0.0, wsum = 0.0;
    for (int i = 0; i < 64; i++) {
        float r = sqrt((float(i) + 0.5) / 64.0), a = float(i) * 2.39996;
        float k = exp(-2.5 * r * r);
        halo += k * clMarkInk(p + 0.2 * h * r * vec2(cos(a), sin(a)), m, w, h, res);
        wsum += k;
    }
    halo /= wsum;
    // Still when the window isn't focused (its style stays, its motion stops).
    bool moving = IDLE_EFFECTS && iFocus != 0;
    float t = moving ? iTime : 0.0, live = moving ? 1.0 : 0.0;
    return markLook(p, col, ink, halo, m, w, h, t, live);
}

// --- The input box lines' look (LINE_EFFECT) ---

// The box line nearest to height y0, within `reach`: ink at three points across
// the window and background four pixels above and below it (like boxLineY()).
// Returns its height, or -1.
float clLineNear(float y0, float reach, vec2 res) {
    vec3 xs = vec3(0.45, 0.66, 0.88) * res.x;
    for (int i = 0; i < 81; i++) {
        float off = float((i + 1) / 2) * (i % 2 == 0 ? 1.0 : -1.0);
        if (abs(off) > reach) break;
        float y = floor(y0) + 0.5 + off;
        if (colorDist(screenAt(vec2(xs.x, y) / res).rgb, iBackgroundColor) < 0.03
                || colorDist(screenAt(vec2(xs.y, y) / res).rgb, iBackgroundColor) < 0.03
                || colorDist(screenAt(vec2(xs.z, y) / res).rgb, iBackgroundColor) < 0.03) continue;
        // Claude Code's box lines run across the whole window; a shorter line (zsh's
        // underline under a path, a rule in some output) is not one.
        if (colorDist(screenAt(vec2(0.05 * res.x, y) / res).rgb, iBackgroundColor) < 0.03
                || colorDist(screenAt(vec2(0.95 * res.x, y) / res).rgb, iBackgroundColor) < 0.03) continue;
        if (colorDist(screenAt(vec2(xs.x, y + 4.0) / res).rgb, iBackgroundColor) > 0.03
                || colorDist(screenAt(vec2(xs.x, y - 4.0) / res).rgb, iBackgroundColor) > 0.03) continue;
        return y;
    }
    return -1.0;
}

// The box lines glow teal, with particles twinkling along them.
vec3 lineLook(vec2 p, vec3 col, float lineY, float h, vec2 res, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    float dy = abs(p.y - lineY), core = exp(-dy / (0.03 * h)), soft = exp(-dy / (0.14 * h));
    float tw = live * smoothstep(0.93, 1.0, clHash(vec2(floor(p.x / 3.0), floor(t * 3.0))));
    return col + (k * (core * 0.3 + soft * 0.12) + mix(k, vec3(1.0), 0.7) * tw * exp(-dy / (0.05 * h)) * 0.9) * LINE_STRENGTH;
}

// LINE_COLOR: the box line's own pixels take the cursor's hue at about the line's own
// brightness. Done in the screen texture's own color space (found from the background),
// subtracting the line's grey and adding the new color, so anything drawn over the line
// (the Enter rain) and its antialiased edges stay as they are.
vec3 lineTint(vec2 p, vec3 col, float lineY, vec2 res) {
    vec3 bgTex = screenAt(vec2(p.x, lineY + 4.0) / res).rgb;          // background right above the line
    vec3 line = screenAt(vec2(p.x, lineY) / res).rgb;                  // the line's full color here
    vec3 s = screenAt(p / res).rgb;
    vec3 dl = line - bgTex;
    float ink = clamp(dot(s - bgTex, dl) / max(dot(dl, dl), 1e-6), 0.0, 1.0);
    if (ink <= 0.0) return col;
    bool linear = distance(bgTex, sRGBToLinear(iBackgroundColor)) < distance(bgTex, iBackgroundColor);
    vec3 k = linear ? sRGBToLinear(iCurrentCursorColor.rgb) : iCurrentCursorColor.rgb;
    vec3 lw = vec3(0.2126, 0.7152, 0.0722);
    vec3 matched = k * (1.25 * dot(line, lw) / max(dot(k, lw), 1e-3));
    return col + ink * (mix(line, min(matched, vec3(1.0)), LINE_COLOR) - line);
}

// Every pixel from a little below the cursor to a few rows above it (the box
// grows upward for a long prompt) looks for a box line within a fifth of the
// cursor's height of itself and lets lineLook() light it. Rows can be taller
// than the cursor (adjust-cell-height), so nothing here counts rows.
vec3 linesLook(vec2 p, vec3 col) {
    vec4 cur = upCursor(iCurrentCursor);
    float h = cur.w;
    if (p.y < cur.y - 2.6 * h || p.y > cur.y + 6.5 * h || (p.y > cur.y - h && p.y < cur.y)) return col;
    vec2 res = iResolution.xy;
    bool look = LINE_EFFECT && iFocus != 0;
    // With only the line color (no LINE_EFFECT light), background pixels have nothing to do.
    if (!look && colorDist(screenAt(p / res).rgb, iBackgroundColor) < 0.02) return col;
    // ...nor do pixels whose row isn't a line across the window (cheap, before the search).
    if (!look && (colorDist(screenAt(vec2(0.05 * res.x, p.y) / res).rgb, iBackgroundColor) < 0.02
            || colorDist(screenAt(vec2(0.95 * res.x, p.y) / res).rgb, iBackgroundColor) < 0.02)) return col;
    float lineY = clLineNear(p.y, 0.2 * h, res);
    if (lineY < 0.0) return col;
    // Only inside an input box: there is a box line just below the cursor too (a
    // shell, or a program with a long rule somewhere, stays plain).
    if (clLineNear(cur.y - 1.7 * h, 0.7 * h, res) < 0.0) return col;
    if (LINE_TINT && LINE_COLOR > 0.0) col = lineTint(p, col, lineY, res);
    if (!look) return col;
    float t = IDLE_EFFECTS ? iTime : 0.0, live = IDLE_EFFECTS ? 1.0 : 0.0;
    return lineLook(p, col, lineY, h, res, t, live);
}

// Everything that answers the cursor's moves and the signals (fragCoord is
// already y-up, fragColor already the screen).
void effects(inout vec4 fragColor, vec2 fragCoord) {
    vec2 res = iResolution.xy;
    if (iFocus == 0 || iCursorVisible == 0) return;

    // Cursor rectangles in pixels (y grows upward; .xy is the top-left corner).
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w);
    vec2 hi = vec2(cur.x + cur.z, cur.y);
    float cellH = max(cur.w, cellWOf(cur) * 2.0);
    float cellW = max(cellWOf(cur), cellH * 0.4);
    vec4 prv = upCursor(iPreviousCursor);
    vec2 plo = vec2(prv.x, prv.y - prv.w);
    vec2 phi = vec2(prv.x + prv.z, prv.y);
    vec2 to = (lo + hi) * 0.5;
    vec2 from = (plo + phi) * 0.5;

    float sinceMove = iTime - iTimeCursorChange;
    float moved = abs(cur.x - prv.x) / cellW + 2.0 * abs(cur.y - prv.y) / cellH;
    // Forward along the row by up to 40 cells is typed text, never a jump: a letter, a
    // Chinese character (two cells) or a whole phrase an input method commits at once
    // (the cursor stays put while you compose it, then jumps). It plays as typing.
    bool typedText = abs(cur.y - prv.y) < 0.5 * cellH && cur.x - prv.x > 0.5 * cellW && cur.x - prv.x < 40.5 * cellW;
    float signalMoved = moved;
    if (typedText) moved = min(moved, 1.0);  // a whole phrase plays like one keystroke
    bool typeOn = TYPE_EFFECT && (PHRASE_EFFECT || !(typedText && cur.x - prv.x > 1.5 * cellW));
    // Wait until the cursor has stayed put a moment (longer after a jump, so the
    // Enter signal can arrive first): spots a program's cursor passes through
    // while it redraws never show anything.
    sinceMove -= moved >= 3.0 ? JUMP_SETTLE : SETTLE;
    if (sinceMove < 0.0) return;
    // A jump while a signal is on means the same Enter only inside Claude Code's input box
    // (a box line just below the cursor: it cleared the box after the signal arrived), and
    // never forward along the row. A Tab completion, a prompt the shell redraws, a program
    // hopping the cursor elsewhere never start the effect again.
    bool back = signalMoved >= 3.0 && (oddByte(iCurrentCursorColor.b) || oddByte(iCurrentCursorColor.g))
        && !(abs(cur.y - prv.y) < 0.5 * cellH && cur.x > prv.x)
        && clLineNear(cur.y - 1.7 * cur.w, 0.7 * cur.w, res) >= 0.0;
    bool done = DONE_EFFECT && doneSignal(back);
    if ((ENTER_EFFECT && enterSignal(back)) || done) {
        if (sinceMove < ENTER_TIME) fragColor.rgb = enterEffect(fragCoord, cur, to, cellW, cellH, res, sinceMove, fragColor.rgb, done);
        return;
    }
    // Only the color changed (the Enter signal ending), the cursor jumped to another
    // row (a program redrawing: never an effect), or along its row with JUMP_EFFECTS
    // off: not a move.
    if (moved < 0.5 || (moved >= TELEPORT_MIN && !(JUMP_EFFECTS && abs(cur.y - prv.y) < 0.5 * cellH))) sinceMove = 1e4;
    float total = STAGGER + TRAVEL_TIME + BEAM_TIME;
    bool teleport = moved >= TELEPORT_MIN && sinceMove < total;
    bool shimmer = typeOn && moved < TELEPORT_MIN && sinceMove < SHIMMER_TIME;
    if (!teleport && !shimmer) return;

    vec2 p = fragCoord;
    vec3 col = fragColor.rgb;
    vec3 tint = sRGBToLinear(iCurrentCursorColor.rgb);

    if (teleport) {
        float len = distance(from, to);
        vec2 dir = (to - from) / max(len, 1e-3);
        vec2 n = vec2(-dir.y, dir.x);
        float arc = min(ARC * len, MAX_ARC * cellH);
        if (segDist(p, from, to) > arc * 1.4 + 1.5 * cellH) {
            fragColor = vec4(col, fragColor.a);
            return;
        }

        // Beams: one where it leaves, one where it arrives.
        float leave = sin(3.14159 * clamp(sinceMove / BEAM_TIME, 0.0, 1.0));
        float arriveStart = STAGGER + TRAVEL_TIME * 0.6;
        float arrive = sin(3.14159 * clamp((sinceMove - arriveStart) / BEAM_TIME, 0.0, 1.0));
        col += beam(p, plo, phi, cellH, leave, tint);
        col += beam(p, lo, hi, cellH, arrive, tint);

        // The pixels: a 3 x 6 grid of the old cursor, each leaving a little
        // later, flying a curve with a short streak behind it, and fading as it
        // lands in the new cursor.
        float size = PIXEL_SIZE * cellH;
        for (float i = 0.0; i < 18.0; i += 1.0) {
            vec2 uv = vec2((mod(i, 3.0) + 0.5) / 3.0, (floor(i / 3.0) + 0.5) / 6.0);
            vec2 start = mix(plo, phi, uv);
            vec2 end = mix(lo, hi, uv);
            float h = hash11(i * 3.17 + 0.5);
            float u = clamp((sinceMove - STAGGER * h) / TRAVEL_TIME, 0.0, 1.0);
            if (u >= 1.0 && sinceMove - STAGGER * h - TRAVEL_TIME > 0.08) continue;
            float bend = arc * (0.6 + 0.8 * hash11(i * 7.1 + 1.3));
            float e = u * u * (3.0 - 2.0 * u);
            float eb = max(u - 0.05, 0.0);  // a short streak, so it still reads as a pixel
            eb = eb * eb * (3.0 - 2.0 * eb);
            vec2 pos = mix(start, end, e) + n * bend * sin(3.14159 * e);
            vec2 tail = mix(start, end, eb) + n * bend * sin(3.14159 * eb);
            float d = segDist(p, tail, pos);
            float landed = u >= 1.0 ? 1.0 - (sinceMove - STAGGER * h - TRAVEL_TIME) / 0.08 : 1.0;
            float core = 1.0 - smoothstep(size * 0.4, size * 0.6, d);
            col += (mix(tint, vec3(1.0), 0.5) * core + tint * exp(-d / (size * 1.2)) * 0.5) * landed;
        }
    } else {
        // Typing: a quick shimmer of specks over the cursor.
        col += beam(p, lo, hi, cellH, 0.5 * (1.0 - sinceMove / SHIMMER_TIME), tint);
    }

    fragColor = vec4(min(col, vec3(1.0)), fragColor.a);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 res = iResolution.xy;
    if (Y_DOWN) fragCoord.y = res.y - fragCoord.y;
    fragColor = screenAt(fragCoord / res);
    effects(fragColor, fragCoord);
    // The line color stays when the window loses focus; the moving looks don't.
    if (((LINE_EFFECT && iFocus != 0) || (LINE_TINT && LINE_COLOR > 0.0)) && iCursorVisible != 0 && iCurrentCursorStyle == CURSORSTYLE_BLOCK) fragColor.rgb = min(linesLook(fragCoord, fragColor.rgb), vec3(1.0));
    if (iCursorVisible == 0) return;
    if (iFocus != 0 && CURSOR_EFFECT && iCurrentCursorStyle == CURSORSTYLE_BLOCK) fragColor.rgb = min(cursorLook(fragCoord, fragColor.rgb), vec3(1.0));
    if (PROMPT_EFFECT && iCurrentCursorStyle == CURSORSTYLE_BLOCK) fragColor.rgb = min(promptLook(fragCoord, fragColor.rgb), vec3(1.0));
}
