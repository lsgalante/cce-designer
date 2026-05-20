struct VertexOutput {
    @builtin(position) clip_position: vec4f,
    @location(0) color: vec4f,
}

@vertex
fn vs_main(
    @location(0) position: vec2f,
    @location(1) color: vec4f,
) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4f(position, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    return in.color;
}
