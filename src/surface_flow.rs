//! The Diffuse and Concentrate nodes: a point attribute flowing over the
//! SURFACE of a mesh, for use inside a simulation.
//!
//! The Neighbour node's Diffuse and Concentrate modes move each value a
//! fraction of the way toward (or away from) the plain average of its
//! neighbours. That is a filter, and as a step of a simulation it has three
//! faults: it is a fact about the TESSELLATION (a finer mesh spreads a value
//! fewer world units per step, and a point with seven neighbours is pulled
//! differently from one with five), it conserves nothing (a spike's total
//! changes as it spreads), and its Concentrate grows without bound. These two
//! nodes are the simulation's versions, and both are built on one thing:
//!
//! - **The surface's own Laplacian.** Each point holds the area around it (a
//!   third of each triangle it is a corner of) and each edge carries the
//!   cotangent weight — half the sum of the cotangents of the two angles
//!   facing it. That is the discrete Laplace–Beltrami operator, the one that
//!   converges to the smooth surface's as the mesh is refined, so a Rate in
//!   square world units means the same on a coarse mesh and a fine one. A
//!   negative weight (an edge facing two obtuse angles) is taken as zero:
//!   it costs exactness on badly shaped triangles and buys the maximum
//!   principle — no value diffuses above the highest or below the lowest it
//!   started from — which a simulation needs more.
//! - **Flux, not averaging.** Whatever leaves one point along an edge arrives
//!   at the other, so the total — each value times its point's area — is
//!   conserved exactly, by both nodes. An open boundary lets nothing out.
//!
//! **Diffuse** is the heat equation, taken as one IMPLICIT step (backward
//! Euler, solved by preconditioned conjugate gradients): stable at any Rate
//! and any step, where an explicit step past its limit rings and blows up.
//!
//! **Concentrate** is the reverse — value flows UP a gradient — which as an
//! equation has no stable form at all: anything run backward from smooth
//! sharpens fastest at the finest scale. What makes it usable is a limiter:
//! a point gives along its edges only what it holds above the FLOOR — zero,
//! or the lowest value anywhere when the step began if that is lower — so
//! values gather into peaks, a point can be emptied but never overdrawn, and
//! the total is still conserved. Zero rather than simply the lowest value,
//! because with Follow naming another attribute a UNIFORM density has to be
//! able to move, and with the lowest value as the floor it would hold
//! nothing to give. It is
//! explicit, cut into as many internal steps as its stiffness asks for, so a
//! Rate means the same whatever the simulation's substep count.
//!
//! Points outside the node's Group hold their values: they are read as
//! neighbours and feed or drain the points beside them, as a fixed
//! temperature does at the edge of a plate, so the total is conserved only
//! when the Group is the whole mesh.

use crate::app::FsNode;
use crate::detail::{AttribType, AttribValue, Detail};
use crate::geometry::{
    generate_single_node_geometry_with_errors, node_param_bool, node_param_f32, node_param_str, param_node, EvalSim,
};

/// A mesh's Laplacian: each point's area and its edges' cotangent weights,
/// in CSR form with every edge listed from both ends.
pub struct Surface {
    /// The area each point stands for: a third of every triangle it is a
    /// corner of.
    pub mass: Vec<f64>,
    /// Where each point's edges begin in `nbr` and `w`; `n + 1` long.
    pub start: Vec<usize>,
    pub nbr: Vec<u32>,
    /// The edge's cotangent weight, never negative.
    pub w: Vec<f64>,
}

impl Surface {
    /// The Laplacian of `geom`'s primitives, fan-triangulated. A point on no
    /// triangle has no area and no edges.
    pub fn of(geom: &Detail) -> Surface {
        let n = geom.num_points();
        let pos: Vec<[f64; 3]> = geom
            .positions()
            .iter()
            .map(|p| [p[0] as f64, p[1] as f64, p[2] as f64])
            .collect();
        let tris = geom.triangulate_points();
        let mut mass = vec![0.0f64; n];
        let mut half: Vec<(u32, u32, f64)> = Vec::with_capacity(tris.len());
        let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
        };
        for t in tris.chunks_exact(3) {
            let idx = [t[0] as usize, t[1] as usize, t[2] as usize];
            if idx[0] == idx[1] || idx[1] == idx[2] || idx[0] == idx[2] {
                continue;
            }
            let [a, b, c] = idx.map(|i| pos[i]);
            let twice_area = dot(cross(sub(b, a), sub(c, a)), cross(sub(b, a), sub(c, a))).sqrt();
            // A sliver with no area has no angles worth the name: its
            // cotangents are a division by nothing.
            if !(twice_area > 1e-14) {
                continue;
            }
            for &i in &idx {
                mass[i] += twice_area / 6.0;
            }
            // The corner at k faces the edge between the other two; its
            // cotangent is cos/sin = dot/|cross| of the two sides leaving it,
            // and |cross| at any corner is twice the area.
            for k in 0..3 {
                let (o, i, j) = (idx[k], idx[(k + 1) % 3], idx[(k + 2) % 3]);
                let cot = dot(sub(pos[i], pos[o]), sub(pos[j], pos[o])) / twice_area;
                let (lo, hi) = (i.min(j) as u32, i.max(j) as u32);
                half.push((lo, hi, 0.5 * cot));
            }
        }
        half.sort_unstable_by_key(|&(a, b, _)| (a, b));
        let mut edges: Vec<(u32, u32, f64)> = Vec::new();
        for (a, b, w) in half {
            match edges.last_mut() {
                Some(e) if e.0 == a && e.1 == b => e.2 += w,
                _ => edges.push((a, b, w)),
            }
        }
        // The maximum principle over exactness: see the module comment.
        let mut count = vec![0usize; n + 1];
        for &(a, b, _) in &edges {
            count[a as usize + 1] += 1;
            count[b as usize + 1] += 1;
        }
        for i in 0..n {
            count[i + 1] += count[i];
        }
        let start = count;
        let mut fill = start.clone();
        let mut nbr = vec![0u32; start[n]];
        let mut w = vec![0.0f64; start[n]];
        for &(a, b, ew) in &edges {
            let ew = ew.max(0.0);
            for (from, to) in [(a, b), (b, a)] {
                let at = fill[from as usize];
                nbr[at] = to;
                w[at] = ew;
                fill[from as usize] += 1;
            }
        }
        Surface { mass, start, nbr, w }
    }

    pub fn len(&self) -> usize {
        self.mass.len()
    }

    /// Every edge of `i` as (neighbour, weight).
    fn edges(&self, i: usize) -> impl Iterator<Item = (usize, f64)> + '_ {
        (self.start[i]..self.start[i + 1]).map(move |e| (self.nbr[e] as usize, self.w[e]))
    }

    /// The total of a value over the surface: each point's value times its
    /// area. What both operators conserve.
    pub fn total(&self, u: &[f64]) -> f64 {
        self.mass.iter().zip(u).map(|(m, x)| m * x).sum()
    }
}

/// An edge's conductance under a per-point rate: the weight times the mean
/// of its ends' rates, so the edge is the same seen from either end and what
/// it carries is still conserved.
fn conductance(rate: &[f64], i: usize, j: usize, w: f64) -> f64 {
    w * 0.5 * (rate[i] + rate[j])
}

/// One backward-Euler step of the heat equation, `(M + L) u' = M u`, where
/// `rate` already holds each point's Rate times the step. Points that are not
/// `free` keep their values and enter the free points' equations as known.
/// Returns the conjugate-gradient iterations it took.
pub fn diffuse(s: &Surface, rate: &[f64], free: &[bool], u: &mut [f64]) -> usize {
    let n = s.len();
    // A point is solved for when it is free and has something to be solved
    // from — an area or an edge. One with neither (a point on no triangle)
    // would be a row of zeros.
    let mut diag = vec![0.0f64; n];
    let mut solve = vec![false; n];
    for i in 0..n {
        let k: f64 = s.edges(i).map(|(j, w)| conductance(rate, i, j, w)).sum();
        diag[i] = s.mass[i] + k;
        solve[i] = free[i] && diag[i] > 0.0 && k > 0.0;
    }
    let mut b = vec![0.0f64; n];
    for i in (0..n).filter(|&i| solve[i]) {
        b[i] = s.mass[i] * u[i]
            + s.edges(i)
                .filter(|&(j, _)| !solve[j])
                .map(|(j, w)| conductance(rate, i, j, w) * u[j])
                .sum::<f64>();
    }
    let apply = |x: &[f64], y: &mut [f64]| {
        for i in 0..n {
            y[i] = if solve[i] {
                diag[i] * x[i]
                    - s.edges(i)
                        .filter(|&(j, _)| solve[j])
                        .map(|(j, w)| conductance(rate, i, j, w) * x[j])
                        .sum::<f64>()
            } else {
                0.0
            };
        }
    };
    let mut x: Vec<f64> = (0..n).map(|i| if solve[i] { u[i] } else { 0.0 }).collect();
    let mut ax = vec![0.0f64; n];
    apply(&x, &mut ax);
    let mut r: Vec<f64> = (0..n).map(|i| b[i] - ax[i]).collect();
    let norm_b = b.iter().map(|v| v * v).sum::<f64>().sqrt();
    let tol = 1e-10 * norm_b.max(1e-300);
    let precond = |r: &[f64]| -> Vec<f64> { (0..n).map(|i| if solve[i] { r[i] / diag[i] } else { 0.0 }).collect() };
    let mut z = precond(&r);
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let mut ap = vec![0.0f64; n];
    let mut iterations = 0;
    while iterations < 4 * n.max(16) {
        if r.iter().map(|v| v * v).sum::<f64>().sqrt() <= tol {
            break;
        }
        apply(&p, &mut ap);
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if !(pap > 0.0) {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        z = precond(&r);
        let rz_next: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = rz_next / rz;
        rz = rz_next;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
        iterations += 1;
    }
    for i in (0..n).filter(|&i| solve[i]) {
        u[i] = x[i];
    }
    iterations
}

/// How hard a Concentrate draws a point on.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Response {
    /// By the difference alone: the heat equation run backward.
    Difference,
    /// The difference times what the giving point holds above the floor —
    /// the aggregation term of Keller–Segel. Against a Diffuse at the same
    /// Rate it gathers where the value stands more than one above the floor
    /// and spreads where it stands less.
    Amount,
}

/// The most internal steps one Concentrate takes, however stiff.
pub const CONCENTRATE_STEPS_MAX: usize = 64;

/// Value flowing up the gradient of `signal` (of `u` itself when `None`),
/// with `rate` already each point's Rate times the step. Conserved, never
/// below the floor, and cut into internal steps so that no point is asked to
/// change by more than about half what it holds in one. Returns the steps.
pub fn concentrate(s: &Surface, rate: &[f64], free: &[bool], u: &mut [f64], signal: Option<&[f64]>, response: Response) -> usize {
    let n = s.len();
    if n == 0 {
        return 0;
    }
    let floor = u.iter().copied().fold(0.0f64, f64::min);
    let top = u.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    // The stiffness: what one point could be asked to pass on in a step, as
    // a fraction of what it holds — the sum of its conductances over its
    // area, times the spread a signal or an Amount puts on top.
    let spread = |v: &[f64]| {
        let (lo, hi) = v.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x)));
        (hi - lo).max(0.0)
    };
    let scale = match (signal, response) {
        (None, Response::Difference) => 1.0,
        (None, Response::Amount) => top - floor,
        (Some(sig), Response::Difference) => spread(sig) / (top - floor).max(1e-12),
        (Some(sig), Response::Amount) => spread(sig),
    };
    let stiffness = (0..n)
        .filter(|&i| s.mass[i] > 0.0)
        .map(|i| s.edges(i).map(|(j, w)| conductance(rate, i, j, w)).sum::<f64>() / s.mass[i])
        .fold(0.0f64, f64::max)
        * scale;
    // With every value on the floor no point has anything to give.
    if !(stiffness > 0.0) || !stiffness.is_finite() || !(top > floor) {
        return 0;
    }
    let steps = ((stiffness / 0.5).ceil() as usize).clamp(1, CONCENTRATE_STEPS_MAX);
    let part = 1.0 / steps as f64;

    let mut flows: Vec<(usize, usize, f64)> = Vec::new();
    let mut out = vec![0.0f64; n];
    for _ in 0..steps {
        flows.clear();
        out.iter_mut().for_each(|o| *o = 0.0);
        let sig: &[f64] = signal.unwrap_or(u);
        for i in 0..n {
            for (j, w) in s.edges(i).filter(|&(j, _)| j > i) {
                let ds = sig[j] - sig[i];
                // Up the gradient: the lower end gives.
                let (give, take) = if ds > 0.0 { (i, j) } else if ds < 0.0 { (j, i) } else { continue };
                if !free[give] && !free[take] || s.mass[give] <= 0.0 || s.mass[take] <= 0.0 {
                    continue;
                }
                let mut f = conductance(rate, i, j, w) * part * ds.abs();
                if response == Response::Amount {
                    f *= (u[give] - floor).max(0.0);
                }
                if f > 0.0 {
                    flows.push((give, take, f));
                    out[give] += f;
                }
            }
        }
        // The limiter: a point gives at most what it holds above the floor,
        // all its edges cut back by the same fraction. A held point (outside
        // the Group) is limited the same way, though it loses nothing.
        let cut: Vec<f64> = (0..n)
            .map(|i| {
                let have = (u[i] - floor).max(0.0) * s.mass[i];
                if out[i] > have { have / out[i] } else { 1.0 }
            })
            .collect();
        for &(give, take, f) in &flows {
            let f = f * cut[give];
            if free[give] {
                u[give] -= f / s.mass[give];
            }
            if free[take] {
                u[take] += f / s.mass[take];
            }
        }
        // A giver drained exactly to the floor can land a rounding under it.
        for v in u.iter_mut() {
            if *v < floor {
                *v = floor;
            }
        }
    }
    steps
}

/// Which of the two nodes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Flow {
    Diffuse,
    Concentrate,
}

impl Flow {
    fn label(self) -> &'static str {
        match self {
            Flow::Diffuse => "Diffuse",
            Flow::Concentrate => "Concentrate",
        }
    }
}

pub fn resolve(
    root: &FsNode,
    target: &FsNode,
    flow: Flow,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    if let Err(why) = apply(&mut geom, target, flow) {
        if ocl_error.is_none() {
            *ocl_error = Some(format!("{} '{}': {}", flow.label(), target.name, why));
        }
    }
    Some(geom)
}

/// A point attribute as one column per component.
fn columns(geom: &Detail, name: &str, k: usize) -> Vec<Vec<f64>> {
    let mut cols = vec![vec![0.0f64; geom.num_points()]; k];
    for p in 0..geom.num_points() {
        let Some(v) = geom.points().value(name, p) else { continue };
        let c: &[f32] = match &v {
            AttribValue::Float(x) => std::slice::from_ref(x),
            AttribValue::Float2(x) => x,
            AttribValue::Float3(x) => x,
            AttribValue::Float4(x) => x,
            AttribValue::Int(_) => &[],
        };
        if let AttribValue::Int(i) = v {
            cols[0][p] = i as f64;
        }
        for (col, x) in cols.iter_mut().zip(c) {
            col[p] = *x as f64;
        }
    }
    cols
}

/// Either node over geometry in hand. Leaves the geometry untouched on an
/// error, which says why.
pub fn apply(geom: &mut Detail, target: &FsNode, flow: Flow) -> Result<(), String> {
    let name = node_param_str(target, "attribute", "").trim().to_string();
    if name.is_empty() {
        return Ok(());
    }
    let Some(ty) = geom.points().get(&name).map(|a| a.ty()) else {
        return Err(format!("no point attribute named '{name}'"));
    };
    let n = geom.num_points();
    let k = ty.components();

    let per_frame = node_param_bool(target, "per_frame", false);
    let dt = if per_frame { geom.detail().value("dt", 0).map_or(1.0, |v| v.as_f32()) } else { 1.0 };
    let dt = if dt.is_finite() && dt > 0.0 { dt as f64 } else { 1.0 };
    let base = (node_param_f32(target, "rate", 0.01) as f64).max(0.0) * dt;
    if base == 0.0 || n == 0 {
        return Ok(());
    }
    let rate_by = node_param_str(target, "rate_by", "").trim().to_string();
    let rate: Vec<f64> = if rate_by.is_empty() {
        vec![base; n]
    } else if !geom.points().has(&rate_by) {
        return Err(format!("Rate By '{rate_by}' is not a point attribute"));
    } else {
        (0..n)
            .map(|p| base * geom.points().value(&rate_by, p).map_or(0.0, |v| v.as_f32() as f64).max(0.0))
            .collect()
    };
    let group = node_param_str(target, "group", "").trim().to_string();
    let free: Vec<bool> = (0..n).map(|p| group.is_empty() || geom.points().in_group(&group, p)).collect();

    let surface = Surface::of(geom);
    if surface.w.is_empty() {
        return Err("no surface to flow over (it needs triangles or polygons)".into());
    }

    let mut cols = columns(geom, &name, k);
    match flow {
        Flow::Diffuse => {
            for col in cols.iter_mut() {
                diffuse(&surface, &rate, &free, col);
            }
        }
        Flow::Concentrate => {
            let response = if node_param_str(target, "response", "Difference").eq_ignore_ascii_case("amount") {
                Response::Amount
            } else {
                Response::Difference
            };
            let follow = node_param_str(target, "follow", "").trim().to_string();
            let signal: Option<Vec<Vec<f64>>> = if follow.is_empty() || follow == name {
                None
            } else {
                let Some(fty) = geom.points().get(&follow).map(|a| a.ty()) else {
                    return Err(format!("Follow '{follow}' is not a point attribute"));
                };
                let fk = fty.components();
                if fk != 1 && fk != k {
                    return Err(format!("Follow '{follow}' is {} wide and '{name}' {k}; it must be one number or as wide", fk));
                }
                Some(columns(geom, &follow, fk))
            };
            for (c, col) in cols.iter_mut().enumerate() {
                let sig = signal.as_ref().map(|s| s[if s.len() == 1 { 0 } else { c }].as_slice());
                concentrate(&surface, &rate, &free, col, sig, response);
            }
        }
    }

    for p in 0..n {
        let at = |c: usize| cols[c][p] as f32;
        let value = match ty {
            AttribType::Float => AttribValue::Float(at(0)),
            AttribType::Float2 => AttribValue::Float2([at(0), at(1)]),
            AttribType::Float3 => AttribValue::Float3([at(0), at(1), at(2)]),
            AttribType::Float4 => AttribValue::Float4([at(0), at(1), at(2), at(3)]),
            AttribType::Int => AttribValue::Int(cols[0][p].round() as i32),
        };
        let _ = geom.points_mut().set_value(&name, p, value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ParamDef;
    use crate::geometry::{sphere_detail, SimCache};
    use glam::Vec3;

    fn node(name: &str, node_type: &str, params: &[(&str, &str)], children: Vec<FsNode>) -> FsNode {
        FsNode {
            id: format!("id-{name}"),
            inputs: 1,
            outputs: 1,
            name: name.to_string(),
            node_type: node_type.to_string(),
            children,
            params: params.iter().map(|(k, v)| ParamDef::new(k.to_string(), "text".to_string(), v.to_string())).collect(),
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
        }
    }

    /// A unit sphere carrying `mass`, set per point from its position.
    fn sphere(rows: usize, cols: usize, mass: impl Fn(Vec3) -> f32) -> Detail {
        let mut g = sphere_detail(Vec3::ZERO, 1.0, rows, cols);
        g.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..g.num_points() {
            let m = mass(g.pos(p));
            g.points_mut().set_value("mass", p, AttribValue::Float(m)).unwrap();
        }
        g
    }

    fn mass(g: &Detail) -> Vec<f64> {
        (0..g.num_points()).map(|p| g.points().value("mass", p).unwrap().as_f32() as f64).collect()
    }

    fn run(g: &Detail, flow: Flow, params: &[(&str, &str)]) -> Detail {
        let mut all = vec![("attribute", "mass"), ("per_frame", "true")];
        all.extend_from_slice(params);
        let mut out = g.clone();
        apply(&mut out, &node("flow1", "x", &all, vec![]), flow).expect("applies");
        out
    }

    #[test]
    fn diffuse_spreads_a_spike_and_conserves_its_total() {
        let before = sphere(24, 32, |p| if p.y > 0.95 { 1.0 } else { 0.0 });
        let after = run(&before, Flow::Diffuse, &[("rate", "0.02")]);
        let s = Surface::of(&before);
        let (u0, u1) = (mass(&before), mass(&after));
        let (t0, t1) = (s.total(&u0), s.total(&u1));
        assert!((t1 - t0).abs() < 1e-5 * t0, "total {t0} became {t1}");
        let max = u1.iter().copied().fold(f64::MIN, f64::max);
        let min = u1.iter().copied().fold(f64::MAX, f64::min);
        assert!(max < 1.0 && min >= -1e-6, "out of the range it started in: {min}..{max}");
        let reached = (0..u1.len()).filter(|&p| u0[p] == 0.0 && u1[p] > 1e-4).count();
        assert!(reached > 0, "nothing spread");
        // Nothing reached the far pole in one short step.
        let south = (0..u1.len()).find(|&p| before.pos(p).y < -0.99).unwrap();
        assert!(u1[south] < 1e-6);
    }

    /// Height on a unit sphere is an eigenfunction of its Laplacian, with
    /// eigenvalue -2, so one backward-Euler step of Rate r scales it by
    /// exactly 1 / (1 + 2r) — on the smooth sphere, and so on any mesh fine
    /// enough to stand for it, whatever its triangles.
    #[test]
    fn diffuse_is_a_rate_over_the_surface_and_not_the_tessellation() {
        for (rows, cols) in [(12, 16), (24, 32), (48, 64)] {
            let before = sphere(rows, cols, |p| p.y);
            let after = run(&before, Flow::Diffuse, &[("rate", "0.10")]);
            let (u0, u1) = (mass(&before), mass(&after));
            let top = (0..u0.len()).max_by(|&a, &b| u0[a].total_cmp(&u0[b])).unwrap();
            let ratio = u1[top] / u0[top];
            assert!((ratio - 1.0 / 1.2).abs() < 0.01, "{rows}x{cols}: the pole decayed to {ratio}, not 0.833");
        }
    }

    #[test]
    fn diffuse_holds_what_is_outside_its_group_and_leaves_a_uniform_field_alone() {
        let mut before = sphere(16, 24, |p| if p.y > 0.0 { 1.0 } else { 0.0 });
        before.points_mut().create_group("north");
        for p in 0..before.num_points() {
            if before.pos(p).y > 0.0 {
                before.points_mut().add_to_group("north", p);
            }
        }
        let after = run(&before, Flow::Diffuse, &[("rate", "0.05"), ("group", "north")]);
        let (u0, u1) = (mass(&before), mass(&after));
        for p in 0..u0.len() {
            if before.pos(p).y <= 0.0 {
                assert_eq!(u0[p], u1[p], "point {p} is outside the group");
            }
        }
        assert!((0..u0.len()).any(|p| u1[p] < u0[p] - 1e-4), "the cold half drained nothing");

        let flat = sphere(16, 24, |_| 0.5);
        let same = run(&flat, Flow::Diffuse, &[("rate", "1.0")]);
        for (a, b) in mass(&flat).iter().zip(mass(&same)) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn concentrate_gathers_into_peaks_conserving_the_total_and_never_below_zero() {
        // A gentle bump: the top gathers from the slope below it.
        let before = sphere(24, 32, |p| 1.0 + 0.1 * p.y);
        let after = run(&before, Flow::Concentrate, &[("rate", "0.05")]);
        let s = Surface::of(&before);
        let (u0, u1) = (mass(&before), mass(&after));
        let (t0, t1) = (s.total(&u0), s.total(&u1));
        assert!((t1 - t0).abs() < 1e-5 * t0, "total {t0} became {t1}");
        let max0 = u0.iter().copied().fold(f64::MIN, f64::max);
        let max1 = u1.iter().copied().fold(f64::MIN, f64::max);
        assert!(max1 > max0 + 1e-3, "the peak did not grow: {max0} -> {max1}");

        // Hard enough to empty points: they stop at zero, and the total holds.
        let mut g = before.clone();
        for _ in 0..20 {
            g = run(&g, Flow::Concentrate, &[("rate", "1.0")]);
        }
        let u = mass(&g);
        assert!(u.iter().all(|&v| v >= 0.0), "overdrawn: {}", u.iter().copied().fold(f64::MAX, f64::min));
        assert!(u.iter().any(|&v| v < 1e-3), "nothing emptied");
        assert!((s.total(&u) - t0).abs() < 1e-4 * t0);
    }

    #[test]
    fn concentrate_follows_another_attribute_up_its_gradient() {
        let mut before = sphere(16, 24, |_| 1.0);
        before.points_mut().create("food", AttribValue::Float(0.0));
        for p in 0..before.num_points() {
            let y = before.pos(p).y;
            before.points_mut().set_value("food", p, AttribValue::Float(y)).unwrap();
        }
        for response in ["Difference", "Amount"] {
            let after = run(&before, Flow::Concentrate, &[("rate", "0.05"), ("follow", "food"), ("response", response)]);
            let u = mass(&after);
            let pole = |north: bool| {
                (0..u.len()).find(|&p| if north { before.pos(p).y > 0.99 } else { before.pos(p).y < -0.99 }).unwrap()
            };
            assert!(u[pole(true)] > 1.01 && u[pole(false)] < 0.99, "{response}: {} / {}", u[pole(true)], u[pole(false)]);
            assert_eq!(mass(&before).len(), u.len());
        }
        // A Follow neither one number nor as wide as the attribute is refused.
        let mut wide = before.clone();
        wide.points_mut().create("pair", AttribValue::Float2([0.0, 0.0]));
        let err = apply(&mut wide, &node("c", "concentrate", &[("attribute", "mass"), ("follow", "pair")], vec![]), Flow::Concentrate);
        assert!(err.is_err());
    }

    /// Inside a simnet, Per Frame divides a frame among the substeps: four
    /// substeps of a quarter spread a frame as far as one substep of the
    /// whole — not exactly, a backward-Euler step being first order, but
    /// nowhere near the four times as far they would without it.
    #[test]
    fn a_diffuse_in_a_simnet_spreads_a_frame_whatever_the_substeps() {
        let graph = |substeps: &str, per_frame: &str| {
            let sphere = node("sphere1", "sphere", &[("radius", "1.0"), ("rows", "24"), ("columns", "32"), ("center_x", "0"), ("center_y", "0"), ("center_z", "0")], vec![]);
            let seed = node("seed1", "wrangle", &[("input", "sphere1"), ("class", "Points"), ("code", "@mass = @P.y;")], vec![]);
            let sim = node(
                "simnet1",
                "simnet",
                &[("input", "seed1"), ("substeps", substeps)],
                vec![
                    node("input1", "input", &[], vec![]),
                    node("diffuse1", "diffuse", &[("input", "input1"), ("attribute", "mass"), ("rate", "0.10"), ("per_frame", per_frame)], vec![]),
                    node("output1", "output", &[("input", "diffuse1")], vec![]),
                ],
            );
            node("root", "node", &[], vec![sphere, seed, sim])
        };
        let pole_after_a_frame = |root: &FsNode| {
            let sim_node = root.children.iter().find(|c| c.node_type == "simnet").unwrap();
            let mut cache = SimCache::default();
            let mut sim = EvalSim::new(2, 1, &mut cache);
            let mut err = None;
            let g = generate_single_node_geometry_with_errors(root, sim_node, &mut Vec::new(), &mut err, &mut sim).expect("solves");
            assert!(err.is_none(), "{err:?}");
            let u = mass(&g);
            let top = (0..u.len()).max_by(|&a, &b| g.pos(a).y.total_cmp(&g.pos(b).y)).unwrap();
            u[top] / g.pos(top).y as f64
        };
        let one = pole_after_a_frame(&graph("1", "true"));
        let four = pole_after_a_frame(&graph("4", "true"));
        let unscaled = pole_after_a_frame(&graph("4", "false"));
        assert!((one - 1.0 / 1.2).abs() < 0.03, "one substep: {one}");
        assert!((four - one).abs() < 0.02, "four substeps {four} against one {one}");
        assert!(unscaled < four - 0.1, "without Per Frame four substeps spread four frames: {unscaled}");
    }

    #[test]
    fn integers_stay_whole_and_errors_say_why() {
        let mut g = sphere(12, 16, |_| 0.0);
        g.points_mut().create("count", AttribValue::Int(0));
        g.points_mut().set_value("count", 0, AttribValue::Int(1000)).unwrap();
        let mut out = g.clone();
        apply(&mut out, &node("d", "diffuse", &[("attribute", "count"), ("rate", "0.05")], vec![]), Flow::Diffuse).unwrap();
        assert!(matches!(out.points().value("count", 0), Some(AttribValue::Int(v)) if v < 1000 && v > 0));

        let mut missing = g.clone();
        let e = apply(&mut missing, &node("d", "diffuse", &[("attribute", "heat")], vec![]), Flow::Diffuse).unwrap_err();
        assert!(e.contains("heat"), "{e}");
        let mut cloud = Detail::new();
        cloud.add_point(Vec3::ZERO);
        cloud.add_point(Vec3::X);
        cloud.points_mut().create("mass", AttribValue::Float(1.0));
        let e = apply(&mut cloud, &node("d", "diffuse", &[("attribute", "mass")], vec![]), Flow::Diffuse).unwrap_err();
        assert!(e.contains("surface"), "{e}");
    }
}
