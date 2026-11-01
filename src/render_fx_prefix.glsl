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
    float iChannelTime[4];
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
    // Termurion: Claude Code's input box lines as read from the grid (y of the top and the
    // bottom line in pixels from the top, z = 1 when there is a box). Not in Ghostty.
    vec4  iInputBox;
};

// Termurion: how far above the cursor the input box reaches (it grows with a long prompt):
// at least the 4 rows cursor-fx assumes.
float termurionAbove(float cellH) {
    if (iInputBox.z < 0.5) return 4.0 * cellH;
    return max(4.0 * cellH, iCurrentCursor.y - iInputBox.x + cellH);
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


