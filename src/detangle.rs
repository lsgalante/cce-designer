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
        Topo { key, rings, edges: geom.edges().to_vec(), starts, excluded }
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
}

pub fn apply(geom: &mut Detail, target: &FsNode) -> Work {
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
    // Thickness in EDGE LENGTHS, so the setting means the same thing before
    // and after a remesh.
    let mean_edge = topo
        .edges
        .iter()
        .map(|e| (geom.pos(e[1] as usize) - geom.pos(e[0] as usize)).length())
        .sum::<f32>()
        / topo.edges.len() as f32;
    let thickness = node_param_f32(target, "Thickness", 1.0).max(0.0) * mean_edge;
    if thickness <= 0.0 {
        return work;
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
    work
}
