//! The four shapes that were kernel subnets — Sphere, Box, Plane, Extrude —
//! as native generators. Phase 7 step 3 of `shapeshifter.md`.
//!
//! Each was a subnet of `input → opencl → output` whose kernel ran under
//! `if (id == 0)`: one work item doing loops, then a weld by position on the
//! way back that threw away every shared point the loop had known about.
//! Native, each builds welded points and real primitives directly — a quad
//! stays a quad — costs no JIT compile, and needs no OpenCL runtime at all.
//! The parameter surfaces are the templates' own, so a saved instance keeps
//! its values through `nativize_kernel_subnets` in `merge_template_defs`.
//!
//! The Sphere's three methods (UV, Icosphere, Cube) weld by a QUANTIZED
//! position key rather than by trusting bit-identical arithmetic across
//! faces: the kernel summed barycentric weights in one fixed expression so
//! shared corners landed on the same bits, then welded at 1e-4 anyway. A
//! quantized key is the same guarantee stated once.

use crate::app::FsNode;
use crate::detail::{AttribData, Detail, CD, DEFAULT_COLOR};
use crate::geometry::{node_param_f32, node_param_str, node_param_vec3, point_normals, sphere_detail};
use glam::Vec3;
use std::collections::HashMap;

/// The golden ratio: the icosahedron's corners sit on three orthogonal
/// golden rectangles.
const T: f32 = 1.618_034;

/// Points welded by quantized position, so a corner two faces share is one
/// point whichever face names it first.
struct Welder {
    map: HashMap<[i64; 3], u32>,
    d: Detail,
}

impl Welder {
    fn new() -> Self {
        Welder { map: HashMap::new(), d: Detail::new() }
    }

    fn point(&mut self, p: Vec3) -> u32 {
        let key = [
            (p.x as f64 * 1e5).round() as i64,
            (p.y as f64 * 1e5).round() as i64,
            (p.z as f64 * 1e5).round() as i64,
        ];
        if let Some(&i) = self.map.get(&key) {
            return i;
        }
        let i = self.d.add_point(p);
        self.map.insert(key, i);
        i
    }

    /// A primitive turned OUTWARD about the origin: the shapes here are
    /// convex about their centre, so a face whose normal points toward its
    /// own centroid is inside out, whatever table it came from.
    fn outward_prim(&mut self, pts: &[u32]) {
        let a = self.d.pos(pts[0] as usize);
        let b = self.d.pos(pts[1] as usize);
        let c = self.d.pos(pts[2] as usize);
        let n = (b - a).cross(c - a);
        let centroid: Vec3 = pts.iter().map(|&p| self.d.pos(p as usize)).sum();
        if n.dot(centroid) < 0.0 {
            let rev: Vec<u32> = pts.iter().rev().copied().collect();
            self.d.add_prim(&rev);
        } else {
            self.d.add_prim(pts);
        }
    }
}

/// Whether a sphere node carries its own Center parameters. A bare `sphere`
/// node built by hand (the tests' `ref_node`) has none and is placed by
/// index, the way Line and Points still are; every node that came through a
/// template or a load does.
pub fn sphere_has_center(target: &FsNode) -> bool {
    target.params.iter().any(|p| p.name == "Center X")
}

/// The Sphere node, by Method: UV (Rows x Columns), Icosphere (20 faces
/// each split into Frequency^2 triangles), Cube (a Resolution x Resolution
/// grid on each face, pushed out through the spherified-cube map). Welded
/// point counts are the closed forms: `2 + (rows - 1) * cols`,
/// `10 f^2 + 2` and `6 r^2 + 2`. Cube builds QUADS — the kernel fanned them.
pub fn sphere_node_detail(target: &FsNode, legacy_center: Option<Vec3>) -> Detail {
    let method = node_param_str(target, "Method", "UV").to_lowercase();
    let radius = node_param_f32(target, "Radius", 0.5).max(1e-4);
    let center = legacy_center.unwrap_or_else(|| {
        Vec3::new(
            node_param_f32(target, "Center X", 0.0),
            node_param_f32(target, "Center Y", 0.55),
            node_param_f32(target, "Center Z", 0.0),
        )
    });
    let colored = node_param_str(target, "Color", "true") != "false";
    let unit = match method.as_str() {
        "icosphere" => icosphere_unit(node_param_f32(target, "Frequency", 4.0).round().clamp(1.0, 16.0) as usize),
        "cube" => cube_sphere_unit(node_param_f32(target, "Resolution", 8.0).round().clamp(1.0, 64.0) as usize),
        _ => {
            let rows = node_param_f32(target, "Rows", 16.0).round().clamp(2.0, 128.0) as usize;
            let cols = node_param_f32(target, "Columns", 24.0).round().clamp(3.0, 128.0) as usize;
            let mut d = sphere_detail(center, radius, rows, cols);
            finish_sphere(&mut d, center, colored);
            return d;
        }
    };
    let mut d = unit;
    for p in 0..d.num_points() {
        let u = d.pos(p);
        d.set_pos(p, center + u * radius);
    }
    finish_sphere(&mut d, center, colored);
    d
}

/// `Norm`, `UV` and `Cd` from the surface normal, as the sphere has always
/// carried them. Colour is the kernel's: the SIGNED normal folded into
/// 0..1, world-anchored so a point keeps its colour as the sphere turns.
fn finish_sphere(d: &mut Detail, center: Vec3, colored: bool) {
    let n: Vec<Vec3> = (0..d.num_points()).map(|p| (d.pos(p) - center).normalize_or_zero()).collect();
    let norms = n.iter().map(|n| n.to_array()).collect();
    let uvs = n
        .iter()
        .map(|n| [0.5 + n.z.atan2(n.x) / std::f32::consts::TAU, 0.5 - n.y.clamp(-1.0, 1.0).asin() / std::f32::consts::PI])
        .collect();
    let cds = n
        .iter()
        .map(|n| if colored { [0.5 + n.x * 0.5, 0.5 + n.y * 0.5, 0.5 + n.z * 0.5] } else { DEFAULT_COLOR })
        .collect();
    let points = d.points_mut();
    let _ = points.insert("Norm", AttribData::Float3(norms));
    let _ = points.insert("UV", AttribData::Float2(uvs));
    let _ = points.insert(CD, AttribData::Float3(cds));
}

/// The unit icosphere: the icosahedron's 20 faces, each split into
/// `freq^2` triangles by integer barycentric weights, every corner pushed
/// onto the sphere. Row i of a face holds `freq - i` upright triangles and
/// `freq - i - 1` inverted ones.
fn icosphere_unit(freq: usize) -> Detail {
    let v: [Vec3; 12] = [
        Vec3::new(-1.0, T, 0.0),
        Vec3::new(1.0, T, 0.0),
        Vec3::new(-1.0, -T, 0.0),
        Vec3::new(1.0, -T, 0.0),
        Vec3::new(0.0, -1.0, T),
        Vec3::new(0.0, 1.0, T),
        Vec3::new(0.0, -1.0, -T),
        Vec3::new(0.0, 1.0, -T),
        Vec3::new(T, 0.0, -1.0),
        Vec3::new(T, 0.0, 1.0),
        Vec3::new(-T, 0.0, -1.0),
        Vec3::new(-T, 0.0, 1.0),
    ];
    let faces: [[usize; 3]; 20] = [
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
        [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
        [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    let f = freq.max(1);
    let mut w = Welder::new();
    for [a, b, c] in faces {
        let corner = |w: &mut Welder, wb: usize, wc: usize| {
            let wa = f - wb - wc;
            let p = (v[a] * wa as f32 + v[b] * wb as f32 + v[c] * wc as f32) / f as f32;
            w.point(p.normalize())
        };
        for i in 0..f {
            for j in 0..(f - i) {
                let p0 = corner(&mut w, i, j);
                let p1 = corner(&mut w, i + 1, j);
                let p2 = corner(&mut w, i, j + 1);
                w.outward_prim(&[p0, p1, p2]);
                if j + 1 < f - i {
                    let q0 = corner(&mut w, i + 1, j);
                    let q1 = corner(&mut w, i + 1, j + 1);
                    let q2 = corner(&mut w, i, j + 1);
                    w.outward_prim(&[q0, q1, q2]);
                }
            }
        }
    }
    w.d
}

/// The unit cube sphere: a `res x res` grid on each face of the cube
/// spanning -1..1, every point pushed onto the sphere by the spherified-cube
/// map — not a bare normalize, which crowds the corners and stretches the
/// face centres. One quad per cell.
fn cube_sphere_unit(res: usize) -> Detail {
    let r = res.max(1);
    let mut w = Welder::new();
    for face in 0..6 {
        let axis = face / 2;
        let sign = if face % 2 == 0 { 1.0 } else { -1.0 };
        let at = |w: &mut Welder, i: usize, j: usize| {
            let u = -1.0 + 2.0 * i as f32 / r as f32;
            let v = -1.0 + 2.0 * j as f32 / r as f32;
            let (x, y, z) = match axis {
                0 => (sign, u, v),
                1 => (v, sign, u),
                _ => (u, v, sign),
            };
            let (x2, y2, z2) = (x * x, y * y, z * z);
            w.point(Vec3::new(
                x * (1.0 - y2 * 0.5 - z2 * 0.5 + y2 * z2 / 3.0).sqrt(),
                y * (1.0 - z2 * 0.5 - x2 * 0.5 + z2 * x2 / 3.0).sqrt(),
                z * (1.0 - x2 * 0.5 - y2 * 0.5 + x2 * y2 / 3.0).sqrt(),
            ))
        };
        for i in 0..r {
            for j in 0..r {
                let q = [at(&mut w, i, j), at(&mut w, i + 1, j), at(&mut w, i + 1, j + 1), at(&mut w, i, j + 1)];
                w.outward_prim(&q);
            }
        }
    }
    w.d
}

/// An axis-aligned box: eight shared corners, six quads wound
/// counter-clockwise seen from outside, normals on the VERTICES (three faces
/// meet at a corner with three different normals), one colour.
pub fn cuboid_detail(center: Vec3, half: Vec3, color: [f32; 3]) -> Detail {
    let mut d = Detail::new();
    let (x, y, z) = (half.x, half.y, half.z);
    let corners = [
        center + Vec3::new(-x, -y, -z),
        center + Vec3::new(x, -y, -z),
        center + Vec3::new(x, y, -z),
        center + Vec3::new(-x, y, -z),
        center + Vec3::new(-x, -y, z),
        center + Vec3::new(x, -y, z),
        center + Vec3::new(x, y, z),
        center + Vec3::new(-x, y, z),
    ];
    for c in corners {
        d.add_point(c);
    }
    let faces: [([u32; 4], Vec3); 6] = [
        ([3, 2, 1, 0], Vec3::NEG_Z),
        ([6, 7, 4, 5], Vec3::Z),
        ([7, 3, 0, 4], Vec3::NEG_X),
        ([2, 6, 5, 1], Vec3::X),
        ([7, 6, 2, 3], Vec3::Y),
        ([1, 5, 4, 0], Vec3::NEG_Y),
    ];
    let mut norms = Vec::with_capacity(24);
    for (quad, normal) in &faces {
        d.add_prim(quad);
        norms.extend(std::iter::repeat(normal.to_array()).take(4));
    }
    let _ = d.verts_mut().insert("Norm", AttribData::Float3(norms));
    let _ = d.verts_mut().insert("UV", AttribData::Float2(vec![[0.0, 0.0]; 24]));
    let _ = d.points_mut().insert(CD, AttribData::Float3(vec![color; 8]));
    d
}

/// The Box node: a cube of Scale about Center — or, with Wireframe on, its
/// twelve edges as thin bars and its eight corners as small cubes, which is
/// what the kernel drew and what a reference frame wants.
pub fn box_node_detail(target: &FsNode) -> Detail {
    let scale = node_param_f32(target, "Scale", 1.0).max(1e-4);
    let wireframe = node_param_str(target, "Wireframe", "false") != "false";
    let center = node_param_vec3(target, "Center", Vec3::new(0.0, 0.55, 0.0));
    let color = [0.8, 0.2, 0.2];
    let h = 0.5 * scale;
    if !wireframe {
        return cuboid_detail(center, Vec3::splat(h), color);
    }
    let (t_line, t_corner) = (0.008 * scale, 0.012 * scale);
    let signs = [-1.0f32, 1.0];
    let mut out = Detail::new();
    for &sx in &signs {
        for &sy in &signs {
            for &sz in &signs {
                out.merge(&cuboid_detail(center + Vec3::new(sx * h, sy * h, sz * h), Vec3::splat(t_corner), color));
            }
        }
    }
    for &sa in &signs {
        for &sb in &signs {
            out.merge(&cuboid_detail(center + Vec3::new(0.0, sa * h, sb * h), Vec3::new(h, t_line, t_line), color));
            out.merge(&cuboid_detail(center + Vec3::new(sa * h, 0.0, sb * h), Vec3::new(t_line, h, t_line), color));
            out.merge(&cuboid_detail(center + Vec3::new(sa * h, sb * h, 0.0), Vec3::new(t_line, t_line, h), color));
        }
    }
    out
}

/// The Plane node: a Width x Length sheet of Columns x Rows quads in the XZ
/// plane about Center, wound counter-clockwise seen from +Y. Colour is the
/// kernel's gradient across the sheet, or the default grey with Color off.
/// Grid is the same sheet with a float3 Center and no gradient; two nodes
/// for history's sake, and this one is the older.
pub fn plane_node_detail(target: &FsNode) -> Detail {
    let width = node_param_f32(target, "Width", 1.0).max(1e-4);
    let length = node_param_f32(target, "Length", 1.0).max(1e-4);
    let cols = node_param_f32(target, "Columns", 16.0).round().clamp(1.0, 500.0) as usize;
    let rows = node_param_f32(target, "Rows", 16.0).round().clamp(1.0, 500.0) as usize;
    let center = Vec3::new(
        node_param_f32(target, "Center X", 0.0),
        node_param_f32(target, "Center Y", 0.0),
        node_param_f32(target, "Center Z", 0.0),
    );
    let colored = node_param_str(target, "Color", "true") != "false";

    let mut d = Detail::new();
    let mut cds = Vec::with_capacity((rows + 1) * (cols + 1));
    let mut uvs = Vec::with_capacity((rows + 1) * (cols + 1));
    for r in 0..=rows {
        for c in 0..=cols {
            let fx = c as f32 / cols as f32;
            let fz = r as f32 / rows as f32;
            d.add_point(center + Vec3::new((fx - 0.5) * width, 0.0, (fz - 0.5) * length));
            uvs.push([fx, fz]);
            cds.push(if colored { [0.35 + fx * 0.35, 0.45 + fz * 0.35, 0.85] } else { DEFAULT_COLOR });
        }
    }
    let at = |r: usize, c: usize| (r * (cols + 1) + c) as u32;
    for r in 0..rows {
        for c in 0..cols {
            d.add_prim(&[at(r, c), at(r + 1, c), at(r + 1, c + 1), at(r, c + 1)]);
        }
    }
    let n = d.num_points();
    let points = d.points_mut();
    let _ = points.insert("Norm", AttribData::Float3(vec![[0.0, 1.0, 0.0]; n]));
    let _ = points.insert("UV", AttribData::Float2(uvs));
    let _ = points.insert(CD, AttribData::Float3(cds));
    d
}

/// Extrude, as a WHOLE: every point moves along its point normal by
/// `distance`, the input's primitives become the top, and one quad wall
/// rises from each BOUNDARY edge — an edge one primitive uses. With Keep
/// Base the original primitives stay, wound the other way, so a sheet
/// becomes a closed slab and a closed surface a two-skinned shell.
///
/// The kernel extruded each triangle on its own and welded the pieces back
/// together afterwards, which put a wall along every interior edge too; a
/// surface extruded that way was a bed of prisms. A point shared by two
/// faces has one top here, which is what "extrude the surface" means, and
/// what makes the result closed when the input was a closed sheet.
///
/// Point attributes and groups carry to the top copies, primitive
/// attributes to the top and base copies; walls are fresh. Wall corners
/// share points with the top and base, so the kernel's 15% darker walls —
/// a per-corner colour a soup could hold — have no place to live and are
/// gone.
pub fn extrude_detail(d: &Detail, distance: f32, keep_base: bool) -> Detail {
    let n = d.num_points();
    if n == 0 || d.num_prims() == 0 {
        return d.clone();
    }
    let normals = point_normals(d);
    let mut out = Detail::new();
    let base: Vec<[f32; 3]> = d.positions().to_vec();
    let top: Vec<[f32; 3]> = (0..n).map(|p| (d.pos(p) + normals[p] * distance).to_array()).collect();
    out.add_points(&base);
    out.add_points(&top);
    // Base points keep their identities; the tops are new.
    let mut ids = d.ids().to_vec();
    let next = ids.iter().max().map_or(0, |m| m + 1);
    ids.extend((0..n as u64).map(|i| next + i));
    let _ = out.set_ids(ids, next + n as u64);

    let n32 = n as u32;
    let mut src: Vec<Option<usize>> = Vec::new();
    let mut directed: Vec<(u32, u32)> = Vec::new();
    let mut uses: HashMap<(u32, u32), usize> = HashMap::new();
    for pr in 0..d.num_prims() {
        let pts = d.prim_points(pr);
        if pts.len() < 3 {
            continue;
        }
        let lifted: Vec<u32> = pts.iter().map(|&p| p + n32).collect();
        out.add_prim(&lifted);
        src.push(Some(pr));
        for k in 0..pts.len() {
            let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
            let key = (a.min(b), a.max(b));
            let count = uses.entry(key).or_insert(0);
            if *count == 0 {
                directed.push((a, b));
            }
            *count += 1;
        }
    }
    // Walls on the boundary, wound so that for a counter-clockwise top and
    // a positive distance the outside faces out: `(b - a) x N` is the
    // outward direction of edge a→b on a counter-clockwise face.
    for (a, b) in directed {
        if uses[&(a.min(b), a.max(b))] == 1 {
            out.add_prim(&[a, b, b + n32, a + n32]);
            src.push(None);
        }
    }
    if keep_base {
        for pr in 0..d.num_prims() {
            let pts = d.prim_points(pr);
            if pts.len() < 3 {
                continue;
            }
            let rev: Vec<u32> = pts.iter().rev().copied().collect();
            out.add_prim(&rev);
            src.push(Some(pr));
        }
    }

    // Attributes: points doubled, primitives by source.
    for name in d.points().names() {
        let data = d.points().get(name).unwrap();
        let mut nd = AttribData::zeroed(data.ty(), 2 * n);
        for p in 0..n {
            if let Some(v) = data.get(p) {
                let _ = nd.set(p, v);
                let _ = nd.set(p + n, v);
            }
        }
        let kind = d.points().kind(name);
        let _ = out.points_mut().insert(name, nd);
        out.points_mut().set_kind(name, kind);
    }
    for g in d.points().group_names() {
        out.points_mut().create_group(g);
        for m in d.points().group_members(g) {
            out.points_mut().add_to_group(g, m as usize);
            out.points_mut().add_to_group(g, m as usize + n);
        }
    }
    let np = out.num_prims();
    for name in d.prims().names() {
        let data = d.prims().get(name).unwrap();
        let mut nd = AttribData::zeroed(data.ty(), np);
        for (i, s) in src.iter().enumerate() {
            if let Some(v) = s.and_then(|pr| data.get(pr)) {
                let _ = nd.set(i, v);
            }
        }
        let kind = d.prims().kind(name);
        let _ = out.prims_mut().insert(name, nd);
        out.prims_mut().set_kind(name, kind);
    }
    for g in d.prims().group_names() {
        out.prims_mut().create_group(g);
        for (i, s) in src.iter().enumerate() {
            if s.is_some_and(|pr| d.prims().in_group(g, pr)) {
                out.prims_mut().add_to_group(g, i);
            }
        }
    }
    for name in d.detail().names() {
        let _ = out.detail_mut().insert(name, d.detail().get(name).unwrap().clone());
    }
    out
}
