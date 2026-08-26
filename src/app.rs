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
use crate::slots::*;
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

impl FsNode {
    /// Can this node be dived into (Enter, double-click, the network's `i`)?
    /// One predicate, because it was hand-copied at three call sites and none
    /// of them learned about new container types: subnet-like types by name,
    /// otherwise anything that actually has children.
    pub fn is_enterable(&self) -> bool {
        matches!(self.node_type.as_str(), "node" | "utility" | "simnet" | "session")
            || !self.children.is_empty()
    }
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
    /// Collapsed plate panes by name ("network", "parameters", "spreadsheet",
    /// "playbar"). Absent from older saves — an empty list expands everything,
    /// so loading is deterministic either way.
    #[serde(default)]
    pub collapsed_panes: Vec<String>,
    /// Splitter positions as fractions of the window width (splitter1,
    /// splitter2), so a project restores its column proportions at any
    /// window size. None in older saves keeps the live positions.
    #[serde(default)]
    pub splitters: Option<(f32, f32)>,
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

/// One entry in a node's right-click context menu, parallel to the visible
/// labels shown via `context_menu::show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeMenuAction {
    /// Dive into the node's subnet (the double-click behavior).
    Enter,
    /// Flip the node's geometry visibility (utility nodes excluded).
    ToggleGeometry,
    /// Remove the node.
    Delete,
}

/// The 3D viewport's right-click context menu actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportMenuAction {
    /// Move the active camera so the visible node geometry fills the view.
    FrameAll,
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
    /// Collapse a pane to its title stub, or restore it — the plate corner
    /// menu's Collapse/Expand, reachable without driving the pointer.
    SetPaneCollapsed { pane: String, collapsed: bool },
    /// Move a pane out into its own window, or take it back — the corner menu's
    /// Detach/Reattach.
    SetPaneDetached { pane: String, detached: bool },
    /// Move the playhead. Simnets solve up to this frame, so it is the only way
    /// to drive a simulation without dragging the playbar.
    SetFrame { frame: f32 },
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

/// Whether a node gets a `meta` (per-node preferences) child: every node the
/// user places that can produce geometry — subnet instances and the native
/// geometry types. Cameras, settings containers, and meta itself do not.
fn meta_eligible(node: &FsNode) -> bool {
    node.node_type.eq_ignore_ascii_case("node")
        || crate::geometry::is_geometry_node_type(&node.node_type)
}

/// Ensure `node` (and its subtree) carries the per-node `meta` child where
/// eligible, and that every existing meta has the full preference set —
/// the migration path for saved scenes, and the instantiation path for new
/// nodes. Idempotent; never touches the Session settings tree.
pub fn ensure_meta_on(node: &mut FsNode) {
    if node.node_type.eq_ignore_ascii_case("session") || node.node_type.eq_ignore_ascii_case("meta")
    {
        return;
    }
    if meta_eligible(node) {
        if !node.children.iter().any(|c| c.node_type == "meta") {
            node.children.push(FsNode {
                id: generate_node_id(),
                name: "meta".to_string(),
                node_type: "meta".to_string(),
                children: vec![],
                params: vec![],
                geometry_visible: false,
                position: (0.0, 4.0),
                inputs: 0,
                outputs: 0,
            });
        }
        let meta = node.children.iter_mut().find(|c| c.node_type == "meta").unwrap();
        for (name, default) in [
            ("Point Markers", "false"),
            ("Point Numbers", "false"),
            ("Point Normals", "false"),
            ("Wireframe", "false"),
        ] {
            if !meta.params.iter().any(|p| p.name == name) {
                meta.params.push(ParamDef {
                    name: name.to_string(),
                    label: String::new(),
                    param_type: "toggle".to_string(),
                    default: default.to_string(),
                    options: Vec::new(),
                    min: None,
                    max: None,
                    step: None,
                });
            }
        }
    }
    for c in &mut node.children {
        ensure_meta_on(c);
    }
}

/// [`ensure_meta_on`] over every node of a project tree (the root itself is a
/// container, not a placed node).
pub fn ensure_meta_children(root: &mut FsNode) {
    for c in &mut root.children {
        ensure_meta_on(c);
    }
}

/// Read a boolean preference off a node's `meta` child; absent meta or
/// absent param reads false.
pub fn meta_pref(node: &FsNode, name: &str) -> bool {
    node.children
        .iter()
        .find(|c| c.node_type == "meta")
        .and_then(|m| m.params.iter().find(|p| p.name == name))
        .map(|p| p.default == "true")
        .unwrap_or(false)
}

/// Merge template evolution into a loaded project tree, so saved scenes gain
/// controls added to a template after they were saved. Every deserialized
/// project routes through this (file load, the detached-window sync reload,
/// the thumbnail renderer).
///
/// The ownership rule: **the template owns the surface and the
/// implementation, the instance owns its values.** Per matched node, params
/// missing from the instance are appended with template defaults; params the
/// instance has keep their value but take the template's UI metadata (type,
/// label, range, options). For subnet templates (type "node" with children —
/// Sphere, Plane, Extrude), the matched children's `Code` is refreshed from
/// the template outright, because the new params are dead weight without the
/// kernel that reads them — which means a kernel hand-edited INSIDE a
/// template instance reverts on load; a custom kernel belongs in a bare
/// OpenCL node, whose Code is instance-owned and never touched here.
///
/// Matching is conservative: native nodes (group, attribute, scatter, …)
/// match their template by node type exactly; subnet instances match by name
/// ("Sphere 3" → "Sphere", so a renamed instance simply keeps its saved
/// shape), and only merge when EVERY template child is present by name and
/// type — a hand-built subnet that happens to share the name is left alone,
/// and nothing is ever injected or deleted. Simnet children (the user's sim
/// chain) are out of scope by construction: simnet is a native type.
pub fn merge_template_defs(root: &mut FsNode, templates: &[NodeTemplate]) {
    fn template_for<'a>(node: &FsNode, templates: &'a [NodeTemplate]) -> Option<&'a FsNode> {
        if node.node_type.eq_ignore_ascii_case("node") {
            let base = node.name.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end();
            templates.iter().map(|t| &t.node).find(|t| {
                t.node_type.eq_ignore_ascii_case("node")
                    && (t.name == node.name || (!base.is_empty() && t.name == base))
            })
        } else {
            templates.iter().map(|t| &t.node).find(|t| {
                !t.node_type.eq_ignore_ascii_case("node")
                    && t.node_type.eq_ignore_ascii_case(&node.node_type)
            })
        }
    }
    fn merge_params(node: &mut FsNode, template: &FsNode) {
        for tp in &template.params {
            if let Some(ip) = node.params.iter_mut().find(|p| p.name == tp.name) {
                ip.param_type = tp.param_type.clone();
                ip.label = tp.label.clone();
                ip.options = tp.options.clone();
                ip.min = tp.min;
                ip.max = tp.max;
                ip.step = tp.step;
            } else {
                node.params.push(tp.clone());
            }
        }
    }
    fn merge_node(node: &mut FsNode, templates: &[NodeTemplate]) {
        if let Some(t) = template_for(node, templates) {
            let owns_impl = t.node_type.eq_ignore_ascii_case("node") && !t.children.is_empty();
            let children_match = t.children.iter().all(|tc| {
                node.children.iter().any(|ic| ic.name == tc.name && ic.node_type == tc.node_type)
            });
            if !owns_impl || children_match {
                merge_params(node, t);
                if owns_impl {
                    for tc in &t.children {
                        let ic = node
                            .children
                            .iter_mut()
                            .find(|ic| ic.name == tc.name && ic.node_type == tc.node_type)
                            .expect("children_match checked above");
                        if let Some(t_code) = tc.params.iter().find(|p| p.name == "Code") {
                            if let Some(i_code) = ic.params.iter_mut().find(|p| p.name == "Code") {
                                i_code.default = t_code.default.clone();
                            }
                        }
                        merge_params(ic, tc);
                    }
                }
            }
        }
        for c in &mut node.children {
            merge_node(c, templates);
        }
    }
    for c in &mut root.children {
        merge_node(c, templates);
    }
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
    /// Project to open at startup instead of the bundled default — the Main
    /// node's "Set As Default" button. A path string (what
    /// `loaded_project_path` held when it was set); absent = the bundled
    /// `default_project.json`. Deliberately NOT the project file itself:
    /// that file is versioned AND is the detached-window sync channel, so
    /// "make this the default" must not rewrite it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_project: Option<String>,
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
        Some(Self::from_kdl_str(&content))
    }

    pub(crate) fn from_kdl_str(content: &str) -> Self {
        let mut json_val = cce_ui::config::parse_kdl_to_json(content);
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
        serde_json::from_value::<Self>(json_val).unwrap_or_else(|_| Self::default())
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
        if let Some(kdl_str) = self.to_kdl_str() {
            let _ = fs::write(path, kdl_str);
        }
    }

    pub(crate) fn to_kdl_str(&self) -> Option<String> {
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
            return Some(cce_ui::config::json_to_kdl_string(&json_val));
        }
        None
    }
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
/// The floating layout's three dock slots. Dimensions belong to the DOCK
/// (left/right column widths, bottom strip height and tucks), panes are
/// assigned to docks — so swapping panes preserves the geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    Left,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AppDrag {
    NetworkResize { dir: ResizeDirection, start_rect: (f32, f32, f32, f32), start_mouse: (f32, f32) },
    ParamResize { start_w: f32, start_mouse_x: f32 },
    SpreadsheetResize { start_h: f32, start_mouse_y: f32 },
    /// The spreadsheet's left edge drag, as an inset past the flush position
    /// beside the network pane: a positive inset tucks the spreadsheet UNDER
    /// the pane, whose bottom the layout raises to make room.
    SpreadsheetResizeLeft { start_inset: f32, start_mouse_x: f32 },
    /// The spreadsheet's right edge, symmetrically, tucking under the parameter pane.
    SpreadsheetResizeRight { start_inset: f32, start_mouse_x: f32 },
    /// Dragging a plate's corner dot repositions the plate: the dock region
    /// under the cursor highlights, and release snaps the plate there,
    /// swapping with whatever pane held that dock. Armed from a press on the
    /// dot once motion exceeds the click threshold; a clean click still opens
    /// the menu. (Sizing stays on the pane edge hotspots.)
    DockDrag { idx: usize },
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
    /// LINE_LIST edge expansion of `spheres` (vertex pairs per triangle
    /// edge) — the wire pass draws real line primitives, never
    /// PolygonMode::LINE (driver-broken; see cce-ui's scene stage).
    pub sphere_edges: cce_ui::vk::MeshId,
    pub grid: cce_ui::vk::MeshId,
    pub origin: cce_ui::vk::MeshId,
    pub pivot: cce_ui::vk::MeshId,
    /// The Render node's point display (one octahedron per distinct vertex).
    pub points: cce_ui::vk::MeshId,
    /// Selected-Group membership markers: while a Group node is selected, one
    /// marker per vertex it tags, so the selection SHOWS the group.
    pub group_points: cce_ui::vk::MeshId,
    /// Per-node meta "Point Markers" overlay.
    pub meta_points: cce_ui::vk::MeshId,
    /// Per-node meta "Wireframe" overlay (LINE_LIST edge pairs).
    pub meta_wires: cce_ui::vk::MeshId,
    /// Per-node meta "Point Normals" overlay (LINE_LIST whiskers).
    pub meta_normals: cce_ui::vk::MeshId,
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

    /// The node right-click context menu: the targeted node slot and the
    /// actions parallel to the visible items pushed into `context_menu::show`.
    /// `None` when no menu is open. The menu's geometry/paint lives in the
    /// cce-ui `context_menu` thread-local; these just remember what to do on
    /// click, since the designer routes menu clicks itself.
    pub node_menu_slot: Option<usize>,
    pub node_menu_actions: Vec<NodeMenuAction>,
    /// The viewport right-click menu (same thread-local `context_menu`
    /// machinery as the node menu; this flag says the open menu is OURS).
    pub viewport_menu_active: bool,
    pub viewport_menu_actions: Vec<ViewportMenuAction>,
    /// The plate corner menu — same `context_menu` thread-local again; the slot
    /// says which plate's control opened it (and doubles as the pressed state
    /// the corner control paints with).
    /// Solved simulation states, kept across frames so playing forward costs one
    /// step per frame instead of re-solving from the start frame every redraw.
    pub sim_cache: crate::geometry::SimCache,
    /// Frame the scene was last built at, so the timeline moving can invalidate it.
    pub last_sim_frame: i32,
    pub plate_menu_slot: Option<usize>,
    pub plate_menu_actions: Vec<crate::plate_corner::PlateMenuAction>,
    /// Panes shrunk to their title stub, indexed by slot. Only the
    /// `plate_corner::PLATE_SLOTS` entries are ever set.
    pub collapsed_panes: [bool; WIDGET_COUNT],
    /// A press on a plate corner dot, not yet resolved into click-opens-menu
    /// or drag-repositions-plate: `(slot, press_x, press_y)`.
    pub corner_press: Option<(usize, f32, f32)>,
    /// Dock occupancy, indexed Left/Right/Bottom. Swapped by dot drags.
    pub dock_panes: [usize; 3],
    /// The dock a live DockDrag would drop into — the render pass highlights it.
    pub dock_drag_target: Option<Dock>,

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
    /// This process IS the detached window for one pane — the generic sibling
    /// of `is_detached_network`, which stays its own flag because the network's
    /// detached window is not merely detached: it is CIRCULAR, with a radial
    /// border resize and custom CSD that no rectangular pane wants.
    pub detached_pane: Option<usize>,
    /// The MAIN window's record of which panes it has handed to a detached
    /// window, so it lays them out as stubs. `detached_circular_network` is the
    /// network's equivalent.
    pub detached_panes: [bool; WIDGET_COUNT],
    /// The detached child PROCESS per pane — so Reattach can close the window it
    /// is taking the pane back from, and so the parent can notice a child the
    /// user closed themselves and take the pane back on its own.
    ///
    /// The handle, not a bare pid: an exited child the parent never waits on is
    /// a ZOMBIE, and `kill(pid, 0)` succeeds for zombies — a pid-based liveness
    /// probe reports a closed window as still running, forever. `try_wait`
    /// reaps and reports for real.
    pub detached_children: std::collections::HashMap<usize, std::process::Child>,
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
    /// How far the spreadsheet's left/right edge reaches INTO the neighboring
    /// pane's span past its flush position (0 = glued beside the neighbor).
    /// A positive inset tucks the spreadsheet UNDER that neighbor: the layout
    /// raises the neighbor's bottom edge to the spreadsheet's top.
    pub floating_spreadsheet_inset_left: f32,
    pub floating_spreadsheet_inset_right: f32,
    pub loaded_project_path: Option<std::path::PathBuf>,
    /// The configured startup project (`DesignSettings::default_project`),
    /// mirrored live so "Set As Default" can rewrite it and `save_settings` —
    /// which reconstructs DesignSettings from live state — can carry it.
    pub default_project_setting: Option<String>,
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
    /// "Show Wireframe" toggle): a wire pass drawn IN ADDITION to the filled
    /// geometry, never instead of it.
    pub wireframe: bool,
    pub last_viewport_wireframe: bool,
    /// "Wire Single Color" toggle: on, the wires draw in `wire_color`; off,
    /// they carry the geometry's vertex colors unlit — brighter than the lit
    /// fill beneath, which is what separates them.
    pub wire_single_color: bool,
    /// RGBA: the alpha channel is the wireframe's OWN opacity in both color
    /// modes — the geometry Opacity slider affects only the polygons.
    pub wire_color: [f32; 4],
    /// Wire line width in framebuffer pixels ("Wire Thickness" slider).
    pub wire_width: f32,
    pub last_viewport_wire_single_color: bool,
    pub last_viewport_wire_color: [f32; 4],
    pub last_viewport_wire_width: f32,
    /// Opacity of the rendered node geometry (the Render node's "Opacity"
    /// slider): 1.0 opaque, straight-alpha blended toward the viewport bg.
    pub geo_opacity: f32,
    pub last_viewport_geo_opacity: f32,
    /// Point display of the node geometry (the Render node's "Render Points"
    /// toggle): one small octahedron per distinct vertex, sized by
    /// "Point Size" and tinted by "Point Color".
    pub render_points: bool,
    pub point_size: f32,
    pub point_color: [f32; 3],
    /// (geometry version, quantized size, color) the points mesh was last
    /// built from; `point_vertex_count` gates the draw.
    pub last_points_key: Option<(u64, i32, [u8; 3])>,
    pub point_vertex_count: u32,
    pub last_viewport_render_points: bool,
    pub last_viewport_point_size: f32,
    pub last_viewport_point_color: [f32; 3],
    /// Selected-Group membership markers: marker vertices staged CPU-side by
    /// `sync_nodes` whenever the selection is a Group node (empty otherwise),
    /// flushed to `meshes.group_points`; `group_point_vertex_count` gates the
    /// draw. The key — (node id, params, geometry version, quantized point
    /// size) — spares the re-evaluation on unrelated `sync_nodes` runs.
    pub group_point_verts: Vec<Vertex3D>,
    pub group_points_dirty: bool,
    pub group_point_vertex_count: u32,
    pub last_group_points_key: Option<(String, Vec<(String, String)>, u64, i32)>,
    /// Per-node meta (preferences) overlays, rebuilt with the scene: marker
    /// geometry for nodes whose meta asks for Point Markers, and (position,
    /// vertex index) labels for Point Numbers — the labels project through
    /// `last_scene_mvp` into 2D text each frame.
    pub meta_marker_verts: Vec<Vertex3D>,
    pub meta_points_dirty: bool,
    pub meta_point_count: u32,
    pub meta_number_labels: Vec<([f32; 3], u32)>,
    /// Per-node meta "Wireframe": LINE_LIST edge pairs of the flagged nodes'
    /// triangles, drawn as a wire pass over the scene fill.
    pub meta_wire_verts: Vec<Vertex3D>,
    pub meta_wire_count: u32,
    /// Per-node meta "Point Normals": LINE_LIST whiskers from each distinct
    /// point along its smooth vertex normal (computed from topology — the
    /// kernel outputs carry only a default up-normal attribute).
    pub meta_normal_verts: Vec<Vertex3D>,
    pub meta_normal_count: u32,
    /// World-unit radius of the meta "Point Markers" overlay — the Guides
    /// subnet's "Point Marker Size" control (stored there in thousandths).
    pub meta_marker_size: f32,
    /// sRGB color of the meta "Point Markers" overlay — the Guides subnet's
    /// "Point Marker Color" control (stored there as hex, like Grid Color).
    pub meta_marker_color: [f32; 3],
    /// The param pane's completion lists — (input node name, geometry
    /// version) → (group names, attribute names) read off that input's
    /// evaluated geometry, feeding the textpick rows on group/attribute
    /// params. One entry: the selected node's input.
    pub pick_cache: Option<((String, u64), (Vec<String>, Vec<String>))>,
    /// The raster scene's model-view-projection and the viewport pane rect in
    /// LOGICAL px, cached at staging so the 2D pass can project 3D overlays.
    pub last_scene_mvp: Option<Mat4>,
    pub last_scene_view_rect: (f32, f32, f32, f32),
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
    pub fn viewport(&self) -> &Viewport3D { self.slots.viewport() }

    pub fn viewport_mut(&mut self) -> &mut Viewport3D { self.slots.viewport_mut() }

    pub fn find_widget_index(&self, target_addr: *const ()) -> Option<usize> {
        self.slots.find_index(target_addr)
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

    // Typed roster accessors: the concrete-type asserts live on `WidgetSlots` (src/slots.rs);
    // these forward so the ~40 call sites keep reading `self.menu(..)` / `self.graph_mut()`.
    pub fn menubar_at(&self, idx: usize) -> Option<&MenuBar> { self.slots.menubar_at(idx) }

    pub fn menu(&self, idx: usize) -> &dyn cce_ui::widget::MenuController { self.slots.menu(idx) }

    pub fn menu_mut(&mut self, idx: usize) -> &mut dyn cce_ui::widget::MenuController { self.slots.menu_mut(idx) }

    pub fn graph(&self) -> &dyn cce_ui::widget::GraphController { self.slots.graph() }

    pub fn graph_mut(&mut self) -> &mut dyn cce_ui::widget::GraphController { self.slots.graph_mut() }

    pub fn param(&self) -> &dyn cce_ui::widget::ParamController { self.slots.param() }

    pub fn param_mut(&mut self) -> &mut dyn cce_ui::widget::ParamController { self.slots.param_mut() }

    pub fn spreadsheet_mut(&mut self) -> &mut dyn cce_ui::widget::SpreadsheetController { self.slots.spreadsheet_mut() }

    pub fn path_mut(&mut self) -> &mut dyn cce_ui::widget::PathController { self.slots.path_mut() }

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

        let main_node = self
            .session_node_mut()
            .and_then(|s| s.children.iter_mut().find(|c| c.name == "Main"));
        if let Some(main_node) = main_node {
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
            default_project: self.default_project_setting.clone(),
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

    /// The timeline frame the graph is evaluated at — what a simnet solves up to.
    pub fn sim_frame(&self) -> i32 {
        self.slots.playbar.inner().current_frame.round() as i32
    }

    /// The timeline's first frame: where every sim sits at its seed.
    pub fn sim_start_frame(&self) -> i32 {
        self.slots.playbar.inner().start_frame.round() as i32
    }

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

    /// Whether the cursor sits over the 3D viewport pane — the wheel arm's
    /// routing test, shared with `handle_pinch`.
    pub fn cursor_in_viewport(&self) -> bool {
        let node_area_y = self.positions[CONTENT_IDX].1;
        self.cursor_x >= self.content_right_x()
            && self.cursor_x < self.splitter_layout.splitter2_x
            && self.cursor_y >= node_area_y
            && self.cursor_y < self.height - STATUS_H
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

    /// The floating spreadsheet pane's rect (the single derivation the layout pass
    /// and the edge hotspots share). A positive side inset pulls that edge past its
    /// flush position into the neighbor's span — the pane tucks UNDER the neighbor,
    /// whose bottom the layout raises to the spreadsheet's top — so the height clamp
    /// keeps 100px of shortened neighbor above.
    pub fn pane_in_dock(&self, dock: Dock) -> usize {
        self.dock_panes[dock as usize]
    }

    pub fn dock_of_pane(&self, slot: usize) -> Option<Dock> {
        [Dock::Left, Dock::Right, Dock::Bottom]
            .into_iter()
            .find(|&d| self.dock_panes[d as usize] == slot)
    }

    /// Is the pane occupying a slot currently shown (its View toggle)?
    pub fn pane_shown(&self, slot: usize) -> bool {
        match slot {
            NETWORK_PANEL_IDX => self.show_network,
            PARAM_IDX => self.show_parameters,
            SPREADSHEET_IDX => self.show_spreadsheet,
            PLAYBAR_IDX => self.show_playbar,
            _ => false,
        }
    }

    fn dock_shown(&self, dock: Dock) -> bool {
        self.pane_shown(self.pane_in_dock(dock))
    }

    /// The dock a cursor position drops into: the lower band is the bottom
    /// strip, the rest splits into left and right halves.
    pub fn dock_region_at(&self, cx: f32, cy: f32) -> Dock {
        if cy > self.height * 0.62 {
            Dock::Bottom
        } else if cx < self.width * 0.5 {
            Dock::Left
        } else {
            Dock::Right
        }
    }

    /// A dock's current rect from the DOCK-owned dimensions — independent of
    /// whether its pane is shown, so the drag overlay can highlight it.
    pub fn dock_rect(&self, dock: Dock) -> (f32, f32, f32, f32) {
        let gap = 18.0_f32;
        let pb_off = if self.show_playbar { PLAYBAR_H + gap } else { 0.0 };
        match dock {
            Dock::Left => {
                let (fx, fy, fw, fh) = self.floating_network_layout;
                (fx.max(gap), fy.max(gap), fw, fh)
            }
            Dock::Right => {
                let param_w = self.floating_param_width.clamp(150.0, (self.width - 2.0 * gap).max(150.0));
                let h = (self.height - STATUS_H - pb_off - 2.0 * gap).max(100.0);
                (self.width - gap - param_w, gap, param_w, h)
            }
            Dock::Bottom => self.floating_spreadsheet_rect(),
        }
    }

    /// Move a plate to a dock, swapping with the pane that held it. The
    /// dock-owned dimensions stay put, so the geometry survives the swap.
    pub fn move_pane_to_dock(&mut self, slot: usize, dock: Dock) {
        let Some(from) = self.dock_of_pane(slot) else { return };
        if from == dock {
            return;
        }
        self.dock_panes.swap(from as usize, dock as usize);
        self.rebuild_positions();
        self.apply_layout();
        self.read_panel_offsets();
    }

    pub fn floating_spreadsheet_rect(&self) -> (f32, f32, f32, f32) {
        let gap = 18.0_f32;
        let fx = gap;
        let (_, _, mut fw, _) = self.floating_network_layout;
        fw = fw.clamp(150.0, (self.width - 2.0 * gap).max(150.0));
        let param_w = self.floating_param_width.clamp(150.0, (self.width - 2.0 * gap).max(150.0));
        let param_x = self.width - gap - param_w;
        let flush_left = if self.dock_shown(Dock::Left) { fx + fw + gap } else { gap };
        let flush_right = if self.dock_shown(Dock::Right) { param_x - gap } else { self.width - gap };
        let ss_x = (flush_left - self.floating_spreadsheet_inset_left.max(0.0)).max(gap);
        let ss_end = (flush_right + self.floating_spreadsheet_inset_right.max(0.0)).min(self.width - gap);
        let ss_w = (ss_end - ss_x).max(150.0);
        let pb_off = if self.show_playbar { PLAYBAR_H + gap } else { 0.0 };
        let ss_y_end = self.height - STATUS_H - pb_off - gap;
        let max_h = if self.spreadsheet_tucks_left() || self.spreadsheet_tucks_right() {
            ss_y_end - (gap + 100.0 + gap)
        } else {
            ss_y_end - gap
        };
        let ss_h = self.floating_spreadsheet_height.clamp(100.0, max_h.max(100.0));
        let ss_y = ss_y_end - ss_h;
        (ss_x, ss_y, ss_w, ss_h)
    }

    /// Whether the spreadsheet is tucked under the network / parameter pane
    /// (side inset active while both panes are shown).
    pub fn spreadsheet_tucks_left(&self) -> bool {
        self.dock_shown(Dock::Bottom) && self.dock_shown(Dock::Left) && self.floating_spreadsheet_inset_left > 0.5
    }

    pub fn spreadsheet_tucks_right(&self) -> bool {
        self.dock_shown(Dock::Bottom) && self.dock_shown(Dock::Right) && self.floating_spreadsheet_inset_right > 0.5
    }

    /// The spreadsheet pane's edge-resize hotspot at (cx, cy): top resizes the
    /// pane's own height; the left/right edges drive the neighboring pane's width
    /// (network / parameters), so each exists only while that neighbor is shown
    /// to make room. `None` off every edge.
    pub fn spreadsheet_resize_edge_at(&self, cx: f32, cy: f32) -> Option<ResizeDirection> {
        if !self.show_spreadsheet {
            return None;
        }
        let (ss_x, ss_y, ss_w, ss_h) = self.floating_spreadsheet_rect();
        let margin = 8.0_f32;
        let in_v = cy >= ss_y - margin && cy <= ss_y + ss_h + margin;
        if self.show_network && !self.circular_network_pane && in_v && cx >= ss_x - margin && cx <= ss_x + margin {
            return Some(ResizeDirection { left: true, right: false, top: false, bottom: false });
        }
        if self.show_parameters && in_v && cx >= ss_x + ss_w - margin && cx <= ss_x + ss_w + margin {
            return Some(ResizeDirection { left: false, right: true, top: false, bottom: false });
        }
        if cx >= ss_x && cx <= ss_x + ss_w && cy >= ss_y - margin && cy <= ss_y + margin {
            return Some(ResizeDirection { left: false, right: false, top: true, bottom: false });
        }
        None
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
                AppDrag::DockDrag { .. } => CursorIcon::Grabbing,
                AppDrag::SpreadsheetResize { .. } => CursorIcon::NsResize,
                AppDrag::SpreadsheetResizeLeft { .. } | AppDrag::SpreadsheetResizeRight { .. } => {
                    CursorIcon::EwResize
                }
            });
        }
        if let Some(dir) = self.network_resize_edge_at(cx, cy) {
            return Some(dir_cursor(dir));
        }
        if self.on_param_resize_edge(cx, cy) {
            return Some(CursorIcon::EwResize);
        }
        if let Some(dir) = self.spreadsheet_resize_edge_at(cx, cy) {
            return Some(dir_cursor(dir));
        }
        None
    }

    pub fn clamp_splitters(&mut self) {
        if self.is_detached_network {
            return;
        }
        self.splitter_layout.clamp(self.width, self.detached_circular_network);
    }

    /// The Session node: the permanent root container for the session-wide
    /// settings nodes (Main/View/Guides/Render). `ensure_menubar_subnets`
    /// guarantees it exists, so `None` only before the first ensure.
    pub fn session_node(&self) -> Option<&FsNode> {
        self.fs_root.children.iter().find(|c| c.node_type == "meta")
    }

    pub fn session_node_mut(&mut self) -> Option<&mut FsNode> {
        self.fs_root.children.iter_mut().find(|c| c.node_type == "meta")
    }

    /// Is the network currently inside a settings directory (the Session node
    /// or any utility node)? Geometry templates are refused there. Checks the
    /// whole path, not `current_path[0]` — the settings nodes live NESTED
    /// under Session now, so the old first-segment check would miss them.
    pub fn in_settings_dir(&self) -> bool {
        let mut node = &self.fs_root;
        for &idx in &self.current_path {
            match node.children.get(idx) {
                Some(child) => {
                    if matches!(child.node_type.as_str(), "utility" | "session" | "meta") {
                        return true;
                    }
                    node = child;
                }
                None => return false,
            }
        }
        false
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
                    // Params whose pane DISPLAY needs resetting without any
                    // action firing (the Open dropdown snapping back to
                    // "- Select -" — its action is file_to_open's, below).
                    let mut display_resets: Vec<String> = Vec::new();
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
                                    // Display reset ONLY — never into
                                    // triggered_buttons, whose entries get
                                    // executed as menu actions: "Open" there
                                    // opened the file chooser ON TOP of
                                    // loading the picked recent file.
                                    display_resets.push("Open".to_string());
                                }
                            }
                        }
                    }

                    if !triggered_buttons.is_empty() || !display_resets.is_empty() {
                        let mut disp_params = self.param().node_params();
                        for btn_name in triggered_buttons.iter().chain(display_resets.iter()) {
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
            "Set As Default" => {
                self.set_current_as_default();
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
        let live_main: [(&str, bool); 2] = [
            ("Circular Pane", self.circular_network_pane),
            ("Ray Traced Preview", self.viewport().rt_mode),
        ];
        let live_view: [(&str, bool); 5] = [
            ("Show Network Pane", self.show_network),
            ("Show Viewport Pane", self.show_viewport),
            ("Show Parameters Pane", self.show_parameters),
            ("Show Spreadsheet Pane", self.show_spreadsheet),
            ("Show Playbar Pane", self.show_playbar),
        ];
        let live_guides: [(&str, bool); 3] = [
            ("Show Grid Guide", self.viewport().show_grid),
            ("Show Reference Cube", self.viewport().show_cube),
            ("Show Origin Axes", self.viewport().show_origin),
        ];
        let dir = self.current_dir_mut();
        let Some(child) = dir.children.get_mut(slot_idx) else { return };
        let live: &[(&str, bool)] = match child.name.as_str() {
            "Main" => &live_main,
            "View" => &live_view,
            "Guides" => &live_guides,
            _ => return,
        };
        for &(name, on) in live {
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
        let params = self.add_pick_lists(params);
        self.param_mut().set_display_params(&params);
    }

    /// Upgrade a selected group/attribute node's group- and attribute-name
    /// text rows to `textpick` rows carrying the candidates read off the
    /// node's INPUT geometry (the Houdini attribute/group chooser). Rows stay
    /// plain text when there is no input, evaluation fails, or the list is
    /// empty — the picker degrades to nothing rather than an empty menu.
    fn add_pick_lists(
        &mut self,
        mut params: Vec<(String, String, String)>,
    ) -> Vec<(String, String, String)> {
        let (node_type, input_name) = {
            if self.is_detached_network {
                return params;
            }
            let Some(slot) = self.graph().selected_node() else { return params };
            let dir = self.current_dir();
            let Some(node) = dir.children.get(slot) else { return params };
            let nt = node.node_type.to_lowercase();
            if nt != "attribute" && nt != "group" && nt != "relax" {
                return params;
            }
            (nt, node_param_str(node, "Input", ""))
        };
        if input_name.is_empty() {
            return params;
        }
        let (groups, attrs) = self.input_pick_lists(&input_name);
        for row in params.iter_mut() {
            let list = match (node_type.as_str(), row.0.as_str()) {
                ("attribute", "Attribute Name") => &attrs,
                ("attribute", "Group") => &groups,
                ("group", "Group Name") => &groups,
                ("relax", "Pin Group") => &groups,
                _ => continue,
            };
            if row.2 == "text" && !list.is_empty() {
                row.2 = format!("textpick:{}", list.join(","));
            }
        }
        params
    }

    /// The (groups, attributes) present on `input_name`'s evaluated geometry,
    /// cached on (name, geometry version). Attribute names get the Pos/Col
    /// built-ins appended (the Attribute node can Modify them); names carrying
    /// a comma are dropped — they cannot ride the type spec-string.
    fn input_pick_lists(&mut self, input_name: &str) -> (Vec<String>, Vec<String>) {
        let key = (input_name.to_string(), self.rt_geometry_version);
        if let Some((k, lists)) = &self.pick_cache {
            if *k == key {
                return lists.clone();
            }
        }
        let (frame, start) = (self.sim_frame(), self.sim_start_frame());
        let mut groups = std::collections::BTreeSet::new();
        let mut attrs = std::collections::BTreeSet::new();
        let mut sim_cache = std::mem::take(&mut self.sim_cache);
        {
            let mut sim = crate::geometry::EvalSim::new(frame, start, &mut sim_cache);
            if let Some(input_node) = find_node_by_name(&self.fs_root, input_name) {
                let mut visited = Vec::new();
                let mut err = None;
                if let Some(geom) = generate_single_node_geometry_with_errors(
                    &self.fs_root,
                    input_node,
                    &mut visited,
                    &mut err,
                    &mut sim,
                ) {
                    for v in &geom.vertices {
                        for k in v.attributes.keys() {
                            if let Some(g) = k.strip_prefix("group:") {
                                if !g.contains(',') {
                                    groups.insert(g.to_string());
                                }
                            } else if !k.contains(',') {
                                attrs.insert(k.clone());
                            }
                        }
                    }
                }
            }
        }
        self.sim_cache = sim_cache;
        let mut attrs: Vec<String> = attrs.into_iter().collect();
        attrs.push("Pos".to_string());
        attrs.push("Col".to_string());
        let lists = (groups.into_iter().collect(), attrs);
        self.pick_cache = Some((key, lists.clone()));
        lists
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

    /// Where a file chooser should open: the loaded project's parent directory
    /// (the current working context — sibling projects live there), or None to
    /// let cce-files use its remembered location. Passing it also skips the
    /// chooser's remembered-dir restore outright.
    pub(crate) fn chooser_start_dir(&self) -> Option<std::path::PathBuf> {
        self.loaded_project_path
            .as_ref()
            .and_then(|p| p.parent())
            .filter(|d| d.is_dir())
            .map(|d| d.to_path_buf())
    }

    pub fn open_file_chooser(&self) {
        let Some(sender) = self.event_sender.clone() else { return };
        let start_dir = self.chooser_start_dir();
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
                .args(start_dir.as_deref())
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
        let start_dir = self.chooser_start_dir();
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
                .args(start_dir.as_deref())
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
        let in_utility = self.in_settings_dir();
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

    /// Open the node right-click context menu at the cursor for `slot`. The
    /// items are contextual: Enter (dive into the subnet) for enterable nodes,
    /// Show/Hide Geometry for non-utility nodes, and Delete always.
    fn open_node_context_menu(&mut self, slot: usize) {
        let (is_utility, geom_visible, enterable) = {
            let dir = self.current_dir();
            let Some(node) = dir.children.get(slot) else { return };
            let enterable = node.is_enterable();
            (
                matches!(node.node_type.as_str(), "utility" | "session" | "meta"),
                node.geometry_visible,
                enterable,
            )
        };
        let deletable = {
            let dir = self.current_dir();
            dir.children
                .get(slot)
                .map(|n| !matches!(n.node_type.as_str(), "session" | "meta"))
                .unwrap_or(false)
        };
        let mut options: Vec<String> = Vec::new();
        let mut actions: Vec<NodeMenuAction> = Vec::new();
        if enterable {
            options.push("Enter".to_string());
            actions.push(NodeMenuAction::Enter);
        }
        if !is_utility {
            options.push(if geom_visible { "Hide Geometry" } else { "Show Geometry" }.to_string());
            actions.push(NodeMenuAction::ToggleGeometry);
        }
        if deletable {
            options.push("Delete".to_string());
            actions.push(NodeMenuAction::Delete);
        }

        let target = self.slots.get_dyn(CONTENT_IDX).base().id();
        cce_ui::widget::context_menu::show(self.cursor_x, self.cursor_y, options, 0, target);
        self.node_menu_slot = Some(slot);
        self.node_menu_actions = actions;
    }

    fn node_menu_open(&self) -> bool {
        cce_ui::widget::context_menu::is_visible() && self.node_menu_slot.is_some()
    }

    fn close_node_menu(&mut self) {
        cce_ui::widget::context_menu::hide();
        self.node_menu_slot = None;
        self.node_menu_actions.clear();
    }

    /// Route a left press while the node menu is open. A press ON the menu is
    /// always consumed (running the clicked item, if any); a press outside
    /// dismisses it and falls through to normal handling.
    fn handle_node_menu_click(&mut self) -> bool {
        if !self.node_menu_open() {
            return false;
        }
        if cce_ui::widget::context_menu::hit_test(self.cursor_x, self.cursor_y) {
            let my = cce_ui::widget::context_menu::y();
            let idx = ((self.cursor_y - my) / 24.0).floor() as usize;
            let picked = self.node_menu_slot.zip(self.node_menu_actions.get(idx).copied());
            self.close_node_menu();
            if let Some((slot, action)) = picked {
                self.dispatch_node_menu(slot, action);
            }
            return true;
        }
        self.close_node_menu();
        false
    }

    /// "Frame All": move the active camera so the visible node geometry
    /// fills the viewport. The bounding sphere of the geometry becomes the
    /// camera pivot, and the camera slides along its EXISTING base direction
    /// (Position relative to Pivot — the view direction is preserved because
    /// get_matrices derives yaw0/pitch0 from that offset) to the distance
    /// where the sphere spans the narrower FOV axis. Viewport zoom resets to
    /// 1 so the distance is authoritative. For the Default Camera (no node
    /// to write) only the zoom is fitted — its pivot is fixed at the origin.
    pub fn frame_all(&mut self) {
        if self.rt_sphere_verts.is_empty() {
            return;
        }
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for v in &self.rt_sphere_verts {
            let p = Vec3::from_array(v.position);
            min = min.min(p);
            max = max.max(p);
        }
        let center = (min + max) * 0.5;
        let mut radius = 0.0f32;
        for v in &self.rt_sphere_verts {
            radius = radius.max((Vec3::from_array(v.position) - center).length());
        }
        let radius = radius.max(0.05);

        // Fit the bounding sphere inside the narrower frustum axis
        // (get_matrices: vertical FOV 0.9 rad), with a little breathing room.
        let (w, h) = (self.last_viewport_width.max(1) as f32, self.last_viewport_height.max(1) as f32);
        let aspect = if self.square_viewport { 1.0 } else { w / h };
        let half_v = 0.45f32;
        let half_h = (half_v.tan() * aspect).atan();
        let half = half_v.min(half_h);
        // 1.25: the sphere spans ~80% of the narrow axis — snug without
        // touching the pane edges.
        let dist = (radius / half.sin()) * 1.25;

        if self.active_camera == "Default Camera" {
            // Fixed eye ray through the origin — fit with zoom alone.
            let base_len = Vec3::new(2.5, 1.8, 2.5).length();
            self.viewport_mut().zoom = (dist / base_len).clamp(0.05, 20.0);
            self.viewport_mut().reset_velocity();
        } else {
            let camera_name = self.active_camera.clone();
            let dir = self.current_dir_mut();
            if let Some(node) = dir.children.iter_mut().find(|c| c.node_type == "camera" && c.name == camera_name) {
                let parse3 = |s: &str| -> Option<Vec3> {
                    let parts: Vec<&str> = s
                        .split(|c| c == ':' || c == ',' || c == ' ')
                        .filter(|s| !s.is_empty())
                        .collect();
                    if parts.len() >= 3 {
                        if let (Ok(x), Ok(y), Ok(z)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>(), parts[2].parse::<f32>()) {
                            return Some(Vec3::new(x, y, z));
                        }
                    }
                    None
                };
                let pos = node.params.iter().find(|p| p.name == "Position")
                    .and_then(|p| parse3(&p.default))
                    .unwrap_or(Vec3::new(2.5, 1.8, 2.5));
                let piv = node.params.iter().find(|p| p.name == "Pivot")
                    .and_then(|p| parse3(&p.default))
                    .unwrap_or(Vec3::ZERO);
                let offset = pos - piv;
                let dir_unit = if offset.length() > 1e-4 {
                    offset.normalize()
                } else {
                    Vec3::new(2.5, 1.8, 2.5).normalize()
                };
                let new_pos = center + dir_unit * dist;
                let fmt3 = |v: Vec3| format!("{:.2}:{:.2}:{:.2}", v.x, v.y, v.z);
                if let Some(p) = node.params.iter_mut().find(|p| p.name == "Pivot") {
                    p.default = fmt3(center);
                }
                if let Some(p) = node.params.iter_mut().find(|p| p.name == "Position") {
                    p.default = fmt3(new_pos);
                }
                self.viewport_mut().zoom = 1.0;
                self.viewport_mut().reset_velocity();
            }
        }
        self.viewport_dirty = true;
        self.sync_parameters_pane();
    }

    /// Open the viewport right-click context menu at the cursor.
    fn open_viewport_context_menu(&mut self) {
        let options = vec!["Frame All".to_string()];
        let actions = vec![ViewportMenuAction::FrameAll];
        let target = self.slots.viewport.id();
        cce_ui::widget::context_menu::show(self.cursor_x, self.cursor_y, options, 0, target);
        self.viewport_menu_active = true;
        self.viewport_menu_actions = actions;
    }

    fn viewport_menu_open(&self) -> bool {
        cce_ui::widget::context_menu::is_visible() && self.viewport_menu_active
    }

    fn close_viewport_menu(&mut self) {
        cce_ui::widget::context_menu::hide();
        self.viewport_menu_active = false;
        self.viewport_menu_actions.clear();
    }

    /// Route a left press while the viewport menu is open — same contract as
    /// `handle_node_menu_click`.
    fn handle_viewport_menu_click(&mut self) -> bool {
        if !self.viewport_menu_open() {
            return false;
        }
        if cce_ui::widget::context_menu::hit_test(self.cursor_x, self.cursor_y) {
            let my = cce_ui::widget::context_menu::y();
            let idx = ((self.cursor_y - my) / 24.0).floor() as usize;
            let picked = self.viewport_menu_actions.get(idx).copied();
            self.close_viewport_menu();
            if let Some(action) = picked {
                match action {
                    ViewportMenuAction::FrameAll => {
                        self.frame_all();
                    }
                }
            }
            return true;
        }
        self.close_viewport_menu();
        false
    }

    fn dispatch_node_menu(&mut self, slot: usize, action: NodeMenuAction) {
        match action {
            NodeMenuAction::Enter => {
                if slot < self.current_dir().children.len() {
                    self.current_path.push(slot);
                    self.on_path_changed();
                }
            }
            NodeMenuAction::ToggleGeometry => {
                let mut redraw = false;
                let _ = self.apply_action(McpAction::ToggleGeometry { slot }, &mut redraw);
            }
            NodeMenuAction::Delete => {
                self.delete_node(slot);
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
            // The camera's base pitch above the horizon (Position vs Pivot): the
            // Rotation.x clamp below is on the TOTAL pitch, matching get_matrices'
            // pitch0 + rx composition.
            let parse3 = |s: &str| -> Option<Vec3> {
                let parts: Vec<&str> = s
                    .split(|c| c == ':' || c == ',' || c == ' ')
                    .filter(|s| !s.is_empty())
                    .collect();
                if parts.len() >= 3 {
                    if let (Ok(x), Ok(y), Ok(z)) = (parts[0].parse::<f32>(), parts[1].parse::<f32>(), parts[2].parse::<f32>()) {
                        return Some(Vec3::new(x, y, z));
                    }
                }
                None
            };
            let pos = node.params.iter().find(|p| p.name == "Position")
                .and_then(|p| parse3(&p.default))
                .unwrap_or(Vec3::new(2.5, 1.8, 2.5));
            let piv = node.params.iter().find(|p| p.name == "Pivot")
                .and_then(|p| parse3(&p.default))
                .unwrap_or(Vec3::ZERO);
            let offset = pos - piv;
            let pitch0_deg = (offset.y / offset.length().max(1e-5)).asin().to_degrees();
            let max_pitch_deg = crate::viewport_3d::Viewport3D::MAX_PITCH.to_degrees();

            if let Some(p) = node.params.iter_mut().find(|p| p.name == "Rotation") {
                let mut rx = 0.0f32;
                let mut ry = 0.0f32;
                let mut rz = 0.0f32;
                if let Some(v) = parse3(&p.default) {
                    rx = v.x;
                    ry = v.y;
                    rz = v.z;
                }
                ry += d_yaw.to_degrees();
                // Clamp the orbit short of the poles: past ±90° total pitch the
                // up-vector flips and orbiting reads as the geometry tumbling.
                rx = (rx - d_pitch.to_degrees())
                    .clamp(-max_pitch_deg - pitch0_deg, max_pitch_deg - pitch0_deg);
                while ry > 180.0 { ry -= 360.0; }
                while ry < -180.0 { ry += 360.0; }
                p.default = format!("{:.2}:{:.2}:{:.2}", rx, ry, rz);

                self.sync_nodes();
                // Through the one pane-sync path, so the pick-list upgrade
                // (textpick rows) survives this rebuild.
                self.sync_parameters_pane();

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
                // Through the one pane-sync path, so the pick-list upgrade
                // (textpick rows) survives this rebuild.
                self.sync_parameters_pane();
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
        // The Session node is permanent, and so is each node's meta
        // (preferences) child: every deletion route (context menu, Delete
        // key, MCP) funnels through here, so this is the one gate.
        if slot < len
            && matches!(self.current_dir().children[slot].node_type.as_str(), "session" | "meta")
        {
            return false;
        }
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


        let sim_frame = self.sim_frame();
        let sim_start = self.sim_start_frame();
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

        // Both refresh passes below evaluate against `selected_node`, which
        // borrows self — so each computes OWNED results here and the mutation
        // tail applies them once the borrow is dead.
        let mut spreadsheet_update = None;
        if self.show_spreadsheet && !cache_hit {
            let mut headers = Vec::new();
            let mut rows = Vec::new();

            if let Some(node) = selected_node {
                let mut visited = Vec::new();
                let mut ocl_error = None;
                // A throwaway cache: `selected_node` borrows self, so the
                // shared one cannot be reached from here. The answer is the
                // same either way — a simnet just re-solves for the
                // spreadsheet, which only runs when the selection changed.
                let mut sim_cache = crate::geometry::SimCache::default();
                let mut sim = crate::geometry::EvalSim::new(sim_frame, sim_start, &mut sim_cache);
                if let Some(geom) = generate_single_node_geometry_with_errors(&self.fs_root, node, &mut visited, &mut ocl_error, &mut sim) {
                    let (h, r) = Self::geometry_to_spreadsheet_data(&geom);
                    headers = h;
                    rows = r;
                }
            }
            spreadsheet_update = Some((headers, rows));
        }

        // Selected-Group viewport markers: while the selection is a Group
        // node, evaluate it and stage a marker at every vertex it tags, so
        // selecting the node SHOWS the group in the viewport — independent of
        // the Highlight bake and of which node holds the display flag. The
        // geometry version keeps the key honest against upstream edits (the
        // scene rebuild bumps it); a non-group selection clears the markers.
        let group_key = selected_node.filter(|n| n.node_type.eq_ignore_ascii_case("group")).map(|n| {
            (
                n.id.clone(),
                n.params.iter().map(|p| (p.name.clone(), p.default.clone())).collect::<Vec<_>>(),
                self.rt_geometry_version,
                (self.point_size * 1000.0).round() as i32,
            )
        });
        let mut group_update = None;
        if group_key != self.last_group_points_key {
            let mut marker_verts = Vec::new();
            if let Some(node) = selected_node.filter(|n| n.node_type.eq_ignore_ascii_case("group")) {
                let group_name = node_param_str(node, "Group Name", "group1");
                let mut visited = Vec::new();
                let mut ocl_error = None;
                // Throwaway sim cache, as for the spreadsheet above.
                let mut sim_cache = crate::geometry::SimCache::default();
                let mut sim = crate::geometry::EvalSim::new(sim_frame, sim_start, &mut sim_cache);
                if let Some(geom) = generate_single_node_geometry_with_errors(&self.fs_root, node, &mut visited, &mut ocl_error, &mut sim) {
                    let members = crate::geometry::group_member_positions(&geom, &group_name);
                    // The Highlight bake's warm accent, so the markers and the
                    // tint read as one feature. Slightly larger than the
                    // Render node's points so both stay legible together.
                    marker_verts = crate::geometry::points_vertices(
                        &members,
                        self.point_size * 1.25,
                        cce_ui::colors::to_linear_rgb([1.0, 0.78, 0.20]),
                    );
                }
            }
            group_update = Some(marker_verts);
        }

        if let Some((headers, rows)) = spreadsheet_update {
            self.spreadsheet_mut().set_spreadsheet_data(headers, rows);
            self.last_spreadsheet_node_name = current_name;
            self.last_spreadsheet_node_params = current_params;
        }
        if let Some(marker_verts) = group_update {
            self.group_point_verts = marker_verts;
            self.group_points_dirty = true;
            self.last_group_points_key = group_key;
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
                if let Ok(mut proj) = serde_json::from_str::<Project>(&content) {
                    merge_template_defs(&mut proj.root, &node_templates);
                    ensure_meta_children(&mut proj.root);
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
            network_panel: PassivePlate::new(),
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
            register("save_document_as", "Ctrl+Shift+s", Action::SaveAs);
            register("next_context", "Ctrl+Tab", Action::NextContext);
            register("previous_context", "Ctrl+Shift+Tab", Action::PrevContext);
            register("play_pause", "Up", Action::PlayPause);
            register("frame_next", "Right", Action::FrameNext);
            register("frame_prev", "Left", Action::FramePrev);
        }

        let mut state = Self {
            title: String::new(),
            meshes: None,
            pending_grid: None,
            pending_origin: None,
            pending_pivot: None,
            pending_viewport_bg: None,
            spheres_dirty: false,
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
            node_menu_slot: None,
            node_menu_actions: Vec::new(),
            viewport_menu_active: false,
            viewport_menu_actions: Vec::new(),
            sim_cache: crate::geometry::SimCache::default(),
            last_sim_frame: i32::MIN,
            plate_menu_slot: None,
            plate_menu_actions: Vec::new(),
            collapsed_panes: [false; WIDGET_COUNT],
            corner_press: None,
            dock_panes: [NETWORK_PANEL_IDX, PARAM_IDX, SPREADSHEET_IDX],
            dock_drag_target: None,
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
            detached_pane: None,
            detached_panes: [false; WIDGET_COUNT],
            detached_children: std::collections::HashMap::new(),
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
            floating_spreadsheet_inset_left: 0.0,
            floating_spreadsheet_inset_right: 0.0,
            loaded_project_path: None,
            default_project_setting: settings.default_project.clone(),
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
            wire_single_color: false,
            wire_color: [1.0, 1.0, 1.0, 1.0],
            wire_width: 1.0,
            last_viewport_wire_single_color: false,
            last_viewport_wire_color: [1.0, 1.0, 1.0, 1.0],
            last_viewport_wire_width: 1.0,
            geo_opacity: 1.0,
            last_viewport_geo_opacity: 1.0,
            render_points: false,
            point_size: 0.02,
            point_color: [1.0, 1.0, 1.0],
            last_points_key: None,
            point_vertex_count: 0,
            last_viewport_render_points: false,
            last_viewport_point_size: 0.0,
            last_viewport_point_color: [0.0, 0.0, 0.0],
            group_point_verts: Vec::new(),
            group_points_dirty: false,
            group_point_vertex_count: 0,
            last_group_points_key: None,
            meta_marker_verts: Vec::new(),
            meta_points_dirty: false,
            meta_point_count: 0,
            meta_number_labels: Vec::new(),
            meta_wire_verts: Vec::new(),
            meta_wire_count: 0,
            meta_normal_verts: Vec::new(),
            meta_normal_count: 0,
            meta_marker_size: 0.02,
            meta_marker_color: [0.85, 0.85, 1.0],
            pick_cache: None,
            last_scene_mvp: None,
            last_scene_view_rect: (0.0, 0.0, 0.0, 0.0),
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
                let mut fh = (self.height - STATUS_H - pb_off - 2.0 * gap).max(100.0);
                self.floating_network_layout = (fx, fy, fw, fh);

                let param_w = self.floating_param_width.clamp(150.0, (self.width - paginator_w - 2.0 * gap).max(150.0));
                self.floating_param_width = param_w;
                let param_x = self.width - gap - param_w;
                let param_y = gap;
                let mut param_h = (self.height - STATUS_H - pb_off - 2.0 * gap).max(100.0);

                // The spreadsheet rect (shared derivation with the edge hotspots —
                // reads the clamped network width written back above). A side inset
                // tucks the spreadsheet UNDER that neighbor: the neighbor's bottom
                // rises to the spreadsheet's top edge to make room.
                let (ss_x, ss_y, ss_w, ss_h) = self.floating_spreadsheet_rect();
                self.floating_spreadsheet_height = ss_h;
                if self.spreadsheet_tucks_left() {
                    fh = (ss_y - gap - fy).max(100.0);
                    self.floating_network_layout.3 = fh;
                }
                if self.spreadsheet_tucks_right() {
                    param_h = (ss_y - gap - param_y).max(100.0);
                }

                let viewport_visible = self.show_viewport;
                let spreadsheet_visible = self.show_spreadsheet;
                let right_visible = self.show_parameters;

                let col_c_x = paginator_w;
                let col_c_w = self.width - paginator_w;
                let vp_y = 0.0;
                let vp_h = if viewport_visible { body_h } else { 0.0 };

                // Dock model: the three rects computed above belong to the
                // DOCKS (left column, right column, bottom strip); which pane
                // wears which rect is the dock assignment, swapped by dragging
                // a plate's corner dot. A hidden pane zeroes its rect wherever
                // it is docked.
                let dock_rects = [
                    (fx, fy, fw, fh),
                    (param_x, param_y, param_w, param_h),
                    (ss_x, ss_y, ss_w, ss_h),
                ];
                let rect_for = |slot: usize, state: &Self| -> (f32, f32, f32, f32) {
                    if !state.pane_shown(slot) {
                        return (0.0, 0.0, 0.0, 0.0);
                    }
                    match state.dock_of_pane(slot) {
                        Some(d) => dock_rects[d as usize],
                        None => (0.0, 0.0, 0.0, 0.0),
                    }
                };

                let (px, py, pw, ph) = rect_for(NETWORK_PANEL_IDX, self);

                if let Some(menubar) = self.slots.left_menubar.as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
                    menubar.set_curved_circle(None);
                }
                
                let mb_h = 0.0;
                let bc_h = if self.show_network { BREADCRUMB_H } else { 0.0 };
                // The breadcrumb HOVERS over the graph: its strip rect stays
                // (the widget lays its run out in it, and hit() claims only
                // the segments), but the content spans the full plate — no
                // reserved titlebar band above the cells.
                let content_h = (ph - mb_h).max(0.0);

                self.positions[CONTENT_IDX] = (px, py + mb_h, pw, content_h);
                self.positions[NETWORK_PANEL_IDX] = (px, py, pw, ph);
                self.slots.network_panel.set_rect(px, py, pw, ph);
                if let Some(plate) = self.slots.network_panel.as_any_mut().downcast_mut::<PassivePlate>() {
                    plate.set_curved_circle(None);
                }
                self.positions[SPLITTER1_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPLITTER2_IDX] = (0.0, 0.0, 0.0, 0.0);
                let p_rect = rect_for(PARAM_IDX, self);
                self.positions[PARAM_IDX] = p_rect;
                self.slots.param.set_rect(p_rect.0, p_rect.1, p_rect.2, p_rect.3);

                self.positions[VIEWPORT_IDX] = (col_c_x, vp_y, col_c_w, vp_h);
                self.positions[RIGHT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[BREADCRUMB_IDX] = (px, py + mb_h, pw, bc_h);
                self.positions[LEFT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);

                self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPREADSHEET_IDX] = rect_for(SPREADSHEET_IDX, self);
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

        self.apply_detached_panes();
        self.apply_collapsed_panes();
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
                    match std::process::Command::new(std::env::current_exe().unwrap())
                        .arg("--detached-network")
                        .spawn()
                    {
                        Ok(child) => {
                            self.detached_children.insert(NETWORK_PANEL_IDX, child);
                        }
                        Err(e) => eprintln!("Failed to spawn detached network: {e:?}"),
                    }
                } else {
                    self.detached_children.remove(&NETWORK_PANEL_IDX);
                }

                self.rebuild_positions();
                self.apply_layout();
                self.sync_grid_settings();
            }
            Action::SaveAs => {
                self.save_file_chooser();
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
            Action::PlayPause => {
                let pb = self.slots.playbar.inner_mut();
                pb.playing = !pb.playing;
            }
            // Whole-frame stepping off the ROUNDED current frame: during
            // playback the playhead sits between frames, and stepping from
            // the fractional value would land off the frame grid. The scene
            // rebuild follows from tick_frame's last_sim_frame diff.
            Action::FrameNext | Action::FramePrev => {
                let step = if action == Action::FrameNext { 1.0 } else { -1.0 };
                let pb = self.slots.playbar.inner_mut();
                pb.current_frame = (pb.current_frame.round() + step).clamp(pb.start_frame, pb.end_frame);
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

                let in_viewport = self.cursor_in_viewport();

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
                } else if in_viewport && self.modifiers.control_key() {
                    // Ctrl+wheel zoom over the 3D viewport. The routed pass
                    // above deliberately skips wheels while ctrl is held, so
                    // hand the event to the viewport widget here — its ctrl
                    // branch is the camera zoom. (Trackpad pinches take the
                    // direct `handle_pinch` path and never reach this.)
                    self.slots.get_dyn_mut(VIEWPORT_IDX).set_modifiers(
                        self.modifiers.control_key(),
                        self.modifiers.shift_key(),
                        self.modifiers.alt_key(),
                    );
                    let wheel_ev = cce_ui::widget::Event::MouseWheel {
                        delta: *delta,
                        x: self.cursor_x,
                        y: self.cursor_y,
                        local_x: self.cursor_x,
                        local_y: self.cursor_y,
                    };
                    let vp = self.slots.viewport.id();
                    self.ui_context.propagate_event(&wheel_ev, vp)
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

                // Track hover on the node/viewport context menus so the
                // highlight follows.
                if (self.node_menu_open() || self.viewport_menu_open())
                    && cce_ui::widget::context_menu::cursor_moved(self.cursor_x, self.cursor_y)
                {
                    changed = true;
                }

                // An armed corner-dot press becomes a layout drag once it
                // moves; stubbed (collapsed/detached) panes stay click-only.
                if let Some((idx, px, py)) = self.corner_press {
                    if cce_ui::widget::plate_dock::press_becomes_drag(
                        (px, py),
                        (self.cursor_x, self.cursor_y),
                    ) {
                        self.corner_press = None;
                        if !self.pane_is_stubbed(idx) && idx != PLAYBAR_IDX {
                            self.app_drag = Some(AppDrag::DockDrag { idx });
                            self.dock_drag_target = Some(self.dock_region_at(self.cursor_x, self.cursor_y));
                            if std::env::var("CCE_DOCK_DEBUG").is_ok() {
                                eprintln!("[dock] drag start slot={idx} target={:?}", self.dock_drag_target);
                            }
                        }
                    }
                }

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
                            AppDrag::SpreadsheetResizeLeft { start_inset, start_mouse_x } => {
                                // Dragging the left edge past its flush position tucks the
                                // spreadsheet under the network pane (the layout raises the
                                // pane's bottom to make room); back to flush un-tucks it.
                                let dx = self.cursor_x - start_mouse_x;
                                let gap = 18.0_f32;
                                let (fx, _, fw, _) = self.floating_network_layout;
                                let flush_left = fx + fw + gap;
                                let max_inset = (flush_left - gap).max(0.0);
                                self.floating_spreadsheet_inset_left =
                                    (start_inset - dx).clamp(0.0, max_inset);
                                self.rebuild_positions();
                                self.apply_layout();
                                self.sync_grid_settings();
                                changed = true;
                            }
                            AppDrag::DockDrag { idx } => {
                                let _ = idx;
                                let target = self.dock_region_at(self.cursor_x, self.cursor_y);
                                if std::env::var("CCE_DOCK_DEBUG").is_ok() {
                                    eprintln!("[dock] motion ({:.0},{:.0}) -> {target:?}", self.cursor_x, self.cursor_y);
                                }
                                if self.dock_drag_target != Some(target) {
                                    self.dock_drag_target = Some(target);
                                    changed = true;
                                }
                            }
                            AppDrag::SpreadsheetResizeRight { start_inset, start_mouse_x } => {
                                // The right edge tucks under the parameter pane, symmetrically.
                                let dx = self.cursor_x - start_mouse_x;
                                let gap = 18.0_f32;
                                let param_x = self.width - gap - self.floating_param_width;
                                let flush_right = param_x - gap;
                                let max_inset = (self.width - gap - flush_right).max(0.0);
                                self.floating_spreadsheet_inset_right =
                                    (start_inset + dx).clamp(0.0, max_inset);
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
                        // The node context menu takes the first shot at a
                        // press: a left click on it runs the item; any other
                        // press (or a left click outside) dismisses it and
                        // falls through to normal handling.
                        if self.node_menu_open() {
                            if *button == MouseButton::Left && self.handle_node_menu_click() {
                                return true;
                            }
                            self.close_node_menu();
                        }
                        // The plate corner menu, same contract as the node
                        // menu above: it took the click, or it is dismissed.
                        if self.plate_menu_open() {
                            if *button == MouseButton::Left && self.handle_plate_menu_click() {
                                return true;
                            }
                            self.close_plate_menu();
                            if *button == MouseButton::Left {
                                return true;
                            }
                        }
                        // A press ON a corner control arms click-vs-drag: a
                        // clean release opens the menu, motion past the
                        // threshold becomes a layout drag. Either way the
                        // press never reaches the pane underneath.
                        if *button == MouseButton::Left {
                            if let Some(idx) = self.plate_corner_at(self.cursor_x, self.cursor_y) {
                                if std::env::var("CCE_DOCK_DEBUG").is_ok() {
                                    eprintln!("[dock] armed corner press slot={idx} at ({:.0},{:.0})", self.cursor_x, self.cursor_y);
                                }
                                self.corner_press = Some((idx, self.cursor_x, self.cursor_y));
                                return true;
                            }
                        }
                        if self.viewport_menu_open() {
                            if *button == MouseButton::Left && self.handle_viewport_menu_click() {
                                return true;
                            }
                            self.close_viewport_menu();
                            // Swallow the dismissing left press: it should not
                            // also orbit/click whatever sits underneath, and
                            // returning true forces the redraw that actually
                            // erases the menu — falling through here left the
                            // menu painted until the next incidental redraw.
                            if *button == MouseButton::Left {
                                return true;
                            }
                        }

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

                            if let Some(dir) = self.spreadsheet_resize_edge_at(cx, cy) {
                                self.app_drag = Some(if dir.left {
                                    AppDrag::SpreadsheetResizeLeft {
                                        start_inset: self.floating_spreadsheet_inset_left,
                                        start_mouse_x: cx,
                                    }
                                } else if dir.right {
                                    AppDrag::SpreadsheetResizeRight {
                                        start_inset: self.floating_spreadsheet_inset_right,
                                        start_mouse_x: cx,
                                    }
                                } else {
                                    let (_, _, _, ss_h) = self.floating_spreadsheet_rect();
                                    AppDrag::SpreadsheetResize {
                                        start_h: ss_h,
                                        start_mouse_y: cy,
                                    }
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
                            self.close_node_menu();
                            self.close_viewport_menu();
                            if self.cursor_in_viewport() && !in_circle_network_pane {
                                self.open_viewport_context_menu();
                                return true;
                            }
                            if in_circle_network_pane {
                                // On a node → its context menu; empty space →
                                // the add-node palette.
                                if let Some(slot) = self.graph().node_at(self.cursor_x, self.cursor_y) {
                                    self.graph_mut().set_selected_node(Some(slot));
                                    self.sync_parameters_pane();
                                    self.open_node_context_menu(slot);
                                    return true;
                                }
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
                                } else if i == BREADCRUMB_IDX {
                                    // Floats over the graph (the content spans the full plate);
                                    // its refined hit() claims only the segment run, so this
                                    // priority never shadows the graph elsewhere in the strip.
                                    1
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
                                    if dir_idx < dir.children.len() && dir.children[dir_idx].is_enterable() {
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
                                AppDrag::SpreadsheetResize { .. }
                                | AppDrag::SpreadsheetResizeLeft { .. }
                                | AppDrag::SpreadsheetResizeRight { .. } => SPREADSHEET_IDX,
                                AppDrag::DockDrag { idx } => idx,
                            };
                            // Dock drop: snap the dragged plate into the
                            // highlighted region, swapping occupants.
                            if let AppDrag::DockDrag { idx } = drag {
                                if let Some(target) = self.dock_drag_target.take() {
                                    if std::env::var("CCE_DOCK_DEBUG").is_ok() {
                                        eprintln!("[dock] drop slot={idx} into {target:?} (was {:?})", self.dock_of_pane(idx));
                                    }
                                    self.move_pane_to_dock(idx, target);
                                }
                            }
                            {
                                let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                                unsafe { (*ptr).handle_event(&cce_ui::widget::Event::DragEnd, &mut self.ui_context); }
                            }
                            self.sync_layout();
                            // Both of these resized the network pane, so the node
                            // offsets need the same refresh.
                            if matches!(
                                drag,
                                AppDrag::NetworkResize { .. }
                                    | AppDrag::SpreadsheetResizeLeft { .. }
                                    | AppDrag::DockDrag { .. }
                            ) {
                                self.read_panel_offsets();
                            }
                            changed = true;
                        }
                        // A corner-dot press that never became a drag is a
                        // click: open that plate's menu now, on release.
                        if let Some((idx, _, _)) = self.corner_press.take() {
                            self.open_plate_menu(idx);
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

                // Playbar transport dispatches ahead of every remaining key
                // path so the timeline answers from any pane ("all contexts").
                // It sits AFTER the param pane's chance on purpose: a focused
                // text/code field consumed the arrows above for its caret.
                if event.state == ElementState::Pressed {
                    if let Some(action @ (Action::PlayPause | Action::FrameNext | Action::FramePrev)) =
                        self.shortcut_manager.match_action(&self.modifiers, &event.logical_key)
                    {
                        self.execute_action(action);
                        return true;
                    }
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

                        // Grid-cursor movement is hjkl-only: the arrows belong
                        // to the playbar transport (dispatched above), in every
                        // pane and context.
                        match &event.logical_key {
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
                                                     if slot_idx < dir.children.len() && dir.children[slot_idx].is_enterable() {
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

        // A simnet's geometry is a function of the frame, so advancing the
        // timeline invalidates the scene the way editing a node does. Gated on
        // the graph actually containing one: without this, every frame of
        // playback would rebuild the scene for a graph that cannot have
        // changed.
        let frame_now = self.sim_frame();
        if frame_now != self.last_sim_frame {
            self.last_sim_frame = frame_now;
            if crate::geometry::contains_simnet(&self.fs_root) {
                self.rebuild_scene_geometry();
                self.viewport_dirty = true;
            }
        }

        // A detached window the user closed hands its pane back here, so a
        // closed window cannot strand the pane as a stub nothing can revive.
        let reclaimed = self.poll_detached_children();

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
                         self.default_project_setting = settings.default_project.clone();
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
        let mut param_ticked = false;
        let ctx = &mut self.ui_context;
        for i in 0..WIDGET_COUNT {
            if self.slots.get_dyn_mut(i).tick(dt, ctx) {
                tick_changed = true;
                if i == PARAM_IDX {
                    param_ticked = true;
                }
            }
        }
        // Live-streaming param widgets (the color picker's --stream lines)
        // change row values inside tick — push them through the same sync the
        // input path uses so they apply while the picker stays open.
        if param_ticked {
            self.sync_parameters_to_project();
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

        tick_changed || panned || reclaimed
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
            // Edge mesh for the wire pass: each triangle's three edges as
            // LINE_LIST vertex pairs, carrying the same colors.
            let mut edges = Vec::with_capacity(self.rt_sphere_verts.len() * 2);
            for tri in self.rt_sphere_verts.chunks_exact(3) {
                for (a, b) in [(0, 1), (1, 2), (2, 0)] {
                    edges.push(tri[a]);
                    edges.push(tri[b]);
                }
            }
            renderer.update_mesh(meshes.sphere_edges, bytemuck::cast_slice(&edges));
        }

        // The Render node's point display: rebuilt whenever the geometry or
        // the point params moved (the key), skipped entirely while off.
        if self.render_points {
            let key = (
                self.rt_geometry_version,
                (self.point_size * 1000.0).round() as i32,
                [
                    (self.point_color[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                    (self.point_color[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                    (self.point_color[2] * 255.0).round().clamp(0.0, 255.0) as u8,
                ],
            );
            if self.last_points_key != Some(key) {
                let verts = crate::geometry::points_vertices(
                    &self.rt_sphere_verts,
                    self.point_size,
                    cce_ui::colors::to_linear_rgb(self.point_color),
                );
                self.point_vertex_count = verts.len() as u32;
                renderer.update_mesh(meshes.points, bytemuck::cast_slice(&verts));
                self.last_points_key = Some(key);
                self.viewport_dirty = true;
            }
        }

        // Selected-Group markers, staged by sync_nodes.
        if self.group_points_dirty {
            self.group_points_dirty = false;
            renderer.update_mesh(meshes.group_points, bytemuck::cast_slice(&self.group_point_verts));
            self.group_point_vertex_count = self.group_point_verts.len() as u32;
            self.viewport_dirty = true;
        }

        // Per-node meta markers and wires, staged by rebuild_scene_geometry.
        if self.meta_points_dirty {
            self.meta_points_dirty = false;
            renderer.update_mesh(meshes.meta_points, bytemuck::cast_slice(&self.meta_marker_verts));
            self.meta_point_count = self.meta_marker_verts.len() as u32;
            renderer.update_mesh(meshes.meta_wires, bytemuck::cast_slice(&self.meta_wire_verts));
            self.meta_wire_count = self.meta_wire_verts.len() as u32;
            renderer
                .update_mesh(meshes.meta_normals, bytemuck::cast_slice(&self.meta_normal_verts));
            self.meta_normal_count = self.meta_normal_verts.len() as u32;
            self.viewport_dirty = true;
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
            sphere_edges: renderer.create_mesh(&[]),
            grid: renderer.create_mesh(bytemuck::cast_slice(&grid_verts)),
            origin: renderer.create_mesh(bytemuck::cast_slice(&origin_verts)),
            pivot: renderer.create_mesh(bytemuck::cast_slice(&pivot_verts)),
            points: renderer.create_mesh(&[]),
            group_points: renderer.create_mesh(&[]),
            meta_points: renderer.create_mesh(&[]),
            meta_wires: renderer.create_mesh(&[]),
            meta_normals: renderer.create_mesh(&[]),
        });
        // Scene geometry built during `State::new` (before the renderer
        // existed) uploads on the first frame's flush.
        self.spheres_dirty = !self.rt_sphere_verts.is_empty();
        self.viewport_dirty = true;
    }

    /// Frame staging (engine `stage_renderer` hook): pending meshes, text, and
    /// the 3D scene / RT pane. Returns true while the path tracer is still
    /// refining, to keep frames coming. The renderer's window-corner clip is
    /// left at the engine default (0) — the compositor rounds the window.
    pub fn stage_frame(&mut self, renderer: &mut cce_ui::vk::VkRenderer) -> bool {
        self.flush_pending_meshes(renderer);
        let meshes = self.meshes.expect("stage_frame before renderer_init");

        // 3D canvas: stage the scene into the renderer's backdrop when the
        // viewport is visible and its inputs changed; unstaged frames reuse the
        // previous backdrop (the renderer's equivalent of the old cached pass).
        // A detached pane's window has no 3D canvas: the scene would stage
        // behind the pane and show through its translucent plate.
        if !self.is_detached_network && self.detached_pane.is_none() && self.show_viewport {
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
                    || self.last_viewport_geo_opacity != self.geo_opacity
                    || self.last_viewport_wire_single_color != self.wire_single_color
                    || self.last_viewport_wire_color != self.wire_color
                    || self.last_viewport_wire_width != self.wire_width
                    || self.last_viewport_render_points != self.render_points
                    || self.last_viewport_point_size != self.point_size
                    || self.last_viewport_point_color != self.point_color;

                if viewport_changed {
                    if !rt_mode {
                    let aspect = cw as f32 / ch as f32;
                    let (proj, view_mat, model) = self.viewport().get_matrices(aspect, Some(camera_pos), Some(Vec3::new(rx, ry, rz)), Some(pivot));
                    let mvp_mat = proj * view_mat * model;
                    let mvp = mvp_mat.to_cols_array_2d();
                    // Cache for the 2D pass's 3D-overlay projection (point
                    // numbers): the matrix, and the pane rect back in logical
                    // px. Refreshed exactly when the camera/pane changes.
                    let s = self.scale as f32;
                    self.last_scene_mvp = Some(mvp_mat);
                    self.last_scene_view_rect =
                        (sx as f32 / s, sy as f32 / s, cw as f32 / s, ch as f32 / s);

                    // The camera-pivot marker is WORLD-FIXED at the pivot point, like
                    // the origin gizmo. Its old yaw rotation existed to keep it glued
                    // to the model-matrix orbit's rotating world; with the camera
                    // doing the moving it must not rotate, or its axis beams read as
                    // scene geometry spinning with the camera over the stationary grid.
                    let model_pivot = Mat4::from_translation(pivot);
                    let mvp_pivot = (proj * view_mat * model_pivot).to_cols_array_2d();

                    // Same draw order as the wgpu pass: bg quad, grid, origin,
                    // pivot, cube, spheres. The node geometry (points +
                    // spheres) carries the Render node's Opacity; scene
                    // furniture stays opaque.
                    const NO_TINT: [f32; 4] = [0.0; 4];
                    let geo_opacity = self.geo_opacity.clamp(0.0, 1.0);
                    let mut draws = vec![SceneDraw { mesh: meshes.viewport_bg, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 }];
                    if self.viewport().show_grid {
                        draws.push(SceneDraw { mesh: meshes.grid, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 });
                    }
                    if self.viewport().show_origin {
                        draws.push(SceneDraw { mesh: meshes.origin, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 });
                    }
                    if self.viewport().show_camera_pivot {
                        draws.push(SceneDraw { mesh: meshes.pivot, mvp: mvp_pivot, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 });
                    }
                    if self.viewport().show_cube {
                        draws.push(SceneDraw { mesh: meshes.cube, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 });
                    }
                    if self.render_points && self.point_vertex_count > 0 {
                        draws.push(SceneDraw { mesh: meshes.points, mvp, wireframe: false, wire_tint: NO_TINT, opacity: geo_opacity, line_width: 1.0, wire_base_width: 0.0 });
                    }
                    // Selected-Group markers: full-opacity selection feedback,
                    // deliberately outside the Render node's Opacity.
                    if self.group_point_vertex_count > 0 {
                        draws.push(SceneDraw { mesh: meshes.group_points, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 });
                    }
                    // Per-node meta "Point Markers", same full-opacity tier.
                    if self.meta_point_count > 0 {
                        draws.push(SceneDraw { mesh: meshes.meta_points, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 });
                    }
                    if self.vertex_count_spheres > 0 {
                        // With wires coming — the global toggle's or any
                        // node's meta Wireframe — the fill is pushed back by
                        // its slope-scaled offset so the lattice reads solid.
                        let base = if self.wireframe || self.meta_wire_count > 0 {
                            self.wire_width
                        } else {
                            0.0
                        };
                        draws.push(SceneDraw { mesh: meshes.spheres, mvp, wireframe: false, wire_tint: NO_TINT, opacity: geo_opacity, line_width: 1.0, wire_base_width: base });
                        if self.wireframe {
                            // The wire pass rides ON TOP of the fill (never
                            // replaces it). Single-color mode replaces the
                            // fragment color outright; geometry-color mode
                            // draws the raw vertex colors — the wire pass is
                            // unlit, so they read brighter than the shaded
                            // fill beneath. Far-side wires that clear the
                            // depth test near the limb show as their own
                            // (complementary) colors — a soft x-ray read.
                            // The wires' opacity is the Wire Color ALPHA in
                            // both modes; the geometry Opacity slider is
                            // polygons-only.
                            let tint = if self.wire_single_color {
                                [self.wire_color[0], self.wire_color[1], self.wire_color[2], 1.0]
                            } else {
                                [0.0, 0.0, 0.0, 0.0]
                            };
                            let wire_alpha = self.wire_color[3].clamp(0.0, 1.0);
                            draws.push(SceneDraw { mesh: meshes.sphere_edges, mvp, wireframe: true, wire_tint: tint, opacity: wire_alpha, line_width: self.wire_width, wire_base_width: 0.0 });
                        }
                        // Per-node meta Wireframe: the same wire pass, scoped
                        // to the flagged nodes' edges, in geometry colors.
                        if self.meta_wire_count > 0 {
                            draws.push(SceneDraw { mesh: meshes.meta_wires, mvp, wireframe: true, wire_tint: NO_TINT, opacity: 1.0, line_width: self.wire_width, wire_base_width: 0.0 });
                        }
                        // Per-node meta Point Normals: thin cyan whiskers,
                        // width deliberately fixed (a chunky Wire Width is a
                        // wireframe styling choice, not a normals one).
                        if self.meta_normal_count > 0 {
                            draws.push(SceneDraw { mesh: meshes.meta_normals, mvp, wireframe: true, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 });
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
                    self.last_viewport_geo_opacity = self.geo_opacity;
                    self.last_viewport_wire_single_color = self.wire_single_color;
                    self.last_viewport_wire_color = self.wire_color;
                    self.last_viewport_wire_width = self.wire_width;
                    self.last_viewport_render_points = self.render_points;
                    self.last_viewport_point_size = self.point_size;
                    self.last_viewport_point_color = self.point_color;
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
            && self.detached_pane.is_none()
            && self.show_viewport
            && self.viewport().rt_mode
            && renderer.rt_accumulating()
    }
}

