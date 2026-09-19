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

    // Wound counter-clockwise seen from OUTSIDE, so the plain
    // cross(B-A, C-A) points away from the surface. That is the raster
    // culling convention, what the template meshes do, and what
    // `point_normals` — and therefore Develop, the normal overlay and
    // Align's surface tangent — all assume.
    //
    // The soup this replaced wound the other way, and had done since it was
    // written: every normal on a native sphere pointed INTO it. Nothing
    // caught it because the winding test covers the template meshes, the
    // overlay test uses a template sphere, and a path tracer shades both
    // sides of a triangle. Develop is the first operator whose answer depends
    // on it, and it grew the surface inward.
    let wrap = |lon: usize| (lon + 1) % lon_steps;
    for lon in 0..lon_steps {
        d.add_prim(&[north, rings[0][wrap(lon)], rings[0][lon]]);
    }
    // Each quad is the soup's quad reversed but ANCHORED on the same corner,
    // so it fans into the same two triangles rather than across the other
    // diagonal. The surface is identical; only the facing changed.
    for lat in 1..lat_steps - 1 {
        let (a, b) = (&rings[lat - 1], &rings[lat]);
        for lon in 0..lon_steps {
            d.add_prim(&[a[lon], a[wrap(lon)], b[wrap(lon)], b[lon]]);
        }
    }
    let last = &rings[lat_steps - 2];
    for lon in 0..lon_steps {
        d.add_prim(&[last[lon], last[wrap(lon)], south]);
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

    // Wound counter-clockwise seen from outside, like the sphere and the
    // template meshes. The soup's order was the reverse, which meant every
    // face of a native box wound against the `Norm` attribute the same
    // function attached to it — the geometry and its own normal disagreed.
    let faces: [([u32; 4], Vec3); 6] = [
        ([3, 2, 1, 0], -dir), // start cap
        ([6, 7, 4, 5], dir),  // end cap
        ([7, 3, 0, 4], -u),   // left
        ([2, 6, 5, 1], u),    // right
        ([7, 6, 2, 3], v),    // top
        ([1, 5, 4, 0], -v),   // bottom
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
    } else if target.node_type.eq_ignore_ascii_case("neighbour") {
        resolve_neighbour_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("time") {
        resolve_time_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("detangle") {
        resolve_detangle_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("suture") {
        resolve_suture_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("remesh") {
        resolve_remesh_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("develop") {
        resolve_develop_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("visualize") {
        resolve_visualize_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("analysis") {
        resolve_analysis_geometry_with_errors(root, target, visited, ocl_error, sim)
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

/// The Detangle node: push a surface off itself.
///
/// Growth folds a surface into its own neighbourhood long before it looks
/// wrong from outside, and a diffusion across a self-intersecting mesh reads
/// neighbours that are topologically far away. This resolves it the way the
/// plugin's Detangle does: a point repulsion over Iterations passes, with
/// Thickness measured in edge lengths and points within Rings of each other
/// excluded.
///
/// The ring exclusion is the whole trick. Every point is within a thickness of
/// its own neighbours by construction — that is what an edge is — so a naive
/// repulsion would blow the mesh apart from the inside. Excluding the
/// topological neighbourhood leaves exactly the pairs that are near in SPACE
/// but far across the SURFACE, which is what a self-intersection is.
pub fn resolve_detangle_geometry_with_errors(
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
    apply_detangle(&mut geom, target);
    Some(geom)
}

pub(crate) fn apply_detangle(geom: &mut Detail, target: &FsNode) {
    let n = geom.num_points();
    if n == 0 || geom.num_prims() == 0 {
        return;
    }
    // Thickness in EDGE LENGTHS, so the setting means the same thing before
    // and after a remesh — an absolute distance would stop separating the
    // moment the mesh got finer.
    let edges = geom.edges().to_vec();
    if edges.is_empty() {
        return;
    }
    let mean_edge = edges
        .iter()
        .map(|e| (geom.pos(e[1] as usize) - geom.pos(e[0] as usize)).length())
        .sum::<f32>()
        / edges.len() as f32;
    let thickness = node_param_f32(target, "Thickness", 1.0).max(0.0) * mean_edge;
    if thickness <= 0.0 {
        return;
    }
    let rings = node_param_f32(target, "Rings", 2.0).clamp(0.0, 6.0) as usize;
    let iterations = node_param_f32(target, "Iterations", 4.0).clamp(1.0, 32.0) as usize;
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let movable: Vec<bool> = (0..n)
        .map(|p| group.is_empty() || geom.points().in_group(&group, p))
        .collect();

    // The excluded neighbourhood, once: it is topology, and the pass does not
    // change topology.
    let excluded: Vec<Vec<u32>> = (0..n)
        .map(|p| {
            let mut seen = vec![p as u32];
            let mut frontier = vec![p as u32];
            for _ in 0..rings {
                let mut next = Vec::new();
                for &q in &frontier {
                    for &r in geom.point_neighbours(q as usize) {
                        if !seen.contains(&r) {
                            seen.push(r);
                            next.push(r);
                        }
                    }
                }
                if next.is_empty() {
                    break;
                }
                frontier = next;
            }
            seen.sort_unstable();
            seen
        })
        .collect();

    let mut pos: Vec<Vec3> = (0..n).map(|p| geom.pos(p)).collect();
    let mut near = Vec::new();
    for _ in 0..iterations {
        let grid = crate::spatial::PointGrid::build(&pos, thickness);
        // Gathered against the positions at the START of the pass and applied
        // at the end, so the result does not depend on the order points are
        // visited in — a Gauss-Seidel sweep here would make the same mesh
        // untangle differently depending on how its points were numbered.
        let mut push = vec![Vec3::ZERO; n];
        for p in 0..n {
            grid.within(pos[p], thickness, &mut near);
            for &q in &near {
                let q = q as usize;
                if q == p || excluded[p].binary_search(&(q as u32)).is_ok() {
                    continue;
                }
                let d = pos[p] - pos[q];
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
            }
        }
        for p in 0..n {
            if movable[p] {
                pos[p] += push[p];
            }
        }
    }
    for (p, v) in pos.iter().enumerate() {
        geom.set_pos(p, *v);
    }
}

/// The Suture node: resolve a surface against another, and fuse what keeps
/// touching.
///
/// Two thresholds, as the plugin has them. A point closer to the `Against`
/// geometry than Distance Threshold is in contact: it is pushed back out to
/// that distance, and its contact counter goes up. A point that is NOT in
/// contact has its counter reset to zero — contact has to be sustained to
/// count, which is the difference between two surfaces brushing past each
/// other and two surfaces growing into each other.
///
/// Once a point's counter passes Fusion Threshold, it fuses with any other
/// such point within Distance Threshold: they become one point, keeping the
/// lower index's identity and values. That is the operation the name is
/// about — a surface that has been pressed against itself for long enough
/// stops being two surfaces.
pub fn resolve_suture_geometry_with_errors(
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

    let against_name = node_param_str(target, "Against", "");
    let against_name = against_name.trim().to_string();
    let against = if against_name.is_empty() {
        None
    } else {
        find_node_by_name(root, &against_name)
            .and_then(|n| generate_single_node_geometry_with_errors(root, n, visited, ocl_error, sim))
    };
    apply_suture(&mut geom, against.as_ref(), target);
    Some(geom)
}

pub(crate) fn apply_suture(geom: &mut Detail, against: Option<&Detail>, target: &FsNode) {
    let n = geom.num_points();
    if n == 0 {
        return;
    }
    let distance = node_param_f32(target, "Distance Threshold", 0.05).max(0.0);
    let fusion = node_param_f32(target, "Fusion Threshold", 3.0).max(1.0) as i32;
    let counter = node_param_str(target, "Counter", "contact");
    let counter = counter.trim().to_string();
    if counter.is_empty() || distance <= 0.0 {
        return;
    }

    // The counter is LIVE: it is the memory that makes sustained contact
    // different from a brush, and a derivative one would reset every step and
    // never reach the threshold.
    geom.points_mut()
        .get_or_create(&counter, AttribValue::Int(0));

    let mut counts: Vec<i32> = (0..n)
        .map(|p| geom.points().value(&counter, p).map(|v| v.as_f32() as i32).unwrap_or(0))
        .collect();

    if let Some(other) = against.filter(|o| o.num_prims() > 0) {
        let grid = crate::spatial::TriGrid::build(other);
        for p in 0..n {
            let here = geom.pos(p);
            let Some((closest, dist)) = grid.closest(here) else { continue };
            // Inclusive, with room for the float error: a point resolved to
            // exactly the threshold last step is STILL in contact this step.
            // Comparing strictly would have every resolved contact read as
            // released on the next pass, and no counter could ever reach the
            // fusion threshold — contact would be unsustainable by
            // construction.
            if dist > distance * (1.0 + 1e-3) {
                counts[p] = 0;
                continue;
            }
            counts[p] += 1;
            // Pushed back out along the line to the surface. A point sitting
            // exactly on it has no such line, and is left where it is rather
            // than shoved in an invented direction.
            let away = here - closest;
            if away.length_squared() > 1e-12 {
                geom.set_pos(p, closest + away.normalize() * distance);
            }
        }
    }

    for p in 0..n {
        let _ = geom
            .points_mut()
            .set_value(&counter, p, AttribValue::Int(counts[p]));
    }

    // Fuse the sustained contacts that are near each other. Lowest index wins,
    // so the result does not depend on visit order.
    let welded: Vec<usize> = (0..n).filter(|&p| counts[p] >= fusion).collect();
    if welded.len() < 2 {
        return;
    }
    let pos: Vec<Vec3> = welded.iter().map(|&p| geom.pos(p)).collect();
    let grid = crate::spatial::PointGrid::build(&pos, distance);
    let mut rep: Vec<u32> = (0..n as u32).collect();
    let mut near = Vec::new();
    for (i, &p) in welded.iter().enumerate() {
        grid.within(pos[i], distance, &mut near);
        for &j in &near {
            let q = welded[j as usize];
            if q > p {
                // Only ever point a higher index at a lower one, so the map
                // cannot contain a cycle.
                rep[q] = rep[q].min(p as u32);
            }
        }
    }
    geom.fuse_points(&rep);
}

/// The Remesh node: keep the triangulation proportional to the surface.
///
/// The counterweight to Develop. Growth pushes points apart and the triangles
/// between them stretch; an attribute diffused across a stretched mesh is
/// being averaged over distances that no longer mean what they meant, so a
/// growth sim without remeshing degenerates within a few dozen frames however
/// good its attribute maths is.
///
/// The passes live in [`crate::remesh`], with their own tests, because the
/// algorithm is worth reading on its own and a node body is not where it
/// belongs.
pub fn resolve_remesh_geometry_with_errors(
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
    let geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    Some(crate::remesh::remesh(&geom, remesh_settings(target)))
}

pub(crate) fn remesh_settings(target: &FsNode) -> crate::remesh::Settings {
    crate::remesh::Settings {
        target: node_param_f32(target, "Target Length", 0.1).max(1e-4),
        iterations: node_param_f32(target, "Iterations", 3.0).clamp(1.0, 20.0) as usize,
        relax: node_param_f32(target, "Relax", 0.5),
        split: node_param_str(target, "Split", "true") == "true",
        collapse: node_param_str(target, "Collapse", "true") == "true",
        flip: node_param_str(target, "Flip", "true") == "true",
        project: node_param_str(target, "Project", "true") == "true",
    }
}

/// The Develop node: move the surface along its normals by an attribute.
///
/// The whole of surface development in one operator — everything else in the
/// Developer set exists to decide WHAT this should read. A growth attribute
/// built by diffusion, migration and decay is a scalar field; Develop is the
/// step that turns a field into a shape.
///
/// Direction Normal displaces along the smooth point normal, which is what
/// growth means on a surface. Direction Attribute takes a vector attribute
/// instead, for the cases where the surface is not what decides — a
/// gravity-fed sag, a flow along a field.
///
/// Note what this deliberately does NOT do: it moves points and touches
/// nothing else. Topology is `remesh`'s business, and a node that quietly
/// retriangulated while it displaced would make it impossible to tell which of
/// the two turned a simulation to mush.
pub fn resolve_develop_geometry_with_errors(
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
    apply_develop(&mut geom, target, ocl_error);
    Some(geom)
}

pub(crate) fn apply_develop(geom: &mut Detail, target: &FsNode, ocl_error: &mut Option<String>) {
    let name = node_param_str(target, "Attribute", "").trim().to_string();
    if name.is_empty() {
        return;
    }
    if !geom.points().has(&name) {
        if ocl_error.is_none() {
            *ocl_error = Some(format!(
                "Develop '{}': no point attribute named '{}'",
                target.name, name
            ));
        }
        return;
    }

    let scale = node_param_f32(target, "Scale", 0.1);
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let by_attr = node_param_str(target, "Direction", "Normal").eq_ignore_ascii_case("attribute");
    let src = node_param_str(target, "Source", "");
    let src = src.trim().to_string();

    // Normals come off the geometry as it arrives, so every point is displaced
    // along the surface it had BEFORE the displacement — otherwise the points
    // computed late would be following a surface the earlier ones had already
    // moved, and the result would depend on point order.
    let dirs: Vec<Vec3> = if by_attr {
        (0..geom.num_points())
            .map(|p| {
                geom.points()
                    .value(&src, p)
                    .map(|v| v.as_vec3())
                    .unwrap_or(Vec3::ZERO)
            })
            .collect()
    } else {
        point_normals(geom)
    };

    for p in 0..geom.num_points() {
        if !group.is_empty() && !geom.points().in_group(&group, p) {
            continue;
        }
        let amount = geom.points().value(&name, p).map(|v| v.as_f32()).unwrap_or(0.0);
        let moved = geom.pos(p) + dirs[p] * (amount * scale);
        geom.set_pos(p, moved);
    }
}

/// Sample one of the built-in ramps at `t`, clamped to 0..1.
///
/// Named ramps rather than an editable curve because there is no ramp widget
/// yet; these four cover the readings that actually come up — a neutral one, a
/// hot/cold one, a full spectrum, and one that stays legible in greyscale and
/// to colour-blind readers, which matters when the picture IS the result.
pub fn ramp_color(name: &str, t: f32) -> [f32; 3] {
    let stops: &[[f32; 3]] = match name {
        "heat" => &[
            [0.0, 0.0, 0.0],
            [0.6, 0.0, 0.0],
            [1.0, 0.4, 0.0],
            [1.0, 0.9, 0.2],
            [1.0, 1.0, 1.0],
        ],
        "spectrum" => &[
            [0.0, 0.0, 0.8],
            [0.0, 0.8, 0.8],
            [0.0, 0.8, 0.0],
            [0.9, 0.9, 0.0],
            [0.9, 0.0, 0.0],
        ],
        "grayscale" => &[[0.0; 3], [1.0; 3]],
        // Viridis, sampled at five stops.
        _ => &[
            [0.267, 0.005, 0.329],
            [0.229, 0.322, 0.545],
            [0.128, 0.567, 0.551],
            [0.369, 0.789, 0.383],
            [0.993, 0.906, 0.144],
        ],
    };
    let t = t.clamp(0.0, 1.0);
    let last = stops.len() - 1;
    let scaled = t * last as f32;
    let i = (scaled.floor() as usize).min(last.saturating_sub(1));
    let f = scaled - i as f32;
    let (a, b) = (stops[i], stops[(i + 1).min(last)]);
    [
        a[0] + (b[0] - a[0]) * f,
        a[1] + (b[1] - a[1]) * f,
        a[2] + (b[2] - a[2]) * f,
    ]
}

/// The vector markers a [`Detail`] is asking to have drawn, as LINE_LIST
/// pairs: one segment per point, from the point along the staged vector, in
/// the point's own colour.
///
/// Reading them off the assembled scene rather than out of the per-node
/// overlay walk keeps a marker a property of the geometry that reached the
/// viewport rather than of a node's display prefs, and costs one pass over
/// geometry already in hand.
pub fn vis_marker_vertices(geom: &Detail, linearize: impl Fn([f32; 3]) -> [f32; 3]) -> Vec<Vertex3D> {
    let mut out = Vec::new();
    for name in geom.points().names() {
        if !name.starts_with(crate::detail::VIS_PREFIX) {
            continue;
        }
        let Some(data) = geom.points().get(name) else { continue };
        for p in 0..geom.num_points() {
            let Some(v) = data.get(p) else { continue };
            let dir = v.as_vec3();
            if dir.length_squared() < 1e-12 {
                continue;
            }
            // Drawn in the point's own colour, so a Ramp Visualize upstream
            // colours the markers too and one chain says two things at once.
            let color = linearize(geom.color(p));
            out.push(Vertex3D { position: geom.positions()[p], color });
            out.push(Vertex3D { position: (geom.pos(p) + dir).to_array(), color });
        }
    }
    out
}

/// The Visualize node: make a simulation's state visible.
///
/// Two readings, chosen by Mode. **Ramp** maps a scalar attribute through a
/// colour ramp into `Cd`. **Vector** stages a vector attribute as markers the
/// viewport draws from each point (see [`crate::detail::VIS_PREFIX`]).
///
/// Compositing is what makes several attributes legible at once, and it is
/// done by CHAINING rather than by one node growing a list of layers: each
/// Visualize blends its ramp into whatever `Cd` it was handed, so a stack of
/// them reads top to bottom like a stack of layers, and any one of them can be
/// bypassed to see what it was contributing. That is how the plugin's Solver
/// Vis tabs work too — the tabs describe, the Visualize nodes in the chain
/// draw — and it means a Visualize can sit anywhere, not only before the
/// output.
///
/// Range Auto measures the attribute every time it runs, which is the setting
/// a simulation wants: the interesting range moves every frame, and a manual
/// range picked at frame 1 goes flat by frame 50.
pub fn resolve_visualize_geometry_with_errors(
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
    apply_visualize(&mut geom, target, ocl_error);
    Some(geom)
}

pub(crate) fn apply_visualize(geom: &mut Detail, target: &FsNode, ocl_error: &mut Option<String>) {
    let name = node_param_str(target, "Attribute", "").trim().to_string();
    if name.is_empty() {
        return;
    }
    if !geom.points().has(&name) {
        if ocl_error.is_none() {
            *ocl_error = Some(format!(
                "Visualize '{}': no point attribute named '{}'",
                target.name, name
            ));
        }
        return;
    }

    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let affected: Vec<usize> = (0..geom.num_points())
        .filter(|&p| group.is_empty() || geom.points().in_group(&group, p))
        .collect();

    if node_param_str(target, "Mode", "Ramp").eq_ignore_ascii_case("vector") {
        let scale = node_param_f32(target, "Scale", 0.2);
        let staged: Vec<[f32; 3]> = (0..geom.num_points())
            .map(|p| {
                if !affected.contains(&p) {
                    return [0.0; 3];
                }
                (geom
                    .points()
                    .value(&name, p)
                    .map(|v| v.as_vec3())
                    .unwrap_or(Vec3::ZERO)
                    * scale)
                    .to_array()
            })
            .collect();
        // Derivative: a marker describes the state it was made from, and one
        // left over from the previous step would draw a lie.
        let vis = format!("{}{}", crate::detail::VIS_PREFIX, name);
        let _ = geom
            .points_mut()
            .create_kind(&vis, AttribValue::Float3([0.0; 3]), crate::detail::AttribKind::Derivative);
        let _ = geom.points_mut().insert(&vis, AttribData::Float3(staged));
        return;
    }

    // Ramp. Auto measures across EVERY point, not just the group: a group's
    // colours should sit where they belong on the whole picture's scale, or
    // two Visualize nodes over two groups would each claim the full ramp.
    let (from, to) = if node_param_str(target, "Range", "Auto").eq_ignore_ascii_case("manual") {
        (
            node_param_f32(target, "From", 0.0),
            node_param_f32(target, "To", 1.0),
        )
    } else {
        let vals: Vec<f32> = (0..geom.num_points())
            .filter_map(|p| geom.points().value(&name, p))
            .map(|v| v.as_f32())
            .collect();
        (
            vals.iter().copied().fold(f32::INFINITY, f32::min),
            vals.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        )
    };
    let span = to - from;

    let ramp = node_param_str(target, "Ramp", "Viridis").to_lowercase();
    let blend = node_param_str(target, "Blend", "Set").to_lowercase();
    let opacity = node_param_f32(target, "Opacity", 1.0).clamp(0.0, 1.0);

    for p in affected {
        let v = geom.points().value(&name, p).map(|v| v.as_f32()).unwrap_or(0.0);
        // A flat attribute has no range to spread across the ramp; showing it
        // all at the bottom is the honest picture of "nothing varies here".
        let t = if span.abs() < 1e-9 { 0.0 } else { (v - from) / span };
        let c = ramp_color(&ramp, t);
        let old = geom.color(p);
        let mixed = match blend.as_str() {
            "multiply" => [old[0] * c[0], old[1] * c[1], old[2] * c[2]],
            "add" => [old[0] + c[0], old[1] + c[1], old[2] + c[2]],
            _ => c,
        };
        // Opacity is applied the same way for every blend, so a stack of
        // Visualize nodes fades uniformly and Mix is just Set at less than
        // full strength.
        let out = [
            old[0] + (mixed[0] - old[0]) * opacity,
            old[1] + (mixed[1] - old[1]) * opacity,
            old[2] + (mixed[2] - old[2]) * opacity,
        ];
        geom.set_color(p, out);
    }
}

/// The Analysis node: measure an attribute (or the mesh's edge lengths) and
/// write the result to DETAIL attributes.
///
/// This is what the detail class is for, and why the port needed no dictionary
/// type. Houdini's Analysis writes an `<attr>_info` dictionary holding min,
/// max, sum, average and spread; here those are five ordinary detail
/// attributes named `<attr>_min` … `<attr>_spread`, which the Attribute node's
/// Promote and Remap can read back without anything learning to index a dict.
///
/// Measuring edge lengths instead of an attribute is the same reduction over a
/// different column, and it is the measurement a remesher steers by — so it
/// shares the node rather than getting one of its own.
pub fn resolve_analysis_geometry_with_errors(
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
    apply_analysis(&mut geom, target, ocl_error);
    Some(geom)
}

pub(crate) fn apply_analysis(geom: &mut Detail, target: &FsNode, ocl_error: &mut Option<String>) {
    let edges = node_param_str(target, "Source", "Attribute").eq_ignore_ascii_case("edge lengths");
    let name = node_param_str(target, "Attribute", "").trim().to_string();
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();

    // The column to reduce, and the name its answers hang off.
    let (label, values) = if edges {
        let lengths: Vec<f32> = geom
            .edges()
            .iter()
            .map(|e| (geom.pos(e[1] as usize) - geom.pos(e[0] as usize)).length())
            .collect();
        ("edges".to_string(), lengths)
    } else {
        if name.is_empty() {
            return;
        }
        if !geom.points().has(&name) {
            if ocl_error.is_none() {
                *ocl_error = Some(format!(
                    "Analysis '{}': no point attribute named '{}'",
                    target.name, name
                ));
            }
            return;
        }
        let vals: Vec<f32> = (0..geom.num_points())
            .filter(|&p| group.is_empty() || geom.points().in_group(&group, p))
            .filter_map(|p| geom.points().value(&name, p))
            .map(|v| v.as_f32())
            .collect();
        (name.clone(), vals)
    };

    // Nothing to measure is not an error — an empty group or a point cloud
    // with no edges is a legitimate state mid-chain. The answers are zeroed so
    // a downstream reader still finds the attributes it expects.
    let n = values.len();
    let (min, max, sum) = if n == 0 {
        (0.0, 0.0, 0.0)
    } else {
        (
            values.iter().copied().fold(f32::INFINITY, f32::min),
            values.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            values.iter().sum::<f32>(),
        )
    };
    let average = if n == 0 { 0.0 } else { sum / n as f32 };
    let spread = max - min;

    // A measurement is derivative by definition: it describes this step's
    // state, and carrying one into the next would be describing the past.
    for (suffix, value) in [
        ("min", min),
        ("max", max),
        ("sum", sum),
        ("average", average),
        ("spread", spread),
        ("count", n as f32),
    ] {
        geom.detail_mut().create_kind(
            &format!("{}_{}", label, suffix),
            AttribValue::Float(value),
            crate::detail::AttribKind::Derivative,
        );
    }
}

/// The Time node: a detail attribute running 0 to 1 across a frame range.
///
/// The simplest node in the set and the one that makes a simnet's chain able
/// to know where it is: everything time-varying downstream reads this rather
/// than each node growing its own frame parameters. It reads the frame off the
/// evaluation, so a scrub moves it and a cached solve does not.
pub fn resolve_time_geometry_with_errors(
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
    let frame = sim.frame;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    apply_time(&mut geom, target, frame);
    Some(geom)
}

pub(crate) fn apply_time(geom: &mut Detail, target: &FsNode, frame: i32) {
    let name = node_param_str(target, "Attribute", "t").trim().to_string();
    if name.is_empty() {
        return;
    }
    let start = node_param_f32(target, "Start Frame", 1.0);
    let end = node_param_f32(target, "End Frame", 100.0);
    let span = end - start;
    // A zero-length range is 1.0 from its first frame on, not a division by
    // zero: "the whole range has elapsed" is the only reading that composes.
    let t = if span.abs() < 1e-9 {
        if frame as f32 >= start { 1.0 } else { 0.0 }
    } else {
        (frame as f32 - start) / span
    };
    let t = if node_param_str(target, "Clamp", "true") == "true" {
        t.clamp(0.0, 1.0)
    } else {
        t
    };
    // Also derivative: it is a function of the frame, so every step computes
    // it afresh and none of them should inherit it.
    geom.detail_mut()
        .create_kind(&name, AttribValue::Float(t), crate::detail::AttribKind::Derivative);
}

/// Smooth point normals: for each point, the normalized sum of the face
/// normals of the primitives touching it.
///
/// Shared because two callers want the same answer — the viewport's normal
/// whiskers and Align's surface-tangent target — and a normal that disagreed
/// between what you see and what an operator steers by would be a miserable
/// thing to debug.
pub fn point_normals(geom: &Detail) -> Vec<Vec3> {
    (0..geom.num_points())
        .map(|p| {
            let mut sum = Vec3::ZERO;
            for &prim in geom.point_prims(p) {
                let pts = geom.prim_points(prim as usize);
                if pts.len() < 3 {
                    continue;
                }
                let a = geom.pos(pts[0] as usize);
                let b = geom.pos(pts[1] as usize);
                let c = geom.pos(pts[2] as usize);
                let n = (b - a).cross(c - a);
                if n.length_squared() > 1e-12 {
                    sum += n;
                }
            }
            sum.normalize_or_zero()
        })
        .collect()
}

/// Which points a neighbourhood operator treats as a point's neighbours.
enum Hood {
    /// Points reachable within N edge rings. The mesh's own connectivity, and
    /// the only one of the three that respects a surface: two points a
    /// hair's breadth apart across a fold are not neighbours.
    Connectivity(usize),
    /// Points within a world-space distance, fold or no fold.
    Radius(f32),
    /// Every point. Not a neighbourhood so much as the limit of one — the
    /// "global" reading of Diffuse and Concentrate, where the thing a value
    /// moves toward is the whole geometry's average.
    Global,
}

/// The Neighbour node: one operator family over an attribute and the points
/// around each point.
///
/// Four Modes, all of them "a value and the values near it":
///
/// - **Diffuse** moves each value toward the average of its neighbours.
/// - **Concentrate** moves it away — the same quantity with the sign flipped,
///   which sharpens a gradient instead of smoothing it.
/// - **Migrate** pushes value along a per-point Direction vector. Neighbours
///   in front of a point receive; the point loses exactly what they gain, so
///   the total is conserved and value is transported rather than created.
/// - **Bleed** decays toward zero. It has no neighbours in it at all, but it
///   belongs to the family: it is what the others compose with to keep a
///   simulation from saturating, and separating it would make the chain
///   longer without making it clearer. Neighbourhood is ignored.
///
/// Every mode works componentwise on any attribute type, so one node covers a
/// float, an integer count and a vector — integers round on the way back so a
/// counter stays whole. Amount scales the whole edit and is the natural
/// per-frame rate inside a simnet.
///
/// This is the node the proposal's table collapses seven Houdini operators
/// into (Diffuse, Concentrate, Migrate, Bleed, plus the Align/Lead/Charge
/// vector steering still to come), and it is deliberately a native Rust
/// evaluator rather than a kernel: it walks topology, which the kernel
/// language's C subset has no way to express.
pub fn resolve_neighbour_geometry_with_errors(
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
    apply_neighbour(&mut geom, target, ocl_error);
    Some(geom)
}

/// The Neighbour operator itself, over geometry already in hand.
///
/// Split from the resolver so the operator can be exercised on geometry a test
/// controls, rather than only on whatever a graph happens to produce.
pub(crate) fn apply_neighbour(geom: &mut Detail, target: &FsNode, ocl_error: &mut Option<String>) {
    let name = node_param_str(target, "Attribute", "").trim().to_string();
    if name.is_empty() {
        return;
    }
    let Some(ty) = geom.points().get(&name).map(|a| a.ty()) else {
        if ocl_error.is_none() {
            *ocl_error = Some(format!(
                "Neighbour '{}': no point attribute named '{}'",
                target.name, name
            ));
        }
        return;
    };

    let n = geom.num_points();
    let k = ty.components();
    let amount = node_param_f32(target, "Amount", 0.5).clamp(0.0, 1.0);
    let mode = node_param_str(target, "Mode", "Diffuse").to_lowercase();

    // The attribute as a flat n x k matrix of components, so one body serves
    // every type. Integers ride through as floats and round on the way back.
    let mut val: Vec<f32> = Vec::with_capacity(n * k);
    for p in 0..n {
        let comps = geom
            .points()
            .value(&name, p)
            .map(attrib_components)
            .unwrap_or_else(|| vec![0.0; k]);
        for c in 0..k {
            val.push(comps.get(c).copied().unwrap_or(0.0));
        }
    }

    // A Group narrows which points are EDITED. Their neighbours are still read
    // from the whole geometry — a diffusion that could only see inside its own
    // group would bend away from the boundary rather than across it.
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let edits: Vec<bool> = (0..n)
        .map(|p| group.is_empty() || geom.points().in_group(&group, p))
        .collect();

    let hood = match node_param_str(target, "Neighbourhood", "Connectivity").to_lowercase().as_str() {
        "radius" => Hood::Radius(node_param_f32(target, "Radius", 0.2).max(0.0)),
        "global" => Hood::Global,
        _ => Hood::Connectivity(node_param_f32(target, "Rings", 1.0).max(1.0) as usize),
    };

    let mut out = val.clone();
    match mode.as_str() {
        "bleed" => {
            for p in (0..n).filter(|&p| edits[p]) {
                for c in 0..k {
                    out[p * k + c] = val[p * k + c] * (1.0 - amount);
                }
            }
        }
        "migrate" => {
            let dir_name = node_param_str(target, "Direction", "");
            let dir_name = dir_name.trim().to_string();
            if dir_name.is_empty() || !geom.points().has(&dir_name) {
                if ocl_error.is_none() {
                    *ocl_error = Some(format!(
                        "Neighbour '{}': Migrate needs a Direction attribute",
                        target.name
                    ));
                }
                return;
            }
            // Each point hands a fraction of its value to the neighbours that
            // lie in front of it, split by how squarely they face the
            // direction. The sender is debited exactly the sum of the credits,
            // which is what makes this transport rather than growth.
            for p in (0..n).filter(|&p| edits[p]) {
                let dir = geom
                    .points()
                    .value(&dir_name, p)
                    .map(|v| v.as_vec3())
                    .unwrap_or(Vec3::ZERO)
                    .normalize_or_zero();
                if dir == Vec3::ZERO {
                    continue;
                }
                let here = geom.pos(p);
                let nbrs = neighbours_of(geom, p, &hood);
                let weights: Vec<(usize, f32)> = nbrs
                    .iter()
                    .filter_map(|&q| {
                        let q = q as usize;
                        let to = (geom.pos(q) - here).normalize_or_zero();
                        let w = to.dot(dir);
                        (w > 0.0).then_some((q, w))
                    })
                    .collect();
                let total: f32 = weights.iter().map(|(_, w)| w).sum();
                if total <= 0.0 {
                    continue;
                }
                for c in 0..k {
                    let moved = val[p * k + c] * amount;
                    out[p * k + c] -= moved;
                    for (q, w) in &weights {
                        out[q * k + c] += moved * (w / total);
                    }
                }
            }
        }
        // The vector-steering modes. Where Diffuse and Migrate move a
        // QUANTITY between points, these turn a DIRECTION and leave its
        // magnitude alone — a vector attribute carries a heading and a
        // strength, and steering is a statement about the heading only.
        "align" | "lead" => {
            if k < 2 {
                if ocl_error.is_none() {
                    *ocl_error = Some(format!(
                        "Neighbour '{}': {} steers a direction, and '{}' is scalar",
                        target.name, mode, name
                    ));
                }
            } else {
                let read = |val: &[f32], p: usize| {
                    Vec3::new(val[p * k], val[p * k + 1], if k > 2 { val[p * k + 2] } else { 0.0 })
                };
                let src_name = node_param_str(target, "Source", "");
                let src_name = src_name.trim().to_string();

                let targets: Vec<Vec3> = if mode == "align" {
                    let kind = node_param_str(target, "Target", "Local Average").to_lowercase();
                    align_targets(geom, &val, &hood, &kind, target, &src_name, &read)
                } else {
                    // Lead: each point turns toward the neighbouring vector
                    // that disagrees with it MOST, scaled by an influence
                    // attribute. Not a typo for "agrees" — the dissenter
                    // leads, which is what makes a reorientation propagate
                    // across a surface instead of settling on the spot.
                    (0..n)
                        .map(|p| {
                            let v = read(&val, p).normalize_or_zero();
                            if v == Vec3::ZERO {
                                return Vec3::ZERO;
                            }
                            let mut best = (0.0f32, Vec3::ZERO);
                            for &q in &neighbours_of(geom, p, &hood) {
                                let w = read(&val, q as usize);
                                if w.normalize_or_zero() == Vec3::ZERO {
                                    continue;
                                }
                                let influence = if src_name.is_empty() {
                                    1.0
                                } else {
                                    geom.points()
                                        .value(&src_name, q as usize)
                                        .map(|x| x.as_f32())
                                        .unwrap_or(0.0)
                                };
                                let score = (1.0 - v.dot(w.normalize_or_zero())) * influence;
                                if score > best.0 {
                                    best = (score, w);
                                }
                            }
                            best.1
                        })
                        .collect()
                };

                for p in (0..n).filter(|&p| edits[p]) {
                    let v = read(&val, p);
                    let len = v.length();
                    // A target has to be big enough to MEAN a direction. The
                    // case that forces this is Surface Tangent on a vector
                    // already normal to the surface: the projection is zero in
                    // exact arithmetic and a few parts in 10^7 in practice, so
                    // an exact-zero guard lets it through and normalizing turns
                    // pure float error into a heading. Judged against the
                    // vector being steered, since that sets the scale.
                    if targets[p].length() <= 1e-6 * len.max(1.0) {
                        continue;
                    }
                    let (dir, goal) = (v.normalize_or_zero(), targets[p].normalize_or_zero());
                    if dir == Vec3::ZERO || goal == Vec3::ZERO {
                        continue;
                    }
                    // Turning a vector onto one exactly opposite has no
                    // shortest arc, and the midpoint of the blend is the zero
                    // vector. Leaving it be is the only answer that does not
                    // invent a direction.
                    let turned = dir.lerp(goal, amount).normalize_or_zero();
                    if turned == Vec3::ZERO {
                        continue;
                    }
                    let out_v = turned * len;
                    out[p * k] = out_v.x;
                    out[p * k + 1] = out_v.y;
                    if k > 2 {
                        out[p * k + 2] = out_v.z;
                    }
                }
            }
        }
        // Charge: accumulate, and discharge to the neighbours on crossing a
        // threshold.
        //
        // NOTE: this is not a port. `developer_charge` is an empty shell in
        // hou-control — two parameters, `release` and `amount`, neither of
        // them read by anything inside it — so there is no behaviour to carry
        // across, only a name and a pair of parameter names. What is written
        // here is the reading those two names most plainly suggest, and the
        // one that earns its place beside the others: Bleed loses value,
        // Migrate transports it, and Charge stores it until there is enough to
        // spend. Integrate-and-fire, which is how an excitable medium makes a
        // wave out of a gradient.
        "charge" => {
            let release = node_param_f32(target, "Release", 1.0);
            for p in (0..n).filter(|&p| edits[p]) {
                for c in 0..k {
                    out[p * k + c] = val[p * k + c] + amount;
                }
            }
            if release > 0.0 {
                // Who fires is decided from the post-accumulation snapshot,
                // not from `out` as it is being written: otherwise a point
                // that received a neighbour's discharge could fire in the same
                // pass, and the result would depend on point order.
                let charged = out.clone();
                for p in (0..n).filter(|&p| edits[p]) {
                    for c in 0..k {
                        if charged[p * k + c] < release {
                            continue;
                        }
                        let nbrs = neighbours_of(geom, p, &hood);
                        if nbrs.is_empty() {
                            continue;
                        }
                        let share = charged[p * k + c] / nbrs.len() as f32;
                        out[p * k + c] -= charged[p * k + c];
                        for &q in &nbrs {
                            out[q as usize * k + c] += share;
                        }
                    }
                }
            }
        }
        // Diffuse and Concentrate are one operation and its negation: the
        // distance to the neighbourhood's average, travelled toward it or
        // away from it.
        other => {
            let sign = if other == "concentrate" { -1.0 } else { 1.0 };
            // A point is never its own neighbour, under any of the three
            // rules — so the global case is the total MINUS this point, over
            // the other n-1. Getting that wrong is invisible on a spread-out
            // attribute and glaring on a spike: a lone high point would
            // average partly with itself and refuse to come down. It also
            // keeps the rules continuous with each other, so a radius wide
            // enough to cover the geometry behaves like Global rather than
            // almost like it.
            let global_total: Option<Vec<f32>> = matches!(hood, Hood::Global).then(|| {
                let mut total = vec![0.0; k];
                for p in 0..n {
                    for c in 0..k {
                        total[c] += val[p * k + c];
                    }
                }
                total
            });

            for p in (0..n).filter(|&p| edits[p]) {
                let mean: Vec<f32> = match &global_total {
                    Some(total) => {
                        if n < 2 {
                            continue;
                        }
                        (0..k)
                            .map(|c| (total[c] - val[p * k + c]) / (n - 1) as f32)
                            .collect()
                    }
                    None => {
                        let nbrs = neighbours_of(geom, p, &hood);
                        if nbrs.is_empty() {
                            continue;
                        }
                        let mut mean = vec![0.0; k];
                        for &q in &nbrs {
                            for c in 0..k {
                                mean[c] += val[q as usize * k + c];
                            }
                        }
                        for m in mean.iter_mut() {
                            *m /= nbrs.len() as f32;
                        }
                        mean
                    }
                };
                for c in 0..k {
                    let v = val[p * k + c];
                    out[p * k + c] = v + sign * amount * (mean[c] - v);
                }
            }
        }
    }

    for p in 0..n {
        let comps = &out[p * k..p * k + k];
        let _ = geom
            .points_mut()
            .set_value(&name, p, components_attrib(ty, comps));
    }
}

/// The vector each point should be steered toward, under one Align target.
///
/// The five targets are the ones the plugin's Align lists, and they are five
/// different answers to "agree with what": the neighbours, the whole
/// geometry, a fixed heading, another attribute, or the surface itself.
#[allow(clippy::too_many_arguments)]
fn align_targets(
    geom: &Detail,
    val: &[f32],
    hood: &Hood,
    kind: &str,
    target: &FsNode,
    src_name: &str,
    read: &impl Fn(&[f32], usize) -> Vec3,
) -> Vec<Vec3> {
    let n = geom.num_points();
    match kind {
        "global average" => {
            // The mean of every OTHER point, for the same reason every
            // neighbourhood excludes its own point: a vector that averaged
            // partly with itself could never be turned all the way.
            let total: Vec3 = (0..n).map(|p| read(val, p)).sum();
            (0..n)
                .map(|p| {
                    if n < 2 {
                        Vec3::ZERO
                    } else {
                        (total - read(val, p)) / (n - 1) as f32
                    }
                })
                .collect()
        }
        "constant" => {
            let c = node_param_vec3(target, "Constant", Vec3::Y);
            vec![c; n]
        }
        "attribute" => (0..n)
            .map(|p| {
                geom.points()
                    .value(src_name, p)
                    .map(|v| v.as_vec3())
                    .unwrap_or(Vec3::ZERO)
            })
            .collect(),
        // The vector with its normal component removed — what is left is the
        // part that lies in the surface. A vector already normal to the
        // surface projects to nothing and is left alone by the caller.
        "surface tangent" => {
            let normals = point_normals(geom);
            (0..n)
                .map(|p| {
                    let v = read(val, p);
                    v - normals[p] * v.dot(normals[p])
                })
                .collect()
        }
        _ => (0..n)
            .map(|p| {
                let nbrs = neighbours_of(geom, p, hood);
                if nbrs.is_empty() {
                    return Vec3::ZERO;
                }
                nbrs.iter().map(|&q| read(val, q as usize)).sum::<Vec3>() / nbrs.len() as f32
            })
            .collect(),
    }
}

/// The points around `p` under one neighbourhood rule, never including `p`.
fn neighbours_of(geom: &Detail, p: usize, hood: &Hood) -> Vec<u32> {
    match *hood {
        Hood::Connectivity(1) => geom.point_neighbours(p).to_vec(),
        Hood::Connectivity(rings) => {
            // Breadth-first over the edge graph. Each ring is the frontier of
            // the last, so the cost is the size of the neighbourhood rather
            // than of the geometry.
            let mut seen = vec![false; geom.num_points()];
            seen[p] = true;
            let mut frontier = vec![p as u32];
            let mut out = Vec::new();
            for _ in 0..rings {
                let mut next = Vec::new();
                for &q in &frontier {
                    for &r in geom.point_neighbours(q as usize) {
                        if !std::mem::replace(&mut seen[r as usize], true) {
                            next.push(r);
                            out.push(r);
                        }
                    }
                }
                if next.is_empty() {
                    break;
                }
                frontier = next;
            }
            out.sort_unstable();
            out
        }
        // Brute force, and knowingly so: a uniform grid is worth building when
        // a node needs it on geometry this does not comfortably handle, and
        // Phase 3's collision work will need the same index.
        Hood::Radius(r) => {
            let here = geom.pos(p);
            let r2 = r * r;
            (0..geom.num_points() as u32)
                .filter(|&q| q as usize != p && (geom.pos(q as usize) - here).length_squared() <= r2)
                .collect()
        }
        Hood::Global => (0..geom.num_points() as u32).filter(|&q| q as usize != p).collect(),
    }
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
    apply_attribute(&mut geom, target, ocl_error);
    Some(geom)
}

/// The Attribute operator itself, over geometry already in hand — split from
/// the resolver for the same reason `apply_neighbour` is.
pub(crate) fn apply_attribute(geom: &mut Detail, target: &FsNode, ocl_error: &mut Option<String>) {
    let name = node_param_str(target, "Attribute Name", "attr1").trim().to_string();
    if name.is_empty() {
        return;
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
        // Fit a range onto another range, optionally through a clamp. The
        // workhorse: a simulation attribute measured by Analysis is almost
        // always remapped before anything reads it.
        "remap" => {
            let (f0, f1) = (
                node_param_f32(target, "From Min", 0.0),
                node_param_f32(target, "From Max", 1.0),
            );
            let (t0, t1) = (
                node_param_f32(target, "To Min", 0.0),
                node_param_f32(target, "To Max", 1.0),
            );
            let span = f1 - f0;
            if span.abs() < 1e-9 {
                fail = format!("From Min and From Max are both {}", f0);
            } else {
                edit_components(geom, &name, &affected, |v| {
                    t0 + (v - f0) / span * (t1 - t0)
                });
            }
        }
        // Clamp into a range. From Min / From Max name the bounds, so Remap
        // and Clip read the same way and chain without renaming anything.
        "clip" => {
            let (lo, hi) = (
                node_param_f32(target, "From Min", 0.0),
                node_param_f32(target, "From Max", 1.0),
            );
            let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
            edit_components(geom, &name, &affected, |v| v.clamp(lo, hi));
        }
        // Rescale so the attribute's sum, maximum or range hits To Max.
        // Unlike Remap this MEASURES first, so it needs no knowledge of what
        // the values happen to be — which is what makes it survive a
        // simulation whose range moves every frame.
        "normalize" => {
            let vals: Vec<f32> = affected
                .iter()
                .filter_map(|&p| geom.points().value(&name, p))
                .map(|v| v.as_f32())
                .collect();
            let goal = node_param_f32(target, "To Max", 1.0);
            let measure = match node_param_str(target, "Target", "Maximum").to_lowercase().as_str() {
                "sum" => vals.iter().sum::<f32>(),
                "range" => {
                    let hi = vals.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    let lo = vals.iter().copied().fold(f32::INFINITY, f32::min);
                    hi - lo
                }
                _ => vals.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            };
            if !measure.is_finite() || measure.abs() < 1e-9 {
                fail = format!("nothing to normalize: the measure is {}", measure);
            } else {
                let k = goal / measure;
                edit_components(geom, &name, &affected, |v| v * k);
            }
        }
        // Fold a second attribute into this one, componentwise. Dot, Distance
        // and Length collapse to a scalar written into every component, since
        // the destination keeps its own type.
        "composite" => {
            let b_name = node_param_str(target, "Source B", "");
            let b_name = b_name.trim().to_string();
            if !geom.points().has(&b_name) {
                fail = format!("Source B '{}' is not a point attribute", b_name);
            } else {
                let op = node_param_str(target, "Combine Op", "Add").to_lowercase();
                let Some(ty) = geom.points().get(&name).map(|a| a.ty()) else {
                    *ocl_error = Some(format!("Attribute '{}': '{}' is missing", target.name, name));
                    return;
                };
                let k = ty.components();
                for &p in &affected {
                    let a = geom.points().value(&name, p).map(attrib_components).unwrap_or_default();
                    let b = geom.points().value(&b_name, p).map(attrib_components).unwrap_or_default();
                    let at = |v: &Vec<f32>, i: usize| v.get(i).copied().unwrap_or(0.0);
                    let out: Vec<f32> = match op.as_str() {
                        "dot" => {
                            let d: f32 = (0..k.max(b.len())).map(|i| at(&a, i) * at(&b, i)).sum();
                            vec![d; k]
                        }
                        "distance" => {
                            let d: f32 = (0..k.max(b.len()))
                                .map(|i| (at(&a, i) - at(&b, i)).powi(2))
                                .sum::<f32>()
                                .sqrt();
                            vec![d; k]
                        }
                        "length" => {
                            let d: f32 =
                                (0..b.len()).map(|i| at(&b, i).powi(2)).sum::<f32>().sqrt();
                            vec![d; k]
                        }
                        _ => (0..k)
                            .map(|i| {
                                let (x, y) = (at(&a, i), at(&b, i));
                                match op.as_str() {
                                    "subtract" => x - y,
                                    "multiply" => x * y,
                                    // Division by zero yields the numerator
                                    // rather than an infinity that poisons
                                    // every later frame of a solve.
                                    "divide" => if y.abs() < 1e-9 { x } else { x / y },
                                    "minimum" => x.min(y),
                                    "maximum" => x.max(y),
                                    "average" => (x + y) * 0.5,
                                    "difference" => (x - y).abs(),
                                    _ => x + y,
                                }
                            })
                            .collect(),
                    };
                    let _ = geom.points_mut().set_value(&name, p, components_attrib(ty, &out));
                }
            }
        }
        // Move an attribute between classes. Point to Detail is the reduction
        // Analysis does by hand; Detail to Point is how a measured constant
        // gets back into per-point arithmetic.
        "promote" => {
            let to_detail =
                node_param_str(target, "To Class", "Detail").eq_ignore_ascii_case("detail");
            let method = node_param_str(target, "Method", "Average").to_lowercase();
            if to_detail {
                match geom.points().get(&name).map(|a| a.ty()) {
                    None => fail = format!("'{}' is not a point attribute", name),
                    Some(ty) => {
                        let k = ty.components();
                        let rows: Vec<Vec<f32>> = (0..geom.num_points())
                            .filter_map(|p| geom.points().value(&name, p))
                            .map(attrib_components)
                            .collect();
                        let reduced = reduce_rows(&rows, k, &method);
                        geom.detail_mut().create(&name, components_attrib(ty, &reduced));
                    }
                }
            } else {
                match geom.detail().get(&name).map(|a| a.ty()) {
                    None => fail = format!("'{}' is not a detail attribute", name),
                    Some(ty) => {
                        let v = geom
                            .detail()
                            .value(&name, 0)
                            .unwrap_or(components_attrib(ty, &[0.0]));
                        geom.points_mut().create(&name, v);
                    }
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
                        // Declared where the author knows the answer: at the
                        // point of creation, not in a list somewhere else that
                        // has to be kept in step.
                        let kind = if node_param_str(target, "Kind", "Live")
                            .eq_ignore_ascii_case("derivative")
                        {
                            crate::detail::AttribKind::Derivative
                        } else {
                            crate::detail::AttribKind::Live
                        };
                        geom.points_mut().create_kind(&name, zero, kind);
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
}

/// Apply a scalar function to every component of `name` on the given points.
///
/// One place for the "same arithmetic, any width" shape that Remap, Clip and
/// Normalize all have — a float takes it once, a vector takes it per
/// component, an integer rounds on the way back.
fn edit_components(geom: &mut Detail, name: &str, points: &[usize], f: impl Fn(f32) -> f32) {
    let Some(ty) = geom.points().get(name).map(|a| a.ty()) else { return };
    for &p in points {
        let Some(cur) = geom.points().value(name, p) else { continue };
        let out: Vec<f32> = attrib_components(cur).into_iter().map(&f).collect();
        let _ = geom.points_mut().set_value(name, p, components_attrib(ty, &out));
    }
}

/// Reduce a column of component rows to one row, componentwise.
fn reduce_rows(rows: &[Vec<f32>], k: usize, method: &str) -> Vec<f32> {
    (0..k)
        .map(|c| {
            let col = rows.iter().map(|r| r.get(c).copied().unwrap_or(0.0));
            match method {
                "sum" => col.sum(),
                "minimum" => col.fold(f32::INFINITY, f32::min),
                "maximum" => col.fold(f32::NEG_INFINITY, f32::max),
                "first" => rows.first().and_then(|r| r.get(c)).copied().unwrap_or(0.0),
                _ => {
                    let n = rows.len().max(1) as f32;
                    col.sum::<f32>() / n
                }
            }
        })
        .map(|v: f32| if v.is_finite() { v } else { 0.0 })
        .collect()
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
        let attr_names = parse_attr_refs(&code);
        let is_generator = code.contains("out_count");

        // A DEFORMER runs over POINTS. It never sees a triangle, so topology,
        // groups and point identities pass through untouched — the flatten,
        // weld and "first corner wins" reconciliation this used to need are
        // all gone. Inside a simnet that is the difference between a solver
        // that can follow a point across frames and one that cannot.
        if !is_generator {
            let result = run_kernel_on_detail(&processed_code, &attr_names, &mut input, &flat_values);
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

    let make = |data: &[cl_float]| -> Result<ClBuffer<cl_float>, String> {
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
    let param_buf = make(&param_data)?;
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
pub fn run_kernel_on_detail(
    code: &str,
    names: &[String],
    geom: &mut Detail,
    params: &[f32],
) -> Result<(), String> {
    let count = geom.num_points();
    if count == 0 {
        return Ok(());
    }
    let mut pos: Vec<cl_float> = bytemuck::cast_slice(geom.positions()).to_vec();
    let mut col: Vec<cl_float> = Vec::with_capacity(count * 3);
    for p in 0..count {
        col.extend_from_slice(&geom.color(p));
    }

    // The names come from the caller, NOT from `code`: by the time a kernel
    // reaches here the preprocessor has rewritten every `attrf("mass", i)`
    // into `attr_0[i]`, so there is nothing left to parse. Reading them off
    // the processed source bound zero buffers and the attribute silently never
    // appeared — which is the gap between a test that parses raw source and a
    // test that runs a kernel with its buffers handed to it.
    let mut attrs: Vec<(String, Vec<cl_float>)> = Vec::with_capacity(names.len());
    for name in names {
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
        || nt == "neighbour"
        || nt == "time"
        || nt == "analysis"
        || nt == "visualize"
        || nt == "develop"
        || nt == "remesh"
        || nt == "suture"
        || nt == "detangle"
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
        } else if node.node_type.eq_ignore_ascii_case("neighbour") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_neighbour_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("time") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_time_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("detangle") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_detangle_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("suture") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_suture_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("remesh") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_remesh_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("develop") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_develop_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("visualize") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_visualize_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("analysis") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_analysis_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
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

        // The same triangles, wound the other way round. The soup wound
        // clockwise seen from outside, so every normal on a native sphere
        // pointed INTO it; the welded generator wound with it until Develop
        // made the bug visible.
        //
        // Compared as an unordered collection of triangles, each keyed by its
        // corners rounded and sorted. Not bit-identical, and the difference is
        // the point: the soup computed its seam corner at phi = TAU and its
        // south pole once per longitude, so the surface had a ~1e-7 crack down
        // it, which is also why the key rounds before it sorts.
        let key = |t: &[[f32; 3]]| {
            let mut c: Vec<[i64; 3]> = t
                .iter()
                .map(|p| {
                    [
                        (p[0] as f64 * 1e4).round() as i64,
                        (p[1] as f64 * 1e4).round() as i64,
                        (p[2] as f64 * 1e4).round() as i64,
                    ]
                })
                .collect();
            c.sort_unstable();
            c
        };
        let mut want: Vec<Vec<[i64; 3]>> = kept.chunks_exact(3).map(key).collect();
        let mut got: Vec<Vec<[i64; 3]>> = after.chunks_exact(3).map(key).collect();
        want.sort();
        got.sort();
        assert_eq!(got, want, "the welded sphere is not the same set of triangles");

        // And they face outward now, which is the whole reason for the change.
        let welded = sphere_detail(center, radius, lat, lon);
        let normals = point_normals(&welded);
        for p in 0..welded.num_points() {
            let radial = (welded.pos(p) - center).normalize();
            assert!(normals[p].dot(radial) > 0.0, "point {p} still faces inward");
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
    // The frame this sim shows its seed at. Empty means "follow the timeline",
    // which is what every sim did before this parameter existed; a number
    // decouples when a simulation starts from when the shot does, so two sims
    // in one scene can begin at different times.
    let start_frame = node_param_f32(target, "Start Frame", sim.start_frame as f32).round() as i32;
    let due = (sim.frame - start_frame).max(0);

    // Resume from the cached solve when it is still valid and has not run PAST
    // the frame asked for; scrubbing backwards has to restart from the seed,
    // because a step is not invertible. Memory first, then disk.
    let cached = sim.cache.entries.get(&target.id).and_then(|prev| {
        (prev.key == key && prev.frame <= due).then(|| (prev.state.clone(), prev.frame))
    });
    let caching = node_param_str(target, "Cache", "false") == "true";
    let (mut state, mut done) = match cached {
        Some(hit) => hit,
        None if caching => match read_sim_cache(&target.id, key, due) {
            Some(hit) => hit,
            None => (seed, 0),
        },
        None => (seed, 0),
    };

    // Substeps run the chain more than once per frame. A step's size is what
    // decides whether an iterative solve converges or oscillates, and one step
    // per frame ties that to the frame rate, which is the wrong coupling for
    // anything meant to model a physical process.
    //
    // Clamped, and deliberately not by trusting the parameter's declared
    // range: a hand-edited project file reaches here too, and a solve of a
    // hundred thousand substeps is indistinguishable from a hang.
    let substeps = (node_param_f32(target, "Substeps", 1.0).round() as i64).clamp(1, 64);
    // What one substep is worth, as a fraction of a frame. The chain reads it
    // by promoting it onto points and compositing it into a rate — halve the
    // step and a rate scaled by `dt` covers the same ground in twice as many
    // steps, which is what makes substeps a stability control rather than a
    // speed control.
    let dt = 1.0 / substeps as f32;

    while done < due {
        for _ in 0..substeps {
            // The step boundary, and the contract that makes a chain
            // composable:
            //
            // Going in, every Derivative attribute is zeroed. The chain is
            // expected to rebuild them from live data this step, and one it
            // forgets reads zero rather than quietly carrying last step's
            // value forward — which is the failure that looks like a physics
            // bug and is not one. Per SUBSTEP, not per frame: each run of the
            // chain is a step, and derivative data describes the state it was
            // computed from.
            state.clear_derivatives();
            state
                .detail_mut()
                .create_kind("dt", AttribValue::Float(dt), crate::detail::AttribKind::Derivative);
            let prev = state.clone();

            sim.feedback.push((target.id.clone(), state));
            let stepped = generate_single_node_geometry_with_errors(root, &output_node, visited, ocl_error, sim);
            let fed_back = sim.feedback.pop().map(|(_, g)| g);
            // A step that yields nothing (an unwired chain, a failed kernel)
            // holds the previous state rather than collapsing the sim to empty
            // geometry.
            state = stepped.or(fed_back).unwrap_or_default();

            // Coming out, any Live attribute the chain DROPPED is restored
            // from the previous state by point identity. A node that rebuilds
            // geometry mid-chain — a kernel generator today, a remesh in Phase
            // 3 — no longer silently takes the simulation's memory with it.
            state.restore_live_from(&prev);
        }
        done += 1;
    }

    sim.cache.entries.insert(
        target.id.clone(),
        SimSolve { key, frame: due, state: state.clone() },
    );
    if caching && due > 0 {
        write_sim_cache(&target.id, key, due, &state);
    }
    Some(state)
}

/// Where a simnet's solved state is parked between runs.
///
/// Under the cache directory, not the project: it is derived data that can be
/// recomputed, and a project directory that silently grew hundreds of
/// megabytes of solver state would be a nasty surprise to copy or back up.
fn sim_cache_path(node_id: &str) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    // The id is a node id, not a filename, so anything that could climb out of
    // the directory is replaced rather than trusted.
    let safe: String = node_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    Some(base.join("cce/cce-designer/sim").join(format!("{safe}.simcache")))
}

/// The header in front of a cached state: which solve it belongs to and which
/// frame it stopped at. Both are checked before the geometry is trusted — a
/// cache from a different chain is worse than no cache, because it looks like
/// an answer.
fn write_sim_cache(node_id: &str, key: u64, frame: i32, state: &Detail) {
    let Some(path) = sim_cache_path(node_id) else { return };
    write_sim_cache_at(&path, key, frame, state);
}

/// [`write_sim_cache`] against a given path, so the format can be exercised
/// without a process-wide environment variable.
fn write_sim_cache_at(path: &std::path::Path, key: u64, frame: i32, state: &Detail) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut blob = Vec::new();
    blob.extend_from_slice(&key.to_le_bytes());
    blob.extend_from_slice(&frame.to_le_bytes());
    blob.extend_from_slice(&state.to_bytes());
    // Written beside the target and renamed, so a cache half-written when the
    // app dies is never read as a whole one.
    let tmp = path.with_extension("simcache.tmp");
    if std::fs::write(&tmp, &blob).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// A cached state for this solve, if one is on disk and has not run past the
/// frame being asked for. Any failure — missing, truncated, stale, corrupt —
/// reads as "no cache" and the sim solves from its seed.
fn read_sim_cache(node_id: &str, key: u64, due: i32) -> Option<(Detail, i32)> {
    read_sim_cache_at(&sim_cache_path(node_id)?, key, due)
}

fn read_sim_cache_at(path: &std::path::Path, key: u64, due: i32) -> Option<(Detail, i32)> {
    let blob = std::fs::read(path).ok()?;
    if blob.len() < 12 || u64::from_le_bytes(blob[0..8].try_into().ok()?) != key {
        return None;
    }
    let frame = i32::from_le_bytes(blob[8..12].try_into().ok()?);
    if frame < 0 || frame > due {
        return None;
    }
    Detail::from_bytes(&blob[12..]).ok().map(|d| (d, frame))
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

    /// Apply one Attribute-node operation to geometry in hand.
    fn run_attr(before: &Detail, params: &[(&str, &str)]) -> (Detail, Option<String>) {
        let mut geom = before.clone();
        let mut ps = vec![param("Input", "In"), param("Attribute Name", "mass")];
        for (k, v) in params {
            match ps.iter_mut().find(|p| p.name == *k) {
                Some(p) => p.default = v.to_string(),
                None => ps.push(param(k, v)),
            }
        }
        let n = node("id-a", "A", "attribute", ps, vec![]);
        let mut err = None;
        apply_attribute(&mut geom, &n, &mut err);
        (geom, err)
    }

    /// A sphere whose `mass` ramps 0, 1, 2 … across its points.
    fn ramped_mass() -> Detail {
        let mut d = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        d.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..d.num_points() {
            d.points_mut()
                .set_value("mass", p, AttribValue::Float(p as f32))
                .unwrap();
        }
        d
    }

    #[test]
    fn test_attribute_remap_and_clip_share_their_range_parameters() {
        let before = ramped_mass();
        let n = before.num_points();
        let last = (n - 1) as f32;
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();

        let (remapped, err) = run_attr(
            &before,
            &[
                ("Operation", "Remap"),
                ("From Min", "0.00"),
                ("From Max", &last.to_string()),
                ("To Min", "0.00"),
                ("To Max", "1.00"),
            ],
        );
        assert!(err.is_none(), "{err:?}");
        assert_eq!(mass(&remapped, 0), 0.0);
        assert!((mass(&remapped, n - 1) - 1.0).abs() < 1e-5);

        // Clip reads the same From Min / From Max, so the two chain without
        // renaming anything between them.
        let (clipped, err) = run_attr(
            &before,
            &[("Operation", "Clip"), ("From Min", "2.00"), ("From Max", "5.00")],
        );
        assert!(err.is_none(), "{err:?}");
        assert_eq!(mass(&clipped, 0), 2.0);
        assert_eq!(mass(&clipped, 3), 3.0);
        assert_eq!(mass(&clipped, n - 1), 5.0);

        // A degenerate source range is refused rather than dividing by zero.
        let (_, err) = run_attr(
            &before,
            &[("Operation", "Remap"), ("From Min", "1.00"), ("From Max", "1.00")],
        );
        assert!(err.is_some(), "a zero-width source range must be reported");
    }

    #[test]
    fn test_attribute_normalize_measures_before_it_scales() {
        let before = ramped_mass();
        let n = before.num_points();
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();
        let sum = |d: &Detail| (0..n).map(|p| mass(d, p)).sum::<f32>();

        // Unlike Remap, Normalize needs no knowledge of the values — which is
        // what lets it sit in a solve whose range moves every frame.
        let (by_max, err) = run_attr(&before, &[("Operation", "Normalize"), ("Target", "Maximum"), ("To Max", "1.00")]);
        assert!(err.is_none(), "{err:?}");
        assert!((mass(&by_max, n - 1) - 1.0).abs() < 1e-5);

        let (by_sum, _) = run_attr(&before, &[("Operation", "Normalize"), ("Target", "Sum"), ("To Max", "1.00")]);
        assert!((sum(&by_sum) - 1.0).abs() < 1e-4, "sum is {}", sum(&by_sum));

        // An all-zero attribute has no scale to hit, and says so instead of
        // filling the geometry with infinities.
        let mut flat = before.clone();
        flat.points_mut().create("mass", AttribValue::Float(0.0));
        let (_, err) = run_attr(&flat, &[("Operation", "Normalize")]);
        assert!(err.is_some(), "normalizing nothing must be reported");
    }

    #[test]
    fn test_attribute_composite_folds_a_second_attribute_in() {
        let mut before = ramped_mass();
        before.points_mut().create("other", AttribValue::Float(2.0));

        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();
        for (op, want) in [("Multiply", 6.0), ("Add", 5.0), ("Subtract", 1.0), ("Maximum", 3.0)] {
            let (g, err) = run_attr(
                &before,
                &[("Operation", "Composite"), ("Source B", "other"), ("Combine Op", op)],
            );
            assert!(err.is_none(), "{op}: {err:?}");
            assert_eq!(mass(&g, 3), want, "{op}");
        }

        // Dividing by zero yields the numerator rather than an infinity that
        // would poison every later frame of a solve.
        let mut zeroed = before.clone();
        zeroed.points_mut().create("other", AttribValue::Float(0.0));
        let (g, _) = run_attr(
            &zeroed,
            &[("Operation", "Composite"), ("Source B", "other"), ("Combine Op", "Divide")],
        );
        assert_eq!(mass(&g, 3), 3.0);

        let (_, err) = run_attr(
            &before,
            &[("Operation", "Composite"), ("Source B", "nope"), ("Combine Op", "Add")],
        );
        assert!(err.is_some(), "a missing Source B must be reported");
    }

    #[test]
    fn test_analysis_writes_its_answers_to_detail_attributes() {
        let before = ramped_mass();
        let n = before.num_points();
        let mut geom = before.clone();
        let an = node(
            "id-an",
            "An",
            "analysis",
            vec![param("Input", "In"), param("Source", "Attribute"), param("Attribute", "mass")],
            vec![],
        );
        let mut err = None;
        apply_analysis(&mut geom, &an, &mut err);
        assert!(err.is_none(), "{err:?}");

        let d = |name: &str| geom.detail().value(name, 0).unwrap().as_f32();
        // Five ordinary detail attributes where Houdini writes one dictionary.
        // Nothing had to learn to index a dict, and the spreadsheet shows them
        // as `d:` columns for free.
        assert_eq!(d("mass_min"), 0.0);
        assert_eq!(d("mass_max"), (n - 1) as f32);
        assert_eq!(d("mass_count"), n as f32);
        assert_eq!(d("mass_sum"), (0..n).map(|p| p as f32).sum::<f32>());
        assert!((d("mass_average") - d("mass_sum") / n as f32).abs() < 1e-4);
        assert_eq!(d("mass_spread"), d("mass_max") - d("mass_min"));

        // Edge lengths are the same reduction over a different column — the
        // measurement a remesher steers by.
        let mut edges = before.clone();
        let an = node(
            "id-an",
            "An",
            "analysis",
            vec![param("Input", "In"), param("Source", "Edge Lengths")],
            vec![],
        );
        apply_analysis(&mut edges, &an, &mut None);
        assert_eq!(
            edges.detail().value("edges_count", 0).unwrap().as_f32(),
            before.edges().len() as f32
        );
        assert!(edges.detail().value("edges_average", 0).unwrap().as_f32() > 0.0);

        // A missing attribute is reported, and the geometry still passes.
        let mut miss = before.clone();
        let an = node(
            "id-an",
            "An",
            "analysis",
            vec![param("Input", "In"), param("Attribute", "nope")],
            vec![],
        );
        let mut err = None;
        apply_analysis(&mut miss, &an, &mut err);
        assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
    }

    #[test]
    fn test_measure_then_remap_is_the_chain_the_detail_class_exists_for() {
        // Analysis measures, Promote lifts the answer back to points, and
        // Composite divides by it — a normalization that survives a range
        // moving every frame, assembled from the vocabulary rather than
        // hard-coded into a node.
        let mut geom = ramped_mass();
        let n = geom.num_points();

        apply_analysis(
            &mut geom,
            &node("a", "A", "analysis", vec![param("Attribute", "mass")], vec![]),
            &mut None,
        );
        let measured_max = geom.detail().value("mass_max", 0).unwrap().as_f32();
        assert_eq!(measured_max, (n - 1) as f32);

        let (geom, err) = run_attr(
            &geom,
            &[("Operation", "Promote"), ("Attribute Name", "mass_max"), ("To Class", "Point")],
        );
        assert!(err.is_none(), "{err:?}");
        assert_eq!(
            geom.points().value("mass_max", 0),
            Some(AttribValue::Float(measured_max)),
            "the measurement is per-point now"
        );

        let (geom, err) = run_attr(
            &geom,
            &[("Operation", "Composite"), ("Source B", "mass_max"), ("Combine Op", "Divide")],
        );
        assert!(err.is_none(), "{err:?}");
        let mass = |p: usize| geom.points().value("mass", p).unwrap().as_f32();
        assert_eq!(mass(0), 0.0);
        assert!((mass(n - 1) - 1.0).abs() < 1e-5, "{}", mass(n - 1));
    }

    #[test]
    fn test_promote_reduces_points_to_one_detail_row() {
        let before = ramped_mass();
        let n = before.num_points();
        for (method, want) in [
            ("Average", (0..n).map(|p| p as f32).sum::<f32>() / n as f32),
            ("Sum", (0..n).map(|p| p as f32).sum::<f32>()),
            ("Minimum", 0.0),
            ("Maximum", (n - 1) as f32),
            ("First", 0.0),
        ] {
            let (g, err) = run_attr(
                &before,
                &[("Operation", "Promote"), ("To Class", "Detail"), ("Method", method)],
            );
            assert!(err.is_none(), "{method}: {err:?}");
            let got = g.detail().value("mass", 0).unwrap().as_f32();
            assert!((got - want).abs() < 1e-3, "{method}: {got} vs {want}");
            assert_eq!(g.detail().len(), 1, "detail stays one row");
        }
    }

    #[test]
    fn test_time_reads_the_frame_off_the_evaluation() {
        let before = ramped_mass();
        let tn = node(
            "id-t",
            "T",
            "time",
            vec![param("Attribute", "t"), param("Start Frame", "1"), param("End Frame", "11")],
            vec![],
        );
        let t_at = |frame: i32| {
            let mut g = before.clone();
            apply_time(&mut g, &tn, frame);
            g.detail().value("t", 0).unwrap().as_f32()
        };
        assert_eq!(t_at(1), 0.0);
        assert!((t_at(6) - 0.5).abs() < 1e-5);
        assert_eq!(t_at(11), 1.0);
        // Clamped by default, so a scrub past the end holds rather than
        // running the chain off into values it was never shaped for.
        assert_eq!(t_at(50), 1.0);
        assert_eq!(t_at(-20), 0.0);

        // A zero-length range reads as "elapsed", not as a division by zero.
        let degenerate = node(
            "id-t",
            "T",
            "time",
            vec![param("Attribute", "t"), param("Start Frame", "5"), param("End Frame", "5")],
            vec![],
        );
        let mut g = before.clone();
        apply_time(&mut g, &degenerate, 5);
        assert_eq!(g.detail().value("t", 0).unwrap().as_f32(), 1.0);
        apply_time(&mut g, &degenerate, 4);
        assert_eq!(g.detail().value("t", 0).unwrap().as_f32(), 0.0);
    }

    fn run_vis(before: &Detail, params: &[(&str, &str)]) -> (Detail, Option<String>) {
        let mut geom = before.clone();
        let mut ps = vec![param("Input", "In"), param("Attribute", "mass")];
        for (k, v) in params {
            match ps.iter_mut().find(|p| p.name == *k) {
                Some(p) => p.default = v.to_string(),
                None => ps.push(param(k, v)),
            }
        }
        let vn = node("id-v", "V", "visualize", ps, vec![]);
        let mut err = None;
        apply_visualize(&mut geom, &vn, &mut err);
        (geom, err)
    }

    #[test]
    fn test_ramps_run_end_to_end_and_clamp_outside_it() {
        for name in ["grayscale", "heat", "spectrum", "viridis"] {
            let (lo, hi) = (ramp_color(name, 0.0), ramp_color(name, 1.0));
            assert_ne!(lo, hi, "{name} goes nowhere");
            assert_eq!(ramp_color(name, -5.0), lo, "{name} clamps below");
            assert_eq!(ramp_color(name, 5.0), hi, "{name} clamps above");
            // Continuous: a small step in t is a small step in colour, or the
            // picture would show banding that is not in the data.
            let a = ramp_color(name, 0.5);
            let b = ramp_color(name, 0.51);
            let d: f32 = (0..3).map(|i| (a[i] - b[i]).abs()).sum();
            assert!(d < 0.1, "{name} jumps at the midpoint: {a:?} -> {b:?}");
        }
        assert_eq!(ramp_color("grayscale", 0.0), [0.0; 3]);
        assert_eq!(ramp_color("grayscale", 1.0), [1.0; 3]);
        assert_eq!(ramp_color("grayscale", 0.5), [0.5; 3]);
    }

    #[test]
    fn test_visualize_auto_range_tracks_the_attribute_every_time_it_runs() {
        let mut before = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        let n = before.num_points();
        before.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..n {
            before.points_mut().set_value("mass", p, AttribValue::Float(p as f32)).unwrap();
        }

        let (g, err) = run_vis(&before, &[("Ramp", "Grayscale"), ("Range", "Auto")]);
        assert!(err.is_none(), "{err:?}");
        // The measured range spreads across the whole ramp regardless of what
        // the numbers happen to be — which is the setting a simulation wants,
        // because the interesting range moves every frame.
        assert_eq!(g.color(0), [0.0; 3]);
        assert_eq!(g.color(n - 1), [1.0; 3]);

        // Scale every value by ten and the picture is identical: Auto is about
        // the shape of the data, not its units.
        let mut scaled = before.clone();
        for p in 0..n {
            scaled.points_mut().set_value("mass", p, AttribValue::Float(p as f32 * 10.0)).unwrap();
        }
        let (h, _) = run_vis(&scaled, &[("Ramp", "Grayscale"), ("Range", "Auto")]);
        for p in 0..n {
            assert_eq!(g.color(p), h.color(p), "point {p}");
        }

        // A flat attribute has no range to spread; everything at the bottom is
        // the honest picture of "nothing varies here", not a division by zero.
        let mut flat = before.clone();
        flat.points_mut().create("mass", AttribValue::Float(3.0));
        let (f, err) = run_vis(&flat, &[("Ramp", "Grayscale"), ("Range", "Auto")]);
        assert!(err.is_none(), "{err:?}");
        assert_eq!(f.color(0), [0.0; 3]);
    }

    #[test]
    fn test_visualize_nodes_composite_by_chaining() {
        let mut before = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        let n = before.num_points();
        before.points_mut().create("mass", AttribValue::Float(1.0));
        before.points_mut().create("heat", AttribValue::Float(0.0));
        for p in 0..n {
            before.points_mut().set_value("heat", p, AttribValue::Float(p as f32)).unwrap();
        }

        // Set lays down a base; a second node blends over it. Several
        // attributes read at once come from STACKING nodes, not from one node
        // growing a list of layers — so any one of them can be bypassed to see
        // what it was contributing.
        let (base, _) = run_vis(&before, &[("Ramp", "Grayscale"), ("Blend", "Set")]);
        assert_eq!(base.color(0), [0.0; 3], "a flat attribute floors the ramp");

        let (over, _) = run_vis(&base, &[("Attribute", "heat"), ("Ramp", "Grayscale"), ("Blend", "Set"), ("Opacity", "0.50")]);
        // Half-strength Set is a half-way mix with what was already there.
        assert!((over.color(n - 1)[0] - 0.5).abs() < 1e-5, "{:?}", over.color(n - 1));

        // Opacity 0 changes nothing at all, whatever the blend.
        for blend in ["Set", "Multiply", "Add"] {
            let (none, _) = run_vis(&base, &[("Attribute", "heat"), ("Blend", blend), ("Opacity", "0.00")]);
            for p in 0..n {
                assert_eq!(none.color(p), base.color(p), "{blend} at zero opacity, point {p}");
            }
        }

        // Multiply darkens toward the ramp, Add brightens away from it.
        let mid = run_vis(&before, &[("Ramp", "Grayscale"), ("Range", "Manual"), ("From", "0.00"), ("To", "2.00")]).0;
        let (mul, _) = run_vis(&mid, &[("Attribute", "heat"), ("Ramp", "Grayscale"), ("Blend", "Multiply")]);
        let (add, _) = run_vis(&mid, &[("Attribute", "heat"), ("Ramp", "Grayscale"), ("Blend", "Add")]);
        assert!(mul.color(0)[0] <= mid.color(0)[0] + 1e-6);
        assert!(add.color(n - 1)[0] >= mid.color(n - 1)[0] - 1e-6);
    }

    #[test]
    fn test_visualize_vector_mode_stages_markers_the_viewport_can_draw() {
        let mut before = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        before.points_mut().create("vel", AttribValue::Float3([0.0, 4.0, 0.0]));

        let (g, err) = run_vis(&before, &[("Attribute", "vel"), ("Mode", "Vector"), ("Scale", "0.25")]);
        assert!(err.is_none(), "{err:?}");
        let vis = format!("{}vel", crate::detail::VIS_PREFIX);
        assert!(g.points().has(&vis), "the marker request rides the geometry");
        assert_eq!(g.points().value(&vis, 0), Some(AttribValue::Float3([0.0, 1.0, 0.0])));

        // A marker describes the state it was made from, so it must not
        // survive into a step that has not remade it.
        assert_eq!(g.points().kind(&vis), crate::detail::AttribKind::Derivative);

        // Outside a group, no marker — so a Visualize can annotate part of a
        // surface without drawing over the rest.
        let mut grouped = before.clone();
        grouped.points_mut().create_group("some");
        grouped.points_mut().add_to_group("some", 2);
        let (h, _) = run_vis(&grouped, &[("Attribute", "vel"), ("Mode", "Vector"), ("Group", "some")]);
        assert_ne!(h.points().value(&vis, 2), Some(AttribValue::Float3([0.0; 3])));
        assert_eq!(h.points().value(&vis, 0), Some(AttribValue::Float3([0.0; 3])));
    }

    #[test]
    fn test_vis_markers_become_line_pairs_in_the_points_own_colour() {
        let mut d = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        d.points_mut().create("vel", AttribValue::Float3([0.0, 1.0, 0.0]));
        // Only two points get a marker; the rest stage a zero vector.
        let (mut g, _) = run_vis(&d, &[("Attribute", "vel"), ("Mode", "Vector"), ("Scale", "0.50")]);
        let vis = format!("{}vel", crate::detail::VIS_PREFIX);
        for p in 2..g.num_points() {
            g.points_mut().set_value(&vis, p, AttribValue::Float3([0.0; 3])).unwrap();
        }
        g.set_color(0, [1.0, 0.0, 0.0]);

        let verts = vis_marker_vertices(&g, |c| c);
        // One PAIR per marker — a zero vector draws nothing rather than a
        // degenerate segment at the point.
        assert_eq!(verts.len(), 4);
        assert_eq!(verts[0].position, g.positions()[0]);
        assert_eq!(verts[1].position, (g.pos(0) + Vec3::new(0.0, 0.5, 0.0)).to_array());
        assert_eq!(verts[0].color, [1.0, 0.0, 0.0], "markers take the point's colour");
        assert_eq!(verts[1].color, verts[0].color);

        // Geometry nobody asked to visualize draws nothing at all.
        assert!(vis_marker_vertices(&d, |c| c).is_empty());
    }

    #[test]
    fn test_an_opencl_deformer_lands_its_named_attribute_on_the_geometry() {
        // The gap between "parse_attr_refs reads raw source" and "run the
        // kernel with buffers handed to it": run_kernel_on_detail was reading
        // the names off the PROCESSED source, where every attrf() call has
        // already become attr_0[], so it bound no buffers and the attribute
        // silently never appeared. Only an end-to-end evaluation catches that.
        let kernel = r#"
            __kernel void process(__global float* pos, __global float* col, int count) {
                int id = get_global_id(0);
                if (id < count) {
                    setattrf("mass", id, pos[id * 3 + 1] * 2.0f);
                }
            }
        "#;
        let sphere = node("id-s", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let dfm = node(
            "id-k",
            "Height 1",
            "opencl",
            vec![param("Input", "Sphere 1"), param("Code", kernel)],
            vec![],
        );
        let root = node("id-root", "root", "node", vec![], vec![sphere, dfm]);

        let mut visited = Vec::new();
        let mut err = None;
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(0, 0, &mut cache);
        let g = generate_single_node_geometry_with_errors(
            &root,
            &root.children[1],
            &mut visited,
            &mut err,
            &mut sim,
        )
        .expect("the deformer evaluates");
        assert!(err.is_none(), "{err:?}");

        assert!(g.points().has("mass"), "the kernel's named attribute must reach the geometry");
        for p in 0..g.num_points() {
            let want = g.positions()[p][1] * 2.0;
            let got = g.points().value("mass", p).unwrap().as_f32();
            assert!((got - want).abs() < 1e-4, "point {p}: {got} vs {want}");
        }
    }

    #[test]
    fn test_visualize_reports_a_missing_attribute_and_leaves_colour_alone() {
        let before = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        let (g, err) = run_vis(&before, &[("Attribute", "nope")]);
        assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
        for p in 0..before.num_points() {
            assert_eq!(g.color(p), before.color(p), "point {p}");
        }
    }

    /// A sphere, one point given a spike of `mass`, then a Neighbour node.
    /// Returns (before, after) so a test can compare the two directly.
    fn neighbour_chain(extra: &[(&str, &str)]) -> (Detail, Detail) {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let seed = node(
            "id-seed",
            "Seed 1",
            "attribute",
            vec![
                param("Input", "Sphere 1"),
                param("Operation", "Create"),
                param("Attribute Name", "mass"),
                param("Type", "Float"),
                param("Value", "0.00"),
            ],
            vec![],
        );
        let mut params = vec![
            param("Input", "Seed 1"),
            param("Attribute", "mass"),
            param("Amount", "0.50"),
        ];
        for (k, v) in extra {
            match params.iter_mut().find(|p| p.name == *k) {
                Some(p) => p.default = v.to_string(),
                None => params.push(param(k, v)),
            }
        }
        let nbr = node("id-nbr", "Neighbour 1", "neighbour", params, vec![]);
        let root = node("id-root", "root", "node", vec![], vec![sphere, seed, nbr]);

        let eval = |name: &str| -> Detail {
            let target = root.children.iter().find(|c| c.name == name).unwrap();
            let mut visited = Vec::new();
            let mut err = None;
            let mut cache = SimCache::default();
            let mut sim = EvalSim::new(0, 0, &mut cache);
            generate_single_node_geometry_with_errors(&root, target, &mut visited, &mut err, &mut sim)
                .expect(name)
        };
        // Spike one point by hand: the interesting behaviour is what happens
        // to a value that is not already uniform.
        let mut before = eval("Seed 1");
        before.points_mut().set_value("mass", 0, AttribValue::Float(10.0)).unwrap();
        (before, eval("Neighbour 1"))
    }

    /// Evaluate a Neighbour node over geometry whose `mass` is already spiked,
    /// bypassing the graph so the spike survives.
    fn run_neighbour(before: &Detail, params: &[(&str, &str)]) -> Detail {
        let mut geom = before.clone();
        let mut params_vec = vec![param("Input", "In"), param("Attribute", "mass")];
        for (k, v) in params {
            match params_vec.iter_mut().find(|p| p.name == *k) {
                Some(p) => p.default = v.to_string(),
                None => params_vec.push(param(k, v)),
            }
        }
        let nbr = node("id-n", "N", "neighbour", params_vec, vec![]);
        apply_neighbour(&mut geom, &nbr, &mut None);
        geom
    }

    #[test]
    fn test_neighbour_diffuse_spreads_a_spike_and_conserves_nothing_in_particular() {
        let (before, _) = neighbour_chain(&[]);
        let spike_nbrs: Vec<u32> = before.point_neighbours(0).to_vec();
        assert!(!spike_nbrs.is_empty());

        let after = run_neighbour(&before, &[("Mode", "Diffuse"), ("Amount", "0.50")]);
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();

        // The spike falls toward its neighbours' average (zero) by Amount, and
        // each neighbour rises toward an average that now includes the spike.
        assert!((mass(&after, 0) - 5.0).abs() < 1e-4, "spike did not decay: {}", mass(&after, 0));
        for &q in &spike_nbrs {
            assert!(mass(&after, q as usize) > 0.0, "neighbour {q} did not receive");
        }
        // A point far from the spike is untouched at one ring.
        let far = (0..before.num_points())
            .find(|&p| p != 0 && !spike_nbrs.contains(&(p as u32)) && !before.point_neighbours(p).contains(&0))
            .unwrap();
        assert_eq!(mass(&after, far), 0.0, "diffusion reached past its ring");
    }

    #[test]
    fn test_neighbour_concentrate_is_diffuse_with_the_sign_flipped() {
        let (before, _) = neighbour_chain(&[]);
        let diffused = run_neighbour(&before, &[("Mode", "Diffuse"), ("Amount", "0.40")]);
        let sharpened = run_neighbour(&before, &[("Mode", "Concentrate"), ("Amount", "0.40")]);
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();

        // Same distance from the starting value, opposite directions.
        for p in 0..before.num_points() {
            let base = mass(&before, p);
            let d = mass(&diffused, p) - base;
            let c = mass(&sharpened, p) - base;
            assert!((d + c).abs() < 1e-4, "point {p}: {d} vs {c}");
        }
        assert!(mass(&sharpened, 0) > mass(&before, 0), "the spike must sharpen");
    }

    #[test]
    fn test_neighbour_migrate_conserves_the_total() {
        let (mut before, _) = neighbour_chain(&[]);
        // Everything flows one way.
        before.points_mut().create("dir", AttribValue::Float3([0.0, 1.0, 0.0]));
        for p in 0..before.num_points() {
            before.points_mut().set_value("mass", p, AttribValue::Float(1.0)).unwrap();
        }
        let total = |d: &Detail| -> f32 {
            (0..d.num_points()).map(|p| d.points().value("mass", p).unwrap().as_f32()).sum()
        };
        let sum_before = total(&before);

        let after = run_neighbour(
            &before,
            &[("Mode", "Migrate"), ("Direction", "dir"), ("Amount", "0.50")],
        );

        // The sender loses exactly what the receivers gain — that is what makes
        // this transport rather than growth, and it is the property a tissue
        // sim leans on when it moves a nutrient around a surface.
        assert!(
            (total(&after) - sum_before).abs() < 1e-3,
            "migrate leaked: {} -> {}",
            sum_before,
            total(&after)
        );
        // And it actually moved: the topmost point, with nothing above it to
        // give to, should have gained without giving.
        let top = (0..before.num_points())
            .max_by(|&a, &b| before.pos(a).y.partial_cmp(&before.pos(b).y).unwrap())
            .unwrap();
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();
        assert!(mass(&after, top) > mass(&before, top), "nothing accumulated downstream");
    }

    #[test]
    fn test_neighbour_bleed_decays_toward_zero_and_ignores_the_hood() {
        let (before, _) = neighbour_chain(&[]);
        let after = run_neighbour(&before, &[("Mode", "Bleed"), ("Amount", "0.25")]);
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();
        assert!((mass(&after, 0) - 7.5).abs() < 1e-4);
        // Applying it repeatedly approaches zero without crossing it.
        let mut g = before.clone();
        for _ in 0..40 {
            g = run_neighbour(&g, &[("Mode", "Bleed"), ("Amount", "0.25")]);
        }
        assert!(mass(&g, 0) > 0.0 && mass(&g, 0) < 1e-3, "{}", mass(&g, 0));
    }

    #[test]
    fn test_neighbour_hoods_differ_and_rings_reach_further() {
        let (before, _) = neighbour_chain(&[]);
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();
        let touched = |d: &Detail| (0..d.num_points()).filter(|&p| mass(d, p) != 0.0).count();

        let one = run_neighbour(&before, &[("Mode", "Diffuse"), ("Neighbourhood", "Connectivity"), ("Rings", "1")]);
        let two = run_neighbour(&before, &[("Mode", "Diffuse"), ("Neighbourhood", "Connectivity"), ("Rings", "2")]);
        assert!(touched(&two) > touched(&one), "a second ring must reach further");

        // Global reaches everything: every point but the spike rises off zero.
        let global = run_neighbour(&before, &[("Mode", "Diffuse"), ("Neighbourhood", "Global"), ("Amount", "1.00")]);
        assert_eq!(touched(&global), before.num_points() - 1);
        // The spike lands on exactly zero, because a point is not its own
        // neighbour and every OTHER point holds zero.
        assert_eq!(mass(&global, 0), 0.0);

        // Radius ignores connectivity, and the rules are continuous with each
        // other: a radius wide enough to swallow the sphere IS Global. That
        // only holds because neither includes the point itself.
        let wide = run_neighbour(&before, &[("Mode", "Diffuse"), ("Neighbourhood", "Radius"), ("Radius", "5.00"), ("Amount", "1.00")]);
        for p in 0..before.num_points() {
            assert!((mass(&wide, p) - mass(&global, p)).abs() < 1e-6, "point {p}");
        }
        let none = run_neighbour(&before, &[("Mode", "Diffuse"), ("Neighbourhood", "Radius"), ("Radius", "0.00")]);
        assert_eq!(touched(&none), 1, "no neighbours means no change");
    }

    #[test]
    fn test_neighbour_group_narrows_the_edit_not_the_reading() {
        let (mut before, _) = neighbour_chain(&[]);
        // Only the spike's first neighbour may be edited.
        let q = before.point_neighbours(0)[0] as usize;
        before.points_mut().create_group("inner");
        before.points_mut().add_to_group("inner", q);

        let after = run_neighbour(&before, &[("Mode", "Diffuse"), ("Group", "inner"), ("Amount", "1.00")]);
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();

        assert_eq!(mass(&after, 0), 10.0, "a point outside the group is not edited");
        // But the edited point still READ the spike, which is outside the
        // group — a diffusion that could only see its own group would bend
        // away from the boundary instead of across it.
        assert!(mass(&after, q) > 0.0, "the group member saw its neighbour outside the group");
    }

    #[test]
    fn test_neighbour_works_componentwise_on_vectors_and_keeps_integers_whole() {
        let (before, _) = neighbour_chain(&[]);
        let mut g = before.clone();
        g.points_mut().create("vel", AttribValue::Float3([0.0; 3]));
        g.points_mut().set_value("vel", 0, AttribValue::Float3([3.0, 6.0, 9.0])).unwrap();
        g.points_mut().create("count", AttribValue::Int(0));
        g.points_mut().set_value("count", 0, AttribValue::Int(10)).unwrap();

        let v = run_neighbour(&g, &[("Attribute", "vel"), ("Mode", "Bleed"), ("Amount", "0.50")]);
        assert_eq!(
            v.points().value("vel", 0),
            Some(AttribValue::Float3([1.5, 3.0, 4.5])),
            "every component decays alike"
        );

        let c = run_neighbour(&g, &[("Attribute", "count"), ("Mode", "Bleed"), ("Amount", "0.25")]);
        // An integer count stays an integer: 10 * 0.75 = 7.5 rounds rather
        // than silently becoming a float nobody can index with.
        assert!(matches!(c.points().value("count", 0), Some(AttribValue::Int(_))));
        assert_eq!(c.points().value("count", 0), Some(AttribValue::Int(7)));
    }

    /// A sphere carrying a `vel` vector attribute, all pointing +Y except
    /// point 0, which points -Y and so disagrees with everyone.
    fn vectored() -> Detail {
        let mut d = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        d.points_mut().create("vel", AttribValue::Float3([0.0, 2.0, 0.0]));
        d.points_mut()
            .set_value("vel", 0, AttribValue::Float3([0.0, -3.0, 0.0]))
            .unwrap();
        d
    }

    fn vel(d: &Detail, p: usize) -> Vec3 {
        d.points().value("vel", p).unwrap().as_vec3()
    }

    #[test]
    fn test_align_turns_a_vector_without_changing_its_length() {
        let before = vectored();
        // The dissenting vector is surrounded by +Y, so aligning to the local
        // average turns it around.
        let after = run_neighbour(
            &before,
            &[("Attribute", "vel"), ("Mode", "Align"), ("Target", "Local Average"), ("Amount", "1.00")],
        );
        assert!(vel(&after, 0).y > 0.0, "the odd one out did not turn: {:?}", vel(&after, 0));
        // Steering is a statement about heading only: a vector attribute
        // carries a direction AND a strength, and Align must not spend the
        // strength.
        assert!(
            (vel(&after, 0).length() - 3.0).abs() < 1e-4,
            "length changed: {}",
            vel(&after, 0).length()
        );
        for p in 1..before.num_points() {
            assert!((vel(&after, p).length() - 2.0).abs() < 1e-4, "point {p}");
        }
    }

    #[test]
    fn test_align_targets_are_five_different_answers_to_agree_with_what() {
        let before = vectored();
        let run = |extra: &[(&str, &str)]| {
            let mut params = vec![("Attribute", "vel"), ("Mode", "Align"), ("Amount", "1.00")];
            params.extend_from_slice(extra);
            run_neighbour(&before, &params)
        };

        // Constant: everyone ends up pointing the same way.
        let c = run(&[("Target", "Constant"), ("Constant", "1.00:0.00:0.00")]);
        for p in 0..before.num_points() {
            assert!((vel(&c, p).normalize() - Vec3::X).length() < 1e-4, "point {p}");
        }

        // Attribute: steer toward another vector attribute.
        let mut with_goal = before.clone();
        with_goal.points_mut().create("goal", AttribValue::Float3([0.0, 0.0, 1.0]));
        let a = run_neighbour(
            &with_goal,
            &[("Attribute", "vel"), ("Mode", "Align"), ("Amount", "1.00"), ("Target", "Attribute"), ("Source", "goal")],
        );
        assert!((vel(&a, 5).normalize() - Vec3::Z).length() < 1e-4);

        // Surface tangent: the result lies in the surface, so it is
        // perpendicular to the point's normal.
        let t = run(&[("Target", "Surface Tangent")]);
        let normals = point_normals(&before);
        let mut turned = 0usize;
        let mut left_alone = 0usize;
        for p in 0..before.num_points() {
            let n = normals[p];
            if n == Vec3::ZERO {
                continue;
            }
            // A vector already parallel to the normal has NO tangent to steer
            // onto: its projection into the surface is zero. Such a point is
            // left as it was — the alternative is normalizing a vector made
            // entirely of float error and calling the result a heading.
            if vel(&before, p).normalize().dot(n).abs() > 0.999 {
                assert_eq!(vel(&t, p), vel(&before, p), "point {p} was given a direction out of noise");
                left_alone += 1;
                continue;
            }
            assert!(
                vel(&t, p).normalize().dot(n).abs() < 1e-3,
                "point {p} is not tangent: {:?} vs normal {:?}",
                vel(&t, p),
                n
            );
            turned += 1;
        }
        assert!(turned > 0 && left_alone > 0, "{turned} turned, {left_alone} left alone");

        // Global average excludes the point itself, exactly as the
        // neighbourhoods do — otherwise the dissenter would average partly
        // with itself and could never be turned all the way.
        let g = run(&[("Target", "Global Average")]);
        assert!(vel(&g, 0).y > 0.0);
    }

    #[test]
    fn test_align_leaves_a_vector_it_cannot_turn_alone() {
        let before = vectored();
        // Steering onto the exact opposite has no shortest arc, and the
        // midpoint of the blend is the zero vector. Half-way must not
        // annihilate it.
        let after = run_neighbour(
            &before,
            &[("Attribute", "vel"), ("Mode", "Align"), ("Target", "Constant"), ("Constant", "0.00:-1.00:0.00"), ("Amount", "0.50")],
        );
        assert!((vel(&after, 5).length() - 2.0).abs() < 1e-4, "{:?}", vel(&after, 5));
        assert_ne!(vel(&after, 5), Vec3::ZERO);

        // A scalar attribute has no direction to steer, and says so.
        let mut err = None;
        let mut g = before.clone();
        let nd = node(
            "id-n",
            "N",
            "neighbour",
            vec![param("Attribute", "mass"), param("Mode", "Align")],
            vec![],
        );
        g.points_mut().create("mass", AttribValue::Float(1.0));
        apply_neighbour(&mut g, &nd, &mut err);
        assert!(err.as_deref().unwrap_or("").contains("scalar"), "{err:?}");
    }

    #[test]
    fn test_lead_follows_the_neighbour_that_disagrees_most() {
        let before = vectored();
        let spread = before.point_neighbours(0).to_vec();
        assert!(!spread.is_empty());

        let after = run_neighbour(
            &before,
            &[("Attribute", "vel"), ("Mode", "Lead"), ("Amount", "1.00")],
        );
        // The dissenter's neighbours each see one vector that disagrees with
        // them — point 0's — so they turn onto it. That is the mechanism:
        // the dissenter LEADS, which is what propagates a reorientation
        // across a surface instead of settling it on the spot.
        for &q in &spread {
            assert!(vel(&after, q as usize).y < 0.0, "neighbour {q} did not follow the dissenter");
            assert!((vel(&after, q as usize).length() - 2.0).abs() < 1e-4);
        }
        // Point 0's own neighbours all agree with each other, so the most
        // disagreeable one among them is still +Y, and it turns to match.
        assert!(vel(&after, 0).y > 0.0);

        // An influence attribute weights the contest: silencing the dissenter
        // leaves its neighbours alone.
        let mut weighted = before.clone();
        weighted.points_mut().create("clout", AttribValue::Float(1.0));
        weighted.points_mut().set_value("clout", 0, AttribValue::Float(0.0)).unwrap();
        let quiet = run_neighbour(
            &weighted,
            &[("Attribute", "vel"), ("Mode", "Lead"), ("Amount", "1.00"), ("Source", "clout")],
        );
        for &q in &spread {
            assert!(vel(&quiet, q as usize).y > 0.0, "a silenced dissenter still led {q}");
        }
    }

    #[test]
    fn test_charge_accumulates_then_discharges_to_its_neighbours() {
        let mut before = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        before.points_mut().create("mass", AttribValue::Float(0.0));
        let n = before.num_points();
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();
        let total = |d: &Detail| (0..n).map(|p| mass(d, p)).sum::<f32>();

        // Below the threshold it simply fills.
        let charged = run_neighbour(&before, &[("Mode", "Charge"), ("Amount", "0.30"), ("Release", "1.00")]);
        assert!((mass(&charged, 0) - 0.3).abs() < 1e-5);
        assert!((total(&charged) - 0.3 * n as f32).abs() < 1e-3);

        // Crossing it empties the point into its neighbours, conserving what
        // was stored — the discharge moves value, only the accumulation makes
        // it.
        let mut primed = before.clone();
        for p in 0..n {
            primed.points_mut().set_value("mass", p, AttribValue::Float(0.9)).unwrap();
        }
        let fired = run_neighbour(&primed, &[("Mode", "Charge"), ("Amount", "0.20"), ("Release", "1.00")]);
        let expected_after_fill = total(&primed) + 0.2 * n as f32;
        assert!(
            (total(&fired) - expected_after_fill).abs() < 1e-2,
            "discharge leaked: {} vs {}",
            total(&fired),
            expected_after_fill
        );

        // Every point fired at once, so each emptied completely and holds
        // exactly the shares its neighbours sent it — nothing of its own.
        // Note that this is not 1.1 back again: a point whose neighbours have
        // few neighbours of their own receives larger shares than it sent, and
        // that redistribution is the point of the mode.
        for p in 0..n {
            let expected: f32 = primed
                .point_neighbours(p)
                .iter()
                .map(|&q| 1.1 / primed.point_neighbours(q as usize).len() as f32)
                .sum();
            assert!(
                (mass(&fired, p) - expected).abs() < 1e-3,
                "point {p}: {} vs {expected}",
                mass(&fired, p)
            );
        }
    }

    #[test]
    fn test_charge_fires_from_a_snapshot_so_point_order_cannot_matter() {
        // One point primed to fire, its neighbours empty. If firing were
        // decided while writing, a neighbour that received the discharge could
        // cross the threshold and fire in the same pass — and whether it did
        // would depend on which index it happened to have.
        let mut before = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        before.points_mut().create("mass", AttribValue::Float(0.0));
        before.points_mut().set_value("mass", 0, AttribValue::Float(5.0)).unwrap();
        let mass = |d: &Detail, p: usize| d.points().value("mass", p).unwrap().as_f32();

        let after = run_neighbour(&before, &[("Mode", "Charge"), ("Amount", "0.00"), ("Release", "1.00")]);
        assert_eq!(mass(&after, 0), 0.0, "the primed point emptied");
        let nbrs = before.point_neighbours(0).to_vec();
        let share = 5.0 / nbrs.len() as f32;
        for &q in &nbrs {
            // Each neighbour holds exactly its share and did not itself fire,
            // even though the share is over the threshold.
            assert!((mass(&after, q as usize) - share).abs() < 1e-4, "neighbour {q}");
        }
    }

    #[test]
    fn test_neighbour_reports_a_missing_attribute_and_passes_geometry_through() {
        let (before, _) = neighbour_chain(&[]);
        let mut err = None;
        let mut geom = before.clone();
        let nbr = node(
            "id-n",
            "N",
            "neighbour",
            vec![param("Input", "In"), param("Attribute", "nope")],
            vec![],
        );
        apply_neighbour(&mut geom, &nbr, &mut err);
        assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
        assert_eq!(geom.num_points(), before.num_points(), "geometry passes through");
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

    /// A sim whose step adds 1 to `acc` every frame. The seed declares `acc`
    /// with the given kind, which is the only difference between the two runs.
    fn accumulating_graph(kind: &str) -> FsNode {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let seed = node(
            "id-seed",
            "Seed 1",
            "attribute",
            vec![
                param("Input", "Sphere 1"),
                param("Operation", "Create"),
                param("Attribute Name", "acc"),
                param("Type", "Float"),
                param("Value", "0.00"),
                param("Kind", kind),
            ],
            vec![],
        );
        let inner_input = node("id-in", "input1", "input", vec![], vec![]);
        let step = node(
            "id-step",
            "step1",
            "attribute",
            vec![
                param("Input", "input1"),
                param("Operation", "Modify"),
                param("Attribute Name", "acc"),
                param("Combine", "Add"),
                param("Value", "1.00"),
            ],
            vec![],
        );
        let inner_output = node("id-out", "output1", "output", vec![param("Input", "step1")], vec![]);
        let sim = node(
            "id-sim",
            "Simnet 1",
            "simnet",
            vec![param("Input", "Seed 1")],
            vec![inner_input, step, inner_output],
        );
        node("id-root", "root", "node", vec![], vec![sphere, seed, sim])
    }

    /// A sim that adds a fixed 1.0 to `acc` every time the chain runs.
    fn substep_graph(substeps: &str) -> FsNode {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let seed = node(
            "id-seed",
            "Seed 1",
            "attribute",
            vec![
                param("Input", "Sphere 1"),
                param("Operation", "Create"),
                param("Attribute Name", "acc"),
                param("Type", "Float"),
                param("Value", "0.00"),
            ],
            vec![],
        );
        let inner_input = node("id-in", "input1", "input", vec![], vec![]);
        let step = node(
            "id-step",
            "step1",
            "attribute",
            vec![
                param("Input", "input1"),
                param("Operation", "Modify"),
                param("Attribute Name", "acc"),
                param("Combine", "Add"),
                param("Value", "1.00"),
            ],
            vec![],
        );
        let inner_output = node("id-out", "output1", "output", vec![param("Input", "step1")], vec![]);
        let sim = node(
            "id-sim",
            "Simnet 1",
            "simnet",
            vec![param("Input", "Seed 1"), param("Substeps", substeps)],
            vec![inner_input, step, inner_output],
        );
        node("id-root", "root", "node", vec![], vec![sphere, seed, sim])
    }

    /// The same sim, but the step adds `dt` instead of a fixed amount — the
    /// chain assembled out of the vocabulary: Promote lifts the solver's `dt`
    /// detail attribute onto points, Composite adds it into `acc`.
    fn substep_dt_graph(substeps: &str) -> FsNode {
        let mut root = substep_graph(substeps);
        let sim = root.children.iter_mut().find(|c| c.node_type == "simnet").unwrap();
        let promote = node(
            "id-prom",
            "promote1",
            "attribute",
            vec![
                param("Input", "input1"),
                param("Operation", "Promote"),
                param("Attribute Name", "dt"),
                param("To Class", "Point"),
            ],
            vec![],
        );
        let step = node(
            "id-step",
            "step1",
            "attribute",
            vec![
                param("Input", "promote1"),
                param("Operation", "Composite"),
                param("Attribute Name", "acc"),
                param("Source B", "dt"),
                param("Combine Op", "Add"),
            ],
            vec![],
        );
        sim.children = vec![
            node("id-in", "input1", "input", vec![], vec![]),
            promote,
            step,
            node("id-out", "output1", "output", vec![param("Input", "step1")], vec![]),
        ];
        root
    }

    #[test]
    fn test_a_simnet_can_start_later_than_the_timeline() {
        let with_start = |start: &str| {
            let mut root = substep_graph("1");
            let sim = root.children.iter_mut().find(|c| c.node_type == "simnet").unwrap();
            sim.params.push(param("Start Frame", start));
            root
        };
        let acc = |root: &FsNode, frame: i32| -> f32 {
            solve_at(root, frame).points().value("acc", 0).unwrap().as_f32()
        };

        // Empty follows the timeline, which is what every sim did before this
        // parameter existed.
        assert_eq!(acc(&with_start(""), 4), 3.0);

        // A number decouples when the simulation starts from when the shot
        // does, so two sims in one scene can begin at different times.
        let late = with_start("5");
        assert_eq!(acc(&late, 5), 0.0, "its own start frame is its seed");
        assert_eq!(acc(&late, 8), 3.0);
        // Before it starts is not negative time, exactly as before the
        // timeline's start was not.
        assert_eq!(acc(&late, 2), 0.0);
    }

    #[test]
    fn test_a_detail_round_trips_through_its_binary_form() {
        let mut d = sphere_detail(Vec3::new(0.1, 0.2, 0.3), 0.7, 4, 6);
        d.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..d.num_points() {
            d.points_mut().set_value("mass", p, AttribValue::Float(p as f32 * 0.25)).unwrap();
        }
        d.points_mut().create_kind("scratch", AttribValue::Int(3), crate::detail::AttribKind::Derivative);
        d.points_mut().create_group("pinned");
        d.points_mut().add_to_group("pinned", 2);
        d.prims_mut().create("area", AttribValue::Float(1.5));
        d.detail_mut().create("dt", AttribValue::Float(0.25));
        d.verts_mut().create("uv", AttribValue::Float2([0.5, 0.25]));

        let blob = d.to_bytes();
        let back = Detail::from_bytes(&blob).expect("round trip");

        assert_eq!(back.num_points(), d.num_points());
        assert_eq!(back.num_prims(), d.num_prims());
        assert_eq!(back.num_verts(), d.num_verts());
        assert_eq!(back.positions(), d.positions());
        // Identity is the whole reason a solver state is worth storing: a
        // resumed sim that renumbered its points would be a different sim.
        assert_eq!(back.ids(), d.ids());
        assert_eq!(back.points().value("mass", 5), d.points().value("mass", 5));
        assert_eq!(back.points().value("scratch", 0), Some(AttribValue::Int(3)));
        assert_eq!(back.points().kind("scratch"), crate::detail::AttribKind::Derivative);
        assert_eq!(back.points().kind("mass"), crate::detail::AttribKind::Live);
        assert_eq!(back.points().group_members("pinned"), vec![2]);
        assert_eq!(back.prims().value("area", 0), Some(AttribValue::Float(1.5)));
        assert_eq!(back.detail().value("dt", 0), Some(AttribValue::Float(0.25)));
        assert_eq!(back.verts().value("uv", 0), Some(AttribValue::Float2([0.5, 0.25])));
        // Topology is rebuilt rather than stored, so it cannot disagree with
        // the primitives it came from.
        assert_eq!(back.edges(), d.edges());

        // A point added after a resume must not reuse an identity.
        let mut back = back;
        let fresh = back.add_point(Vec3::ZERO);
        assert!(!d.ids().contains(&back.id(fresh as usize).unwrap()));
    }

    #[test]
    fn test_a_corrupt_cache_blob_is_an_error_not_a_panic() {
        let good = sphere_detail(Vec3::ZERO, 0.5, 4, 6).to_bytes();
        assert!(Detail::from_bytes(b"").is_err(), "empty");
        assert!(Detail::from_bytes(b"not a detail at all").is_err(), "wrong magic");
        // Truncated at every length: a cache lives in a directory anything can
        // write to, and half a file must never reach the code that indexes on
        // its counts.
        for cut in (0..good.len()).step_by(7) {
            let _ = Detail::from_bytes(&good[..cut]);
        }
        let mut wrong_type = good.clone();
        // Corrupt a byte in the middle and it either errors or reads as
        // something harmless; what it must not do is panic.
        for i in (8..good.len()).step_by(101) {
            wrong_type[i] = 0xff;
            let _ = Detail::from_bytes(&wrong_type);
            wrong_type[i] = good[i];
        }
    }

    #[test]
    fn test_the_disk_cache_resumes_only_the_solve_it_belongs_to() {
        let dir = std::env::temp_dir().join(format!("cce-simcache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sim.simcache");
        let mut state = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        state.points_mut().create("acc", AttribValue::Float(9.0));

        write_sim_cache_at(&path, 0xABCD, 12, &state);
        assert!(path.exists(), "the cache was written");

        // The right solve, at or before the frame being asked for.
        let (got, frame) = read_sim_cache_at(&path, 0xABCD, 20).expect("a matching cache resumes");
        assert_eq!(frame, 12);
        assert_eq!(got.points().value("acc", 0), Some(AttribValue::Float(9.0)));
        assert_eq!(got.ids(), state.ids());

        // A cache from a DIFFERENT chain is worse than no cache, because it
        // looks like an answer.
        assert!(read_sim_cache_at(&path, 0x1234, 20).is_none(), "a stale key must not resume");
        // Having run past the frame asked for, it cannot help: a step is not
        // invertible, so scrubbing back restarts from the seed.
        assert!(read_sim_cache_at(&path, 0xABCD, 5).is_none(), "a future state must not resume");
        // A file that is not there, or is rubbish, reads as "no cache".
        assert!(read_sim_cache_at(&dir.join("absent"), 0xABCD, 20).is_none());
        std::fs::write(&path, b"rubbish").unwrap();
        assert!(read_sim_cache_at(&path, 0xABCD, 20).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_cache_filename_cannot_climb_out_of_its_directory() {
        // The id is a node id, not a filename, and a project file is data.
        let path = sim_cache_path("../../../etc/passwd").expect("a cache path");
        assert!(!path.to_string_lossy().contains(".."), "{path:?}");
        assert!(path.to_string_lossy().ends_with("_________etc_passwd.simcache"), "{path:?}");
        assert!(
            path.to_string_lossy().contains("cce/cce-designer/sim"),
            "derived state belongs under the cache directory: {path:?}"
        );
    }

    #[test]
    fn test_substeps_run_the_chain_more_than_once_per_frame() {
        let acc = |root: &FsNode, frame: i32| -> f32 {
            solve_at(root, frame).points().value("acc", 0).unwrap().as_f32()
        };

        // Three frames elapsed, a fixed +1 per run of the chain.
        assert_eq!(acc(&substep_graph("1"), 4), 3.0);
        assert_eq!(acc(&substep_graph("4"), 4), 12.0);
        // The start frame has taken no steps whatever the substep count.
        assert_eq!(acc(&substep_graph("8"), 1), 0.0);

        // Substeps is part of the simnet's subtree, so changing it changes the
        // cache key and the sim restarts rather than resuming someone else's
        // arithmetic.
        let one = sim_solve_key(
            substep_graph("1").children.iter().find(|c| c.node_type == "simnet").unwrap(),
            &Detail::new(),
        );
        let four = sim_solve_key(
            substep_graph("4").children.iter().find(|c| c.node_type == "simnet").unwrap(),
            &Detail::new(),
        );
        assert_ne!(one, four);
    }

    #[test]
    fn test_dt_makes_substeps_a_stability_control_not_a_speed_control() {
        let acc = |root: &FsNode, frame: i32| -> f32 {
            solve_at(root, frame).points().value("acc", 0).unwrap().as_f32()
        };

        // A chain that scales its rate by the solver's `dt` covers the SAME
        // ground however finely the frame is cut: four substeps of a quarter
        // each is one frame's worth, exactly as one substep of a whole is.
        // That is the difference between subdividing a step and running the
        // simulation faster — and it is assembled from Promote and Composite
        // rather than built into the solver.
        let coarse = acc(&substep_dt_graph("1"), 5);
        for substeps in ["2", "4", "16"] {
            let fine = acc(&substep_dt_graph(substeps), 5);
            assert!(
                (fine - coarse).abs() < 1e-3,
                "{substeps} substeps drifted: {fine} vs {coarse}"
            );
        }
        assert!((coarse - 4.0).abs() < 1e-4, "four frames of one unit each: {coarse}");
    }

    #[test]
    fn test_dt_is_derivative_and_reads_one_over_the_substep_count() {
        let dt = |root: &FsNode| -> f32 {
            solve_at(root, 3).detail().value("dt", 0).unwrap().as_f32()
        };
        assert_eq!(dt(&substep_graph("1")), 1.0);
        assert_eq!(dt(&substep_graph("4")), 0.25);
        assert_eq!(
            solve_at(&substep_graph("4"), 3).detail().kind("dt"),
            crate::detail::AttribKind::Derivative
        );

        // A hand-edited project reaches the solver too, and a hundred thousand
        // substeps is indistinguishable from a hang. The declared range is a
        // UI affordance; this is the guard.
        let wild = substep_graph("100000");
        let acc = solve_at(&wild, 2).points().value("acc", 0).unwrap().as_f32();
        assert_eq!(acc, 64.0, "substeps are clamped to the documented ceiling");
    }

    #[test]
    fn test_live_data_accumulates_across_steps_and_derivative_data_does_not() {
        let acc = |root: &FsNode, frame: i32| -> f32 {
            solve_at(root, frame).points().value("acc", 0).unwrap().as_f32()
        };

        // Live data "runs like a stream through the simulation": each step
        // reads what the last one wrote, so five steps of +1 is 5.
        let live = accumulating_graph("Live");
        assert_eq!(acc(&live, 1), 0.0, "the start frame is the seed");
        assert_eq!(acc(&live, 4), 3.0);
        assert_eq!(acc(&live, 6), 5.0);

        // Derivative data is "calculated anew every frame": the boundary zeroes
        // it before the chain runs, so every step starts from nothing and the
        // answer is 1 however long the sim runs. Same graph, same chain — the
        // ONLY difference is what the attribute was declared to be.
        let derived = accumulating_graph("Derivative");
        assert_eq!(acc(&derived, 4), 1.0);
        assert_eq!(acc(&derived, 6), 1.0);
        assert_eq!(acc(&derived, 60), 1.0);
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
