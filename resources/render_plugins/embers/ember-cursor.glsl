// Ember cursor for Ghostty (`custom-shader`).
// The cursor smolders like a hot coal. Typing kicks up glowing embers that
// rise, drift and cool from yellow-white to deep red before they go out;
// a jump along the row (delete-all, Home) throws more, with JUMP_EFFECTS;
// jumps to another row throw none. While idle, a stray ember floats up now and
// then. Only the area around and above the cursor changes. Enter (a prompt sent
// in Claude Code, a command run in zsh, via `cursor-fx signal`) makes the lines
// of Claude Code's input box flare with heat, out from the cursor, and throw up
// a wave of embers along their length.
// Its coal has a living surface: hot cracks and cooler patches shift slowly
// across the block. The prompt mark (❯) on its line glows like an ember.
// The input box's lines are hot wires: they glow, hot spots drift along them,
// and an ember rises off them now and then.

// macOS: true. On Linux, change it to false (install.sh does this for you);
// otherwise the effect plays upside down.
#define Y_DOWN true

// --- settings: the ones worth a tweak (edit and save, or use `cursor-fx edit`; Ghostty reloads on Command+Shift+,) ---
// -- Typing
const bool  TYPE_EFFECT    = true;  // every keystroke kicks up glowing embers that rise and cool
const bool  PHRASE_EFFECT  = true;  // picking a character or phrase from your input method plays the typing effect too (like one keystroke)
const float TYPE_EMBERS = 7.0;   // embers per keystroke
const float LIFE        = 0.9;  // seconds an ember lives (each one varies a little)
// -- Enter & done
const bool  ENTER_EFFECT = true; // embers off the input box lines on Enter (`cursor-fx enter off` pauses every effect's Enter)
const bool  DONE_EFFECT = true;  // play it again, from the edges inward, when Claude finishes answering (Stop hook) or a long zsh command ends
const float ENTER_EMBERS = 28.0;  // embers along the box lines (at most 32)
const float ENTER_TIME  = 1.45;  // seconds until the last Enter ember is out
// -- Delete-all
const bool  JUMP_EFFECTS = true;  // big burst when the cursor jumps along its row (`cursor-fx jumps on|off`)
// -- Cursor
const bool  CURSOR_EFFECT  = true;  // the cursor's own look: a burning coal: a glow, and hot cracks shifting across it
const bool  IDLE_EFFECTS = true;  // the stray ember while idle (`cursor-fx idle on|off`; off keeps the screen calm)
const float COAL_GLOW   = 0.25;  // the steady warm glow around the cursor
const float IDLE_CHANCE = 0.6;  // how likely a stray ember is each time it gets a chance (0 to 1)
// -- Prompt mark
const bool  PROMPT_EFFECT  = true;  // the prompt mark (❯) at the start of the cursor's line: a glowing ember: hot yellow-orange that flickers, a warm glow
// -- Box lines
const bool  LINE_TINT      = true;  // the input box lines take the cursor's color (a plain color, no glow)
const float LINE_COLOR     = 0.42;  // how much of the cursor's color the lines take, at their own brightness
const bool  LINE_EFFECT    = false;  // the input box lines, always: hot wires: glowing, hot spots drifting, an ember rising now and then

// --- fine-tuning ---
// -- Typing
const float RISE        = 2.6;  // how fast embers start rising, in character heights per second
const float SPREAD      = 0.8;  // how fast they drift sideways, in character heights per second (more after a jump)
const float EMBER_SIZE  = 0.07;  // ember radius, in character heights
const float GLOW_SIZE   = 0.2;   // glow around each ember, in character heights
// -- Enter & done
const float ENTER_SPREAD = 0.25;  // seconds the heat takes to run from the cursor to the edges
// -- Delete-all
const float JUMP_EMBERS = 16.0;  // embers on a jump (at most 16)
// -- Cursor
const float IDLE_PERIOD = 2.5;  // seconds between a stray ember's chances
const float IDLE_LIFE   = 1.6;   // seconds a stray ember lives
// -- Box lines
const float LINE_STRENGTH  = 1.0;   // how strong the box lines' own look is
// -- Timing
const float SETTLE      = 0.04;  // seconds the cursor must stay put before an effect starts
const float JUMP_SETTLE = 0.12;  // the same after a jump (gives the Enter signal time to arrive)

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

// Signed distance to a box of half-size b centered at the origin (negative inside).
float sdBox(vec2 p, vec2 b) {
    vec2 q = abs(p) - b;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
}

// Light from one ember at pos; a = 0 when born, 1 when it goes out.
vec3 ember(vec2 p, vec2 pos, float r, float glow, float a) {
    vec3 hot = vec3(1.0, 0.92, 0.65), warm = vec3(1.0, 0.45, 0.1), cold = vec3(0.6, 0.06, 0.02);
    vec3 c = a < 0.5 ? mix(hot, warm, a / 0.5) : mix(warm, cold, (a - 0.5) / 0.5);
    float d = distance(p, pos);
    float core = 1.0 - smoothstep(r * 0.5, r, d);
    return sRGBToLinear(c) * (core + exp(-d / glow) * 0.8) * pow(1.0 - a, 1.2);
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

// Enter: the input box lines flare with heat, out from the cursor, and embers
// rise from all along them, drifting and cooling from yellow-white to red.
vec3 enterEffect(vec2 p, vec4 cur, vec2 center, float cellW, float cellH, vec2 res, float t, vec3 col, bool inward) {
    if (p.y < center.y - 2.5 * cellH || p.y > center.y + 5.5 * cellH) return col;
    vec2 lines = boxLines(cur, center, cellW, cellH, res);
    float share = lines.x >= 0.0 && lines.y >= 0.0 ? 0.5 : 1.0;  // embers per line
    vec3 light = vec3(0.0);
    float heat = 0.35 * max(0.0, 1.0 - t / 0.3) * step(spreadDist(p, center, res, inward), t / ENTER_SPREAD * res.x);
    float r = EMBER_SIZE * cellH, glow = GLOW_SIZE * cellH;
    float seed = mod(iTimeCursorChange * 5.17, 83.0);
    for (int k = 0; k < 2; k++) {
        float lineY = k == 0 ? lines.x : lines.y;
        if (lineY < 0.0) continue;
        light += sRGBToLinear(iCurrentCursorColor.rgb) * exp(-abs(p.y - lineY) / (0.3 * cellH)) * heat;
        for (float i = 0.0; i < 32.0; i += 1.0) {
            if (i >= ENTER_EMBERS * share) break;
            float j = i + float(k) * 32.0;
            float h1 = hash11(seed + j * 1.37), h2 = hash11(seed + j * 2.71);
            float h3 = hash11(seed + j * 4.13), h4 = hash11(seed + j * 5.89);
            float life = 0.7 + 0.5 * h4;
            vec2 start = vec2(mix(0.02, 0.98, h1) * res.x, lineY + (h2 - 0.5) * 0.2 * cellH);
            float age = t - spreadDist(start, center, res, inward) / res.x * ENTER_SPREAD - 0.1 * h3;  // they leave as the heat reaches them
            if (age < 0.0 || age > life) continue;
            vec2 pos = start + vec2(
                (h3 - 0.5) * 1.2 * cellH * age + 0.12 * cellH * sin(age * (6.0 + 6.0 * h1) + h4 * 6.28),
                RISE * (0.7 + 0.8 * h2) * cellH * age * (1.0 - 0.35 * age));
            light += ember(p, pos, r, glow, age / life);
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

// The prompt mark as a glowing ember: hot yellow and orange shifting through
// it, a flicker, and a warm glow.
vec3 markLook(vec2 p, vec3 col, float ink, float halo, vec2 m, float w, float h, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    vec2 res = iResolution.xy;
    float n = clNoise((p - m) / (0.15 * h) + vec2(0.0, t * 1.5));
    float flick = 0.85 + 0.15 * sin(t * 7.0) * sin(t * 3.3 + 1.0) * live;
    col = mix(col, mix(k, sRGBToLinear(vec3(1.0, 0.85, 0.45)), smoothstep(0.3, 0.9, n)) * flick, ink);
    return col + k * halo * (1.0 - ink) * 0.7 * flick;
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

// The box lines as hot wires: an orange glow with hotter spots drifting along,
// and now and then a stray ember rising off them.
vec3 lineLook(vec2 p, vec3 col, float lineY, float h, vec2 res, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    float dy = abs(p.y - lineY), core = exp(-dy / (0.03 * h)), soft = exp(-dy / (0.14 * h));
    float heat = clNoise(vec2(p.x / (2.0 * h) + t * 0.4, t * 0.2));
    vec3 hot = mix(k, sRGBToLinear(vec3(1.0, 0.85, 0.45)), smoothstep(0.55, 0.95, heat));
    vec3 light = hot * (core * (0.35 + 0.4 * heat) + soft * 0.18);
    for (int i = 0; i < 2; i++) {
        float ph = t / 2.6 + 0.5 * float(i), u = fract(ph);
        vec2 e = vec2(clHash(vec2(floor(ph), float(i) + 3.0)) * res.x + 0.1 * h * sin(u * 6.0), lineY + 0.19 * h * u);
        light += mix(k, sRGBToLinear(vec3(1.0, 0.8, 0.4)), 0.5) * exp(-length(p - e) / 1.3) * (1.0 - u) * live;
    }
    return col + light * LINE_STRENGTH;
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

    // Cursor rectangle in pixels (y grows upward; .xy is the top-left corner).
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w);
    vec2 hi = vec2(cur.x + cur.z, cur.y);
    float cellH = max(cur.w, cellWOf(cur) * 2.0);
    float cellW = max(cellWOf(cur), cellH * 0.4);
    vec2 center = (lo + hi) * 0.5;
    vec4 prv = upCursor(iPreviousCursor);
    float moved = abs(cur.x - prv.x) / cellW + 2.0 * abs(cur.y - prv.y) / cellH;
    // Forward along the row by up to 40 cells is typed text, never a jump: a letter, a
    // Chinese character (two cells) or a whole phrase an input method commits at once
    // (the cursor stays put while you compose it, then jumps). It plays as typing.
    bool typedText = abs(cur.y - prv.y) < 0.5 * cellH && cur.x - prv.x > 0.5 * cellW && cur.x - prv.x < 40.5 * cellW;
    float signalMoved = moved;
    if (typedText) moved = min(moved, 1.0);  // a whole phrase plays like one keystroke
    bool typeOn = TYPE_EFFECT && (PHRASE_EFFECT || !(typedText && cur.x - prv.x > 1.5 * cellW));
    float big = smoothstep(1.0, 6.0, moved);

    // Moves count once the cursor has stayed put a moment (longer after a jump, so
    // the Enter signal can arrive first): spots a program's cursor passes through
    // while it redraws never show anything. The coal keeps glowing meanwhile.
    vec2 p = fragCoord;
    float sinceMove = iTime - iTimeCursorChange - (moved >= 3.0 ? JUMP_SETTLE : SETTLE);
    // A jump while a signal is on means the same Enter only inside Claude Code's input box
    // (a box line just below the cursor: it cleared the box after the signal arrived), and
    // never forward along the row. A Tab completion, a prompt the shell redraws, a program
    // hopping the cursor elsewhere never start the effect again.
    bool back = signalMoved >= 3.0 && (oddByte(iCurrentCursorColor.b) || oddByte(iCurrentCursorColor.g))
        && !(abs(cur.y - prv.y) < 0.5 * cellH && cur.x > prv.x)
        && clLineNear(cur.y - 1.7 * cur.w, 0.7 * cur.w, res) >= 0.0;
    bool done = DONE_EFFECT && doneSignal(back);
    if (sinceMove >= 0.0 && ((ENTER_EFFECT && enterSignal(back)) || done)) {
        if (sinceMove < ENTER_TIME) fragColor.rgb = enterEffect(p, cur, center, cellW, cellH, res, sinceMove, fragColor.rgb, done);
        sinceMove = 1e4;  // no burst at the cursor for the signal itself
    }
    // Still settling, only the color changed (the Enter signal ending), the cursor
    // jumped to another row (a program redrawing: never an effect), or along its row
    // with JUMP_EFFECTS off: not a move.
    if (sinceMove < 0.0 || moved < 0.5 || (moved >= 3.0 && !(JUMP_EFFECTS && abs(cur.y - prv.y) < 0.5 * cellH))) sinceMove = 1e4;

    // Embers rise about two and a half character heights at most; nothing
    // outside this box changes.
    if (p.x < lo.x - 3.0 * cellH || p.x > hi.x + 3.0 * cellH
            || p.y < lo.y - 2.5 * cellH || p.y > hi.y + 4.0 * cellH) return;

    float r = EMBER_SIZE * cellH, glow = GLOW_SIZE * cellH;
    vec3 light = vec3(0.0);

    // Embers thrown up by the last move, from the top of the cursor.
    if ((typeOn || moved >= 3.0) && sinceMove < LIFE * 1.3) {
        float count = mix(TYPE_EMBERS, JUMP_EMBERS, big);
        float seed = mod(iTimeCursorChange * 7.31, 97.0);
        for (float i = 0.0; i < 16.0; i += 1.0) {
            if (i >= count) break;
            float h1 = hash11(seed + i * 1.37), h2 = hash11(seed + i * 2.71);
            float h3 = hash11(seed + i * 4.13), h4 = hash11(seed + i * 5.89);
            float life = LIFE * (0.6 + 0.6 * h4);
            float age = sinceMove - 0.08 * h3;  // a small stagger, so they don't all leave at once
            if (age < 0.0 || age > life) continue;
            vec2 start = vec2(mix(lo.x, hi.x, h1), hi.y - 0.25 * cur.w * h2);
            vec2 pos = start + vec2(
                (h3 - 0.5) * 2.0 * SPREAD * (1.0 + 0.6 * big) * cellH * age
                    + 0.12 * cellH * sin(age * (6.0 + 6.0 * h1) + h4 * 6.28),
                RISE * (0.6 + 0.8 * h2) * cellH * age * (1.0 - 0.35 * age));
            float twinkle = 0.75 + 0.25 * sin(iTime * 40.0 + i * 3.0);
            light += ember(p, pos, r, glow, age / life) * twinkle;
        }
    }

    // A stray ember while idle (the previous window's ember may still be in the air).
    float win = floor(iTime / IDLE_PERIOD);
    for (float k = 0.0; k < 2.0; k += 1.0) {
        float w = win - k;
        float born = w * IDLE_PERIOD + hash11(w + 0.37) * IDLE_PERIOD * 0.5;
        float age = iTime - born;
        if (!IDLE_EFFECTS || hash11(w * 1.7 + 9.1) >= IDLE_CHANCE || born < iTimeCursorChange + 1.0
                || age < 0.0 || age > IDLE_LIFE) continue;
        float h = hash11(w + 5.5);
        vec2 pos = vec2(mix(lo.x, hi.x, h), hi.y)
            + vec2(0.15 * cellH * sin(age * 4.0 + h * 6.28), RISE * 0.55 * cellH * age);
        light += ember(p, pos, r, glow, age / IDLE_LIFE);
    }

    // The coal: a warm, slowly flickering glow around the cursor while it shows.
    float dBlock = sdBox(p - center, (hi - lo) * 0.5);
    if (CURSOR_EFFECT && dBlock > 0.0 && dBlock < 2.5 * cellH && cursorDrawn(lo, cur, res)) {
        float flicker = 0.85 + 0.15 * sin(iTime * 7.0) * sin(iTime * 3.3 + 1.0);
        light += sRGBToLinear(iCurrentCursorColor.rgb) * exp(-dBlock / (0.3 * cellH)) * COAL_GLOW * flicker;
    }

    // The coal's surface: hot cracks and cooler patches shifting slowly across the block.
    if (CURSOR_EFFECT && dBlock <= 0.0 && cursorDrawn(lo, cur, res)) {
        float n = clNoise((p - lo) / (0.22 * cellH) + vec2(0.0, (IDLE_EFFECTS ? iTime : 0.0) * 0.5));
        vec3 coal = mix(sRGBToLinear(vec3(0.55, 0.12, 0.03)), sRGBToLinear(vec3(1.0, 0.85, 0.45)), smoothstep(0.25, 0.85, n));
        fragColor.rgb = mix(fragColor.rgb, mix(fragColor.rgb, coal, 0.45), clOnBlock(fragColor.rgb));
    }

    fragColor = vec4(min(fragColor.rgb + light, vec3(1.0)), fragColor.a);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 res = iResolution.xy;
    if (Y_DOWN) fragCoord.y = res.y - fragCoord.y;
    fragColor = screenAt(fragCoord / res);
    effects(fragColor, fragCoord);
    // The line color stays when the window loses focus; the moving looks don't.
    if (((LINE_EFFECT && iFocus != 0) || (LINE_TINT && LINE_COLOR > 0.0)) && iCursorVisible != 0 && iCurrentCursorStyle == CURSORSTYLE_BLOCK) fragColor.rgb = min(linesLook(fragCoord, fragColor.rgb), vec3(1.0));
    if (iCursorVisible == 0) return;
    if (PROMPT_EFFECT && iCurrentCursorStyle == CURSORSTYLE_BLOCK) fragColor.rgb = min(promptLook(fragCoord, fragColor.rgb), vec3(1.0));
}
