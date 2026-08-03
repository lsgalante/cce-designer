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

#[allow(dead_code)]
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

pub fn sphere_vertices_res(center: Vec3, radius: f32, lat_steps: usize, lon_steps: usize) -> Geometry {
    let mut vertices = Vec::new();

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
            vertices.push(sphere_vertex(center, p00));
            vertices.push(sphere_vertex(center, p10));
            vertices.push(sphere_vertex(center, p11));
            vertices.push(sphere_vertex(center, p00));
            vertices.push(sphere_vertex(center, p11));
            vertices.push(sphere_vertex(center, p01));
        }
    }

    Geometry { vertices }
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

fn sphere_vertex(center: Vec3, point: Vec3) -> GVertex {
    let n = (point - center).normalize_or_zero();
    let u = 0.5 + n.z.atan2(n.x) / std::f32::consts::TAU;
    let v = 0.5 - n.y.asin() / std::f32::consts::PI;

    let pos = point.to_array();
    let col = [0.35 + n.x.abs() * 0.35, 0.45 + n.y.abs() * 0.35, 0.85];

    let mut attributes = HashMap::new();
    attributes.insert("Norm".to_string(), GAttribute::Float3(n.to_array()));
    attributes.insert("UV".to_string(), GAttribute::Float2([u, v]));

    GVertex {
        pos,
        col,
        attributes,
    }
}

pub fn line_vertices(start: Vec3, end: Vec3, thickness: f32) -> Geometry {
    let mut vertices = Vec::new();
    let dir = (end - start).normalize_or_zero();
    if dir.length_squared() < 0.0001 {
        return Geometry { vertices };
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
    
    // Helper to add a triangle face
    let mut add_quad = |p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, normal: Vec3, color: [f32; 3]| {
        let make_vertex = |p: Vec3| {
            let mut attributes = HashMap::new();
            attributes.insert("Norm".to_string(), GAttribute::Float3(normal.to_array()));
            attributes.insert("UV".to_string(), GAttribute::Float2([0.0, 0.0]));
            GVertex {
                pos: p.to_array(),
                col: color,
                attributes,
            }
        };
        // Triangle 1: p0, p1, p2
        vertices.push(make_vertex(p0));
        vertices.push(make_vertex(p1));
        vertices.push(make_vertex(p2));
        // Triangle 2: p0, p2, p3
        vertices.push(make_vertex(p0));
        vertices.push(make_vertex(p2));
        vertices.push(make_vertex(p3));
    };

    let col = [0.85, 0.45, 0.35]; // distinct color for lines
    
    // Front face (start cap)
    add_quad(c0, c1, c2, c3, -dir, col);
    // Back face (end cap)
    add_quad(c5, c4, c7, c6, dir, col);
    // Left face
    add_quad(c4, c0, c3, c7, -u, col);
    // Right face
    add_quad(c1, c5, c6, c2, u, col);
    // Top face
    add_quad(c3, c2, c6, c7, v, col);
    // Bottom face
    add_quad(c0, c4, c5, c1, -v, col);

    Geometry { vertices }
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

pub fn generate_single_node_geometry(root: &FsNode, target: &FsNode, visited: &mut Vec<String>) -> Option<Geometry> {
    let mut err = None;
    generate_single_node_geometry_with_errors(root, target, visited, &mut err)
}

pub fn generate_single_node_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
) -> Option<Geometry> {
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
        Some(sphere_vertices(center, node_param_f32(target, "Radius", 0.5).max(0.05)))
    } else if target.node_type.eq_ignore_ascii_case("line") {
        let idx = find_sphere_index(root, target)?;
        let start = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
        let length = node_param_f32(target, "Length", 1.0);
        let thickness = node_param_f32(target, "Thickness", 0.02);
        let end = start + Vec3::new(0.0, length, 0.0);
        Some(line_vertices(start, end, thickness))
    } else if target.node_type.eq_ignore_ascii_case("add") {
        let idx = find_sphere_index(root, target)?;
        let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
        let num_points = node_param_f32(target, "Points", 100.0) as i32;
        let mut geom = Geometry::new();
        for i in 0..num_points {
            let t = i as f32 / num_points.max(1) as f32;
            let angle = t * std::f32::consts::TAU * 3.0;
            let r = 0.4 * t;
            let px = center.x + r * angle.cos();
            let py = center.y + t * 0.5 - 0.25;
            let pz = center.z + r * angle.sin();
            let pt_center = Vec3::new(px, py, pz);
            geom.merge(sphere_vertices_res(pt_center, 0.02, 6, 8));
        }
        Some(geom)
    } else if target.node_type.eq_ignore_ascii_case("transform") {
        resolve_transform_geometry_with_errors(root, target, visited, ocl_error)
    } else if target.node_type.eq_ignore_ascii_case("scatter") {
        resolve_scatter_geometry_with_errors(root, target, visited, ocl_error)
    } else if target.node_type.eq_ignore_ascii_case("group") {
        resolve_group_geometry_with_errors(root, target, visited, ocl_error)
    } else if target.node_type.eq_ignore_ascii_case("opencl") {
        resolve_opencl_geometry_with_errors(root, target, visited, ocl_error)
    } else if target.node_type.eq_ignore_ascii_case("node") {
        if let Some(output_node) = target.children.iter().find(|c| c.node_type.eq_ignore_ascii_case("output")) {
            generate_single_node_geometry_with_errors(root, output_node, visited, ocl_error)
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
                generate_single_node_geometry_with_errors(root, node, visited, ocl_error)
            } else {
                None
            }
        }
    } else if target.node_type.eq_ignore_ascii_case("input") {
        if let Some(parent) = find_parent_node(root, &target.id) {
            let input_name = node_param_str(parent, "Input", "");
            if !input_name.is_empty() {
                if let Some(input_node) = find_node_by_name(root, &input_name) {
                    generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error)
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

pub fn resolve_transform_geometry(root: &FsNode, target: &FsNode, visited: &mut Vec<String>) -> Option<Geometry> {
    let mut err = None;
    resolve_transform_geometry_with_errors(root, target, visited, &mut err)
}

pub fn resolve_transform_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
) -> Option<Geometry> {
    let input_name = node_param_str(target, "Input", "");
    if input_name.is_empty() {
        return None;
    }
    let input_node = find_node_by_name(root, &input_name)?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error)?;
    let translation = node_param_vec3(target, "Translation", Vec3::ZERO);
    for v in &mut geom.vertices {
        v.pos[0] += translation.x;
        v.pos[1] += translation.y;
        v.pos[2] += translation.z;
    }
    Some(geom)
}

pub fn resolve_scatter_geometry(root: &FsNode, target: &FsNode, visited: &mut Vec<String>) -> Option<Geometry> {
    let mut err = None;
    resolve_scatter_geometry_with_errors(root, target, visited, &mut err)
}

/// The Group node: pass the input geometry through, tagging the elements
/// selected by an axis-aligned box (Center/Size) with a per-vertex membership
/// attribute `group:<name>` = Float(1.0). Element Type picks the selection
/// unit over the triangle soup — Points (per vertex), Primitives (a
/// triangle's centroid; all three vertices tag together), Edges (a triangle
/// edge with both endpoints inside; its two vertices tag). Membership rides
/// GVertex::attributes so it survives merges and native filters — downstream
/// nodes consume it by reading the same attribute. Highlight tints members
/// toward a warm accent so the group reads in the viewport.
pub fn resolve_group_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
) -> Option<Geometry> {
    let input_name = node_param_str(target, "Input", "");
    if input_name.is_empty() {
        return None;
    }
    let input_node = find_node_by_name(root, &input_name)?;
    let mut geom = generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error)?;

    let group_name = node_param_str(target, "Group Name", "group1");
    let attr = format!("group:{}", group_name.trim());
    let etype = node_param_str(target, "Element Type", "Points").to_lowercase();
    let center = node_param_vec3(target, "Center", Vec3::ZERO);
    let half = node_param_vec3(target, "Size", Vec3::ONE) * 0.5;
    let invert = node_param_str(target, "Invert", "false") == "true";
    let highlight = node_param_str(target, "Highlight", "true") == "true";

    let inside = |p: &[f32; 3]| -> bool {
        (p[0] - center.x).abs() <= half.x
            && (p[1] - center.y).abs() <= half.y
            && (p[2] - center.z).abs() <= half.z
    };

    let n = geom.vertices.len();
    let mut member = vec![false; n];
    match etype.as_str() {
        "primitives" => {
            for tri in 0..n / 3 {
                let b = tri * 3;
                let centroid = [
                    (geom.vertices[b].pos[0] + geom.vertices[b + 1].pos[0] + geom.vertices[b + 2].pos[0]) / 3.0,
                    (geom.vertices[b].pos[1] + geom.vertices[b + 1].pos[1] + geom.vertices[b + 2].pos[1]) / 3.0,
                    (geom.vertices[b].pos[2] + geom.vertices[b + 1].pos[2] + geom.vertices[b + 2].pos[2]) / 3.0,
                ];
                if inside(&centroid) {
                    member[b] = true;
                    member[b + 1] = true;
                    member[b + 2] = true;
                }
            }
        }
        "edges" => {
            for tri in 0..n / 3 {
                let b = tri * 3;
                for (a, c) in [(0usize, 1usize), (1, 2), (2, 0)] {
                    if inside(&geom.vertices[b + a].pos) && inside(&geom.vertices[b + c].pos) {
                        member[b + a] = true;
                        member[b + c] = true;
                    }
                }
            }
        }
        _ => {
            for (i, v) in geom.vertices.iter().enumerate() {
                if inside(&v.pos) {
                    member[i] = true;
                }
            }
        }
    }
    if invert {
        for m in member.iter_mut() {
            *m = !*m;
        }
    }

    for (i, v) in geom.vertices.iter_mut().enumerate() {
        if member[i] {
            v.attributes.insert(attr.clone(), GAttribute::Float(1.0));
            if highlight {
                let acc = [1.0, 0.78, 0.20];
                for k in 0..3 {
                    v.col[k] = v.col[k] * 0.35 + acc[k] * 0.65;
                }
            }
        }
    }
    Some(geom)
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
) -> Option<Geometry> {
    if visited.contains(&target.id) {
        return None;
    }
    visited.push(target.id.clone());

    let input_name = node_param_str(target, "Input", "");
    if input_name.is_empty() {
        visited.pop();
        return None;
    }
    let input_node = match find_node_by_name(root, &input_name) {
        Some(node) => node,
        None => {
            visited.pop();
            return None;
        }
    };
    let geom = match generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error) {
        Some(g) => g,
        None => {
            visited.pop();
            return None;
        }
    };

    let num_points = node_param_f32(target, "Points", 100.0) as usize;
    let radius = node_param_f32(target, "Radius", 0.02);

    let ray_dir = Vec3::new(0.19, 0.98, 0.05).normalize();
    let mut triangles = Vec::new();
    let mut min_pos = Vec3::splat(f32::MAX);
    let mut max_pos = Vec3::splat(f32::MIN);

    for chunk in geom.vertices.chunks_exact(3) {
        let v0 = Vec3::from_array(chunk[0].pos);
        let v1 = Vec3::from_array(chunk[1].pos);
        let v2 = Vec3::from_array(chunk[2].pos);

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
        Geometry::new()
    } else {
        let mut rng = SimpleRng::new(1337);
        let mut scattered_geom = Geometry::new();
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
                scattered_geom.merge(sphere_vertices_res(candidate, radius, 6, 8));
                found_count += 1;
            }
        }
        scattered_geom
    };

    visited.pop();
    Some(res)
}

fn rewrite_kernel_signature(code: &str) -> String {
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
                let args_str = &code[start_args..closing_paren_idx].trim();
                let insertion = if args_str.is_empty() {
                    "__global const float* param_values"
                } else {
                    ", __global const float* param_values"
                };
                return format!("{}{}{}", before, insertion, after);
            }
        }
    }
    code.to_string()
}

pub fn preprocess_opencl_code(code: &str) -> String {
    let parsed_params = parse_dynamic_params(code);
    if parsed_params.is_empty() {
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

    let mut processed = rewrite_kernel_signature(code);

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
                if let Some(first_quote_pos) = args_str.find(|c| c == '"' || c == '\'') {
                    let quote_char = args_str.chars().nth(first_quote_pos).unwrap();
                    if let Some(second_quote_pos) = args_str[first_quote_pos + 1..].find(quote_char) {
                        let name = &args_str[first_quote_pos + 1..first_quote_pos + 1 + second_quote_pos];
                        if !name.is_empty() {
                            if let Some(&flat_idx) = param_indices.get(name) {
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
) -> Option<Geometry> {
    let input_name = node_param_str(target, "Input", "");
    let mut geom = if !input_name.is_empty() {
        // Siblings first, exactly like the output type's lookup: subnet
        // templates (Extrude) wire their inner opencl to a child named
        // "input1", and a global-first search would resolve to the FIRST
        // subnet's child once two instances exist.
        let sibling = find_parent_node(root, &target.id)
            .and_then(|p| p.children.iter().find(|c| c.name == input_name || c.id == input_name));
        if let Some(input_node) = sibling.or_else(|| find_node_by_name(root, &input_name)) {
            generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error).unwrap_or_default()
        } else {
            Geometry::default()
        }
    } else {
        Geometry::default()
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
        if let Err(e) = run_opencl_kernel_with_params(&processed_code, &mut geom, &flat_values) {
            if ocl_error.is_none() {
                *ocl_error = Some(e);
            }
        }
    }
    Some(geom)
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
        let count = geom.vertices.len();

        // Prepare flat position and color buffers
        let mut pos_data: Vec<cl_float> = Vec::with_capacity(count * 3);
        let mut col_data: Vec<cl_float> = Vec::with_capacity(count * 3);
        for v in &geom.vertices {
            pos_data.extend_from_slice(&v.pos);
            col_data.extend_from_slice(&v.col);
        }

        // Create device buffers
        let mut pos_buf = unsafe {
            ClBuffer::<cl_float>::create(&context, CL_MEM_READ_WRITE, count * 3, std::ptr::null_mut())
                .map_err(|e| format!("Failed to create positions buffer: {:?}", e))?
        };
        let mut col_buf = unsafe {
            ClBuffer::<cl_float>::create(&context, CL_MEM_READ_WRITE, count * 3, std::ptr::null_mut())
                .map_err(|e| format!("Failed to create colors buffer: {:?}", e))?
        };

        // Write data to device
        let _write_pos_event = unsafe {
            queue.enqueue_write_buffer(&mut pos_buf, CL_TRUE, 0, &pos_data, &[])
                .map_err(|e| format!("Failed to write positions buffer: {:?}", e))?
        };
        let _write_col_event = unsafe {
            queue.enqueue_write_buffer(&mut col_buf, CL_TRUE, 0, &col_data, &[])
                .map_err(|e| format!("Failed to write colors buffer: {:?}", e))?
        };

        // Execute kernel
        let mut exec = ExecuteKernel::new(kernel);
        let kernel_event = unsafe {
            exec.set_arg(&pos_buf)
                .set_arg(&col_buf)
                .set_arg(&(count as cl_int));

            let num_args = kernel.num_args().unwrap_or(0);
            if num_args >= 4 {
                exec.set_arg(&param_values_buf);
            }

            exec.set_global_work_size(count)
                .enqueue_nd_range(&queue)
                .map_err(|e| format!("Failed to enqueue kernel: {:?}", e))?
        };

        kernel_event.wait().map_err(|e| format!("Failed to wait for kernel: {:?}", e))?;

        // Read data back from device
        let _read_pos_event = unsafe {
            queue.enqueue_read_buffer(&pos_buf, CL_TRUE, 0, &mut pos_data, &[])
                .map_err(|e| format!("Failed to read positions buffer: {:?}", e))?
        };
        let _read_col_event = unsafe {
            queue.enqueue_read_buffer(&col_buf, CL_TRUE, 0, &mut col_data, &[])
                .map_err(|e| format!("Failed to read colors buffer: {:?}", e))?
        };

        // Write back to Geometry
        for i in 0..count {
            geom.vertices[i].pos = [pos_data[i * 3], pos_data[i * 3 + 1], pos_data[i * 3 + 2]];
            geom.vertices[i].col = [col_data[i * 3], col_data[i * 3 + 1], col_data[i * 3 + 2]];
        }
    }

    Ok(())
}

pub fn is_geometry_node_type(node_type: &str) -> bool {
    let nt = node_type.to_lowercase();
    nt == "sphere"
        || nt == "line"
        || nt == "add"
        || nt == "transform"
        || nt == "opencl"
        || nt == "box"
        || nt == "input"
        || nt == "output"
        || nt == "scatter"
        || nt == "group"
}

pub fn network_sphere_vertices(root: &FsNode) -> Geometry {
    let mut err = None;
    network_sphere_vertices_with_errors(root, &mut err)
}

pub fn network_sphere_vertices_with_errors(root: &FsNode, ocl_error: &mut Option<String>) -> Geometry {
    fn visit(root: &FsNode, node: &FsNode, parent_visible: bool, count: &mut usize, out: &mut Geometry, ocl_error: &mut Option<String>) {
        let is_visible = parent_visible && node.geometry_visible;
        if node.node_type.eq_ignore_ascii_case("sphere") {
            let idx = *count;
            *count += 1;
            if is_visible {
                let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                out.merge(sphere_vertices(center, node_param_f32(node, "Radius", 0.5).max(0.05)));
            }
        } else if node.node_type.eq_ignore_ascii_case("line") {
            let idx = *count;
            *count += 1;
            if is_visible {
                let start = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                let length = node_param_f32(node, "Length", 1.0);
                let thickness = node_param_f32(node, "Thickness", 0.02);
                let end = start + Vec3::new(0.0, length, 0.0);
                out.merge(line_vertices(start, end, thickness));
            }
        } else if node.node_type.eq_ignore_ascii_case("add") {
            let idx = *count;
            *count += 1;
            if is_visible {
                let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                let num_points = node_param_f32(node, "Points", 100.0) as i32;
                for i in 0..num_points {
                    let t = i as f32 / num_points.max(1) as f32;
                    let angle = t * std::f32::consts::TAU * 3.0;
                    let r = 0.4 * t;
                    let px = center.x + r * angle.cos();
                    let py = center.y + t * 0.5 - 0.25;
                    let pz = center.z + r * angle.sin();
                    let pt_center = Vec3::new(px, py, pz);
                    out.merge(sphere_vertices_res(pt_center, 0.02, 6, 8));
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("transform") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_transform_geometry_with_errors(root, node, &mut visited, ocl_error) {
                    out.merge(geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("scatter") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_scatter_geometry_with_errors(root, node, &mut visited, ocl_error) {
                    out.merge(geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("group") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_group_geometry_with_errors(root, node, &mut visited, ocl_error) {
                    out.merge(geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("opencl") {
            let _idx = *count;
            *count += 1;
            if is_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_opencl_geometry_with_errors(root, node, &mut visited, ocl_error) {
                    out.merge(geom);
                }
            }
        }
        for child in &node.children {
            visit(root, child, is_visible, count, out, ocl_error);
        }
    }

    let mut out = Geometry::new();
    let mut count = 0;
    for child in &root.children {
        visit(root, child, true, &mut count, &mut out, ocl_error);
    }
    out
}

pub fn find_sphere_index(root: &FsNode, target: &FsNode) -> Option<usize> {
    fn visit(node: &FsNode, target: &FsNode, count: &mut usize) -> Option<usize> {
        let is_target = std::ptr::eq(node, target);
        if node.node_type.eq_ignore_ascii_case("sphere") 
            || node.node_type.eq_ignore_ascii_case("line") 
            || node.node_type.eq_ignore_ascii_case("add")
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_add_node_points() {
        let add_node = FsNode {
            id: "id Add points test".to_string(),
            inputs: 1,
            outputs: 1,
            name: "Add points test".to_string(),
            node_type: "add".to_string(),
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
            children: vec![add_node],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let geom = network_sphere_vertices(&root);
        assert_eq!(geom.vertices.len(), 5 * 288);
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
        assert!(!geom1.vertices.is_empty());
        let avg_x = geom1.vertices.iter().map(|v| v.pos[0]).sum::<f32>() / geom1.vertices.len() as f32;
        let avg_y = geom1.vertices.iter().map(|v| v.pos[1]).sum::<f32>() / geom1.vertices.len() as f32;
        let avg_z = geom1.vertices.iter().map(|v| v.pos[2]).sum::<f32>() / geom1.vertices.len() as f32;
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
        let avg_chained_x = geom2.vertices.iter().map(|v| v.pos[0]).sum::<f32>() / geom2.vertices.len() as f32;
        let avg_chained_y = geom2.vertices.iter().map(|v| v.pos[1]).sum::<f32>() / geom2.vertices.len() as f32;
        let avg_chained_z = geom2.vertices.iter().map(|v| v.pos[2]).sum::<f32>() / geom2.vertices.len() as f32;
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
        let geom = resolve_opencl_geometry_with_errors(&root, &opencl_node, &mut visited, &mut err).unwrap();
        assert!(!geom.vertices.is_empty());
        assert!(err.is_none());

        // The sphere should be translated up by 2.0 on the y axis compared to the standard sphere (which centers around y=0.55 for index 0)
        let avg_y = geom.vertices.iter().map(|v| v.pos[1]).sum::<f32>() / geom.vertices.len() as f32;
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

        // 15 scattered spheres. Each sphere with lat_steps=6, lon_steps=8 has:
        // 6 * 8 = 48 quads. Each quad has 6 vertices. 48 * 6 = 288 vertices.
        // 15 * 288 = 4320 vertices.
        assert_eq!(geom.vertices.len(), 15 * 288);

        // Center of sphere at idx 0 is Vec3::new(-1.875, 0.55, 0.0). Radius = 0.5.
        // Let's check that each scattered sphere's center is indeed inside the parent sphere.
        let center = Vec3::new(-1.875, 0.55, 0.0);
        for chunk in geom.vertices.chunks_exact(288) {
            let mut sum = Vec3::ZERO;
            for v in chunk {
                sum += Vec3::from_array(v.pos);
            }
            let avg = sum / 288.0;
            let dist = avg.distance(center);
            assert!(dist <= 0.5, "Scattered point center {:?} (distance {}) is outside the sphere of radius 0.5", avg, dist);
        }
    }
}


