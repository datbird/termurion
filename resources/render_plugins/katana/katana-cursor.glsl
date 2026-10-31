// Katana cursor for Ghostty (`custom-shader`).
// Jump the cursor along its row (delete-all, Home; JUMP_EFFECTS) and a
// blade slashes along the path: a white
// line flashes across the text, the text on both sides of the cut splits apart
// and slides back together. Typing slashes the cursor itself: the blade crosses
// it corner to corner, its two halves part for an instant and sparks fly off;
// an idle cursor glints now and then. At rest the cursor is left clean.
// Enter (a prompt sent in Claude Code, a command run in zsh, via `cursor-fx
// signal`) sweeps the blade out from the cursor along the lines of Claude Code's
// input box, cutting each lengthwise: the top half slides one way, the bottom
// half the other, then they join again.
// Its cursor is polished steel: a beveled edge, a fine brushed grain, lighter
// toward the top, with a slow sheen gliding down it. The prompt mark (❯) on its
// line takes the same style.
// The input box's lines are steel edges: a glint slides along them every few
// seconds.

// macOS: true. On Linux, change it to false (install.sh does this for you);
// otherwise the effect plays upside down.
#define Y_DOWN true

// --- settings: the ones worth a tweak (edit and save, or use `cursor-fx edit`; Ghostty reloads on Command+Shift+,) ---
// -- Typing
const bool  TYPE_EFFECT    = true;  // every keystroke slashes the cursor corner to corner and its halves part for an instant
const bool  PHRASE_EFFECT  = true;  // picking a character or phrase from your input method plays the typing effect too (like one keystroke)
const float TYPE_SPLIT    = 0.06;  // how far the cursor's halves part along the cut, in character heights
// -- Enter & done
const bool  ENTER_EFFECT  = true;  // cut the input box lines on Enter (`cursor-fx enter off` pauses every effect's Enter)
const bool  DONE_EFFECT  = true;  // play it again, from the edges inward, when Claude finishes answering (Stop hook) or a long zsh command ends
const float ENTER_TIME    = 0.5;  // seconds until the cut along the lines has closed
// -- Delete-all
const bool  JUMP_EFFECTS  = true;  // slash when the cursor jumps along its row (`cursor-fx jumps on|off`)
const float SPLIT         = 0.16;  // how far the two sides of a cut part, in character heights
const float SLIDE         = 0.3;  // how far they slide along the cut, in character heights
// -- Cursor
const bool  CURSOR_EFFECT  = true;  // the cursor's own look: polished steel: a bevel, brushed grain and a slow sheen
const bool  IDLE_EFFECTS  = true;  // the glint while idle (`cursor-fx idle on|off`; off keeps the screen calm)
const float IDLE_PERIOD   = 5.0;  // seconds between idle glints
// -- Prompt mark
const bool  PROMPT_EFFECT  = true;  // the prompt mark (❯) at the start of the cursor's line: polished steel with a glint sweeping across it
// -- Box lines
const bool  LINE_TINT      = true;  // the input box lines take the cursor's color (a plain color, no glow)
const float LINE_COLOR     = 0.42;  // how much of the cursor's color the lines take, at their own brightness
const bool  LINE_EFFECT    = false;  // the input box lines, always: steel edges with a glint sliding along
// -- Look
const float GLOW_RADIUS   = 0.18;  // the glow around the blade, in character heights

// --- fine-tuning ---
// -- Typing
const float TYPE_TIME     = 0.22;  // seconds until the cursor's halves have joined again
const float TYPE_SWEEP    = 0.06;  // seconds the blade takes to cross the cursor
// -- Enter & done
const float ENTER_SWEEP   = 0.1;  // seconds the blade takes from the cursor to the edges
const float LINE_REACH    = 0.7;  // how close to a box line pixels take part, in character heights
// -- Delete-all
const float SLASH_MIN     = 3.0;  // moves this long or longer are a jump (in characters; a row counts as two)
const float SWEEP_TIME    = 0.07;  // seconds the blade takes to cross the jump
const float SLASH_TIME    = 0.45;  // seconds until the cut has closed again
const float OVERSHOOT     = 0.35;  // how far the cut runs past both ends, as a share of its length
const float MAX_OVERSHOOT = 3.0;  // the most the cut runs past its ends, in character heights
const float SPLIT_REACH   = 1.6;   // how far from the cut the text moves, in character heights
// -- Cursor
const float CURSOR_STRENGTH = 1.0;  // how strong the cursor's own look is
const float GLINT_TIME    = 0.14;  // seconds a glint takes to cross the cursor
// -- Box lines
const float LINE_STRENGTH  = 1.0;   // how strong the box lines' own look is
// -- Look
const float LINE_WIDTH    = 0.03;  // the blade's thickness, in character heights
// -- Timing
const float SETTLE        = 0.04;  // seconds the cursor must stay put before an effect starts
const float JUMP_SETTLE   = 0.12;  // the same after a jump (gives the Enter signal time to arrive)

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

// Ghostty passes colors in sRGB, but the shader works in linear light.
vec3 sRGBToLinear(vec3 c) {
    return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), c));
}

// Distance from a screen pixel to a uniform color, in whichever form (sRGB or
// linear) the screen texture turns out to use.
float colorDist(vec3 s, vec3 c) {
    return min(distance(s, c), distance(s, sRGBToLinear(c)));
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
// (the box grows upward for a long prompt) and 1.6 rows below it (the status
// lines never count). Returns its height, or -1.
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

// The box line nearest to the pixel within `reach`, or, with no box at all (a
// shell), a line just under the row that was sent. -1 when there is none.
float lineAt(vec2 p, vec4 cur, vec2 center, float cellW, float cellH, vec2 res, float reach) {
    float lineY = boxLineY(p, center, cur, res, reach);
    if (lineY >= 0.0) return lineY;
    float under = sentRowY(cur, cellW, res) - 0.5 * cur.w + 2.0;
    if (abs(p.y - under) >= reach
            || boxLineY(center + vec2(0.0, 0.5 * cellH), center, cur, res, 0.9 * cellH) >= 0.0
            || boxLineY(center - vec2(0.0, 0.5 * cellH), center, cur, res, 0.9 * cellH) >= 0.0) return -1.0;
    return under;
}

// How far the pixel is from where the effect starts: the cursor (Enter), or the
// window's edges when the answer arrived (done: it runs inward instead).
float spreadDist(vec2 p, vec2 center, vec2 res, bool inward) {
    return inward ? min(p.x, res.x - p.x) : abs(p.x - center.x);
}

// Enter: the blade sweeps out from the cursor along both input box lines; each
// line is cut lengthwise, its top half sliding one way and its bottom half the
// other over a dark slit, then they slide back together.
vec3 enterEffect(vec2 p, vec4 cur, vec2 center, float cellW, float cellH, vec2 res, float t, vec3 col, bool inward) {
    float reach = LINE_REACH * cellH;
    if (p.y - center.y > 4.0 * cellH + reach || center.y - p.y > 1.6 * cellH + reach) return col;
    float lineY = lineAt(p, cur, center, cellW, cellH, res, reach);
    if (lineY < 0.0) return col;
    float across = p.y - lineY;  // above the cut is positive
    float dist = spreadDist(p, center, res, inward);
    float reached = clamp(t / ENTER_SWEEP, 0.0, 1.0) * res.x;
    float cutHere = step(dist, reached);
    float x = t / ENTER_TIME;
    float open = smoothstep(0.0, 0.15, x) * (1.0 - smoothstep(0.35, 1.0, x)) * cutHere;
    float near = 1.0 - smoothstep(0.05 * cellH, reach, abs(across));
    float side = across >= 0.0 ? 1.0 : -1.0;
    col = screenAt((p - vec2(-side * SLIDE * cellH, side * 0.5 * SPLIT * cellH) * open * near) / res).rgb;
    float gap = 0.5 * SPLIT * cellH * open;
    col *= mix(1.0, 0.25, (1.0 - smoothstep(gap * 0.6, gap + 0.5, abs(across))) * clamp(gap, 0.0, 1.0));
    float fade = 1.0 - smoothstep(ENTER_SWEEP, 0.3, t);
    float w = LINE_WIDTH * cellH;
    float core = 1.0 - smoothstep(w * 0.5, w, abs(across));
    float halo = exp(-abs(across) / (GLOW_RADIUS * cellH));
    col += (vec3(1.0) * core + sRGBToLinear(iCurrentCursorColor.rgb) * halo * 0.7) * cutHere * fade;
    if (t < ENTER_SWEEP) col += vec3(1.0) * exp(-pow((dist - reached) / (0.8 * cellH), 2.0)) * halo * 0.8;
    return min(col, vec3(1.0));
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

// Katana's cursor: polished steel. A bevel (a bright top-left edge, a dark
// bottom-right one), a fine brushed grain, lighter toward the top, and a slow
// sheen gliding down the blade.
vec3 cursorLook(vec2 p, vec3 col) {
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w), hi = vec2(cur.x + cur.z, cur.y);
    vec2 c = 0.5 * (lo + hi), hs = 0.5 * (hi - lo);
    float cellH = max(cur.w, cellWOf(cur) * 2.0);
    vec2 q = abs(p - c) - hs;
    if (max(q.x, q.y) > 0.05 * cellH) return col;
    vec2 res = iResolution.xy;
    if (!cursorDrawn(lo, cur, res)) return col;
    float t = IDLE_EFFECTS ? iTime : 0.0, live = IDLE_EFFECTS ? 1.0 : 0.0;
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    float d = clBox(p - c, hs);
    float on = clamp(clOnBlock(col) * CURSOR_STRENGTH, 0.0, 1.0);
    if (on <= 0.0) return col;
    vec2 uv = (p - lo) / (hi - lo);
    float bev = max(1.0, 0.06 * cur.z);
    float lit = 1.0 - smoothstep(0.0, bev, min(p.x - lo.x, hi.y - p.y));
    float shade = 1.0 - smoothstep(0.0, bev, min(hi.x - p.x, p.y - lo.y));
    float grain = 1.0 + 0.07 * (clHash(vec2(floor(p.y), 3.0)) - 0.5);
    float sheen = exp(-pow((uv.y - 1.25 + fract(t / 4.0) * 1.5) / 0.12, 2.0)) * 0.22 * live;
    vec3 steel = col * (0.88 + 0.22 * uv.y) * grain + vec3(0.3 * lit + sheen) - col * 0.35 * shade;
    return mix(col, max(steel, vec3(0.0)), on);
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

// The prompt mark in polished steel: lighter toward the top, with a glint
// sweeping across it every three seconds.
vec3 markLook(vec2 p, vec3 col, float ink, float halo, vec2 m, float w, float h, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    vec2 res = iResolution.xy;
    vec2 uv = vec2((p.x - m.x) / w, (m.y - p.y) / h);
    float glint = live * exp(-pow((uv.x + 0.3 * uv.y - (fract(t / 3.0) * 2.4 - 0.7)) / 0.08, 2.0));
    col = mix(col, k * (1.1 - 0.35 * uv.y) + vec3(0.6 * glint), ink);
    return col + vec3(0.4) * glint * halo * (1.0 - ink);
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

// The box lines as steel edges: slightly brighter, with a glint sliding along
// them every four seconds.
vec3 lineLook(vec2 p, vec3 col, float lineY, float h, vec2 res, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    float dy = abs(p.y - lineY), core = exp(-dy / (0.03 * h)), soft = exp(-dy / (0.14 * h));
    float glint = live * exp(-pow((p.x - (fract(t / 4.0) * 3.0 - 0.5) * res.x) / (1.5 * h), 2.0));
    return col + mix(k, vec3(1.0), 0.6) * (core * (0.18 + 0.9 * glint) + soft * 0.25 * glint) * LINE_STRENGTH;
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
    vec2 to = (lo + hi) * 0.5;
    vec4 prv = upCursor(iPreviousCursor);
    vec2 from = vec2(prv.x + prv.z * 0.5, prv.y - prv.w * 0.5);

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
    if (moved < 0.5 || (moved >= SLASH_MIN && !(JUMP_EFFECTS && abs(cur.y - prv.y) < 0.5 * cellH))) sinceMove = 1e4;
    bool slash = moved >= SLASH_MIN && sinceMove < SLASH_TIME;

    // Typing: a quick slash through the cursor itself. Idle: a glint now and then.
    bool typed = typeOn && moved < SLASH_MIN && sinceMove < TYPE_TIME;
    float glint = -1.0;
    float idle = mod(iTime, IDLE_PERIOD);
    if (IDLE_EFFECTS && idle < GLINT_TIME && iTime - idle > iTimeCursorChange + 1.0) glint = idle / GLINT_TIME;
    if (!slash && !typed && glint < 0.0) return;

    vec2 p = fragCoord;
    vec3 col = fragColor.rgb;
    vec3 white = vec3(1.0);
    vec3 steel = sRGBToLinear(iCurrentCursorColor.rgb);

    if (slash) {
        // The cut runs along the move, overshooting both ends.
        vec2 ab = to - from;
        float len = length(ab);
        vec2 dir = ab / len;
        vec2 n = vec2(-dir.y, dir.x);
        float ext = min(OVERSHOOT * len, MAX_OVERSHOOT * cellH);
        vec2 a = from - dir * ext;
        float cutLen = len + 2.0 * ext;
        float along = dot(p - a, dir);   // 0 .. cutLen along the cut
        float across = dot(p - a, n);    // signed distance from the cut
        float glowPx = GLOW_RADIUS * cellH;
        float reach = max(SPLIT_REACH * cellH, 8.0 * glowPx);
        if (along > -reach && along < cutLen + reach && abs(across) < reach) {
            // The two sides part quickly, then slide back together. Text near the
            // cut moves the most; the ends of the cut stay closed.
            float x = sinceMove / SLASH_TIME;
            float open = smoothstep(0.0, 0.15, x) * (1.0 - smoothstep(0.35, 1.0, x));
            float ends = smoothstep(0.0, 0.08 * cutLen, along) * (1.0 - smoothstep(0.92 * cutLen, cutLen, along));
            float near = 1.0 - smoothstep(0.2 * cellH, SPLIT_REACH * cellH, abs(across));
            float side = across >= 0.0 ? 1.0 : -1.0;
            vec2 offset = side * (n * SPLIT * 0.5 + dir * SLIDE * 0.5) * cellH * open * ends * near;
            col = screenAt((p - offset) / res).rgb;

            // The opening itself is a dark slit.
            float gap = SPLIT * 0.5 * cellH * open * ends;
            col *= mix(1.0, 0.25, (1.0 - smoothstep(gap * 0.6, gap + 0.5, abs(across))) * clamp(gap, 0.0, 1.0));

            // The blade: a white line drawn across in SWEEP_TIME with a bright
            // head, glowing in the cursor's color, then fading.
            float reached = clamp(sinceMove / SWEEP_TIME, 0.0, 1.0) * cutLen;
            float lit = step(0.0, along) * step(along, reached);
            float fade = 1.0 - smoothstep(SWEEP_TIME, 0.25, sinceMove);
            float taper = smoothstep(0.0, 0.05 * cutLen, along) * (1.0 - smoothstep(0.95 * cutLen, cutLen, along));
            float w = LINE_WIDTH * cellH;
            float core = 1.0 - smoothstep(w * 0.5, w, abs(across));
            float halo = exp(-abs(across) / glowPx);
            col += (white * core + steel * halo * 0.7) * lit * fade * taper;
            float head = sinceMove < SWEEP_TIME ? exp(-pow((along - reached) / (0.6 * cellH), 2.0)) : 0.0;
            col += white * head * halo * 0.8;
        }
    }

    vec2 hs = (hi - lo) * 0.5;

    // Typing: the blade crosses the cursor from its top-left corner to its
    // bottom-right one, the two halves part along the cut for an instant with a
    // dark slit between them, and a few sparks fly off where the blade leaves.
    // The light stays inside the block: nothing smears onto the text around it.
    if (typed && all(lessThan(abs(p - to), hs + vec2(0.35 * cellH))) && cursorDrawn(lo, cur, res)) {
        vec2 a = vec2(lo.x, hi.y), b = vec2(hi.x, lo.y);
        float len = distance(a, b);
        vec2 dir = (b - a) / len, n = vec2(-dir.y, dir.x);
        float along = dot(p - a, dir) / len;   // 0 at the top-left corner, 1 at the bottom-right
        float across = dot(p - a, n);          // signed distance from the cut
        float x = sinceMove / TYPE_TIME;
        float open = smoothstep(0.0, 0.2, x) * (1.0 - smoothstep(0.4, 1.0, x));
        float side = across >= 0.0 ? 1.0 : -1.0;
        float inBlock = all(lessThan(abs(p - to), hs)) ? 1.0 : 0.0;
        col = screenAt((p - n * side * TYPE_SPLIT * cellH * open * inBlock) / res).rgb;
        float gap = TYPE_SPLIT * cellH * open;
        col *= mix(1.0, 0.35, (1.0 - smoothstep(gap * 0.6, gap + 0.5, abs(across))) * clamp(gap, 0.0, 1.0) * inBlock);
        float reached = clamp(sinceMove / TYPE_SWEEP, 0.0, 1.0) * 1.2 - 0.1;  // runs just past both corners
        float lit = step(-0.1, along) * step(along, min(reached, 1.1));
        float fade = 1.0 - smoothstep(TYPE_SWEEP, 0.7 * TYPE_TIME, sinceMove);
        float w = LINE_WIDTH * cellH;
        float core = 1.0 - smoothstep(0.5 * w, 1.2 * w, abs(across));
        float glow = exp(-abs(across) / (0.12 * cellH)) * inBlock;
        col += (white * core * inBlock + mix(white, steel, 0.5) * glow * 0.6) * lit * fade;
        vec2 head = a + dir * len * clamp(reached, 0.0, 1.0);
        if (sinceMove < TYPE_SWEEP) col += white * exp(-distance(p, head) / (0.07 * cellH)) * 1.2;
        float seed = mod(iTimeCursorChange * 5.3, 71.0);
        float age = sinceMove - 0.6 * TYPE_SWEEP;
        if (age >= 0.0 && age < 0.18) {
            for (float k = 0.0; k < 4.0; k += 1.0) {
                float h = hash11(seed + k * 1.7), h2 = hash11(seed + k * 3.1);
                vec2 sp = b + (dir * (0.6 + 0.4 * h2) + n * (h - 0.5) * 1.4) * cellH * (0.1 + 2.0 * age);
                col += white * exp(-distance(p, sp) / (0.035 * cellH)) * (1.0 - age / 0.18) * 0.9;
            }
        }
    }

    // A diagonal streak of light running across the cursor, like steel catching it.
    if (glint >= 0.0 && all(lessThan(abs(p - to), hs)) && cursorDrawn(lo, cur, res)) {
        float diag = (p.x - lo.x) / (hi.x - lo.x) + 0.35 * (p.y - lo.y) / (hi.y - lo.y);
        float pos = mix(-0.3, 1.65, glint);
        col += white * 0.9 * exp(-pow((diag - pos) / 0.12, 2.0));
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
