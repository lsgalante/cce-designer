//! The Embryo node: the seed geometry a simulation starts from.
//!
//! Ported from hou-control's `developer_embryo`, the first of the Developer
//! family's Pre-Simulation operators — "the embryo or seed is created". The
//! HDA is a small network behind two switches, and this is that network in
//! order:
//!
//! 1. **Source** — a polygon sphere of Radius with Base Resolution rows and
//!    columns (`Internal`), or whatever is wired to the node (`Input`).
//! 2. **Method** — `Basic` uses that geometry as is; `Scatter` scatters
//!    Scatter Count points over it, relaxes them apart across the surface,
//!    and wraps them in a convex hull (the HDA's `shrinkwrap`).
//! 3. **Relax** — the Relax SOP on the result's points: spheres of Relax
//!    Radius are pushed apart until they stop overlapping, sliding in each
//!    point's tangent plane unless Relax in 3D Space lets them leave it.
//!    Off by default (zero iterations), as in the HDA.
//! 4. **Subdivide** — Subdivision Depth rounds of this app's `subdivide`,
//!    which splits every triangle into four and does NOT smooth; the HDA
//!    runs OpenSubdiv Catmull-Clark here, which does. Same name, same
//!    parameter, one deliberate difference — see `remesh::subdivide` for
//!    why this app keeps the two operations apart.
//! 5. **Normals last**, as the attribute `N`, the way the Normal node writes
//!    them.
//!
//! The convex hull is the one piece nothing in the app had. It is the
//! incremental algorithm: a starting tetrahedron from the extreme points,
//! then each remaining point in turn either lies inside the hull so far or
//! sees some of its faces — those are removed, and the ring of edges where
//! visible meets hidden (the horizon) is fanned to the new point. Points
//! within a tolerance of a face are treated as inside, which is what the
//! HDA's Remove Inline Points does: a hull with a thousand coplanar slivers
//! is not a better hull.

use crate::detail::{AttribData, AttribValue, Detail};
use crate::spatial::{PointGrid, TriGrid};
use glam::Vec3;

/// Where the seed geometry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Internal,
    Input,
}

impl Source {
    pub fn parse(s: &str) -> Source {
        match s.trim().to_ascii_lowercase().as_str() {
            "input" | "second input" | "second_input" => Source::Input,
            _ => Source::Internal,
        }
    }
}

/// What is done with the seed before the relax and subdivide steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Basic,
    Scatter,
}

impl Method {
    pub fn parse(s: &str) -> Method {
        match s.trim().to_ascii_lowercase().as_str() {
            "scatter" => Method::Scatter,
            _ => Method::Basic,
        }
    }
}

/// The node's parameters, the HDA's in the HDA's order.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbryoParams {
    pub source: Source,
    pub method: Method,
    /// Rows AND columns of the internal sphere.
    pub base_resolution: usize,
    pub radius: f32,
    pub scatter_count: usize,
    pub scatter_seed: f32,
    /// The scatter's own relaxation — the Scatter SOP's Relax Points.
    pub relax_points: bool,
    pub scatter_relax_iterations: usize,
    /// Scales the radius each scattered point pushes with, which is derived
    /// from the area per point.
    pub scale_radii_by: f32,
    pub use_max_radius: bool,
    pub max_radius: f32,
    /// The Relax SOP that follows the method switch; zero is off.
    pub relax_iterations: usize,
    pub relax_radius: f32,
    pub relax_in_3d: bool,
    pub subdivision_depth: usize,
}

impl Default for EmbryoParams {
    /// The HDA's defaults, which are also the template's.
    fn default() -> Self {
        EmbryoParams {
            source: Source::Internal,
            method: Method::Basic,
            base_resolution: 50,
            radius: 0.5,
            scatter_count: 1000,
            scatter_seed: 1.1,
            relax_points: true,
            scatter_relax_iterations: 50,
            scale_radii_by: 1.248,
            use_max_radius: true,
            max_radius: 10.0,
            relax_iterations: 0,
            relax_radius: 1.0,
            relax_in_3d: false,
            subdivision_depth: 0,
        }
    }
}

/// A xorshift32, seeded from the float the parameter carries.
///
/// The Scatter Seed is a float because Houdini's is; two seeds that differ
/// in any digit give different bit patterns, and that is all a seed needs.
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

/// The seed geometry: the whole pipeline above.
///
/// `input` is consulted only for [`Source::Input`]; with that source and no
/// input there is nothing to seed from, and the node produces nothing rather
/// than quietly substituting the sphere.
pub fn embryo(input: Option<&Detail>, p: &EmbryoParams) -> Option<Detail> {
    let seed = match p.source {
        Source::Internal => {
            let res = p.base_resolution.clamp(3, 128);
            crate::geometry::sphere_detail(Vec3::ZERO, p.radius.max(1e-4), res, res)
        }
        Source::Input => input?.clone(),
    };

    let mut geom = match p.method {
        Method::Basic => seed,
        Method::Scatter => {
            let mut pts = scatter_on_surface(&seed, p.scatter_count, p.scatter_seed);
            if p.relax_points && p.scatter_relax_iterations > 0 && !pts.is_empty() {
                // The Scatter SOP derives each point's radius from the area it
                // has to itself: spheres of that radius roughly tile the
                // surface, so pushing them apart until they stop overlapping
                // spreads the points evenly. Scale Radii By tunes how hard
                // they push; the HDA's 1.248 is what its author settled on.
                let per_point = surface_area(&seed) / pts.len().max(1) as f32;
                let mut radius = p.scale_radii_by * (per_point / std::f32::consts::PI).sqrt();
                if p.use_max_radius {
                    radius = radius.min(p.max_radius);
                }
                let grid = TriGrid::build(&seed);
                relax_on_surface(&mut pts, &grid, radius, p.scatter_relax_iterations);
            }
            // A scatter with nothing to hull — an empty input — is empty,
            // and a hull that cannot be built (every point coplanar) keeps
            // the points as points so what went in is at least visible.
            match convex_hull(&pts) {
                Some(hull) => hull,
                None => {
                    let mut d = Detail::new();
                    for q in &pts {
                        d.add_point(*q);
                    }
                    d
                }
            }
        }
    };

    if p.relax_iterations > 0 && p.relax_radius > 0.0 && geom.num_points() > 1 {
        let normals = if p.relax_in_3d { None } else { Some(crate::geometry::point_normals(&geom)) };
        let mut pts: Vec<Vec3> = (0..geom.num_points()).map(|i| geom.pos(i)).collect();
        relax_points(&mut pts, normals.as_deref(), p.relax_radius, p.relax_iterations);
        for (i, q) in pts.into_iter().enumerate() {
            geom.set_pos(i, q);
        }
    }

    if p.subdivision_depth > 0 {
        geom = crate::remesh::subdivide(&geom, p.subdivision_depth);
    }

    if geom.num_prims() > 0 {
        let n: Vec<[f32; 3]> = crate::geometry::point_normals(&geom).iter().map(|v| v.to_array()).collect();
        geom.points_mut().create("N", AttribValue::Float3([0.0; 3]));
        let _ = geom.points_mut().insert("N", AttribData::Float3(n));
    }
    Some(geom)
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
/// embryo.
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

/// The convex hull of `points`, as a closed triangle mesh over only the
/// points that lie on it — `None` when the points do not span a volume.
pub fn convex_hull(points: &[Vec3]) -> Option<Detail> {
    let faces = hull_faces(points)?;
    let mut remap = vec![u32::MAX; points.len()];
    let mut d = Detail::new();
    for f in &faces {
        let mut ids = [0u32; 3];
        for (k, &pi) in f.iter().enumerate() {
            if remap[pi] == u32::MAX {
                remap[pi] = d.add_point(points[pi]);
            }
            ids[k] = remap[pi];
        }
        d.add_prim(&ids);
    }
    Some(d)
}

/// The hull's faces as index triples into `points`, wound outward.
fn hull_faces(points: &[Vec3]) -> Option<Vec<[usize; 3]>> {
    if points.len() < 4 {
        return None;
    }
    let (lo, hi) = points
        .iter()
        .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    let diag = (hi - lo).length();
    if !(diag > 0.0) {
        return None;
    }
    // "On the face" and "no volume" are both judged against the cloud's own
    // size, so the hull of a millimetre embryo and of a metre one build the
    // same way.
    let eps = diag * 1e-5;

    // The starting tetrahedron: the two points furthest apart along an axis,
    // the point furthest from that line, the point furthest from that plane.
    let mut ext = [0usize; 6];
    for (i, p) in points.iter().enumerate() {
        for a in 0..3 {
            if p[a] < points[ext[a]][a] {
                ext[a] = i;
            }
            if p[a] > points[ext[a + 3]][a] {
                ext[a + 3] = i;
            }
        }
    }
    let (mut i0, mut i1, mut best) = (0, 0, -1.0);
    for &a in &ext {
        for &b in &ext {
            let d = (points[a] - points[b]).length();
            if d > best {
                best = d;
                i0 = a;
                i1 = b;
            }
        }
    }
    if best <= eps {
        return None;
    }
    let dir = (points[i1] - points[i0]).normalize();
    let (mut i2, mut best) = (0, -1.0);
    for (i, &p) in points.iter().enumerate() {
        let off = p - points[i0];
        let d = (off - dir * off.dot(dir)).length();
        if d > best {
            best = d;
            i2 = i;
        }
    }
    if best <= eps {
        return None;
    }
    let n = (points[i1] - points[i0]).cross(points[i2] - points[i0]).normalize();
    let (mut i3, mut best) = (0, -1.0);
    for (i, &p) in points.iter().enumerate() {
        let d = (p - points[i0]).dot(n).abs();
        if d > best {
            best = d;
            i3 = i;
        }
    }
    if best <= eps {
        return None;
    }

    // Wind the four faces so every normal points away from the centroid.
    let centroid = (points[i0] + points[i1] + points[i2] + points[i3]) / 4.0;
    let mut faces: Vec<[usize; 3]> = Vec::new();
    for f in [[i0, i1, i2], [i0, i1, i3], [i0, i2, i3], [i1, i2, i3]] {
        let n = face_normal(points, f);
        if (points[f[0]] - centroid).dot(n) < 0.0 {
            faces.push([f[0], f[2], f[1]]);
        } else {
            faces.push(f);
        }
    }

    let mut edges: Vec<(usize, usize)> = Vec::new();
    for (pi, &p) in points.iter().enumerate() {
        if pi == i0 || pi == i1 || pi == i2 || pi == i3 {
            continue;
        }
        // The faces this point looks at from outside.
        let visible: Vec<bool> = faces
            .iter()
            .map(|f| (p - points[f[0]]).dot(face_normal(points, *f)) > eps)
            .collect();
        if !visible.iter().any(|&v| v) {
            continue;
        }
        // The horizon: directed edges of visible faces whose reverse is not
        // an edge of a visible face. The winding of the visible face gives
        // the new face's winding for free.
        edges.clear();
        for (f, &v) in faces.iter().zip(&visible) {
            if v {
                edges.push((f[0], f[1]));
                edges.push((f[1], f[2]));
                edges.push((f[2], f[0]));
            }
        }
        let horizon: Vec<(usize, usize)> =
            edges.iter().copied().filter(|&(a, b)| !edges.contains(&(b, a))).collect();
        let mut kept: Vec<[usize; 3]> =
            faces.iter().zip(&visible).filter(|(_, &v)| !v).map(|(f, _)| *f).collect();
        for (a, b) in horizon {
            kept.push([a, b, pi]);
        }
        faces = kept;
    }
    Some(faces)
}

fn face_normal(points: &[Vec3], f: [usize; 3]) -> Vec3 {
    (points[f[1]] - points[f[0]]).cross(points[f[2]] - points[f[0]]).normalize_or_zero()
}
