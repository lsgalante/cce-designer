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

impl Vertex3D {
    const ATTRIBS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];

    pub fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
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

pub fn sphere_vertices(center: Vec3, radius: f32) -> Geometry {
    let lat_steps = 16;
    let lon_steps = 24;
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
    if visited.contains(&target.name) {
        return None;
    }
    visited.push(target.name.clone());

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
            geom.merge(sphere_vertices(pt_center, 0.02));
        }
        Some(geom)
    } else if target.node_type.eq_ignore_ascii_case("transform") {
        resolve_transform_geometry_with_errors(root, target, visited, ocl_error)
    } else if target.node_type.eq_ignore_ascii_case("opencl") {
        resolve_opencl_geometry_with_errors(root, target, visited, ocl_error)
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

pub fn resolve_opencl_geometry_with_errors(
    root: &FsNode,
    target: &FsNode,
    visited: &mut Vec<String>,
    ocl_error: &mut Option<String>,
) -> Option<Geometry> {
    let input_name = node_param_str(target, "Input", "");
    let mut geom = if !input_name.is_empty() {
        let input_node = find_node_by_name(root, &input_name)?;
        generate_single_node_geometry_with_errors(root, input_node, visited, ocl_error)?
    } else {
        Geometry::default()
    };
    let code = node_param_str(target, "Code", "");
    if !code.is_empty() {
        if let Err(e) = run_opencl_kernel(&code, &mut geom) {
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
    let queue = unsafe { CommandQueue::create(&context, device_id, 0) }.ok()?;
    Some(OpenClCache {
        device,
        context,
        queue,
        kernels: std::collections::HashMap::new(),
    })
}

pub fn run_opencl_kernel(code: &str, geom: &mut Geometry) -> Result<(), String> {
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
            ClBuffer::<cl_float>::create(&context, CL_MEM_READ_WRITE, in_count * 3, std::ptr::null_mut())
                .map_err(|e| format!("Failed to create input positions buffer: {:?}", e))?
        };
        let mut in_col_buf = unsafe {
            ClBuffer::<cl_float>::create(&context, CL_MEM_READ_WRITE, in_count * 3, std::ptr::null_mut())
                .map_err(|e| format!("Failed to create input colors buffer: {:?}", e))?
        };

        // Write input data to GPU
        let _write_pos_event = unsafe {
            queue.enqueue_write_buffer(&mut in_pos_buf, CL_TRUE, 0, &in_pos_data, &[])
                .map_err(|e| format!("Failed to write input positions buffer: {:?}", e))?
        };
        let _write_col_event = unsafe {
            queue.enqueue_write_buffer(&mut in_col_buf, CL_TRUE, 0, &in_col_data, &[])
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
        let kernel_event = unsafe {
            ExecuteKernel::new(kernel)
                .set_arg(&in_pos_buf)
                .set_arg(&in_col_buf)
                .set_arg(&(in_count as cl_int))
                .set_arg(&out_pos_buf)
                .set_arg(&out_col_buf)
                .set_arg(&out_count_buf)
                .set_arg(&(max_vertices as cl_int))
                .set_global_work_size(global_work_size)
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
        let kernel_event = unsafe {
            ExecuteKernel::new(kernel)
                .set_arg(&pos_buf)
                .set_arg(&col_buf)
                .set_arg(&(count as cl_int))
                .set_global_work_size(count)
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

pub fn network_sphere_vertices(root: &FsNode) -> Geometry {
    let mut err = None;
    network_sphere_vertices_with_errors(root, &mut err)
}

pub fn network_sphere_vertices_with_errors(root: &FsNode, ocl_error: &mut Option<String>) -> Geometry {
    fn visit(root: &FsNode, node: &FsNode, count: &mut usize, out: &mut Geometry, ocl_error: &mut Option<String>) {
        if node.node_type.eq_ignore_ascii_case("sphere") {
            let idx = *count;
            *count += 1;
            if node.geometry_visible {
                let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                out.merge(sphere_vertices(center, node_param_f32(node, "Radius", 0.5).max(0.05)));
            }
        } else if node.node_type.eq_ignore_ascii_case("line") {
            let idx = *count;
            *count += 1;
            if node.geometry_visible {
                let start = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                let length = node_param_f32(node, "Length", 1.0);
                let thickness = node_param_f32(node, "Thickness", 0.02);
                let end = start + Vec3::new(0.0, length, 0.0);
                out.merge(line_vertices(start, end, thickness));
            }
        } else if node.node_type.eq_ignore_ascii_case("add") {
            let idx = *count;
            *count += 1;
            if node.geometry_visible {
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
                    out.merge(sphere_vertices(pt_center, 0.02));
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("transform") {
            let idx = *count;
            *count += 1;
            if node.geometry_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_transform_geometry_with_errors(root, node, &mut visited, ocl_error) {
                    out.merge(geom);
                }
            }
        } else if node.node_type.eq_ignore_ascii_case("opencl") {
            let idx = *count;
            *count += 1;
            if node.geometry_visible {
                let mut visited = Vec::new();
                if let Some(geom) = resolve_opencl_geometry_with_errors(root, node, &mut visited, ocl_error) {
                    out.merge(geom);
                }
            }
        }
        for child in &node.children {
            visit(root, child, count, out, ocl_error);
        }
    }

    let mut out = Geometry::new();
    let mut count = 0;
    for child in &root.children {
        visit(root, child, &mut count, &mut out, ocl_error);
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
            || node.node_type.eq_ignore_ascii_case("opencl") {
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
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![add_node],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let geom = network_sphere_vertices(&root);
        assert_eq!(geom.vertices.len(), 5 * 2304);
    }

    #[test]
    fn test_transform_node() {
        let sphere = FsNode {
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
}


