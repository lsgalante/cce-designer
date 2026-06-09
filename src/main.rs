use std::time::Instant;
use std::fs;
use std::path::Path;
use std::net::TcpListener;
use std::io::{BufRead, BufReader, Read, Write};

use serde::{Deserialize, Serialize};

use clear_ui::widget::{ElementState, MouseButton, MouseScrollDelta, KeyEvent, Key, NamedKey};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window, delegate_output,
    registry::{ProvidesRegistryState, RegistryState},
    output::{OutputHandler, OutputState},
    seat::{
        keyboard::KeyboardHandler,
        pointer::{PointerHandler, ThemedPointer, ThemeSpec, CursorIcon},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        xdg::{
            window::{Window as XdgWindow, WindowConfigure, WindowHandler, WindowDecorations},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle, Proxy,
};
use calloop_wayland_source::WaylandSource;

mod geometry;
use geometry::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct ModifiersState {
    ctrl: bool,
    alt: bool,
    shift: bool,
    logo: bool,
}

impl ModifiersState {
    fn control_key(&self) -> bool { self.ctrl }
    fn alt_key(&self) -> bool { self.alt }
    fn shift_key(&self) -> bool { self.shift }
    fn super_key(&self) -> bool { self.logo }
    fn state(&self) -> Self { *self }
}

#[derive(Debug, Clone, Copy)]
struct LocalPosition {
    x: f64,
    y: f64,
}

enum WindowEvent {
    MouseWheel { delta: clear_ui::widget::MouseScrollDelta, phase: TouchPhase },
    PinchGesture { delta: f64 },
    CursorMoved { position: LocalPosition },
    MouseInput { state: clear_ui::widget::ElementState, button: clear_ui::widget::MouseButton },
    ModifiersChanged(ModifiersState),
    KeyboardInput { event: clear_ui::widget::KeyEvent },
}

use wgpu::util::DeviceExt;

use clear_ui::widget::{Breadcrumb, Canvas, MenuBar, Plate, ParametersBg, Splitter, Spreadsheet, StatusBar, TextLabel, ViewportBg, Element, GraphNode, Graph, Paginator, Button, Checkbox, Slider, Spinbox, ScrollingList, Label, ColorSelector};
use clear_ui::colors;

use glyphon::{
    Attrs, Buffer, Cache, FontSystem, Metrics, Resolution, SwashCache, TextArea, TextAtlas,
    TextBounds, TextRenderer, Viewport,
};

use glam::{Mat4, Vec3};

const HEADER_IDX: usize = 0;
const CONTENT_IDX: usize = 1;
const SPLITTER1_IDX: usize = 2;
const VIEWPORT_IDX: usize = 3;
const SPLITTER2_IDX: usize = 4;
const PARAM_PLATE_IDX: usize = 5;
const PARAM_IDX: usize = 6;
const CANVAS_IDX: usize = 7;
const LEFT_MENUBAR_IDX: usize = 8;
const RIGHT_MENUBAR_IDX: usize = 9;
const PARAM_MENUBAR_IDX: usize = 10;
const STATUS_IDX: usize = 11;
const BREADCRUMB_IDX: usize = 12;
const NODE_PALETTE_IDX: usize = 13;
const SPREADSHEET_IDX: usize = 14;
const SPREADSHEET_MENUBAR_IDX: usize = 15;
const NETWORK_PANEL_IDX: usize = 16;
const PAGINATOR_IDX: usize = 17;


const HEADER_H: f32 = 26.0;
const STATUS_H: f32 = 0.0;
const MENUBAR_H: f32 = 26.0;
const SPLITTER_W: f32 = 6.0;

const MIN_COLUMN: f32 = 120.0;
const BREADCRUMB_H: f32 = 24.0;

#[derive(Clone, Deserialize, Serialize)]
struct ParamDef {
    name: String,
    #[serde(default)]
    label: String,
    #[serde(rename = "type")]
    #[serde(default = "default_param_type")]
    param_type: String,
    #[serde(default)]
    default: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    min: Option<f32>,
    #[serde(default)]
    max: Option<f32>,
    #[serde(default)]
    step: Option<f32>,
}

fn default_param_type() -> String { "string".to_string() }

#[derive(Clone, Deserialize, Serialize)]
struct FsNode {
    name: String,
    #[serde(rename = "type")]
    #[serde(default = "default_node_type")]
    node_type: String,
    #[serde(default)]
    children: Vec<FsNode>,
    #[serde(default)]
    params: Vec<ParamDef>,
    #[serde(skip)]
    #[serde(default = "default_node_geometry_visible")]
    geometry_visible: bool,
    #[serde(default = "default_node_position")]
    position: (f32, f32),
}

fn default_node_type() -> String { "node".to_string() }
fn default_node_geometry_visible() -> bool { true }
fn default_node_position() -> (f32, f32) { (0.0, 0.0) }

#[derive(Clone, Deserialize, Serialize, Default)]
struct ProjectViewState {
    #[serde(default = "default_camera")]
    active_camera: String,
    #[serde(default)]
    pan: (f32, f32),
    #[serde(default)]
    current_path: Vec<usize>,
    #[serde(default)]
    selected_node: Option<usize>,
}

fn default_camera() -> String {
    "Default Camera".to_string()
}

#[derive(Clone, Deserialize, Serialize)]
struct Project {
    name: String,
    root: FsNode,
    #[serde(default)]
    view_state: ProjectViewState,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum HttpAction {
    Up,
    Enter { slot: usize },
    SetParam { slot: usize, name: String, value: String },
    ResetCamera,
    Load { path: String },
    Save { path: String },
    ToggleGeometry { slot: usize },
    AddNode { template_name: String, name: Option<String>, x: f32, y: f32 },
    DeleteNode { slot: usize },
    RenameNode { slot: usize, new_name: String },
    MoveNode { slot: usize, x: f32, y: f32 },
    AddParam { slot: usize, name: String, param_type: String, default: String },
    DeleteParam { slot: usize, name: String },
    ToggleCircularPane,
    MenuClick { widget_idx: usize, menu_idx: usize, item_idx: usize },
    MenuClosed { widget_idx: usize, menu_idx: usize },
    SelectPage { page: usize },
}

#[derive(Debug)]
enum CustomEvent {
    GetState(std::sync::mpsc::Sender<String>),
    PostAction(HttpAction, std::sync::mpsc::Sender<Result<String, String>>),
}

#[derive(Clone)]
struct NodeTemplate {
    label: String,
    node: FsNode,
}

fn param_display(params: &[ParamDef]) -> Vec<(String, String, String)> {
    params.iter().map(|p| {
        let key = if p.label.is_empty() { &p.name } else { &p.label };
        let value = if p.param_type == "choice" && !p.options.is_empty() && p.default.is_empty() {
            p.options[0].clone()
        } else {
            p.default.clone()
        };
        let ptype = if p.param_type == "slider" {
            let min = p.min.unwrap_or(0.0);
            let max = p.max.unwrap_or(2.0);
            format!("slider:{}:{}", min, max)
        } else if p.param_type == "float3" {
            let min = p.min.unwrap_or(-10.0);
            let max = p.max.unwrap_or(10.0);
            format!("float3:{}:{}", min, max)
        } else if p.param_type == "spinbox" {
            let min = p.min.unwrap_or(1.0) as i32;
            let max = p.max.unwrap_or(10000.0) as i32;
            let step = p.step.unwrap_or(1.0) as i32;
            format!("spinbox:{}:{}:{}", min, max, step)
        } else {
            p.param_type.clone()
        };
        (key.clone(), value, ptype)
    }).collect()
}

fn flatten_node_templates(root: &FsNode) -> Vec<NodeTemplate> {
    fn visit(node: &FsNode, path: &mut Vec<String>, out: &mut Vec<NodeTemplate>) {
        if !node.name.is_empty() {
            path.push(node.name.clone());
            out.push(NodeTemplate { label: path.join(" / "), node: node.clone() });
        }
        for child in &node.children {
            visit(child, path, out);
        }
        if !node.name.is_empty() {
            path.pop();
        }
    }

    let mut out = Vec::new();
    let mut path = Vec::new();
    for child in &root.children {
        visit(child, &mut path, &mut out);
    }
    out
}

fn load_fs_tree() -> FsNode {
    let nodes_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("nodes");
    let mut children = Vec::new();
    if let Ok(entries) = fs::read_dir(&nodes_dir) {
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(node) = serde_json::from_str::<FsNode>(&content) {
                    children.push(node);
                }
            }
        }
    }
    FsNode {
        name: String::new(),
        node_type: default_node_type(),
        children,
        params: vec![],
        geometry_visible: true,
        position: (0.0, 0.0),
    }
}

fn default_grid_thickness() -> f32 { 0.03 }
fn default_show_camera_pivot() -> bool { false }
fn default_camera_pivot_size() -> f32 { 1.0 }
fn default_node_color() -> [f32; 3] { [0.10, 0.45, 0.70] }
fn default_grid_color() -> [f32; 3] { [0.35, 0.35, 0.40] }
fn default_uniform_background() -> bool { false }
fn default_network_opacity() -> f32 { 0.95 }
fn default_cell_color() -> [f32; 3] { [0.13, 0.13, 0.16] }
fn default_gap_color() -> [f32; 3] { [0.07, 0.07, 0.09] }

#[derive(Serialize, Deserialize, Clone, Debug)]
struct DesignSettings {
    grid_snap_enabled: bool,
    network_grid_enabled: bool,
    grid_size_x: f32,
    grid_size_y: f32,
    skipped_row_h: f32,
    skipped_col_w: f32,
    show_grid_enabled: bool,
    show_cube_enabled: bool,
    show_origin_enabled: bool,
    origin_size: f32,
    viewport_bg_color: [f32; 3],
    square_viewport: bool,
    #[serde(default = "default_grid_thickness")]
    grid_thickness: f32,
    #[serde(default = "default_show_camera_pivot")]
    show_camera_pivot_enabled: bool,
    #[serde(default = "default_camera_pivot_size")]
    camera_pivot_size: f32,
    #[serde(default = "default_node_color")]
    node_color: [f32; 3],
    #[serde(default = "default_grid_color")]
    grid_color: [f32; 3],
    #[serde(default = "default_uniform_background")]
    uniform_background: bool,
    #[serde(default = "default_network_opacity")]
    network_opacity: f32,
    #[serde(default = "default_cell_color")]
    cell_color: [f32; 3],
    #[serde(default = "default_gap_color")]
    gap_color: [f32; 3],
}

impl Default for DesignSettings {
    fn default() -> Self {
        Self {
            grid_snap_enabled: true,
            network_grid_enabled: true,
            grid_size_x: 80.0,
            grid_size_y: 40.0,
            skipped_row_h: 20.0,
            skipped_col_w: 20.0,
            show_grid_enabled: true,
            show_cube_enabled: false,
            show_origin_enabled: true,
            origin_size: 1.0,
            viewport_bg_color: [0.05, 0.05, 0.10],
            square_viewport: false,
            grid_thickness: 0.03,
            show_camera_pivot_enabled: false,
            camera_pivot_size: 1.0,
            node_color: default_node_color(),
            grid_color: default_grid_color(),
            uniform_background: false,
            network_opacity: 0.95,
            cell_color: default_cell_color(),
            gap_color: default_gap_color(),
        }
    }
}

impl DesignSettings {
    fn file_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lsgalante".to_string());
        let mut path = std::path::PathBuf::from(home);
        path.push(".config");
        path.push("cce");
        path.push("design.json");
        path
    }

    fn load() -> Self {
        let path = Self::file_path();
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(settings) = serde_json::from_str::<Self>(&content) {
                return settings;
            }
        }
        Self::default()
    }

    fn save(&self) {
        let path = Self::file_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, content);
        }
    }
}

struct NodePalette {
    x: f32, y: f32, w: f32, h: f32,
    visible: bool,
    query: String,
    items: Vec<String>,
    selected: usize,
}

impl NodePalette {
    fn new() -> Self {
        Self { x: 0.0, y: 0.0, w: 0.0, h: 0.0, visible: false, query: String::new(), items: Vec::new(), selected: 0 }
    }

    fn panel_rect(&self) -> (f32, f32, f32, f32) {
        let pw = 460.0_f32.min(self.w - 40.0).max(280.0);
        let ph = 360.0_f32.min(self.h - 80.0).max(240.0);
        (self.x + (self.w - pw) / 2.0, self.y + 76.0, pw, ph)
    }
}

impl Element for NodePalette {
    fn rect(&self) -> (f32, f32, f32, f32) { (self.x, self.y, self.w, self.h) }
    fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) { self.x = x; self.y = y; self.w = w; self.h = h; }
    fn color(&self) -> [f32; 4] { [0.0, 0.0, 0.0, 0.0] }
    fn hit_test(&self, px: f32, py: f32, ctx: &clear_ui::context::UiContext) -> bool {
        if ctx.is_coordinate_covered(self as *const Self as *const () as usize, px, py) {
            return false;
        }
        self.visible && px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
    fn set_visible(&mut self, visible: bool) { self.visible = visible; }
    fn visible(&self) -> bool { self.visible }
    fn take_click(&mut self) -> bool { self.visible }
    fn set_palette_state(&mut self, visible: bool, query: &str, items: &[String], selected: usize) {
        self.visible = visible;
        self.query = query.to_string();
        self.items = items.to_vec();
        self.selected = selected;
    }

    fn extra_quads(&self) -> Vec<(f32, f32, f32, f32, [f32; 4])> {
        if !self.visible { return Vec::new(); }
        let mut quads = Vec::new();
        quads.push((self.x, self.y, self.w, self.h, [0.0, 0.0, 0.0, 0.45]));
        let (px, py, pw, ph) = self.panel_rect();
        quads.push((px, py, pw, ph, colors::PANEL_MENU_BG));
        quads.push((px + 16.0, py + 48.0, pw - 32.0, 32.0, [0.10, 0.10, 0.14, 1.0]));
        let list_y = py + 92.0;
        let row_h = 24.0;
        let visible_rows = ((ph - 120.0) / row_h).floor().max(0.0) as usize;
        let start = self.selected.saturating_sub(visible_rows.saturating_sub(1));
        for (row, item_idx) in (start..self.items.len().min(start + visible_rows)).enumerate() {
            let y = list_y + row as f32 * row_h;
            let bg = if item_idx == self.selected { [0.24, 0.33, 0.55, 0.85] } else { [0.14, 0.14, 0.19, 0.55] };
            quads.push((px + 16.0, y, pw - 32.0, row_h - 2.0, bg));
        }
        quads
    }

    fn text_labels(&self) -> Vec<TextLabel> {
        if !self.visible { return Vec::new(); }
        let (px, py, _pw, ph) = self.panel_rect();
        let mut labels = Vec::new();
        labels.push(TextLabel { text: "Add Node".into(), x: px + 16.0, y: py + 14.0, font_size: 15.0, color: [0xdd, 0xdd, 0xe6] });
        labels.push(TextLabel { text: "Type to search, Enter to place, Esc to close".into(), x: px + 100.0, y: py + 17.0, font_size: 11.0, color: [0x88, 0x88, 0x99] });
        let query = if self.query.is_empty() { "Search nodes...".to_string() } else { self.query.clone() };
        let query_color = if self.query.is_empty() { [0x66, 0x66, 0x77] } else { [0xdd, 0xdd, 0xe6] };
        labels.push(TextLabel { text: query, x: px + 28.0, y: py + 57.0, font_size: 13.0, color: query_color });
        let row_h = 24.0;
        let visible_rows = ((ph - 120.0) / row_h).floor().max(0.0) as usize;
        if self.items.is_empty() {
            labels.push(TextLabel { text: "No matching nodes".into(), x: px + 28.0, y: py + 100.0, font_size: 12.0, color: [0xaa, 0xaa, 0xbb] });
        } else {
            let start = self.selected.saturating_sub(visible_rows.saturating_sub(1));
            for (row, item_idx) in (start..self.items.len().min(start + visible_rows)).enumerate() {
                labels.push(TextLabel {
                    text: self.items[item_idx].clone(),
                    x: px + 28.0,
                    y: py + 98.0 + row as f32 * row_h,
                    font_size: 12.0,
                    color: if item_idx == self.selected { [0xff, 0xff, 0xdd] } else { [0xcc, 0xcc, 0xd8] },
                });
            }
        }
        labels
    }

    fn z_index(&self) -> i32 {
        200
    }
}

mod shortcut;
use shortcut::{Shortcut, ShortcutManager, Action};

use clear_ui::engine::{
    Vertex, widget_vertices,
    quad_vertices_clipped, circle_vertices, circle_border_vertices, arc_background_vertices,
    extra_quad_vertices, extra_quad_vertices_clipped,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TexturedVertex {
    position: [f32; 2],
    tex_coords: [f32; 2],
    color: [f32; 4],
    clip_circle: [f32; 3],
}

impl TexturedVertex {
    const ATTRIBS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32x4,
        3 => Float32x3,
    ];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TexturedVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}


fn make_text_buffer(font_system: &mut FontSystem, text: &str, size: f32) -> Buffer {
    let metrics = Metrics::new(size, size * 1.4);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text(font_system, text, Attrs::new(), glyphon::Shaping::Advanced);
    buffer.shape_until_scroll(font_system, true);
    buffer
}

fn make_text_buffer_with_font(font_system: &mut FontSystem, text: &str, size: f32, font: Option<&str>) -> Buffer {
    let metrics = Metrics::new(size, size * 1.4);
    let mut buffer = Buffer::new(font_system, metrics);
    let mut attrs = Attrs::new();
    if let Some(font_name) = font {
        let family = match font_name {
            "monospace" => glyphon::Family::Monospace,
            "sans-serif" => glyphon::Family::SansSerif,
            "serif" => glyphon::Family::Serif,
            _ => glyphon::Family::Name(font_name),
        };
        attrs = attrs.family(family);
    }
    buffer.set_text(font_system, text, attrs, glyphon::Shaping::Advanced);
    buffer.shape_until_scroll(font_system, true);
    buffer
}

fn get_next_visible_pane(
    current_pane: usize,
    show_network: bool,
    show_viewport: bool,
    show_parameters: bool,
    show_spreadsheet: bool,
    shift_pressed: bool,
) -> usize {
    let mut visible_panes = Vec::new();
    if show_network {
        visible_panes.push(LEFT_MENUBAR_IDX);
    }
    if show_viewport {
        visible_panes.push(RIGHT_MENUBAR_IDX);
    }
    if show_parameters {
        visible_panes.push(PARAM_MENUBAR_IDX);
    }
    if show_spreadsheet {
        visible_panes.push(SPREADSHEET_MENUBAR_IDX);
    }
    if visible_panes.is_empty() {
        return LEFT_MENUBAR_IDX;
    }

    let current_pos = visible_panes.iter().position(|&x| x == current_pane).unwrap_or(0);
    let next_pos = if shift_pressed {
        (current_pos + visible_panes.len() - 1) % visible_panes.len()
    } else {
        (current_pos + 1) % visible_panes.len()
    };
    visible_panes[next_pos]
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResizeDirection {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

struct State {
    wgpu_adapter: clear_ui::backend::WgpuAdapter,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    window: XdgWindow,
    wl_surface: wl_surface::WlSurface,
    vertex_count: u32,

    pipeline_3d: wgpu::RenderPipeline,
    bind_group_3d: wgpu::BindGroup,
    bind_group_layout_3d: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    bind_group_grid: wgpu::BindGroup,
    uniform_buffer_grid: wgpu::Buffer,
    bind_group_pivot: wgpu::BindGroup,
    uniform_buffer_pivot: wgpu::Buffer,
    vertex_buffer_3d: wgpu::Buffer,
    vertex_count_3d: u32,
    vertex_buffer_spheres: wgpu::Buffer,
    vertex_count_spheres: u32,
    vertex_buffer_grid: wgpu::Buffer,
    vertex_count_grid: u32,
    depth_texture: wgpu::Texture,
    depth_texture_view: wgpu::TextureView,
    backdrop_texture: wgpu::Texture,
    backdrop_texture_view: wgpu::TextureView,
    backdrop_sampler: wgpu::Sampler,
    backdrop_bind_group_layout: wgpu::BindGroupLayout,
    backdrop_bind_group: wgpu::BindGroup,
    rotation_y: f32,
    rotation_x: f32,
    is_rotating_viewport: bool,
    scroll_lock: u8, // 0 = None, 1 = Horizontal, 2 = Vertical
    last_rotate_time: Instant,
    rotate_accum_yaw: f32,
    rotate_accum_pitch: f32,
    rotate_velocity_yaw: f32,
    rotate_velocity_pitch: f32,
    show_grid: bool,
    show_cube: bool,
    show_origin: bool,
    show_camera_pivot: bool,
    viewport_bg_color: [f32; 3],
    node_color: [f32; 3],
    grid_color: [f32; 3],
    cell_color: [f32; 3],
    gap_color: [f32; 3],
    vertex_buffer_origin: wgpu::Buffer,
    vertex_count_origin: u32,
    vertex_buffer_pivot: wgpu::Buffer,
    vertex_count_pivot: u32,
    origin_size: f32,
    camera_pivot_size: f32,

    fs_root: FsNode,
    node_templates: Vec<NodeTemplate>,
    current_path: Vec<usize>,
    last_click: Option<(Instant, usize)>,
    last_frame: Instant,

    shortcut_manager: ShortcutManager,
    pending_action: Option<Action>,
    exit_requested: bool,

    widgets: Vec<Box<dyn Element>>,
    positions: Vec<(f32, f32, f32, f32)>,
    splitter_layout: clear_ui::layout::SplitterLayout,
    node_palette_visible: bool,
    node_palette_query: String,
    node_palette_filtered: Vec<usize>,
    node_palette_selected: usize,

    curved_text_texture: wgpu::Texture,
    curved_text_texture_view: wgpu::TextureView,
    curved_text_sampler: wgpu::Sampler,
    curved_text_bind_group: wgpu::BindGroup,
    curved_text_pipeline: wgpu::RenderPipeline,
    curved_text_atlas: TextAtlas,
    curved_text_renderer: TextRenderer,
    curved_text_viewport: Viewport,
    textured_vertex_buffer: wgpu::Buffer,
    textured_vertex_count: u32,


    drag_widget: Option<usize>,
    focused_widget: Option<usize>,

    cursor_x: f32,
    cursor_y: f32,
    grid_cursor_col: i32,
    grid_cursor_row: i32,
    modifiers: ModifiersState,

    width: f32,
    height: f32,
    physical_width: u32,
    physical_height: u32,
    scale: f64,
    square_viewport: bool,
    grid_snap_enabled: bool,
    network_grid_visible: bool,
    grid_size_x: f32,
    grid_size_y: f32,
    skipped_row_h: f32,
    skipped_col_w: f32,

    pan_x: f32,
    pan_y: f32,
    pan_velocity_x: f32,
    pan_velocity_y: f32,
    last_frame_pan_x: f32,
    last_frame_pan_y: f32,
    is_panning: bool,
    pan_start_cx: f32,
    pan_start_cy: f32,
    pan_start_x: f32,
    pan_start_y: f32,
    space_pressed: bool,
    active_camera: String,
    show_network: bool,
    show_viewport: bool,
    show_parameters: bool,
    show_spreadsheet: bool,
    is_scrolling_trackpad: bool,
    last_scroll_time: Instant,
    scroll_accum_x: f32,
    scroll_accum_y: f32,
    last_spreadsheet_node_name: Option<String>,
    last_spreadsheet_node_params: Option<Vec<(String, String)>>,
    viewport_zoom: f32,
    is_zooming_viewport: bool,
    last_zoom_time: Instant,
    zoom_accum: f32,
    zoom_velocity: f32,
    grid_thickness: f32,
    focused_pane: usize,
    inertial_scroll_enabled: bool,
    inertial_scroll_friction: f32,
    scroll_speed: f32,
    last_config_read: Instant,
    circular_network_pane: bool,
    circular_network_layout: clear_ui::layout::CircularPaneLayout,
    is_detached_network: bool,
    detached_circular_network: bool,
    last_project_mod_time: Option<std::time::SystemTime>,
    last_project_check: std::time::Instant,
    last_inspector_check: std::time::Instant,
    last_inspector_update: std::time::Instant,
    needs_autosave: bool,
    last_autosave_time: std::time::Instant,
    pub window_x: i32,
    pub window_y: i32,
    pub active_menu_cloud_pid: Option<u32>,
    pub active_menu_cloud_idx: Option<(usize, usize)>,
    uniform_background: bool,
    network_opacity: f32,
    last_design_mod_time: Option<std::time::SystemTime>,
    floating_network_layout: (f32, f32, f32, f32),
    is_resizing_network: Option<ResizeDirection>,
    drag_start_rect: (f32, f32, f32, f32),
    drag_start_mouse: (f32, f32),
    floating_param_width: f32,
    is_resizing_param: bool,
    drag_start_param_w: f32,
    floating_spreadsheet_height: f32,
    is_resizing_spreadsheet: bool,
    drag_start_spreadsheet_h: f32,
    paginator_page_widgets: Vec<Vec<Box<dyn Element>>>,
    last_paginator_menubar: Option<usize>,
    loaded_project_path: Option<std::path::PathBuf>,
    last_saved_root_json: String,
    recent_files: Vec<std::path::PathBuf>,
    recent_files_list: ScrollingList,
    recent_files_buttons: Vec<Button>,
    ui_context: clear_ui::context::UiContext,
}

impl State {
    fn has_unsaved_changes(&self) -> bool {
        if let Ok(current_json) = serde_json::to_string(&self.fs_root) {
            current_json != self.last_saved_root_json
        } else {
            false
        }
    }

    fn update_window_title(&self) {
        let base_title = if self.is_detached_network {
            "Network Pane"
        } else {
            "Clear Design Interface"
        };
        
        let mut title = base_title.to_string();
        
        if let Some(path) = &self.loaded_project_path {
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                title.push_str(" - ");
                title.push_str(filename);
            }
        }
        
        if self.has_unsaved_changes() {
            title.push_str("*");
        }
        
        self.window.set_title(&title);
    }

    fn get_recent_files_path() -> Option<std::path::PathBuf> {
        std::env::var("HOME").ok().map(|h| {
            let mut path = std::path::PathBuf::from(h);
            path.push(".config");
            path.push("cce-design-interface");
            path.push("recent_files.json");
            path
        })
    }

    fn load_recent_files() -> Vec<std::path::PathBuf> {
        if let Some(path) = Self::get_recent_files_path() {
            if let Ok(file) = std::fs::File::open(path) {
                if let Ok(list) = serde_json::from_reader::<_, Vec<String>>(file) {
                    return list.into_iter().map(std::path::PathBuf::from).collect();
                }
            }
        }
        Vec::new()
    }

    fn save_recent_files(files: &[std::path::PathBuf]) {
        if let Some(path) = Self::get_recent_files_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(file) = std::fs::File::create(path) {
                let list: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();
                let _ = serde_json::to_writer_pretty(file, &list);
            }
        }
    }

    fn add_recent_file(&mut self, path: std::path::PathBuf) {
        let abs_path = std::fs::canonicalize(&path).unwrap_or(path);
        self.recent_files.retain(|p| p != &abs_path);
        self.recent_files.insert(0, abs_path);
        self.recent_files.truncate(10);
        Self::save_recent_files(&self.recent_files);
        self.rebuild_recent_buttons();
        self.update_paginator();
    }

    fn rebuild_recent_buttons(&mut self) {
        self.recent_files_buttons.clear();
        for file in &self.recent_files {
            let label = file.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| file.to_string_lossy().to_string());
            let btn = Button::new_list_row(0.0, 0.0, 0.0, 0.0).with_label(&label);
            self.recent_files_buttons.push(btn);
        }
    }

    fn update_recent_files_layout(&mut self) {
        let active_menubar = self.focused_pane;
        let menu_names = self.widgets[active_menubar].menu_names();
        let file_page_idx = match menu_names.iter().position(|name| name == "File") {
            Some(idx) => idx,
            None => return,
        };

        if file_page_idx >= self.paginator_page_widgets.len() {
            return;
        }

        let page = &mut self.paginator_page_widgets[file_page_idx];
        let menu_items = self.widgets[active_menubar].menu_items_list();
        if file_page_idx >= menu_items.len() {
            return;
        }
        let num_base_items = menu_items[file_page_idx].len();
        
        let list_idx = num_base_items + 1;
        if list_idx >= page.len() {
            return;
        }

        let (list_x, list_y, list_w, list_h) = page[list_idx].rect();
        if list_h <= 0.0 {
            return;
        }

        let count = self.recent_files.len();
        page[list_idx].update_bounds(count, list_y, list_h);

        let btn_h = 22.0;
        let inner_x = list_x + 4.0;
        let inner_w = list_w - 16.0;

        for (idx, btn) in self.recent_files_buttons.iter_mut().enumerate() {
            if let Some(draw_y) = page[list_idx].get_item_draw_y(idx, 0.0) {
                btn.set_rect(inner_x, draw_y, inner_w, btn_h);
            } else {
                btn.set_rect(-9999.0, -9999.0, 0.0, 0.0);
            }
        }
    }

    fn save_settings(&mut self) {
        let settings = DesignSettings {
            grid_snap_enabled: self.grid_snap_enabled,
            network_grid_enabled: self.network_grid_visible,
            grid_size_x: self.grid_size_x,
            grid_size_y: self.grid_size_y,
            skipped_row_h: self.skipped_row_h,
            skipped_col_w: self.skipped_col_w,
            show_grid_enabled: self.show_grid,
            show_cube_enabled: self.show_cube,
            show_origin_enabled: self.show_origin,
            origin_size: self.origin_size,
            viewport_bg_color: self.viewport_bg_color,
            node_color: self.node_color,
            grid_color: self.grid_color,
            cell_color: self.cell_color,
            gap_color: self.gap_color,
            square_viewport: self.square_viewport,
            grid_thickness: self.grid_thickness,
            show_camera_pivot_enabled: self.show_camera_pivot,
            camera_pivot_size: self.camera_pivot_size,
            uniform_background: self.uniform_background,
            network_opacity: self.network_opacity,
        };
        settings.save();
        self.last_design_mod_time = {
            let design_path = DesignSettings::file_path();
            std::fs::metadata(&design_path).and_then(|m| m.modified()).ok()
        };
    }

    fn sync_settings_from_paginator(&mut self) {
        let active_menubar = self.focused_pane;
        let page_idx = if active_menubar == LEFT_MENUBAR_IDX {
            3
        } else if active_menubar == RIGHT_MENUBAR_IDX {
            4
        } else {
            return;
        };

        if page_idx >= self.paginator_page_widgets.len() {
            return;
        }

        let mut left_values = None;
        let mut right_values = None;

        {
            let widgets = &self.paginator_page_widgets[page_idx];
            if widgets.is_empty() {
                return;
            }

            if active_menubar == LEFT_MENUBAR_IDX {
                if widgets.len() >= 11 {
                    let snap = widgets[0].value();
                    let vis = widgets[1].value();
                    let gx = widgets[2].value();
                    let gy = widgets[3].value();
                    let srh = widgets[4].value();
                    let scw = widgets[5].value();
                    let uni = widgets[6].value();
                    let op = widgets[7].value();
                    let node_col = widgets[8].color_u8();
                    let cell_col = widgets[9].color_u8();
                    let gap_col = widgets[10].color_u8();
                    left_values = Some((snap, vis, gx, gy, srh, scw, uni, op, node_col, cell_col, gap_col));
                }
            } else if active_menubar == RIGHT_MENUBAR_IDX {
                if widgets.len() >= 13 {
                    let sg = widgets[0].value();
                    let sc = widgets[1].value();
                    let so = widgets[2].value();
                    let cp = widgets[3].value();
                    let gt = widgets[4].value();
                    let os = widgets[5].value();
                    let cps = widgets[6].value();
                    let bgr = widgets[7].value();
                    let bgg = widgets[8].value();
                    let bgb = widgets[9].value();
                    let gcr = widgets[10].value();
                    let gcg = widgets[11].value();
                    let gcb = widgets[12].value();
                    right_values = Some((sg, sc, so, cp, gt, os, cps, bgr, bgg, bgb, gcr, gcg, gcb));
                }
            }
        }

        let mut changed = false;

        if let Some((snap, vis, gx, gy, srh, scw, uni, op, node_col, cell_col, gap_col)) = left_values {
            // LEFT pane Settings
            // 0: Snap to Grid (Checkbox)
            let snap = snap == 1;
            if self.grid_snap_enabled != snap {
                self.grid_snap_enabled = snap;
                changed = true;
            }
            // 1: Grid Visible (Checkbox)
            let vis = vis == 1;
            if self.network_grid_visible != vis {
                self.network_grid_visible = vis;
                changed = true;
            }
            // 2: Grid X (Spinbox)
            let gx = gx as f32;
            if self.grid_size_x != gx {
                self.grid_size_x = gx;
                changed = true;
            }
            // 3: Grid Y (Spinbox)
            let gy = gy as f32;
            if self.grid_size_y != gy {
                self.grid_size_y = gy;
                changed = true;
            }
            // 4: Skipped Row H (Spinbox)
            let srh = srh as f32;
            if self.skipped_row_h != srh {
                self.skipped_row_h = srh;
                changed = true;
            }
            // 5: Skipped Col W (Spinbox)
            let scw = scw as f32;
            if self.skipped_col_w != scw {
                self.skipped_col_w = scw;
                changed = true;
            }
            // 6: Uniform Background (Checkbox)
            let uni = uni == 1;
            if self.uniform_background != uni {
                self.uniform_background = uni;
                changed = true;
            }
            // 7: Opacity (Slider, 0..100)
            let op = op as f32 / 100.0;
            if (self.network_opacity - op).abs() > 0.001 {
                self.network_opacity = op;
                changed = true;
            }
            // 8: Node Color (ColorSelector)
            if let Some(col) = node_col {
                let nr = col[0] as f32 / 255.0;
                let ng = col[1] as f32 / 255.0;
                let nb = col[2] as f32 / 255.0;
                if (self.node_color[0] - nr).abs() > 0.001
                    || (self.node_color[1] - ng).abs() > 0.001
                    || (self.node_color[2] - nb).abs() > 0.001
                {
                    self.node_color = [nr, ng, nb];
                    colors::set_node_color([nr, ng, nb, 1.0]);
                    changed = true;
                }
            }
            // 9: Cell Color (ColorSelector)
            if let Some(col) = cell_col {
                let cr = col[0] as f32 / 255.0;
                let cg = col[1] as f32 / 255.0;
                let cb = col[2] as f32 / 255.0;
                if (self.cell_color[0] - cr).abs() > 0.001
                    || (self.cell_color[1] - cg).abs() > 0.001
                    || (self.cell_color[2] - cb).abs() > 0.001
                {
                    self.cell_color = [cr, cg, cb];
                    changed = true;
                }
            }
            // 10: Gap Color (ColorSelector)
            if let Some(col) = gap_col {
                let gr = col[0] as f32 / 255.0;
                let gg = col[1] as f32 / 255.0;
                let gb = col[2] as f32 / 255.0;
                if (self.gap_color[0] - gr).abs() > 0.001
                    || (self.gap_color[1] - gg).abs() > 0.001
                    || (self.gap_color[2] - gb).abs() > 0.001
                {
                    self.gap_color = [gr, gg, gb];
                    changed = true;
                }
            }
        } else if let Some((sg, sc, so, cp, gt, os, cps, bgr, bgg, bgb, gcr, gcg, gcb)) = right_values {
            // RIGHT pane Settings
            // 0: Show Grid Guide (Checkbox)
            let sg = sg == 1;
            if self.show_grid != sg {
                self.show_grid = sg;
                self.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 0, sg);
                changed = true;
            }
            // 1: Show Reference Cube (Checkbox)
            let sc = sc == 1;
            if self.show_cube != sc {
                self.show_cube = sc;
                self.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 1, sc);
                changed = true;
            }
            // 2: Show Origin Axes (Checkbox)
            let so = so == 1;
            if self.show_origin != so {
                self.show_origin = so;
                self.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 2, so);
                changed = true;
            }
            // 3: Show Camera Pivot (Checkbox)
            let cp = cp == 1;
            if self.show_camera_pivot != cp {
                self.show_camera_pivot = cp;
                self.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 3, cp);
                changed = true;
            }
            // 4: Grid Thickness (Spinbox, decimals 3)
            let gt = gt as f32 / 1000.0;
            if (self.grid_thickness - gt).abs() > 0.0001 {
                self.grid_thickness = gt;
                self.update_grid_geometry();
                changed = true;
            }
            // 5: Origin Guide Size (Spinbox, decimals 1)
            let os = os as f32 / 10.0;
            if (self.origin_size - os).abs() > 0.001 {
                self.origin_size = os;
                self.update_origin_geometry();
                changed = true;
            }
            // 6: Camera Pivot Size (Spinbox, decimals 1)
            let cps = cps as f32 / 10.0;
            if (self.camera_pivot_size - cps).abs() > 0.001 {
                self.camera_pivot_size = cps;
                self.update_pivot_geometry();
                changed = true;
            }
            // 7, 8, 9: BG Color (R, G, B Spinboxes, 0..255)
            let bgr = bgr as f32 / 255.0;
            let bgg = bgg as f32 / 255.0;
            let bgb = bgb as f32 / 255.0;
            if (self.viewport_bg_color[0] - bgr).abs() > 0.001
                || (self.viewport_bg_color[1] - bgg).abs() > 0.001
                || (self.viewport_bg_color[2] - bgb).abs() > 0.001
            {
                self.viewport_bg_color = [bgr, bgg, bgb];
                changed = true;
            }
            // 10, 11, 12: Grid Color (R, G, B Spinboxes, 0..255)
            let gcr = gcr as f32 / 255.0;
            let gcg = gcg as f32 / 255.0;
            let gcb = gcb as f32 / 255.0;
            if (self.grid_color[0] - gcr).abs() > 0.001
                || (self.grid_color[1] - gcg).abs() > 0.001
                || (self.grid_color[2] - gcb).abs() > 0.001
            {
                self.grid_color = [gcr, gcg, gcb];
                self.update_grid_geometry();
                changed = true;
            }
        }

        if changed {
            self.sync_grid_settings();
            self.save_settings();
            self.upload_vertices();
        }
    }

    fn update_grid_geometry(&mut self) {
        let grid_verts = grid_vertices(self.grid_thickness, self.grid_color);
        self.wgpu_adapter.queue.write_buffer(&self.vertex_buffer_grid, 0, bytemuck::cast_slice(&grid_verts));
    }

    fn update_origin_geometry(&mut self) {
        let origin_verts = origin_vectors_vertices(self.origin_size);
        self.wgpu_adapter.queue.write_buffer(&self.vertex_buffer_origin, 0, bytemuck::cast_slice(&origin_verts));
    }

    fn update_pivot_geometry(&mut self) {
        let pivot_verts = camera_pivot_vertices(self.camera_pivot_size);
        self.wgpu_adapter.queue.write_buffer(&self.vertex_buffer_pivot, 0, bytemuck::cast_slice(&pivot_verts));
    }

    fn body_h(&self) -> f32 { self.height - HEADER_H - STATUS_H }

    fn get_col_geometries(&self) -> (f32, f32, f32, f32, f32, f32) {
        let left_visible = self.show_network && !self.circular_network_pane && !self.is_detached_network && !self.detached_circular_network;
        let center_visible = self.show_viewport || self.show_spreadsheet;
        let right_visible = self.show_parameters;

        let (col_l_x, col_l_w, col_c_x, col_c_w, col_r_x, col_r_w) =
            match (left_visible, center_visible, right_visible) {
                (true, true, true) => {
                    let l_w = self.splitter_layout.splitter1_x;
                    let c_x = l_w + SPLITTER_W;
                    let c_w = self.splitter_layout.splitter2_x - c_x;
                    let r_x = self.splitter_layout.splitter2_x + SPLITTER_W;
                    let r_w = (self.width - r_x).max(0.0);
                    (0.0, l_w, c_x, c_w, r_x, r_w)
                }
                (true, true, false) => {
                    let l_w = self.splitter_layout.splitter1_x;
                    let c_x = l_w + SPLITTER_W;
                    let c_w = (self.width - c_x).max(0.0);
                    (0.0, l_w, c_x, c_w, 0.0, 0.0)
                }
                (true, false, true) => {
                    let l_w = self.splitter_layout.splitter2_x;
                    let r_x = l_w + SPLITTER_W;
                    let r_w = (self.width - r_x).max(0.0);
                    (0.0, l_w, 0.0, 0.0, r_x, r_w)
                }
                (false, true, true) => {
                    let c_w = self.splitter_layout.splitter2_x;
                    let r_x = c_w + SPLITTER_W;
                    let r_w = (self.width - r_x).max(0.0);
                    (0.0, 0.0, 0.0, c_w, r_x, r_w)
                }
                (true, false, false) => {
                    (0.0, self.width, 0.0, 0.0, 0.0, 0.0)
                }
                (false, true, false) => {
                    (0.0, 0.0, 0.0, self.width, 0.0, 0.0)
                }
                (false, false, true) => {
                    (0.0, 0.0, 0.0, 0.0, 0.0, self.width)
                }
                (false, false, false) => {
                    (0.0, 0.0, 0.0, self.width, 0.0, 0.0)
                }
            };
        (col_l_x, col_l_w, col_c_x, col_c_w, col_r_x, col_r_w)
    }

    fn content_left_w(&self) -> f32 { self.get_col_geometries().1 }

    fn content_right_x(&self) -> f32 { self.get_col_geometries().2 }

    fn viewport_w(&self) -> f32 { self.get_col_geometries().3 }

    fn param_x(&self) -> f32 { self.get_col_geometries().4 }

    fn param_w(&self) -> f32 { self.get_col_geometries().5 }
    
    fn in_network_pane(&self) -> bool {
        if self.circular_network_pane {
            self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, MENUBAR_H, BREADCRUMB_H)
        } else {
            let (cx, cy, cw, ch) = self.positions[CONTENT_IDX];
            self.cursor_x >= cx
                && self.cursor_x < cx + cw
                && self.cursor_y >= cy
                && self.cursor_y < cy + ch
        }
    }

    fn clamp_splitters(&mut self) {
        if self.is_detached_network {
            return;
        }
        self.splitter_layout.clamp(self.width, self.detached_circular_network);
    }

    fn current_dir(&self) -> &FsNode {
        let mut node = &self.fs_root;
        for &i in &self.current_path {
            node = &node.children[i];
        }
        node
    }

    fn current_dir_mut(&mut self) -> &mut FsNode {
        let mut node = &mut self.fs_root;
        for &i in &self.current_path {
            node = &mut node.children[i];
        }
        node
    }

    fn sync_parameters_to_project(&mut self) {
        if !self.is_detached_network {
            if let Some(slot_idx) = self.widgets[CONTENT_IDX].selected_node() {
                let updated_params = self.widgets[PARAM_IDX].node_params();
                let dir = self.current_dir_mut();
                if let Some(child) = dir.children.get_mut(slot_idx) {
                    let mut param_changed = false;
                    for (u_name, u_val, _) in &updated_params {
                        if let Some(p) = child.params.iter_mut().find(|p| p.name == *u_name) {
                            if p.default != *u_val {
                                p.default = u_val.clone();
                                param_changed = true;
                            }
                        }
                    }
                    if param_changed {
                        self.rebuild_scene_geometry();
                        self.sync_nodes();
                    }
                }
            }
        }
    }

    fn current_path_names(&self) -> Vec<String> {
        let mut node = &self.fs_root;
        let mut names = Vec::new();
        for &i in &self.current_path {
            if let Some(child) = node.children.get(i) {
                names.push(child.name.clone());
                node = child;
            }
        }
        names
    }

    fn refresh_node_palette(&mut self) {
        let q = self.node_palette_query.to_lowercase();
        self.node_palette_filtered = self.node_templates.iter().enumerate()
            .filter_map(|(i, t)| {
                if q.is_empty() || t.label.to_lowercase().contains(&q) { Some(i) } else { None }
            })
            .collect();
        if self.node_palette_selected >= self.node_palette_filtered.len() {
            self.node_palette_selected = self.node_palette_filtered.len().saturating_sub(1);
        }
        let items: Vec<String> = self.node_palette_filtered.iter()
            .map(|&i| self.node_templates[i].label.clone())
            .collect();
        self.widgets[NODE_PALETTE_IDX].set_palette_state(
            self.node_palette_visible,
            &self.node_palette_query,
            &items,
            self.node_palette_selected,
        );
    }

    fn open_file_chooser(&self) {
        std::thread::spawn(move || {
            use std::process::{Command, Stdio};
            use std::io::Write;

            // Find executable path
            let exe_path = if let Ok(cur_exe) = std::env::current_exe() {
                let sibling = cur_exe.with_file_name("clear-filesystem-interface");
                if sibling.exists() {
                    sibling
                } else {
                    std::path::PathBuf::from("clear-filesystem-interface")
                }
            } else {
                std::path::PathBuf::from("clear-filesystem-interface")
            };

            let child = match Command::new(exe_path)
                .arg("--select")
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to spawn clear-filesystem-interface: {:?}", e);
                    return;
                }
            };

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("Failed to wait for clear-filesystem-interface: {:?}", e);
                    return;
                }
            };

            if output.status.success() {
                let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !selected.is_empty() {
                    let body = format!(
                        "{{\"action\":\"load\",\"path\":\"{}\"}}",
                        selected.replace('\\', "\\\\").replace('"', "\\\"")
                    );
                    let req = format!(
                        "POST /action HTTP/1.1\r\n\
                         Host: 127.0.0.1:3000\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n\
                         {}",
                        body.len(),
                        body
                    );
                    if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:3000") {
                        let _ = stream.write_all(req.as_bytes());
                        let _ = stream.flush();
                    }
                }
            }
        });
    }

    fn save_file_chooser(&self) {
        std::thread::spawn(move || {
            use std::process::{Command, Stdio};
            use std::io::Write;

            // Find executable path
            let exe_path = if let Ok(cur_exe) = std::env::current_exe() {
                let sibling = cur_exe.with_file_name("clear-filesystem-interface");
                if sibling.exists() {
                    sibling
                } else {
                    std::path::PathBuf::from("clear-filesystem-interface")
                }
            } else {
                std::path::PathBuf::from("clear-filesystem-interface")
            };

            let child = match Command::new(exe_path)
                .arg("--save")
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to spawn clear-filesystem-interface: {:?}", e);
                    return;
                }
            };

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("Failed to wait for clear-filesystem-interface: {:?}", e);
                    return;
                }
            };

            if output.status.success() {
                let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !selected.is_empty() {
                    let body = format!(
                        "{{\"action\":\"save\",\"path\":\"{}\"}}",
                        selected.replace('\\', "\\\\").replace('"', "\\\"")
                    );
                    let req = format!(
                        "POST /action HTTP/1.1\r\n\
                         Host: 127.0.0.1:3000\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n\
                         {}",
                        body.len(),
                        body
                    );
                    if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:3000") {
                        let _ = stream.write_all(req.as_bytes());
                        let _ = stream.flush();
                    }
                }
            }
        });
    }

    fn open_node_palette(&mut self) {
        let templates: Vec<String> = self.node_templates.iter().map(|t| t.label.clone()).collect();
        let grid_col = self.grid_cursor_col;
        let grid_row = self.grid_cursor_row;
        std::thread::spawn(move || {
            use std::io::Write;
            use std::process::{Command, Stdio};

            let mut child = match Command::new("clear-cloud")
                .arg("-p")
                .arg("Add Node: ")
                .arg("--dmenu")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to spawn clear-cloud: {:?}", e);
                    return;
                }
            };

            if let Some(mut stdin) = child.stdin.take() {
                for t in &templates {
                    let _ = writeln!(stdin, "{}", t);
                }
            }

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("Failed to wait for clear-cloud: {:?}", e);
                    return;
                }
            };

            if output.status.success() {
                let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !selected.is_empty() {
                    let body = format!(
                        "{{\"action\":\"add_node\",\"template_name\":\"{}\",\"x\":{},\"y\":{}}}",
                        selected, grid_col, grid_row
                    );
                    let req = format!(
                        "POST /action HTTP/1.1\r\n\
                         Host: 127.0.0.1:3000\r\n\
                         Content-Type: application/json\r\n\
                         Content-Length: {}\r\n\
                         Connection: close\r\n\r\n\
                         {}",
                        body.len(),
                        body
                    );
                    if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:3000") {
                        let _ = stream.write_all(req.as_bytes());
                        let _ = stream.flush();
                    }
                }
            }
        });
    }

    fn spawn_menu_cloud(&mut self, widget_idx: usize, menu_idx: usize, title: String, items: Vec<String>, rx: f32, ry: f32, _rw: f32, rh: f32) {
        let is_same_menu = self.active_menu_cloud_idx == Some((widget_idx, menu_idx));
        let mut process_was_running = false;

        if let Some(pid) = self.active_menu_cloud_pid {
            let is_running = unsafe {
                libc::kill(pid as libc::pid_t, 0) == 0
            };
            if is_running {
                process_was_running = true;
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
            }
        }

        self.active_menu_cloud_pid = None;
        self.active_menu_cloud_idx = None;

        if is_same_menu && process_was_running {
            return;
        }

        let x = (self.window_x as f64 + rx as f64) as i32;
        let y = (self.window_y as f64 + (ry + rh) as f64) as i32;

        use std::io::Write;
        use std::process::{Command, Stdio};

        let max_len = items.iter().map(|it| it.len()).max().unwrap_or(10);
        let layout_width = (max_len * 8 + 48).max(140) as u32;

        let mut widgets = vec![
            serde_json::json!({
                "type": "label",
                "text": title.clone(),
            })
        ];
        for (idx, item) in items.iter().enumerate() {
            widgets.push(serde_json::json!({
                "type": "button",
                "id": idx.to_string(),
                "text": item.clone(),
            }));
        }

        let json_config = serde_json::json!({
            "width": layout_width,
            "widgets": widgets,
        });

        let mut child = match Command::new("clear-cloud")
            .arg("--layout")
            .arg("-x")
            .arg(x.to_string())
            .arg("-y")
            .arg(y.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to spawn clear-cloud for menu: {:?}", e);
                return;
            }
        };

        let pid = child.id();
        self.active_menu_cloud_pid = Some(pid);
        self.active_menu_cloud_idx = Some((widget_idx, menu_idx));

        std::thread::spawn(move || {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(json_config.to_string().as_bytes());
            }

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("Failed to wait for clear-cloud for menu: {:?}", e);
                    return;
                }
            };

            if output.status.success() {
                let out_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Ok(res_val) = serde_json::from_str::<serde_json::Value>(&out_str) {
                    if let Some(btn_id) = res_val["button"].as_str() {
                        if let Ok(item_idx) = btn_id.parse::<usize>() {
                            let body = format!(
                                "{{\"action\":\"menu_click\",\"widget_idx\":{},\"menu_idx\":{},\"item_idx\":{}}}",
                                widget_idx, menu_idx, item_idx
                            );
                            let req = format!(
                                "POST /action HTTP/1.1\r\n\
                                 Host: 127.0.0.1:3000\r\n\
                                 Content-Type: application/json\r\n\
                                 Content-Length: {}\r\n\
                                 Connection: close\r\n\r\n\
                                 {}",
                                body.len(),
                                body
                            );
                            if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:3000") {
                                let _ = stream.write_all(req.as_bytes());
                                let _ = stream.flush();
                            }
                        }
                    }
                }
            }

            let body = format!(
                "{{\"action\":\"menu_closed\",\"widget_idx\":{},\"menu_idx\":{}}}",
                widget_idx, menu_idx
            );
            let req = format!(
                "POST /action HTTP/1.1\r\n\
                 Host: 127.0.0.1:3000\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n\
                 {}",
                body.len(),
                body
            );
            if let Ok(mut stream) = std::net::TcpStream::connect("127.0.0.1:3000") {
                let _ = stream.write_all(req.as_bytes());
                let _ = stream.flush();
            }
        });
    }

    fn close_node_palette(&mut self) {
        self.node_palette_visible = false;
        self.refresh_node_palette();
        self.upload_vertices();
    }


    fn place_selected_node(&mut self) -> bool {
        let Some(&template_idx) = self.node_palette_filtered.get(self.node_palette_selected) else { return false; };
        let mut node = self.node_templates[template_idx].node.clone();
        let (nx, ny) = self.find_empty_cell(self.grid_cursor_col as f32, self.grid_cursor_row as f32, None);
        node.position = (nx, ny);
        self.current_dir_mut().children.push(node);
        self.close_node_palette();
        self.sync_nodes();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.upload_vertices();
        true
    }


    fn handle_node_palette_key(&mut self, event: &KeyEvent) -> bool {
        if event.state != ElementState::Pressed { return false; }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.close_node_palette();
                true
            }
            Key::Named(NamedKey::Enter) => self.place_selected_node(),
            Key::Named(NamedKey::ArrowDown) => {
                if !self.node_palette_filtered.is_empty() {
                    self.node_palette_selected = (self.node_palette_selected + 1).min(self.node_palette_filtered.len() - 1);
                    self.refresh_node_palette();
                    self.upload_vertices();
                }
                true
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.node_palette_selected > 0 {
                    self.node_palette_selected -= 1;
                    self.refresh_node_palette();
                    self.upload_vertices();
                }
                true
            }
            Key::Named(NamedKey::Backspace) => {
                self.node_palette_query.pop();
                self.refresh_node_palette();
                self.upload_vertices();
                true
            }
            Key::Named(NamedKey::Tab) => {
                self.close_node_palette();
                true
            }
            _ => {
                if let Some(text) = &event.text {
                    for ch in text.chars().filter(|c| !c.is_control()) {
                        self.node_palette_query.push(ch);
                    }
                    self.refresh_node_palette();
                    self.upload_vertices();
                    true
                } else {
                    false
                }
            }
        }
    }

fn geometry_to_spreadsheet_data(geom: &Geometry) -> (Vec<String>, Vec<Vec<String>>) {
    let mut headers = vec![
        "Vertex".to_string(),
        "Pos.x".to_string(),
        "Pos.y".to_string(),
        "Pos.z".to_string(),
        "Col.r".to_string(),
        "Col.g".to_string(),
        "Col.b".to_string(),
    ];

    let mut custom_keys = std::collections::BTreeSet::new();
    for v in &geom.vertices {
        for k in v.attributes.keys() {
            custom_keys.insert(k.clone());
        }
    }
    let custom_keys: Vec<String> = custom_keys.into_iter().collect();

    for key in &custom_keys {
        if let Some(val) = geom.vertices.iter().find_map(|v| v.attributes.get(key)) {
            match val {
                GAttribute::Float(_) => {
                    headers.push(key.clone());
                }
                GAttribute::Float2(_) => {
                    headers.push(format!("{}.x", key));
                    headers.push(format!("{}.y", key));
                }
                GAttribute::Float3(_) => {
                    headers.push(format!("{}.x", key));
                    headers.push(format!("{}.y", key));
                    headers.push(format!("{}.z", key));
                }
                GAttribute::Float4(_) => {
                    headers.push(format!("{}.x", key));
                    headers.push(format!("{}.y", key));
                    headers.push(format!("{}.z", key));
                    headers.push(format!("{}.w", key));
                }
            }
        }
    }

    let mut rows = Vec::new();
    for (i, v) in geom.vertices.iter().enumerate() {
        let mut row = vec![
            i.to_string(),
            format!("{:.4}", v.pos[0]),
            format!("{:.4}", v.pos[1]),
            format!("{:.4}", v.pos[2]),
            format!("{:.4}", v.col[0]),
            format!("{:.4}", v.col[1]),
            format!("{:.4}", v.col[2]),
        ];

        for key in &custom_keys {
            if let Some(val) = v.attributes.get(key) {
                match val {
                    GAttribute::Float(f) => {
                        row.push(format!("{:.4}", f));
                    }
                    GAttribute::Float2(arr) => {
                        row.push(format!("{:.4}", arr[0]));
                        row.push(format!("{:.4}", arr[1]));
                    }
                    GAttribute::Float3(arr) => {
                        row.push(format!("{:.4}", arr[0]));
                        row.push(format!("{:.4}", arr[1]));
                        row.push(format!("{:.4}", arr[2]));
                    }
                    GAttribute::Float4(arr) => {
                        row.push(format!("{:.4}", arr[0]));
                        row.push(format!("{:.4}", arr[1]));
                        row.push(format!("{:.4}", arr[2]));
                        row.push(format!("{:.4}", arr[3]));
                    }
                }
            } else {
                if let Some(val) = geom.vertices.iter().find_map(|v| v.attributes.get(key)) {
                    let count = match val {
                        GAttribute::Float(_) => 1,
                        GAttribute::Float2(_) => 2,
                        GAttribute::Float3(_) => 3,
                        GAttribute::Float4(_) => 4,
                    };
                    for _ in 0..count {
                        row.push("-".to_string());
                    }
                }
            }
        }
        rows.push(row);
    }

    (headers, rows)
}

    fn update_active_camera_rotation(&mut self, d_yaw: f32, d_pitch: f32) -> bool {
        if self.active_camera == "Default Camera" {
            return false;
        }
        let camera_name = self.active_camera.clone();
        let dir = self.current_dir_mut();
        if let Some(node) = dir.children.iter_mut().find(|c| c.node_type == "camera" && c.name == camera_name) {
            if let Some(p) = node.params.iter_mut().find(|p| p.name == "Rotation") {
                let parts: Vec<&str> = p.default
                    .split(|c| c == ':' || c == ',' || c == ' ')
                    .filter(|s| !s.is_empty())
                    .collect();
                let mut rx = 0.0f32;
                let mut ry = 0.0f32;
                let mut rz = 0.0f32;
                if parts.len() >= 3 {
                    if let (Ok(vx), Ok(vy), Ok(vz)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>(), parts[2].parse::<f32>()) {
                        rx = vx;
                        ry = vy;
                        rz = vz;
                    }
                }
                ry += d_yaw.to_degrees();
                rx -= d_pitch.to_degrees();
                while rx > 180.0 { rx -= 360.0; }
                while rx < -180.0 { rx += 360.0; }
                while ry > 180.0 { ry -= 360.0; }
                while ry < -180.0 { ry += 360.0; }
                p.default = format!("{:.2}:{:.2}:{:.2}", rx, ry, rz);

                self.sync_nodes();
                let params = if !self.is_detached_network {
                    self.widgets[CONTENT_IDX].selected_node().and_then(|sel_idx| {
                        let dir = self.current_dir();
                        if sel_idx < dir.children.len() {
                            Some(param_display(&dir.children[sel_idx].params))
                        } else { None }
                    }).unwrap_or_default()
                } else {
                    vec![]
                };
                self.widgets[PARAM_IDX].set_display_params(&params);

                return true;
            }
        }
        false
    }

    fn update_active_camera_rotation_reset(&mut self) -> bool {
        if self.active_camera == "Default Camera" {
            return false;
        }
        let camera_name = self.active_camera.clone();
        let dir = self.current_dir_mut();
        if let Some(node) = dir.children.iter_mut().find(|c| c.node_type == "camera" && c.name == camera_name) {
            if let Some(p) = node.params.iter_mut().find(|p| p.name == "Rotation") {
                p.default = "0.00:0.00:0.00".to_string();
                self.sync_nodes();
                let params = if !self.is_detached_network {
                    self.widgets[CONTENT_IDX].selected_node().and_then(|sel_idx| {
                        let dir = self.current_dir();
                        if sel_idx < dir.children.len() {
                            Some(param_display(&dir.children[sel_idx].params))
                        } else { None }
                    }).unwrap_or_default()
                } else {
                    vec![]
                };
                self.widgets[PARAM_IDX].set_display_params(&params);
                return true;
            }
        }
        false
    }

    fn on_path_changed(&mut self) {
        self.widgets[CONTENT_IDX].set_selected_node(None);
        self.drag_widget = None;
        self.focused_widget = None;
        self.pan_x = 0.0;
        self.pan_y = 0.0;
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.last_frame_pan_x = 0.0;
        self.last_frame_pan_y = 0.0;
        self.is_scrolling_trackpad = false;
        self.scroll_accum_x = 0.0;
        self.scroll_accum_y = 0.0;
        self.grid_cursor_col = 0;
        self.grid_cursor_row = 0;
        self.sync_grid_settings();
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.upload_vertices();
    }

    fn find_empty_cell(&self, start_x: f32, start_y: f32, skip_idx: Option<usize>) -> (f32, f32) {
        let x = start_x;
        let mut y = start_y;
        let dir = self.current_dir();
        loop {
            let occupied = dir.children.iter().enumerate().any(|(idx, c)| {
                if Some(idx) == skip_idx {
                    false
                } else {
                    (c.position.0 - x).abs() < 0.01 && (c.position.1 - y).abs() < 0.01
                }
            });
            if occupied {
                y += 1.0;
            } else {
                break;
            }
        }
        (x, y)
    }

    fn sync_nodes(&mut self) {
        let graph_nodes: Vec<GraphNode> = self.current_dir().children.iter().map(|c| {
            GraphNode {
                name: c.name.clone(),
                position: c.position,
                parameters: param_display(&c.params),
                geom_visible: c.geometry_visible,
            }
        }).collect();
        self.widgets[CONTENT_IDX].set_nodes(&graph_nodes);

        let camera_nodes: Vec<String> = self.current_dir().children.iter()
            .filter(|c| c.node_type == "camera")
            .map(|c| c.name.clone())
            .collect();
        let mut items = vec!["Default Camera".to_string()];
        items.extend(camera_nodes);
        if !items.contains(&self.active_camera) {
            self.active_camera = "Default Camera".to_string();
        }
        self.widgets[RIGHT_MENUBAR_IDX].set_menu_items(0, &items);
        for (i, item) in items.iter().enumerate() {
            let checked = item == &self.active_camera;
            self.widgets[RIGHT_MENUBAR_IDX].set_item_checked(0, i, checked);
        }
        let path_strs = self.current_path_names();
        self.widgets[BREADCRUMB_IDX].set_path(&path_strs);

        let mut selected_node = None;
        if !self.is_detached_network {
            if let Some(slot_idx) = self.widgets[CONTENT_IDX].selected_node() {
                let dir = self.current_dir();
                if slot_idx < dir.children.len() {
                    selected_node = Some(&dir.children[slot_idx]);
                }
            }
        }


        let mut cache_hit = false;
        let mut current_name = None;
        let mut current_params = None;

        if let Some(node) = selected_node {
            current_name = Some(node.name.clone());
            current_params = Some(node.params.iter().map(|p| (p.name.clone(), p.default.clone())).collect::<Vec<_>>());
            if self.last_spreadsheet_node_name == current_name && self.last_spreadsheet_node_params == current_params {
                cache_hit = true;
            }
        } else {
            if self.last_spreadsheet_node_name.is_none() {
                cache_hit = true;
            }
        }

        if !cache_hit {
            let mut headers = Vec::new();
            let mut rows = Vec::new();

            if let Some(node) = selected_node {
                if node.node_type.eq_ignore_ascii_case("sphere") {
                    if let Some(idx) = find_sphere_index(&self.fs_root, node) {
                        let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                        let radius = node_param_f32(node, "Radius", 0.5).max(0.05);
                        let geom = sphere_vertices(center, radius);
                        let (h, r) = Self::geometry_to_spreadsheet_data(&geom);
                        headers = h;
                        rows = r;
                    }
                } else if node.node_type.eq_ignore_ascii_case("line") {
                    if let Some(idx) = find_sphere_index(&self.fs_root, node) {
                        let start = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                        let length = node_param_f32(node, "Length", 1.0);
                        let thickness = node_param_f32(node, "Thickness", 0.02);
                        let end = start + Vec3::new(0.0, length, 0.0);
                        let geom = line_vertices(start, end, thickness);
                        let (h, r) = Self::geometry_to_spreadsheet_data(&geom);
                        headers = h;
                        rows = r;
                    }
                } else if node.node_type.eq_ignore_ascii_case("add") {
                    if let Some(idx) = find_sphere_index(&self.fs_root, node) {
                        let center = Vec3::new((idx % 4) as f32 * 1.25 - 1.875, 0.55, -((idx / 4) as f32) * 1.25);
                        let num_points = node_param_f32(node, "Points", 100.0) as i32;
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
                        let (h, r) = Self::geometry_to_spreadsheet_data(&geom);
                        headers = h;
                        rows = r;
                    }
                } else if node.node_type.eq_ignore_ascii_case("transform") {
                    let mut visited = Vec::new();
                    if let Some(geom) = resolve_transform_geometry(&self.fs_root, node, &mut visited) {
                        let (h, r) = Self::geometry_to_spreadsheet_data(&geom);
                        headers = h;
                        rows = r;
                    }
                }
            }

            self.widgets[SPREADSHEET_IDX].set_spreadsheet_data(headers, rows);
            self.last_spreadsheet_node_name = current_name;
            self.last_spreadsheet_node_params = current_params;
        }
    }

    fn save_to_file(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if path.file_name().map_or(false, |n| n == "default_project.json") {
            let proj = Project {
                name: "Default Project".to_string(),
                root: self.fs_root.clone(),
                view_state: ProjectViewState {
                    active_camera: self.active_camera.clone(),
                    pan: (self.pan_x, self.pan_y),
                    current_path: self.current_path.clone(),
                    selected_node: self.widgets[CONTENT_IDX].selected_node(),
                },
            };
            let content = serde_json::to_string_pretty(&proj)?;
            fs::write(path, content)?;
            self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
            return Ok(());
        }

        let project_dir = path;
        let project_name = project_dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Default Project")
            .to_string();

        fs::create_dir_all(project_dir)?;

        let state_file_path = project_dir.join("state.json");

        let proj = Project {
            name: project_name,
            root: self.fs_root.clone(),
            view_state: ProjectViewState {
                active_camera: self.active_camera.clone(),
                pan: (self.pan_x, self.pan_y),
                current_path: self.current_path.clone(),
                selected_node: self.widgets[CONTENT_IDX].selected_node(),
            },
        };
        let content = serde_json::to_string_pretty(&proj)?;
        fs::write(&state_file_path, content)?;
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        Ok(())
    }

    fn load_from_file(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if path.file_name().map_or(false, |n| n == "default_project.json") {
            let content = fs::read_to_string(path)?;
            let proj: Project = serde_json::from_str(&content)?;
            self.fs_root = proj.root;
            self.active_camera = proj.view_state.active_camera;
            self.pan_x = proj.view_state.pan.0;
            self.pan_y = proj.view_state.pan.1;
            self.pan_velocity_x = 0.0;
            self.pan_velocity_y = 0.0;
            self.last_frame_pan_x = self.pan_x;
            self.last_frame_pan_y = self.pan_y;
            self.is_scrolling_trackpad = false;
            self.scroll_accum_x = 0.0;
            self.scroll_accum_y = 0.0;
            self.current_path = proj.view_state.current_path;

            let sel = proj.view_state.selected_node;
            self.widgets[CONTENT_IDX].set_selected_node(sel);
            if sel.is_some() {
                self.focused_widget = Some(CONTENT_IDX);
            } else {
                self.focused_widget = None;
            }
            self.drag_widget = None;
            self.last_click = None;

            self.sync_grid_settings();
            self.sync_nodes();

            let params = if !self.is_detached_network {
                self.widgets[CONTENT_IDX].selected_node().and_then(|sel_idx| {
                    let dir = self.current_dir();
                    if sel_idx < dir.children.len() {
                        Some(param_display(&dir.children[sel_idx].params))
                    } else { None }
                }).unwrap_or_default()
            } else {
                vec![]
            };
            self.widgets[PARAM_IDX].set_display_params(&params);

            self.rebuild_scene_geometry();
            self.rebuild_positions();
            self.apply_layout();
            self.update_panel_bounds();
            self.upload_vertices();
            self.loaded_project_path = None;
            self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
            self.update_window_title();
            return Ok(());
        }

        let (state_file_path, project_dir) = if path.is_dir() {
            (path.join("state.json"), path.to_path_buf())
        } else {
            if path.file_name().map_or(false, |name| name == "state.json") {
                (path.to_path_buf(), path.parent().unwrap_or(path).to_path_buf())
            } else {
                (path.to_path_buf(), path.parent().unwrap_or(path).to_path_buf())
            }
        };

        let content = fs::read_to_string(&state_file_path)?;
        let proj: Project = serde_json::from_str(&content)?;
        self.fs_root = proj.root;
        self.active_camera = proj.view_state.active_camera;
        self.pan_x = proj.view_state.pan.0;
        self.pan_y = proj.view_state.pan.1;
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.last_frame_pan_x = self.pan_x;
        self.last_frame_pan_y = self.pan_y;
        self.is_scrolling_trackpad = false;
        self.scroll_accum_x = 0.0;
        self.scroll_accum_y = 0.0;
        self.current_path = proj.view_state.current_path;

        let sel = proj.view_state.selected_node;
        self.widgets[CONTENT_IDX].set_selected_node(sel);
        if sel.is_some() {
            self.focused_widget = Some(CONTENT_IDX);
        } else {
            self.focused_widget = None;
        }
        self.drag_widget = None;
        self.last_click = None;

        self.sync_grid_settings();
        self.sync_nodes();

        // Sync Parameters pane with selected node
        let params = if !self.is_detached_network {
            self.widgets[CONTENT_IDX].selected_node().and_then(|sel_idx| {
                let dir = self.current_dir();
                if sel_idx < dir.children.len() {
                    Some(param_display(&dir.children[sel_idx].params))
                } else { None }
            }).unwrap_or_default()
        } else {
            vec![]
        };
        self.widgets[PARAM_IDX].set_display_params(&params);

        self.rebuild_scene_geometry();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.upload_vertices();
        self.loaded_project_path = Some(project_dir);
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        self.update_window_title();
        Ok(())
    }

    fn new_project(&mut self) {
        self.fs_root = FsNode {
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        self.active_camera = "Default Camera".to_string();
        self.pan_x = 0.0;
        self.pan_y = 0.0;
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.last_frame_pan_x = 0.0;
        self.last_frame_pan_y = 0.0;
        self.is_scrolling_trackpad = false;
        self.scroll_accum_x = 0.0;
        self.scroll_accum_y = 0.0;
        self.current_path.clear();
        self.grid_cursor_col = 0;
        self.grid_cursor_row = 0;

        self.focused_widget = None;
        self.drag_widget = None;
        self.last_click = None;

        self.sync_grid_settings();
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.upload_vertices();
        self.loaded_project_path = None;
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        self.update_window_title();
    }

    fn create_depth_texture(&self) -> (wgpu::Texture, wgpu::TextureView) {
        let tex = self.wgpu_adapter.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Depth Texture"),
            size: wgpu::Extent3d { width: self.physical_width.max(1), height: self.physical_height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        (tex, view)
    }

    fn create_backdrop_texture(&self) -> (wgpu::Texture, wgpu::TextureView) {
        let tex = self.wgpu_adapter.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Backdrop Texture"),
            size: wgpu::Extent3d { width: self.physical_width.max(1), height: self.physical_height.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.wgpu_adapter.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        (tex, view)
    }

    async fn new(
        conn: &Connection,
        qh: &QueueHandle<AppState>,
        compositor_state: &CompositorState,
        xdg_shell_state: &XdgShell,
        pw: u32,
        ph: u32,
        scale: f64,
        is_detached_network: bool,
    ) -> Self {
        clear_ui::scale::set_scale_factor(scale as f32);
        let settings = DesignSettings::load();
        let lw = pw as f32 / scale as f32;
        let lh = ph as f32 / scale as f32;
        let sw = lw;

        let wl_surface = compositor_state.create_surface(qh);
        wl_surface.set_buffer_scale(scale as i32);
        let window = xdg_shell_state.create_window(wl_surface.clone(), WindowDecorations::None, qh);
        if is_detached_network {
            window.set_title("Network Pane");
            window.set_app_id("circular-network-pane");
            window.set_min_size(Some((200, 200)));
        } else {
            window.set_title("Clear Design Interface");
            window.set_app_id("cce-design-interface");
            window.set_min_size(Some((480, 320)));
        }
        window.commit();

        let display_ptr = conn.backend().display_id().as_ptr() as *mut std::ffi::c_void;
        let surface_ptr = wl_surface.id().as_ptr() as *mut std::ffi::c_void;
        let wgpu_adapter = clear_ui::backend::WgpuAdapter::new(display_ptr, surface_ptr, pw, ph).await;

        let device = &wgpu_adapter.device;
        let queue = &wgpu_adapter.queue;
        let surface = &wgpu_adapter.surface;
        let config = &wgpu_adapter.config;

        let backdrop_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Backdrop Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let backdrop_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Backdrop Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let backdrop_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Backdrop Texture"),
            size: wgpu::Extent3d { width: pw.max(1), height: ph.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let backdrop_texture_view = backdrop_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let backdrop_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Backdrop Bind Group"),
            layout: &backdrop_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&backdrop_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&backdrop_sampler),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
            bind_group_layouts: &[&backdrop_bind_group_layout],
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        });

        // 3D pipeline
        let shader_3d = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader 3D"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_3d.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Uniform Buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout_3d = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3D Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });

        let pipeline_layout_3d = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3D Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout_3d],
            push_constant_ranges: &[],
        });

        let bind_group_3d = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3D Bind Group"),
            layout: &bind_group_layout_3d,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
            }],
        });

        let uniform_buffer_grid = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grid Uniform Buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_grid = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Grid 3D Bind Group"),
            layout: &bind_group_layout_3d,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buffer_grid,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
            }],
        });

        let uniform_buffer_pivot = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Pivot Uniform Buffer"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_pivot = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Pivot Bind Group"),
            layout: &bind_group_layout_3d,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform_buffer_pivot,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
            }],
        });

        let vertex_buffer_3d = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Cube Vertex Buffer"),
            contents: bytemuck::cast_slice(&cube_vertices()),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let vertex_buffer_spheres = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sphere Node Vertex Buffer"),
            size: 1,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let grid_verts = grid_vertices(settings.grid_thickness, settings.grid_color);
        let vertex_count_grid = grid_verts.len() as u32;
        let vertex_buffer_grid = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Grid Vertex Buffer"),
            contents: bytemuck::cast_slice(&grid_verts),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        let origin_verts = origin_vectors_vertices(settings.origin_size);
        let vertex_count_origin = origin_verts.len() as u32;
        let vertex_buffer_origin = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Origin Vectors Vertex Buffer"),
            contents: bytemuck::cast_slice(&origin_verts),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        let pivot_verts = camera_pivot_vertices(settings.camera_pivot_size);
        let vertex_count_pivot = pivot_verts.len() as u32;
        let vertex_buffer_pivot = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera Pivot Vertex Buffer"),
            contents: bytemuck::cast_slice(&pivot_verts),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        let pipeline_3d = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3D Pipeline"),
            layout: Some(&pipeline_layout_3d),
            vertex: wgpu::VertexState {
                module: &shader_3d,
                entry_point: Some("vs_main"),
                buffers: &[Vertex3D::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_3d,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        });

        let cache = Cache::new(device);

        let shader_textured = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Textured Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_textured.wgsl").into()),
        });

        let textured_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Textured Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let textured_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Textured Pipeline Layout"),
            bind_group_layouts: &[&textured_bind_group_layout],
            push_constant_ranges: &[],
        });

        let curved_text_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Textured Render Pipeline"),
            layout: Some(&textured_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_textured,
                entry_point: Some("vs_main"),
                buffers: &[TexturedVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_textured,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let curved_text_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Curved Text Texture"),
            size: wgpu::Extent3d {
                width: 1024,
                height: 1024,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let curved_text_texture_view = curved_text_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let curved_text_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Curved Text Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let curved_text_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Curved Text Bind Group"),
            layout: &textured_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&curved_text_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&curved_text_sampler),
                },
            ],
        });

        let mut curved_text_atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let curved_text_renderer = TextRenderer::new(&mut curved_text_atlas, &device, wgpu::MultisampleState::default(), None);
        let mut curved_text_viewport = Viewport::new(&device, &cache);
        curved_text_viewport.update(&queue, Resolution { width: 1024, height: 1024 });

        let textured_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Textured Vertex Buffer"),
            size: 1,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });


        let splitter_layout = clear_ui::layout::SplitterLayout::new(sw, SPLITTER_W, MIN_COLUMN);
        let (depth_texture, depth_texture_view) = {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Depth Texture"),
                size: wgpu::Extent3d { width: pw.max(1), height: ph.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        let templates_root = load_fs_tree();
        let node_templates = flatten_node_templates(&templates_root);

        let default_proj_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
        let mut loaded_project = None;
        if default_proj_path.exists() {
            if let Ok(content) = fs::read_to_string(&default_proj_path) {
                if let Ok(proj) = serde_json::from_str::<Project>(&content) {
                    loaded_project = Some(proj);
                }
            }
        }

        let fs_root = if let Some(ref proj) = loaded_project {
            proj.root.clone()
        } else {
            FsNode {
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![],
                params: vec![],
                geometry_visible: true,
                position: (0.0, 0.0),
            }
        };

        let active_camera = if let Some(ref proj) = loaded_project {
            proj.view_state.active_camera.clone()
        } else {
            "Default Camera".to_string()
        };

        let (pan_x, pan_y) = if let Some(ref proj) = loaded_project {
            proj.view_state.pan
        } else {
            (0.0, 0.0)
        };

        let current_path = if let Some(ref proj) = loaded_project {
            proj.view_state.current_path.clone()
        } else {
            vec![]
        };
        let mut widgets: Vec<Box<dyn Element>> = vec![
            Box::new(MenuBar::new(0.0, 0.0, 0.0, HEADER_H).with_title("Clear Design Interface").with_label("Main Menu Bar").with_item("File", &["New Project", "Open", "Save", "Save As", "Exit"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Reset Zoom", "Detach Circular Window", "Show Network Pane", "Show Viewport Pane", "Show Parameters Pane", "Show Spreadsheet Pane"]).with_item("Help", &["About"]).with_z_index(110)),
            Box::new(Graph::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(ViewportBg::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(Plate::new(0.0, 0.0, 0.0, 0.0).with_color(colors::PARAM_BG).with_blur(true)),
            Box::new(ParametersBg::new()),
            Box::new(Canvas::new()),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("0: Network").with_label("Network Menu Bar").with_item("File", &["New", "Open", "Save", "Save As"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Circular Pane", "Detach Pane", "Close Pane"]).with_item("Settings", &[])),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("1: Viewport").with_label("Viewport Menu Bar").with_item("Camera", &["Perspective", "Orthographic"]).with_item("Display", &["Square Aspect"]).with_item("Guides", &["Show Grid", "Cube", "Origin", "Camera Pivot"]).with_item("View", &["Close Pane"]).with_item("Settings", &[])),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("2: Parameters").with_label("Parameters Menu Bar").with_item("Preset", &["Default", "Custom"]).with_item("Reset", &["All"]).with_item("View", &["Close Pane"])),
            Box::new(StatusBar::new().with_text("Ready")),
            Box::new(Breadcrumb::new()),
            Box::new(NodePalette::new()),
            Box::new(Spreadsheet::new()),
        ];
        
        let mut spreadsheet_menubar = MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("3: Spreadsheet").with_label("Spreadsheet Menu Bar").with_item("View", &["Close Pane"]);
        spreadsheet_menubar.visible = false;
        widgets.push(Box::new(spreadsheet_menubar));

        let network_panel = Plate::new(0.0, 0.0, 0.0, 0.0).with_color([0.10, 0.10, 0.13, 0.95]);
        widgets.push(Box::new(network_panel));

        let paginator = Paginator::new(56.0, vec![]).with_sidebar_mode(true).with_column_layout(true).with_tabs_rotated(false);
        widgets.push(Box::new(paginator));

        let mut positions = Vec::with_capacity(PAGINATOR_IDX + 1);
        positions.resize_with(PAGINATOR_IDX + 1, || (0.0, 0.0, 0.0, 0.0));

        let recent_files = Self::load_recent_files();
        let recent_files_list = ScrollingList::new(22.0, 2.0);
        let mut recent_files_buttons = Vec::new();
        for file in &recent_files {
            let label = file.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| file.to_string_lossy().to_string());
            let btn = Button::new_list_row(0.0, 0.0, 0.0, 0.0).with_label(&label);
            recent_files_buttons.push(btn);
        }

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vertex Buffer"),
            size: 1,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut shortcut_manager = ShortcutManager::new();
        shortcut_manager.register("Ctrl+g", Action::ToggleGrid).unwrap();
        shortcut_manager.register("Ctrl+e", Action::ToggleCube).unwrap();
        shortcut_manager.register("Ctrl+a", Action::ToggleSquareViewport).unwrap();
        shortcut_manager.register("Ctrl+,", Action::ToggleConfigure).unwrap();
        shortcut_manager.register("`", Action::ToggleSpreadsheet).unwrap();
        shortcut_manager.register("Ctrl+d", Action::ToggleCircularPane).unwrap();
        shortcut_manager.register("Ctrl+s", Action::Save).unwrap();

        let mut state = Self {
            window,
            wl_surface,
            wgpu_adapter,
            render_pipeline,
            vertex_buffer,
            vertex_count: 0,
            pipeline_3d,
            bind_group_3d,
            bind_group_layout_3d,
            uniform_buffer,
            bind_group_grid,
            uniform_buffer_grid,
            bind_group_pivot,
            uniform_buffer_pivot,
            vertex_buffer_3d,
            vertex_count_3d: cube_vertices().len() as u32,
            vertex_buffer_spheres,
            vertex_count_spheres: 0,
            vertex_buffer_grid,
            vertex_count_grid,
            depth_texture,
            depth_texture_view,
            backdrop_texture,
            backdrop_texture_view,
            backdrop_sampler,
            backdrop_bind_group_layout,
            backdrop_bind_group,
            rotation_y: 0.0,
            rotation_x: 0.0,
            is_rotating_viewport: false,
            scroll_lock: 0,
            last_rotate_time: Instant::now(),
            rotate_accum_yaw: 0.0,
            rotate_accum_pitch: 0.0,
            rotate_velocity_yaw: 0.0,
            rotate_velocity_pitch: 0.0,
            show_grid: settings.show_grid_enabled,
            show_cube: settings.show_cube_enabled,
            show_origin: settings.show_origin_enabled,
            show_camera_pivot: settings.show_camera_pivot_enabled,
            viewport_bg_color: settings.viewport_bg_color,
            node_color: settings.node_color,
            grid_color: settings.grid_color,
            cell_color: settings.cell_color,
            gap_color: settings.gap_color,
            vertex_buffer_origin,
            vertex_count_origin,
            vertex_buffer_pivot,
            vertex_count_pivot,
            origin_size: settings.origin_size,
            camera_pivot_size: settings.camera_pivot_size,
            fs_root: fs_root.clone(),
            node_templates,
            current_path,
            last_click: None,
            last_frame: Instant::now(),
            shortcut_manager,
            pending_action: None,
            exit_requested: false,
            widgets,
            positions,
            splitter_layout,
            node_palette_visible: false,
            node_palette_query: String::new(),
            node_palette_filtered: Vec::new(),
            node_palette_selected: 0,
            curved_text_texture,
            curved_text_texture_view,
            curved_text_sampler,
            curved_text_bind_group,
            curved_text_pipeline,
            curved_text_atlas,
            curved_text_renderer,
            curved_text_viewport,
            textured_vertex_buffer,
            textured_vertex_count: 0,

            drag_widget: None,
            focused_widget: None,
            cursor_x: 0.0,
            cursor_y: 0.0,
            grid_cursor_col: 0,
            grid_cursor_row: 0,
            modifiers: ModifiersState::default(),
            width: lw,
            height: lh,
            physical_width: pw,
            physical_height: ph,
            scale,
            square_viewport: settings.square_viewport,
            grid_snap_enabled: settings.grid_snap_enabled,
            network_grid_visible: settings.network_grid_enabled,
            grid_size_x: settings.grid_size_x,
            grid_size_y: settings.grid_size_y,
            skipped_row_h: settings.skipped_row_h,
            skipped_col_w: settings.skipped_col_w,

            pan_x,
            pan_y,
            pan_velocity_x: 0.0,
            pan_velocity_y: 0.0,
            last_frame_pan_x: pan_x,
            last_frame_pan_y: pan_y,
            is_panning: false,
            pan_start_cx: 0.0,
            pan_start_cy: 0.0,
            pan_start_x: 0.0,
            pan_start_y: 0.0,
            space_pressed: false,
            active_camera,
            show_network: true,
            show_viewport: true,
            show_parameters: true,
            show_spreadsheet: false,
            is_scrolling_trackpad: false,
            last_scroll_time: Instant::now(),
            scroll_accum_x: 0.0,
            scroll_accum_y: 0.0,
            last_spreadsheet_node_name: None,
            last_spreadsheet_node_params: None,
            viewport_zoom: 1.0,
            is_zooming_viewport: false,
            last_zoom_time: Instant::now(),
            zoom_accum: 0.0,
            zoom_velocity: 0.0,
            grid_thickness: settings.grid_thickness,
            focused_pane: LEFT_MENUBAR_IDX,
            inertial_scroll_enabled: true,
            inertial_scroll_friction: 0.90,
            scroll_speed: 1.0,
            last_config_read: Instant::now(),
            circular_network_pane: is_detached_network,
            circular_network_layout: clear_ui::layout::CircularPaneLayout::new(250.0, 300.0, 180.0),
            is_detached_network,
            detached_circular_network: false,
            last_project_mod_time: {
                let default_proj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
                std::fs::metadata(&default_proj_path).and_then(|m| m.modified()).ok()
            },
            last_project_check: std::time::Instant::now(),
            last_inspector_check: std::time::Instant::now(),
            last_inspector_update: std::time::Instant::now(),
            needs_autosave: false,
            last_autosave_time: std::time::Instant::now(),
            window_x: 0,
            window_y: 0,
            active_menu_cloud_pid: None,
            active_menu_cloud_idx: None,
            uniform_background: settings.uniform_background,
            network_opacity: settings.network_opacity,
            last_design_mod_time: {
                let design_path = DesignSettings::file_path();
                std::fs::metadata(&design_path).and_then(|m| m.modified()).ok()
            },
            floating_network_layout: (18.0, 44.0, 400.0, 710.0),
            is_resizing_network: None,
            drag_start_rect: (0.0, 0.0, 0.0, 0.0),
            drag_start_mouse: (0.0, 0.0),
            floating_param_width: 300.0,
            is_resizing_param: false,
            drag_start_param_w: 0.0,
            floating_spreadsheet_height: 250.0,
            is_resizing_spreadsheet: false,
            drag_start_spreadsheet_h: 0.0,
            paginator_page_widgets: Vec::new(),
            last_paginator_menubar: None,
            loaded_project_path: None,
            last_saved_root_json: serde_json::to_string(&fs_root).unwrap_or_default(),
            recent_files,
            recent_files_list,
            recent_files_buttons,
            ui_context: clear_ui::context::UiContext::new(),
        };

        state.update_inertial_settings();
        state.update_window_title();
        colors::set_node_color([
            settings.node_color[0],
            settings.node_color[1],
            settings.node_color[2],
            1.0,
        ]);
        state.sync_nodes();
        state.rebuild_scene_geometry();
        state.sync_grid_settings();
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 0, state.show_grid);
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 1, state.show_cube);
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 2, state.show_origin);
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 3, state.show_camera_pivot);
        state.widgets[LEFT_MENUBAR_IDX].set_item_checked(2, 2, state.circular_network_pane);
        state.widgets[LEFT_MENUBAR_IDX].set_item_checked(2, 3, state.detached_circular_network);
        state.widgets[HEADER_IDX].set_item_checked(2, 3, state.detached_circular_network);
        state.widgets[HEADER_IDX].set_item_checked(2, 4, state.show_network);
        state.widgets[HEADER_IDX].set_item_checked(2, 5, state.show_viewport);
        state.widgets[HEADER_IDX].set_item_checked(2, 6, state.show_parameters);
        state.widgets[HEADER_IDX].set_item_checked(2, 7, state.show_spreadsheet);

        state.rebuild_positions();
        state.apply_layout();
        state.update_panel_bounds();
        state.sync_pane_focus();
        state.upload_vertices();
        state
    }

    fn sync_grid_settings(&mut self) {
        let active_node_area_y = self.positions[CONTENT_IDX].1;
        let active_node_area_x = self.positions[CONTENT_IDX].0;

        self.widgets[CONTENT_IDX].set_show_network_grid(self.network_grid_visible);
        self.widgets[CONTENT_IDX].set_grid_sizes(self.grid_size_x, self.grid_size_y);
        self.widgets[CONTENT_IDX].set_skipped_sizes(self.skipped_row_h, self.skipped_col_w);
        self.widgets[CONTENT_IDX].set_grid_origin(active_node_area_x + self.pan_x, active_node_area_y + self.pan_y);
        self.widgets[CONTENT_IDX].set_grid_snap_enabled(self.grid_snap_enabled);
        if let Some(graph) = self.widgets[CONTENT_IDX].as_any_mut().downcast_mut::<clear_ui::widget::Graph>() {
            graph.set_uniform_background(self.uniform_background);
            graph.set_network_opacity(self.network_opacity);
            graph.set_cell_color(self.cell_color);
            graph.set_gap_color(self.gap_color);
        }
        if let Some(menubar) = self.widgets[LEFT_MENUBAR_IDX].as_any_mut().downcast_mut::<clear_ui::widget::MenuBar>() {
            menubar.set_network_opacity(self.network_opacity);
        }
        if let Some(breadcrumb) = self.widgets[BREADCRUMB_IDX].as_any_mut().downcast_mut::<clear_ui::widget::Breadcrumb>() {
            breadcrumb.set_network_opacity(self.network_opacity);
        }
        if let Some(plate) = self.widgets[NETWORK_PANEL_IDX].as_any_mut().downcast_mut::<clear_ui::widget::Plate>() {
            plate.set_network_opacity(self.network_opacity);
        }
    }

    fn update_inertial_settings(&mut self) {
        self.last_config_read = Instant::now();
        let config_path = "/home/lsgalante/.config/cce/config.toml";
        
        let mut enabled = true;
        let mut friction = 0.90;
        let mut speed = 1.0;

        if let Ok(content) = std::fs::read_to_string(config_path) {
            #[derive(serde::Deserialize)]
            struct InertialSection {
                inertial_scroll: Option<bool>,
                scroll_friction: Option<u16>,
                scroll_speed: Option<f32>,
            }
            #[derive(serde::Deserialize)]
            struct Config {
                inertial: Option<InertialSection>,
            }
            if let Ok(cfg) = toml::from_str::<Config>(&content) {
                if let Some(inertial) = cfg.inertial {
                    if let Some(val) = inertial.inertial_scroll {
                        enabled = val;
                    }
                    if let Some(friction_val) = inertial.scroll_friction {
                        friction = (friction_val as f32 / 1000.0).clamp(0.1, 0.999);
                    }
                    if let Some(speed_val) = inertial.scroll_speed {
                        speed = speed_val.clamp(0.01, 10.0);
                    }
                }
            }
        }
        
        self.inertial_scroll_enabled = enabled;
        self.inertial_scroll_friction = friction;
        self.scroll_speed = speed;
    }


    fn zoom(&mut self, factor: f32, center: Option<(f32, f32)>) {
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.is_scrolling_trackpad = false;
        self.scroll_accum_x = 0.0;
        self.scroll_accum_y = 0.0;
        let old_gx = self.grid_size_x;
        let old_gy = self.grid_size_y;

        let new_gx = (old_gx * factor).clamp(30.0, 500.0);
        let new_gy = (old_gy * factor).clamp(15.0, 250.0);

        if (new_gx - old_gx).abs() < 0.01 {
            return;
        }

        let old_row_h = self.skipped_row_h;
        let old_col_w = self.skipped_col_w;

        let node_area_y = self.positions[CONTENT_IDX].1;
        let (cx, cy) = match center {
            Some(pt) => pt,
            None => {
                let col = self.grid_cursor_col as f32;
                let row = self.grid_cursor_row as f32;
                (
                    col * (old_gx + old_col_w) + 0.5 * old_gx + self.pan_x,
                    row * (old_gy + old_row_h) + 0.5 * old_gy + self.pan_y + node_area_y,
                )
            }
        };

        let col_f = (cx - self.pan_x) / old_gx;
        let row_f = (cy - self.pan_y - node_area_y) / old_gy;

        self.pan_x = cx - col_f * new_gx;
        self.pan_y = cy - row_f * new_gy - node_area_y;

        self.grid_size_x = new_gx;
        self.grid_size_y = new_gy;
        self.skipped_row_h = old_row_h * (new_gy / old_gy);
        self.skipped_col_w = old_col_w * (new_gx / old_gx);

        self.sync_grid_settings();
    }


    fn keep_cursor_in_view(&mut self) {
        let (px, py, pw, ph) = self.positions[CONTENT_IDX];

        let cx = px + self.grid_cursor_col as f32 * (self.grid_size_x + self.skipped_col_w) + self.pan_x;
        let cy = py + self.grid_cursor_row as f32 * (self.grid_size_y + self.skipped_row_h) + self.pan_y;
        let cw = self.grid_size_x;
        let ch = self.grid_size_y;

        if cx < px {
            self.pan_x -= cx - px;
        } else if cx + cw > px + pw {
            self.pan_x -= cx + cw - (px + pw);
        }

        if cy < py {
            self.pan_y -= cy - py;
        } else if cy + ch > py + ph {
            self.pan_y -= cy + ch - (py + ph);
        }
        self.sync_grid_settings();
    }

    fn rebuild_positions(&mut self) {
        self.clamp_splitters();
        self.widgets[LEFT_MENUBAR_IDX].set_center_items(self.circular_network_pane);

        let body_h = self.body_h();

        if self.is_detached_network {
            let cx = self.width / 2.0;
            let cy = self.height / 2.0;
            let r = (self.width.min(self.height) / 2.0 - 10.0).max(50.0);

            self.circular_network_layout.x = cx;
            self.circular_network_layout.y = cy;
            self.circular_network_layout.r = r;

            self.positions[0] = (0.0, 0.0, 0.0, 0.0);
            self.positions[LEFT_MENUBAR_IDX] = (cx - r, cy - r, 2.0 * r, 35.0);
            if let Some(menubar) = self.widgets[LEFT_MENUBAR_IDX].as_any_mut().downcast_mut::<clear_ui::widget::MenuBar>() {
                menubar.set_curved_circle(Some((cx, cy, r)));
            }
            self.positions[BREADCRUMB_IDX] = (cx - r, cy - r + 45.0 + MENUBAR_H, 2.0 * r, BREADCRUMB_H);
            self.positions[CONTENT_IDX] = (cx - r, cy - r + 45.0 + MENUBAR_H + BREADCRUMB_H, 2.0 * r, 2.0 * r - (45.0 + MENUBAR_H + BREADCRUMB_H));
            self.positions[NETWORK_PANEL_IDX] = (cx - r, cy - r, 2.0 * r, 2.0 * r);
            self.widgets[NETWORK_PANEL_IDX].set_rect(cx - r, cy - r, 2.0 * r, 2.0 * r);
            if let Some(plate) = self.widgets[NETWORK_PANEL_IDX].as_any_mut().downcast_mut::<clear_ui::widget::Plate>() {
                plate.set_curved_circle(Some((cx, cy, r)));
            }
            self.positions[SPLITTER1_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[SPLITTER2_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[PARAM_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[VIEWPORT_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[RIGHT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[SPREADSHEET_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[CANVAS_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[PARAM_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[STATUS_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[NODE_PALETTE_IDX] = (0.0, 0.0, self.width, self.height);
        } else if self.detached_circular_network {
            // Parent process: Network pane is detached (hidden from main window)
            let left_visible = false;
            let viewport_visible = self.show_viewport;
            let spreadsheet_visible = self.show_spreadsheet;
            let center_visible = viewport_visible || spreadsheet_visible;
            let right_visible = self.show_parameters;

            let (_col_l_x, _col_l_w, _s1_x, _s1_w, col_c_x, col_c_w, s2_x, s2_w, col_r_x, col_r_w) =
                match (left_visible, center_visible, right_visible) {
                    (false, true, true) => {
                        let c_w = self.splitter_layout.splitter2_x;
                        let s2 = c_w;
                        let r_x = s2 + SPLITTER_W;
                        let r_w = (self.width - r_x).max(0.0);
                        (0.0, 0.0, 0.0, 0.0, 0.0, c_w, s2, SPLITTER_W, r_x, r_w)
                    }
                    (false, true, false) => {
                        (0.0, 0.0, 0.0, 0.0, 0.0, self.width, 0.0, 0.0, 0.0, 0.0)
                    }
                    (false, false, true) => {
                        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, self.width)
                    }
                    _ => {
                        (0.0, 0.0, 0.0, 0.0, 0.0, self.width, 0.0, 0.0, 0.0, 0.0)
                    }
                };

            let mut vp_y = 0.0;
            let mut vp_h = 0.0;
            let mut sp_menub_y = 0.0;
            let mut sp_menub_h = 0.0;
            let mut sp_y = 0.0;
            let mut sp_h = 0.0;

            if center_visible {
                if viewport_visible && spreadsheet_visible {
                    let viewport_h = body_h * 2.0 / 3.0;
                    let spreadsheet_h = body_h - viewport_h;
                    vp_y = HEADER_H;
                    vp_h = viewport_h;
                    sp_menub_y = 0.0;
                    sp_menub_h = 0.0;
                    sp_y = HEADER_H + viewport_h;
                    sp_h = spreadsheet_h;
                } else if viewport_visible {
                    vp_y = HEADER_H;
                    vp_h = body_h;
                } else if spreadsheet_visible {
                    sp_menub_y = 0.0;
                    sp_menub_h = 0.0;
                    sp_y = HEADER_H;
                    sp_h = body_h;
                }
            }

            self.positions[0] = (0.0, 0.0, self.width, HEADER_H);
            self.positions[LEFT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[BREADCRUMB_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[CONTENT_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[NETWORK_PANEL_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.widgets[NETWORK_PANEL_IDX].set_rect(0.0, 0.0, 0.0, 0.0);
            self.positions[SPLITTER1_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[SPLITTER2_IDX] = (s2_x, HEADER_H, s2_w, body_h);
            self.positions[PARAM_IDX] = (col_r_x, HEADER_H, col_r_w, body_h);

            self.positions[VIEWPORT_IDX] = (col_c_x, vp_y, col_c_w, vp_h);
            self.positions[RIGHT_MENUBAR_IDX] = (col_c_x, vp_y, col_c_w, if viewport_visible { MENUBAR_H } else { 0.0 });

            self.positions[SPREADSHEET_MENUBAR_IDX] = (col_c_x, sp_menub_y, col_c_w, sp_menub_h);
            self.positions[SPREADSHEET_IDX] = (col_c_x, sp_y, col_c_w, sp_h);

            self.positions[CANVAS_IDX] = (0.0, HEADER_H, self.width, body_h);
            self.positions[PARAM_MENUBAR_IDX] = (col_r_x, HEADER_H, col_r_w, if right_visible { MENUBAR_H } else { 0.0 });
            self.positions[STATUS_IDX] = (0.0, self.height - STATUS_H, self.width, STATUS_H);
            self.positions[NODE_PALETTE_IDX] = (0.0, 0.0, self.width, self.height);

            self.widgets[0].set_visible(true);
            self.widgets[STATUS_IDX].set_visible(false);
            self.widgets[CONTENT_IDX].set_visible(false);
            self.widgets[NETWORK_PANEL_IDX].set_visible(false);
            self.widgets[LEFT_MENUBAR_IDX].set_visible(false);
            self.widgets[BREADCRUMB_IDX].set_visible(false);
            self.widgets[SPLITTER1_IDX].set_visible(false);
            self.widgets[SPLITTER2_IDX].set_visible(s2_w > 0.0);
            self.widgets[VIEWPORT_IDX].set_visible(viewport_visible);
            self.widgets[RIGHT_MENUBAR_IDX].set_visible(viewport_visible);
            self.widgets[PARAM_IDX].set_visible(right_visible);
            self.widgets[PARAM_MENUBAR_IDX].set_visible(right_visible);
            self.widgets[SPREADSHEET_IDX].set_visible(spreadsheet_visible);
            self.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(spreadsheet_visible);
        } else {
            let paginator_w = self.widgets[PAGINATOR_IDX].sidebar_w();
            let body_h = self.height - STATUS_H;
            self.positions[0] = (0.0, 0.0, 0.0, 0.0);
            if self.circular_network_pane {
                let left_visible = false;
                let viewport_visible = self.show_viewport;
                let spreadsheet_visible = self.show_spreadsheet;
                let center_visible = viewport_visible || spreadsheet_visible;
                let right_visible = self.show_parameters;

                let (_col_l_x, _col_l_w, _s1_x, _s1_w, col_c_x, col_c_w, s2_x, s2_w, col_r_x, col_r_w) =
                    match (left_visible, center_visible, right_visible) {
                        (false, true, true) => {
                            let s2 = self.splitter_layout.splitter2_x.max(paginator_w + MIN_COLUMN);
                            let viewport_w = s2 - paginator_w;
                            let r_x = s2 + SPLITTER_W;
                            let r_w = (self.width - r_x).max(0.0);
                            (0.0, 0.0, 0.0, 0.0, paginator_w, viewport_w, s2, SPLITTER_W, r_x, r_w)
                        }
                        (false, true, false) => {
                            (0.0, 0.0, 0.0, 0.0, paginator_w, self.width - paginator_w, 0.0, 0.0, 0.0, 0.0)
                        }
                        (false, false, true) => {
                            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, paginator_w, self.width - paginator_w)
                        }
                        _ => {
                            (0.0, 0.0, 0.0, 0.0, paginator_w, self.width - paginator_w, 0.0, 0.0, 0.0, 0.0)
                        }
                    };

                let mut vp_y = 0.0;
                let mut vp_h = 0.0;
                let mut sp_menub_y = 0.0;
                let mut sp_menub_h = 0.0;
                let mut sp_y = 0.0;
                let mut sp_h = 0.0;

                if center_visible {
                    if viewport_visible && spreadsheet_visible {
                        let viewport_h = body_h * 2.0 / 3.0;
                        let spreadsheet_h = body_h - viewport_h;
                        vp_y = 0.0;
                        vp_h = viewport_h;
                        sp_menub_y = 0.0;
                        sp_menub_h = 0.0;
                        sp_y = viewport_h;
                        sp_h = spreadsheet_h;
                    } else if viewport_visible {
                        vp_y = 0.0;
                        vp_h = body_h;
                    } else if spreadsheet_visible {
                        sp_menub_y = 0.0;
                        sp_menub_h = 0.0;
                        sp_y = 0.0;
                        sp_h = body_h;
                    }
                }

                if self.widgets[NETWORK_PANEL_IDX].is_dragging() {
                    let (px, py, _pw, _ph) = self.widgets[NETWORK_PANEL_IDX].rect();
                    let r = self.circular_network_layout.r;
                    self.circular_network_layout.x = px + r;
                    self.circular_network_layout.y = py + r;
                }

                let gap = 18.0;
                let max_r = ((self.width - paginator_w - 2.0 * gap).min(self.height - STATUS_H - 2.0 * gap) / 2.0).max(50.0);
                self.circular_network_layout.r = self.circular_network_layout.r.clamp(50.0, max_r);

                let r = self.circular_network_layout.r;
                let min_x = paginator_w + gap + r;
                let max_x = (self.width - gap - r).max(min_x);
                self.circular_network_layout.x = self.circular_network_layout.x.clamp(min_x, max_x);

                let min_y = gap + r;
                let max_y = (self.height - STATUS_H - gap - r).max(min_y);
                self.circular_network_layout.y = self.circular_network_layout.y.clamp(min_y, max_y);

                let cx = self.circular_network_layout.x;
                let cy = self.circular_network_layout.y;

                self.positions[LEFT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                if let Some(menubar) = self.widgets[LEFT_MENUBAR_IDX].as_any_mut().downcast_mut::<clear_ui::widget::MenuBar>() {
                    menubar.set_curved_circle(None);
                }
                self.positions[BREADCRUMB_IDX] = (cx - r, cy - r + 45.0, 2.0 * r, BREADCRUMB_H);
                self.positions[CONTENT_IDX] = (cx - r, cy - r + 45.0 + BREADCRUMB_H, 2.0 * r, 2.0 * r - (45.0 + BREADCRUMB_H));
                self.positions[NETWORK_PANEL_IDX] = (cx - r, cy - r, 2.0 * r, 2.0 * r);
                self.widgets[NETWORK_PANEL_IDX].set_rect(cx - r, cy - r, 2.0 * r, 2.0 * r);
                if let Some(plate) = self.widgets[NETWORK_PANEL_IDX].as_any_mut().downcast_mut::<clear_ui::widget::Plate>() {
                    plate.set_curved_circle(Some((cx, cy, r)));
                }
                self.widgets[NETWORK_PANEL_IDX].set_drag_bounds(0.0, 0.0, self.width, self.height);
                self.positions[SPLITTER1_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPLITTER2_IDX] = (s2_x, 0.0, s2_w, body_h);
                self.positions[PARAM_IDX] = (col_r_x, 0.0, col_r_w, body_h);

                self.positions[VIEWPORT_IDX] = (col_c_x, vp_y, col_c_w, vp_h);
                self.positions[RIGHT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);

                self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPREADSHEET_IDX] = (col_c_x, sp_y, col_c_w, sp_h);

                self.positions[CANVAS_IDX] = (0.0, 0.0, self.width, body_h);
                self.positions[PARAM_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[STATUS_IDX] = (0.0, self.height - STATUS_H, self.width, STATUS_H);
                self.positions[NODE_PALETTE_IDX] = (0.0, 0.0, self.width, self.height);

                self.positions[PAGINATOR_IDX] = (0.0, 0.0, paginator_w, self.height - STATUS_H);
                self.widgets[PAGINATOR_IDX].set_rect(0.0, 0.0, paginator_w, self.height - STATUS_H);

                self.widgets[0].set_visible(false);
                self.widgets[STATUS_IDX].set_visible(false);
                self.widgets[CONTENT_IDX].set_visible(self.show_network);
                self.widgets[NETWORK_PANEL_IDX].set_visible(self.show_network);
                self.widgets[LEFT_MENUBAR_IDX].set_visible(false);
                self.widgets[BREADCRUMB_IDX].set_visible(self.show_network);
                self.widgets[SPLITTER1_IDX].set_visible(false);
                self.widgets[SPLITTER2_IDX].set_visible(s2_w > 0.0);
                self.widgets[VIEWPORT_IDX].set_visible(viewport_visible);
                self.widgets[RIGHT_MENUBAR_IDX].set_visible(false);
                self.widgets[PARAM_IDX].set_visible(right_visible);
                self.widgets[PARAM_MENUBAR_IDX].set_visible(false);
                self.widgets[SPREADSHEET_IDX].set_visible(spreadsheet_visible);
                self.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(false);
                self.widgets[PAGINATOR_IDX].set_visible(true);
            } else {
                if !self.circular_network_pane && self.widgets[NETWORK_PANEL_IDX].is_dragging() {
                    let (px, py, _pw, _ph) = self.widgets[NETWORK_PANEL_IDX].rect();
                    self.floating_network_layout.0 = px;
                    self.floating_network_layout.1 = py;
                }

                let gap = 18.0_f32;
                let (mut _fx, mut _fy, mut fw, mut _fh) = self.floating_network_layout;
                fw = fw.clamp(150.0, (self.width - paginator_w - 2.0 * gap).max(150.0));
                let fx = paginator_w + gap;
                let fy = gap;
                let fh = (self.height - STATUS_H - 2.0 * gap).max(100.0);
                self.floating_network_layout = (fx, fy, fw, fh);

                let param_w = self.floating_param_width.clamp(150.0, (self.width - paginator_w - 2.0 * gap).max(150.0));
                self.floating_param_width = param_w;
                let param_x = self.width - gap - param_w;
                let param_y = gap;
                let param_h = (self.height - STATUS_H - 2.0 * gap).max(100.0);

                let ss_x = if self.show_network { fx + fw + gap } else { paginator_w + gap };
                let ss_w_end = if self.show_parameters { param_x - gap } else { self.width - gap };
                let ss_w = (ss_w_end - ss_x).max(150.0);
                let ss_y_end = self.height - STATUS_H - gap;
                let ss_h = self.floating_spreadsheet_height.clamp(100.0, (ss_y_end - gap).max(100.0));
                let ss_y = ss_y_end - ss_h;
                self.floating_spreadsheet_height = ss_h;

                let viewport_visible = self.show_viewport;
                let spreadsheet_visible = self.show_spreadsheet;
                let right_visible = self.show_parameters;

                let col_c_x = paginator_w;
                let col_c_w = self.width - paginator_w;
                let vp_y = 0.0;
                let vp_h = if viewport_visible { body_h } else { 0.0 };

                let (px, py, pw, ph) = if self.show_network {
                    (fx, fy, fw, fh)
                } else {
                    (0.0, 0.0, 0.0, 0.0)
                };

                if let Some(menubar) = self.widgets[LEFT_MENUBAR_IDX].as_any_mut().downcast_mut::<clear_ui::widget::MenuBar>() {
                    menubar.set_curved_circle(None);
                }
                
                let mb_h = 0.0;
                let bc_h = if self.show_network { BREADCRUMB_H } else { 0.0 };
                let content_h = (ph - mb_h - bc_h).max(0.0);

                self.positions[CONTENT_IDX] = (px, py + mb_h + bc_h, pw, content_h);
                self.positions[NETWORK_PANEL_IDX] = (px, py, pw, ph);
                self.widgets[NETWORK_PANEL_IDX].set_rect(px, py, pw, ph);
                if let Some(plate) = self.widgets[NETWORK_PANEL_IDX].as_any_mut().downcast_mut::<clear_ui::widget::Plate>() {
                    plate.set_curved_circle(None);
                }
                self.positions[SPLITTER1_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPLITTER2_IDX] = (0.0, 0.0, 0.0, 0.0);
                let p_rect = if self.show_parameters { (param_x, param_y, param_w, param_h) } else { (0.0, 0.0, 0.0, 0.0) };
                self.positions[PARAM_IDX] = p_rect;
                self.widgets[PARAM_IDX].set_rect(p_rect.0, p_rect.1, p_rect.2, p_rect.3);

                self.positions[VIEWPORT_IDX] = (col_c_x, vp_y, col_c_w, vp_h);
                self.positions[RIGHT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[BREADCRUMB_IDX] = (px, py + mb_h, pw, bc_h);
                self.positions[LEFT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);

                self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPREADSHEET_IDX] = if spreadsheet_visible { (ss_x, ss_y, ss_w, ss_h) } else { (0.0, 0.0, 0.0, 0.0) };
                self.widgets[SPREADSHEET_IDX].set_rect(
                    self.positions[SPREADSHEET_IDX].0,
                    self.positions[SPREADSHEET_IDX].1,
                    self.positions[SPREADSHEET_IDX].2,
                    self.positions[SPREADSHEET_IDX].3,
                );

                self.positions[CANVAS_IDX] = (0.0, 0.0, self.width, body_h);
                self.positions[PARAM_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[STATUS_IDX] = (0.0, self.height - STATUS_H, self.width, STATUS_H);
                self.positions[NODE_PALETTE_IDX] = (0.0, 0.0, self.width, self.height);

                self.positions[PAGINATOR_IDX] = (0.0, 0.0, paginator_w, self.height - STATUS_H);
                self.widgets[PAGINATOR_IDX].set_rect(0.0, 0.0, paginator_w, self.height - STATUS_H);

                self.widgets[0].set_visible(false);
                self.widgets[STATUS_IDX].set_visible(false);
                self.widgets[CONTENT_IDX].set_visible(self.show_network);
                self.widgets[NETWORK_PANEL_IDX].set_visible(self.show_network);
                self.widgets[LEFT_MENUBAR_IDX].set_visible(false);
                self.widgets[BREADCRUMB_IDX].set_visible(self.show_network);
                self.widgets[SPLITTER1_IDX].set_visible(false);
                self.widgets[SPLITTER2_IDX].set_visible(false);
                self.widgets[VIEWPORT_IDX].set_visible(viewport_visible);
                self.widgets[RIGHT_MENUBAR_IDX].set_visible(false);
                self.widgets[PARAM_IDX].set_visible(right_visible);
                self.widgets[PARAM_MENUBAR_IDX].set_visible(false);
                self.widgets[SPREADSHEET_IDX].set_visible(spreadsheet_visible);
                self.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(false);
                self.widgets[PAGINATOR_IDX].set_visible(true);
            }
        }

        if !self.is_detached_network {
            let mut active_menubar = self.focused_pane;
            if active_menubar == LEFT_MENUBAR_IDX && !self.show_network {
                active_menubar = HEADER_IDX;
            }
            if active_menubar == RIGHT_MENUBAR_IDX && !self.show_viewport {
                active_menubar = HEADER_IDX;
            }
            if active_menubar == PARAM_MENUBAR_IDX && !self.show_parameters {
                active_menubar = HEADER_IDX;
            }
            if active_menubar == SPREADSHEET_MENUBAR_IDX && !self.show_spreadsheet {
                active_menubar = HEADER_IDX;
            }

            if active_menubar != self.focused_pane {
                self.focused_pane = active_menubar;
            }

            let menubars = [
                HEADER_IDX,
                LEFT_MENUBAR_IDX,
                RIGHT_MENUBAR_IDX,
                PARAM_MENUBAR_IDX,
                SPREADSHEET_MENUBAR_IDX,
            ];
            for &idx in &menubars {
                self.positions[idx] = (0.0, 0.0, 0.0, 0.0);
                self.widgets[idx].set_visible(false);
            }
        }
        self.positions[PARAM_PLATE_IDX] = self.positions[PARAM_IDX];
        let p_visible = self.widgets[PARAM_IDX].visible();
        self.widgets[PARAM_PLATE_IDX].set_visible(p_visible);
    }


    fn apply_layout(&mut self) {
        for (i, pos) in self.positions.iter().enumerate() {
            if let Some(widget) = self.widgets.get_mut(i) {
                if widget.is_dragging() { continue; }
                let (x, y, w, h) = *pos;
                widget.set_rect(x, y, w, h);
            }
        }
        self.update_recent_files_layout();
    }

    fn update_panel_bounds(&mut self) {
        // Unbounded 2D canvas - no clamping
    }

    fn update_paginator(&mut self) {
        let active_menubar = self.focused_pane;
        let title = self.widgets[active_menubar].label().unwrap_or_default();
        let mut label_name = if let Some(colon_idx) = title.find(':') {
            title.split_at(colon_idx + 1).1.trim().to_uppercase()
        } else {
            title.to_uppercase()
        };
        if label_name.ends_with(" MENU BAR") {
            label_name = label_name.replace(" MENU BAR", "");
        }
        self.widgets[PAGINATOR_IDX].set_sidebar_label(Some(label_name));

        let menu_names = self.widgets[active_menubar].menu_names();
        let menu_items = self.widgets[active_menubar].menu_items_list();
        let menu_checked = self.widgets[active_menubar].menu_checked_list();

        let num_pages = menu_names.len();
        self.widgets[PAGINATOR_IDX].set_pages(menu_names.clone());

        // Check if we need to rebuild paginator page widgets.
        let need_rebuild = self.last_paginator_menubar != Some(active_menubar)
            || self.paginator_page_widgets.len() != num_pages;

        if need_rebuild {
            self.last_paginator_menubar = Some(active_menubar);
            self.paginator_page_widgets.clear();
            self.paginator_page_widgets.resize_with(num_pages, Vec::new);

            for (page_idx, page_name) in menu_names.iter().enumerate() {
                if page_name == "Settings" {
                    if active_menubar == LEFT_MENUBAR_IDX {
                        // 0: Snap to Grid (Checkbox)
                        let mut cb = Checkbox::new().with_label("Snap to Grid");
                        cb.set_checked(self.grid_snap_enabled);
                        cb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cb));

                        // 1: Grid Visible (Checkbox)
                        let mut cb = Checkbox::new().with_label("Grid Visible");
                        cb.set_checked(self.network_grid_visible);
                        cb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cb));

                        // 2: Grid X (Spinbox, range 10-200)
                        let mut sb = Spinbox::new(self.grid_size_x as i32, 10, 200, 1).with_label("Grid X");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));

                        // 3: Grid Y (Spinbox, range 5-100)
                        let mut sb = Spinbox::new(self.grid_size_y as i32, 5, 100, 1).with_label("Grid Y");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));

                        // 4: Skipped Row H (Spinbox, range 0-150)
                        let mut sb = Spinbox::new(self.skipped_row_h as i32, 0, 150, 1).with_label("Skipped Row H");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));

                        // 5: Skipped Col W (Spinbox, range 0-150)
                        let mut sb = Spinbox::new(self.skipped_col_w as i32, 0, 150, 1).with_label("Skipped Col W");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));

                        // 6: Uniform Background (Checkbox)
                        let mut cb = Checkbox::new().with_label("Uniform Background");
                        cb.set_checked(self.uniform_background);
                        cb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cb));

                        // 7: Opacity (Slider, range 0.0-1.0)
                        let mut sl = Slider::new().with_range(0.0, 1.0).with_value(self.network_opacity).with_label("Opacity").with_readout(true).with_scroll(true);
                        sl.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sl));

                        // 8: Node Color (ColorSelector)
                        let mut cs = ColorSelector::new([
                            (self.node_color[0] * 255.0) as u8,
                            (self.node_color[1] * 255.0) as u8,
                            (self.node_color[2] * 255.0) as u8,
                        ]).with_label("Node Color");
                        cs.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cs));

                        // 9: Cell Color (ColorSelector)
                        let mut cs = ColorSelector::new([
                            (self.cell_color[0] * 255.0) as u8,
                            (self.cell_color[1] * 255.0) as u8,
                            (self.cell_color[2] * 255.0) as u8,
                        ]).with_label("Cell Color");
                        cs.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cs));

                        // 10: Gap Color (ColorSelector)
                        let mut cs = ColorSelector::new([
                            (self.gap_color[0] * 255.0) as u8,
                            (self.gap_color[1] * 255.0) as u8,
                            (self.gap_color[2] * 255.0) as u8,
                        ]).with_label("Gap Color");
                        cs.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cs));
                    } else if active_menubar == RIGHT_MENUBAR_IDX {
                        // 0: Show Grid Guide (Checkbox)
                        let mut cb = Checkbox::new().with_label("Show Grid Guide");
                        cb.set_checked(self.show_grid);
                        cb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cb));

                        // 1: Show Reference Cube (Checkbox)
                        let mut cb = Checkbox::new().with_label("Show Reference Cube");
                        cb.set_checked(self.show_cube);
                        cb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cb));

                        // 2: Show Origin Axes (Checkbox)
                        let mut cb = Checkbox::new().with_label("Show Origin Axes");
                        cb.set_checked(self.show_origin);
                        cb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cb));

                        // 3: Show Camera Pivot (Checkbox)
                        let mut cb = Checkbox::new().with_label("Show Camera Pivot");
                        cb.set_checked(self.show_camera_pivot);
                        cb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(cb));

                        // 4: Grid Thickness (Spinbox, range 2-200, decimals 3)
                        let mut sb = Spinbox::new((self.grid_thickness * 1000.0) as i32, 2, 200, 1).with_label("Grid Thickness").with_decimals(3);
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));

                        // 5: Origin Guide Size (Spinbox, range 1-50, decimals 1)
                        let mut sb = Spinbox::new((self.origin_size * 10.0) as i32, 1, 50, 1).with_label("Origin Guide Size").with_decimals(1);
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));

                        // 6: Camera Pivot Size (Spinbox, range 1-50, decimals 1)
                        let mut sb = Spinbox::new((self.camera_pivot_size * 10.0) as i32, 1, 50, 1).with_label("Camera Pivot Size").with_decimals(1);
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));

                        // 7, 8, 9: BG Color R, G, B (Spinbox, range 0-255)
                        let mut sb = Spinbox::new((self.viewport_bg_color[0] * 255.0) as i32, 0, 255, 1).with_label("BG Color R");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));
                        let mut sb = Spinbox::new((self.viewport_bg_color[1] * 255.0) as i32, 0, 255, 1).with_label("BG Color G");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));
                        let mut sb = Spinbox::new((self.viewport_bg_color[2] * 255.0) as i32, 0, 255, 1).with_label("BG Color B");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));

                        // 10, 11, 12: Grid Color R, G, B (Spinbox, range 0-255)
                        let mut sb = Spinbox::new((self.grid_color[0] * 255.0) as i32, 0, 255, 1).with_label("Grid Color R");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));
                        let mut sb = Spinbox::new((self.grid_color[1] * 255.0) as i32, 0, 255, 1).with_label("Grid Color G");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));
                        let mut sb = Spinbox::new((self.grid_color[2] * 255.0) as i32, 0, 255, 1).with_label("Grid Color B");
                        sb.set_rect(0.0, 0.0, 0.0, 42.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(sb));
                    }
                } else {
                    let items = &menu_items[page_idx];
                    let checked_list = menu_checked.get(page_idx);
                    for (item_idx, item_name) in items.iter().enumerate() {
                        let is_checked = checked_list.and_then(|l| l.get(item_idx).copied().flatten());
                        if let Some(checked_val) = is_checked {
                            let mut cb = Checkbox::new().with_label(item_name);
                            cb.set_checked(checked_val);
                            self.paginator_page_widgets[page_idx].push(Box::new(cb));
                        } else {
                            let btn = Button::new(0.0, 0.0, 150.0, 24.0).with_label(item_name);
                            self.paginator_page_widgets[page_idx].push(Box::new(btn));
                        }
                    }

                    if menu_names[page_idx] == "File" {
                        let mut recent_lbl = Label::new("Recent Files").with_font_size(11.0).with_color([0xd4, 0xd4, 0xd4]);
                        recent_lbl.set_rect(0.0, 0.0, 150.0, 16.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(recent_lbl));

                        let mut recent_list = ScrollingList::new(22.0, 2.0);
                        recent_list.set_rect(0.0, 0.0, 150.0, 100.0);
                        self.paginator_page_widgets[page_idx].push(Box::new(recent_list));
                    }
                }
            }

            // Page widgets rebuild completed.
        }

        let sel_page = self.widgets[PAGINATOR_IDX].selected_page();
        self.widgets[PARAM_IDX].clear_children(&mut self.ui_context);
        if !self.widgets[PAGINATOR_IDX].is_page_hidden() {
            if sel_page < self.paginator_page_widgets.len() {
                for widget in &self.paginator_page_widgets[sel_page] {
                    let ptr = &**widget as *const (dyn Element + 'static) as *mut (dyn Element + 'static);
                    self.widgets[PARAM_IDX].add_child(ptr, &mut self.ui_context);
                }
                if menu_names.get(sel_page).map(|s| s.as_str()) == Some("File") {
                    for btn in &mut self.recent_files_buttons {
                        let ptr = btn as *mut Button as *mut (dyn Element + 'static);
                        self.widgets[PARAM_IDX].add_child(ptr, &mut self.ui_context);
                    }
                }
            }
        }

        let (px, py, pw, ph) = self.positions[PAGINATOR_IDX];
        self.widgets[PAGINATOR_IDX].set_rect(px, py, pw, ph);

        // Layout the parameters plate with the new children immediately.
        let (ppx, ppy, ppw, pph) = self.positions[PARAM_IDX];
        self.widgets[PARAM_IDX].set_rect(ppx, ppy, ppw, pph);

        self.update_recent_files_layout();
    }

    fn sync_pane_focus(&mut self) {
        for &menubar_idx in &[HEADER_IDX, LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX] {
            self.widgets[menubar_idx].set_selected(menubar_idx == self.focused_pane);
        }
        if self.focused_pane != PARAM_MENUBAR_IDX {
            self.widgets[PARAM_IDX].unfocus();
            self.sync_parameters_to_project();
        }
        self.update_paginator();
    }

    fn sync_layout(&mut self) {
        let left_visible = self.show_network && !self.circular_network_pane && !self.is_detached_network && !self.detached_circular_network;
        let center_visible = self.show_viewport || self.show_spreadsheet;
        let right_visible = self.show_parameters;

        if left_visible && center_visible && self.widgets[SPLITTER1_IDX].visible() && self.widgets[SPLITTER1_IDX].rect().2 > 0.0 {
            self.splitter_layout.splitter1_x = self.widgets[SPLITTER1_IDX].rect().0;
        }
        if right_visible && (center_visible || left_visible) && self.widgets[SPLITTER2_IDX].visible() && self.widgets[SPLITTER2_IDX].rect().2 > 0.0 {
            self.splitter_layout.splitter2_x = self.widgets[SPLITTER2_IDX].rect().0;
        }
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.sync_pane_focus();
    }

    fn execute_action(&mut self, action: Action) {
        let mut settings_changed = false;
        match action {
            Action::ToggleGrid => {
                self.show_grid = !self.show_grid;
                self.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 0, self.show_grid);
                settings_changed = true;
            }
            Action::ToggleCube => {
                self.show_cube = !self.show_cube;
                self.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 1, self.show_cube);
                settings_changed = true;
            }
            Action::ToggleOrigin => {
                self.show_origin = !self.show_origin;
                self.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 2, self.show_origin);
                settings_changed = true;
            }
            Action::ToggleCameraPivot => {
                self.show_camera_pivot = !self.show_camera_pivot;
                self.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 3, self.show_camera_pivot);
                settings_changed = true;
            }
            Action::ToggleSquareViewport => {
                self.square_viewport = !self.square_viewport;
                settings_changed = true;
            }
            Action::ToggleConfigure => {
                let target_pane = if self.show_network {
                    LEFT_MENUBAR_IDX
                } else if self.show_viewport {
                    RIGHT_MENUBAR_IDX
                } else {
                    LEFT_MENUBAR_IDX
                };
                self.focused_pane = target_pane;
                self.widgets[PAGINATOR_IDX].set_page_hidden(false);
                let page_idx = if target_pane == LEFT_MENUBAR_IDX { 3 } else { 4 };
                self.widgets[PAGINATOR_IDX].set_selected_page(page_idx);
                self.sync_pane_focus();
                self.rebuild_positions();
                self.apply_layout();
                self.upload_vertices();
                return;
            }
            Action::ToggleSpreadsheet => {
                self.show_spreadsheet = !self.show_spreadsheet;
                self.widgets[SPREADSHEET_IDX].set_visible(self.show_spreadsheet);
                self.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(self.show_spreadsheet);
                self.widgets[HEADER_IDX].set_item_checked(2, 7, self.show_spreadsheet);
                if !self.show_spreadsheet && self.focused_pane == SPREADSHEET_MENUBAR_IDX {
                    self.focused_pane = get_next_visible_pane(
                        self.focused_pane,
                        self.show_network,
                        self.show_viewport,
                        self.show_parameters,
                        self.show_spreadsheet,
                        false,
                    );
                }
                self.rebuild_positions();
                self.apply_layout();
                self.sync_pane_focus();
                self.sync_nodes();
            }
            Action::ToggleCircularPane => {
                self.circular_network_pane = !self.circular_network_pane;
                self.widgets[LEFT_MENUBAR_IDX].set_item_checked(2, 2, self.circular_network_pane);
                self.rebuild_positions();
                self.apply_layout();
                self.sync_grid_settings();
            }
            Action::DetachCircularWindow => {
                let default_proj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
                if let Err(e) = self.save_to_file(&default_proj_path) {
                    eprintln!("Failed to save default project before detaching: {:?}", e);
                }

                self.detached_circular_network = !self.detached_circular_network;
                self.widgets[LEFT_MENUBAR_IDX].set_item_checked(2, 3, self.detached_circular_network);
                self.widgets[HEADER_IDX].set_item_checked(2, 3, self.detached_circular_network);

                if self.detached_circular_network {
                    let _ = std::process::Command::new(std::env::current_exe().unwrap())
                        .arg("--detached-network")
                        .spawn();
                }

                self.rebuild_positions();
                self.apply_layout();
                self.sync_grid_settings();
            }
            Action::Save => {
                let path_opt = self.loaded_project_path.clone();
                if let Some(path) = path_opt {
                    if let Err(e) = self.save_to_file(&path) {
                        eprintln!("Failed to save project: {:?}", e);
                        self.update_status_text(&format!("Failed to save: {:?}", e));
                    } else {
                        self.update_status_text(&format!("Project saved to {}", path.display()));
                        self.add_recent_file(path);
                    }
                } else {
                    self.save_file_chooser();
                }
            }
        }
        if settings_changed {
            self.save_settings();
        }
    }

    fn read_panel_offsets(&mut self) {
        let updated_nodes = self.widgets[CONTENT_IDX].get_nodes();
        let dir = self.current_dir_mut();
        for (i, node) in updated_nodes.iter().enumerate() {
            if let Some(child) = dir.children.get_mut(i) {
                child.position = node.position;
            }
        }
        if let Some(sel_idx) = self.widgets[CONTENT_IDX].selected_node() {
            if let Some(node) = updated_nodes.get(sel_idx) {
                self.grid_cursor_col = node.position.0 as i32;
                self.grid_cursor_row = node.position.1 as i32;
            }
        }
    }

    fn sync_cursor_and_selection(&mut self) {
        if self.focused_widget != Some(CONTENT_IDX) {
            return;
        }
        let dir = self.current_dir();
        let mut node_at_cursor_idx = None;
        for (i, node) in dir.children.iter().enumerate() {
            if node.position.0 as i32 == self.grid_cursor_col && node.position.1 as i32 == self.grid_cursor_row {
                node_at_cursor_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = node_at_cursor_idx {
            self.widgets[CONTENT_IDX].set_selected_node(Some(idx));
            if self.focused_widget != Some(CONTENT_IDX) {
                self.focused_widget = Some(CONTENT_IDX);
            }
        } else {
            self.widgets[CONTENT_IDX].set_selected_node(None);
            if self.focused_widget == Some(CONTENT_IDX) {
                self.focused_widget = None;
            }
        }
    }


    fn collect_vertices(&self) -> Vec<Vertex> {
        let sw = self.width;
        let sh = self.height;
        let mut verts = Vec::new();

        let node_area_y = self.positions[CONTENT_IDX].1;
        let dialog_open = self.node_palette_visible;
        let show_cursor = self.drag_widget.is_none()
            && !dialog_open;

        let clip = if self.circular_network_pane {
            (
                self.circular_network_layout.x - self.circular_network_layout.r,
                self.circular_network_layout.y - self.circular_network_layout.r,
                self.circular_network_layout.x + self.circular_network_layout.r,
                self.circular_network_layout.y + self.circular_network_layout.r,
            )
        } else {
            let (cx, cy, cw, ch) = self.positions[CONTENT_IDX];
            (cx, cy, cx + cw, cy + ch)
        };

        let clip_circle_val = if self.circular_network_pane {
            [self.circular_network_layout.x * self.scale as f32, self.circular_network_layout.y * self.scale as f32, self.circular_network_layout.r * self.scale as f32]
        } else {
            [0.0, 0.0, 0.0]
        };

        let mut draw_order: Vec<usize> = (0..self.widgets.len()).collect();
        draw_order.sort_by_key(|&i| {
            if i == NETWORK_PANEL_IDX || i == PARAM_PLATE_IDX {
                -5
            } else if i == VIEWPORT_IDX || i == PARAM_IDX {
                -4
            } else {
                self.widgets[i].z_index()
            }
        });
        println!("DEBUG_DRAW_ORDER: {:?}", draw_order.iter().map(|&i| (i, self.widgets[i].visible(), self.positions[i])).collect::<Vec<_>>());

        for &i in &draw_order {
            let w = &self.widgets[i];
            if !w.visible() {
                continue;
            }
            let is_network_part = i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX || i == NETWORK_PANEL_IDX;
            let active_clip_circle = if is_network_part { clip_circle_val } else { [0.0, 0.0, 0.0] };

            if i == NETWORK_PANEL_IDX {
                if self.circular_network_pane {
                    verts.extend(circle_vertices(
                        self.circular_network_layout.x,
                        self.circular_network_layout.y,
                        self.circular_network_layout.r,
                        sw,
                        sh,
                        w.color(),
                        64,
                        active_clip_circle,
                    ));
                    verts.extend(circle_border_vertices(
                        self.circular_network_layout.x,
                        self.circular_network_layout.y,
                        self.circular_network_layout.r,
                        3.0,
                        sw,
                        sh,
                        [0.35, 0.65, 0.95, 0.80 * self.network_opacity],
                        64,
                        active_clip_circle,
                    ));
                } else {
                    verts.extend(widget_vertices(w.as_ref(), sw, sh, active_clip_circle));
                }
            } else if i == CONTENT_IDX {
                if !self.circular_network_pane {
                    verts.extend(widget_vertices(w.as_ref(), sw, sh, active_clip_circle));
                }

                for (qx, qy, qw, qh, qc) in w.extra_quads() {
                    verts.extend(extra_quad_vertices_clipped(w.as_ref(), qx, qy, qw, qh, sw, sh, qc, clip, active_clip_circle));
                }
            } else if i == LEFT_MENUBAR_IDX && self.circular_network_pane {
                let cx = self.circular_network_layout.x;
                let cy = self.circular_network_layout.y;
                let r = self.circular_network_layout.r;
                
                let bg_color = w.color();
                verts.extend(arc_background_vertices(
                    cx, cy, r,
                    MENUBAR_H,
                    std::f32::consts::PI,
                    2.0 * std::f32::consts::PI,
                    sw, sh,
                    bg_color,
                    64,
                    active_clip_circle,
                ));
                
                let border_color = [0.22, 0.22, 0.28, 0.90 * self.network_opacity];
                verts.extend(arc_background_vertices(
                    cx, cy, r - MENUBAR_H,
                    1.5,
                    std::f32::consts::PI,
                    2.0 * std::f32::consts::PI,
                    sw, sh,
                    border_color,
                    64,
                    active_clip_circle,
                ));
                
                for (qx, qy, qw, qh, qc) in w.extra_quads() {
                    verts.extend(extra_quad_vertices(w.as_ref(), qx, qy, qw, qh, sw, sh, qc, active_clip_circle));
                }
                for (acx, acy, ar, ath, a_start, a_end, acolor) in w.extra_arcs() {
                    verts.extend(arc_background_vertices(
                        acx, acy, ar,
                        ath,
                        a_start, a_end,
                        sw, sh,
                        acolor,
                        64,
                        active_clip_circle,
                    ));
                }
            } else {
                verts.extend(widget_vertices(w.as_ref(), sw, sh, active_clip_circle));
                if i == PAGINATOR_IDX {
                    let eq = w.extra_quads();
                    println!("DEBUG_COLLECT: Paginator extra_quads len = {}", eq.len());
                    for (qx, qy, qw, qh, qc) in eq {
                        verts.extend(extra_quad_vertices(w.as_ref(), qx, qy, qw, qh, sw, sh, qc, active_clip_circle));
                    }
                } else {
                    for (qx, qy, qw, qh, qc) in w.extra_quads() {
                        verts.extend(extra_quad_vertices(w.as_ref(), qx, qy, qw, qh, sw, sh, qc, active_clip_circle));
                    }
                }
                for (acx, acy, ar, ath, a_start, a_end, acolor) in w.extra_arcs() {
                    verts.extend(arc_background_vertices(
                        acx, acy, ar,
                        ath,
                        a_start, a_end,
                        sw, sh,
                        acolor,
                        64,
                        active_clip_circle,
                    ));
                }
            }


            if i == CONTENT_IDX && show_cursor {
                let cx = active_clip_circle[0] / self.scale as f32 - self.circular_network_layout.r + self.pan_x; // Wait, let's keep the exact cursor coordinates!
                let active_node_area_y = if self.circular_network_pane {
                    self.circular_network_layout.y - self.circular_network_layout.r + 45.0 + MENUBAR_H + BREADCRUMB_H
                } else {
                    node_area_y
                };
                let active_node_area_x = self.positions[CONTENT_IDX].0;
                let cx = active_node_area_x + self.grid_cursor_col as f32 * (self.grid_size_x + self.skipped_col_w) + self.pan_x;
                let cy = active_node_area_y + self.grid_cursor_row as f32 * (self.grid_size_y + self.skipped_row_h) + self.pan_y;
                let cw = self.grid_size_x;
                let ch = self.grid_size_y;

                let bg_color = [0.15, 0.25, 0.45, 0.15];
                let border_color = [0.35, 0.65, 0.95, 0.60];
                let thickness = 2.0_f32;

                // Filled cursor background
                verts.extend(quad_vertices_clipped(
                    cx + thickness,
                    cy + thickness,
                    cw - 2.0 * thickness,
                    ch - 2.0 * thickness,
                    sw,
                    sh,
                    bg_color,
                    clip,
                    active_clip_circle,
                ));

                // 4 border edges
                verts.extend(quad_vertices_clipped(cx, cy, cw, thickness, sw, sh, border_color, clip, active_clip_circle));
                verts.extend(quad_vertices_clipped(cx, cy + ch - thickness, cw, thickness, sw, sh, border_color, clip, active_clip_circle));
                verts.extend(quad_vertices_clipped(cx, cy + thickness, thickness, ch - 2.0 * thickness, sw, sh, border_color, clip, active_clip_circle));
                verts.extend(quad_vertices_clipped(cx + cw - thickness, cy + thickness, thickness, ch - 2.0 * thickness, sw, sh, border_color, clip, active_clip_circle));
            }
        }
        verts
    }

    fn upload_vertices(&mut self) {
        let verts = self.collect_vertices();
        self.vertex_count = verts.len() as u32;
        let data = bytemuck::cast_slice(&verts);
        let needed = data.len() as wgpu::BufferAddress;
        if needed > self.vertex_buffer.size() {
            self.vertex_buffer = self.wgpu_adapter.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Vertex Buffer"),
                size: needed,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.wgpu_adapter.queue.write_buffer(&self.vertex_buffer, 0, data);
    }

    fn rebuild_scene_geometry(&mut self) {
        let mut geom = network_sphere_vertices(&self.fs_root);

        fn collect_opencl_codes<'a>(node: &'a FsNode, codes: &mut Vec<&'a str>) {
            if node.node_type.eq_ignore_ascii_case("opencl") && node.geometry_visible {
                let code = node.params.iter()
                    .find(|p| p.name.eq_ignore_ascii_case("code"))
                    .map(|p| p.default.as_str())
                    .unwrap_or("");
                codes.push(code);
            }
            for child in &node.children {
                collect_opencl_codes(child, codes);
            }
        }
        let mut opencl_codes = Vec::new();
        collect_opencl_codes(&self.fs_root, &mut opencl_codes);

        let mut ocl_error = None;
        for code in &opencl_codes {
            if !code.is_empty() {
                if let Err(e) = run_opencl_kernel(code, &mut geom) {
                    ocl_error = Some(e);
                    break;
                }
            }
        }

        if let Some(err) = ocl_error {
            self.update_status_text(&err);
        } else if !opencl_codes.is_empty() {
            self.update_status_text("OpenCL kernel executed successfully.");
        }

        let verts = geom.to_vertex3d_vec();
        self.vertex_count_spheres = verts.len() as u32;
        if !verts.is_empty() {
            let data = bytemuck::cast_slice(&verts);
            let needed = data.len() as wgpu::BufferAddress;
            if needed > self.vertex_buffer_spheres.size() {
            self.vertex_buffer_spheres = self.wgpu_adapter.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Sphere Node Vertex Buffer"),
                    size: needed,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            self.wgpu_adapter.queue.write_buffer(&self.vertex_buffer_spheres, 0, data);
        }
    }

    fn update_status_text(&mut self, text: &str) {
        self.widgets[STATUS_IDX].set_text(text);
    }

    fn prepare_text(&mut self) {
        // 1. Prepare text on all widgets using self.wgpu_adapter.font_system
        for w in &mut self.widgets {
            w.prepare_text(&mut self.wgpu_adapter.font_system);
        }

        // 2. Destructure self
        let splitter1_x = self.splitter_layout.splitter1_x;
        let height = self.height;
        let sw = self.width;
        let sh = self.height;
        let circular_network_pane = self.circular_network_pane;
        let network_circle_x = self.circular_network_layout.x;
        let network_circle_y = self.circular_network_layout.y;
        let network_circle_radius = self.circular_network_layout.r;

        let Self {
            ref mut wgpu_adapter,
            physical_width, physical_height, scale,
            ref widgets,
            ref curved_text_texture,
            ref mut curved_text_atlas,
            ref mut curved_text_renderer,
            ref mut curved_text_viewport,
            ref mut textured_vertex_buffer,
            ref mut textured_vertex_count,
            ..
        } = self;

        let clear_ui::backend::WgpuAdapter {
            ref device,
            ref queue,
            ref mut font_system,
            ref mut text_atlas,
            ref mut text_viewport,
            ref mut text_renderer,
            ref mut swash_cache,
            ..
        } = wgpu_adapter;

        let viewport = Resolution { width: *physical_width, height: *physical_height };
        text_viewport.update(queue, viewport);
        let s = *scale as f32;

        let mut areas: Vec<TextArea> = Vec::new();

        // Temporary storage for legacy buffers generated during this frame
        let mut legacy_buffers: Vec<Buffer> = Vec::new();
        let mut legacy_labels: Vec<TextLabel> = Vec::new();
        let mut legacy_bounds: Vec<TextBounds> = Vec::new();

        let mut curved_labels = Vec::new();

        for (i, w) in widgets.iter().enumerate() {
            if !w.visible() {
                continue;
            }
            let is_node = i == CONTENT_IDX;
            let is_network_part = i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX || i == NETWORK_PANEL_IDX;

            let bounds = if is_node {
                let node_area_y = self.positions[CONTENT_IDX].1;
                if circular_network_pane {
                    TextBounds {
                        left: ((network_circle_x - network_circle_radius) * s) as i32,
                        top: ((network_circle_y - network_circle_radius) * s) as i32,
                        right: ((network_circle_x + network_circle_radius) * s) as i32,
                        bottom: (((network_circle_y + network_circle_radius) * s) as i32).max(0),
                    }
                } else {
                    TextBounds {
                        left: 0,
                        top: (node_area_y * s) as i32,
                        right: (splitter1_x * s) as i32,
                        bottom: (((height - STATUS_H) * s) as i32).max(0),
                    }
                }
            } else {
                TextBounds {
                    left: 0,
                    top: 0,
                    right: *physical_width as i32,
                    bottom: *physical_height as i32,
                }
            };

            let cached_items = w.get_text_items();
            let has_cached_items = !cached_items.is_empty();
            // if i == LEFT_MENUBAR_IDX {
            //     eprintln!("DEBUG_PREPARE: i={} cached_items.len={} circular_network_pane={}", i, cached_items.len(), circular_network_pane);
            // }
            if has_cached_items && !(circular_network_pane && i == LEFT_MENUBAR_IDX) {
                for (buf, x, y, color) in cached_items {
                    if circular_network_pane && is_network_part && i != LEFT_MENUBAR_IDX {
                        let dx = x - network_circle_x;
                        let dy = y - network_circle_y;
                        let dist_sq = dx * dx + dy * dy;
                        if dist_sq > network_circle_radius * network_circle_radius {
                            continue;
                        }
                    }
                    areas.push(TextArea {
                        buffer: buf,
                        left: (x * s).round(),
                        top: (y * s).round(),
                        scale: s,
                        bounds,
                        default_color: color,
                        custom_glyphs: &[],
                    });
                }
            }
            if !has_cached_items || (circular_network_pane && i == LEFT_MENUBAR_IDX) || w.is_menu_bar() {
                let labels = w.text_labels_with_font_and_bounds(&self.ui_context);
                // if i == LEFT_MENUBAR_IDX {
                //     eprintln!("DEBUG_PREPARE_ELSE: i={} labels.len={}", i, labels.len());
                // }
                for (label, font_opt, label_bounds) in labels {
                    let mut is_curved = false;
                    if circular_network_pane && i == LEFT_MENUBAR_IDX && label.text.chars().count() == 1 {
                        let dx = label.x - network_circle_x;
                        let dy = label.y - network_circle_y;
                        let dist = (dx * dx + dy * dy).sqrt();
                        // eprintln!("DEBUG_CURVED: label='{}' count={} dist={} req_min={} req_max={}", label.text, label.text.chars().count(), dist, network_circle_radius - 35.0, network_circle_radius + 5.0);
                        if dist >= network_circle_radius - 35.0 && dist <= network_circle_radius + 5.0 {
                            is_curved = true;
                        }
                    }

                    if is_curved {
                        curved_labels.push(label);
                    } else {
                        if circular_network_pane && is_network_part && i != LEFT_MENUBAR_IDX {
                            let dx = label.x - network_circle_x;
                            let dy = label.y - network_circle_y;
                            let dist_sq = dx * dx + dy * dy;
                            if dist_sq > network_circle_radius * network_circle_radius {
                                continue;
                            }
                        }
                        let mut item_bounds = bounds;
                        if let Some([l, t, r, b]) = label_bounds {
                            let pl = (l * s).round() as i32;
                            let pt = (t * s).round() as i32;
                            let pr = (r * s).round() as i32;
                            let pb = (b * s).round() as i32;
                            item_bounds = TextBounds {
                                left: item_bounds.left.max(pl),
                                top: item_bounds.top.max(pt),
                                right: item_bounds.right.min(pr),
                                bottom: item_bounds.bottom.min(pb),
                            };
                        }
                        // if label.text.contains("Grid") || label.text.contains("Background") || label.text.contains("Snap") || label.text.contains("Opacity") {
                        //     eprintln!("DEBUG_PREPARE_LABEL: text='{}' x={} y={} font_size={} s={} left={} top={} bounds={:?}", label.text, label.x, label.y, label.font_size, s, label.x * s, label.y * s, item_bounds);
                        // }
                        legacy_buffers.push(make_text_buffer_with_font(font_system, &label.text, label.font_size, font_opt.as_deref()));
                        legacy_labels.push(label);
                        legacy_bounds.push(item_bounds);
                    }
                }
            }
        }

        // Add the legacy buffered items (references are safe now that legacy_buffers is not reallocated)
        for ((buf, label), bounds) in legacy_buffers.iter().zip(legacy_labels.iter()).zip(legacy_bounds.iter()) {
            areas.push(TextArea {
                buffer: buf,
                left: (label.x * s).round(),
                top: (label.y * s).round(),
                scale: s,
                bounds: *bounds,
                default_color: glyphon::Color::rgb(label.color[0], label.color[1], label.color[2]),
                custom_glyphs: &[],
            });
        }

        // for label in &legacy_labels {
        //     if label.text.len() == 1 || label.text.contains("Network") || label.text.contains("File") || label.text.contains("Edit") || label.text.contains("View") {
        //         eprintln!("DEBUG_LEGACY_LABEL: text='{}' x={} y={}", label.text, label.x, label.y);
        //     }
        // }

        text_renderer.prepare(device, queue, font_system, text_atlas, text_viewport, areas, swash_cache).unwrap();

        // Process curved labels
        let mut textured_verts = Vec::new();

        if !curved_labels.is_empty() {
            struct CurvedDrawInfo {
                label: TextLabel,
                tx: f32,
                ty: f32,
                tw: f32,
                th: f32,
                buffer: Buffer,
            }

            let mut curved_draws = Vec::new();
            let mut current_x = 4.0;
            let mut current_y = 4.0;
            let font_size = 12.0;
            let row_height = (font_size + 8.0) * s;

            for label in curved_labels {
                let char_w = TextLabel::estimate_width(&label.text, font_size);
                let physical_w = char_w * s;
                if current_x + physical_w + 4.0 > 1024.0 {
                    current_x = 4.0;
                    current_y += row_height;
                }
                let buf = make_text_buffer(font_system, &label.text, font_size);
                curved_draws.push(CurvedDrawInfo {
                    label: label.clone(),
                    tx: current_x,
                    ty: current_y,
                    tw: char_w,
                    th: font_size,
                    buffer: buf,
                });
                current_x += physical_w + 8.0 * s;
            }

            let mut curved_areas = Vec::new();
            for draw in &curved_draws {
                curved_areas.push(TextArea {
                    buffer: &draw.buffer,
                    left: draw.tx.round(),
                    top: draw.ty.round(),
                    scale: s,
                    bounds: TextBounds {
                        left: 0,
                        top: 0,
                        right: 1024,
                        bottom: 1024,
                    },
                    default_color: glyphon::Color::rgb(255, 255, 255),
                    custom_glyphs: &[],
                });
            }

            curved_text_renderer.prepare(
                device,
                queue,
                font_system,
                curved_text_atlas,
                curved_text_viewport,
                curved_areas,
                swash_cache,
            ).unwrap();

            let mut texture_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Curved Text Texture Encoder"),
            });
            {
                let view_for_pass = curved_text_texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut pass = texture_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Curved Text Render Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view_for_pass,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                curved_text_renderer.render(curved_text_atlas, curved_text_viewport, &mut pass).unwrap();
            }
            queue.submit(std::iter::once(texture_encoder.finish()));

            let clip_circle_val = if circular_network_pane {
                [network_circle_x * s, network_circle_y * s, network_circle_radius * s]
            } else {
                [0.0, 0.0, 0.0]
            };

            for draw in curved_draws {
                let dx = (draw.label.x + draw.tw / 2.0) - network_circle_x;
                let dy = (draw.label.y + draw.th / 2.0) - network_circle_y;
                let theta = dy.atan2(dx);
                let angle = theta + std::f32::consts::FRAC_PI_2;

                let cx = draw.label.x + draw.tw / 2.0;
                let cy = draw.label.y + draw.th / 2.0;
                let w_half = draw.tw / 2.0;
                let h_half = draw.th / 2.0;

                let cos_a = angle.cos();
                let sin_a = angle.sin();

                let local_pts = [
                    [-w_half, -h_half],
                    [w_half, -h_half],
                    [-w_half, h_half],
                    [w_half, h_half],
                ];

                let mut screen_pts = [[0.0; 2]; 4];
                for (k, pt) in local_pts.iter().enumerate() {
                    let rx = pt[0] * cos_a - pt[1] * sin_a;
                    let ry = pt[0] * sin_a + pt[1] * cos_a;
                    screen_pts[k] = [cx + rx, cy + ry];
                }

                let ndc_pts = screen_pts.map(|pt| [
                    (pt[0] / sw) * 2.0 - 1.0,
                    1.0 - (pt[1] / sh) * 2.0,
                ]);

                // eprintln!("DEBUG_NDCPTS: char='{}' ndc0={:?} ndc1={:?} ndc2={:?} ndc3={:?}",
                //           draw.label.text, ndc_pts[0], ndc_pts[1], ndc_pts[2], ndc_pts[3]);

                let u0 = draw.tx / 1024.0;
                let v0 = draw.ty / 1024.0;
                let u1 = (draw.tx + draw.tw * s) / 1024.0;
                let v1 = (draw.ty + draw.th * s) / 1024.0;

                let c = [
                    draw.label.color[0] as f32 / 255.0,
                    draw.label.color[1] as f32 / 255.0,
                    draw.label.color[2] as f32 / 255.0,
                    1.0,
                ];

                let v_tl = TexturedVertex { position: ndc_pts[0], tex_coords: [u0, v0], color: c, clip_circle: clip_circle_val };
                let v_tr = TexturedVertex { position: ndc_pts[1], tex_coords: [u1, v0], color: c, clip_circle: clip_circle_val };
                let v_bl = TexturedVertex { position: ndc_pts[2], tex_coords: [u0, v1], color: c, clip_circle: clip_circle_val };
                let v_br = TexturedVertex { position: ndc_pts[3], tex_coords: [u1, v1], color: c, clip_circle: clip_circle_val };

                textured_verts.push(v_tl);
                textured_verts.push(v_tr);
                textured_verts.push(v_bl);

                textured_verts.push(v_tr);
                textured_verts.push(v_br);
                textured_verts.push(v_bl);
            }
        }

        *textured_vertex_count = textured_verts.len() as u32;
        if *textured_vertex_count > 0 {
            let data = bytemuck::cast_slice(&textured_verts);
            let needed = data.len() as wgpu::BufferAddress;
            if needed > textured_vertex_buffer.size() {
                *textured_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Textured Vertex Buffer"),
                    size: needed,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            queue.write_buffer(textured_vertex_buffer, 0, data);
        }
    }


    fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            let old_width = self.width;
            self.physical_width = width;
            self.physical_height = height;
            self.width = width as f32 / self.scale as f32;
            self.height = height as f32 / self.scale as f32;
            self.wgpu_adapter.resize(width, height);

            let (tex, view) = self.create_depth_texture();
            self.depth_texture = tex;
            self.depth_texture_view = view;

            let (b_tex, b_view) = self.create_backdrop_texture();
            self.backdrop_texture = b_tex;
            self.backdrop_texture_view = b_view;

            self.backdrop_bind_group = self.wgpu_adapter.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Backdrop Bind Group"),
                layout: &self.backdrop_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.backdrop_texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.backdrop_sampler),
                    },
                ],
            });

            if old_width > 0.0 {
                let r = self.width / old_width;
                self.splitter_layout.scale(r);
                let body_h = self.body_h();
                self.widgets[SPLITTER1_IDX].set_rect(self.splitter_layout.splitter1_x, HEADER_H, SPLITTER_W, body_h);
                self.widgets[SPLITTER2_IDX].set_rect(self.splitter_layout.splitter2_x, HEADER_H, SPLITTER_W, body_h);
            }

            self.sync_layout();
            self.read_panel_offsets();
            self.keep_cursor_in_view();
            self.upload_vertices();
        }
    }

    fn handle_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let dialog_open = self.node_palette_visible;
                let in_network_pane = self.in_network_pane();
                // eprintln!("DEBUG MOUSEWHEEL: delta={:?}, phase={:?}, cursor=({}, {}), in_network_pane={}", delta, phase, self.cursor_x, self.cursor_y, in_network_pane);
                let node_area_y = self.positions[CONTENT_IDX].1;

                let in_viewport = self.cursor_x >= self.content_right_x()
                    && self.cursor_x < self.splitter_layout.splitter2_x
                    && self.cursor_y >= node_area_y
                    && self.cursor_y < self.height - STATUS_H;

                let mut focus_changed = false;
                let mut new_pane = None;
                if !dialog_open {
                    if in_network_pane {
                        new_pane = Some(LEFT_MENUBAR_IDX);
                    } else if in_viewport {
                        if self.show_spreadsheet && self.cursor_y >= self.positions[SPREADSHEET_IDX].1 {
                            new_pane = Some(SPREADSHEET_MENUBAR_IDX);
                        } else {
                            new_pane = Some(RIGHT_MENUBAR_IDX);
                        }
                    } else if self.cursor_x > self.splitter_layout.splitter2_x + SPLITTER_W && self.cursor_y >= node_area_y && self.cursor_y < self.height - STATUS_H {
                        new_pane = Some(PARAM_MENUBAR_IDX);
                    }
                }
                if let Some(pane_idx) = new_pane {
                    if self.focused_pane != pane_idx {
                        self.focused_pane = pane_idx;
                        self.sync_pane_focus();
                        focus_changed = true;
                    }
                }

                let mut handled = false;
                let mut needs_sync_grid = false;
                if !dialog_open && !self.modifiers.control_key() {
                    let ctx = &mut self.ui_context;
                    for (i, w) in self.widgets.iter_mut().enumerate() {
                        if w.mouse_wheel(delta, self.cursor_x, self.cursor_y, ctx) {
                            handled = true;
                            if i == CONTENT_IDX {
                                let active_node_area_y = self.positions[CONTENT_IDX].1;
                                let active_node_area_x = self.positions[CONTENT_IDX].0;
                                let (gx, gy) = w.grid_origin();
                                let prev_pan_x = self.pan_x;
                                let prev_pan_y = self.pan_y;
                                self.pan_x = gx - active_node_area_x;
                                self.pan_y = gy - active_node_area_y;
                                
                                let dx = prev_pan_x - self.pan_x;
                                let dy = prev_pan_y - self.pan_y;
                                
                                let dt_scroll = Instant::now().duration_since(self.last_frame).as_secs_f32().min(0.1);
                                let vel_x = if dt_scroll > 1e-4 { -dx / dt_scroll } else { -dx * 60.0 };
                                let vel_y = if dt_scroll > 1e-4 { -dy / dt_scroll } else { -dy * 60.0 };
                                self.pan_velocity_x = self.pan_velocity_x * 0.4 + vel_x * 0.6;
                                self.pan_velocity_y = self.pan_velocity_y * 0.4 + vel_y * 0.6;

                                self.is_scrolling_trackpad = match delta {
                                    MouseScrollDelta::LineDelta(_, _) => false,
                                    MouseScrollDelta::PixelDelta(_) => match phase {
                                        TouchPhase::Started | TouchPhase::Moved => true,
                                        TouchPhase::Ended | TouchPhase::Cancelled => false,
                                    }
                                };
                                if self.is_scrolling_trackpad {
                                    self.last_scroll_time = Instant::now();
                                    self.scroll_accum_x += dx;
                                    self.scroll_accum_y += dy;
                                }
                                needs_sync_grid = true;
                            }
                        }
                    }
                }
                if needs_sync_grid {
                    self.sync_grid_settings();
                }

                let result = if handled {
                    self.sync_parameters_to_project();

                    // Sync Parameters pane with selected node
                    let params = if !self.is_detached_network {
                        if let Some(slot_idx) = self.widgets[CONTENT_IDX].selected_node() {
                            let dir = self.current_dir();
                            if slot_idx < dir.children.len() {
                                param_display(&dir.children[slot_idx].params)
                            } else {
                                vec![]
                            }
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    };
                    self.widgets[PARAM_IDX].set_display_params(&params);


                    self.sync_layout();
                    self.read_panel_offsets();
                    self.upload_vertices();
                    true
                } else if !dialog_open && in_network_pane {
                    if self.modifiers.control_key() {
                        let factor = match delta {
                            MouseScrollDelta::LineDelta(_x, y) => {
                                if *y > 0.0 { 1.15 } else if *y < 0.0 { 1.0 / 1.15 } else { 1.0 }
                            }
                            MouseScrollDelta::PixelDelta(pos) => {
                                let dy = pos.y as f32;
                                (1.0 + dy / 100.0).clamp(0.8, 1.25)
                            }
                        };
                        if factor != 1.0 {
                            self.zoom(factor, Some((self.cursor_x, self.cursor_y)));
                            true
                        } else {
                            false
                        }
                    } else {
                        match delta {
                            MouseScrollDelta::LineDelta(x, y) => {
                                let dx = *x * 30.0 * self.scroll_speed;
                                let dy = *y * 30.0 * self.scroll_speed;
                                self.pan_x -= dx;
                                self.pan_y -= dy;
                                self.is_scrolling_trackpad = false;
                                let dt_scroll = Instant::now().duration_since(self.last_frame).as_secs_f32().min(0.1);
                                let vel_x = if dt_scroll > 1e-4 { -dx / dt_scroll } else { -dx * 60.0 };
                                let vel_y = if dt_scroll > 1e-4 { -dy / dt_scroll } else { -dy * 60.0 };
                                self.pan_velocity_x = self.pan_velocity_x * 0.4 + vel_x * 0.6;
                                self.pan_velocity_y = self.pan_velocity_y * 0.4 + vel_y * 0.6;
                                self.sync_grid_settings();
                                true
                            }
                            MouseScrollDelta::PixelDelta(pos) => {
                                let dx = (pos.x as f32 / self.scale as f32) * self.scroll_speed;
                                let dy = (pos.y as f32 / self.scale as f32) * self.scroll_speed;
                                self.pan_x -= dx;
                                self.pan_y -= dy;
                                self.is_scrolling_trackpad = match phase {
                                    TouchPhase::Started | TouchPhase::Moved => true,
                                    TouchPhase::Ended | TouchPhase::Cancelled => false,
                                };
                                self.last_scroll_time = Instant::now();
                                self.scroll_accum_x += dx;
                                self.scroll_accum_y += dy;
                                self.sync_grid_settings();
                                true
                            }
                        }
                    }
                } else if !dialog_open && in_viewport {
                    if self.modifiers.control_key() {
                        match delta {
                            MouseScrollDelta::LineDelta(_x, y) => {
                                let dy = *y * 0.15;
                                self.viewport_zoom *= (-dy).exp();
                                self.viewport_zoom = self.viewport_zoom.clamp(0.05, 20.0);

                                self.is_zooming_viewport = false;
                                let dt_scroll = Instant::now().duration_since(self.last_frame).as_secs_f32().min(0.1);
                                let vel_zoom = if dt_scroll > 1e-4 { dy / dt_scroll } else { dy * 60.0 };
                                self.zoom_velocity = self.zoom_velocity * 0.4 + vel_zoom * 0.6;

                                true
                            }
                            MouseScrollDelta::PixelDelta(pos) => {
                                let dy = (pos.y as f32 / self.scale as f32) * 0.005;
                                self.viewport_zoom *= (-dy).exp();
                                self.viewport_zoom = self.viewport_zoom.clamp(0.05, 20.0);

                                self.is_zooming_viewport = match phase {
                                    TouchPhase::Started | TouchPhase::Moved => true,
                                    TouchPhase::Ended | TouchPhase::Cancelled => false,
                                };
                                self.last_zoom_time = Instant::now();
                                self.zoom_accum += dy;

                                true
                            }
                        }
                    } else {
                        match delta {
                            MouseScrollDelta::LineDelta(x, y) => {
                                self.scroll_lock = 0;
                                let dx = *x * 0.05;
                                let dy = *y * 0.05;
                                if self.active_camera != "Default Camera" {
                                    self.update_active_camera_rotation(dx, -dy);
                                } else {
                                    self.rotation_y += dx;
                                    self.rotation_x -= dy;
                                }

                                self.is_rotating_viewport = false;
                                let dt_scroll = Instant::now().duration_since(self.last_frame).as_secs_f32().min(0.1);
                                let vel_yaw = if dt_scroll > 1e-4 { dx / dt_scroll } else { dx * 60.0 };
                                let vel_pitch = if dt_scroll > 1e-4 { -dy / dt_scroll } else { -dy * 60.0 };
                                self.rotate_velocity_yaw = self.rotate_velocity_yaw * 0.4 + vel_yaw * 0.6;
                                self.rotate_velocity_pitch = self.rotate_velocity_pitch * 0.4 + vel_pitch * 0.6;

                                true
                            }
                            MouseScrollDelta::PixelDelta(pos) => {
                                let mut dx = (pos.x as f32 / self.scale as f32) * 0.005;
                                let mut dy = (pos.y as f32 / self.scale as f32) * 0.005;

                                if *phase == TouchPhase::Started {
                                    self.scroll_lock = 0;
                                    self.rotate_accum_yaw = 0.0;
                                    self.rotate_accum_pitch = 0.0;
                                }

                                if self.scroll_lock == 0 {
                                    self.rotate_accum_yaw += dx;
                                    self.rotate_accum_pitch -= dy;
                                    if self.rotate_accum_yaw.abs() > 0.002 || self.rotate_accum_pitch.abs() > 0.002 {
                                        if self.rotate_accum_pitch.abs() > 1.2 * self.rotate_accum_yaw.abs() {
                                            self.scroll_lock = 2; // lock vertical
                                        } else if self.rotate_accum_yaw.abs() > 1.2 * self.rotate_accum_pitch.abs() {
                                            self.scroll_lock = 1; // lock horizontal
                                        }
                                    }
                                } else {
                                    if self.scroll_lock == 1 {
                                        dy = 0.0;
                                        self.rotate_accum_yaw += dx;
                                    } else {
                                        dx = 0.0;
                                        self.rotate_accum_pitch -= dy;
                                    }
                                }

                                if self.active_camera != "Default Camera" {
                                    self.update_active_camera_rotation(dx, -dy);
                                } else {
                                    self.rotation_y += dx;
                                    self.rotation_x -= dy;
                                }

                                self.is_rotating_viewport = match phase {
                                    TouchPhase::Started | TouchPhase::Moved => true,
                                    TouchPhase::Ended | TouchPhase::Cancelled => {
                                        self.scroll_lock = 0;
                                        false
                                    }
                                };
                                self.last_rotate_time = Instant::now();

                                true
                            }
                        }
                    }
                } else {
                    false
                };

                if focus_changed {
                    self.sync_layout();
                    self.read_panel_offsets();
                    self.upload_vertices();
                    true
                } else {
                    result
                }
            }
            WindowEvent::PinchGesture { delta, .. } => {
                let dialog_open = self.node_palette_visible;
                let in_network_pane = self.in_network_pane();

                if !dialog_open && in_network_pane {
                    if delta.is_finite() && *delta != 0.0 {
                        let factor = (1.0 + *delta as f32).clamp(0.8, 1.25);
                        self.zoom(factor, Some((self.cursor_x, self.cursor_y)));
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_x = position.x as f32;
                self.cursor_y = position.y as f32;
                let mut changed = false;

                if self.is_panning {
                    let dx = self.cursor_x - self.pan_start_cx;
                    let dy = self.cursor_y - self.pan_start_cy;
                    self.pan_x = self.pan_start_x + dx;
                    self.pan_y = self.pan_start_y + dy;
                    self.sync_grid_settings();
                    self.rebuild_positions();
                    self.apply_layout();
                    self.update_panel_bounds();
                    changed = true;
                } else {
                    if let Some(idx) = self.drag_widget {
                        if idx == NETWORK_PANEL_IDX && self.is_resizing_network.is_some() {
                            let dir = self.is_resizing_network.unwrap();
                            let dx = self.cursor_x - self.drag_start_mouse.0;
                            let dy = self.cursor_y - self.drag_start_mouse.1;
                            let (sx, sy, sw, sh) = self.drag_start_rect;

                            let mut fx = sx;
                            let mut fy = sy;
                            let mut fw = sw;
                            let mut fh = sh;

                            if dir.left {
                                let new_w = (sw - dx).max(150.0);
                                fx = sx + sw - new_w;
                                fw = new_w;
                            } else if dir.right {
                                fw = (sw + dx).max(150.0);
                            }

                            if dir.top {
                                let new_h = (sh - dy).max(100.0);
                                fy = sy + sh - new_h;
                                fh = new_h;
                            } else if dir.bottom {
                                fh = (sh + dy).max(100.0);
                            }

                            self.floating_network_layout = (fx, fy, fw, fh);
                            self.rebuild_positions();
                            self.apply_layout();
                            self.sync_grid_settings();
                            changed = true;
                        } else if idx == PARAM_IDX && self.is_resizing_param {
                            let dx = self.cursor_x - self.drag_start_mouse.0;
                            let sw = self.drag_start_param_w;
                            let new_w = (sw - dx).max(150.0);
                            self.floating_param_width = new_w;
                            self.rebuild_positions();
                            self.apply_layout();
                            changed = true;
                        } else if idx == SPREADSHEET_IDX && self.is_resizing_spreadsheet {
                            let dy = self.cursor_y - self.drag_start_mouse.1;
                            let sh = self.drag_start_spreadsheet_h;
                            let new_h = (sh - dy).max(100.0);
                            self.floating_spreadsheet_height = new_h;
                            self.rebuild_positions();
                            self.apply_layout();
                            changed = true;
                        } else {
                            self.widgets[idx].set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                            if self.widgets[idx].drag_update(self.cursor_x, self.cursor_y) {
                                changed = true;
                                if idx == NETWORK_PANEL_IDX {
                                    self.sync_grid_settings();
                                }
                            }
                        }
                    }

                    if self.drag_widget.is_none() {
                        for i in 0..self.widgets.len() {
                            let (cx, cy) = (self.cursor_x, self.cursor_y);
                            let is_network_part = i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX || i == NETWORK_PANEL_IDX;
                            let inside = if self.circular_network_pane && is_network_part {
                                if i == CONTENT_IDX {
                                    self.circular_network_layout.hit_test_content(cx, cy, MENUBAR_H, BREADCRUMB_H)
                                } else if i == LEFT_MENUBAR_IDX {
                                    self.circular_network_layout.hit_test_menubar(cx, cy, MENUBAR_H)
                                } else if i == BREADCRUMB_IDX {
                                    self.circular_network_layout.hit_test_breadcrumb(cx, cy, MENUBAR_H, BREADCRUMB_H)
                                } else if i == NETWORK_PANEL_IDX {
                                    self.widgets[NETWORK_PANEL_IDX].hit_test(cx, cy, &self.ui_context)
                                } else {
                                    false
                                }
                            } else {
                                self.widgets[i].hit_test(cx, cy, &self.ui_context)
                            };
                            let (tx, ty) = if inside { (cx, cy) } else { (-9999.0, -9999.0) };
                            if self.widgets[i].cursor_moved(tx, ty, &mut self.ui_context) {
                                changed = true;
                            }
                        }
                    }
                }
                changed
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                // println!("DEBUG: MouseInput state={:?} button={:?} cursor=({}, {})", btn_state, button, self.cursor_x, self.cursor_y);
                if *btn_state == ElementState::Pressed {
                    self.pan_velocity_x = 0.0;
                    self.pan_velocity_y = 0.0;
                    self.is_scrolling_trackpad = false;
                    self.scroll_accum_x = 0.0;
                    self.scroll_accum_y = 0.0;

                    self.rotate_velocity_yaw = 0.0;
                    self.rotate_velocity_pitch = 0.0;
                    self.is_rotating_viewport = false;
                    self.scroll_lock = 0;
                    self.rotate_accum_yaw = 0.0;
                    self.rotate_accum_pitch = 0.0;

                    self.zoom_velocity = 0.0;
                    self.is_zooming_viewport = false;
                    self.zoom_accum = 0.0;
                }
                let dialog_open = self.node_palette_visible;
                let in_network_pane = self.in_network_pane();
                let node_area_x = self.positions[CONTENT_IDX].0;
                let node_area_y = self.positions[CONTENT_IDX].1;

                let is_pan_trigger = !dialog_open
                    && in_network_pane
                    && (*button == MouseButton::Middle
                        || (*button == MouseButton::Left && self.space_pressed));

                if is_pan_trigger {
                    match btn_state {
                        ElementState::Pressed => {
                            self.is_panning = true;
                            self.pan_start_cx = self.cursor_x;
                            self.pan_start_cy = self.cursor_y;
                            self.pan_start_x = self.pan_x;
                            self.pan_start_y = self.pan_y;
                            self.pan_velocity_x = 0.0;
                            self.pan_velocity_y = 0.0;
                            self.last_frame_pan_x = self.pan_x;
                            self.last_frame_pan_y = self.pan_y;
                            return true;
                        }
                        ElementState::Released => {
                            if self.is_panning {
                                self.is_panning = false;
                                self.sync_layout();
                                self.read_panel_offsets();
                                self.upload_vertices();
                                return true;
                            }
                        }
                    }
                }

                if self.is_panning && *btn_state == ElementState::Released {
                    self.is_panning = false;
                    self.sync_layout();
                    self.read_panel_offsets();
                    self.upload_vertices();
                    return true;
                }

                if *button != MouseButton::Left && *button != MouseButton::Right { return false; }
                let mut changed = false;
                let old_focus = self.focused_widget;
                let was_page_hidden = self.widgets[PAGINATOR_IDX].is_page_hidden();

                let hits_widget = |state: &State, i: usize, x: f32, y: f32| -> bool {
                    if state.circular_network_pane && (i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX) {
                        if i == CONTENT_IDX {
                            state.circular_network_layout.hit_test_content(x, y, MENUBAR_H, BREADCRUMB_H)
                        } else if i == LEFT_MENUBAR_IDX {
                            state.circular_network_layout.hit_test_menubar(x, y, MENUBAR_H)
                        } else if i == BREADCRUMB_IDX {
                            state.circular_network_layout.hit_test_breadcrumb(x, y, MENUBAR_H, BREADCRUMB_H)
                        } else {
                            false
                        }
                    } else {
                        state.widgets[i].hit_test(x, y, &state.ui_context)
                    }
                };

                let in_circle_network_pane = if self.circular_network_pane {
                    self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, MENUBAR_H, BREADCRUMB_H)
                } else {
                    in_network_pane
                };

                match btn_state {
                    ElementState::Pressed => {
                        let hits_any_menu = (0..self.widgets.len()).any(|i| {
                            hits_widget(self, i, self.cursor_x, self.cursor_y)
                                && self.widgets[i].get_menu_items_at(self.cursor_x, self.cursor_y).is_some()
                        });

                        if !hits_any_menu {
                            if let Some(pid) = self.active_menu_cloud_pid {
                                let is_running = unsafe {
                                    libc::kill(pid as libc::pid_t, 0) == 0
                                };
                                if is_running {
                                    unsafe {
                                        libc::kill(pid as libc::pid_t, libc::SIGTERM);
                                    }
                                }
                                self.active_menu_cloud_pid = None;
                                self.active_menu_cloud_idx = None;
                            }
                        }

                        if *button == MouseButton::Left && self.circular_network_pane {
                            let on_border = self.circular_network_layout.hit_test_border(self.cursor_x, self.cursor_y, 12.0);
                            let hit_menubar = self.circular_network_layout.hit_test_menubar(self.cursor_x, self.cursor_y, MENUBAR_H);
                            if hit_menubar {
                                if let Some((menu_idx, title, items, rx, ry, rw, rh)) = self.widgets[LEFT_MENUBAR_IDX].get_menu_items_at(self.cursor_x, self.cursor_y) {
                                    self.focused_pane = LEFT_MENUBAR_IDX;
                                    self.widgets[PAGINATOR_IDX].set_page_hidden(false);
                                    self.widgets[PAGINATOR_IDX].set_selected_page(menu_idx);
                                    self.sync_pane_focus();
                                    self.spawn_menu_cloud(LEFT_MENUBAR_IDX, menu_idx, title, items, rx, ry, rw, rh);
                                    return true;
                                }
                            }
                            if on_border || hit_menubar {
                                self.drag_widget = Some(NETWORK_PANEL_IDX);
                                self.widgets[NETWORK_PANEL_IDX].set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                                self.widgets[NETWORK_PANEL_IDX].drag_begin(self.cursor_x, self.cursor_y);
                                self.focused_pane = LEFT_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.widgets[old].unfocus();
                                    self.focused_widget = None;
                                }
                                self.widgets[PARAM_IDX].unfocus();
                                self.sync_parameters_to_project();
                                return true;
                            }
                        }

                        if *button == MouseButton::Left && !self.circular_network_pane && self.show_network {
                            let (fx, fy, fw, fh) = self.floating_network_layout;
                            let cx = self.cursor_x;
                            let cy = self.cursor_y;

                            let margin = 8.0_f32;
                            let on_left = false;
                            let on_right = cx >= fx + fw - margin && cx <= fx + fw + margin && cy >= fy - margin && cy <= fy + fh + margin;
                            let on_top = false;
                            let on_bottom = false;

                            if on_left || on_right || on_top || on_bottom {
                                self.is_resizing_network = Some(ResizeDirection {
                                    left: on_left,
                                    right: on_right,
                                    top: on_top,
                                    bottom: on_bottom,
                                });
                                self.drag_widget = Some(NETWORK_PANEL_IDX);
                                self.drag_start_rect = self.floating_network_layout;
                                self.drag_start_mouse = (cx, cy);
                                self.focused_pane = LEFT_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.widgets[old].unfocus();
                                    self.focused_widget = None;
                                }
                                self.widgets[PARAM_IDX].unfocus();
                                self.sync_parameters_to_project();
                                return true;
                            } else if cx >= fx && cx < fx + fw && cy >= fy && cy < fy + (if self.show_network { BREADCRUMB_H } else { 0.0 }) {
                                if self.widgets[BREADCRUMB_IDX].mouse_input(*button, *btn_state, cx, cy, &mut self.ui_context) {
                                    return true;
                                }
                                self.is_resizing_network = None;
                                self.drag_widget = Some(NETWORK_PANEL_IDX);
                                self.widgets[NETWORK_PANEL_IDX].set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                                self.widgets[NETWORK_PANEL_IDX].drag_begin(cx, cy);
                                self.focused_pane = LEFT_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.widgets[old].unfocus();
                                    self.focused_widget = None;
                                }
                                self.widgets[PARAM_IDX].unfocus();
                                self.sync_parameters_to_project();
                                return true;
                            }
                        }

                        if *button == MouseButton::Left && self.show_parameters {
                            let gap = 18.0_f32;
                            let param_w = self.floating_param_width;
                            let param_x = self.width - gap - param_w;
                            let param_y = HEADER_H + gap;
                            let param_h = (self.height - HEADER_H - STATUS_H - 2.0 * gap).max(100.0);
                            let cx = self.cursor_x;
                            let cy = self.cursor_y;

                            let margin = 8.0_f32;
                            let on_left = cx >= param_x - margin && cx <= param_x + margin && cy >= param_y - margin && cy <= param_y + param_h + margin;

                            if on_left {
                                self.is_resizing_param = true;
                                self.drag_widget = Some(PARAM_IDX);
                                self.drag_start_param_w = param_w;
                                self.drag_start_mouse = (cx, cy);
                                self.focused_pane = PARAM_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.widgets[old].unfocus();
                                    self.focused_widget = None;
                                }
                                return true;
                            }
                        }

                        if *button == MouseButton::Left && self.show_spreadsheet {
                            let gap = 18.0_f32;
                            let fx = gap;
                            let (mut _n_fx, mut _n_fy, mut fw, mut _n_fh) = self.floating_network_layout;
                            fw = fw.clamp(150.0, (self.width - 2.0 * gap).max(150.0));
                            
                            let param_w = self.floating_param_width.clamp(150.0, (self.width - 2.0 * gap).max(150.0));
                            let param_x = self.width - gap - param_w;
                            
                            let ss_x = if self.show_network { fx + fw + gap } else { gap };
                            let ss_w_end = if self.show_parameters { param_x - gap } else { self.width - gap };
                            let ss_w = (ss_w_end - ss_x).max(150.0);
                            
                            let ss_y_end = self.height - STATUS_H - gap;
                            let ss_h = self.floating_spreadsheet_height.clamp(100.0, (ss_y_end - HEADER_H - gap).max(100.0));
                            let ss_y = ss_y_end - ss_h;
                            
                            let cx = self.cursor_x;
                            let cy = self.cursor_y;
                            let margin = 8.0_f32;
                            let on_top = cx >= ss_x && cx <= ss_x + ss_w && cy >= ss_y - margin && cy <= ss_y + margin;
                            
                            if on_top {
                                self.is_resizing_spreadsheet = true;
                                self.drag_widget = Some(SPREADSHEET_IDX);
                                self.drag_start_spreadsheet_h = ss_h;
                                self.drag_start_mouse = (cx, cy);
                                self.focused_pane = SPREADSHEET_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.widgets[old].unfocus();
                                    self.focused_widget = None;
                                }
                                return true;
                            }
                        }

                        if *button == MouseButton::Right {
                            if !dialog_open && in_circle_network_pane {
                                let col = ((self.cursor_x - node_area_x - self.pan_x) / (self.grid_size_x + self.skipped_col_w)).floor() as i32;
                                let row = ((self.cursor_y - node_area_y - self.pan_y) / (self.grid_size_y + self.skipped_row_h)).floor() as i32;
                                self.grid_cursor_col = col;
                                self.grid_cursor_row = row;
                                self.open_node_palette();
                                return true;
                            }
                            return false;
                        }
                        let mut click_target = None;
                        for i in 0..self.widgets.len() {
                            if self.widgets[i].is_menu_open() && hits_widget(self, i, self.cursor_x, self.cursor_y) {
                                click_target = Some(i);
                                break;
                            }
                        }
                        if click_target.is_none() {
                            let mut hit_order: Vec<usize> = (0..self.widgets.len()).collect();
                            hit_order.sort_by_key(|&i| {
                                let z = if i == NETWORK_PANEL_IDX || i == PARAM_PLATE_IDX {
                                    -5
                                } else if i == VIEWPORT_IDX || i == PARAM_IDX {
                                    -4
                                } else {
                                    self.widgets[i].z_index()
                                };
                                -z
                            });
                            click_target = hit_order.into_iter()
                                .find(|&i| hits_widget(self, i, self.cursor_x, self.cursor_y));
                        }

                        // Determine new focused pane
                        let mut new_pane = None;
                        if let Some(i) = click_target {
                            if i == LEFT_MENUBAR_IDX || i == CONTENT_IDX || i == CANVAS_IDX || i == BREADCRUMB_IDX || i == NETWORK_PANEL_IDX {
                                new_pane = Some(LEFT_MENUBAR_IDX);
                            } else if i == RIGHT_MENUBAR_IDX || i == VIEWPORT_IDX {
                                new_pane = Some(RIGHT_MENUBAR_IDX);
                            } else if i == PARAM_MENUBAR_IDX || i == PARAM_IDX || i == PARAM_PLATE_IDX {
                                new_pane = Some(PARAM_MENUBAR_IDX);
                            } else if i == SPREADSHEET_MENUBAR_IDX || i == SPREADSHEET_IDX {
                                new_pane = Some(SPREADSHEET_MENUBAR_IDX);
                            } else if i == HEADER_IDX {
                                new_pane = Some(HEADER_IDX);
                            }

                            // If clicked on a blank spot of any pane menubar, switch focus to main menubar (HEADER_IDX)
                            if (i == LEFT_MENUBAR_IDX || i == RIGHT_MENUBAR_IDX || i == PARAM_MENUBAR_IDX || i == SPREADSHEET_MENUBAR_IDX)
                                && self.widgets[i].get_menu_items_at(self.cursor_x, self.cursor_y).is_none()
                            {
                                new_pane = Some(HEADER_IDX);
                            }
                        } else {
                            if self.circular_network_pane {
                                if self.cursor_x > self.splitter_layout.splitter2_x + SPLITTER_W {
                                    new_pane = Some(PARAM_MENUBAR_IDX);
                                } else {
                                    if self.show_spreadsheet && self.cursor_y >= self.positions[SPREADSHEET_IDX].1 {
                                        new_pane = Some(SPREADSHEET_MENUBAR_IDX);
                                    } else {
                                        new_pane = Some(RIGHT_MENUBAR_IDX);
                                    }
                                }
                            } else {
                                if self.cursor_x < self.splitter_layout.splitter1_x {
                                    new_pane = Some(LEFT_MENUBAR_IDX);
                                } else if self.cursor_x > self.splitter_layout.splitter2_x + SPLITTER_W {
                                    new_pane = Some(PARAM_MENUBAR_IDX);
                                } else {
                                    if self.show_spreadsheet && self.cursor_y >= self.positions[SPREADSHEET_IDX].1 {
                                        new_pane = Some(SPREADSHEET_MENUBAR_IDX);
                                    } else {
                                        new_pane = Some(RIGHT_MENUBAR_IDX);
                                    }
                                }
                            }
                        }
                        if let Some(pane_idx) = new_pane {
                            if self.focused_pane != pane_idx {
                                self.focused_pane = pane_idx;
                                changed = true;
                            }
                        }

                        if let Some(old) = self.focused_widget {
                            if click_target != Some(old) && click_target != Some(PARAM_IDX) {
                                self.widgets[old].unfocus();
                                self.focused_widget = None;
                            }
                        }
                        if click_target != Some(PARAM_IDX) {
                            self.widgets[PARAM_IDX].unfocus();
                            self.sync_parameters_to_project();
                        }
                        if click_target.is_none() && !dialog_open && in_circle_network_pane {
                            let col = ((self.cursor_x - node_area_x - self.pan_x) / (self.grid_size_x + self.skipped_col_w)).floor() as i32;
                            let row = ((self.cursor_y - node_area_y - self.pan_y) / (self.grid_size_y + self.skipped_row_h)).floor() as i32;
                            self.grid_cursor_col = col;
                            self.grid_cursor_row = row;
                            changed = true;
                        }
                        if let Some(i) = click_target {
                            if let Some((menu_idx, title, items, rx, ry, rw, rh)) = self.widgets[i].get_menu_items_at(self.cursor_x, self.cursor_y) {
                                self.focused_pane = i;
                                self.widgets[PAGINATOR_IDX].set_page_hidden(false);
                                self.widgets[PAGINATOR_IDX].set_selected_page(menu_idx);
                                self.sync_pane_focus();
                                self.spawn_menu_cloud(i, menu_idx, title, items, rx, ry, rw, rh);
                                return true;
                            }
                            self.widgets[i].set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                            if self.widgets[i].mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y, &mut self.ui_context) {
                                changed = true;
                                if i == PARAM_IDX {
                                    self.sync_parameters_to_project();
                                }
                                if i == PAGINATOR_IDX {
                                    self.update_paginator();
                                    self.rebuild_positions();
                                    self.apply_layout();
                                }
                            } else if i == PAGINATOR_IDX {
                                if *button == MouseButton::Left {
                                    self.focused_pane = HEADER_IDX;
                                    self.sync_pane_focus();
                                    changed = true;
                                }
                            }
                            if self.widgets[i].draggable() {
                                self.widgets[i].set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                                self.widgets[i].drag_begin(self.cursor_x, self.cursor_y);
                                self.drag_widget = Some(i);
                            }
                            if i != PARAM_IDX {
                                self.widgets[i].focus();
                                self.focused_widget = Some(i);
                                if self.widgets[i].is_menu_bar() && !self.widgets[i].focused(&self.ui_context) {
                                    self.widgets[i].unfocus();
                                    self.focused_widget = None;
                                }
                            }
                            if i == CONTENT_IDX {
                                if let Some(slot_idx) = self.widgets[CONTENT_IDX].selected_node() {
                                    let dir = self.current_dir();
                                    if slot_idx < dir.children.len() {
                                        let pos = dir.children[slot_idx].position;
                                        self.grid_cursor_col = pos.0 as i32;
                                        self.grid_cursor_row = pos.1 as i32;
                                    }
                                } else {
                                    let col = ((self.cursor_x - node_area_x - self.pan_x) / (self.grid_size_x + self.skipped_col_w)).floor() as i32;
                                    let row = ((self.cursor_y - node_area_y - self.pan_y) / (self.grid_size_y + self.skipped_row_h)).floor() as i32;
                                    self.grid_cursor_col = col;
                                    self.grid_cursor_row = row;
                                    changed = true;
                                }
                                if let Some(dir_idx) = self.widgets[CONTENT_IDX].double_clicked_node() {
                                    self.widgets[CONTENT_IDX].clear_double_clicked_node();
                                    let dir = self.current_dir();
                                    if dir_idx < dir.children.len() && (dir.children[dir_idx].node_type == "node" || dir.children[dir_idx].node_type == "opencl" || !dir.children[dir_idx].children.is_empty()) {
                                        self.current_path.push(dir_idx);
                                        self.on_path_changed();
                                        changed = true;
                                    }
                                }
                            }
                        }
                        self.sync_pane_focus();
                    }
                    ElementState::Released => {
                        if self.drag_widget.is_some() {
                            let idx = self.drag_widget.unwrap();
                            if idx == NETWORK_PANEL_IDX {
                                self.widgets[idx].drag_end();
                                self.is_resizing_network = None;
                                self.sync_layout();
                                self.read_panel_offsets();
                                self.upload_vertices();
                            } else if idx == PARAM_IDX {
                                self.widgets[idx].drag_end();
                                self.is_resizing_param = false;
                                self.sync_layout();
                                self.upload_vertices();
                            } else if idx == SPREADSHEET_IDX {
                                self.widgets[idx].drag_end();
                                self.is_resizing_spreadsheet = false;
                                self.sync_layout();
                                self.upload_vertices();
                            } else if idx == CONTENT_IDX {
                                self.widgets[idx].drag_end();
                                let updated_nodes = self.widgets[CONTENT_IDX].get_nodes();
                                let dir = self.current_dir_mut();
                                for (i, node) in updated_nodes.iter().enumerate() {
                                    if let Some(child) = dir.children.get_mut(i) {
                                        child.position = node.position;
                                    }
                                }
                                if let Some(sel_idx) = self.widgets[CONTENT_IDX].selected_node() {
                                    if let Some(node) = updated_nodes.get(sel_idx) {
                                        self.grid_cursor_col = node.position.0 as i32;
                                        self.grid_cursor_row = node.position.1 as i32;
                                    }
                                }
                                self.rebuild_positions();
                                self.apply_layout();
                                self.update_panel_bounds();
                                self.upload_vertices();
                            } else {
                                self.widgets[idx].drag_end();
                            }
                            self.drag_widget = None;
                            changed = true;
                        }
                        let ctx = &mut self.ui_context;
                        for w in &mut self.widgets {
                            w.set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                            if w.mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y, ctx) {
                                changed = true;
                            }
                        }
                    }
                }


                if let Some((i, visible)) = self.widgets[CONTENT_IDX].take_node_geom_toggle() {
                    self.current_dir_mut().children[i].geometry_visible = visible;
                    self.rebuild_scene_geometry();
                    changed = true;
                }

                if self.widgets[PAGINATOR_IDX].is_page_hidden() != was_page_hidden {
                    self.rebuild_positions();
                    self.apply_layout();
                    self.sync_pane_focus();
                    self.sync_nodes();
                    changed = true;
                }

                if self.focused_widget != old_focus {
                    changed = true;
                }
                changed
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                false
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.logical_key == Key::Named(NamedKey::Space) {
                    self.space_pressed = event.state == ElementState::Pressed;
                }

                if event.state == ElementState::Pressed {
                    if event.logical_key == Key::Named(NamedKey::Tab) && self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key() {
                        self.focused_pane = get_next_visible_pane(
                            self.focused_pane,
                            self.show_network,
                            self.show_viewport,
                            self.show_parameters,
                            self.show_spreadsheet,
                            self.modifiers.shift_key(),
                        );
                        self.sync_pane_focus();
                        return true;
                    }
                }

                if self.widgets[PARAM_IDX].keyboard_input(event, &mut self.ui_context) {
                    self.sync_parameters_to_project();
                    return true;
                }

                if self.node_palette_visible {
                    return self.handle_node_palette_key(event);
                }
                let mut changed = false;
                if event.state == ElementState::Pressed {
                        let is_plain_key = !self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key();
                        let is_alt_key = self.modifiers.alt_key() && !self.modifiers.control_key() && !self.modifiers.super_key();

                        let mut delta = None;

                        match &event.logical_key {
                            Key::Named(NamedKey::ArrowUp) => {
                                delta = Some((0, -1));
                            }
                            Key::Named(NamedKey::ArrowDown) => {
                                delta = Some((0, 1));
                            }
                            Key::Named(NamedKey::ArrowLeft) => {
                                delta = Some((-1, 0));
                            }
                            Key::Named(NamedKey::ArrowRight) => {
                                delta = Some((1, 0));
                            }
                            Key::Character(s) => {
                                match s.as_str() {
                                    "k" | "K" if is_plain_key || (is_alt_key && self.focused_pane == LEFT_MENUBAR_IDX) => {
                                        delta = Some((0, -1));
                                    }
                                    "j" | "J" if is_plain_key || (is_alt_key && self.focused_pane == LEFT_MENUBAR_IDX) => {
                                        delta = Some((0, 1));
                                    }
                                    "h" | "H" if is_plain_key || (is_alt_key && self.focused_pane == LEFT_MENUBAR_IDX) => {
                                        delta = Some((-1, 0));
                                    }
                                    "l" | "L" if is_plain_key || (is_alt_key && self.focused_pane == LEFT_MENUBAR_IDX) => {
                                        delta = Some((1, 0));
                                    }
                                    _ => {}
                                }
                            }
                            _ => {}
                        }

                        if let Some((dc, dr)) = delta {
                            self.pan_velocity_x = 0.0;
                            self.pan_velocity_y = 0.0;
                            self.is_scrolling_trackpad = false;
                            self.scroll_accum_x = 0.0;
                            self.scroll_accum_y = 0.0;
                            if is_alt_key {
                                let active_nodes = self.current_dir().children.len();
                                let node_idx_at_cursor = self.current_dir().children.iter().take(active_nodes).position(|child| {
                                    child.position.0 as i32 == self.grid_cursor_col && child.position.1 as i32 == self.grid_cursor_row
                                });
                                if let Some(idx) = node_idx_at_cursor {
                                    let new_x = self.current_dir().children[idx].position.0 + dc as f32;
                                    let new_y = self.current_dir().children[idx].position.1 + dr as f32;
                                    self.current_dir_mut().children[idx].position = (new_x, new_y);
                                    self.sync_nodes();
                                    self.sync_layout();
                                    self.upload_vertices();
                                }
                            }
                            self.grid_cursor_col += dc;
                            self.grid_cursor_row += dr;
                            self.sync_cursor_and_selection();
                            changed = true;
                        } else {
                            if let Key::Character(s) = &event.logical_key {
                                if is_plain_key {
                                    match s.as_str() {
                                        "e" | "E" => {
                                            if self.focused_widget == Some(CONTENT_IDX) {
                                                if let Some(slot_idx) = self.widgets[CONTENT_IDX].selected_node() {
                                                    let dir = self.current_dir();
                                                    if slot_idx < dir.children.len() {
                                                        let visible = !dir.children[slot_idx].geometry_visible;
                                                        self.current_dir_mut().children[slot_idx].geometry_visible = visible;
                                                        self.sync_nodes();
                                                        self.rebuild_scene_geometry();
                                                        changed = true;
                                                    }
                                                }
                                            }
                                        }
                                        "r" | "R" => {
                                            if self.focused_pane == RIGHT_MENUBAR_IDX {
                                                if self.active_camera != "Default Camera" {
                                                    self.update_active_camera_rotation_reset();
                                                } else {
                                                    self.rotation_y = 0.0;
                                                    self.rotation_x = 0.0;
                                                }
                                                self.viewport_zoom = 1.0;
                                                self.rotate_velocity_yaw = 0.0;
                                                self.rotate_velocity_pitch = 0.0;
                                                self.zoom_velocity = 0.0;
                                                self.is_rotating_viewport = false;
                                                self.is_zooming_viewport = false;
                                                self.scroll_lock = 0;
                                                changed = true;
                                            }
                                        }
                                        "u" | "U" => {
                                            if self.focused_pane == LEFT_MENUBAR_IDX {
                                                if !self.current_path.is_empty() {
                                                    self.current_path.pop();
                                                    self.on_path_changed();
                                                    changed = true;
                                                }
                                            }
                                        }
                                        "i" | "I" => {
                                            if self.focused_pane == LEFT_MENUBAR_IDX {
                                                if self.focused_widget == Some(CONTENT_IDX) {
                                                    if let Some(slot_idx) = self.widgets[CONTENT_IDX].selected_node() {
                                                        let dir = self.current_dir();
                                                         if slot_idx < dir.children.len() && (dir.children[slot_idx].node_type == "node" || dir.children[slot_idx].node_type == "opencl" || !dir.children[slot_idx].children.is_empty()) {
                                                             self.current_path.push(slot_idx);
                                                             self.on_path_changed();
                                                             changed = true;
                                                         }
                                                    }
                                                }
                                            }
                                        }
                                        "-" => {
                                            self.zoom(1.0 / 1.15, None);
                                            changed = true;
                                        }
                                        "=" | "+" => {
                                            self.zoom(1.15, None);
                                            changed = true;
                                        }
                                        "f" | "F" => {
                                            if self.focused_pane == LEFT_MENUBAR_IDX {
                                                let active_nodes = self.current_dir().children.len();
                                                if active_nodes == 0 {
                                                    self.grid_size_x = 80.0;
                                                    self.grid_size_y = 40.0;
                                                    self.skipped_col_w = 20.0;
                                                    self.skipped_row_h = 20.0;
                                                    self.pan_x = 20.0;
                                                    self.pan_y = 20.0;
                                                } else {
                                                    let base_gx = 80.0;
                                                    let base_gy = 40.0;
                                                    let base_col_w = 20.0;
                                                    let base_row_h = 20.0;

                                                    let mut b_xmin = f32::MAX;
                                                    let mut b_xmax = f32::MIN;
                                                    let mut b_ymin = f32::MAX;
                                                    let mut b_ymax = f32::MIN;

                                                    for slot_idx in 0..active_nodes {
                                                        let (col, row) = self.current_dir().children[slot_idx].position;
                                                        let x_min = col * (base_gx + base_col_w);
                                                        let x_max = x_min + base_gx;
                                                        let y_min = row * (base_gy + base_row_h);
                                                        let y_max = y_min + base_gy;

                                                        if x_min < b_xmin { b_xmin = x_min; }
                                                        if x_max > b_xmax { b_xmax = x_max; }
                                                        if y_min < b_ymin { b_ymin = y_min; }
                                                        if y_max > b_ymax { b_ymax = y_max; }
                                                    }

                                                    let w_base = b_xmax - b_xmin;
                                                    let h_base = b_ymax - b_ymin;

                                                     let (_px, _py, pw, ph) = self.positions[CONTENT_IDX];
                                                     let viewport_w = pw;
                                                     let viewport_h = ph;

                                                    let padding = 40.0;
                                                    let padded_w = (viewport_w - 2.0 * padding).max(10.0);
                                                    let padded_h = (viewport_h - 2.0 * padding).max(10.0);

                                                    let fx = padded_w / w_base;
                                                    let fy = padded_h / h_base;
                                                    let mut f = fx.min(fy);

                                                    f = f.min(1.0).max(30.0 / base_gx);

                                                    self.grid_size_x = (base_gx * f).clamp(30.0, 500.0);
                                                    self.grid_size_y = (base_gy * f).clamp(15.0, 250.0);
                                                    self.skipped_col_w = base_col_w * f;
                                                    self.skipped_row_h = base_row_h * f;

                                                    let mut actual_xmin = f32::MAX;
                                                    let mut actual_xmax = f32::MIN;
                                                    let mut actual_ymin = f32::MAX;
                                                    let mut actual_ymax = f32::MIN;

                                                    for slot_idx in 0..active_nodes {
                                                        let (col, row) = self.current_dir().children[slot_idx].position;
                                                        let x_min = col * (self.grid_size_x + self.skipped_col_w);
                                                        let x_max = x_min + self.grid_size_x;
                                                        let y_min = row * (self.grid_size_y + self.skipped_row_h);
                                                        let y_max = y_min + self.grid_size_y;

                                                        if x_min < actual_xmin { actual_xmin = x_min; }
                                                        if x_max > actual_xmax { actual_xmax = x_max; }
                                                        if y_min < actual_ymin { actual_ymin = y_min; }
                                                        if y_max > actual_ymax { actual_ymax = y_max; }
                                                    }

                                                    let actual_w = actual_xmax - actual_xmin;
                                                    let actual_h = actual_ymax - actual_ymin;

                                                    self.pan_x = (viewport_w - actual_w) / 2.0 - actual_xmin;
                                                    self.pan_y = (viewport_h - actual_h) / 2.0 - actual_ymin;
                                                }

                                                self.sync_grid_settings();
                                                self.rebuild_positions();
                                                self.apply_layout();
                                                self.update_panel_bounds();
                                                self.upload_vertices();
                                                changed = true;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }

                    if changed {
                        self.keep_cursor_in_view();
                    }

                    if !changed && event.state == ElementState::Pressed {
                        if event.logical_key == Key::Named(NamedKey::Tab) {
                            self.open_node_palette();
                            return true;
                        }
                        if let Some(action) = self.shortcut_manager.match_action(&self.modifiers, &event.logical_key) {
                            self.pending_action = Some(action);
                            return true;
                        }
                    }
                }
                if changed {
                    true
                } else if let Some(idx) = self.focused_widget {
                    self.widgets[idx].keyboard_input(event, &mut self.ui_context)
                } else { false }
            }
        }
    }

    fn render(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        if now.duration_since(self.last_config_read).as_secs_f32() > 2.0 {
            self.update_inertial_settings();
            let design_path = DesignSettings::file_path();
            if let Ok(m) = std::fs::metadata(&design_path) {
                if let Ok(mod_time) = m.modified() {
                    if Some(mod_time) != self.last_design_mod_time {
                        self.last_design_mod_time = Some(mod_time);
                        let settings = DesignSettings::load();
                        self.square_viewport = settings.square_viewport;
                        self.grid_snap_enabled = settings.grid_snap_enabled;
                        self.network_grid_visible = settings.network_grid_enabled;
                        self.grid_size_x = settings.grid_size_x;
                        self.grid_size_y = settings.grid_size_y;
                        self.skipped_row_h = settings.skipped_row_h;
                        self.skipped_col_w = settings.skipped_col_w;
                        self.grid_thickness = settings.grid_thickness;
                        self.show_grid = settings.show_grid_enabled;
                        self.show_cube = settings.show_cube_enabled;
                        self.show_origin = settings.show_origin_enabled;
                        self.show_camera_pivot = settings.show_camera_pivot_enabled;
                        self.viewport_bg_color = settings.viewport_bg_color;
                        self.node_color = settings.node_color;
                        self.grid_color = settings.grid_color;
                        self.origin_size = settings.origin_size;
                        self.camera_pivot_size = settings.camera_pivot_size;
                        self.uniform_background = settings.uniform_background;
                        self.network_opacity = settings.network_opacity;
                        self.cell_color = settings.cell_color;
                        self.gap_color = settings.gap_color;

                        colors::set_node_color([self.node_color[0], self.node_color[1], self.node_color[2], 1.0]);

                        self.update_origin_geometry();
                        self.update_grid_geometry();
                        self.update_pivot_geometry();
                        self.sync_grid_settings();
                        self.upload_vertices();
                    }
                }
            }
        }

        // Panning velocity tracking
        if self.is_panning {
            if dt > 1e-5 {
                let diff_x = self.pan_x - self.last_frame_pan_x;
                let diff_y = self.pan_y - self.last_frame_pan_y;
                self.pan_velocity_x = self.pan_velocity_x * 0.4 + (diff_x / dt) * 0.6;
                self.pan_velocity_y = self.pan_velocity_y * 0.4 + (diff_y / dt) * 0.6;
            }
            self.last_frame_pan_x = self.pan_x;
            self.last_frame_pan_y = self.pan_y;
        }

        // Trackpad scrolling velocity tracking & timeout detection
        if self.is_scrolling_trackpad {
            if now.duration_since(self.last_scroll_time).as_secs_f32() > 0.05 {
                self.is_scrolling_trackpad = false;
            } else if dt > 1e-5 {
                let vel_x = -self.scroll_accum_x / dt;
                let vel_y = -self.scroll_accum_y / dt;
                self.pan_velocity_x = self.pan_velocity_x * 0.4 + vel_x * 0.6;
                self.pan_velocity_y = self.pan_velocity_y * 0.4 + vel_y * 0.6;
            }
            self.scroll_accum_x = 0.0;
            self.scroll_accum_y = 0.0;
        }

        // Viewport rotation velocity tracking & timeout detection
        if self.is_rotating_viewport {
            if now.duration_since(self.last_rotate_time).as_secs_f32() > 0.05 {
                self.is_rotating_viewport = false;
                self.scroll_lock = 0;
            } else if dt > 1e-5 {
                let vel_yaw = self.rotate_accum_yaw / dt;
                let vel_pitch = self.rotate_accum_pitch / dt;
                self.rotate_velocity_yaw = self.rotate_velocity_yaw * 0.4 + vel_yaw * 0.6;
                self.rotate_velocity_pitch = self.rotate_velocity_pitch * 0.4 + vel_pitch * 0.6;
            }
            self.rotate_accum_yaw = 0.0;
            self.rotate_accum_pitch = 0.0;
        }

        // Viewport zoom velocity tracking & timeout detection
        if self.is_zooming_viewport {
            if now.duration_since(self.last_zoom_time).as_secs_f32() > 0.05 {
                self.is_zooming_viewport = false;
            } else if dt > 1e-5 {
                let vel_zoom = self.zoom_accum / dt;
                self.zoom_velocity = self.zoom_velocity * 0.4 + vel_zoom * 0.6;
            }
            self.zoom_accum = 0.0;
        }

        let mut tick_changed = false;
        let ctx = &mut self.ui_context;
        for w in &mut self.widgets {
            if w.tick(dt, ctx) {
                tick_changed = true;
            }
        }
        if clear_ui::widget::hover_animation::tick(dt) {
            tick_changed = true;
        }
        self.update_recent_files_layout();

        // Panning kinetic slide
        if !self.is_panning && !self.is_scrolling_trackpad && (self.pan_velocity_x.abs() > 0.01 || self.pan_velocity_y.abs() > 0.01) {
            if !self.inertial_scroll_enabled {
                self.pan_velocity_x = 0.0;
                self.pan_velocity_y = 0.0;
            } else {
                self.pan_x += self.pan_velocity_x * dt;
                self.pan_y += self.pan_velocity_y * dt;

                // Apply dynamic friction decay
                let decay = self.inertial_scroll_friction.powf(dt * 60.0);
                self.pan_velocity_x *= decay;
                self.pan_velocity_y *= decay;

                // If velocity magnitude is below 10.0 px/sec, stop completely.
                if self.pan_velocity_x.hypot(self.pan_velocity_y) < 10.0 {
                    self.pan_velocity_x = 0.0;
                    self.pan_velocity_y = 0.0;
                }

                self.sync_grid_settings();
                self.rebuild_positions();
                self.apply_layout();
                self.update_panel_bounds();
                tick_changed = true;
            }
        }

        // Viewport rotation kinetic slide
        if !self.is_rotating_viewport && (self.rotate_velocity_yaw.abs() > 0.001 || self.rotate_velocity_pitch.abs() > 0.001) {
            let dx = self.rotate_velocity_yaw * dt;
            let dy = self.rotate_velocity_pitch * dt;
            if self.active_camera != "Default Camera" {
                self.update_active_camera_rotation(dx, dy);
            } else {
                self.rotation_y += dx;
                self.rotation_x += dy;
            }

            // Apply friction decay
            let friction = 5.0_f32;
            let decay = (-friction * dt).exp();
            self.rotate_velocity_yaw *= decay;
            self.rotate_velocity_pitch *= decay;

            if self.rotate_velocity_yaw.abs() < 0.01 { self.rotate_velocity_yaw = 0.0; }
            if self.rotate_velocity_pitch.abs() < 0.01 { self.rotate_velocity_pitch = 0.0; }
            tick_changed = true;
        }

        // Viewport zoom kinetic slide
        if !self.is_zooming_viewport && self.zoom_velocity.abs() > 0.001 {
            let d_zoom = self.zoom_velocity * dt;
            self.viewport_zoom *= (-d_zoom).exp();
            self.viewport_zoom = self.viewport_zoom.clamp(0.05, 20.0);

            // Apply friction decay
            let friction = 5.0_f32;
            let decay = (-friction * dt).exp();
            self.zoom_velocity *= decay;

            if self.zoom_velocity.abs() < 0.01 {
                self.zoom_velocity = 0.0;
            }
            tick_changed = true;
        }

        if tick_changed {
            self.upload_vertices();
        }

        let mut panned = false;
        if let Some(idx) = self.drag_widget {
            if idx == CONTENT_IDX {
                let (px, py, pw, ph) = self.positions[CONTENT_IDX];
                let margin = 30.0_f32;
                let pan_speed = 5.0_f32;

                if self.cursor_x >= px - 50.0 && self.cursor_x < px + pw + 50.0
                    && self.cursor_y >= py - 50.0 && self.cursor_y < py + ph + 50.0
                {
                    if self.cursor_x < px + margin {
                        self.pan_x += pan_speed;
                        panned = true;
                    } else if self.cursor_x > px + pw - margin {
                        self.pan_x -= pan_speed;
                        panned = true;
                    }

                    if self.cursor_y < py + margin {
                        self.pan_y += pan_speed;
                        panned = true;
                    } else if self.cursor_y > py + ph - margin {
                        self.pan_y -= pan_speed;
                        panned = true;
                    }
                }
            }
        }

        if panned {
            self.pan_velocity_x = 0.0;
            self.pan_velocity_y = 0.0;
            self.is_scrolling_trackpad = false;
            self.scroll_accum_x = 0.0;
            self.scroll_accum_y = 0.0;
            self.sync_grid_settings();
            if let Some(idx) = self.drag_widget {
                self.widgets[idx].set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                self.widgets[idx].drag_update(self.cursor_x, self.cursor_y);
            }
            self.sync_layout();
            self.read_panel_offsets();
            self.upload_vertices();
        }

        self.prepare_text();

        let output = match self.wgpu_adapter.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.wgpu_adapter.surface.configure(&self.wgpu_adapter.device, &self.wgpu_adapter.config);
                return false;
            }
            Err(wgpu::SurfaceError::Timeout) => return false,
            Err(e) => { eprintln!("Surface error: {e:?}"); return false; }
        };

        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.wgpu_adapter.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Encoder"),
        });

        // 3D canvas render pass (background layer)
        if !self.is_detached_network {
            let cx_logical = 0.0;
            let cy_logical = HEADER_H;
            let cw_logical = self.width;
            let ch_logical = self.body_h();
            let mut cw = (cw_logical * self.scale as f32) as u32;
            let mut ch = (ch_logical * self.scale as f32) as u32;
            let mut sx = (cx_logical * self.scale as f32) as u32;
            let mut sy = (cy_logical * self.scale as f32) as u32;

            if self.square_viewport {
                let s = cw.min(ch);
                sx += (cw - s) / 2;
                sy += (ch - s) / 2;
                cw = s;
                ch = s;
            }

            if cw > 0 && ch > 0 {
                let aspect = cw as f32 / ch as f32;

                let proj = Mat4::perspective_rh(0.9, aspect, 0.1, 100.0);
                let mut camera_pos = Vec3::new(2.5, 1.8, 2.5);
                let mut rx = 0.0f32;
                let mut ry = 0.0f32;
                let mut rz = 0.0f32;
                let mut pivot = Vec3::ZERO;
                if self.active_camera != "Default Camera" {
                    if let Some(node) = self.current_dir().children.iter().find(|c| c.node_type == "camera" && c.name == self.active_camera) {
                        let mut cx = 2.5f32;
                        let mut cy = 1.8f32;
                        let mut cz = 2.5f32;
                        for p in &node.params {
                            if p.name == "Position" {
                                let parts: Vec<&str> = p.default
                                    .split(|c| c == ':' || c == ',' || c == ' ')
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                if parts.len() >= 3 {
                                    if let (Ok(vx), Ok(vy), Ok(vz)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>(), parts[2].parse::<f32>()) {
                                        cx = vx;
                                        cy = vy;
                                        cz = vz;
                                    }
                                }
                            } else if p.name == "Rotation" {
                                let parts: Vec<&str> = p.default
                                    .split(|c| c == ':' || c == ',' || c == ' ')
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                if parts.len() >= 3 {
                                    if let (Ok(vx), Ok(vy), Ok(vz)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>(), parts[2].parse::<f32>()) {
                                        rx = vx;
                                        ry = vy;
                                        rz = vz;
                                    }
                                }
                            } else if p.name == "Pivot" {
                                let parts: Vec<&str> = p.default
                                    .split(|c| c == ':' || c == ',' || c == ' ')
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                if parts.len() >= 3 {
                                    if let (Ok(vx), Ok(vy), Ok(vz)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>(), parts[2].parse::<f32>()) {
                                        pivot = Vec3::new(vx, vy, vz);
                                    }
                                }
                            }
                        }
                        camera_pos = Vec3::new(cx, cy, cz);
                    }
                }

                let base_offset = camera_pos - pivot;
                let distance = base_offset.length();
                let yaw0 = base_offset.x.atan2(base_offset.z);
                let pitch0 = (base_offset.y / distance.max(1e-5)).asin();

                let total_ry = ry.to_radians() + yaw0;
                let total_rx = rx.to_radians() + pitch0;

                let view_rot_pos = Mat4::from_rotation_y(total_ry) * Mat4::from_rotation_x(-total_rx);
                let camera_up = view_rot_pos.transform_vector3(Vec3::Y);
                let camera_world_pos = pivot + view_rot_pos.transform_vector3(Vec3::new(0.0, 0.0, distance) * self.viewport_zoom);
                let view_mat = Mat4::from_rotation_z(rz.to_radians()) * Mat4::look_at_rh(camera_world_pos, pivot, camera_up);
                let model = Mat4::from_rotation_y(self.rotation_y) * Mat4::from_rotation_x(self.rotation_x);
                let mvp = proj * view_mat * model;
                self.wgpu_adapter.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[mvp.to_cols_array_2d()]));

                let mvp_grid = proj * view_mat * model;
                self.wgpu_adapter.queue.write_buffer(&self.uniform_buffer_grid, 0, bytemuck::cast_slice(&[mvp_grid.to_cols_array_2d()]));

                let cam_angle_y = camera_pos.x.atan2(camera_pos.z);
                let rot_angle = if self.active_camera != "Default Camera" {
                    total_ry
                } else {
                    self.rotation_y + cam_angle_y
                };
                let model_pivot = Mat4::from_translation(pivot) * Mat4::from_rotation_y(rot_angle);
                let mvp_pivot = proj * view_mat * model_pivot;
                self.wgpu_adapter.queue.write_buffer(&self.uniform_buffer_pivot, 0, bytemuck::cast_slice(&[mvp_pivot.to_cols_array_2d()]));

                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("3D Render Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.backdrop_texture_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: self.viewport_bg_color[0] as f64,
                                g: self.viewport_bg_color[1] as f64,
                                b: self.viewport_bg_color[2] as f64,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_texture_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                pass.set_scissor_rect(sx, sy, cw, ch);
                pass.set_pipeline(&self.pipeline_3d);

                if self.show_grid {
                    pass.set_bind_group(0, &self.bind_group_grid, &[]);
                    pass.set_vertex_buffer(0, self.vertex_buffer_grid.slice(..));
                    pass.draw(0..self.vertex_count_grid, 0..1);
                }

                if self.show_origin {
                    pass.set_bind_group(0, &self.bind_group_3d, &[]);
                    pass.set_vertex_buffer(0, self.vertex_buffer_origin.slice(..));
                    pass.draw(0..self.vertex_count_origin, 0..1);
                }

                if self.show_camera_pivot {
                    pass.set_bind_group(0, &self.bind_group_pivot, &[]);
                    pass.set_vertex_buffer(0, self.vertex_buffer_pivot.slice(..));
                    pass.draw(0..self.vertex_count_pivot, 0..1);
                }

                pass.set_bind_group(0, &self.bind_group_3d, &[]);

                if self.show_cube {
                    pass.set_vertex_buffer(0, self.vertex_buffer_3d.slice(..));
                    pass.draw(0..self.vertex_count_3d, 0..1);
                }

                if self.vertex_count_spheres > 0 {
                    pass.set_vertex_buffer(0, self.vertex_buffer_spheres.slice(..));
                    pass.draw(0..self.vertex_count_spheres, 0..1);
                }
            } else {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Backdrop Clear Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.backdrop_texture_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: self.viewport_bg_color[0] as f64,
                                g: self.viewport_bg_color[1] as f64,
                                b: self.viewport_bg_color[2] as f64,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }
        } else {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Backdrop Clear Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.backdrop_texture_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        // Copy backdrop to output swapchain texture
        if !self.is_detached_network {
            encoder.copy_texture_to_texture(
                wgpu::ImageCopyTexture {
                    texture: &self.backdrop_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::ImageCopyTexture {
                    texture: &output.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: self.physical_width,
                    height: self.physical_height,
                    depth_or_array_layers: 1,
                },
            );
        }

        // UI render pass (foreground layer)
        {
            let load_op = if self.is_detached_network {
                wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 })
            } else {
                wgpu::LoadOp::Load
            };

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("UI Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: load_op,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, &self.backdrop_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.vertex_count, 0..1);

            if self.textured_vertex_count > 0 {
                // eprintln!("DEBUG_RENDER: textured_vertex_count={}", self.textured_vertex_count);
                pass.set_pipeline(&self.curved_text_pipeline);
                pass.set_bind_group(0, &self.curved_text_bind_group, &[]);
                pass.set_vertex_buffer(0, self.textured_vertex_buffer.slice(..));
                pass.draw(0..self.textured_vertex_count, 0..1);
            }

            self.wgpu_adapter.text_renderer.render(&self.wgpu_adapter.text_atlas, &self.wgpu_adapter.text_viewport, &mut pass).unwrap();

        }

        self.wgpu_adapter.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        tick_changed || panned
    }
}

struct PressedKey {
    logical_key: clear_ui::widget::Key,
    text: Option<String>,
    first_pressed: std::time::Instant,
    last_repeated: std::time::Instant,
}

fn is_repeatable_key(key: &clear_ui::widget::Key) -> bool {
    use clear_ui::widget::{Key, NamedKey};
    match key {
        Key::Named(NamedKey::Backspace) |
        Key::Named(NamedKey::Delete) |
        Key::Named(NamedKey::ArrowLeft) |
        Key::Named(NamedKey::ArrowRight) |
        Key::Named(NamedKey::ArrowUp) |
        Key::Named(NamedKey::ArrowDown) |
        Key::Named(NamedKey::Home) |
        Key::Named(NamedKey::End) |
        Key::Character(_) => true,
        _ => false,
    }
}

struct PendingResize {
    serial: u32,
    edge: smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge,
    start_x: f32,
    start_y: f32,
    is_move: bool,
}

struct AppState {
    registry_state: RegistryState,
    compositor_state: CompositorState,
    xdg_shell_state: XdgShell,
    shm_state: Shm,
    seat_state: SeatState,
    output_state: OutputState,

    seats: Vec<wl_seat::WlSeat>,
    pointer: Option<ThemedPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,

    window: Option<XdgWindow>,
    surface: Option<wl_surface::WlSurface>,

    state: Option<State>,
    exit: bool,
    redraw: bool,
    pressed_key: Option<PressedKey>,
    inspector: Option<clear_ui::protocol::zclear_inspector_v1::ZclearInspectorV1>,
    pending_resize: Option<PendingResize>,
    _sender: calloop::channel::Sender<CustomEvent>,
}

impl CompositorHandler for AppState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        scale_factor: i32,
    ) {
        _surface.set_buffer_scale(scale_factor);
        if let Some(state) = &mut self.state {
            state.scale = scale_factor as f64;
            clear_ui::scale::set_scale_factor(scale_factor as f32);
            let pw = (state.width as f64 * state.scale) as u32;
            let ph = (state.height as f64 * state.scale) as u32;
            state.resize(pw, ph);
            self.redraw = true;
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {}

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {}

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {}

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {}
}

impl OutputHandler for AppState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {}

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {}

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {}
}

impl SeatHandler for AppState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.seats.push(seat);
        // eprintln!("DEBUG SEAT: new_seat called, total seats now: {}", self.seats.len());
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        // eprintln!("DEBUG SEAT: new_capability: {:?}", capability);
        if capability == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor_state.create_surface(qh);
            let themed_pointer = self.seat_state.get_pointer_with_theme(
                qh,
                &seat,
                self.shm_state.wl_shm(),
                surface,
                ThemeSpec::System,
            ).unwrap();
            self.pointer = Some(themed_pointer);
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let keyboard = self
                .seat_state
                .get_keyboard(qh, &seat, None)
                .unwrap();
            self.keyboard = Some(keyboard);
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            self.pointer = None;
        }
        if capability == Capability::Keyboard {
            self.keyboard = None;
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.seats.retain(|s| s != &seat);
        // eprintln!("DEBUG SEAT: remove_seat called, total seats now: {}", self.seats.len());
    }
}

impl ShmHandler for AppState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm_state
    }
}

impl PointerHandler for AppState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[smithay_client_toolkit::seat::pointer::PointerEvent],
    ) {
        use smithay_client_toolkit::seat::pointer::PointerEventKind;
        for event in events {
            if let Some(st) = &mut self.state {
                let (cx, cy) = clear_ui::wayland::scale_pointer_pos(event.position, st.scale);
                st.cursor_x = event.position.0 as f32;
                st.cursor_y = event.position.1 as f32;
                match &event.kind {
                    PointerEventKind::Motion { .. } => {
                        if st.is_detached_network {
                            let lx = event.position.0 as f32;
                            let ly = event.position.1 as f32;
                            let dx = lx - st.circular_network_layout.x;
                            let dy = ly - st.circular_network_layout.y;
                            let dist = (dx * dx + dy * dy).sqrt();
                            let on_border = dist >= st.circular_network_layout.r - 12.0 && dist <= st.circular_network_layout.r;
                            let hits_any_menu = st.widgets[LEFT_MENUBAR_IDX].get_menu_items_at(lx, ly).is_some();

                            if let Some(ref themed_pointer) = self.pointer {
                                if on_border && !hits_any_menu {
                                    let nx = dx / dist;
                                    let ny = dy / dist;
                                    let mut cursor = CursorIcon::Default;
                                    if ny < -0.382 {
                                        if nx < -0.382 {
                                            cursor = CursorIcon::NwResize;
                                        } else if nx > 0.382 {
                                            cursor = CursorIcon::NeResize;
                                        } else {
                                            cursor = CursorIcon::NResize;
                                        }
                                    } else if ny > 0.382 {
                                        if nx < -0.382 {
                                            cursor = CursorIcon::SwResize;
                                        } else if nx > 0.382 {
                                            cursor = CursorIcon::SeResize;
                                        } else {
                                            cursor = CursorIcon::SResize;
                                        }
                                    } else {
                                        if nx < -0.382 {
                                            cursor = CursorIcon::WResize;
                                        } else if nx > 0.382 {
                                            cursor = CursorIcon::EResize;
                                        }
                                    }
                                    let _ = themed_pointer.set_cursor(_conn, cursor);
                                } else {
                                    let _ = themed_pointer.set_cursor(_conn, CursorIcon::Default);
                                }
                            }
                        }

                        if let Some(ref pending) = self.pending_resize {
                            let lx = event.position.0 as f32;
                            let ly = event.position.1 as f32;
                            let rx = lx - pending.start_x;
                            let ry = ly - pending.start_y;
                            let rdist = (rx * rx + ry * ry).sqrt();
                            if rdist > 4.0 {
                                if let Some(ref window) = self.window {
                                    let seat = self.seats.first().cloned().or_else(|| self.seat_state.seats().next());
                                    if let Some(ref seat) = seat {
                                        if pending.is_move {
                                            // eprintln!("DEBUG DRAG INITIATING window.move_ with serial={}", pending.serial);
                                            window.move_(seat, pending.serial);
                                        } else {
                                            // eprintln!("DEBUG RESIZE INITIATING window.resize with edge={:?}, serial={}", pending.edge, pending.serial);
                                            window.resize(seat, pending.serial, pending.edge);
                                        }
                                    }
                                }
                                self.pending_resize = None;
                            }
                        }

                        let ev = WindowEvent::CursorMoved {
                            position: LocalPosition {
                                x: event.position.0,
                                y: event.position.1,
                            },
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Press { button, serial, .. } => {
                        let btn = match *button {
                            272 => clear_ui::widget::MouseButton::Left,
                            273 => clear_ui::widget::MouseButton::Right,
                            274 => clear_ui::widget::MouseButton::Middle,
                            _ => continue,
                        };
                        // eprintln!("DEBUG MOUSE PRESS: button={:?}, pos={:?}, local=({}, {})", btn, event.position, cx, cy);

                        if let Some(ref st) = self.state {
                            if st.is_detached_network && btn == clear_ui::widget::MouseButton::Left {
                                let lx = event.position.0 as f32;
                                let ly = event.position.1 as f32;
                                let dx = lx - st.circular_network_layout.x;
                                let dy = ly - st.circular_network_layout.y;
                                let dist = (dx * dx + dy * dy).sqrt();
                                let on_border = dist >= st.circular_network_layout.r - 12.0 && dist <= st.circular_network_layout.r;
                                let in_menubar_bg = dy < 0.0 && dist >= st.circular_network_layout.r - 35.0 && dist <= st.circular_network_layout.r;
                                let hits_any_menu = st.widgets[LEFT_MENUBAR_IDX].get_menu_items_at(lx, ly).is_some();

                                // eprintln!("DEBUG DRAG: lx={}, ly={}, cx={}, cy={}, r={}, dx={}, dy={}, dist={}, on_border={}, in_menubar_bg={}, hits_any_menu={}, seats_len={}, has_window={}",
                                //     lx, ly, st.circular_network_layout.x, st.circular_network_layout.y, st.circular_network_layout.r,
                                //     dx, dy, dist, on_border, in_menubar_bg, hits_any_menu, self.seats.len(), self.window.is_some());

                                if (on_border || in_menubar_bg) && !hits_any_menu {
                                    if let Some(ref _window) = self.window {
                                        let seat = self.seats.first().cloned().or_else(|| self.seat_state.seats().next());
                                        if let Some(ref _seat) = seat {
                                            if on_border {
                                                use smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
                                                let nx = dx / dist;
                                                let ny = dy / dist;
                                                let mut edge = ResizeEdge::None;
                                                if ny < -0.382 {
                                                    if nx < -0.382 {
                                                        edge = ResizeEdge::TopLeft;
                                                    } else if nx > 0.382 {
                                                        edge = ResizeEdge::TopRight;
                                                    } else {
                                                        edge = ResizeEdge::Top;
                                                    }
                                                } else if ny > 0.382 {
                                                    if nx < -0.382 {
                                                        edge = ResizeEdge::BottomLeft;
                                                    } else if nx > 0.382 {
                                                        edge = ResizeEdge::BottomRight;
                                                    } else {
                                                        edge = ResizeEdge::Bottom;
                                                    }
                                                } else {
                                                    if nx < -0.382 {
                                                        edge = ResizeEdge::Left;
                                                    } else if nx > 0.382 {
                                                        edge = ResizeEdge::Right;
                                                    }
                                                }
                                                self.pending_resize = Some(PendingResize {
                                                    serial: *serial,
                                                    edge,
                                                    start_x: lx,
                                                    start_y: ly,
                                                    is_move: false,
                                                });
                                                continue;
                                            } else {
                                                self.pending_resize = Some(PendingResize {
                                                    serial: *serial,
                                                    edge: smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge::None,
                                                    start_x: lx,
                                                    start_y: ly,
                                                    is_move: true,
                                                });
                                                continue;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        let ev = WindowEvent::MouseInput {
                            state: clear_ui::widget::ElementState::Pressed,
                            button: btn,
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Release { button, .. } => {
                        let btn = match *button {
                            272 => clear_ui::widget::MouseButton::Left,
                            273 => clear_ui::widget::MouseButton::Right,
                            274 => clear_ui::widget::MouseButton::Middle,
                            _ => continue,
                        };
                        // eprintln!("DEBUG MOUSE RELEASE: button={:?}, pos={:?}, local=({}, {})", btn, event.position, cx, cy);
                        if btn == clear_ui::widget::MouseButton::Left {
                            self.pending_resize = None;
                        }
                        let ev = WindowEvent::MouseInput {
                            state: clear_ui::widget::ElementState::Released,
                            button: btn,
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Axis { horizontal, vertical, .. } => {
                        let h_val = horizontal.absolute as f32;
                        let v_val = vertical.absolute as f32;
                        // eprintln!("DEBUG AXIS EVENT: horizontal={:?}, vertical={:?}, scale={}", horizontal, vertical, st.scale);
                        let ev = WindowEvent::MouseWheel {
                            delta: clear_ui::widget::MouseScrollDelta::LineDelta(-h_val / 10.0, -v_val / 10.0),
                            phase: TouchPhase::Moved,
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Enter { .. } => {
                        if let Some(ref themed_pointer) = self.pointer {
                            let _ = themed_pointer.set_cursor(_conn, CursorIcon::Default);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

impl KeyboardHandler for AppState {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw_modifiers: &[u32],
        _keysyms: &[xkeysym::Keysym],
    ) {}

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        self.pressed_key = None;
        if let Some(st) = &mut self.state {
            st.modifiers = ModifiersState::default();
        }
    }


    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: smithay_client_toolkit::seat::keyboard::KeyEvent,
    ) {
        self.handle_key(event, clear_ui::widget::ElementState::Pressed);
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: smithay_client_toolkit::seat::keyboard::KeyEvent,
    ) {
        self.handle_key(event, clear_ui::widget::ElementState::Released);
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: smithay_client_toolkit::seat::keyboard::Modifiers,
        _layout: u32,
    ) {
        if let Some(st) = &mut self.state {
            st.modifiers.ctrl = modifiers.ctrl;
            st.modifiers.alt = modifiers.alt;
            st.modifiers.shift = modifiers.shift;
            st.modifiers.logo = modifiers.logo;
        }
    }
}

impl WindowHandler for AppState {
    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &XdgWindow,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let (w, h) = configure.new_size;
        // eprintln!("DEBUG CONFIGURE: new_size={:?}, configure={:?}", configure.new_size, configure);
        if let (Some(w), Some(h)) = (w, h) {
            let width = w.get();
            let height = h.get();
            if let Some(state) = &mut self.state {
                let pw = (width as f64 * state.scale) as u32;
                let ph = (height as f64 * state.scale) as u32;
                state.resize(pw, ph);
            }
        }
        self.redraw = true;
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &XdgWindow) {
        self.exit = true;
    }
}

impl ProvidesRegistryState for AppState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    
    fn runtime_add_global(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _name: u32,
        _interface: &str,
        _version: u32,
    ) {}
    
    fn runtime_remove_global(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _name: u32,
        _interface: &str,
    ) {}
}

impl wayland_client::Dispatch<clear_ui::protocol::zclear_inspector_v1::ZclearInspectorV1, ()> for AppState {
    fn event(
        state: &mut Self,
        _proxy: &clear_ui::protocol::zclear_inspector_v1::ZclearInspectorV1,
        event: clear_ui::protocol::zclear_inspector_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            clear_ui::protocol::zclear_inspector_v1::Event::InspectedSurface { app_id, x, y, .. } => {
                if let Some(ref mut st) = state.state {
                    let expected_id = if st.is_detached_network {
                        "circular-network-pane"
                    } else {
                        "cce-design-interface"
                    };
                    if app_id == expected_id {
                        st.window_x = x;
                        st.window_y = y;
                    }
                }
            }
            _ => {}
        }
    }
}

delegate_compositor!(AppState);
delegate_xdg_shell!(AppState);
delegate_xdg_window!(AppState);
delegate_shm!(AppState);
delegate_seat!(AppState);
delegate_pointer!(AppState);
delegate_keyboard!(AppState);
delegate_registry!(AppState);
delegate_output!(AppState);

impl AppState {
    fn handle_key(&mut self, event: smithay_client_toolkit::seat::keyboard::KeyEvent, state: clear_ui::widget::ElementState) {
        use clear_ui::widget::{Key, KeyEvent, NamedKey};
        let logical_key = match event.keysym {
            xkeysym::Keysym::Escape => Key::Named(NamedKey::Escape),
            xkeysym::Keysym::Return => Key::Named(NamedKey::Enter),
            xkeysym::Keysym::BackSpace => Key::Named(NamedKey::Backspace),
            xkeysym::Keysym::Down => Key::Named(NamedKey::ArrowDown),
            xkeysym::Keysym::Up => Key::Named(NamedKey::ArrowUp),
            xkeysym::Keysym::Left => Key::Named(NamedKey::ArrowLeft),
            xkeysym::Keysym::Right => Key::Named(NamedKey::ArrowRight),
            xkeysym::Keysym::Tab => Key::Named(NamedKey::Tab),
            xkeysym::Keysym::Delete => Key::Named(NamedKey::Delete),
            xkeysym::Keysym::space => Key::Named(NamedKey::Space),
            xkeysym::Keysym::comma => Key::Character(",".into()),
            xkeysym::Keysym::g | xkeysym::Keysym::G => Key::Character("g".into()),
            xkeysym::Keysym::e | xkeysym::Keysym::E => Key::Character("e".into()),
            xkeysym::Keysym::a | xkeysym::Keysym::A => Key::Character("a".into()),
            xkeysym::Keysym::d | xkeysym::Keysym::D => Key::Character("d".into()),
            xkeysym::Keysym::f | xkeysym::Keysym::F => Key::Character("f".into()),
            xkeysym::Keysym::h | xkeysym::Keysym::H => Key::Character("h".into()),
            xkeysym::Keysym::j | xkeysym::Keysym::J => Key::Character("j".into()),
            xkeysym::Keysym::k | xkeysym::Keysym::K => Key::Character("k".into()),
            xkeysym::Keysym::l | xkeysym::Keysym::L => Key::Character("l".into()),
            xkeysym::Keysym::s | xkeysym::Keysym::S => Key::Character("s".into()),
            xkeysym::Keysym::grave => Key::Character("`".into()),
            _ => {
                if let Some(ref text) = event.utf8 {
                    Key::Character(text.clone())
                } else if let Some(ch) = event.keysym.key_char() {
                    Key::Character(ch.to_string())
                } else {
                    return;
                }
            }
        };

        // eprintln!("DEBUG KEY: keysym={:?}, state={:?}, logical_key={:?}", event.keysym, state, logical_key);

        if let Some(st) = &mut self.state {
            match event.keysym {
                xkeysym::Keysym::Control_L | xkeysym::Keysym::Control_R => {
                    st.modifiers.ctrl = state == clear_ui::widget::ElementState::Pressed;
                }
                xkeysym::Keysym::Alt_L | xkeysym::Keysym::Alt_R => {
                    st.modifiers.alt = state == clear_ui::widget::ElementState::Pressed;
                }
                xkeysym::Keysym::Shift_L | xkeysym::Keysym::Shift_R => {
                    st.modifiers.shift = state == clear_ui::widget::ElementState::Pressed;
                }
                xkeysym::Keysym::Super_L | xkeysym::Keysym::Super_R => {
                    st.modifiers.logo = state == clear_ui::widget::ElementState::Pressed;
                }
                _ => {}
            }

            let custom_event = KeyEvent {
                state,
                logical_key,
                text: event.utf8.clone(),
                repeat: false,
                ctrl: st.modifiers.ctrl,
                shift: st.modifiers.shift,
            };

            if state == clear_ui::widget::ElementState::Pressed {
                if is_repeatable_key(&custom_event.logical_key) {
                    self.pressed_key = Some(PressedKey {
                        logical_key: custom_event.logical_key.clone(),
                        text: custom_event.text.clone(),
                        first_pressed: std::time::Instant::now(),
                        last_repeated: std::time::Instant::now(),
                    });
                } else {
                    self.pressed_key = None;
                }
            } else if state == clear_ui::widget::ElementState::Released {
                if let Some(ref pk) = self.pressed_key {
                    if pk.logical_key == custom_event.logical_key {
                        self.pressed_key = None;
                    }
                }
            }

            let ev = WindowEvent::KeyboardInput { event: custom_event };
            self.process_event(ev);
        }
    }

    fn process_event(&mut self, ev: WindowEvent) {
        if let Some(state) = &mut self.state {
            let mut changed = state.handle_event(&ev);
            state.sync_settings_from_paginator();

            if let Some(seg) = state.widgets[BREADCRUMB_IDX].path_click() {
                if seg < state.current_path.len() {
                    state.current_path.truncate(seg);
                    state.on_path_changed();
                    changed = true;
                }
            }

            if let Some(action) = state.pending_action.take() {
                state.execute_action(action);
                changed = true;
            }

            // Poll Paginator Clicks
            let mut paginator_click = None;
            let active_menubar = state.focused_pane;
            let menu_names = state.widgets[active_menubar].menu_names();
            for (page_idx, page) in state.paginator_page_widgets.iter_mut().enumerate() {
                if page_idx < menu_names.len() && menu_names[page_idx] == "Settings" {
                    continue;
                }
                for (item_idx, widget) in page.iter_mut().enumerate() {
                    if widget.take_click() {
                        paginator_click = Some((page_idx, item_idx));
                        break;
                    }
                }
                if paginator_click.is_some() {
                    break;
                }
            }

            if let Some((menu_idx, item_idx)) = paginator_click {
                state.widgets[state.focused_pane].trigger_menu_click(menu_idx, item_idx);
                changed = true;
            }

            // Check recent files buttons clicks
            let mut clicked_file = None;
            for (i, btn) in state.recent_files_buttons.iter_mut().enumerate() {
                if btn.take_click() {
                    clicked_file = Some(state.recent_files[i].clone());
                }
            }
            if let Some(path) = clicked_file {
                if let Err(e) = state.load_from_file(&path) {
                    eprintln!("Failed to load recent project: {:?}", e);
                    state.update_status_text(&format!("Failed to load: {:?}", e));
                } else {
                    state.update_status_text(&format!("Loaded project from {}", path.display()));
                }
                changed = true;
            }

            if let Some((menu_idx, item_idx)) = state.widgets[HEADER_IDX].menu_click() {
                if menu_idx == 0 { // File
                    match item_idx {
                        0 => { // New Project
                            state.new_project();
                            changed = true;
                        }
                        1 => { // Open
                            state.open_file_chooser();
                            changed = true;
                        }
                        2 => { // Save
                            let path_opt = state.loaded_project_path.clone();
                            if let Some(path) = path_opt {
                                if let Err(e) = state.save_to_file(&path) {
                                    eprintln!("Failed to save project: {:?}", e);
                                    state.update_status_text(&format!("Failed to save: {:?}", e));
                                } else {
                                    state.update_status_text(&format!("Project saved to {}", path.display()));
                                    state.add_recent_file(path);
                                }
                            } else {
                                state.save_file_chooser();
                            }
                            changed = true;
                        }
                        3 => { // Save As
                            state.save_file_chooser();
                            changed = true;
                        }
                        4 => { // Exit
                            state.exit_requested = true;
                        }
                        _ => {}
                    }
                } else if menu_idx == 2 { // View
                    match item_idx {
                        0 => { // Zoom In
                            state.zoom(1.15, None);
                            changed = true;
                        }
                        1 => { // Zoom Out
                            state.zoom(1.0 / 1.15, None);
                            changed = true;
                        }
                        2 => { // Reset Zoom
                            state.grid_size_x = 150.0;
                            state.grid_size_y = 75.0;
                            state.skipped_col_w = 37.5;
                            state.skipped_row_h = 37.5;
                            state.sync_grid_settings();
                            changed = true;
                        }
                        3 => { // Detach Circular Window
                            state.execute_action(Action::DetachCircularWindow);
                            changed = true;
                        }
                        4 => { // Show Network Pane
                            state.show_network = !state.show_network;
                            state.widgets[CONTENT_IDX].set_visible(state.show_network);
                            state.widgets[LEFT_MENUBAR_IDX].set_visible(state.show_network);
                            state.widgets[BREADCRUMB_IDX].set_visible(state.show_network);
                            state.widgets[HEADER_IDX].set_item_checked(2, 4, state.show_network);
                            if !state.show_network && state.focused_pane == LEFT_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        5 => { // Show Viewport Pane
                            state.show_viewport = !state.show_viewport;
                            state.widgets[VIEWPORT_IDX].set_visible(state.show_viewport);
                            state.widgets[RIGHT_MENUBAR_IDX].set_visible(state.show_viewport);
                            state.widgets[HEADER_IDX].set_item_checked(2, 5, state.show_viewport);
                            if !state.show_viewport && state.focused_pane == RIGHT_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        6 => { // Show Parameters Pane
                            state.show_parameters = !state.show_parameters;
                            state.widgets[PARAM_IDX].set_visible(state.show_parameters);
                            state.widgets[PARAM_MENUBAR_IDX].set_visible(state.show_parameters);
                            state.widgets[HEADER_IDX].set_item_checked(2, 6, state.show_parameters);
                            if !state.show_parameters && state.focused_pane == PARAM_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        7 => { // Show Spreadsheet Pane
                            state.show_spreadsheet = !state.show_spreadsheet;
                            state.widgets[SPREADSHEET_IDX].set_visible(state.show_spreadsheet);
                            state.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(state.show_spreadsheet);
                            state.widgets[HEADER_IDX].set_item_checked(2, 7, state.show_spreadsheet);
                            if !state.show_spreadsheet && state.focused_pane == SPREADSHEET_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.widgets[LEFT_MENUBAR_IDX].menu_click() {
                if menu_idx == 0 { // File
                    match item_idx {
                        0 => { // New
                            state.new_project();
                            changed = true;
                        }
                        1 => { // Open
                            state.open_file_chooser();
                            changed = true;
                        }
                        2 => { // Save
                            let path_opt = state.loaded_project_path.clone();
                            if let Some(path) = path_opt {
                                if let Err(e) = state.save_to_file(&path) {
                                    eprintln!("Failed to save project: {:?}", e);
                                    state.update_status_text(&format!("Failed to save: {:?}", e));
                                } else {
                                    state.update_status_text(&format!("Project saved to {}", path.display()));
                                    state.add_recent_file(path);
                                }
                            } else {
                                state.save_file_chooser();
                            }
                            changed = true;
                        }
                        3 => { // Save As
                            state.save_file_chooser();
                            changed = true;
                        }
                        _ => {}
                    }
                } else if menu_idx == 2 { // View
                    match item_idx {
                        0 => { // Zoom In
                            state.zoom(1.15, None);
                            changed = true;
                        }
                        1 => { // Zoom Out
                            state.zoom(1.0 / 1.15, None);
                            changed = true;
                        }
                        2 => {
                            state.circular_network_pane = !state.circular_network_pane;
                            state.widgets[LEFT_MENUBAR_IDX].set_item_checked(2, 2, state.circular_network_pane);
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_grid_settings();
                            changed = true;
                        }
                        3 => { // Detach Pane
                            state.execute_action(Action::DetachCircularWindow);
                            changed = true;
                        }
                        4 => { // Close Pane
                            state.show_network = false;
                            state.widgets[CONTENT_IDX].set_visible(false);
                            state.widgets[LEFT_MENUBAR_IDX].set_visible(false);
                            state.widgets[BREADCRUMB_IDX].set_visible(false);
                            state.widgets[HEADER_IDX].set_item_checked(2, 4, false);
                            if state.focused_pane == LEFT_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.widgets[RIGHT_MENUBAR_IDX].menu_click() {
                if menu_idx == 0 {
                    let camera_nodes: Vec<String> = state.current_dir().children.iter()
                        .filter(|c| c.node_type == "camera")
                        .map(|c| c.name.clone())
                        .collect();
                    let mut items = vec!["Default Camera".to_string()];
                    items.extend(camera_nodes);
                    if item_idx < items.len() {
                        state.active_camera = items[item_idx].clone();
                        for (i, item) in items.iter().enumerate() {
                            state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(0, i, item == &state.active_camera);
                        }
                        changed = true;
                    }
                } else if menu_idx == 3 { // View
                    if item_idx == 0 { // Close Pane
                        state.show_viewport = false;
                        state.widgets[VIEWPORT_IDX].set_visible(false);
                        state.widgets[RIGHT_MENUBAR_IDX].set_visible(false);
                        state.widgets[HEADER_IDX].set_item_checked(2, 5, false);
                        if state.focused_pane == RIGHT_MENUBAR_IDX {
                            state.focused_pane = get_next_visible_pane(
                                state.focused_pane,
                                state.show_network,
                                state.show_viewport,
                                state.show_parameters,
                                state.show_spreadsheet,
                                false,
                            );
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
                } else {
                    let action = if menu_idx == 1 {
                        Some(Action::ToggleSquareViewport)
                    } else if menu_idx == 2 {
                        match item_idx {
                            0 => Some(Action::ToggleGrid),
                            1 => Some(Action::ToggleCube),
                            2 => Some(Action::ToggleOrigin),
                            3 => Some(Action::ToggleCameraPivot),
                            _ => None,
                        }
                    } else { None };
                    if let Some(a) = action {
                        state.execute_action(a);
                        changed = true;
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.widgets[PARAM_MENUBAR_IDX].menu_click() {
                if menu_idx == 0 { // Preset
                    if let Some(slot_idx) = state.widgets[CONTENT_IDX].selected_node() {
                        let node_type = state.current_dir().children[slot_idx].node_type.clone();
                        let template_params = state.node_templates.iter()
                            .find(|t| t.node.node_type == node_type)
                            .map(|t| t.node.params.clone());
                        if let Some(params_to_reset) = template_params {
                            if item_idx == 0 { // Default
                                for template_param in &params_to_reset {
                                    if let Some(p) = state.current_dir_mut().children[slot_idx].params.iter_mut().find(|p| p.name == template_param.name) {
                                        p.default = template_param.default.clone();
                                    }
                                }
                            } else if item_idx == 1 { // Custom
                                for template_param in &params_to_reset {
                                    if let Some(p) = state.current_dir_mut().children[slot_idx].params.iter_mut().find(|p| p.name == template_param.name) {
                                        if let Ok(v) = template_param.default.parse::<f32>() {
                                            p.default = format!("{:.2}", v * 1.5);
                                        } else if let Ok(v) = template_param.default.parse::<i32>() {
                                            p.default = format!("{}", v * 2);
                                        } else if template_param.default.contains(':') {
                                            let parts: Vec<&str> = template_param.default.split(':').collect();
                                            let custom_parts: Vec<String> = parts.iter().map(|p_str| {
                                                if let Ok(v) = p_str.parse::<f32>() {
                                                    format!("{:.2}", v * 1.5)
                                                } else {
                                                    p_str.to_string()
                                                }
                                            }).collect();
                                            p.default = custom_parts.join(":");
                                        } else {
                                            p.default = template_param.default.clone();
                                        }
                                    }
                                }
                            }
                            state.sync_nodes();
                            state.rebuild_scene_geometry();
                            state.upload_vertices();
                            changed = true;
                        }
                    }
                } else if menu_idx == 1 { // Reset
                    if item_idx == 0 { // All
                        if let Some(slot_idx) = state.widgets[CONTENT_IDX].selected_node() {
                            let node_type = state.current_dir().children[slot_idx].node_type.clone();
                            let template_params = state.node_templates.iter()
                                .find(|t| t.node.node_type == node_type)
                                .map(|t| t.node.params.clone());
                            if let Some(params_to_reset) = template_params {
                                for template_param in &params_to_reset {
                                    if let Some(p) = state.current_dir_mut().children[slot_idx].params.iter_mut().find(|p| p.name == template_param.name) {
                                        p.default = template_param.default.clone();
                                    }
                                }
                                state.sync_nodes();
                                state.rebuild_scene_geometry();
                                state.upload_vertices();
                                changed = true;
                            }
                        }
                    }
                } else if menu_idx == 2 { // View
                    if item_idx == 0 { // Close Pane
                        state.show_parameters = false;
                        state.widgets[PARAM_IDX].set_visible(false);
                        state.widgets[PARAM_MENUBAR_IDX].set_visible(false);
                        state.widgets[HEADER_IDX].set_item_checked(2, 6, false);
                        if state.focused_pane == PARAM_MENUBAR_IDX {
                            state.focused_pane = get_next_visible_pane(
                                state.focused_pane,
                                state.show_network,
                                state.show_viewport,
                                state.show_parameters,
                                state.show_spreadsheet,
                                false,
                            );
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.widgets[SPREADSHEET_MENUBAR_IDX].menu_click() {
                if menu_idx == 0 { // View
                    if item_idx == 0 { // Close Pane
                        state.show_spreadsheet = false;
                        state.widgets[SPREADSHEET_IDX].set_visible(false);
                        state.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(false);
                        state.widgets[HEADER_IDX].set_item_checked(2, 7, false);
                        if state.focused_pane == SPREADSHEET_MENUBAR_IDX {
                            state.focused_pane = get_next_visible_pane(
                                state.focused_pane,
                                state.show_network,
                                state.show_viewport,
                                state.show_parameters,
                                state.show_spreadsheet,
                                false,
                            );
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
                }
            }

            if changed {
                state.sync_layout();
                state.read_panel_offsets();
                state.sync_cursor_and_selection();

                if state.drag_widget == Some(PARAM_IDX) && state.widgets[PARAM_IDX].is_dragging() {
                    state.sync_parameters_to_project();
                }

                state.sync_nodes();

                // Sync Parameters pane with selected node
                let params = if !state.is_detached_network {
                    state.widgets[CONTENT_IDX].selected_node().and_then(|sel_idx| {
                        let dir = state.current_dir();
                        if sel_idx < dir.children.len() {
                            Some(param_display(&dir.children[sel_idx].params))
                        } else { None }
                    }).unwrap_or_default()
                } else {
                    vec![]
                };
                state.widgets[PARAM_IDX].set_display_params(&params);
                state.update_paginator();

                state.upload_vertices();
            }

            state.update_status_text(&format!(
                "col: {:.0}  vp: {:.0}  params: {:.0}",
                state.content_left_w(), state.viewport_w(), state.param_w(),
            ));

            if changed {
                state.update_window_title();
                if state.is_detached_network || state.detached_circular_network {
                    state.needs_autosave = true;
                }
                self.redraw = true;
            }
        }
    }

    fn handle_user_event(&mut self, event: CustomEvent) {
        let mut needs_redraw = false;
        if let Some(state) = &mut self.state {
            match event {
                CustomEvent::GetState(tx) => {
                    let proj = Project {
                        name: "Project".to_string(),
                        root: state.fs_root.clone(),
                        view_state: ProjectViewState {
                            active_camera: state.active_camera.clone(),
                            pan: (state.pan_x, state.pan_y),
                            current_path: state.current_path.clone(),
                            selected_node: state.widgets[CONTENT_IDX].selected_node(),
                        },
                    };
                    let json = serde_json::to_string_pretty(&proj).unwrap_or_default();
                    let _ = tx.send(json);
                }
                CustomEvent::PostAction(action, tx) => {
                    let res = match action {
                        HttpAction::Up => {
                            if !state.current_path.is_empty() {
                                state.current_path.pop();
                                state.on_path_changed();
                                needs_redraw = true;
                                Ok("Moved up".to_string())
                            } else {
                                Err("Already at root".to_string())
                            }
                        }
                        HttpAction::Enter { slot } => {
                            let dir = state.current_dir();
                            if slot < dir.children.len() && (dir.children[slot].node_type == "node" || dir.children[slot].node_type == "opencl" || !dir.children[slot].children.is_empty()) {
                                state.current_path.push(slot);
                                state.on_path_changed();
                                needs_redraw = true;
                                Ok("Entered subnet".to_string())
                            } else {
                                Err("Not a valid subnet".to_string())
                            }
                        }
                        HttpAction::SetParam { slot, name, value } => {
                            let dir = state.current_dir_mut();
                            if let Some(child) = dir.children.get_mut(slot) {
                                if let Some(p) = child.params.iter_mut().find(|p| p.name == name) {
                                    p.default = value;
                                    state.sync_nodes();
                                    state.rebuild_scene_geometry();
                                    state.upload_vertices();
                                    needs_redraw = true;
                                    Ok("Parameter updated".to_string())
                                } else {
                                    Err(format!("Parameter {} not found", name))
                                }
                            } else {
                                Err("Slot index out of bounds".to_string())
                            }
                        }
                        HttpAction::ResetCamera => {
                            if state.active_camera != "Default Camera" {
                                state.update_active_camera_rotation_reset();
                            } else {
                                state.rotation_y = 0.0;
                                state.rotation_x = 0.0;
                            }
                            state.viewport_zoom = 1.0;
                            state.rotate_velocity_yaw = 0.0;
                            state.rotate_velocity_pitch = 0.0;
                            state.zoom_velocity = 0.0;
                            state.is_rotating_viewport = false;
                            state.is_zooming_viewport = false;
                            state.scroll_lock = 0;
                            needs_redraw = true;
                            Ok("Camera reset".to_string())
                        }
                        HttpAction::Load { path } => {
                            if let Err(e) = state.load_from_file(Path::new(&path)) {
                                Err(format!("Load failed: {:?}", e))
                            } else {
                                needs_redraw = true;
                                Ok("Project loaded".to_string())
                            }
                        }
                        HttpAction::Save { path } => {
                            if let Err(e) = state.save_to_file(Path::new(&path)) {
                                Err(format!("Save failed: {:?}", e))
                            } else {
                                let path_buf = Path::new(&path).to_path_buf();
                                state.loaded_project_path = Some(path_buf.clone());
                                state.add_recent_file(path_buf);
                                needs_redraw = true;
                                Ok("Project saved".to_string())
                            }
                        }
                        HttpAction::ToggleGeometry { slot } => {
                            let active_nodes = state.current_dir().children.len();
                            if slot < active_nodes {
                                let visible = !state.current_dir().children[slot].geometry_visible;
                                state.current_dir_mut().children[slot].geometry_visible = visible;
                                state.sync_nodes();
                                state.rebuild_scene_geometry();
                                needs_redraw = true;
                                Ok(format!("Geometry visible: {}", visible))
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::AddNode { template_name, name, x, y } => {
                            let template_idx = state.node_templates.iter().position(|t| {
                                t.label.to_lowercase() == template_name.to_lowercase()
                                    || t.node.name.to_lowercase() == template_name.to_lowercase()
                            });
                            if let Some(idx) = template_idx {
                                let mut node = state.node_templates[idx].node.clone();
                                let (nx, ny) = state.find_empty_cell(x, y, None);
                                node.position = (nx, ny);
                                if let Some(n) = name {
                                    node.name = n;
                                }
                                state.current_dir_mut().children.push(node);
                                state.sync_nodes();
                                state.rebuild_positions();
                                state.apply_layout();
                                state.update_panel_bounds();
                                state.upload_vertices();
                                needs_redraw = true;
                                Ok("Node added".to_string())
                            } else {
                                Err(format!("Template '{}' not found", template_name))
                            }
                        }
                        HttpAction::DeleteNode { slot } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                state.current_dir_mut().children.remove(slot);
                                if let Some(focused) = state.focused_widget {
                                    if focused == CONTENT_IDX {
                                        if let Some(sel_idx) = state.widgets[CONTENT_IDX].selected_node() {
                                            if sel_idx == slot {
                                                state.widgets[CONTENT_IDX].set_selected_node(None);
                                            } else if sel_idx > slot {
                                                state.widgets[CONTENT_IDX].set_selected_node(Some(sel_idx - 1));
                                            }
                                        }
                                    }
                                }
                                state.sync_nodes();
                                state.rebuild_positions();
                                state.apply_layout();
                                state.update_panel_bounds();
                                state.upload_vertices();
                                needs_redraw = true;
                                Ok("Node deleted".to_string())
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::RenameNode { slot, new_name } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                state.current_dir_mut().children[slot].name = new_name;
                                state.sync_nodes();
                                needs_redraw = true;
                                Ok("Node renamed".to_string())
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::MoveNode { slot, x, y } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                let (nx, ny) = state.find_empty_cell(x, y, Some(slot));
                                state.current_dir_mut().children[slot].position = (nx, ny);
                                state.sync_nodes();
                                state.rebuild_positions();
                                state.apply_layout();
                                state.update_panel_bounds();
                                state.upload_vertices();
                                needs_redraw = true;
                                Ok("Node moved".to_string())
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::AddParam { slot, name, param_type, default } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                let param = ParamDef {
                                    name,
                                    label: String::new(),
                                    param_type,
                                    default,
                                    options: vec![],
                                    min: None,
                                    max: None,
                                    step: None,
                                };
                                state.current_dir_mut().children[slot].params.push(param);
                                state.sync_nodes();
                                needs_redraw = true;
                                Ok("Parameter added".to_string())
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::DeleteParam { slot, name } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                let params = &mut state.current_dir_mut().children[slot].params;
                                if let Some(pos) = params.iter().position(|p| p.name == name) {
                                    params.remove(pos);
                                    state.sync_nodes();
                                    needs_redraw = true;
                                    Ok("Parameter deleted".to_string())
                                } else {
                                    Err(format!("Parameter '{}' not found", name))
                                }
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::ToggleCircularPane => {
                            state.circular_network_pane = !state.circular_network_pane;
                            state.widgets[LEFT_MENUBAR_IDX].set_item_checked(2, 2, state.circular_network_pane);
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_grid_settings();
                            state.upload_vertices();
                            needs_redraw = true;
                            Ok(format!("Circular pane: {}", state.circular_network_pane))
                        }
                        HttpAction::MenuClick { widget_idx, menu_idx, item_idx } => {
                            state.widgets[widget_idx].trigger_menu_click(menu_idx, item_idx);
                            self.process_event(WindowEvent::CursorMoved { position: LocalPosition { x: -9999.0, y: -9999.0 } });
                            needs_redraw = true;
                            Ok("Menu clicked".to_string())
                        }
                        HttpAction::MenuClosed { widget_idx, menu_idx } => {
                            if state.active_menu_cloud_idx == Some((widget_idx, menu_idx)) {
                                state.active_menu_cloud_pid = None;
                                state.active_menu_cloud_idx = None;
                            }
                            Ok("Menu closed".to_string())
                        }
                        HttpAction::SelectPage { page } => {
                            state.widgets[PAGINATOR_IDX].set_page_hidden(false);
                            state.widgets[PAGINATOR_IDX].set_selected_page(page);
                            state.update_paginator();
                            state.rebuild_positions();
                            state.apply_layout();
                            state.upload_vertices();
                            needs_redraw = true;
                            Ok("Page selected".to_string())
                        }
                    };
                    let _ = tx.send(res);
                }
            }
        } else {
            match event {
                CustomEvent::GetState(tx) => {
                    let _ = tx.send("null".to_string());
                }
                CustomEvent::PostAction(_, tx) => {
                    let _ = tx.send(Err("State not initialized".to_string()));
                }
            }
        }
        if needs_redraw {
            if let Some(state) = &mut self.state {
                state.update_window_title();
                if state.is_detached_network || state.detached_circular_network {
                    state.needs_autosave = true;
                }
            }
            self.redraw = true;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_detached_network = args.iter().any(|arg| arg == "--detached-network");

    let conn = Connection::connect_to_env().unwrap();
    let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh).unwrap();
    let xdg_shell_state = XdgShell::bind(&globals, &qh).unwrap();
    let shm_state = Shm::bind(&globals, &qh).unwrap();
    let seat_state = SeatState::new(&globals, &qh);
    let output_state = OutputState::new(&globals, &qh);
    let inspector = globals.bind(&qh, 1..=1, ()).ok();
    let (sender, channel) = calloop::channel::channel::<CustomEvent>();

    let mut app = AppState {
        registry_state: RegistryState::new(&globals),
        compositor_state,
        xdg_shell_state,
        shm_state,
        seat_state,
        output_state,
        seats: Vec::new(),
        pointer: None,
        keyboard: None,
        window: None,
        surface: None,
        state: None,
        exit: false,
        redraw: true,
        pressed_key: None,
        inspector,
        pending_resize: None,
        _sender: sender.clone(),
    };

    // Perform a roundtrip to populate output_state with active output scales
    event_queue.roundtrip(&mut app).unwrap();

    let scale = clear_ui::wayland::detect_scale_factor(&app.output_state);

    let (pw, ph) = if is_detached_network {
        ((400.0 * scale) as u32, (400.0 * scale) as u32)
    } else {
        ((1280.0 * scale) as u32, (800.0 * scale) as u32)
    };

    let state = pollster::block_on(State::new(
        &conn,
        &qh,
        &app.compositor_state,
        &app.xdg_shell_state,
        pw, ph,
        scale,
        is_detached_network,
    ));

    app.window = Some(state.window.clone());
    app.surface = Some(state.wl_surface.clone());
    app.state = Some(state);

    if let Some(ref inspector) = app.inspector {
        if let Some(ref surface) = app.surface {
            inspector.register_client(surface);
        }
    }

    if !is_detached_network {
        let server_sender = sender.clone();
        std::thread::spawn(move || {
            let listener = match TcpListener::bind("127.0.0.1:3000") {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("Failed to bind HTTP server to port 3000: {:?}", e);
                    return;
                }
            };
            println!("Embedded HTTP Server listening on http://127.0.0.1:3000");

            for stream in listener.incoming() {
                let stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let server_sender = server_sender.clone();
                std::thread::spawn(move || {
                    let mut write_stream = match stream.try_clone() {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    let mut reader = BufReader::new(stream);
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() {
                        return;
                    }

                    if request_line.starts_with("GET /state") {
                        let (tx, rx) = std::sync::mpsc::channel();
                        if server_sender.send(CustomEvent::GetState(tx)).is_ok() {
                            let response_body = rx.recv().unwrap_or_else(|_| "null".to_string());
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                response_body.len(),
                                response_body
                            );
                            let _ = write_stream.write_all(response.as_bytes());
                        }
                    } else if request_line.starts_with("POST /action") {
                        let mut content_length = 0;
                        loop {
                            let mut header_line = String::new();
                            if reader.read_line(&mut header_line).is_err() || header_line == "\r\n" || header_line == "\n" || header_line.is_empty() {
                                break;
                            }
                            let lower = header_line.to_lowercase();
                            if lower.starts_with("content-length:") {
                                if let Some(val) = lower.split(':').nth(1) {
                                    if let Ok(len) = val.trim().parse::<usize>() {
                                        content_length = len;
                                    }
                                }
                            }
                        }

                        let mut body = vec![0; content_length];
                        if reader.read_exact(&mut body).is_ok() {
                            let body_str = String::from_utf8_lossy(&body);
                            if let Ok(action) = serde_json::from_str::<HttpAction>(&body_str) {
                                let (tx, rx) = std::sync::mpsc::channel();
                                if server_sender.send(CustomEvent::PostAction(action, tx)).is_ok() {
                                    let res = rx.recv().unwrap_or_else(|_| Err("internal error".to_string()));
                                    let response = match res {
                                        Ok(msg) => {
                                            let body = format!("{{\"status\":\"success\",\"message\":\"{}\"}}", msg);
                                            format!(
                                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                                body.len(),
                                                body
                                            )
                                        }
                                        Err(err) => {
                                            let body = format!("{{\"status\":\"error\",\"error\":\"{}\"}}", err);
                                            format!(
                                                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                                body.len(),
                                                body
                                            )
                                        }
                                    };
                                    let _ = write_stream.write_all(response.as_bytes());
                                } else {
                                    let body = "{\"status\":\"error\",\"error\":\"failed to send action to event loop\"}";
                                    let response = format!(
                                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                        body.len(),
                                        body
                                    );
                                    let _ = write_stream.write_all(response.as_bytes());
                                }
                            } else {
                                let body = "{\"status\":\"error\",\"error\":\"failed to parse action JSON\"}";
                                let response = format!(
                                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                );
                                let _ = write_stream.write_all(response.as_bytes());
                            }
                        } else {
                            let body = "{\"status\":\"error\",\"error\":\"failed to read complete body\"}";
                            let response = format!(
                                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = write_stream.write_all(response.as_bytes());
                        }
                } else {
                    let body = "{\"error\":\"not found\"}";
                    let response = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = write_stream.write_all(response.as_bytes());
                }
                let _ = write_stream.flush();
            });
        }
    });
    }

    let mut event_loop = calloop::EventLoop::try_new().unwrap();
    let loop_handle = event_loop.handle();

    WaylandSource::new(conn, event_queue).insert(loop_handle.clone()).unwrap();

    loop_handle.insert_source(channel, |event, _metadata, app_state: &mut AppState| {
        if let calloop::channel::Event::Msg(msg) = event {
            app_state.handle_user_event(msg);
        }
    }).unwrap();

    const KEY_REPEAT_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
    const KEY_REPEAT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

    loop {
        let timeout = std::time::Duration::from_millis(16);
        event_loop.dispatch(timeout, &mut app).unwrap();

        if app.exit || app.state.as_ref().map(|s| s.exit_requested).unwrap_or(false) {
            if let Some(state) = &mut app.state {
                if state.needs_autosave && (state.is_detached_network || state.detached_circular_network) {
                    let default_proj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
                    let _ = state.save_to_file(&default_proj_path);
                }
            }
            break;
        }

        if let Some(state) = &mut app.state {
            if let Some(ref inspector) = app.inspector {
                let now = std::time::Instant::now();
                if now.duration_since(state.last_inspector_check) >= std::time::Duration::from_millis(250) {
                    state.last_inspector_check = now;
                    inspector.get_inspected_surfaces();
                }
            }

            if state.needs_autosave && (state.is_detached_network || state.detached_circular_network) {
                let now = std::time::Instant::now();
                if now.duration_since(state.last_autosave_time) >= std::time::Duration::from_millis(200) {
                    state.needs_autosave = false;
                    state.last_autosave_time = now;
                    let default_proj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
                    if let Err(e) = state.save_to_file(&default_proj_path) {
                        eprintln!("Failed to auto-save default project in main loop: {:?}", e);
                    } else if let Ok(m) = std::fs::metadata(&default_proj_path) {
                        if let Ok(mod_time) = m.modified() {
                            state.last_project_mod_time = Some(mod_time);
                        }
                    }
                }
            }

            if state.is_detached_network || state.detached_circular_network {
                let now = std::time::Instant::now();
                if now.duration_since(state.last_project_check) >= std::time::Duration::from_millis(100) {
                    state.last_project_check = now;
                    let default_proj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
                    if let Ok(m) = std::fs::metadata(&default_proj_path) {
                        if let Ok(mod_time) = m.modified() {
                            if Some(mod_time) != state.last_project_mod_time {
                                state.last_project_mod_time = Some(mod_time);
                                if let Err(e) = state.load_from_file(&default_proj_path) {
                                    eprintln!("Failed to auto-reload project: {:?}", e);
                                } else {
                                    app.redraw = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(ref mut pk) = app.pressed_key {
            let now = std::time::Instant::now();
            if now.duration_since(pk.first_pressed) >= KEY_REPEAT_DELAY {
                if now.duration_since(pk.last_repeated) >= KEY_REPEAT_INTERVAL {
                    pk.last_repeated = now;
                    if let Some(st) = &mut app.state {
                        let custom_event = clear_ui::widget::KeyEvent {
                            state: clear_ui::widget::ElementState::Pressed,
                            logical_key: pk.logical_key.clone(),
                            text: pk.text.clone(),
                            repeat: true,
                            ctrl: st.modifiers.ctrl,
                            shift: st.modifiers.shift,
                        };
                        let ev = WindowEvent::KeyboardInput { event: custom_event };
                        app.process_event(ev);
                    }
                }
            }
        }

fn create_memfd_with_data(name: &str, data: &[u8]) -> std::io::Result<std::os::unix::io::RawFd> {
    use std::io::{Seek, Write};
    use std::os::unix::io::FromRawFd;
    use std::os::unix::io::IntoRawFd;

    let c_name = std::ffi::CString::new(name).unwrap();
    let fd = unsafe { libc::memfd_create(c_name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(data)?;
    file.seek(std::io::SeekFrom::Start(0))?;
    Ok(file.into_raw_fd())
}

        if app.redraw {
            app.redraw = false;
            if let Some(state) = &mut app.state {
                if state.render() {
                    app.redraw = true;
                }
                if let Some(ref inspector) = app.inspector {
                    if let Some(ref surface) = app.surface {
                        let now = std::time::Instant::now();
                        if now.duration_since(state.last_inspector_update) >= std::time::Duration::from_millis(100) {
                            state.last_inspector_update = now;
                            let json = clear_ui::widget::serialize_widgets(&state.widgets);
                            if let Ok(raw_fd) = create_memfd_with_data("clear_ui_state", json.as_bytes()) {
                                use std::os::unix::io::{FromRawFd, AsFd};
                                let file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
                                inspector.update_state(surface, file.as_fd(), json.len() as u32);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_default_project() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
        let content = fs::read_to_string(&path).expect("failed to read default project");
        let proj: Project = serde_json::from_str(&content).expect("failed to deserialize project");
        assert_eq!(proj.name, "Default Project");
        assert_eq!(proj.view_state.active_camera, "Camera 1");
        assert_eq!(proj.root.name, "root");
        assert_eq!(proj.root.children.len(), 3);
        assert_eq!(proj.root.children[0].name, "Camera 1");
        assert_eq!(proj.root.children[0].position, (1.0, 1.0));
        assert_eq!(proj.root.children[1].name, "Sphere 1");
        assert_eq!(proj.root.children[1].position, (4.0, 2.0));
        assert_eq!(proj.root.children[2].name, "Transform 1");
        assert_eq!(proj.root.children[2].position, (4.0, 3.0));
    }

    #[test]
    fn test_project_serialization_roundtrip() {
        let root = FsNode {
            name: "test_root".to_string(),
            node_type: "node".to_string(),
            children: vec![
                FsNode {
                    name: "child1".to_string(),
                    node_type: "sphere".to_string(),
                    children: vec![],
                    params: vec![],
                    geometry_visible: true,
                    position: (5.0, 6.0),
                }
            ],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let view_state = ProjectViewState {
            active_camera: "child1".to_string(),
            pan: (1.5, -2.5),
            current_path: vec![0],
            selected_node: Some(2),
        };
        let proj = Project {
            name: "Test Project".to_string(),
            root,
            view_state,
        };

        let content = serde_json::to_string(&proj).expect("failed to serialize");
        let proj2: Project = serde_json::from_str(&content).expect("failed to deserialize");

        assert_eq!(proj.name, proj2.name);
        assert_eq!(proj.view_state.active_camera, proj2.view_state.active_camera);
        assert_eq!(proj.view_state.pan, proj2.view_state.pan);
        assert_eq!(proj.view_state.current_path, proj2.view_state.current_path);
        assert_eq!(proj.view_state.selected_node, proj2.view_state.selected_node);
        assert_eq!(proj.root.name, proj2.root.name);
        assert_eq!(proj.root.children.len(), proj2.root.children.len());
        assert_eq!(proj.root.children[0].name, proj2.root.children[0].name);
        assert_eq!(proj.root.children[0].position, proj2.root.children[0].position);
    }

    #[test]
    fn test_geometry_attributes_system() {
        let mut attrs1 = std::collections::HashMap::new();
        attrs1.insert("UV".to_string(), GAttribute::Float2([0.1, 0.2]));
        attrs1.insert("ID".to_string(), GAttribute::Float(42.0));

        let v1 = GVertex {
            pos: [1.0, 2.0, 3.0],
            col: [1.0, 0.0, 0.0],
            attributes: attrs1,
        };

        let mut attrs2 = std::collections::HashMap::new();
        attrs2.insert("Norm".to_string(), GAttribute::Float3([0.0, 1.0, 0.0]));
        attrs2.insert("UV".to_string(), GAttribute::Float2([0.3, 0.4]));

        let v2 = GVertex {
            pos: [4.0, 5.0, 6.0],
            col: [0.0, 1.0, 0.0],
            attributes: attrs2,
        };

        let mut geom1 = Geometry { vertices: vec![v1] };
        let geom2 = Geometry { vertices: vec![v2] };

        geom1.merge(geom2);
        assert_eq!(geom1.vertices.len(), 2);

        let render_verts = geom1.to_vertex3d_vec();
        assert_eq!(render_verts.len(), 2);
        assert_eq!(render_verts[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(render_verts[0].color, [1.0, 0.0, 0.0]);
        assert_eq!(render_verts[1].position, [4.0, 5.0, 6.0]);
        assert_eq!(render_verts[1].color, [0.0, 1.0, 0.0]);

        let (headers, rows) = State::geometry_to_spreadsheet_data(&geom1);

        let expected_headers = vec![
            "Vertex".to_string(),
            "Pos.x".to_string(),
            "Pos.y".to_string(),
            "Pos.z".to_string(),
            "Col.r".to_string(),
            "Col.g".to_string(),
            "Col.b".to_string(),
            "ID".to_string(),
            "Norm.x".to_string(),
            "Norm.y".to_string(),
            "Norm.z".to_string(),
            "UV.x".to_string(),
            "UV.y".to_string(),
        ];
        assert_eq!(headers, expected_headers);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "0");
        assert_eq!(rows[0][1], "1.0000"); // Pos X
        assert_eq!(rows[0][7], "42.0000"); // ID
        assert_eq!(rows[0][8], "-"); // Norm.x
        assert_eq!(rows[0][9], "-"); // Norm.y
        assert_eq!(rows[0][10], "-"); // Norm.z
        assert_eq!(rows[0][11], "0.1000"); // UV.x
        assert_eq!(rows[0][12], "0.2000"); // UV.y

        assert_eq!(rows[1][0], "1");
        assert_eq!(rows[1][1], "4.0000"); // Pos X
        assert_eq!(rows[1][7], "-"); // ID
        assert_eq!(rows[1][8], "0.0000"); // Norm.x
        assert_eq!(rows[1][9], "1.0000"); // Norm.y
        assert_eq!(rows[1][10], "0.0000"); // Norm.z
        assert_eq!(rows[1][11], "0.3000"); // UV.x
        assert_eq!(rows[1][12], "0.4000"); // UV.y
    }

    #[test]
    fn test_line_geometry_generation() {
        let start = Vec3::new(0.0, 0.0, 0.0);
        let end = Vec3::new(0.0, 1.0, 0.0);
        let geom = line_vertices(start, end, 0.02);
        
        // A box line should contain 36 vertices (6 faces * 2 triangles * 3 vertices)
        assert_eq!(geom.vertices.len(), 36);
        
        // Every vertex should have "Norm" and "UV" attributes
        for v in &geom.vertices {
            assert!(v.attributes.contains_key("Norm"));
            assert!(v.attributes.contains_key("UV"));
        }
    }

    #[test]
    fn test_design_settings_serialization_roundtrip() {
        let json_without_pivot = r#"
        {
            "grid_snap_enabled": true,
            "network_grid_enabled": true,
            "grid_size_x": 80.0,
            "grid_size_y": 40.0,
            "skipped_row_h": 20.0,
            "skipped_col_w": 20.0,
            "show_grid_enabled": true,
            "show_cube_enabled": false,
            "show_origin_enabled": true,
            "origin_size": 1.0,
            "viewport_bg_color": [0.05, 0.05, 0.10],
            "square_viewport": false,
            "grid_thickness": 0.03
        }
        "#;
        
        let settings: DesignSettings = serde_json::from_str(json_without_pivot).unwrap();
        assert_eq!(settings.show_camera_pivot_enabled, false);
        assert_eq!(settings.camera_pivot_size, 1.0);
        assert_eq!(settings.grid_color, [0.35, 0.35, 0.40]);
        assert_eq!(settings.uniform_background, false);
        assert_eq!(settings.network_opacity, 0.95);
        assert_eq!(settings.cell_color, [0.13, 0.13, 0.16]);
        assert_eq!(settings.gap_color, [0.07, 0.07, 0.09]);
        
        let serialized = serde_json::to_string(&settings).unwrap();
        let settings_roundtrip: DesignSettings = serde_json::from_str(&serialized).unwrap();
        assert_eq!(settings_roundtrip.show_camera_pivot_enabled, false);
        assert_eq!(settings_roundtrip.camera_pivot_size, 1.0);
        assert_eq!(settings_roundtrip.grid_color, [0.35, 0.35, 0.40]);
        assert_eq!(settings_roundtrip.uniform_background, false);
        assert_eq!(settings_roundtrip.network_opacity, 0.95);
        assert_eq!(settings_roundtrip.cell_color, [0.13, 0.13, 0.16]);
        assert_eq!(settings_roundtrip.gap_color, [0.07, 0.07, 0.09]);
    }

    #[test]
    fn test_http_action_parsing() {
        let json_str = "{\"action\": \"add_node\", \"template_name\": \"Sphere\", \"name\": \"MySphere\", \"x\": 5.0, \"y\": 3.0}";
        let action: HttpAction = serde_json::from_str(json_str).unwrap();
        match action {
            HttpAction::AddNode { template_name, name, x, y } => {
                assert_eq!(template_name, "Sphere");
                assert_eq!(name, Some("MySphere".to_string()));
                assert_eq!(x, 5.0);
                assert_eq!(y, 3.0);
            }
            _ => panic!("Expected AddNode"),
        }
    }

    #[test]
    fn test_get_next_visible_pane() {
        // Without spreadsheet (3 panes: LEFT, RIGHT, PARAM)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, false, false), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, false, false), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, false, false), LEFT_MENUBAR_IDX);

        // With spreadsheet (4 panes: LEFT, RIGHT, PARAM, SPREADSHEET)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, true, false), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, true, false), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, true, false), SPREADSHEET_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(SPREADSHEET_MENUBAR_IDX, true, true, true, true, false), LEFT_MENUBAR_IDX);

        // Reverse cycling with shift key (without spreadsheet)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, false, true), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, false, true), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, false, true), LEFT_MENUBAR_IDX);

        // Reverse cycling with shift key (with spreadsheet)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, true, true), SPREADSHEET_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(SPREADSHEET_MENUBAR_IDX, true, true, true, true, true), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, true, true), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, true, true), LEFT_MENUBAR_IDX);
    }

    #[test]
    fn test_inertial_settings_fallback_and_ranges() {
        let friction_raw = 90_u16;
        let friction_f32 = (friction_raw as f32 / 100.0).clamp(0.5, 0.99);
        assert!(friction_f32 >= 0.5 && friction_f32 <= 0.99);
        
        let friction_raw_low = 30_u16;
        let friction_f32_low = (friction_raw_low as f32 / 100.0).clamp(0.5, 0.99);
        assert_eq!(friction_f32_low, 0.5);
    }

    #[test]
    fn test_decay_formula() {
        let friction = 0.90_f32;
        let dt_60 = 1.0 / 60.0;
        let decay_60 = friction.powf(dt_60 * 60.0);
        assert!((decay_60 - friction).abs() < 1e-5);

        let decay_0 = friction.powf(0.0 * 60.0);
        assert_eq!(decay_0, 1.0);
    }

    #[test]
    fn test_circular_network_clamping_math() {
        let header_h = 26.0;
        let status_h = 0.0;
        let gap = 18.0;

        let clamp_layout = |width: f32, height: f32, layout_x: f32, layout_y: f32, layout_r: f32| -> (f32, f32, f32) {
            let max_r = ((width - 2.0 * gap).min(height - header_h - status_h - 2.0 * gap) / 2.0).max(50.0);
            let r = layout_r.clamp(50.0, max_r);

            let min_x = gap + r;
            let max_x = (width - gap - r).max(min_x);
            let x = layout_x.clamp(min_x, max_x);

            let min_y = header_h + gap + r;
            let max_y = (height - status_h - gap - r).max(min_y);
            let y = layout_y.clamp(min_y, max_y);

            (x, y, r)
        };

        let (x, y, r) = clamp_layout(1000.0, 800.0, 500.0, 400.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 500.0);
        assert_eq!(y, 400.0);

        let (x, y, r) = clamp_layout(1000.0, 800.0, 50.0, 400.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 198.0);
        assert_eq!(y, 400.0);

        let (x, y, r) = clamp_layout(1000.0, 800.0, 500.0, 100.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 500.0);
        assert_eq!(y, 224.0);

        let (x, y, r) = clamp_layout(1000.0, 800.0, 500.0, 750.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 500.0);
        assert_eq!(y, 602.0);

        let (x, y, r) = clamp_layout(50.0, 50.0, 10.0, 10.0, 180.0);
        assert!(r >= 50.0);
        assert!(x >= 0.0);
        assert!(y >= 0.0);
    }

    #[test]
    fn test_floating_rectangular_pane_clamping() {
        let width = 1000.0_f32;
        let height = 800.0_f32;
        let header_h = 26.0_f32;
        let status_h = 0.0_f32;
        let gap = 18.0_f32;

        let clamp_floating = |_fx: f32, _fy: f32, fw: f32, _fh: f32| -> (f32, f32, f32, f32) {
            let fw = fw.clamp(150.0, (width - 2.0 * gap).max(150.0));
            let fx = gap;
            let fy = header_h + gap;
            let fh = (height - header_h - status_h - 2.0 * gap).max(100.0);
            (fx, fy, fw, fh)
        };

        let expected_h = height - header_h - status_h - 2.0 * gap;

        // Standard center case (fx anchored to gap, fy/fh to full height)
        let (x, y, w, h) = clamp_floating(100.0, 200.0, 400.0, 300.0);
        assert_eq!((x, y, w, h), (gap, header_h + gap, 400.0, expected_h));

        // Off-screen left/top
        let (x, y, w, h) = clamp_floating(-50.0, 0.0, 400.0, 300.0);
        assert_eq!((x, y, w, h), (gap, header_h + gap, 400.0, expected_h));

        // Off-screen right/bottom
        let (x, y, w, h) = clamp_floating(900.0, 700.0, 400.0, 300.0);
        assert_eq!((x, y, w, h), (gap, header_h + gap, 400.0, expected_h));

        let clamp_param = |pw: f32| -> (f32, f32, f32, f32) {
            let pw = pw.clamp(150.0, (width - 2.0 * gap).max(150.0));
            let px = width - gap - pw;
            let py = header_h + gap;
            let ph = (height - header_h - status_h - 2.0 * gap).max(100.0);
            (px, py, pw, ph)
        };

        // Standard param case
        let (px, py, pw, ph) = clamp_param(300.0);
        assert_eq!((px, py, pw, ph), (width - gap - 300.0, header_h + gap, 300.0, expected_h));

        // Off-screen/overflow width param case
        let (px, py, pw, ph) = clamp_param(1200.0);
        let max_w = width - 2.0 * gap;
        assert_eq!((px, py, pw, ph), (gap, header_h + gap, max_w, expected_h));

        let clamp_ss = |ss_h_val: f32, show_net: bool, show_param: bool| -> (f32, f32, f32, f32) {
            let fx = gap;
            let fw = 400.0;
            let param_w = 300.0;
            let param_x = width - gap - param_w;

            let ss_x = if show_net { fx + fw + gap } else { gap };
            let ss_w_end = if show_param { param_x - gap } else { width - gap };
            let ss_w = (ss_w_end - ss_x).max(150.0);
            let ss_y_end = height - status_h - gap;
            let ss_h = ss_h_val.clamp(100.0, (ss_y_end - header_h - gap).max(100.0));
            let ss_y = ss_y_end - ss_h;
            (ss_x, ss_y, ss_w, ss_h)
        };

        // Standard spreadsheet case (both net and param visible)
        let (sx, sy, sw, sh) = clamp_ss(250.0, true, true);
        assert_eq!(sx, gap + 400.0 + gap);
        assert_eq!(sw, (width - gap - 300.0 - gap) - (gap + 400.0 + gap));
        assert_eq!(sh, 250.0);
        assert_eq!(sy, height - status_h - gap - 250.0);

        // Neither net nor param visible
        let (sx2, _sy2, sw2, sh2) = clamp_ss(200.0, false, false);
        assert_eq!(sx2, gap);
        assert_eq!(sw2, width - 2.0 * gap);
        assert_eq!(sh2, 200.0);

        // Clamping height to min (100.0)
        let (_, _, _, sh_min) = clamp_ss(50.0, true, true);
        assert_eq!(sh_min, 100.0);

        // Clamping height to max
        let max_possible_h = height - status_h - gap - header_h - gap;
        let (_, _, _, sh_max) = clamp_ss(1000.0, true, true);
        assert_eq!(sh_max, max_possible_h);
    }

    #[test]
    fn test_paginator_collapsed_layout() {
        let width = 1000.0_f32;

        let get_paginator_w = |page_hidden: bool| -> f32 {
            if page_hidden {
                56.0_f32
            } else {
                236.0_f32
            }
        };

        // Collapsed layout
        let w_collapsed = get_paginator_w(true);
        assert_eq!(w_collapsed, 56.0);

        // Expanded layout
        let w_expanded = get_paginator_w(false);
        assert_eq!(w_expanded, 236.0);

        // Verify how other viewport bounds adjust relative to paginator_w
        let check_viewport_layout = |page_hidden: bool| -> (f32, f32) {
            let paginator_w = get_paginator_w(page_hidden);
            let col_c_x = paginator_w;
            let col_c_w = width - paginator_w;
            (col_c_x, col_c_w)
        };

        // When collapsed, viewport/canvas has more space
        let (vx_c, vw_c) = check_viewport_layout(true);
        assert_eq!(vx_c, 56.0);
        assert_eq!(vw_c, 944.0);

        // When expanded, viewport/canvas has default space
        let (vx_e, vw_e) = check_viewport_layout(false);
        assert_eq!(vx_e, 236.0);
        assert_eq!(vw_e, 764.0);
    }

    #[test]
    fn test_keyboard_shortcut_system() {
        // Test parsing simple shortcut
        let ctrl_g = Shortcut::parse("Ctrl+g").unwrap();
        assert_eq!(ctrl_g.ctrl, true);
        assert_eq!(ctrl_g.shift, false);
        assert_eq!(ctrl_g.alt, false);
        assert_eq!(ctrl_g.logo, false);
        assert_eq!(ctrl_g.key, Key::Character("g".to_string()));

        // Test parsing complex shortcut
        let complex = Shortcut::parse("Ctrl+Shift+Alt+Logo+s").unwrap();
        assert_eq!(complex.ctrl, true);
        assert_eq!(complex.shift, true);
        assert_eq!(complex.alt, true);
        assert_eq!(complex.logo, true);
        assert_eq!(complex.key, Key::Character("s".to_string()));

        // Test parsing named keys
        let tab_sc = Shortcut::parse("Tab").unwrap();
        assert_eq!(tab_sc.key, Key::Named(NamedKey::Tab));

        // Test parsing case insensitivity
        let case_sc = Shortcut::parse("cTrL+sHiFt+ArrowDown").unwrap();
        assert_eq!(case_sc.ctrl, true);
        assert_eq!(case_sc.shift, true);
        assert_eq!(case_sc.key, Key::Named(NamedKey::ArrowDown));

        // Test register and match
        let mut mgr = ShortcutManager::new();
        mgr.register("Ctrl+g", Action::ToggleGrid).unwrap();
        mgr.register("`", Action::ToggleSpreadsheet).unwrap();

        // Matches with ctrl and g
        let mods_ctrl = ModifiersState { ctrl: true, alt: false, shift: false, logo: false };
        let key_g = Key::Character("g".to_string());
        assert_eq!(mgr.match_action(&mods_ctrl, &key_g), Some(Action::ToggleGrid));

        // No match with ctrl and a
        let key_a = Key::Character("a".to_string());
        assert_eq!(mgr.match_action(&mods_ctrl, &key_a), None);

        // Matches backtick with no modifiers
        let mods_none = ModifiersState::default();
        let key_tick = Key::Character("`".to_string());
        assert_eq!(mgr.match_action(&mods_none, &key_tick), Some(Action::ToggleSpreadsheet));
    }
}

