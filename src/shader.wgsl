@group(0) @binding(0) var t_backdrop: texture_2d<f32>;
@group(0) @binding(1) var s_backdrop: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4f,
    @location(0) color: vec4f,
    @location(1) clip_circle: vec3f,
}

@vertex
fn vs_main(
    @location(0) position: vec2f,
    @location(1) color: vec4f,
    @location(2) clip_circle: vec3f,
) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4f(position, 0.0, 1.0);
    out.color = color;
    out.clip_circle = clip_circle;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    if (in.clip_circle.z > 0.0) {
        let dx = in.clip_position.x - in.clip_circle.x;
        let dy = in.clip_position.y - in.clip_circle.y;
        if (dx * dx + dy * dy > in.clip_circle.z * in.clip_circle.z) {
            discard;
        }
    }
    
    if (in.color.a < 0.0) {
        let tex_size = vec2f(textureDimensions(t_backdrop));
        let clean_backdrop = textureSample(t_backdrop, s_backdrop, in.clip_position.xy / tex_size);
        
        var blurred = vec4f(0.0);
        var total_weight = 0.0;
        
        // 7x7 Gaussian blur kernel
        for (var x = -3.0; x <= 3.0; x += 1.0) {
            for (var y = -3.0; y <= 3.0; y += 1.0) {
                let offset = vec2f(x, y) * 2.0; // sample every 2 pixels for a wider blur
                let sample_uv = (in.clip_position.xy + offset) / tex_size;
                let weight = exp(-(x*x + y*y) / (2.0 * 2.0 * 2.0));
                blurred += textureSample(t_backdrop, s_backdrop, sample_uv) * weight;
                total_weight += weight;
            }
        }
        
        let backdrop_color = blurred / total_weight;
        let opacity = -in.color.a;
        let plate_color = vec4f(in.color.rgb, 1.0);
        let blurred_plate = mix(backdrop_color, plate_color, opacity);
        return mix(clean_backdrop, blurred_plate, opacity);
    }
    
    return in.color;
}
