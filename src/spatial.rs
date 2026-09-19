//! Uniform grids for the "what is near this?" questions.
//!
//! Three operators want one of these and all three arrived at once, which is
//! why it is built here rather than inside any of them: the remesher's
//! projection pass asks for the closest point on a surface, Detangle asks
//! which points are within a thickness, and Suture asks both.
//!
//! A uniform grid rather than a BVH because the geometry these run on is
//! already near-uniform — that is what remeshing is for — and a grid sized to
//! the mesh's own scale has no degenerate case on it. A surface with wildly
//! varying triangle sizes would want a tree, and that is the day to write one.

use crate::detail::Detail;
use glam::Vec3;

/// The closest point to `p` on triangle `(a, b, c)`.
///
/// The Voronoi-region walk from Ericson's *Real-Time Collision Detection*
/// §5.1.5: test the three vertex regions, then the three edge regions, and
/// what is left is the face interior.
pub fn closest_point_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let denom = d1 - d3;
        let v = if denom.abs() < 1e-20 { 0.0 } else { d1 / denom };
        return a + ab * v;
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let denom = d2 - d6;
        let w = if denom.abs() < 1e-20 { 0.0 } else { d2 / denom };
        return a + ac * w;
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let denom = (d4 - d3) + (d5 - d6);
        let w = if denom.abs() < 1e-20 { 0.0 } else { (d4 - d3) / denom };
        return b + (c - b) * w;
    }
    let denom = va + vb + vc;
    if denom.abs() < 1e-20 {
        return a;
    }
    a + ab * (vb / denom) + ac * (vc / denom)
}

/// Where a ray meets a triangle, as a distance along the ray.
///
/// Möller–Trumbore. Lives here beside the other spatial queries because three
/// callers want it now: Collision's inside test, and the volume builder's sign
/// pass, which casts one ray per grid row.
pub fn ray_triangle(origin: Vec3, dir: Vec3, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    let edge1 = v1 - v0;
    let edge2 = v2 - v0;
    let h = dir.cross(edge2);
    let a = edge1.dot(h);
    if a.abs() < 1e-6 {
        return None;
    }
    let f = 1.0 / a;
    let s = origin - v0;
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = f * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * edge2.dot(q);
    (t > 1e-5).then_some(t)
}

/// Where things are, bucketed by cell.
///
/// Shared by both grids: they differ only in what they store and how they
/// answer, not in how they divide space.
struct Grid {
    min: Vec3,
    cell: f32,
    dims: [i32; 3],
    buckets: Vec<Vec<u32>>,
}

impl Grid {
    /// A grid over `bounds` with cells about `cell` across, capped so that a
    /// pathological request cannot ask for a billion buckets.
    fn new(min: Vec3, max: Vec3, cell: f32) -> Grid {
        let span = (max - min).max(Vec3::splat(1e-6));
        let cell = cell.max(span.max_element() / 128.0).max(1e-6);
        let dims = [
            ((span.x / cell).ceil() as i32 + 1).clamp(1, 256),
            ((span.y / cell).ceil() as i32 + 1).clamp(1, 256),
            ((span.z / cell).ceil() as i32 + 1).clamp(1, 256),
        ];
        let n = (dims[0] * dims[1] * dims[2]) as usize;
        Grid { min, cell, dims, buckets: vec![Vec::new(); n] }
    }

    fn coord(&self, p: Vec3) -> [i32; 3] {
        let rel = (p - self.min) / self.cell;
        [
            (rel.x.floor() as i32).clamp(0, self.dims[0] - 1),
            (rel.y.floor() as i32).clamp(0, self.dims[1] - 1),
            (rel.z.floor() as i32).clamp(0, self.dims[2] - 1),
        ]
    }

    fn index(&self, c: [i32; 3]) -> usize {
        ((c[2] * self.dims[1] + c[1]) * self.dims[0] + c[0]) as usize
    }

    fn insert(&mut self, p: Vec3, id: u32) {
        let i = self.index(self.coord(p));
        self.buckets[i].push(id);
    }

    /// Everything in the cells overlapping the box, deduplicated.
    fn gather(&self, lo: Vec3, hi: Vec3, out: &mut Vec<u32>) {
        out.clear();
        let (a, b) = (self.coord(lo), self.coord(hi));
        for z in a[2]..=b[2] {
            for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    out.extend_from_slice(&self.buckets[self.index([x, y, z])]);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// What a surface lookup found.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub point: Vec3,
    pub distance: f32,
    /// The face normal of the triangle the hit is on, normalized. Lets a
    /// caller tell inside from outside in constant time — the sign of
    /// `(p - point) . normal` — instead of casting a ray through the whole
    /// mesh. It reads the wrong way in a concave crease, where the nearest
    /// face is not the one facing you, which is why the node that uses it
    /// says so.
    pub normal: Vec3,
}

/// A grid over a surface's triangles, for asking what the nearest surface
/// point is.
pub struct TriGrid {
    tris: Vec<[Vec3; 3]>,
    grid: Grid,
}

impl TriGrid {
    pub fn build(d: &Detail) -> TriGrid {
        let tris: Vec<[Vec3; 3]> = d
            .triangulate(|pos, _| Vec3::from(pos))
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect();
        let (min, max) = d.bounds().unwrap_or((Vec3::ZERO, Vec3::ZERO));
        // Cells about the size of a triangle: small enough that a cell holds
        // few, large enough that one triangle spans few.
        let mean = if tris.is_empty() {
            1.0
        } else {
            tris.iter()
                .map(|t| (t[1] - t[0]).length().max((t[2] - t[0]).length()))
                .sum::<f32>()
                / tris.len() as f32
        };
        let mut grid = Grid::new(min, max, mean.max(1e-5));
        // A triangle goes in every cell its bounding box touches, so a lookup
        // that finds a cell finds every triangle passing through it.
        for (i, t) in tris.iter().enumerate() {
            let lo = t[0].min(t[1]).min(t[2]);
            let hi = t[0].max(t[1]).max(t[2]);
            let (a, b) = (grid.coord(lo), grid.coord(hi));
            for z in a[2]..=b[2] {
                for y in a[1]..=b[1] {
                    for x in a[0]..=b[0] {
                        let idx = grid.index([x, y, z]);
                        grid.buckets[idx].push(i as u32);
                    }
                }
            }
        }
        TriGrid { tris, grid }
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// The triangles behind this grid, for a caller that needs them directly —
    /// the volume builder's scanline sign pass casts rays at all of them.
    pub fn triangles(&self) -> &[[Vec3; 3]] {
        &self.tris
    }

    /// The closest point on the surface, and its distance.
    ///
    /// Searches an expanding box until the best hit is closer than the box is
    /// wide — at which point nothing outside can beat it, because anything out
    /// there is at least that far away.
    pub fn closest(&self, p: Vec3) -> Option<Hit> {
        self.closest_within(p, f32::INFINITY)
    }

    /// [`TriGrid::closest`], giving up once the search passes `limit`.
    ///
    /// The unbounded form doubles its reach until it finds something, so a
    /// query far from the surface ends up gathering every triangle in the mesh
    /// and sorting them — which is fine for the handful of queries an operator
    /// makes and ruinous for the hundred thousand a volume build makes, where
    /// most samples are nowhere near the surface. A caller that only needs to
    /// know "further than this" says so and pays for a few cells.
    pub fn closest_within(&self, p: Vec3, limit: f32) -> Option<Hit> {
        if self.tris.is_empty() {
            return None;
        }
        if limit.is_finite() {
            // ONE gather of exactly the box asked for, rather than doubling up
            // to it: a bounded query knows how far it cares about, and growing
            // into that size in stages means gathering and sorting the same
            // cells over and over. This is the difference between a volume
            // build taking thirty seconds and taking two.
            let mut scratch = Vec::new();
            self.grid
                .gather(p - Vec3::splat(limit), p + Vec3::splat(limit), &mut scratch);
            return scratch
                .iter()
                .map(|&i| {
                    let t = self.tris[i as usize];
                    let q = closest_point_on_triangle(p, t[0], t[1], t[2]);
                    Hit {
                        point: q,
                        distance: (q - p).length(),
                        normal: (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero(),
                    }
                })
                .min_by(|a, b| {
                    a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal)
                })
                .filter(|h| h.distance <= limit);
        }
        let hit = |i: usize| {
            let t = self.tris[i];
            let q = closest_point_on_triangle(p, t[0], t[1], t[2]);
            Hit {
                point: q,
                distance: (q - p).length(),
                normal: (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero(),
            }
        };
        let nearer = |a: &Hit, b: &Hit| {
            a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal)
        };

        let mut reach = self.grid.cell;
        let mut scratch = Vec::new();
        for _ in 0..12 {
            self.grid
                .gather(p - Vec3::splat(reach), p + Vec3::splat(reach), &mut scratch);
            let best = scratch.iter().map(|&i| hit(i as usize)).min_by(nearer);
            match best {
                Some(h) if h.distance <= reach => return Some(h),
                _ => reach *= 2.0,
            }
        }
        // The grid is clamped to the geometry's bounds, so twelve doublings
        // have gathered everything; whatever it found is the answer.
        (0..self.tris.len()).map(hit).min_by(nearer)
    }
}

/// A grid over a point set, for asking which points are near a place.
pub struct PointGrid {
    points: Vec<Vec3>,
    grid: Grid,
}

impl PointGrid {
    pub fn build(points: &[Vec3], cell: f32) -> PointGrid {
        let (min, max) = points.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        );
        let (min, max) = if points.is_empty() { (Vec3::ZERO, Vec3::ZERO) } else { (min, max) };
        let mut grid = Grid::new(min, max, cell);
        for (i, &p) in points.iter().enumerate() {
            grid.insert(p, i as u32);
        }
        PointGrid { points: points.to_vec(), grid }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The nearest point, and its distance.
    ///
    /// Expands the search box until the best hit is closer than the box is
    /// wide, the same argument [`TriGrid::closest`] makes: anything outside a
    /// box that wide is at least that far away, so nothing out there can beat
    /// what is already in hand.
    pub fn nearest(&self, p: Vec3) -> Option<(u32, f32)> {
        if self.points.is_empty() {
            return None;
        }
        let mut reach = self.grid.cell;
        let mut scratch = Vec::new();
        for _ in 0..12 {
            self.grid
                .gather(p - Vec3::splat(reach), p + Vec3::splat(reach), &mut scratch);
            let best = scratch
                .iter()
                .map(|&i| (i, (self.points[i as usize] - p).length()))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            match best {
                Some(hit) if hit.1 <= reach => return Some(hit),
                _ => reach *= 2.0,
            }
        }
        self.points
            .iter()
            .enumerate()
            .map(|(i, q)| (i as u32, (*q - p).length()))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// Indices of points within `radius` of `p`, excluding nothing — the
    /// caller decides what does not count as a neighbour, because "not
    /// itself" and "not topologically adjacent" are different questions.
    pub fn within(&self, p: Vec3, radius: f32, out: &mut Vec<u32>) {
        let mut scratch = Vec::new();
        self.grid
            .gather(p - Vec3::splat(radius), p + Vec3::splat(radius), &mut scratch);
        let r2 = radius * radius;
        out.clear();
        out.extend(
            scratch
                .into_iter()
                .filter(|&i| (self.points[i as usize] - p).length_squared() <= r2),
        );
    }
}
