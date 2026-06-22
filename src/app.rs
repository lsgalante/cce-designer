#![allow(unused_imports)]
use std::time::Instant;
use std::fs;
use std::path::Path;
use std::net::TcpListener;
use std::io::BufReader;

use serde::{Deserialize, Serialize};

use cce_ui::widget::{ElementState, MouseButton, MouseScrollDelta, KeyEvent, Key, NamedKey};

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
            window::{Window as XdgWindow, WindowConfigure, WindowDecorations},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    protocol::wl_surface,
    Connection, QueueHandle, Proxy,
};

use wgpu::util::DeviceExt;
use cce_ui::widget::{Breadcrumb, Canvas, MenuBar, Plate, ParametersBg, Splitter, Spreadsheet, StatusBar, TextLabel, ViewportBg, Element, GraphNode, Graph, Button, Checkbox, ScrollingList, Label};
use cce_ui::colors;
use glyphon::{Attrs, Buffer, Cache, FontSystem, Metrics, Resolution, TextAtlas, TextRenderer, Viewport};
use glam::{Mat4, Vec3};

use crate::geometry::*;
use crate::shortcut::{ShortcutManager, Action};
use crate::graphics::TexturedVertex;
use cce_ui::engine::Vertex;
use crate::window::{AppState, WindowEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModifiersState {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

impl ModifiersState {
    pub fn control_key(&self) -> bool { self.ctrl }
    pub fn alt_key(&self) -> bool { self.alt }
    pub fn shift_key(&self) -> bool { self.shift }
    pub fn super_key(&self) -> bool { self.logo }
    pub fn state(&self) -> Self { *self }
}

pub const HEADER_IDX: usize = 0;
pub const CONTENT_IDX: usize = 1;
pub const SPLITTER1_IDX: usize = 2;
pub const VIEWPORT_IDX: usize = 3;
pub const SPLITTER2_IDX: usize = 4;
pub const PARAM_PLATE_IDX: usize = 5;
pub const PARAM_IDX: usize = 6;
pub const CANVAS_IDX: usize = 7;
pub const LEFT_MENUBAR_IDX: usize = 8;
pub const RIGHT_MENUBAR_IDX: usize = 9;
pub const PARAM_MENUBAR_IDX: usize = 10;
pub const STATUS_IDX: usize = 11;
pub const BREADCRUMB_IDX: usize = 12;
pub const NODE_PALETTE_IDX: usize = 13;
pub const SPREADSHEET_IDX: usize = 14;
pub const SPREADSHEET_MENUBAR_IDX: usize = 15;
pub const NETWORK_PANEL_IDX: usize = 16;


pub const HEADER_H: f32 = 0.0;
pub const STATUS_H: f32 = 0.0;
pub const MENUBAR_H: f32 = 0.0;
pub const SPLITTER_W: f32 = 6.0;

pub const MIN_COLUMN: f32 = 120.0;
pub const BREADCRUMB_H: f32 = 24.0;

#[derive(Clone, Deserialize, Serialize)]
pub struct ParamDef {
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(rename = "type")]
    #[serde(default = "default_param_type")]
    pub param_type: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub min: Option<f32>,
    #[serde(default)]
    pub max: Option<f32>,
    #[serde(default)]
    pub step: Option<f32>,
}

fn default_param_type() -> String { "string".to_string() }

static NODE_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn generate_node_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = NODE_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("node_{:x}_{:x}", now, count)
}

fn default_node_inputs() -> usize { 1 }
fn default_node_outputs() -> usize { 1 }

#[derive(Clone, Deserialize, Serialize)]
pub struct FsNode {
    #[serde(default = "generate_node_id")]
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    #[serde(default = "default_node_type")]
    pub node_type: String,
    #[serde(default)]
    pub children: Vec<FsNode>,
    #[serde(default)]
    pub params: Vec<ParamDef>,
    #[serde(skip)]
    #[serde(default = "default_node_geometry_visible")]
    pub geometry_visible: bool,
    #[serde(default = "default_node_position")]
    pub position: (f32, f32),
    #[serde(default = "default_node_inputs")]
    pub inputs: usize,
    #[serde(default = "default_node_outputs")]
    pub outputs: usize,
}

fn default_node_type() -> String { "node".to_string() }
fn default_node_geometry_visible() -> bool { true }
fn default_node_position() -> (f32, f32) { (0.0, 0.0) }

#[derive(Clone, Deserialize, Serialize, Default)]
pub struct ProjectViewState {
    #[serde(default = "default_camera")]
    pub active_camera: String,
    #[serde(default)]
    pub pan: (f32, f32),
    #[serde(default)]
    pub current_path: Vec<usize>,
    #[serde(default)]
    pub selected_node: Option<usize>,
}

fn default_camera() -> String {
    "Default Camera".to_string()
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Project {
    pub name: String,
    pub root: FsNode,
    #[serde(default)]
    pub view_state: ProjectViewState,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum HttpAction {
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
}

#[derive(Debug)]
pub enum CustomEvent {
    GetState(std::sync::mpsc::Sender<String>),
    PostAction(HttpAction, std::sync::mpsc::Sender<Result<String, String>>),
}

#[derive(Clone)]
pub struct NodeTemplate {
    pub label: String,
    pub node: FsNode,
}

pub fn param_display(params: &[ParamDef]) -> Vec<(String, String, String)> {
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
        } else if p.param_type == "choice" {
            format!("choice:{}", p.options.join(","))
        } else {
            p.param_type.clone()
        };
        (key.clone(), value, ptype)
    }).collect()
}

pub fn flatten_node_templates(root: &FsNode) -> Vec<NodeTemplate> {
    let mut out = Vec::new();
    for child in &root.children {
        out.push(NodeTemplate { label: child.name.clone(), node: child.clone() });
    }
    out
}

pub fn load_fs_tree() -> FsNode {
    let nodes_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("nodes");
    let mut children = Vec::new();
    if let Ok(entries) = fs::read_dir(&nodes_dir) {
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        paths.sort();
        
        let mut raw_nodes = Vec::new();
        for path in paths {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(node) = serde_json::from_str::<FsNode>(&content) {
                    raw_nodes.push(node);
                }
            }
        }

        for mut node in raw_nodes.clone() {
            let mut resolved_children = Vec::new();
            for child in &node.children {
                let base_template = raw_nodes.iter().find(|t| {
                    t.node_type == child.node_type || t.name.to_lowercase() == child.node_type.to_lowercase()
                });
                if let Some(base) = base_template {
                    let mut resolved_child = base.clone();
                    resolved_child.id = child.id.clone();
                    if resolved_child.id.is_empty() || resolved_child.id == "node_0_0" || resolved_child.id.starts_with("node_") {
                        resolved_child.id = generate_node_id();
                    }
                    resolved_child.name = child.name.clone();
                    resolved_child.position = child.position;
                    // Merge parameters
                    for override_p in &child.params {
                        if let Some(base_p) = resolved_child.params.iter_mut().find(|p| p.name == override_p.name) {
                            base_p.default = override_p.default.clone();
                        }
                    }
                    resolved_children.push(resolved_child);
                } else {
                    panic!(
                        "Node template of type '{}' not found for child '{}' in template '{}'",
                        child.node_type, child.name, node.name
                    );
                }
            }
            node.children = resolved_children;
            children.push(node);
        }
    }
    FsNode {
        id: generate_node_id(),
        name: String::new(),
        node_type: default_node_type(),
        children,
        params: vec![],
        geometry_visible: true,
        position: (0.0, 0.0),
        inputs: 0,
        outputs: 0,
    }
}

fn default_grid_thickness() -> f32 { 0.03 }
fn default_show_camera_pivot() -> bool { false }
fn default_camera_pivot_size() -> f32 { 1.0 }
fn default_node_color() -> [f32; 3] { [0.10, 0.45, 0.70] }
fn default_grid_color() -> [f32; 3] { [0.35, 0.35, 0.40] }

fn default_cell_color() -> [f32; 3] { [0.13, 0.13, 0.16] }
fn default_gap_color() -> [f32; 3] { [0.07, 0.07, 0.09] }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DesignSettings {
    pub grid_size_x: f32,
    pub grid_size_y: f32,
    pub skipped_row_h: f32,
    pub skipped_col_w: f32,
    pub show_grid_enabled: bool,
    pub show_cube_enabled: bool,
    pub show_origin_enabled: bool,
    pub origin_size: f32,
    pub viewport_bg_color: [f32; 3],
    pub square_viewport: bool,
    #[serde(default = "default_grid_thickness")]
    pub grid_thickness: f32,
    #[serde(default = "default_show_camera_pivot")]
    pub show_camera_pivot_enabled: bool,
    #[serde(default = "default_camera_pivot_size")]
    pub camera_pivot_size: f32,
    #[serde(default = "default_node_color")]
    pub node_color: [f32; 3],
    #[serde(default = "default_grid_color")]
    pub grid_color: [f32; 3],
    #[serde(default = "default_cell_color")]
    pub cell_color: [f32; 3],
    #[serde(default = "default_gap_color")]
    pub gap_color: [f32; 3],
}

impl Default for DesignSettings {
    fn default() -> Self {
        Self {
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
            serde_json::from_str::<Self>(&content).unwrap_or_else(|_| Self::default())
        } else {
            Self::default()
        }
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

pub struct NodePalette {
    pub x: f32, y: f32, w: f32, h: f32,
    pub visible: bool,
    pub query: String,
    pub items: Vec<String>,
    pub selected: usize,
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

    pub fn set_palette_state(&mut self, visible: bool, query: &str, items: &[String], selected: usize) {
        self.visible = visible;
        self.query = query.to_string();
        self.items = items.to_vec();
        self.selected = selected;
    }
}

impl Element for NodePalette {
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    fn rect(&self) -> (f32, f32, f32, f32) { (self.x, self.y, self.w, self.h) }
    fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) { self.x = x; self.y = y; self.w = w; self.h = h; }
    fn color(&self) -> [f32; 4] { [0.0, 0.0, 0.0, 0.0] }
    fn hit_test(&self, px: f32, py: f32, ctx: &cce_ui::context::UiContext) -> bool {
        if ctx.is_coordinate_covered(self as *const Self as *const () as usize, px, py) {
            return false;
        }
        self.visible && px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
    fn set_visible(&mut self, visible: bool) { self.visible = visible; }
    fn visible(&self) -> bool { self.visible }
    fn take_click(&mut self) -> bool { self.visible }


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


pub fn make_text_buffer(font_system: &mut FontSystem, text: &str, size: f32) -> Buffer {
    let metrics = Metrics::new(size, size * 1.4);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text(font_system, text, Attrs::new(), glyphon::Shaping::Advanced);
    buffer.shape_until_scroll(font_system, true);
    buffer
}

pub fn make_text_buffer_with_font(font_system: &mut FontSystem, text: &str, size: f32, font: Option<&str>) -> Buffer {
    let metrics = Metrics::new(size, size * 1.4);
    let mut buffer = Buffer::new(font_system, metrics);
    let mut attrs = Attrs::new();
    let family_name = font.map(|f| cce_ui::layout::parse_font_string(f).0);
    if let Some(ref name) = family_name {
        let family = match name.as_str() {
            "monospace" => glyphon::Family::Name(cce_ui::layout::get_system_monospace_font()),
            "sans-serif" => glyphon::Family::SansSerif,
            "serif" => glyphon::Family::Serif,
            _ => glyphon::Family::Name(name),
        };
        attrs = attrs.family(family);
    }
    buffer.set_text(font_system, text, attrs, glyphon::Shaping::Advanced);
    buffer.shape_until_scroll(font_system, true);
    buffer
}

pub fn get_next_visible_pane(
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

pub fn push_circle_vertices(
    cx: f32, cy: f32, r: f32,
    sw: f32, sh: f32,
    color: [f32; 4],
    segments: usize,
    clip_circle: [f32; 3],
    out: &mut Vec<Vertex>,
) {
    for i in 0..segments {
        let theta1 = (i as f32) * 2.0 * std::f32::consts::PI / (segments as f32);
        let theta2 = ((i + 1) as f32) * 2.0 * std::f32::consts::PI / (segments as f32);
        let x0 = cx;
        let y0 = cy;
        let x1 = cx + r * theta1.cos();
        let y1 = cy + r * theta1.sin();
        let x2 = cx + r * theta2.cos();
        let y2 = cy + r * theta2.sin();
        
        let ndc_x0 = (x0 / sw) * 2.0 - 1.0;
        let ndc_y0 = 1.0 - (y0 / sh) * 2.0;
        let ndc_x1 = (x1 / sw) * 2.0 - 1.0;
        let ndc_y1 = 1.0 - (y1 / sh) * 2.0;
        let ndc_x2 = (x2 / sw) * 2.0 - 1.0;
        let ndc_y2 = 1.0 - (y2 / sh) * 2.0;
        
        out.push(Vertex { position: [ndc_x0, ndc_y0], color, clip_circle });
        out.push(Vertex { position: [ndc_x1, ndc_y1], color, clip_circle });
        out.push(Vertex { position: [ndc_x2, ndc_y2], color, clip_circle });
    }
}

pub fn push_circle_border_vertices(
    cx: f32, cy: f32, r: f32,
    thickness: f32,
    sw: f32, sh: f32,
    color: [f32; 4],
    segments: usize,
    clip_circle: [f32; 3],
    out: &mut Vec<Vertex>,
) {
    for i in 0..segments {
        let theta1 = (i as f32) * 2.0 * std::f32::consts::PI / (segments as f32);
        let theta2 = ((i + 1) as f32) * 2.0 * std::f32::consts::PI / (segments as f32);
        
        let x0 = cx + (r - thickness) * theta1.cos();
        let y0 = cy + (r - thickness) * theta1.sin();
        let x1 = cx + r * theta1.cos();
        let y1 = cy + r * theta1.sin();
        
        let x2 = cx + r * theta2.cos();
        let y2 = cy + r * theta2.sin();
        let x3 = cx + (r - thickness) * theta2.cos();
        let y3 = cy + (r - thickness) * theta2.sin();
        
        let ndc_x0 = (x0 / sw) * 2.0 - 1.0; let ndc_y0 = 1.0 - (y0 / sh) * 2.0;
        let ndc_x1 = (x1 / sw) * 2.0 - 1.0; let ndc_y1 = 1.0 - (y1 / sh) * 2.0;
        let ndc_x2 = (x2 / sw) * 2.0 - 1.0; let ndc_y2 = 1.0 - (y2 / sh) * 2.0;
        let ndc_x3 = (x3 / sw) * 2.0 - 1.0; let ndc_y3 = 1.0 - (y3 / sh) * 2.0;
        
        out.push(Vertex { position: [ndc_x0, ndc_y0], color, clip_circle });
        out.push(Vertex { position: [ndc_x1, ndc_y1], color, clip_circle });
        out.push(Vertex { position: [ndc_x2, ndc_y2], color, clip_circle });
        
        out.push(Vertex { position: [ndc_x0, ndc_y0], color, clip_circle });
        out.push(Vertex { position: [ndc_x2, ndc_y2], color, clip_circle });
        out.push(Vertex { position: [ndc_x3, ndc_y3], color, clip_circle });
    }
}



#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeDirection {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewportUniforms {
    pub mvp: [[f32; 4]; 4],
    pub window_size: [f32; 2],
    pub window_radius: f32,
    pub _padding: f32,
}

pub struct State {
    pub wgpu_adapter: cce_ui::backend::WgpuAdapter,
    pub render_pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: wgpu::Buffer,
    pub window: XdgWindow,
    pub wl_surface: wl_surface::WlSurface,
    pub vertex_count: u32,
    pub vertex_data: Vec<Vertex>,

    pub pipeline_3d: wgpu::RenderPipeline,
    pub bind_group_3d: wgpu::BindGroup,
    pub bind_group_layout_3d: wgpu::BindGroupLayout,
    pub uniform_buffer: wgpu::Buffer,
    pub bind_group_grid: wgpu::BindGroup,
    pub uniform_buffer_grid: wgpu::Buffer,
    pub bind_group_pivot: wgpu::BindGroup,
    pub uniform_buffer_pivot: wgpu::Buffer,
    pub vertex_buffer_3d: wgpu::Buffer,
    pub vertex_count_3d: u32,
    pub vertex_buffer_spheres: wgpu::Buffer,
    pub vertex_count_spheres: u32,
    pub vertex_buffer_grid: wgpu::Buffer,
    pub vertex_count_grid: u32,
    pub depth_texture: wgpu::Texture,
    pub depth_texture_view: wgpu::TextureView,
    pub backdrop_texture: wgpu::Texture,
    pub backdrop_texture_view: wgpu::TextureView,
    pub backdrop_sampler: wgpu::Sampler,
    pub backdrop_bind_group_layout: wgpu::BindGroupLayout,
    pub backdrop_bind_group: wgpu::BindGroup,
    pub window_info_buffer: wgpu::Buffer,
    pub rotation_y: f32,
    pub rotation_x: f32,
    pub is_rotating_viewport: bool,
    pub scroll_lock: u8, // 0 = None, 1 = Horizontal, 2 = Vertical
    pub last_rotate_time: Instant,
    pub rotate_accum_yaw: f32,
    pub rotate_accum_pitch: f32,
    pub rotate_velocity_yaw: f32,
    pub rotate_velocity_pitch: f32,
    pub show_grid: bool,
    pub show_cube: bool,
    pub show_origin: bool,
    pub show_camera_pivot: bool,
    pub viewport_bg_color: [f32; 3],
    pub node_color: [f32; 3],
    pub grid_color: [f32; 3],
    pub cell_color: [f32; 3],
    pub gap_color: [f32; 3],
    pub vertex_buffer_origin: wgpu::Buffer,
    pub vertex_count_origin: u32,
    pub vertex_buffer_pivot: wgpu::Buffer,
    pub vertex_count_pivot: u32,
    pub origin_size: f32,
    pub camera_pivot_size: f32,

    pub root_window: Box<cce_ui::widget::Window>,
    pub fs_root: FsNode,
    pub node_templates: Vec<NodeTemplate>,
    pub current_path: Vec<usize>,
    pub node_clipboard: Option<FsNode>,
    pub last_click: Option<(Instant, usize)>,
    pub last_frame: Instant,

    pub shortcut_manager: ShortcutManager,
    pub pending_action: Option<Action>,
    pub exit_requested: bool,

    pub widgets: Vec<Box<dyn Element>>,
    pub positions: Vec<(f32, f32, f32, f32)>,
    pub splitter_layout: cce_ui::layout::SplitterLayout,
    pub node_palette_visible: bool,
    pub node_palette_query: String,
    pub node_palette_filtered: Vec<usize>,
    pub node_palette_selected: usize,

    pub curved_text_texture: wgpu::Texture,
    pub curved_text_texture_view: wgpu::TextureView,
    pub curved_text_sampler: wgpu::Sampler,
    pub curved_text_bind_group: wgpu::BindGroup,
    pub curved_text_pipeline: wgpu::RenderPipeline,
    pub curved_text_atlas: TextAtlas,
    pub curved_text_renderer: TextRenderer,
    pub curved_text_viewport: Viewport,
    pub textured_vertex_buffer: wgpu::Buffer,
    pub textured_vertex_count: u32,


    pub drag_widget: Option<usize>,
    pub focused_widget: Option<usize>,

    pub cursor_x: f32,
    pub cursor_y: f32,
    pub grid_cursor_col: i32,
    pub grid_cursor_row: i32,
    pub modifiers: ModifiersState,

    pub width: f32,
    pub height: f32,
    pub physical_width: u32,
    pub physical_height: u32,
    pub scale: f64,
    pub square_viewport: bool,
    pub grid_snap_enabled: bool,
    pub network_grid_visible: bool,
    pub grid_size_x: f32,
    pub grid_size_y: f32,
    pub skipped_row_h: f32,
    pub skipped_col_w: f32,

    pub pan_x: f32,
    pub pan_y: f32,
    pub pan_velocity_x: f32,
    pub pan_velocity_y: f32,
    pub last_frame_pan_x: f32,
    pub last_frame_pan_y: f32,
    pub is_panning: bool,
    pub pan_start_cx: f32,
    pub pan_start_cy: f32,
    pub pan_start_x: f32,
    pub pan_start_y: f32,
    pub space_pressed: bool,
    pub active_camera: String,
    pub show_network: bool,
    pub show_viewport: bool,
    pub show_parameters: bool,
    pub show_spreadsheet: bool,
    pub is_scrolling_trackpad: bool,
    pub last_scroll_time: Instant,
    pub scroll_accum_x: f32,
    pub scroll_accum_y: f32,
    pub last_spreadsheet_node_name: Option<String>,
    pub last_spreadsheet_node_params: Option<Vec<(String, String)>>,
    pub viewport_zoom: f32,
    pub is_zooming_viewport: bool,
    pub last_zoom_time: Instant,
    pub zoom_accum: f32,
    pub zoom_velocity: f32,
    pub grid_thickness: f32,
    pub focused_pane: usize,
    pub inertial_scroll_enabled: bool,
    pub inertial_scroll_friction: f32,
    pub scroll_speed: f32,
    pub last_config_read: Instant,
    pub circular_network_pane: bool,
    pub circular_network_layout: cce_ui::layout::CircularPaneLayout,
    pub is_detached_network: bool,
    pub detached_circular_network: bool,
    pub last_project_mod_time: Option<std::time::SystemTime>,
    pub last_project_check: std::time::Instant,
    pub last_inspector_check: std::time::Instant,
    pub last_inspector_update: std::time::Instant,
    pub last_serialized: String,
    pub needs_autosave: bool,
    pub last_autosave_time: std::time::Instant,
    pub window_x: i32,
    pub window_y: i32,
    pub active_menu_cloud_pid: Option<u32>,
    pub active_menu_cloud_idx: Option<(usize, usize)>,
    pub uniform_background: bool,
    pub network_opacity: f32,
    pub cell_opacity: f32,
    pub gap_opacity: f32,
    pub last_design_mod_time: Option<std::time::SystemTime>,
    pub last_config_mod_time: Option<std::time::SystemTime>,
    pub floating_network_layout: (f32, f32, f32, f32),
    pub is_resizing_network: Option<ResizeDirection>,
    pub drag_start_rect: (f32, f32, f32, f32),
    pub drag_start_mouse: (f32, f32),
    pub floating_param_width: f32,
    pub is_resizing_param: bool,
    pub drag_start_param_w: f32,
    pub floating_spreadsheet_height: f32,
    pub is_resizing_spreadsheet: bool,
    pub drag_start_spreadsheet_h: f32,
    pub loaded_project_path: Option<std::path::PathBuf>,
    pub last_saved_root_json: String,
    pub recent_files: Vec<std::path::PathBuf>,
    pub recent_files_list: ScrollingList,
    pub recent_files_buttons: Vec<Button>,
    pub text_buffer_cache: std::collections::HashMap<(String, u32, Option<String>), Buffer>,
    pub viewport_dirty: bool,
    pub text_dirty: bool,
    pub last_popover_rects: Vec<(f32, f32, f32, f32)>,
    pub last_status_text: String,
    pub last_viewport_camera_pos: Vec3,
    pub last_viewport_camera_rx: f32,
    pub last_viewport_camera_ry: f32,
    pub last_viewport_camera_rz: f32,
    pub last_viewport_pivot: Vec3,
    pub last_viewport_zoom: f32,
    pub last_viewport_rotation_x: f32,
    pub last_viewport_rotation_y: f32,
    pub last_viewport_bg_color: [f32; 3],
    pub last_viewport_show_grid: bool,
    pub last_viewport_show_cube: bool,
    pub last_viewport_show_origin: bool,
    pub last_viewport_show_camera_pivot: bool,
    pub last_viewport_width: u32,
    pub last_viewport_height: u32,
    pub last_viewport_active_camera: String,
    pub last_viewport_show_viewport: bool,
    pub ui_context: cce_ui::context::UiContext,
}

impl State {
    pub fn find_widget_index(&self, child_ptr: *mut (dyn Element + 'static)) -> Option<usize> {
        let target_addr = child_ptr as *const ();
        self.widgets.iter().position(|w| {
            let w_ptr = w.as_ref() as *const dyn Element as *const ();
            w_ptr == target_addr
        })
    }

    pub fn has_any_open_menu(&self, idx: usize) -> bool {
        let mut visited = vec![false; self.widgets.len()];
        self.has_any_open_menu_impl(idx, &mut visited)
    }

    pub fn has_any_open_menu_impl(&self, idx: usize, visited: &mut [bool]) -> bool {
        if idx >= self.widgets.len() {
            return false;
        }
        if visited[idx] {
            return false;
        }
        visited[idx] = true;
        if let Some(menu) = self.widgets[idx].as_menu_controller() {
            if menu.is_menu_open() {
                return true;
            }
        }
        let child_ptrs = self.widgets[idx].children(&self.ui_context);
        for child_ptr in child_ptrs {
            if let Some(child_idx) = self.find_widget_index(child_ptr) {
                if self.has_any_open_menu_impl(child_idx, visited) {
                    return true;
                }
            }
        }
        false
    }

    pub fn palette(&self) -> &NodePalette {
        self.widgets[NODE_PALETTE_IDX].as_any().downcast_ref::<NodePalette>().expect("not a NodePalette")
    }

    pub fn palette_mut(&mut self) -> &mut NodePalette {
        self.widgets[NODE_PALETTE_IDX].as_any_mut().downcast_mut::<NodePalette>().expect("not a NodePalette")
    }

    pub fn menu(&self, idx: usize) -> &dyn cce_ui::widget::MenuController {
        self.widgets[idx].as_menu_controller().expect("not a MenuController")
    }

    pub fn menu_mut(&mut self, idx: usize) -> &mut dyn cce_ui::widget::MenuController {
        self.widgets[idx].as_menu_controller_mut().expect("not a MenuController")
    }

    pub fn graph(&self) -> &dyn cce_ui::widget::GraphController {
        self.widgets[CONTENT_IDX].as_graph_controller().expect("not a GraphController")
    }

    pub fn graph_mut(&mut self) -> &mut dyn cce_ui::widget::GraphController {
        self.widgets[CONTENT_IDX].as_graph_controller_mut().expect("not a GraphController")
    }

    pub fn page_selector(&self, idx: usize) -> &dyn cce_ui::widget::PageSelector {
        self.widgets[idx].as_page_selector().expect("not a PageSelector")
    }

    pub fn page_selector_mut(&mut self, idx: usize) -> &mut dyn cce_ui::widget::PageSelector {
        self.widgets[idx].as_page_selector_mut().expect("not a PageSelector")
    }

    pub fn param(&self) -> &dyn cce_ui::widget::ParamController {
        self.widgets[PARAM_IDX].as_param_controller().expect("not a ParamController")
    }

    pub fn param_mut(&mut self) -> &mut dyn cce_ui::widget::ParamController {
        self.widgets[PARAM_IDX].as_param_controller_mut().expect("not a ParamController")
    }

    pub fn spreadsheet_mut(&mut self) -> &mut dyn cce_ui::widget::SpreadsheetController {
        self.widgets[SPREADSHEET_IDX].as_spreadsheet_controller_mut().expect("not a SpreadsheetController")
    }

    pub fn path_mut(&mut self) -> &mut dyn cce_ui::widget::PathController {
        self.widgets[BREADCRUMB_IDX].as_path_controller_mut().expect("not a PathController")
    }

    pub fn geom_mut(&mut self, idx: usize) -> &mut dyn cce_ui::widget::GeomController {
        self.widgets[idx].as_geom_controller_mut().expect("not a GeomController")
    }

    pub fn scroll_mut(&mut self, idx: usize) -> &mut dyn cce_ui::widget::ScrollController {
        self.widgets[idx].as_scroll_controller_mut().expect("not a ScrollController")
    }

    pub fn has_unsaved_changes(&self) -> bool {
        if let Ok(current_json) = serde_json::to_string(&self.fs_root) {
            current_json != self.last_saved_root_json
        } else {
            false
        }
    }





    pub fn update_recent_files_layout(&mut self) {}

    pub fn save_settings(&mut self) {
        let settings = DesignSettings {
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
        };
        settings.save();
        self.last_design_mod_time = {
            let design_path = DesignSettings::file_path();
            std::fs::metadata(&design_path).and_then(|m| m.modified()).ok()
        };
    }



    pub fn update_grid_geometry(&mut self) {
        let grid_verts = grid_vertices(self.grid_thickness, self.grid_color);
        self.wgpu_adapter.queue.write_buffer(&self.vertex_buffer_grid, 0, bytemuck::cast_slice(&grid_verts));
        self.viewport_dirty = true;
    }

    pub fn update_origin_geometry(&mut self) {
        let origin_verts = origin_vectors_vertices(self.origin_size);
        self.wgpu_adapter.queue.write_buffer(&self.vertex_buffer_origin, 0, bytemuck::cast_slice(&origin_verts));
        self.viewport_dirty = true;
    }

    pub fn update_pivot_geometry(&mut self) {
        let pivot_verts = camera_pivot_vertices(self.camera_pivot_size);
        self.wgpu_adapter.queue.write_buffer(&self.vertex_buffer_pivot, 0, bytemuck::cast_slice(&pivot_verts));
        self.viewport_dirty = true;
    }

    pub fn body_h(&self) -> f32 { self.height - HEADER_H - STATUS_H }

    pub fn get_col_geometries(&self) -> (f32, f32, f32, f32, f32, f32) {
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

    pub fn content_left_w(&self) -> f32 { self.get_col_geometries().1 }

    pub fn content_right_x(&self) -> f32 { self.get_col_geometries().2 }

    pub fn viewport_w(&self) -> f32 { self.get_col_geometries().3 }

    pub fn param_x(&self) -> f32 { self.get_col_geometries().4 }

    pub fn param_w(&self) -> f32 { self.get_col_geometries().5 }
    
    pub fn in_network_pane(&self) -> bool {
        if self.circular_network_pane {
            self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, 0.0, BREADCRUMB_H)
        } else {
            let (cx, cy, cw, ch) = self.positions[CONTENT_IDX];
            self.cursor_x >= cx
                && self.cursor_x < cx + cw
                && self.cursor_y >= cy
                && self.cursor_y < cy + ch
        }
    }

    pub fn clamp_splitters(&mut self) {
        if self.is_detached_network {
            return;
        }
        self.splitter_layout.clamp(self.width, self.detached_circular_network);
    }

    pub fn current_dir(&self) -> &FsNode {
        let mut node = &self.fs_root;
        for &i in &self.current_path {
            node = &node.children[i];
        }
        node
    }

    pub fn current_dir_mut(&mut self) -> &mut FsNode {
        let mut node = &mut self.fs_root;
        for &i in &self.current_path {
            node = &mut node.children[i];
        }
        node
    }

    pub fn get_lowest_unused_name(&self, base_name: &str) -> String {
        let dir = self.current_dir();
        let mut index = 1;
        loop {
            let candidate = format!("{} {}", base_name, index);
            if !dir.children.iter().any(|c| c.name == candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    pub fn sync_parameters_to_project(&mut self) {
        if !self.is_detached_network {
            if let Some(slot_idx) = self.graph().selected_node() {
                let updated_params = self.param().node_params();
                let mut recent_file_to_open = None;
                let dir = self.current_dir_mut();
                if let Some(child) = dir.children.get_mut(slot_idx) {
                    let mut param_changed = false;
                    let mut triggered_buttons = Vec::new();
                    for (u_name, u_val, _) in &updated_params {
                        if let Some(p) = child.params.iter_mut().find(|p| p.name == *u_name) {
                            if p.default != *u_val {
                                p.default = u_val.clone();
                                param_changed = true;
                                if p.param_type == "button" && p.default == "clicked" {
                                    triggered_buttons.push(p.name.clone());
                                    p.default = "".to_string();
                                }
                                if p.name == "Open Recent" && p.default != "- Select -" && !p.default.is_empty() {
                                    recent_file_to_open = Some(p.default.clone());
                                    p.default = "- Select -".to_string();
                                }
                            }
                        }
                    }

                    if !triggered_buttons.is_empty() || recent_file_to_open.is_some() {
                        let mut disp_params = self.param().node_params();
                        for btn_name in &triggered_buttons {
                            if let Some(pos) = disp_params.iter().position(|p| p.0 == *btn_name) {
                                disp_params[pos].1 = "".to_string();
                            }
                        }
                        if recent_file_to_open.is_some() {
                            if let Some(pos) = disp_params.iter().position(|p| p.0 == "Open Recent") {
                                disp_params[pos].1 = "- Select -".to_string();
                            }
                        }
                        self.param_mut().set_display_params(&disp_params);
                    }

                    if param_changed {
                        self.apply_settings_from_menubar_subnets();
                        self.sync_grid_settings();
                        self.rebuild_scene_geometry();
                        self.sync_nodes();

                        for btn_name in triggered_buttons {
                            match btn_name.as_str() {
                                "Update Parameters" => {
                                    if let Some(slot_idx) = self.graph().selected_node() {
                                        let dir = self.current_dir_mut();
                                        if let Some(child) = dir.children.get_mut(slot_idx) {
                                            if child.node_type == "opencl" {
                                                let code_val = child.params.iter()
                                                    .find(|p| p.name == "Code")
                                                    .map(|p| p.default.clone())
                                                    .unwrap_or_default();
                                                let parsed_params = crate::geometry::parse_dynamic_params(&code_val);
                                                let mut new_params = Vec::new();
                                                for base_name in &["Input", "Code", "Update Parameters"] {
                                                    if let Some(p) = child.params.iter().find(|p| p.name == *base_name) {
                                                        new_params.push(p.clone());
                                                    }
                                                }
                                                for mut parsed in parsed_params {
                                                    if let Some(existing) = child.params.iter().find(|p| p.name == parsed.name) {
                                                        parsed.default = existing.default.clone();
                                                    }
                                                    new_params.push(parsed);
                                                }
                                                child.params = new_params;
                                                let updated_disp = param_display(&child.params);
                                                self.param_mut().set_display_params(&updated_disp);
                                                self.rebuild_scene_geometry();
                                                self.sync_nodes();
                                            }
                                        }
                                    }
                                }
                                "New Project" | "New" => {
                                    self.new_project();
                                }
                                "Open" => {
                                    self.open_file_chooser();
                                }
                                "Save" => {
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
                                "Save As" => {
                                    self.save_file_chooser();
                                }
                                "Exit" => {
                                    self.exit_requested = true;
                                }
                                "Zoom In" => {
                                    self.zoom(1.15, None);
                                }
                                "Zoom Out" => {
                                    self.zoom(1.0 / 1.15, None);
                                }
                                "Reset Zoom" => {
                                    self.grid_size_x = 150.0;
                                    self.grid_size_y = 75.0;
                                    self.skipped_col_w = 37.5;
                                    self.skipped_row_h = 37.5;
                                    self.sync_grid_settings();
                                }
                                "Detach Circular Window" | "Detach Pane" => {
                                    self.execute_action(Action::DetachCircularWindow);
                                }
                                "Show Network Pane" => {
                                    self.show_network = !self.show_network;
                                    self.widgets[CONTENT_IDX].set_visible(self.show_network);
                                    self.widgets[LEFT_MENUBAR_IDX].set_visible(self.show_network);
                                    self.widgets[BREADCRUMB_IDX].set_visible(self.show_network);
                                    let val = self.show_network;
                                    self.menu_mut(HEADER_IDX).set_item_checked(2, 4, val);
                                    if !self.show_network && self.focused_pane == LEFT_MENUBAR_IDX {
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
                                }
                                "Show Viewport Pane" => {
                                    self.show_viewport = !self.show_viewport;
                                    self.widgets[VIEWPORT_IDX].set_visible(self.show_viewport);
                                    self.widgets[RIGHT_MENUBAR_IDX].set_visible(self.show_viewport);
                                    let val = self.show_viewport;
                                    self.menu_mut(HEADER_IDX).set_item_checked(2, 5, val);
                                    if !self.show_viewport && self.focused_pane == RIGHT_MENUBAR_IDX {
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
                                }
                                "Show Parameters Pane" => {
                                    self.show_parameters = !self.show_parameters;
                                    self.widgets[PARAM_IDX].set_visible(self.show_parameters);
                                    self.widgets[PARAM_MENUBAR_IDX].set_visible(self.show_parameters);
                                    let val = self.show_parameters;
                                    self.menu_mut(HEADER_IDX).set_item_checked(2, 6, val);
                                    if !self.show_parameters && self.focused_pane == PARAM_MENUBAR_IDX {
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
                                }
                                "Show Spreadsheet Pane" => {
                                    self.show_spreadsheet = !self.show_spreadsheet;
                                    self.widgets[SPREADSHEET_IDX].set_visible(self.show_spreadsheet);
                                    self.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(self.show_spreadsheet);
                                    let val = self.show_spreadsheet;
                                    self.menu_mut(HEADER_IDX).set_item_checked(2, 7, val);
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
                                }
                                "Close Pane" => {
                                    let mut parent_name = "";
                                    if self.current_path.len() >= 1 {
                                        let root_idx = self.current_path[0];
                                        if let Some(r_node) = self.fs_root.children.get(root_idx) {
                                            parent_name = r_node.name.as_str();
                                        }
                                    }
                                    match parent_name {
                                        "Network" => {
                                            self.show_network = false;
                                            self.widgets[CONTENT_IDX].set_visible(false);
                                            self.widgets[LEFT_MENUBAR_IDX].set_visible(false);
                                            self.widgets[BREADCRUMB_IDX].set_visible(false);
                                            self.menu_mut(HEADER_IDX).set_item_checked(2, 4, false);
                                            if self.focused_pane == LEFT_MENUBAR_IDX {
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
                                        }
                                        "Viewport" => {
                                            self.show_viewport = false;
                                            self.widgets[VIEWPORT_IDX].set_visible(false);
                                            self.widgets[RIGHT_MENUBAR_IDX].set_visible(false);
                                            self.menu_mut(HEADER_IDX).set_item_checked(2, 5, false);
                                            if self.focused_pane == RIGHT_MENUBAR_IDX {
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
                                        }
                                        "Parameters" => {
                                            self.show_parameters = false;
                                            self.widgets[PARAM_IDX].set_visible(false);
                                            self.widgets[PARAM_MENUBAR_IDX].set_visible(false);
                                            self.menu_mut(HEADER_IDX).set_item_checked(2, 6, false);
                                            if self.focused_pane == PARAM_MENUBAR_IDX {
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
                                        }
                                        "Spreadsheet" => {
                                            self.show_spreadsheet = false;
                                            self.widgets[SPREADSHEET_IDX].set_visible(false);
                                            self.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(false);
                                            self.menu_mut(HEADER_IDX).set_item_checked(2, 7, false);
                                            if self.focused_pane == SPREADSHEET_MENUBAR_IDX {
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
                                        }
                                        _ => {}
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }

                if let Some(path_str) = recent_file_to_open {
                    let path = std::path::PathBuf::from(path_str);
                    if let Err(e) = self.load_from_file(&path) {
                        eprintln!("Failed to load recent file: {:?}", e);
                        self.update_status_text(&format!("Failed to load: {:?}", e));
                    } else {
                        self.update_status_text(&format!("Loaded project from {}", path.display()));
                        self.add_recent_file(path);
                    }
                }
            }
        }
    }

    pub fn sync_parameters_pane(&mut self) {
        let params = if !self.is_detached_network {
            if let Some(slot_idx) = self.graph().selected_node() {
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
        self.param_mut().set_display_params(&params);
    }

    pub fn current_path_names(&self) -> Vec<String> {
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

    pub fn refresh_node_palette(&mut self) {
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
        let visible = self.node_palette_visible;
        let query = self.node_palette_query.clone();
        let selected = self.node_palette_selected;
        self.palette_mut().set_palette_state(
            visible,
            &query,
            &items,
            selected,
        );
    }

    pub fn open_file_chooser(&self) {
        std::thread::spawn(move || {
            use std::process::{Command, Stdio};
            use std::io::Write;

            // Find executable path
            let exe_path = if let Ok(cur_exe) = std::env::current_exe() {
                let sibling = cur_exe.with_file_name("cce-filesystem-interface");
                if sibling.exists() {
                    sibling
                } else if let Ok(home) = std::env::var("HOME") {
                    let path = std::path::PathBuf::from(home)
                        .join(".local")
                        .join("bin")
                        .join("cce-filesystem-interface");
                    if path.exists() {
                        path
                    } else {
                        std::path::PathBuf::from("cce-filesystem-interface")
                    }
                } else {
                    std::path::PathBuf::from("cce-filesystem-interface")
                }
            } else if let Ok(home) = std::env::var("HOME") {
                let path = std::path::PathBuf::from(home)
                    .join(".local")
                    .join("bin")
                    .join("cce-filesystem-interface");
                if path.exists() {
                    path
                } else {
                    std::path::PathBuf::from("cce-filesystem-interface")
                }
            } else {
                std::path::PathBuf::from("cce-filesystem-interface")
            };

            let child = match Command::new(exe_path)
                .arg("--select")
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to spawn cce-filesystem-interface: {:?}", e);
                    return;
                }
            };

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("Failed to wait for cce-filesystem-interface: {:?}", e);
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

    pub fn save_file_chooser(&self) {
        std::thread::spawn(move || {
            use std::process::{Command, Stdio};
            use std::io::Write;

            // Find executable path
            let exe_path = if let Ok(cur_exe) = std::env::current_exe() {
                let sibling = cur_exe.with_file_name("cce-filesystem-interface");
                if sibling.exists() {
                    sibling
                } else if let Ok(home) = std::env::var("HOME") {
                    let path = std::path::PathBuf::from(home)
                        .join(".local")
                        .join("bin")
                        .join("cce-filesystem-interface");
                    if path.exists() {
                        path
                    } else {
                        std::path::PathBuf::from("cce-filesystem-interface")
                    }
                } else {
                    std::path::PathBuf::from("cce-filesystem-interface")
                }
            } else if let Ok(home) = std::env::var("HOME") {
                let path = std::path::PathBuf::from(home)
                    .join(".local")
                    .join("bin")
                    .join("cce-filesystem-interface");
                if path.exists() {
                    path
                } else {
                    std::path::PathBuf::from("cce-filesystem-interface")
                }
            } else {
                std::path::PathBuf::from("cce-filesystem-interface")
            };

            let child = match Command::new(exe_path)
                .arg("--save")
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to spawn cce-filesystem-interface: {:?}", e);
                    return;
                }
            };

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("Failed to wait for cce-filesystem-interface: {:?}", e);
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

    pub fn open_node_palette(&mut self) {
        self.node_palette_visible = true;
        self.node_palette_query = String::new();
        self.node_palette_selected = 0;
        self.refresh_node_palette();
        self.upload_vertices();
    }

    pub fn close_node_palette(&mut self) {
        self.node_palette_visible = false;
        self.refresh_node_palette();
        self.upload_vertices();
    }


    pub fn place_selected_node(&mut self) -> bool {
        let Some(&template_idx) = self.node_palette_filtered.get(self.node_palette_selected) else { return false; };
        let mut node = self.node_templates[template_idx].node.clone();
        let is_in_utility = !self.current_path.is_empty() && self.fs_root.children[self.current_path[0]].node_type == "utility";
        if is_in_utility {
            if crate::geometry::is_geometry_node_type(&node.node_type) {
                self.update_status_text("Utility nodes cannot contain geometry.");
                self.close_node_palette();
                return false;
            }
        }
        let (nx, ny) = self.find_empty_cell(self.grid_cursor_col as f32, self.grid_cursor_row as f32, None);
        node.position = (nx, ny);
        node.name = self.get_lowest_unused_name(&node.name);
        self.current_dir_mut().children.push(node);
        self.close_node_palette();
        self.sync_nodes();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.rebuild_scene_geometry();
        self.upload_vertices();
        true
    }


    pub fn handle_node_palette_key(&mut self, event: &KeyEvent) -> bool {
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

pub(crate) fn geometry_to_spreadsheet_data(geom: &Geometry) -> (Vec<String>, Vec<Vec<String>>) {
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

    pub fn update_active_camera_rotation(&mut self, d_yaw: f32, d_pitch: f32) -> bool {
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
                    self.graph().selected_node().and_then(|sel_idx| {
                        let dir = self.current_dir();
                        if sel_idx < dir.children.len() {
                            Some(param_display(&dir.children[sel_idx].params))
                        } else { None }
                    }).unwrap_or_default()
                } else {
                    vec![]
                };
                self.param_mut().set_display_params(&params);

                return true;
            }
        }
        false
    }

    pub fn update_active_camera_rotation_reset(&mut self) -> bool {
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
                    self.graph().selected_node().and_then(|sel_idx| {
                        let dir = self.current_dir();
                        if sel_idx < dir.children.len() {
                            Some(param_display(&dir.children[sel_idx].params))
                        } else { None }
                    }).unwrap_or_default()
                } else {
                    vec![]
                };
                self.param_mut().set_display_params(&params);
                return true;
            }
        }
        false
    }

    pub fn on_path_changed(&mut self) {
        self.graph_mut().set_selected_node(None);
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
        self.sync_cursor_and_selection();
    }

    pub fn move_up(&mut self) -> bool {
        if !self.current_path.is_empty() {
            let exited_idx = self.current_path.pop();
            self.on_path_changed();
            if let Some(idx) = exited_idx {
                let pos = {
                    let dir = self.current_dir();
                    if idx < dir.children.len() {
                        Some(dir.children[idx].position)
                    } else {
                        None
                    }
                };
                if let Some((pos_x, pos_y)) = pos {
                    self.grid_cursor_col = pos_x as i32;
                    self.grid_cursor_row = pos_y as i32;
                    self.sync_cursor_and_selection();
                    self.upload_vertices();
                }
            }
            true
        } else {
            false
        }
    }

    pub fn find_empty_cell(&self, start_x: f32, start_y: f32, skip_idx: Option<usize>) -> (f32, f32) {
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

    pub fn delete_node(&mut self, slot: usize) -> bool {
        let len = self.current_dir().children.len();
        if slot < len {
            self.current_dir_mut().children.remove(slot);
            if let Some(sel_idx) = self.graph().selected_node() {
                if sel_idx == slot {
                    self.graph_mut().set_selected_node(None);
                } else if sel_idx > slot {
                    self.graph_mut().set_selected_node(Some(sel_idx - 1));
                }
            }
            self.sync_nodes();
            self.rebuild_positions();
            self.apply_layout();
            self.update_panel_bounds();
            self.rebuild_scene_geometry();
            self.upload_vertices();
            self.viewport_dirty = true;
            true
        } else {
            false
        }
    }

    pub fn sync_nodes(&mut self) {
        let graph_nodes: Vec<GraphNode> = self.current_dir().children.iter().map(|c| {
            GraphNode {
                id: c.id.clone(),
                name: c.name.clone(),
                position: c.position,
                parameters: param_display(&c.params),
                geom_visible: c.geometry_visible,
                node_type: c.node_type.clone(),
                inputs: c.inputs,
                outputs: c.outputs,
            }
        }).collect();
        self.graph_mut().set_nodes(&graph_nodes);

        let camera_nodes: Vec<String> = self.current_dir().children.iter()
            .filter(|c| c.node_type == "camera")
            .map(|c| c.name.clone())
            .collect();
        let mut items = vec!["Default Camera".to_string()];
        items.extend(camera_nodes);
        if !items.contains(&self.active_camera) {
            self.active_camera = "Default Camera".to_string();
        }
        self.menu_mut(RIGHT_MENUBAR_IDX).set_menu_items(0, &items);
        for (i, item) in items.iter().enumerate() {
            let checked = item == &self.active_camera;
            self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(0, i, checked);
        }
        let path_strs = self.current_path_names();
        self.path_mut().set_path(&path_strs);

        let mut selected_node = None;
        if !self.is_detached_network {
            if let Some(slot_idx) = self.graph().selected_node() {
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

        if self.show_spreadsheet && !cache_hit {
            let mut headers = Vec::new();
            let mut rows = Vec::new();

            if let Some(node) = selected_node {
                let mut visited = Vec::new();
                let mut ocl_error = None;
                if let Some(geom) = generate_single_node_geometry_with_errors(&self.fs_root, node, &mut visited, &mut ocl_error) {
                    let (h, r) = Self::geometry_to_spreadsheet_data(&geom);
                    headers = h;
                    rows = r;
                }
            }

            self.spreadsheet_mut().set_spreadsheet_data(headers, rows);
            self.last_spreadsheet_node_name = current_name;
            self.last_spreadsheet_node_params = current_params;
        }
    }











    pub async fn new(
        conn: &Connection,
        qh: &QueueHandle<AppState>,
        compositor_state: &CompositorState,
        xdg_shell_state: &XdgShell,
        pw: u32,
        ph: u32,
        scale: f64,
        is_detached_network: bool,
    ) -> Self {
        cce_ui::scale::set_scale_factor(scale as f32);
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
        let wgpu_adapter = cce_ui::backend::WgpuAdapter::new(display_ptr, surface_ptr, pw, ph).await;

        let device = &wgpu_adapter.device;
        let queue = &wgpu_adapter.queue;

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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
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

        let window_info_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Window Info Buffer"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let window_info_data = [
            pw as f32,
            ph as f32,
            cce_ui::color::window_corner_radius() * scale as f32,
            0.0,
        ];
        queue.write_buffer(&window_info_buffer, 0, bytemuck::cast_slice(&window_info_data));

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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: window_info_buffer.as_entire_binding(),
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
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout_3d = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3D Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(80),
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
                    size: wgpu::BufferSize::new(80),
                }),
            }],
        });

        let uniform_buffer_grid = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grid Uniform Buffer"),
            size: 80,
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
                    size: wgpu::BufferSize::new(80),
                }),
            }],
        });

        let uniform_buffer_pivot = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Pivot Uniform Buffer"),
            size: 80,
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
                    size: wgpu::BufferSize::new(80),
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


        let splitter_layout = cce_ui::layout::SplitterLayout::new(sw, SPLITTER_W, MIN_COLUMN);
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
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![],
                params: vec![],
                geometry_visible: true,
                position: (0.0, 0.0),
                inputs: 0,
                outputs: 0,
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
        let context_opts = vec![
            "0: Network".to_string(),
            "1: Viewport".to_string(),
            "2: Parameters".to_string(),
            "3: Spreadsheet".to_string(),
            "Main Menu".to_string(),
        ];
        let mut widgets: Vec<Box<dyn Element>> = vec![
            Box::new(MenuBar::new(0.0, 0.0, 0.0, HEADER_H).with_title("Clear Design Interface").with_label("Main Menu Bar").with_item("File", &["New Project", "Open", "Save", "Save As", "Exit"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Reset Zoom", "Detach Circular Window", "Show Network Pane", "Show Viewport Pane", "Show Parameters Pane", "Show Spreadsheet Pane"]).with_item("Help", &["About"]).with_z_index(110).with_context_options(context_opts.clone(), 4)),
            Box::new(Graph::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(ViewportBg::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(Plate::new(0.0, 0.0, 0.0, 0.0).with_color(colors::PARAM_BG).with_blur(true)),
            Box::new(ParametersBg::new()),
            Box::new(Canvas::new()),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("0: Network").with_label("Network Menu Bar").with_item("File", &["New", "Open", "Save", "Save As"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Circular Pane", "Detach Pane", "Close Pane"]).with_context_options(context_opts.clone(), 0)),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("1: Viewport").with_label("Viewport Menu Bar").with_item("Camera", &["Perspective", "Orthographic"]).with_item("Display", &["Square Aspect"]).with_item("Guides", &["Show Grid", "Cube", "Origin", "Camera Pivot"]).with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 1)),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("2: Parameters").with_label("Parameters Menu Bar").with_item("Preset", &["Default", "Custom"]).with_item("Reset", &["All"]).with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 2)),
            Box::new(StatusBar::new().with_text("Ready")),
            Box::new(Breadcrumb::new()),
            Box::new(NodePalette::new()),
            Box::new(Spreadsheet::new()),
        ];
        
        let mut spreadsheet_menubar = MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("3: Spreadsheet").with_label("Spreadsheet Menu Bar").with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 3);
        spreadsheet_menubar.set_visible(false);
        widgets.push(Box::new(spreadsheet_menubar));

        let network_panel = Plate::new(0.0, 0.0, 0.0, 0.0).with_color([0.10, 0.10, 0.13, 0.95]);
        widgets.push(Box::new(network_panel));

        let mut positions = Vec::with_capacity(widgets.len());
        positions.resize_with(widgets.len(), || (0.0, 0.0, 0.0, 0.0));

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
            vertex_data: Vec::with_capacity(4096),
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
            window_info_buffer,
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
            node_clipboard: None,
            last_click: None,
            last_frame: Instant::now(),
            shortcut_manager,
            pending_action: None,
            exit_requested: false,
            root_window: Box::new(
                cce_ui::widget::Window::new(0.0, 0.0, lw, lh)
                    .with_background([0.0, 0.0, 0.0, 0.0])
                    .with_border([0.0, 0.0, 0.0, 0.0], 0.0)
            ),
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
            grid_snap_enabled: true,
            network_grid_visible: true,
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
            circular_network_layout: cce_ui::layout::CircularPaneLayout::new(250.0, 300.0, 180.0),
            is_detached_network,
            detached_circular_network: false,
            last_project_mod_time: {
                let default_proj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
                std::fs::metadata(&default_proj_path).and_then(|m| m.modified()).ok()
            },
            last_project_check: std::time::Instant::now(),
            last_inspector_check: std::time::Instant::now(),
            last_inspector_update: std::time::Instant::now() - std::time::Duration::from_secs(1),
            last_serialized: String::new(),
            needs_autosave: false,
            last_autosave_time: std::time::Instant::now(),
            window_x: 0,
            window_y: 0,
            active_menu_cloud_pid: None,
            active_menu_cloud_idx: None,
            uniform_background: false,
            network_opacity: 0.95,
            cell_opacity: 0.95,
            gap_opacity: 0.95,
            last_design_mod_time: {
                let design_path = DesignSettings::file_path();
                std::fs::metadata(&design_path).and_then(|m| m.modified()).ok()
            },
            last_config_mod_time: {
                let paths = [
                    "/home/lsgalante/.config/cce/config.toml",
                    "/home/lsgalante/.config/ccec/config.toml",
                ];
                let mut mod_time = None;
                for path in &paths {
                    if let Ok(m) = std::fs::metadata(path) {
                        if let Ok(t) = m.modified() {
                            mod_time = Some(t);
                            break;
                        }
                    }
                }
                mod_time
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
            loaded_project_path: None,
            last_saved_root_json: serde_json::to_string(&fs_root).unwrap_or_default(),
            recent_files,
            recent_files_list,
            recent_files_buttons,
            text_buffer_cache: std::collections::HashMap::new(),
            viewport_dirty: true,
            text_dirty: true,
            last_popover_rects: Vec::new(),
            last_status_text: String::new(),
            last_viewport_camera_pos: Vec3::ZERO,
            last_viewport_camera_rx: 0.0,
            last_viewport_camera_ry: 0.0,
            last_viewport_camera_rz: 0.0,
            last_viewport_pivot: Vec3::ZERO,
            last_viewport_zoom: 0.0,
            last_viewport_rotation_x: 0.0,
            last_viewport_rotation_y: 0.0,
            last_viewport_bg_color: [0.0; 3],
            last_viewport_show_grid: false,
            last_viewport_show_cube: false,
            last_viewport_show_origin: false,
            last_viewport_show_camera_pivot: false,
            last_viewport_width: 0,
            last_viewport_height: 0,
            last_viewport_active_camera: String::new(),
            last_viewport_show_viewport: false,
            ui_context: cce_ui::context::UiContext::new(),
        };

        state.update_inertial_settings();
        state.update_graph_settings_from_config();
        state.update_window_title();
        colors::set_node_color([
            settings.node_color[0],
            settings.node_color[1],
            settings.node_color[2],
            1.0,
        ]);
        state.ensure_menubar_subnets();
        state.apply_settings_from_menubar_subnets();
        state.sync_nodes();
        state.rebuild_scene_geometry();
        state.sync_grid_settings();
        let sg = state.show_grid;
        let sc = state.show_cube;
        let so = state.show_origin;
        let cp = state.show_camera_pivot;
        let cnp = state.circular_network_pane;
        let dcn = state.detached_circular_network;
        let sn = state.show_network;
        let sv = state.show_viewport;
        let sp = state.show_parameters;
        let ss = state.show_spreadsheet;

        state.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 0, sg);
        state.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 1, sc);
        state.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 2, so);
        state.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 3, cp);
        state.menu_mut(LEFT_MENUBAR_IDX).set_item_checked(2, 2, cnp);
        state.menu_mut(LEFT_MENUBAR_IDX).set_item_checked(2, 3, dcn);
        state.menu_mut(HEADER_IDX).set_item_checked(2, 3, dcn);
        state.menu_mut(HEADER_IDX).set_item_checked(2, 4, sn);
        state.menu_mut(HEADER_IDX).set_item_checked(2, 5, sv);
        state.menu_mut(HEADER_IDX).set_item_checked(2, 6, sp);
        state.menu_mut(HEADER_IDX).set_item_checked(2, 7, ss);

        state.rebuild_positions();
        state.apply_layout();
        state.update_panel_bounds();
        state.sync_pane_focus();
        state.upload_vertices();
        state.sync_cursor_and_selection();
        state.sync_parameters_pane();
        for w in &mut state.widgets {
            state.root_window.add_child(w.as_ptr_mut(), &mut state.ui_context);
        }
        state
    }

    pub fn sync_grid_settings(&mut self) {
        let active_node_area_y = self.positions[CONTENT_IDX].1;
        let active_node_area_x = self.positions[CONTENT_IDX].0;

        let network_grid_visible = self.network_grid_visible;
        let grid_size_x = self.grid_size_x;
        let grid_size_y = self.grid_size_y;
        let skipped_row_h = self.skipped_row_h;
        let skipped_col_w = self.skipped_col_w;
        let pan_x = self.pan_x;
        let pan_y = self.pan_y;
        let grid_snap_enabled = self.grid_snap_enabled;
        let graph = self.graph_mut();
        graph.set_show_network_grid(network_grid_visible);
        graph.set_grid_sizes(grid_size_x, grid_size_y);
        graph.set_skipped_sizes(skipped_row_h, skipped_col_w);
        graph.set_grid_origin(active_node_area_x + pan_x, active_node_area_y + pan_y);
        graph.set_grid_snap_enabled(grid_snap_enabled);
        if let Some(graph) = self.widgets[CONTENT_IDX].as_any_mut().downcast_mut::<cce_ui::widget::Graph>() {
            graph.set_uniform_background(self.uniform_background);
            graph.set_cell_opacity(self.cell_opacity);
            graph.set_gap_opacity(self.gap_opacity);
            graph.set_cell_color(self.cell_color);
            graph.set_gap_color(self.gap_color);
        }
        if let Some(menubar) = self.widgets[LEFT_MENUBAR_IDX].as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
            menubar.set_network_opacity(self.network_opacity);
        }
        if let Some(breadcrumb) = self.widgets[BREADCRUMB_IDX].as_any_mut().downcast_mut::<cce_ui::widget::Breadcrumb>() {
            breadcrumb.set_network_opacity(self.network_opacity);
        }
        if let Some(plate) = self.widgets[NETWORK_PANEL_IDX].as_any_mut().downcast_mut::<cce_ui::widget::Plate>() {
            plate.set_network_opacity(self.network_opacity);
        }
    }

    pub fn update_inertial_settings(&mut self) {
        self.last_config_read = Instant::now();
        let config_path = "/home/lsgalante/.config/cce/config.json";
        
        let mut enabled = true;
        let mut friction = 0.90;
        let mut speed = 1.0;

        if let Ok(content) = std::fs::read_to_string(config_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(inertial) = val.get("inertial") {
                    if let Some(val) = inertial.get("inertial_scroll").and_then(|v| v.as_bool()) {
                        enabled = val;
                    }
                    if let Some(friction_val) = inertial.get("scroll_friction").and_then(|v| v.as_i64()) {
                        friction = (friction_val as f32 / 1000.0).clamp(0.1, 0.999);
                    }
                    if let Some(speed_val) = inertial.get("scroll_speed").and_then(|v| v.as_f64()) {
                        speed = speed_val as f32;
                    }
                }
            }
        }
        
        self.inertial_scroll_enabled = enabled;
        self.inertial_scroll_friction = friction;
        self.scroll_speed = speed;
    }

    pub fn update_graph_settings_from_config(&mut self) {
        let config_paths = [
            "/home/lsgalante/.config/cce/config.json",
            "/home/lsgalante/.config/ccec/config.json",
        ];
        
        let mut show_grid = None;
        let mut snap_enabled = None;
        let mut uniform_background = None;
        let mut cell_opacity = None;
        let mut gap_opacity = None;
        let mut network_opacity = None;

        for path in &config_paths {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(layout) = val.get("layout") {
                        show_grid = layout.get("graph_show_grid").and_then(|v| v.as_bool());
                        snap_enabled = layout.get("graph_snap_enabled").and_then(|v| v.as_bool());
                        uniform_background = layout.get("graph_uniform_background").and_then(|v| v.as_bool());
                        network_opacity = layout.get("graph_network_opacity").and_then(|v| v.as_f64()).map(|n| n as f32);
                        cell_opacity = layout.get("graph_cell_opacity").and_then(|v| v.as_f64()).map(|n| n as f32).or(network_opacity);
                        gap_opacity = layout.get("graph_gap_opacity").and_then(|v| v.as_f64()).map(|n| n as f32).or(network_opacity);
                        break;
                    }
                }
            }
        }

        let mut changed = false;
        if let Some(val) = show_grid {
            if self.show_grid != val {
                self.show_grid = val;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 0, val);
                changed = true;
            }
        }
        if let Some(val) = snap_enabled {
            if self.grid_snap_enabled != val {
                self.grid_snap_enabled = val;
                changed = true;
            }
        }
        if let Some(val) = uniform_background {
            if self.uniform_background != val {
                self.uniform_background = val;
                changed = true;
            }
        }
        if let Some(val) = network_opacity {
            if (self.network_opacity - val).abs() > 0.001 {
                self.network_opacity = val;
                changed = true;
            }
        }
        if let Some(val) = cell_opacity {
            if (self.cell_opacity - val).abs() > 0.001 {
                self.cell_opacity = val;
                changed = true;
            }
        }
        if let Some(val) = gap_opacity {
            if (self.gap_opacity - val).abs() > 0.001 {
                self.gap_opacity = val;
                changed = true;
            }
        }
        
        if changed {
            self.sync_grid_settings();
            self.viewport_dirty = true;
        }
    }



    pub fn zoom(&mut self, factor: f32, center: Option<(f32, f32)>) {
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


    pub fn keep_cursor_in_view(&mut self) {
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

    pub fn rebuild_positions(&mut self) {
        self.clamp_splitters();
        let center_items = self.circular_network_pane;
        self.menu_mut(LEFT_MENUBAR_IDX).set_center_items(center_items);

        let body_h = self.body_h();

        if self.is_detached_network {
            let cx = self.width / 2.0;
            let cy = self.height / 2.0;
            let r = (self.width.min(self.height) / 2.0 - 10.0).max(50.0);

            self.circular_network_layout.x = cx;
            self.circular_network_layout.y = cy;
            self.circular_network_layout.r = r;

            self.positions[0] = (0.0, 0.0, 0.0, 0.0);
            self.positions[LEFT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
            if let Some(menubar) = self.widgets[LEFT_MENUBAR_IDX].as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
                menubar.set_curved_circle(None);
            }
            self.positions[BREADCRUMB_IDX] = (cx - r, cy - r + 45.0, 2.0 * r, BREADCRUMB_H);
            self.positions[CONTENT_IDX] = (cx - r, cy - r + 45.0 + BREADCRUMB_H, 2.0 * r, 2.0 * r - (45.0 + BREADCRUMB_H));
            self.positions[NETWORK_PANEL_IDX] = (cx - r, cy - r, 2.0 * r, 2.0 * r);
            self.widgets[NETWORK_PANEL_IDX].set_rect(cx - r, cy - r, 2.0 * r, 2.0 * r);
            if let Some(plate) = self.widgets[NETWORK_PANEL_IDX].as_any_mut().downcast_mut::<cce_ui::widget::Plate>() {
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

            self.widgets[0].set_visible(false);
            self.widgets[STATUS_IDX].set_visible(false);
            self.widgets[CONTENT_IDX].set_visible(true);
            self.widgets[NETWORK_PANEL_IDX].set_visible(true);
            self.widgets[LEFT_MENUBAR_IDX].set_visible(false);
            self.widgets[BREADCRUMB_IDX].set_visible(true);
            self.widgets[SPLITTER1_IDX].set_visible(false);
            self.widgets[SPLITTER2_IDX].set_visible(false);
            self.widgets[VIEWPORT_IDX].set_visible(false);
            self.widgets[RIGHT_MENUBAR_IDX].set_visible(false);
            self.widgets[PARAM_IDX].set_visible(false);
            self.widgets[PARAM_MENUBAR_IDX].set_visible(false);
            self.widgets[SPREADSHEET_IDX].set_visible(false);
            self.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(false);
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
            let paginator_w = 0.0;
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
                let mut sp_y = 0.0;
                let mut sp_h = 0.0;

                if center_visible {
                    if viewport_visible && spreadsheet_visible {
                        let viewport_h = body_h * 2.0 / 3.0;
                        let spreadsheet_h = body_h - viewport_h;
                        vp_y = 0.0;
                        vp_h = viewport_h;
                        sp_y = viewport_h;
                        sp_h = spreadsheet_h;
                    } else if viewport_visible {
                        vp_y = 0.0;
                        vp_h = body_h;
                    } else if spreadsheet_visible {
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
                if let Some(menubar) = self.widgets[LEFT_MENUBAR_IDX].as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
                    menubar.set_curved_circle(None);
                }
                self.positions[BREADCRUMB_IDX] = (cx - r, cy - r + 45.0, 2.0 * r, BREADCRUMB_H);
                self.positions[CONTENT_IDX] = (cx - r, cy - r + 45.0 + BREADCRUMB_H, 2.0 * r, 2.0 * r - (45.0 + BREADCRUMB_H));
                self.positions[NETWORK_PANEL_IDX] = (cx - r, cy - r, 2.0 * r, 2.0 * r);
                self.widgets[NETWORK_PANEL_IDX].set_rect(cx - r, cy - r, 2.0 * r, 2.0 * r);
                if let Some(plate) = self.widgets[NETWORK_PANEL_IDX].as_any_mut().downcast_mut::<cce_ui::widget::Plate>() {
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

                if let Some(menubar) = self.widgets[LEFT_MENUBAR_IDX].as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
                    menubar.set_curved_circle(None);
                }
                
                let mb_h = 0.0;
                let bc_h = if self.show_network { BREADCRUMB_H } else { 0.0 };
                let content_h = (ph - mb_h - bc_h).max(0.0);

                self.positions[CONTENT_IDX] = (px, py + mb_h + bc_h, pw, content_h);
                self.positions[NETWORK_PANEL_IDX] = (px, py, pw, ph);
                self.widgets[NETWORK_PANEL_IDX].set_rect(px, py, pw, ph);
                if let Some(plate) = self.widgets[NETWORK_PANEL_IDX].as_any_mut().downcast_mut::<cce_ui::widget::Plate>() {
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


    pub fn apply_layout(&mut self) {
        for (i, pos) in self.positions.iter().enumerate() {
            if let Some(widget) = self.widgets.get_mut(i) {
                if widget.is_dragging() { continue; }
                let (x, y, w, h) = *pos;
                widget.set_rect(x, y, w, h);
            }
        }
        self.update_recent_files_layout();
    }

    pub fn update_panel_bounds(&mut self) {
        // Unbounded 2D canvas - no clamping
    }



    pub fn sync_context_dropdowns(&mut self) {
        let selected_idx = match self.focused_pane {
            LEFT_MENUBAR_IDX => 0,
            RIGHT_MENUBAR_IDX => 1,
            PARAM_MENUBAR_IDX => 2,
            SPREADSHEET_MENUBAR_IDX => 3,
            HEADER_IDX => 4,
            _ => return,
        };
        for &widget_idx in &[HEADER_IDX, LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX] {
            self.menu_mut(widget_idx).set_context_selected(selected_idx);
        }
    }

    pub fn sync_pane_focus(&mut self) {
        for &menubar_idx in &[HEADER_IDX, LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX] {
            self.widgets[menubar_idx].set_selected(menubar_idx == self.focused_pane);
        }
        if self.focused_pane != PARAM_MENUBAR_IDX {
            self.widgets[PARAM_IDX].unfocus();
            self.sync_parameters_to_project();
        }
        self.sync_context_dropdowns();
    }

    pub fn sync_layout(&mut self) {
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

    pub fn execute_action(&mut self, action: Action) {
        let mut settings_changed = false;
        match action {
            Action::ToggleGrid => {
                self.show_grid = !self.show_grid;
                let val = self.show_grid;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 0, val);
                settings_changed = true;
            }
            Action::ToggleCube => {
                self.show_cube = !self.show_cube;
                let val = self.show_cube;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 1, val);
                settings_changed = true;
            }
            Action::ToggleOrigin => {
                self.show_origin = !self.show_origin;
                let val = self.show_origin;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 2, val);
                settings_changed = true;
            }
            Action::ToggleCameraPivot => {
                self.show_camera_pivot = !self.show_camera_pivot;
                let val = self.show_camera_pivot;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 3, val);
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
                let val = self.show_spreadsheet;
                self.menu_mut(HEADER_IDX).set_item_checked(2, 7, val);
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
                let val = self.circular_network_pane;
                self.menu_mut(LEFT_MENUBAR_IDX).set_item_checked(2, 2, val);
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
                let val = self.detached_circular_network;
                self.menu_mut(LEFT_MENUBAR_IDX).set_item_checked(2, 3, val);
                self.menu_mut(HEADER_IDX).set_item_checked(2, 3, val);

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

    pub fn read_panel_offsets(&mut self) {
        let updated_nodes = self.graph().get_nodes();
        let dir = self.current_dir_mut();
        for (i, node) in updated_nodes.iter().enumerate() {
            if let Some(child) = dir.children.get_mut(i) {
                child.position = node.position;
            }
        }
        if let Some(sel_idx) = self.graph().selected_node() {
            if let Some(node) = updated_nodes.get(sel_idx) {
                self.grid_cursor_col = node.position.0 as i32;
                self.grid_cursor_row = node.position.1 as i32;
            }
        }
    }

    pub fn sync_cursor_and_selection(&mut self) {
        if self.focused_pane != LEFT_MENUBAR_IDX {
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
            self.graph_mut().set_selected_node(Some(idx));
            if self.focused_widget != Some(CONTENT_IDX) {
                self.focused_widget = Some(CONTENT_IDX);
            }
        } else {
            self.graph_mut().set_selected_node(None);
            if self.focused_widget == Some(CONTENT_IDX) {
                self.focused_widget = None;
            }
        }
    }

    pub fn sync_cursor_and_selection_from_loaded(&mut self) {
        if let Some(sel_idx) = self.graph().selected_node() {
            let pos = {
                let dir = self.current_dir();
                if sel_idx < dir.children.len() {
                    Some(dir.children[sel_idx].position)
                } else {
                    None
                }
            };
            if let Some((pos_x, pos_y)) = pos {
                self.grid_cursor_col = pos_x as i32;
                self.grid_cursor_row = pos_y as i32;
            }
        } else {
            self.sync_cursor_and_selection();
        }
    }













    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            let old_width = self.width;
            self.physical_width = width;
            self.physical_height = height;
            self.width = width as f32 / self.scale as f32;
            self.height = height as f32 / self.scale as f32;
            self.root_window.set_rect(0.0, 0.0, self.width, self.height);
            self.wgpu_adapter.resize(width, height);

            let (tex, view) = self.create_depth_texture();
            self.depth_texture = tex;
            self.depth_texture_view = view;

            let (b_tex, b_view) = self.create_backdrop_texture();
            self.backdrop_texture = b_tex;
            self.backdrop_texture_view = b_view;

            let window_info_data = [
                width as f32,
                height as f32,
                cce_ui::color::window_corner_radius() * self.scale as f32,
                0.0,
            ];
            self.wgpu_adapter.queue.write_buffer(&self.window_info_buffer, 0, bytemuck::cast_slice(&window_info_data));

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
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.window_info_buffer.as_entire_binding(),
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
            self.viewport_dirty = true;
        }
    }

    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
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
                                let (gx, gy) = w.as_graph_controller().expect("not a GraphController").grid_origin();
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
                    self.sync_parameters_pane();


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
                                if idx == PARAM_IDX {
                                    self.sync_parameters_to_project();
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
                                    self.circular_network_layout.hit_test_content(cx, cy, 0.0, BREADCRUMB_H)
                                } else if i == LEFT_MENUBAR_IDX {
                                    false
                                } else if i == BREADCRUMB_IDX {
                                    self.circular_network_layout.hit_test_breadcrumb(cx, cy, 0.0, BREADCRUMB_H)
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
                println!("DEBUG: MouseInput state={:?} button={:?} cursor=({}, {})", btn_state, button, self.cursor_x, self.cursor_y);
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
                if self.node_palette_visible {
                    if *btn_state == ElementState::Pressed {
                        let (px, py, pw, ph) = self.palette().panel_rect();
                        if self.cursor_x < px || self.cursor_x > px + pw || self.cursor_y < py || self.cursor_y > py + ph {
                            self.close_node_palette();
                        }
                    }
                    self.upload_vertices();
                    return true;
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

                let hits_widget = |state: &State, i: usize, x: f32, y: f32| -> bool {
                    if state.circular_network_pane && (i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX) {
                        if i == CONTENT_IDX {
                            state.circular_network_layout.hit_test_content(x, y, 0.0, BREADCRUMB_H)
                        } else if i == LEFT_MENUBAR_IDX {
                            false
                        } else if i == BREADCRUMB_IDX {
                            state.circular_network_layout.hit_test_breadcrumb(x, y, 0.0, BREADCRUMB_H)
                        } else {
                            false
                        }
                    } else {
                        state.widgets[i].hit_test(x, y, &state.ui_context)
                    }
                };

                let in_circle_network_pane = if self.circular_network_pane {
                    self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, 0.0, BREADCRUMB_H)
                } else {
                    in_network_pane
                };

                match btn_state {
                    ElementState::Pressed => {
                        let hits_any_menu = (0..self.widgets.len()).any(|i| {
                            hits_widget(self, i, self.cursor_x, self.cursor_y)
                                && self.widgets[i].as_menu_controller().and_then(|m| m.get_menu_items_at(self.cursor_x, self.cursor_y)).is_some()
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
                            let hit_menubar = false;
                            if hit_menubar {
                                if self.menu(LEFT_MENUBAR_IDX).get_menu_items_at(self.cursor_x, self.cursor_y).is_some() {
                                    self.focused_pane = LEFT_MENUBAR_IDX;
                                    self.widgets[LEFT_MENUBAR_IDX].set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                                    if self.widgets[LEFT_MENUBAR_IDX].mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y, &mut self.ui_context) {
                                        changed = true;
                                    }
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
                            if self.widgets[i].as_menu_controller().map(|m| m.is_menu_open()).unwrap_or(false)
                                && hits_widget(self, i, self.cursor_x, self.cursor_y)
                            {
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
                                && self.menu(i).get_menu_items_at(self.cursor_x, self.cursor_y).is_none()
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
                            self.widgets[i].set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                            if self.widgets[i].mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y, &mut self.ui_context) {
                                changed = true;
                                if i == PARAM_IDX {
                                    self.sync_parameters_to_project();
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
                                if self.widgets[i].as_menu_controller().map(|m| m.is_menu_bar()).unwrap_or(false)
                                    && !self.widgets[i].focused(&self.ui_context)
                                {
                                    self.widgets[i].unfocus();
                                    self.focused_widget = None;
                                }
                            }
                            if i == CONTENT_IDX {
                                self.sync_parameters_pane();
                                if let Some(slot_idx) = self.graph().selected_node() {
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
                                if let Some(dir_idx) = self.graph().double_clicked_node() {
                                    self.graph_mut().clear_double_clicked_node();
                                    let dir = self.current_dir();
                                    if dir_idx < dir.children.len() && (dir.children[dir_idx].node_type == "node" || dir.children[dir_idx].node_type == "utility" || !dir.children[dir_idx].children.is_empty()) {
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
                                let updated_nodes = self.graph().get_nodes();
                                let dir = self.current_dir_mut();
                                for (i, node) in updated_nodes.iter().enumerate() {
                                    if let Some(child) = dir.children.get_mut(i) {
                                        child.position = node.position;
                                    }
                                }
                                if let Some(sel_idx) = self.graph().selected_node() {
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
                        let mut sync_params = false;
                        {
                            let ctx = &mut self.ui_context;
                            for (i, w) in self.widgets.iter_mut().enumerate() {
                                w.set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                                if w.mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y, ctx) {
                                    changed = true;
                                    if i == PARAM_IDX {
                                        sync_params = true;
                                    }
                                }
                            }
                        }
                        if sync_params {
                            self.sync_parameters_to_project();
                        }
                    }
                }


                if let Some((i, visible)) = self.graph_mut().take_node_geom_toggle() {
                    self.current_dir_mut().children[i].geometry_visible = visible;
                    self.rebuild_scene_geometry();
                    changed = true;
                }

                if let Some((input_node_id, output_node_name)) = self.graph_mut().take_pending_connection() {
                    let dir = self.current_dir_mut();
                    if let Some(child) = dir.children.iter_mut().find(|c| c.id == input_node_id) {
                        if let Some(p) = child.params.iter_mut().find(|p| p.name == "Input") {
                            p.default = output_node_name;
                            self.sync_nodes();
                            self.rebuild_scene_geometry();
                            self.upload_vertices();
                            self.sync_parameters_pane();
                            changed = true;
                        }
                    }
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

                if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) {
                    self.graph_mut().cancel_connecting();
                    self.upload_vertices();
                    return true;
                }
                let mut changed = false;
                if event.state == ElementState::Pressed {
                        let is_plain_key = !self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key();
                        let is_alt_key = self.modifiers.alt_key() && !self.modifiers.control_key() && !self.modifiers.super_key();
                        let is_ctrl_only = self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key() && !self.modifiers.shift_key();

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
                            if event.logical_key == Key::Named(NamedKey::Delete) {
                                if is_plain_key && self.focused_pane == LEFT_MENUBAR_IDX {
                                    if let Some(slot_idx) = self.graph().selected_node() {
                                        if self.delete_node(slot_idx) {
                                            changed = true;
                                        }
                                    }
                                }
                            } else if let Key::Character(s) = &event.logical_key {
                                if is_plain_key {
                                    match s.as_str() {
                                        "e" | "E" => {
                                            if self.focused_pane == LEFT_MENUBAR_IDX {
                                                if let Some(slot_idx) = self.graph().selected_node() {
                                                    let dir = self.current_dir();
                                                    if slot_idx < dir.children.len() && dir.children[slot_idx].node_type != "utility" {
                                                        let visible = !dir.children[slot_idx].geometry_visible;
                                                        self.current_dir_mut().children[slot_idx].geometry_visible = visible;
                                                        self.sync_nodes();
                                                        self.rebuild_scene_geometry();
                                                        changed = true;
                                                    }
                                                }
                                            }
                                        }
                                        "u" | "U" => {
                                            if self.focused_pane == LEFT_MENUBAR_IDX {
                                                if self.move_up() {
                                                    changed = true;
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
                                        "i" | "I" => {
                                            if self.focused_pane == LEFT_MENUBAR_IDX {
                                                if let Some(slot_idx) = self.graph().selected_node() {
                                                    let dir = self.current_dir();
                                                     if slot_idx < dir.children.len() && (dir.children[slot_idx].node_type == "node" || dir.children[slot_idx].node_type == "utility" || !dir.children[slot_idx].children.is_empty()) {
                                                         self.current_path.push(slot_idx);
                                                         self.on_path_changed();
                                                         changed = true;
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
                                } else if is_ctrl_only && self.focused_pane == LEFT_MENUBAR_IDX {
                                    match s.as_str() {
                                        "c" | "C" => {
                                            if let Some(slot_idx) = self.graph().selected_node() {
                                                let dir = self.current_dir();
                                                if slot_idx < dir.children.len() {
                                                    self.node_clipboard = Some(dir.children[slot_idx].clone());
                                                }
                                            }
                                        }
                                        "x" | "X" => {
                                            if let Some(slot_idx) = self.graph().selected_node() {
                                                let dir = self.current_dir();
                                                if slot_idx < dir.children.len() {
                                                    self.node_clipboard = Some(dir.children[slot_idx].clone());
                                                    self.delete_node(slot_idx);
                                                    changed = true;
                                                }
                                            }
                                        }
                                        "v" | "V" => {
                                            if let Some(ref clipboard_node) = self.node_clipboard {
                                                let mut node = clipboard_node.clone();
                                                fn regenerate_ids(n: &mut FsNode) {
                                                    n.id = generate_node_id();
                                                    for child in &mut n.children {
                                                        regenerate_ids(child);
                                                    }
                                                }
                                                regenerate_ids(&mut node);
                                                let start_x = self.grid_cursor_col as f32;
                                                let start_y = self.grid_cursor_row as f32;
                                                let (nx, ny) = self.find_empty_cell(start_x, start_y, None);
                                                node.position = (nx, ny);
                                                self.current_dir_mut().children.push(node);
                                                self.grid_cursor_col = nx as i32;
                                                self.grid_cursor_row = ny as i32;
                                                self.sync_nodes();
                                                self.sync_cursor_and_selection();
                                                self.rebuild_positions();
                                                self.apply_layout();
                                                self.update_panel_bounds();
                                                self.rebuild_scene_geometry();
                                                self.upload_vertices();
                                                self.viewport_dirty = true;
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

    pub fn render(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        if now.duration_since(self.last_config_read).as_secs_f32() > 2.0 {
            self.last_config_read = now;
            let config_paths = [
                "/home/lsgalante/.config/cce/config.json",
                "/home/lsgalante/.config/ccec/config.json",
            ];
            let mut current_mod_time = None;
            for path in &config_paths {
                if let Ok(m) = std::fs::metadata(path) {
                    if let Ok(t) = m.modified() {
                        current_mod_time = Some(t);
                        break;
                    }
                }
            }
            if current_mod_time != self.last_config_mod_time {
                self.last_config_mod_time = current_mod_time;
                cce_ui::layout::reload_config();
                self.update_inertial_settings();
                self.update_graph_settings_from_config();
                self.rebuild_positions();
                self.apply_layout();
                self.upload_vertices();
            } else {
                self.update_inertial_settings();
            }
            let design_path = DesignSettings::file_path();
            if let Ok(m) = std::fs::metadata(&design_path) {
                if let Ok(mod_time) = m.modified() {
                    if Some(mod_time) != self.last_design_mod_time {
                        self.last_design_mod_time = Some(mod_time);
                         let settings = DesignSettings::load();
                         self.square_viewport = settings.square_viewport;
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
        if cce_ui::widget::hover_animation::tick(dt) {
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
        if !self.is_detached_network && self.show_viewport {
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

                let viewport_changed = self.viewport_dirty
                    || self.last_viewport_camera_pos != camera_pos
                    || self.last_viewport_camera_rx != rx
                    || self.last_viewport_camera_ry != ry
                    || self.last_viewport_camera_rz != rz
                    || self.last_viewport_pivot != pivot
                    || self.last_viewport_zoom != self.viewport_zoom
                    || self.last_viewport_rotation_x != self.rotation_x
                    || self.last_viewport_rotation_y != self.rotation_y
                    || self.last_viewport_bg_color != self.viewport_bg_color
                    || self.last_viewport_show_grid != self.show_grid
                    || self.last_viewport_show_cube != self.show_cube
                    || self.last_viewport_show_origin != self.show_origin
                    || self.last_viewport_show_camera_pivot != self.show_camera_pivot
                    || self.last_viewport_width != cw
                    || self.last_viewport_height != ch
                    || self.last_viewport_active_camera != self.active_camera
                    || self.last_viewport_show_viewport != self.show_viewport;

                if viewport_changed {
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
                    let window_size = [self.physical_width as f32, self.physical_height as f32];
                    let window_radius = cce_ui::color::window_corner_radius() * self.scale as f32;

                    let uniforms = ViewportUniforms {
                        mvp: mvp.to_cols_array_2d(),
                        window_size,
                        window_radius,
                        _padding: 0.0,
                    };
                    self.wgpu_adapter.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

                    let mvp_grid = proj * view_mat * model;
                    let uniforms_grid = ViewportUniforms {
                        mvp: mvp_grid.to_cols_array_2d(),
                        window_size,
                        window_radius,
                        _padding: 0.0,
                    };
                    self.wgpu_adapter.queue.write_buffer(&self.uniform_buffer_grid, 0, bytemuck::cast_slice(&[uniforms_grid]));

                    let cam_angle_y = camera_pos.x.atan2(camera_pos.z);
                    let rot_angle = if self.active_camera != "Default Camera" {
                        total_ry
                    } else {
                        self.rotation_y + cam_angle_y
                    };
                    let model_pivot = Mat4::from_translation(pivot) * Mat4::from_rotation_y(rot_angle);
                    let mvp_pivot = proj * view_mat * model_pivot;
                    let uniforms_pivot = ViewportUniforms {
                        mvp: mvp_pivot.to_cols_array_2d(),
                        window_size,
                        window_radius,
                        _padding: 0.0,
                    };
                    self.wgpu_adapter.queue.write_buffer(&self.uniform_buffer_pivot, 0, bytemuck::cast_slice(&[uniforms_pivot]));

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

                    // Update viewport cache
                    self.last_viewport_camera_pos = camera_pos;
                    self.last_viewport_camera_rx = rx;
                    self.last_viewport_camera_ry = ry;
                    self.last_viewport_camera_rz = rz;
                    self.last_viewport_pivot = pivot;
                    self.last_viewport_zoom = self.viewport_zoom;
                    self.last_viewport_rotation_x = self.rotation_x;
                    self.last_viewport_rotation_y = self.rotation_y;
                    self.last_viewport_bg_color = self.viewport_bg_color;
                    self.last_viewport_show_grid = self.show_grid;
                    self.last_viewport_show_cube = self.show_cube;
                    self.last_viewport_show_origin = self.show_origin;
                    self.last_viewport_show_camera_pivot = self.show_camera_pivot;
                    self.last_viewport_width = cw;
                    self.last_viewport_height = ch;
                    self.last_viewport_active_camera = self.active_camera.clone();
                    self.last_viewport_show_viewport = self.show_viewport;
                    self.viewport_dirty = false;
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
                wgpu::TexelCopyTextureInfo {
                    texture: &self.backdrop_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
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

