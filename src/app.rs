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

use cce_ui::widget::{Adapted, Breadcrumb, MenuBar, MenuController, ParametersBg, Splitter, Spreadsheet, StatusBar, TextLabel, WidgetHost, GraphNode, Graph, Button, Label, Dropdown};
use cce_ui::widget::UiContext;
use crate::playbar::Playbar;
use crate::viewport_3d::Viewport3D;
use cce_ui::colors;
use glam::{Mat4, Vec3};

use crate::geometry::*;
use crate::shortcut::{ShortcutManager, Action};
use cce_ui::vk::{SceneDraw, TextSpan};
use cce_ui::engine::Vertex;
use crate::window::WindowEvent;

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
pub const PARAM_IDX: usize = 5;
pub const CANVAS_IDX: usize = 6;
pub const LEFT_MENUBAR_IDX: usize = 7;
pub const RIGHT_MENUBAR_IDX: usize = 8;
pub const PARAM_MENUBAR_IDX: usize = 9;
pub const STATUS_IDX: usize = 10;
pub const BREADCRUMB_IDX: usize = 11;
pub const SPREADSHEET_IDX: usize = 12;
pub const SPREADSHEET_MENUBAR_IDX: usize = 13;
pub const NETWORK_PANEL_IDX: usize = 14;
pub const PLAYBAR_IDX: usize = 15;

pub const WIDGET_COUNT: usize = 16;

/// The roster, concretely typed (Phase 6bb): every slot's type is statically known — the
/// old `Vec<Box<dyn WidgetHost>>` erased that and pinned `WidgetHost`'s full surface through the
/// broadcast loops. Boxed as a whole so registered widget pointers stay stable while the
/// containing `State` moves. The `*_IDX` constants keep addressing the same slots through
/// `get_dyn`/`get_dyn_mut` for the genuinely index-driven paths (draw order, focus cycling,
/// broadcast loops); everything else reaches the concrete field.
pub struct WidgetSlots {
    pub header: Adapted<MenuBar>,
    pub content: Adapted<Graph>,
    pub splitter1: Adapted<Splitter>,
    pub viewport: Adapted<Viewport3D>,
    pub splitter2: Adapted<Splitter>,
    pub param: Adapted<ParametersBg>,
    pub canvas: Adapted<Canvas>,
    pub left_menubar: Adapted<MenuBar>,
    pub right_menubar: Adapted<MenuBar>,
    pub param_menubar: Adapted<MenuBar>,
    pub status: Adapted<StatusBar>,
    pub breadcrumb: Adapted<Breadcrumb>,
    pub spreadsheet: Adapted<Spreadsheet>,
    pub spreadsheet_menubar: Adapted<MenuBar>,
    pub network_panel: Adapted<PassivePlate>,
    pub playbar: Adapted<Playbar>,
}

impl WidgetSlots {

    // Per-slot drag queries (the ControlPanel endgame took `draggable`/`is_dragging`
    // off `WidgetHost`): the roster routes an index to the concrete slot's inherent
    // `Adapted` read, like the other value drains.
    pub fn draggable(&self, idx: usize) -> bool {
        match idx {
            HEADER_IDX => self.header.draggable(),
            CONTENT_IDX => self.content.draggable(),
            SPLITTER1_IDX => self.splitter1.draggable(),
            VIEWPORT_IDX => self.viewport.draggable(),
            SPLITTER2_IDX => self.splitter2.draggable(),
            PARAM_IDX => self.param.draggable(),
            CANVAS_IDX => self.canvas.draggable(),
            LEFT_MENUBAR_IDX => self.left_menubar.draggable(),
            RIGHT_MENUBAR_IDX => self.right_menubar.draggable(),
            PARAM_MENUBAR_IDX => self.param_menubar.draggable(),
            STATUS_IDX => self.status.draggable(),
            BREADCRUMB_IDX => self.breadcrumb.draggable(),
            SPREADSHEET_IDX => self.spreadsheet.draggable(),
            SPREADSHEET_MENUBAR_IDX => self.spreadsheet_menubar.draggable(),
            NETWORK_PANEL_IDX => self.network_panel.draggable(),
            PLAYBAR_IDX => self.playbar.draggable(),
            _ => panic!("widget slot index out of range: {idx}"),
        }
    }

    pub fn is_dragging(&self, idx: usize) -> bool {
        match idx {
            HEADER_IDX => self.header.is_dragging(),
            CONTENT_IDX => self.content.is_dragging(),
            SPLITTER1_IDX => self.splitter1.is_dragging(),
            VIEWPORT_IDX => self.viewport.is_dragging(),
            SPLITTER2_IDX => self.splitter2.is_dragging(),
            PARAM_IDX => self.param.is_dragging(),
            CANVAS_IDX => self.canvas.is_dragging(),
            LEFT_MENUBAR_IDX => self.left_menubar.is_dragging(),
            RIGHT_MENUBAR_IDX => self.right_menubar.is_dragging(),
            PARAM_MENUBAR_IDX => self.param_menubar.is_dragging(),
            STATUS_IDX => self.status.is_dragging(),
            BREADCRUMB_IDX => self.breadcrumb.is_dragging(),
            SPREADSHEET_IDX => self.spreadsheet.is_dragging(),
            SPREADSHEET_MENUBAR_IDX => self.spreadsheet_menubar.is_dragging(),
            NETWORK_PANEL_IDX => self.network_panel.is_dragging(),
            PLAYBAR_IDX => self.playbar.is_dragging(),
            _ => panic!("widget slot index out of range: {idx}"),
        }
    }

    pub fn get_dyn(&self, idx: usize) -> &(dyn WidgetHost + 'static) {
        match idx {
            HEADER_IDX => &self.header,
            CONTENT_IDX => &self.content,
            SPLITTER1_IDX => &self.splitter1,
            VIEWPORT_IDX => &self.viewport,
            SPLITTER2_IDX => &self.splitter2,
            PARAM_IDX => &self.param,
            CANVAS_IDX => &self.canvas,
            LEFT_MENUBAR_IDX => &self.left_menubar,
            RIGHT_MENUBAR_IDX => &self.right_menubar,
            PARAM_MENUBAR_IDX => &self.param_menubar,
            STATUS_IDX => &self.status,
            BREADCRUMB_IDX => &self.breadcrumb,
            SPREADSHEET_IDX => &self.spreadsheet,
            SPREADSHEET_MENUBAR_IDX => &self.spreadsheet_menubar,
            NETWORK_PANEL_IDX => &self.network_panel,
            PLAYBAR_IDX => &self.playbar,
            _ => panic!("widget slot index out of range: {idx}"),
        }
    }

    pub fn get_dyn_mut(&mut self, idx: usize) -> &mut (dyn WidgetHost + 'static) {
        match idx {
            HEADER_IDX => &mut self.header,
            CONTENT_IDX => &mut self.content,
            SPLITTER1_IDX => &mut self.splitter1,
            VIEWPORT_IDX => &mut self.viewport,
            SPLITTER2_IDX => &mut self.splitter2,
            PARAM_IDX => &mut self.param,
            CANVAS_IDX => &mut self.canvas,
            LEFT_MENUBAR_IDX => &mut self.left_menubar,
            RIGHT_MENUBAR_IDX => &mut self.right_menubar,
            PARAM_MENUBAR_IDX => &mut self.param_menubar,
            STATUS_IDX => &mut self.status,
            BREADCRUMB_IDX => &mut self.breadcrumb,
            SPREADSHEET_IDX => &mut self.spreadsheet,
            SPREADSHEET_MENUBAR_IDX => &mut self.spreadsheet_menubar,
            NETWORK_PANEL_IDX => &mut self.network_panel,
            PLAYBAR_IDX => &mut self.playbar,
            _ => panic!("widget slot index out of range: {idx}"),
        }
    }
}



pub const HEADER_H: f32 = 0.0;
pub const STATUS_H: f32 = 0.0;
pub const MENUBAR_H: f32 = 0.0;
pub const SPLITTER_W: f32 = 6.0;

pub const MIN_COLUMN: f32 = 120.0;
pub const BREADCRUMB_H: f32 = 24.0;
pub const PLAYBAR_H: f32 = 36.0;

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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum McpAction {
    Up,
    Enter { slot: usize },
    Select { slot: usize },
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
    /// Execute a label-matched menu-pane action ("Show Spreadsheet Pane", "Save", ...)
    /// — the items `menu_click`'s index-matched menubar dispatch cannot reach.
    MenuAction { label: String },
}

#[derive(Debug, Clone)]
pub enum CustomEvent {
    /// An MCP `tools/call` from the embedded MCP server (carries its own
    /// reply channel) — the tool name is an `McpAction` tag, or `get_state`.
    McpCall(cce_ui::mcp::McpToolCall),
    /// A fire-and-forget action from an app-internal thread (the cce-files
    /// choosers deliver their picked path this way).
    RunAction(McpAction),
    /// A cce-cloud popup thread announced its process (CloudPopupTracker
    /// adoption — the add-node palette).
    CloudSpawned { pid: u32, source: String },
    /// A cce-cloud popup thread reported its popup closed.
    CloudClosed { pid: u32, source: String },
    /// App-requested exit (menu File > Exit, MCP menu_action): the engine's
    /// update hook is the only place with exit access, so input handlers that
    /// see `exit_requested` route it here.
    Exit,
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
fn default_grid_color() -> [f32; 3] { [0.35, 0.35, 0.40] }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ViewportSettings {
    pub bg_color: [f32; 3],
    pub square: bool,
    pub show_camera_pivot_enabled: bool,
    pub camera_pivot_size: f32,
    pub show_grid_enabled: bool,
    pub show_cube_enabled: bool,
    pub show_origin_enabled: bool,
    pub origin_size: f32,
    #[serde(default = "default_grid_thickness")]
    pub grid_thickness: f32,
    #[serde(default = "default_grid_color")]
    pub grid_color: [f32; 3],
}

impl Default for ViewportSettings {
    fn default() -> Self {
        Self {
            bg_color: [0.05, 0.05, 0.10],
            square: false,
            show_camera_pivot_enabled: false,
            camera_pivot_size: 1.0,
            show_grid_enabled: true,
            show_cube_enabled: false,
            show_origin_enabled: true,
            origin_size: 1.0,
            grid_thickness: default_grid_thickness(),
            grid_color: default_grid_color(),
        }
    }
}

/// The node grid's cell size and the gap between cells, as configured — `spacing_*` is the
/// cell, `gap_*` the space after it, and one node slot to the next is the two added up.
///
/// Config-owned (`style.surface.graph.*`), NOT state: it is user-authored, so the app reads
/// it and never writes it back — the same split `../CLAUDE.md` describes for scroll
/// behavior. Zoom scales these in memory; the configured values are the 100% baseline that
/// Reset Zoom returns to.
pub fn configured_grid_geometry() -> (f32, f32, f32, f32) {
    (
        cce_ui::layout::graph_spacing_x(),
        cce_ui::layout::graph_spacing_y(),
        cce_ui::layout::graph_gap_col_w(),
        cce_ui::layout::graph_gap_row_h(),
    )
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DesignSettings {
    #[serde(default)]
    pub viewport: ViewportSettings,
}

fn float_array_to_hex(rgb: &[f32; 3]) -> String {
    let r = (rgb[0] * 255.0).clamp(0.0, 255.0).round() as u8;
    let g = (rgb[1] * 255.0).clamp(0.0, 255.0).round() as u8;
    let b = (rgb[2] * 255.0).clamp(0.0, 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

fn hex_to_float_array(hex: &str) -> Option<[f32; 3]> {
    cce_ui::color::parse_hex_rgb(hex)
}

impl DesignSettings {
    fn file_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut path = std::path::PathBuf::from(home);
        path.push(".config");
        path.push("cce");
        path.push("cce-designer");
        path.push("state.kdl");
        path
    }

    fn load_kdl(path: &std::path::Path) -> Option<Self> {
        let content = fs::read_to_string(path).ok()?;
        let mut json_val = cce_ui::config::parse_kdl_to_json(&content);
        // Convert hex strings back to color arrays
        if let Some(obj) = json_val.as_object_mut() {
            if let Some(viewport) = obj.get_mut("viewport").and_then(|v| v.as_object_mut()) {
                if let Some(serde_json::Value::String(hex_str)) = viewport.get("bg_color") {
                    if let Some(arr) = hex_to_float_array(hex_str) {
                        if let Ok(arr_val) = serde_json::to_value(arr) {
                            viewport.insert("bg_color".to_string(), arr_val);
                        }
                    }
                }
                if let Some(serde_json::Value::String(hex_str)) = viewport.get("grid_color") {
                    if let Some(arr) = hex_to_float_array(hex_str) {
                        if let Ok(arr_val) = serde_json::to_value(arr) {
                            viewport.insert("grid_color".to_string(), arr_val);
                        }
                    }
                }
            }
        }
        Some(serde_json::from_value::<Self>(json_val).unwrap_or_else(|_| Self::default()))
    }

    fn load() -> Self {
        let path = Self::file_path();
        if let Some(settings) = Self::load_kdl(&path) {
            return settings;
        }

        // Migration fallback: the pre-rename design.kdl (same format), then
        // the ancient design.json — load, re-save as state.kdl, delete the old.
        let legacy_kdl = path.with_file_name("design.kdl");
        if let Some(settings) = Self::load_kdl(&legacy_kdl) {
            settings.save();
            let _ = fs::remove_file(legacy_kdl);
            return settings;
        }
        let legacy_json = path.with_file_name("design.json");
        if let Ok(content) = fs::read_to_string(&legacy_json) {
            if let Ok(settings) = serde_json::from_str::<Self>(&content) {
                settings.save();
                let _ = fs::remove_file(legacy_json);
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
        if let Ok(mut json_val) = serde_json::to_value(self) {
            // Convert color arrays to hex strings
            if let Some(obj) = json_val.as_object_mut() {
                if let Some(viewport) = obj.get_mut("viewport").and_then(|v| v.as_object_mut()) {
                    if let Some(val) = viewport.get("bg_color") {
                        if let Ok(arr) = serde_json::from_value::<[f32; 3]>(val.clone()) {
                            let hex_str = float_array_to_hex(&arr);
                            viewport.insert("bg_color".to_string(), serde_json::Value::String(hex_str));
                        }
                    }
                    if let Some(val) = viewport.get("grid_color") {
                        if let Ok(arr) = serde_json::from_value::<[f32; 3]>(val.clone()) {
                            let hex_str = float_array_to_hex(&arr);
                            viewport.insert("grid_color".to_string(), serde_json::Value::String(hex_str));
                        }
                    }
                }
            }
            let kdl_str = cce_ui::config::json_to_kdl_string(&json_val);
            let _ = fs::write(path, kdl_str);
        }
    }
}

/// Dissolved cce-ui `Plate` (Phase 6as): a passive translucent panel — configured color
/// at `plate_opacity` times the network fade, alpha negated when blur is on (the
/// scenefx blur marker) — with no children and no events.
pub struct PassivePlate {
    color: [f32; 4],
    blur: bool,
    pub network_opacity: f32,
    /// Circular hit shape while the network pane is round (the legacy Plate marker).
    curved_circle: Option<(f32, f32, f32)>,
}

impl PassivePlate {
    pub fn new(color: [f32; 4], blur: bool) -> cce_ui::widget::Adapted<PassivePlate> {
        cce_ui::widget::Adapted::new(Self {
            color,
            blur,
            network_opacity: 1.0,
            curved_circle: None,
        })
    }

    pub fn set_network_opacity(&mut self, opacity: f32) {
        self.network_opacity = opacity;
    }

    pub fn set_curved_circle(&mut self, circle: Option<(f32, f32, f32)>) {
        self.curved_circle = circle;
    }
}

impl cce_ui::widget::Layout for PassivePlate {}

impl cce_ui::widget::Paint for PassivePlate {
    /// Nothing: the plate's fill AND border are drawn by `append_widget_plate` (or, in
    /// circular mode, the circle+arc branch) in the designer's hand-ordered paint walk.
    /// The default `paint` would emit a plain square-cornered quad of the whole rect,
    /// which the walk then re-drew through `extra_quads` ON TOP of the rounded plate —
    /// square corners over the rounded ones.
    fn paint(&self, _rect: cce_ui::scene::layout::Rect, _ctx: &mut cce_ui::scene::paint::PaintCtx) {}

    fn color(&self) -> [f32; 4] {
        let mut c = self.color;
        c[3] *= cce_ui::layout::plate_opacity();
        c[3] *= self.network_opacity;
        if self.blur && cce_ui::colors::plate_blur() {
            c[3] = -c[3].abs();
        }
        c
    }

    fn corner_style(&self, _rect: cce_ui::scene::layout::Rect) -> Option<(f32, (bool, bool, bool, bool))> {
        let r = cce_ui::layout::plate_corner_radius();
        let on = r > 0.0;
        Some((r, (on, on, on, on)))
    }

    fn solid_border(&self) -> Option<([f32; 4], f32)> {
        if let Some(bc) = cce_ui::colors::plate_border_color() {
            Some((bc, cce_ui::colors::plate_border_thickness()))
        } else {
            None
        }
    }
}

impl cce_ui::widget::Input for PassivePlate {
    fn hit(&self, rect: cce_ui::scene::layout::Rect, x: f32, y: f32) -> bool {
        if let Some((cx, cy, r)) = self.curved_circle {
            let dx = x - cx;
            let dy = y - cy;
            return dx * dx + dy * dy <= r * r;
        }
        // Exclusive right/bottom edges, like the legacy Plate hit test.
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    }
}


/// App-owned copy of the dissolved cce-ui `Canvas` (Phase 6ay part 2): the transparent
/// hit-through pane behind the network area. Verbatim; dies with the machinery retype.
pub struct Canvas;

impl Canvas {
    pub fn new() -> cce_ui::widget::Adapted<Canvas> { cce_ui::widget::Adapted::new(Canvas) }
}

impl cce_ui::widget::Layout for Canvas {}

impl cce_ui::widget::Paint for Canvas {
    fn color(&self) -> [f32; 4] { [0.0, 0.0, 0.0, 0.0] }
}

impl cce_ui::widget::Input for Canvas {
    // Hit-through: the pane never claims the pointer (the graph decides its own hits).
    fn hit(&self, _rect: cce_ui::scene::layout::Rect, _x: f32, _y: f32) -> bool { false }
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeDirection {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

/// An app-mode drag: one of the designer's own windowing gestures (floating-pane edge
/// resizes), carrying its whole gesture state. The event-layer redesign splits this out
/// of `drag_widget`, which previously doubled as widget-drag index AND app-mode-drag
/// marker (via three `is_resizing_*` flags keyed against pane indices) — `drag_widget`
/// now ONLY ever names a widget drag (a slot whose `Input` drag hooks are driving:
/// panel move, graph node drag, param slider, spreadsheet scroll). Exactly one of
/// `app_drag`/`drag_widget` is armed per press.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AppDrag {
    NetworkResize { dir: ResizeDirection, start_rect: (f32, f32, f32, f32), start_mouse: (f32, f32) },
    ParamResize { start_w: f32, start_mouse_x: f32 },
    SpreadsheetResize { start_h: f32, start_mouse_y: f32 },
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewportUniforms {
    pub mvp: [[f32; 4]; 4],
    pub window_size: [f32; 2],
    pub window_radius: f32,
    pub _padding: f32,
}

/// The designer's persistent 3D meshes, created in `renderer_init` once the
/// engine's renderer exists.
#[derive(Clone, Copy)]
pub struct SceneMeshes {
    pub cube: cce_ui::vk::MeshId,
    pub viewport_bg: cce_ui::vk::MeshId,
    pub spheres: cce_ui::vk::MeshId,
    pub grid: cce_ui::vk::MeshId,
    pub origin: cce_ui::vk::MeshId,
    pub pivot: cce_ui::vk::MeshId,
}

/// A left-press on the detached circular window's chrome that becomes an
/// interactive move/resize once the pointer travels past a small threshold
/// (so a plain click doesn't start a compositor grab).
#[derive(Clone, Copy, Debug)]
pub struct PendingWindowDrag {
    pub start_x: f32,
    pub start_y: f32,
    pub action: cce_ui::engine::WindowAction,
}

pub struct State {
    /// Window title; the engine polls `Application::settings` and applies it.
    pub title: String,

    /// GPU meshes — `None` until `renderer_init`.
    pub meshes: Option<SceneMeshes>,
    /// CPU-staged mesh updates, flushed in `stage_renderer`.
    pub pending_grid: Option<Vec<Vertex3D>>,
    pub pending_origin: Option<Vec<Vertex3D>>,
    pub pending_pivot: Option<Vec<Vertex3D>>,
    pub pending_viewport_bg: Option<Vec<Vertex3D>>,
    /// The spheres mesh needs re-upload from `rt_sphere_verts`.
    pub spheres_dirty: bool,
    /// Renderer corner radius applied last frame (physical px); re-set on change.
    pub last_corner_radius: f32,
    pub pending_window_drag: Option<PendingWindowDrag>,
    pub window_action: Option<cce_ui::engine::WindowAction>,
    pub vertex_count_spheres: u32,
    pub node_color: [f32; 4],
    pub grid_color: [f32; 3],
    pub cell_color: [f32; 3],
    pub gap_color: [f32; 3],
    pub origin_size: f32,
    pub camera_pivot_size: f32,

    pub fs_root: FsNode,
    pub node_templates: Vec<NodeTemplate>,
    pub current_path: Vec<usize>,
    pub node_clipboard: Option<FsNode>,
    pub last_click: Option<(Instant, usize)>,
    pub last_frame: Instant,

    pub shortcut_manager: ShortcutManager,
    pub pending_action: Option<Action>,
    pub exit_requested: bool,
    /// Engine event-loop sender so app-spawned threads (the cce-files
    /// choosers) can deliver results back as `CustomEvent`s; set once by
    /// `Application::new`.
    pub event_sender: Option<calloop::channel::Sender<CustomEvent>>,

    pub slots: Box<WidgetSlots>,
    pub positions: Vec<(f32, f32, f32, f32)>,
    pub splitter_layout: cce_ui::layout::SplitterLayout,
    /// The add-node palette's cce-cloud popup (single active popup, toggle
    /// semantics — the status bar's tracker pattern).
    pub cloud_popups: cce_ui::process::CloudPopupTracker,

    pub drag_widget: Option<usize>,
    /// Where the pointer pressed when `drag_widget` armed — the drag
    /// edge-panning gate: a bare click (press ~ release with jitter) must not
    /// slide the graph under the armed node drag, or the commit re-derives the
    /// node's cell against the panned origin and it teleports. Cleared (latch
    /// open) once the pointer strays a real-drag distance from the press.
    drag_press_cursor: Option<(f32, f32)>,
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
    pub gap_row_h: f32,
    pub gap_col_w: f32,

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
    pub show_playbar: bool,
    pub is_scrolling_trackpad: bool,
    pub last_scroll_time: Instant,
    pub scroll_accum_x: f32,
    pub scroll_accum_y: f32,
    pub last_spreadsheet_node_name: Option<String>,
    pub last_spreadsheet_node_params: Option<Vec<(String, String)>>,
    pub grid_thickness: f32,
    pub focused_pane: usize,
    pub graph_scroll_speed: f32,
    pub graph_inertial_scroll: bool,
    pub graph_scroll_friction: f32,
    pub last_config_read: Instant,
    pub circular_network_pane: bool,
    pub circular_network_layout: cce_ui::layout::CircularPaneLayout,
    pub is_detached_network: bool,
    pub detached_circular_network: bool,
    pub last_project_mod_time: Option<std::time::SystemTime>,
    pub last_project_check: std::time::Instant,
    pub needs_autosave: bool,
    pub last_autosave_time: std::time::Instant,
    pub active_menu_cloud_pid: Option<u32>,
    pub active_menu_cloud_idx: Option<(usize, usize)>,
    pub uniform_background: bool,
    pub network_opacity: f32,
    /// Node-domain opacity (style.surface.graph.node.opacity) — independent of
    /// the pane's network_opacity; fades node bodies/wires/ports and node text.
    pub node_opacity: f32,
    pub last_design_mod_time: Option<std::time::SystemTime>,
    pub last_config_mod_time: Option<std::time::SystemTime>,
    pub floating_network_layout: (f32, f32, f32, f32),
    /// The active app-mode drag (pane edge resize), if any. See [`AppDrag`].
    pub app_drag: Option<AppDrag>,
    pub floating_param_width: f32,
    pub floating_spreadsheet_height: f32,
    pub loaded_project_path: Option<std::path::PathBuf>,
    pub last_saved_root_json: String,
    pub recent_files: Vec<std::path::PathBuf>,
    pub viewport_dirty: bool,
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
    /// Wireframe display of the node geometry (the Render utility node's
    /// "Show Wireframe" toggle).
    pub wireframe: bool,
    pub last_viewport_wireframe: bool,
    /// Draw the wireframe OVER the shaded geometry (the Render node's
    /// "Wireframe Overlay" toggle) — edges visualized without giving up the
    /// filled primitives. Wins over `wireframe` when both are set.
    pub wireframe_overlay: bool,
    pub last_viewport_wireframe_overlay: bool,
    pub last_viewport_rt_mode: bool,
    /// Sphere-geometry cache for the path tracer (a copy of the last
    /// `rebuild_scene_geometry` output, so entering RT mode never re-runs
    /// the node graph / OpenCL kernels).
    pub rt_sphere_verts: Vec<Vertex3D>,
    /// Bumped by `rebuild_scene_geometry`; part of the RT-scene cache key.
    pub rt_geometry_version: u64,
    /// (show_cube, rt_geometry_version) the RT scene was last built from.
    pub last_rt_scene_key: Option<(bool, u64)>,
    pub ui_context: cce_ui::context::UiContext,
}

impl State {
    pub fn viewport(&self) -> &Viewport3D {
        self.slots.viewport
            .as_any()
            .downcast_ref::<Viewport3D>()
            .expect("VIEWPORT_IDX must be a Viewport3D")
    }

    pub fn viewport_mut(&mut self) -> &mut Viewport3D {
        self.slots.viewport
            .as_any_mut()
            .downcast_mut::<Viewport3D>()
            .expect("VIEWPORT_IDX must be a Viewport3D")
    }

    /// Roster index of the slot at `target_addr` (a thin widget address — the comparison
    /// never dereferences; callers pass `ptr as *const ()`).
    pub fn find_widget_index(&self, target_addr: *const ()) -> Option<usize> {
        (0..WIDGET_COUNT).position(|i| {
            let w_ptr = self.slots.get_dyn(i) as *const dyn WidgetHost as *const ();
            w_ptr == target_addr
        })
    }

    pub fn has_any_open_menu(&self, idx: usize) -> bool {
        let mut visited = vec![false; WIDGET_COUNT];
        self.has_any_open_menu_impl(idx, &mut visited)
    }

    pub fn has_any_open_menu_impl(&self, idx: usize, visited: &mut [bool]) -> bool {
        if idx >= WIDGET_COUNT {
            return false;
        }
        if visited[idx] {
            return false;
        }
        visited[idx] = true;
        // Concrete roster typing (Phase 6aw): the only menu-capable roster entries are the
        // Adapted<MenuBar> bars — WidgetHost's capability-discovery hooks are gone.
        if let Some(mb) = self.menubar_at(idx) {
            if mb.is_menu_open() {
                return true;
            }
        }
        let child_ptrs = self.ui_context.tree.children_ptrs(self.slots.get_dyn(idx).base().id());
        for child_ptr in child_ptrs {
            if let Some(child_idx) = self.find_widget_index(child_ptr as *const ()) {
                if self.has_any_open_menu_impl(child_idx, visited) {
                    return true;
                }
            }
        }
        false
    }

    /// The menu-capable roster entries are exactly the `Adapted<MenuBar>` bars (Phase 6aw
    /// concrete typing); `None` for everything else.
    pub fn menubar_at(&self, idx: usize) -> Option<&MenuBar> {
        // Adapted::as_any exposes the INNER widget, so the downcast targets MenuBar itself.
        self.slots.get_dyn(idx).as_any().downcast_ref::<MenuBar>()
    }


    // Roster accessors on CONCRETE types (Phase 6aw, controller decision option 2): each
    // index's type is known statically, so the capability traits are reached by downcast +
    // Deref instead of WidgetHost's deleted as_*_controller discovery hooks. Signatures keep
    // returning the narrow trait objects so the ~40 call sites stay unchanged. The dynamic
    // `idx` of menu()/menu_mut() only ever receives the five menubar indexes.
    pub fn menu(&self, idx: usize) -> &dyn cce_ui::widget::MenuController {
        self.slots.get_dyn(idx).as_any().downcast_ref::<MenuBar>().expect("not a MenuBar")
    }

    pub fn menu_mut(&mut self, idx: usize) -> &mut dyn cce_ui::widget::MenuController {
        self.slots.get_dyn_mut(idx).as_any_mut().downcast_mut::<MenuBar>().expect("not a MenuBar")
    }

    pub fn graph(&self) -> &dyn cce_ui::widget::GraphController {
        self.slots.content.as_any().downcast_ref::<Graph>().expect("CONTENT_IDX must be a Graph")
    }

    pub fn graph_mut(&mut self) -> &mut dyn cce_ui::widget::GraphController {
        self.slots.content.as_any_mut().downcast_mut::<Graph>().expect("CONTENT_IDX must be a Graph")
    }

    pub fn param(&self) -> &dyn cce_ui::widget::ParamController {
        self.slots.param.as_any().downcast_ref::<ParametersBg>().expect("PARAM_IDX must be a ParametersBg")
    }

    pub fn param_mut(&mut self) -> &mut dyn cce_ui::widget::ParamController {
        self.slots.param.as_any_mut().downcast_mut::<ParametersBg>().expect("PARAM_IDX must be a ParametersBg")
    }

    pub fn spreadsheet_mut(&mut self) -> &mut dyn cce_ui::widget::SpreadsheetController {
        self.slots.spreadsheet.as_any_mut().downcast_mut::<Spreadsheet>().expect("SPREADSHEET_IDX must be a Spreadsheet")
    }

    pub fn path_mut(&mut self) -> &mut dyn cce_ui::widget::PathController {
        self.slots.breadcrumb.as_any_mut().downcast_mut::<Breadcrumb>().expect("BREADCRUMB_IDX must be a Breadcrumb")
    }

    pub fn has_unsaved_changes(&self) -> bool {
        if let Ok(current_json) = serde_json::to_string(&self.fs_root) {
            current_json != self.last_saved_root_json
        } else {
            false
        }
    }





    pub fn update_recent_files_layout(&mut self) {
        let mut opts = vec!["- Select -".to_string()];
        for path in &self.recent_files {
            opts.push(path.to_string_lossy().to_string());
        }
        opts.push("Other".to_string());

        if let Some(main_node) = self.fs_root.children.iter_mut().find(|c| c.name == "Main") {
            if let Some(p) = main_node.params.iter_mut().find(|p| p.name == "Open") {
                p.options = opts;
                if !p.options.contains(&p.default) {
                    p.default = "- Select -".to_string();
                }
            }
        }
        self.sync_parameters_pane();
    }

    pub fn save_settings(&mut self) {
        let settings = DesignSettings {
            viewport: ViewportSettings {
                bg_color: self.viewport().bg_color,
                square: self.square_viewport,
                show_camera_pivot_enabled: self.viewport().show_camera_pivot,
                camera_pivot_size: self.camera_pivot_size,
                show_grid_enabled: self.viewport().show_grid,
                show_cube_enabled: self.viewport().show_cube,
                show_origin_enabled: self.viewport().show_origin,
                origin_size: self.origin_size,
                grid_thickness: self.grid_thickness,
                grid_color: self.viewport().grid_color,
            },
        };
        settings.save();
        self.last_design_mod_time = {
            let design_path = DesignSettings::file_path();
            std::fs::metadata(&design_path).and_then(|m| m.modified()).ok()
        };
    }



    // The engine owns the renderer, so geometry changes stage CPU-side here
    // and flush to the GPU meshes in `stage_renderer`.

    pub fn update_grid_geometry(&mut self) {
        let linear_grid_color = cce_ui::colors::to_linear_rgb(self.grid_color);
        self.pending_grid = Some(grid_vertices(self.grid_thickness, linear_grid_color));
        self.viewport_dirty = true;
    }

    pub fn update_origin_geometry(&mut self) {
        self.pending_origin = Some(origin_vectors_vertices(self.origin_size));
        self.viewport_dirty = true;
    }

    pub fn update_pivot_geometry(&mut self) {
        self.pending_pivot = Some(camera_pivot_vertices(self.camera_pivot_size));
        self.viewport_dirty = true;
    }

    pub fn update_viewport_bg_geometry(&mut self) {
        let bg_color = cce_ui::colors::to_linear_rgb(self.viewport().bg_color);
        self.pending_viewport_bg = Some(Self::viewport_bg_vertices(bg_color));
        self.viewport_dirty = true;
    }

    pub(crate) fn viewport_bg_vertices(bg_color: [f32; 3]) -> Vec<Vertex3D> {
        vec![
            Vertex3D { position: [-1.0, -1.0, 9.99], color: bg_color }, // Bottom-left
            Vertex3D { position: [ 1.0, -1.0, 9.99], color: bg_color }, // Bottom-right
            Vertex3D { position: [-1.0,  1.0, 9.99], color: bg_color }, // Top-left

            Vertex3D { position: [ 1.0, -1.0, 9.99], color: bg_color }, // Bottom-right
            Vertex3D { position: [ 1.0,  1.0, 9.99], color: bg_color }, // Top-right
            Vertex3D { position: [-1.0,  1.0, 9.99], color: bg_color }, // Top-left
        ]
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

    // --- Pane edge-resize hotspots. Each is the single source of truth for its zone:
    // the press handlers arm the matching `AppDrag` off it, and `pane_resize_cursor`
    // shows the resize cursor over it, so the two can't drift apart.

    /// The floating network pane's edge-resize hotspot at (cx, cy) — only the right
    /// edge resizes. `None` while the pane is circular or hidden.
    pub fn network_resize_edge_at(&self, cx: f32, cy: f32) -> Option<ResizeDirection> {
        if self.circular_network_pane || !self.show_network {
            return None;
        }
        let (fx, fy, fw, fh) = self.floating_network_layout;
        let margin = 8.0_f32;
        let on_right = cx >= fx + fw - margin && cx <= fx + fw + margin && cy >= fy - margin && cy <= fy + fh + margin;
        if on_right {
            Some(ResizeDirection { left: false, right: true, top: false, bottom: false })
        } else {
            None
        }
    }

    /// Whether (cx, cy) is on the parameter pane's left edge-resize hotspot.
    pub fn on_param_resize_edge(&self, cx: f32, cy: f32) -> bool {
        if !self.show_parameters {
            return false;
        }
        let gap = 18.0_f32;
        let param_w = self.floating_param_width;
        let param_x = self.width - gap - param_w;
        let param_y = HEADER_H + gap;
        let param_h = (self.height - HEADER_H - STATUS_H - 2.0 * gap).max(100.0);
        let margin = 8.0_f32;
        cx >= param_x - margin && cx <= param_x + margin && cy >= param_y - margin && cy <= param_y + param_h + margin
    }

    /// The floating spreadsheet pane's rect (same derivation as the layout pass).
    pub fn floating_spreadsheet_rect(&self) -> (f32, f32, f32, f32) {
        let gap = 18.0_f32;
        let fx = gap;
        let (_, _, mut fw, _) = self.floating_network_layout;
        fw = fw.clamp(150.0, (self.width - 2.0 * gap).max(150.0));
        let param_w = self.floating_param_width.clamp(150.0, (self.width - 2.0 * gap).max(150.0));
        let param_x = self.width - gap - param_w;
        let ss_x = if self.show_network { fx + fw + gap } else { gap };
        let ss_w_end = if self.show_parameters { param_x - gap } else { self.width - gap };
        let ss_w = (ss_w_end - ss_x).max(150.0);
        let ss_y_end = self.height - STATUS_H - gap;
        let ss_h = self.floating_spreadsheet_height.clamp(100.0, (ss_y_end - HEADER_H - gap).max(100.0));
        let ss_y = ss_y_end - ss_h;
        (ss_x, ss_y, ss_w, ss_h)
    }

    /// Whether (cx, cy) is on the spreadsheet pane's top edge-resize hotspot.
    pub fn on_spreadsheet_resize_edge(&self, cx: f32, cy: f32) -> bool {
        if !self.show_spreadsheet {
            return false;
        }
        let (ss_x, ss_y, ss_w, _ss_h) = self.floating_spreadsheet_rect();
        let margin = 8.0_f32;
        cx >= ss_x && cx <= ss_x + ss_w && cy >= ss_y - margin && cy <= ss_y + margin
    }

    /// The resize cursor for an active pane-edge drag, or for hovering one of the
    /// hotspots above. `None` otherwise (the engine then falls back to its CSD cursors).
    pub fn pane_resize_cursor(&self, cx: f32, cy: f32) -> Option<cce_ui::engine::CursorIcon> {
        use cce_ui::engine::CursorIcon;
        let dir_cursor = |dir: ResizeDirection| {
            if dir.left || dir.right { CursorIcon::EwResize } else { CursorIcon::NsResize }
        };
        if let Some(drag) = self.app_drag {
            return Some(match drag {
                AppDrag::NetworkResize { dir, .. } => dir_cursor(dir),
                AppDrag::ParamResize { .. } => CursorIcon::EwResize,
                AppDrag::SpreadsheetResize { .. } => CursorIcon::NsResize,
            });
        }
        if let Some(dir) = self.network_resize_edge_at(cx, cy) {
            return Some(dir_cursor(dir));
        }
        if self.on_param_resize_edge(cx, cy) {
            return Some(CursorIcon::EwResize);
        }
        if self.on_spreadsheet_resize_edge(cx, cy) {
            return Some(CursorIcon::NsResize);
        }
        None
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
        let mut file_to_open = None;
        if !self.is_detached_network {
            if let Some(slot_idx) = self.graph().selected_node() {
                let updated_params = self.param().node_params();
                // Live pane state, so a pane toggle only fires the visibility
                // action when it actually flips relative to what's on screen.
                let cur_show = (self.show_network, self.show_viewport, self.show_parameters, self.show_spreadsheet, self.show_playbar);
                let dir = self.current_dir_mut();
                if let Some(child) = dir.children.get_mut(slot_idx) {
                    let mut param_changed = false;
                    let mut triggered_buttons = Vec::new();
                    let mut pane_actions = Vec::new();
                    for (u_name, u_val, _) in &updated_params {
                        // The params pane reports its display key (label when
                        // set, else name), so resolve back to the param by that
                        // key rather than by name alone.
                        if let Some(p) = child.params.iter_mut().find(|p| {
                            let key = if p.label.is_empty() { &p.name } else { &p.label };
                            key == u_name
                        }) {
                            if p.default != *u_val {
                                p.default = u_val.clone();
                                param_changed = true;
                                if p.param_type == "button" && p.default == "clicked" {
                                    triggered_buttons.push(p.name.clone());
                                    p.default = "".to_string();
                                }
                                if p.param_type == "toggle" {
                                    let desired = p.default == "true";
                                    let cur = match p.name.as_str() {
                                        "Show Network Pane" => Some(cur_show.0),
                                        "Show Viewport Pane" => Some(cur_show.1),
                                        "Show Parameters Pane" => Some(cur_show.2),
                                        "Show Spreadsheet Pane" => Some(cur_show.3),
                                        "Show Playbar Pane" => Some(cur_show.4),
                                        _ => None,
                                    };
                                    // execute_menu_action flips the pane, so only
                                    // fire it when the target differs from now.
                                    if cur == Some(!desired) {
                                        pane_actions.push(p.name.clone());
                                    }
                                }
                                if p.name == "Open" && p.default != "- Select -" && !p.default.is_empty() {
                                    file_to_open = Some(p.default.clone());
                                    p.default = "- Select -".to_string();
                                    triggered_buttons.push("Open".to_string());
                                }
                            }
                        }
                    }

                    if !triggered_buttons.is_empty() {
                        let mut disp_params = self.param().node_params();
                        for btn_name in &triggered_buttons {
                            if let Some(pos) = disp_params.iter().position(|p| p.0 == *btn_name) {
                                disp_params[pos].1 = if btn_name == "Open" { "- Select -".to_string() } else { "".to_string() };
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
                            self.execute_menu_action(&btn_name);
                        }
                        for pane_name in pane_actions {
                            self.execute_menu_action(&pane_name);
                        }
                    }
                }


            }
        }
        if let Some(option_text) = file_to_open {
            if option_text == "Other" {
                self.open_file_chooser();
            } else {
                let path = std::path::PathBuf::from(option_text);
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

    /// One label-matched menu action: the button-param menu pane's items, drained
    /// per triggered button by `sync_parameters_to_project`, and reachable directly
    /// through the `menu_action` tool (the index-matched `menu_click` cannot
    /// reach these). Returns false for an unrecognized label.
    pub fn execute_menu_action(&mut self, label: &str) -> bool {
        match label {
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
                let (gx, gy, gw, gh) = configured_grid_geometry();
                self.grid_size_x = gx;
                self.grid_size_y = gy;
                self.gap_col_w = gw;
                self.gap_row_h = gh;
                self.sync_grid_settings();
            }
            "Detach Circular Window" | "Detach Pane" => {
                self.execute_action(Action::DetachCircularWindow);
            }
            "Show Network Pane" => {
                self.show_network = !self.show_network;
                self.slots.content.set_visible(self.show_network);
                self.slots.left_menubar.set_visible(self.show_network);
                self.slots.breadcrumb.set_visible(self.show_network);
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
                self.slots.viewport.set_visible(self.show_viewport);
                self.slots.right_menubar.set_visible(self.show_viewport);
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
                self.slots.param.set_visible(self.show_parameters);
                self.slots.param_menubar.set_visible(self.show_parameters);
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
                self.slots.spreadsheet.set_visible(self.show_spreadsheet);
                self.slots.spreadsheet_menubar.set_visible(self.show_spreadsheet);
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
            "Show Playbar Pane" => {
                self.show_playbar = !self.show_playbar;
                self.slots.playbar.set_visible(self.show_playbar);
                let val = self.show_playbar;
                self.menu_mut(HEADER_IDX).set_item_checked(2, 8, val);
                // Not in the focus-cycle roster (get_next_visible_pane): the
                // playbar is a transport strip with no menubar of its own.
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
                        self.slots.content.set_visible(false);
                        self.slots.left_menubar.set_visible(false);
                        self.slots.breadcrumb.set_visible(false);
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
                        self.slots.viewport.set_visible(false);
                        self.slots.right_menubar.set_visible(false);
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
                        self.slots.param.set_visible(false);
                        self.slots.param_menubar.set_visible(false);
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
                        self.slots.spreadsheet.set_visible(false);
                        self.slots.spreadsheet_menubar.set_visible(false);
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
            _ => return false,
        }
        true
    }

    /// Rewrite the Main node's setting toggles from live app state, so the
    /// switches show the real value even after panes/settings were changed
    /// through the menus or keyboard while another node was selected.
    /// Write a per-camera display toggle (Square Aspect / Show Camera Pivot)
    /// back to the ACTIVE camera node — the setting's home — so the next
    /// settings apply doesn't revert a menu/shortcut flip. No-op under
    /// Default Camera, which has no node: the live value stands alone.
    fn write_active_camera_toggle(&mut self, name: &str, val: bool) {
        if self.active_camera == "Default Camera" {
            return;
        }
        let active = self.active_camera.clone();
        if let Some(cam) = self
            .current_dir_mut()
            .children
            .iter_mut()
            .find(|c| c.node_type == "camera" && c.name == active)
        {
            if let Some(p) = cam.params.iter_mut().find(|p| p.name == name) {
                p.default = if val { "true" } else { "false" }.to_string();
            }
        }
    }

    fn refresh_main_node_live_toggles(&mut self, slot_idx: usize) {
        let live: [(&str, bool); 10] = [
            ("Show Network Pane", self.show_network),
            ("Show Viewport Pane", self.show_viewport),
            ("Show Parameters Pane", self.show_parameters),
            ("Show Spreadsheet Pane", self.show_spreadsheet),
            ("Show Playbar Pane", self.show_playbar),
            ("Circular Pane", self.circular_network_pane),
            ("Show Grid Guide", self.viewport().show_grid),
            ("Show Reference Cube", self.viewport().show_cube),
            ("Show Origin Axes", self.viewport().show_origin),
            ("Ray Traced Preview", self.viewport().rt_mode),
        ];
        let dir = self.current_dir_mut();
        let Some(child) = dir.children.get_mut(slot_idx) else { return };
        if child.name != "Main" { return; }
        for (name, on) in live {
            if let Some(p) = child.params.iter_mut().find(|p| p.name == name && p.param_type == "toggle") {
                p.default = if on { "true" } else { "false" }.to_string();
            }
        }
    }

    pub fn sync_parameters_pane(&mut self) {
        if !self.is_detached_network {
            if let Some(slot_idx) = self.graph().selected_node() {
                self.refresh_main_node_live_toggles(slot_idx);
            }
        }
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

    pub fn open_file_chooser(&self) {
        let Some(sender) = self.event_sender.clone() else { return };
        std::thread::spawn(move || {
            use std::process::{Command, Stdio};

            // Find executable path
            let exe_path = if let Ok(cur_exe) = std::env::current_exe() {
                let sibling = cur_exe.with_file_name("cce-files");
                if sibling.exists() {
                    sibling
                } else if let Ok(home) = std::env::var("HOME") {
                    let path = std::path::PathBuf::from(home)
                        .join(".local")
                        .join("bin")
                        .join("cce-files");
                    if path.exists() {
                        path
                    } else {
                        std::path::PathBuf::from("cce-files")
                    }
                } else {
                    std::path::PathBuf::from("cce-files")
                }
            } else if let Ok(home) = std::env::var("HOME") {
                let path = std::path::PathBuf::from(home)
                    .join(".local")
                    .join("bin")
                    .join("cce-files");
                if path.exists() {
                    path
                } else {
                    std::path::PathBuf::from("cce-files")
                }
            } else {
                std::path::PathBuf::from("cce-files")
            };

            let child = match Command::new(exe_path)
                .arg("--select")
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to spawn cce-files: {:?}", e);
                    return;
                }
            };

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("Failed to wait for cce-files: {:?}", e);
                    return;
                }
            };

            if output.status.success() {
                let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !selected.is_empty() {
                    let _ = sender.send(CustomEvent::RunAction(McpAction::Load { path: selected }));
                }
            }
        });
    }

    pub fn save_file_chooser(&self) {
        let Some(sender) = self.event_sender.clone() else { return };
        std::thread::spawn(move || {
            use std::process::{Command, Stdio};

            // Find executable path
            let exe_path = if let Ok(cur_exe) = std::env::current_exe() {
                let sibling = cur_exe.with_file_name("cce-files");
                if sibling.exists() {
                    sibling
                } else if let Ok(home) = std::env::var("HOME") {
                    let path = std::path::PathBuf::from(home)
                        .join(".local")
                        .join("bin")
                        .join("cce-files");
                    if path.exists() {
                        path
                    } else {
                        std::path::PathBuf::from("cce-files")
                    }
                } else {
                    std::path::PathBuf::from("cce-files")
                }
            } else if let Ok(home) = std::env::var("HOME") {
                let path = std::path::PathBuf::from(home)
                    .join(".local")
                    .join("bin")
                    .join("cce-files");
                if path.exists() {
                    path
                } else {
                    std::path::PathBuf::from("cce-files")
                }
            } else {
                std::path::PathBuf::from("cce-files")
            };

            let child = match Command::new(exe_path)
                .arg("--save")
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to spawn cce-files: {:?}", e);
                    return;
                }
            };

            let output = match child.wait_with_output() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("Failed to wait for cce-files: {:?}", e);
                    return;
                }
            };

            if output.status.success() {
                let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !selected.is_empty() {
                    let _ = sender.send(CustomEvent::RunAction(McpAction::Save { path: selected }));
                }
            }
        });
    }

    /// The add-node palette, as a `cce-cloud --dmenu` popup: toggle-tracked like the
    /// status bar's popups, positioned at the pointer and parented to the
    /// designer surface. The picked template comes back through the event
    /// loop as a fire-and-forget `AddNode` at the grid cursor.
    pub fn open_node_palette(&mut self) {
        const SOURCE: &str = "node-palette";
        if self.cloud_popups.click(SOURCE) == cce_ui::process::CloudPopupClick::ToggledOff {
            return;
        }
        let Some(sender) = self.event_sender.clone() else { return };
        // In a utility dir geometry templates are rejected at placement —
        // don't offer them.
        let in_utility = !self.current_path.is_empty()
            && self.fs_root.children[self.current_path[0]].node_type == "utility";
        let items: String = self
            .node_templates
            .iter()
            .filter(|t| !in_utility || !crate::geometry::is_geometry_node_type(&t.node.node_type))
            .map(|t| t.label.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let (px, py) = (self.cursor_x as i32, self.cursor_y as i32);
        let (gx, gy) = (self.grid_cursor_col as f32, self.grid_cursor_row as f32);
        std::thread::spawn(move || {
            let popup = cce_ui::process::CloudPopup::at(px, py).parent_app_id("cce-designer");
            let mut spawned_pid = 0;
            let result = popup.run_dmenu("Add Node:", &items, |pid| {
                spawned_pid = pid;
                let _ = sender.send(CustomEvent::CloudSpawned { pid, source: SOURCE.to_string() });
            });
            if let Ok(Some(selected)) = &result {
                if !selected.is_empty() {
                    let _ = sender.send(CustomEvent::RunAction(McpAction::AddNode {
                        template_name: selected.clone(),
                        name: None,
                        x: gx,
                        y: gy,
                    }));
                }
            }
            let _ = sender.send(CustomEvent::CloudClosed { pid: spawned_pid, source: SOURCE.to_string() });
        });
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
        self.app_drag = None;
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











    pub fn new(is_detached_network: bool) -> Self {
        // The engine detected the output scale before constructing the app.
        let scale = cce_ui::scale::scale_factor() as f64;
        let settings = DesignSettings::load();
        // Grid geometry is config-owned, not part of the saved state.
        let cfg_grid = configured_grid_geometry();
        let (lw, lh) = if is_detached_network {
            (400.0f32, 400.0f32)
        } else {
            (1280.0f32, 800.0f32)
        };
        let pw = (lw as f64 * scale) as u32;
        let ph = (lh as f64 * scale) as u32;
        let sw = lw;

        // Bundled fonts only (the designer's UI uses bundled families).

        let splitter_layout = cce_ui::layout::SplitterLayout::new(sw, SPLITTER_W, MIN_COLUMN);
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
        let mut slots = Box::new(WidgetSlots {
            header: MenuBar::new(0.0, 0.0, 0.0, HEADER_H).with_title("Designer").with_label("Main Menu Bar").with_item("File", &["New Project", "Save", "Save As", "Exit"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Reset Zoom", "Detach Circular Window", "Show Network Pane", "Show Viewport Pane", "Show Parameters Pane", "Show Spreadsheet Pane", "Show Playbar Pane"]).with_item("Help", &["About"]).with_z_index(110).with_context_options(context_opts.clone(), 4),
            content: Graph::new(),
            splitter1: Splitter::new(SPLITTER_W),
            viewport: Viewport3D::new(),
            splitter2: Splitter::new(SPLITTER_W),
            param: ParametersBg::new(),
            canvas: Canvas::new(),
            left_menubar: MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("0: Network").with_label("Network Menu Bar").with_item("File", &["New", "Save", "Save As"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Circular Pane", "Detach Pane", "Close Pane"]).with_context_options(context_opts.clone(), 0),
            right_menubar: MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("1: Viewport").with_label("Viewport Menu Bar").with_item("Camera", &["Perspective", "Orthographic"]).with_item("Display", &["Square Aspect"]).with_item("Guides", &["Show Grid", "Cube", "Origin", "Camera Pivot"]).with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 1),
            param_menubar: MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("2: Parameters").with_label("Parameters Menu Bar").with_item("Preset", &["Default", "Custom"]).with_item("Reset", &["All"]).with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 2),
            status: StatusBar::new().with_text("Ready"),
            breadcrumb: Breadcrumb::new(),
            spreadsheet: Spreadsheet::new(),
            spreadsheet_menubar: {
                let mut mb = MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("3: Spreadsheet").with_label("Spreadsheet Menu Bar").with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 3);
                mb.set_visible(false);
                mb
            },
            network_panel: PassivePlate::new([0.10, 0.10, 0.13, 0.95], false),
            playbar: {
                let mut pb = Playbar::new();
                pb.set_visible(false);
                pb
            },
        });

        if let Some(viewport) = slots.viewport.as_any_mut().downcast_mut::<Viewport3D>() {
            viewport.show_grid = settings.viewport.show_grid_enabled;
            viewport.show_cube = settings.viewport.show_cube_enabled;
            viewport.show_origin = settings.viewport.show_origin_enabled;
            viewport.show_camera_pivot = settings.viewport.show_camera_pivot_enabled;
            viewport.bg_color = settings.viewport.bg_color;
            viewport.grid_color = settings.viewport.grid_color;
            viewport.active_camera = active_camera.clone();
        }

        let recent_files = Self::load_recent_files();

        let mut positions = Vec::with_capacity(WIDGET_COUNT);
        positions.resize_with(WIDGET_COUNT, || (0.0, 0.0, 0.0, 0.0));


        // Chords from input.kdl (`cce-designer` domain → `cce-ui` domain),
        // defaulting to the historical bindings; an invalid user chord logs
        // and falls back to the default instead of panicking.
        let mut shortcut_manager = ShortcutManager::new();
        {
            let mut register = |name: &str, default: &str, action: Action| {
                let chord = cce_ui::input::app_chord(name, default);
                if shortcut_manager.register(&chord, action).is_err() {
                    eprintln!("[cce-designer] invalid chord {:?} for {}; using {:?}", chord, name, default);
                    let _ = shortcut_manager.register(default, action);
                }
            };
            register("toggle_grid", "Ctrl+g", Action::ToggleGrid);
            register("toggle_cube", "Ctrl+e", Action::ToggleCube);
            register("toggle_square_viewport", "Ctrl+a", Action::ToggleSquareViewport);
            register("toggle_configure", "Ctrl+,", Action::ToggleConfigure);
            register("toggle_spreadsheet", "`", Action::ToggleSpreadsheet);
            register("toggle_circular_pane", "Ctrl+d", Action::ToggleCircularPane);
            register("save_document", "Ctrl+s", Action::Save);
            register("next_context", "Ctrl+Tab", Action::NextContext);
            register("previous_context", "Ctrl+Shift+Tab", Action::PrevContext);
        }

        let mut state = Self {
            title: String::new(),
            meshes: None,
            pending_grid: None,
            pending_origin: None,
            pending_pivot: None,
            pending_viewport_bg: None,
            spheres_dirty: false,
            last_corner_radius: -1.0,
            pending_window_drag: None,
            window_action: None,
            vertex_count_spheres: 0,
            node_color: cce_ui::color::graph_node_color(),
            grid_color: settings.viewport.grid_color,
            cell_color: cce_ui::color::graph_cell_color(),
            gap_color: cce_ui::color::graph_gap_color(),
            origin_size: settings.viewport.origin_size,
            camera_pivot_size: settings.viewport.camera_pivot_size,
            fs_root: fs_root.clone(),
            node_templates,
            current_path,
            node_clipboard: None,
            last_click: None,
            last_frame: Instant::now(),
            shortcut_manager,
            pending_action: None,
            exit_requested: false,
            event_sender: None,
            slots,
            positions,
            splitter_layout,
            cloud_popups: cce_ui::process::CloudPopupTracker::new(),
            drag_widget: None,
            drag_press_cursor: None,
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
            square_viewport: settings.viewport.square,
            grid_snap_enabled: true,
            network_grid_visible: true,
            grid_size_x: cfg_grid.0,
            grid_size_y: cfg_grid.1,
            gap_col_w: cfg_grid.2,
            gap_row_h: cfg_grid.3,
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
            show_playbar: false,
            is_scrolling_trackpad: false,
            last_scroll_time: Instant::now(),
            scroll_accum_x: 0.0,
            scroll_accum_y: 0.0,
            last_spreadsheet_node_name: None,
            last_spreadsheet_node_params: None,
            grid_thickness: settings.viewport.grid_thickness,
            focused_pane: LEFT_MENUBAR_IDX,
            // Config-owned; update_inertial_settings overwrites these from
            // config.kdl's input.inertial right after construction.
            graph_scroll_speed: 1.0,
            graph_inertial_scroll: true,
            graph_scroll_friction: 0.90,
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
            needs_autosave: false,
            last_autosave_time: std::time::Instant::now(),
            active_menu_cloud_pid: None,
            active_menu_cloud_idx: None,
            uniform_background: false,
            network_opacity: 0.95,
            node_opacity: 1.0,
            last_design_mod_time: {
                let design_path = DesignSettings::file_path();
                std::fs::metadata(&design_path).and_then(|m| m.modified()).ok()
            },
            last_config_mod_time: {
                let paths = [
                    cce_ui::config::cce_config_dir().join("config.toml"),
                    cce_ui::config::config_home().join("ccec").join("config.toml"),
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
            app_drag: None,
            floating_param_width: 300.0,
            floating_spreadsheet_height: 250.0,
            loaded_project_path: None,
            last_saved_root_json: serde_json::to_string(&fs_root).unwrap_or_default(),
            recent_files,
            viewport_dirty: true,
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
            wireframe: false,
            last_viewport_wireframe: false,
            wireframe_overlay: false,
            last_viewport_wireframe_overlay: false,
            last_viewport_rt_mode: false,
            rt_sphere_verts: Vec::new(),
            rt_geometry_version: 0,
            last_rt_scene_key: None,
            ui_context: cce_ui::context::UiContext::new(),
        };

        state.update_inertial_settings();
        state.update_graph_settings_from_config();
        state.update_window_title();
        colors::set_node_color(state.node_color);
        state.ensure_menubar_subnets();
        state.apply_settings_from_menubar_subnets();
        state.sync_nodes();
        state.rebuild_scene_geometry();
        state.sync_grid_settings();
        let sg = state.viewport().show_grid;
        let sc = state.viewport().show_cube;
        let so = state.viewport().show_origin;
        let cp = state.viewport().show_camera_pivot;
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
        state.sync_cursor_and_selection();
        state.sync_parameters_pane();
        for i in 0..WIDGET_COUNT {
            let w = state.slots.get_dyn_mut(i);
            let id = w.base().id();
            let ptr = w as *mut (dyn WidgetHost + 'static);
            state.ui_context.register_widget(id, ptr);
        }
        state
    }

    pub fn sync_grid_settings(&mut self) {
        let active_node_area_y = self.positions[CONTENT_IDX].1;
        let active_node_area_x = self.positions[CONTENT_IDX].0;

        let network_grid_visible = self.network_grid_visible;
        let grid_size_x = self.grid_size_x;
        let grid_size_y = self.grid_size_y;
        let gap_row_h = self.gap_row_h;
        let gap_col_w = self.gap_col_w;
        let pan_x = self.pan_x;
        let pan_y = self.pan_y;
        let grid_snap_enabled = self.grid_snap_enabled;
        let graph = self.graph_mut();
        graph.set_show_network_grid(network_grid_visible);
        graph.set_grid_sizes(grid_size_x, grid_size_y);
        graph.set_skipped_sizes(gap_row_h, gap_col_w);
        graph.set_grid_origin(active_node_area_x + pan_x, active_node_area_y + pan_y);
        graph.set_grid_snap_enabled(grid_snap_enabled);
        if let Some(graph) = self.slots.content.as_any_mut().downcast_mut::<cce_ui::widget::Graph>() {
            graph.set_uniform_background(self.uniform_background);
            graph.set_network_opacity(self.network_opacity);
            graph.set_node_opacity(self.node_opacity);
            graph.set_cell_color(self.cell_color);
            graph.set_gap_color(self.gap_color);
        }
        if let Some(menubar) = self.slots.left_menubar.as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
            menubar.set_network_opacity(self.network_opacity);
        }
        if let Some(breadcrumb) = self.slots.breadcrumb.as_any_mut().downcast_mut::<cce_ui::widget::Breadcrumb>() {
            breadcrumb.set_network_opacity(self.network_opacity);
        }
        if let Some(plate) = self.slots.network_panel.as_any_mut().downcast_mut::<PassivePlate>() {
            plate.set_network_opacity(self.network_opacity);
        }
    }

    pub fn update_inertial_settings(&mut self) {
        self.last_config_read = Instant::now();
        let config_path = cce_ui::config::get_config_path();

        let mut enabled = true;
        let mut friction = 0.90;
        let mut speed = 1.0;

        if let Ok(content) = std::fs::read_to_string(&config_path) {
            let val = cce_ui::config::parse_kdl_to_json(&content);
            if let Some(input) = val.get("input") {
                if let Some(inertial) = input.get("inertial") {
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
        
        // Scroll behavior is config-owned (input.inertial in config.kdl) —
        // state.kdl carries no copy, so config edits always take effect.
        self.graph_scroll_speed = speed;
        self.graph_inertial_scroll = enabled;
        self.graph_scroll_friction = friction;
        self.viewport_mut().scroll_speed = speed;
        self.viewport_mut().inertial_scroll = enabled;
        self.viewport_mut().scroll_friction = friction;
    }

    pub fn update_graph_settings_from_config(&mut self) {
        cce_ui::layout::reload_config();
        
        let opacity = cce_ui::color::graph_opacity();
        let node_opacity = cce_ui::color::graph_node_opacity();
        let cell_color = cce_ui::color::graph_cell_color();
        let gap_color = cce_ui::color::graph_gap_color();
        let snap_enabled = cce_ui::layout::graph_grid_snap();
        let node_color = cce_ui::color::graph_node_color();

        let mut changed = false;
        
        if (self.network_opacity - opacity).abs() > 0.001 {
            self.network_opacity = opacity;
            changed = true;
        }
        if (self.node_opacity - node_opacity).abs() > 0.001 {
            self.node_opacity = node_opacity;
            changed = true;
        }
        if self.cell_color != cell_color {
            self.cell_color = cell_color;
            changed = true;
        }
        if self.gap_color != gap_color {
            self.gap_color = gap_color;
            changed = true;
        }
        if self.node_color != node_color {
            self.node_color = node_color;
            colors::set_node_color(node_color);
            changed = true;
        }
        if self.grid_snap_enabled != snap_enabled {
            self.grid_snap_enabled = snap_enabled;
            changed = true;
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

        let old_row_h = self.gap_row_h;
        let old_col_w = self.gap_col_w;

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
        self.gap_row_h = old_row_h * (new_gy / old_gy);
        self.gap_col_w = old_col_w * (new_gx / old_gx);

        self.sync_grid_settings();
    }


    pub fn keep_cursor_in_view(&mut self) {
        let (px, py, pw, ph) = self.positions[CONTENT_IDX];

        let cx = px + self.grid_cursor_col as f32 * (self.grid_size_x + self.gap_col_w) + self.pan_x;
        let cy = py + self.grid_cursor_row as f32 * (self.grid_size_y + self.gap_row_h) + self.pan_y;
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
            if let Some(menubar) = self.slots.left_menubar.as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
                menubar.set_curved_circle(None);
            }
            self.positions[BREADCRUMB_IDX] = (cx - r, cy - r + 45.0, 2.0 * r, BREADCRUMB_H);
            self.positions[CONTENT_IDX] = (cx - r, cy - r + 45.0 + BREADCRUMB_H, 2.0 * r, 2.0 * r - (45.0 + BREADCRUMB_H));
            self.positions[NETWORK_PANEL_IDX] = (cx - r, cy - r, 2.0 * r, 2.0 * r);
            self.slots.network_panel.set_rect(cx - r, cy - r, 2.0 * r, 2.0 * r);
            if let Some(plate) = self.slots.network_panel.as_any_mut().downcast_mut::<PassivePlate>() {
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
            self.positions[PLAYBAR_IDX] = (0.0, 0.0, 0.0, 0.0);

            self.slots.header.set_visible(false);
            self.slots.status.set_visible(false);
            self.slots.playbar.set_visible(false);
            self.slots.content.set_visible(true);
            self.slots.network_panel.set_visible(true);
            self.slots.left_menubar.set_visible(false);
            self.slots.breadcrumb.set_visible(true);
            self.slots.splitter1.set_visible(false);
            self.slots.splitter2.set_visible(false);
            self.slots.viewport.set_visible(false);
            self.slots.right_menubar.set_visible(false);
            self.slots.param.set_visible(false);
            self.slots.param_menubar.set_visible(false);
            self.slots.spreadsheet.set_visible(false);
            self.slots.spreadsheet_menubar.set_visible(false);
        } else if self.detached_circular_network {
            // Parent process: Network pane is detached (hidden from main window)
            let pb_h = if self.show_playbar { PLAYBAR_H } else { 0.0 };
            let body_h = body_h - pb_h;
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
            self.slots.network_panel.set_rect(0.0, 0.0, 0.0, 0.0);
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
            self.positions[PLAYBAR_IDX] = (0.0, HEADER_H + body_h, self.width, pb_h);

            self.slots.header.set_visible(true);
            self.slots.status.set_visible(false);
            self.slots.content.set_visible(false);
            self.slots.network_panel.set_visible(false);
            self.slots.left_menubar.set_visible(false);
            self.slots.breadcrumb.set_visible(false);
            self.slots.splitter1.set_visible(false);
            self.slots.splitter2.set_visible(s2_w > 0.0);
            self.slots.viewport.set_visible(viewport_visible);
            self.slots.right_menubar.set_visible(viewport_visible);
            self.slots.param.set_visible(right_visible);
            self.slots.param_menubar.set_visible(right_visible);
            self.slots.spreadsheet.set_visible(spreadsheet_visible);
            self.slots.spreadsheet_menubar.set_visible(spreadsheet_visible);
            self.slots.playbar.set_visible(self.show_playbar);
        } else {
            let paginator_w = 0.0;
            let body_h = self.height - STATUS_H;
            self.positions[0] = (0.0, 0.0, 0.0, 0.0);
            if self.circular_network_pane {
                let pb_h = if self.show_playbar { PLAYBAR_H } else { 0.0 };
                let body_h = body_h - pb_h;
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

                let gap = 18.0;
                let max_r = ((self.width - paginator_w - 2.0 * gap).min(body_h - 2.0 * gap) / 2.0).max(50.0);
                self.circular_network_layout.r = self.circular_network_layout.r.clamp(50.0, max_r);

                let r = self.circular_network_layout.r;
                let min_x = paginator_w + gap + r;
                let max_x = (self.width - gap - r).max(min_x);
                self.circular_network_layout.x = self.circular_network_layout.x.clamp(min_x, max_x);

                let min_y = gap + r;
                let max_y = (body_h - gap - r).max(min_y);
                self.circular_network_layout.y = self.circular_network_layout.y.clamp(min_y, max_y);

                let cx = self.circular_network_layout.x;
                let cy = self.circular_network_layout.y;

                self.positions[LEFT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                if let Some(menubar) = self.slots.left_menubar.as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
                    menubar.set_curved_circle(None);
                }
                self.positions[BREADCRUMB_IDX] = (cx - r, cy - r + 45.0, 2.0 * r, BREADCRUMB_H);
                self.positions[CONTENT_IDX] = (cx - r, cy - r + 45.0 + BREADCRUMB_H, 2.0 * r, 2.0 * r - (45.0 + BREADCRUMB_H));
                self.positions[NETWORK_PANEL_IDX] = (cx - r, cy - r, 2.0 * r, 2.0 * r);
                self.slots.network_panel.set_rect(cx - r, cy - r, 2.0 * r, 2.0 * r);
                if let Some(plate) = self.slots.network_panel.as_any_mut().downcast_mut::<PassivePlate>() {
                    plate.set_curved_circle(Some((cx, cy, r)));
                }
                self.slots.network_panel.set_drag_bounds(0.0, 0.0, self.width, self.height);
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
                self.positions[PLAYBAR_IDX] = (0.0, body_h, self.width, pb_h);

                self.slots.header.set_visible(false);
                self.slots.status.set_visible(false);
                self.slots.content.set_visible(self.show_network);
                self.slots.network_panel.set_visible(self.show_network);
                self.slots.left_menubar.set_visible(false);
                self.slots.breadcrumb.set_visible(self.show_network);
                self.slots.splitter1.set_visible(false);
                self.slots.splitter2.set_visible(s2_w > 0.0);
                self.slots.viewport.set_visible(viewport_visible);
                self.slots.right_menubar.set_visible(false);
                self.slots.param.set_visible(right_visible);
                self.slots.param_menubar.set_visible(false);
                self.slots.spreadsheet.set_visible(spreadsheet_visible);
                self.slots.spreadsheet_menubar.set_visible(false);
                self.slots.playbar.set_visible(self.show_playbar);
            } else {
                let gap = 18.0_f32;
                let pb_off = if self.show_playbar { PLAYBAR_H + gap } else { 0.0 };
                let (mut _fx, mut _fy, mut fw, mut _fh) = self.floating_network_layout;
                fw = fw.clamp(150.0, (self.width - paginator_w - 2.0 * gap).max(150.0));
                let fx = paginator_w + gap;
                let fy = gap;
                let fh = (self.height - STATUS_H - pb_off - 2.0 * gap).max(100.0);
                self.floating_network_layout = (fx, fy, fw, fh);

                let param_w = self.floating_param_width.clamp(150.0, (self.width - paginator_w - 2.0 * gap).max(150.0));
                self.floating_param_width = param_w;
                let param_x = self.width - gap - param_w;
                let param_y = gap;
                let param_h = (self.height - STATUS_H - pb_off - 2.0 * gap).max(100.0);

                let ss_x = if self.show_network { fx + fw + gap } else { paginator_w + gap };
                let ss_w_end = if self.show_parameters { param_x - gap } else { self.width - gap };
                let ss_w = (ss_w_end - ss_x).max(150.0);
                let ss_y_end = self.height - STATUS_H - pb_off - gap;
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

                if let Some(menubar) = self.slots.left_menubar.as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
                    menubar.set_curved_circle(None);
                }
                
                let mb_h = 0.0;
                let bc_h = if self.show_network { BREADCRUMB_H } else { 0.0 };
                let content_h = (ph - mb_h - bc_h).max(0.0);

                self.positions[CONTENT_IDX] = (px, py + mb_h + bc_h, pw, content_h);
                self.positions[NETWORK_PANEL_IDX] = (px, py, pw, ph);
                self.slots.network_panel.set_rect(px, py, pw, ph);
                if let Some(plate) = self.slots.network_panel.as_any_mut().downcast_mut::<PassivePlate>() {
                    plate.set_curved_circle(None);
                }
                self.positions[SPLITTER1_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPLITTER2_IDX] = (0.0, 0.0, 0.0, 0.0);
                let p_rect = if self.show_parameters { (param_x, param_y, param_w, param_h) } else { (0.0, 0.0, 0.0, 0.0) };
                self.positions[PARAM_IDX] = p_rect;
                self.slots.param.set_rect(p_rect.0, p_rect.1, p_rect.2, p_rect.3);

                self.positions[VIEWPORT_IDX] = (col_c_x, vp_y, col_c_w, vp_h);
                self.positions[RIGHT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[BREADCRUMB_IDX] = (px, py + mb_h, pw, bc_h);
                self.positions[LEFT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);

                self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPREADSHEET_IDX] = if spreadsheet_visible { (ss_x, ss_y, ss_w, ss_h) } else { (0.0, 0.0, 0.0, 0.0) };
                self.slots.spreadsheet.set_rect(
                    self.positions[SPREADSHEET_IDX].0,
                    self.positions[SPREADSHEET_IDX].1,
                    self.positions[SPREADSHEET_IDX].2,
                    self.positions[SPREADSHEET_IDX].3,
                );

                self.positions[CANVAS_IDX] = (0.0, 0.0, self.width, body_h);
                self.positions[PARAM_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[STATUS_IDX] = (0.0, self.height - STATUS_H, self.width, STATUS_H);
                self.positions[PLAYBAR_IDX] = if self.show_playbar {
                    (gap, self.height - STATUS_H - gap - PLAYBAR_H, self.width - 2.0 * gap, PLAYBAR_H)
                } else {
                    (0.0, 0.0, 0.0, 0.0)
                };

                self.slots.header.set_visible(false);
                self.slots.status.set_visible(false);
                self.slots.playbar.set_visible(self.show_playbar);
                self.slots.content.set_visible(self.show_network);
                self.slots.network_panel.set_visible(self.show_network);
                self.slots.left_menubar.set_visible(false);
                self.slots.breadcrumb.set_visible(self.show_network);
                self.slots.splitter1.set_visible(false);
                self.slots.splitter2.set_visible(false);
                self.slots.viewport.set_visible(viewport_visible);
                self.slots.right_menubar.set_visible(false);
                self.slots.param.set_visible(right_visible);
                self.slots.param_menubar.set_visible(false);
                self.slots.spreadsheet.set_visible(spreadsheet_visible);
                self.slots.spreadsheet_menubar.set_visible(false);
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
                self.slots.get_dyn_mut(idx).set_visible(false);
            }
        }
    }


    pub fn apply_layout(&mut self) {
        for (i, pos) in self.positions.iter().enumerate() {
            if i < WIDGET_COUNT {
                if self.slots.is_dragging(i) { continue; }
                let widget = self.slots.get_dyn_mut(i);
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
        // 6bd value shrink: `set_selected` left `WidgetHost` — the five menubar slots are
        // concrete `Adapted<MenuBar>` fields, selected-state sync goes to them directly.
        let f = self.focused_pane;
        self.slots.header.set_selected(HEADER_IDX == f);
        self.slots.left_menubar.set_selected(LEFT_MENUBAR_IDX == f);
        self.slots.right_menubar.set_selected(RIGHT_MENUBAR_IDX == f);
        self.slots.param_menubar.set_selected(PARAM_MENUBAR_IDX == f);
        self.slots.spreadsheet_menubar.set_selected(SPREADSHEET_MENUBAR_IDX == f);
        if self.focused_pane != PARAM_MENUBAR_IDX {
            self.slots.param.unfocus();
            self.sync_parameters_to_project();
        }
        self.sync_context_dropdowns();
    }

    pub fn sync_layout(&mut self) {
        let left_visible = self.show_network && !self.circular_network_pane && !self.is_detached_network && !self.detached_circular_network;
        let center_visible = self.show_viewport || self.show_spreadsheet;
        let right_visible = self.show_parameters;

        if left_visible && center_visible && self.slots.splitter1.visible() && self.slots.splitter1.rect().2 > 0.0 {
            self.splitter_layout.splitter1_x = self.slots.splitter1.rect().0;
        }
        if right_visible && (center_visible || left_visible) && self.slots.splitter2.visible() && self.slots.splitter2.rect().2 > 0.0 {
            self.splitter_layout.splitter2_x = self.slots.splitter2.rect().0;
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
                let val = !self.viewport().show_grid;
                self.viewport_mut().show_grid = val;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 0, val);
                settings_changed = true;
            }
            Action::ToggleCube => {
                let val = !self.viewport().show_cube;
                self.viewport_mut().show_cube = val;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 1, val);
                settings_changed = true;
            }
            Action::ToggleOrigin => {
                let val = !self.viewport().show_origin;
                self.viewport_mut().show_origin = val;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 2, val);
                settings_changed = true;
            }
            Action::ToggleCameraPivot => {
                let val = !self.viewport().show_camera_pivot;
                self.viewport_mut().show_camera_pivot = val;
                self.write_active_camera_toggle("Show Camera Pivot", val);
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(2, 3, val);
                settings_changed = true;
            }
            Action::ToggleSquareViewport => {
                self.square_viewport = !self.square_viewport;
                let val = self.square_viewport;
                self.write_active_camera_toggle("Square Aspect", val);
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
                return;
            }
            Action::ToggleSpreadsheet => {
                self.show_spreadsheet = !self.show_spreadsheet;
                self.slots.spreadsheet.set_visible(self.show_spreadsheet);
                self.slots.spreadsheet_menubar.set_visible(self.show_spreadsheet);
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
            Action::NextContext | Action::PrevContext => {
                self.focused_pane = get_next_visible_pane(
                    self.focused_pane,
                    self.show_network,
                    self.show_viewport,
                    self.show_parameters,
                    self.show_spreadsheet,
                    action == Action::PrevContext,
                );
                self.sync_pane_focus();
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













    /// Logical size + scale, from the engine's `handle_resize` hook (the
    /// engine has already resized the renderer; the corner radius re-applies
    /// on the next `stage_renderer` flush).
    pub fn resize(&mut self, width: f32, height: f32, scale: f64) {
        if width > 0.0 && height > 0.0 {
            let old_width = self.width;
            self.scale = scale;
            cce_ui::scale::set_scale_factor(scale as f32);
            self.physical_width = (width as f64 * scale) as u32;
            self.physical_height = (height as f64 * scale) as u32;
            self.width = width;
            self.height = height;
            self.last_corner_radius = -1.0;

            if old_width > 0.0 {
                let r = self.width / old_width;
                self.splitter_layout.scale(r);
                let body_h = self.body_h();
                self.slots.splitter1.set_rect(self.splitter_layout.splitter1_x, HEADER_H, SPLITTER_W, body_h);
                self.slots.splitter2.set_rect(self.splitter_layout.splitter2_x, HEADER_H, SPLITTER_W, body_h);
            }

            self.sync_layout();
            self.read_panel_offsets();
            self.keep_cursor_in_view();
            self.viewport_dirty = true;
        }
    }

    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let in_network_pane = self.in_network_pane();
                // eprintln!("DEBUG MOUSEWHEEL: delta={:?}, phase={:?}, cursor=({}, {}), in_network_pane={}", delta, phase, self.cursor_x, self.cursor_y, in_network_pane);
                let node_area_y = self.positions[CONTENT_IDX].1;

                let in_viewport = self.cursor_x >= self.content_right_x()
                    && self.cursor_x < self.splitter_layout.splitter2_x
                    && self.cursor_y >= node_area_y
                    && self.cursor_y < self.height - STATUS_H;

                let mut focus_changed = false;
                let mut new_pane = None;
                {
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
                if !self.modifiers.control_key() {
                    // Routed (6bd shrink): propagate_event owns the scroll-gesture
                    // bookkeeping this loop used to hand-roll. The wheel is the only
                    // designer surface routed so far — the press/move cascade stays
                    // app-sovereign (see the routed-events deferral note below).
                    let wheel_ev = cce_ui::widget::Event::MouseWheel {
                        delta: *delta,
                        x: self.cursor_x,
                        y: self.cursor_y,
                        local_x: self.cursor_x,
                        local_y: self.cursor_y,
                    };
                    for i in 0..WIDGET_COUNT {
                        if i == VIEWPORT_IDX {
                            continue;
                        }
                        let root = self.slots.get_dyn_mut(i).base().id();
                        if self.ui_context.propagate_event(&wheel_ev, root) {
                            handled = true;
                            if i == CONTENT_IDX {
                                let active_node_area_y = self.positions[CONTENT_IDX].1;
                                let active_node_area_x = self.positions[CONTENT_IDX].0;
                                let w = self.slots.get_dyn(i);
                                let (gx, gy) = cce_ui::widget::GraphController::grid_origin(
                                    w.as_any().downcast_ref::<Graph>().expect("CONTENT_IDX must be a Graph"),
                                );
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
                            break;
                        }
                    }
                    if !handled {
                        let vp = self.slots.viewport.id();
                        if self.ui_context.propagate_event(&wheel_ev, vp) {
                            handled = true;
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
                    true
                } else if in_network_pane {
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
                                let dx = *x * 30.0 * self.graph_scroll_speed;
                                let dy = *y * 30.0 * self.graph_scroll_speed;
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
                                let dx = (pos.x as f32 / self.scale as f32) * self.graph_scroll_speed;
                                let dy = (pos.y as f32 / self.scale as f32) * self.graph_scroll_speed;
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
                } else {
                    false
                };

                if focus_changed {
                    self.sync_layout();
                    self.read_panel_offsets();
                    true
                } else {
                    result
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
                    if let Some(drag) = self.app_drag {
                        match drag {
                            AppDrag::NetworkResize { dir, start_rect, start_mouse } => {
                                let dx = self.cursor_x - start_mouse.0;
                                let dy = self.cursor_y - start_mouse.1;
                                let (sx, sy, sw, sh) = start_rect;

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
                            }
                            AppDrag::ParamResize { start_w, start_mouse_x } => {
                                let dx = self.cursor_x - start_mouse_x;
                                let new_w = (start_w - dx).max(150.0);
                                self.floating_param_width = new_w;
                                self.rebuild_positions();
                                self.apply_layout();
                                changed = true;
                            }
                            AppDrag::SpreadsheetResize { start_h, start_mouse_y } => {
                                let dy = self.cursor_y - start_mouse_y;
                                let new_h = (start_h - dy).max(100.0);
                                self.floating_spreadsheet_height = new_h;
                                self.rebuild_positions();
                                self.apply_layout();
                                changed = true;
                            }
                        }
                    } else if let Some(idx) = self.drag_widget {
                        // A widget drag: the slot's own Input drag hooks are driving.
                        self.slots.get_dyn_mut(idx).set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                        if {
                            let ev = cce_ui::widget::Event::DragUpdate { dx: 0.0, dy: 0.0, x: self.cursor_x, y: self.cursor_y, local_x: self.cursor_x, local_y: self.cursor_y };
                            let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                            unsafe { (*ptr).handle_event(&ev, &mut self.ui_context) }
                        } {
                            changed = true;
                            if idx == PARAM_IDX {
                                self.sync_parameters_to_project();
                            }
                        }
                    }

                    if self.drag_widget.is_none() && self.app_drag.is_none() {
                        for i in 0..WIDGET_COUNT {
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
                                    self.slots.network_panel.hit_test(cx, cy, &self.ui_context)
                                } else {
                                    false
                                }
                            } else {
                                self.slots.get_dyn_mut(i).hit_test(cx, cy, &self.ui_context)
                            };
                            let (tx, ty) = if inside { (cx, cy) } else { (-9999.0, -9999.0) };
                            let mv = cce_ui::widget::Event::PointerMove { x: tx, y: ty, local_x: tx, local_y: ty };
                            let ptr = self.slots.get_dyn_mut(i) as *mut (dyn WidgetHost + 'static);
                            if unsafe { (*ptr).handle_event(&mv, &mut self.ui_context) } {
                                changed = true;
                            }
                        }
                    }
                }
                changed
            }
            // THE DESIGNER'S EVENT LAYER IS ITS OWN WINDOWING SYSTEM (the event redesign,
            // closing the 6bd deferral): presses resolve a z-ordered click target through
            // app-owned hit shapes (circular-pane overrides, hardcoded pane z), derive pane
            // focus, run the unfocus rituals, and deliver through `handle_event` directly —
            // the UiContext router's hit-gating/descent/drag tracker cannot own that policy,
            // so it is deliberately not used here (mixing the two would double-run drag
            // state machines). Drags are split: `app_drag` is the app-mode gesture state
            // machine (floating-pane edge resizes, each variant carrying its whole gesture),
            // `drag_widget` only ever names a widget drag driven through the slot's Input
            // drag hooks. Exactly one of the two is armed per press.
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                if *btn_state == ElementState::Pressed {
                    self.pan_velocity_x = 0.0;
                    self.pan_velocity_y = 0.0;
                    self.is_scrolling_trackpad = false;
                    self.scroll_accum_x = 0.0;
                    self.scroll_accum_y = 0.0;

                    self.viewport_mut().reset_velocity();
                }
                let in_network_pane = self.in_network_pane();
                let node_area_x = self.positions[CONTENT_IDX].0;
                let node_area_y = self.positions[CONTENT_IDX].1;

                let is_pan_trigger = in_network_pane
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
                                return true;
                            }
                        }
                    }
                }

                if self.is_panning && *btn_state == ElementState::Released {
                    self.is_panning = false;
                    self.sync_layout();
                    self.read_panel_offsets();
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
                        state.slots.get_dyn(i).hit_test(x, y, &state.ui_context)
                    }
                };

                let in_circle_network_pane = if self.circular_network_pane {
                    self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, 0.0, BREADCRUMB_H)
                } else {
                    in_network_pane
                };

                match btn_state {
                    ElementState::Pressed => {
                        let hits_any_menu = (0..WIDGET_COUNT).any(|i| {
                            hits_widget(self, i, self.cursor_x, self.cursor_y)
                                && self.menubar_at(i).and_then(|m| m.get_menu_items_at(self.cursor_x, self.cursor_y)).is_some()
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
                            // A border press focuses the pane and consumes. It used to also arm
                            // a NETWORK_PANEL_IDX widget drag, but PassivePlate has no drag
                            // hooks (the panel move died with the 6as Plate dissolution), so
                            // the drag scaffolding is gone.
                            let on_border = self.circular_network_layout.hit_test_border(self.cursor_x, self.cursor_y, 12.0);
                            if on_border {
                                self.focused_pane = LEFT_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.slots.get_dyn_mut(old).unfocus();
                                    self.focused_widget = None;
                                }
                                self.slots.param.unfocus();
                                self.sync_parameters_to_project();
                                return true;
                            }
                        }

                        if *button == MouseButton::Left && !self.circular_network_pane && self.show_network {
                            let (fx, fy, fw, _fh) = self.floating_network_layout;
                            let cx = self.cursor_x;
                            let cy = self.cursor_y;

                            if let Some(dir) = self.network_resize_edge_at(cx, cy) {
                                self.app_drag = Some(AppDrag::NetworkResize {
                                    dir,
                                    start_rect: self.floating_network_layout,
                                    start_mouse: (cx, cy),
                                });
                                self.focused_pane = LEFT_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.slots.get_dyn_mut(old).unfocus();
                                    self.focused_widget = None;
                                }
                                self.slots.param.unfocus();
                                self.sync_parameters_to_project();
                                return true;
                            } else if cx >= fx && cx < fx + fw && cy >= fy && cy < fy + (if self.show_network { BREADCRUMB_H } else { 0.0 }) {
                                if self.slots.breadcrumb.mouse_input(*button, *btn_state, cx, cy, &mut self.ui_context) {
                                    return true;
                                }
                                // A strip press off the crumbs focuses the pane and consumes.
                                // The NETWORK_PANEL_IDX drag it used to arm was inert (see the
                                // circular-border note above).
                                self.focused_pane = LEFT_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.slots.get_dyn_mut(old).unfocus();
                                    self.focused_widget = None;
                                }
                                self.slots.param.unfocus();
                                self.sync_parameters_to_project();
                                return true;
                            }
                        }

                        if *button == MouseButton::Left && self.show_parameters {
                            let cx = self.cursor_x;
                            let cy = self.cursor_y;

                            if self.on_param_resize_edge(cx, cy) {
                                self.app_drag = Some(AppDrag::ParamResize {
                                    start_w: self.floating_param_width,
                                    start_mouse_x: cx,
                                });
                                self.focused_pane = PARAM_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.slots.get_dyn_mut(old).unfocus();
                                    self.focused_widget = None;
                                }
                                return true;
                            }
                        }

                        if *button == MouseButton::Left && self.show_spreadsheet {
                            let cx = self.cursor_x;
                            let cy = self.cursor_y;

                            if self.on_spreadsheet_resize_edge(cx, cy) {
                                let (_, _, _, ss_h) = self.floating_spreadsheet_rect();
                                self.app_drag = Some(AppDrag::SpreadsheetResize {
                                    start_h: ss_h,
                                    start_mouse_y: cy,
                                });
                                self.focused_pane = SPREADSHEET_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.slots.get_dyn_mut(old).unfocus();
                                    self.focused_widget = None;
                                }
                                return true;
                            }
                        }

                        if *button == MouseButton::Right {
                            if in_circle_network_pane {
                                let col = ((self.cursor_x - node_area_x - self.pan_x) / (self.grid_size_x + self.gap_col_w)).floor() as i32;
                                let row = ((self.cursor_y - node_area_y - self.pan_y) / (self.grid_size_y + self.gap_row_h)).floor() as i32;
                                self.grid_cursor_col = col;
                                self.grid_cursor_row = row;
                                self.open_node_palette();
                                return true;
                            }
                            return false;
                        }
                        let mut click_target = None;
                        for i in 0..WIDGET_COUNT {
                            if self.menubar_at(i).map(|m| m.is_menu_open()).unwrap_or(false)
                                && hits_widget(self, i, self.cursor_x, self.cursor_y)
                            {
                                click_target = Some(i);
                                break;
                            }
                        }
                        if click_target.is_none() {
                            let mut hit_order: Vec<usize> = (0..WIDGET_COUNT).collect();
                            hit_order.sort_by_key(|&i| {
                                let z = if i == NETWORK_PANEL_IDX {
                                    -5
                                } else if i == VIEWPORT_IDX {
                                    // Below PARAM_IDX: the params pane floats over the viewport,
                                    // and the old shared -4 tier let the stable sort's index order
                                    // hand every press over the pane to the viewport instead.
                                    -4
                                } else if i == PARAM_IDX {
                                    -3
                                } else {
                                    self.slots.get_dyn_mut(i).z_index()
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
                            } else if i == PARAM_MENUBAR_IDX || i == PARAM_IDX {
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
                                self.slots.get_dyn_mut(old).unfocus();
                                self.focused_widget = None;
                            }
                        }
                        if click_target != Some(PARAM_IDX) {
                            self.slots.param.unfocus();
                            self.sync_parameters_to_project();
                        }
                        if click_target.is_none() && in_circle_network_pane {
                            let col = ((self.cursor_x - node_area_x - self.pan_x) / (self.grid_size_x + self.gap_col_w)).floor() as i32;
                            let row = ((self.cursor_y - node_area_y - self.pan_y) / (self.grid_size_y + self.gap_row_h)).floor() as i32;
                            self.grid_cursor_col = col;
                            self.grid_cursor_row = row;
                            changed = true;
                        }
                        if let Some(i) = click_target {
                            self.slots.get_dyn_mut(i).set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                            if {
                                let ev = cce_ui::widget::Event::MouseButton { button: *button, state: *btn_state, x: self.cursor_x, y: self.cursor_y, local_x: self.cursor_x, local_y: self.cursor_y };
                                let ptr = self.slots.get_dyn_mut(i) as *mut (dyn WidgetHost + 'static);
                                unsafe { (*ptr).handle_event(&ev, &mut self.ui_context) }
                            } {
                                changed = true;
                                if i == PARAM_IDX {
                                    self.sync_parameters_to_project();
                                }

                            }
                            if self.slots.draggable(i) {
                                self.slots.get_dyn_mut(i).set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                                {
                                    let ev = cce_ui::widget::Event::DragStart { start_x: self.cursor_x, start_y: self.cursor_y };
                                    let ptr = self.slots.get_dyn_mut(i) as *mut (dyn WidgetHost + 'static);
                                    unsafe { (*ptr).handle_event(&ev, &mut self.ui_context); }
                                }
                                self.drag_widget = Some(i);
                                self.drag_press_cursor = Some((self.cursor_x, self.cursor_y));
                            }
                            if i != PARAM_IDX {
                                self.slots.get_dyn_mut(i).focus();
                                self.focused_widget = Some(i);
                                if self.menubar_at(i).map(|m| m.is_menu_bar()).unwrap_or(false)
                                    && !self.slots.get_dyn_mut(i).focused(&self.ui_context)
                                {
                                    self.slots.get_dyn_mut(i).unfocus();
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
                                    let col = ((self.cursor_x - node_area_x - self.pan_x) / (self.grid_size_x + self.gap_col_w)).floor() as i32;
                                    let row = ((self.cursor_y - node_area_y - self.pan_y) / (self.grid_size_y + self.gap_row_h)).floor() as i32;
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
                        if let Some(drag) = self.app_drag.take() {
                            // App-mode drag teardown. The DragEnd send is kept from the old
                            // shared teardown for faithfulness — the pane widget never began
                            // a drag in resize mode, so its commit/cancel hook is a no-op.
                            let idx = match drag {
                                AppDrag::NetworkResize { .. } => NETWORK_PANEL_IDX,
                                AppDrag::ParamResize { .. } => PARAM_IDX,
                                AppDrag::SpreadsheetResize { .. } => SPREADSHEET_IDX,
                            };
                            {
                                let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                                unsafe { (*ptr).handle_event(&cce_ui::widget::Event::DragEnd, &mut self.ui_context); }
                            }
                            self.sync_layout();
                            if matches!(drag, AppDrag::NetworkResize { .. }) {
                                self.read_panel_offsets();
                            }
                            changed = true;
                        }
                        if self.drag_widget.is_some() {
                            let idx = self.drag_widget.unwrap();
                            if idx == PARAM_IDX {
                                {
                                    let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                                    unsafe { (*ptr).handle_event(&cce_ui::widget::Event::DragEnd, &mut self.ui_context); }
                                }
                                self.sync_layout();
                            } else if idx == SPREADSHEET_IDX {
                                {
                                    let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                                    unsafe { (*ptr).handle_event(&cce_ui::widget::Event::DragEnd, &mut self.ui_context); }
                                }
                                self.sync_layout();
                            } else if idx == CONTENT_IDX {
                                {
                                    let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                                    unsafe { (*ptr).handle_event(&cce_ui::widget::Event::DragEnd, &mut self.ui_context); }
                                }
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
                            } else {
                                {
                                    let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                                    unsafe { (*ptr).handle_event(&cce_ui::widget::Event::DragEnd, &mut self.ui_context); }
                                }
                            }
                            self.drag_widget = None;
                            self.drag_press_cursor = None;
                            changed = true;
                        }
                        let mut sync_params = false;
                        {
                            let ctx = &mut self.ui_context;
                            for i in 0..WIDGET_COUNT {
                                let w = self.slots.get_dyn_mut(i);
                                w.set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                                let ev = cce_ui::widget::Event::MouseButton { button: *button, state: *btn_state, x: self.cursor_x, y: self.cursor_y, local_x: self.cursor_x, local_y: self.cursor_y };
                                if w.handle_event(&ev, ctx) {
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
            WindowEvent::KeyboardInput { event, .. } => {
                if event.logical_key == Key::Named(NamedKey::Space) {
                    self.space_pressed = event.state == ElementState::Pressed;
                }

                // Context switching dispatches ahead of the widget key paths
                // (param pane, node palette) so the chord works from any pane.
                if event.state == ElementState::Pressed {
                    if let Some(action @ (Action::NextContext | Action::PrevContext)) =
                        self.shortcut_manager.match_action(&self.modifiers, &event.logical_key)
                    {
                        self.execute_action(action);
                        return true;
                    }
                }

                if self.slots.param.keyboard_input(event, &mut self.ui_context) {
                    self.sync_parameters_to_project();
                    return true;
                }

                if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) {
                    self.graph_mut().cancel_connecting();
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
                                                    self.viewport_mut().rotation_y = 0.0;
                                                    self.viewport_mut().rotation_x = 0.0;
                                                }
                                                self.viewport_mut().zoom = 1.0;
                                                self.viewport_mut().reset_velocity();
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
                                                    self.gap_col_w = 20.0;
                                                    self.gap_row_h = 20.0;
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
                                                    self.gap_col_w = base_col_w * f;
                                                    self.gap_row_h = base_row_h * f;

                                                    let mut actual_xmin = f32::MAX;
                                                    let mut actual_xmax = f32::MIN;
                                                    let mut actual_ymin = f32::MAX;
                                                    let mut actual_ymax = f32::MIN;

                                                    for slot_idx in 0..active_nodes {
                                                        let (col, row) = self.current_dir().children[slot_idx].position;
                                                        let x_min = col * (self.grid_size_x + self.gap_col_w);
                                                        let x_max = x_min + self.grid_size_x;
                                                        let y_min = row * (self.grid_size_y + self.gap_row_h);
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
                    {
                    let kev = cce_ui::widget::Event::KeyInput(event.clone());
                    let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                    unsafe { (*ptr).handle_event(&kev, &mut self.ui_context) }
                }
                } else { false }
            }
        }
    }

    /// Per-tick simulation (engine `tick` hook): config polling, widget
    /// ticks, inertia, drag edge-panning. Returns true when the frame needs
    /// a rebuild. The render half lives in [`State::stage_frame`].
    pub fn tick_frame(&mut self, dt: f32) -> bool {
        let now = Instant::now();
        self.last_frame = now;

        if now.duration_since(self.last_config_read).as_secs_f32() > 2.0 {
            self.last_config_read = now;
            let config_paths = [
                cce_ui::config::get_config_path(),
                cce_ui::config::config_home().join("ccec").join("config.kdl"),
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
            } else {
                self.update_inertial_settings();
            }
            let design_path = DesignSettings::file_path();
            if let Ok(m) = std::fs::metadata(&design_path) {
                if let Ok(mod_time) = m.modified() {
                    if Some(mod_time) != self.last_design_mod_time {
                        self.last_design_mod_time = Some(mod_time);
                         let settings = DesignSettings::load();
                         self.square_viewport = settings.viewport.square;
                         self.grid_thickness = settings.viewport.grid_thickness;
                         self.viewport_mut().show_grid = settings.viewport.show_grid_enabled;
                         self.viewport_mut().show_cube = settings.viewport.show_cube_enabled;
                         self.viewport_mut().show_origin = settings.viewport.show_origin_enabled;
                         self.viewport_mut().show_camera_pivot = settings.viewport.show_camera_pivot_enabled;
                         self.viewport_mut().bg_color = settings.viewport.bg_color;
                         self.node_color = cce_ui::color::graph_node_color();
                         self.viewport_mut().grid_color = settings.viewport.grid_color;
                         self.origin_size = settings.viewport.origin_size;
                         self.camera_pivot_size = settings.viewport.camera_pivot_size;
                         self.cell_color = cce_ui::color::graph_cell_color();
                         self.gap_color = cce_ui::color::graph_gap_color();

                        colors::set_node_color(self.node_color);

                        self.update_origin_geometry();
                        self.update_grid_geometry();
                        self.update_pivot_geometry();
                        self.update_viewport_bg_geometry();
                        self.sync_grid_settings();
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

        let mut tick_changed = false;
        let ctx = &mut self.ui_context;
        for i in 0..WIDGET_COUNT {
            if self.slots.get_dyn_mut(i).tick(dt, ctx) {
                tick_changed = true;
            }
        }
        if cce_ui::widget::hover_animation::tick(dt) {
            tick_changed = true;
        }

        let py = self.viewport().pending_yaw;
        let pp = self.viewport().pending_pitch;
        if py != 0.0 || pp != 0.0 {
            if self.update_active_camera_rotation(py, pp) {
                tick_changed = true;
            }
            self.viewport_mut().pending_yaw = 0.0;
            self.viewport_mut().pending_pitch = 0.0;
        }

        self.update_recent_files_layout();

        // Panning kinetic slide
        if !self.is_panning && !self.is_scrolling_trackpad && (self.pan_velocity_x.abs() > 0.01 || self.pan_velocity_y.abs() > 0.01) {
            if !self.graph_inertial_scroll {
                self.pan_velocity_x = 0.0;
                self.pan_velocity_y = 0.0;
            } else {
                self.pan_x += self.pan_velocity_x * dt;
                self.pan_y += self.pan_velocity_y * dt;

                // Apply dynamic friction decay
                let decay = self.graph_scroll_friction.powf(dt * 60.0);
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



        if tick_changed {
        }

        // Edge-panning waits for a real drag gesture: the pointer must stray
        // from the press point first (see `drag_press_cursor`).
        if let Some((sx, sy)) = self.drag_press_cursor {
            let dx = self.cursor_x - sx;
            let dy = self.cursor_y - sy;
            if dx * dx + dy * dy >= 64.0 {
                self.drag_press_cursor = None;
            }
        }
        let mut panned = false;
        if let Some(idx) = self.drag_widget {
            if idx == CONTENT_IDX && self.drag_press_cursor.is_none() {
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
                self.slots.get_dyn_mut(idx).set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                {
                                let ev = cce_ui::widget::Event::DragUpdate { dx: 0.0, dy: 0.0, x: self.cursor_x, y: self.cursor_y, local_x: self.cursor_x, local_y: self.cursor_y };
                                let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                                unsafe { (*ptr).handle_event(&ev, &mut self.ui_context) }
                            };
            }
            self.sync_layout();
            self.read_panel_offsets();
        }

        tick_changed || panned
    }

    /// Flush CPU-staged mesh updates to the renderer's persistent meshes.
    fn flush_pending_meshes(&mut self, renderer: &mut cce_ui::vk::VkRenderer) {
        let Some(meshes) = self.meshes else { return };
        if let Some(verts) = self.pending_grid.take() {
            renderer.update_mesh(meshes.grid, bytemuck::cast_slice(&verts));
        }
        if let Some(verts) = self.pending_origin.take() {
            renderer.update_mesh(meshes.origin, bytemuck::cast_slice(&verts));
        }
        if let Some(verts) = self.pending_pivot.take() {
            renderer.update_mesh(meshes.pivot, bytemuck::cast_slice(&verts));
        }
        if let Some(verts) = self.pending_viewport_bg.take() {
            renderer.update_mesh(meshes.viewport_bg, bytemuck::cast_slice(&verts));
        }
        if self.spheres_dirty {
            self.spheres_dirty = false;
            renderer.update_mesh(meshes.spheres, bytemuck::cast_slice(&self.rt_sphere_verts));
        }
    }

    /// One-time renderer setup (engine `renderer_init` hook): the persistent
    /// 3D meshes. The spheres mesh starts empty and fills from the node graph
    /// via the pending-mesh flush.
    pub fn init_renderer(&mut self, renderer: &mut cce_ui::vk::VkRenderer) {
        let cube_verts = cube_vertices();
        let linear_grid_color = cce_ui::colors::to_linear_rgb(self.grid_color);
        let grid_verts = grid_vertices(self.grid_thickness, linear_grid_color);
        let origin_verts = origin_vectors_vertices(self.origin_size);
        let pivot_verts = camera_pivot_vertices(self.camera_pivot_size);
        let bg_verts =
            Self::viewport_bg_vertices(cce_ui::colors::to_linear_rgb(self.viewport().bg_color));
        self.meshes = Some(SceneMeshes {
            cube: renderer.create_mesh(bytemuck::cast_slice(&cube_verts)),
            viewport_bg: renderer.create_mesh(bytemuck::cast_slice(&bg_verts)),
            spheres: renderer.create_mesh(&[]),
            grid: renderer.create_mesh(bytemuck::cast_slice(&grid_verts)),
            origin: renderer.create_mesh(bytemuck::cast_slice(&origin_verts)),
            pivot: renderer.create_mesh(bytemuck::cast_slice(&pivot_verts)),
        });
        // Scene geometry built during `State::new` (before the renderer
        // existed) uploads on the first frame's flush.
        self.spheres_dirty = !self.rt_sphere_verts.is_empty();
        self.viewport_dirty = true;
    }

    /// Frame staging (engine `stage_renderer` hook): corner radius, pending
    /// meshes, text, and the 3D scene / RT pane. Returns true while the path
    /// tracer is still refining, to keep frames coming.
    pub fn stage_frame(&mut self, renderer: &mut cce_ui::vk::VkRenderer) -> bool {
        let radius = cce_ui::color::backplate_corner_radius() * self.scale as f32;
        if radius != self.last_corner_radius {
            self.last_corner_radius = radius;
            renderer.set_corner_radius(radius);
        }
        self.flush_pending_meshes(renderer);
        let meshes = self.meshes.expect("stage_frame before renderer_init");

        // 3D canvas: stage the scene into the renderer's backdrop when the
        // viewport is visible and its inputs changed; unstaged frames reuse the
        // previous backdrop (the renderer's equivalent of the old cached pass).
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

                let rt_mode = self.viewport().rt_mode;
                let viewport_changed = self.viewport_dirty
                    || self.last_viewport_rt_mode != rt_mode
                    || self.last_viewport_camera_pos != camera_pos
                    || self.last_viewport_camera_rx != rx
                    || self.last_viewport_camera_ry != ry
                    || self.last_viewport_camera_rz != rz
                    || self.last_viewport_pivot != pivot
                    || self.last_viewport_zoom != self.viewport().zoom
                    || self.last_viewport_rotation_x != self.viewport().rotation_x
                    || self.last_viewport_rotation_y != self.viewport().rotation_y
                    || self.last_viewport_bg_color != self.viewport().bg_color
                    || self.last_viewport_show_grid != self.viewport().show_grid
                    || self.last_viewport_show_cube != self.viewport().show_cube
                    || self.last_viewport_show_origin != self.viewport().show_origin
                    || self.last_viewport_show_camera_pivot != self.viewport().show_camera_pivot
                    || self.last_viewport_width != cw
                    || self.last_viewport_height != ch
                    || self.last_viewport_active_camera != self.active_camera
                    || self.last_viewport_show_viewport != self.show_viewport
                    || self.last_viewport_wireframe != self.wireframe
                    || self.last_viewport_wireframe_overlay != self.wireframe_overlay;

                if viewport_changed {
                    if !rt_mode {
                    let aspect = cw as f32 / ch as f32;
                    let (proj, view_mat, model) = self.viewport().get_matrices(aspect, Some(camera_pos), Some(Vec3::new(rx, ry, rz)), Some(pivot));
                    let mvp = (proj * view_mat * model).to_cols_array_2d();

                    let cam_angle_y = camera_pos.x.atan2(camera_pos.z);
                    let rot_angle = if self.active_camera != "Default Camera" {
                        let base_offset = camera_pos - pivot;
                        let yaw0 = base_offset.x.atan2(base_offset.z);
                        ry.to_radians() + yaw0
                    } else {
                        self.viewport().rotation_y + cam_angle_y
                    };
                    let model_pivot = Mat4::from_translation(pivot) * Mat4::from_rotation_y(rot_angle);
                    let mvp_pivot = (proj * view_mat * model_pivot).to_cols_array_2d();

                    // Same draw order as the wgpu pass: bg quad, grid, origin,
                    // pivot, cube, spheres.
                    const NO_TINT: [f32; 4] = [0.0; 4];
                    let mut draws = vec![SceneDraw { mesh: meshes.viewport_bg, mvp, wireframe: false, wire_tint: NO_TINT }];
                    if self.viewport().show_grid {
                        draws.push(SceneDraw { mesh: meshes.grid, mvp, wireframe: false, wire_tint: NO_TINT });
                    }
                    if self.viewport().show_origin {
                        draws.push(SceneDraw { mesh: meshes.origin, mvp, wireframe: false, wire_tint: NO_TINT });
                    }
                    if self.viewport().show_camera_pivot {
                        draws.push(SceneDraw { mesh: meshes.pivot, mvp: mvp_pivot, wireframe: false, wire_tint: NO_TINT });
                    }
                    if self.viewport().show_cube {
                        draws.push(SceneDraw { mesh: meshes.cube, mvp, wireframe: false, wire_tint: NO_TINT });
                    }
                    if self.vertex_count_spheres > 0 {
                        if self.wireframe_overlay {
                            // Shaded + wireframe: the filled mesh, then its
                            // wire pass darkened so the edges separate from
                            // the identical fill (the wire pipeline's depth
                            // bias keeps the lines above the coplanar fill).
                            draws.push(SceneDraw { mesh: meshes.spheres, mvp, wireframe: false, wire_tint: NO_TINT });
                            draws.push(SceneDraw { mesh: meshes.spheres, mvp, wireframe: true, wire_tint: [0.0, 0.0, 0.0, 0.75] });
                        } else {
                            draws.push(SceneDraw { mesh: meshes.spheres, mvp, wireframe: self.wireframe, wire_tint: NO_TINT });
                        }
                    }
                    renderer.stage_scene((sx, sy, cw, ch), draws);
                    }

                    // Update viewport cache
                    self.last_viewport_camera_pos = camera_pos;
                    self.last_viewport_camera_rx = rx;
                    self.last_viewport_camera_ry = ry;
                    self.last_viewport_camera_rz = rz;
                    self.last_viewport_pivot = pivot;
                    self.last_viewport_zoom = self.viewport().zoom;
                    self.last_viewport_rotation_x = self.viewport().rotation_x;
                    self.last_viewport_rotation_y = self.viewport().rotation_y;
                    self.last_viewport_bg_color = self.viewport().bg_color;
                    self.last_viewport_show_grid = self.viewport().show_grid;
                    self.last_viewport_show_cube = self.viewport().show_cube;
                    self.last_viewport_show_origin = self.viewport().show_origin;
                    self.last_viewport_show_camera_pivot = self.viewport().show_camera_pivot;
                    self.last_viewport_width = cw;
                    self.last_viewport_height = ch;
                    self.last_viewport_active_camera = self.active_camera.clone();
                    self.last_viewport_show_viewport = self.show_viewport;
                    self.last_viewport_wireframe = self.wireframe;
                    self.last_viewport_wireframe_overlay = self.wireframe_overlay;
                    self.last_viewport_rt_mode = rt_mode;
                    self.viewport_dirty = false;
                }

                // Path-traced mode: staged EVERY frame (each one adds a
                // sample); the renderer resets the accumulation itself when
                // the camera/pane changes, so camera drags stay interactive
                // (1-spp noise) and stillness converges.
                if rt_mode {
                    let key = (self.viewport().show_cube, self.rt_geometry_version);
                    if self.last_rt_scene_key != Some(key) {
                        let (rt_tris, rt_mats) = self.collect_rt_scene();
                        renderer.set_rt_scene(&rt_tris, &rt_mats);
                        self.last_rt_scene_key = Some(key);
                    }
                    let aspect = cw as f32 / ch as f32;
                    let (proj, view_mat, model) = self.viewport().get_matrices(aspect, Some(camera_pos), Some(Vec3::new(rx, ry, rz)), Some(pivot));
                    let inv_mvp = (proj * view_mat * model).inverse().to_cols_array_2d();
                    renderer.stage_rt((sx, sy, cw, ch), cce_ui::vk::RtCamera { inv_mvp });
                }
            } else if self.viewport_dirty {
                // Zero-area pane: clear the backdrop once.
                renderer
                    .stage_scene((0, 0, self.physical_width, self.physical_height), Vec::new());
                self.last_viewport_show_viewport = false;
                self.viewport_dirty = false;
            }
        } else if !self.is_detached_network && self.viewport_dirty {
            // Viewport hidden: clear the backdrop (the old path's clear pass),
            // and force a re-stage when it comes back.
            renderer
                .stage_scene((0, 0, self.physical_width, self.physical_height), Vec::new());
            self.last_viewport_show_viewport = false;
            self.viewport_dirty = false;
        }

        // The engine draws the frame; keep frames coming while the path
        // tracer is still refining.
        !self.is_detached_network
            && self.show_viewport
            && self.viewport().rt_mode
            && renderer.rt_accumulating()
    }
}

