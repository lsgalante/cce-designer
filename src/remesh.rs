//! Incremental isotropic remeshing.
//!
//! The load-bearing half of Phase 3. Without topology that keeps primitives
//! proportional to surface area, every growth simulation degenerates within a
//! few dozen frames: `develop` pushes points apart, the triangles between them
//! stretch, and an attribute diffused across a stretched mesh is being
//! averaged over distances that no longer mean what they meant.
//!
//! The algorithm is Botsch and Kobbelt's, four passes over the mesh repeated a
//! few times:
//!
//! 1. **Split** every edge longer than 4/3 of the target length.
//! 2. **Collapse** every edge shorter than 4/5 of it.
//! 3. **Flip** edges that would bring their four points closer to valence 6.
//! 4. **Relax** each point toward the centroid of its neighbours, with the
//!    normal component removed so the pass smooths the triangulation without
//!    moving the surface.
//!
//! The 4/3 and 4/5 are the paper's, and they are not arbitrary: a window
//! narrower than that lets a split produce two edges short enough for the next
//! collapse to undo, and the mesh oscillates instead of converging.
//!
//! ## What it does with the simulation's data
//!
//! This is the node that Phase 2's contract was built for, so it is careful
//! about identity and attributes:
//!
//! - A **split** allocates a new point and interpolates every attribute from
//!   the two endpoints. A place that did not exist gets values consistent with
//!   its neighbourhood rather than zeros.
//! - A **collapse** keeps one endpoint — its identity and its values — rather
//!   than averaging into a new point. The surviving point is one the solver
//!   has been writing to, and a remesh should cost the simulation as little
//!   memory as it can.
//! - A **flip** and a **relax** change no attributes at all.
//!
//! What it does NOT do is project back onto the input surface, which the
//! paper's fifth pass does. Tangential relaxation alone lets a surface creep
//! slightly over many iterations; the projection needs a spatial index over the
//! original triangles, which is the same index Detangle and Suture will want,
//! and is better built once for all three.

use crate::detail::{AttribData, AttribValue, Detail, PointId};
use glam::Vec3;
use std::collections::HashMap;

/// How the four passes are tuned. Defaults are the paper's.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// The edge length the mesh is steered toward.
    pub target: f32,
    /// How many times the four passes run.
    pub iterations: usize,
    /// Strength of the tangential relaxation, 0 to 1.
    pub relax: f32,
    pub split: bool,
    pub collapse: bool,
    pub flip: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { target: 0.1, iterations: 3, relax: 0.5, split: true, collapse: true, flip: true }
    }
}

/// A triangle mesh in a form that can be edited in place.
///
/// [`Detail`]'s CSR storage is compact and good for reading, which is what
/// every other operator does to it. Remeshing is the one thing that rewires
/// topology per edge, so it converts in, edits, and converts back rather than
/// making every other operator pay for an edit-friendly layout.
///
/// Points and triangles are tombstoned rather than removed during a pass:
/// compacting mid-pass would invalidate every index the pass is holding.
struct Mesh {
    pos: Vec<Vec3>,
    ids: Vec<PointId>,
    /// Per point, its attribute values as loose components, in `attr_names`
    /// order — the form the split interpolation works in.
    attrs: Vec<Vec<f32>>,
    attr_names: Vec<String>,
    attr_types: Vec<crate::detail::AttribType>,
    groups: Vec<(String, Vec<bool>)>,
    tris: Vec<[u32; 3]>,
    dead_point: Vec<bool>,
    dead_tri: Vec<bool>,
    /// Point to the triangles that have referenced it. Maintained as
    /// triangles are added and rewired, and read through a filter that drops
    /// dead entries and ones the point has since been rewired out of — so a
    /// stale entry is harmless and nothing has to be removed eagerly.
    ///
    /// Without this every adjacency question is a scan of the whole mesh, and
    /// the flip pass alone asks four per edge.
    p2t: Vec<Vec<usize>>,
    next_id: PointId,
}

impl Mesh {
    fn from_detail(d: &Detail) -> Mesh {
        let attr_names: Vec<String> = d.points().names().iter().map(|s| s.to_string()).collect();
        let attr_types: Vec<crate::detail::AttribType> = attr_names
            .iter()
            .filter_map(|n| d.points().get(n).map(|a| a.ty()))
            .collect();
        let attrs: Vec<Vec<f32>> = (0..d.num_points())
            .map(|p| {
                attr_names
                    .iter()
                    .flat_map(|n| match d.points().value(n, p) {
                        Some(AttribValue::Float(x)) => vec![x],
                        Some(AttribValue::Float2(x)) => x.to_vec(),
                        Some(AttribValue::Float3(x)) => x.to_vec(),
                        Some(AttribValue::Float4(x)) => x.to_vec(),
                        Some(AttribValue::Int(x)) => vec![x as f32],
                        None => vec![],
                    })
                    .collect()
            })
            .collect();
        let groups: Vec<(String, Vec<bool>)> = d
            .points()
            .group_names()
            .iter()
            .map(|g| {
                (
                    g.to_string(),
                    (0..d.num_points()).map(|p| d.points().in_group(g, p)).collect(),
                )
            })
            .collect();

        // Only triangles are remeshed. A polygon fans on the way in, which is
        // what the renderer does with it anyway.
        let mut tris = Vec::new();
        for prim in 0..d.num_prims() {
            let pts = d.prim_points(prim);
            for i in 1..pts.len().saturating_sub(1) {
                tris.push([pts[0], pts[i], pts[i + 1]]);
            }
        }

        let n = d.num_points();
        let max_id = d.ids().iter().copied().max().map(|m| m + 1).unwrap_or(0);
        let mut p2t: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (t, tri) in tris.iter().enumerate() {
            for &q in tri {
                p2t[q as usize].push(t);
            }
        }
        Mesh {
            pos: (0..n).map(|p| d.pos(p)).collect(),
            ids: d.ids().to_vec(),
            attrs,
            attr_names,
            attr_types,
            groups,
            dead_tri: vec![false; tris.len()],
            tris,
            dead_point: vec![false; n],
            p2t,
            next_id: max_id,
        }
    }

    /// Add a triangle, keeping the incidence in step.
    fn add_tri(&mut self, tri: [u32; 3]) {
        let t = self.tris.len();
        self.tris.push(tri);
        self.dead_tri.push(false);
        for &q in &tri {
            self.p2t[q as usize].push(t);
        }
    }

    /// Point `from` to `to` in triangle `t`, keeping the incidence in step.
    fn rewire(&mut self, t: usize, from: u32, to: u32) {
        for slot in self.tris[t].iter_mut() {
            if *slot == from {
                *slot = to;
            }
        }
        self.p2t[to as usize].push(t);
    }

    /// Live triangles using both endpoints of an edge.
    fn tris_on_edge(&self, a: u32, b: u32) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .p2t
            .get(a as usize)
            .map(|ts| {
                ts.iter()
                    .copied()
                    .filter(|&t| {
                        !self.dead_tri[t] && self.tris[t].contains(&a) && self.tris[t].contains(&b)
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_unstable();
        out.dedup();
        out
    }

    fn into_detail(mut self) -> Detail {
        self.dead_tri.resize(self.tris.len(), false);
        // Drop points nothing references any more, as well as the ones
        // collapse tombstoned: a split-then-collapse can strand a point that
        // was never itself collapsed.
        let mut used = vec![false; self.pos.len()];
        for (t, tri) in self.tris.iter().enumerate() {
            if self.dead_tri[t] {
                continue;
            }
            for &p in tri {
                used[p as usize] = true;
            }
        }

        let mut d = Detail::new();
        let mut remap = vec![u32::MAX; self.pos.len()];
        let mut kept: Vec<usize> = Vec::new();
        for p in 0..self.pos.len() {
            if self.dead_point[p] || !used[p] {
                continue;
            }
            remap[p] = d.add_point(self.pos[p]);
            kept.push(p);
        }
        // Identities are restored rather than re-allocated: a point that
        // survived a remesh is the same point, and the solver has been writing
        // to it.
        // `kept` and the points just added are the same list, so this cannot
        // fail. Asserted rather than discarded because the failure mode is a
        // silently renumbered mesh, which a simulation would experience as
        // every point forgetting itself at once.
        d.set_ids(kept.iter().map(|&p| self.ids[p]).collect(), self.next_id)
            .expect("one identity per surviving point");

        for (t, tri) in self.tris.iter().enumerate() {
            if self.dead_tri[t] {
                continue;
            }
            let mapped = [remap[tri[0] as usize], remap[tri[1] as usize], remap[tri[2] as usize]];
            if mapped.iter().any(|&m| m == u32::MAX) || mapped[0] == mapped[1] || mapped[1] == mapped[2] || mapped[0] == mapped[2] {
                continue;
            }
            d.add_prim(&mapped);
        }

        let mut offset = 0usize;
        for (i, name) in self.attr_names.iter().enumerate() {
            let ty = self.attr_types[i];
            let k = ty.components();
            let mut data = AttribData::zeroed(ty, kept.len());
            for (new, &old) in kept.iter().enumerate() {
                let row = &self.attrs[old];
                let comps: Vec<f32> = (0..k).map(|c| row.get(offset + c).copied().unwrap_or(0.0)).collect();
                let _ = data.set(new, components(ty, &comps));
            }
            let _ = d.points_mut().insert(name, data);
            offset += k;
        }
        for (name, members) in &self.groups {
            d.points_mut().create_group(name);
            for (new, &old) in kept.iter().enumerate() {
                if members.get(old).copied().unwrap_or(false) {
                    d.points_mut().add_to_group(name, new);
                }
            }
        }
        d
    }

    /// A point halfway along an edge, with every attribute interpolated.
    fn split_point(&mut self, a: u32, b: u32) -> u32 {
        let (a, b) = (a as usize, b as usize);
        let pos = (self.pos[a] + self.pos[b]) * 0.5;
        let attrs: Vec<f32> = self.attrs[a]
            .iter()
            .zip(self.attrs[b].iter())
            .map(|(x, y)| (x + y) * 0.5)
            .collect();
        self.pos.push(pos);
        self.attrs.push(attrs);
        self.ids.push(self.next_id);
        self.next_id += 1;
        self.dead_point.push(false);
        self.p2t.push(Vec::new());
        // A new point joins a group only where BOTH its parents were in it: a
        // point that is half in a selection is not in it, and the alternative
        // grows every group along its own boundary every time the mesh is
        // remeshed.
        for (_, members) in self.groups.iter_mut() {
            let inherits = members.get(a).copied().unwrap_or(false)
                && members.get(b).copied().unwrap_or(false);
            members.push(inherits);
        }
        (self.pos.len() - 1) as u32
    }

    /// Live triangles touching a point.
    fn tris_of(&self, p: u32) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .p2t
            .get(p as usize)
            .map(|ts| {
                ts.iter()
                    .copied()
                    .filter(|&t| !self.dead_tri[t] && self.tris[t].contains(&p))
                    .collect()
            })
            .unwrap_or_default();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Unique live edges, each as `[low, high]`, with the triangles on them.
    fn edges(&self) -> Vec<([u32; 2], Vec<usize>)> {
        let mut map: HashMap<[u32; 2], Vec<usize>> = HashMap::new();
        for (t, tri) in self.tris.iter().enumerate() {
            if self.dead_tri[t] {
                continue;
            }
            for i in 0..3 {
                let (a, b) = (tri[i], tri[(i + 1) % 3]);
                map.entry([a.min(b), a.max(b)]).or_default().push(t);
            }
        }
        let mut out: Vec<([u32; 2], Vec<usize>)> = map.into_iter().collect();
        // Sorted, because HashMap order would make the result depend on the
        // hasher's seed and a remesh has to be reproducible.
        out.sort_unstable_by_key(|(e, _)| *e);
        out
    }

    fn len_of(&self, e: [u32; 2]) -> f32 {
        (self.pos[e[1] as usize] - self.pos[e[0] as usize]).length()
    }
}

fn components(ty: crate::detail::AttribType, c: &[f32]) -> AttribValue {
    let at = |i: usize| c.get(i).copied().unwrap_or(0.0);
    match ty {
        crate::detail::AttribType::Float => AttribValue::Float(at(0)),
        crate::detail::AttribType::Float2 => AttribValue::Float2([at(0), at(1)]),
        crate::detail::AttribType::Float3 => AttribValue::Float3([at(0), at(1), at(2)]),
        crate::detail::AttribType::Float4 => AttribValue::Float4([at(0), at(1), at(2), at(3)]),
        crate::detail::AttribType::Int => AttribValue::Int(at(0).round() as i32),
    }
}

/// Split every edge longer than 4/3 of the target.
fn split_pass(m: &mut Mesh, target: f32) -> usize {
    let long = target * 4.0 / 3.0;
    let mut done = 0;
    // The edge LIST is a snapshot — the pass decides up front which edges it
    // will consider, so a split cannot cascade within one pass. The TRIANGLES
    // are looked up at the moment of the split: an earlier split in the same
    // pass has already replaced the faces this edge sits on, and acting on
    // the snapshot's stale indices is what tears the surface open.
    for (e, _) in m.edges() {
        if m.dead_point[e[0] as usize] || m.dead_point[e[1] as usize] || m.len_of(e) <= long {
            continue;
        }
        let tris = m.tris_on_edge(e[0], e[1]);
        if tris.is_empty() {
            continue;
        }
        let mid = m.split_point(e[0], e[1]);
        for t in tris {
            if m.dead_tri[t] {
                continue;
            }
            let tri = m.tris[t];
            // The corner opposite the split edge; the triangle becomes two,
            // each keeping the original winding.
            let Some(i) = (0..3).find(|&i| !e.contains(&tri[i])) else { continue };
            let (opp, x, y) = (tri[i], tri[(i + 1) % 3], tri[(i + 2) % 3]);
            m.dead_tri[t] = true;
            m.add_tri([opp, x, mid]);
            m.add_tri([opp, mid, y]);
        }
        done += 1;
    }
    done
}

/// Collapse every edge shorter than 4/5 of the target.
///
/// The survivor keeps its identity and values; the other endpoint is
/// tombstoned and every triangle referencing it is rewired. Collapses that
/// would leave a neighbour edge too long are refused, which is what stops the
/// pass from undoing the splits that just ran.
fn collapse_pass(m: &mut Mesh, target: f32) -> usize {
    let short = target * 4.0 / 5.0;
    let long = target * 4.0 / 3.0;
    let mut done = 0;
    for (e, _) in m.edges() {
        let (a, b) = (e[0], e[1]);
        if m.dead_point[a as usize] || m.dead_point[b as usize] || m.len_of(e) >= short {
            continue;
        }
        // Would the survivor end up with an edge that the next split pass
        // would just cut again? Then leave it: two passes undoing each other
        // is how a remesh oscillates instead of converging.
        let keep = m.pos[a as usize];
        let too_long = m.tris_of(b).iter().any(|&t| {
            m.tris[t]
                .iter()
                .any(|&q| q != b && q != a && (m.pos[q as usize] - keep).length() > long)
        });
        if too_long {
            continue;
        }
        // Refuse a collapse that would flip a triangle over: if any triangle
        // keeping both points would end up facing the other way, the surface
        // would self-intersect where it used to be flat.
        let folds = m.tris_of(b).iter().any(|&t| {
            let tri = m.tris[t];
            if tri.contains(&a) {
                return false;
            }
            let before = face_normal(m, tri);
            let after_tri = tri.map(|q| if q == b { a } else { q });
            let after = face_normal(m, after_tri);
            before.dot(after) <= 0.0
        });
        if folds {
            continue;
        }

        m.dead_point[b as usize] = true;
        for t in m.tris_of(b) {
            if m.dead_tri[t] {
                continue;
            }
            if m.tris[t].contains(&a) {
                // The two triangles along the collapsed edge fold to nothing.
                m.dead_tri[t] = true;
                continue;
            }
            m.rewire(t, b, a);
        }
        done += 1;
    }
    done
}

fn face_normal(m: &Mesh, tri: [u32; 3]) -> Vec3 {
    let (a, b, c) = (
        m.pos[tri[0] as usize],
        m.pos[tri[1] as usize],
        m.pos[tri[2] as usize],
    );
    (b - a).cross(c - a)
}

/// Flip edges whose two triangles would be better shaped the other way.
///
/// "Better" is total deviation from valence 6, which is the valence a regular
/// triangulation of a plane has — the measure the paper uses, and the one that
/// drives a mesh toward equilateral triangles.
fn flip_pass(m: &mut Mesh) -> usize {
    let mut done = 0;
    for (e, _) in m.edges() {
        // Looked up now rather than taken from the snapshot, for the same
        // reason the split pass does: an earlier flip has rewired faces.
        let tris = m.tris_on_edge(e[0], e[1]);
        if tris.len() != 2 {
            continue; // a boundary edge has nothing to flip into
        }
        let (t0, t1) = (tris[0], tris[1]);
        let Some(&o0) = m.tris[t0].iter().find(|q| !e.contains(q)) else { continue };
        let Some(&o1) = m.tris[t1].iter().find(|q| !e.contains(q)) else { continue };
        if o0 == o1 {
            continue;
        }

        let val = |p: u32| m.tris_of(p).len() as i32;
        let dev = |v: i32| (v - 6).abs();
        let before = dev(val(e[0])) + dev(val(e[1])) + dev(val(o0)) + dev(val(o1));
        // The flip moves one triangle off each endpoint and onto each opposite
        // corner.
        let after = dev(val(e[0]) - 1) + dev(val(e[1]) - 1) + dev(val(o0) + 1) + dev(val(o1) + 1);
        if after >= before {
            continue;
        }
        // Refuse a flip that would fold either new triangle against the
        // surface it came from.
        let n0 = face_normal(m, m.tris[t0]);
        let (new0, new1) = ([o0, e[0], o1], [o1, e[1], o0]);
        if face_normal(m, new0).dot(n0) <= 0.0 || face_normal(m, new1).dot(n0) <= 0.0 {
            continue;
        }
        // Rewiring both triangles wholesale, so the incidence is rebuilt for
        // the corners that changed.
        m.tris[t0] = new0;
        m.tris[t1] = new1;
        for &q in new0.iter().chain(new1.iter()) {
            m.p2t[q as usize].push(t0);
            m.p2t[q as usize].push(t1);
        }
        done += 1;
    }
    done
}

/// Move each point toward the centroid of its neighbours, with the normal
/// component removed.
///
/// Removing the normal component is what makes this a retriangulation rather
/// than a smooth: the points slide within the surface to even out the
/// triangles, and the shape they describe is left where it was.
fn relax_pass(m: &mut Mesh, amount: f32) {
    if amount <= 0.0 {
        return;
    }
    let mut nbrs: Vec<Vec<u32>> = vec![Vec::new(); m.pos.len()];
    for (t, tri) in m.tris.iter().enumerate() {
        if m.dead_tri[t] {
            continue;
        }
        for i in 0..3 {
            let (a, b) = (tri[i], tri[(i + 1) % 3]);
            if !nbrs[a as usize].contains(&b) {
                nbrs[a as usize].push(b);
            }
            if !nbrs[b as usize].contains(&a) {
                nbrs[b as usize].push(a);
            }
        }
    }
    let mut normals: Vec<Vec3> = vec![Vec3::ZERO; m.pos.len()];
    for (t, tri) in m.tris.iter().enumerate() {
        if m.dead_tri[t] {
            continue;
        }
        let n = face_normal(m, *tri);
        for &p in tri {
            normals[p as usize] += n;
        }
    }

    let before = m.pos.clone();
    for p in 0..m.pos.len() {
        if m.dead_point[p] || nbrs[p].is_empty() {
            continue;
        }
        let centroid: Vec3 =
            nbrs[p].iter().map(|&q| before[q as usize]).sum::<Vec3>() / nbrs[p].len() as f32;
        let mut delta = (centroid - before[p]) * amount;
        let n = normals[p].normalize_or_zero();
        if n != Vec3::ZERO {
            delta -= n * delta.dot(n);
        }
        m.pos[p] = before[p] + delta;
    }
}

/// Remesh toward `settings.target` edge length.
pub fn remesh(input: &Detail, settings: Settings) -> Detail {
    if input.num_prims() == 0 || settings.target <= 0.0 {
        return input.clone();
    }
    let mut m = Mesh::from_detail(input);
    for _ in 0..settings.iterations.min(20) {
        if settings.split {
            split_pass(&mut m, settings.target);
        }
        if settings.collapse {
            collapse_pass(&mut m, settings.target);
        }
        if settings.flip {
            flip_pass(&mut m);
        }
        relax_pass(&mut m, settings.relax.clamp(0.0, 1.0));
    }
    m.into_detail()
}
