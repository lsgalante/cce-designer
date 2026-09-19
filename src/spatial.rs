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

    /// The closest point on the surface, and its distance.
    ///
    /// Searches an expanding box until the best hit is closer than the box is
    /// wide — at which point nothing outside can beat it, because anything out
    /// there is at least that far away.
    pub fn closest(&self, p: Vec3) -> Option<Hit> {
        if self.tris.is_empty() {
            return None;
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
