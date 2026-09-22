//! Points on a surface, and pushing points apart: the Scatter node's
//! Surface mode and the Relax node's Repel mode, which together are what
//! hou-control's Scatter SOP (with Relax Points on) and Relax SOP do.
//!
//! Both came in with the Embryo, which needed them as pipeline steps; they
//! live here as node modes so the Embryo can be a template of nodes rather
//! than a pipeline of its own.

use crate::detail::Detail;
use crate::spatial::{PointGrid, TriGrid};
use glam::Vec3;

/// A xorshift32, seeded from the float the parameter carries.
///
/// The Seed is a float because Houdini's is; two seeds that differ in any
/// digit give different bit patterns, and that is all a seed needs.
struct Rng(u32);

impl Rng {
    fn from_seed(seed: f32) -> Rng {
        let bits = seed.to_bits() ^ 0x9e37_79b9;
        Rng(if bits == 0 { 1 } else { bits })
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// Uniform in [0, 1).
    fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// The surface's triangles, fanned from its primitives.
fn triangles(d: &Detail) -> Vec<[Vec3; 3]> {
    d.triangulate(|pos, _| Vec3::from(pos))
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect()
}

fn tri_area(t: &[Vec3; 3]) -> f32 {
    0.5 * (t[1] - t[0]).cross(t[2] - t[0]).length()
}

pub fn surface_area(d: &Detail) -> f32 {
    triangles(d).iter().map(tri_area).sum()
}

/// `count` points scattered uniformly by area over the surface.
///
/// Uniform by AREA, not by primitive: a triangle is chosen with probability
/// proportional to its area, then a point inside it by the square-root
/// barycentric draw, so a big face gets its share and a sliver gets almost
/// none. Deterministic for a seed, so a scrub or a reload gives the same
/// points.
pub fn scatter_on_surface(surface: &Detail, count: usize, seed: f32) -> Vec<Vec3> {
    let tris = triangles(surface);
    if tris.is_empty() || count == 0 {
        return Vec::new();
    }
    let mut cumulative = Vec::with_capacity(tris.len());
    let mut total = 0.0;
    for t in &tris {
        total += tri_area(t);
        cumulative.push(total);
    }
    if total <= 0.0 {
        return Vec::new();
    }
    let mut rng = Rng::from_seed(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let r = rng.next_f32() * total;
        let i = cumulative.partition_point(|&c| c < r).min(tris.len() - 1);
        let [a, b, c] = tris[i];
        let u = rng.next_f32().sqrt();
        let v = rng.next_f32();
        out.push(a * (1.0 - u) + b * (u * (1.0 - v)) + c * (u * v));
    }
    out
}

/// The radius each scattered point pushes with when relaxing: derived from
/// the area it has to itself, so spheres of that radius roughly tile the
/// surface, scaled by `scale` (the Scatter SOP's Scale Radii By; 1.248 is
/// what hou-control's author settled on) and capped at `max` when given.
pub fn relax_radius(surface: &Detail, count: usize, scale: f32, max: Option<f32>) -> f32 {
    let per_point = surface_area(surface) / count.max(1) as f32;
    let r = scale * (per_point / std::f32::consts::PI).sqrt();
    match max {
        Some(m) => r.min(m),
        None => r,
    }
}

/// One pass of pushing overlapping spheres apart. Returns the displacements
/// rather than applying them, so a caller can constrain them first.
fn repulsion(pts: &[Vec3], radius: f32) -> Vec<Vec3> {
    let mut moves = vec![Vec3::ZERO; pts.len()];
    if radius <= 0.0 || pts.len() < 2 {
        return moves;
    }
    let reach = 2.0 * radius;
    let grid = PointGrid::build(pts, reach.max(1e-6));
    let mut near = Vec::new();
    for (i, &p) in pts.iter().enumerate() {
        grid.within(p, reach, &mut near);
        for &j in &near {
            let j = j as usize;
            if j == i {
                continue;
            }
            let d = p - pts[j];
            let len = d.length();
            if len >= reach {
                continue;
            }
            // Each of the pair moves half the overlap; a coincident pair has
            // no direction to move in, so it is nudged along an axis and the
            // next pass separates it properly.
            let dir = if len > 1e-9 { d / len } else { Vec3::X };
            moves[i] += dir * ((reach - len) * 0.5);
        }
    }
    moves
}

/// Push points apart across a surface: repel, then put every point back on
/// the nearest surface point, `iterations` times.
pub fn relax_on_surface(pts: &mut [Vec3], surface: &TriGrid, radius: f32, iterations: usize) {
    if surface.is_empty() {
        return;
    }
    for _ in 0..iterations {
        let moves = repulsion(pts, radius);
        for (p, m) in pts.iter_mut().zip(moves) {
            let moved = *p + m;
            *p = surface.closest(moved).map_or(moved, |h| h.point);
        }
    }
}

/// The Relax SOP: push spheres of `radius` apart. With `normals`, each
/// point's move is flattened into its tangent plane, so a relaxed mesh keeps
/// its shape and only its points slide; without, points move freely.
pub fn relax_points(pts: &mut [Vec3], normals: Option<&[Vec3]>, radius: f32, iterations: usize) {
    for _ in 0..iterations {
        let moves = repulsion(pts, radius);
        for (i, (p, mut m)) in pts.iter_mut().zip(moves).enumerate() {
            if let Some(n) = normals.and_then(|ns| ns.get(i)) {
                if n.length_squared() > 0.0 {
                    m -= *n * m.dot(*n);
                }
            }
            *p += m;
        }
    }
}
