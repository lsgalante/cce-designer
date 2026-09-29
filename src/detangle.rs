//! The Detangle node's solve: push a surface off itself.
//!
//! The ALGORITHM is `geometry::apply_detangle`'s, unchanged, and what it
//! does and why is said there. This module is how it is run: what it costs
//! is a function of four things the first version paid for every step of a
//! simulation, and none of which a step needs to pay for.
//!
//! - **The topology is built once.** The edge list and each point's excluded
//!   neighbourhood are connectivity, and a chain of pull, relax and detangle
//!   never changes connectivity — but every step arrives as a fresh
//!   `Detail`, whose derived topology is deliberately not cloned. They are
//!   kept here by a hash of the primitives ([`Topo`]), so a solve of a
//!   hundred steps builds them on the first.
//! - **A pass that separates nothing ends the solve.** The passes gather
//!   against the positions at their start; if one moves nothing, the next
//!   starts from the same positions and finds the same nothing.
//! - **The grid is built once and reused** while the points stay near where
//!   it filed them, in a flat array sorted by cell rather than a vector per
//!   cell. A point that has moved is still found: the search reaches as far
//!   as anything has moved since the grid was built.
//! - **A point that cannot move is not searched for.** With a Group, what
//!   would push the others was worked out and thrown away.
//!
//! The results are the first version's BIT FOR BIT, which is what lets these
//! be optimizations and not changes: the same pairs, summed in the same
//! order. `the_detangle_solve_matches_its_reference` holds them together.
//!
//! That is the node's `Points` method. Two things here are NOT the first
//! version's, and are asked for by name:
//!
//! - **The `Surface` method** ([`solve_surface`]) tests each point against
//!   the TRIANGLES near it, where Points tests it against points. A point
//!   over the middle of a triangle is near no corner of it, so on a mesh
//!   whose triangles are larger than the thickness the point test sees
//!   nothing at all. Told where the points were when the step began
//!   ([`apply_from`]), it also knows which side of a triangle a point
//!   belongs on, and can hold a step to a length.
//! - **The measure** ([`self_intersections`]): every edge that passes
//!   through a triangle. It is what says whether a change to the solve
//!   helped, and what the node's `Tangled Group` is written from.

use crate::app::FsNode;
use crate::detail::Detail;
use crate::geometry::{node_param_f32, node_param_str};
use glam::Vec3;
use std::cell::RefCell;
use std::rc::Rc;

/// What a mesh's connectivity gives the solve, for one ring count.
struct Topo {
    key: u64,
    rings: usize,
    edges: Vec<[u32; 2]>,
    /// The primitives as triangles, fanned as `Detail::triangulate` fans
    /// them, and the primitive each came from.
    tris: Vec<[u32; 3]>,
    tri_prims: Vec<u32>,
    /// Each point's excluded neighbourhood, itself included, ascending:
    /// point `p`'s is `excluded[starts[p]..starts[p + 1]]`.
    starts: Vec<u32>,
    excluded: Vec<u32>,
}

impl Topo {
    fn build(geom: &Detail, key: u64, rings: usize) -> Topo {
        let n = geom.num_points();
        let mut starts = Vec::with_capacity(n + 1);
        let mut excluded = Vec::new();
        let mut seen: Vec<u32> = Vec::new();
        let mut frontier: Vec<u32> = Vec::new();
        let mut next: Vec<u32> = Vec::new();
        // One mark per point instead of a search of `seen` per neighbour.
        let mut mark = vec![u32::MAX; n];
        for p in 0..n {
            starts.push(excluded.len() as u32);
            seen.clear();
            frontier.clear();
            seen.push(p as u32);
            frontier.push(p as u32);
            mark[p] = p as u32;
            for _ in 0..rings {
                next.clear();
                for &q in &frontier {
                    for &r in geom.point_neighbours(q as usize) {
                        if mark[r as usize] != p as u32 {
                            mark[r as usize] = p as u32;
                            seen.push(r);
                            next.push(r);
                        }
                    }
                }
                if next.is_empty() {
                    break;
                }
                std::mem::swap(&mut frontier, &mut next);
            }
            seen.sort_unstable();
            excluded.extend_from_slice(&seen);
        }
        starts.push(excluded.len() as u32);
        let mut tris = Vec::new();
        let mut tri_prims = Vec::new();
        for prim in 0..geom.num_prims() {
            let pts = geom.prim_points(prim);
            for i in 1..pts.len().saturating_sub(1) {
                tris.push([pts[0], pts[i], pts[i + 1]]);
                tri_prims.push(prim as u32);
            }
        }
        Topo { key, rings, edges: geom.edges().to_vec(), tris, tri_prims, starts, excluded }
    }

    fn excludes(&self, p: usize, q: u32) -> bool {
        let (a, b) = (self.starts[p] as usize, self.starts[p + 1] as usize);
        self.excluded[a..b].binary_search(&q).is_ok()
    }
}

/// How many topologies are kept: a project has a few detangles at most, and
/// each is asked for in turn as a frame's graph is walked.
const KEPT: usize = 4;

thread_local! {
    static TOPOS: RefCell<Vec<Rc<Topo>>> = const { RefCell::new(Vec::new()) };
}

/// The connectivity's identity: the point count and every primitive's
/// points, in order. Positions are not in it.
fn topology_key(geom: &Detail) -> u64 {
    // FNV-1a over the indices: a few thousand words a step, and nothing
    // here needs a keyed hash.
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |v: u32| {
        for b in v.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    eat(geom.num_points() as u32);
    eat(geom.num_prims() as u32);
    for prim in 0..geom.num_prims() {
        let points = geom.prim_points(prim);
        eat(points.len() as u32);
        for &p in points {
            eat(p);
        }
    }
    h
}

fn topo_for(geom: &Detail, rings: usize) -> Rc<Topo> {
    let key = topology_key(geom);
    TOPOS.with(|kept| {
        let mut kept = kept.borrow_mut();
        if let Some(i) = kept.iter().position(|t| t.key == key && t.rings == rings && t.starts.len() == geom.num_points() + 1) {
            // Most recent last, so the one dropped is the one longest unused.
            let hit = kept.remove(i);
            kept.push(hit.clone());
            return hit;
        }
        let built = Rc::new(Topo::build(geom, key, rings));
        if kept.len() >= KEPT {
            kept.remove(0);
        }
        kept.push(built.clone());
        built
    })
}

/// How many topologies are kept right now — for the tests.
#[cfg(test)]
pub(crate) fn kept_topologies() -> usize {
    TOPOS.with(|k| k.borrow().len())
}

/// Points filed by cell, flat: `ids[starts[c]..starts[c + 1]]` are the
/// points the grid filed in cell `c`, ascending.
struct FlatGrid {
    min: Vec3,
    cell: f32,
    dims: [i32; 3],
    starts: Vec<u32>,
    ids: Vec<u32>,
    /// Where each point was when it was filed.
    filed: Vec<Vec3>,
}

impl FlatGrid {
    fn build(points: &[Vec3], cell: f32) -> FlatGrid {
        let (min, max) = points.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        );
        let (min, max) = if points.is_empty() { (Vec3::ZERO, Vec3::ZERO) } else { (min, max) };
        // The shape `spatial::Grid` gives itself: cells about `cell` across,
        // capped so a pathological request cannot ask for a billion of them.
        let span = (max - min).max(Vec3::splat(1e-6));
        let cell = cell.max(span.max_element() / 128.0).max(1e-6);
        let dims = [
            ((span.x / cell).ceil() as i32 + 1).clamp(1, 256),
            ((span.y / cell).ceil() as i32 + 1).clamp(1, 256),
            ((span.z / cell).ceil() as i32 + 1).clamp(1, 256),
        ];
        let mut grid = FlatGrid { min, cell, dims, starts: Vec::new(), ids: Vec::new(), filed: points.to_vec() };
        let cells = (dims[0] * dims[1] * dims[2]) as usize;
        // A counting sort: count, prefix-sum, place. Placing in point order
        // leaves each cell's ids ascending.
        let of: Vec<u32> = points.iter().map(|&p| grid.index(grid.coord(p)) as u32).collect();
        let mut starts = vec![0u32; cells + 1];
        for &c in &of {
            starts[c as usize + 1] += 1;
        }
        for c in 0..cells {
            starts[c + 1] += starts[c];
        }
        let mut next = starts.clone();
        let mut ids = vec![0u32; points.len()];
        for (i, &c) in of.iter().enumerate() {
            ids[next[c as usize] as usize] = i as u32;
            next[c as usize] += 1;
        }
        grid.starts = starts;
        grid.ids = ids;
        grid
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

    /// Every point filed in a cell the box about `p` touches, ascending.
    fn gather(&self, p: Vec3, reach: f32, out: &mut Vec<u32>) {
        out.clear();
        let (a, b) = (self.coord(p - Vec3::splat(reach)), self.coord(p + Vec3::splat(reach)));
        for z in a[2]..=b[2] {
            for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    let c = self.index([x, y, z]);
                    out.extend_from_slice(&self.ids[self.starts[c] as usize..self.starts[c + 1] as usize]);
                }
            }
        }
        // Cells are disjoint, so there is nothing to deduplicate; the order
        // is what the sum over a point's pairs is taken in.
        out.sort_unstable();
    }

    /// How far the furthest point has come from where it was filed.
    fn drift(&self, points: &[Vec3]) -> f32 {
        points.iter().zip(&self.filed).map(|(p, f)| (*p - *f).length_squared()).fold(0.0f32, f32::max).sqrt()
    }
}

/// How far points may drift from where the grid filed them, in cells,
/// before it is built again. The search reaches a drift further than the
/// thickness, so a larger allowance trades rebuilds for wider searches.
const DRIFT_CELLS: f32 = 0.5;

/// What a solve did, for the tests and the timing.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Work {
    pub passes: usize,
    pub grids: usize,
    pub searched: usize,
    /// Point-triangle contacts the Surface method resolved, over every pass.
    pub contacts: usize,
    /// Of those, the ones resolved as a point that had gone THROUGH a
    /// triangle since the step began, and was put back on its own side.
    pub crossed: usize,
    /// Points put back where the step began, being still through a
    /// triangle when the passes were done, or a corner of one.
    pub held: usize,
    /// Points whose move since the step began was cut to the Step Limit.
    pub limited: usize,
    /// Edges passing through a triangle when the solve was done — counted
    /// only when the node names a Tangled Group to write them to.
    pub crossings: usize,
}

pub fn apply(geom: &mut Detail, target: &FsNode) -> Work {
    apply_from(geom, None, target)
}

/// [`apply`], told where the points were when the step began: inside a
/// simnet, the state the substep consumed. It is what the Surface method
/// knows a point's SIDE from, and what the Step Limit is measured against.
/// A `before` that is not this mesh — another point count, other
/// primitives — is no memory of it and is not used.
pub fn apply_from(geom: &mut Detail, before: Option<&Detail>, target: &FsNode) -> Work {
    let mut work = Work::default();
    let n = geom.num_points();
    if n == 0 || geom.num_prims() == 0 {
        return work;
    }
    let rings = node_param_f32(target, "Rings", 2.0).clamp(0.0, 6.0) as usize;
    let topo = topo_for(geom, rings);
    if topo.edges.is_empty() {
        return work;
    }
    // A node without the row is one from before it, and solves as it did.
    if node_param_str(target, "Method", "Points").trim().eq_ignore_ascii_case("Surface") {
        let before: Option<Vec<Vec3>> = before
            .filter(|b| b.num_points() == n && b.num_prims() == geom.num_prims() && topology_key(b) == topo.key)
            .map(|b| (0..n).map(|p| b.pos(p)).collect());
        solve_surface(geom, before.as_deref(), target, &topo, &mut work);
    } else {
        solve_points(geom, target, &topo, &mut work);
    }
    // What is STILL crossed once the solve is done, which is the part worth
    // looking at. Only when asked for: it is a second search of the mesh.
    let mark = node_param_str(target, "Tangled Group", "");
    let mark = mark.trim();
    if !mark.is_empty() {
        let found = intersections_of(geom, &topo, false);
        work.crossings = found.crossings;
        geom.points_mut().create_group(mark);
        for p in found.points {
            geom.points_mut().add_to_group(mark, p as usize);
        }
    }
    work
}

/// The node's thickness as a length: the setting is in EDGE LENGTHS, so it
/// means the same thing before and after a remesh.
fn thickness_of(geom: &Detail, target: &FsNode, topo: &Topo) -> f32 {
    let mean_edge = mean_edge(geom, topo);
    node_param_f32(target, "Thickness", 1.0).max(0.0) * mean_edge
}

fn mean_edge(geom: &Detail, topo: &Topo) -> f32 {
    topo.edges
        .iter()
        .map(|e| (geom.pos(e[1] as usize) - geom.pos(e[0] as usize)).length())
        .sum::<f32>()
        / topo.edges.len() as f32
}

/// The Points method: the first version's solve.
fn solve_points(geom: &mut Detail, target: &FsNode, topo: &Topo, work: &mut Work) {
    let n = geom.num_points();
    let thickness = thickness_of(geom, target, topo);
    if thickness <= 0.0 {
        return;
    }
    let iterations = node_param_f32(target, "Iterations", 4.0).clamp(1.0, 32.0) as usize;
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let movable: Vec<bool> = (0..n).map(|p| group.is_empty() || geom.points().in_group(&group, p)).collect();

    let mut pos: Vec<Vec3> = (0..n).map(|p| geom.pos(p)).collect();
    let mut grid = FlatGrid::build(&pos, thickness);
    work.grids += 1;
    let mut drift = 0.0f32;
    let mut near = Vec::new();
    let mut push = vec![Vec3::ZERO; n];
    let mut moved_at_all = false;
    for _ in 0..iterations {
        work.passes += 1;
        if drift > DRIFT_CELLS * grid.cell {
            grid = FlatGrid::build(&pos, thickness);
            work.grids += 1;
            drift = 0.0;
        }
        // Gathered against the positions at the START of the pass and
        // applied at the end, so the result does not depend on the order
        // points are visited in.
        let mut any = false;
        for p in 0..n {
            push[p] = Vec3::ZERO;
            if !movable[p] {
                continue;
            }
            work.searched += 1;
            // Anything within a thickness of here now was filed within a
            // thickness and a drift of here.
            grid.gather(pos[p], thickness + drift, &mut near);
            for &q in &near {
                let qi = q as usize;
                if qi == p || topo.excludes(p, q) {
                    continue;
                }
                let d = pos[p] - pos[qi];
                let len = d.length();
                if len >= thickness {
                    continue;
                }
                // Two coincident points have no direction to separate along;
                // nudging along an arbitrary axis at least breaks the tie.
                let dir = if len < 1e-9 {
                    Vec3::new((p % 7) as f32 - 3.0, (p % 5) as f32 - 2.0, 1.0).normalize_or_zero()
                } else {
                    d / len
                };
                push[p] += dir * ((thickness - len) * 0.5);
                any = true;
            }
        }
        if !any {
            // Nothing is within a thickness of anything it may push: the
            // next pass would start from these positions and find the same.
            break;
        }
        for p in 0..n {
            if movable[p] {
                pos[p] += push[p];
            }
        }
        moved_at_all = true;
        drift = grid.drift(&pos);
    }
    if moved_at_all {
        for (p, v) in pos.iter().enumerate() {
            geom.set_pos(p, *v);
        }
    }
}

/// Triangles filed by cell, flat, as [`FlatGrid`] files points: a triangle
/// is in every cell its bounding box touched when it was filed.
struct TriCells {
    min: Vec3,
    cell: f32,
    dims: [i32; 3],
    starts: Vec<u32>,
    ids: Vec<u32>,
    /// Where each POINT was when the triangles were filed.
    filed: Vec<Vec3>,
}

impl TriCells {
    fn build(points: &[Vec3], tris: &[[u32; 3]], cell: f32) -> TriCells {
        let (min, max) = points.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), &p| (lo.min(p), hi.max(p)),
        );
        let (min, max) = if points.is_empty() { (Vec3::ZERO, Vec3::ZERO) } else { (min, max) };
        let span = (max - min).max(Vec3::splat(1e-6));
        let cell = cell.max(span.max_element() / 128.0).max(1e-6);
        let dims = [
            ((span.x / cell).ceil() as i32 + 1).clamp(1, 256),
            ((span.y / cell).ceil() as i32 + 1).clamp(1, 256),
            ((span.z / cell).ceil() as i32 + 1).clamp(1, 256),
        ];
        let mut grid = TriCells { min, cell, dims, starts: Vec::new(), ids: Vec::new(), filed: points.to_vec() };
        let cells = (dims[0] * dims[1] * dims[2]) as usize;
        let boxes: Vec<([i32; 3], [i32; 3])> = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| points[i as usize]);
                (grid.coord(a.min(b).min(c)), grid.coord(a.max(b).max(c)))
            })
            .collect();
        // The counting sort again, a triangle counted once per cell it is
        // in. Placing in triangle order leaves each cell's ids ascending.
        let mut starts = vec![0u32; cells + 1];
        for (a, b) in &boxes {
            for z in a[2]..=b[2] {
                for y in a[1]..=b[1] {
                    for x in a[0]..=b[0] {
                        starts[grid.index([x, y, z]) + 1] += 1;
                    }
                }
            }
        }
        for c in 0..cells {
            starts[c + 1] += starts[c];
        }
        let mut next = starts.clone();
        let mut ids = vec![0u32; starts[cells] as usize];
        for (i, (a, b)) in boxes.iter().enumerate() {
            for z in a[2]..=b[2] {
                for y in a[1]..=b[1] {
                    for x in a[0]..=b[0] {
                        let c = grid.index([x, y, z]);
                        ids[next[c] as usize] = i as u32;
                        next[c] += 1;
                    }
                }
            }
        }
        grid.starts = starts;
        grid.ids = ids;
        grid
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

    /// Every triangle filed in a cell the box touches, once, in the order
    /// the cells are walked. `seen` is one mark per triangle and `stamp`
    /// this gather's, which is cheaper than sorting what came back to find
    /// the triangles that came back twice.
    fn gather(&self, lo: Vec3, hi: Vec3, seen: &mut [u32], stamp: u32, out: &mut Vec<u32>) {
        out.clear();
        let (a, b) = (self.coord(lo), self.coord(hi));
        for z in a[2]..=b[2] {
            for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    let c = self.index([x, y, z]);
                    for &t in &self.ids[self.starts[c] as usize..self.starts[c + 1] as usize] {
                        if seen[t as usize] != stamp {
                            seen[t as usize] = stamp;
                            out.push(t);
                        }
                    }
                }
            }
        }
    }

    fn drift(&self, points: &[Vec3]) -> f32 {
        points.iter().zip(&self.filed).map(|(p, f)| (*p - *f).length_squared()).fold(0.0f32, f32::max).sqrt()
    }
}

/// The Surface method: each point against the triangles near it.
///
/// A contact is a point closer than the thickness to a triangle none of
/// whose corners is in the point's excluded rings. It is resolved along the
/// line from the closest point on the triangle to the point — the triangle's
/// own normal where the point lies ON it, which is a direction, where two
/// coincident points have none — and the move is SHARED: the point takes
/// its part one way and the triangle's corners theirs the other, each corner
/// by how much of the closest point it is. What cannot move (outside the
/// Group) takes none and the rest take all of it, so a contact with a fixed
/// triangle is resolved whole, where Points resolves half of it.
///
/// A pass is gathered against the positions at its start and applied at its
/// end, as Points is. What a point receives from several contacts is their
/// AVERAGE, weighted by how deep each is: a point over a shared edge is in
/// contact with both triangles and must move once, not twice, and a sum is
/// what makes a dense contact overshoot and ring.
///
/// **With `before`, a point has a side.** Distance alone cannot tell a
/// point that is near a triangle from one that has gone through it, and
/// pushes the second further through. Where the point was on one side of a
/// triangle when the step began and is on the other now, having passed
/// through the triangle's own extent ([`went_through`]), the contact is
/// resolved along the triangle's normal back to the side it came from, to a
/// thickness clear of it. A point with such a contact takes no other in
/// that pass: the triangles beside the one it went through see it near and
/// on the wrong side, and would push it on.
///
/// The memory is one step long. A point the passes did not bring back is,
/// to the next step, a point that began on that side.
///
/// **The Step Limit** cuts each point's move since the step began to that
/// many thicknesses before anything is resolved, so what arrives here is
/// close to what left and a contact is met while it is still a contact.
fn solve_surface(geom: &mut Detail, before: Option<&[Vec3]>, target: &FsNode, topo: &Topo, work: &mut Work) {
    let n = geom.num_points();
    let thickness = thickness_of(geom, target, topo);
    if thickness <= 0.0 || topo.tris.is_empty() {
        return;
    }
    let iterations = node_param_f32(target, "Iterations", 4.0).clamp(1.0, 32.0) as usize;
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let movable: Vec<bool> = (0..n).map(|p| group.is_empty() || geom.points().in_group(&group, p)).collect();
    let free = |p: usize| if movable[p] { 1.0f32 } else { 0.0 };

    let mut pos: Vec<Vec3> = (0..n).map(|p| geom.pos(p)).collect();
    let mut moved_at_all = false;
    // A node without the row is one from before it, and is not limited.
    let limit = node_param_f32(target, "Step Limit", 0.0).max(0.0) * thickness;
    if let (Some(before), true) = (before, limit > 0.0) {
        for p in (0..n).filter(|&p| movable[p]) {
            let d = pos[p] - before[p];
            let len = d.length();
            if len > limit {
                pos[p] = before[p] + d * (limit / len);
                work.limited += 1;
                moved_at_all = true;
            }
        }
    }
    // How far anything has come since the step began: a triangle a point
    // went through may be that far from where either is now.
    let travelled = before.map_or(0.0, |b| pos.iter().zip(b).map(|(p, q)| (*p - *q).length_squared()).fold(0.0f32, f32::max).sqrt());

    // Cells no smaller than a triangle, or each is filed in dozens.
    let cell = thickness.max(mean_edge(geom, topo));
    let mut grid = TriCells::build(&pos, &topo.tris, cell);
    work.grids += 1;
    let mut drift = 0.0f32;
    let mut near = Vec::new();
    let mut seen = vec![u32::MAX; topo.tris.len()];
    let mut stamp = 0u32;
    let mut push = vec![Vec3::ZERO; n];
    let mut weight = vec![0.0f32; n];
    let mut found: Vec<Contact> = Vec::new();
    for _ in 0..iterations {
        work.passes += 1;
        if drift > DRIFT_CELLS * grid.cell {
            grid = TriCells::build(&pos, &topo.tris, cell);
            work.grids += 1;
            drift = 0.0;
        }
        push.iter_mut().for_each(|v| *v = Vec3::ZERO);
        weight.iter_mut().for_each(|w| *w = 0.0);
        let mut any = false;
        for p in 0..n {
            work.searched += 1;
            // A triangle within a thickness of here now had its box within
            // a thickness and a drift of here when it was filed; one the
            // point went through lies along the way it came.
            let reach = thickness + drift;
            let from = before.map_or(pos[p], |b| b[p]);
            let (lo, hi) = (pos[p].min(from), pos[p].max(from));
            stamp = stamp.wrapping_add(1);
            if stamp == u32::MAX {
                seen.iter_mut().for_each(|m| *m = u32::MAX);
                stamp = 0;
            }
            let wide = if before.is_some() { reach + travelled } else { reach };
            grid.gather(lo - wide, hi + wide, &mut seen, stamp, &mut near);
            found.clear();
            let mut through = false;
            for &t in &near {
                let corners = topo.tris[t as usize];
                let [ia, ib, ic] = corners.map(|c| c as usize);
                let (a, b, c) = (pos[ia], pos[ib], pos[ic]);
                // Most of what a cell holds is nowhere near: a triangle
                // whose box is a thickness away on any axis is further
                // than that, and is turned away before it costs a search
                // of the rings or a closest point.
                let (tlo, thi) = (a.min(b).min(c) - thickness, a.max(b).max(c) + thickness);
                if hi.cmplt(tlo).any() || lo.cmpgt(thi).any() {
                    continue;
                }
                if corners.iter().any(|&c| topo.excludes(p, c)) {
                    continue;
                }
                let w = crate::spatial::closest_weights_on_triangle(pos[p], a, b, c);
                if let Some(side) = before.and_then(|was| went_through(was[p], [was[ia], was[ib], was[ic]], pos[p], [a, b, c], 0.0)) {
                    // `side` is the triangle's normal, turned to the side
                    // the point came from; it is below the plane by that
                    // much and belongs a thickness above it.
                    let below = (pos[p] - a).dot(side);
                    found.push(Contact { corners: [ia, ib, ic], w, dir: side, deep: thickness - below, through: true });
                    through = true;
                    continue;
                }
                let d = pos[p] - (a * w[0] + b * w[1] + c * w[2]);
                let len = d.length();
                if len >= thickness {
                    continue;
                }
                let dir = if len < 1e-9 {
                    let normal = (b - a).cross(c - a).normalize_or_zero();
                    if normal == Vec3::ZERO {
                        continue;
                    }
                    normal
                } else {
                    d / len
                };
                found.push(Contact { corners: [ia, ib, ic], w, dir, deep: thickness - len, through: false });
            }
            for contact in found.iter().filter(|c| c.through == through) {
                let Contact { corners: [ia, ib, ic], w, dir, deep, .. } = *contact;
                // Inverse masses of one or none: the point's, and each
                // corner's by the square of its share.
                let give = free(p) + free(ia) * w[0] * w[0] + free(ib) * w[1] * w[1] + free(ic) * w[2] * w[2];
                if give <= 0.0 {
                    continue;
                }
                let step = dir * (deep / give);
                push[p] += step * (free(p) * deep);
                weight[p] += free(p) * deep;
                for (i, share) in [(ia, w[0]), (ib, w[1]), (ic, w[2])] {
                    // The corner moves by its share; its say in the average
                    // is its share too, so a corner that is barely part of
                    // one contact does not water down another it carries.
                    push[i] -= step * (share * free(i) * share * deep);
                    weight[i] += free(i) * share * deep;
                }
                work.contacts += 1;
                work.crossed += through as usize;
                any = true;
            }
        }
        if !any {
            break;
        }
        for p in 0..n {
            if weight[p] > 0.0 {
                pos[p] += push[p] / weight[p];
            }
        }
        moved_at_all = true;
        drift = grid.drift(&pos);
    }
    // The hold. The passes share a move out and average what they are
    // given, and under a push that does not let up they can run out before
    // a point is back on its side — and a point left through a triangle is,
    // to the next step, a point that began there. So whatever is still
    // through a triangle goes back to where the step began, and the
    // triangle with it: the one arrangement of the four known not to cross.
    // It costs them the step's movement and nothing else.
    if let Some(was) = before {
        let mut back: Vec<usize> = Vec::new();
        for _ in 0..HOLD_ROUNDS {
            if drift > DRIFT_CELLS * grid.cell {
                grid = TriCells::build(&pos, &topo.tris, cell);
                work.grids += 1;
                drift = 0.0;
            }
            back.clear();
            for p in 0..n {
                let (lo, hi) = (pos[p].min(was[p]), pos[p].max(was[p]));
                stamp = stamp.wrapping_add(1);
                if stamp == u32::MAX {
                    seen.iter_mut().for_each(|m| *m = u32::MAX);
                    stamp = 0;
                }
                let wide = Vec3::splat(drift + travelled);
                grid.gather(lo - wide, hi + wide, &mut seen, stamp, &mut near);
                for &t in &near {
                    let corners = topo.tris[t as usize];
                    let [ia, ib, ic] = corners.map(|c| c as usize);
                    let (a, b, c) = (pos[ia], pos[ib], pos[ic]);
                    let (tlo, thi) = (a.min(b).min(c).min(was[ia]).min(was[ib]).min(was[ic]), a.max(b).max(c).max(was[ia]).max(was[ib]).max(was[ic]));
                    if hi.cmplt(tlo).any() || lo.cmpgt(thi).any() {
                        continue;
                    }
                    if corners.iter().any(|&c| topo.excludes(p, c)) {
                        continue;
                    }
                    if went_through(was[p], [was[ia], was[ib], was[ic]], pos[p], [a, b, c], HOLD_MARGIN).is_some() {
                        back.extend([p, ia, ib, ic]);
                    }
                }
            }
            let mut put = 0;
            for &i in &back {
                if movable[i] && pos[i] != was[i] {
                    pos[i] = was[i];
                    put += 1;
                }
            }
            if put == 0 {
                break;
            }
            work.held += put;
            moved_at_all = true;
            drift = grid.drift(&pos);
        }
    }
    if moved_at_all {
        for (p, v) in pos.iter().enumerate() {
            geom.set_pos(p, *v);
        }
    }
}

/// One point against one triangle, as a pass found it.
#[derive(Clone, Copy)]
struct Contact {
    corners: [usize; 3],
    /// How much of the closest point each corner is.
    w: [f32; 3],
    /// The way the point is to move.
    dir: Vec3,
    /// How far, were it to take the whole move.
    deep: f32,
    through: bool,
}

/// A point's height over a triangle's plane, along the normal its winding
/// gives, and where its foot is in the triangle's own terms: three weights
/// summing to one, any of them negative outside it. `None` for a triangle
/// with no area.
fn over_triangle(p: Vec3, [a, b, c]: [Vec3; 3]) -> Option<(f32, [f32; 3], Vec3)> {
    let (e1, e2, v) = (b - a, c - a, p - a);
    let n = e1.cross(e2);
    let nn = n.length_squared();
    if nn < 1e-30 {
        return None;
    }
    let (w1, w2) = (v.cross(e2).dot(n) / nn, e1.cross(v).dot(n) / nn);
    let normal = n / nn.sqrt();
    Some((v.dot(normal), [1.0 - w1 - w2, w1, w2], normal))
}

/// How far outside a triangle, in its own weights, a passage still counts
/// as through it when the question is whether to HOLD the point. A point
/// going through the edge two triangles share is, after rounding, a little
/// outside both, and holding one that did not quite go through costs a
/// step's movement there and nothing else. The passes take no margin: a
/// push is along the triangle's normal, and measured on the sphere test a
/// tenth of a margin pushed points off triangles they had gone around,
/// leaving more crossed than no memory at all.
const HOLD_MARGIN: f32 = 0.05;

/// How many times the hold looks again: putting points back can leave
/// others through what was put back.
const HOLD_ROUNDS: usize = 8;

/// Whether a point went through a triangle between then and now, and if so
/// the triangle's normal as it is now, turned to the side the point came
/// from.
///
/// Both move, so the question is asked in the TRIANGLE'S terms: the point's
/// height over it and the place of its foot in it, then and now, with the
/// passage taken as a straight line between the two. It went through if
/// the height changed sign and the foot, where the height was nothing, was
/// inside the triangle.
fn went_through(p_then: Vec3, tri_then: [Vec3; 3], p_now: Vec3, tri_now: [Vec3; 3], margin: f32) -> Option<Vec3> {
    let (h0, w0, _) = over_triangle(p_then, tri_then)?;
    let (h1, w1, normal) = over_triangle(p_now, tri_now)?;
    if h0 == 0.0 || h0 * h1 >= 0.0 {
        return None;
    }
    let at = h0 / (h0 - h1);
    let inside = (0..3).all(|i| w0[i] + (w1[i] - w0[i]) * at >= -margin);
    inside.then(|| normal * h0.signum())
}

/// Where a surface passes through itself.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tangles {
    /// Edge-triangle pairs where the edge passes through the triangle.
    pub crossings: usize,
    /// The points of those edges and triangles, ascending, each once.
    pub points: Vec<u32>,
}

/// Every edge of `geom` that passes through one of its triangles.
///
/// This is the MEASURE, and it is geometric: no thickness and no rings. An
/// edge is not tested against a triangle it shares a point with — they meet
/// there by construction — nor against one fanned from a primitive the edge
/// is a side of.
pub fn self_intersections(geom: &Detail) -> Tangles {
    if geom.num_points() == 0 || geom.num_prims() == 0 {
        return Tangles::default();
    }
    let topo = topo_for(geom, 0);
    if topo.edges.is_empty() {
        return Tangles::default();
    }
    intersections_of(geom, &topo, false)
}

/// [`self_intersections`] counting only what the solve is MEANT to see at
/// this ring count: an edge through a triangle none of whose corners is
/// within `rings` of either end of it. The rest are folds inside the
/// excluded neighbourhood, which the node leaves alone by design, and
/// telling the two apart is what says whether a crossing is the method's
/// miss or the setting's.
pub fn crossings_beyond(geom: &Detail, rings: usize) -> usize {
    if geom.num_points() == 0 || geom.num_prims() == 0 {
        return 0;
    }
    let topo = topo_for(geom, rings);
    if topo.edges.is_empty() {
        return 0;
    }
    intersections_of(geom, &topo, true).crossings
}

fn intersections_of(geom: &Detail, topo: &Topo, beyond_rings: bool) -> Tangles {
    let pos: Vec<Vec3> = (0..geom.num_points()).map(|p| geom.pos(p)).collect();
    let grid = TriCells::build(&pos, &topo.tris, mean_edge(geom, topo));
    let mut found = Tangles::default();
    let mut marked = vec![false; pos.len()];
    let mut near = Vec::new();
    let mut seen = vec![u32::MAX; topo.tris.len()];
    for (stamp, e) in topo.edges.iter().enumerate() {
        let (a, b) = (pos[e[0] as usize], pos[e[1] as usize]);
        grid.gather(a.min(b), a.max(b), &mut seen, stamp as u32, &mut near);
        for &t in &near {
            let tri = topo.tris[t as usize];
            if tri.contains(&e[0]) || tri.contains(&e[1]) {
                continue;
            }
            if beyond_rings && tri.iter().any(|&c| topo.excludes(e[0] as usize, c) || topo.excludes(e[1] as usize, c)) {
                continue;
            }
            let of = geom.prim_points(topo.tri_prims[t as usize] as usize);
            if of.contains(&e[0]) && of.contains(&e[1]) {
                continue;
            }
            let [v0, v1, v2] = tri.map(|i| pos[i as usize]);
            if crate::spatial::segment_crosses_triangle(a, b, v0, v1, v2) {
                found.crossings += 1;
                for i in e.iter().chain(tri.iter()) {
                    marked[*i as usize] = true;
                }
            }
        }
    }
    found.points = (0..pos.len() as u32).filter(|&p| marked[p as usize]).collect();
    found
}
