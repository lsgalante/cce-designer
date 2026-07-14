// Glyph-atlas pipeline for the ash text stage. Mask glyphs are stored as
// white-with-alpha texels, color (emoji) glyphs as-is with a white vertex
// color — one multiply covers both.

@group(0) @binding(0) var t_atlas: texture_2d<f32>;
@group(0) @binding(1) var s_atlas: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
    @location(2) clip_circle: vec3f,
}

@vertex
fn vs_main(
    @location(0) position: vec2f,
    @location(1) uv: vec2f,
    @location(2) color: vec4f,
    @location(3) clip_circle: vec3f,
) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4f(position, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    out.clip_circle = clip_circle;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Same circle clip as shader.wgsl (framebuffer px): used by the circular
    // network pane's curved rim labels.
    if (in.clip_circle.z > 0.0) {
        let dx = in.clip_position.x - in.clip_circle.x;
        let dy = in.clip_position.y - in.clip_circle.y;
        if (dx * dx + dy * dy > in.clip_circle.z * in.clip_circle.z) {
            discard;
        }
    }
    return in.color * textureSample(t_atlas, s_atlas, in.uv);
}
