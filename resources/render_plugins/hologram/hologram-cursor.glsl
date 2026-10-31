// Hologram cursor for Ghostty (`custom-shader`).
// The cursor is a translucent cyan projection: scanlines, a brighter projector
// base, a thin bright frame, a band of light drifting up it and, now and then, a
// tiny flicker; a letter under it shows as projected light. Typing prints each
// letter (a Chinese character or phrase as a whole): a scanline rises through its cell, the letter appearing below it and a
// few voxels flickering above. Enter (a prompt sent in Claude Code, a command run
// in zsh, via `cursor-fx signal`) turns the input box into a hologram: a
// projector's scan runs out from the cursor along it, the box lines light up
// cyan and a wireframe grid flickers in behind the scan, then everything fades;
// in a shell, a strip under the line that was sent. "Done" runs the scan in from
// both edges; where they meet the lines pulse once. A jump along the row
// (delete-all, Home; JUMP_EFFECTS) leaves a scan streak along it. The prompt
// mark (❯) is projected in cyan, interlaced. No full-screen flashes.

// macOS: true. On Linux, change it to false (install.sh does this for you);
// otherwise the effect plays upside down.
#define Y_DOWN true

// --- settings: the ones worth a tweak (edit and save, or use `cursor-fx edit`; Ghostty reloads on Command+Shift+,) ---
// -- Typing
const bool  TYPE_EFFECT    = true;  // each letter you type is printed by a scanline rising through its cell (Chinese as a whole)
const bool  PHRASE_EFFECT  = true;  // picking a character or phrase from your input method plays the typing effect too (like one keystroke)
// -- Enter & done
const bool  ENTER_EFFECT   = true;  // the input box turns into a hologram on Enter: a scan runs out from the cursor and a wireframe grid flickers in (`cursor-fx enter off` pauses every effect's Enter)
const bool  DONE_EFFECT    = true;  // the scan comes in from both edges and pulses where they meet when Claude finishes answering (Stop hook) or a long zsh command ends
const float ENTER_TIME     = 1.3;   // seconds the hologram lasts
const float GRID           = 0.5;  // how strong the wireframe grid is
// -- Delete-all
const bool  JUMP_EFFECTS   = true;  // a scan streak along the row when the cursor jumps along it (`cursor-fx jumps on|off`)
// -- Cursor
const bool  CURSOR_EFFECT  = true;  // the cursor's own look: a translucent cyan projection with scanlines, a bright base and a thin frame
const bool  IDLE_EFFECTS   = true;  // the cursor's drifting band and its rare flicker, the prompt's flicker (`cursor-fx idle on|off`; off keeps the screen calm)
// -- Prompt mark
const bool  PROMPT_EFFECT  = true;  // the prompt mark (❯) at the start of the cursor's line: projected in cyan, interlaced
// -- Box lines
const bool  LINE_TINT      = true;  // the input box lines take the cursor's color (a plain color, no glow)
const float LINE_COLOR     = 0.42;  // how much of the cursor's color the lines take, at their own brightness
const bool  LINE_EFFECT    = false; // the input box lines, always: projected edges with a glint running along
// -- Look
const vec3  HOLO_COLOR     = vec3(0.35, 0.85, 1.0);  // the projection's light
const float BRIGHTNESS     = 1.0;   // overall strength of the light

// --- fine-tuning ---
// -- Typing
const float PRINT_TIME     = 0.2;   // seconds a letter takes to print
// -- Enter & done
const float SCAN_TIME      = 0.35;  // seconds the scan takes to reach the window's edge
const float GRID_SPACING   = 2.0;   // the grid's columns, in characters
// -- Delete-all
const float STREAK_TIME    = 0.4;   // seconds a jump's streak lasts
// -- Cursor
const float CURSOR_ALPHA   = 0.6;   // how solid the projected cursor looks (1 = as solid as a plain cursor)
const float SCANLINE_GAP   = 3.0;   // pixels from one scanline to the next
const float BAND_PERIOD    = 3.5;   // a band of light drifts up the cursor once every this many seconds
const float FLICKER_PERIOD = 4.5;  // seconds between the projection's brief flickers
const float CURSOR_STRENGTH = 1.0;  // how strong the cursor's own look is
// -- Box lines
const float LINE_STRENGTH  = 1.0;   // how strong the box lines' own look is
// -- Timing
const float SETTLE         = 0.04;  // seconds the cursor must stay put before an effect starts
const float JUMP_SETTLE    = 0.12;  // the same after a jump (gives the Enter signal time to arrive)

// Ghostty hands shaders y-down pixels on macOS (Metal) and y-up ones on Linux
// (OpenGL). The effects are written y-up, so with Y_DOWN (top of the file)
// mainImage turns its pixel and the cursors over and screenAt() turns every
// read of the screen back.

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

bool oddByte(float v) {
    return mod(floor(v * 255.0 + 0.5), 2.0) > 0.5;
}

// Enter (a prompt sent in Claude Code, a command run in zsh): `cursor-fx signal`
// sets the lowest bit of the cursor color's blue value for a moment, and flips the
// red value's lowest bit with every signal, so a new one is always a change of
// color. True while this frame belongs to that moment: the signal just started
// (blue turned odd, or red flipped while it was), or `back`: the cursor jumped while
// it was on in a way that means the same Enter (Claude Code clearing its input box
// after the signal arrived).
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

// The height of the line that was just sent, for a shell (no input box): the
// cursor's row, or the row above when the cursor is at the left edge. Claude
// Code's sent prompt would show on a lighter bar above the input box; that is
// looked for first (right end of a row off the background near its top and
// bottom), so the same shader works if the box lines ever go missing.
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

// Distance from p to the segment a-b.
float sdSegment(vec2 p, vec2 a, vec2 b) {
    vec2 pa = p - a, ba = b - a;
    return length(pa - ba * clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0));
}

// Is there ink in the cell whose left edge is at x (cursor row lo.y..hi.y)? A grid
// of 3 x 6 points, so thin letters (an "o", an "l") count too.
bool cellInk(float x, vec2 lo, vec4 cur, vec2 res) {
    for (int i = 0; i < 18; i++) {
        vec2 at = vec2(x + cur.z * (0.25 + 0.25 * float(i % 3)), lo.y + cur.w * (0.18 + 0.13 * float(i / 3)));
        if (colorDist(screenAt(at / res).rgb, iBackgroundColor) > 0.08) return true;
    }
    return false;
}

// Pixel hash, value noise, box distance and the block test, shared by the looks.
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

// The projection's color in the screen texture's own color space (decided from the
// window's corner, which is always background).
vec3 holoColor(vec2 res) {
    vec3 corner = screenAt(vec2(1.5, 1.5) / res).rgb;
    bool lin = distance(corner, sRGBToLinear(iBackgroundColor)) < distance(corner, iBackgroundColor);
    return lin ? sRGBToLinear(HOLO_COLOR) : HOLO_COLOR;
}

// --- Enter and done: the input box as a hologram ---

// A scan front runs out from the cursor (in from both edges with `inward`) in
// SCAN_TIME: a bright curtain of light spanning the box at the front; behind it the
// box lines light up and wireframe columns flicker in, each after a small delay,
// over faint scanlines; when inward scans meet the lines pulse once. Returns the color.
vec3 enterEffect(vec2 p, vec3 col, vec4 cur, vec2 center, float cellW, float cellH, vec2 res, float t, bool inward) {
    if (p.y - center.y > 4.5 * cellH || center.y - p.y > 2.6 * cellH) return col;
    vec2 lines = boxLines(cur, center, cellW, cellH, res);
    bool shell = lines.x < 0.0;
    if (lines.y < 0.0 && shell) return col;
    float y1 = shell ? lines.y + 0.12 * cellH : lines.x, y0 = shell ? lines.y - 0.12 * cellH : lines.y;
    if (y0 > y1) { float s = y0; y0 = y1; y1 = s; }
    if (p.y < y0 - 0.4 * cellH || p.y > y1 + 0.4 * cellH) return col;
    vec3 hc = holoColor(res);
    float reach = inward ? 0.5 * res.x : max(center.x, res.x - center.x);
    float along = inward ? min(p.x, res.x - p.x) : abs(p.x - center.x);
    float tp = t - SCAN_TIME * clamp(along / reach, 0.0, 1.0);        // seconds since the scan passed here
    float fade = 1.0 - smoothstep(0.6, 1.0, t / ENTER_TIME);
    bool inBox = p.y >= y0 && p.y <= y1;
    vec3 light = vec3(0.0);
    // the curtain at the front, spanning the box
    float front = exp(-abs(tp) / 0.025) * (1.0 - smoothstep(SCAN_TIME * 0.9, SCAN_TIME * 1.05, t));
    if (inBox) light += (hc * 0.5 + mix(hc, vec3(1.0), 0.6) * exp(-abs(tp) / 0.008) * 0.6) * front;
    if (tp > 0.0) {
        // the box lines, lit cyan (a pulse when inward scans meet)
        float meet = inward ? exp(-max(0.0, t - SCAN_TIME) / 0.15) * step(SCAN_TIME, t) : 0.0;
        for (int k = 0; k < 2; k++) {
            float ly = k == 0 ? lines.x : lines.y;
            if (ly < 0.0) continue;
            float dy = abs(p.y - ly);
            light += hc * (exp(-dy / (0.03 * cellH)) * (0.6 + 0.6 * meet) + exp(-dy / (0.12 * cellH)) * (0.12 + 0.2 * meet));
        }
        if (inBox && !shell) {
            float gs = GRID_SPACING * cellW, column = floor(p.x / gs);
            float delay = 0.04 + 0.16 * hash11(column * 1.73 + 0.4);
            float on = step(delay, tp) * (step(delay + 0.04, tp) + (1.0 - step(delay + 0.04, tp)) * step(0.5, hash11(column + floor(t * 40.0))));
            float gx = min(mod(p.x, gs), gs - mod(p.x, gs));
            light += hc * (1.0 - smoothstep(0.5, 1.3, gx)) * on * 0.6 * GRID;
            light += hc * 0.06 * step(mod(floor(p.y), 3.0), 0.5) * on;   // faint scanlines over the panel
        }
    }
    return min(col + light * fade * BRIGHTNESS, vec3(1.0));
}

// --- Typing and jumps ---

// TYPE_EFFECT: the typed letter (cell left edge x0) is printed by a scanline rising
// through its cell; below the line the letter shows, tinted cyan at first; above it
// it hides and a few voxels where the letter will be flicker.
vec3 materialize(vec2 p, vec3 col, float x0, vec2 lo, vec4 cur, float t, vec3 hc, vec2 res) {
    float u = t / PRINT_TIME;
    float ly = lo.y + u * cur.w;                                     // the scanline (rising)
    float letter = smoothstep(0.015, 0.06, colorDist(col, iBackgroundColor));
    vec3 light = mix(hc, vec3(1.0), 0.5) * exp(-abs(p.y - ly) / max(1.0, 0.03 * cur.w)) * 0.9;
    if (p.y > ly) {
        vec3 bg = screenAt(vec2(x0 + 1.5, cur.y - 1.5) / res).rgb;
        col = mix(col, bg, letter);
        vec2 vs = vec2(cur.z, cur.w) / vec2(3.0, 6.0);                 // voxels: 3 x 6 a cell
        vec2 cell = floor((p - vec2(x0, lo.y)) / vs), cc = vec2(x0, lo.y) + (cell + 0.5) * vs;
        float covered = smoothstep(0.015, 0.06, colorDist(screenAt(cc / res).rgb, iBackgroundColor));
        float twinkle = step(0.55, hash21(cell + floor(t * 25.0) + 7.0));
        vec2 inV = abs(p - cc) / (0.5 * vs);
        light += hc * covered * twinkle * (1.0 - step(0.6, max(inV.x, inV.y))) * 0.55;
    } else {
        light += hc * letter * 0.6 * (1.0 - u);
    }
    return col + light * BRIGHTNESS;
}

// A jump: a scan streak along the row from where the cursor was to where it landed,
// interlaced, fading in STREAK_TIME.
vec3 streak(vec2 p, vec2 from, vec2 to, float cellH, float t, vec3 hc) {
    if (t >= STREAK_TIME) return vec3(0.0);
    float x0 = min(from.x, to.x), x1 = max(from.x, to.x);
    float dy = abs(p.y - to.y);
    if (p.x < x0 - cellH || p.x > x1 + cellH || dy > 0.6 * cellH) return vec3(0.0);
    float travel = clamp(t / 0.1, 0.0, 1.0);
    float headX = mix(from.x, to.x, travel);
    float passed = (to.x < from.x) ? step(headX, p.x) * step(p.x, from.x) : step(from.x, p.x) * step(p.x, headX);
    float u = t / STREAK_TIME;
    float body = exp(-dy / (0.12 * cellH)) * passed * (0.75 + 0.25 * step(mod(floor(p.y), 3.0), 0.5));
    float head = exp(-length(vec2(p.x - headX, dy)) / (0.3 * cellH)) * (1.0 - travel * step(1.0, travel));
    return (hc * body * 0.6 + mix(hc, vec3(1.0), 0.5) * head * 0.7) * (1.0 - u) * BRIGHTNESS;
}

// Everything that answers the cursor's moves and the signals (fragCoord is
// already y-up).
void effects(inout vec4 fragColor, vec2 fragCoord) {
    vec2 res = iResolution.xy;
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w), hi = vec2(cur.x + cur.z, cur.y);
    float cellH = max(cur.w, cellWOf(cur) * 2.0), cellW = max(cellWOf(cur), cellH * 0.4);
    vec2 center = (lo + hi) * 0.5;
    vec4 prv = upCursor(iPreviousCursor);
    vec2 from = vec2(prv.x + prv.z * 0.5, prv.y - prv.w * 0.5);
    vec2 p = fragCoord;
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
    // Enter signal can arrive first).
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
        if (sinceMove < ENTER_TIME) fragColor.rgb = enterEffect(p, fragColor.rgb, cur, center, cellW, cellH, res, sinceMove, done);
        return;
    }
    // Only the color changed, the cursor jumped to another row (a program redrawing),
    // or along its row with JUMP_EFFECTS off: not a move.
    if (moved < 0.5 || (moved >= 3.0 && !(JUMP_EFFECTS && abs(cur.y - prv.y) < 0.5 * cellH))) return;
    vec3 hc = holoColor(res);
    if (moved >= 3.0) { fragColor.rgb = min(fragColor.rgb + streak(p, from, center, cellH, sinceMove, hc), vec3(1.0)); return; }
    // Typed text (a letter, a Chinese character, a phrase an input method committed):
    // each of its cells with ink is printed by the rising scanline, all at once (a
    // space has nothing to print).
    if (typeOn && typedText && sinceMove < PRINT_TIME
            && p.x > prv.x && p.x < lo.x && p.y > lo.y && p.y < hi.y) {
        float cw = cellWOf(cur), x0 = prv.x + floor((p.x - prv.x) / cw) * cw;
        if (cellInk(x0, lo, cur, res))
            fragColor.rgb = min(materialize(p, fragColor.rgb, x0, lo, cur, sinceMove, hc, res), vec3(1.0));
    }
}

// --- The cursor's look (CURSOR_EFFECT): a projection ---

// Over the block's own pixels: the background shows through a cyan projection that
// is denser at its base, with scanlines, a thin bright frame, a bright base line, a
// band of light drifting up and a rare 80 ms flicker; a letter under the cursor shows
// as projected light. Outside, a tight, faint cyan glow.
vec3 cursorLook(vec2 p, vec3 col, vec2 res) {
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w), hi = vec2(cur.x + cur.z, cur.y), center = 0.5 * (lo + hi);
    float cellH = max(cur.w, cellWOf(cur) * 2.0);
    vec2 q = abs(p - center) - 0.5 * (hi - lo);
    if (max(q.x, q.y) > 0.3 * cellH) return col;
    // Only over a drawn block (not over pinyin being composed, where Ghostty hides the cursor).
    if (!cursorDrawn(lo, cur, res)) return col;
    float t = IDLE_EFFECTS ? iTime : 0.0;
    vec3 hc = holoColor(res);
    if (max(q.x, q.y) > 0.0) {
        if (!cursorDrawn(lo, cur, res)) return col;
        float e = length(max(q, 0.0));
        return col + hc * (exp(-0.5 * pow(e / (0.03 * cellH), 2.0)) * 0.3 + exp(-0.5 * pow(e / (0.1 * cellH), 2.0)) * 0.12) * CURSOR_STRENGTH * BRIGHTNESS;
    }
    float onBlock = clOnBlock(col);
    vec2 sz = hi - lo, P = p - lo, uv = P / sz;
    vec3 bg = screenAt(vec2(1.5, 1.5) / res).rgb;
    float flick = 1.0;
    if (IDLE_EFFECTS) { float f = mod(t, FLICKER_PERIOD); flick = f < 0.08 ? 0.72 : 1.0; }
    float scan = 0.72 + 0.28 * step(1.0, mod(floor(P.y), SCANLINE_GAP));
    float a = CURSOR_ALPHA * (0.6 + 0.4 * (1.0 - uv.y)) * scan * flick;
    float band = IDLE_EFFECTS ? exp(-pow((uv.y - (fract(t / BAND_PERIOD) * 1.4 - 0.2)) / 0.07, 2.0)) * 0.35 : 0.0;
    vec3 body = mix(bg, hc, a) + hc * band;
    float edge = 1.0 - smoothstep(0.5, 1.5, min(min(P.x, sz.x - P.x), sz.y - P.y));
    body = mix(body, mix(hc, vec3(1.0), 0.3) * flick, 0.55 * edge);
    body += mix(hc, vec3(1.0), 0.5) * (1.0 - smoothstep(0.5, 2.0, P.y)) * 0.6;
    vec3 glyph = mix(hc, vec3(1.0), 0.65) * scan * flick;
    return mix(col, mix(glyph, body, onBlock), CURSOR_STRENGTH);
}

// --- The prompt mark's look (PROMPT_EFFECT) ---

// The prompt mark projected in cyan: interlaced rows, a soft glow, and every 5 s a
// short flicker.
vec3 markLook(vec2 p, vec3 col, float ink, float halo, vec2 m, float w, float h, float t, float live) {
    vec2 res = iResolution.xy;
    vec3 hc = holoColor(res);
    float scan = 0.7 + 0.3 * step(1.0, mod(floor(p.y), 3.0));
    float flick = live > 0.0 && mod(t, 5.0) < 0.08 ? 0.65 : 1.0;
    col = mix(col, mix(hc, vec3(1.0), 0.35) * scan * flick, ink);
    return col + hc * halo * (1.0 - ink) * 0.4 * flick;
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

// The box lines as projected edges: tinted cyan, interlaced, a glint running along.
vec3 lineLook(vec2 p, vec3 col, float lineY, float h, vec2 res, float t, float live) {
    vec3 hc = holoColor(res);
    float dy = abs(p.y - lineY);
    float glint = exp(-abs(p.x - mod(t * 0.4 * res.x, 1.4 * res.x)) / (3.0 * h)) * live;
    return col + hc * (exp(-dy / (0.03 * h)) * (0.25 + 0.6 * glint) + exp(-dy / (0.12 * h)) * 0.06) * LINE_STRENGTH;
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

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 res = iResolution.xy;
    if (Y_DOWN) fragCoord.y = res.y - fragCoord.y;
    fragColor = screenAt(fragCoord / res);
    if (iCursorVisible == 0) return;
    if (iFocus != 0) effects(fragColor, fragCoord);
    // The line color stays when the window loses focus; the moving looks don't.
    if (((LINE_EFFECT && iFocus != 0) || (LINE_TINT && LINE_COLOR > 0.0)) && iCurrentCursorStyle == CURSORSTYLE_BLOCK) fragColor.rgb = min(linesLook(fragCoord, fragColor.rgb), vec3(1.0));
    if (iFocus != 0 && CURSOR_EFFECT && iCurrentCursorStyle == CURSORSTYLE_BLOCK) fragColor.rgb = min(cursorLook(fragCoord, fragColor.rgb, res), vec3(1.0));
    if (PROMPT_EFFECT && iCurrentCursorStyle == CURSORSTYLE_BLOCK) fragColor.rgb = min(promptLook(fragCoord, fragColor.rgb), vec3(1.0));
}
