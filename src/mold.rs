//! Mold tooling: the shell a cast is poured into.
//!
//! The first GEM operator, ported from `gem_mold_shell`. Its four parameters
//! are the plugin's — Maximum Thickness, Minimum Thickness, Remesh Division
//! Size, Thickness Ramp — and the production notes from the original cast give
//! the numbers that worked: max 0.75, min 0.6, division 0.9, ramp linear.
//!
//! **Thickness varies with CURVATURE**, which is the whole point of the
//! operator and the reason a uniform `volume` shell will not do. The plugin
//! does it with an `im_ramp_scalar` node named `curvature_to_thickness`; this
//! does the same three steps:
//!
//! 1. remesh to the division size, so thickness is carried on evenly spaced
//!    points rather than on whatever triangulation arrived;
//! 2. measure curvature per point;
//! 3. map it through a ramp into the thickness range, and displace a copy of
//!    the surface inward by that much.
//!
//! The inner surface is a DISPLACEMENT, not a field offset. A `volume` shell
//! offsets a signed distance field by a constant, which cannot vary per point;
//! displacing each point along its own normal by its own thickness can. The
//! cost is the usual one for offset-by-displacement: where the thickness
//! exceeds the local radius of curvature the inner surface folds through
//! itself. That is exactly what the minimum/maximum range is for — it is a
//! range because the geometry constrains it, not because a single number was
//! hard to choose.

use crate::detail::Detail;
use glam::Vec3;

/// How the curvature measure is shaped before it lands in the thickness range.
///
/// The plugin uses a free-form float ramp. There is no ramp PARAMETER type in
/// this app yet — `cce-ui` has the widget, but nothing wires it as a node
/// parameter — so this ports the three shapes the falloff choices already use
/// elsewhere (`soft_transform`'s Falloff). The production notes say the cast
/// that worked used a linear ramp, so the default is the one that was actually
/// printed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ramp {
    Linear,
    Smooth,
    Constant,
}

impl Ramp {
    pub fn parse(s: &str) -> Ramp {
        match s.trim().to_ascii_lowercase().as_str() {
            "smooth" => Ramp::Smooth,
            "constant" => Ramp::Constant,
            _ => Ramp::Linear,
        }
    }

    /// Shape `t` in 0..1.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ramp::Linear => t,
            // Smoothstep: flat at both ends, so the thickest and thinnest
            // regions are even rather than knife-edged into their neighbours.
            Ramp::Smooth => t * t * (3.0 - 2.0 * t),
            // Everything at the maximum — the uniform shell, reachable without
            // leaving the node.
            Ramp::Constant => 1.0,
        }
    }
}

/// Per-point curvature, as a dimensionless signed measure in roughly -1..1.
///
/// For each point: the mean of `dot(normalize(neighbour - p), n)`. A neighbour
/// lying exactly in the tangent plane contributes zero; one below it (the
/// surface bulging out, CONVEX) contributes negative; one above it (the
/// surface cupping in, CONCAVE) contributes positive.
///
/// Dimensionless on purpose. Every term is a dot product of two unit vectors,
/// so the measure does not change when the model is scaled or when the remesh
/// division size changes — which matters here, because thickness is chosen
/// from it and a thickness that moved when you re-tessellated would be
/// unusable. A true mean curvature in 1/length would do the opposite.
///
/// Points with no neighbours (an isolated point, a stray primitive) read zero:
/// flat, which puts them in the middle of the ramp rather than at an extreme.
pub fn curvature(d: &Detail) -> Vec<f32> {
    let normals = crate::geometry::point_normals(d);
    (0..d.num_points())
        .map(|p| {
            let here = d.pos(p);
            let n = normals.get(p).copied().unwrap_or(Vec3::Y);
            let neighbours = d.point_neighbours(p);
            if neighbours.is_empty() {
                return 0.0;
            }
            let sum: f32 = neighbours
                .iter()
                .map(|&q| {
                    let to = d.pos(q as usize) - here;
                    let len = to.length();
                    // A coincident neighbour has no direction to contribute.
                    if len < 1e-6 { 0.0 } else { (to / len).dot(n) }
                })
                .sum();
            sum / neighbours.len() as f32
        })
        .collect()
}

/// Thickness per point, from curvature through the ramp.
///
/// The measure is mapped `-1..1 -> 0..1` by a fixed affine step rather than by
/// normalizing over the range present in this particular model. Normalizing
/// would make the thickness of one part depend on how curved the REST of it
/// is, so adding a sharp corner somewhere would thin the whole shell.
///
/// Concave regions get the maximum. A mould is weakest where it cups inward —
/// that is where it has least material behind it and where it is levered on
/// when the cast is pulled — so that is where the thickness goes.
pub fn thickness_from_curvature(curv: &[f32], min: f32, max: f32, ramp: Ramp) -> Vec<f32> {
    let (lo, hi) = (min.min(max), min.max(max));
    curv.iter()
        .map(|&c| {
            let t = ramp.apply((c * 0.5 + 0.5).clamp(0.0, 1.0));
            lo + (hi - lo) * t
        })
        .collect()
}

/// Build the mold shell: the surface, and an inner surface displaced inward by
/// a per-point thickness, wound to face the cavity.
///
/// Returns `None` when the input has no primitives — there is no surface to
/// thicken, and a shell of nothing is not an empty shell.
pub fn mold_shell(input: &Detail, min: f32, max: f32, division: f32, ramp: Ramp) -> Option<Detail> {
    if input.num_prims() == 0 {
        return None;
    }
    // Remesh first: thickness is carried per POINT, so the points have to be
    // spaced evenly or the shell's thickness resolution follows whatever
    // triangulation happened to arrive.
    let base = if division > 0.0 {
        crate::remesh::remesh(
            input,
            crate::remesh::Settings { target: division, ..Default::default() },
        )
    } else {
        input.clone()
    };
    if base.num_prims() == 0 {
        return None;
    }

    let normals = crate::geometry::point_normals(&base);
    let thickness = thickness_from_curvature(&curvature(&base), min, max, ramp);

    // The outer surface as it is, then the inner one: the same points pushed
    // along -normal, the same faces wound backwards so they look into the
    // cavity rather than out of it. Two shells facing opposite ways is what
    // makes the pair a solid rather than two surfaces in the same place.
    let mut out = base.clone();
    let inner_start = out.num_points();
    for p in 0..base.num_points() {
        let n = normals.get(p).copied().unwrap_or(Vec3::Y);
        out.add_point(base.pos(p) - n * thickness[p]);
    }
    for prim in 0..base.num_prims() {
        let mut pts: Vec<u32> =
            base.prim_points(prim).iter().map(|&i| i + inner_start as u32).collect();
        pts.reverse();
        out.add_prim(&pts);
    }
    Some(out)
}
