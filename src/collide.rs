//! The Collision node's test — is this point inside the collider, or within
//! Distance of its surface — on the CPU and on the GPU. The second Phase 7
//! step 4 operator, and the one the GPU is made for: every query walks EVERY
//! collider triangle (the CPU test has always been that brute-force loop),
//! so the work is queries x triangles, per-point, and embarrassingly
//! parallel, with one dispatch and no passes to chain.
//!
//! Same algorithm on both sides — the Voronoi-region point-triangle distance
//! and the Möller–Trumbore ray test, step for step — held together by
//! `collision_gpu_matches_cpu`. The layout is the GPU's: queries as a flat
//! `xyz` array, triangles as nine floats each, one `u32` flag out.

use cce_ui::vk::{Binding, ComputeDevice, Kernel};
use glam::Vec3;

/// Which test the node runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Test {
    /// Enclosed by the collider's volume: parity of a ray cast's crossings.
    Inside,
    /// Within this distance of the collider's surface.
    Proximity(f32),
}

/// Fixed irrational-ish ray, NOT axis-aligned: the template meshes
/// tessellate on the axes, and a ray along one skims edge-on through whole
/// fans of triangles, double-counting crossings.
pub fn ray_dir() -> Vec3 {
    Vec3::new(0.9174771, 0.3369154, 0.2095338).normalize()
}

/// The CPU test: one flag per query, walking every triangle.
pub fn hits_cpu(queries: &[Vec3], tris: &[[Vec3; 3]], test: Test) -> Vec<u32> {
    let dir = ray_dir();
    queries
        .iter()
        .map(|&p| match test {
            Test::Proximity(d) => {
                let d2 = d * d;
                u32::from(tris.iter().any(|t| crate::geometry::point_triangle_distance_sq(p, t[0], t[1], t[2]) <= d2))
            }
            Test::Inside => {
                let crossings = tris.iter().filter(|t| crate::spatial::ray_triangle(p, dir, t[0], t[1], t[2]).is_some()).count();
                (crossings % 2 == 1) as u32
            }
        })
        .collect()
}

/// The same test as a WGSL kernel: one invocation per query.
pub const COLLIDE_WGSL: &str = r#"
struct Params { n: u32, m: u32, mode: u32, pad: u32, d2: f32, rx: f32, ry: f32, rz: f32 }
@group(0) @binding(0) var<storage, read> queries: array<f32>;
@group(0) @binding(1) var<storage, read> tris: array<f32>;
@group(0) @binding(2) var<storage, read_write> hits: array<u32>;
@group(0) @binding(3) var<uniform> params: Params;

// Squared distance from p to triangle (a, b, c): the Voronoi-region walk.
fn dist_sq(p: vec3<f32>, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>) -> f32 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if (d1 <= 0.0 && d2 <= 0.0) { return dot(ap, ap); }
    let bp = p - b;
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if (d3 >= 0.0 && d4 <= d3) { return dot(bp, bp); }
    let vc = d1 * d4 - d3 * d2;
    if (vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0) {
        let v = d1 / (d1 - d3);
        let e = ap - ab * v;
        return dot(e, e);
    }
    let cp = p - c;
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if (d6 >= 0.0 && d5 <= d6) { return dot(cp, cp); }
    let vb = d5 * d2 - d1 * d6;
    if (vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0) {
        let w = d2 / (d2 - d6);
        let e = ap - ac * w;
        return dot(e, e);
    }
    let va = d3 * d6 - d5 * d4;
    if (va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0) {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let e = bp - (c - b) * w;
        return dot(e, e);
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    let e = ap - ab * v - ac * w;
    return dot(e, e);
}

// Möller–Trumbore, as spatial::ray_triangle: a hit strictly ahead of the origin.
fn ray_hits(o: vec3<f32>, d: vec3<f32>, v0: vec3<f32>, v1: vec3<f32>, v2: vec3<f32>) -> bool {
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let h = cross(d, edge2);
    let a = dot(edge1, h);
    if (abs(a) < 1e-6) { return false; }
    let f = 1.0 / a;
    let s = o - v0;
    let u = f * dot(s, h);
    if (u < 0.0 || u > 1.0) { return false; }
    let q = cross(s, edge1);
    let v = f * dot(d, q);
    if (v < 0.0 || u + v > 1.0) { return false; }
    let t = f * dot(edge2, q);
    return t > 1e-5;
}

@compute @workgroup_size(64)
fn collide(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= params.n) { return; }
    let p = vec3<f32>(queries[i * 3u], queries[i * 3u + 1u], queries[i * 3u + 2u]);
    var hit = 0u;
    if (params.mode == 1u) {
        for (var t = 0u; t < params.m; t = t + 1u) {
            let b = t * 9u;
            let a0 = vec3<f32>(tris[b], tris[b + 1u], tris[b + 2u]);
            let a1 = vec3<f32>(tris[b + 3u], tris[b + 4u], tris[b + 5u]);
            let a2 = vec3<f32>(tris[b + 6u], tris[b + 7u], tris[b + 8u]);
            if (dist_sq(p, a0, a1, a2) <= params.d2) { hit = 1u; break; }
        }
    } else {
        let d = vec3<f32>(params.rx, params.ry, params.rz);
        var crossings = 0u;
        for (var t = 0u; t < params.m; t = t + 1u) {
            let b = t * 9u;
            let a0 = vec3<f32>(tris[b], tris[b + 1u], tris[b + 2u]);
            let a1 = vec3<f32>(tris[b + 3u], tris[b + 4u], tris[b + 5u]);
            let a2 = vec3<f32>(tris[b + 6u], tris[b + 7u], tris[b + 8u]);
            if (ray_hits(p, d, a0, a1, a2)) { crossings = crossings + 1u; }
        }
        hit = crossings & 1u;
    }
    hits[i] = hit;
}"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    n: u32,
    m: u32,
    mode: u32,
    pad: u32,
    d2: f32,
    rx: f32,
    ry: f32,
    rz: f32,
}

/// The GPU test: one dispatch, one flag per query.
pub fn hits_gpu(dev: &mut ComputeDevice, queries: &[Vec3], tris: &[[Vec3; 3]], test: Test) -> Result<Vec<u32>, String> {
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    let q: Vec<f32> = queries.iter().flat_map(|p| [p.x, p.y, p.z]).collect();
    let t: Vec<f32> = tris.iter().flat_map(|t| [t[0].x, t[0].y, t[0].z, t[1].x, t[1].y, t[1].z, t[2].x, t[2].y, t[2].z]).collect();
    let dir = ray_dir();
    let params = Params {
        n: queries.len() as u32,
        m: tris.len() as u32,
        mode: match test {
            Test::Inside => 0,
            Test::Proximity(_) => 1,
        },
        pad: 0,
        d2: match test {
            Test::Proximity(d) => d * d,
            Test::Inside => 0.0,
        },
        rx: dir.x,
        ry: dir.y,
        rz: dir.z,
    };
    let mut hits = vec![0u32; queries.len()];
    dev.run_over(
        &Kernel::new(COLLIDE_WGSL, "collide"),
        &mut [Binding::input(&q), Binding::input(&t), Binding::rw(&mut hits), Binding::uniform(&params)],
        queries.len() as u32,
    )?;
    Ok(hits)
}

/// Below this many query-triangle pairs the CPU is faster; in auto mode it
/// takes them. From `collision_timing` in release on an Intel Iris Xe,
/// Proximity: 360k pairs cpu 2.7 ms / gpu 15 ms (that run pays the ~15 ms
/// pipeline compile); 3.6M pairs cpu 20 ms / gpu 2.2 ms; 15M pairs cpu
/// 76 ms / gpu 6.6 ms; 242M pairs cpu 1150 ms / gpu 52 ms. The GPU is
/// ahead from a few hundred thousand pairs up, and by 20x at the sizes
/// where the CPU test stalls the app.
pub const GPU_MIN_WORK: usize = 250_000;

/// The test as the node runs it: the backend chosen by `CCE_COMPUTE` and the
/// amount of work, a GPU failure in auto mode falling back to the CPU with
/// a note and in forced-GPU mode reported back for the node-error slot.
pub fn hits(queries: &[Vec3], tris: &[[Vec3; 3]], test: Test) -> Result<Vec<u32>, String> {
    let work = queries.len().saturating_mul(tris.len());
    if crate::gpu::use_gpu(work, GPU_MIN_WORK) {
        match crate::gpu::with_any_device(|dev| hits_gpu(dev, queries, tris, test)) {
            Ok(Ok(h)) => return Ok(h),
            Ok(Err(e)) | Err(e) => {
                if crate::gpu::choice() == crate::gpu::Choice::Gpu {
                    return Err(format!("CCE_COMPUTE=gpu but the collision test could not run there ({e}); tested on the CPU"));
                }
                note_fallback_once(&e);
            }
        }
    }
    Ok(hits_cpu(queries, tris, test))
}

fn note_fallback_once(e: &str) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| eprintln!("cce-designer: collision test fell back to the CPU: {e}"));
}
