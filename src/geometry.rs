use crate::app::{FsNode, ParamDef};
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


/// A [`Detail`]'s triangles as renderer vertices — the one place the 3D scene
/// crosses out of the geometry model.
pub fn detail_vertices(d: &Detail) -> Vec<Vertex3D> {
    d.triangulate(|position, color| Vertex3D { position, color })
}

/// The fill's triangles reordered FARTHEST FIRST from `eye` (mesh space),
/// by the squared distance to each centroid — what a see-through fill
/// needs, since with no depth writes the pass blends in submission order and
/// a near layer drawn before a far one would sit under it. Painter's order
/// by centroid is exact for non-intersecting triangles of similar size and
/// close enough for the rest, which is the trade every sorted-transparency
/// viewport makes. `verts` is a triangle list; a trailing partial triangle
/// is dropped.
pub fn sort_triangles_back_to_front(verts: &[Vertex3D], eye: Vec3) -> Vec<Vertex3D> {
    let tris = verts.len() / 3;
    let mut keyed: Vec<(f32, usize)> = (0..tris)
        .map(|t| {
            let c = (Vec3::from_array(verts[3 * t].position)
                + Vec3::from_array(verts[3 * t + 1].position)
                + Vec3::from_array(verts[3 * t + 2].position))
                / 3.0;
            ((c - eye).length_squared(), t)
        })
        .collect();
    keyed.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
    let mut out = Vec::with_capacity(tris * 3);
    for (_, t) in keyed {
        out.extend_from_slice(&verts[3 * t..3 * t + 3]);
    }
    out
}

/// How much of what stands at each of `points` reaches `eye` through the
/// translucent fill: 1 with nothing in front, `(1 - opacity)` per layer of
/// fill the sight line crosses, 0 behind an opaque one. It is what the depth
/// test and the blend do to a marker drawn under the fill, worked out on the
/// CPU for the point NUMBERS, which are 2D text and never meet the depth
/// buffer.
///
/// A layer is what the raster pass would blend there. Seen through, that
/// is every triangle the line crosses, either side. Otherwise the fill
/// culls back faces and writes depth, so a triangle counts when it faces
/// the eye and is nearer than every one drawn before it — `verts` is in
/// draw order. Triangles are binned by their screen bounds (`mvp`) so a
/// point is tested against the ones over its own pixel, not the mesh; the
/// primitives that meet AT the point cross the line at its end and are not
/// in front of it. The wires are not counted: a number is not behind a line
/// a pixel wide in any way a single alpha could show.
pub fn point_transmittance(
    verts: &[Vertex3D],
    mvp: glam::Mat4,
    eye: Vec3,
    points: &[[f32; 3]],
    opacity: f32,
    see_through: bool,
) -> Vec<f32> {
    const CELLS: usize = 48;
    let tris = verts.len() / 3;
    let opacity = opacity.clamp(0.0, 1.0);
    if tris == 0 || opacity <= 0.0 || points.is_empty() {
        return vec![1.0; points.len()];
    }
    let corner = |t: usize, k: usize| Vec3::from_array(verts[3 * t + k].position);
    let cell_of = |ndc: f32| (((ndc * 0.5 + 0.5) * CELLS as f32).floor().max(0.0) as usize).min(CELLS - 1);
    let mut cells: Vec<Vec<u32>> = vec![Vec::new(); CELLS * CELLS];
    // Triangles reaching behind the eye have no screen bounds to bin by.
    let mut everywhere: Vec<u32> = Vec::new();
    for t in 0..tris {
        let (mut lo, mut hi, mut behind) = ([f32::MAX; 2], [f32::MIN; 2], false);
        for k in 0..3 {
            let c = mvp * corner(t, k).extend(1.0);
            if c.w <= 1e-6 {
                behind = true;
                break;
            }
            for (axis, v) in [c.x / c.w, c.y / c.w].into_iter().enumerate() {
                lo[axis] = lo[axis].min(v);
                hi[axis] = hi[axis].max(v);
            }
        }
        if behind {
            everywhere.push(t as u32);
            continue;
        }
        if hi[0] < -1.0 || lo[0] > 1.0 || hi[1] < -1.0 || lo[1] > 1.0 {
            continue;
        }
        for y in cell_of(lo[1])..=cell_of(hi[1]) {
            for x in cell_of(lo[0])..=cell_of(hi[0]) {
                cells[y * CELLS + x].push(t as u32);
            }
        }
    }
    let through = 1.0 - opacity;
    let mut candidates: Vec<u32> = Vec::new();
    points
        .iter()
        .map(|&p| {
            let p = Vec3::from_array(p);
            let c = mvp * p.extend(1.0);
            if c.w <= 1e-6 {
                return 1.0;
            }
            let (x, y) = (c.x / c.w, c.y / c.w);
            if x.abs() > 1.0 || y.abs() > 1.0 {
                return 1.0;
            }
            candidates.clear();
            candidates.extend_from_slice(&cells[cell_of(y) * CELLS + cell_of(x)]);
            if !everywhere.is_empty() {
                candidates.extend_from_slice(&everywhere);
                candidates.sort_unstable();
            }
            let d = p - eye;
            let (mut layers, mut nearest) = (0i32, f32::MAX);
            for &t in &candidates {
                let t = t as usize;
                let (v0, v1, v2) = (corner(t, 0), corner(t, 1), corner(t, 2));
                let (e1, e2) = (v1 - v0, v2 - v0);
                if !see_through && e1.cross(e2).dot(eye - v0) <= 0.0 {
                    continue;
                }
                // Möller–Trumbore with the sight line as the ray, the
                // tolerance relative to the lengths as in
                // `spatial::segment_crosses_triangle`.
                let h = d.cross(e2);
                let det = e1.dot(h);
                if det.abs() <= 1e-7 * e1.length() * e2.length() * d.length() {
                    continue;
                }
                let f = 1.0 / det;
                let s = eye - v0;
                let u = f * s.dot(h);
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = s.cross(e1);
                let v = f * d.dot(q);
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let at = f * e2.dot(q);
                if at <= 0.0 || at >= 1.0 - 1e-3 {
                    continue;
                }
                if see_through {
                    layers += 1;
                } else if at < nearest {
                    nearest = at;
                    layers += 1;
                }
            }
            through.powi(layers)
        })
        .collect()
}

/// The raster pass's light, in WORLD space — `scene3d.wgsl`'s `l`, which the
/// smooth bake below has to match or switching shading modes would move
/// the lit side of the model.
pub const SCENE_LIGHT: [f32; 3] = [-0.55, 0.45, 0.7];

/// The raster pass's shading factor for a surface whose OUTWARD normal is
/// `n` — the multiplier `scene3d.wgsl` applies to a fragment's colour.
///
/// The shader's normal is `cross(dpdx(world), dpdy(world))`: screen right
/// crossed with framebuffer DOWN, which for any visible surface points away
/// from the viewer — into the surface. So the shader's `dot(n, l)` is this
/// function's `dot(-n, l)`, and the wrap term and the 0.55 floor are its
/// own. A zero normal (a point on no primitive) takes the midpoint.
pub fn shade_factor(n: Vec3) -> f32 {
    let l = Vec3::from_array(SCENE_LIGHT).normalize();
    let d = if n.length_squared() > 0.0 { ((-n).dot(l) * 0.5 + 0.5).clamp(0.0, 1.0) } else { 0.5 };
    0.55 + 0.45 * d
}

/// The scene's fill mesh with SMOOTH shading baked into its colours: each
/// corner lit by its point's smooth normal (`point_normals`) under the
/// raster pass's own world-fixed light, then drawn `prelit` so the shader
/// adds nothing. Because the light never moves with the camera, lighting
/// per vertex and interpolating is exact — Gouraud shading with no normal
/// in the vertex format.
///
/// Smoothing follows the TOPOLOGY: points shared between primitives average
/// their faces, so a welded mesh rounds off, while a soup of unshared
/// triangles or a cut along a seam stays faceted there — as it would in
/// any smooth-shaded viewport. Colours here only; the path tracer keeps
/// reading the unlit ones.
pub fn smooth_lit_vertices(d: &Detail) -> Vec<Vertex3D> {
    let normals = point_normals(d);
    let lit: Vec<f32> = normals.iter().map(|n| shade_factor(*n)).collect();
    // Vertex normals, where the geometry carries them, are the normals it
    // asked to be shaded by: a corner is lit by its own, so a crease the
    // Normal node cusped reads hard and the rest smooth.
    let own = own_vertex_normals(d);
    let mut out = Vec::new();
    for prim in 0..d.num_prims() {
        let pts = d.prim_points(prim);
        if pts.len() < 3 {
            continue;
        }
        let first = d.prim_verts(prim).start;
        for i in 1..pts.len() - 1 {
            for &corner in &[0, i, i + 1] {
                let p = pts[corner] as usize;
                let by_vertex = own
                    .and_then(|n| n.get(first + corner))
                    .map(|n| n.as_vec3())
                    .filter(|n| n.length_squared() > 1e-12)
                    .map(|n| shade_factor(n.normalize()));
                let k = by_vertex.unwrap_or_else(|| lit.get(p).copied().unwrap_or(1.0));
                let c = d.color(p);
                out.push(Vertex3D { position: d.pos(p).to_array(), color: [c[0] * k, c[1] * k, c[2] * k] });
            }
        }
    }
    out
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
/// each sampled span an oriented box via [`box_detail`]. Points are
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

fn find_param<'a>(node: &'a FsNode, name: &str) -> Option<&'a ParamDef> {
    node.params.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// A number: the parsed value of a number, whole-number or toggle
/// parameter, else the text as a number (what every read was before the
/// value was typed — so a text row holding `12` still reads 12), else
/// `fallback`.
pub fn node_param_f32(node: &FsNode, name: &str, fallback: f32) -> f32 {
    use crate::app::ParamValue;
    let Some(p) = find_param(node, name) else { return fallback };
    match p.value() {
        Some(ParamValue::Number(n)) => *n,
        Some(ParamValue::Int(i)) => *i as f32,
        _ => p.text().parse::<f32>().unwrap_or(fallback),
    }
}

/// The text as written — for a choice the option as the row shows it, for
/// an expression the expression.
pub fn node_param_str(node: &FsNode, name: &str, fallback: &str) -> String {
    find_param(node, name).map(|p| p.text().to_string()).unwrap_or_else(|| fallback.to_string())
}

/// Three numbers: a float3's parsed value, else `x:y:z` read from the text
/// (a text row holding a vector, like the Attribute node's Value), else
/// `fallback`.
pub fn node_param_vec3(node: &FsNode, name: &str, fallback: Vec3) -> Vec3 {
    use crate::app::ParamValue;
    let Some(p) = find_param(node, name) else { return fallback };
    if let Some(ParamValue::Vec3(v)) = p.value() {
        return Vec3::from(*v);
    }
    let parts: Vec<&str> = p.text().split(':').collect();
    if let [x, y, z] = parts[..] {
        if let (Ok(x), Ok(y), Ok(z)) = (x.parse::<f32>(), y.parse::<f32>(), z.parse::<f32>()) {
            return Vec3::new(x, y, z);
        }
    }
    fallback
}

/// A toggle's value. `true` / `false` in any case — and `1` / `0`, `on` /
/// `off`, which a hand-edited file or an expression may leave — else
/// `fallback`: absent, empty and unparseable all mean "the default", where
/// the hand-rolled `== "true"` and `!= "false"` this replaced disagreed about
/// garbage depending on which way round each site had been written.
pub fn node_param_bool(node: &FsNode, name: &str, fallback: bool) -> bool {
    let Some(p) = find_param(node, name) else { return fallback };
    if let Some(crate::app::ParamValue::Bool(b)) = p.value() {
        return *b;
    }
    match p.text().trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "on" => true,
        "false" | "0" | "off" => false,
        _ => fallback,
    }
}

/// The node NAME a reference parameter holds (an `Input` wire, a Boolean's
/// `With`, a Relax's `Rest`), trimmed — `None` when it is unconnected,
/// absent or empty alike.
pub fn node_param_node(node: &FsNode, name: &str) -> Option<String> {
    let v = node_param_str(node, name, "");
    let v = v.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// The node `target`'s reference parameter `name` points at, resolved the
/// way every wire is ([`find_input_node`]: a sibling first, then anywhere).
/// `None` when the parameter is unconnected or names nothing.
pub fn param_node<'a>(root: &'a FsNode, target: &FsNode, name: &str) -> Option<&'a FsNode> {
    find_input_node(root, target, &node_param_node(target, name)?)
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

/// The node `name` refers to from `target`: a SIBLING first, then anywhere
/// under `root`.
///
/// Every resolver used to search the whole tree from the top, so inside the
/// second instance of a subnet a child wired to "input1" found the FIRST
/// instance's `input1` — the opencl and output resolvers had each grown a
/// sibling-first lookup of their own to dodge exactly that. A subnet built
/// from other nodes (the Embryo, composed) wires its children to each other
/// by name, and every one of those wires has to land on its own sibling.
/// The global fallback keeps what worked before: a node inside a subnet may
/// still name a node outside it.
pub fn find_input_node<'a>(root: &'a FsNode, target: &FsNode, name: &str) -> Option<&'a FsNode> {
    if name.is_empty() {
        return None;
    }
    find_parent_node(root, &target.id)
        .and_then(|p| p.children.iter().find(|c| c.name == name || c.id == name))
        .or_else(|| find_node_by_name(root, name))
}

pub use crate::expr::{ChKind, Value};

/// Whether any of `node`'s parameters holds an expression — the cheap test
/// that lets evaluation skip the clone for the common node.
pub fn has_param_refs(node: &FsNode) -> bool {
    node.params.iter().any(|p| p.is_expr())
}

/// A choice's options: the `options` list, else the `choice:A,B` type.
pub fn choice_options(p: &ParamDef) -> Vec<String> {
    p.choice_options()
}

/// A parameter's value as a NUMBER — what a kernel's `chf` / `chi` / `chb`
/// and an expression's `ch()` both read. A toggle is 0 or 1; a choice is its
/// option INDEX, the position in the template's list, the way an ordinal
/// menu evaluates in Houdini. The index is what lets a subnet's dropdown
/// drive a child switch's Index or a kernel's `chi("Method")`: the option
/// text parses as nothing, and until 2026-09-24 the kernel path parsed it
/// anyway, so every choice read as 0 from inside a kernel.
pub fn param_number(p: &ParamDef) -> f32 {
    use crate::app::ParamValue;
    match p.value() {
        Some(ParamValue::Number(n)) => *n,
        Some(ParamValue::Int(i)) => *i as f32,
        Some(ParamValue::Bool(b)) => f32::from(u8::from(*b)),
        Some(ParamValue::Choice(o)) => choice_options(p).iter().position(|x| x == o).map_or(0.0, |i| i as f32),
        // Text, a vector, an expression or a value that does not fit: the
        // text as a number, which is what every read was before the value
        // was typed.
        _ if p.kind() == crate::app::ParamKind::Choice => 0.0,
        _ => number_of_str(p.text().trim()),
    }
}

/// A bare value's number: `true` / `false` as 1 / 0, else parsed, else 0.
fn number_of_str(raw: &str) -> f32 {
    if raw.eq_ignore_ascii_case("true") {
        1.0
    } else if raw.eq_ignore_ascii_case("false") {
        0.0
    } else {
        raw.parse::<f32>().unwrap_or(0.0)
    }
}

/// The nodes from the root's first level down to `id`, the root itself
/// excluded — empty for the root, None for an id not in the tree.
pub fn node_chain<'a>(root: &'a FsNode, id: &str) -> Option<Vec<&'a FsNode>> {
    fn visit<'a>(node: &'a FsNode, id: &str, stack: &mut Vec<&'a FsNode>) -> bool {
        for c in &node.children {
            stack.push(c);
            if c.id == id || visit(c, id, stack) {
                return true;
            }
            stack.pop();
        }
        false
    }
    let mut stack = Vec::new();
    if root.id == id {
        return Some(stack);
    }
    if visit(root, id, &mut stack) { Some(stack) } else { None }
}

/// A node's absolute path as names: `["sphere1", "opencl1"]`.
pub fn node_path_names(root: &FsNode, id: &str) -> Option<Vec<String>> {
    node_chain(root, id).map(|chain| chain.iter().map(|n| n.name.clone()).collect())
}

/// The node part of a channel path walked from `start` — `..` its parent,
/// `.` itself, a name one of its children. Every step is reported by name,
/// because the error lands on the node for the user to read.
fn walk_ref_path<'a>(root: &'a FsNode, start: &'a FsNode, segments: &[&str]) -> Result<&'a FsNode, String> {
    let mut cur = start;
    for seg in segments {
        cur = match *seg {
            "." => cur,
            ".." => find_parent_node(root, &cur.id)
                .or_else(|| if cur.id == root.id { None } else { Some(root) })
                .ok_or_else(|| format!("`..` climbs above the root from {}", cur.name))?,
            name => cur
                .children
                .iter()
                .find(|c| c.name == name)
                .or_else(|| cur.children.iter().find(|c| c.name.eq_ignore_ascii_case(name)))
                .ok_or_else(|| format!("no node `{name}` in {}", if cur.id == root.id { "/" } else { cur.name.as_str() }))?,
        };
    }
    Ok(cur)
}

/// A channel path split: whether it is absolute, its node segments, and its
/// final parameter segment.
fn split_ref_path(path: &str) -> (bool, Vec<&str>, &str) {
    let t = path.trim();
    let absolute = t.starts_with('/');
    let mut segs: Vec<&str> = t.split('/').filter(|s| !s.is_empty()).collect();
    let param = segs.pop().unwrap_or("");
    (absolute, segs, param)
}

/// A parameter named by the last path segment, with an optional `.x` / `.y`
/// / `.z` component for a float3 — tried as a whole name first, so a
/// parameter that really is called `Size.x` still resolves.
fn find_ref_param<'a>(node: &'a FsNode, name: &str) -> Option<(&'a ParamDef, Option<usize>)> {
    if let Some(p) = node.params.iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
        return Some((p, None));
    }
    let (base, comp) = name.rsplit_once('.')?;
    let comp = match comp {
        "x" | "0" => 0,
        "y" | "1" => 1,
        "z" | "2" => 2,
        _ => return None,
    };
    node.params.iter().find(|p| p.name.eq_ignore_ascii_case(base)).map(|p| (p, Some(comp)))
}

/// What an expression on `node` sees: the tree for its channels, the frame
/// for `$F`, and the chain of parameters being evaluated, so a reference
/// that comes back round to itself is an error rather than a stack overflow.
///
/// Public so a SCRIPT on a node (the wrangle) can read channels through
/// the same scope its parameters do: a `ch("../a/Radius")` in a script
/// then sees an expression-valued Radius evaluated, not its text.
pub struct TreeScope<'a> {
    root: &'a FsNode,
    node: &'a FsNode,
    frame: i32,
    stack: Vec<(String, String)>,
}

impl<'a> TreeScope<'a> {
    /// A scope for something on `node` — an expression on one of its
    /// parameters, or a script it runs — evaluated at `frame`.
    pub fn new(root: &'a FsNode, node: &'a FsNode, frame: i32) -> Self {
        TreeScope { root, node, frame, stack: Vec::new() }
    }
}

impl<'a> crate::expr::Scope for TreeScope<'a> {
    fn channel(&mut self, path: &str, kind: ChKind) -> Result<Value, String> {
        let (absolute, segs, pname) = split_ref_path(path);
        if pname.is_empty() {
            return Err(format!("{}: ch(\"{path}\") names no parameter", self.node.name));
        }
        // The REAL node, looked up by id: `self.node` may be a resolved clone,
        // and a relative path starts from where it sits in the tree.
        let start = if absolute {
            self.root
        } else {
            crate::viewer_state::find_node_by_id(self.root, &self.node.id).unwrap_or(self.node)
        };
        let node = walk_ref_path(self.root, start, &segs).map_err(|e| format!("{}: ch(\"{path}\"): {e}", self.node.name))?;
        let (p, comp) = find_ref_param(node, pname)
            .ok_or_else(|| format!("{}: ch(\"{path}\") names no parameter {pname} on {}", self.node.name, if node.id == self.root.id { "/" } else { &node.name }))?;
        let raw = if p.is_expr() {
            let key = (node.id.clone(), p.name.clone());
            if self.stack.contains(&key) {
                return Err(format!("{}: ch(\"{path}\") is a circular reference", self.node.name));
            }
            if self.stack.len() >= 32 {
                return Err(format!("{}: ch(\"{path}\") chains too deep", self.node.name));
            }
            self.stack.push(key);
            let mut inner = TreeScope { root: self.root, node, frame: self.frame, stack: std::mem::take(&mut self.stack) };
            let r = eval_param_value(&mut inner, p);
            self.stack = inner.stack;
            self.stack.pop();
            r?.text()
        } else {
            p.text().to_string()
        };
        let value = match comp {
            Some(i) => Value::Num(raw.split(':').nth(i).and_then(|c| c.trim().parse::<f64>().ok()).unwrap_or(0.0)),
            None => {
                let mut lit = p.clone();
                lit.bake(raw);
                match kind {
                    ChKind::Str => Value::Str(lit.text().trim().to_string()),
                    _ => Value::Num(param_number(&lit) as f64),
                }
            }
        };
        Ok(match kind {
            ChKind::Float | ChKind::Str => value,
            ChKind::Int => Value::Num(value.as_num().trunc()),
            ChKind::Bool => Value::Num(if value.truthy() { 1.0 } else { 0.0 }),
        })
    }

    fn var(&self, name: &str) -> Option<Value> {
        match name {
            "F" | "FF" => Some(Value::Num(self.frame as f64)),
            _ => None,
        }
    }
}

/// What an expression evaluated to, in the parameter's own terms: a value
/// that fits its kind ([`ParamDef::value_from_expr`] — a toggle's truth, a
/// choice's option, a spinbox's whole part), or the value's text when it
/// fits nothing (a string into a slider), which is stored and flagged
/// exactly as the old write-back stored it and a reader then fell back from.
#[derive(Debug)]
pub enum Evaluated {
    Value(crate::app::ParamValue),
    Unfit(String),
}

impl Evaluated {
    pub fn text(&self) -> String {
        match self {
            Evaluated::Value(v) => v.to_text(),
            Evaluated::Unfit(s) => s.clone(),
        }
    }
}

/// One parameter's expression evaluated. A float3 is three expressions
/// separated by `:` — each component its own, as Houdini's channels are —
/// so `chf("../a/Size.x"):0:0` reads naturally.
fn eval_param_value(scope: &mut TreeScope, p: &ParamDef) -> Result<Evaluated, String> {
    if p.kind() == crate::app::ParamKind::Float3 {
        let parts: Vec<&str> = p.text().split(':').collect();
        if parts.len() == 3 {
            let mut out = [0.0f32; 3];
            for (c, part) in out.iter_mut().zip(parts) {
                let t = part.trim();
                *c = match t.parse::<f32>() {
                    Ok(n) => n,
                    Err(_) => {
                        let e = crate::expr::parse(t).map_err(|e| format!("{}: {} — {e}", scope.node.name, p.name))?;
                        e.eval(scope)?.as_num() as f32
                    }
                };
            }
            return Ok(Evaluated::Value(crate::app::ParamValue::Vec3(out)));
        }
    }
    let e = crate::expr::parse(p.text()).map_err(|e| format!("{}: {} — {e}", scope.node.name, p.name))?;
    let v = e.eval(scope)?;
    Ok(match p.value_from_expr(&v) {
        Some(value) => Evaluated::Value(value),
        None => Evaluated::Unfit(v.as_str()),
    })
}

/// `target` with every expression replaced by the value it evaluates to at
/// `frame`, or `None` when it has none — so the common node costs one scan
/// and no clone. A failing expression (bad syntax, a path to nothing, a
/// circle) is reported through `error` and its text left as written, which
/// the resolvers then read as they always have — a number that parses as
/// nothing is 0. The clone's parameters come back as VALUES (`expr` off),
/// so nothing downstream evaluates twice.
pub fn resolve_param_refs(root: &FsNode, target: &FsNode, frame: i32, error: &mut Option<String>) -> Option<FsNode> {
    if !has_param_refs(target) {
        return None;
    }
    let mut out = target.clone();
    for p in &mut out.params {
        if !p.is_expr() {
            continue;
        }
        let mut scope = TreeScope { root, node: target, frame, stack: vec![(target.id.clone(), p.name.clone())] };
        match eval_param_value(&mut scope, p) {
            Ok(Evaluated::Value(v)) => p.set_value(v),
            Ok(Evaluated::Unfit(text)) => p.bake(text),
            Err(e) => {
                if error.is_none() {
                    *error = Some(e);
                }
            }
        }
    }
    Some(out)
}

/// The path an expression on `from` would use to reach `to`, relative and
/// without the parameter: `../sphere1` for a sibling, `..` for the parent,
/// `` for the node itself. What Paste Relative Reference writes.
pub fn relative_ref_path(root: &FsNode, from: &str, to: &str) -> Option<String> {
    let a = node_path_names(root, from)?;
    let b = node_path_names(root, to)?;
    let common = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let mut segs: Vec<String> = vec!["..".to_string(); a.len() - common];
    segs.extend(b[common..].iter().cloned());
    Some(segs.join("/"))
}

/// The absolute path to `to`, `/a/b`, for Paste Absolute Reference.
pub fn absolute_ref_path(root: &FsNode, to: &str) -> Option<String> {
    node_path_names(root, to).map(|names| format!("/{}", names.join("/")))
}

/// Rename the node `id` to `new_name` and keep everything that names it
/// pointing at it: the wires — sibling parameters whose value is the old
/// name, as the load-time sanitizer rewrites them — and every expression
/// anywhere in the tree whose channel path passes through the node.
/// Expressions are rewritten by resolving each path from where it stands
/// BEFORE the name changes, since a path is names and the old one is what
/// still resolves.
pub fn rename_node_in_tree(root: &mut FsNode, id: &str, new_name: &str) -> bool {
    let Some(chain) = node_chain(root, id) else { return false };
    let Some(node) = chain.last() else { return false };
    let old_name = node.name.clone();
    if old_name == new_name {
        return false;
    }
    let parent_id = chain.iter().rev().nth(1).map(|p| p.id.clone()).unwrap_or_else(|| root.id.clone());

    // Pass one, immutable: every expression that changes.
    let mut edits: Vec<(String, String, String)> = Vec::new();
    fn collect(root: &FsNode, node: &FsNode, id: &str, new_name: &str, edits: &mut Vec<(String, String, String)>) {
        for p in &node.params {
            if !p.is_expr() {
                continue;
            }
            let rewritten = crate::expr::rewrite_paths(p.text(), |path| {
                let (absolute, segs, pname) = split_ref_path(path);
                let mut cur = if absolute { root } else { node };
                let mut out: Vec<String> = Vec::new();
                let mut changed = false;
                for seg in segs {
                    match seg {
                        "." => {
                            out.push(".".into());
                        }
                        ".." => {
                            cur = find_parent_node(root, &cur.id)?;
                            out.push("..".into());
                        }
                        name => {
                            cur = cur.children.iter().find(|c| c.name == name)?;
                            if cur.id == id {
                                out.push(new_name.to_string());
                                changed = true;
                            } else {
                                out.push(name.to_string());
                            }
                        }
                    }
                }
                if !changed {
                    return None;
                }
                out.push(pname.to_string());
                Some(format!("{}{}", if absolute { "/" } else { "" }, out.join("/")))
            });
            if rewritten != p.text() {
                edits.push((node.id.clone(), p.name.clone(), rewritten));
            }
        }
        for c in &node.children {
            collect(root, c, id, new_name, edits);
        }
    }
    collect(root, root, id, new_name, &mut edits);

    // Pass two, mutable: the expressions, the wires, the name.
    for (nid, pname, value) in edits {
        if let Some(n) = crate::viewer_state::find_node_by_id_mut(root, &nid) {
            if let Some(p) = n.params.iter_mut().find(|p| p.name == pname) {
                p.set_text(value);
            }
        }
    }
    if let Some(parent) = crate::viewer_state::find_node_by_id_mut(root, &parent_id) {
        for sibling in &mut parent.children {
            for p in &mut sibling.params {
                if !p.is_expr() && p.text() == old_name {
                    p.set_text(new_name.to_string());
                }
            }
        }
    }
    if let Some(n) = crate::viewer_state::find_node_by_id_mut(root, id) {
        n.name = new_name.to_string();
    }
    true
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
    /// What the LAST substep that produced `state` consumed — the seed until
    /// a step has run — derivatives cleared and `dt` set, exactly as the
    /// chain saw it. What a visible child inside the simnet is evaluated
    /// against when the interior is displayed (see
    /// [`network_sphere_vertices_with_errors`]): `input` yields it, the
    /// chain shows the pass that landed on the displayed state, so a node
    /// that is the chain's last mover draws where the output draws. Until
    /// 2026-09-28 it was the state at the START of the frame, which under
    /// substeps showed one substep of a frame that took several.
    prev: Detail,
    /// Earlier frames of the same solve, kept so a backward scrub resumes
    /// from the nearest one behind it instead of the seed. See
    /// [`Checkpoint`].
    checkpoints: Checkpoints,
}

/// A frame of a solve, kept in memory: its state and what its last substep
/// consumed, as [`SimSolve`] keeps the latest.
///
/// A step is not invertible, so going BACK used to mean solving from the
/// seed: a scrub from frame 120 to 119 was 119 steps, and dragging the
/// playhead backwards re-solved the whole history at every frame it passed.
/// One of these is kept every [`CHECKPOINT_EVERY`] frames as a solve runs
/// (further apart once there are too many: [`Checkpoints::keep`]),
/// and the frame a solve is asked to leave behind is kept too, so a scrub
/// in either direction inside what has been solved steps less than an
/// interval.
///
/// They belong to one key. An edit to the chain or the seed changes it and
/// they go with the state they were frames of, which is why an edit at
/// frame 120 is still 120 steps: nothing earlier than the edit survives it.
#[derive(Clone)]
struct Checkpoint {
    frame: i32,
    state: Detail,
    prev: Detail,
}

// Every run of a chain on this thread, through whichever cache — so a
// test can see a solve that went through a cache it has no hold of.
#[cfg(test)]
thread_local! {
    pub static STEPS_ON_THIS_THREAD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many frames apart checkpoints start out.
pub const CHECKPOINT_EVERY: i32 = 10;
/// The most kept for one simnet, however large its states.
pub const CHECKPOINTS_MAX: usize = 48;
/// What one simnet's checkpoints may hold, by [`checkpoint_bytes`]. A state
/// is kept twice over (itself and what its last substep consumed), so a
/// hundred-thousand-point surface is tens of megabytes a checkpoint.
pub const CHECKPOINT_BUDGET: usize = 512 * 1024 * 1024;

/// About what a checkpoint of `state` holds: positions, ids and a handful
/// of attributes a point, the primitives' indices, twice. An estimate — the
/// budget is a guard against a runaway, not an accounting.
fn checkpoint_bytes(state: &Detail) -> usize {
    2 * (state.num_points() * 96 + state.num_verts() * 8 + state.num_prims() * 8 + 256)
}

/// One solve's checkpoints, in frame order, and how far apart they are
/// being kept.
struct Checkpoints {
    every: i32,
    kept: Vec<Checkpoint>,
}

impl Default for Checkpoints {
    fn default() -> Self {
        Checkpoints { every: CHECKPOINT_EVERY, kept: Vec::new() }
    }
}

impl Checkpoints {
    /// Keep `at`, within the count and the budget. When there is no room
    /// the SPACING doubles, and stays doubled: what is not on the wider
    /// interval goes, and what arrives from then on arrives that far
    /// apart. Dropping the oldest would leave a long solve with nothing
    /// near its start, and dropping every other one while new ones kept
    /// arriving at the old spacing thinned the start again and again — a
    /// scrub is as likely to land there as anywhere, and an even spacing
    /// is what bounds the steps from anywhere.
    fn keep(&mut self, at: Checkpoint) {
        let each = checkpoint_bytes(&at.state).max(1);
        match self.kept.binary_search_by_key(&at.frame, |c| c.frame) {
            Ok(i) => self.kept[i] = at,
            Err(i) => self.kept.insert(i, at),
        }
        let room = (CHECKPOINT_BUDGET / each).clamp(2, CHECKPOINTS_MAX);
        while self.kept.len() > room {
            self.every = self.every.saturating_mul(2);
            let every = self.every;
            self.kept.retain(|c| c.frame % every == 0);
        }
    }

    /// The nearest kept frame at or behind `due` and ahead of `after`.
    fn behind(&self, due: i32, after: i32) -> Option<&Checkpoint> {
        self.kept.iter().rev().find(|c| c.frame <= due && c.frame > after)
    }
}

/// Per-simnet solved states, keyed by node id. Owned by the caller (the app keeps
/// one across frames; a one-shot render can pass a fresh one) rather than being a
/// global, so two evaluations of different graphs cannot poison each other.
#[derive(Default)]
pub struct SimCache {
    entries: std::collections::HashMap<String, SimSolve>,
    /// Runs of a chain since this cache was made — every substep of every
    /// simnet. What a solve COST, for the tests that a scrub resumes and
    /// does not re-solve.
    steps_run: usize,
}

impl SimCache {
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// See [`SimCache::steps_run`].
    pub fn steps_run(&self) -> usize {
        self.steps_run
    }

    /// The frames simnet `id` has checkpoints at, ascending — relative to
    /// its start frame, as the solve counts them.
    pub fn checkpoint_frames(&self, id: &str) -> Vec<i32> {
        self.entries.get(id).map(|e| e.checkpoints.kept.iter().map(|c| c.frame).collect()).unwrap_or_default()
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

/// Whether a node is bypassed: in the graph, and doing nothing.
///
/// What reads a bypassed node gets what the node reads — its `Input`,
/// untouched, and nothing at all from a node that has none, which is what a
/// generator switched off gives. It is decided HERE, ahead of every
/// resolver and ahead of the node's own parameters, so an expression that
/// fails on a bypassed node is not evaluated and not reported.
///
/// `input` and `output` are a subnet's plumbing and a camera is not
/// geometry: the flag on any of them says nothing.
pub fn is_bypassed(node: &FsNode) -> bool {
    node.bypassed && !["input", "output", "camera"].iter().any(|ty| node.node_type.eq_ignore_ascii_case(ty))
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

    // Parameter references resolve here, once, for every resolver below:
    // a child of a composed subnet reads its parent's controls through
    // `ch("Name")` and the resolvers never know.
    if is_bypassed(target) {
        let res = param_node(root, target, "Input").and_then(|input| generate_single_node_geometry_with_errors(root, input, visited, ocl_error, sim));
        visited.pop();
        return res;
    }

    let resolved = resolve_param_refs(root, target, sim.frame, ocl_error);
    let target = resolved.as_ref().unwrap_or(target);

    let res = if target.node_type.eq_ignore_ascii_case("sphere") {
        // A sphere with no Center parameters is a bare hand-built node,
        // placed by index as Line and Points still are.
        let legacy = if crate::shapes::sphere_has_center(target) {
            None
        } else {
            Some(index_center(find_sphere_index(root, target)?))
        };
        Some(crate::shapes::sphere_node_detail(target, legacy))
    } else if target.node_type.eq_ignore_ascii_case("box") {
        Some(crate::shapes::box_node_detail(target))
    } else if target.node_type.eq_ignore_ascii_case("plane") {
        Some(crate::shapes::plane_node_detail(target))
    } else if target.node_type.eq_ignore_ascii_case("extrude") {
        resolve_extrude_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("line") {
        let idx = find_sphere_index(root, target)?;
        let start = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
        let length = node_param_f32(target, "Length", 1.0);
        let thickness = node_param_f32(target, "Thickness", 0.02);
        let end = start + Vec3::new(0.0, length, 0.0);
        Some(box_detail(start, end, thickness))
    } else if target.node_type.eq_ignore_ascii_case("curve") {
        Some(curve_detail(target))
    } else if target.node_type.eq_ignore_ascii_case("grid") {
        Some(grid_detail(target))
    } else if target.node_type.eq_ignore_ascii_case("polygon") {
        Some(polygon_detail(target))
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
    } else if target.node_type.eq_ignore_ascii_case("normal") {
        resolve_normal_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("bounds") {
        resolve_bounds_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("distance") {
        resolve_distance_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("connectivity") {
        resolve_connectivity_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("cull") {
        resolve_cull_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("copy") {
        resolve_copy_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("soft_transform") {
        resolve_soft_transform_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("transfer") {
        resolve_transfer_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("valence") {
        resolve_valence_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("deform") {
        resolve_deform_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("volume") {
        resolve_volume_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("mold_shell") {
        resolve_mold_shell_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("hull") {
        resolve_hull_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("wrangle") {
        resolve_wrangle_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("switch") {
        resolve_switch_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("boolean") {
        resolve_boolean_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("export") {
        resolve_export_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("subdivide") {
        resolve_subdivide_geometry_with_errors(root, target, visited, ocl_error, sim)
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
        retired_opencl_node(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("simnet") {
        resolve_simnet_geometry_with_errors(root, target, visited, ocl_error, sim)
    } else if target.node_type.eq_ignore_ascii_case("node") {
        if let Some(output_node) = target.children.iter().find(|c| c.node_type.eq_ignore_ascii_case("output")) {
            generate_single_node_geometry_with_errors(root, output_node, visited, ocl_error, sim)
        } else {
            None
        }
    } else if target.node_type.eq_ignore_ascii_case("output") {
        // Sibling-first, then anywhere — `find_input_node`'s own rule,
        // which this arm spelled out by hand before that function existed.
        param_node(root, target, "Input")
            .and_then(|node| generate_single_node_geometry_with_errors(root, node, visited, ocl_error, sim))
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
            // Resolved, like the kernel's parent read above: a subnet's
            // Input may itself be a reference.
            let resolved_parent = resolve_param_refs(root, parent, sim.frame, ocl_error);
            let parent = resolved_parent.as_ref().unwrap_or(parent);
            node_param_node(parent, "Input")
                .and_then(|name| find_input_node(root, target, &name))
                .and_then(|input_node| generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim))
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
    let input_node = param_node(root, target, "Input")?;
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
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let group_name = node_param_str(target, "Group Name", "group1").trim().to_string();
    let etype = node_param_str(target, "Element Type", "Points").to_lowercase();
    let center = node_param_vec3(target, "Center", Vec3::ZERO);
    let half = node_param_vec3(target, "Size", Vec3::ONE) * 0.5;
    let invert = node_param_bool(target, "Invert", false);
    let highlight = node_param_bool(target, "Highlight", true);

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
    if mode == "attribute" {
        // Select by what a point IS rather than where it is. This is what
        // makes the measuring nodes composable: Distance, Connectivity or a
        // solver's own attribute becomes a named selection that Cull, Relax's
        // pin, Attribute's group and Soft Transform all already read.
        let attr = node_param_str(target, "Attribute", "");
        let attr = attr.trim().to_string();
        let below = node_param_str(target, "Comparison", "Above").eq_ignore_ascii_case("below");
        let threshold = node_param_f32(target, "Threshold", 0.5);
        if geom.points().has(&attr) {
            for p in 0..geom.num_points() {
                let v = geom.points().value(&attr, p).map(|v| v.as_f32()).unwrap_or(0.0);
                if if below { v < threshold } else { v > threshold } {
                    member[p] = true;
                }
            }
        }
    } else if mode == "expand" {
        // Grow or shrink an existing selection across the surface. Rings is
        // signed: positive walks outward from the members, negative peels the
        // boundary off, which is how an erode/dilate pair reads without two
        // nodes.
        let src = node_param_str(target, "Source Group", "");
        let src = src.trim().to_string();
        let rings = node_param_f32(target, "Rings", 1.0).clamp(-8.0, 8.0) as i32;
        let mut inside: Vec<bool> = (0..geom.num_points())
            .map(|p| !src.is_empty() && geom.points().in_group(&src, p))
            .collect();
        for _ in 0..rings.unsigned_abs() {
            let before = inside.clone();
            for p in 0..geom.num_points() {
                let touches_other = geom
                    .point_neighbours(p)
                    .iter()
                    .any(|&q| before[q as usize] != before[p]);
                if !touches_other {
                    continue;
                }
                // Growing takes the boundary's outside; shrinking gives up the
                // boundary's inside.
                if rings > 0 && !before[p] {
                    inside[p] = true;
                } else if rings < 0 && before[p] {
                    inside[p] = false;
                }
            }
        }
        for (p, &m) in inside.iter().enumerate() {
            member[p] = m;
        }
    } else if mode == "random" {
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
pub(crate) fn point_triangle_distance_sq(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
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
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    // Sibling-first like every other wire: this was a whole-tree
    // `find_node_by_name`, so inside a second copy of a subnet it found the
    // first copy's collider.
    let Some(collider_node) = param_node(root, target, "Collider") else { return Some(geom) };
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
    let test = if method == "proximity" { crate::collide::Test::Proximity(distance) } else { crate::collide::Test::Inside };

    // The test runs as ONE batch over every element the type asks about —
    // points, or primitive centroids — so it can go to the GPU whole
    // (`crate::collide`, the second Phase 7 step 4 operator); the
    // per-element closure `select_elements` takes would have asked one
    // point at a time. Edges test both endpoints, as before.
    let etype = node_param_str(target, "Element Type", "Points").to_lowercase();
    let invert = node_param_bool(target, "Invert", false);
    let queries: Vec<Vec3> = if etype == "primitives" {
        (0..geom.num_prims())
            .map(|prim| {
                let pts = geom.prim_points(prim);
                if pts.is_empty() {
                    Vec3::ZERO
                } else {
                    pts.iter().map(|&p| geom.pos(p as usize)).sum::<Vec3>() / pts.len() as f32
                }
            })
            .collect()
    } else {
        (0..geom.num_points()).map(|p| geom.pos(p)).collect()
    };
    let flags = match crate::collide::hits(&queries, &tris, test) {
        Ok(f) => f,
        Err(e) => {
            if ocl_error.is_none() {
                *ocl_error = Some(format!("{}: {e}", target.name));
            }
            crate::collide::hits_cpu(&queries, &tris, test)
        }
    };
    let mut member = vec![false; geom.num_points()];
    let mut prim_member = vec![false; geom.num_prims()];
    match etype.as_str() {
        "primitives" => {
            for prim in 0..geom.num_prims() {
                if flags[prim] != 0 {
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
                if flags[a] != 0 && flags[b] != 0 {
                    member[a] = true;
                    member[b] = true;
                }
            }
        }
        _ => {
            for p in 0..geom.num_points() {
                member[p] = flags[p] != 0;
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

    let group_name = node_param_str(target, "Group Name", "collisions").trim().to_string();
    let highlight = node_param_bool(target, "Highlight", true);
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
/// surface with a stiffness-shaped falloff. Iterations JACOBI passes over
/// the rest topology (`crate::springs` — one algorithm on the CPU and the
/// GPU, the first operator of Phase 7 step 4); Stiffness scales each
/// correction. With no Rest,
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
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    // Repel mode: the Relax SOP — spheres of Radius pushed apart until they
    // stop overlapping, each point sliding in its tangent plane unless In 3D
    // Space lets it leave. No rest shape, no springs; zero iterations is off.
    if node_param_str(target, "Mode", "Springs").eq_ignore_ascii_case("repel") {
        let iterations = node_param_f32(target, "Iterations", 8.0).max(0.0) as usize;
        let radius = node_param_f32(target, "Radius", 0.05);
        if iterations > 0 && radius > 0.0 && geom.num_points() >= 2 {
            let in_3d = node_param_bool(target, "In 3D Space", false);
            let normals = if in_3d { None } else { Some(point_normals(&geom)) };
            let mut pts: Vec<Vec3> = (0..geom.num_points()).map(|p| geom.pos(p)).collect();
            crate::scatter::relax_points(&mut pts, normals.as_deref(), radius, iterations);
            for (i, q) in pts.into_iter().enumerate() {
                geom.set_pos(i, q);
            }
        }
        relax_transfer(&mut geom, root, target, visited, ocl_error, sim);
        return Some(geom);
    }

    // Sibling-first, as the collider is: a simnet's `Rest: input1` has to
    // be ITS input1, not the first one in the tree.
    let Some(rest_node) = param_node(root, target, "Rest") else { return Some(geom) };
    let Some(rest) = generate_single_node_geometry_with_errors(root, rest_node, visited, ocl_error, sim) else {
        return Some(geom);
    };
    if rest.num_points() != geom.num_points() || geom.is_empty() {
        // Springs need the index correspondence; the transfer does not.
        relax_transfer(&mut geom, root, target, visited, ocl_error, sim);
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

    let sys = crate::springs::SpringSystem::build(&rest, &pinned);
    let mut flat: Vec<f32> = pos.iter().flat_map(|v| [v.x, v.y, v.z]).collect();
    if let Err(e) = crate::springs::solve(&sys, stiffness, iterations, &mut flat) {
        if ocl_error.is_none() {
            *ocl_error = Some(format!("{}: {e}", target.name));
        }
    }
    for (p, v) in pos.iter_mut().enumerate() {
        *v = Vec3::new(flat[p * 3], flat[p * 3 + 1], flat[p * 3 + 2]);
    }

    for (p, v) in pos.iter().enumerate() {
        geom.set_pos(p, *v);
    }
    relax_transfer(&mut geom, root, target, visited, ocl_error, sim);
    Some(geom)
}

/// The Relax node's copy of the Transfer node, `Transfer From Rest`: once
/// the points have moved, the Rest geometry's attributes and groups laid
/// over them by nearest point (`transfer_onto`), in either mode. What it
/// is for: a chain whose remesh renumbers, splits and collapses points
/// can keep a group alive — the pull's — by reading it back off a rest
/// shape that still carries it, at every step, without a Transfer node
/// wired in beside the relax.
fn relax_transfer(
    geom: &mut Detail,
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) {
    if !node_param_bool(target, "Transfer From Rest", false) {
        return;
    }
    let Some(rest_node) = param_node(root, target, "Rest") else {
        if ocl_error.is_none() {
            *ocl_error = Some(format!("{}: Transfer From Rest needs a Rest", target.name));
        }
        return;
    };
    let Some(rest) = generate_single_node_geometry_with_errors(root, rest_node, visited, ocl_error, sim) else { return };
    let groups = node_param_bool(target, "Transfer Groups", false).then(|| name_list(&node_param_str(target, "Groups", "")));
    transfer_onto(
        geom,
        &rest,
        &name_list(&node_param_str(target, "Attributes", "")),
        groups.as_deref(),
        node_param_f32(target, "Maximum Distance", 0.0),
        "",
    );
}

// ---------------------------------------------------------------------------
// The Immutable Methods set: measure and filter.
//
// None of these carry a description in hou-control — the IM family has no
// prose anywhere — but unlike `developer_surface_adapt`, their names say
// exactly what they are. A node called Normal computes normals. What is
// implemented here is the plain reading of each name, with the parameter
// names the audit recovered from the HDAs where it had them (`piece_attr` on
// Connectivity, `dir_attr` on Distance).
// ---------------------------------------------------------------------------

/// The Normal node: the surface normal as an attribute you can read.
///
/// The normal has been computed inside the app for a while — the viewport
/// whiskers draw it, Develop displaces along it, Align's Surface Tangent
/// projects onto it — but nothing could get at it. As an attribute it becomes
/// ordinary data: a kernel can read it through the Phase 1 ABI, Align can
/// steer toward it, Visualize can colour by it, Migrate can flow along it.
pub fn resolve_normal_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    let name = node_param_str(target, "Attribute", "N").trim().to_string();
    if name.is_empty() {
        return Some(geom);
    }
    let flip = node_param_bool(target, "Flip", false);
    let sign = if flip { -1.0 } else { 1.0 };
    // A node without the Class row is one from before it, and writes the
    // points' as it always did.
    if node_param_str(target, "Class", "Points").trim().eq_ignore_ascii_case("Vertices") {
        let cusp = node_param_f32(target, "Cusp Angle", 60.0);
        let normals: Vec<[f32; 3]> = vertex_normals(&geom, cusp).iter().map(|n| (*n * sign).to_array()).collect();
        geom.verts_mut().create(&name, AttribValue::Float3([0.0; 3]));
        let _ = geom.verts_mut().insert(&name, AttribData::Float3(normals));
        return Some(geom);
    }
    let normals: Vec<[f32; 3]> = point_normals(&geom).iter().map(|n| (*n * sign).to_array()).collect();
    geom.points_mut().create(&name, AttribValue::Float3([0.0; 3]));
    let _ = geom.points_mut().insert(&name, AttribData::Float3(normals));
    Some(geom)
}

/// The Bounds node: the geometry's extent, as detail attributes.
///
/// Four of them — `_min`, `_max`, `_size`, `_center` — rather than one box
/// type, for the reason Analysis writes six numbers instead of a dictionary:
/// everything downstream can already read a detail attribute, and nothing has
/// to learn a new shape.
pub fn resolve_bounds_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let prefix = node_param_str(target, "Prefix", "bounds").trim().to_string();
    if prefix.is_empty() {
        return Some(geom);
    }
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let pts: Vec<Vec3> = (0..geom.num_points())
        .filter(|&p| group.is_empty() || geom.points().in_group(&group, p))
        .map(|p| geom.pos(p))
        .collect();
    let (lo, hi) = if pts.is_empty() {
        (Vec3::ZERO, Vec3::ZERO)
    } else {
        pts.iter().fold((pts[0], pts[0]), |(a, b), &p| (a.min(p), b.max(p)))
    };
    // Derivative: a measurement describes the state it was taken from.
    for (suffix, v) in [
        ("min", lo),
        ("max", hi),
        ("size", hi - lo),
        ("center", (lo + hi) * 0.5),
    ] {
        geom.detail_mut().create_kind(
            &format!("{}_{}", prefix, suffix),
            AttribValue::Float3(v.to_array()),
            crate::detail::AttribKind::Derivative,
        );
    }
    Some(geom)
}

/// The Distance node: how far each point is from another piece of geometry.
///
/// The measurement a chain drives proximity growth from — and, with Direction
/// written too, the vector Migrate flows along and Align steers by. Both come
/// out of one surface lookup, which is why they are one node.
///
/// Signed uses the nearest face's normal rather than casting a ray: constant
/// time instead of a pass over every triangle. It reads the wrong way inside a
/// concave crease, where the nearest face is not the one you are behind, and
/// that is the trade — a ray cast is exact and turns this node from a lookup
/// into a full intersection test per point.
pub fn resolve_distance_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let to_name = node_param_node(target, "To").unwrap_or_default();
    let Some(other) = find_input_node(root, target, &to_name)
        .and_then(|n| generate_single_node_geometry_with_errors(root, n, visited, ocl_error, sim))
    else {
        if ocl_error.is_none() && !to_name.is_empty() {
            *ocl_error = Some(format!("Distance '{}': cannot resolve '{}'", target.name, to_name));
        }
        return Some(geom);
    };

    let name = node_param_str(target, "Attribute", "dist").trim().to_string();
    let dir_name = node_param_str(target, "Direction", "");
    let dir_name = dir_name.trim().to_string();
    let signed = node_param_bool(target, "Signed", false);
    // Zero means no clamp: a maximum is for keeping a falloff bounded, and a
    // node whose default quietly flattened every measurement to zero would be
    // a trap.
    let maximum = node_param_f32(target, "Maximum", 0.0).max(0.0);

    let grid = crate::spatial::TriGrid::build(&other);
    // A point ON the surface has no direction to it, and the vector between
    // them is pure float error. Normalizing that would hand the chain a
    // heading made of nothing — the same mistake Align's Surface Tangent made
    // before it was caught, so the guard is scaled to the geometry rather than
    // being an exact-zero test.
    let scale = other
        .bounds()
        .map(|(lo, hi)| (hi - lo).length())
        .unwrap_or(1.0)
        .max(1.0);
    let eps = scale * 1e-6;

    let n = geom.num_points();
    let mut dists = vec![0.0f32; n];
    let mut dirs = vec![[0.0f32; 3]; n];
    for p in 0..n {
        let here = geom.pos(p);
        let Some(hit) = grid.closest(here) else { continue };
        let away = here - hit.point;
        let mut d = hit.distance;
        if signed && away.dot(hit.normal) < 0.0 {
            d = -d;
        }
        if maximum > 0.0 {
            d = d.clamp(-maximum, maximum);
        }
        dists[p] = d;
        if hit.distance > eps {
            dirs[p] = (-away).normalize().to_array();
        }
    }

    if !name.is_empty() {
        geom.points_mut().create(&name, AttribValue::Float(0.0));
        let _ = geom.points_mut().insert(&name, AttribData::Float(dists));
    }
    if !dir_name.is_empty() {
        geom.points_mut().create(&dir_name, AttribValue::Float3([0.0; 3]));
        let _ = geom.points_mut().insert(&dir_name, AttribData::Float3(dirs));
    }
    Some(geom)
}

/// The Connectivity node: which connected piece each point belongs to.
///
/// Pieces are numbered by SIZE, largest first, so piece 0 is the main body
/// however the points happen to be ordered. That is what makes "keep the
/// largest piece" a Cull with a threshold rather than a special operator —
/// and the audit shows `isolate_largest` and `extract_longest` were exactly
/// what the GEM mold chain used `im_cull` for.
pub fn resolve_connectivity_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    apply_connectivity(&mut geom, target);
    Some(geom)
}

/// Exposed for tests that build their geometry by hand rather than by graph.
pub fn apply_connectivity_for_test(geom: &mut Detail, target: &FsNode, _err: &mut Option<String>) {
    apply_connectivity(geom, target);
}

pub(crate) fn apply_connectivity(geom: &mut Detail, target: &FsNode) {
    let name = node_param_str(target, "Attribute", "piece").trim().to_string();
    if name.is_empty() {
        return;
    }

    let n = geom.num_points();
    let mut label = vec![u32::MAX; n];
    let mut sizes: Vec<(u32, usize)> = Vec::new();
    for seed in 0..n {
        if label[seed] != u32::MAX {
            continue;
        }
        let id = sizes.len() as u32;
        let mut count = 0usize;
        let mut stack = vec![seed];
        label[seed] = id;
        while let Some(p) = stack.pop() {
            count += 1;
            for &q in geom.point_neighbours(p) {
                if label[q as usize] == u32::MAX {
                    label[q as usize] = id;
                    stack.push(q as usize);
                }
            }
        }
        sizes.push((id, count));
    }

    // Renumber by size, descending. Ties break on the original label so the
    // answer does not depend on sort stability.
    sizes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut rank = vec![0i32; sizes.len()];
    for (r, (id, _)) in sizes.iter().enumerate() {
        rank[*id as usize] = r as i32;
    }

    let data: Vec<i32> = label.iter().map(|&l| rank[l as usize]).collect();
    geom.points_mut().create(&name, AttribValue::Int(0));
    let _ = geom.points_mut().insert(&name, AttribData::Int(data));
}

/// The Cull node: remove points, and the primitives that needed them.
///
/// Selection is the intersection of a Group and an attribute comparison, and
/// what is selected is DELETED — the name says remove. Invert keeps the
/// selection instead, which is how "isolate the largest piece" reads: a
/// Connectivity, then a Cull inverted on `piece` below 1.
///
/// Nothing else in the app deletes geometry, which is why this one is in the
/// first five.
pub fn resolve_cull_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let attr = node_param_str(target, "Attribute", "");
    let attr = attr.trim().to_string();
    if group.is_empty() && attr.is_empty() {
        return Some(geom);
    }
    if !attr.is_empty() && !geom.points().has(&attr) {
        if ocl_error.is_none() {
            *ocl_error = Some(format!(
                "Cull '{}': no point attribute named '{}'",
                target.name, attr
            ));
        }
        return Some(geom);
    }

    let below = node_param_str(target, "Comparison", "Below").eq_ignore_ascii_case("below");
    let threshold = node_param_f32(target, "Threshold", 0.5);
    let invert = node_param_bool(target, "Invert", false);

    let selected: Vec<bool> = (0..geom.num_points())
        .map(|p| {
            let in_group = group.is_empty() || geom.points().in_group(&group, p);
            let passes = attr.is_empty()
                || geom
                    .points()
                    .value(&attr, p)
                    .map(|v| if below { v.as_f32() < threshold } else { v.as_f32() > threshold })
                    .unwrap_or(false);
            (in_group && passes) != invert
        })
        .collect();

    let keep: Vec<bool> = selected.iter().map(|&s| !s).collect();
    geom.keep_points(&keep);
    Some(geom)
}

/// The Volume node: offset or shell a surface through a distance field.
///
/// Offsetting a mesh directly means resolving every self-intersection the move
/// creates; through a field it is a subtraction and the result is closed by
/// construction. Shell is the same trick twice — the shape minus the shape
/// moved inward — which is what a mold wall is.
///
/// The cost is resolution: the result is a surface extracted from a grid, so
/// detail finer than the Voxel Size is gone. That is the trade the
/// representation makes, and it is why this is a node rather than something
/// applied silently.
pub fn resolve_volume_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    if geom.num_prims() == 0 {
        return Some(geom);
    }

    let voxel = node_param_f32(target, "Voxel Size", 0.05).max(1e-3);
    let offset = node_param_f32(target, "Offset", 0.0);
    let shell = node_param_str(target, "Mode", "Offset").eq_ignore_ascii_case("shell");
    let thickness = node_param_f32(target, "Thickness", 0.05).max(1e-4);

    // Room for everything the operation will ask the field to reach: the
    // offset itself, and for a shell the wall's thickness beyond it.
    let want = offset.abs() + if shell { thickness } else { 0.0 } + voxel * 2.0;
    let Some((lo, hi)) = crate::volume::Volume::bounds_for(&geom, want) else {
        return Some(geom);
    };
    // A grid is capped at 256 samples an axis, so a voxel size far too small
    // for the model silently gives a coarse answer. Saying so beats a result
    // that looks like the node is broken.
    let cells = ((hi - lo).max_element() / voxel).ceil();
    if cells > 256.0 && ocl_error.is_none() {
        *ocl_error = Some(format!(
            "Volume '{}': voxel {:.3} needs {} samples across, over the 256 cap — the result is coarser than asked",
            target.name, voxel, cells as i64
        ));
    }

    let mut vol = crate::volume::Volume::build(&geom, lo, hi, voxel, want);
    if shell {
        // The wall between the offset surface and the same surface moved in by
        // Thickness: intersect what is inside the outer with what is outside
        // the inner.
        let mut inner = vol.clone();
        inner.offset(offset - thickness);
        vol.offset(offset);
        vol.subtract(&inner);
    } else {
        vol.offset(offset);
    }
    Some(vol.to_mesh())
}

/// The Boolean node: union, intersection and difference through a field.
///
/// Both inputs are sampled over ONE grid covering both, which is what makes
/// the operation elementwise — a minimum, a maximum, a maximum against a
/// negation. A mesh boolean spends its whole length finding intersection
/// curves and stitching; this cannot produce an open surface because there is
/// no stitching to get wrong.
pub fn resolve_boolean_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let a = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let with_name = node_param_node(target, "With").unwrap_or_default();
    let Some(b) = find_input_node(root, target, &with_name)
        .and_then(|n| generate_single_node_geometry_with_errors(root, n, visited, ocl_error, sim))
    else {
        if ocl_error.is_none() && !with_name.is_empty() {
            *ocl_error = Some(format!("Boolean '{}': cannot resolve '{}'", target.name, with_name));
        }
        return Some(a);
    };
    if a.num_prims() == 0 || b.num_prims() == 0 {
        return Some(a);
    }

    let voxel = node_param_f32(target, "Voxel Size", 0.05).max(1e-3);
    let op = node_param_str(target, "Operation", "Union").to_lowercase();
    // One grid over BOTH, so the two fields line up sample for sample.
    let (alo, ahi) = a.bounds()?;
    let (blo, bhi) = b.bounds()?;
    let pad = Vec3::splat(voxel * 3.0);
    let (lo, hi) = (alo.min(blo) - pad, ahi.max(bhi) + pad);

    let mut va = crate::volume::Volume::build(&a, lo, hi, voxel, voxel * 3.0);
    let vb = crate::volume::Volume::build(&b, lo, hi, voxel, voxel * 3.0);
    match op.as_str() {
        "intersect" => va.intersect(&vb),
        "subtract" => va.subtract(&vb),
        _ => va.union(&vb),
    }
    Some(va.to_mesh())
}

/// The Mold Shell node: a cast's shell, thickened by curvature.
pub fn resolve_mold_shell_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let input = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    let shell = crate::mold::mold_shell(
        &input,
        node_param_f32(target, "Minimum Thickness", 0.6),
        node_param_f32(target, "Maximum Thickness", 0.75),
        node_param_f32(target, "Remesh Division Size", 0.9),
        crate::mold::Ramp::parse(&node_param_str(target, "Ramp", "Linear")),
    );
    // A node with nothing to thicken passes its input through rather than
    // vanishing: an empty result in the middle of a chain reads as a broken
    // node, and the thing that is actually wrong is upstream.
    Some(shell.unwrap_or(input))
}

/// The Hull node: the convex hull of the input's points, as a closed
/// triangle mesh (`crate::hull`). Points that do not span a volume — fewer
/// than four, or all coplanar — pass through as they are, so a scatter too
/// thin to hull is at least still visible.
pub fn resolve_hull_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let input = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    let pts: Vec<Vec3> = (0..input.num_points()).map(|i| input.pos(i)).collect();
    Some(crate::hull::convex_hull(&pts).unwrap_or(input))
}

/// The Wrangle node: a Rhai script run once per element (`src/wrangle.rs`).
///
/// A wrangle with no input still runs — in Detail class, once, which is how
/// a script builds geometry from nothing with `addpoint` / `addprim`. The
/// channels the script names as literals are resolved HERE, before the run,
/// through the expression scope: `ch("../Radius")` on a parameter that is
/// itself an expression sees the evaluated value, and neither language has
/// to know the other exists. A failing script reports through the error
/// slot and the input passes through unchanged.
pub fn resolve_wrangle_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input = match param_node(root, target, "Input") {
        Some(n) => generate_single_node_geometry_with_errors(root, n, visited, ocl_error, sim).unwrap_or_default(),
        None => Detail::new(),
    };
    let code = node_param_str(target, "Code", "");
    let class = crate::wrangle::parse_class(&node_param_str(target, "Class", "Points"));
    let group = node_param_str(target, "Group", "");

    let mut chans = std::collections::HashMap::new();
    {
        use crate::expr::Scope as _;
        let mut scope = TreeScope::new(root, target, sim.frame);
        for path in crate::wrangle::channel_refs(&code) {
            let r = scope
                .channel(&path, ChKind::Float)
                .and_then(|n| scope.channel(&path, ChKind::Str).map(|s| crate::wrangle::Chan::new(n.as_num(), s.as_str())));
            chans.insert(path, r);
        }
    }

    match crate::wrangle::run_wrangle(input.clone(), &code, class, &group, sim.frame, chans) {
        Ok(d) => Some(d),
        Err(e) => {
            if ocl_error.is_none() {
                *ocl_error = Some(format!("{}: {e}", target.name));
            }
            Some(input)
        }
    }
}

/// The Extrude node: the input extruded as a whole along its point normals
/// (`crate::shapes::extrude_detail`).
pub fn resolve_extrude_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let input = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    let distance = node_param_f32(target, "Distance", 0.2);
    let keep_base = node_param_bool(target, "Keep Base", true);
    Some(crate::shapes::extrude_detail(&input, distance, keep_base))
}

/// An `opencl` node from a save older than 2026-09-24, when the node and
/// the runtime behind it were retired (`shapeshifter.md`, Phase 7 step 3).
/// It passes its input through unchanged and reports itself through the
/// error slot — visible, not silently dropped — so the fix is one rewrite
/// as a wrangle rather than a hunt for geometry that stopped appearing.
pub fn retired_opencl_node(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    if ocl_error.is_none() {
        *ocl_error = Some(format!("{}: OpenCL nodes are retired; rewrite the kernel as a wrangle", target.name));
    }
    let input_node = param_node(root, target, "Input")?;
    generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)
}

/// The Switch node: one of up to four inputs, chosen by Index.
///
/// What a composed subnet puts behind a choice: the Embryo's Source is a
/// switch whose Index reads `chi("Source")`, so Internal is input 0 and
/// Input is input 1. Nothing else about it — it passes the chosen geometry
/// through untouched, and an empty slot passes nothing.
///
/// The inputs are `Input` and `Input 2` … `Input 4`. Only `Input` draws a
/// wire, the same limit every second operand has (Boolean's With, Copy's
/// target); when those become wires these should too.
pub const SWITCH_INPUTS: usize = 4;

pub fn switch_input_param(index: usize) -> String {
    if index == 0 { "Input".to_string() } else { format!("Input {}", index + 1) }
}

pub fn resolve_switch_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let index = (node_param_f32(target, "Index", 0.0).round().max(0.0) as usize).min(SWITCH_INPUTS - 1);
    let input_node = param_node(root, target, &switch_input_param(index))?;
    generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)
}

/// The Export node: geometry out of the app.
///
/// A pass-through in the chain — it hands its input straight on, so it can sit
/// anywhere rather than only at the end — that writes a file when its Export
/// button is pressed. NOT when it evaluates: evaluation happens on every
/// redraw and every frame of a solve, and a node that wrote a file each time
/// would fill a disk while you scrubbed the timeline.
pub fn resolve_export_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)
}

/// The format and scale an Export node is configured for.
pub fn export_settings(target: &FsNode) -> (crate::export::Format, f32) {
    let format = match node_param_str(target, "Format", "STL").as_str() {
        "OBJ" => crate::export::Format::Obj,
        "STL (ASCII)" => crate::export::Format::StlAscii,
        _ => crate::export::Format::StlBinary,
    };
    (format, node_param_f32(target, "Scale", 1.0).max(1e-6))
}

/// The Grid node: a flat sheet of quads in the XZ plane.
///
/// Native from the start, where Plane was an OpenCL subnet until 2026-09-24
/// (it is native too now, in `crate::shapes`). Both make a sheet of quads;
/// Plane keeps its three Center sliders and its colour gradient, this one a
/// float3 Center and no colour. Two nodes for history's sake.
///
/// Placed at Center rather than by its position in the graph. The older
/// generators offset themselves by an index so several of them do not stack,
/// which is surprising the first time and unadjustable after; a parameter says
/// what it is.
pub fn grid_detail(target: &FsNode) -> Detail {
    let rows = node_param_f32(target, "Rows", 10.0).clamp(1.0, 500.0) as usize;
    let cols = node_param_f32(target, "Columns", 10.0).clamp(1.0, 500.0) as usize;
    let width = node_param_f32(target, "Width", 1.0).max(1e-4);
    let length = node_param_f32(target, "Length", 1.0).max(1e-4);
    let centre = node_param_vec3(target, "Center", Vec3::ZERO);

    let mut d = Detail::new();
    for r in 0..=rows {
        for c in 0..=cols {
            let u = c as f32 / cols as f32 - 0.5;
            let v = r as f32 / rows as f32 - 0.5;
            d.add_point(centre + Vec3::new(u * width, 0.0, v * length));
        }
    }
    let at = |r: usize, c: usize| (r * (cols + 1) + c) as u32;
    // Wound counter-clockwise seen from +Y, so the plain cross points up —
    // the same convention the sphere and the template meshes follow.
    for r in 0..rows {
        for c in 0..cols {
            d.add_prim(&[at(r, c), at(r + 1, c), at(r + 1, c + 1), at(r, c + 1)]);
        }
    }
    d
}

/// The Polygon node: a regular n-gon, a star, or a disc.
///
/// One node for four of the plugin's Create operators — `im_square`,
/// `im_triangle`, `im_star` and the circle nobody got round to — because they
/// are one shape with one parameter varying. Three sides is a triangle, four
/// is a square, thirty-two is a circle, and a non-zero Inner Radius makes any
/// of them a star by pulling every other point inward.
///
/// Filled fans from a CENTRE POINT rather than from the first corner. A fan
/// from a corner is fine on a convex polygon and wrong on a star: the
/// triangles cross the concave notches and the shape renders as its own convex
/// hull.
pub fn polygon_detail(target: &FsNode) -> Detail {
    let sides = node_param_f32(target, "Sides", 4.0).clamp(3.0, 256.0) as usize;
    let radius = node_param_f32(target, "Radius", 0.5).max(1e-4);
    let inner = node_param_f32(target, "Inner Radius", 0.0).max(0.0);
    let fill = node_param_bool(target, "Fill", true);
    let centre = node_param_vec3(target, "Center", Vec3::ZERO);

    let mut d = Detail::new();
    // A star alternates between the two radii, so it has twice the corners.
    let star = inner > 0.0;
    let corners = if star { sides * 2 } else { sides };
    let ring: Vec<u32> = (0..corners)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / corners as f32;
            let r = if star && i % 2 == 1 { inner } else { radius };
            d.add_point(centre + Vec3::new(r * a.cos(), 0.0, -r * a.sin()))
        })
        .collect();

    if fill {
        let hub = d.add_point(centre);
        for i in 0..corners {
            d.add_prim(&[hub, ring[i], ring[(i + 1) % corners]]);
        }
    } else {
        // Two-point primitives: the outline as edges, which Detail's topology
        // reads as a closed loop and nothing tries to shade.
        for i in 0..corners {
            d.add_prim(&[ring[i], ring[(i + 1) % corners]]);
        }
    }
    d
}

/// The Transfer node: carry attributes from one geometry onto another./// The Transfer node: carry attributes from one geometry onto another.
///
/// How a field outlives the geometry it was defined on. A remesh keeps values
/// for the points that survived, but a chain that REBUILDS — a kernel
/// generator, a Copy, a fresh Scatter — starts from nothing, and this is what
/// samples the old state onto the new one.
///
/// Matching is by nearest POINT, not nearest surface position. A surface match
/// would interpolate across the triangle a point lands in and read a little
/// better on a coarse source; nearest-point is predictable, which matters more
/// when the thing being transferred is a simulation's state and a wrong value
/// is a wrong simulation rather than a slightly wrong colour.
///
/// Attributes names a comma-separated list, or takes every point attribute on
/// the source when left empty. Maximum Distance of zero means no limit; above
/// zero, a target with nothing near enough keeps whatever it had.
pub fn resolve_transfer_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let from_name = node_param_node(target, "From").unwrap_or_default();
    let Some(source) = find_input_node(root, target, &from_name)
        .and_then(|n| generate_single_node_geometry_with_errors(root, n, visited, ocl_error, sim))
    else {
        if ocl_error.is_none() && !from_name.is_empty() {
            *ocl_error = Some(format!("Transfer '{}': cannot resolve '{}'", target.name, from_name));
        }
        return Some(geom);
    };
    if source.num_points() == 0 {
        return Some(geom);
    }

    let groups = node_param_bool(target, "Transfer Groups", false).then(|| name_list(&node_param_str(target, "Groups", "")));
    transfer_onto(
        &mut geom,
        &source,
        &name_list(&node_param_str(target, "Attributes", "")),
        groups.as_deref(),
        node_param_f32(target, "Maximum Distance", 0.0),
        node_param_str(target, "Group", "").trim(),
    );
    Some(geom)
}

/// A comma list of names, trimmed, the empty ones dropped.
pub(crate) fn name_list(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// The Transfer node's work, and the Relax node's copy of it: onto each
/// point of `geom` (in `only`, when that names a group; within `limit` of
/// its nearest source point, when that is above zero), the nearest point
/// of `source`'s attributes and group memberships.
///
/// `attributes` names the point attributes, every one the source has when
/// empty. `groups` names the point GROUPS — every one the source has when
/// empty, and none at all when `None`, which is what a node from before
/// groups could be transferred asks (2026-09-30). A membership is COPIED:
/// a point whose nearest source point is in the group joins it, and one
/// whose is not leaves it, so a group carried this way is the source's
/// group laid over the target and not a union with what the target had.
/// A group the target lacks is created, so it exists everywhere the
/// attribute columns do.
pub(crate) fn transfer_onto(
    geom: &mut Detail,
    source: &Detail,
    attributes: &[String],
    groups: Option<&[String]>,
    limit: f32,
    only: &str,
) {
    if source.num_points() == 0 || geom.num_points() == 0 {
        return;
    }
    let names: Vec<String> = if attributes.is_empty() {
        source.points().names().iter().map(|s| s.to_string()).collect()
    } else {
        attributes.iter().filter(|n| source.points().has(n)).cloned().collect()
    };
    let group_names: Vec<String> = match groups {
        None => Vec::new(),
        Some(wanted) if wanted.is_empty() => source.points().group_names().iter().map(|s| s.to_string()).collect(),
        Some(wanted) => wanted.iter().filter(|g| source.points().has_group(g)).cloned().collect(),
    };
    if names.is_empty() && group_names.is_empty() {
        return;
    }
    let limit = limit.max(0.0);
    let src_pos: Vec<Vec3> = (0..source.num_points()).map(|p| source.pos(p)).collect();
    let grid = crate::spatial::PointGrid::build(&src_pos, limit.max(1e-3));
    // Each target point's source, found once for every column.
    let from: Vec<Option<usize>> = (0..geom.num_points())
        .map(|p| {
            if !only.is_empty() && !geom.points().in_group(only, p) {
                return None;
            }
            let (q, dist) = grid.nearest(geom.pos(p))?;
            (limit <= 0.0 || dist <= limit).then_some(q as usize)
        })
        .collect();

    for name in &names {
        let Some(ty) = source.points().get(name).map(|a| a.ty()) else { continue };
        // Created with the source's type so the column exists everywhere even
        // where nothing was near enough to fill it — a reader downstream finds
        // the attribute present and zero rather than missing.
        geom.points_mut()
            .get_or_create(name, components_attrib(ty, &vec![0.0; ty.components()]));
        geom.points_mut().set_kind(name, source.points().kind(name));
        for (p, q) in from.iter().enumerate() {
            let Some(q) = q else { continue };
            if let Some(v) = source.points().value(name, *q) {
                let _ = geom.points_mut().set_value(name, p, v);
            }
        }
    }
    for g in &group_names {
        if !geom.points().has_group(g) {
            geom.points_mut().create_group(g);
        }
        for (p, q) in from.iter().enumerate() {
            let Some(q) = q else { continue };
            geom.points_mut().set_in_group(g, p, source.points().in_group(g, *q));
        }
    }
}

/// The Valence node: how connected each point is, as data.
///
/// Valence is what the remesher steers toward — six is a regular
/// triangulation — so being able to see it is how you tell a mesh that has
/// settled from one that has not. Ramp it through Visualize and the irregular
/// vertices light up.
pub fn resolve_valence_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    let name = node_param_str(target, "Attribute", "valence").trim().to_string();
    if name.is_empty() {
        return Some(geom);
    }
    let by_prims = node_param_str(target, "Measure", "Neighbours").eq_ignore_ascii_case("primitives");
    let data: Vec<i32> = (0..geom.num_points())
        .map(|p| {
            if by_prims {
                geom.point_prims(p).len() as i32
            } else {
                geom.point_neighbours(p).len() as i32
            }
        })
        .collect();
    geom.points_mut().create(&name, AttribValue::Int(0));
    let _ = geom.points_mut().insert(&name, AttribData::Int(data));
    Some(geom)
}

/// The Deform node: twist, bend and taper about an axis.
///
/// Three operators in hou-control — `im_twist`, `im_bend`, `im_curl` — and one
/// here, because they are the same shape: a transform whose strength varies
/// with how far along an axis a point sits. Only what varies differs.
///
/// The position along the axis is NORMALIZED against the geometry's own extent,
/// so Amount means the same thing on a model of any size and a deform set up on
/// a rough shape survives that shape growing.
pub fn resolve_deform_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    apply_deform(&mut geom, target);
    Some(geom)
}

pub(crate) fn apply_deform(geom: &mut Detail, target: &FsNode) {
    let Some((lo, hi)) = geom.bounds() else { return };
    let axis = match node_param_str(target, "Axis", "Y").to_uppercase().as_str() {
        "X" => 0,
        "Z" => 2,
        _ => 1,
    };
    let (u, v) = match axis {
        0 => (1, 2),
        2 => (0, 1),
        _ => (0, 2),
    };
    let span = (hi - lo)[axis];
    if span.abs() < 1e-9 {
        return;
    }
    let amount = node_param_f32(target, "Amount", 1.0);
    let mode = node_param_str(target, "Mode", "Twist").to_lowercase();
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let centre = (lo + hi) * 0.5;

    for p in 0..geom.num_points() {
        if !group.is_empty() && !geom.points().in_group(&group, p) {
            continue;
        }
        let here = geom.pos(p);
        // Minus a half so the middle of the geometry is the still point and
        // the two ends deform in opposite directions, which is what makes a
        // twist read as a twist rather than a rotation.
        let t = (here[axis] - lo[axis]) / span - 0.5;
        let (du, dv) = (here[u] - centre[u], here[v] - centre[v]);
        let mut out = here;
        match mode.as_str() {
            "bend" => {
                // Rotate in the plane of the axis and one perpendicular, by an
                // angle that grows along the axis.
                let a = amount * t;
                let (s, c) = a.sin_cos();
                let along = here[axis] - centre[axis];
                out[axis] = centre[axis] + along * c - du * s;
                out[u] = centre[u] + along * s + du * c;
            }
            "taper" => {
                // Scale the perpendicular components. Clamped at zero so a
                // large Amount pinches to a point instead of turning the
                // geometry inside out.
                let k = (1.0 + amount * t).max(0.0);
                out[u] = centre[u] + du * k;
                out[v] = centre[v] + dv * k;
            }
            _ => {
                let a = amount * t;
                let (s, c) = a.sin_cos();
                out[u] = centre[u] + du * c - dv * s;
                out[v] = centre[v] + du * s + dv * c;
            }
        }
        geom.set_pos(p, out);
    }
}

/// The Copy node: one piece of geometry at every point of another./// The Copy node: one piece of geometry at every point of another.
///
/// The layout operator — scatter real geometry rather than the marker spheres
/// the Points and Scatter nodes draw. Orient reads the target's `N`, so a
/// Normal node upstream is what makes copies stand on a surface rather than
/// all facing the same way, and Scale Attribute lets a field drive their size.
///
/// Identities are reallocated per copy by `merge`, so a thousand copies are a
/// thousand distinct sets of points rather than one set repeated — which is
/// what lets a solver treat them separately.
pub fn resolve_copy_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let source_node = param_node(root, target, "Input")?;
    let source = generate_single_node_geometry_with_errors(root, source_node, visited, ocl_error, sim)?;

    let to_name = node_param_node(target, "To").unwrap_or_default();
    let Some(onto) = find_input_node(root, target, &to_name)
        .and_then(|n| generate_single_node_geometry_with_errors(root, n, visited, ocl_error, sim))
    else {
        if ocl_error.is_none() && !to_name.is_empty() {
            *ocl_error = Some(format!("Copy '{}': cannot resolve '{}'", target.name, to_name));
        }
        return Some(source);
    };

    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let targets: Vec<usize> = (0..onto.num_points())
        .filter(|&p| group.is_empty() || onto.points().in_group(&group, p))
        .collect();

    // A copy of a 400-point sphere onto a 10,000-point surface is four million
    // points, which is not a render — it is a hang. Refused with a number
    // rather than attempted.
    const CEILING: usize = 2_000_000;
    let total = source.num_points().saturating_mul(targets.len());
    if total > CEILING {
        if ocl_error.is_none() {
            *ocl_error = Some(format!(
                "Copy '{}': {} copies of {} points is {} — over the {} ceiling",
                target.name,
                targets.len(),
                source.num_points(),
                total,
                CEILING
            ));
        }
        return Some(Detail::new());
    }

    let orient = node_param_str(target, "Orient", "None").eq_ignore_ascii_case("normal");
    let scale = node_param_f32(target, "Scale", 1.0);
    let scale_attr = node_param_str(target, "Scale Attribute", "");
    let scale_attr = scale_attr.trim().to_string();
    let normals = orient.then(|| point_normals(&onto));

    let mut out = Detail::new();
    for &t in &targets {
        let mut inst = source.clone();
        let mut s = scale;
        if !scale_attr.is_empty() {
            if let Some(v) = onto.points().value(&scale_attr, t) {
                s *= v.as_f32();
            }
        }
        let rot = match &normals {
            // Whatever rotation takes +Y onto the point's normal. Copies
            // modelled standing up therefore stand up on the surface.
            Some(n) if n[t] != Vec3::ZERO => {
                glam::Quat::from_rotation_arc(Vec3::Y, n[t])
            }
            _ => glam::Quat::IDENTITY,
        };
        let at = onto.pos(t);
        for p in 0..inst.num_points() {
            inst.set_pos(p, at + rot * (inst.pos(p) * s));
        }
        out.merge(&inst);
    }
    Some(out)
}

/// The Soft Transform node: move a region and let the surface follow.
///
/// A hard translation on a group leaves a step at the group's edge. This falls
/// off with distance, so the neighbourhood comes along and the surface stays
/// continuous — the modelling counterpart of what Relax does after a pull.
///
/// Distance is measured from the nearest member of Group where one is given,
/// not from the group's centre: a selection shaped like a ridge should drag
/// its whole length, and a centroid would make it drag hardest in the middle
/// of a line it is nowhere near.
pub fn resolve_soft_transform_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    apply_soft_transform(&mut geom, target);
    Some(geom)
}

pub(crate) fn apply_soft_transform(geom: &mut Detail, target: &FsNode) {
    let n = geom.num_points();
    if n == 0 {
        return;
    }
    let translation = node_param_vec3(target, "Translation", Vec3::ZERO);
    let radius = node_param_f32(target, "Radius", 0.5).max(0.0);
    let falloff = node_param_str(target, "Falloff", "Smooth").to_lowercase();
    let group = node_param_str(target, "Group", "");
    let group = group.trim().to_string();
    let write_attr = node_param_str(target, "Attribute", "");
    let write_attr = write_attr.trim().to_string();

    let members: Vec<Vec3> = (0..n)
        .filter(|&p| !group.is_empty() && geom.points().in_group(&group, p))
        .map(|p| geom.pos(p))
        .collect();
    let centre = node_param_vec3(target, "Center", Vec3::ZERO);
    let grid = (!members.is_empty()).then(|| crate::spatial::PointGrid::build(&members, radius.max(1e-4)));

    let mut weights = vec![0.0f32; n];
    let mut near = Vec::new();
    for p in 0..n {
        let here = geom.pos(p);
        let d = match &grid {
            Some(g) => {
                g.within(here, radius, &mut near);
                near.iter()
                    .map(|&i| (members[i as usize] - here).length())
                    .fold(f32::INFINITY, f32::min)
            }
            None => (here - centre).length(),
        };
        // Outside the radius nothing moves, and a zero radius moves only what
        // is exactly at the centre — a falloff with no width.
        let t = if radius <= 0.0 {
            if d <= 0.0 { 0.0 } else { 1.0 }
        } else {
            (d / radius).clamp(0.0, 1.0)
        };
        weights[p] = match falloff.as_str() {
            "constant" => if t < 1.0 { 1.0 } else { 0.0 },
            "linear" => 1.0 - t,
            // Smoothstep, so the moved region meets the still one with no
            // crease — which is the entire reason to prefer this to a hard
            // translate on a group.
            _ => 1.0 - (3.0 * t * t - 2.0 * t * t * t),
        };
    }

    for p in 0..n {
        let moved = geom.pos(p) + translation * weights[p];
        geom.set_pos(p, moved);
    }
    // The falloff is useful as data too: written out, it is the mask that
    // drove the move, ready for Visualize or for a second operator to reuse.
    if !write_attr.is_empty() {
        geom.points_mut().create(&write_attr, AttribValue::Float(0.0));
        let _ = geom.points_mut().insert(&write_attr, AttribData::Float(weights));
    }
}

/// The Subdivide node: four triangles where there was one./// The Subdivide node: four triangles where there was one.
pub fn resolve_subdivide_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let input_node = param_node(root, target, "Input")?;
    let geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    let depth = node_param_f32(target, "Depth", 1.0).clamp(0.0, 6.0) as usize;
    Some(crate::remesh::subdivide(&geom, depth))
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
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    // Inside a simnet that is mid-solve, what the substep consumed is where
    // the points were when the step began: the nearest simnet above this
    // node that has pushed a state.
    let mut before = None;
    let mut at = target;
    while let Some(parent) = find_parent_node(root, &at.id) {
        if let Some(state) = sim.feedback_for(&parent.id) {
            before = Some(state);
            break;
        }
        at = parent;
    }
    crate::detangle::apply_from(&mut geom, before, target);
    Some(geom)
}

#[cfg(test)]
pub(crate) fn apply_detangle(geom: &mut Detail, target: &FsNode) {
    crate::detangle::apply(geom, target);
}

/// The solve as it was first written, kept as what the one in
/// `detangle.rs` is held to: the same algorithm with nothing kept between
/// steps, every pass run, a grid per pass, every point searched for.
#[cfg(test)]
pub(crate) fn apply_detangle_reference(geom: &mut Detail, target: &FsNode) {
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
    let input_node = param_node(root, target, "Input")?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let against = param_node(root, target, "Against")
        .and_then(|n| generate_single_node_geometry_with_errors(root, n, visited, ocl_error, sim));
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
            let Some(hit) = grid.closest(here) else { continue };
            let (closest, dist) = (hit.point, hit.distance);
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
    let input_node = param_node(root, target, "Input")?;
    let geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;
    Some(crate::remesh::remesh(&geom, remesh_settings(target)))
}

pub(crate) fn remesh_settings(target: &FsNode) -> crate::remesh::Settings {
    crate::remesh::Settings {
        target: node_param_f32(target, "Target Length", 0.1).max(1e-4),
        iterations: node_param_f32(target, "Iterations", 3.0).clamp(1.0, 20.0) as usize,
        relax: node_param_f32(target, "Relax", 0.5),
        split: node_param_bool(target, "Split", true),
        collapse: node_param_bool(target, "Collapse", true),
        flip: node_param_bool(target, "Flip", true),
        project: node_param_bool(target, "Project", true),
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
    let input_node = param_node(root, target, "Input")?;
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

/// Whether `node` is an Attribute node that moves points — one writing the
/// built-in `Pos`. Those are the nodes whose effect the viewport draws as
/// pull arrows while one is selected.
pub fn moves_points(node: &FsNode) -> bool {
    node.node_type.eq_ignore_ascii_case("attribute")
        && node_param_str(node, "Attribute Name", "").trim().eq_ignore_ascii_case("Pos")
}

/// `target`'s geometry as the scene SHOWS it: a node inside a simnet as the
/// current frame's last substep saw it, with the feedback stack holding the
/// state that substep consumed, and any other node as it evaluates.
///
/// What reads a selected node for display goes through here — the
/// spreadsheet's rows, the markers on them, the selected group's. Until
/// 2026-09-29 they evaluated the node bare, so inside a simnet the `input`
/// child read the simnet's seed and the rows showed the first frame at
/// every frame, while the scene beside them played. The simnet is the
/// nearest one above the node, so a node in a subnet inside a simnet is
/// read the same way.
pub fn node_geometry_as_shown(
    root: &FsNode,
    target: &FsNode,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    let mut simnet = None;
    let mut at = target;
    while let Some(parent) = find_parent_node(root, &at.id) {
        if parent.node_type.eq_ignore_ascii_case("simnet") {
            simnet = Some(parent);
            break;
        }
        at = parent;
    }
    let mut pushed = false;
    if let Some(simnet) = simnet {
        if let Some(fed) = simnet_step_feedback(root, simnet, &mut Vec::new(), ocl_error, sim) {
            sim.feedback.push((simnet.id.clone(), fed));
            pushed = true;
        }
    }
    let geom = generate_single_node_geometry_with_errors(root, target, &mut Vec::new(), ocl_error, sim);
    if pushed {
        sim.feedback.pop();
    }
    geom
}

/// Where `target` moves each point it moves, as `(before, after)` positions:
/// its input's `P` against its own. Measured rather than read off Value, so
/// Set and Multiply — whose vector differs point to point — and an
/// expression-driven Value all come out as what actually happened, and the
/// affected points are exactly the ones that moved.
///
/// A node inside a simnet is evaluated as the current frame's LAST substep
/// saw it, with the feedback stack holding the state that substep consumed —
/// the same rule the dived-in scene walk draws by — so the arrows start
/// where the points were going into the pass that landed on the displayed
/// state, and end ON the displayed points when the pull is the chain's last
/// mover; not at the seed. Point counts that differ (the
/// node was rewired onto something that adds or removes points) give nothing,
/// since the indices no longer pair up.
pub fn point_displacements(root: &FsNode, target: &FsNode, sim: &mut EvalSim) -> Vec<(Vec3, Vec3)> {
    let Some(input_node) = param_node(root, target, "Input") else {
        return Vec::new();
    };
    let mut ocl_error = None;
    let simnet = find_parent_node(root, &target.id).filter(|p| p.node_type.eq_ignore_ascii_case("simnet"));
    let mut pushed = false;
    if let Some(simnet) = simnet {
        let mut visited = Vec::new();
        match simnet_step_feedback(root, simnet, &mut visited, &mut ocl_error, sim) {
            Some(fed) => {
                sim.feedback.push((simnet.id.clone(), fed));
                pushed = true;
            }
            None => return Vec::new(),
        }
    }
    let before = generate_single_node_geometry_with_errors(root, input_node, &mut Vec::new(), &mut ocl_error, sim);
    let after = generate_single_node_geometry_with_errors(root, target, &mut Vec::new(), &mut ocl_error, sim);
    if pushed {
        sim.feedback.pop();
    }
    let (Some(before), Some(after)) = (before, after) else { return Vec::new() };
    if before.num_points() != after.num_points() {
        return Vec::new();
    }
    (0..after.num_points())
        .map(|p| (before.pos(p), after.pos(p)))
        .filter(|(a, b)| (*b - *a).length_squared() > 1e-12)
        .collect()
}

/// Up to `k` of `points`, spread out: farthest-point sampling, starting from
/// the first point and repeatedly taking the one farthest from everything
/// taken so far. A pull on a thousand points reads from a dozen arrows as
/// well as from a thousand, and a thousand are a hedgehog that hides the
/// mesh; sampling by index instead would bunch wherever the numbering runs
/// locally, which on a scattered or remeshed surface is anywhere. Returned in
/// ascending index order; all of them when there are no more than `k`.
pub fn spread_sample(points: &[Vec3], k: usize) -> Vec<usize> {
    if points.len() <= k {
        return (0..points.len()).collect();
    }
    let mut chosen = Vec::with_capacity(k);
    let mut dist = vec![f32::INFINITY; points.len()];
    let mut next = 0;
    while chosen.len() < k {
        chosen.push(next);
        let at = points[next];
        let mut far = (0, -1.0_f32);
        for (i, d) in dist.iter_mut().enumerate() {
            *d = d.min(points[i].distance_squared(at));
            if *d > far.1 {
                far = (i, *d);
            }
        }
        if far.1 <= 0.0 {
            break; // every remaining point coincides with a chosen one
        }
        next = far.0;
    }
    chosen.sort_unstable();
    chosen
}

/// Arrows as LINE_LIST pairs: a shaft from each `from` to its `to`, and a
/// head of four strokes flaring back from the tip. The head is a fixed
/// fraction of the shaft, so an arrow's whole length is the true
/// displacement — the one number it exists to show.
pub fn arrow_vertices(pairs: &[(Vec3, Vec3)], color: [f32; 3]) -> Vec<Vertex3D> {
    let mut out = Vec::with_capacity(pairs.len() * 10);
    for &(from, to) in pairs {
        let d = to - from;
        let len = d.length();
        if len < 1e-6 {
            continue;
        }
        let dir = d / len;
        let (u, v) = dir.any_orthonormal_pair();
        let head = len * 0.25;
        let back = to - dir * head;
        let mut seg = |a: Vec3, b: Vec3| {
            out.push(Vertex3D { position: a.to_array(), color });
            out.push(Vertex3D { position: b.to_array(), color });
        };
        seg(from, to);
        for side in [u, -u, v, -v] {
            seg(to, back + side * head * 0.4);
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
    let input_node = param_node(root, target, "Input")?;
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
    let input_node = param_node(root, target, "Input")?;
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
    let input_node = param_node(root, target, "Input")?;
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
    let t = if node_param_bool(target, "Clamp", true) {
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

/// Vertex normals with a cusp: for each vertex — a corner of one primitive
/// — the normalized sum of the face normals of the primitives around its
/// point that lie within `cusp_degrees` of its OWN primitive's.
///
/// That is what a vertex normal can say and a point normal cannot: the
/// faces either side of a crease share the crease's points, and a point
/// has one normal to give them both. Where the faces around a point turn
/// less than the cusp angle from each other they are averaged and the
/// surface reads smooth; where they turn more, each keeps to its own side
/// and the edge reads hard. At 180 every face around the point is in, and a
/// vertex's normal is its point's (`point_normals`, the same sum, term for
/// term); at 0 it is its primitive's alone.
///
/// A primitive of fewer than three points has no face, and its vertices
/// get zero.
pub fn vertex_normals(geom: &Detail, cusp_degrees: f32) -> Vec<Vec3> {
    // The face normal as `point_normals` takes it, unnormalized, so the two
    // weigh the faces alike.
    let faces: Vec<Vec3> = (0..geom.num_prims())
        .map(|prim| {
            let pts = geom.prim_points(prim);
            if pts.len() < 3 {
                return Vec3::ZERO;
            }
            let (a, b, c) = (geom.pos(pts[0] as usize), geom.pos(pts[1] as usize), geom.pos(pts[2] as usize));
            let n = (b - a).cross(c - a);
            if n.length_squared() > 1e-12 { n } else { Vec3::ZERO }
        })
        .collect();
    let units: Vec<Vec3> = faces.iter().map(|n| n.normalize_or_zero()).collect();
    // A hair of slack, so that faces exactly at the angle are in and 180
    // takes faces that oppose each other outright.
    let limit = cusp_degrees.clamp(0.0, 180.0).to_radians().cos() - 1e-5;
    let mut out = vec![Vec3::ZERO; geom.num_verts()];
    for prim in 0..geom.num_prims() {
        let own = units[prim];
        if own == Vec3::ZERO {
            continue;
        }
        for (v, &p) in geom.prim_verts(prim).zip(geom.prim_points(prim)) {
            let mut sum = Vec3::ZERO;
            for &around in geom.point_prims(p as usize) {
                if units[around as usize].dot(own) >= limit {
                    sum += faces[around as usize];
                }
            }
            out[v] = sum.normalize_or_zero();
        }
    }
    out
}

/// The scene's vertex normals, where it carries them: a Float3 `N` on its
/// vertices, which is what the Normal node's Vertices class writes.
pub fn own_vertex_normals(d: &Detail) -> Option<&AttribData> {
    d.verts().get("N").filter(|n| n.ty().components() == 3 && n.len() == d.num_verts())
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
    let input_node = param_node(root, target, "Input")?;
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
    let input_node = param_node(root, target, "Input")?;
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
            // How much of the change lands, per point: Strength, times the
            // point's value of the Scale By attribute when one is named.
            // They scale the EFFECT — the difference the node makes — so
            // they mean the same under every Combine: an Add moves by that
            // much of Value, a Set goes that far toward it, a Multiply that
            // far toward the product. At exactly one the combined value is
            // written as it always was, bit for bit.
            let strength = node_param_f32(target, "Strength", 1.0);
            let scale_by = node_param_str(target, "Scale By", "");
            let scale_by = scale_by.trim().to_string();
            let weights: Option<Vec<f32>> = if scale_by.is_empty() {
                None
            } else if !geom.points().has(&scale_by) {
                fail = format!("Scale By '{}' is not a point attribute", scale_by);
                None
            } else {
                Some(affected.iter().map(|&p| geom.points().value(&scale_by, p).map_or(0.0, |v| v.as_f32())).collect())
            };
            let amount = |i: usize| strength * weights.as_ref().map_or(1.0, |w| w[i]);
            // The step's size. Inside a simnet the chain runs once per
            // SUBSTEP and the solver leaves `dt` on the state — a frame
            // over the substep count — so a change that ACCUMULATES has to
            // be a rate, or four substeps pull four times as far a frame
            // and the substep count, which is there to steady a solve,
            // becomes its speed. An Add lands `dt` of its amount; a
            // Multiply the `dt`-th power of its factor, which is what
            // compounds back to the factor over a frame (a factor that is
            // not positive has no such power, and lands `dt` of its amount
            // instead). A Set does not accumulate — setting twice is
            // setting once — and is left alone. Outside a simnet there is
            // no `dt` and the step is the whole of it.
            //
            // `Per Frame` is the switch, on in the template. A node that
            // does not carry the row reads OFF: that is every node built
            // before it, and the chains that Add one to a counter to count
            // the runs of the chain, which a rate would make a count of
            // frames.
            let per_frame = node_param_bool(target, "Per Frame", false);
            let dt = if per_frame { geom.detail().value("dt", 0).map_or(1.0, |v| v.as_f32()) } else { 1.0 };
            let dt = if dt.is_finite() && dt > 0.0 { dt } else { 1.0 };
            let (adds, multiplies) = (combine_mode == "add", combine_mode == "multiply");
            let blend = |old: &[f32], new: &mut [f32], k: f32| {
                if multiplies && dt != 1.0 {
                    for (n, o) in new.iter_mut().zip(old) {
                        // The factor this point takes at amount k, over a
                        // whole frame: 1 at none of it, Value at all of it.
                        let factor = if o.abs() > 1e-12 { 1.0 + (*n / o - 1.0) * k } else { 1.0 };
                        *n = if factor > 0.0 { o * factor.powf(dt) } else { o + (o * factor - o) * dt };
                    }
                    return;
                }
                let k = if adds { k * dt } else { k };
                if k != 1.0 {
                    for (n, o) in new.iter_mut().zip(old) {
                        *n = o + (*n - o) * k;
                    }
                }
            };
            if !fail.is_empty() {
                // Scale By names nothing: the message is set, nothing moves.
            } else if !value_ok {
                fail = format!("Value '{}' does not parse as numbers", value_str);
            } else if builtin {
                match fit(3) {
                    Some(src) => {
                        for (i, &p) in affected.iter().enumerate() {
                            let old = if is_col { geom.color(p) } else { geom.pos(p).to_array() };
                            let mut v = old;
                            combine(&mut v, &src);
                            blend(&old, &mut v, amount(i));
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
                            for (i, &p) in affected.iter().enumerate() {
                                let Some(cur) = geom.points().value(&name, p) else { continue };
                                let old = attrib_components(cur);
                                let mut buf = old.clone();
                                combine(&mut buf, &src);
                                blend(&old, &mut buf, amount(i));
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
                                // A single number is every component's:
                                // a Float3 times a Float is the vector
                                // scaled. Until 2026-09-29 the components
                                // Source B lacked read zero, so that
                                // product kept X and zeroed Y and Z — and
                                // a sum or a minimum touched X alone. A
                                // Source B of two or more components still
                                // pairs off by position, the missing ones
                                // zero: only ONE number has an obvious
                                // meaning for all of them.
                                let y = if b.len() == 1 { b[0] } else { at(&b, i) };
                                let x = at(&a, i);
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
    let input_node = param_node(root, target, "Input")?;
    let geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error, sim)?;

    let num_points = node_param_f32(target, "Points", 100.0) as usize;
    let radius = node_param_f32(target, "Radius", 0.02);
    // See points_detail: markers are for looking at, bare points are for
    // working with.
    let markers = node_param_bool(target, "Markers", true);

    // Surface mode: points ON the surface, by area, optionally pushed apart
    // across it — the Scatter SOP with Relax Points, which is what a seed
    // for a hull wants. Volume mode below is what this node did first:
    // points INSIDE the shape, by parity.
    if node_param_str(target, "Mode", "Volume").eq_ignore_ascii_case("surface") {
        let seed = node_param_f32(target, "Seed", 1.1);
        let mut pts = crate::scatter::scatter_on_surface(&geom, num_points, seed);
        let relax = node_param_bool(target, "Relax Points", false);
        let iterations = node_param_f32(target, "Relax Iterations", 50.0).max(0.0) as usize;
        if relax && iterations > 0 && !pts.is_empty() {
            let scale = node_param_f32(target, "Scale Radii By", 1.248);
            let max = (node_param_bool(target, "Use Max Relax Radius", true))
                .then(|| node_param_f32(target, "Max Relax Radius", 10.0));
            let r = crate::scatter::relax_radius(&geom, pts.len(), scale, max);
            let grid = crate::spatial::TriGrid::build(&geom);
            crate::scatter::relax_on_surface(&mut pts, &grid, r, iterations);
        }
        let mut out = Detail::new();
        for p in pts {
            if markers {
                out.merge(&sphere_detail(p, radius, 6, 8));
            } else {
                out.add_point(p);
            }
        }
        return Some(out);
    }

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
                if markers {
                    scattered_geom.merge(&sphere_detail(candidate, radius, 6, 8));
                } else {
                    scattered_geom.add_point(candidate);
                }
                found_count += 1;
            }
        }
        scattered_geom
    };

    Some(res)
}

/// The NATIVE geometry types — the ones
/// [`generate_single_node_geometry_with_errors`] dispatches on directly.
///
/// Subnet templates (the Embryo) are NOT here: they instantiate as type
/// `node` and resolve through their `output` child, so their type never
/// reaches this list. `"box"` sat here from 2026-06-13 to 2026-09-19 while
/// `nodes/box.json` was a kernel subnet no node ever carried the type of —
/// it is a native type now (2026-09-24, with sphere, plane and extrude), and
/// `test_every_listed_geometry_type_has_a_resolver` keeps an entry with no
/// dispatch arm from recurring: that is a node type that would resolve to
/// nothing, silently.
pub fn is_geometry_node_type(node_type: &str) -> bool {
    let nt = node_type.to_lowercase();
    nt == "sphere"
        || nt == "line"
        || nt == "curve"
        || nt == "grid"
        || nt == "polygon"
        || nt == "points"
        || nt == "transform"
        || nt == "opencl"
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
        || nt == "subdivide"
        || nt == "export"
        || nt == "boolean"
        || nt == "mold_shell"
        || nt == "hull"
        || nt == "wrangle"
        || nt == "box"
        || nt == "plane"
        || nt == "extrude"
        || nt == "switch"
        || nt == "volume"
        || nt == "deform"
        || nt == "valence"
        || nt == "transfer"
        || nt == "soft_transform"
        || nt == "copy"
        || nt == "cull"
        || nt == "connectivity"
        || nt == "distance"
        || nt == "bounds"
        || nt == "normal"
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
    // Inside a simnet the chain is the simulation STEP, so its nodes are not
    // walked like a subnet's: the output child's geometry flag shows the
    // SOLVED state at the current frame — the same geometry the parent level
    // draws for the simnet — and every other visible child draws itself as
    // the current frame's step saw it, with the feedback stack holding the
    // state that step consumed. So `input` shows what the step reads, a
    // chain node shows this frame's pass over it, and a node that is not
    // wired into the chain at all (a seed being built beside it) simply
    // draws. Before 2026-09-21 only the output flag drew anything, and a
    // visible node inside a simnet was a node you could not see.
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
        let shown: Vec<&FsNode> = start
            .children
            .iter()
            .filter(|c| {
                c.geometry_visible
                    && !c.node_type.eq_ignore_ascii_case("output")
                    && is_geometry_node_type(&c.node_type)
            })
            .collect();
        if !shown.is_empty() {
            let mut visited = Vec::new();
            if let Some(fed) = simnet_step_feedback(root, start, &mut visited, ocl_error, sim) {
                sim.feedback.push((start.id.clone(), fed));
                for child in shown {
                    let mut visited = Vec::new();
                    if let Some(geom) = generate_single_node_geometry_with_errors(root, child, &mut visited, ocl_error, sim) {
                        out.merge(&geom);
                    }
                }
                sim.feedback.pop();
            }
        }
        return out;
    }
    fn visit(root: &FsNode, node: &FsNode, parent_visible: bool, top: bool, count: &mut usize, out: &mut Detail, ocl_error: &mut Option<String>, sim: &mut EvalSim) {
        // The walk hands nodes to their resolvers directly, so it resolves
        // references itself — dived into a composed subnet, its children are
        // what is drawn, and their controls live on the subnet.
        if is_bypassed(node) {
            // Counted as it would have been, so that what is placed by its
            // index stays where it was; drawn, if it is shown, as what it
            // passes through; and not gone into — a bypassed subnet's
            // children are part of what is switched off.
            if is_geometry_node_type(&node.node_type) {
                *count += 1;
            }
            if parent_visible && node.geometry_visible {
                if let Some(geom) = generate_single_node_geometry_with_errors(root, node, &mut Vec::new(), ocl_error, sim) {
                    out.merge(&geom);
                }
            }
            return;
        }
        let resolved = resolve_param_refs(root, node, sim.frame, ocl_error);
        let node = resolved.as_ref().unwrap_or(node);
        let is_visible = parent_visible && node.geometry_visible;
        if node.node_type.eq_ignore_ascii_case("sphere") {
            let idx = *count;
            *count += 1;
            if is_visible {
                let legacy = (!crate::shapes::sphere_has_center(node)).then(|| index_center(idx));
                out.merge(&crate::shapes::sphere_node_detail(node, legacy));
            }
        } else if node.node_type.eq_ignore_ascii_case("box") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                out.merge(&crate::shapes::box_node_detail(node));
            }
        } else if node.node_type.eq_ignore_ascii_case("plane") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                out.merge(&crate::shapes::plane_node_detail(node));
            }
        } else if node.node_type.eq_ignore_ascii_case("extrude") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_extrude_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
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
        } else if node.node_type.eq_ignore_ascii_case("grid") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                out.merge(&grid_detail(node));
            }
        } else if node.node_type.eq_ignore_ascii_case("polygon") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                out.merge(&polygon_detail(node));
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
        } else if node.node_type.eq_ignore_ascii_case("normal") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_normal_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("bounds") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_bounds_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("distance") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_distance_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("connectivity") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_connectivity_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("cull") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_cull_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("copy") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_copy_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("soft_transform") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_soft_transform_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("transfer") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_transfer_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("valence") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_valence_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("deform") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_deform_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("volume") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_volume_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("boolean") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_boolean_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("mold_shell") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_mold_shell_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("hull") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_hull_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("wrangle") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_wrangle_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("switch") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_switch_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("export") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_export_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
                    out.merge(&geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("subdivide") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_subdivide_geometry_with_errors(root, node, &mut visited, ocl_error, sim) {
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
                if let Some(geom) = retired_opencl_node(root, node, &mut visited, ocl_error, sim) {
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

/// The Points node: a marker sphere at each generated location.
///
/// Every marker stays its own piece — `merge` reallocates identities, so two
/// markers that happen to land on the same spot are still two points with two
/// identities rather than one welded blob.
pub fn points_detail(node: &FsNode, center: Vec3) -> Detail {
    let num_points = node_param_f32(node, "Points", 100.0) as i32;
    let shape = node_param_str(node, "Shape", "None");
    // Markers off emits BARE POINTS — no marker geometry at all. Everything
    // that generates locations drew little spheres at them, which is right for
    // looking at and wrong for working with: Copy placed one instance per
    // marker vertex rather than one per location, because the markers were the
    // only points there were.
    let markers = node_param_bool(node, "Markers", true);
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
        if markers {
            d.merge(&sphere_detail(center + offset, 0.02, 6, 8));
        } else {
            d.add_point(center + offset);
        }
    }
    d
}

/// Where the index-placed generators (a bare sphere, Line, Points) stand:
/// a 4-wide row of cells 1.25 apart, so several of them do not stack.
pub fn index_center(idx: usize) -> Vec3 {
    Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25)
}

pub fn find_sphere_index(root: &FsNode, target: &FsNode) -> Option<usize> {
    fn visit(node: &FsNode, target: &FsNode, count: &mut usize) -> Option<usize> {
        // By id, not by pointer: a node evaluated with its parameter
        // references resolved is a CLONE of the one in the tree.
        let is_target = node.id == target.id;
        if node.node_type.eq_ignore_ascii_case("sphere") 
            || node.node_type.eq_ignore_ascii_case("line") 
            || node.node_type.eq_ignore_ascii_case("points")
            || node.node_type.eq_ignore_ascii_case("transform")
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
                // Counter-clockwise seen from OUTSIDE, as `sphere_detail`
                // winds and as the raster fill's back-face cull expects.
                // This kept the retired soup's inward order until
                // 2026-09-24, so the cull drew the INSIDE of each marker's
                // far half and nothing of its near half — and a marker on a
                // surface showed only where that far half poked out of the
                // mesh, vanishing from the views where it did not.
                // `point_markers_wind_outward` holds the sign.
                for p in [p00, p11, p10, p00, p01, p11] {
                    out.push(Vertex3D { position: p, color });
                }
            }
        }
    }
    out
}

/// The camera-pivot marker: three axis beams from the pivot. `scale` is
/// their LENGTH alone; the thickness is fixed, so a small marker is short
/// rather than too thin to see.
pub fn camera_pivot_vertices(scale: f32) -> Vec<Vertex3D> {
    let mut verts = Vec::new();
    let t = 0.002;
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
    let mut verts = Vec::new();

    let mut bar = |start: Vec3, end: Vec3| {
        verts.extend(
            detail_vertices(&box_detail(start, end, thickness))
                .into_iter()
                .map(|v| Vertex3D { position: v.position, color }),
        );
    };

    let mut z = -range;
    while z <= range {
        bar(Vec3::new(-range, 0.0, z), Vec3::new(range, 0.0, z));
        z += step;
    }

    let mut x = -range;
    while x <= range {
        bar(Vec3::new(x, 0.0, -range), Vec3::new(x, 0.0, range));
        x += step;
    }

    verts
}

/// How many triangle corners a welded UV sphere fans out to: two pole bands of
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

    /// Every type [`is_geometry_node_type`] lists must have a resolver arm in
    /// [`generate_single_node_geometry_with_errors`].
    ///
    /// A type listed here with no dispatch arm is the worst kind of dead
    /// node: it passes every "is this geometry?" check the app makes — it
    /// gets a `meta` child, it shows in the palette's geometry filter, the
    /// network editor treats it as a producer — and then resolves to `None`.
    /// Nothing reports it. The node draws nothing, exports as "produced no
    /// geometry", and anything downstream of it silently resolves to nothing
    /// too. `"box"` sat in the list that way from the day it was written
    /// (2026-06-13) until 2026-09-19, harmless only because `nodes/box.json`
    /// is a subnet and no node ever actually carried the type.
    ///
    /// Scanning the source is an odd way to assert it, but the alternative —
    /// building a node of each type and resolving it — needs a plausible
    /// parameter set per type, which is exactly the per-type knowledge that
    /// goes stale.
    #[test]
    fn test_every_listed_geometry_type_has_a_resolver() {
        let src = include_str!("geometry.rs");

        // The listed types, read off `nt == "..."` in is_geometry_node_type.
        let list_start = src
            .find("pub fn is_geometry_node_type")
            .expect("is_geometry_node_type moved; this test scans for it");
        let list_end = list_start
            + src[list_start..].find("\n}").expect("unterminated is_geometry_node_type");
        let listed: Vec<&str> = src[list_start..list_end]
            .match_indices("nt == \"")
            .map(|(i, m)| {
                let rest = &src[list_start + i + m.len()..];
                &rest[..rest.find('"').expect("unterminated type literal")]
            })
            .collect();
        assert!(
            listed.len() > 20,
            "only {} types parsed out of is_geometry_node_type — the scan broke, not the list",
            listed.len()
        );

        // The dispatch, bounded to the resolver itself: `visit` further down
        // carries its own arms, and scanning the whole file would let a type
        // that only the display walk knows about pass as resolvable.
        let disp_start = src
            .find("pub fn generate_single_node_geometry_with_errors")
            .expect("generate_single_node_geometry_with_errors moved; this test scans for it");
        let after_sig = disp_start + "pub fn ".len();
        let disp_end = src[after_sig..]
            .find("\npub fn ")
            .map(|i| after_sig + i)
            .expect("resolver is the last fn in the file?");
        let dispatch = &src[disp_start..disp_end];

        let mut missing: Vec<&str> = Vec::new();
        for ty in &listed {
            if !dispatch.contains(&format!("eq_ignore_ascii_case(\"{ty}\")")) {
                missing.push(ty);
            }
        }
        assert!(
            missing.is_empty(),
            "is_geometry_node_type lists {missing:?}, which generate_single_node_geometry_with_errors \
             does not dispatch — such a node resolves to nothing, silently. Either give it a \
             resolver arm or drop it from the list (a subnet template like Box instantiates as \
             type `node` and belongs in neither)."
        );
    }

    /// The inverse of the test above: every template in `nodes/` must name a
    /// type that something actually resolves.
    ///
    /// The pair closes the loop. That test catches a type LISTED as geometry
    /// with no resolver; this one catches a template whose type is in no
    /// list at all — a palette entry that places a node the resolver walks
    /// straight past. Both fail the same silent way: the node draws nothing,
    /// exports as "produced no geometry", and anything reading from it
    /// resolves to nothing with no error anywhere.
    ///
    /// The four things a template's type may legitimately be:
    /// a native geometry type; `node`, the subnet form (Box, Sphere, Plane,
    /// Extrude), which resolves through its `output` child; a page node,
    /// which belongs to the raster context and has no geometry by design;
    /// or `camera`, which the project reads directly. A new type outside
    /// those is a decision to make, not a default to fall into.
    #[test]
    fn test_every_template_names_a_type_something_resolves() {
        let templates = crate::app::load_fs_tree();

        // load_fs_tree drops a template whose JSON does not parse — silently,
        // in an `if let Ok`. A typo'd brace would otherwise just remove the
        // node from the palette, which looks nothing like a parse error.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("nodes");
        let on_disk = std::fs::read_dir(&dir)
            .expect("nodes/ is missing")
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .count();
        assert_eq!(
            templates.children.len(),
            on_disk,
            "load_fs_tree returned {} of {on_disk} templates in {} — the missing one(s) failed to \
             parse and were dropped without a word",
            templates.children.len(),
            dir.display()
        );

        let mut orphans: Vec<String> = Vec::new();
        let mut headless: Vec<String> = Vec::new();
        for t in &templates.children {
            let ty = t.node_type.as_str();
            let resolvable = is_geometry_node_type(ty)
                || ty.eq_ignore_ascii_case("node")
                || crate::page::is_page_node(ty)
                || ty.eq_ignore_ascii_case("camera");
            if !resolvable {
                orphans.push(format!("{} (type {ty:?})", t.name));
            }
            // A subnet resolves through its `output` child and returns None
            // without one, so a template that ships children must ship that.
            // An EMPTY subnet (Subnet itself) is fine: it is a container the
            // user fills, not a node that promises geometry.
            if ty.eq_ignore_ascii_case("node")
                && !t.children.is_empty()
                && !t.children.iter().any(|c| c.node_type.eq_ignore_ascii_case("output"))
            {
                headless.push(t.name.clone());
            }
        }
        assert!(
            orphans.is_empty(),
            "these templates name a type nothing resolves: {orphans:?} — a node placed from the \
             palette that the resolver walks straight past. Give the type a resolver arm (and a \
             line in is_geometry_node_type), or make the template a `node` subnet."
        );
        assert!(
            headless.is_empty(),
            "these subnet templates ship children but no `output` child: {headless:?} — the `node` \
             resolver arm reads the output child, so they resolve to nothing."
        );
    }

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

    #[test]
    fn test_welded_sphere_reproduces_the_soup_it_replaced() {
        let (center, radius, lat, lon) = (Vec3::new(0.1, 0.2, 0.3), 0.7, 16, 24);
        let before = sphere_soup_before_welding(center, radius, lat, lon);
        let after: Vec<[f32; 3]> = detail_vertices(&sphere_detail(center, radius, lat, lon))
            .iter()
            .map(|v| v.position)
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

        // Fanned for the renderer, the six quads are twelve triangles.
        assert_eq!(detail_vertices(&d).len(), 36);
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
                .map(|(name, default)| crate::app::ParamDef::new(name.to_string(), "text".to_string(), default.to_string()))
                .collect(),
            geometry_visible: true,
            bypassed: false,
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
        assert_eq!(detail_vertices(&d).len(), 72);

        // Every point still has its own identity across the merge.
        let mut ids = d.ids().to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 16);
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
                crate::app::ParamDef::new("Points".to_string(), "spinbox".to_string(), "5".to_string()).with_range(Some(1.0), Some(10.0)).with_step(Some(1.0)),
                crate::app::ParamDef::new("Shape".to_string(), "choice:None,Spiral,Line,Circle,Grid".to_string(), shape.to_string()),
            ],
            geometry_visible: true,
            bypassed: false,
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
            bypassed: false,
            position: (0.0, 0.0),
        };
        let geom = network_sphere_vertices(&root);
        assert_eq!(geom.num_points(), 5 * super::sphere_point_len(6, 8));

        // Shape "None": every point sits in the same spot, so all five marker
        // spheres cover an identical (tiny) extent. A spread shape must not.
        let extent = |g: &Detail| {
            let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for &p in g.positions() {
                min = min.min(Vec3::from_array(p));
                max = max.max(Vec3::from_array(p));
            }
            max - min
        };
        let none = points_detail(&points_node("None"), Vec3::ZERO);
        let e = extent(&none);
        assert!(e.length() < 0.1, "None must collapse to one spot, extent {e:?}");

        for shape in ["Spiral", "Line", "Circle", "Grid"] {
            let g = points_detail(&points_node(shape), Vec3::ZERO);
            assert_eq!(g.num_points(), 5 * super::sphere_point_len(6, 8), "{shape}");
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
                crate::app::ParamDef::new("Radius".to_string(), "slider".to_string(), "0.5".to_string())
            ],
            geometry_visible: true,
            bypassed: false,
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
                crate::app::ParamDef::new("Input".to_string(), "text".to_string(), "Sphere 1".to_string()),
                crate::app::ParamDef::new("Translation".to_string(), "float3".to_string(), "1.00:2.00:3.00".to_string())
            ],
            geometry_visible: true,
            bypassed: false,
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
            bypassed: false,
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
                crate::app::ParamDef::new("Input".to_string(), "text".to_string(), "Transform 1".to_string()),
                crate::app::ParamDef::new("Translation".to_string(), "float3".to_string(), "-1.00:-1.00:-1.00".to_string())
            ],
            geometry_visible: true,
            bypassed: false,
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
            bypassed: false,
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
                crate::app::ParamDef::new("Input".to_string(), "text".to_string(), "Transform Loop".to_string()),
                crate::app::ParamDef::new("Translation".to_string(), "float3".to_string(), "1.00:1.00:1.00".to_string())
            ],
            geometry_visible: true,
            bypassed: false,
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
            bypassed: false,
            position: (0.0, 0.0),
        };
        let mut visited = Vec::new();
        let geom_loop = resolve_transform_geometry(&root_loop, &transform_loop, &mut visited);
        assert!(geom_loop.is_none());
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
                crate::app::ParamDef::new("Radius".to_string(), "slider".to_string(), "0.5".to_string())
            ],
            geometry_visible: true,
            bypassed: false,
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
                crate::app::ParamDef::new("Input".to_string(), "text".to_string(), "Sphere 1".to_string()),
                crate::app::ParamDef::new("Points".to_string(), "spinbox".to_string(), "15".to_string()),
                crate::app::ParamDef::new("Radius".to_string(), "slider".to_string(), "0.02".to_string())
            ],
            geometry_visible: true,
            bypassed: false,
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
            bypassed: false,
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
        param_node(root, target, "Input")
            .and_then(|n| generate_single_node_geometry_with_errors(root, n, visited, ocl_error, sim))
            .unwrap_or_default()
    };

    let key = sim_solve_key(target, &seed);
    // The frame this sim shows its seed at. Empty means "follow the timeline",
    // which is what every sim did before this parameter existed; a number
    // decouples when a simulation starts from when the shot does, so two sims
    // in one scene can begin at different times.
    let start_frame = node_param_f32(target, "Start Frame", sim.start_frame as f32).round() as i32;
    let due = (sim.frame - start_frame).max(0);

    // Resume from what is kept of this solve: the latest state when it has
    // not run PAST the frame asked for, else the nearest checkpoint behind
    // it — a step is not invertible, so going back means going forward from
    // somewhere earlier. Memory first, then disk, then the seed.
    //
    // The entry is taken out while the solve runs and put back at its end.
    // One of another key (the chain or the seed was edited) is dropped
    // here, its checkpoints with it.
    let prior = sim.cache.entries.remove(&target.id).filter(|e| e.key == key);
    let mut checkpoints = Checkpoints::default();
    let mut cached: Option<(Detail, Detail, i32)> = None;
    if let Some(prior) = prior {
        checkpoints = prior.checkpoints;
        if prior.frame <= due {
            // Playing forward a frame at a time, the frame in hand is
            // always the one before the frame asked for, so the solve never
            // PASSES a frame on the interval — it arrives on one and leaves
            // from it. Kept as it is left.
            if prior.frame < due && prior.frame > 0 && prior.frame % checkpoints.every == 0 {
                checkpoints.keep(Checkpoint { frame: prior.frame, state: prior.state.clone(), prev: prior.prev.clone() });
            }
            cached = Some((prior.state, prior.prev, prior.frame));
        } else {
            // Going back. The frame being left is kept, so coming forward
            // to it again is a resume too.
            checkpoints.keep(Checkpoint { frame: prior.frame, state: prior.state, prev: prior.prev });
        }
    }
    // A checkpoint nearer the frame than what is in hand wins: behind a
    // backward scrub there is nothing in hand, and a forward jump past
    // several may land on one.
    let in_hand = cached.as_ref().map_or(-1, |c| c.2);
    if let Some(c) = checkpoints.behind(due, in_hand) {
        cached = Some((c.state.clone(), c.prev.clone(), c.frame));
    }
    let caching = node_param_bool(target, "Cache", false);
    // What the last substep consumed, carried with the solve so the interior
    // view can be drawn without re-solving: resumed from the cache when the
    // cache is what we resume from, the seed otherwise.
    // The disk carries its `prev` too: a resume landing EXACTLY on the frame
    // asked for runs no step, and until 2026-09-28 left the feedback at the
    // seed for that frame — the interior view and the pull arrows drawn from
    // where the sim started, once per app launch with Cache on.
    let disk = if cached.is_none() && caching { read_sim_cache(&target.id, key, due) } else { None };
    let (mut state, mut prev_frame, mut done) = match (cached, disk) {
        (Some(hit), _) => hit,
        (None, Some(hit)) => hit,
        (None, None) => (seed.clone(), seed, 0),
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
            prev_frame = prev;
            sim.cache.steps_run += 1;
            #[cfg(test)]
            STEPS_ON_THIS_THREAD.with(|s| s.set(s.get() + 1));
        }
        done += 1;
        // A frame on the interval is kept as the solve passes it — not the
        // frame asked for, which is the entry itself.
        if done % checkpoints.every == 0 && done < due {
            checkpoints.keep(Checkpoint { frame: done, state: state.clone(), prev: prev_frame.clone() });
        }
    }

    if caching && due > 0 {
        write_sim_cache(&target.id, key, due, &state, &prev_frame);
    }
    sim.cache.entries.insert(
        target.id.clone(),
        SimSolve { key, frame: due, state: state.clone(), prev: prev_frame, checkpoints },
    );
    Some(state)
}

/// The state the LAST substep of a simnet's current frame was stepped FROM:
/// the seed at the start frame, the previous frame's solution with one
/// substep per frame, the last substep's input otherwise. Solves the simnet
/// (filling the cache) and reads it back, so it costs nothing beyond the
/// solve the display needs anyway.
pub fn simnet_step_feedback(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
    sim: &mut EvalSim,
) -> Option<Detail> {
    resolve_simnet_geometry_with_errors(root, target, visited, ocl_error, sim)?;
    sim.cache.entries.get(&target.id).map(|e| e.prev.clone())
}

/// Where a simnet's solved state is parked between runs.
///
/// Under the cache directory, not the project: it is derived data that can be
/// recomputed, and a project directory that silently grew hundreds of
/// megabytes of solver state would be a nasty surprise to copy or back up.
///
/// Under `cfg(test)` it is a process-scoped temp directory, as the settings
/// file is (see "App-written settings" in CLAUDE.md): a test that solves a
/// Cache-on simnet must not leave solver state in the user's own cache, and
/// the suite never sets an environment variable, since libtest runs tests in
/// parallel and one that did would race every other test reading it.
fn sim_cache_path(node_id: &str) -> Option<std::path::PathBuf> {
    #[cfg(test)]
    let base = std::env::temp_dir().join(format!("cce-designer-test-cache-{}", std::process::id()));
    #[cfg(not(test))]
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
/// an answer. Then the state's byte length, the state, and what its last
/// substep consumed (`SimSolve::prev`), so a resume can draw the interior
/// view without a step. The length field is what tells a file written before
/// `prev` was carried (2026-09-28) apart: there the Detail magic sits where
/// the length goes, reads as an impossible length, and the file is refused —
/// no cache, a solve from the seed, and the file rewritten in the new shape.
fn write_sim_cache(node_id: &str, key: u64, frame: i32, state: &Detail, prev: &Detail) {
    let Some(path) = sim_cache_path(node_id) else { return };
    write_sim_cache_at(&path, key, frame, state, prev);
}

/// [`write_sim_cache`] against a given path, so the format can be exercised
/// without a process-wide environment variable.
fn write_sim_cache_at(path: &std::path::Path, key: u64, frame: i32, state: &Detail, prev: &Detail) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let state_bytes = state.to_bytes();
    let mut blob = Vec::new();
    blob.extend_from_slice(&key.to_le_bytes());
    blob.extend_from_slice(&frame.to_le_bytes());
    blob.extend_from_slice(&(state_bytes.len() as u64).to_le_bytes());
    blob.extend_from_slice(&state_bytes);
    blob.extend_from_slice(&prev.to_bytes());
    // Written beside the target and renamed, so a cache half-written when the
    // app dies is never read as a whole one.
    let tmp = path.with_extension("simcache.tmp");
    if std::fs::write(&tmp, &blob).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// A cached solve — `(state, prev, frame)` — if one is on disk and has not
/// run past the frame being asked for. Any failure — missing, truncated,
/// stale, corrupt, the old shape without `prev` — reads as "no cache" and
/// the sim solves from its seed.
fn read_sim_cache(node_id: &str, key: u64, due: i32) -> Option<(Detail, Detail, i32)> {
    read_sim_cache_at(&sim_cache_path(node_id)?, key, due)
}

fn read_sim_cache_at(path: &std::path::Path, key: u64, due: i32) -> Option<(Detail, Detail, i32)> {
    let blob = std::fs::read(path).ok()?;
    if blob.len() < 20 || u64::from_le_bytes(blob[0..8].try_into().ok()?) != key {
        return None;
    }
    let frame = i32::from_le_bytes(blob[8..12].try_into().ok()?);
    if frame < 0 || frame > due {
        return None;
    }
    let state_len = usize::try_from(u64::from_le_bytes(blob[12..20].try_into().ok()?)).ok()?;
    let state_end = state_len.checked_add(20)?;
    if state_end > blob.len() {
        return None;
    }
    let state = Detail::from_bytes(&blob[20..state_end]).ok()?;
    let prev = Detail::from_bytes(&blob[state_end..]).ok()?;
    Some((state, prev, frame))
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
        crate::app::ParamDef::new(name.to_string(), "text".to_string(), value.to_string())
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
            bypassed: false,
            position: (0.0, 0.0),
        }
    }

    /// Apply one Attribute-node operation to geometry in hand.
    fn run_attr(before: &Detail, params: &[(&str, &str)]) -> (Detail, Option<String>) {
        let mut geom = before.clone();
        let mut ps = vec![param("Input", "In"), param("Attribute Name", "mass")];
        for (k, v) in params {
            match ps.iter_mut().find(|p| p.name == *k) {
                Some(p) => p.set_text(v.to_string()),
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

    /// A single number is every component's: a Float3 composited with a
    /// Float scales, shifts or clamps the whole vector. It used to pair the
    /// number with X and zero with the rest, so the product kept X and
    /// zeroed Y and Z. Wider pairs still go by position, and the three
    /// operations that reduce to one number are as they were.
    #[test]
    fn test_composite_broadcasts_a_single_number_across_a_vector() {
        let mut before = ramped_mass();
        before.points_mut().create("v", AttribValue::Float3([1.0, 2.0, 3.0]));
        before.points_mut().create("w", AttribValue::Float(0.5));
        before.points_mut().create("uv", AttribValue::Float2([10.0, 20.0]));
        let v = |d: &Detail| match d.points().value("v", 3).unwrap() {
            AttribValue::Float3(x) => x,
            other => panic!("v is still a Float3: {other:?}"),
        };
        let with = |b: &str, op: &str| {
            let (g, err) = run_attr(
                &before,
                &[("Attribute Name", "v"), ("Operation", "Composite"), ("Source B", b), ("Combine Op", op)],
            );
            assert!(err.is_none(), "{op} with {b}: {err:?}");
            v(&g)
        };
        for (op, want) in [
            ("Multiply", [0.5, 1.0, 1.5]),
            ("Add", [1.5, 2.5, 3.5]),
            ("Subtract", [0.5, 1.5, 2.5]),
            ("Divide", [2.0, 4.0, 6.0]),
            ("Minimum", [0.5, 0.5, 0.5]),
            ("Maximum", [1.0, 2.0, 3.0]),
            ("Average", [0.75, 1.25, 1.75]),
            ("Difference", [0.5, 1.5, 2.5]),
        ] {
            assert_eq!(with("w", op), want, "{op}");
        }
        // Two numbers against three pair off by position, as before: the
        // third has nothing to meet.
        assert_eq!(with("uv", "Add"), [11.0, 22.0, 3.0]);
        assert_eq!(with("uv", "Multiply"), [10.0, 40.0, 0.0]);
        // The reductions are not componentwise, and did not change.
        assert_eq!(with("w", "Length"), [0.5, 0.5, 0.5]);
        assert_eq!(with("w", "Dot"), [0.5, 0.5, 0.5]);

        // A per-point weight scales a vector per point: the ramped mass,
        // which differs from point to point, times the same vector.
        let (g, err) = run_attr(
            &before,
            &[("Attribute Name", "v"), ("Operation", "Composite"), ("Source B", "mass"), ("Combine Op", "Multiply")],
        );
        assert!(err.is_none(), "{err:?}");
        for p in [0, 3, g.num_points() - 1] {
            let m = before.points().value("mass", p).unwrap().as_f32();
            let got = match g.points().value("v", p).unwrap() { AttribValue::Float3(x) => x, _ => unreachable!() };
            assert_eq!(got, [m, 2.0 * m, 3.0 * m], "point {p} weighs {m}");
        }
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
                Some(p) => p.set_text(v.to_string()),
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

    /// The Boolean node end to end, through the resolver the viewport calls —
    /// not just Volume's arithmetic.
    ///
    /// Written because a hand-built project of two spheres and a Boolean
    /// rendered an EMPTY scene while every volume unit test passed: the
    /// arithmetic was right and the wiring was never exercised. A node nobody
    /// can reach from the network is not a feature.
    #[test]
    fn test_the_boolean_node_resolves_two_spheres_into_one_solid() {
        // Spheres sit on the resolver's own 1.25-spaced layout, so a radius of
        // 0.8 makes them overlap by 0.35 — a lens neither operation can
        // mistake for the other.
        let a = node("id-a", "sphere1", "sphere", vec![param("Radius", "0.8")], vec![]);
        let b = node("id-b", "sphere2", "sphere", vec![param("Radius", "0.8")], vec![]);

        for (op, expect) in [("Union", "wider"), ("Intersect", "narrower"), ("Subtract", "narrower")] {
            let bool_node = node(
                "id-bool",
                "bool1",
                "boolean",
                vec![
                    param("Input", "sphere1"),
                    param("With", "sphere2"),
                    param("Operation", op),
                    param("Voxel Size", "0.08"),
                ],
                vec![],
            );
            let root = node("id-root", "root", "node", vec![], vec![a.clone(), b.clone(), bool_node]);
            let target = &root.children[2];

            let mut err = None;
            let mut cache = SimCache::default();
            let mut sim = EvalSim::new(0, 0, &mut cache);
            let out = resolve_boolean_geometry_with_errors(&root, target, &mut Vec::new(), &mut err, &mut sim)
                .unwrap_or_else(|| panic!("{op}: the Boolean resolved to nothing"));
            assert!(err.is_none(), "{op}: {err:?}");
            assert!(out.num_prims() > 0, "{op}: the Boolean produced no primitives");
            assert!(out.is_closed(), "{op}: the result is not a closed surface");
            // Union and Intersect come out fully manifold. Subtract does not:
            // the bite leaves a rim thinner than a voxel, and surface nets has
            // one vertex per cell to give it, so the two sheets pinch and a
            // handful of edges carry four faces. Watertight, still solid, but
            // recorded here rather than discovered downstream.
            if op != "Subtract" {
                assert!(out.is_manifold(), "{op}: the result is not manifold");
            }

            // The single sphere it started from, for a size to compare against.
            let (slo, shi) = generate_single_node_geometry_with_errors(
                &root, &root.children[0], &mut Vec::new(), &mut None, &mut sim,
            )
            .unwrap()
            .bounds()
            .unwrap();
            let (olo, ohi) = out.bounds().unwrap();
            let (one, both) = (shi.x - slo.x, ohi.x - olo.x);
            if expect == "wider" {
                assert!(both > one * 1.3, "{op}: {both} is not wider than one sphere's {one}");
            } else {
                assert!(both < one * 0.9, "{op}: {both} is not narrower than one sphere's {one}");
            }
        }
    }

    /// The Volume node's two modes, likewise through the resolver.
    #[test]
    fn test_the_volume_node_offsets_and_shells() {
        let sphere = node("id-s", "sphere1", "sphere", vec![param("Radius", "0.8")], vec![]);

        let grown = node(
            "id-v",
            "vol1",
            "volume",
            vec![
                param("Input", "sphere1"),
                param("Mode", "Offset"),
                param("Voxel Size", "0.08"),
                param("Offset", "0.2"),
            ],
            vec![],
        );
        let root = node("id-root", "root", "node", vec![], vec![sphere.clone(), grown]);
        let mut err = None;
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(0, 0, &mut cache);
        let out = resolve_volume_geometry_with_errors(&root, &root.children[1], &mut Vec::new(), &mut err, &mut sim)
            .expect("the Volume node resolved to nothing");
        assert!(err.is_none(), "{err:?}");
        assert!(out.is_closed(), "an offset sphere is not a closed surface");
        let (slo, shi) = generate_single_node_geometry_with_errors(
            &root, &root.children[0], &mut Vec::new(), &mut None, &mut sim,
        )
        .unwrap()
        .bounds()
        .unwrap();
        let (olo, ohi) = out.bounds().unwrap();
        assert!(
            (ohi.x - olo.x) > (shi.x - slo.x) + 0.25,
            "a +0.2 offset did not grow the sphere: {} vs {}",
            ohi.x - olo.x,
            shi.x - slo.x
        );

        // A shell is hollow: closed, and with twice the surface of the solid.
        let shell = node(
            "id-v2",
            "vol2",
            "volume",
            vec![
                param("Input", "sphere1"),
                param("Mode", "Shell"),
                param("Voxel Size", "0.08"),
                param("Offset", "0.0"),
                param("Thickness", "0.15"),
            ],
            vec![],
        );
        let root = node("id-root", "root", "node", vec![], vec![sphere, shell]);
        let mut err = None;
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(0, 0, &mut cache);
        let out = resolve_volume_geometry_with_errors(&root, &root.children[1], &mut Vec::new(), &mut err, &mut sim)
            .expect("the Volume node resolved to nothing");
        assert!(err.is_none(), "{err:?}");
        assert!(out.is_closed(), "a shell is not a closed surface");
        assert!(out.num_prims() > 0, "the shell is empty");

        // Hollow, and provably so: a shell has an INNER surface, so its points
        // sit at two radii, not one. Rendered from outside it is
        // indistinguishable from the solid sphere, which is exactly why this
        // is asserted rather than looked at.
        let centre = (slo + shi) * 0.5;
        let radii: Vec<f32> = (0..out.num_points()).map(|p| (out.pos(p) - centre).length()).collect();
        let (near, far) = radii.iter().fold((f32::MAX, 0.0f32), |(n, f), &r| (n.min(r), f.max(r)));
        assert!(
            (far - 0.8).abs() < 0.1,
            "the shell's outer surface is at {far}, not the sphere's 0.8"
        );
        assert!(
            (near - 0.65).abs() < 0.1,
            "the shell has no cavity: its innermost point is at {near}, expected 0.8 - 0.15"
        );
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
                Some(p) => p.set_text(v.to_string()),
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
                Some(p) => p.set_text(v.to_string()),
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
        // A directory of its own, and files removed one by one at the end
        // rather than with remove_dir_all: this process has been seen to fail
        // a closedir with EBADF while other threads are running, and a test
        // should not be the thing that trips it.
        let dir = std::env::temp_dir().join(format!("cce-simcache-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("sim.simcache");
        let mut state = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        state.points_mut().create("acc", AttribValue::Float(9.0));
        let mut prev = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        prev.points_mut().create("acc", AttribValue::Float(8.0));

        write_sim_cache_at(&path, 0xABCD, 12, &state, &prev);
        assert!(path.exists(), "the cache was written");

        // The right solve, at or before the frame being asked for — and what
        // its last substep consumed, so a resume can draw the interior.
        let (got, got_prev, frame) = read_sim_cache_at(&path, 0xABCD, 20).expect("a matching cache resumes");
        assert_eq!(frame, 12);
        assert_eq!(got.points().value("acc", 0), Some(AttribValue::Float(9.0)));
        assert_eq!(got.ids(), state.ids());
        assert_eq!(got_prev.points().value("acc", 0), Some(AttribValue::Float(8.0)), "prev rides the file");

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
        // A file in the shape written before `prev` was carried: key, frame,
        // then the state with no length in front of it. The Detail magic
        // reads as the length, and the file is refused rather than the
        // state read back with a seed for a prev.
        let mut old = Vec::new();
        old.extend_from_slice(&0xABCDu64.to_le_bytes());
        old.extend_from_slice(&12i32.to_le_bytes());
        old.extend_from_slice(&state.to_bytes());
        std::fs::write(&path, &old).unwrap();
        assert!(read_sim_cache_at(&path, 0xABCD, 20).is_none(), "the old shape is no cache");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("simcache.tmp"));
        let _ = std::fs::remove_dir(&dir);
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
            step.params.iter_mut().find(|p| p.name == "Translation").unwrap().set_text("2.00:0.00:0.00".to_string());
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
        let mut root = stepping_graph();
        // Only the output flag on: the other children draw themselves too
        // now (the test below), and this one is about the solved state.
        for c in &mut root.children.iter_mut().find(|c| c.node_type == "simnet").unwrap().children {
            c.geometry_visible = c.node_type == "output";
        }
        let sim_node = root.children.iter().find(|c| c.node_type == "simnet").unwrap();
        let base = min_x(&solve_at(&root, 1));

        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(4, 1, &mut cache);
        let mut err = None;
        let interior = network_sphere_vertices_with_errors(&root, sim_node, &mut err, &mut sim);
        assert!(!interior.is_empty(), "simnet interior rendered empty");
        let moved = min_x(&interior) - base;
        assert!((moved - 3.0).abs() < 1e-4, "frame 4 = 3 steps of +1.0, got {moved}");

        // The output child's geometry toggle is the solved state's switch;
        // with every child's flag off the interior is empty.
        let mut hidden = root.clone();
        for c in &mut hidden.children.iter_mut().find(|c| c.node_type == "simnet").unwrap().children {
            c.geometry_visible = false;
        }
        let sim_node = hidden.children.iter().find(|c| c.node_type == "simnet").unwrap();
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(4, 1, &mut cache);
        let mut err = None;
        let toggled = network_sphere_vertices_with_errors(&hidden, sim_node, &mut err, &mut sim);
        assert!(toggled.is_empty(), "output toggle off should hide the solved state");
    }

    /// Dived into a simnet, a visible child other than the output draws
    /// itself as the current frame's step saw it: `input` is the state the
    /// step consumed, a chain node is this frame's pass over it, and a node
    /// not wired into the chain at all simply draws. Before this a visible
    /// node inside a simnet was a node you could not see — the embryo being
    /// built beside a solver's chain drew nothing.
    #[test]
    fn test_scene_walk_inside_a_simnet_draws_visible_children_as_the_step_sees_them() {
        let base = min_x(&solve_at(&stepping_graph(), 1));
        // Every child off, then one on at a time.
        let with_only = |name: &str| {
            let mut root = stepping_graph();
            let sim_node = root.children.iter_mut().find(|c| c.node_type == "simnet").unwrap();
            sim_node.children.push(node("id-seed", "seed1", "sphere", vec![param("Radius", "0.25")], vec![]));
            for c in &mut sim_node.children {
                c.geometry_visible = c.name == name;
            }
            root
        };
        let interior_at = |root: &FsNode, frame: i32| {
            let sim_node = root.children.iter().find(|c| c.node_type == "simnet").unwrap();
            let mut cache = SimCache::default();
            let mut sim = EvalSim::new(frame, 1, &mut cache);
            let mut err = None;
            network_sphere_vertices_with_errors(root, sim_node, &mut err, &mut sim)
        };

        // `input1` at frame 4 is the state at frame 3: two steps of +1.
        let fed = interior_at(&with_only("input1"), 4);
        assert!(!fed.is_empty(), "a visible input draws what the step reads");
        assert!((min_x(&fed) - base - 2.0).abs() < 1e-4, "input1 shows frame 3's state, got +{}", min_x(&fed) - base);

        // `step1` is this frame's pass over that: three steps.
        let pass = interior_at(&with_only("step1"), 4);
        assert!((min_x(&pass) - base - 3.0).abs() < 1e-4, "step1 shows frame 4's pass, got +{}", min_x(&pass) - base);

        // At the start frame the step reads the seed itself.
        let at_start = interior_at(&with_only("input1"), 1);
        assert!((min_x(&at_start) - base).abs() < 1e-4, "at the start frame input1 is the seed");

        // A node beside the chain draws regardless of the sim.
        let beside = interior_at(&with_only("seed1"), 4);
        assert_eq!(beside.num_points(), sphere_detail(Vec3::ZERO, 0.25, 16, 24).num_points(), "the unwired sphere draws");

        // And with nothing on, nothing: the output flag still owns the solved state.
        assert!(interior_at(&with_only("none"), 4).is_empty());
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

    /// The pull arrows measure what the selected node DID: one pair per
    /// point it moved, from its input position to its output one, and none
    /// for the points it left alone.
    #[test]
    fn pull_arrows_measure_what_the_node_moves() {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let group = node(
            "id-group",
            "Group 1",
            "group",
            vec![
                param("Input", "Sphere 1"),
                param("Group Name", "pull"),
                param("Mode", "Random"),
                param("Count", "30"),
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
                param("Value", "0.00:0.06:0.00"),
                param("Combine", "Add"),
                param("Group", "pull"),
            ],
            vec![],
        );
        let root = node("id-root", "root", "node", vec![], vec![sphere, group, pull]);
        let pull = &root.children[2];
        assert!(moves_points(pull));
        assert!(!moves_points(&root.children[1]), "a Group node moves nothing");

        let base = eval(&root, "Group 1");
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(1, 1, &mut cache);
        let moved = point_displacements(&root, pull, &mut sim);
        assert_eq!(moved.len(), base.points().group_members("pull").len(), "one pair per pulled point");
        for (a, b) in &moved {
            assert!((*b - *a - Vec3::new(0.0, 0.06, 0.0)).length() < 1e-5, "vector {:?}", *b - *a);
            assert!(
                base.points().group_members("pull").iter().any(|&p| base.pos(p as usize).distance(*a) < 1e-6),
                "an arrow starts at a point outside the group: {a:?}"
            );
        }

        // Twelve of the thirty, distinct.
        let bases: Vec<Vec3> = moved.iter().map(|(a, _)| *a).collect();
        let picked = spread_sample(&bases, 12);
        assert_eq!(picked.len(), 12);
        assert!(picked.windows(2).all(|w| w[0] < w[1]), "ascending and distinct: {picked:?}");
        // Five strokes per arrow: the shaft and a four-stroke head.
        let shown: Vec<(Vec3, Vec3)> = picked.iter().map(|&i| moved[i]).collect();
        assert_eq!(arrow_vertices(&shown, [1.0; 3]).len(), 12 * 10);
    }

    /// Strength and Scale By scale the pull's EFFECT — the change the node
    /// makes — so they mean one thing under every Combine: an Add moves by
    /// that much of Value, a Set goes that far toward it. Scale By is a
    /// point attribute, so the pull can fall off across the mesh; Strength
    /// is a number, so it takes an expression and the pull can ramp with
    /// the frame. At one, nothing changes from before they existed.
    #[test]
    fn strength_and_scale_by_scale_the_pulls_effect() {
        let pull_of = |extra: Vec<(&str, &str)>, combine: &str, value: &str| {
            let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
            let mut params = vec![
                param("Input", "Sphere 1"),
                param("Operation", "Modify"),
                param("Attribute Name", "Pos"),
                param("Value", value),
                param("Combine", combine),
                param("Group", ""),
            ];
            params.extend(extra.into_iter().map(|(k, v)| param(k, v)));
            let pull = node("id-pull", "Pull 1", "attribute", params, vec![]);
            node("id-root", "root", "node", vec![], vec![sphere, pull])
        };
        let moved_by = |root: &FsNode, frame: i32| -> (Vec<Vec3>, Option<String>) {
            let mut err = None;
            let mut cache = SimCache::default();
            let mut sim = EvalSim::new(frame, 1, &mut cache);
            let before = generate_single_node_geometry_with_errors(root, &root.children[0], &mut Vec::new(), &mut err, &mut sim).unwrap();
            let after = generate_single_node_geometry_with_errors(root, &root.children[1], &mut Vec::new(), &mut err, &mut sim).unwrap();
            ((0..after.num_points()).map(|p| after.pos(p) - before.pos(p)).collect(), err)
        };
        let up = Vec3::new(0.0, 0.06, 0.0);
        let all = |d: &[Vec3], want: Vec3| d.iter().all(|v| v.distance(want) < 1e-5);

        // Absent (an older save) and at one: the pull as it always was.
        let (d, err) = moved_by(&pull_of(vec![], "Add", "0.00:0.06:0.00"), 1);
        assert!(err.is_none() && all(&d, up), "{err:?}");
        let (d, _) = moved_by(&pull_of(vec![("Strength", "1.00")], "Add", "0.00:0.06:0.00"), 1);
        assert!(all(&d, up));
        // Half, none, double.
        for (strength, want) in [("0.50", up * 0.5), ("0.00", Vec3::ZERO), ("2.00", up * 2.0)] {
            let (d, _) = moved_by(&pull_of(vec![("Strength", strength)], "Add", "0.00:0.06:0.00"), 1);
            assert!(all(&d, want), "strength {strength}: {:?}", d[0]);
        }
        // Under Set it is how far toward Value each point goes: at a half,
        // halfway from where it was to (0, 2, 0).
        let root = pull_of(vec![("Strength", "0.50")], "Set", "0.00:2.00:0.00");
        let base = eval(&root, "Sphere 1");
        let out = eval(&root, "Pull 1");
        for p in 0..out.num_points() {
            let want = base.pos(p).lerp(Vec3::new(0.0, 2.0, 0.0), 0.5);
            assert!(out.pos(p).distance(want) < 1e-5, "point {p}: {:?} against {want:?}", out.pos(p));
        }

        // Scale By: each point by its own value of the attribute — the
        // sphere's UV, whose first component runs around it — times Strength.
        let root = pull_of(vec![("Strength", "0.50"), ("Scale By", "UV")], "Add", "0.00:0.06:0.00");
        let base = eval(&root, "Sphere 1");
        let (d, err) = moved_by(&root, 1);
        assert!(err.is_none(), "{err:?}");
        let mut weights = Vec::new();
        for (p, moved) in d.iter().enumerate() {
            let w = base.points().value("UV", p).expect("the sphere carries UV").as_f32();
            assert!(moved.distance(up * 0.5 * w) < 1e-5, "point {p} weighs {w}: {moved:?}");
            weights.push(w);
        }
        let (lo, hi) = weights.iter().fold((f32::MAX, f32::MIN), |(l, h), w| (l.min(*w), h.max(*w)));
        assert!(hi - lo > 0.5, "the fixture's weights vary, or this proves nothing: {lo}..{hi}");

        // An attribute the input lacks is said, and nothing moves.
        let (d, err) = moved_by(&pull_of(vec![("Scale By", "nothing_here")], "Add", "0.00:0.06:0.00"), 1);
        assert!(err.as_deref().is_some_and(|e| e.contains("Scale By") && e.contains("nothing_here")), "{err:?}");
        assert!(all(&d, Vec3::ZERO));

        // Strength is a number, so it takes an expression: a pull that
        // ramps in over ten frames.
        let mut root = pull_of(vec![("Strength", "$F / 10")], "Add", "0.00:0.06:0.00");
        let strength = root.children[1].params.iter_mut().find(|p| p.name == "Strength").unwrap();
        strength.set_type("float");
        strength.set_expr(true);
        for frame in [2, 5, 10] {
            let (d, err) = moved_by(&root, frame);
            assert!(err.is_none(), "{err:?}");
            assert!(all(&d, up * (frame as f32 / 10.0)), "frame {frame}: {:?}", d[0]);
        }
    }

    /// Per Frame makes the pull a RATE: inside a simnet it lands the same
    /// distance a frame whatever the substep count, where without it four
    /// substeps pull four times as far. A Multiply compounds to its factor
    /// over the frame, a Set — which does not accumulate — is left alone,
    /// and a node without the row keeps the per-step behaviour it had.
    #[test]
    fn per_frame_makes_the_pull_independent_of_the_substep_count() {
        let sim_of = |substeps: &str, combine: &str, value: &str, extra: Vec<(&str, &str)>| {
            let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
            let mover = node("id-shift", "Shift 1", "attribute", vec![
                param("Input", "Sphere 1"), param("Operation", "Modify"), param("Attribute Name", "Pos"),
                param("Value", "3.00:0.00:0.00"), param("Combine", "Add"), param("Group", ""),
            ], vec![]);
            let inner_input = node("id-in", "input1", "input", vec![], vec![]);
            let mut params = vec![
                param("Input", "input1"), param("Operation", "Modify"), param("Attribute Name", "Pos"),
                param("Value", value), param("Combine", combine), param("Group", ""),
            ];
            params.extend(extra.into_iter().map(|(k, v)| param(k, v)));
            let pull = node("id-pull", "pull1", "attribute", params, vec![]);
            let inner_output = node("id-out", "output1", "output", vec![param("Input", "pull1")], vec![]);
            let sim_node = node("id-sim", "Simnet 1", "simnet",
                vec![param("Input", "Shift 1"), param("Substeps", substeps)],
                vec![inner_input, pull, inner_output]);
            node("id-root", "root", "node", vec![], vec![sphere, mover, sim_node])
        };
        let at = |root: &FsNode, frame: i32| min_x(&solve_at(root, frame));
        let seed = at(&sim_of("1", "Add", "1.00:0.00:0.00", vec![]), 1);

        // Add: five frames on from the seed, one unit a frame, at any count.
        for substeps in ["1", "4", "16"] {
            let root = sim_of(substeps, "Add", "1.00:0.00:0.00", vec![("Per Frame", "true")]);
            assert!((at(&root, 6) - (seed + 5.0)).abs() < 1e-3, "{substeps} substeps: {}", at(&root, 6) - seed);
        }
        // With Strength: half a unit a frame.
        let root = sim_of("4", "Add", "1.00:0.00:0.00", vec![("Per Frame", "true"), ("Strength", "0.50")]);
        assert!((at(&root, 6) - (seed + 2.5)).abs() < 1e-3);
        // Off, or on a node without the row: per step, four times as far.
        for extra in [vec![("Per Frame", "false")], vec![]] {
            let root = sim_of("4", "Add", "1.00:0.00:0.00", extra);
            assert!((at(&root, 6) - (seed + 20.0)).abs() < 1e-3, "{}", at(&root, 6) - seed);
        }

        // Multiply compounds to its factor over a frame: doubling a frame,
        // whatever the count. (The seed sits well clear of zero.)
        assert!(seed > 0.1, "the fixture's points are on the positive side: {seed}");
        for substeps in ["1", "4", "16"] {
            let root = sim_of(substeps, "Multiply", "2.00:1.00:1.00", vec![("Per Frame", "true")]);
            let got = at(&root, 4);
            assert!((got - seed * 8.0).abs() < seed * 8.0 * 1e-3, "{substeps} substeps: {got} against {}", seed * 8.0);
        }
        // A Set does not accumulate: it sets, at any count, switch or no.
        let root = sim_of("4", "Set", "7.00:0.00:0.00", vec![("Per Frame", "true")]);
        assert!((at(&root, 3) - 7.0).abs() < 1e-4);

        // Outside a simnet there is no step to account for.
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let pull = node("id-pull", "Pull 1", "attribute", vec![
            param("Input", "Sphere 1"), param("Operation", "Modify"), param("Attribute Name", "Pos"),
            param("Value", "1.00:0.00:0.00"), param("Combine", "Add"), param("Group", ""), param("Per Frame", "true"),
        ], vec![]);
        let root = node("id-root", "root", "node", vec![], vec![sphere, pull]);
        assert!((min_x(&eval(&root, "Pull 1")) - (min_x(&eval(&root, "Sphere 1")) + 1.0)).abs() < 1e-5);
    }

    /// Farthest-point sampling takes the extremes before anything between
    /// them, and hands every point back when there are no more than asked.
    #[test]
    fn spread_sample_spreads_out() {
        let line: Vec<Vec3> = (0..=100).map(|i| Vec3::new(i as f32, 0.0, 0.0)).collect();
        let picked = spread_sample(&line, 3);
        assert_eq!(picked, vec![0, 50, 100], "the two ends, then the middle");
        assert_eq!(spread_sample(&line[..5], 12), vec![0, 1, 2, 3, 4]);
        // Coincident points: no more picks than distinct positions.
        let same = vec![Vec3::ONE; 20];
        assert_eq!(spread_sample(&same, 12), vec![0]);
    }

    /// A detangle inside a simnet is told where the substep began: the
    /// state its simnet pushed. The Step Limit is measured against it, so
    /// a pull of a whole unit a frame arrives as half a thickness a frame —
    /// and the same node outside a simnet, with nothing to measure against,
    /// limits nothing. Inside a plain subnet inside the simnet it is still
    /// the simnet's state: the nearest one above that has pushed.
    #[test]
    fn a_detangle_in_a_simnet_knows_where_the_step_began() {
        let graph = |nested: bool| {
            let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
            let pull = |input: &str| {
                node(
                    "id-pull",
                    "pull1",
                    "attribute",
                    vec![
                        param("Input", input),
                        param("Operation", "Modify"),
                        param("Attribute Name", "Pos"),
                        param("Value", "1.00:0.00:0.00"),
                        param("Combine", "Add"),
                        param("Group", ""),
                    ],
                    vec![],
                )
            };
            let detangle = |input: &str| {
                node(
                    "id-detangle",
                    "detangle1",
                    "detangle",
                    vec![param("Input", input), param("Method", "Surface"), param("Thickness", "1.00"), param("Step Limit", "0.50")],
                    vec![],
                )
            };
            let chain = if nested {
                let inside = node(
                    "id-sub",
                    "sub1",
                    "node",
                    vec![param("Input", "pull1")],
                    vec![
                        node("id-sub-in", "input1", "input", vec![], vec![]),
                        detangle("input1"),
                        node("id-sub-out", "output1", "output", vec![param("Input", "detangle1")], vec![]),
                    ],
                );
                vec![
                    node("id-in", "input1", "input", vec![], vec![]),
                    pull("input1"),
                    inside,
                    node("id-out", "output1", "output", vec![param("Input", "sub1")], vec![]),
                ]
            } else {
                vec![
                    node("id-in", "input1", "input", vec![], vec![]),
                    pull("input1"),
                    detangle("pull1"),
                    node("id-out", "output1", "output", vec![param("Input", "detangle1")], vec![]),
                ]
            };
            let sim = node("id-sim", "Simnet 1", "simnet", vec![param("Input", "Sphere 1")], chain);
            let outside = detangle("pull1");
            node("id-root", "root", "node", vec![], vec![sphere, sim, pull("Sphere 1"), outside])
        };
        for nested in [false, true] {
            let root = graph(nested);
            let seed = solve_at(&root, 1);
            let edges = seed.edges();
            let edge = edges.iter().map(|e| (seed.pos(e[1] as usize) - seed.pos(e[0] as usize)).length()).sum::<f32>() / edges.len() as f32;
            let after = solve_at(&root, 4);
            let moved = min_x(&after) - min_x(&seed);
            assert!((moved - 3.0 * 0.5 * edge).abs() < 1e-4, "nested {nested}: three frames moved {moved}, an edge is {edge}");

            let mut cache = SimCache::default();
            let mut sim = EvalSim::new(4, 1, &mut cache);
            let mut err = None;
            let alone = generate_single_node_geometry_with_errors(&root, &root.children[3], &mut Vec::new(), &mut err, &mut sim).expect("evaluates");
            assert!((min_x(&alone) - min_x(&seed) - 1.0).abs() < 1e-4, "outside a simnet the pull arrives whole");
        }
    }

    /// Inside a simnet the arrows start where the points were going into
    /// THIS frame's step — with one substep per frame, the previous frame's
    /// solve — not at the seed, which is what evaluating the node on its own
    /// would give (the `input` node reads the seed with no feedback pushed).
    #[test]
    fn pull_arrows_inside_a_simnet_start_from_this_frames_state() {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let inner_input = node("id-in", "input1", "input", vec![], vec![]);
        let pull = node(
            "id-pull",
            "pull1",
            "attribute",
            vec![
                param("Input", "input1"),
                param("Operation", "Modify"),
                param("Attribute Name", "Pos"),
                param("Value", "1.00:0.00:0.00"),
                param("Combine", "Add"),
                param("Group", ""),
            ],
            vec![],
        );
        let inner_output = node("id-out", "output1", "output", vec![param("Input", "pull1")], vec![]);
        let sim_node = node(
            "id-sim",
            "Simnet 1",
            "simnet",
            vec![param("Input", "Sphere 1")],
            vec![inner_input, pull, inner_output],
        );
        let root = node("id-root", "root", "node", vec![], vec![sphere, sim_node]);
        let pull = &root.children[1].children[1];

        let prev = solve_at(&root, 3);
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(4, 1, &mut cache);
        let moved = point_displacements(&root, pull, &mut sim);
        assert_eq!(moved.len(), prev.num_points());
        let lo = moved.iter().map(|(a, _)| a.x).fold(f32::INFINITY, f32::min);
        assert!((lo - min_x(&prev)).abs() < 1e-4, "arrows start at {lo}, frame 3 solved to {}", min_x(&prev));
        assert!(min_x(&prev) > min_x(&solve_at(&root, 1)) + 1.0, "the fixture must have stepped, or this proves nothing");
        for (a, b) in &moved {
            assert!((*b - *a - Vec3::X).length() < 1e-5);
        }
    }

    /// Under substeps the arrows are anchored at the LAST substep: each
    /// tip lands on the displayed point, since the pull is the chain's only
    /// mover, and each base is one pull short of it. Until 2026-09-28 the
    /// feedback was the frame's starting state, so with four substeps the
    /// arrows sat three pulls behind the geometry they were drawn over — the
    /// "lagging a frame" look. The dived-in scene walk reads the same
    /// feedback, so a visible chain node draws where the output does too.
    #[test]
    fn pull_arrows_inside_a_simnet_anchor_at_the_last_substep() {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let inner_input = node("id-in", "input1", "input", vec![], vec![]);
        let pull = node(
            "id-pull",
            "pull1",
            "attribute",
            vec![
                param("Input", "input1"),
                param("Operation", "Modify"),
                param("Attribute Name", "Pos"),
                param("Value", "1.00:0.00:0.00"),
                param("Combine", "Add"),
                param("Group", ""),
            ],
            vec![],
        );
        let inner_output = node("id-out", "output1", "output", vec![param("Input", "pull1")], vec![]);
        let sim_node = node(
            "id-sim",
            "Simnet 1",
            "simnet",
            vec![param("Input", "Sphere 1"), param("Substeps", "4")],
            vec![inner_input, pull, inner_output],
        );
        let root = node("id-root", "root", "node", vec![], vec![sphere, sim_node]);
        let pull = &root.children[1].children[1];

        let shown = solve_at(&root, 3);
        assert!((min_x(&shown) - (min_x(&solve_at(&root, 1)) + 8.0)).abs() < 1e-4, "two frames of four pulls each");
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(3, 1, &mut cache);
        let moved = point_displacements(&root, pull, &mut sim);
        assert_eq!(moved.len(), shown.num_points());
        for (i, (a, b)) in moved.iter().enumerate() {
            assert!(b.distance(shown.pos(i)) < 1e-4, "arrow {i} ends at {b:?}, the point is drawn at {:?}", shown.pos(i));
            assert!((*b - *a - Vec3::X).length() < 1e-5);
        }

        // The interior view agrees: dived in, the pull node draws on the output.
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(3, 1, &mut cache);
        let mut visited = Vec::new();
        let mut err = None;
        let fed = simnet_step_feedback(&root, &root.children[1], &mut visited, &mut err, &mut sim).expect("solved");
        assert!((min_x(&fed) - (min_x(&shown) - 1.0)).abs() < 1e-4, "the feedback is one pull short of the display");
    }

    /// A Cache-on simnet resumed from DISK exactly at the frame asked for
    /// runs no step, so the feedback the interior view and the pull arrows
    /// read has to come from the file: until 2026-09-28 it was the seed for
    /// that one frame, once per app launch. The disk path is process-scoped
    /// under `cfg(test)`, so the node id here reaches no user cache.
    #[test]
    fn a_disk_resume_landing_on_the_frame_keeps_its_last_substeps_input() {
        let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
        let inner_input = node("id-in", "input1", "input", vec![], vec![]);
        let pull = node(
            "id-pull",
            "pull1",
            "attribute",
            vec![
                param("Input", "input1"),
                param("Operation", "Modify"),
                param("Attribute Name", "Pos"),
                param("Value", "1.00:0.00:0.00"),
                param("Combine", "Add"),
                param("Group", ""),
            ],
            vec![],
        );
        let inner_output = node("id-out", "output1", "output", vec![param("Input", "pull1")], vec![]);
        let sim_node = node(
            "id-sim-disk-resume",
            "Simnet 1",
            "simnet",
            vec![param("Input", "Sphere 1"), param("Substeps", "2"), param("Cache", "true")],
            vec![inner_input, pull, inner_output],
        );
        let root = node("id-root", "root", "node", vec![], vec![sphere, sim_node]);
        let simnet = &root.children[1];
        let path = sim_cache_path(&simnet.id).expect("a cache path");
        let _ = std::fs::remove_file(&path);

        // Solve to frame 3 (two frames of two pulls): the file is written.
        let shown = solve_at(&root, 3);
        assert!(path.exists(), "Cache on wrote {path:?}");
        assert!((min_x(&shown) - (min_x(&solve_at(&root, 1)) + 4.0)).abs() < 1e-4);

        // A fresh in-memory cache — a relaunch — asked for frame 3 resumes
        // from the file with no step to run. The feedback is the last
        // substep's input, one pull short of the display, not the seed.
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(3, 1, &mut cache);
        let mut visited = Vec::new();
        let mut err = None;
        let fed = simnet_step_feedback(&root, simnet, &mut visited, &mut err, &mut sim).expect("resumed");
        assert!((min_x(&fed) - (min_x(&shown) - 1.0)).abs() < 1e-4, "feedback at {}, display at {}", min_x(&fed), min_x(&shown));
        let moved = point_displacements(&root, &simnet.children[1], &mut sim);
        assert_eq!(moved.len(), shown.num_points());
        for (i, (_, b)) in moved.iter().enumerate() {
            assert!(b.distance(shown.pos(i)) < 1e-4, "arrow {i} ends on the displayed point after a disk resume");
        }

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("simcache.tmp"));
    }

    /// A solve keeps checkpoints, and a scrub resumes from the nearest one
    /// behind it: going back a frame is a handful of steps, not the whole
    /// history. What it arrives at is what a solve from the seed arrives
    /// at — the state and the feedback both — whichever way it came; an
    /// edit drops them; and they are kept within a count however long the
    /// solve runs.
    #[test]
    fn a_scrub_resumes_from_a_checkpoint_and_arrives_at_the_same_state() {
        let sim_of = |value: &str, substeps: &str| {
            let sphere = node("id-sphere", "Sphere 1", "sphere", vec![param("Radius", "0.5")], vec![]);
            let inner_input = node("id-in", "input1", "input", vec![], vec![]);
            let pull = node("id-pull", "pull1", "attribute", vec![
                param("Input", "input1"), param("Operation", "Modify"), param("Attribute Name", "Pos"),
                param("Value", value), param("Combine", "Multiply"), param("Group", ""),
            ], vec![]);
            let inner_output = node("id-out", "output1", "output", vec![param("Input", "pull1")], vec![]);
            let sim_node = node("id-sim-checkpoints", "Simnet 1", "simnet",
                vec![param("Input", "Sphere 1"), param("Substeps", substeps)],
                vec![inner_input, pull, inner_output]);
            node("id-root", "root", "node", vec![], vec![sphere, sim_node])
        };
        // A Multiply, so every frame's state is a function of the one
        // before and a wrong resume shows.
        let root = sim_of("1.01:0.99:1.02", "2");
        let simnet = &root.children[1];
        let at = |cache: &mut SimCache, frame: i32| -> (Detail, Detail, usize) {
            let before = cache.steps_run();
            let mut sim = EvalSim::new(frame, 1, cache);
            let mut err = None;
            let fed = simnet_step_feedback(&root, simnet, &mut Vec::new(), &mut err, &mut sim).expect("solves");
            let state = resolve_simnet_geometry_with_errors(&root, simnet, &mut Vec::new(), &mut err, &mut sim).expect("solves");
            (state, fed, cache.steps_run() - before)
        };
        let fresh = |frame: i32| {
            let mut cache = SimCache::default();
            let (state, fed, _) = at(&mut cache, frame);
            (state, fed)
        };

        let mut cache = SimCache::default();
        // Out to frame 101: a hundred frames of two substeps.
        let (_, _, cost) = at(&mut cache, 101);
        assert_eq!(cost, 200);
        assert_eq!(cache.checkpoint_frames(&simnet.id), vec![10, 20, 30, 40, 50, 60, 70, 80, 90]);

        // Back, forward, back again, onto a checkpoint, to the start and to
        // where it came from: each arrives where a solve from the seed
        // does, and costs the frames from the checkpoint behind it.
        for (frame, frames_stepped) in [(100, 9), (38, 7), (84, 3), (37, 6), (61, 0), (1, 0), (101, 0), (96, 5), (99, 3)] {
            let (state, fed, cost) = at(&mut cache, frame);
            let (want, want_fed) = fresh(frame);
            assert_eq!(state.positions(), want.positions(), "frame {frame}: the state");
            assert_eq!(fed.positions(), want_fed.positions(), "frame {frame}: what its last substep consumed");
            assert_eq!(cost, frames_stepped * 2, "frame {frame} cost {cost} substeps");
        }
        // The frames a scrub LEFT are kept beside the interval's.
        let kept = cache.checkpoint_frames(&simnet.id);
        for frame in [100, 83, 60] {
            assert!(kept.contains(&frame), "{frame} was left behind, and kept: {kept:?}");
        }

        // Played a frame at a time, as the playbar does, the interval's
        // frames are kept as they are left.
        let mut cache = SimCache::default();
        for frame in 1..=35 {
            at(&mut cache, frame);
        }
        assert_eq!(cache.checkpoint_frames(&simnet.id), vec![10, 20, 30]);
        let (state, _, cost) = at(&mut cache, 24);
        assert_eq!((cost, state.positions() == fresh(24).0.positions()), (3 * 2, true));

        // An edit is another solve: nothing of the old one is resumed from.
        let edited = sim_of("1.02:0.99:1.02", "2");
        let mut sim = EvalSim::new(51, 1, &mut cache);
        let mut err = None;
        let before = sim.cache.steps_run();
        let state = resolve_simnet_geometry_with_errors(&edited, &edited.children[1], &mut Vec::new(), &mut err, &mut sim).unwrap();
        assert_eq!(sim.cache.steps_run() - before, 100, "fifty frames from the seed");
        assert_ne!(state.positions(), fresh(51).0.positions());
        assert_eq!(cache.checkpoint_frames(&simnet.id), vec![10, 20, 30, 40], "and its own checkpoints");

        // However long it runs, the count is bounded, and what is kept
        // stays spread over the whole of it.
        let root = sim_of("1.0001:1.0:1.0", "1");
        let simnet = &root.children[1];
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(2001, 1, &mut cache);
        resolve_simnet_geometry_with_errors(&root, simnet, &mut Vec::new(), &mut None, &mut sim).unwrap();
        let kept = cache.checkpoint_frames(&simnet.id);
        assert!(kept.len() <= CHECKPOINTS_MAX && kept.len() > CHECKPOINTS_MAX / 3, "{}", kept.len());
        assert!(kept[0] <= 100 && *kept.last().unwrap() >= 1900, "{kept:?}");
        let widest = kept.windows(2).map(|w| w[1] - w[0]).max().unwrap();
        assert!(widest <= 80, "no gap wider than the spacing the cap asks for: {widest}");
    }

    /// A Relax's `Rest` wire resolves to its OWN sibling. It was a
    /// whole-tree search by name, so in the second of two subnets that each
    /// hold a `shape`, Rest found the first one's — here a sphere of half
    /// the radius, whose shorter edges the relax then pulled the second
    /// sphere toward. Rest equal to the input is a relax with nothing to do.
    #[test]
    fn a_rest_wire_resolves_to_its_own_sibling() {
        let small = node("id-a-shape", "shape", "sphere", vec![param("Radius", "0.5")], vec![]);
        let a = node("id-a", "a", "node", vec![], vec![small]);
        let big = node("id-b-shape", "shape", "sphere", vec![param("Radius", "1.0")], vec![]);
        let relax = node(
            "id-b-relax",
            "relax1",
            "relax",
            vec![
                param("Input", "shape"),
                param("Rest", "shape"),
                param("Stiffness", "1.00"),
                param("Iterations", "8"),
            ],
            vec![],
        );
        let b = node("id-b", "b", "node", vec![], vec![big, relax]);
        let root = node("id-root", "root", "node", vec![], vec![a, b]);
        let relax = &root.children[1].children[1];

        assert_eq!(param_node(&root, relax, "Rest").map(|n| n.id.as_str()), Some("id-b-shape"));
        let mut cache = SimCache::default();
        let mut sim = EvalSim::new(1, 1, &mut cache);
        let input = generate_single_node_geometry_with_errors(&root, &root.children[1].children[0], &mut Vec::new(), &mut None, &mut sim).unwrap();
        let out = generate_single_node_geometry_with_errors(&root, relax, &mut Vec::new(), &mut None, &mut sim).unwrap();
        let moved = (0..out.num_points()).map(|p| out.pos(p).distance(input.pos(p))).fold(0.0f32, f32::max);
        assert!(moved < 1e-5, "relax against its own input moved a point {moved}");
    }
}
