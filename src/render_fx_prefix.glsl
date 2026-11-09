#version 450 core
// Ghostty's custom shader prefix (src/renderer/shaders/shadertoy_prefix.glsl), copyright (c) 2024
// Mitchell Hashimoto, Ghostty contributors, MIT (resources/render_plugins/LICENSE-ghostty). Changed for
// naga: explicit sets, a separate texture and sampler behind the iChannel0 name, and no forward
// declared mainImage (render_fx.rs adds main() after the shader body).

layout(set = 0, binding = 1, std140) uniform Globals {
    vec3  iResolution;
    float iTime;
    float iTimeDelta;
    float iFrameRate;
    int   iFrame;
    // Termurion: a vec4, not Ghostty's float[4]. naga's Metal output packs a float array 4 bytes
    // apart, not std140's 16, so every member after it read zeros on macOS. iChannelTime[i] still works.
    vec4  iChannelTime;
    vec3  iChannelResolution[4];
    vec4  iMouse;
    vec4  iDate;
    float iSampleRate;
    vec4  iCurrentCursor;
    vec4  iPreviousCursor;
    vec4  iCurrentCursorColor;
    vec4  iPreviousCursorColor;
    int   iCurrentCursorStyle;
    int   iPreviousCursorStyle;
    int   iCursorVisible;
    float iTimeCursorChange;
    float iTimeFocus;
    int iFocus;
    vec3  iPalette[256];
    vec3  iBackgroundColor;
    vec3  iForegroundColor;
    vec3  iCursorColor;
    vec3  iCursorText;
    vec3  iSelectionForegroundColor;
    vec3  iSelectionBackgroundColor;
    // Termurion: the input box as read from the grid (x, y: top and bottom in pixels from the
    // top; z = 1 for a line box, 2 for a shaded band, 0 for none). Not in Ghostty.
    vec4  iInputBox;
    // Termurion: a band box's left and right edge in pixels (x, y), zeros for a line box.
    vec4  iInputBoxX;
};

// Termurion: how far above the cursor the input box reaches (it grows with a long prompt):
// at least the 4 rows cursor-fx assumes.
float termurionAbove(float cellH) {
    if (iInputBox.z < 0.5) return 4.0 * cellH;
    return max(4.0 * cellH, iCurrentCursor.y - iInputBox.x + cellH);
}

// Termurion: how far below the cursor's middle a band's bottom edge reaches (opencode's box
// can run several rows under the cursor): at least the `rows` the shader assumes.
float termurionBelow(float cellH, float rows) {
    if (iInputBox.z < 1.5) return rows * cellH;
    return max(rows * cellH, iInputBox.y - iCurrentCursor.y + 0.5 * cellH);
}

// Termurion: a band (z = 2) has no line ink for LINE_TINT to color, so its edge at lineY
// gets a thin line of color k instead, about a box glyph thick with soft edges.
vec3 termurionBandLine(vec2 p, vec3 col, float lineY, vec3 k) {
    float core = max(0.75, 0.04 * iCurrentCursor.w);
    return mix(col, k, 1.0 - smoothstep(core, core + 1.0, abs(p.y - lineY)));
}

#define CURSORSTYLE_BLOCK        0
#define CURSORSTYLE_BLOCK_HOLLOW 1
#define CURSORSTYLE_BAR          2
#define CURSORSTYLE_UNDERLINE    3
#define CURSORSTYLE_LOCK         4

layout(set = 0, binding = 0) uniform texture2D iChannel0_tex;
layout(set = 0, binding = 2) uniform sampler iChannel0_smp;
#define iChannel0 sampler2D(iChannel0_tex, iChannel0_smp)

layout(location = 0) out vec4 _fragColor;

#define texture2D texture
// The frame has one mip level, so an explicit level 0 samples the same texels. It keeps texture
// reads in loops free of derivatives, which Windows' FXC compiler otherwise refuses (X3511).
#define texture(s, uv) textureLod(s, uv, 0.0)


