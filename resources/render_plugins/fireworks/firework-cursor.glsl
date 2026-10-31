// Fireworks cursor for Ghostty (`custom-shader`).
// Jump the cursor along its row (delete-all, Home; JUMP_EFFECTS) and a
// firework shoots up from it and bursts
// into a ring of sparks that fall and fade; each firework picks its own color.
// Near the top of the window it fires downward instead. Typing pops a few tiny
// sparkles off the cursor. At rest the cursor is left clean. Enter (a prompt
// sent in Claude Code, a command run in zsh, via `cursor-fx signal`) lights a
// fuse that burns out from the cursor along the lines of Claude Code's input
// box and launches three fireworks from the line as it passes.
// Its cursor is a sparkler: tiny star sparkles in gold, pink and blue flare and
// fade around its top, in a faint warm glow. The prompt mark (❯) on its line
// takes the same style.
// A slow fuse burns along the input box's lines, throwing small sparks.

// macOS: true. On Linux, change it to false (install.sh does this for you);
// otherwise the effect plays upside down.
#define Y_DOWN true

// --- settings: the ones worth a tweak (edit and save, or use `cursor-fx edit`; Ghostty reloads on Command+Shift+,) ---
// -- Typing
const bool  TYPE_EFFECT    = true;  // tiny colored sparkles pop off the top of the cursor
const bool  PHRASE_EFFECT  = true;  // picking a character or phrase from your input method plays the typing effect too (like one keystroke)
// -- Enter & done
const bool  ENTER_EFFECT = true;  // fuse and fireworks on the input box lines on Enter (`cursor-fx enter off` pauses every effect's Enter)
const bool  DONE_EFFECT = true;  // play it again, from the edges inward, when Claude finishes answering (Stop hook) or a long zsh command ends
const float ENTER_TIME   = 1.7;  // seconds until the last firework is out
// -- Delete-all
const bool  JUMP_EFFECTS = true;  // launch when the cursor jumps along its row (`cursor-fx jumps on|off`)
// -- Cursor
const bool  CURSOR_EFFECT  = true;  // the cursor's own look: a sparkler: tiny colored stars flare around its top
const bool  IDLE_EFFECTS   = true;  // the cursor's motion while idle (`cursor-fx idle on|off`; off keeps the screen calm)
// -- Prompt mark
const bool  PROMPT_EFFECT  = true;  // the prompt mark (❯) at the start of the cursor's line: gold, with a sparkle twinkling at its tip
// -- Box lines
const bool  LINE_TINT      = true;  // the input box lines take the cursor's color (a plain color, no glow)
const float LINE_COLOR     = 0.42;  // how much of the cursor's color the lines take, at their own brightness
const bool  LINE_EFFECT    = true;  // the input box lines, always: a slow fuse burning along, throwing sparks
// -- Look
const float RISE_HEIGHT  = 2.6;  // how high a firework bursts, in character heights
const float BURST_RADIUS = 2.2;  // how far its sparks fly, in character heights
const float SPARKS       = 28.0;  // sparks in a burst (at most 32)

// --- fine-tuning ---
// -- Typing
const float POP_TIME     = 0.25;  // seconds the sparkles last
// -- Enter & done
const float ENTER_FUSE   = 0.35;  // seconds the fuse takes from the cursor to the edges
// -- Delete-all
const float LAUNCH_MIN   = 3.0;  // moves this long or longer launch a firework (in characters; a row counts as two)
const float RISE_TIME    = 0.3;  // seconds a rocket flies before it bursts
// -- Cursor
const float CURSOR_STRENGTH = 1.0;  // how strong the cursor's own look is
// -- Box lines
const float LINE_STRENGTH  = 1.0;   // how strong the box lines' own look is
// -- Look
const float BURST_TIME   = 1.0;  // seconds the sparks live
const float GRAVITY      = 2.5;  // how fast sparks fall, in character heights per second squared
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

// The firework colors (sRGB): gold, pink, cyan, green, violet.
vec3 palette(float k) {
    if (k < 1.0) return vec3(1.0, 0.8, 0.35);
    if (k < 2.0) return vec3(1.0, 0.4, 0.75);
    if (k < 3.0) return vec3(0.35, 0.9, 1.0);
    if (k < 4.0) return vec3(0.5, 1.0, 0.45);
    return vec3(0.75, 0.55, 1.0);
}

// Light from a short glowing streak from a to b.
vec3 streak(vec2 p, vec2 a, vec2 b, float width, vec3 color) {
    float d = segDist(p, a, b);
    return color * (1.0 - smoothstep(width * 0.4, width, d)) + color * exp(-d / (width * 1.6)) * 0.5;
}

// Light from one firework launched from start at time 0 and bursting at apex;
// t is the time since launch. Nothing outside its box is touched.
vec3 firework(vec2 p, vec2 start, vec2 apex, float seed, float t, float cellH) {
    float fall = 0.5 * GRAVITY * BURST_TIME * BURST_TIME;
    float top = max(apex.y, start.y) + (BURST_RADIUS + 1.0) * cellH;
    float bottom = min(apex.y, start.y) - (BURST_RADIUS + fall + 1.0) * cellH;
    if (t < 0.0 || t > RISE_TIME + BURST_TIME || abs(p.x - start.x) > (BURST_RADIUS + 1.0) * cellH
            || p.y > top || p.y < bottom) return vec3(0.0);
    vec3 light = vec3(0.0);
    float w = 0.05 * cellH;
    vec3 color = sRGBToLinear(palette(floor(hash11(seed) * 5.0)));
    if (t < RISE_TIME) {
        // The rocket: a bright head with a sparkly trail, slowing as it climbs.
        float x = t / RISE_TIME;
        vec2 head = mix(start, apex, 1.0 - (1.0 - x) * (1.0 - x));
        float xt = max(x - 0.3, 0.0);
        vec2 tail = mix(start, apex, 1.0 - (1.0 - xt) * (1.0 - xt));
        float flick = 0.7 + 0.3 * hash11(floor(iTime * 40.0));
        light += streak(p, tail, head, w, sRGBToLinear(vec3(1.0, 0.85, 0.6)) * 0.6 * flick);
        light += vec3(1.0) * exp(-distance(p, head) / (0.08 * cellH)) * 0.9;
        return light;
    }
    // The burst: sparks thrown out in a ring that slow down, fall and fade.
    float bt = t - RISE_TIME;
    float life = bt / BURST_TIME;
    float fade = pow(1.0 - life, 1.5);
    light += mix(color, vec3(1.0), 0.6) * exp(-distance(p, apex) / (0.5 * cellH)) * max(0.0, 1.0 - bt / 0.1);
    for (float i = 0.0; i < 32.0; i += 1.0) {
        if (i >= SPARKS) break;
        float h = hash11(seed + i * 1.91);
        float ang = (i + 0.5 * h) / SPARKS * 6.2832;
        vec2 dir = vec2(cos(ang), sin(ang));
        float reach = BURST_RADIUS * cellH * (0.7 + 0.45 * hash11(seed + i * 3.3));
        // Drag: fast out, then slowing; gravity pulls down over time.
        float out1 = reach * (1.0 - exp(-bt * 4.0));
        float bt0 = max(bt - 0.12, 0.0);  // where the spark was a moment ago: its trail
        float out0 = reach * (1.0 - exp(-bt0 * 4.0));
        vec2 g = vec2(0.0, -0.5 * GRAVITY * cellH);
        vec2 pos = apex + dir * out1 + g * bt * bt;
        vec2 prev = apex + dir * out0 + g * bt0 * bt0;
        vec3 sc = h > 0.85 ? vec3(1.0) : color;
        float twinkle = life > 0.6 ? step(0.35, hash11(floor(iTime * 25.0) + i * 7.7)) : 1.0;
        light += streak(p, prev, pos, w, sc * fade * twinkle);
    }
    return light;
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

// Enter: a fuse burns out from the cursor along both input box lines, a
// sparkling head leaving a glowing trail, and three fireworks launch from the
// upper line as the fuse passes them, each its own color and height.
vec3 enterEffect(vec2 p, vec4 cur, vec2 center, float cellW, float cellH, vec2 res, float t, vec3 col, bool inward) {
    // Nothing below the box unless the fireworks have to fire downward (no room above).
    bool roomAbove = center.y + (RISE_HEIGHT + 1.8 + BURST_RADIUS) * cellH <= res.y;
    if (roomAbove && p.y < center.y - 2.5 * cellH) return col;
    vec2 lines = boxLines(cur, center, cellW, cellH, res);
    vec3 light = vec3(0.0);
    float head = t / ENTER_FUSE * res.x;  // how far the fuse has burnt from the cursor
    float dist = spreadDist(p, center, res, inward);
    float side = inward ? (p.x < 0.5 * res.x ? 1.0 : -1.0) : (p.x < center.x ? -1.0 : 1.0);
    float originX = inward ? (p.x < 0.5 * res.x ? 0.0 : res.x) : center.x;
    for (int k = 0; k < 2; k++) {
        float lineY = k == 0 ? lines.x : lines.y;
        if (lineY < 0.0) continue;
        float dy = abs(p.y - lineY);
        if (dy > 1.2 * cellH || t > ENTER_FUSE + 0.4) continue;
        float tick = floor(iTime * 40.0);
        float at = exp(-pow((dist - head) / (0.5 * cellH), 2.0));
        float flick = 0.7 + 0.3 * hash11(tick + float(k));
        light += vec3(1.0, 0.9, 0.6) * at * exp(-dy / (0.25 * cellH)) * 1.2 * flick;  // the burning head
        for (float i = 0.0; i < 5.0; i += 1.0) {  // sparks spitting off it
            float h = hash11(tick * 3.1 + i * 2.3 + float(k) * 9.0);
            vec2 sp = vec2(originX + side * head + (h - 0.5) * 1.2 * cellH, lineY + (hash11(h * 77.0 + i) - 0.5) * 1.6 * cellH);
            light += sRGBToLinear(vec3(1.0, 0.85, 0.5)) * exp(-distance(p, sp) / (0.05 * cellH)) * 0.9;
        }
        float burnt = (head - dist) / res.x * ENTER_FUSE;  // seconds since the fuse passed this spot
        if (burnt > 0.0) light += vec3(1.0, 0.5, 0.15) * exp(-dy / (0.12 * cellH)) * exp(-burnt / 0.15) * 0.8;
    }
    float lineY = lines.x >= 0.0 ? lines.x : lines.y;
    float up = lineY + (RISE_HEIGHT + 0.8 + BURST_RADIUS) * cellH > res.y ? -1.0 : 1.0;
    float margin = (BURST_RADIUS + 1.0) * cellH;
    for (float k = 0.0; k < 3.0; k += 1.0) {
        float x = clamp(res.x * (0.2 + 0.3 * k), margin, res.x - margin);
        vec2 start = vec2(x, lineY);
        vec2 apex = start + vec2(0.0, up * (RISE_HEIGHT + 0.4 * mod(k + 1.0, 3.0)) * cellH);
        float seed = mod(iTimeCursorChange * 3.7 + k * 17.0, 61.0);
        light += firework(p, start, apex, seed, t - spreadDist(vec2(x, 0.0), center, res, inward) / res.x * ENTER_FUSE, cellH);
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

// Fireworks' cursor: a sparkler. Tiny four-pointed sparkles in gold, pink and
// blue flare and fade one after another around the block's top, in a faint
// warm glow.
vec3 cursorLook(vec2 p, vec3 col) {
    vec4 cur = upCursor(iCurrentCursor);
    vec2 lo = vec2(cur.x, cur.y - cur.w), hi = vec2(cur.x + cur.z, cur.y);
    vec2 c = 0.5 * (lo + hi), hs = 0.5 * (hi - lo);
    float cellH = max(cur.w, cellWOf(cur) * 2.0);
    vec2 q = abs(p - c) - hs;
    if (max(q.x, q.y) > 0.55 * cellH) return col;
    vec2 res = iResolution.xy;
    if (!cursorDrawn(lo, cur, res)) return col;
    float t = IDLE_EFFECTS ? iTime : 0.0, live = IDLE_EFFECTS ? 1.0 : 0.0;
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    float d = clBox(p - c, hs);
    vec3 light = k * exp(-max(d, 0.0) / (0.08 * cellH)) * 0.2 * step(0.0, d);
    for (int i = 0; i < 5; i++) {
        float fi = float(i);
        float per = 0.9 + 0.5 * clHash(vec2(fi, 1.0));
        float ph = t / per + clHash(vec2(fi, 2.0));
        float w = floor(ph), u = fract(ph);
        vec2 at = vec2(lo.x + (clHash(vec2(fi, w + 3.0)) * 1.6 - 0.3) * cur.z,
                       hi.y + (clHash(vec2(fi, w + 5.0)) * 0.35 - 0.05) * cur.w);
        float life = sin(3.14159 * clamp(u / 0.6, 0.0, 1.0));
        vec2 dp = abs(p - at);
        float size = 0.12 * cellH * life + 0.01;
        float star = 0.5 * (exp(-dp.x / 0.8) * exp(-dp.y / size) + exp(-dp.y / 0.8) * exp(-dp.x / size)) + exp(-length(dp) / 1.2);
        vec3 tint = i == 1 ? sRGBToLinear(vec3(1.0, 0.45, 0.7)) : i == 3 ? sRGBToLinear(vec3(0.45, 0.85, 1.0)) : mix(k, vec3(1.0), 0.5);
        light += tint * star * life * 0.7 * live;
    }
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

// The prompt mark in gold, with a little four-pointed sparkle twinkling at
// its tip and a warm glow.
vec3 markLook(vec2 p, vec3 col, float ink, float halo, vec2 m, float w, float h, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    vec2 res = iResolution.xy;
    vec2 tip = m + vec2(0.78 * w, -0.5 * h);
    float life = live * pow(max(sin(t * 2.4), 0.0), 3.0);
    vec2 dp = abs(p - tip);
    float size = 0.25 * h * life + 0.01;
    float star = 0.5 * (exp(-dp.x / 0.8) * exp(-dp.y / size) + exp(-dp.y / 0.8) * exp(-dp.x / size)) + exp(-length(dp) / 1.2) * life;
    col = mix(col, mix(k, vec3(1.0), 0.2), ink);
    return col + k * halo * (1.0 - ink) * 0.35 + mix(k, vec3(1.0), 0.6) * star * 0.8;
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

// A slow fuse burns along the box lines: a bright point with a glowing trail
// that crosses the window in eight seconds, throwing small sparks.
vec3 lineLook(vec2 p, vec3 col, float lineY, float h, vec2 res, float t, float live) {
    vec3 k = sRGBToLinear(iCurrentCursorColor.rgb);
    float dy = abs(p.y - lineY), core = exp(-dy / (0.03 * h)), soft = exp(-dy / (0.14 * h));
    float fx = fract(t / 8.0) * res.x, s = fx - p.x;
    float trail = live * (s >= 0.0 ? exp(-s / (4.0 * h)) : exp(s / (0.15 * h)));
    vec3 light = k * (core * 0.15 + soft * 0.06) + mix(k, vec3(1.0), 0.6) * trail * (core + soft * 0.4);
    for (int i = 0; i < 4; i++) {
        float ph = t * 3.0 + 0.25 * float(i), u = fract(ph);
        vec2 dir = normalize(vec2(clHash(vec2(floor(ph), float(i))) - 0.5, 0.2 + 0.8 * clHash(vec2(floor(ph), float(i) + 5.0))));
        light += mix(k, vec3(1.0), 0.5) * exp(-length(p - vec2(fx, lineY) - dir * u * 0.35 * h) / 1.0) * (1.0 - u) * 0.8 * live;
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
    vec4 prv = upCursor(iPreviousCursor);

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
        if (sinceMove < ENTER_TIME) fragColor.rgb = enterEffect(fragCoord, cur, (lo + hi) * 0.5, cellW, cellH, res, sinceMove, fragColor.rgb, done);
        return;
    }
    // Only the color changed (the Enter signal ending), the cursor jumped to another
    // row (a program redrawing: never an effect), or along its row with JUMP_EFFECTS
    // off: not a move.
    if (moved < 0.5 || (moved >= LAUNCH_MIN && !(JUMP_EFFECTS && abs(cur.y - prv.y) < 0.5 * cellH))) sinceMove = 1e4;
    bool launch = moved >= LAUNCH_MIN && sinceMove < RISE_TIME + BURST_TIME;
    bool pop = typeOn && moved < LAUNCH_MIN && sinceMove < POP_TIME;
    if (!launch && !pop) return;

    // Fire upward, or downward when there's no room above.
    float up = hi.y + (RISE_HEIGHT + BURST_RADIUS) * cellH > res.y ? -1.0 : 1.0;
    vec2 start = vec2((lo.x + hi.x) * 0.5, up > 0.0 ? hi.y : lo.y);
    vec2 apex = start + vec2(0.0, up * RISE_HEIGHT * cellH);
    float seed = mod(iTimeCursorChange * 3.7, 61.0);

    vec2 p = fragCoord;
    vec3 col = fragColor.rgb;
    if (launch) {
        col += firework(p, start, apex, seed, sinceMove, cellH);
    } else {
        // Typing: a few tiny sparkles popping off the top of the cursor.
        if (distance(p, start) > 1.5 * cellH) return;
        float x = sinceMove / POP_TIME;
        for (float i = 0.0; i < 4.0; i += 1.0) {
            float h = hash11(seed + i * 2.3);
            float ang = mix(0.35, 2.8, h) * (up > 0.0 ? 1.0 : -1.0);
            vec2 pos = start + vec2(cos(ang), sin(ang)) * (0.25 + 0.5 * x) * cellH;
            vec3 sc = sRGBToLinear(palette(floor(hash11(seed + i * 5.1) * 5.0)));
            col += sc * exp(-distance(p, pos) / (0.05 * cellH)) * (1.0 - x) * 0.9;
        }
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
