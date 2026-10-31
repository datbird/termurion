// Cyber cursor for Ghostty (`custom-shader`).
// Data rain. Each character you type is decrypted: it hides behind flickering
// glyphs for a moment before it shows (DECODE_STYLE: a glyph flicker, a slot
// reel, a pixel resolve or a signal lock-in; TYPE_BEAM: a scan beam burns it in);
// TYPE_PULSE sends data packets along the input box lines, WORD_DECODE
// re-decrypts a finished word when you type a space, TYPE_KICK runs a scanline
// down the cursor. Nothing falls out of the input row. Chinese (an input method):
// while you compose, Ghostty draws the pinyin itself and hides the cursor; the
// characters you pick decrypt together, a little apart, within PHRASE_TIME.
// TEXT_COLOR shows your text (Claude Code's input box and sent prompts, zsh's
// command line) in the cursor's color, as easy to read as the terminal's text.
// A jump back along the row (delete-all, Home; JUMP_EFFECTS) dissolves the row
// into rain (forward, up to 40 cells, counts as typed text and decrypts). Enter (a prompt sent in Claude Code, a command run in zsh, via
// `cursor-fx signal`) pours columns of glowing glyphs from just above Claude
// Code's input box down through it, starting at the cursor and spreading out,
// with a fainter layer falling behind for depth; the box lines power up green
// as it lands. "Done" (the answer arrived) starts the rain at the edges. The cursor itself is
// alive: glyph rain scrolls down inside the block, a scanline sweeps it now and
// then, and a soft green glow around it breathes; it stays one character wide.
// Crisp glyphs, no flashes. Only the area around the box and the cursor changes.
// The prompt mark (❯) on its line glows green and decrypts now and then.
// Settings are grouped by `// -- Name` lines (the panel shows them as headings).
// The input box's lines are data buses with faint dashes streaming along them
// (LINE_EFFECT); LINE_COLOR tints them in the cursor's color, no glow.

// macOS: true. On Linux, change it to false (install.sh does this for you);
// otherwise the effect plays upside down.
#define Y_DOWN true

// --- settings: the ones worth a tweak (edit and save, or use `cursor-fx edit`; Ghostty reloads on Command+Shift+,) ---
// -- Typing
const bool  TYPE_EFFECT    = true;  // each character you type decrypts (Chinese you pick decrypts as a whole)
const bool  PHRASE_EFFECT  = true;  // picking a character or phrase from your input method plays the typing effect too (like one keystroke)
const int   DECODE_STYLE   = 0;  // how each letter decrypts: 0 glyph flicker, 1 slot reel, 2 pixel resolve, 3 signal lock-in
const bool  WORD_DECODE    = false;  // typing a space: the word you just finished re-decrypts, left to right
const bool  TYPE_PULSE     = false;  // each key sends two data packets along the input box lines, away from the cursor
const bool  TYPE_BEAM      = false;  // a scan beam sweeps the decrypted letter and burns it in (glyph flicker style)
const bool  TYPE_KICK      = true;  // a scanline runs down the cursor with a pulse of glow
// -- Enter & done
const bool  ENTER_EFFECT   = true; // rain through the input box on Enter (`cursor-fx enter off` pauses every effect's Enter)
const bool  DONE_EFFECT    = true;  // rain again, starting at the edges, when Claude finishes answering (Stop hook) or a long zsh command ends
const float ENTER_TIME     = 1.5;   // seconds the rain lasts
// -- Delete-all
const bool  JUMP_EFFECTS   = true;  // the row dissolves into rain when the cursor jumps along it (`cursor-fx jumps on|off`)
// -- Cursor
const bool  CURSOR_EFFECT  = true;  // the cursor's own look: glyph rain inside the block, a sweeping scanline, a soft glow
const bool  IDLE_EFFECTS   = true;  // the cursor's motion while idle (`cursor-fx idle on|off`; off keeps the screen calm)
const float CURSOR_GLOW    = 0.6;   // strength of the glow around the cursor
// -- Prompt mark
const bool  PROMPT_EFFECT  = true;  // the prompt mark (❯) at the start of the cursor's line: netrunner green, decrypting into a glyph now and then
// -- Box lines
const bool  LINE_TINT      = true;  // the input box lines take the cursor's color (a plain color, no glow)
const float LINE_COLOR     = 0.42;  // how much of the cursor's color the lines take, at their own brightness
const bool  LINE_EFFECT    = false;  // the input box lines, always: data buses: faint dashes streaming along
// -- Your text
const bool  TEXT_COLOR     = true;  // your text in the cursor's color (Claude Code's input box and your sent prompts, zsh's command line), as easy to read as the terminal's text; dim and colored text stay as they are
// -- Look
const vec3  RAIN_COLOR     = vec3(0.35, 1.0, 0.55);  // the glyphs near the head of each rain column
const vec3  TRAIL_COLOR    = vec3(0.0, 0.55, 0.45);  // the color the rain fades to toward the end of its trail
const float BRIGHTNESS     = 1.0;   // overall strength of the light

// --- fine-tuning ---
// -- Typing
const float DECODE_TIME    = 0.22;  // seconds the character you typed stays hidden behind glyphs
const float TYPE_TIME      = 0.45;  // seconds the typing light lasts (packets, word wave)
const float PULSE_REACH    = 7.0;  // how far the packets travel, in characters
const float WORD_WAVE      = 0.035;  // word decrypt: seconds from one letter to the next
const float PHRASE_TIME    = 0.16;  // Chinese input: seconds a character or phrase you pick takes to decrypt, all of it (quicker than a letter's, so you can read along)
// -- Enter & done
const float ENTER_SPREAD   = 0.35;  // seconds the rain's start takes to spread from the cursor to the edges
const float RAIN_SPEED     = 6.0;   // how fast a column falls, in character heights per second
const float TRAIL          = 6.0;   // glyphs in a column's trail
const float DENSITY        = 0.5;   // share of the columns that rain
const float DEPTH          = 0.0;   // brightness of the smaller, slower rain layer behind (0 = none)
const float GLYPH_SIZE     = 0.62;  // glyph height, in character heights
const float RAIN_HEIGHT    = 0.0;   // the rain starts this far above the top box line, in character heights
// -- Delete-all
const float DISSOLVE_TIME  = 0.7;  // seconds the row's rain lasts
// -- Cursor
const float GLOW_SIZE      = 0.10;  // how far the soft outer part of the cursor's glow reaches, in character heights
const float GLOW_CORE      = 0.7;  // how much of the cursor's glow hugs the block (the rest spreads out)
const float CURSOR_RAIN    = 1.2;  // glyph rows scrolling down inside the cursor per second
const float CURSOR_SHADE   = 0.3;  // how much darker than the block those glyphs are
const float SCAN_PERIOD    = 2.6;  // seconds between scanlines sweeping down the cursor
const float BREATH_PERIOD  = 3.0;  // seconds per breath of the cursor's glow
// -- Box lines
const float LINE_STRENGTH  = 1.0;   // how strong the box lines' own look is
// -- Your text
const float TEXT_TINT      = 1.0;  // how much of the cursor's color your text takes
// -- Timing
const float SETTLE         = 0.04;  // seconds the cursor must stay put before an effect starts
const float JUMP_SETTLE    = 0.12;  // the same after a jump (gives the Enter signal time to arrive)

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

// The 14 strokes of a segment display (x 0..1 left to right, y 0..1 top to
// bottom): edges, middle bar halves, center stem halves, and the diagonals.
const vec4 STROKES[14] = vec4[14](
    vec4(0.0, 0.0, 1.0, 0.0), vec4(0.0, 1.0, 1.0, 1.0), vec4(0.0, 0.5, 0.5, 0.5), vec4(0.5, 0.5, 1.0, 0.5),
    vec4(0.0, 0.0, 0.0, 0.5), vec4(0.0, 0.5, 0.0, 1.0), vec4(1.0, 0.0, 1.0, 0.5), vec4(1.0, 0.5, 1.0, 1.0),
    vec4(0.5, 0.0, 0.5, 0.5), vec4(0.5, 0.5, 0.5, 1.0), vec4(0.0, 0.0, 0.5, 0.5), vec4(1.0, 0.0, 0.5, 0.5),
    vec4(0.0, 1.0, 0.5, 0.5), vec4(1.0, 1.0, 0.5, 0.5));

// A made-up character: some of the 14 strokes, chosen by `id`. Returns the
// distance in pixels from q (pixels from the glyph's top-left) to its nearest
// stroke, for a glyph `size` pixels big.
float glyphDist(float id, vec2 q, vec2 size) {
    float d = 1e4;
    for (int i = 0; i < 14; i++) {
        if (hash21(vec2(id * 1.37 + 0.5, float(i))) > 0.36) continue;
        vec4 s = STROKES[i];
        d = min(d, sdSegment(q, s.xy * size, s.zw * size));
    }
    return d;
}

// How far a rain column's head has fallen, in pixels below the top, at time t;
// -1 when the column doesn't rain. Columns start once the spread reaches
// their center: out from originX at `spread` pixels per second, or in from
// the window's edges with `inward`.
float columnHead(float col, float gw, float xOff, float t, float originX, float spread, bool inward,
                 float speed, float density, float salt, vec2 res) {
    if (hash11(col * 1.31 + salt) > density) return -1.0;
    float colX = (col + 0.5) * gw + xOff;
    float dist = inward ? min(colX, res.x - colX) : abs(colX - originX);
    float start = dist / spread + 0.15 * hash11(col * 2.7 + salt);
    return max(t - start, 0.0) * speed * (0.8 + 0.5 * hash11(col * 5.3 + salt));
}

// Falling columns of glyphs between yTop and yBot, over x0..x1 (pixels; glyph
// rows gh tall, columns 0.62 gh wide from xOff). Each column's head glyph is
// bright and nearly white with a soft bloom around it, a faint streak of light
// runs down the column behind it, its trail fades from c1 to c2, every glyph
// has a soft glow and changes now and then; the rain fades in over its first
// glyph row and out over its last.
vec3 rain(vec2 p, float yTop, float yBot, float x0, float x1, float xOff, float gh, float t, float originX,
          float spread, bool inward, float speed, float trail, float density, float salt, vec2 res, vec3 c1, vec3 c2) {
    if (p.y > yTop || p.y < yBot || p.x < x0 || p.x > x1) return vec3(0.0);
    float gw = 0.62 * gh;
    float col = floor((p.x - xOff) / gw);
    float head = columnHead(col, gw, xOff, t, originX, spread, inward, speed, density, salt, res);
    if (head <= 0.0) return vec3(0.0);
    float down = yTop - p.y;
    float edges = (1.0 - smoothstep(yTop - gh, yTop, p.y)) * smoothstep(yBot, yBot + gh, p.y);
    float dx = abs(p.x - ((col + 0.5) * gw + xOff));
    float toHead = head - down;  // pixels behind the head along the column (negative: below it)
    vec3 light = mix(c1, vec3(1.0), 0.3) * exp(-pow(dx / (0.3 * gw), 2.0)) * exp(-abs(toHead) / (0.5 * gh)) * 0.45;
    float row = floor(down / gh);
    float behind = head - (row + 0.5) * gh;  // pixels behind the head, per glyph
    if (behind >= 0.0 && behind <= trail * gh) {
        float cellSeed = hash21(vec2(col + salt, row));
        float id = floor(cellSeed * 97.0) + floor(t * (3.0 + 6.0 * cellSeed));
        vec2 q = vec2(p.x - xOff - col * gw - 0.2 * gw, down - row * gh - 0.15 * gh);
        float d = glyphDist(id, q, vec2(0.6 * gw, 0.7 * gh));
        float sw = max(1.2, 0.075 * gh);
        float ink = 1.0 - smoothstep(0.5 * sw - 0.5, 0.5 * sw + 0.5, d);
        float glow = exp(-d / (0.09 * gh));
        float k = behind / (trail * gh);
        light += behind < gh ? mix(c1, vec3(1.0), 0.55) * ink * 1.3 + c1 * glow * 0.6
                             : mix(c1, c2, smoothstep(0.05, 0.85, k)) * (ink * 0.95 + glow * 0.4) * (1.0 - k);
        light += c1 * exp(-pow(dx / (0.15 * gw), 2.0)) * (1.0 - k) * (1.0 - k) * 0.25;
    }
    return light * edges * (1.0 - 0.85 * smoothstep(0.06, 0.2, colorDist(screenAt(p / res).rgb, iBackgroundColor)));
}

float cursorGlyphInk(float id, vec2 q, vec2 size, float sw);  // further down, with the cursor's look

// Rain on the terminal's own grid: a column per character cell (DENSITY of them rain), glyph
// rows gh pixels apart from yTop down to yBot, only whole rows. A column starts when the
// spread reaches it (out from originX, or in from the window's edges), then falls a row at a
// time: its head is a near-white glyph with a small glow, its trail of TRAIL glyphs fades
// from c1 to c2, each glyph drawn with whole-pixel strokes and changing now and then. Over
// text it stays faint, so the text reads through.
vec3 gridRain(vec2 p, float yTop, float yBot, float x0, float cw, float gh, float t, float originX, float spread,
              bool inward, float speed, float density, float salt, vec2 res, vec3 c1, vec3 c2) {
    float n = floor((yTop - yBot) / gh);                       // whole rows that fit
    if (n < 1.0) return vec3(0.0);
    float y0 = yTop - floor(0.5 * (yTop - yBot - n * gh));     // the grid's top, centered between the lines
    float down = y0 - p.y;
    if (down < 0.0 || down >= n * gh) return vec3(0.0);
    float col = floor((p.x - x0) / cw);
    if (hash11(col * 1.31 + salt) > density) return vec3(0.0);
    float colX = x0 + (col + 0.5) * cw;
    float dist = inward ? min(colX, res.x - colX) : abs(colX - originX);
    float start = dist / spread + 0.1 * hash11(col * 2.7 + salt);
    float v = speed * (0.85 + 0.35 * hash11(col * 5.3 + salt));
    // Up to three drops per column, one after another, for most of the Enter time: for this
    // row, the nearest head above it (or on it).
    float row = floor(down / gh), behind = 1e4;
    for (int k = 0; k < 3; k++) {
        float st = start + float(k) * (0.3 + 0.25 * hash11(col * 7.1 + float(k) * 3.3 + salt));
        if (k > 0 && st > 0.55 * ENTER_TIME) break;
        float fallen = (t - st) * v;
        if (fallen <= 0.0) continue;
        float b = floor(fallen / gh) - row;
        if (b >= 0.0 && b < behind) behind = b;
    }
    if (behind > TRAIL) return vec3(0.0);
    float sw = max(1.0, floor(0.065 * gh + 0.5));
    vec2 size = vec2(max(2.0, 2.0 * floor(0.33 * cw)), max(2.0, 2.0 * floor(0.33 * gh)));
    float snap = mod(sw, 2.0) > 0.5 ? 0.5 : 0.0;
    vec2 q = vec2(p.x - (floor(x0 + col * cw + 0.5 * (cw - size.x)) + snap),
                  down - row * gh - (floor(0.5 * (gh - size.y)) + snap));
    float seed = hash21(vec2(col + salt, row));
    float id = floor(seed * 97.0) + floor(t * (behind < 0.5 ? 14.0 : 1.5 + 4.0 * seed));
    float ink = cursorGlyphInk(id, q, size, sw);
    float k = behind / TRAIL;
    vec3 c = behind < 0.5 ? mix(c1, vec3(1.0), 0.6) : mix(c1, c2, smoothstep(0.0, 0.9, k));
    float level = behind < 0.5 ? 1.2 : 0.95 * (1.0 - k) * (1.0 - k) + 0.04;
    vec3 light = c * ink * level;
    // The head glows along its own strokes (the glyph sampled around this pixel), not as a box.
    if (behind < 0.5) {
        float g = 0.0;
        for (int i = 0; i < 8; i++) {
            float a = float(i) * 0.7854;
            g += cursorGlyphInk(id, q + max(1.5, 0.08 * gh) * vec2(cos(a), sin(a)), size, sw);
        }
        light += c1 * (g / 8.0) * (1.0 - ink) * 0.55;
    }
    float text = smoothstep(0.06, 0.2, colorDist(screenAt(p / res).rgb, iBackgroundColor));
    return light * (1.0 - 0.8 * text);
}

// Enter: rain falls through the input box, a column per character cell, starting at the
// cursor and spreading out to the edges; an energy sweep runs out along both box lines ahead
// of it and leaves them lit, and each column throws a small spark where it lands on the
// bottom line. Done: the same from both edges inward; the sweeps meet in the middle and flare.
// A shell (no box): the rain falls in the rows under the line that was sent.
vec3 enterEffect(vec2 p, vec4 cur, vec2 center, float cellW, float cellH, vec2 res, float t, bool inward, vec3 c1, vec3 c2) {
    if (p.y - center.y > (4.0 + RAIN_HEIGHT) * cellH || center.y - p.y > 2.6 * cellH) return vec3(0.0);
    vec2 lines = boxLines(cur, center, cellW, cellH, res);
    float top = lines.x >= 0.0 ? lines.x : lines.y + cellH;
    float bot = lines.y >= 0.0 ? lines.y : lines.x - 2.0 * cellH;
    bool shell = lines.x < 0.0 && boxLineY(center - vec2(0.0, cellH), center, cur, res, 0.35 * cellH) < 0.0;
    float yTop = top - 3.0 + RAIN_HEIGHT * cellH, yBot = bot + 3.0;
    if (shell) { yTop = lines.y - 2.0; yBot = lines.y - 1.8 * cellH; }
    float cw = cellWOf(cur), x0 = mod(cur.x, cw);
    float gh = max(6.0, floor(GLYPH_SIZE * cur.w + 0.5));
    float speed = RAIN_SPEED * cellH, spread = res.x / ENTER_SPREAD;
    vec3 light = gridRain(p, yTop, yBot, x0, cw, gh, t, center.x, spread, inward, speed, DENSITY, 0.0, res, c1, c2);
    if (DEPTH > 0.0)  // a fainter, slower layer between the columns
        light += DEPTH * gridRain(p, yTop, yBot, x0 + 0.5 * cw, cw, floor(0.8 * gh), t - 0.08, center.x, spread, inward,
                                  0.6 * speed, 0.45, 5.0, res, mix(c1, c2, 0.5), c2);
    if (shell) return light * (1.0 - smoothstep(0.8, 1.0, t / ENTER_TIME)) * BRIGHTNESS;
    // The sweep along the lines: a bright front, the line lit behind it and fading.
    float mid = 0.5 * res.x;
    for (int k = 0; k < 2; k++) {
        float lineY = k == 0 ? lines.x : lines.y;
        float dy = abs(p.y - lineY);
        if (lineY < 0.0 || dy > 0.45 * cellH) continue;
        float d = inward ? mid - abs(p.x - mid) : abs(p.x - center.x);   // how far the sweep has to go to here
        float since = t - d / spread;
        if (since < 0.0) continue;
        float profile = exp(-dy / (0.03 * cellH)) + 0.22 * exp(-dy / (0.14 * cellH));
        float lit = exp(-since / (0.45 * ENTER_TIME)) * 0.55;
        float front = exp(-pow(since * spread / (1.6 * cw), 2.0)) * 1.1;
        light += mix(c1, vec3(1.0), 0.35 * front) * profile * (lit + front);
        // Done: where the two sweeps meet, a flare.
        if (inward) {
            float meet = t - mid / spread;
            light += mix(c1, vec3(1.0), 0.4) * profile * exp(-pow((p.x - mid) / (4.0 * cw), 2.0))
                   * exp(-pow(meet / 0.12, 2.0)) * 1.2;
        }
    }
    // A spark where each column lands on the bottom line.
    if (lines.y >= 0.0 && abs(p.y - lines.y) < 0.45 * cellH) {
        float col = floor((p.x - x0) / cw);
        if (hash11(col * 1.31) <= DENSITY) {
            float colX = x0 + (col + 0.5) * cw;
            float dist = inward ? min(colX, res.x - colX) : abs(colX - center.x);
            float start = dist / spread + 0.1 * hash11(col * 2.7);
            float fallen = (t - start) * speed * (0.85 + 0.35 * hash11(col * 5.3));
            float n = floor((yTop - yBot) / gh);
            float landed = (fallen - n * gh) / speed;                  // seconds since it landed
            if (landed > 0.0 && landed < 0.3) {
                float dx = abs(p.x - colX), dy = abs(p.y - lines.y);
                light += mix(c1, vec3(1.0), 0.5) * exp(-pow(dx / (0.9 * cw), 2.0)) * exp(-dy / (0.05 * cellH))
                       * (1.0 - landed / 0.3) * 0.9;
            }
        }
    }
    return light * (1.0 - smoothstep(0.8, 1.0, t / ENTER_TIME)) * BRIGHTNESS;
}

// A made-up character for the cursor's inside: mostly straight strokes (diagonals smear
// at a cursor's size), drawn with a one-pixel antialiased edge. Returns its ink, 0..1.
float cursorGlyphInk(float id, vec2 q, vec2 size, float sw) {
    float d = 1e4;
    for (int i = 0; i < 14; i++) {
        if (hash21(vec2(id * 1.37 + 0.5, float(i))) > (i >= 10 ? 0.1 : 0.45)) continue;
        vec4 s = STROKES[i];
        d = min(d, sdSegment(q, s.xy * size, s.zw * size));
    }
    return 1.0 - smoothstep(0.5 * sw - 0.5, 0.5 * sw + 0.5, d);
}

// TYPE_KICK: seconds since a keystroke (after SETTLE), or -1 when there was none just now.
float typingKick(vec4 cur, float cellH) {
    if (!TYPE_KICK || !TYPE_EFFECT) return -1.0;
    vec4 prv = upCursor(iPreviousCursor);
    float cellW = max(cur.z, cellH * 0.4);
    float moved = abs(cur.x - prv.x) / cellW + 2.0 * abs(cur.y - prv.y) / cellH;
    float s = iTime - iTimeCursorChange - SETTLE;
    return (moved >= 0.5 && moved < 3.0 && s >= 0.0 && s < 0.2) ? s : -1.0;
}

// The cursor's own look: glyph rain scrolling down inside the block, drawn a
// little darker than the block and only on its pixels (a letter under the
// cursor stays as it is), a scanline sweeping down it now and then, and a soft
// glow around it that breathes. Still while IDLE_EFFECTS is off.
vec3 cursorLook(vec2 p, vec3 col, vec2 res, vec3 c1) {
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w), hi = vec2(cur.x + cur.z, cur.y);
    vec2 center = 0.5 * (lo + hi);
    float cellH = max(cur.w, cellWOf(cur) * 2.0);
    vec2 q = abs(p - center) - 0.5 * (hi - lo);
    if (max(q.x, q.y) > 0.6 * cellH) return col;
    float t = IDLE_EFFECTS ? iTime : 0.0;
    // No block drawn (blinked off, or Chinese input being composed: Ghostty then draws
    // the composing text there instead of the cursor): nothing to restyle.
    if (!cursorDrawn(lo, cur, res)) return col;
    if (max(q.x, q.y) > 0.0) {
        float kick = typingKick(cur, cellH);
        float breath = (0.75 + 0.25 * sin(t * 6.2832 / BREATH_PERIOD)) * (kick >= 0.0 ? 1.0 + 1.3 * exp(-kick / 0.08) : 1.0);
        // Two soft layers, like a bloom: a tight, slightly whiter glow hugging the block (no
        // hard rim) and a wide, faint one; a little noise keeps the dark gradient from banding.
        float e = length(max(q, 0.0));
        float core = exp(-0.5 * pow(e / (0.035 * cellH), 2.0)), bloom = exp(-0.5 * pow(e / (GLOW_SIZE * cellH), 2.0));
        vec3 glow = mix(c1, vec3(1.0), 0.25) * core * GLOW_CORE + c1 * bloom * (1.0 - GLOW_CORE);
        return col + glow * CURSOR_GLOW * breath * BRIGHTNESS + (hash21(p + fract(iTime) * 61.0) - 0.5) / 255.0;
    }
    // Only the block's own pixels: closer to the cursor color than to the background
    // or the letter color (Ghostty's block isn't exactly the uniform's color).
    float onBlock = smoothstep(0.0, 0.05, min(colorDist(col, iBackgroundColor), colorDist(col, iCursorText))
                                        - colorDist(col, iCurrentCursorColor.rgb));
    if (onBlock <= 0.0) return col;
    // Pixel-sharp at a cursor's small size: whole-pixel rows, glyph box and strokes (odd
    // strokes on pixel centers, even ones between pixels), scrolling in whole pixels.
    float gh = floor(0.42 * cur.w + 0.5);
    float v = (hi.y - p.y) - floor(t * CURSOR_RAIN * gh);
    float row = floor(v / gh);
    float seed = hash11(row * 3.7 + floor(cur.x / max(cur.z, 1.0)) * 1.3);
    float id = floor(seed * 97.0) + floor(t * 1.5 + seed * 5.0);
    float sw = max(1.0, floor(0.045 * cur.w + 0.5));
    vec2 size = vec2(max(2.0, 2.0 * floor(0.3 * cur.z)), max(2.0, 2.0 * floor(0.35 * gh)));
    float snap = mod(sw, 2.0) > 0.5 ? 0.5 : 0.0;
    vec2 gq = vec2(p.x - (floor(lo.x + 0.5 * (cur.z - size.x)) + snap), v - row * gh - (floor(0.15 * gh) + snap));
    float ink = cursorGlyphInk(id, gq, size, sw);
    float scanY = hi.y - fract(t / SCAN_PERIOD) * 2.0 * cur.w;  // down the block in the first half of each period
    float scan = IDLE_EFFECTS ? exp(-abs(p.y - scanY) / (0.03 * cellH)) * 0.5 : 0.0;
    float kick = typingKick(cur, cellH);
    if (kick >= 0.0) scan += exp(-abs(p.y - (hi.y - kick / 0.14 * cur.w)) / (0.04 * cellH)) * 1.1 * (1.0 - smoothstep(0.1, 0.14, kick));
    vec3 look = col * (1.0 - CURSOR_SHADE * ink) + mix(c1, vec3(1.0), 0.5) * scan * BRIGHTNESS;
    return mix(col, look, onBlock);
}

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

// The prompt mark's cell (the ❯ of Claude Code and of the shell): the first character
// on the cursor's row, when a blank cell follows it and both sit left of the cursor.
// Cells line up with the cursor's, so only the first few cells from the window's left
// edge are checked. Returns the cell's left edge, or -1. Callers then check its shape
// with clChevron(), only for the pixels that need it (it reads many pixels).
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

// The prompt mark in netrunner green with a soft glow; every few seconds it
// decrypts: for a moment it hides behind a glowing glyph, then shows again.
vec3 markLook(vec2 p, vec3 col, float ink, float halo, vec2 m, float w, float h, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    vec2 res = iResolution.xy;
    float u = fract(t / 3.2);
    if (live > 0.0 && u < 0.08) {
        vec3 bg = screenAt((m + vec2(1.5, -1.5)) / res).rgb;
        vec2 size = vec2(0.6 * w, 0.6 * h);
        float d = glyphDist(floor(t * 30.0) + floor(t / 3.2) * 7.0, vec2(p.x - m.x - 0.2 * w, m.y - 0.2 * h - p.y), size);
        float g = 1.0 - smoothstep(0.5, 1.6, d);
        col = mix(col, bg, ink);
        return col + k * (g * 1.1 + exp(-d / (0.06 * h)) * 0.3);
    }
    col = mix(col, mix(k, vec3(1.0), 0.15), ink);
    return col + k * halo * (1.0 - ink) * 0.5;
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

// The box lines as data buses: faint green dashes streaming along them.
vec3 lineLook(vec2 p, vec3 col, float lineY, float h, vec2 res, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    float dy = abs(p.y - lineY), core = exp(-dy / (0.03 * h)), soft = exp(-dy / (0.14 * h));
    float s = (p.x - t * 2.0 * h) / (0.9 * h);
    float dash = smoothstep(3.0, 4.0, fract(s) * 0.9 * h) * (0.4 + 0.6 * clHash(vec2(floor(s), 3.0)));
    return col + k * (core * 0.4 * dash + soft * 0.12) * LINE_STRENGTH;
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
    // With only the line color (no LINE_EFFECT light), background pixels have nothing to do.
    if (!LINE_EFFECT && colorDist(screenAt(p / res).rgb, iBackgroundColor) < 0.02) return col;
    float lineY = clLineNear(p.y, 0.2 * h, res);
    if (lineY < 0.0) return col;
    // Only inside an input box: there is a box line just below the cursor too (a
    // shell, or a program with a long rule somewhere, stays plain).
    if (clLineNear(cur.y - 1.7 * h, 0.7 * h, res) < 0.0) return col;
    if (LINE_TINT && LINE_COLOR > 0.0) col = lineTint(p, col, lineY, res);
    if (!LINE_EFFECT) return col;
    float t = IDLE_EFFECTS ? iTime : 0.0, live = IDLE_EFFECTS ? 1.0 : 0.0;
    return lineLook(p, col, lineY, h, res, t, live);
}

// --- The typed text's color (TEXT_COLOR) ---

vec3 linearToSRGB(vec3 c) {
    return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), c));
}

// Relative luminance of a linear color.
float luma(vec3 c) {
    return dot(c, vec3(0.2126, 0.7152, 0.0722));
}

// A line across the whole window at height y (Claude Code's input box border): one
// color, not the background, at twelve points along it, and background four pixels
// above and below. A long row of text can have ink at a few points along one pixel
// row, but never the same color all the way across.
bool fullLine(float y, vec2 res) {
    vec3 first = screenAt(vec2(0.05 * res.x, y) / res).rgb;
    if (colorDist(first, iBackgroundColor) < 0.03) return false;
    for (int i = 1; i < 12; i++)
        if (distance(screenAt(vec2((0.05 + 0.08 * float(i)) * res.x, y) / res).rgb, first) > 0.03) return false;
    return colorDist(screenAt(vec2(0.45 * res.x, y + 4.0) / res).rgb, iBackgroundColor) < 0.03
        && colorDist(screenAt(vec2(0.45 * res.x, y - 4.0) / res).rgb, iBackgroundColor) < 0.03;
}

// Such a line between heights y0 and y1, searched from y0 upward (or from y1 downward
// with `down`), so the edge nearer the cursor is found: one pixel read per step, near
// the window's right edge where text rarely reaches, in steps finer than the line is
// thick; the whole line is checked only where that pixel isn't background. Its
// height, or -1.
float lineIn(float y0, float y1, bool down, float h, vec2 res) {
    float st = max(1.0, floor(h / 19.0)), x = 0.97 * res.x;
    for (int i = 0; i < 80; i++) {
        float at = down ? y1 - float(i) * st : y0 + float(i) * st;
        if (at < y0 || at > y1) break;  // compared before rounding: a rounded first step can fall just outside
        float y = floor(at) + 0.5;
        if (colorDist(screenAt(vec2(x, y) / res).rgb, iBackgroundColor) < 0.03) continue;
        if (fullLine(y, res)) return y;
    }
    return -1.0;
}

// Claude Code's input box around the cursor: its top and bottom lines (-1 when there
// is no box, as in a shell). The line on one side is looked for one row away, then
// two, then three (the cursor inside a prompt of many rows); from it the row height
// is known, so the other line is looked for a whole number of rows away on the other
// side. Rows can be taller than the cursor (adjust-cell-height), so the windows
// allow rows of 0.98 to 1.47 cursor heights, and never two row counts at once.
vec2 inputBox(vec4 cur, vec2 res) {
    float h = cur.w, c = cur.y - 0.5 * h;
    float top = lineIn(c + 0.85 * h, c + 1.75 * h, false, h, res), bot = -1.0, rows = 1.0;
    if (top < 0.0) bot = lineIn(c - 1.75 * h, c - 0.85 * h, true, h, res);
    if (top < 0.0 && bot < 0.0) {
        // A shell prompt (a mark starting the cursor's row, no line beside it): no box,
        // and no need for the longer searches.
        if (clMarkLeft(cur, res) >= 0.0) return vec2(-1.0);
        for (int k = 2; k <= 3; k++) {
            rows = float(k);
            vec2 w = k == 2 ? vec2(1.85, 2.95) * h : vec2(2.95, 3.9) * h;
            top = lineIn(c + w.x, c + w.y, false, h, res);
            if (top < 0.0) bot = lineIn(c - w.y, c - w.x, true, h, res);
            if (top >= 0.0 || bot >= 0.0) break;
        }
        if (top < 0.0 && bot < 0.0) return vec2(-1.0);
    }
    float pitch = (top >= 0.0 ? top - c : c - bot) / rows;
    for (int k = 1; k <= 8; k++) {
        float at = float(k) * pitch;
        if (top < 0.0) top = lineIn(c + at - 0.3 * h, c + at + 0.3 * h, false, h, res);
        else if (bot < 0.0) bot = lineIn(c - at - 0.3 * h, c - at + 0.3 * h, true, h, res);
        else break;
    }
    return top >= 0.0 && bot >= 0.0 ? vec2(top, bot) : vec2(-1.0);
}

// How far color c sits along the way from background `back` to text color `ink`, 0..1.
float inkAmount(vec3 c, vec3 back, vec3 ink) {
    vec3 d = ink - back;
    return clamp(dot(c - back, d) / max(dot(d, d), 1e-6), 0.0, 1.0);
}

// The text you type and the prompts you sent, in the cursor's color: inside Claude
// Code's input box (between its two lines across the window), on the grey bars
// Claude Code draws your sent prompts on, and in a shell on the cursor's row right of
// the prompt mark (❯). Only text drawn at full strength in the terminal's text color
// or in white (Claude Code's color for your text) changes; dim text (a placeholder,
// zsh's grey suggestion) and colored text (syntax highlighting, links) stay. Its
// contrast against the background behind it matches the terminal's own text.
vec3 textTint(vec2 p, vec3 col, vec2 res) {
    vec3 s = screenAt(p / res).rgb;
    if (colorDist(s, iBackgroundColor) < 0.03) return col;
    // Worked out in sRGB, whatever the screen texture holds (told by the window's
    // corner, which is background).
    vec3 corner = screenAt(vec2(1.5) / res).rgb;
    bool linear = distance(corner, sRGBToLinear(iBackgroundColor)) < distance(corner, iBackgroundColor);
    vec3 bg = iBackgroundColor;
    // A sent prompt's bar: the darkest of six points along this row (spaces and the
    // row's end show the bar; letters don't) is exactly the background Claude Code's
    // dark theme gives your own messages: rgb(55, 55, 55) for a prompt, rgb(65, 60, 65)
    // for a `!` shell command. Other greys stay out: a message you click open gets
    // rgb(70, 70, 70) behind Claude's output too.
    vec3 back = vec3(1.0);
    for (int i = 0; i < 6; i++) {
        vec3 b = screenAt(vec2((0.12 + 0.165 * float(i)) * res.x, p.y) / res).rgb;
        b = linear ? linearToSRGB(b) : b;
        if (dot(b, vec3(1.0)) < dot(back, vec3(1.0))) back = b;
    }
    bool bar = all(lessThan(abs(back - vec3(55.0) / 255.0), vec3(4.0 / 255.0)))
            || (all(lessThan(abs(back - vec3(65.0, 60.0, 65.0) / 255.0), vec3(4.0 / 255.0)))
                && min(back.r, back.b) - back.g > 2.5 / 255.0);  // that one's faint pink, not a plain grey
    if (!bar) back = bg;
    vec4 cur = upCursor(iCurrentCursor);
    float h = cur.w, c = cur.y - 0.5 * h;
    if (!bar && (iCursorVisible == 0 || abs(p.y - c) > 10.0 * h)) return col;
    // Which text color it is (the terminal's or white), how much of it this pixel has,
    // and whether the glyph reaches it at full strength within a stroke's width.
    vec3 cs = linear ? linearToSRGB(col) : col, ss = linear ? linearToSRGB(s) : s;
    vec3 fg = iForegroundColor;
    float df = distance(cs, back + inkAmount(cs, back, fg) * (fg - back));
    float dw = distance(cs, back + inkAmount(cs, back, vec3(1.0)) * (vec3(1.0) - back));
    vec3 ink = df <= dw ? fg : vec3(1.0);
    float a = inkAmount(cs, back, ink);
    float gate = 1.0 - smoothstep(0.04, 0.09, min(df, dw));
    if (a <= 0.0 || gate <= 0.0) return col;
    float r = max(1.0, 0.04 * h), core = inkAmount(ss, back, ink);
    for (int i = 0; i < 4; i++) {
        vec3 n = screenAt((p + r * vec2(i % 2 == 0 ? -1.0 : 1.0, i < 2 ? -1.0 : 1.0)) / res).rgb;
        core = max(core, inkAmount(linear ? linearToSRGB(n) : n, back, ink));
    }
    gate *= smoothstep(0.55, 0.8, core);
    if (gate <= 0.0) return col;
    if (!bar) {
        vec2 box = inputBox(cur, res);
        if (box.x >= 0.0) {
            if (p.y > box.x - 2.0 || p.y < box.y + 2.0) return col;
            float w = cellWOf(cur);
            if (p.x < mod(cur.x, w) + 7.0 * w) {
                float x0 = clMarkLeft(cur, res);
                vec2 m = x0 >= 0.0 ? vec2(x0, cur.y) : clMarkAbove(cur, res);
                if (m.x >= 0.0 && p.x >= m.x - 1.0 && p.x <= m.x + w + 1.0 && p.y <= m.y + 1.0 && p.y >= m.y - h - 1.0
                        && clChevron(m.x, w, vec4(cur.x, m.y, cur.zw), res)) return col;
            }
        } else {
            // A shell: the command you're typing, after the prompt mark.
            if (p.y < cur.y - 1.1 * h || p.y > cur.y + 0.1 * h) return col;
            float m = clMarkLeft(cur, res), w = cellWOf(cur);
            if (m < 0.0 || p.x < m + 1.5 * w || !clChevron(m, w, cur, res)) return col;
        }
    }
    // The cursor's color at the brightness that gives the terminal text's contrast on
    // this background (the bar is lighter, so the green is too there).
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    float lb = luma(sRGBToLinear(bg)), want = (luma(sRGBToLinear(fg)) + 0.05) / (lb + 0.05) * (luma(sRGBToLinear(back)) + 0.05) - 0.05;
    vec3 green = linearToSRGB(min(k * want / max(luma(k), 1e-3), vec3(1.0)));
    cs += a * gate * TEXT_TINT * (green - ink);
    return linear ? sRGBToLinear(max(cs, 0.0)) : cs;
}

// Is there ink in the cell (w wide) whose left edge is at x (cursor row lo.y..hi.y)?
// A grid of 3 x 6 points, so thin letters (an "o", an "l") count too.
bool cellInk(float x, float w, vec2 lo, vec4 cur, vec2 res) {
    for (int i = 0; i < 18; i++) {
        vec2 at = vec2(x + w * (0.25 + 0.25 * float(i % 3)), lo.y + cur.w * (0.18 + 0.13 * float(i / 3)));
        if (colorDist(screenAt(at / res).rgb, iBackgroundColor) > 0.08) return true;
    }
    return false;
}

// A decrypting glyph over the cell (w wide: one cell, or two for a Chinese character)
// whose left edge is at x: flickers through made-up characters, `seed` keeps
// neighbouring cells different.
vec3 cellGlyph(vec2 p, float x, float w, vec2 lo, vec4 cur, float t, float seed, vec3 c1) {
    vec2 size = vec2(0.6 * w, 0.6 * cur.w);
    vec2 q = vec2(p.x - (x + 0.5 * w - 0.5 * size.x), (lo.y + 0.5 * cur.w + 0.5 * size.y) - p.y);
    float id = floor(t * 30.0) + floor(seed * 97.0);
    float d = glyphDist(id, q, size);
    float sw = max(1.2, 0.07 * cur.w);
    float ink = 1.0 - smoothstep(0.5 * sw - 0.5, 0.5 * sw + 0.5, d);
    return mix(c1, vec3(1.0), 0.3) * ink + c1 * exp(-d / (0.05 * cur.w)) * 0.25;
}

// Does a stroke cross the cell edge at x (on the cursor's row)? A Chinese character's
// glyph spans two cells and usually does; two letters side by side leave a gap.
bool glyphAcross(float x, vec2 lo, vec4 cur, vec2 res) {
    for (int i = 0; i < 12; i++) {
        float y = lo.y + cur.w * (0.1 + 0.8 * float(i) / 11.0);
        if (colorDist(screenAt(vec2(x - 0.5, y) / res).rgb, iBackgroundColor) > 0.08
                && colorDist(screenAt(vec2(x + 0.5, y) / res).rgb, iBackgroundColor) > 0.08) return true;
    }
    return false;
}

// Decrypts one typed character: the slot x0..x1 on the cursor's row (one cell, or
// two for a Chinese character), t seconds into its decrypt of `dur`, in the DECODE_STYLE: the
// letter in `col` hides behind glyphs in the rain's color and shows again. Returns
// the light to add. cw: one cell's width.
vec3 decrypt(inout vec3 col, vec2 p, float x0, float x1, float cw, vec2 lo, vec2 hi, vec4 cur, float t, float dur, float seed, vec3 c1, vec2 res) {
    float u = t / dur, wd = x1 - x0;
    vec3 light = vec3(0.0);
    vec3 bg = screenAt(vec2(x0 + 1.5, hi.y - 1.5) / res).rgb;  // the slot's corner: background
    float letter = smoothstep(0.015, 0.06, colorDist(col, iBackgroundColor));
    float sw = max(1.2, 0.07 * cur.w);
    if (DECODE_STYLE == 1) {
        // Slot reel: a column of glyphs rolls down through the cell and slows;
        // the real letter is the last one on the reel and lands in place.
        float ur = min(1.0, u / 0.6), speed = 3.0 * (1.0 - ur) * (1.0 - ur);  // lands at 60 % of the time
        float v = (hi.y - p.y) + pow(1.0 - ur, 3.0) * 3.0 * cur.w;     // along the reel, 0 = the letter's top
        float j = floor(v / cur.w), ly = v - j * cur.w;                // j 0: the letter, 1...: glyphs above it
        float edge = smoothstep(0.0, 0.14 * cur.w, hi.y - p.y) * smoothstep(0.0, 0.14 * cur.w, p.y - lo.y);
        col = mix(col, bg, letter);
        if (j < 0.5) {
            vec3 s = screenAt(vec2(p.x, hi.y - ly) / res).rgb;
            float ink = smoothstep(0.015, 0.06, colorDist(s, iBackgroundColor));
            col = mix(col, mix(mix(c1, vec3(1.0), 0.45), s, smoothstep(0.6, 1.0, u)), ink * mix(edge, 1.0, ur));
        } else {
            vec2 size = vec2(0.6 * wd, 0.6 * cur.w);
            vec2 gq = vec2(p.x - (x0 + 0.2 * wd), ly - 0.2 * cur.w);
            float d = min(glyphDist(floor(seed * 97.0) + j * 7.0, gq, size),         // smeared down while it spins
                          glyphDist(floor(seed * 97.0) + j * 7.0, gq - vec2(0.0, 0.12 * speed * cur.w), size));
            float ink = 1.0 - smoothstep(0.5 * sw - 0.5, 0.5 * sw + 0.5, d);
            light += (mix(c1, vec3(1.0), 0.25) * ink + c1 * exp(-d / (0.05 * cur.w)) * 0.2) * edge * BRIGHTNESS;
        }
        light += c1 * 0.07 * (1.0 - ur) * edge * BRIGHTNESS;               // the reel's window, faintly lit
    } else if (DECODE_STYLE == 2) {
        // Pixel resolve: the letter builds up from coarse green blocks that get finer
        // (2x3, 4x6, 8x12 a cell, a few stray blocks early on), then sharpens and cools.
        float stage = floor(u * 4.0);
        vec2 q = p - vec2(x0, lo.y);
        col = mix(col, bg, letter * (1.0 - step(3.0, stage)));
        if (stage < 3.0) {
            vec2 bs = vec2(cw, cur.w) / (vec2(2.0, 3.0) * pow(2.0, stage));
            vec2 cell = floor(q / bs), cc = (cell + 0.5) * bs;
            float cov = 0.0;
            for (int k = 0; k < 4; k++) {
                vec2 o = (vec2(float(k % 2), float(k / 2)) - 0.5) * 0.5 * bs;
                cov += smoothstep(0.015, 0.06, colorDist(screenAt((vec2(x0, lo.y) + cc + o) / res).rgb, iBackgroundColor));
            }
            float stray = step(0.84, hash21(cell + floor(t * 30.0) + seed * 50.0)) * (1.0 - stage / 2.0);
            float on = max(step(0.24, 0.25 * cov), stray * 0.5);
            vec2 inBlock = abs(q - cc) / (0.5 * bs);
            float gap = 1.0 - step(0.84, max(inBlock.x, inBlock.y));
            light += mix(c1, vec3(1.0), 0.4 - 0.1 * stage) * on * gap * (1.0 - 0.15 * stage) * BRIGHTNESS;
        } else {
            light += mix(c1, vec3(1.0), 0.3) * letter * 0.8 * (1.0 - smoothstep(0.75, 1.0, u)) * BRIGHTNESS;
        }
    } else if (DECODE_STYLE == 3) {
        // Signal lock-in: a glyph flickers, then the letter arrives as a green and a teal
        // copy sliding together over interlaced scanlines; they lock with a short flash.
        col = mix(col, bg, letter * (1.0 - smoothstep(0.85, 1.0, u)));
        if (u < 0.2) {
            light += cellGlyph(p, x0, wd, lo, cur, t, seed, c1) * BRIGHTNESS;
        } else {
            float k = min(1.0, (u - 0.2) / 0.55);
            float jit = (hash11(floor(t * 30.0) + seed * 13.0) - 0.5) * 0.14 * cw * (1.0 - k);
            float off = pow(1.0 - k, 2.0) * 0.5 * cw;
            vec2 pa = vec2(p.x + off + jit, p.y), pb = vec2(p.x - off + jit, p.y);
            float a = (pa.x > x0 && pa.x < x1) ? smoothstep(0.015, 0.06, colorDist(screenAt(pa / res).rgb, iBackgroundColor)) : 0.0;
            float b = (pb.x > x0 && pb.x < x1) ? smoothstep(0.015, 0.06, colorDist(screenAt(pb / res).rgb, iBackgroundColor)) : 0.0;
            float scan = 0.7 + 0.3 * step(0.5, fract(p.y / max(2.0, 0.06 * cur.w)));
            float lock = exp(-pow((k - 0.85) / 0.12, 2.0));
            float out_ = 1.0 - smoothstep(0.85, 1.0, u);
            light += (c1 * a + vec3(0.12, 0.75, 0.75) * b) * 0.85 * scan * out_ * BRIGHTNESS;
            light += mix(c1, vec3(1.0), 0.7) * letter * lock * 0.9 * BRIGHTNESS;
        }
    } else {
        // TYPE_BEAM: over the last 45 % a beam sweeps the cell left to right; the letter shows
        // behind it (tinted green for a moment), the glyph stays ahead of it.
        float beamX = x0 + wd * smoothstep(0.3, 0.8, u);
        float ahead = TYPE_BEAM ? step(beamX, p.x) : 1.0 - smoothstep(0.6, 1.0, u);
        if (TYPE_BEAM && u > 0.3) {
            light += mix(c1, vec3(1.0), 0.7) * exp(-abs(p.x - beamX) / (0.05 * cw)) * 0.9 * (1.0 - smoothstep(0.75, 0.95, u)) * BRIGHTNESS;
            light += c1 * letter * (1.0 - ahead) * 0.6 * (1.0 - smoothstep(0.3, 1.0, u)) * BRIGHTNESS;
        }
        col = mix(col, bg, letter * (TYPE_BEAM ? ahead : 1.0 - smoothstep(0.6, 1.0, u)));
        vec2 size = vec2(0.6 * wd, 0.6 * cur.w);
        vec2 q = vec2(p.x - (x0 + 0.5 * wd - 0.5 * size.x), (0.5 * (lo.y + hi.y) + 0.5 * size.y) - p.y);
        float id = floor(t * 30.0) + floor(seed * 97.0);
        float d = glyphDist(id, q, size);
        float ink = 1.0 - smoothstep(0.5 * sw - 0.5, 0.5 * sw + 0.5, d);
        light += (mix(c1, vec3(1.0), 0.3) * ink + c1 * exp(-d / (0.05 * cur.w)) * 0.25)
               * (TYPE_BEAM ? ahead : 1.0 - smoothstep(0.55, 0.9, u)) * BRIGHTNESS;
    }
    return light;
}

// TYPE_PULSE: two packets of data shoot along each input box line, out from the
// cursor both ways: a bright head, a dashed body of bits, a soft glow. Only on
// real box lines (never in a shell).
vec3 busPulse(vec2 p, vec4 cur, vec2 center, float cellW, float cellH, vec2 res, float t, vec3 c1) {
    float reach = PULSE_REACH * cellW;
    if (abs(p.x - center.x) > reach + 2.0 * cellW || abs(p.y - center.y) > 1.7 * cellH) return vec3(0.0);
    float u = t / TYPE_TIME;
    float travel = 0.5 * cellW + reach * (1.0 - (1.0 - u) * (1.0 - u));
    float fade = 1.0 - smoothstep(0.55, 1.0, u);
    vec3 light = vec3(0.0);
    for (int k = 0; k < 2; k++) {
        float lineY = boxLineY(center + vec2(0.0, k == 0 ? cellH : -cellH), center, cur, res, 0.35 * cellH);
        if (lineY < 0.0) continue;
        float dy = abs(p.y - lineY);
        if (dy > 0.3 * cellH) continue;
        for (int s = 0; s < 2; s++) {
            float side = s == 0 ? -1.0 : 1.0;
            float headX = center.x + side * travel;
            float along = side * (p.x - headX);           // 0 at the head, negative behind it
            float len = 1.8 * cellW;
            float body = step(-len, along) * step(along, 0.0) * (1.0 + along / len);
            float bits = step(0.35, fract((p.x - center.x) / (0.34 * cellW)));
            float core = exp(-dy / (0.03 * cellH));
            float head = exp(-length(vec2(p.x - headX, 2.0 * dy)) / (0.22 * cellH));
            light += (mix(c1, vec3(1.0), 0.6) * head * 0.9 + c1 * body * (bits * core * 1.1 + exp(-dy / (0.12 * cellH)) * 0.18)) * fade;
        }
    }
    return light * BRIGHTNESS;
}

// Everything that answers the cursor's moves and the signals (fragCoord is
// already y-up).
void effects(inout vec4 fragColor, vec2 fragCoord) {
    vec2 res = iResolution.xy;

    // Cursor rectangles in pixels (y grows upward; .xy is the top-left corner).
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w);
    vec2 hi = vec2(cur.x + cur.z, cur.y);
    float cw = cellWOf(cur);
    float cellH = max(cur.w, cw * 2.0);
    float cellW = max(cw, cellH * 0.4);
    vec2 center = (lo + hi) * 0.5;
    vec4 prv = upCursor(iPreviousCursor);
    vec2 from = vec2(prv.x + prv.z * 0.5, prv.y - prv.w * 0.5);
    vec3 c1 = sRGBToLinear(RAIN_COLOR), c2 = sRGBToLinear(TRAIL_COLOR);
    vec2 p = fragCoord;

    float sinceMove = iTime - iTimeCursorChange;
    float moved = abs(cur.x - prv.x) / cellW + 2.0 * abs(cur.y - prv.y) / cellH;
    // Forward along the row by up to 40 cells: text was just typed there. A letter, a
    // Chinese character (two cells) or a whole phrase that an input method commits at
    // once (the cursor stays put while you compose it, then jumps): all decrypt.
    bool sameRow = abs(cur.y - prv.y) < 0.5 * cellH;
    float span = sameRow ? floor((cur.x - prv.x) / cellW + 0.5) : 0.0;
    bool typed = span >= 1.0 && span <= 40.0;
    // Wait until the cursor has stayed put a moment (longer after a jump, so the
    // Enter signal can arrive first): spots a program's cursor passes through
    // while it redraws never show anything.
    sinceMove -= moved >= 3.0 && !typed ? JUMP_SETTLE : SETTLE;
    if (sinceMove < 0.0) return;
    // A jump while a signal is on means the same Enter only inside Claude Code's input box
    // (a box line just below the cursor: it cleared the box after the signal arrived), and
    // never forward along the row. A Tab completion, a prompt the shell redraws, a program
    // hopping the cursor elsewhere never start the effect again.
    bool back = moved >= 3.0 && (oddByte(iCurrentCursorColor.b) || oddByte(iCurrentCursorColor.g))
        && !(abs(cur.y - prv.y) < 0.5 * cellH && cur.x > prv.x)
        && clLineNear(cur.y - 1.7 * cur.w, 0.7 * cur.w, res) >= 0.0;
    bool done = DONE_EFFECT && doneSignal(back);
    if ((ENTER_EFFECT && enterSignal(back)) || done) {
        if (sinceMove < ENTER_TIME)
            fragColor.rgb = min(fragColor.rgb + enterEffect(p, cur, center, cellW, cellH, res, sinceMove, done, c1, c2), vec3(1.0));
        return;
    }
    // Only the color changed (the Enter signal ending), the cursor jumped to another
    // row (a program redrawing: never an effect), or back along its row (or more than
    // 40 cells forward) with JUMP_EFFECTS off: not a move.
    if (moved < 0.5 || (moved >= 3.0 && !typed && !(JUMP_EFFECTS && sameRow))) sinceMove = 1e4;
    bool jump = moved >= 3.0 && !typed && sinceMove < DISSOLVE_TIME;
    bool typing = TYPE_EFFECT && (PHRASE_EFFECT || !(typed && span > 1.5)) && (moved < 3.0 || typed) && sinceMove < max(TYPE_TIME, DECODE_TIME + 22.0 * WORD_WAVE);
    if (!jump && !typing) return;

    vec3 light = vec3(0.0);
    // Jump along the row: the row dissolves into rain, starting at the cursor's
    // new spot and spreading along the path.
    if (jump) {
        float x0 = min(from.x, center.x) - 0.5 * cellW, x1 = max(from.x, center.x) + 0.5 * cellW;
        light += rain(p, hi.y, lo.y - 1.2 * cellH, x0, x1, 0.0, GLYPH_SIZE * cellH, sinceMove, center.x, (x1 - x0) / 0.2 + 1.0, false,
                      6.0 * cellH, 4.0, 0.8, 3.0, res, c1, c2)
               * (1.0 - smoothstep(0.6, 1.0, sinceMove / DISSOLVE_TIME)) * BRIGHTNESS;
    }
    if (typing) {
        // The text the move just filled in, left of the cursor, is decrypted: one letter,
        // a Chinese character (two cells, one glyph across both, decrypted as one), or a
        // phrase an input method committed, left to right; each one hides until its turn.
        // Only forward along the row: after Backspace or Left the letters there are old.
        bool forward = typed && span < 2.5;  // one key (WORD_DECODE)
        if (typed && p.y > lo.y && p.y < hi.y && p.x > prv.x && p.x < prv.x + span * cw) {
            float k = floor((p.x - prv.x) / cw);  // this cell, counted from the first new one
            // Chinese characters: an even number of cells, a single character, or one
            // of the first four pairs with a stroke across its middle (letters leave a gap).
            bool wide = mod(span, 2.0) < 0.5 && span < 2.5;
            for (int i = 0; i < 4; i++) {
                if (wide || mod(span, 2.0) > 0.5 || float(2 * i + 2) > span) break;
                wide = glyphAcross(prv.x + float(2 * i + 1) * cw, lo, cur, res);
            }
            float ks = wide ? 2.0 * floor(0.5 * k) : k;              // the character's first cell
            float x0 = prv.x + ks * cw, x1 = x0 + (wide ? 2.0 : 1.0) * cw;
            // A letter takes DECODE_TIME; Chinese input is quicker so you can read along: the
            // whole phrase within PHRASE_TIME, each character starting a little after the last.
            float dur = span < 1.5 ? DECODE_TIME : 0.7 * PHRASE_TIME;
            float tk = sinceMove - (span < 1.5 ? 0.0 : 0.3 * PHRASE_TIME * ks / span);  // its own clock, left to right
            // An empty cell (a space) has nothing to decrypt.
            if (tk < dur && cellInk(x0, x1 - x0, lo, cur, res)) {
                if (tk < 0.0) {
                    float letter = smoothstep(0.015, 0.06, colorDist(fragColor.rgb, iBackgroundColor));
                    fragColor.rgb = mix(fragColor.rgb, screenAt(vec2(x0 + 1.5, hi.y - 1.5) / res).rgb, letter);
                } else {
                    light += decrypt(fragColor.rgb, p, x0, x1, cw, lo, hi, cur, tk, dur, hash11(iTimeCursorChange + ks * 7.31), c1, res);
                }
            }
        }
        // WORD_DECODE: a space was just typed (forward, the cell left of the cursor is
        // blank): the word before it decrypts again, its first letter first.
        if (WORD_DECODE && forward && p.y > lo.y && p.y < hi.y && p.x < lo.x - cw && p.x > lo.x - 23.0 * cw
                && !cellInk(lo.x - cw, cw, lo, cur, res)) {
            float k = floor((lo.x - p.x) / cw);                 // 0 = the space just typed, 1 = the word's last letter
            float x = lo.x - (k + 1.0) * cw;                     // this cell's left edge
            float n = 0.0;                                        // the word: unbroken ink from cell 1 leftward
            for (int i = 1; i <= 22; i++) { if (!cellInk(lo.x - float(i + 1) * cw, cw, lo, cur, res)) break; n = float(i); }
            if (k >= 1.0 && k <= n) {
                float tk = sinceMove - (n - k) * WORD_WAVE;      // this letter's own clock: the first letter first
                if (tk > 0.0 && tk < 0.16) {
                    vec3 bg = screenAt(vec2(x + 1.5, hi.y - 1.5) / res).rgb;
                    float letter = smoothstep(0.015, 0.06, colorDist(fragColor.rgb, iBackgroundColor));
                    float hide = 1.0 - smoothstep(0.09, 0.16, tk);
                    fragColor.rgb = mix(fragColor.rgb, bg, letter * hide);
                    light += cellGlyph(p, x, cw, lo, cur, tk, hash11(k * 3.1 + iTimeCursorChange), c1) * hide * BRIGHTNESS;
                } else if (tk >= 0.16 && tk < 0.4) {
                    float letter = smoothstep(0.015, 0.06, colorDist(fragColor.rgb, iBackgroundColor));
                    light += c1 * letter * 0.5 * (1.0 - smoothstep(0.16, 0.4, tk)) * BRIGHTNESS;
                }
            }
        }
        if (TYPE_PULSE && sinceMove < TYPE_TIME) light += busPulse(p, cur, center, cellW, cellH, res, sinceMove, c1);
    }
    fragColor = vec4(min(fragColor.rgb + light, vec3(1.0)), fragColor.a);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 res = iResolution.xy;
    if (Y_DOWN) fragCoord.y = res.y - fragCoord.y;
    fragColor = screenAt(fragCoord / res);
    // The moving effects play only in the focused window; the colors (your text, the box
    // lines, the prompt mark) stay when you switch to another window or Space.
    if (iFocus != 0 && iCursorVisible != 0) effects(fragColor, fragCoord);
    if (TEXT_COLOR && iCurrentCursorStyle == CURSORSTYLE_BLOCK) fragColor.rgb = clamp(textTint(fragCoord, fragColor.rgb, res), 0.0, 1.0);
    if (iCursorVisible == 0 || iCurrentCursorStyle != CURSORSTYLE_BLOCK) return;
    if (LINE_EFFECT || (LINE_TINT && LINE_COLOR > 0.0)) fragColor.rgb = min(linesLook(fragCoord, fragColor.rgb), vec3(1.0));
    if (CURSOR_EFFECT && iFocus != 0) fragColor.rgb = min(cursorLook(fragCoord, fragColor.rgb, res, sRGBToLinear(RAIN_COLOR)), vec3(1.0));
    if (PROMPT_EFFECT) fragColor.rgb = min(promptLook(fragCoord, fragColor.rgb), vec3(1.0));
}
