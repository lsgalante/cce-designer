//! Signed distance fields, and the way back to a surface.
//!
//! The representation the manufacturing work stands on. Shelling a shape,
//! offsetting it, cutting one shape out of another — none of those are natural
//! on a triangle mesh, where they mean finding every self-intersection the
//! operation creates and stitching the result back into something closed. On a
//! distance field they are arithmetic: offsetting is subtraction, union is a
//! minimum, difference is a maximum against a negation. The mesh comes back at
//! the end, closed by construction.
//!
//! ## Dense, not sparse
//!
//! A dense grid over the shape's bounding box, not a sparse tree. At the sizes
//! this tool works on — a thing you can hold, meshed finely enough to print — a
//! 128³ grid is eight megabytes and answers every query by indexing. A sparse
//! structure buys memory back on volumes mostly made of empty space, and costs
//! a tree walk on every one of the millions of lookups the surface extraction
//! makes. The day a model needs 512³ is the day to write one.
//!
//! ## Two passes, because sign and distance are different questions
//!
//! Building the field from a mesh is done twice over:
//!
//! 1. **Distance** comes from the triangle grid — the closest point on the
//!    surface, which is exact and needs no assumptions about the mesh.
//! 2. **Sign** applies only to a CLOSED surface — an open one has no inside,
//!    and is left unsigned so that offsetting thickens it into a slab. Where
//!    there is an inside, it comes from two tests, each used where it is the
//!    accurate one: a
//!    flood outward from the grid boundary decides everything far from the
//!    surface, and the nearest face's normal decides the thin band either side
//!    of it.
//!
//! A sign that is wrong anywhere is a bubble or a hole in the result, where in
//! the Distance node the same mistake was only a slightly wrong number — which
//! is why this is worth two passes and not one ray cast.

use crate::detail::Detail;
use crate::spatial::TriGrid;
use glam::Vec3;

/// A signed distance field on a regular grid. Negative is inside.
#[derive(Clone, Debug)]
pub struct Volume {
    origin: Vec3,
    voxel: f32,
    /// Samples per axis, so the last sample sits at `origin + (dims-1) * voxel`.
    dims: [usize; 3],
    data: Vec<f32>,
}

/// How far from the surface distances are measured, in voxels.
///
/// Beyond this the field records only "further than this", which is all the
/// extraction and the flood fill need. It bounds the work per sample, and it
/// bounds what an offset can do: moving the surface more than this far has
/// nothing to move into, so a node offsetting further must ask for a bigger
/// voxel or accept the clamp.
pub const REACH_VOXELS: f32 = 3.0;

/// How many samples an axis needs to span `extent` at `voxel`, clamped.
fn axis_dims(extent: f32, voxel: f32) -> usize {
    ((extent / voxel).ceil() as usize + 3).clamp(2, 256)
}

impl Volume {
    pub fn dims(&self) -> [usize; 3] {
        self.dims
    }

    pub fn voxel(&self) -> f32 {
        self.voxel
    }

    fn index(&self, i: usize, j: usize, k: usize) -> usize {
        (k * self.dims[1] + j) * self.dims[0] + i
    }

    pub fn at(&self, i: usize, j: usize, k: usize) -> f32 {
        self.data[self.index(i, j, k)]
    }

    /// The world position of a sample. Public so a caller can check a field
    /// against the shape it was built from.
    pub fn sample_position(&self, i: usize, j: usize, k: usize) -> Vec3 {
        self.position(i, j, k)
    }

    fn position(&self, i: usize, j: usize, k: usize) -> Vec3 {
        self.origin + Vec3::new(i as f32, j as f32, k as f32) * self.voxel
    }

    /// The bounds a mesh needs, with room for the offset a caller will apply.
    ///
    /// Padding matters: a field built tight to the surface has no room to
    /// dilate into, and the offset surface would be clipped flat at the edge of
    /// the grid rather than rounded.
    pub fn bounds_for(d: &Detail, padding: f32) -> Option<(Vec3, Vec3)> {
        let (lo, hi) = d.bounds()?;
        Some((lo - Vec3::splat(padding), hi + Vec3::splat(padding)))
    }

    /// Sample a mesh into a field over the given bounds.
    ///
    /// Two meshes sampled over the SAME bounds at the same voxel size share a
    /// grid, which is what lets the booleans be elementwise.
    pub fn from_mesh_in(d: &Detail, lo: Vec3, hi: Vec3, voxel: f32) -> Volume {
        Self::build(d, lo, hi, voxel, voxel.max(1e-5) * REACH_VOXELS)
    }

    /// [`Volume::from_mesh_in`] measuring distances out to `reach`.
    ///
    /// Beyond `reach` the field says only "further than this", so it bounds
    /// both the cost and how far an offset can move the surface. A caller that
    /// means to offset by more must say so here.
    pub fn build(d: &Detail, lo: Vec3, hi: Vec3, voxel: f32, reach: f32) -> Volume {
        let voxel = voxel.max(1e-5);
        let reach = reach.max(voxel * 2.0);
        let extent = hi - lo;
        let dims = [
            axis_dims(extent.x, voxel),
            axis_dims(extent.y, voxel),
            axis_dims(extent.z, voxel),
        ];
        // Centred, so the padding is even on both sides rather than piling up
        // wherever the rounding landed.
        let span = Vec3::new(
            (dims[0] - 1) as f32,
            (dims[1] - 1) as f32,
            (dims[2] - 1) as f32,
        ) * voxel;
        let origin = (lo + hi) * 0.5 - span * 0.5;

        let n = dims[0] * dims[1] * dims[2];
        let mut vol = Volume { origin, voxel, dims, data: vec![f32::MAX; n] };
        let grid = TriGrid::build(d);
        if grid.is_empty() {
            // No surface: everything is outside, at a distance nothing will
            // mistake for a crossing.
            vol.data.fill(span.max_element().max(1.0));
            return vol;
        }

        // Pass one: unsigned distance, exact from the triangle grid — but only
        // out to REACH, beyond which the field records "further than this".
        //
        // A volume does not need an exact distance far from the surface: the
        // extraction only looks at the zero crossing and the flood fill only
        // asks "further than the band". Measuring it anyway is what made this
        // slow, because an unbounded nearest-surface query from a corner of the
        // grid gathers every triangle in the mesh. The consequence to know is
        // that an OFFSET larger than the reach is not represented — which is
        // why the reach is taken from the padding, the room the caller asked
        // for in order to offset into.
        for k in 0..dims[2] {
            for j in 0..dims[1] {
                for i in 0..dims[0] {
                    let p = vol.position(i, j, k);
                    let d = grid
                        .closest_within(p, reach)
                        .map(|h| h.distance)
                        .unwrap_or(reach);
                    let idx = vol.index(i, j, k);
                    vol.data[idx] = d;
                }
            }
        }

        // Pass two: sign — but only if there is an inside to speak of.
        //
        // "What is inside this?" only has an answer for a CLOSED surface. A
        // flat disc, a single polygon or a torn mesh has none, and signing one
        // anyway gives whichever side its normals happen to face: the field
        // then reads as a half-space and a boolean against it carves something
        // nobody asked for. Left unsigned, the same disc is the set of points
        // near it — so an offset thickens it into a slab, which is the useful
        // answer and the honest one.
        if !d.is_closed() {
            return vol;
        }

        // Which way the mesh is wound, by the divergence theorem: the signed
        // volume of a closed surface is positive when its faces look outward.
        //
        // The band test below asks the nearest face which side a sample is on,
        // which trusts the winding — and a mesh wound inside out then produces
        // a field whose band signs alternate against the flood fill's, giving
        // a surface riddled with holes. Rather than trust it, measure it once
        // and flip. An imported mesh is not obliged to agree with this app's
        // convention.
        let signed_volume: f32 = grid
            .triangles()
            .iter()
            .map(|t| t[0].dot(t[1].cross(t[2])))
            .sum::<f32>()
            / 6.0;
        let facing = if signed_volume < 0.0 { -1.0 } else { 1.0 };

        // Two tests, each used where it is the accurate one.
        //
        // Ray parity was the obvious choice and is wrong here: a ray along a
        // grid row is axis-aligned, the meshes tessellate on the axes, and a
        // ray through a shared edge hits two triangles at the same point. The
        // parity flips and STAYS flipped for the rest of the row — which shows
        // up as contiguous runs of outside samples reading as inside. The
        // collision resolver already carried a comment about exactly this.
        //
        // Instead:
        //
        // 1. FAR FROM THE SURFACE, flood outward from the grid's boundary,
        //    which is outside by construction. Anything the flood cannot reach
        //    without crossing the surface band is enclosed, however convoluted
        //    the shape. No rays, no epsilons, no degenerate cases.
        // 2. IN THE BAND either side of the surface, ask the nearest face which
        //    way it points. That test is unreliable far away in a concave
        //    shape — the nearest face can be one across the gap — and reliable
        //    within half a voxel of the surface, where the nearest face is the
        //    one the sample is sitting on.
        // A FULL voxel, not a fraction of one. Two samples one voxel apart
        // cannot both be more than a voxel from the surface AND have the
        // surface between them — so flooding only between such samples can
        // never cross it. At 0.75 of a voxel two adjacent samples straddling a
        // thin wall could both qualify, and the flood walked straight through
        // into the interior: a slab came back hollow and a boolean against it
        // cut almost nothing.
        let band = voxel * 1.01;
        let idx_of = |i: usize, j: usize, k: usize| (k * dims[1] + j) * dims[0] + i;

        let mut outside = vec![false; n];
        let mut queue: Vec<(usize, usize, usize)> = Vec::new();
        for k in 0..dims[2] {
            for j in 0..dims[1] {
                for i in 0..dims[0] {
                    let on_boundary = i == 0
                        || j == 0
                        || k == 0
                        || i == dims[0] - 1
                        || j == dims[1] - 1
                        || k == dims[2] - 1;
                    if on_boundary && vol.data[idx_of(i, j, k)] > band {
                        outside[idx_of(i, j, k)] = true;
                        queue.push((i, j, k));
                    }
                }
            }
        }
        while let Some((i, j, k)) = queue.pop() {
            let mut visit = |i: usize, j: usize, k: usize, queue: &mut Vec<(usize, usize, usize)>| {
                let idx = idx_of(i, j, k);
                if !outside[idx] && vol.data[idx] > band {
                    outside[idx] = true;
                    queue.push((i, j, k));
                }
            };
            if i > 0 { visit(i - 1, j, k, &mut queue); }
            if j > 0 { visit(i, j - 1, k, &mut queue); }
            if k > 0 { visit(i, j, k - 1, &mut queue); }
            if i + 1 < dims[0] { visit(i + 1, j, k, &mut queue); }
            if j + 1 < dims[1] { visit(i, j + 1, k, &mut queue); }
            if k + 1 < dims[2] { visit(i, j, k + 1, &mut queue); }
        }

        for k in 0..dims[2] {
            for j in 0..dims[1] {
                for i in 0..dims[0] {
                    let idx = idx_of(i, j, k);
                    let inside = if vol.data[idx] > band {
                        !outside[idx]
                    } else {
                        // In the band: which side of the nearest face.
                        let p = vol.position(i, j, k);
                        match grid.closest_within(p, band * 4.0) {
                            Some(h) => (p - h.point).dot(h.normal) * facing < 0.0,
                            None => false,
                        }
                    };
                    if inside {
                        vol.data[idx] = -vol.data[idx];
                    }
                }
            }
        }
        vol
    }

    /// Sample a mesh into a field sized to it, with room to grow.
    ///
    /// The padding IS the room to offset into, so it is also the distance the
    /// field measures out to — asking for space and then not measuring it
    /// would give an offset nothing to find.
    pub fn from_mesh(d: &Detail, voxel: f32, padding: f32) -> Volume {
        let (lo, hi) = Self::bounds_for(d, padding).unwrap_or((Vec3::ZERO, Vec3::ZERO));
        Self::build(d, lo, hi, voxel, padding)
    }

    /// Move the surface out (positive) or in (negative).
    ///
    /// The whole reason for the representation: an offset on a mesh means
    /// resolving every self-intersection the move creates, and here it is a
    /// subtraction.
    pub fn offset(&mut self, by: f32) {
        for v in self.data.iter_mut() {
            *v -= by;
        }
    }

    /// Whether two fields share a grid, which the booleans require.
    pub fn aligned_with(&self, other: &Volume) -> bool {
        self.dims == other.dims
            && (self.voxel - other.voxel).abs() < 1e-6
            && (self.origin - other.origin).length() < 1e-4
    }

    /// Keep whatever is inside EITHER — the minimum of the two distances.
    pub fn union(&mut self, other: &Volume) {
        self.combine(other, |a, b| a.min(b));
    }

    /// Keep only what is inside BOTH.
    pub fn intersect(&mut self, other: &Volume) {
        self.combine(other, |a, b| a.max(b));
    }

    /// Cut `other` out of this one.
    pub fn subtract(&mut self, other: &Volume) {
        self.combine(other, |a, b| a.max(-b));
    }

    fn combine(&mut self, other: &Volume, f: impl Fn(f32, f32) -> f32) {
        if !self.aligned_with(other) {
            return;
        }
        for (a, b) in self.data.iter_mut().zip(other.data.iter()) {
            *a = f(*a, *b);
        }
    }

    /// Extract the zero surface as a quad mesh, by naive surface nets.
    ///
    /// Surface nets rather than marching cubes: one vertex per cell that the
    /// surface passes through, placed at the average of that cell's edge
    /// crossings, and a quad around each grid edge the surface crosses. It is a
    /// tenth of the code of a correct marching-cubes table, produces quads
    /// rather than slivers, and cannot be got subtly wrong in one of 256 cases
    /// nobody exercises. The triangulation it gives is not beautiful, which
    /// does not matter here: this output goes into the remesher.
    pub fn to_mesh(&self) -> Detail {
        let [nx, ny, nz] = self.dims;
        let mut d = Detail::new();
        if nx < 2 || ny < 2 || nz < 2 {
            return d;
        }

        // One vertex per crossed cell, indexed by cell.
        let cells = (nx - 1) * (ny - 1) * (nz - 1);
        let mut vertex = vec![u32::MAX; cells];
        let cell_index = |i: usize, j: usize, k: usize| (k * (ny - 1) + j) * (nx - 1) + i;

        const CORNERS: [[usize; 3]; 8] = [
            [0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0],
            [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1],
        ];
        const EDGES: [[usize; 2]; 12] = [
            [0, 1], [1, 2], [2, 3], [3, 0],
            [4, 5], [5, 6], [6, 7], [7, 4],
            [0, 4], [1, 5], [2, 6], [3, 7],
        ];

        for k in 0..nz - 1 {
            for j in 0..ny - 1 {
                for i in 0..nx - 1 {
                    let s: Vec<f32> = CORNERS
                        .iter()
                        .map(|c| self.at(i + c[0], j + c[1], k + c[2]))
                        .collect();
                    if s.iter().all(|v| *v < 0.0) || s.iter().all(|v| *v >= 0.0) {
                        continue;
                    }
                    let mut sum = Vec3::ZERO;
                    let mut hits = 0.0f32;
                    for e in EDGES {
                        let (a, b) = (s[e[0]], s[e[1]]);
                        if (a < 0.0) == (b < 0.0) {
                            continue;
                        }
                        // Where along the edge the field is zero. Linear, which
                        // is exactly right for a field that is a distance.
                        let t = a / (a - b);
                        let pa = self.position(
                            i + CORNERS[e[0]][0],
                            j + CORNERS[e[0]][1],
                            k + CORNERS[e[0]][2],
                        );
                        let pb = self.position(
                            i + CORNERS[e[1]][0],
                            j + CORNERS[e[1]][1],
                            k + CORNERS[e[1]][2],
                        );
                        sum += pa + (pb - pa) * t;
                        hits += 1.0;
                    }
                    if hits > 0.0 {
                        vertex[cell_index(i, j, k)] = d.add_point(sum / hits);
                    }
                }
            }
        }

        // One quad per grid edge the surface crosses, joining the four cells
        // around that edge. Winding follows the sign: the face must look from
        // inside to outside, so that the plain cross points away from the
        // solid like every other generator in the app.
        let quad = |a: u32, b: u32, c: u32, e: u32, flip: bool, d: &mut Detail| {
            if [a, b, c, e].iter().any(|&v| v == u32::MAX) {
                return;
            }
            if flip {
                d.add_prim(&[a, b, c, e]);
            } else {
                d.add_prim(&[e, c, b, a]);
            }
        };
        for k in 0..nz - 1 {
            for j in 0..ny - 1 {
                for i in 0..nx - 1 {
                    // +X edge: the four cells sharing it differ in j and k.
                    if j > 0 && k > 0 && (self.at(i, j, k) < 0.0) != (self.at(i + 1, j, k) < 0.0) {
                        let inside = self.at(i, j, k) < 0.0;
                        quad(
                            vertex[cell_index(i, j - 1, k - 1)],
                            vertex[cell_index(i, j, k - 1)],
                            vertex[cell_index(i, j, k)],
                            vertex[cell_index(i, j - 1, k)],
                            inside,
                            &mut d,
                        );
                    }
                    if i > 0 && k > 0 && (self.at(i, j, k) < 0.0) != (self.at(i, j + 1, k) < 0.0) {
                        let inside = self.at(i, j, k) < 0.0;
                        quad(
                            vertex[cell_index(i - 1, j, k - 1)],
                            vertex[cell_index(i, j, k - 1)],
                            vertex[cell_index(i, j, k)],
                            vertex[cell_index(i - 1, j, k)],
                            !inside,
                            &mut d,
                        );
                    }
                    if i > 0 && j > 0 && (self.at(i, j, k) < 0.0) != (self.at(i, j, k + 1) < 0.0) {
                        let inside = self.at(i, j, k) < 0.0;
                        quad(
                            vertex[cell_index(i - 1, j - 1, k)],
                            vertex[cell_index(i, j - 1, k)],
                            vertex[cell_index(i, j, k)],
                            vertex[cell_index(i - 1, j, k)],
                            inside,
                            &mut d,
                        );
                    }
                }
            }
        }
        d
    }
}
