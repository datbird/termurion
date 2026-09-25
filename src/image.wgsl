struct Screen {
    size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var picture: texture_2d<f32>;
@group(1) @binding(1) var picture_sampler: sampler;

struct In {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(v: In) -> Out {
    var o: Out;
    let p = v.pos / screen.size * 2.0 - 1.0;
    o.pos = vec4<f32>(p.x, -p.y, 0.0, 1.0);
    o.uv = v.uv;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return textureSample(picture, picture_sampler, i.uv);
}
