struct Uniforms {
    mvp: mat4x4<f32>,
    window_size: vec2<f32>,
    window_radius: f32,
    padding: f32,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

fn is_outside_window_corners(pos: vec2<f32>) -> bool {
    let w = uniforms.window_size.x;
    let h = uniforms.window_size.y;
    let r = uniforms.window_radius;
    
    // Top-left
    if (pos.x < r && pos.y < r) {
        let dx = pos.x - r;
        let dy = pos.y - r;
        return (dx * dx + dy * dy) > r * r;
    }
    // Top-right
    if (pos.x > w - r && pos.y < r) {
        let dx = pos.x - (w - r);
        let dy = pos.y - r;
        return (dx * dx + dy * dy) > r * r;
    }
    // Bottom-left
    if (pos.x < r && pos.y > h - r) {
        let dx = pos.x - r;
        let dy = pos.y - (h - r);
        return (dx * dx + dy * dy) > r * r;
    }
    // Bottom-right
    if (pos.x > w - r && pos.y > h - r) {
        let dx = pos.x - (w - r);
        let dy = pos.y - (h - r);
        return (dx * dx + dy * dy) > r * r;
    }
    // Boundary check
    if (pos.x < 0.0 || pos.x > w || pos.y < 0.0 || pos.y > h) {
        return true;
    }
    return false;
}

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) color: vec3f,
};

@vertex
fn vs_main(
    @location(0) position: vec3f,
    @location(1) color: vec3f,
) -> VertexOutput {
    var out: VertexOutput;
    if (abs(position.z - 9.99) < 0.01) {
        out.position = vec4f(position.xy, 0.9999, 1.0);
    } else {
        out.position = uniforms.mvp * vec4f(position, 1.0);
    }
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    if (is_outside_window_corners(in.position.xy)) {
        discard;
    }
    return vec4f(in.color, 1.0);
}
