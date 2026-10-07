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
    /// Every side of every triangle, once, and each triangle's three. The
    /// mesh's edges and the diagonals a fan cuts across a quad: what the
    /// triangles are made of, which is what can pass through itself.
    sides: Vec<[u32; 2]>,
    tri_sides: Vec<[u32; 3]>,
    /// The triangles at each point, and the sides: point `p`'s are
    /// `at_tris[tri_starts[p]..tri_starts[p + 1]]`, and likewise.
    tri_starts: Vec<u32>,
    at_tris: Vec<u32>,
    side_starts: Vec<u32>,
    at_sides: Vec<u32>,
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
        let mut sides: Vec<[u32; 2]> = Vec::new();
        let mut side_of: std::collections::HashMap<[u32; 2], u32> = std::collections::HashMap::new();
        let tri_sides = tris
            .iter()
            .map(|t| {
                [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]].map(|[a, b]| {
                    let ends = [a.min(b), a.max(b)];
                    *side_of.entry(ends).or_insert_with(|| {
                        sides.push(ends);
                        sides.len() as u32 - 1
                    })
                })
            })
            .collect();
        let tri_sides: Vec<[u32; 3]> = tri_sides;
        let (tri_starts, at_tris) = by_point(n, tris.iter().map(|t| &t[..]));
        let (side_starts, at_sides) = by_point(n, sides.iter().map(|e| &e[..]));
        Topo { key, rings, edges: geom.edges().to_vec(), tris, tri_prims, sides, tri_sides, tri_starts, at_tris, side_starts, at_sides, starts, excluded }
    }

    fn own(&self, p: usize) -> &[u32] {
        &self.excluded[self.starts[p] as usize..self.starts[p + 1] as usize]
    }

    fn tris_at(&self, p: usize) -> &[u32] {
        &self.at_tris[self.tri_starts[p] as usize..self.tri_starts[p + 1] as usize]
    }

    fn sides_at(&self, p: usize) -> &[u32] {
        &self.at_sides[self.side_starts[p] as usize..self.side_starts[p + 1] as usize]
    }

    /// Whether `q`, or a point a side away from it, is stirred: the other
    /// end of a pair reaches one ring past the rings.
    fn neighbours_stirred(&self, q: u32, stirred: &[bool]) -> bool {
        stirred[q as usize] || self.sides_at(q as usize).iter().any(|&j| self.sides[j as usize].iter().any(|&r| stirred[r as usize]))
    }

    fn excludes(&self, p: usize, q: u32) -> bool {
        let (a, b) = (self.starts[p] as usize, self.starts[p + 1] as usize);
        self.excluded[a..b].binary_search(&q).is_ok()
    }
}

/// Which of `items` each point is part of, flat: point `p`'s are
/// `ids[starts[p]..starts[p + 1]]`, ascending.
fn by_point<'a>(n: usize, items: impl Iterator<Item = &'a [u32]> + Clone) -> (Vec<u32>, Vec<u32>) {
    let mut starts = vec![0u32; n + 1];
    for item in items.clone() {
        for &p in item {
            starts[p as usize + 1] += 1;
        }
    }
    for p in 0..n {
        starts[p + 1] += starts[p];
    }
    let mut next = starts.clone();
    let mut ids = vec![0u32; starts[n] as usize];
    for (i, item) in items.enumerate() {
        for &p in item {
            ids[next[p as usize] as usize] = i as u32;
            next[p as usize] += 1;
        }
    }
    (starts, ids)
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
    /// Edges tested against the edges near them, over every pass: the
    /// ones with an end near something that is not their own neighbourhood.
    pub edges_searched: usize,
    /// Of those contacts, the ones between two EDGES.
    pub edge_contacts: usize,
    /// Of those contacts, the ones resolved as having gone THROUGH since
    /// the step began — a point through a triangle, an edge through an
    /// edge — and put back on the side they came from.
    pub crossed: usize,
    /// Pairs inside each other's rings found gone through each other when
    /// the solve began: folds.
    pub folds: usize,
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
    let rings = node_param_f32(target, "rings", 2.0).clamp(0.0, 6.0) as usize;
    let topo = topo_for(geom, rings);
    if topo.edges.is_empty() {
        return work;
    }
    // A node without the row is one from before it, and solves as it did.
    if node_param_str(target, "method", "Points").trim().eq_ignore_ascii_case("Surface") {
        let before: Option<Vec<Vec3>> = before
            .filter(|b| b.num_points() == n && b.num_prims() == geom.num_prims() && topology_key(b) == topo.key)
            .map(|b| (0..n).map(|p| b.pos(p)).collect());
        solve_surface(geom, before.as_deref(), target, &topo, &mut work);
    } else {
        solve_points(geom, target, &topo, &mut work);
    }
    // What is STILL crossed once the solve is done, which is the part worth
    // looking at. Only when asked for: it is a second search of the mesh.
    let mark = node_param_str(target, "tangled_group", "");
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
    node_param_f32(target, "thickness", 1.0).max(0.0) * mean_edge
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
    let iterations = node_param_f32(target, "iterations", 4.0).clamp(1.0, 32.0) as usize;
    let group = node_param_str(target, "group", "");
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
        let mut grid = TriCells { min, cell, dims, starts: Vec::new(), ids: Vec::new() };
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
/// **Edges meet edges.** Two edges can pass through each other with no
/// point of either going through any triangle, and then every test above
/// reports nothing while the measure counts a crossing. With Edge Contact
/// on, each side of each triangle is tested against the sides near it
/// that share no neighbourhood with it: where the two are nearest at a
/// place INSIDE both — an end is a point, and a point near an edge is near
/// the edge's triangle, which is the test above — they are parted along
/// the line between those places, the four ends sharing the move by how
/// near each is to it. Told where the step began, two edges that have
/// passed through each other ([`edges_went_through`]) are put back, and
/// held if the passes leave them through.
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
    let iterations = node_param_f32(target, "iterations", 4.0).clamp(1.0, 32.0) as usize;
    // A node without the row is one from before it: points and triangles.
    let edges_too = crate::geometry::node_param_bool(target, "edge_contact", false);
    let group = node_param_str(target, "group", "");
    let group = group.trim().to_string();
    let movable: Vec<bool> = (0..n).map(|p| group.is_empty() || geom.points().in_group(&group, p)).collect();
    let free = |p: usize| if movable[p] { 1.0f32 } else { 0.0 };

    let mut pos: Vec<Vec3> = (0..n).map(|p| geom.pos(p)).collect();
    let mut moved_at_all = false;
    // A node without the row is one from before it, and is not limited.
    let limit = node_param_f32(target, "step_limit", 0.0).max(0.0) * thickness;
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

    let looking = Looking { thickness, cell: thickness.max(mean_edge(geom, topo)), edges_too };
    let mut near = Near::find(topo, before, &pos, looking);
    work.grids += 1;
    work.edges_searched += near.sides_looked_at;
    // What has folded through its own neighbourhood since the step began.
    // A node without the row is one from before it.
    let folds_too = before.is_some() && crate::geometry::node_param_bool(target, "fold_contact", false);
    let mut folds: Vec<Fold> = Vec::new();
    if let (Some(was), true) = (before, folds_too) {
        folded(topo, was, &pos, edges_too, 0.0, None, &mut folds);
        work.folds = folds.len();
    }
    let mut push = vec![Vec3::ZERO; n];
    let mut weight = vec![0.0f32; n];
    let mut found: Vec<Contact> = Vec::new();
    // Which points have gone through something this pass.
    let mut through = vec![false; n];
    for _ in 0..iterations {
        work.passes += 1;
        if near.is_stale(&pos) {
            near = Near::find(topo, before, &pos, looking);
            work.grids += 1;
            work.edges_searched += near.sides_looked_at;
        }
        found.clear();
        through.iter_mut().for_each(|t| *t = false);
        // What folded through is put back as far over its neighbour as it
        // began, which is nearer than a thickness: that is what a
        // neighbour is.
        if let Some(was) = before {
            for fold in &folds {
                let (who, then, now) = fold.points(topo, was, &pos);
                if fold.sides {
                    let ([e0, e1], [f0, f1]) = ([now[0], now[1]], [now[2], now[3]]);
                    if let Some((side, at, height)) = edges_went_through([then[0], then[1]], [then[2], then[3]], [e0, e1], [f0, f1], 0.0) {
                        let (s, t) = (at[0].clamp(0.0, 1.0), at[1].clamp(0.0, 1.0));
                        let below = ((e0 + (e1 - e0) * s) - (f0 + (f1 - f0) * t)).dot(side);
                        found.push(Contact { who, share: [1.0 - s, s, t - 1.0, -t], dir: side, deep: height.min(thickness) - below, through: true, edges: true });
                        who.iter().for_each(|&p| through[p] = true);
                    }
                } else if let Some((side, height)) = went_through(then[0], [then[1], then[2], then[3]], now[0], [now[1], now[2], now[3]], 0.0) {
                    let w = crate::spatial::closest_weights_on_triangle(now[0], now[1], now[2], now[3]);
                    let below = (now[0] - now[1]).dot(side);
                    found.push(Contact { who, share: [1.0, -w[0], -w[1], -w[2]], dir: side, deep: height.min(thickness) - below, through: true, edges: false });
                    through[who[0]] = true;
                }
            }
        }
        let (touching, positions) = (&near.touching, &pos);
        let by_point = in_pieces(touching.len(), 4096, |pairs| {
            let (pos, mut found) = (positions, Vec::new());
            for &[p, t] in &touching[pairs] {
                let p = p as usize;
                let [ia, ib, ic] = topo.tris[t as usize].map(|c| c as usize);
                let (a, b, c) = (pos[ia], pos[ib], pos[ic]);
                let who = [p, ia, ib, ic];
                if let Some((side, _)) = before.and_then(|was| went_through(was[p], [was[ia], was[ib], was[ic]], pos[p], [a, b, c], 0.0)) {
                    // `side` is the triangle's normal, turned to the side
                    // the point came from; it is below the plane by that
                    // much and belongs a thickness above it.
                    let w = crate::spatial::closest_weights_on_triangle(pos[p], a, b, c);
                    let below = (pos[p] - a).dot(side);
                    found.push(Contact { who, share: [1.0, -w[0], -w[1], -w[2]], dir: side, deep: thickness - below, through: true, edges: false });
                    continue;
                }
                // What is listed is what was near when it was listed, and
                // most of it is not near enough: a triangle whose box is a
                // thickness away on any axis is further than that.
                let (tlo, thi) = (a.min(b).min(c) - thickness, a.max(b).max(c) + thickness);
                if pos[p].cmplt(tlo).any() || pos[p].cmpgt(thi).any() {
                    continue;
                }
                let w = crate::spatial::closest_weights_on_triangle(pos[p], a, b, c);
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
                found.push(Contact { who, share: [1.0, -w[0], -w[1], -w[2]], dir, deep: thickness - len, through: false, edges: false });
            }
            found
        });
        found.extend(by_point.into_iter().flatten());
        work.searched += n;
        let meeting = &near.meeting;
        let by_side = in_pieces(meeting.len(), 4096, |pairs| {
            let (pos, mut found) = (positions, Vec::new());
            for &[i, j] in &meeting[pairs] {
                let (e, f) = (topo.sides[i as usize], topo.sides[j as usize]);
                let ([e0, e1], [f0, f1]) = (e.map(|p| pos[p as usize]), f.map(|p| pos[p as usize]));
                let who = [e[0] as usize, e[1] as usize, f[0] as usize, f[1] as usize];
                let was = before.map(|was| (e.map(|p| was[p as usize]), f.map(|p| was[p as usize])));
                if let Some((side, at, _)) = was.and_then(|(e_was, f_was)| edges_went_through(e_was, f_was, [e0, e1], [f0, f1], 0.0)) {
                    let (s, t) = (at[0].clamp(0.0, 1.0), at[1].clamp(0.0, 1.0));
                    let below = ((e0 + (e1 - e0) * s) - (f0 + (f1 - f0) * t)).dot(side);
                    found.push(Contact { who, share: [1.0 - s, s, t - 1.0, -t], dir: side, deep: thickness - below, through: true, edges: true });
                    continue;
                }
                let (lo, hi) = (e0.min(e1), e0.max(e1));
                let (flo, fhi) = (f0.min(f1) - thickness, f0.max(f1) + thickness);
                if hi.cmplt(flo).any() || lo.cmpgt(fhi).any() {
                    continue;
                }
                let (s, t) = nearest_on_segments([e0, e1], [f0, f1]);
                // An end is a point, and the point test has it.
                if !(s > ENDS && s < 1.0 - ENDS && t > ENDS && t < 1.0 - ENDS) {
                    continue;
                }
                let d = (e0 + (e1 - e0) * s) - (f0 + (f1 - f0) * t);
                let len = d.length();
                if len >= thickness {
                    continue;
                }
                let dir = if len < 1e-9 {
                    let across = (e1 - e0).cross(f1 - f0).normalize_or_zero();
                    if across == Vec3::ZERO {
                        continue;
                    }
                    across
                } else {
                    d / len
                };
                found.push(Contact { who, share: [1.0 - s, s, t - 1.0, -t], dir, deep: thickness - len, through: false, edges: true });
            }
            found
        });
        found.extend(by_side.into_iter().flatten());
        // What has gone through something, this pass.
        for contact in found.iter().filter(|c| c.through) {
            let subjects = if contact.edges { &contact.who[..] } else { &contact.who[..1] };
            subjects.iter().for_each(|&p| through[p] = true);
        }
        push.iter_mut().for_each(|v| *v = Vec3::ZERO);
        weight.iter_mut().for_each(|w| *w = 0.0);
        let mut any = false;
        for contact in &found {
            let Contact { who, share, dir, deep, .. } = *contact;
            // What has gone through something is put back before it is
            // parted from anything: whatever is beside the thing it went
            // through sees it near, on the wrong side, and would push it on.
            let subjects = if contact.edges { &who[..] } else { &who[..1] };
            if !contact.through && subjects.iter().any(|&p| through[p]) {
                continue;
            }
            // Inverse masses of one or none, each by the square of its
            // share of the contact.
            let give: f32 = (0..4).map(|i| free(who[i]) * share[i] * share[i]).sum();
            if give <= 0.0 {
                continue;
            }
            let step = dir * (deep / give);
            for i in 0..4 {
                // Each moves by its share; its say in the average is its
                // share too, so one that is barely part of a contact does
                // not water down another it carries.
                let say = free(who[i]) * share[i].abs() * deep;
                push[who[i]] += step * (share[i] * say);
                weight[who[i]] += say;
            }
            work.contacts += 1;
            work.edge_contacts += contact.edges as usize;
            work.crossed += contact.through as usize;
            any = true;
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
    }
    // The hold. The passes share a move out and average what they are
    // given, and under a push that does not let up they can run out before
    // a point is back on its side — and a point left through a triangle is,
    // to the next step, a point that began there. So whatever is still
    // through goes back to where the step began, and what it is through
    // with it: the one arrangement of them known not to cross. It costs
    // them the step's movement and nothing else.
    if let Some(was) = before {
        let mut back: Vec<usize> = Vec::new();
        // What the last look put back, and so what this one has to look
        // at: a pair none of whose points has moved since it was last
        // looked at is as it was. The first look is at everything.
        let mut stirred = vec![true; n];
        for _ in 0..HOLD_ROUNDS {
            if near.is_stale(&pos) {
                near = Near::find(topo, before, &pos, looking);
                work.grids += 1;
                work.edges_searched += near.sides_looked_at;
            }
            back.clear();
            let (touching, meeting, positions, stirred_now) = (&near.touching, &near.meeting, &pos, &stirred);
            let by_point = in_pieces(touching.len(), 8192, |pairs| {
                let (pos, stirred, mut back) = (positions, stirred_now, Vec::new());
                for &[p, t] in &touching[pairs] {
                    let p = p as usize;
                    let [ia, ib, ic] = topo.tris[t as usize].map(|c| c as usize);
                    if !(stirred[p] || stirred[ia] || stirred[ib] || stirred[ic]) {
                        continue;
                    }
                    if went_through(was[p], [was[ia], was[ib], was[ic]], pos[p], [pos[ia], pos[ib], pos[ic]], HOLD_MARGIN).is_some() {
                        back.extend([p, ia, ib, ic]);
                    }
                }
                back
            });
            let by_side = in_pieces(meeting.len(), 8192, |pairs| {
                let (pos, stirred, mut back) = (positions, stirred_now, Vec::new());
                for &[i, j] in &meeting[pairs] {
                    let (e, f) = (topo.sides[i as usize], topo.sides[j as usize]);
                    if !e.iter().chain(f.iter()).any(|&p| stirred[p as usize]) {
                        continue;
                    }
                    let (e_now, e_was) = (e.map(|p| pos[p as usize]), e.map(|p| was[p as usize]));
                    let (f_now, f_was) = (f.map(|p| pos[p as usize]), f.map(|p| was[p as usize]));
                    if edges_went_through(e_was, f_was, e_now, f_now, HOLD_MARGIN).is_some() {
                        back.extend(e.iter().chain(f.iter()).map(|&p| p as usize));
                    }
                }
                back
            });
            back.extend(by_point.into_iter().chain(by_side).flatten());
            if folds_too {
                folded(topo, was, &pos, edges_too, HOLD_MARGIN, Some(&stirred), &mut folds);
                for fold in &folds {
                    back.extend(fold.points(topo, was, &pos).0);
                }
            }
            stirred.iter_mut().for_each(|s| *s = false);
            let mut put = 0;
            for &i in &back {
                if movable[i] && pos[i] != was[i] {
                    pos[i] = was[i];
                    stirred[i] = true;
                    put += 1;
                }
            }
            if put == 0 {
                break;
            }
            work.held += put;
            moved_at_all = true;
        }
    }
    if moved_at_all {
        for (p, v) in pos.iter().enumerate() {
            geom.set_pos(p, *v);
        }
    }
}

/// `run` over `0..count` in pieces, on as many threads as the machine has
/// and the work is worth — no piece smaller than `least` — and what each
/// piece made, in order. The order is what makes this a way of RUNNING the
/// work and not a change to it: put end to end, the pieces are what one
/// thread would have made.
///
/// Threads and not a pool, since the crate has none: a thread costs tens
/// of microseconds to start, which is what `least` is for.
pub(crate) fn in_pieces<R: Send>(count: usize, least: usize, run: impl Fn(std::ops::Range<usize>) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(count / least.max(1)).max(1);
    if threads == 1 {
        return vec![run(0..count)];
    }
    let size = count.div_ceil(threads);
    std::thread::scope(|scope| {
        let run = &run;
        let pieces: Vec<_> = (0..threads).map(|k| scope.spawn(move || run((k * size).min(count)..((k + 1) * size).min(count)))).collect();
        pieces.into_iter().map(|piece| piece.join().expect("a piece of the detangle solve panicked")).collect()
    })
}

/// What a search is for.
#[derive(Clone, Copy)]
struct Looking {
    thickness: f32,
    cell: f32,
    edges_too: bool,
}

/// How much further than a thickness the search looks, as a part of one,
/// so that what it finds is still everything near once the passes have
/// moved the points: until any has moved half of this.
const SLACK: f32 = 0.5;

/// What is near what: the pairs a pass has to look at, found ONCE and kept
/// while the points stay near where they were when it was.
///
/// The passes of a solve, and the looks of the hold after them, ask the
/// same question of nearly the same positions, and until 2026-09-29 each
/// answered it from the grid: a gather of cells per point and per edge, a
/// search of the rings per candidate, four or five times over. Measured
/// on the sphere test, that — and not the contacts — was what the Surface
/// method cost. The pairs are found with [`SLACK`] to spare, and found
/// again only when a point has moved half of it.
struct Near {
    /// Where the points were when this was found.
    filed: Vec<Vec3>,
    slack: f32,
    /// A point and a triangle none of whose corners is in its rings.
    touching: Vec<[u32; 2]>,
    /// Two sides neither of whose ends is in the rings of the other's.
    meeting: Vec<[u32; 2]>,
    /// How many sides were searched for sides near them.
    sides_looked_at: usize,
}

impl Near {
    fn is_stale(&self, pos: &[Vec3]) -> bool {
        let moved = pos.iter().zip(&self.filed).map(|(p, f)| (*p - *f).length_squared()).fold(0.0f32, f32::max);
        moved > (self.slack * 0.5) * (self.slack * 0.5)
    }

    fn find(topo: &Topo, before: Option<&[Vec3]>, pos: &[Vec3], looking: Looking) -> Near {
        let Looking { thickness, cell, edges_too } = looking;
        let n = pos.len();
        let slack = thickness * SLACK;
        let grid = TriCells::build(pos, &topo.tris, cell);
        // How far anything has come since the step began: what a point or
        // an edge went through may be that far from where either is now.
        let travelled = before.map_or(0.0, |b| pos.iter().zip(b).map(|(p, q)| (*p - *q).length_squared()).fold(0.0f32, f32::max).sqrt());
        // Which points are near a triangle with a side that is none of
        // their own neighbourhood: what says which edges are worth testing
        // against others. Two edges nearer than `close` somewhere along
        // them have an end within that and half the edge's length of the
        // other edge, which is a side of a triangle — so an edge with
        // neither end that near anything is near nothing, and most of a
        // mesh is. `close` is the thickness, or as far as two edges that
        // have passed through each other this step can have come apart
        // since.
        let mut near_something = vec![false; n];
        let close = thickness.max(2.0 * travelled) + slack;
        // Half the longest side at each point.
        let mut half = vec![0.0f32; n];
        if edges_too {
            for e in &topo.sides {
                let [a, b] = e.map(|p| p as usize);
                let len = (pos[b] - pos[a]).length() * 0.5;
                (half[a], half[b]) = (half[a].max(len), half[b].max(len));
            }
        }
        // What may touch is what is within a thickness, and what may have
        // gone through since the step began is what is within twice what
        // anything has travelled: each has come no further than that from
        // where they met. Boxes turn most of a cell away; the distance
        // itself turns away most of what is left, which on one sheet is
        // the sides a few edges off — near enough for their boxes, and
        // further than any thickness.
        let reach = thickness.max(2.0 * travelled) + slack;
        let boxed = |points: &[u32]| points.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |at, &p| (at.0.min(pos[p as usize]), at.1.max(pos[p as usize])));
        let by_point = in_pieces(n, 64, |points| {
            let mut search = Search::new(topo);
            let (mut touching, mut near_something) = (Vec::new(), Vec::new());
            for p in points {
                // How far this point looks: for what it may touch, and
                // further for what its edges may.
                let look = if edges_too { close + half[p] } else { reach };
                let mut near = false;
                search.triangles(&grid, pos[p] - look, pos[p] + look);
                for &t in &search.near {
                    let corners = topo.tris[t as usize];
                    let (tlo, thi) = boxed(&corners);
                    if pos[p].cmplt(tlo - look).any() || pos[p].cmpgt(thi + look).any() {
                        continue;
                    }
                    let [a, b, c] = corners.map(|c| pos[c as usize]);
                    let away = (pos[p] - crate::spatial::closest_point_on_triangle(pos[p], a, b, c)).length_squared();
                    if away > look * look {
                        continue;
                    }
                    // A triangle with two corners that are none of the
                    // point's own has a side its edges may meet; one with
                    // three is a triangle it may touch.
                    let own = corners.iter().filter(|&&c| topo.excludes(p, c)).count();
                    if own > 1 || (own == 1 && !edges_too) {
                        continue;
                    }
                    near = true;
                    if own == 0 && away <= reach * reach {
                        touching.push([p as u32, t]);
                    }
                }
                if near {
                    near_something.push(p);
                }
            }
            (touching, near_something)
        });
        let mut touching = Vec::new();
        for (pairs, near) in by_point {
            touching.extend(pairs);
            near.into_iter().for_each(|p| near_something[p] = true);
        }
        let near_something = &near_something;
        let mut meeting = Vec::new();
        let mut sides_looked_at = 0;
        if edges_too {
            let by_side = in_pieces(topo.sides.len(), 64, |sides| {
                let mut search = Search::new(topo);
                let (mut meeting, mut looked_at) = (Vec::new(), 0);
                for i in sides {
                    let e = topo.sides[i];
                    if !e.iter().any(|&p| near_something[p as usize]) {
                        continue;
                    }
                    looked_at += 1;
                    let (lo, hi) = boxed(&e);
                    let [e0, e1] = e.map(|p| pos[p as usize]);
                    search.sides(&grid, topo, i, lo - reach, hi + reach);
                    for &j in &search.near {
                        let f = topo.sides[j as usize];
                        let (flo, fhi) = boxed(&f);
                        if hi.cmplt(flo - reach).any() || lo.cmpgt(fhi + reach).any() {
                            continue;
                        }
                        let [f0, f1] = f.map(|p| pos[p as usize]);
                        let (s, t) = nearest_on_segments([e0, e1], [f0, f1]);
                        if ((e0 + (e1 - e0) * s) - (f0 + (f1 - f0) * t)).length_squared() > reach * reach {
                            continue;
                        }
                        if e.iter().any(|&p| f.iter().any(|&q| topo.excludes(p as usize, q))) {
                            continue;
                        }
                        meeting.push([i as u32, j]);
                    }
                }
                (meeting, looked_at)
            });
            for (pairs, looked_at) in by_side {
                meeting.extend(pairs);
                sides_looked_at += looked_at;
            }
        }
        Near { filed: pos.to_vec(), slack, touching, meeting, sides_looked_at }
    }
}

/// A point and a triangle of its own neighbourhood, or two sides that are
/// each other's, one of which went through the other.
#[derive(Clone, Copy)]
struct Fold {
    sides: bool,
    /// The point and the triangle, or the two sides.
    pair: [u32; 2],
}

impl Fold {
    /// The four points of it, where they were and where they are.
    fn points(&self, topo: &Topo, was: &[Vec3], pos: &[Vec3]) -> ([usize; 4], [Vec3; 4], [Vec3; 4]) {
        let who = if self.sides {
            let ([a, b], [c, d]) = (topo.sides[self.pair[0] as usize], topo.sides[self.pair[1] as usize]);
            [a, b, c, d].map(|p| p as usize)
        } else {
            let [a, b, c] = topo.tris[self.pair[1] as usize];
            [self.pair[0], a, b, c].map(|p| p as usize)
        };
        (who, who.map(|p| was[p]), who.map(|p| pos[p]))
    }
}

/// Everything that has gone through its own NEIGHBOURHOOD since the step
/// began: a point through a triangle with a corner inside the point's
/// rings, a side through a side with an end inside the other's.
///
/// The rings are excluded from contact because a neighbour is nearer than
/// a thickness by construction, and no distance says whether it is too
/// near. Going THROUGH is not a distance. A point that was on one side of
/// its neighbour's triangle and is on the other has folded the surface
/// through itself, whatever the thickness, and the only pairs with nothing
/// to say are the ones that share a point — which meet there.
///
/// Found through the mesh and not through the grid: what is in a point's
/// rings is the triangles at the points of its rings, however far apart
/// the fold has left them.
fn folded(topo: &Topo, was: &[Vec3], pos: &[Vec3], sides_too: bool, margin: f32, stirred: Option<&[bool]>, out: &mut Vec<Fold>) {
    out.clear();
    // With `stirred`, only the pairs with a point that is: what has a
    // stirred point in its rings, or is one, is warm, and of a warm
    // point's pairs the ones with no stirred point are as they were.
    let is = |p: u32| stirred.is_none_or(|s| s[p as usize]);
    let warm: Option<Vec<bool>> = stirred.map(|s| (0..pos.len()).map(|p| topo.own(p).iter().any(|&q| topo.neighbours_stirred(q, s))).collect());
    let is_warm = |p: u32| warm.as_ref().is_none_or(|w| w[p as usize]);
    let by_point = in_pieces(pos.len(), 256, |points| {
        let mut search = Search::new(topo);
        let mut out = Vec::new();
        for p in points {
            if !is_warm(p as u32) {
                continue;
            }
            search.next();
            for &q in topo.own(p) {
                for &t in topo.tris_at(q as usize) {
                    if search.seen[t as usize] == search.stamp {
                        continue;
                    }
                    search.seen[t as usize] = search.stamp;
                    let corners = topo.tris[t as usize];
                    if corners.contains(&(p as u32)) || !(is(p as u32) || corners.iter().any(|&c| is(c))) {
                        continue;
                    }
                    let [a, b, c] = corners.map(|c| c as usize);
                    if went_through(was[p], [was[a], was[b], was[c]], pos[p], [pos[a], pos[b], pos[c]], margin).is_some() {
                        out.push(Fold { sides: false, pair: [p as u32, t] });
                    }
                }
            }
        }
        out
    });
    out.extend(by_point.into_iter().flatten());
    if !sides_too {
        return;
    }
    let by_side = in_pieces(topo.sides.len(), 256, |sides| {
        let mut search = Search::new(topo);
        let mut out = Vec::new();
        for i in sides {
            let e = topo.sides[i];
            if !e.iter().any(|&p| is_warm(p)) {
                continue;
            }
            search.next();
            let (e_was, e_now) = (e.map(|p| was[p as usize]), e.map(|p| pos[p as usize]));
            for &end in &e {
                for &q in topo.own(end as usize) {
                    for &j in topo.sides_at(q as usize) {
                        if j as usize <= i || search.met[j as usize] == search.stamp {
                            continue;
                        }
                        search.met[j as usize] = search.stamp;
                        let f = topo.sides[j as usize];
                        if f.iter().any(|q| e.contains(q)) || !e.iter().chain(f.iter()).any(|&p| is(p)) {
                            continue;
                        }
                        let (f_was, f_now) = (f.map(|p| was[p as usize]), f.map(|p| pos[p as usize]));
                        if edges_went_through(e_was, f_was, e_now, f_now, margin).is_some() {
                            out.push(Fold { sides: true, pair: [i as u32, j] });
                        }
                    }
                }
            }
        }
        out
    });
    out.extend(by_side.into_iter().flatten());
}

/// A contact as a pass found it: a point and a triangle's three corners,
/// or two edges' four ends.
#[derive(Clone, Copy)]
struct Contact {
    who: [usize; 4],
    /// How much of the move each takes, and which way: the point one, the
    /// corners against it by how much of the closest point each is; an
    /// edge's ends by how near each is to where the edges are nearest, and
    /// the other edge's against them.
    share: [f32; 4],
    /// The way the first of them is to move.
    dir: Vec3,
    /// How far they are to part.
    deep: f32,
    through: bool,
    edges: bool,
}

/// How near its end, as a part of its length, the nearest place on an edge
/// may be and still be the edge's and not the end's.
const ENDS: f32 = 1e-3;

/// What a search of the grid keeps between one query and the next.
struct Search {
    near: Vec<u32>,
    seen: Vec<u32>,
    stamp: u32,
    /// One mark per side, as `seen` is one per triangle.
    met: Vec<u32>,
    /// The triangles a search for sides went through.
    held: Vec<u32>,
}

impl Search {
    fn new(topo: &Topo) -> Search {
        Search { near: Vec::new(), seen: vec![u32::MAX; topo.tris.len()], stamp: 0, met: vec![u32::MAX; topo.sides.len()], held: Vec::new() }
    }

    fn next(&mut self) {
        self.stamp = self.stamp.wrapping_add(1);
        if self.stamp == u32::MAX {
            self.seen.iter_mut().for_each(|m| *m = u32::MAX);
            self.met.iter_mut().for_each(|m| *m = u32::MAX);
            self.stamp = 0;
        }
    }

    /// The triangles filed in the cells the box touches, into `near`.
    fn triangles(&mut self, grid: &TriCells, lo: Vec3, hi: Vec3) {
        self.next();
        grid.gather(lo, hi, &mut self.seen, self.stamp, &mut self.near);
    }

    /// The sides of those triangles that come AFTER side `i`, each once,
    /// into `near`: a pair of sides is met from the earlier of the two.
    fn sides(&mut self, grid: &TriCells, topo: &Topo, i: usize, lo: Vec3, hi: Vec3) {
        self.triangles(grid, lo, hi);
        std::mem::swap(&mut self.near, &mut self.held);
        self.near.clear();
        for &t in &self.held {
            for &j in &topo.tri_sides[t as usize] {
                if j as usize > i && self.met[j as usize] != self.stamp {
                    self.met[j as usize] = self.stamp;
                    self.near.push(j);
                }
            }
        }
    }
}

/// Where two segments are nearest each other, as a part of each one's
/// length. Ericson's *Real-Time Collision Detection* §5.1.9.
fn nearest_on_segments([p1, q1]: [Vec3; 2], [p2, q2]: [Vec3; 2]) -> (f32, f32) {
    let (d1, d2, r) = (q1 - p1, q2 - p2, p1 - p2);
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
    if a <= 1e-30 && e <= 1e-30 {
        return (0.0, 0.0);
    }
    if a <= 1e-30 {
        return (0.0, (f / e).clamp(0.0, 1.0));
    }
    let c = d1.dot(r);
    if e <= 1e-30 {
        return ((-c / a).clamp(0.0, 1.0), 0.0);
    }
    let b = d1.dot(d2);
    let denom = a * e - b * b;
    let mut s = if denom > 1e-12 * a * e { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
    let mut t = (b * s + f) / e;
    if t < 0.0 {
        t = 0.0;
        s = (-c / a).clamp(0.0, 1.0);
    } else if t > 1.0 {
        t = 1.0;
        s = ((b - c) / a).clamp(0.0, 1.0);
    }
    (s, t)
}

/// One edge over another, as lines: how far the first is from the second
/// along the direction across both, where on each the two are nearest (as
/// a part of its length, outside nought to one beyond its ends), and that
/// direction. `None` for edges that run the same way, which have none.
fn over_edge([e0, e1]: [Vec3; 2], [f0, f1]: [Vec3; 2]) -> Option<(f32, [f32; 2], Vec3)> {
    let (d1, d2, r) = (e1 - e0, f1 - f0, e0 - f0);
    let across = d1.cross(d2);
    let (a, e) = (d1.length_squared(), d2.length_squared());
    let denom = across.length_squared();
    if denom <= 1e-8 * a * e {
        return None;
    }
    let (b, c, f) = (d1.dot(d2), d1.dot(r), d2.dot(r));
    let s = (b * f - c * e) / denom;
    let t = (a * f - b * c) / denom;
    let across = across / denom.sqrt();
    Some((r.dot(across), [s, t], across))
}

/// When, between nought and one, something that is `at(0)` on one side of
/// nothing and `at(1)` on the other is nothing: found by halving, since
/// what is asked of is a cubic in the time and its ends are all that is
/// known of it.
fn crossing_time(at: impl Fn(f32) -> f32, began: f32) -> f32 {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..20 {
        let mid = (lo + hi) * 0.5;
        if at(mid) * began > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) * 0.5
}

/// Whether two edges passed through each other between then and now, and
/// if so the direction across them as they are now, turned to the side the
/// first came from, where on each they are nearest, and how far apart
/// they began.
///
/// [`went_through`]'s question, of two edges, each point taken to have
/// gone straight from where it was to where it is: the volume the four
/// span changed sign — they were in one plane at some moment between — and
/// at that moment the lines met within both edges. A volume is also
/// nothing when the two run the same way, which is no meeting, and then
/// there is no place on either where they are nearest.
fn edges_went_through(e_then: [Vec3; 2], f_then: [Vec3; 2], e_now: [Vec3; 2], f_now: [Vec3; 2], margin: f32) -> Option<(Vec3, [f32; 2], f32)> {
    let volume = |e: [Vec3; 2], f: [Vec3; 2]| (e[0] - f[0]).dot((e[1] - e[0]).cross(f[1] - f[0]));
    let (v0, v1) = (volume(e_then, f_then), volume(e_now, f_now));
    if v0 == 0.0 || v0 * v1 >= 0.0 {
        return None;
    }
    let between = |when: f32| ([0, 1].map(|i| e_then[i].lerp(e_now[i], when)), [0, 1].map(|i| f_then[i].lerp(f_now[i], when)));
    let when = crossing_time(|t| { let (e, f) = between(t); volume(e, f) }, v0);
    let (e, f) = between(when);
    let (_, at, _) = over_edge(e, f)?;
    if !at.iter().all(|&a| a >= -margin && a <= 1.0 + margin) {
        return None;
    }
    let (began, _, _) = over_edge(e_then, f_then)?;
    let (_, at_now, across) = over_edge(e_now, f_now)?;
    Some((across * v0.signum(), at_now, began.abs()))
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
/// from, and how far over the triangle it began.
///
/// Both move, each point taken to have gone straight from where it was to
/// where it is. It went through if the volume the four span changed sign —
/// the point was in the triangle's plane at some moment between — and at
/// that moment its foot was inside the triangle. Until 2026-09-29 the
/// moment and the foot were read off a straight line between the two ends'
/// heights and weights, which is right for a small step and wrong for a
/// long one: a point carried across several triangles was said to have
/// gone around the one it went through.
fn went_through(p_then: Vec3, tri_then: [Vec3; 3], p_now: Vec3, tri_now: [Vec3; 3], margin: f32) -> Option<(Vec3, f32)> {
    let volume = |p: Vec3, [a, b, c]: [Vec3; 3]| (p - a).dot((b - a).cross(c - a));
    let (v0, v1) = (volume(p_then, tri_then), volume(p_now, tri_now));
    if v0 == 0.0 || v0 * v1 >= 0.0 {
        return None;
    }
    let between = |when: f32| (p_then.lerp(p_now, when), [0, 1, 2].map(|i| tri_then[i].lerp(tri_now[i], when)));
    let when = crossing_time(|t| { let (p, tri) = between(t); volume(p, tri) }, v0);
    let (p, tri) = between(when);
    let (_, w, _) = over_triangle(p, tri)?;
    if !w.iter().all(|&w| w >= -margin) {
        return None;
    }
    let (began, _, _) = over_triangle(p_then, tri_then)?;
    let (_, _, normal) = over_triangle(p_now, tri_now)?;
    Some((normal * v0.signum(), began.abs()))
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
