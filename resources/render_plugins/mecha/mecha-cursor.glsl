// Mecha cursor for Ghostty (`custom-shader`).
// A robot style. The cursor is armor: white, with a blue chest band, a red
// lower plate, a yellow V crest, two sensor eyes and thin panel seams (the letter
// under it stays readable); a sheen runs across it now and then and its eyes
// glint. Typing fires a short blue-white thruster burst behind the cursor, and
// Backspace fires one from its right side as it backs up (both stay inside the
// input row). Enter (a prompt sent in Claude Code, a command
// run in zsh, via `cursor-fx signal`) ignites energy blades from the cursor along
// the input box lines out to the window's edges: a white-hot core, a pink glow,
// a soft hum; in a shell, under the line that was sent. "Done" ignites them at
// both edges instead and they clash in the middle with a burst of sparks. A jump
// along the row (delete-all, Home; JUMP_EFFECTS) fires an energy shot
// from where the cursor was to where it landed. The prompt mark (❯) on its line
// is a steel targeting chevron with a cyan glow that locks on now and then. The input box lines
// can be HUD rails with tick marks (LINE_EFFECT). No full-screen flashes.

// macOS: true. On Linux, change it to false (install.sh does this for you);
// otherwise the effect plays upside down.
#define Y_DOWN true

// --- settings: the ones worth a tweak (edit and save, or use `cursor-fx edit`; Ghostty reloads on Command+Shift+,) ---
// -- Typing
const bool  TYPE_EFFECT      = true;  // a short thruster burst fires behind the cursor as you type
const bool  BACK_THRUST    = true;  // Backspace fires a burst from the cursor's right side as it backs up
const bool  PHRASE_EFFECT  = true;  // picking a character or phrase from your input method plays the typing effect too (like one keystroke)
// -- Enter & done
const bool  ENTER_EFFECT   = true;  // energy blades ignite along the input box on Enter (`cursor-fx enter off` pauses every effect's Enter)
const bool  DONE_EFFECT    = true;  // two blades ignite at the window's edges and clash in the middle when Claude finishes answering (Stop hook) or a long zsh command ends
const float ENTER_TIME     = 1.0;   // seconds the blades stay lit
// -- Delete-all
const bool  JUMP_EFFECTS   = true;  // an energy shot along the row when the cursor jumps along it (`cursor-fx jumps on|off`)
// -- Cursor
const bool  CURSOR_EFFECT  = true;  // the cursor's own look: robot armor, white with blue and red plates, a yellow V crest and sensor eyes
const bool  IDLE_EFFECTS   = true;  // the armor's sheen, the eyes' glint and the prompt's lock-on while idle (`cursor-fx idle on|off`; off keeps the screen calm)
// -- Prompt mark
const bool  PROMPT_EFFECT  = true;  // the prompt mark (❯) at the start of the cursor's line: a steel-blue targeting chevron that locks on now and then
// -- Box lines
const bool  LINE_TINT      = true;  // the input box lines take the cursor's color (a plain color, no glow)
const float LINE_COLOR     = 0.42;  // how much of the cursor's color the lines take, at their own brightness
const bool  LINE_EFFECT    = false; // the input box lines, always: HUD rails with tick marks and a slow scan
// -- Look
const vec3  BEAM_COLOR     = vec3(0.3608, 0.8392, 1.0);  // the energy blades' glow
const vec3  CORE_COLOR     = vec3(1.0, 0.92, 0.96);  // the blades' white-hot core
const vec3  THRUST_COLOR   = vec3(0.62, 0.84, 1.0);  // the thrusters' exhaust
const float BRIGHTNESS     = 1.0;   // overall strength of the light

// --- fine-tuning ---
// -- Typing
const float THRUST_TIME    = 0.2;   // seconds a thruster burst lasts
const float THRUST_LENGTH  = 1.6;  // how far the exhaust reaches, in characters
// -- Enter & done
const float IGNITE_TIME    = 0.22;  // seconds a blade takes to reach its full length
const float BEAM_WIDTH     = 0.05;  // the white-hot core's thickness, in character heights
const float GLOW_RADIUS    = 0.14;  // the pink glow around it, in character heights
const float CLASH_TIME     = 0.4;  // seconds the sparks last where the two blades meet (done)
// -- Delete-all
const float SHOT_TIME      = 0.45;  // seconds an energy shot, its trail and its rings last
// -- Cursor
const vec3  ARMOR_BLUE     = vec3(0.24, 0.44, 0.86);  // the chest band
const vec3  ARMOR_RED      = vec3(0.86, 0.26, 0.30);  // the lower plate
const vec3  ARMOR_YELLOW   = vec3(0.96, 0.76, 0.20);  // the V crest
const float SHEEN_PERIOD   = 5.0;  // seconds between sheens across the armor
const float GLINT_PERIOD   = 4.0;  // seconds between the eyes' glints
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

// --- The blades (Enter, done) ---

// An energy blade along the line at height lineY: out from x0 to both edges (in from
// both edges with `inward`), reaching full length in IGNITE_TIME: a white-hot core, a
// pink glow, a gentle hum, a hot tip while it grows, the emitter flaring as it
// ignites. t = seconds in.
vec3 blade(vec2 p, float lineY, float x0, float t, vec2 res, float cellH, bool inward) {
    float dy = abs(p.y - lineY);
    if (dy > 0.7 * cellH) return vec3(0.0);
    float reach = inward ? 0.5 * res.x : max(x0, res.x - x0);
    float front = reach * pow(clamp(t / IGNITE_TIME, 0.0, 1.0), 0.7);
    float along = inward ? min(p.x, res.x - p.x) : abs(p.x - x0);   // from the emitter
    float lit = 1.0 - smoothstep(front - 0.15 * cellH, front + 0.05 * cellH, along);
    if (lit <= 0.0) return vec3(0.0);
    float fade = 1.0 - smoothstep(0.65, 1.0, t / ENTER_TIME);
    float hum = 0.93 + 0.07 * sin(t * 47.0 + p.x * 0.02);
    vec3 beam = sRGBToLinear(BEAM_COLOR), hot = sRGBToLinear(CORE_COLOR);
    float core = 1.0 - smoothstep(0.5 * BEAM_WIDTH * cellH, BEAM_WIDTH * cellH + 0.5, dy);
    float glow = exp(-dy / (GLOW_RADIUS * cellH)) * 0.7 + exp(-dy / (2.5 * GLOW_RADIUS * cellH)) * 0.18;
    vec3 light = (hot * core + beam * glow) * hum;
    float growing = 1.0 - smoothstep(0.7, 1.0, t / IGNITE_TIME);
    light += (hot * 0.6 + beam * 0.8) * exp(-length(vec2(along - front, 1.5 * dy)) / (0.25 * cellH)) * growing;
    if (!inward) light += (hot * 0.5 + beam) * exp(-length(vec2(p.x - x0, dy)) / (0.3 * cellH)) * exp(-t / 0.12) * 0.8;
    return light * lit * fade * BRIGHTNESS;
}

// Done: where the two blades meet in the middle of a line, a small flash, a ring
// spreading out and ten sparks flying off, all gone in CLASH_TIME.
vec3 clash(vec2 p, float lineY, float t, vec2 res, float cellH) {
    float tc = t - IGNITE_TIME;
    if (tc < 0.0 || tc > CLASH_TIME) return vec3(0.0);
    vec2 d = p - vec2(0.5 * res.x, lineY);
    float r = length(d);
    if (r > 2.6 * cellH) return vec3(0.0);
    float u = tc / CLASH_TIME, k = (1.0 - u) * (1.0 - u);
    vec3 hot = sRGBToLinear(CORE_COLOR), beam = sRGBToLinear(BEAM_COLOR);
    vec3 light = (hot * 0.7 + beam * 0.6) * exp(-r / (0.3 * cellH)) * k;
    light += beam * exp(-abs(r - u * 1.6 * cellH) / (0.05 * cellH)) * (1.0 - u) * 0.6;
    for (int i = 0; i < 10; i++) {
        float a = hash11(float(i) * 7.1 + 3.0) * 6.2832, sp = (0.8 + 1.2 * hash11(float(i) * 3.3)) * cellH;
        vec2 dir = vec2(cos(a), 0.6 * sin(a));
        vec2 head = dir * sp * (1.0 - k);
        light += mix(hot, beam, u) * exp(-sdSegment(d, head - dir * 0.25 * cellH, head) / max(0.8, 0.02 * cellH)) * (1.0 - u) * 0.7;
    }
    return light * BRIGHTNESS;
}

// Enter: blades on both input box lines, out from the cursor; done: in from the edges
// and clashing. In a shell (no box) the one line is just under the row that was sent.
vec3 enterEffect(vec2 p, vec4 cur, vec2 center, float cellW, float cellH, vec2 res, float t, bool inward) {
    if (p.y - center.y > 4.5 * cellH || center.y - p.y > 2.6 * cellH) return vec3(0.0);
    vec2 lines = boxLines(cur, center, cellW, cellH, res);
    vec3 light = vec3(0.0);
    for (int k = 0; k < 2; k++) {
        float lineY = k == 0 ? lines.x : lines.y;
        if (lineY < 0.0) continue;
        light += blade(p, lineY, center.x, t, res, cellH, inward);
        if (inward) light += clash(p, lineY, t, res, cellH);
    }
    return light;
}

// --- Typing and jumps ---

// A thruster burst behind the cursor: a blue-white plume at mid height, widening and
// flickering, gone in THRUST_TIME. dir 1: it moved forward (typing), the plume trails
// left from its left edge; dir -1: it backed up (Backspace), the plume fires right.
vec3 thruster(vec2 p, vec2 lo, vec2 hi, float cellH, float cellW, float t, float dir) {
    float u = t / THRUST_TIME;
    if (u >= 1.0) return vec3(0.0);
    float len = THRUST_LENGTH * cellW * (0.5 + 0.5 * smoothstep(0.0, 0.3, u));
    float back = dir > 0.0 ? lo.x - p.x : p.x - hi.x;
    if (back < -0.1 * cellW || back > len) return vec3(0.0);
    float k = clamp(back / len, 0.0, 1.0), dy = abs(p.y - 0.5 * (lo.y + hi.y));
    float width = (0.10 + 0.22 * k) * cellH;
    if (dy > 1.6 * width) return vec3(0.0);
    float grain = clHash(vec2(floor(p.x / 2.0), floor(t * 40.0)));
    float body = exp(-pow(dy / width, 2.0)) * (1.0 - k) * (0.75 + 0.25 * grain);
    float core = exp(-pow(dy / (0.35 * width), 2.0)) * (1.0 - k) * (1.0 - k);
    vec3 th = sRGBToLinear(THRUST_COLOR);
    return (th * body * 0.7 + mix(th, vec3(1.0), 0.6) * core * 0.6) * (1.0 - u) * BRIGHTNESS;
}

// An energy shot along the row from where the cursor was to where it landed: a
// bright bolt crossing in 0.12 s, the path glowing for a moment, a ring at each end.
vec3 shot(vec2 p, vec2 from, vec2 to, float cellH, float t) {
    if (t >= SHOT_TIME) return vec3(0.0);
    vec2 dir = to - from;
    float L2 = dot(dir, dir);
    if (L2 < 1.0) return vec3(0.0);
    vec2 pa = p - from;
    float s = clamp(dot(pa, dir) / L2, 0.0, 1.0), d = length(pa - dir * s);
    float rm = length(p - from), ri = length(p - to);
    if (d > 0.8 * cellH && rm > 1.2 * cellH && ri > 1.2 * cellH) return vec3(0.0);
    vec3 beam = sRGBToLinear(BEAM_COLOR), hot = sRGBToLinear(CORE_COLOR);
    float travel = clamp(t / 0.12, 0.0, 1.0);
    float bolt = smoothstep(travel - 0.3, travel - 0.05, s) * (1.0 - smoothstep(travel, travel + 0.02, s))
               * (t < 0.12 ? 1.0 : exp(-(t - 0.12) / 0.05));
    float path = step(s, travel) * exp(-t / 0.15);
    float core = 1.0 - smoothstep(0.03 * cellH, 0.06 * cellH + 0.5, d), glow = exp(-d / (0.12 * cellH));
    vec3 light = (hot * core + beam * glow) * (bolt + 0.35 * path);
    light += beam * exp(-abs(rm - t / 0.15 * 0.6 * cellH) / (0.05 * cellH)) * (1.0 - smoothstep(0.0, 0.15, t)) * 0.6;
    float ti = t - 0.12;
    if (ti > 0.0) light += (hot * 0.5 + beam) * exp(-abs(ri - ti / 0.3 * 0.9 * cellH) / (0.06 * cellH)) * (1.0 - smoothstep(0.0, 0.3, ti)) * 0.7;
    return light * BRIGHTNESS;
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
        if (sinceMove < ENTER_TIME)
            fragColor.rgb = min(fragColor.rgb + enterEffect(p, cur, center, cellW, cellH, res, sinceMove, done), vec3(1.0));
        return;
    }
    // Only the color changed, the cursor jumped to another row (a program redrawing),
    // or along its row with JUMP_EFFECTS off: not a move.
    if (moved < 0.5 || (moved >= 3.0 && !(JUMP_EFFECTS && abs(cur.y - prv.y) < 0.5 * cellH))) return;
    vec3 light = vec3(0.0);
    if (moved >= 3.0) light += shot(p, from, center, cellH, sinceMove);
    else if (typeOn && (cur.x > prv.x + 0.5 * cellW || (BACK_THRUST && cur.x < prv.x - 0.5 * cellW)) && sinceMove < THRUST_TIME) {
        // Forward (typing): the plume trails behind; back (Backspace): it fires from the right side.
        float ink = smoothstep(0.06, 0.2, colorDist(fragColor.rgb, iBackgroundColor));  // fainter over letters
        light += thruster(p, lo, hi, cellH, cellW, sinceMove, cur.x > prv.x ? 1.0 : -1.0) * (1.0 - 0.6 * ink);
    }
    fragColor.rgb = min(fragColor.rgb + light, vec3(1.0));
}

// --- The cursor's look (CURSOR_EFFECT): mobile-suit armor ---

// The armor is painted over the block's own pixels (a letter under the cursor stays):
// lit from above, a blue chest band, a red lower plate, a yellow V crest, two sensor
// eyes, panel seams and a bevel (all stepping aside around a letter under the cursor);
// a sheen crosses it every SHEEN_PERIOD and the eyes
// glint every GLINT_PERIOD. Outside the block only a faint cool rim of light.
// Colors go into the screen texture's own color space (found from the block).
vec3 cursorLook(vec2 p, vec3 col, vec2 res) {
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w), hi = vec2(cur.x + cur.z, cur.y), center = 0.5 * (lo + hi);
    float cellH = max(cur.w, cellWOf(cur) * 2.0);
    vec2 q = abs(p - center) - 0.5 * (hi - lo);
    if (max(q.x, q.y) > 0.3 * cellH) return col;
    // Only over a drawn block (not over pinyin being composed, where Ghostty hides the cursor).
    if (!cursorDrawn(lo, cur, res)) return col;
    float t = IDLE_EFFECTS ? iTime : 0.0;
    if (max(q.x, q.y) > 0.0) {
        if (!cursorDrawn(lo, cur, res)) return col;
        float e = length(max(q, 0.0));
        return col + sRGBToLinear(vec3(0.55, 0.7, 1.0)) * exp(-0.5 * pow(e / (0.03 * cellH), 2.0)) * 0.18 * CURSOR_STRENGTH * BRIGHTNESS;
    }
    float onBlock = clOnBlock(col);
    if (onBlock <= 0.0) return col;
    // A letter under the cursor reads: the band, plate, crest, seams and eyes step aside
    // within a pixel and a half of its strokes, and are calmer over the whole block.
    float clear = 0.0, letter = 0.0;
    for (int i = 0; i < 8; i++) {
        float a = float(i) * 0.7854;
        vec2 o = p + 1.6 * vec2(cos(a), sin(a));
        if (o.x < lo.x + 0.5 || o.x > hi.x - 0.5 || o.y < lo.y + 0.5 || o.y > hi.y - 0.5) continue;
        vec3 sc = screenAt(o / res).rgb;
        if (colorDist(sc, iCursorText) < colorDist(sc, iCurrentCursorColor.rgb)) clear = 1.0;
    }
    for (int i = 0; i < 15; i++) {
        vec3 sc = screenAt(vec2(lo.x + (0.25 + 0.25 * float(i % 3)) * (hi.x - lo.x), lo.y + (0.15 + 0.175 * float(i / 3)) * (hi.y - lo.y)) / res).rgb;
        if (colorDist(sc, iCursorText) < colorDist(sc, iCurrentCursorColor.rgb)) letter = 1.0;
    }
    float detail = (1.0 - clear) * (1.0 - 0.45 * letter);
    bool lin = distance(col, sRGBToLinear(iCurrentCursorColor.rgb)) < distance(col, iCurrentCursorColor.rgb);
    vec3 blue = ARMOR_BLUE, red = ARMOR_RED, yel = ARMOR_YELLOW, eye = vec3(0.45, 1.0, 0.55);
    if (lin) { blue = sRGBToLinear(blue); red = sRGBToLinear(red); yel = sRGBToLinear(yel); eye = sRGBToLinear(eye); }
    vec2 sz = hi - lo, P = p - lo, uv = P / sz;
    vec3 armor = col * (0.88 + 0.16 * uv.y);
    armor = mix(armor, blue, step(0.40, uv.y) * step(uv.y, 0.54) * detail);
    armor = mix(armor, red, step(uv.y, 0.16) * detail);
    // V crest from the top corners down to the middle
    vec2 A = vec2(0.1, 0.95) * sz, B = vec2(0.5, 0.79) * sz, C = vec2(0.9, 0.95) * sz;
    float v = 1.0 - smoothstep(0.6, 1.4, min(sdSegment(P, A, B), sdSegment(P, C, B)));
    armor = mix(armor, yel, v * detail);
    // panel seams and a bevel
    float seam = max(1.0 - smoothstep(0.5, 1.0, abs(P.y - 0.68 * sz.y)),
                     (1.0 - smoothstep(0.5, 1.0, abs(P.x - 0.5 * sz.x))) * step(0.16 * sz.y, P.y) * step(P.y, 0.40 * sz.y));
    armor *= 1.0 - 0.3 * seam * detail;
    armor *= 1.0 + 0.14 * (1.0 - smoothstep(0.5, 1.5, sz.y - P.y)) - 0.2 * (1.0 - smoothstep(0.5, 1.5, P.y));
    // sensor eyes: dim, glinting for 0.3 s every GLINT_PERIOD
    float g = IDLE_EFFECTS ? fract(t / GLINT_PERIOD) * GLINT_PERIOD : 1.0;
    float glint = 0.25 + 0.75 * (1.0 - smoothstep(0.0, 0.3, g)) * step(0.0, g);
    for (int i = 0; i < 2; i++) {
        vec2 e = vec2(i == 0 ? 0.3 : 0.7, 0.66) * sz;
        float d = length((P - e) / vec2(1.0, 0.7));
        armor = mix(armor, eye * (0.4 + 0.8 * glint), (1.0 - smoothstep(0.08 * sz.x, 0.08 * sz.x + 1.0, d)) * detail);
    }
    // the sheen: a soft diagonal band crossing in the first 0.8 s of each period
    float ph = fract(t / SHEEN_PERIOD) * SHEEN_PERIOD / 0.8;
    if (IDLE_EFFECTS && ph < 1.0) armor += 0.22 * exp(-pow((uv.x + 0.5 * uv.y - (ph * 2.2 - 0.6)) / 0.12, 2.0));
    return mix(col, armor, onBlock * CURSOR_STRENGTH);
}

// --- The prompt mark's look (PROMPT_EFFECT) ---

// The prompt mark as a targeting chevron in the armor's steel white with a cyan glow; every
// 4 s two pairs of cyan corner brackets close in on it and fade (a lock-on).
vec3 markLook(vec2 p, vec3 col, float ink, float halo, vec2 m, float w, float h, float t, float live) {
    vec3 steel = sRGBToLinear(iCurrentCursorColor.rgb), beam = sRGBToLinear(BEAM_COLOR);
    col = mix(col, mix(steel, vec3(1.0), 0.3), ink);
    col += beam * halo * (1.0 - ink) * 0.35;
    float u = fract(t / 4.0) * 4.0;
    if (live > 0.0 && u < 0.6) {
        vec2 d = abs(p - (m + vec2(0.5 * w, -0.5 * h)));
        float s = mix(0.6 * h, 0.42 * h, smoothstep(0.0, 0.35, u));
        float box = abs(max(d.x, d.y) - s);
        float corner = step(0.55 * s, min(d.x, d.y));
        col += mix(beam, vec3(1.0), 0.3) * (1.0 - smoothstep(0.6, 1.4, box)) * corner * (1.0 - smoothstep(0.45, 0.6, u)) * 0.8;
    }
    return col;
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

// The box lines as HUD rails: small ticks along them (every fifth one longer) and a
// slow scan of light running across.
vec3 lineLook(vec2 p, vec3 col, float lineY, float h, vec2 res, float t, float live) {
    vec3 c = sRGBToLinear(vec3(0.62, 0.78, 1.0));
    float dy = abs(p.y - lineY), spacing = 1.2 * h;
    float major = step(mod(floor(p.x / spacing), 5.0), 0.5);
    float tick = (1.0 - smoothstep(0.6, 1.2, abs(mod(p.x, spacing) - 0.5 * spacing))) * step(dy, mix(0.1, 0.2, major) * h) * mix(0.35, 0.8, major);
    float scan = exp(-abs(p.x - mod(t * 0.35 * res.x, 1.4 * res.x)) / (2.0 * h)) * exp(-dy / (0.05 * h)) * live;
    return col + c * (tick * 0.5 + scan * 0.25) * LINE_STRENGTH;
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
