use crate::app::{FsNode, ParamDef};
use std::collections::HashMap;
use opencl3::platform::get_platforms;
use opencl3::device::{Device, CL_DEVICE_TYPE_GPU, CL_DEVICE_TYPE_CPU};
use opencl3::context::Context;
use opencl3::command_queue::CommandQueue;
use opencl3::program::Program;
use opencl3::kernel::{Kernel, ExecuteKernel};
use opencl3::memory::{Buffer as ClBuffer, CL_MEM_READ_WRITE};
use opencl3::types::{cl_float, cl_int, CL_TRUE};
use glam::Vec3;
use crate::detail::{AttribData, AttribValue, Detail, CD};

struct SimpleRng {
    state: u32,
}

impl SimpleRng {
    fn new(seed: u32) -> Self {
        Self { state: if seed == 0 { 1 } else { seed } }
    }
    
    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }
    
    fn next_f32(&mut self) -> f32 {
        (self.next_u32() as f32) / (u32::MAX as f32)
    }
}

fn ray_triangle_intersect(
    origin: Vec3,
    dir: Vec3,
    v0: Vec3,
    v1: Vec3,
    v2: Vec3,
) -> Option<f32> {
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
    if u < 0.0 || u > 1.0 {
        return None;
    }
    let q = s.cross(edge1);
    let v = f * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * edge2.dot(q);
    if t > 1e-5 {
        Some(t)
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum GAttribute {
    Float(f32),
    Float2([f32; 2]),
    Float3([f32; 3]),
    Float4([f32; 4]),
}

#[derive(Clone, Debug)]
pub struct GVertex {
    pub pos: [f32; 3],
    pub col: [f32; 3],
    pub attributes: HashMap<String, GAttribute>,
}

#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub vertices: Vec<GVertex>,
}

impl Geometry {
    pub fn new() -> Self {
        Geometry { vertices: Vec::new() }
    }

    pub fn merge(&mut self, other: Geometry) {
        self.vertices.extend(other.vertices);
    }

    pub fn to_vertex3d_vec(&self) -> Vec<Vertex3D> {
        self.vertices.iter().map(|v| Vertex3D {
            position: v.pos,
            color: v.col,
        }).collect()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex3D {
    pub position: [f32; 3],
    pub color: [f32; 3],
}

/// Colored triangles → the path tracer's scene schema, one Lambertian
/// material per distinct (8-bit-quantized) vertex color. Shared by the
/// viewport's RT mode and the `--thumbnail` renderer.
pub fn rt_scene_from_verts(
    verts: &[Vertex3D],
) -> (Vec<cce_ui::vk::RtTriangle>, Vec<cce_ui::vk::RtMaterial>) {
    let mut tris: Vec<cce_ui::vk::RtTriangle> = Vec::new();
    let mut mats: Vec<cce_ui::vk::RtMaterial> = Vec::new();
    let mut by_color: std::collections::HashMap<[u8; 3], u32> = std::collections::HashMap::new();
    for tri in verts.chunks_exact(3) {
        let key = [
            (tri[0].color[0].clamp(0.0, 1.0) * 255.0) as u8,
            (tri[0].color[1].clamp(0.0, 1.0) * 255.0) as u8,
            (tri[0].color[2].clamp(0.0, 1.0) * 255.0) as u8,
        ];
        let material = *by_color.entry(key).or_insert_with(|| {
            mats.push(cce_ui::vk::RtMaterial { albedo: tri[0].color, emission: [0.0; 3] });
            (mats.len() - 1) as u32
        });
        tris.push(cce_ui::vk::RtTriangle {
            p0: tri[0].position,
            p1: tri[1].position,
            p2: tri[2].position,
            material,
        });
    }
    (tris, mats)
}

pub fn cube_vertices() -> Vec<Vertex3D> {
    let s = 0.5;
    let data: &[([f32; 3], [f32; 3])] = &[
        ([-s, -s, s], [0.8, 0.2, 0.2]), ([s, -s, s], [0.8, 0.2, 0.2]), ([s, s, s], [0.8, 0.2, 0.2]),
        ([-s, -s, s], [0.8, 0.2, 0.2]), ([s, s, s], [0.8, 0.2, 0.2]), ([-s, s, s], [0.8, 0.2, 0.2]),
        ([s, -s, -s], [0.2, 0.8, 0.2]), ([-s, -s, -s], [0.2, 0.8, 0.2]), ([-s, s, -s], [0.2, 0.8, 0.2]),
        ([s, -s, -s], [0.2, 0.8, 0.2]), ([-s, s, -s], [0.2, 0.8, 0.2]), ([s, s, -s], [0.2, 0.8, 0.2]),
        ([-s, s, s], [0.2, 0.2, 0.8]), ([s, s, s], [0.2, 0.2, 0.8]), ([s, s, -s], [0.2, 0.2, 0.8]),
        ([-s, s, s], [0.2, 0.2, 0.8]), ([s, s, -s], [0.2, 0.2, 0.8]), ([-s, s, -s], [0.2, 0.2, 0.8]),
        ([-s, -s, -s], [0.8, 0.8, 0.2]), ([s, -s, -s], [0.8, 0.8, 0.2]), ([s, -s, s], [0.8, 0.8, 0.2]),
        ([-s, -s, -s], [0.8, 0.8, 0.2]), ([s, -s, s], [0.8, 0.8, 0.2]), ([-s, -s, s], [0.8, 0.8, 0.2]),
        ([s, -s, s], [0.8, 0.2, 0.8]), ([s, -s, -s], [0.8, 0.2, 0.8]), ([s, s, -s], [0.8, 0.2, 0.8]),
        ([s, -s, s], [0.8, 0.2, 0.8]), ([s, s, -s], [0.8, 0.2, 0.8]), ([s, s, s], [0.8, 0.2, 0.8]),
        ([-s, -s, -s], [0.2, 0.8, 0.8]), ([-s, -s, s], [0.2, 0.8, 0.8]), ([-s, s, s], [0.2, 0.8, 0.8]),
        ([-s, -s, -s], [0.2, 0.8, 0.8]), ([-s, s, s], [0.2, 0.8, 0.8]), ([-s, s, -s], [0.2, 0.8, 0.8]),
    ];
    data.iter().map(|&(p, c)| Vertex3D { position: p, color: c }).collect()
}

/// A [`Detail`]'s triangles as renderer vertices — the one place the 3D scene
/// crosses out of the geometry model.
pub fn detail_vertices(d: &Detail) -> Vec<Vertex3D> {
    d.triangulate(|position, color| Vertex3D { position, color })
}

/// Fan-triangulate a [`Detail`] back into the triangle soup the evaluation
/// pipeline still speaks, carrying attributes onto every corner.
///
/// **This is the migration bridge, and it is meant to die.** Generators build
/// real geometry now; the resolvers, the spreadsheet and the kernel launcher
/// have not been converted yet, so each generator's public entry point still
/// hands them a soup. When the pipeline's currency becomes `Detail`, this
/// function and the adapters calling it go with it.
///
/// Point and vertex attributes both land on the corner, vertex winning a name
/// clash — a vertex attribute is by definition the more specific answer for
/// that corner. `Cd` is dropped from the attribute map because the soup keeps
/// color in its own field. Integers widen to floats, the soup's `GAttribute`
/// having no integer case; nothing round-trips back through here, so the
/// narrowing is one-way and harmless.
pub fn detail_to_soup(d: &Detail) -> Geometry {
    let point_attrs: Vec<&str> = d.points().names().into_iter().filter(|n| *n != CD).collect();
    let vert_attrs = d.verts().names();

    let mut vertices = Vec::new();
    for prim in 0..d.num_prims() {
        let verts = d.prim_verts(prim);
        let pts = d.prim_points(prim);
        if pts.len() < 3 {
            continue;
        }
        for i in 1..pts.len() - 1 {
            for corner in [0, i, i + 1] {
                let p = pts[corner] as usize;
                let v = verts.start + corner;
                let mut attributes = HashMap::new();
                for name in &point_attrs {
                    if let Some(val) = d.points().value(name, p) {
                        attributes.insert(name.to_string(), soup_attr(val));
                    }
                }
                for name in &vert_attrs {
                    if let Some(val) = d.verts().value(name, v) {
                        attributes.insert(name.to_string(), soup_attr(val));
                    }
                }
                vertices.push(GVertex { pos: d.positions()[p], col: d.color(p), attributes });
            }
        }
    }
    Geometry { vertices }
}

/// Weld a soup back into a [`Detail`], carrying its per-corner attributes onto
/// the points they welded into (first corner wins).
///
/// The inverse of [`detail_to_soup`], and the other half of the migration
/// bridge. Attribute transfer is not incidental: the kernel launcher stamps a
/// default `Norm` and `UV` on every vertex it generates, and a weld that only
/// took positions and colors would quietly drop them — which is exactly what
/// the parameter pane's attribute picker reads.
pub fn soup_to_detail(soup: &Geometry) -> Detail {
    let positions: Vec<[f32; 3]> = soup.vertices.iter().map(|v| v.pos).collect();
    let colors: Vec<[f32; 3]> = soup.vertices.iter().map(|v| v.col).collect();
    let (mut d, point_of) = Detail::from_triangle_soup_with_map(&positions, &colors);

    let mut names: Vec<&str> = Vec::new();
    for v in &soup.vertices {
        for k in v.attributes.keys() {
            if !names.contains(&k.as_str()) {
                names.push(k);
            }
        }
    }
    names.sort_unstable();

    for name in names {
        let mut written = vec![false; d.num_points()];
        let mut data: Option<AttribData> = None;
        for (corner, v) in soup.vertices.iter().enumerate() {
            let (Some(&p), Some(val)) = (point_of.get(corner), v.attributes.get(name)) else {
                continue;
            };
            let p = p as usize;
            if std::mem::replace(&mut written[p], true) {
                continue;
            }
            let value = detail_attr(val);
            let arr = data.get_or_insert_with(|| AttribData::zeroed(value.ty(), d.num_points()));
            let _ = arr.set(p, value);
        }
        if let Some(arr) = data {
            let _ = d.points_mut().insert(name, arr);
        }
    }
    d
}

fn detail_attr(v: &GAttribute) -> AttribValue {
    match *v {
        GAttribute::Float(x) => AttribValue::Float(x),
        GAttribute::Float2(x) => AttribValue::Float2(x),
        GAttribute::Float3(x) => AttribValue::Float3(x),
        GAttribute::Float4(x) => AttribValue::Float4(x),
    }
}

fn soup_attr(v: AttribValue) -> GAttribute {
    match v {
        AttribValue::Float(x) => GAttribute::Float(x),
        AttribValue::Float2(x) => GAttribute::Float2(x),
        AttribValue::Float3(x) => GAttribute::Float3(x),
        AttribValue::Float4(x) => GAttribute::Float4(x),
        AttribValue::Int(x) => GAttribute::Float(x as f32),
    }
}

/// A UV sphere as shared points and quads.
///
/// The poles are ONE point each, not a ring of coincident copies, and the
/// seam at phi = 0 closes onto itself — so every interior point has valence 4
/// and the surface is genuinely connected. The soup this replaced could
/// express neither: it emitted `lat_steps * lon_steps * 6` loose corners, of
/// which the two pole bands were zero-area triangles.
///
/// `Norm` and `UV` are POINT attributes, computed from the surface normal
/// exactly as before. Both are pure functions of the normal, so a welded point
/// has one answer — including at the seam, where the old per-corner UVs
/// already agreed because they were derived from the normal rather than from
/// phi.
pub fn sphere_detail(center: Vec3, radius: f32, lat_steps: usize, lon_steps: usize) -> Detail {
    let lat_steps = lat_steps.max(2);
    let lon_steps = lon_steps.max(3);
    let mut d = Detail::new();

    let north = d.add_point(sphere_point(center, radius, 0.0, 0.0));
    let mut rings: Vec<Vec<u32>> = Vec::with_capacity(lat_steps - 1);
    for lat in 1..lat_steps {
        let theta = std::f32::consts::PI * lat as f32 / lat_steps as f32;
        let ring = (0..lon_steps)
            .map(|lon| {
                let phi = std::f32::consts::TAU * lon as f32 / lon_steps as f32;
                d.add_point(sphere_point(center, radius, theta, phi))
            })
            .collect();
        rings.push(ring);
    }
    let south = d.add_point(sphere_point(center, radius, std::f32::consts::PI, 0.0));

    // Winding follows the soup's exactly, so the fan in `Detail::triangulate`
    // reproduces the old triangles corner for corner — minus the degenerate
    // pole pair, which is why a sphere is now 2*lon_steps triangles lighter.
    let wrap = |lon: usize| (lon + 1) % lon_steps;
    for lon in 0..lon_steps {
        d.add_prim(&[north, rings[0][lon], rings[0][wrap(lon)]]);
    }
    for lat in 1..lat_steps - 1 {
        let (a, b) = (&rings[lat - 1], &rings[lat]);
        for lon in 0..lon_steps {
            d.add_prim(&[a[lon], b[lon], b[wrap(lon)], a[wrap(lon)]]);
        }
    }
    let last = &rings[lat_steps - 2];
    for lon in 0..lon_steps {
        d.add_prim(&[last[lon], south, last[wrap(lon)]]);
    }

    let n: Vec<Vec3> = (0..d.num_points())
        .map(|p| (d.pos(p) - center).normalize_or_zero())
        .collect();
    let norms = n.iter().map(|n| n.to_array()).collect();
    let uvs = n
        .iter()
        .map(|n| {
            [
                0.5 + n.z.atan2(n.x) / std::f32::consts::TAU,
                0.5 - n.y.asin() / std::f32::consts::PI,
            ]
        })
        .collect();
    let cds = n
        .iter()
        .map(|n| [0.35 + n.x.abs() * 0.35, 0.45 + n.y.abs() * 0.35, 0.85])
        .collect();
    let points = d.points_mut();
    let _ = points.insert("Norm", AttribData::Float3(norms));
    let _ = points.insert("UV", AttribData::Float2(uvs));
    let _ = points.insert(CD, AttribData::Float3(cds));
    d
}

pub fn sphere_vertices_res(center: Vec3, radius: f32, lat_steps: usize, lon_steps: usize) -> Geometry {
    detail_to_soup(&sphere_detail(center, radius, lat_steps, lon_steps))
}

pub fn sphere_vertices(center: Vec3, radius: f32) -> Geometry {
    sphere_vertices_res(center, radius, 16, 24)
}

fn sphere_point(center: Vec3, radius: f32, theta: f32, phi: f32) -> Vec3 {
    center + Vec3::new(
        radius * theta.sin() * phi.cos(),
        radius * theta.cos(),
        radius * theta.sin() * phi.sin(),
    )
}

/// A line as a box: eight shared corner points and six quads.
///
/// Where the sphere's normals live on points because the surface is smooth,
/// a box's live on VERTICES — three faces meet at every corner with three
/// different normals, and a point attribute could only hold one of them. This
/// is what the vertex class is for, and the soup had no way to say it: it
/// stored 36 corners so that it could store 36 normals.
pub fn box_detail(start: Vec3, end: Vec3, thickness: f32) -> Detail {
    let mut d = Detail::new();
    let dir = (end - start).normalize_or_zero();
    if dir.length_squared() < 0.0001 {
        return d;
    }

    // Find two orthogonal vectors to dir
    let up = if dir.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let u = dir.cross(up).normalize();
    let v = dir.cross(u).normalize();
    
    let t = thickness * 0.5;
    
    // 8 corners of the box
    let c0 = start - t * u - t * v;
    let c1 = start + t * u - t * v;
    let c2 = start + t * u + t * v;
    let c3 = start - t * u + t * v;
    
    let c4 = end - t * u - t * v;
    let c5 = end + t * u - t * v;
    let c6 = end + t * u + t * v;
    let c7 = end - t * u + t * v;
    
    let corners = [c0, c1, c2, c3, c4, c5, c6, c7];
    for c in corners {
        d.add_point(c);
    }

    // Face winding is the soup's: each `add_quad(p0, p1, p2, p3)` emitted
    // (p0, p1, p2) then (p0, p2, p3), which is exactly a fan over the quad.
    let faces: [([u32; 4], Vec3); 6] = [
        ([0, 1, 2, 3], -dir), // start cap
        ([5, 4, 7, 6], dir),  // end cap
        ([4, 0, 3, 7], -u),   // left
        ([1, 5, 6, 2], u),    // right
        ([3, 2, 6, 7], v),    // top
        ([0, 4, 5, 1], -v),   // bottom
    ];
    for (quad, _) in &faces {
        d.add_prim(quad);
    }

    let mut norms = Vec::with_capacity(d.num_verts());
    for (_, normal) in &faces {
        norms.extend(std::iter::repeat(normal.to_array()).take(4));
    }
    let (num_verts, num_points) = (d.num_verts(), d.num_points());
    let _ = d.verts_mut().insert("Norm", AttribData::Float3(norms));
    let _ = d
        .verts_mut()
        .insert("UV", AttribData::Float2(vec![[0.0, 0.0]; num_verts]));
    // A distinct color for lines.
    let _ = d
        .points_mut()
        .insert(CD, AttribData::Float3(vec![[0.85, 0.45, 0.35]; num_points]));
    d
}

pub fn line_vertices(start: Vec3, end: Vec3, thickness: f32) -> Geometry {
    detail_to_soup(&box_detail(start, end, thickness))
}

/// Parse a curve node's "Points" param: control points as `x y z` triples
/// separated by `;`. Commas are accepted alongside whitespace inside a
/// triple; chunks that don't yield exactly three numbers are skipped, so a
/// half-typed point in the params pane degrades to "not there yet" instead
/// of corrupting its neighbors.
pub fn parse_curve_points(s: &str) -> Vec<Vec3> {
    s.split(';')
        .filter_map(|chunk| {
            let n: Vec<f32> = chunk
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|t| !t.is_empty())
                .map(|t| t.parse::<f32>())
                .collect::<Result<_, _>>()
                .ok()?;
            if n.len() == 3 && n.iter().all(|v| v.is_finite()) {
                Some(Vec3::new(n[0], n[1], n[2]))
            } else {
                None
            }
        })
        .collect()
}

/// The inverse of [`parse_curve_points`] — what the curve viewer state
/// writes back into the "Points" param.
pub fn format_curve_points(pts: &[Vec3]) -> String {
    pts.iter()
        .map(|p| format!("{} {} {}", p.x, p.y, p.z))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Uniform Catmull-Rom through the control points, `segs` samples per span,
/// endpoints clamped (the first/last point doubles as its own neighbor). The
/// result includes the first control point and passes through every control
/// point at span boundaries. Fewer than two points sample as themselves.
pub fn sample_catmull_rom(pts: &[Vec3], segs: usize) -> Vec<Vec3> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let segs = segs.max(1);
    let mut out = Vec::with_capacity((pts.len() - 1) * segs + 1);
    out.push(pts[0]);
    for i in 0..pts.len() - 1 {
        let p0 = if i == 0 { pts[0] } else { pts[i - 1] };
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = if i + 2 < pts.len() { pts[i + 2] } else { pts[pts.len() - 1] };
        for s in 1..=segs {
            let t = s as f32 / segs as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            out.push(
                0.5 * ((2.0 * p1)
                    + (-p0 + p2) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3),
            );
        }
    }
    out
}

/// The native `curve` node: a Catmull-Rom strip through the "Points" param,
/// each sampled span an oriented box via [`line_vertices`]. Points are
/// absolute world coordinates — deliberately not offset by the grid index
/// the other primitives use, because the curve viewer state edits them in
/// world space.
/// The curve node's geometry: one box per sampled span.
///
/// The spans stay separate pieces — consecutive boxes meet but do not share
/// points, exactly as the soup had it. Welding them would fuse the curve into
/// one surface, which is a modeling decision the node has never made and is
/// not this migration's to make.
pub fn curve_detail(node: &FsNode) -> Detail {
    let pts = parse_curve_points(&node_param_str(node, "Points", ""));
    let segs = node_param_f32(node, "Segments", 8.0).max(1.0) as usize;
    let thickness = node_param_f32(node, "Thickness", 0.02).max(0.001);
    let samples = sample_catmull_rom(&pts, segs);
    let mut d = Detail::new();
    for w in samples.windows(2) {
        d.merge(&box_detail(w[0], w[1], thickness));
    }
    d
}

pub fn curve_geometry(node: &FsNode) -> Geometry {
    detail_to_soup(&curve_detail(node))
}

pub fn node_param_f32(node: &FsNode, name: &str, fallback: f32) -> f32 {
    node.params.iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .and_then(|p| p.default.parse::<f32>().ok())
        .unwrap_or(fallback)
}

pub fn node_param_str(node: &FsNode, name: &str, fallback: &str) -> String {
    node.params.iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .map(|p| p.default.clone())
        .unwrap_or_else(|| fallback.to_string())
}

pub fn node_param_vec3(node: &FsNode, name: &str, fallback: Vec3) -> Vec3 {
    node.params.iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .and_then(|p| {
            let parts: Vec<&str> = p.default.split(':').collect();
            if parts.len() == 3 {
                let x = parts[0].parse::<f32>().ok()?;
                let y = parts[1].parse::<f32>().ok()?;
                let z = parts[2].parse::<f32>().ok()?;
                Some(Vec3::new(x, y, z))
            } else {
                None
            }
        })
        .unwrap_or(fallback)
}

pub fn find_node_by_name<'a>(root: &'a FsNode, name: &str) -> Option<&'a FsNode> {
    fn visit<'a>(node: &'a FsNode, name: &str) -> Option<&'a FsNode> {
        if node.name == name {
            return Some(node);
        }
        for child in &node.children {
            if let Some(res) = visit(child, name) {
                return Some(res);
            }
        }
        None
    }
    for child in &root.children {
        if let Some(res) = visit(child, name) {
            return Some(res);
        }
    }
    None
}

pub fn find_parent_node<'a>(root: &'a FsNode, child_id: &str) -> Option<&'a FsNode> {
    fn visit<'a>(node: &'a FsNode, child_id: &str) -> Option<&'a FsNode> {
        for child in &node.children {
            if child.id == child_id {
                return Some(node);
            }
            if let Some(res) = visit(child, child_id) {
                return Some(res);
            }
        }
        None
    }
    visit(root, child_id)
}

/// One simnet's solved state, kept between evaluations so playing forward costs
/// one iteration per frame instead of re-solving the whole history every redraw.
struct SimSolve {
    /// Identity of everything the solve depends on — the simnet's own subtree and
    /// its seed geometry. When this changes the cached state is meaningless and
    /// the sim restarts from the seed.
    key: u64,
    /// The frame `state` is the solution FOR.
    frame: i32,
    state: Detail,
}

/// Per-simnet solved states, keyed by node id. Owned by the caller (the app keeps
/// one across frames; a one-shot render can pass a fresh one) rather than being a
/// global, so two evaluations of different graphs cannot poison each other.
#[derive(Default)]
pub struct SimCache {
    entries: std::collections::HashMap<String, SimSolve>,
}

impl SimCache {
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// The simulation half of an evaluation: which frame the graph is being evaluated
/// at, the solve cache, and the feedback stack that makes iteration possible.
///
/// The stack is what an `input` node inside a simnet reads instead of jumping to
/// the outer graph: during iteration N its parent simnet has pushed the state
/// from iteration N-1, and that — not the seed — is what the chain consumes.
pub struct EvalSim<'a> {
    pub frame: i32,
    /// The timeline's first frame — the frame at which every sim shows its seed,
    /// having taken no steps yet.
    pub start_frame: i32,
    pub cache: &'a mut SimCache,
    feedback: Vec<(String, Detail)>,
}

impl<'a> EvalSim<'a> {
    pub fn new(frame: i32, start_frame: i32, cache: &'a mut SimCache) -> Self {
        Self { frame, start_frame, cache, feedback: Vec::new() }
    }

    /// Steps the sim owes at the frame being evaluated. Scrubbing before the
    /// start frame is not negative time — it is simply the seed.
    fn steps_due(&self) -> i32 {
        (self.frame - self.start_frame).max(0)
    }

    /// The state an `input` node should yield, if its parent simnet is mid-solve.
    fn feedback_for(&self, simnet_id: &str) -> Option<&Detail> {
        self.feedback
            .iter()
            .rev()
            .find(|(id, _)| id == simnet_id)
            .map(|(_, g)| g)
    }
}

/// Hash of everything a simnet's solve depends on: its own subtree (so editing any
/// node in the chain restarts the sim) and the seed geometry (so an upstream change
/// does too).
fn sim_solve_key(simnet: &FsNode, seed: &Detail) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Ok(json) = serde_json::to_string(simnet) {
        json.hash(&mut h);
    }
    seed.num_points().hash(&mut h);
    for p in seed.positions() {
        for c in p {
            c.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// Evaluate with neither error reporting nor a persistent sim cache. Any simnet
/// reached this way solves at frame 0 — that is, shows its seed — because there
/// is no timeline in scope to say otherwise.
pub fn generate_single_node_geometry(root: &FsNode, target: &FsNode, visited: &mut Vec<String>) -> Option<Detail> {
    let mut err = None;
    let mut cache = SimCache::default();
    let mut sim = EvalSim::new(0, 0, &mut cache);
    generate_single_node_geometry_with_errors(root, target, visited, &mut err, &mut sim)
}

pub fn generate_single_node_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    // Cycle guard by ID, not name: subnet instances share child names
    // ("output1", "opencl1"), so a name guard falsely blocks a subnet that
    // consumes another subnet's geometry (Extrude eating a Sphere never
    // reaches the sphere's own output1). The wire-walk guard below (line
    // ~490) already keys on id.
    if visited.contains(&target.id) {
        return None;
    }
    visited.push(target.id.clone());

    let res = if target.node_type.eq_ignore_ascii_case("sphere") {
        let idx = find_sphere_index(root, target)?;
        let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
        Some(sphere_detail(center, node_param_f32(target, "Radius", 0.5).max(0.05), 16, 24))
    } else if target.node_type.eq_ignore_ascii_case("line") {
        let idx = find_sphere_index(root, target)?;
        let start = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
        let length = node_param_f32(target, "Length", 1.0);
        let thickness = node_param_f32(target, "Thickness", 0.02);
        let end = start + Vec3::new(0.0, length, 0.0);
        Some(box_detail(start, end, thickness))
    } else if target.node_type.eq_ignore_ascii_case("curve") {
        Some(curve_detail(target))
    } else if target.node_type.eq_ignore_ascii_case("points") {
        let idx = find_sphere_index(root, target)?;
        let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
        Some(points_detail(target, center))
    } else if target.node_type.eq_ignore_ascii_case("transform") {
        resolve_transform_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("scatter") {
        resolve_scatter_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("group") {
        resolve_group_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("collision") {
        resolve_collision_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("relax") {
        resolve_relax_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("attribute") {
        resolve_attribute_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("opencl") {
        resolve_opencl_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("simnet") {
        resolve_simnet_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("node") {
        if let Some(output_node) = target.children.iter().find(|c| c.node_type.eq_ignore_ascii_case("output")) {
            generate_single_node_geometry_with_errors(root, output_node, visited, ocl_error, sim)
        } else {
            None
        }
    } else if target.node_type.eq_ignore_ascii_case("output") {
        let input_name = node_param_str(target, "Input", "");
        if input_name.is_empty() {
            None
        } else {
            let parent_node = find_parent_node(root, &target.id);
            let input_node = if let Some(parent) = parent_node {
                parent.children.iter().find(|c| c.name == input_name || c.id == input_name)
            } else {
                None
            };
            let input_node = input_node.or_else(|| find_node_by_name(root, &input_name));
            if let Some(node) = input_node {
                generate_single_node_geometry_with_errors(root, node, visited, ocl_error, sim)
            } else {
                None
            }
        }
    } else if target.node_type.eq_ignore_ascii_case("input") {
        if let Some(parent) = find_parent_node(root, &target.id) {
            // Inside a simnet that is mid-solve, the input IS the previous
            // iteration's state — that feedback, not the outer graph, is what
            // makes the chain iterate rather than recompute the same thing.
            if let Some(prev) = sim.feedback_for(&parent.id) {
                let fed = prev.clone();
                visited.pop();
                return Some(fed);
            }
            let input_name = node_param_str(parent, "Input", "");
            if !input_name.is_empty() {
                if let Some(input_node) = find_node_by_name(root, &input_name) {
                    generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    visited.pop();
    res
}

pub fn resolve_transform_geometry(root: &FsNode, target: &FsNode, visited: &mut Vec<String>) -> Option<Detail> {
    let mut err = None;
    let mut cache = SimCache::default();
    let mut sim = EvalSim::new(0, 0, &mut cache);
    resolve_transform_geometry_with_errors(root, target, visited, &mut err, &mut sim)
}

pub fn resolve_transform_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_name = node_param_str(target, "Input", "");
    if input_name.is_empty() {
        return None;
    }
    let input_node = find_node_by_name(root, &input_name)?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    let translation = node_param_vec3(target, "Translation", Vec3::ZERO).to_array();
    // Moving points changes no topology, so the cache rides along.
    for p in geom.positions_mut() {
        for k in 0..3 {
            p[k] += translation[k];
        }
    }
    Some(geom)
}

pub fn resolve_scatter_geometry(root: &FsNode, target: &FsNode, visited: &mut Vec<String>) -> Option<Detail> {
    let mut err = None;
    let mut cache = SimCache::default();
    let mut sim = EvalSim::new(0, 0, &mut cache);
    resolve_scatter_geometry_with_errors(root, target, visited, &mut err, &mut sim)
}

/// Deterministic 64-bit PRNG step (splitmix64). Node evaluation must be a
/// pure function of the graph, so anything "random" derives from a Seed
/// param through this — never from a real entropy source.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4B9B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// The Group node: pass the input geometry through, putting the selected
/// elements in a named group.
///
/// Element Type picks the selection unit, and now they are real: Points are
/// points, Primitives are primitives, Edges are the edges topology already
/// knows about. The soup had to fake all three out of triangle-corner index
/// arithmetic — `tri * 3 + side` — and had to weld on the spot before it could
/// draw one random *point* rather than one random loose corner.
///
/// Mode picks the selector: Box selects by an axis-aligned box (Center/Size);
/// Random draws Count elements deterministically from Seed.
///
/// Membership always lands in a POINT group — for Primitives and Edges, the
/// points of the selected elements — because that is what every consumer
/// downstream reads (Relax's Pin Group, Attribute's Group, the viewport
/// markers), and it is what the per-vertex tagging it replaces amounted to.
/// Selecting primitives additionally writes a prim group of the same name;
/// groups are per-class, so the two names do not collide, and the operators
/// that want prims are coming.
///
/// Highlight tints members toward a warm accent so the group reads in the
/// viewport.
pub fn resolve_group_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_name = node_param_str(target, "Input", "");
    if input_name.is_empty() {
        return None;
    }
    let input_node = find_node_by_name(root, &input_name)?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let group_name = node_param_str(target, "Group Name", "group1").trim().to_string();
    let etype = node_param_str(target, "Element Type", "Points").to_lowercase();
    let center = node_param_vec3(target, "Center", Vec3::ZERO);
    let half = node_param_vec3(target, "Size", Vec3::ONE) * 0.5;
    let invert = node_param_str(target, "Invert", "false") == "true";
    let highlight = node_param_str(target, "Highlight", "true") == "true";

    let inside = |p: Vec3| -> bool {
        (p.x - center.x).abs() <= half.x
            && (p.y - center.y).abs() <= half.y
            && (p.z - center.z).abs() <= half.z
    };

    let (member, prim_member) =
        select_elements(&geom, &etype, target, |p| inside(p), invert);

    apply_group(
        &mut geom,
        &group_name,
        &member,
        &prim_member,
        highlight.then_some([1.0, 0.78, 0.20]),
    );
    Some(geom)
}

/// Which points (and, for a primitive selection, which primitives) a Group or
/// Collision node selects. Shared because the two nodes differ only in the
/// predicate: a box test versus a ray-cast or proximity test.
///
/// Returns a point mask and a primitive mask. `invert` flips the point mask,
/// matching what the per-vertex inversion did.
fn select_elements(
    geom: &Detail,
    etype: &str,
    target: &FsNode,
    hit: impl Fn(Vec3) -> bool,
    invert: bool,
) -> (Vec<bool>, Vec<bool>) {
    let mut member = vec![false; geom.num_points()];
    let mut prim_member = vec![false; geom.num_prims()];

    let prim_centroid = |prim: usize| -> Vec3 {
        let pts = geom.prim_points(prim);
        if pts.is_empty() {
            return Vec3::ZERO;
        }
        pts.iter().map(|&p| geom.pos(p as usize)).sum::<Vec3>() / pts.len() as f32
    };

    let mode = node_param_str(target, "Mode", "Box").to_lowercase();
    if mode == "random" {
        let count = node_param_f32(target, "Count", 1.0).max(0.0) as usize;
        // Seed offsets the stream, and the element type joins it so switching
        // type reshuffles instead of replaying the same index sequence.
        let mut rng = node_param_f32(target, "Seed", 0.0) as u64 ^ 0xCCE0;
        // Partial Fisher-Yates: draw `count` distinct indices out of `m`.
        let mut draw = |m: usize, count: usize| -> Vec<usize> {
            let mut idx: Vec<usize> = (0..m).collect();
            let take = count.min(m);
            for i in 0..take {
                let j = i + (splitmix64(&mut rng) as usize) % (m - i);
                idx.swap(i, j);
            }
            idx.truncate(take);
            idx
        };
        match etype {
            "primitives" => {
                for prim in draw(geom.num_prims(), count) {
                    prim_member[prim] = true;
                    for &p in geom.prim_points(prim) {
                        member[p as usize] = true;
                    }
                }
            }
            "edges" => {
                let edges = geom.edges().to_vec();
                for e in draw(edges.len(), count) {
                    member[edges[e][0] as usize] = true;
                    member[edges[e][1] as usize] = true;
                }
            }
            _ => {
                for p in draw(geom.num_points(), count) {
                    member[p] = true;
                }
            }
        }
    } else {
        match etype {
            "primitives" => {
                for prim in 0..geom.num_prims() {
                    if hit(prim_centroid(prim)) {
                        prim_member[prim] = true;
                        for &p in geom.prim_points(prim) {
                            member[p as usize] = true;
                        }
                    }
                }
            }
            "edges" => {
                for e in geom.edges() {
                    let (a, b) = (e[0] as usize, e[1] as usize);
                    if hit(geom.pos(a)) && hit(geom.pos(b)) {
                        member[a] = true;
                        member[b] = true;
                    }
                }
            }
            _ => {
                for p in 0..geom.num_points() {
                    if hit(geom.pos(p)) {
                        member[p] = true;
                    }
                }
            }
        }
    }

    if invert {
        for m in member.iter_mut() {
            *m = !*m;
        }
        for m in prim_member.iter_mut() {
            *m = !*m;
        }
    }
    (member, prim_member)
}

/// Write a selection into a named group, optionally tinting its members.
fn apply_group(
    geom: &mut Detail,
    name: &str,
    member: &[bool],
    prim_member: &[bool],
    highlight: Option<[f32; 3]>,
) {
    geom.points_mut().create_group(name);
    for (p, _) in member.iter().enumerate().filter(|(_, &m)| m) {
        geom.points_mut().add_to_group(name, p);
    }
    if prim_member.iter().any(|&m| m) {
        geom.prims_mut().create_group(name);
        for (prim, _) in prim_member.iter().enumerate().filter(|(_, &m)| m) {
            geom.prims_mut().add_to_group(name, prim);
        }
    }
    if let Some(acc) = highlight {
        for (p, _) in member.iter().enumerate().filter(|(_, &m)| m) {
            let base = geom.color(p);
            let mixed = [
                base[0] * 0.35 + acc[0] * 0.65,
                base[1] * 0.35 + acc[1] * 0.65,
                base[2] * 0.35 + acc[2] * 0.65,
            ];
            geom.set_color(p, mixed);
        }
    }
}

/// Squared distance from `p` to triangle `(a, b, c)` — closest point via the
/// Voronoi-region walk (Ericson, Real-Time Collision Detection §5.1.5).
fn point_triangle_distance_sq(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length_squared();
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length_squared();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (ap - ab * v).length_squared();
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length_squared();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (ap - ac * w).length_squared();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (bp - (c - b) * w).length_squared();
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (ap - ab * v - ac * w).length_squared()
}

/// The Collision node: marks the elements of `Input` that collide with the
/// `Collider` node's geometry (a node name, evaluated like a second input —
/// the Relax `Rest` pattern), as the group `group:<Group Name>` (the Group
/// node's convention, so downstream group pickers — Relax's Pin Group, the
/// Attribute node's Group — list it automatically). Two methods:
/// - "Inside": parity ray cast against the collider's triangles — the
///   element is enclosed by the collider's volume. Meaningful against
///   closed meshes; an open surface reads as inside from one of its sides.
/// - "Proximity": within `Distance` of the collider's surface (closest
///   point on any triangle) — touching counts, containment not required.
///
/// Points mark per WELDED point — every copy of a position marks together,
/// which is also one test per distinct position instead of per corner;
/// primitives test their centroid, matching the Group node's box test.
/// With no Collider, an unresolvable one, or one with no triangles, the
/// input passes through unchanged — half-configured nodes stay visible.
///
/// No visited guard here (the dispatch already pushed this node's id — the
/// Relax/Attribute trap); the Collider is a second chain off `root`.
pub fn resolve_collision_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_name = node_param_str(target, "Input", "");
    if input_name.is_empty() {
        return None;
    }
    let input_node = find_node_by_name(root, &input_name)?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let collider_name = node_param_str(target, "Collider", "");
    let collider_name = collider_name.trim();
    if collider_name.is_empty() {
        return Some(geom);
    }
    let Some(collider_node) = find_node_by_name(root, collider_name) else { return Some(geom) };
    let Some(collider) = generate_single_node_geometry_with_errors(root, collider_node, visited, ocl_error, sim) else {
        return Some(geom);
    };
    if collider.num_prims() == 0 || geom.is_empty() {
        return Some(geom);
    }

    // The collider is still tested as triangles: both the ray cast and the
    // closest-point walk are triangle routines, so a polygon is fanned here
    // rather than each routine growing an n-gon case.
    let tris: Vec<[Vec3; 3]> = collider
        .triangulate(|pos, _| Vec3::from(pos))
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();

    let method = node_param_str(target, "Method", "Inside").to_lowercase();
    let distance = node_param_f32(target, "Distance", 0.05).max(0.0);
    // Fixed irrational-ish direction, NOT axis-aligned: the template meshes
    // tessellate on the axes, and a ray along one skims edge-on through
    // whole fans of triangles, double-counting crossings.
    let ray_dir = Vec3::new(0.9174771, 0.3369154, 0.2095338).normalize();
    let hit = |pt: Vec3| -> bool {
        if method == "proximity" {
            let d2 = distance * distance;
            tris.iter().any(|t| point_triangle_distance_sq(pt, t[0], t[1], t[2]) <= d2)
        } else {
            let crossings = tris
                .iter()
                .filter(|t| ray_triangle_intersect(pt, ray_dir, t[0], t[1], t[2]).is_some())
                .count();
            crossings % 2 == 1
        }
    };

    // Collision has no Mode parameter, so `select_elements` takes its Box
    // branch and applies `hit` per element — which is the whole difference
    // between this node and Group.
    let etype = node_param_str(target, "Element Type", "Points").to_lowercase();
    let invert = node_param_str(target, "Invert", "false") == "true";
    let (member, prim_member) = select_elements(&geom, &etype, target, hit, invert);

    let group_name = node_param_str(target, "Group Name", "collisions").trim().to_string();
    let highlight = node_param_str(target, "Highlight", "true") == "true";
    apply_group(
        &mut geom,
        &group_name,
        &member,
        &prim_member,
        // Contact reads as red — distinct from the Group node's amber.
        highlight.then_some([1.0, 0.30, 0.24]),
    );
    Some(geom)
}

/// The Relax node: an edge-length constraint solver — the organic-tissue
/// response. Pass the input geometry through, then move every welded point
/// toward restoring the edge lengths of the `Rest` geometry (a node name,
/// evaluated like a second input; inside a simnet, `input1` — the previous
/// sim state). Vertices carrying `group:<Pin Group>` are pinned: they keep
/// their input position and push everyone else instead — chain a
/// displacement over that group first and this node spreads it through the
/// surface with a stiffness-shaped falloff. Iterations Gauss–Seidel passes
/// over the unique edges; Stiffness scales each correction. With no Rest,
/// an unresolvable Rest, or a Rest whose vertex count differs from the
/// input's, the geometry passes through unchanged — there is nothing
/// coherent to restore toward.
///
/// No visited guard here (the dispatch already pushed this node's id — the
/// same trap Attribute documents below). Rest is evaluated as a second
/// chain off `root`, which is legal mid-solve: the feedback stack, not the
/// call graph, is what makes an `input` node yield the previous state.
pub fn resolve_relax_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_name = node_param_str(target, "Input", "");
    if input_name.is_empty() {
        return None;
    }
    let input_node = find_node_by_name(root, &input_name)?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let rest_name = node_param_str(target, "Rest", "");
    let rest_name = rest_name.trim();
    if rest_name.is_empty() {
        return Some(geom);
    }
    let Some(rest_node) = find_node_by_name(root, rest_name) else { return Some(geom) };
    let Some(rest) = generate_single_node_geometry_with_errors(root, rest_node, visited, ocl_error, sim) else {
        return Some(geom);
    };
    if rest.num_points() != geom.num_points() || geom.is_empty() {
        return Some(geom);
    }

    let stiffness = node_param_f32(target, "Stiffness", 0.5).clamp(0.0, 1.0);
    let iterations = node_param_f32(target, "Iterations", 8.0).max(1.0) as usize;
    let pin = node_param_str(target, "Pin Group", "").trim().to_string();

    // The edges come from the REST shape's topology. This is where the soup
    // cost the most: it had to weld both shapes by position and rebuild the
    // unique edge list inside this node, every call, and weld on the rest
    // positions specifically so that a displacement already applied to the
    // input could not split a point apart. Points are points now, so the
    // correspondence is just the index, and `edges()` is cached on the rest
    // geometry for anyone else who asks.
    let mut pos: Vec<Vec3> = (0..geom.num_points()).map(|p| geom.pos(p)).collect();
    let pinned: Vec<bool> = if pin.is_empty() {
        vec![false; geom.num_points()]
    } else {
        (0..geom.num_points())
            .map(|p| geom.points().in_group(&pin, p))
            .collect()
    };

    let edges: Vec<(usize, usize, f32)> = rest
        .edges()
        .iter()
        .map(|e| {
            let (a, b) = (e[0] as usize, e[1] as usize);
            (a, b, (rest.pos(b) - rest.pos(a)).length())
        })
        .collect();

    for _ in 0..iterations {
        for &(a, b, rest_len) in &edges {
            let d = pos[b] - pos[a];
            let len = d.length();
            if len < 1e-6 {
                continue;
            }
            let corr = d * ((len - rest_len) / len * 0.5 * stiffness);
            match (pinned[a], pinned[b]) {
                (false, false) => {
                    pos[a] += corr;
                    pos[b] -= corr;
                }
                (true, false) => pos[b] -= corr * 2.0,
                (false, true) => pos[a] += corr * 2.0,
                (true, true) => {}
            }
        }
    }

    for (p, v) in pos.iter().enumerate() {
        geom.set_pos(p, *v);
    }
    Some(geom)
}

/// The Attribute node: pass the input geometry through, running one attribute
/// edit over its POINTS. Operation picks the edit —
///
/// - **Create** inserts `Attribute Name` as the chosen Type parsed from Value,
///   overwriting an existing attribute of that name.
/// - **Modify** combines Value into an attribute that already exists
///   (Combine = Set / Add / Multiply, componentwise). The built-ins `Pos` and
///   `Col` are reachable by name here (Float3), so the node can displace or
///   tint geometry; they cannot be created or deleted.
/// - **Delete** removes the attribute.
///
/// Value splits on `:`/`,`/space like every vector param; a single-component
/// Value broadcasts across wider types (`0.5` scales a Float3 uniformly). A
/// non-empty Group name restricts every operation to the points a Group node
/// put in it, composing the two nodes. Errors (bad Value, component mismatch,
/// editing a built-in) surface on the status line and pass the geometry
/// through unchanged.
///
/// Create and Delete are whole-attribute operations, so a Group narrows what
/// they WRITE, not what exists: creating into a group leaves non-members at
/// the type's zero rather than leaving them without the attribute, because a
/// column covers its whole class. That is the one behaviour the columnar
/// store changes here, and it is the reason a solver can read any attribute at
/// any point without checking whether it is there.
pub fn resolve_attribute_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    // No visited guard here: `generate_single_node_geometry_with_errors`
    // pushes the target's id before dispatching to this resolver, so a local
    // `visited.contains` check would see it and refuse every call (the trap
    // that broke this node's first draft).
    let input_name = node_param_str(target, "Input", "");
    if input_name.is_empty() {
        return None;
    }
    let input_node = find_node_by_name(root, &input_name)?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let name = node_param_str(target, "Attribute Name", "attr1").trim().to_string();
    if name.is_empty() {
        return Some(geom);
    }
    let op = node_param_str(target, "Operation", "Create").to_lowercase();
    let combine_mode = node_param_str(target, "Combine", "Set").to_lowercase();
    let is_pos = name.eq_ignore_ascii_case("Pos");
    let is_col = name.eq_ignore_ascii_case("Col");
    let builtin = is_pos || is_col;

    let mut fail = String::new();
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let affected: Vec<usize> = (0..geom.num_points())
        .filter(|&p| group.is_empty() || geom.points().in_group(&group, p))
        .collect();

    // Value, as raw components. Delete never reads it; Create/Modify reject
    // the edit outright when any component fails to parse.
    let value_str = node_param_str(target, "Value", "");
    let raw: Vec<&str> = value_str
        .split(|c| c == ':' || c == ',' || c == ' ')
        .filter(|p| !p.is_empty())
        .collect();
    let comps: Vec<f32> = raw.iter().filter_map(|p| p.parse::<f32>().ok()).collect();
    let value_ok = !comps.is_empty() && comps.len() == raw.len();
    // Value resized to an attribute's width: exact match passes through, a
    // single component broadcasts, anything else is a mismatch.
    let fit = |n: usize| -> Option<Vec<f32>> {
        if comps.len() == n {
            Some(comps.clone())
        } else if comps.len() == 1 {
            Some(vec![comps[0]; n])
        } else {
            None
        }
    };
    let combine = |dst: &mut [f32], src: &[f32]| {
        for (d, s) in dst.iter_mut().zip(src) {
            match combine_mode.as_str() {
                "add" => *d += s,
                "multiply" => *d *= s,
                _ => *d = *s,
            }
        }
    };

    match op.as_str() {
        "delete" => {
            if builtin {
                fail = format!("'{}' is built-in and cannot be deleted", name);
            } else {
                geom.points_mut().remove(&name);
            }
        }
        "modify" => {
            if !value_ok {
                fail = format!("Value '{}' does not parse as numbers", value_str);
            } else if builtin {
                match fit(3) {
                    Some(src) => {
                        for &p in &affected {
                            let mut v = if is_col { geom.color(p) } else { geom.pos(p).to_array() };
                            combine(&mut v, &src);
                            if is_col {
                                geom.set_color(p, v);
                            } else {
                                geom.set_pos(p, Vec3::from(v));
                            }
                        }
                    }
                    None => fail = format!("Value '{}' does not fit Float3 '{}'", value_str, name),
                }
            } else {
                match geom.points().get(&name).map(|a| a.ty()) {
                    None => {}
                    Some(ty) => match fit(ty.components()) {
                        None => fail = format!("Value '{}' does not fit '{}'", value_str, name),
                        Some(src) => {
                            for &p in &affected {
                                let Some(cur) = geom.points().value(&name, p) else { continue };
                                let mut buf = attrib_components(cur);
                                combine(&mut buf, &src);
                                let _ = geom.points_mut().set_value(&name, p, components_attrib(ty, &buf));
                            }
                        }
                    },
                }
            }
        }
        // Create (the default).
        _ => {
            if builtin {
                fail = format!("'{}' is built-in and cannot be created", name);
            } else if !value_ok {
                fail = format!("Value '{}' does not parse as numbers", value_str);
            } else {
                let ty = match node_param_str(target, "Type", "Float").to_lowercase().as_str() {
                    "float2" => crate::detail::AttribType::Float2,
                    "float3" => crate::detail::AttribType::Float3,
                    "float4" => crate::detail::AttribType::Float4,
                    _ => crate::detail::AttribType::Float,
                };
                match fit(ty.components()) {
                    Some(src) => {
                        let zero = components_attrib(ty, &vec![0.0; ty.components()]);
                        let value = components_attrib(ty, &src);
                        geom.points_mut().create(&name, zero);
                        for &p in &affected {
                            let _ = geom.points_mut().set_value(&name, p, value);
                        }
                    }
                    None => {
                        fail = format!("Value '{}' does not fit {}", value_str, ty.name());
                    }
                }
            }
        }
    }

    if !fail.is_empty() && ocl_error.is_none() {
        *ocl_error = Some(format!("Attribute '{}': {}", target.name, fail));
    }
    Some(geom)
}

/// An attribute value as loose components, for the arithmetic that does not
/// care how wide it is.
fn attrib_components(v: AttribValue) -> Vec<f32> {
    match v {
        AttribValue::Float(x) => vec![x],
        AttribValue::Float2(x) => x.to_vec(),
        AttribValue::Float3(x) => x.to_vec(),
        AttribValue::Float4(x) => x.to_vec(),
        AttribValue::Int(x) => vec![x as f32],
    }
}

/// Components back into a value of the given type, zero-padding a short slice.
fn components_attrib(ty: crate::detail::AttribType, c: &[f32]) -> AttribValue {
    let at = |i: usize| c.get(i).copied().unwrap_or(0.0);
    match ty {
        crate::detail::AttribType::Float => AttribValue::Float(at(0)),
        crate::detail::AttribType::Float2 => AttribValue::Float2([at(0), at(1)]),
        crate::detail::AttribType::Float3 => AttribValue::Float3([at(0), at(1), at(2)]),
        crate::detail::AttribType::Float4 => AttribValue::Float4([at(0), at(1), at(2), at(3)]),
        crate::detail::AttribType::Int => AttribValue::Int(at(0) as i32),
    }
}

/// Positions of the points in a group — the source data for the
/// selected-Group viewport markers.
///
/// The soup version had to hand back duplicates (it repeated every shared
/// corner) and relied on `points_vertices` deduping by quantized position.
/// A point group has each point once.
pub fn group_member_positions(geom: &Detail, group_name: &str) -> Vec<Vertex3D> {
    geom.points()
        .group_members(group_name)
        .into_iter()
        .map(|p| Vertex3D { position: geom.positions()[p as usize], color: [0.0; 3] })
        .collect()
}

struct PrecomputedTriangle {
    v0: Vec3,
    edge1: Vec3,
    edge2: Vec3,
    h: Vec3,
    f: f32,
}

pub fn resolve_scatter_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    // No visited guard here: `generate_single_node_geometry_with_errors`
    // pushes the target's id before dispatching to this resolver, so a local
    // `visited.contains` check refused every dispatched call — scatter
    // geometry evaluated to None for the spreadsheet and for any downstream
    // consumer, while the scene walk's direct call (fresh `visited`) kept the
    // node LOOKING healthy. Cycles stay guarded by the dispatch itself.
    let input_name = node_param_str(target, "Input", "");
    if input_name.is_empty() {
        return None;
    }
    let input_node = find_node_by_name(root, &input_name)?;
    let geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let num_points = node_param_f32(target, "Points", 100.0) as usize;
    let radius = node_param_f32(target, "Radius", 0.02);

    let ray_dir = Vec3::new(0.19, 0.98, 0.05).normalize();
    let mut triangles = Vec::new();
    let mut min_pos = Vec3::splat(f32::MAX);
    let mut max_pos = Vec3::splat(f32::MIN);

    for chunk in geom.triangulate(|pos, _| Vec3::from(pos)).chunks_exact(3) {
        let (v0, v1, v2) = (chunk[0], chunk[1], chunk[2]);

        min_pos = min_pos.min(v0).min(v1).min(v2);
        max_pos = max_pos.max(v0).max(v1).max(v2);

        let edge1 = v1 - v0;
        let edge2 = v2 - v0;
        let h = ray_dir.cross(edge2);
        let a = edge1.dot(h);
        if a.abs() >= 1e-6 {
            let f = 1.0 / a;
            triangles.push(PrecomputedTriangle {
                v0,
                edge1,
                edge2,
                h,
                f,
            });
        }
    }

    let res = if triangles.is_empty() {
        Detail::new()
    } else {
        let mut rng = SimpleRng::new(1337);
        let mut scattered_geom = Detail::new();
        let mut found_count = 0;
        let max_attempts = (num_points * 100).max(10_000);

        for _ in 0..max_attempts {
            if found_count >= num_points {
                break;
            }
            let rx = min_pos.x + rng.next_f32() * (max_pos.x - min_pos.x);
            let ry = min_pos.y + rng.next_f32() * (max_pos.y - min_pos.y);
            let rz = min_pos.z + rng.next_f32() * (max_pos.z - min_pos.z);
            let candidate = Vec3::new(rx, ry, rz);

            let mut intersection_count = 0;
            for tri in &triangles {
                let s = candidate - tri.v0;
                let u = tri.f * s.dot(tri.h);
                if u < 0.0 || u > 1.0 {
                    continue;
                }
                let q = s.cross(tri.edge1);
                let v = tri.f * ray_dir.dot(q);
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let t = tri.f * tri.edge2.dot(q);
                if t > 1e-5 {
                    intersection_count += 1;
                }
            }

            if intersection_count % 2 == 1 {
                scattered_geom.merge(&sphere_detail(candidate, radius, 6, 8));
                found_count += 1;
            }
        }
        scattered_geom
    };

    Some(res)
}

/// The point attributes a kernel names, in first-use order — one buffer each,
/// bound after `param_values`.
///
/// A kernel reaches an attribute the same way it reaches a parameter: by name,
/// through a call the preprocessor rewrites. `attrf("mass", i)` reads point
/// `i`'s float attribute and `setattrf("mass", i, v)` writes it. The name is
/// the declaration — an attribute the geometry does not carry is created,
/// zeroed, so a kernel can produce one.
pub fn parse_attr_refs(code: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for call in ["attrf(", "setattrf("] {
        let mut from = 0;
        while let Some(pos) = code[from..].find(call) {
            let at = from + pos;
            // `setattrf(` also contains `attrf(`; only count the outer one.
            let is_inner = call == "attrf(" && at >= 3 && &code[at - 3..at] == "set";
            from = at + call.len();
            if is_inner {
                continue;
            }
            let Some(name) = quoted_arg(&code[from..]) else { continue };
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// The first single- or double-quoted string in an argument list.
fn quoted_arg(args: &str) -> Option<String> {
    let q = args.find(|c| c == '"' || c == '\'')?;
    let quote = args.as_bytes()[q] as char;
    let rest = &args[q + 1..];
    let end = rest.find(quote)?;
    let name = &rest[..end];
    (!name.is_empty()).then(|| name.to_string())
}

/// Append arguments to `process`'s parameter list, in binding order.
fn rewrite_kernel_signature_with(code: &str, extra: &[String]) -> String {
    if extra.is_empty() {
        return code.to_string();
    }
    let bytes = code.as_bytes();
    if let Some(process_idx) = code.find("process") {
        let mut idx = process_idx + "process".len();
        while idx < bytes.len() && (bytes[idx] as char).is_whitespace() {
            idx += 1;
        }
        if idx < bytes.len() && bytes[idx] == b'(' {
            let start_args = idx + 1;
            let mut paren_count = 1;
            let mut end_args = start_args;
            while end_args < bytes.len() && paren_count > 0 {
                if bytes[end_args] == b'(' {
                    paren_count += 1;
                } else if bytes[end_args] == b')' {
                    paren_count -= 1;
                }
                end_args += 1;
            }
            if paren_count == 0 {
                let closing_paren_idx = end_args - 1;
                let before = &code[..closing_paren_idx];
                let after = &code[closing_paren_idx..];
                let args_str = code[start_args..closing_paren_idx].trim();
                let sep = if args_str.is_empty() { "" } else { ", " };
                return format!("{}{}{}{}", before, sep, extra.join(", "), after);
            }
        }
    }
    code.to_string()
}

pub fn preprocess_opencl_code(code: &str) -> String {
    let parsed_params = parse_dynamic_params(code);
    let attrs = parse_attr_refs(code);
    if parsed_params.is_empty() && attrs.is_empty() {
        return code.to_string();
    }

    let mut param_indices = std::collections::HashMap::new();
    let mut flat_idx = 0;
    for p in &parsed_params {
        param_indices.insert(p.name.clone(), flat_idx);
        if p.param_type == "float3" {
            flat_idx += 3;
        } else {
            flat_idx += 1;
        }
    }

    // Binding order, and therefore signature order: the fixed arguments, then
    // `param_values` if the kernel names any parameter, then one buffer per
    // attribute it names. Both launchers bind positionally against exactly
    // this list.
    let mut extra: Vec<String> = Vec::new();
    if !parsed_params.is_empty() {
        extra.push("__global const float* param_values".to_string());
    }
    for i in 0..attrs.len() {
        extra.push(format!("__global float* attr_{}", i));
    }
    let mut processed = rewrite_kernel_signature_with(code, &extra);

    // Attribute access rewrites before parameter ones: `attrf("mass", chi("k"))`
    // is legal, and the parameter pass would otherwise rewrite inside a call
    // this pass still needs to find by name.
    for (slot, name) in attrs.iter().enumerate() {
        processed = rewrite_attr_calls(&processed, name, slot);
    }

    let prefixes = [("chf", "slider"), ("chi", "spinbox"), ("chv", "float3"), ("chb", "toggle")];
    for &(prefix, _) in &prefixes {
        let pattern = format!("{}(", prefix);
        while let Some(pos) = processed.find(&pattern) {
            let start_idx = pos + pattern.len();
            let mut paren_count = 1;
            let mut end_pos = start_idx;
            let bytes = processed.as_bytes();
            while end_pos < bytes.len() && paren_count > 0 {
                if bytes[end_pos] == b'(' {
                    paren_count += 1;
                } else if bytes[end_pos] == b')' {
                    paren_count -= 1;
                }
                end_pos += 1;
            }
            if paren_count == 0 {
                let full_match = &processed[pos..end_pos];
                let args_str = &processed[start_idx..end_pos - 1];
                let mut replacement = None;
                if let Some(name) = quoted_arg(args_str) {
                    if let Some(&flat_idx) = param_indices.get(&name) {
                        match prefix {
                            "chf" => {
                                replacement = Some(format!("param_values[{}]", flat_idx));
                            }
                            "chi" | "chb" => {
                                replacement = Some(format!("((int)param_values[{}])", flat_idx));
                            }
                            "chv" => {
                                replacement = Some(format!(
                                    "(float3)(param_values[{}], param_values[{}], param_values[{}])",
                                    flat_idx, flat_idx + 1, flat_idx + 2
                                ));
                            }
                            _ => {}
                        }
                    }
                }
                if let Some(rep) = replacement {
                    processed = processed.replace(full_match, &rep);
                } else {
                    processed = processed.replace(full_match, "0");
                }
            } else {
                break;
            }
        }
    }
    processed
}

/// Rewrite one attribute's reads and writes into indexing on its buffer.
///
/// `attrf("mass", E)` becomes `attr_N[E]` and `setattrf("mass", I, V)` becomes
/// `attr_N[I] = (V)` — an assignment expression, so it reads as a statement
/// where the kernel wrote one and still composes where it did not.
fn rewrite_attr_calls(code: &str, name: &str, slot: usize) -> String {
    let mut out = code.to_string();
    for setter in [true, false] {
        let call = if setter { "setattrf(" } else { "attrf(" };
        let mut from = 0;
        loop {
            let Some(pos) = out[from..].find(call) else { break };
            let at = from + pos;
            if !setter && at >= 3 && &out[at - 3..at] == "set" {
                from = at + call.len();
                continue;
            }
            let args_start = at + call.len();
            let Some(args_end) = matching_paren(&out, args_start) else { break };
            let args = &out[args_start..args_end];
            let Some(found) = quoted_arg(args) else {
                from = args_end + 1;
                continue;
            };
            if found != name {
                from = args_end + 1;
                continue;
            }
            // Arguments after the name, split at the top level so an index
            // expression containing a comma inside parentheses stays whole.
            let after_name = match args.find(',') {
                Some(c) => &args[c + 1..],
                None => "",
            };
            let parts = split_top_level(after_name);
            let replacement = if setter {
                match (parts.first(), parts.get(1)) {
                    (Some(i), Some(v)) => format!("attr_{}[{}] = ({})", slot, i.trim(), v.trim()),
                    _ => "0".to_string(),
                }
            } else {
                match parts.first() {
                    Some(i) => format!("attr_{}[{}]", slot, i.trim()),
                    None => "0".to_string(),
                }
            };
            out.replace_range(at..args_end + 1, &replacement);
            from = at + replacement.len();
        }
    }
    out
}

/// Index just past the `(` at `open`, of its matching `)`.
fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split on commas that are not inside parentheses or brackets.
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start <= s.len() {
        parts.push(&s[start..]);
    }
    parts.retain(|p| !p.trim().is_empty());
    parts
}

pub fn parse_dynamic_params(code: &str) -> Vec<ParamDef> {
    let mut parsed = Vec::new();
    let prefixes = [("chf", "slider"), ("chi", "spinbox"), ("chv", "float3"), ("chb", "toggle")];
    for &(prefix, ptype) in &prefixes {
        let pattern = format!("{}(", prefix);
        let mut start_idx = 0;
        while let Some(pos) = code[start_idx..].find(&pattern) {
            let actual_pos = start_idx + pos;
            start_idx = actual_pos + pattern.len();
            let mut paren_count = 1;
            let mut end_pos = start_idx;
            let code_bytes = code.as_bytes();
            while end_pos < code_bytes.len() && paren_count > 0 {
                if code_bytes[end_pos] == b'(' {
                    paren_count += 1;
                } else if code_bytes[end_pos] == b')' {
                    paren_count -= 1;
                }
                end_pos += 1;
            }
            if paren_count == 0 {
                let args_str = &code[start_idx..end_pos - 1];
                if let Some(first_quote_pos) = args_str.find(|c| c == '"' || c == '\'') {
                    let quote_char = args_str.chars().nth(first_quote_pos).unwrap();
                    if let Some(second_quote_pos) = args_str[first_quote_pos + 1..].find(quote_char) {
                        let name = &args_str[first_quote_pos + 1..first_quote_pos + 1 + second_quote_pos];
                        if !name.is_empty() {
                            let mut default_val = match prefix {
                                "chf" => "0.5".to_string(),
                                "chi" => "0".to_string(),
                                "chb" => "false".to_string(),
                                "chv" => "0.00:0.00:0.00".to_string(),
                                _ => "".to_string(),
                            };
                            let rest = &args_str[first_quote_pos + 1 + second_quote_pos + 1..];
                            if let Some(comma_pos) = rest.find(',') {
                                let val_part = rest[comma_pos + 1..].trim();
                                if !val_part.is_empty() {
                                    let mut clean_val = val_part.to_string();
                                    if clean_val.ends_with('f') {
                                        clean_val.pop();
                                    }
                                    if clean_val.ends_with("f32") {
                                        clean_val.truncate(clean_val.len() - 3);
                                    }
                                    let clean_val = clean_val.trim();
                                    if prefix == "chv" {
                                        let parts: Vec<String> = val_part.split(',')
                                            .map(|p| {
                                                let mut s = p.trim().to_string();
                                                if s.ends_with('f') { s.pop(); }
                                                if s.ends_with("f32") { s.truncate(s.len() - 3); }
                                                s.trim().to_string()
                                            })
                                            .collect();
                                        if parts.len() >= 3 {
                                            if let (Ok(x), Ok(y), Ok(z)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>(), parts[2].parse::<f32>()) {
                                                default_val = format!("{:.2}:{:.2}:{:.2}", x, y, z);
                                            }
                                        } else {
                                            if let Ok(val) = clean_val.parse::<f32>() {
                                                default_val = format!("{:.2}:{:.2}:{:.2}", val, val, val);
                                            }
                                        }
                                    } else {
                                        default_val = clean_val.to_string();
                                    }
                                }
                            }
                            if !parsed.iter().any(|p: &ParamDef| p.name == name) {
                                let (min, max, step) = match prefix {
                                    "chf" => (Some(0.0), Some(2.0), Some(0.01)),
                                    "chi" => (Some(0.0), Some(1000.0), Some(1.0)),
                                    _ => (None, None, None),
                                };
                                parsed.push(ParamDef {
                                    name: name.to_string(),
                                    label: String::new(),
                                    param_type: ptype.to_string(),
                                    default: default_val,
                                    options: Vec::new(),
                                    min,
                                    max,
                                    step,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    parsed
}

pub fn resolve_opencl_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_name = node_param_str(target, "Input", "");
    let mut input = if !input_name.is_empty() {
        // Siblings first, exactly like the output type's lookup: subnet
        // templates (Extrude) wire their inner opencl to a child named
        // "input1", and a global-first search would resolve to the FIRST
        // subnet's child once two instances exist.
        let sibling = find_parent_node(root, &target.id)
            .and_then(|p| p.children.iter().find(|c| c.name == input_name || c.id == input_name));
        if let Some(input_node) = sibling.or_else(|| find_node_by_name(root, &input_name)) {
            generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)
                .unwrap_or_default()
        } else {
            Detail::new()
        }
    } else {
        Detail::new()
    };

    let code = node_param_str(target, "Code", "");
    if !code.is_empty() {
        let parsed_params = parse_dynamic_params(&code);
        let mut flat_values = Vec::new();
        for p in &parsed_params {
            let mut val_str = node_param_str(target, &p.name, &p.default);
            if !target.params.iter().any(|p_def| p_def.name.eq_ignore_ascii_case(&p.name)) {
                if let Some(parent) = find_parent_node(root, &target.id) {
                    val_str = node_param_str(parent, &p.name, &val_str);
                }
            }
            if p.param_type == "float3" {
                let parts: Vec<&str> = val_str.split(':').collect();
                let (x, y, z) = if parts.len() >= 3 {
                    (parts[0].parse::<f32>().unwrap_or(0.0), parts[1].parse::<f32>().unwrap_or(0.0), parts[2].parse::<f32>().unwrap_or(0.0))
                } else {
                    (0.0, 0.0, 0.0)
                };
                flat_values.push(x);
                flat_values.push(y);
                flat_values.push(z);
            } else {
                let val = if val_str.eq_ignore_ascii_case("true") {
                    1.0
                } else if val_str.eq_ignore_ascii_case("false") {
                    0.0
                } else {
                    val_str.parse::<f32>().unwrap_or(0.0)
                };
                flat_values.push(val);
            }
        }

        let processed_code = preprocess_opencl_code(&code);
        let is_generator = code.contains("out_count");

        // A DEFORMER runs over POINTS. It never sees a triangle, so topology,
        // groups and point identities pass through untouched — the flatten,
        // weld and "first corner wins" reconciliation this used to need are
        // all gone. Inside a simnet that is the difference between a solver
        // that can follow a point across frames and one that cannot.
        if !is_generator {
            let result = run_kernel_on_detail(&processed_code, &mut input, &flat_values);
            if let Err(e) = result {
                if ocl_error.is_none() {
                    *ocl_error = Some(e);
                }
            }
            return Some(input);
        }

        // A GENERATOR builds a new corner list, so it still speaks soup and
        // its output welds into fresh geometry with fresh identities. Widening
        // the generator ABI to emit points and primitives directly is the
        // piece of Phase 1 still outstanding.
        let mut geom = detail_to_soup(&input);
        // CPU reference backend (kernel_cpu): forced via CCE_KERNEL_CPU=1, and
        // the automatic fallback when there is no OpenCL platform at all — the
        // state this machine reached silently when nvidia-open fell out of
        // kernel lockstep, which used to mean every kernel node produced
        // empty geometry. A kernel that FAILS on a present platform (compile
        // error, bad code) does NOT fall back: the two backends share the
        // language, so the error is almost certainly in the kernel, and
        // hiding the GPU diagnostics behind a second attempt would obscure it.
        let result = if crate::kernel_cpu::forced() {
            crate::kernel_cpu::run_kernel_cpu(&processed_code, &mut geom, &flat_values)
        } else {
            match run_opencl_kernel_with_params(&processed_code, &mut geom, &flat_values) {
                Err(e) if e.contains("No OpenCL platforms/devices found") => {
                    note_cpu_fallback_once();
                    crate::kernel_cpu::run_kernel_cpu(&processed_code, &mut geom, &flat_values)
                }
                r => r,
            }
        };
        if let Err(e) = result {
            if ocl_error.is_none() {
                *ocl_error = Some(e);
            }
        }
        return Some(soup_to_detail(&geom));
    }

    Some(input)
}


/// One stderr note per process when kernels silently move to the CPU
/// reference — the sphere disappearing taught us "silently" is the problem.
fn note_cpu_fallback_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        eprintln!("cce-designer: no OpenCL platform — node kernels running on the CPU reference backend");
    });
}

struct OpenClCache {
    device: opencl3::device::Device,
    context: opencl3::context::Context,
    queue: opencl3::command_queue::CommandQueue,
    kernels: std::collections::HashMap<String, opencl3::kernel::Kernel>,
}

static OPENCL_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<OpenClCache>>> = std::sync::OnceLock::new();

fn init_opencl() -> Option<OpenClCache> {
    let platforms = get_platforms().ok()?;
    if platforms.is_empty() {
        return None;
    }
    let mut device_id = None;
    for platform in &platforms {
        if let Ok(devices) = platform.get_devices(CL_DEVICE_TYPE_GPU) {
            if !devices.is_empty() {
                device_id = Some(devices[0]);
                break;
            }
        }
    }
    if device_id.is_none() {
        for platform in &platforms {
            if let Ok(devices) = platform.get_devices(CL_DEVICE_TYPE_CPU) {
                if !devices.is_empty() {
                    device_id = Some(devices[0]);
                    break;
                }
            }
        }
    }
    let device_id = device_id?;
    let device = Device::new(device_id);
    let context = Context::from_device(&device).ok()?;
    let queue = unsafe { CommandQueue::create_with_properties(&context, device_id, 0, 0) }.ok()?;
    Some(OpenClCache {
        device,
        context,
        queue,
        kernels: std::collections::HashMap::new(),
    })
}

pub fn run_opencl_kernel(code: &str, geom: &mut Geometry) -> Result<(), String> {
    run_opencl_kernel_with_params(code, geom, &[])
}

pub fn run_opencl_kernel_with_params(code: &str, geom: &mut Geometry, params: &[f32]) -> Result<(), String> {
    let is_generator = code.contains("out_count");
    if geom.vertices.is_empty() && !is_generator {
        return Ok(());
    }

    let mut cache_guard = OPENCL_CACHE
        .get_or_init(|| std::sync::Mutex::new(init_opencl()))
        .lock()
        .map_err(|e| format!("Failed to lock OpenCL cache: {:?}", e))?;

    let cache = cache_guard.as_mut().ok_or_else(|| "No OpenCL platforms/devices found".to_string())?;

    if !cache.kernels.contains_key(code) {
        let mut program = Program::create_from_source(&cache.context, code)
            .map_err(|e| format!("Failed to create Program: {:?}", e))?;
        if let Err(e) = program.build(&[cache.device.id()], "") {
            let log = program.get_build_log(cache.device.id()).unwrap_or_else(|_| "Failed to retrieve build log".to_string());
            return Err(format!("OpenCL JIT compilation error: {}\nLog:\n{}", e, log));
        }
        let kernel = Kernel::create(&program, "process")
            .map_err(|e| format!("Failed to create kernel 'process': {:?}", e))?;
        cache.kernels.insert(code.to_string(), kernel);
    }
    let kernel = cache.kernels.get(code).unwrap();
    let context = &cache.context;
    let queue = &cache.queue;

    // Prepare parameter values buffer
    let mut param_values_data = params.to_vec();
    if param_values_data.is_empty() {
        param_values_data.push(0.0);
    }
    let mut param_values_buf = unsafe {
        ClBuffer::<cl_float>::create(&context, CL_MEM_READ_WRITE, param_values_data.len(), std::ptr::null_mut())
            .map_err(|e| format!("Failed to create param_values buffer: {:?}", e))?
    };
    let _write_param_event = unsafe {
        queue.enqueue_write_buffer(&mut param_values_buf, CL_TRUE, 0, &param_values_data, &[])
            .map_err(|e| format!("Failed to write param_values buffer: {:?}", e))?
    };

    if is_generator {
        let in_count = geom.vertices.len();
        let max_vertices = 200_000;

        // Prepare flat input position and color data
        let mut in_pos_data: Vec<cl_float> = Vec::with_capacity(in_count * 3);
        let mut in_col_data: Vec<cl_float> = Vec::with_capacity(in_count * 3);
        for v in &geom.vertices {
            in_pos_data.extend_from_slice(&v.pos);
            in_col_data.extend_from_slice(&v.col);
        }

        // Create GPU buffers for inputs
        let mut in_pos_buf = unsafe {
            ClBuffer::<cl_float>::create(&context, CL_MEM_READ_WRITE, (in_count * 3).max(1), std::ptr::null_mut())
                .map_err(|e| format!("Failed to create input positions buffer: {:?}", e))?
        };
        let mut in_col_buf = unsafe {
            ClBuffer::<cl_float>::create(&context, CL_MEM_READ_WRITE, (in_count * 3).max(1), std::ptr::null_mut())
                .map_err(|e| format!("Failed to create input colors buffer: {:?}", e))?
        };

        // Write input data to GPU
        let _write_pos_event = unsafe {
            let write_data = if in_pos_data.is_empty() { &[0.0f32] } else { &in_pos_data[..] };
            queue.enqueue_write_buffer(&mut in_pos_buf, CL_TRUE, 0, write_data, &[])
                .map_err(|e| format!("Failed to write input positions buffer: {:?}", e))?
        };
        let _write_col_event = unsafe {
            let write_data = if in_col_data.is_empty() { &[0.0f32] } else { &in_col_data[..] };
            queue.enqueue_write_buffer(&mut in_col_buf, CL_TRUE, 0, write_data, &[])
                .map_err(|e| format!("Failed to write input colors buffer: {:?}", e))?
        };

        // Create GPU buffers for outputs
        let out_pos_buf = unsafe {
            ClBuffer::<cl_float>::create(&context, CL_MEM_READ_WRITE, max_vertices * 3, std::ptr::null_mut())
                .map_err(|e| format!("Failed to create output positions buffer: {:?}", e))?
        };
        let out_col_buf = unsafe {
            ClBuffer::<cl_float>::create(&context, CL_MEM_READ_WRITE, max_vertices * 3, std::ptr::null_mut())
                .map_err(|e| format!("Failed to create output colors buffer: {:?}", e))?
        };

        // Create output count buffer initialized to 0
        let mut out_count_buf = unsafe {
            ClBuffer::<cl_int>::create(&context, CL_MEM_READ_WRITE, 1, std::ptr::null_mut())
                .map_err(|e| format!("Failed to create output count buffer: {:?}", e))?
        };
        let initial_count_data: [cl_int; 1] = [0];
        let _write_count_event = unsafe {
            queue.enqueue_write_buffer(&mut out_count_buf, CL_TRUE, 0, &initial_count_data, &[])
                .map_err(|e| format!("Failed to write output count buffer: {:?}", e))?
        };

        // Execute kernel
        let global_work_size = if in_count == 0 { 1 } else { in_count };
        let mut exec = ExecuteKernel::new(kernel);
        let kernel_event = unsafe {
            exec.set_arg(&in_pos_buf)
                .set_arg(&in_col_buf)
                .set_arg(&(in_count as cl_int))
                .set_arg(&out_pos_buf)
                .set_arg(&out_col_buf)
                .set_arg(&out_count_buf)
                .set_arg(&(max_vertices as cl_int));

            let num_args = kernel.num_args().unwrap_or(0);
            if num_args >= 8 {
                exec.set_arg(&param_values_buf);
            }

            exec.set_global_work_size(global_work_size)
                .enqueue_nd_range(&queue)
                .map_err(|e| format!("Failed to enqueue kernel: {:?}", e))?
        };

        kernel_event.wait().map_err(|e| format!("Failed to wait for kernel: {:?}", e))?;

        // Read count back
        let mut final_count_data: [cl_int; 1] = [0];
        let _read_count_event = unsafe {
            queue.enqueue_read_buffer(&out_count_buf, CL_TRUE, 0, &mut final_count_data, &[])
                .map_err(|e| format!("Failed to read output count: {:?}", e))?
        };
        let final_count = (final_count_data[0] as usize).min(max_vertices);

        // Read output positions and colors back
        let mut out_pos_data: Vec<cl_float> = vec![0.0; final_count * 3];
        let mut out_col_data: Vec<cl_float> = vec![0.0; final_count * 3];
        if final_count > 0 {
            let _read_pos_event = unsafe {
                queue.enqueue_read_buffer(&out_pos_buf, CL_TRUE, 0, &mut out_pos_data, &[])
                    .map_err(|e| format!("Failed to read output positions buffer: {:?}", e))?
            };
            let _read_col_event = unsafe {
                queue.enqueue_read_buffer(&out_col_buf, CL_TRUE, 0, &mut out_col_data, &[])
                    .map_err(|e| format!("Failed to read output colors buffer: {:?}", e))?
            };
        }

        // Rebuild geometry vertices
        geom.vertices.clear();
        for i in 0..final_count {
            let mut attributes = HashMap::new();
            attributes.insert("Norm".to_string(), GAttribute::Float3([0.0, 1.0, 0.0]));
            attributes.insert("UV".to_string(), GAttribute::Float2([0.0, 0.0]));
            geom.vertices.push(GVertex {
                pos: [out_pos_data[i * 3], out_pos_data[i * 3 + 1], out_pos_data[i * 3 + 2]],
                col: [out_col_data[i * 3], out_col_data[i * 3 + 1], out_col_data[i * 3 + 2]],
                attributes,
            });
        }
    } else {
        let mut pos_data: Vec<cl_float> = Vec::with_capacity(geom.vertices.len() * 3);
        let mut col_data: Vec<cl_float> = Vec::with_capacity(geom.vertices.len() * 3);
        for v in &geom.vertices {
            pos_data.extend_from_slice(&v.pos);
            col_data.extend_from_slice(&v.col);
        }
        let count = geom.vertices.len();
        drop(cache_guard);
        run_deformer_flat(code, &mut pos_data, &mut col_data, count, &mut [], params)?;
        for i in 0..count {
            geom.vertices[i].pos = [pos_data[i * 3], pos_data[i * 3 + 1], pos_data[i * 3 + 2]];
            geom.vertices[i].col = [col_data[i * 3], col_data[i * 3 + 1], col_data[i * 3 + 2]];
        }
        return Ok(());
    }

    Ok(())
}

/// Run a deformer kernel over flat per-element buffers, reading everything back
/// in place.
///
/// `pos` and `col` hold three floats per element; each entry of `attrs` holds
/// one, and they bind in the order [`parse_attr_refs`] found them — which is
/// the order [`preprocess_opencl_code`] appended them to the signature.
///
/// The one implementation behind both the soup path (which binds no
/// attributes) and the point-native path.
fn run_deformer_flat(
    code: &str,
    pos: &mut Vec<cl_float>,
    col: &mut Vec<cl_float>,
    count: usize,
    attrs: &mut [(String, Vec<cl_float>)],
    params: &[f32],
) -> Result<(), String> {
    if count == 0 {
        return Ok(());
    }
    let mut cache_guard = OPENCL_CACHE
        .get_or_init(|| std::sync::Mutex::new(init_opencl()))
        .lock()
        .map_err(|e| format!("Failed to lock OpenCL cache: {:?}", e))?;
    let cache = cache_guard.as_mut().ok_or_else(|| "No OpenCL platforms/devices found".to_string())?;

    if !cache.kernels.contains_key(code) {
        let mut program = Program::create_from_source(&cache.context, code)
            .map_err(|e| format!("Failed to create Program: {:?}", e))?;
        if let Err(e) = program.build(&[cache.device.id()], "") {
            let log = program
                .get_build_log(cache.device.id())
                .unwrap_or_else(|_| "Failed to retrieve build log".to_string());
            return Err(format!("OpenCL JIT compilation error: {}\nLog:\n{}", e, log));
        }
        let kernel = Kernel::create(&program, "process")
            .map_err(|e| format!("Failed to create kernel 'process': {:?}", e))?;
        cache.kernels.insert(code.to_string(), kernel);
    }
    let kernel = cache.kernels.get(code).unwrap();
    let (context, queue) = (&cache.context, &cache.queue);

    let mut param_data = params.to_vec();
    if param_data.is_empty() {
        param_data.push(0.0);
    }

    let mut make = |data: &[cl_float]| -> Result<ClBuffer<cl_float>, String> {
        let mut buf = unsafe {
            ClBuffer::<cl_float>::create(context, CL_MEM_READ_WRITE, data.len().max(1), std::ptr::null_mut())
                .map_err(|e| format!("Failed to create buffer: {:?}", e))?
        };
        unsafe {
            queue
                .enqueue_write_buffer(&mut buf, CL_TRUE, 0, data, &[])
                .map_err(|e| format!("Failed to write buffer: {:?}", e))?
        };
        Ok(buf)
    };

    let mut pos_buf = make(pos)?;
    let mut col_buf = make(col)?;
    let mut param_buf = make(&param_data)?;
    let mut attr_bufs: Vec<ClBuffer<cl_float>> = Vec::with_capacity(attrs.len());
    for (_, data) in attrs.iter() {
        attr_bufs.push(make(data)?);
    }

    let mut exec = ExecuteKernel::new(kernel);
    let kernel_event = unsafe {
        exec.set_arg(&pos_buf).set_arg(&col_buf).set_arg(&(count as cl_int));
        // The signature carries param_values only when the kernel names a
        // parameter, so the attribute buffers slide up one slot when it does
        // not. Arity is the authority, exactly as it was for the old
        // `num_args >= 4` check.
        let num_args = kernel.num_args().unwrap_or(0) as usize;
        if num_args > 3 + attrs.len() {
            exec.set_arg(&param_buf);
        }
        for buf in &attr_bufs {
            exec.set_arg(buf);
        }
        exec.set_global_work_size(count)
            .enqueue_nd_range(queue)
            .map_err(|e| format!("Failed to enqueue kernel: {:?}", e))?
    };
    kernel_event.wait().map_err(|e| format!("Failed to wait for kernel: {:?}", e))?;

    unsafe {
        queue
            .enqueue_read_buffer(&mut pos_buf, CL_TRUE, 0, pos, &[])
            .map_err(|e| format!("Failed to read positions buffer: {:?}", e))?;
        queue
            .enqueue_read_buffer(&mut col_buf, CL_TRUE, 0, col, &[])
            .map_err(|e| format!("Failed to read colors buffer: {:?}", e))?;
        for (buf, (_, data)) in attr_bufs.iter().zip(attrs.iter_mut()) {
            queue
                .enqueue_read_buffer(buf, CL_TRUE, 0, data, &[])
                .map_err(|e| format!("Failed to read attribute buffer: {:?}", e))?;
        }
    }
    Ok(())
}

/// Run a DEFORMER kernel over a [`Detail`]'s points.
///
/// This is the Phase 1 ABI: the work item is a POINT, not a triangle corner,
/// and the kernel reaches named attributes through buffers of its own. Two
/// things follow. A deformer no longer flattens and welds — topology, groups
/// and point identities are simply untouched, because the kernel never saw
/// them. And "the first corner wins", which the soup round trip had to invent
/// where corners of one point disagreed, stops being a question: there is one
/// value per point because there is one point.
///
/// An attribute the kernel names but the geometry lacks is created and zeroed:
/// naming it is the declaration.
pub fn run_kernel_on_detail(code: &str, geom: &mut Detail, params: &[f32]) -> Result<(), String> {
    let count = geom.num_points();
    if count == 0 {
        return Ok(());
    }
    let mut pos: Vec<cl_float> = bytemuck::cast_slice(geom.positions()).to_vec();
    let mut col: Vec<cl_float> = Vec::with_capacity(count * 3);
    for p in 0..count {
        col.extend_from_slice(&geom.color(p));
    }

    let names = parse_attr_refs(code);
    let mut attrs: Vec<(String, Vec<cl_float>)> = Vec::with_capacity(names.len());
    for name in &names {
        let data = match geom.points().get(name) {
            Some(AttribData::Float(v)) => v.clone(),
            Some(other) => (0..count)
                .map(|p| other.get(p).map(|v| v.as_f32()).unwrap_or(0.0))
                .collect(),
            None => vec![0.0; count],
        };
        attrs.push((name.clone(), data));
    }

    let run = if crate::kernel_cpu::forced() {
        crate::kernel_cpu::run_deformer_cpu(code, &mut pos, &mut col, count, &mut attrs, params)
    } else {
        match run_deformer_flat(code, &mut pos, &mut col, count, &mut attrs, params) {
            Err(e) if e.contains("No OpenCL platforms/devices found") => {
                note_cpu_fallback_once();
                crate::kernel_cpu::run_deformer_cpu(code, &mut pos, &mut col, count, &mut attrs, params)
            }
            r => r,
        }
    };
    run?;

    for (p, slot) in geom.positions_mut().iter_mut().enumerate() {
        *slot = [pos[p * 3], pos[p * 3 + 1], pos[p * 3 + 2]];
    }
    for p in 0..count {
        geom.set_color(p, [col[p * 3], col[p * 3 + 1], col[p * 3 + 2]]);
    }
    for (name, data) in attrs {
        let _ = geom.points_mut().insert(&name, AttribData::Float(data));
    }
    Ok(())
}

pub fn is_geometry_node_type(node_type: &str) -> bool {
    let nt = node_type.to_lowercase();
    nt == "sphere"
        || nt == "line"
        || nt == "curve"
        || nt == "points"
        || nt == "transform"
        || nt == "opencl"
        || nt == "box"
        || nt == "input"
        || nt == "output"
        || nt == "scatter"
        || nt == "group"
        || nt == "collision"
        || nt == "relax"
        || nt == "attribute"
        || nt == "simnet"
}

/// The scene at the timeline's start frame, with a throwaway sim cache — every
/// simnet shows its seed. Callers that have a timeline should build their own
/// [`EvalSim`] and keep its [`SimCache`] across frames.
pub fn network_sphere_vertices(root: &FsNode) -> Detail {
    let mut err = None;
    let mut cache = SimCache::default();
    let mut sim = EvalSim::new(0, 0, &mut cache);
    network_sphere_vertices_with_errors(root, root, &mut err, &mut sim)
}

/// Collect the scene's geometry by walking `start`'s children. `start` is the
/// network level the viewport displays — the network editor's current
/// directory — while `root` stays the evaluation root: resolvers look inputs
/// up by name from `root`, so a chain inside the displayed level still
/// resolves references exactly as it does when drawn from the top. Pass
/// `root` for both to draw the whole scene (thumbnails do).
///
/// Two interior-display rules apply only to the displayed level itself:
/// started AT a simnet the walk draws the solved state instead of the step
/// chain, and `input`/`output` children draw their resolved geometry — but
/// only as direct children of `start`, so viewing a subnet from OUTSIDE
/// still draws its internals exactly once (via recursion, as before).
pub fn network_sphere_vertices_with_errors(
    root: &FsNode,
    start: &FsNode,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Detail {
    // Inside a simnet the chain is the simulation STEP; drawing its nodes
    // would show one un-iterated pass of the chain. The interior view is the
    // solved state at the current frame — the same geometry the parent level
    // draws for the simnet — toggled by the output child's geometry flag.
    if start.node_type.eq_ignore_ascii_case("simnet") {
        let mut out = Detail::new();
        let display_on = start
            .children
            .iter()
            .find(|c| c.node_type.eq_ignore_ascii_case("output"))
            .map(|o| o.geometry_visible)
            .unwrap_or(true);
        if display_on {
            let mut visited = Vec::new();
            if let Some(geom) = resolve_simnet_geometry_with_errors(root, start, &mut visited, ocl_error, sim) {
                out.merge(&geom);
            }
        }
        return out;
    }
    fn visit(root: &FsNode, node: &FsNode, parent_visible: bool, top: bool, count: &mut usize, out: &mut Detail, ocl_error: &mut Option<String>, sim: &mut EvalSim) {
        let is_visible = parent_visible && node.geometry_visible;
        if node.node_type.eq_ignore_ascii_case("sphere") {
            let idx = *count;
            *count += 1;
            if is_visible {
                let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                out.merge(&sphere_detail(center, node_param_f32(node, "Radius", 0.5).max(0.05), 16, 24));
            }
        } else if node.node_type.eq_ignore_ascii_case("line") {
            let idx = *count;
            *count += 1;
            if is_visible {
                let start = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                let length = node_param_f32(node, "Length", 1.0);
                let thickness = node_param_f32(node, "Thickness", 0.02);
                let end = start + Vec3::new(0.0, length, 0.0);
                out.merge(&box_detail(start, end, thickness));
            }
        } else if node.node_type.eq_ignore_ascii_case("curve") {
            // Absolute world coordinates: no grid-index placement, and
            // `count` untouched so find_sphere_index stays aligned.
            if is_visible {
                out.merge(&curve_detail(node));
            }
        } else if node.node_type.eq_ignore_ascii_case("points") {
            let idx = *count;
            *count += 1;
            if is_visible {
                let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                out.merge(&points_detail(node, center));
            }
        } else if node.node_type.eq_ignore_ascii_case("transform") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_transform_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("scatter") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_scatter_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("group") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_group_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("attribute") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_attribute_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("relax") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_relax_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("collision") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_collision_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("opencl") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_opencl_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("simnet") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_simnet_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
            // The chain inside a simnet is the simulation STEP, not scene
            // content. Recursing into it the way a subnet is recursed into
            // would merge one un-iterated pass of the chain alongside the
            // solved result — the sim would draw itself twice, once wrong.
            return;
        } else if top
            && (node.node_type.eq_ignore_ascii_case("input")
                || node.node_type.eq_ignore_ascii_case("output"))
        {
            // Interior display: dived into a subnet whose chain ends at the
            // pass-through nodes, the input draws the incoming geometry and
            // the output draws the chain's result — otherwise a subnet with
            // no generator inside shows an empty viewport. Top level only:
            // from outside, a subnet's internals already draw by recursion,
            // and adding these would draw the chain a second time. Not
            // counted — `count` stays aligned with find_sphere_index.
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = generate_single_node_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
            return;
        }
        for child in &node.children {
            visit(root, child, is_visible, false, count, out, ocl_error, sim);
        }
    }

    let mut out = Detail::new();
    let mut count = 0;
    for child in &start.children {
        visit(root, child, true, true, &mut count, &mut out, ocl_error, sim);
    }
    out
}

/// The Points node's cloud (type "points", nee "add"): `Points` markers
/// arranged by the `Shape` param around `center`. One function for both
/// consumers — the single-node resolver and the scene walk — so the two
/// renderings can never drift apart.
pub fn points_node_geometry(node: &FsNode, center: Vec3) -> Geometry {
    detail_to_soup(&points_detail(node, center))
}

/// The Points node: a marker sphere at each generated location.
///
/// Every marker stays its own piece — `merge` reallocates identities, so two
/// markers that happen to land on the same spot are still two points with two
/// identities rather than one welded blob.
pub fn points_detail(node: &FsNode, center: Vec3) -> Detail {
    let num_points = node_param_f32(node, "Points", 100.0) as i32;
    let shape = node_param_str(node, "Shape", "None");
    let mut d = Detail::new();
    for i in 0..num_points {
        let t = i as f32 / num_points.max(1) as f32;
        let offset = match shape.as_str() {
            "Spiral" => {
                let angle = t * std::f32::consts::TAU * 3.0;
                let r = 0.4 * t;
                Vec3::new(r * angle.cos(), t * 0.5 - 0.25, r * angle.sin())
            }
            "Line" => Vec3::new(t - 0.5, 0.0, 0.0),
            "Circle" => {
                let angle = t * std::f32::consts::TAU;
                Vec3::new(0.4 * angle.cos(), 0.0, 0.4 * angle.sin())
            }
            "Grid" => {
                let side = (num_points as f32).sqrt().ceil().max(1.0) as i32;
                let step = if side > 1 { 0.8 / (side - 1) as f32 } else { 0.0 };
                Vec3::new((i % side) as f32 * step - 0.4, 0.0, (i / side) as f32 * step - 0.4)
            }
            // "None" and anything unrecognized: every point at the same spot.
            _ => Vec3::ZERO,
        };
        d.merge(&sphere_detail(center + offset, 0.02, 6, 8));
    }
    d
}

pub fn find_sphere_index(root: &FsNode, target: &FsNode) -> Option<usize> {
    fn visit(node: &FsNode, target: &FsNode, count: &mut usize) -> Option<usize> {
        let is_target = std::ptr::eq(node, target);
        if node.node_type.eq_ignore_ascii_case("sphere") 
            || node.node_type.eq_ignore_ascii_case("line") 
            || node.node_type.eq_ignore_ascii_case("points")
            || node.node_type.eq_ignore_ascii_case("transform")
            || node.node_type.eq_ignore_ascii_case("opencl")
            || node.node_type.eq_ignore_ascii_case("scatter") {
            let idx = *count;
            *count += 1;
            if is_target {
                return Some(idx);
            }
        }
        for child in &node.children {
            if let Some(res) = visit(child, target, count) {
                return Some(res);
            }
        }
        None
    }
    let mut count = 0;
    for child in &root.children {
        if let Some(res) = visit(child, target, &mut count) {
            return Some(res);
        }
    }
    None
}

fn add_box(center: Vec3, size: Vec3, color: [f32; 3], verts: &mut Vec<Vertex3D>) {
    let dx = size.x * 0.5;
    let dy = size.y * 0.5;
    let dz = size.z * 0.5;

    let faces = [
        // front (z = +dz)
        [-dx, -dy, dz,  dx, -dy, dz,  dx, dy, dz,  -dx, -dy, dz,  dx, dy, dz,  -dx, dy, dz],
        // back (z = -dz)
        [-dx, -dy, -dz,  -dx, dy, -dz,  dx, dy, -dz,  -dx, -dy, -dz,  dx, dy, -dz,  dx, -dy, -dz],
        // left (x = -dx)
        [-dx, -dy, -dz,  -dx, -dy, dz,  -dx, dy, dz,  -dx, -dy, -dz,  -dx, dy, dz,  -dx, dy, -dz],
        // right (x = +dx)
        [dx, -dy, -dz,  dx, dy, -dz,  dx, dy, dz,  dx, -dy, -dz,  dx, dy, dz,  dx, -dy, dz],
        // top (y = +dy)
        [-dx, dy, -dz,  -dx, dy, dz,  dx, dy, dz,  -dx, dy, -dz,  dx, dy, dz,  dx, dy, -dz],
        // bottom (y = -dy)
        [-dx, -dy, -dz,  dx, -dy, -dz,  dx, -dy, dz,  -dx, -dy, -dz,  dx, -dy, dz,  -dx, -dy, dz],
    ];

    for face in &faces {
        for chunk in face.chunks(3) {
            verts.push(Vertex3D {
                position: [center.x + chunk[0], center.y + chunk[1], center.z + chunk[2]],
                color,
            });
        }
    }
}

fn add_pyramid_x(base_center: Vec3, base_size: f32, height: f32, color: [f32; 3], verts: &mut Vec<Vertex3D>) {
    let s = base_size * 0.5;
    let x = base_center.x;
    let y = base_center.y;
    let z = base_center.z;
    
    let p0 = Vec3::new(x, y - s, z - s);
    let p1 = Vec3::new(x, y + s, z - s);
    let p2 = Vec3::new(x, y + s, z + s);
    let p3 = Vec3::new(x, y - s, z + s);
    let tip = Vec3::new(x + height, y, z);
    
    // Base (two triangles)
    verts.push(Vertex3D { position: [p0.x, p0.y, p0.z], color });
    verts.push(Vertex3D { position: [p2.x, p2.y, p2.z], color });
    verts.push(Vertex3D { position: [p1.x, p1.y, p1.z], color });
    
    verts.push(Vertex3D { position: [p0.x, p0.y, p0.z], color });
    verts.push(Vertex3D { position: [p3.x, p3.y, p3.z], color });
    verts.push(Vertex3D { position: [p2.x, p2.y, p2.z], color });
    
    // Sides
    let sides = [
        (p0, p3), (p3, p2), (p2, p1), (p1, p0)
    ];
    for (a, b) in &sides {
        verts.push(Vertex3D { position: [a.x, a.y, a.z], color });
        verts.push(Vertex3D { position: [tip.x, tip.y, tip.z], color });
        verts.push(Vertex3D { position: [b.x, b.y, b.z], color });
    }
}

fn add_pyramid_y(base_center: Vec3, base_size: f32, height: f32, color: [f32; 3], verts: &mut Vec<Vertex3D>) {
    let s = base_size * 0.5;
    let x = base_center.x;
    let y = base_center.y;
    let z = base_center.z;
    
    let p0 = Vec3::new(x - s, y, z - s);
    let p1 = Vec3::new(x + s, y, z - s);
    let p2 = Vec3::new(x + s, y, z + s);
    let p3 = Vec3::new(x - s, y, z + s);
    let tip = Vec3::new(x, y + height, z);
    
    // Base (two triangles)
    verts.push(Vertex3D { position: [p0.x, p0.y, p0.z], color });
    verts.push(Vertex3D { position: [p1.x, p1.y, p1.z], color });
    verts.push(Vertex3D { position: [p2.x, p2.y, p2.z], color });
    
    verts.push(Vertex3D { position: [p0.x, p0.y, p0.z], color });
    verts.push(Vertex3D { position: [p2.x, p2.y, p2.z], color });
    verts.push(Vertex3D { position: [p3.x, p3.y, p3.z], color });
    
    // Sides
    let sides = [
        (p0, p1), (p1, p2), (p2, p3), (p3, p0)
    ];
    for (a, b) in &sides {
        verts.push(Vertex3D { position: [a.x, a.y, a.z], color });
        verts.push(Vertex3D { position: [tip.x, tip.y, tip.z], color });
        verts.push(Vertex3D { position: [b.x, b.y, b.z], color });
    }
}

fn add_pyramid_z(base_center: Vec3, base_size: f32, height: f32, color: [f32; 3], verts: &mut Vec<Vertex3D>) {
    let s = base_size * 0.5;
    let x = base_center.x;
    let y = base_center.y;
    let z = base_center.z;
    
    let p0 = Vec3::new(x - s, y - s, z);
    let p1 = Vec3::new(x + s, y - s, z);
    let p2 = Vec3::new(x + s, y + s, z);
    let p3 = Vec3::new(x - s, y + s, z);
    let tip = Vec3::new(x, y, z + height);
    
    // Base (two triangles)
    verts.push(Vertex3D { position: [p0.x, p0.y, p0.z], color });
    verts.push(Vertex3D { position: [p2.x, p2.y, p2.z], color });
    verts.push(Vertex3D { position: [p1.x, p1.y, p1.z], color });
    
    verts.push(Vertex3D { position: [p0.x, p0.y, p0.z], color });
    verts.push(Vertex3D { position: [p3.x, p3.y, p3.z], color });
    verts.push(Vertex3D { position: [p2.x, p2.y, p2.z], color });
    
    // Sides
    let sides = [
        (p0, p3), (p3, p2), (p2, p1), (p1, p0)
    ];
    for (a, b) in &sides {
        verts.push(Vertex3D { position: [a.x, a.y, a.z], color });
        verts.push(Vertex3D { position: [tip.x, tip.y, tip.z], color });
        verts.push(Vertex3D { position: [b.x, b.y, b.z], color });
    }
}

pub fn origin_vectors_vertices(scale: f32) -> Vec<Vertex3D> {
    let mut verts = Vec::new();
    
    let t = 0.008 * scale; 
    let a_size = 0.024 * scale;
    let a_height = 0.15 * scale;
    let axis_len = 0.85 * scale;
    let half_axis_len = 0.425 * scale;
    
    // Red for X-axis (points to +scale)
    let red = [0.9, 0.1, 0.1];
    add_box(Vec3::new(half_axis_len, 0.0, 0.0), Vec3::new(axis_len, t, t), red, &mut verts);
    add_pyramid_x(Vec3::new(axis_len, 0.0, 0.0), a_size, a_height, red, &mut verts);

    // Green for Y-axis (points to +scale)
    let green = [0.1, 0.8, 0.1];
    add_box(Vec3::new(0.0, half_axis_len, 0.0), Vec3::new(t, axis_len, t), green, &mut verts);
    add_pyramid_y(Vec3::new(0.0, axis_len, 0.0), a_size, a_height, green, &mut verts);

    // Blue for Z-axis (points to +scale)
    let blue = [0.1, 0.1, 0.9];
    add_box(Vec3::new(0.0, 0.0, half_axis_len), Vec3::new(t, t, axis_len), blue, &mut verts);
    add_pyramid_z(Vec3::new(0.0, 0.0, axis_len), a_size, a_height, blue, &mut verts);

    verts
}

/// The Render node's point display: one small ball per DISTINCT vertex
/// position of `src` (positions quantized for the dedup — the raw triangle
/// soup repeats each vertex per face). A low-res UV sphere in a uniform color
/// reads as a flat CIRCLE from every viewpoint, with no camera-dependent
/// billboarding to rebuild on orbit. The vertex ordering copies the OpenCL
/// sphere kernel's exactly — that winding is the one the raster pass's
/// backface cull is known to keep.
pub fn points_vertices(src: &[Vertex3D], size: f32, color: [f32; 3]) -> Vec<Vertex3D> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let r = size.max(0.001);
    const LAT_STEPS: usize = 4;
    const LON_STEPS: usize = 10;
    let pi = std::f32::consts::PI;
    for v in src {
        let key = (
            (v.position[0] * 1000.0).round() as i32,
            (v.position[1] * 1000.0).round() as i32,
            (v.position[2] * 1000.0).round() as i32,
        );
        if !seen.insert(key) {
            continue;
        }
        let [cx, cy, cz] = v.position;
        let sp = |theta: f32, phi: f32| {
            [
                cx + r * theta.sin() * phi.cos(),
                cy + r * theta.cos(),
                cz + r * theta.sin() * phi.sin(),
            ]
        };
        for lat in 0..LAT_STEPS {
            let theta0 = pi * lat as f32 / LAT_STEPS as f32;
            let theta1 = pi * (lat + 1) as f32 / LAT_STEPS as f32;
            for lon in 0..LON_STEPS {
                let phi0 = 2.0 * pi * lon as f32 / LON_STEPS as f32;
                let phi1 = 2.0 * pi * (lon + 1) as f32 / LON_STEPS as f32;
                let p00 = sp(theta0, phi0);
                let p10 = sp(theta1, phi0);
                let p11 = sp(theta1, phi1);
                let p01 = sp(theta0, phi1);
                for p in [p00, p10, p11, p00, p11, p01] {
                    out.push(Vertex3D { position: p, color });
                }
            }
        }
    }
    out
}

pub fn camera_pivot_vertices(scale: f32) -> Vec<Vertex3D> {
    let mut verts = Vec::new();
    let t = 0.002 * scale; 
    let len = 0.4 * scale;
    
    // Red for X-axis
    let red = [0.9, 0.1, 0.1];
    add_box(Vec3::new(len * 0.5, 0.0, 0.0), Vec3::new(len, t, t), red, &mut verts);

    // Green for Y-axis
    let green = [0.1, 0.8, 0.1];
    add_box(Vec3::new(0.0, len * 0.5, 0.0), Vec3::new(t, len, t), green, &mut verts);

    // Blue for Z-axis
    let blue = [0.1, 0.1, 0.9];
    add_box(Vec3::new(0.0, 0.0, len * 0.5), Vec3::new(t, t, len), blue, &mut verts);

    verts
}

pub fn grid_vertices(thickness: f32, color: [f32; 3]) -> Vec<Vertex3D> {
    let range = 4.0;
    let step = 1.0;
    let mut geom = Geometry::new();

    let mut z = -range;
    while z <= range {
        let start = Vec3::new(-range, 0.0, z);
        let end = Vec3::new(range, 0.0, z);
        let mut line_geom = line_vertices(start, end, thickness);
        for v in &mut line_geom.vertices {
            v.col = color;
        }
        geom.merge(line_geom);
        z += step;
    }

    let mut x = -range;
    while x <= range {
        let start = Vec3::new(x, 0.0, -range);
        let end = Vec3::new(x, 0.0, range);
        let mut line_geom = line_vertices(start, end, thickness);
        for v in &mut line_geom.vertices {
            v.col = color;
        }
        geom.merge(line_geom);
        x += step;
    }

    geom.to_vertex3d_vec()
}

/// How many soup vertices a welded UV sphere fans out to: two pole bands of
/// triangles, `lat_steps - 2` bands of quads, three vertices per triangle.
///
/// Spelled out because it is no longer `lat_steps * lon_steps * 6` — the two
/// pole bands used to contribute a zero-area triangle each, and welded poles
/// do not.
/// Points in a welded UV sphere: the two poles plus `lat_steps - 1` rings.
///
/// The counterpart of [`sphere_soup_len`] on the other side of the weld, and
/// the number every test that used to say `lat * lon * 6` now wants.
#[cfg(test)]
pub(crate) const fn sphere_point_len(lat_steps: usize, lon_steps: usize) -> usize {
    2 + (lat_steps - 1) * lon_steps
}

#[cfg(test)]
pub(crate) const fn sphere_soup_len(lat_steps: usize, lon_steps: usize) -> usize {
    (2 + (lat_steps - 2) * 2) * lon_steps * 3
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The soup generator the welded sphere replaced, kept verbatim so the
    /// migration can be checked against it rather than against a remembered
    /// vertex count.
    #[cfg(test)]
    fn sphere_soup_before_welding(
        center: Vec3,
        radius: f32,
        lat_steps: usize,
        lon_steps: usize,
    ) -> Vec<[f32; 3]> {
        let mut v = Vec::new();
        for lat in 0..lat_steps {
            let theta0 = std::f32::consts::PI * lat as f32 / lat_steps as f32;
            let theta1 = std::f32::consts::PI * (lat + 1) as f32 / lat_steps as f32;
            for lon in 0..lon_steps {
                let phi0 = std::f32::consts::TAU * lon as f32 / lon_steps as f32;
                let phi1 = std::f32::consts::TAU * (lon + 1) as f32 / lon_steps as f32;
                let p00 = sphere_point(center, radius, theta0, phi0);
                let p10 = sphere_point(center, radius, theta1, phi0);
                let p11 = sphere_point(center, radius, theta1, phi1);
                let p01 = sphere_point(center, radius, theta0, phi1);
                for p in [p00, p10, p11, p00, p11, p01] {
                    v.push(p.to_array());
                }
            }
        }
        v
    }

    /// A deformer that grows a named attribute and displaces by it — the
    /// smallest kernel that needs the Phase 1 ABI.
    const ATTR_KERNEL: &str = r#"
        __kernel void process(__global float* pos, __global float* col, int count) {
            int id = get_global_id(0);
            if (id < count) {
                setattrf("mass", id, attrf("mass", id) + 2.0f);
                pos[id * 3 + 1] += attrf("mass", id);
            }
        }
    "#;

    #[test]
    fn test_kernel_names_its_attributes_and_they_become_buffers() {
        assert_eq!(parse_attr_refs(ATTR_KERNEL), vec!["mass".to_string()]);
        // `setattrf(` contains `attrf(`; the outer call must not be counted
        // twice, or the second buffer would shift every later binding.
        assert_eq!(parse_attr_refs(r#"setattrf("a", i, 1.0f);"#), vec!["a".to_string()]);

        let out = preprocess_opencl_code(ATTR_KERNEL);
        assert!(out.contains("__global float* attr_0"), "{out}");
        assert!(out.contains("attr_0[id] = (attr_0[id] + 2.0f)"), "{out}");
        assert!(!out.contains("attrf("), "every call is rewritten: {out}");
        // No ch* parameters here, so param_values is absent and the attribute
        // buffer takes the fourth slot. The launchers read arity to tell.
        assert!(!out.contains("param_values"), "{out}");
    }

    #[test]
    fn test_deformer_runs_over_points_and_keeps_the_geometry_whole() {
        let mut d = sphere_detail(Vec3::ZERO, 1.0, 6, 8);
        d.points_mut().create_group("keep");
        d.points_mut().add_to_group("keep", 3);
        let before_ids = d.ids().to_vec();
        let before_prims = d.num_prims();
        let before_y: Vec<f32> = d.positions().iter().map(|p| p[1]).collect();

        let code = preprocess_opencl_code(ATTR_KERNEL);
        let count = d.num_points();
        let mut pos: Vec<f32> = bytemuck::cast_slice(d.positions()).to_vec();
        let mut col: Vec<f32> = (0..count).flat_map(|p| d.color(p)).collect();
        let mut attrs = vec![("mass".to_string(), vec![0.0f32; count])];
        crate::kernel_cpu::run_deformer_cpu(&code, &mut pos, &mut col, count, &mut attrs, &[]).unwrap();

        // One work item per POINT, not per triangle corner: 42 points where the
        // soup would have handed the kernel 240 corners and then had to decide
        // which corner's answer a shared point takes.
        assert_eq!(count, 42);
        assert_eq!(attrs[0].1, vec![2.0f32; 42], "the kernel created and wrote the attribute");
        for p in 0..count {
            assert!((pos[p * 3 + 1] - (before_y[p] + 2.0)).abs() < 1e-5, "point {p}");
        }

        // Write back through the real entry point and check the geometry is
        // otherwise untouched — this is what the soup round trip could not do.
        for (p, slot) in d.positions_mut().iter_mut().enumerate() {
            *slot = [pos[p * 3], pos[p * 3 + 1], pos[p * 3 + 2]];
        }
        let _ = d.points_mut().insert("mass", AttribData::Float(attrs[0].1.clone()));
        assert_eq!(d.ids(), &before_ids[..], "identities survive a deformer");
        assert_eq!(d.num_prims(), before_prims, "topology survives a deformer");
        assert_eq!(d.points().group_members("keep"), vec![3], "groups survive a deformer");
        assert_eq!(d.points().value("mass", 0), Some(AttribValue::Float(2.0)));
    }

    #[test]
    fn test_attribute_abi_matches_across_both_backends() {
        if opencl3::platform::get_platforms().unwrap_or_default().is_empty() {
            println!("Skipping cross-backend attribute ABI test: no OpenCL platform");
            return;
        }
        let d = sphere_detail(Vec3::ZERO, 1.0, 6, 8);
        let code = preprocess_opencl_code(ATTR_KERNEL);
        let count = d.num_points();
        let base_pos: Vec<f32> = bytemuck::cast_slice(d.positions()).to_vec();
        let base_col: Vec<f32> = (0..count).flat_map(|p| d.color(p)).collect();

        let run = |gpu: bool| -> (Vec<f32>, Vec<f32>) {
            let (mut pos, mut col) = (base_pos.clone(), base_col.clone());
            let mut attrs = vec![("mass".to_string(), vec![0.5f32; count])];
            if gpu {
                run_deformer_flat(&code, &mut pos, &mut col, count, &mut attrs, &[]).unwrap();
            } else {
                crate::kernel_cpu::run_deformer_cpu(&code, &mut pos, &mut col, count, &mut attrs, &[])
                    .unwrap();
            }
            (pos, attrs.remove(0).1)
        };

        let (gpu_pos, gpu_mass) = run(true);
        let (cpu_pos, cpu_mass) = run(false);
        // The interpreter is the semantic reference, so the widened ABI has to
        // bind the same arguments to the same slots on both sides.
        assert_eq!(gpu_mass, cpu_mass);
        for (i, (a, b)) in gpu_pos.iter().zip(cpu_pos.iter()).enumerate() {
            assert!((a - b).abs() < 1e-5, "component {i}: {a} vs {b}");
        }
        assert_eq!(gpu_mass, vec![2.5f32; count], "an existing attribute is read, not reset");
    }

    #[test]
    fn test_welded_sphere_reproduces_the_soup_it_replaced() {
        let (center, radius, lat, lon) = (Vec3::new(0.1, 0.2, 0.3), 0.7, 16, 24);
        let before = sphere_soup_before_welding(center, radius, lat, lon);
        let after: Vec<[f32; 3]> = sphere_vertices_res(center, radius, lat, lon)
            .vertices
            .iter()
            .map(|v| v.pos)
            .collect();

        // Drop the triangles the old generator emitted with two corners in the
        // same place: the pole bands. They drew nothing, and a zero-area
        // triangle has no normal for a later remesh to use.
        let d = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let kept: Vec<[f32; 3]> = before
            .chunks_exact(3)
            .filter(|t| d(t[0], t[1]) > 1e-6 && d(t[1], t[2]) > 1e-6 && d(t[0], t[2]) > 1e-6)
            .flatten()
            .copied()
            .collect();

        assert_eq!(before.len(), lat * lon * 6);
        assert_eq!(kept.len(), sphere_soup_len(lat, lon), "only the pole bands go");
        assert_eq!(after.len(), kept.len());
        for (i, (a, b)) in after.iter().zip(kept.iter()).enumerate() {
            // Not bit-identical, and the difference is the point: the soup
            // computed its seam corner at phi = TAU and its south pole once per
            // longitude, so the surface had a ~1e-7 crack down it. The welded
            // sphere computes each of those places once.
            assert!(d(*a, *b) < 1e-5, "corner {i}: {a:?} vs {b:?}");
        }
    }

    #[test]
    fn test_welded_sphere_shares_its_poles_and_closes_its_seam() {
        let (lat, lon) = (16, 24);
        let d = sphere_detail(Vec3::ZERO, 1.0, lat, lon);

        // Two poles plus the interior rings — not lat*lon*6 loose corners.
        assert_eq!(d.num_points(), 2 + (lat - 1) * lon);
        assert_eq!(d.num_prims(), lat * lon, "one band row per latitude");

        // A pole is one point that every triangle of its band meets.
        assert_eq!(d.point_prims(0).len(), lon);
        assert_eq!(d.topology().valence(0), lon);

        // Every interior point has four neighbours, including the ones on the
        // seam — which is what "the seam closed" means. The soup could not
        // express this at all.
        let seam = 1; // first point of the first ring, at phi = 0
        assert_eq!(d.topology().valence(seam), 4);
        assert_eq!(
            d.point_prims(seam).len(),
            4,
            "two triangles of the pole band and two quads below it"
        );
        for p in 1 + lon..d.num_points() - 1 - lon {
            assert_eq!(d.topology().valence(p), 4, "interior point {p}");
        }
    }

    #[test]
    fn test_box_keeps_normals_on_vertices_because_its_edges_are_hard() {
        let d = box_detail(Vec3::ZERO, Vec3::Y, 0.02);
        assert_eq!(d.num_points(), 8, "a box has eight corners, not 36");
        assert_eq!(d.num_prims(), 6);
        assert_eq!(d.num_verts(), 24);

        // Three faces meet at every corner with three different normals, so
        // the normal cannot live on the point. This is what the vertex class
        // is for.
        assert!(d.verts().has("Norm"));
        assert!(!d.points().has("Norm"));
        assert_eq!(d.point_prims(0).len(), 3);
        assert_eq!(d.topology().valence(0), 3);

        let normals: Vec<[f32; 3]> = (0..d.num_verts())
            .filter_map(|v| match d.verts().value("Norm", v) {
                Some(crate::detail::AttribValue::Float3(n)) => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(normals.len(), 24);
        let corner_0_normals: Vec<[f32; 3]> = (0..d.num_prims())
            .filter(|&p| d.prim_points(p).contains(&0))
            .filter_map(|p| match d.verts().value("Norm", d.prim_verts(p).start) {
                Some(crate::detail::AttribValue::Float3(n)) => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(corner_0_normals.len(), 3);
        assert!(
            corner_0_normals[0] != corner_0_normals[1],
            "the faces meeting at a corner disagree: {corner_0_normals:?}"
        );

        // The soup adapter still hands every corner both attributes, which is
        // what the rest of the pipeline still reads.
        let soup = line_vertices(Vec3::ZERO, Vec3::Y, 0.02);
        assert_eq!(soup.vertices.len(), 36);
        assert!(soup
            .vertices
            .iter()
            .all(|v| v.attributes.contains_key("Norm") && v.attributes.contains_key("UV")));
    }

    #[test]
    fn test_curve_spans_stay_separate_pieces() {
        let node = FsNode {
            id: "c".into(),
            name: "Curve".into(),
            node_type: "curve".into(),
            children: vec![],
            params: [("Points", "0 0 0; 1 0 0"), ("Segments", "2"), ("Thickness", "0.02")]
                .into_iter()
                .map(|(name, default)| ParamDef {
                    name: name.to_string(),
                    label: String::new(),
                    param_type: "text".to_string(),
                    default: default.to_string(),
                    options: Vec::new(),
                    min: None,
                    max: None,
                    step: None,
                })
                .collect(),
            geometry_visible: true,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 1,
        };
        let d = curve_detail(&node);
        // Two sampled spans, one box each. Consecutive boxes touch but are not
        // welded — fusing the curve into one surface is a modeling decision the
        // node has never made.
        assert_eq!(d.num_prims(), 12, "two boxes of six faces");
        assert_eq!(d.num_points(), 16, "eight corners each, nothing shared");
        assert_eq!(detail_to_soup(&d).vertices.len(), 72);

        // Every point still has its own identity across the merge.
        let mut ids = d.ids().to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 16);
    }

    #[test]
    fn test_opencl_deformer_mode() {
        if opencl3::platform::get_platforms().unwrap_or_default().is_empty() {
            println!("Skipping OpenCL deformer test: No OpenCL platforms found");
            return;
        }

        let code = r#"
            __kernel void process(__global float* pos, __global float* col, int count) {
                int id = get_global_id(0);
                if (id < count) {
                    pos[id * 3 + 1] += 1.0f;
                }
            }
        "#;

        let mut geom = Geometry {
            vertices: vec![GVertex {
                pos: [1.0, 2.0, 3.0],
                col: [1.0, 0.0, 0.0],
                attributes: std::collections::HashMap::new(),
            }],
        };

        run_opencl_kernel(code, &mut geom).unwrap();

        assert_eq!(geom.vertices.len(), 1);
        assert_eq!(geom.vertices[0].pos, [1.0, 3.0, 3.0]);
    }

    #[test]
    fn test_opencl_generator_mode() {
        if opencl3::platform::get_platforms().unwrap_or_default().is_empty() {
            println!("Skipping OpenCL generator test: No OpenCL platforms found");
            return;
        }

        let code = r#"
            __kernel void process(
                __global const float* in_pos,
                __global const float* in_col,
                int in_count,
                __global float* out_pos,
                __global float* out_col,
                __global int* out_count,
                int max_out_count
            ) {
                int id = get_global_id(0);
                if (id < in_count) {
                    // Copy original
                    int idx1 = atomic_inc(out_count);
                    if (idx1 < max_out_count) {
                        out_pos[idx1 * 3] = in_pos[id * 3];
                        out_pos[idx1 * 3 + 1] = in_pos[id * 3 + 1];
                        out_pos[idx1 * 3 + 2] = in_pos[id * 3 + 2];
                        out_col[idx1 * 3] = in_col[id * 3];
                        out_col[idx1 * 3 + 1] = in_col[id * 3 + 1];
                        out_col[idx1 * 3 + 2] = in_col[id * 3 + 2];
                    }
                    // Generate new offset
                    int idx2 = atomic_inc(out_count);
                    if (idx2 < max_out_count) {
                        out_pos[idx2 * 3] = in_pos[id * 3] + 1.0f;
                        out_pos[idx2 * 3 + 1] = in_pos[id * 3 + 1] + 2.0f;
                        out_pos[idx2 * 3 + 2] = in_pos[id * 3 + 2] + 3.0f;
                        out_col[idx2 * 3] = 0.5f;
                        out_col[idx2 * 3 + 1] = 0.5f;
                        out_col[idx2 * 3 + 2] = 0.5f;
                    }
                }
            }
        "#;

        let mut geom = Geometry {
            vertices: vec![GVertex {
                pos: [1.0, 2.0, 3.0],
                col: [1.0, 0.0, 0.0],
                attributes: std::collections::HashMap::new(),
            }],
        };

        run_opencl_kernel(code, &mut geom).unwrap();

        assert_eq!(geom.vertices.len(), 2);
        assert_eq!(geom.vertices[0].pos, [1.0, 2.0, 3.0]);
        assert_eq!(geom.vertices[1].pos, [2.0, 4.0, 6.0]);
        assert_eq!(geom.vertices[1].col, [0.5, 0.5, 0.5]);
    }

    #[test]
    fn test_points_node_shapes() {
        let points_node = |shape: &str| FsNode {
            id: format!("id Points {shape} test"),
            inputs: 1,
            outputs: 1,
            name: "Points test".to_string(),
            node_type: "points".to_string(),
            children: vec![],
            params: vec![
                ParamDef {
                    name: "Points".to_string(),
                    label: String::new(),
                    param_type: "spinbox".to_string(),
                    default: "5".to_string(),
                    options: vec![],
                    min: Some(1.0),
                    max: Some(10.0),
                    step: Some(1.0),
                },
                ParamDef {
                    name: "Shape".to_string(),
                    label: String::new(),
                    param_type: "choice:None,Spiral,Line,Circle,Grid".to_string(),
                    default: shape.to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                },
            ],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let root = FsNode {
            id: "id root".to_string(),
            inputs: 1,
            outputs: 1,
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![points_node("None")],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let geom = network_sphere_vertices(&root);
        assert_eq!(geom.num_points(), 5 * super::sphere_point_len(6, 8));

        // Shape "None": every point sits in the same spot, so all five marker
        // spheres cover an identical (tiny) extent. A spread shape must not.
        let extent = |g: &Geometry| {
            let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in &g.vertices {
                min = min.min(Vec3::from_array(v.pos));
                max = max.max(Vec3::from_array(v.pos));
            }
            max - min
        };
        let none = points_node_geometry(&points_node("None"), Vec3::ZERO);
        let e = extent(&none);
        assert!(e.length() < 0.1, "None must collapse to one spot, extent {e:?}");

        for shape in ["Spiral", "Line", "Circle", "Grid"] {
            let g = points_node_geometry(&points_node(shape), Vec3::ZERO);
            assert_eq!(g.vertices.len(), 5 * super::sphere_soup_len(6, 8), "{shape}");
            assert!(
                extent(&g).length() > 0.3,
                "{shape} must spread its points, extent {:?}",
                extent(&g)
            );
        }
    }

    #[test]
    fn test_transform_node() {
        let sphere = FsNode {
            id: "id Sphere 1".to_string(),
            inputs: 1,
            outputs: 1,
            name: "Sphere 1".to_string(),
            node_type: "sphere".to_string(),
            children: vec![],
            params: vec![
                ParamDef {
                    name: "Radius".to_string(),
                    label: String::new(),
                    param_type: "slider".to_string(),
                    default: "0.5".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                }
            ],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let transform1 = FsNode {
            id: "id Transform 1".to_string(),
            inputs: 1,
            outputs: 1,
            name: "Transform 1".to_string(),
            node_type: "transform".to_string(),
            children: vec![],
            params: vec![
                ParamDef {
                    name: "Input".to_string(),
                    label: String::new(),
                    param_type: "text".to_string(),
                    default: "Sphere 1".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                },
                ParamDef {
                    name: "Translation".to_string(),
                    label: String::new(),
                    param_type: "float3".to_string(),
                    default: "1.00:2.00:3.00".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                }
            ],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let root = FsNode {
            id: "id root".to_string(),
            inputs: 1,
            outputs: 1,
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere.clone(), transform1.clone()],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };

        // Test normal transform
        let mut visited = Vec::new();
        let geom1 = resolve_transform_geometry(&root, &transform1, &mut visited).unwrap();
        assert!(!geom1.is_empty());
        let avg_x = geom1.positions().iter().map(|p| p[0]).sum::<f32>() / geom1.num_points() as f32;
        let avg_y = geom1.positions().iter().map(|p| p[1]).sum::<f32>() / geom1.num_points() as f32;
        let avg_z = geom1.positions().iter().map(|p| p[2]).sum::<f32>() / geom1.num_points() as f32;
        assert!((avg_x - -0.875).abs() < 0.01);
        assert!((avg_y - 2.55).abs() < 0.01);
        assert!((avg_z - 3.0).abs() < 0.01);

        // Test chained transform
        let transform2 = FsNode {
            id: "id Transform 2".to_string(),
            inputs: 1,
            outputs: 1,
            name: "Transform 2".to_string(),
            node_type: "transform".to_string(),
            children: vec![],
            params: vec![
                ParamDef {
                    name: "Input".to_string(),
                    label: String::new(),
                    param_type: "text".to_string(),
                    default: "Transform 1".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                },
                ParamDef {
                    name: "Translation".to_string(),
                    label: String::new(),
                    param_type: "float3".to_string(),
                    default: "-1.00:-1.00:-1.00".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                }
            ],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let root_chained = FsNode {
            id: "id root".to_string(),
            inputs: 1,
            outputs: 1,
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere, transform1, transform2.clone()],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let mut visited = Vec::new();
        let geom2 = resolve_transform_geometry(&root_chained, &transform2, &mut visited).unwrap();
        let avg_chained_x = geom2.positions().iter().map(|p| p[0]).sum::<f32>() / geom2.num_points() as f32;
        let avg_chained_y = geom2.positions().iter().map(|p| p[1]).sum::<f32>() / geom2.num_points() as f32;
        let avg_chained_z = geom2.positions().iter().map(|p| p[2]).sum::<f32>() / geom2.num_points() as f32;
        assert!((avg_chained_x - -1.875).abs() < 0.01);
        assert!((avg_chained_y - 1.55).abs() < 0.01);
        assert!((avg_chained_z - 2.0).abs() < 0.01);

        // Test loop detection
        let transform_loop = FsNode {
            id: "id Transform Loop".to_string(),
            inputs: 1,
            outputs: 1,
            name: "Transform Loop".to_string(),
            node_type: "transform".to_string(),
            children: vec![],
            params: vec![
                ParamDef {
                    name: "Input".to_string(),
                    label: String::new(),
                    param_type: "text".to_string(),
                    default: "Transform Loop".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                },
                ParamDef {
                    name: "Translation".to_string(),
                    label: String::new(),
                    param_type: "float3".to_string(),
                    default: "1.00:1.00:1.00".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                }
            ],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let root_loop = FsNode {
            id: "id root".to_string(),
            inputs: 1,
            outputs: 1,
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![transform_loop.clone()],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let mut visited = Vec::new();
        let geom_loop = resolve_transform_geometry(&root_loop, &transform_loop, &mut visited);
        assert!(geom_loop.is_none());
    }

    #[test]
    fn test_opencl_local_node() {
        if opencl3::platform::get_platforms().unwrap_or_default().is_empty() {
            println!("Skipping OpenCL local node test: No OpenCL platforms found");
            return;
        }

        let sphere = FsNode {
            id: "id Sphere 1".to_string(),
            inputs: 1,
            outputs: 1,
            name: "Sphere 1".to_string(),
            node_type: "sphere".to_string(),
            children: vec![],
            params: vec![
                ParamDef {
                    name: "Radius".to_string(),
                    label: String::new(),
                    param_type: "slider".to_string(),
                    default: "0.5".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                }
            ],
            geometry_visible: true,
            position: (0.0, 0.0),
        };

        let opencl_node = FsNode {
            id: "id OpenCL 1".to_string(),
            inputs: 1,
            outputs: 1,
            name: "OpenCL 1".to_string(),
            node_type: "opencl".to_string(),
            children: vec![],
            params: vec![
                ParamDef {
                    name: "Input".to_string(),
                    label: String::new(),
                    param_type: "text".to_string(),
                    default: "Sphere 1".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                },
                ParamDef {
                    name: "Code".to_string(),
                    label: String::new(),
                    param_type: "code".to_string(),
                    default: r#"
                        __kernel void process(__global float* pos, __global float* col, int count) {
                            int id = get_global_id(0);
                            if (id < count) {
                                pos[id * 3 + 1] += 2.0f;
                            }
                        }
                    "#.to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                }
            ],
            geometry_visible: true,
            position: (0.0, 0.0),
        };

        let root = FsNode {
            id: "id root".to_string(),
            inputs: 1,
            outputs: 1,
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere, opencl_node.clone()],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };

        let mut visited = Vec::new();
        let mut err = None;
        let geom = resolve_opencl_geometry_with_errors(&root, &opencl_node, &mut visited, &mut err, &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default())).unwrap();
        assert!(!geom.is_empty());
        assert!(err.is_none());

        // The sphere should be translated up by 2.0 on the y axis compared to the standard sphere (which centers around y=0.55 for index 0)
        let avg_y = geom.positions().iter().map(|p| p[1]).sum::<f32>() / geom.num_points() as f32;
        assert!((avg_y - 2.55).abs() < 0.01);
    }

    #[test]
    fn test_scatter_node() {
        let sphere = FsNode {
            id: "sphere1".to_string(),
            inputs: 1,
            outputs: 1,
            name: "Sphere 1".to_string(),
            node_type: "sphere".to_string(),
            children: vec![],
            params: vec![
                ParamDef {
                    name: "Radius".to_string(),
                    label: String::new(),
                    param_type: "slider".to_string(),
                    default: "0.5".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                }
            ],
            geometry_visible: true,
            position: (0.0, 0.0),
        };

        let scatter = FsNode {
            id: "scatter1".to_string(),
            inputs: 1,
            outputs: 1,
            name: "Scatter 1".to_string(),
            node_type: "scatter".to_string(),
            children: vec![],
            params: vec![
                ParamDef {
                    name: "Input".to_string(),
                    label: String::new(),
                    param_type: "text".to_string(),
                    default: "Sphere 1".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                },
                ParamDef {
                    name: "Points".to_string(),
                    label: String::new(),
                    param_type: "spinbox".to_string(),
                    default: "15".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                },
                ParamDef {
                    name: "Radius".to_string(),
                    label: String::new(),
                    param_type: "slider".to_string(),
                    default: "0.02".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                }
            ],
            geometry_visible: true,
            position: (0.0, 0.0),
        };

        let root = FsNode {
            id: "id root".to_string(),
            inputs: 1,
            outputs: 1,
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere, scatter.clone()],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };

        let mut visited = Vec::new();
        let geom = resolve_scatter_geometry(&root, &scatter, &mut visited).unwrap();

        // 15 scattered spheres, each a lat_steps=6, lon_steps=8 marker.
        assert_eq!(geom.num_points(), 15 * super::sphere_point_len(6, 8));

        // Center of sphere at idx 0 is Vec3::new(-1.875, 0.55, 0.0). Radius = 0.5.
        // Let's check that each scattered sphere's center is indeed inside the parent sphere.
        let center = Vec3::new(-1.875, 0.55, 0.0);
        // Each marker is a lat=6, lon=8 sphere: two poles plus five rings of
        // eight, and merge lays them down one after another.
        const MARKER_POINTS: usize = 2 + (6 - 1) * 8;
        for chunk in geom.positions().chunks_exact(MARKER_POINTS) {
            let mut sum = Vec3::ZERO;
            for p in chunk {
                sum += Vec3::from_array(*p);
            }
            let avg = sum / MARKER_POINTS as f32;
            let dist = avg.distance(center);
            assert!(dist <= 0.5, "Scattered point center {:?} (distance {}) is outside the sphere of radius 0.5", avg, dist);
        }
    }
}



/// Solve a simnet up to the frame being evaluated.
///
/// The chain between the simnet's `input` and `output` children is one step of
/// the simulation. Step 1 consumes the seed (the simnet's own `Input`, exactly
/// like a subnet's); every later step consumes the step before it, which the
/// `input` node picks up from the feedback stack. At the timeline's start frame
/// the sim has taken no steps and IS the seed.
pub fn resolve_simnet_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let output_node = target
        .children
        .iter()
        .find(|c| c.node_type.eq_ignore_ascii_case("output"))?
        .clone();

    let seed = {
        let input_name = node_param_str(target, "Input", "");
        if input_name.is_empty() {
            Detail::new()
        } else {
            find_node_by_name(root, &input_name)
                .and_then(|n| generate_single_node_geometry_with_errors(root, n, visited, ocl_error, sim))
                .unwrap_or_default()
        }
    };

    let key = sim_solve_key(target, &seed);
    let due = sim.steps_due();

    // Resume from the cached solve when it is still valid and has not run PAST
    // the frame asked for; scrubbing backwards has to restart from the seed,
    // because a step is not invertible.
    let (mut state, mut done) = match sim.cache.entries.get(&target.id) {
        Some(prev) if prev.key == key && prev.frame <= due => (prev.state.clone(), prev.frame),
        _ => (seed, 0),
    };

    while done < due {
        sim.feedback.push((target.id.clone(), state));
        let stepped = generate_single_node_geometry_with_errors(root, &output_node, visited, ocl_error, sim);
        let fed_back = sim.feedback.pop().map(|(_, g)| g);
        // A step that yields nothing (an unwired chain, a failed kernel) holds
        // the previous state rather than collapsing the sim to empty geometry.
        state = stepped.or(fed_back).unwrap_or_default();
        done += 1;
    }

    sim.cache.entries.insert(
        target.id.clone(),
        SimSolve { key, frame: due, state: state.clone() },
    );
    Some(state)
}

/// Does this graph contain a simnet anywhere? The frame-change invalidation asks
/// before rebuilding the scene, since for a graph without one the timeline
/// changes nothing.
pub fn contains_simnet(root: &FsNode) -> bool {
    if root.node_type.eq_ignore_ascii_case("simnet") {
        return true;
    }
    root.children.iter().any(contains_simnet)
}

#[cfg(test)]
mod simnet_tests {
    use super::*;
    use crate::app::{FsNode, ParamDef};

    fn param(name: &str, value: &str) -> ParamDef {
        ParamDef {
            name: name.to_string(),
            label: String::new(),
            param_type: "text".to_string(),
            default: value.to_string(),
            options: vec![],
            min: None,
            max: None,
            step: None,
        }
    }

    fn node(id: &str, name: &str, node_type: &str, params: Vec<ParamDef>, children: Vec<FsNode>) -> FsNode {
        FsNode {
            id: id.to_string(),
            inputs: 1,
            outputs: 1,
            name: name.to_string(),
            node_type: node_type.to_string(),
            children,
            params,
            geometry_visible: true,
            position: (0.0, 0.0),
        }
    }

    /// The scene walk draws the level it is STARTED at — the network editor's
    /// current directory — while evaluation stays rooted at the tree root.
    /// From the root a subnet's internals draw (recursion); started at the
    /// subnet, only its own children do.
    #[test]
    fn test_scene_walk_scoped_to_start_level() {
        let outer = node("id-outer", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let inner = node("id-inner", "Sphere 2", "sphere", vec![param("Radius", "0.5")], vec![]);
        let sub = node("id-sub", "Sub 1", "node", vec![], vec![inner]);
        let root = node("id-root", "root", "node", vec![], vec![outer, sub]);

        const SPHERE: usize = super::sphere_point_len(16, 24);
        let mut err = None;
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(0, 0, &mut cache);
        let all = network_sphere_vertices_with_errors(&root, &root, &mut err, &mut sim);
        assert_eq!(all.num_points(), 2 * SPHERE);

        let mut err = None;
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(0, 0, &mut cache);
        let scoped =
            network_sphere_vertices_with_errors(&root, &root.children[1], &mut err, &mut sim);
        assert_eq!(scoped.num_points(), SPHERE);
    }

    /// A simnet whose chain is one Transform: each step shifts the geometry by
    /// the same offset, so the solved position reads back the step COUNT. Uses
    /// transform, not an OpenCL node, so the test is pure CPU.
    fn stepping_graph() -> FsNode {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let inner_input = node("id-in", "input1", "input", vec![], vec![]);
        let step = node(
            "id-step",
            "step1",
            "transform",
            vec![param("Input", "input1"), param("Translation", "1.00:0.00:0.00")],
            vec![],
        );
        let inner_output = node("id-out", "output1", "output", vec![param("Input", "step1")], vec![]);
        let sim = node(
            "id-sim",
            "Simnet 1",
            "simnet",
            vec![param("Input", "Sphere 1")],
            vec![inner_input, step, inner_output],
        );
        node("id-root", "root", "node", vec![], vec![sphere, sim])
    }

    fn solve_at(root: &FsNode, frame: i32) -> Detail {
        let sim_node = root.children.iter().find(|c| c.node_type == "simnet").unwrap();
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(frame, 1, &mut cache);
        let mut visited = Vec::new();
        let mut err = None;
        resolve_simnet_geometry_with_errors(root, sim_node, &mut visited, &mut err, &mut sim)
            .expect("simnet solves")
    }

    fn min_x(g: &Detail) -> f32 {
        g.positions().iter().map(|p| p[0]).fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn test_simnet_at_start_frame_is_its_seed() {
        let root = stepping_graph();
        let seeded = solve_at(&root, 1);
        let mut visited = Vec::new();
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(1, 1, &mut cache);
        let mut err = None;
        let raw = generate_single_node_geometry_with_errors(
            &root,
            root.children.iter().find(|c| c.name == "Sphere 1").unwrap(),
            &mut visited,
            &mut err,
            &mut sim,
        )
        .expect("seed geometry");

        assert!(!seeded.is_empty(), "the sim produced nothing at its start frame");
        assert_eq!(seeded.num_points(), raw.num_points());
        assert!((min_x(&seeded) - min_x(&raw)).abs() < 1e-4,
            "at the start frame the sim has taken no steps, so it must BE the seed");
    }

    /// The point of the whole thing: frame N is N applications of the chain, not
    /// one. A simnet that resolved its input to the outer graph every time would
    /// sit at one step forever.
    #[test]
    fn test_simnet_iterates_once_per_frame() {
        let root = stepping_graph();
        let base = min_x(&solve_at(&root, 1));
        for steps in 1..=4 {
            let solved = solve_at(&root, 1 + steps);
            let moved = min_x(&solved) - base;
            assert!((moved - steps as f32).abs() < 1e-4,
                "frame {} should be {} steps of +1.0, got {moved}", 1 + steps, steps);
        }
    }

    /// Scrubbing before the start frame is not negative time.
    #[test]
    fn test_simnet_before_the_start_frame_holds_its_seed() {
        let root = stepping_graph();
        let base = min_x(&solve_at(&root, 1));
        assert!((min_x(&solve_at(&root, -20)) - base).abs() < 1e-4);
    }

    /// Resuming from the cache must land on the same answer as solving cold, or
    /// playback and scrubbing would disagree about the same frame.
    #[test]
    fn test_simnet_cache_resume_matches_a_cold_solve() {
        let root = stepping_graph();
        let sim_node = root.children.iter().find(|c| c.node_type == "simnet").unwrap();
        let mut cache = SimCache::default();

        // Step forward frame by frame through the shared cache.
        let mut warm = 0.0;
        for frame in 1..=6 {
            let mut sim = EvalSim::new(frame, 1, &mut cache);
            let mut visited = Vec::new();
            let mut err = None;
            let g = resolve_simnet_geometry_with_errors(&root, sim_node, &mut visited, &mut err, &mut sim).unwrap();
            warm = min_x(&g);
        }
        let cold = min_x(&solve_at(&root, 6));
        assert!((warm - cold).abs() < 1e-4, "resumed solve {warm} != cold solve {cold}");
    }

    /// Editing the chain has to restart the sim: a cached state solved from the
    /// old chain is not a state of the new one.
    #[test]
    fn test_editing_the_chain_invalidates_the_cache() {
        let mut root = stepping_graph();
        let sim_node = root.children.iter().find(|c| c.node_type == "simnet").unwrap().clone();
        let mut cache = SimCache::default();
        {
            let mut sim = EvalSim::new(5, 1, &mut cache);
            let mut visited = Vec::new();
            let mut err = None;
            resolve_simnet_geometry_with_errors(&root, &sim_node, &mut visited, &mut err, &mut sim).unwrap();
        }

        // Double the step size; frame 5 (4 steps) must now read 8, not 4.
        {
            let sim_mut = root.children.iter_mut().find(|c| c.node_type == "simnet").unwrap();
            let step = sim_mut.children.iter_mut().find(|c| c.name == "step1").unwrap();
            step.params.iter_mut().find(|p| p.name == "Translation").unwrap().default =
                "2.00:0.00:0.00".to_string();
        }
        let sim_node = root.children.iter().find(|c| c.node_type == "simnet").unwrap().clone();
        let base = min_x(&solve_at(&root, 1));
        let mut sim = EvalSim::new(5, 1, &mut cache);
        let mut visited = Vec::new();
        let mut err = None;
        let g = resolve_simnet_geometry_with_errors(&root, &sim_node, &mut visited, &mut err, &mut sim).unwrap();
        let moved = min_x(&g) - base;
        assert!((moved - 8.0).abs() < 1e-4,
            "stale cache: expected 4 steps of +2.0 = 8, got {moved}");
    }

    /// Dived INTO a simnet the walk draws the solved state — its children are
    /// the step chain, which has no draw arms (a fresh simnet is only
    /// input/output), so the interior used to render an empty viewport even
    /// though the sim resolved fine from the parent level.
    #[test]
    fn test_scene_walk_inside_a_simnet_draws_the_solved_state() {
        let root = stepping_graph();
        let sim_node = root.children.iter().find(|c| c.node_type == "simnet").unwrap();
        let base = min_x(&solve_at(&root, 1));

        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(4, 1, &mut cache);
        let mut err = None;
        let interior = network_sphere_vertices_with_errors(&root, sim_node, &mut err, &mut sim);
        assert!(!interior.is_empty(), "simnet interior rendered empty");
        let moved = min_x(&interior) - base;
        assert!((moved - 3.0).abs() < 1e-4, "frame 4 = 3 steps of +1.0, got {moved}");

        // The output child's geometry toggle is the interior display switch.
        let mut hidden = root.clone();
        hidden.children.iter_mut().find(|c| c.node_type == "simnet").unwrap()
            .children.iter_mut().find(|c| c.node_type == "output").unwrap()
            .geometry_visible = false;
        let sim_node = hidden.children.iter().find(|c| c.node_type == "simnet").unwrap();
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(4, 1, &mut cache);
        let mut err = None;
        let toggled = network_sphere_vertices_with_errors(&hidden, sim_node, &mut err, &mut sim);
        assert!(toggled.is_empty(), "output toggle off should hide the solved state");
    }

    /// Dived into a pass-through subnet (input → output, no generator) the
    /// interior draws the resolved chain instead of nothing. Top level only:
    /// from the root the subnet's internals draw exactly as before, so the
    /// arms add no second copy to outer views.
    #[test]
    fn test_scene_walk_draws_passthrough_subnet_interior() {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let inner_input = node("id-in", "input1", "input", vec![], vec![]);
        let inner_output = node("id-out", "output1", "output", vec![param("Input", "input1")], vec![]);
        let sub = node("id-sub", "Subnet 1", "node", vec![param("Input", "Sphere 1")],
            vec![inner_input, inner_output]);
        let root = node("id-root", "root", "node", vec![], vec![sphere, sub]);

        const SPHERE: usize = super::sphere_point_len(16, 24);
        let mut err = None;
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(0, 0, &mut cache);
        let all = network_sphere_vertices_with_errors(&root, &root, &mut err, &mut sim);
        assert_eq!(all.num_points(), SPHERE, "outer view must not gain a copy from the arms");

        let mut err = None;
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(0, 0, &mut cache);
        let interior =
            network_sphere_vertices_with_errors(&root, &root.children[1], &mut err, &mut sim);
        assert_eq!(interior.num_points(), 2 * SPHERE,
            "input draws the seed and output draws the chain result");
    }

    fn eval(root: &FsNode, name: &str) -> Detail {
        let target = root.children.iter().find(|c| c.name == name).unwrap();
        let mut visited = Vec::new();
        let mut err = None;
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(0, 0, &mut cache);
        generate_single_node_geometry_with_errors(root, target, &mut visited, &mut err, &mut sim)
            .expect("node evaluates")
    }

    /// Random point groups draw from WELDED points: one random point tags
    /// every coincident vertex copy, deterministically from Seed.
    #[test]
    fn test_group_random_point_is_welded_and_deterministic() {
        let make_root = |seed: &str| {
            let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
            let group = node(
                "id-group",
                "Group 1",
                "group",
                vec![
                    param("Input", "Sphere 1"),
                    param("Group Name", "pull"),
                    param("Mode", "Random"),
                    param("Count", "1"),
                    param("Seed", seed),
                    param("Highlight", "false"),
                ],
                vec![],
            );
            node("id-root", "root", "node", vec![], vec![sphere, group])
        };

        let g = eval(&make_root("7"), "Group 1");
        let tagged = g.points().group_members("pull");
        // Count = 1 means ONE point. The soup version had to assert this the
        // long way round — that every coincident copy of the drawn position was
        // tagged too, or a downstream move would tear the surface open. There
        // are no copies to miss now.
        assert_eq!(tagged.len(), 1, "Count = 1 must select exactly one point");

        let again = eval(&make_root("7"), "Group 1");
        assert_eq!(
            tagged,
            again.points().group_members("pull"),
            "same Seed must select the same point"
        );
        let other = eval(&make_root("12"), "Group 1");
        assert_eq!(other.points().group_members("pull").len(), 1);
    }

    /// The Collision node's Inside method marks exactly the input points
    /// enclosed by the collider's volume — the group written where two
    /// spheres overlap, absent everywhere clearly outside — and an
    /// unconfigured Collider passes the input through untouched.
    #[test]
    fn test_collision_inside_marks_enclosed_points() {
        let build = |collider: &str, method: &str| {
            let s1 = node("id-s1", "Sphere 1", "sphere", vec![param("Radius", "0.7")], vec![]);
            let s2 = node("id-s2", "Sphere 2", "sphere", vec![param("Radius", "0.7")], vec![]);
            let col = node(
                "id-col",
                "Collision 1",
                "collision",
                vec![
                    param("Input", "Sphere 1"),
                    param("Collider", collider),
                    param("Method", method),
                    param("Group Name", "collisions"),
                    param("Highlight", "false"),
                ],
                vec![],
            );
            node("id-root", "root", "node", vec![], vec![s1, s2, col])
        };

        // The collider's center, measured: the tessellation is
        // center-symmetric, so the vertex mean is the center.
        let root = build("Sphere 2", "Inside");
        let s2_geom = eval(&root, "Sphere 2");
        let n2 = s2_geom.num_points() as f32;
        let mut c2 = [0.0f32; 3];
        for p in s2_geom.positions() {
            for k in 0..3 {
                c2[k] += p[k] / n2;
            }
        }

        let g = eval(&root, "Collision 1");
        let dist = |p: [f32; 3]| {
            ((p[0] - c2[0]).powi(2) + (p[1] - c2[1]).powi(2) + (p[2] - c2[2]).powi(2)).sqrt()
        };
        let mut tagged = 0usize;
        for (p, pos) in g.positions().iter().enumerate() {
            let has = g.points().in_group("collisions", p);
            if dist(*pos) < 0.7 - 1e-3 {
                assert!(has, "enclosed point untagged at {:?}", pos);
                tagged += 1;
            } else if dist(*pos) > 0.7 + 1e-2 {
                assert!(!has, "outside point tagged at {:?}", pos);
            }
        }
        assert!(tagged > 0, "overlapping spheres must tag the overlap cap");
        assert!(tagged < g.num_points(), "only the cap is enclosed, not the whole sphere");

        // Proximity is a SURFACE band, not containment: every tagged point
        // sits within Distance of the collider's surface, and with the two
        // spheres interpenetrating the band is non-empty.
        let prox = eval(&build("Sphere 2", "Proximity"), "Collision 1");
        let mut band = 0usize;
        for p in prox.points().group_members("collisions") {
            let pos = prox.positions()[p as usize];
            assert!(
                (dist(pos) - 0.7).abs() <= 0.05 + 1e-2,
                "proximity tag outside the band at {:?}",
                pos
            );
            band += 1;
        }
        assert!(band > 0, "a 0.05 band around an intersecting surface must catch boundary points");

        // No collider configured: pass-through, nothing tagged.
        let clean = eval(&build("", "Inside"), "Collision 1");
        assert!(
            clean.points().group_members("collisions").is_empty(),
            "an unconfigured collider must not write the group"
        );
    }

    /// The tissue chain: pull one random point with an Attribute Pos edit,
    /// then Relax against the pre-pull shape with the point pinned — the
    /// point keeps its pulled position, neighbors follow part of the way.
    #[test]
    fn test_relax_spreads_a_pinned_pull() {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let group = node(
            "id-group",
            "Group 1",
            "group",
            vec![
                param("Input", "Sphere 1"),
                param("Group Name", "pull"),
                param("Mode", "Random"),
                param("Count", "1"),
                param("Seed", "3"),
                param("Highlight", "false"),
            ],
            vec![],
        );
        let pull = node(
            "id-pull",
            "Pull 1",
            "attribute",
            vec![
                param("Input", "Group 1"),
                param("Operation", "Modify"),
                param("Attribute Name", "Pos"),
                param("Value", "0.00:0.50:0.00"),
                param("Combine", "Add"),
                param("Group", "pull"),
            ],
            vec![],
        );
        let relax = node(
            "id-relax",
            "Relax 1",
            "relax",
            vec![
                param("Input", "Pull 1"),
                param("Rest", "Group 1"),
                param("Pin Group", "pull"),
                param("Stiffness", "0.50"),
                param("Iterations", "8"),
            ],
            vec![],
        );
        let root = node("id-root", "root", "node", vec![], vec![sphere, group, pull, relax]);

        let base = eval(&root, "Group 1");
        let pulled = eval(&root, "Pull 1");
        let relaxed = eval(&root, "Relax 1");
        assert_eq!(relaxed.num_points(), base.num_points());

        let dist = |a: [f32; 3], b: [f32; 3]| -> f32 {
            a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
        };
        let mut max_response: f32 = 0.0;
        for i in 0..base.num_points() {
            let moved = dist(relaxed.positions()[i], base.positions()[i]);
            if base.points().in_group("pull", i) {
                assert!(dist(relaxed.positions()[i], pulled.positions()[i]) < 1e-4,
                    "the pinned point must keep its pulled position");
            } else {
                assert!(moved <= 0.5 + 1e-3, "a neighbor overshot the pull itself");
                max_response = max_response.max(moved);
            }
        }
        assert!(max_response > 0.01,
            "no neighbor responded to the pull (relax did nothing), max {max_response}");
    }
}
