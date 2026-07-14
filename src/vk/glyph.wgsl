// Glyph-atlas pipeline for the ash text stage. Mask glyphs are stored as
// white-with-alpha texels, color (emoji) glyphs as-is with a white vertex
// color — one multiply covers both.

@group(0) @binding(0) var t_atlas: texture_2d<f32>;
@group(0) @binding(1) var s_atlas: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
}

@vertex
fn vs_main(
    @location(0) position: vec2f,
    @location(1) uv: vec2f,
    @location(2) color: vec4f,
) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4f(position, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    return in.color * textureSample(t_atlas, s_atlas, in.uv);
}
