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

use cce_ui::widget::{Adapted, Breadcrumb, ImageView, MenuBar, MenuController, ParametersBg, Splitter, Spreadsheet, StatusBar, TextLabel, WidgetHost, GraphNode, Graph, Button, Label, Dropdown};
use cce_ui::widget::UiContext;
use crate::playbar::Playbar;
use crate::viewport_3d::Viewport3D;
use cce_ui::colors;
use glam::{Mat4, Vec3};

use crate::geometry::*;
use crate::detail::Detail;
use crate::slots::*;
use crate::shortcut::{ShortcutManager, Action};
use cce_ui::vk::{SceneDraw, TextSpan};
use cce_ui::engine::Vertex;
use crate::window::WindowEvent;

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
    /// When this parameter should be SHOWN, as a condition over its siblings'
    /// current values. Empty means always.
    ///
    /// Grammar, deliberately tiny: `Mode == Twist`, `Mode == Twist|Bend` for
    /// any-of, `Mode != Bleed` for unless, and ` && ` between clauses. It
    /// exists because collapsing fifty operators into ten traded node count
    /// for parameter count — Attribute reached sixteen parameters, of which
    /// four matter at any moment — and a pane showing twelve irrelevant rows
    /// is worse than the twelve nodes it replaced.
    ///
    /// Houdini calls this `hideWhen`. Phrased the positive way round here
    /// because a template author is describing when a control APPLIES, and
    /// stating that directly is easier to get right than stating its negation.
    #[serde(default)]
    pub show_when: String,
}

fn default_param_type() -> String { "string".to_string() }

/// Expand a leading `~` to the home directory. A path typed into a text field
/// is typed by a person, and `~/models/thing.stl` is what a person writes.
pub fn shellexpand_home(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => format!("{}/{}", home.to_string_lossy(), rest),
            None => path.to_string(),
        },
        None => path.to_string(),
    }
}

/// Whether a parameter's `show_when` condition holds, given its siblings.
///
/// Values are compared case-insensitively against the sibling's CURRENT value
/// (`default` is where this app keeps live values). A condition naming a
/// parameter that does not exist is treated as unmet: a template that
/// misspells a name hides the row rather than showing it unconditionally, so
/// the mistake is visible instead of silent.
pub fn param_visible(params: &[ParamDef], cond: &str) -> bool {
    let cond = cond.trim();
    if cond.is_empty() {
        return true;
    }
    cond.split("&&").all(|clause| {
        let clause = clause.trim();
        let (name, wanted, negated) = match clause.split_once("!=") {
            Some((n, v)) => (n.trim(), v.trim(), true),
            None => match clause.split_once("==") {
                Some((n, v)) => (n.trim(), v.trim(), false),
                // Not a comparison at all: an unparseable condition is a
                // template bug, and hiding the row makes it noticeable.
                None => return false,
            },
        };
        let Some(sibling) = params.iter().find(|p| p.name.eq_ignore_ascii_case(name)) else {
            return false;
        };
        let matches = wanted
            .split('|')
            .any(|w| w.trim().eq_ignore_ascii_case(sibling.default.trim()));
        matches != negated
    })
}

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

    /// Set one child's geometry visibility. Enabling is EXCLUSIVE within the
    /// directory — at most one node per path shows its geometry, so turning a
    /// node on turns every sibling off (a display flag, not a per-node render
    /// flag). Disabling touches only the named child. Every toggle route
    /// (keyboard `e`, the graph widgets' click toggles, MCP/context-menu
    /// ToggleGeometry) must go through here or the invariant silently rots.
    pub fn set_child_geometry_visible(&mut self, slot: usize, visible: bool) {
        if slot >= self.children.len() {
            return;
        }
        if visible {
            for (i, child) in self.children.iter_mut().enumerate() {
                child.geometry_visible = i == slot;
            }
        } else {
            self.children[slot].geometry_visible = false;
        }
    }
}

/// Fresh ids for a node and its whole subtree — required whenever an
/// existing tree is cloned into the graph (paste, template instantiation),
/// because everything keyed by id (the eval cycle guard, sim caches, the
/// curve viewer state's node binding) assumes ids are unique.
pub(crate) fn regenerate_node_ids(n: &mut FsNode) {
    n.id = generate_node_id();
    for child in &mut n.children {
        regenerate_node_ids(child);
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
    /// The docks' tab groups, Left/Right/Bottom order, pane names with the
    /// ACTIVE tab first. Empty (older saves) keeps the default one-pane-per-
    /// dock arrangement; a list that does not name each core docked pane
    /// exactly once (plus "network2" at most once — its presence recreates
    /// the second editor) is ignored the same way.
    #[serde(default)]
    pub dock_tabs: Vec<Vec<String>>,
    /// The second network editor's own path. Clamped on load, so a save
    /// whose tree changed shape degrades to the deepest valid ancestor.
    #[serde(default)]
    pub current_path2: Vec<usize>,
    /// The viewport pin as a pane name ("network"/"network2"); absent or
    /// unresolvable follows the active editor.
    #[serde(default)]
    pub viewport_pin: Option<String>,
    /// The parameters pane's pin, same encoding.
    #[serde(default)]
    pub params_pin: Option<String>,
    /// The spreadsheet's pin, same encoding.
    #[serde(default)]
    pub spreadsheet_pin: Option<String>,
    /// The floating layout's plate geometry — every edge the user can drag
    /// — as window fractions, so a project restores its plate sizes and
    /// positions at any window size (the splitter convention). Absent in
    /// older saves keeps the live geometry.
    #[serde(default)]
    pub plates: Option<PlateGeometry>,
}

/// The user-dragged plate edges of the floating layout, each as a fraction
/// of the window dimension it spans: widths and side insets of the width,
/// the spreadsheet height of the height. The remaining plate coordinates
/// (the network plate's top-left, the parameter plate's right anchor, the
/// spreadsheet's bottom) are derived by `rebuild_positions`, so these five
/// numbers fix every plate's size and position.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct PlateGeometry {
    pub network_width: f32,
    pub params_width: f32,
    pub spreadsheet_height: f32,
    /// How far the spreadsheet's left/right edge tucks under its neighbor
    /// (0 = flush beside it) — see `floating_spreadsheet_inset_left`.
    pub spreadsheet_inset_left: f32,
    pub spreadsheet_inset_right: f32,
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
    /// Enter/exit the curve viewer state (curve nodes only).
    EditCurve,
    /// Remove the node.
    Delete,
}

/// The 3D viewport's right-click context menu actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportMenuAction {
    /// Move the active camera so the visible node geometry fills the view.
    FrameAll,
    /// Put the pivot plane at true size: one world unit (the Guides "World
    /// Unit") spans its real length on this display.
    OneToOne,
    /// Follow whichever editor took the last node click (the default).
    PinFollow,
    /// Lock the viewport to one editor's level (CONTENT_IDX / CONTENT2_IDX).
    PinTo(usize),
    /// A "-" row: engraved, inert.
    Separator,
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
    /// Execute a label-matched menu-pane action ("Show Spreadsheet Pane", "Save", ...)
    /// — the items `menu_click`'s index-matched menubar dispatch cannot reach.
    MenuAction { label: String },
    /// Run a registry command by id — every command the palette lists and
    /// every chord the keyboard can send, under one name. `menu_action`
    /// reaches only the label-dispatched half.
    RunCommand { id: String },
    /// Collapse a pane to its title stub, or restore it — the plate corner
    /// menu's Collapse/Expand, reachable without driving the pointer.
    SetPaneCollapsed { pane: String, collapsed: bool },
    /// Move a pane out into its own window, or take it back — the corner menu's
    /// Detach/Reattach.
    SetPaneDetached { pane: String, detached: bool },
    /// Move the playhead. Simnets solve up to this frame, so it is the only way
    /// to drive a simulation without dragging the playbar.
    SetFrame { frame: f32 },
    /// Replace a curve node's control points wholesale — the automation
    /// counterpart of the curve viewer state's move/add/delete. Structured
    /// [x, y, z] triples rather than the "Points" param string, so agents
    /// never have to know the serialization.
    CurveSetPoints { slot: usize, points: Vec<[f32; 3]> },
}

#[derive(Debug, Clone)]
pub enum CustomEvent {
    /// An MCP `tools/call` from the embedded MCP server (carries its own
    /// reply channel) — the tool name is an `McpAction` tag, or `get_state`.
    McpCall(cce_ui::mcp::McpToolCall),
    /// A fire-and-forget action from an app-internal thread (the cce-files
    /// choosers deliver their picked path this way).
    RunAction(McpAction),
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
    // Rows whose condition does not hold are not shown. Write-back resolves a
    // row by its display key rather than by position, so a hidden parameter
    // simply is not reported and keeps whatever value it had.
    params.iter().filter(|p| param_visible(params, &p.show_when)).map(|p| {
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
                    show_when: String::new(),
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
    // Legacy retypes, session->meta style: renamed native types are rewritten
    // in place (params and name intact) BEFORE matching, so old saves find the
    // renamed template and gain its new params through the normal merge.
    // "add" became "points" (2026-09, gaining the Shape param).
    fn retype_legacy(node: &mut FsNode) {
        if node.node_type.eq_ignore_ascii_case("add") {
            node.node_type = "points".to_string();
        }
        for c in &mut node.children {
            retype_legacy(c);
        }
    }
    retype_legacy(root);

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
                // The condition is UI metadata like the rest: the template
                // owns when a control applies, the instance owns its value.
                // Without this a saved project keeps the pane it had on the
                // day it was made, and a node that later learned to hide its
                // irrelevant rows would not hide them there.
                ip.show_when = tp.show_when.clone();
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
    /// Whether the network pane draws its plate. Lives beside the viewport
    /// settings rather than in a pane-state list because it is an appearance
    /// choice that outlives any one project — a pane's VISIBILITY belongs to
    /// the project, but whether its surface is drawn is how you like to work.
    #[serde(default = "default_network_plate")]
    pub network_plate: bool,
}

fn default_network_plate() -> bool {
    true
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
            network_plate: true,
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

/// `dock_panes` entry for a dock whose tabs were all pulled elsewhere: no
/// slot index, so `pane_shown` reads it as hidden and the dock lays out
/// nothing. Never a valid `positions[..]` index.
pub const NO_PANE: usize = usize::MAX;

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

/// Animated drop-target glow (see [`State::drop_glow`]).
#[derive(Debug, Clone, Copy)]
pub struct DropGlow {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 0..1 — scales the whole feather's alpha profile.
    pub alpha: f32,
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
    /// The SECOND network editor's own path — the point of having one: the
    /// two graph views dive independently. Always valid against `fs_root`
    /// (every structural edit re-clamps it); starts at root.
    pub current_path2: Vec<usize>,
    /// The editor whose SELECTION feeds the parameters pane (and the param
    /// writeback): CONTENT_IDX or CONTENT2_IDX — whichever took the last
    /// node click. Selection itself stays per-editor.
    pub param_editor: usize,
    /// The viewport's pin: None follows `param_editor`; Some(CONTENT_IDX /
    /// CONTENT2_IDX) locks the scene to that editor's level regardless of
    /// where clicks land. Set from the viewport's right-click menu.
    pub viewport_pin: Option<usize>,
    /// The parameters pane's pin — same shape, set from its plate's corner
    /// menu. Pinned, the pane shows and edits the pinned editor's selection
    /// no matter where clicks land.
    pub params_pin: Option<usize>,
    /// The spreadsheet's pin — same shape, set from its plate's corner menu.
    pub spreadsheet_pin: Option<usize>,
    pub node_clipboard: Option<FsNode>,
    pub last_click: Option<(Instant, usize)>,

    pub shortcut_manager: ShortcutManager,
    /// A chord matched this frame, run on the next tick — the command's
    /// ID, since a chord binds a registry row rather than an `Action`.
    pub pending_command: Option<&'static str>,
    pub exit_requested: bool,
    /// Engine event-loop sender so app-spawned threads (the cce-files
    /// choosers) can deliver results back as `CustomEvent`s; set once by
    /// `Application::new`.
    pub event_sender: Option<calloop::channel::Sender<CustomEvent>>,

    pub slots: Box<WidgetSlots>,
    pub positions: Vec<(f32, f32, f32, f32)>,
    /// The Alt+D dialog's Settings rows as last handed to its `ParametersBg` —
    /// the baseline `sync_dialog_settings_to_project` diffs the controls
    /// against. The params pane gets away without one because it can compare a
    /// reported value to the `ParamDef` it came from; the dialog's rows are
    /// assembled from several owners, so what was shown is its own fact.
    pub dialog_settings_shown: Vec<(String, String, String)>,
    pub splitter_layout: cce_ui::layout::SplitterLayout,
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
    /// The GPU image behind the page pane. Owned here — `ImageView` only
    /// borrows an id — so replacing a page frees the one it replaces.
    pub page_image: Option<u32>,
    /// Whether `renderer_init` has run before. There is no separate reconnect
    /// callback: the runner calls `renderer_init` once per renderer, so the
    /// first call is this process's own and every later one is a REPLACEMENT
    /// after a reconnect. Remembering is the only way to tell them apart.
    pub seen_renderer: bool,
    /// The grid cell an explicit deselect happened in.
    ///
    /// The selection IS whatever sits in the cursor's cell — that is what
    /// `sync_cursor_and_selection` means — so simply clearing it does not
    /// stick: the sync runs on nearly every frame that changes anything and
    /// puts it straight back. Remembering the cell lets the sync leave that one
    /// alone, and only that one: navigate anywhere else and selection resumes,
    /// which is why this is a cell rather than a flag.
    pub deselected_cell: Option<(i32, i32)>,
    /// An in-flight camera orbit drag: the cursor position the last motion was
    /// measured from. `None` when no orbit drag is running.
    pub orbit_drag: Option<(f32, f32)>,
    /// Set when the page raster must be re-uploaded — after a replacement
    /// renderer drops the old id. Consumed on the next tick rather than acted
    /// on in `renderer_init`, which runs before the frame has settled and
    /// where relaying the panes would be premature.
    pub page_dirty: bool,
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
    /// With tabs this names each dock's ACTIVE pane — always a member of the
    /// dock's `dock_tabs` list — or [`NO_PANE`] for a dock whose tabs were
    /// all pulled elsewhere.
    pub dock_panes: [usize; 3],
    /// The panes tabbed into each dock, indexed Left/Right/Bottom. One dock
    /// rect, several panes: only the active one (`dock_panes`) is laid out;
    /// the rest wait as tabs, switched and moved through the plate corner
    /// menus. Every docked pane lives in exactly ONE dock's list.
    pub dock_tabs: [Vec<usize>; 3],
    /// The dock a live DockDrag would drop into — the render pass highlights it.
    pub dock_drag_target: Option<Dock>,

    pub drag_widget: Option<usize>,
    /// Where the pointer pressed when `drag_widget` armed — the drag
    /// edge-panning gate: a bare click (press ~ release with jitter) must not
    /// slide the graph under the armed node drag, or the commit re-derives the
    /// node's cell against the panned origin and it teleports. Cleared (latch
    /// open) once the pointer strays a real-drag distance from the press.
    pub(crate) drag_press_cursor: Option<(f32, f32)>,
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
    /// Drag-release fling only (middle / space+left drag over the network
    /// pane): tracked while `is_panning`, integrated by the kinetic slide in
    /// `tick_frame`. Wheel and trackpad panning no longer feed it — that
    /// motion is the Graph widget's own `ScrollMotion` (glide / coast).
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
    /// Whether the network pane draws its PLATE — the filled, frosted surface
    /// the graph sits on. With it off the nodes and wires overlay the 3D scene
    /// directly, since the viewport is full-bleed and the network floats over
    /// it. The pane is still there: it keeps its rect, its focus, its corner
    /// menus and its clip; only the surface under it stops being drawn.
    pub network_plate: bool,
    pub show_viewport: bool,
    pub show_parameters: bool,
    pub show_spreadsheet: bool,
    pub show_playbar: bool,
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
    /// The drop-target glow's animation state: position glides toward the
    /// cell an in-flight node drag will land on, alpha fades in on drag
    /// start and out after release (the glow lingers at its last cell while
    /// fading). None once fully faded. Advanced in [`State::tick_frame`],
    /// drawn by the CONTENT branch in render.rs.
    pub drop_glow: Option<DropGlow>,
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
    /// The pane layout as of the last save — [`State::pane_layout_json`] —
    /// so a dragged plate edge, a collapse or a re-dock dirties the title
    /// like an edit to the tree: the save file carries them, so unsaved
    /// they are unsaved changes.
    pub last_saved_layout_json: String,
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
    /// What one world unit IS — the Guides subnet's "World Unit" choice
    /// (mm / cm / m / in), persisted with the project. Geometry never
    /// converts; this is the declaration that lets the viewport state its
    /// scale against the display metric (`cce_ui::units`) and `View 1:1`
    /// put the pivot plane at true size.
    pub world_unit: cce_ui::units::Unit,
    /// The param pane's completion lists — (input node name, geometry
    /// version) → (group names, attribute names) read off that input's
    /// evaluated geometry, feeding the textpick rows on group/attribute
    /// params. One entry: the selected node's input.
    pub pick_cache: Option<((String, u64), (Vec<String>, Vec<String>))>,
    /// The raster scene's model-view-projection and the viewport pane rect in
    /// LOGICAL px, cached at staging so the 2D pass can project 3D overlays.
    pub last_scene_mvp: Option<Mat4>,
    pub last_scene_view_rect: (f32, f32, f32, f32),
    /// The active curve viewer state (viewport point editing), if any.
    pub viewer_tool: Option<crate::viewer_state::ViewerTool>,
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
        let tree_changed = match serde_json::to_string(&self.fs_root) {
            Ok(current_json) => current_json != self.last_saved_root_json,
            Err(_) => false,
        };
        tree_changed || self.pane_layout_json() != self.last_saved_layout_json
    }

    /// The pane layout the save file carries, keyed for the unsaved-changes
    /// check: what `project_view_state` records MINUS navigation (pan, path,
    /// selection, camera — moving around a project is not editing it).
    /// Plates key in px, which a window resize leaves alone; splitters as
    /// rounded fractions, which a resize scales proportionally — so
    /// resizing the window dirties nothing.
    pub fn pane_layout_json(&self) -> String {
        let vs = self.project_view_state();
        let splitters = vs.splitters.map(|(a, b)| ((a * 1000.0).round(), (b * 1000.0).round()));
        let plates = (
            self.floating_network_layout.2.round(),
            self.floating_param_width.round(),
            self.floating_spreadsheet_height.round(),
            self.floating_spreadsheet_inset_left.round(),
            self.floating_spreadsheet_inset_right.round(),
        );
        serde_json::to_string(&(
            vs.collapsed_panes,
            splitters,
            vs.dock_tabs,
            vs.viewport_pin,
            vs.params_pin,
            vs.spreadsheet_pin,
            plates,
        ))
        .unwrap_or_default()
    }

    /// Record the live tree and pane layout as the saved baseline — every
    /// save and load path calls this, so the title's asterisk clears.
    pub fn mark_saved(&mut self) {
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        self.last_saved_layout_json = self.pane_layout_json();
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
                network_plate: self.network_plate,
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
    
    /// Radians of camera rotation per logical pixel of drag.
    ///
    /// The same constant the trackpad's pixel-delta orbit uses, so a drag and a
    /// two-finger swipe turn the scene at the same rate and the two gestures do
    /// not feel like different cameras. A 300px drag is about 86 degrees.
    pub const ORBIT_RADIANS_PER_PX: f32 = 0.005;

    /// Turn the camera by a drag delta in logical pixels.
    ///
    /// Mirrors the scroll path's split: the default camera carries its own
    /// orbit in `rotation_x`/`rotation_y`, while a named camera accumulates
    /// into `pending_yaw`/`pending_pitch` for the node to pick up. Doing it any
    /// other way would give a dragged camera a different meaning from a
    /// scrolled one.
    pub(crate) fn orbit_camera_by(&mut self, dx_px: f32, dy_px: f32) {
        let dx = dx_px * Self::ORBIT_RADIANS_PER_PX;
        let dy = dy_px * Self::ORBIT_RADIANS_PER_PX;
        if self.active_camera != "Default Camera" {
            let vp = self.viewport_mut();
            vp.pending_yaw += dx;
            vp.pending_pitch += -dy;
        } else {
            let vp = self.viewport_mut();
            vp.rotation_y += dx;
            vp.rotation_x -= dy;
            vp.clamp_orbit_pitch();
        }
        // A drag is a direct gesture: the scene stops when the pointer does,
        // rather than coasting the way a flicked scroll does.
        self.viewport_mut().reset_velocity();
        self.viewport_dirty = true;
    }

    /// Whether the network is drawn as an OVERLAY on the scene rather than on
    /// its own plate: the plate switched off, in the ordinary docked layout.
    ///
    /// The circular pane and a detached network window have their own
    /// geometry and their own hit tests, and neither is a thing to overlay.
    pub fn network_overlay(&self) -> bool {
        !self.network_plate
            && self.show_network
            && !self.circular_network_pane
            && !self.is_detached_network
    }

    /// Whether the cursor is over a pane that FLOATS above the network —
    /// which, when the network spans the whole window, is the only thing
    /// keeping a node drawn under the params pane from stealing its clicks.
    pub fn over_floating_pane_at(&self, px: f32, py: f32) -> bool {
        [PARAM_IDX, SPREADSHEET_IDX, PLAYBAR_IDX].iter().any(|&idx| {
            let (x, y, w, h) = self.positions[idx];
            w > 0.0 && h > 0.0 && px >= x && px < x + w && py >= y && py < y + h
        })
    }

    fn over_floating_pane(&self) -> bool {
        self.over_floating_pane_at(self.cursor_x, self.cursor_y)
    }

    /// Whether the network claims the pointer at (px, py) while it is an
    /// overlay: only where it has actually drawn a node, and only where no
    /// floating pane covers it.
    pub fn overlay_claims(&self, px: f32, py: f32) -> bool {
        !self.over_floating_pane_at(px, py) && self.graph().node_at(px, py).is_some()
    }

    /// Whether (px, py) is inside the network's AREA — the region it is laid
    /// out over, whatever it has drawn there.
    ///
    /// Distinct from [`in_network_pane`](Self::in_network_pane), which in
    /// overlay mode narrows to the nodes so a click can reach the scene. The
    /// two differ only when the plate is off, and the difference is the point:
    /// a CLICK on empty space is not the network's, but a PAN gesture over
    /// that same space is — middle-drag and space+left mean nothing to the
    /// scene, and a graph you cannot pan by dragging because its own surface
    /// stopped being drawn would be a strange thing to ship.
    pub fn in_network_area(&self, px: f32, py: f32) -> bool {
        if self.circular_network_pane {
            return self.circular_network_layout.hit_test_content(px, py, 0.0, BREADCRUMB_H);
        }
        let (cx, cy, cw, ch) = self.positions[CONTENT_IDX];
        px >= cx
            && px < cx + cw
            && py >= cy
            && py < cy + ch
            && !(self.network_overlay() && self.over_floating_pane_at(px, py))
    }

    pub fn in_network_pane(&self) -> bool {
        if self.circular_network_pane {
            self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, 0.0, BREADCRUMB_H)
        } else if self.network_overlay() {
            // An overlay claims only what it DRAWS. The pane spans the window,
            // so a rect test would swallow every click meant for the scene
            // behind it — there would be no way left to orbit the camera. A
            // node under the cursor is the network's; empty space is the
            // scene's. The circular pane already routes this way.
            self.overlay_claims(self.cursor_x, self.cursor_y)
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
        if self.network_overlay() {
            // The complement of the overlay: everything in the body the
            // network is not holding and no floating pane covers.
            return !self.in_network_pane()
                && !self.over_floating_pane()
                && self.cursor_x < self.splitter_layout.splitter2_x
                && self.cursor_y >= HEADER_H
                && self.cursor_y < self.height - STATUS_H;
        }
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
            // The second network editor has no View-menu flag: being placed
            // in a dock's tab list IS its existence.
            crate::slots::NETWORK_PANEL2_IDX => true,
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
    /// With tabs, the whole GROUPS trade places — a dot drag moves the
    /// plate and every tab riding it, exactly what the drag shows moving.
    pub fn move_pane_to_dock(&mut self, slot: usize, dock: Dock) {
        let Some(from) = self.dock_of_pane(slot) else { return };
        if from == dock {
            return;
        }
        self.dock_panes.swap(from as usize, dock as usize);
        self.dock_tabs.swap(from as usize, dock as usize);
        self.after_dock_change();
    }

    /// The full re-sync a dock/tab change needs: layout, panel offsets, the
    /// graphs' grid origins (they follow their pane rects), and the node
    /// lists — a fronted pane must not wait for the next unrelated event to
    /// fill in.
    fn after_dock_change(&mut self) {
        self.rebuild_positions();
        self.apply_layout();
        self.read_panel_offsets();
        self.sync_grid_settings();
        self.sync_nodes();
    }

    /// The dock whose TAB LIST holds `slot` — its home whether or not it is
    /// the active tab there ([`Self::dock_of_pane`] finds only actives).
    pub fn tab_dock_of_pane(&self, slot: usize) -> Option<Dock> {
        [Dock::Left, Dock::Right, Dock::Bottom]
            .into_iter()
            .find(|&d| self.dock_tabs[d as usize].contains(&slot))
    }

    /// Bring one of a dock's tabs to the front (the corner menu's tab switch).
    pub fn show_dock_tab(&mut self, dock: Dock, slot: usize) {
        if !self.dock_tabs[dock as usize].contains(&slot) || self.dock_panes[dock as usize] == slot {
            return;
        }
        self.dock_panes[dock as usize] = slot;
        self.after_dock_change();
    }

    /// Pull `slot` out of its current dock (if it has one — an unplaced pane
    /// like a fresh second network editor simply joins) and tab it into
    /// `dock`, active. The dock it leaves fronts its next remaining tab, or
    /// empties ([`NO_PANE`]) — its rect stays reserved by the dock-owned
    /// dimensions either way, ready for a tab to move back.
    pub fn add_dock_tab(&mut self, dock: Dock, slot: usize) {
        if let Some(from) = self.tab_dock_of_pane(slot) {
            if from == dock {
                self.show_dock_tab(dock, slot);
                return;
            }
            let f = from as usize;
            self.dock_tabs[f].retain(|&s| s != slot);
            if self.dock_panes[f] == slot {
                self.dock_panes[f] = self.dock_tabs[f].first().copied().unwrap_or(NO_PANE);
            }
        }
        self.dock_tabs[dock as usize].push(slot);
        self.dock_panes[dock as usize] = slot;
        self.after_dock_change();
    }

    /// Remove a CLOSABLE pane (the second network editor) from the docks
    /// entirely — it stops existing until a tab row re-adds it. Its dock
    /// fronts the next tab or empties.
    pub fn close_dock_tab(&mut self, slot: usize) {
        let Some(from) = self.tab_dock_of_pane(slot) else { return };
        // A closed editor cannot hold the viewport or the params pane.
        if slot == crate::slots::NETWORK_PANEL2_IDX {
            for pin in [&mut self.viewport_pin, &mut self.params_pin, &mut self.spreadsheet_pin] {
                if *pin == Some(crate::slots::CONTENT2_IDX) {
                    *pin = None;
                }
            }
            if self.param_editor == crate::slots::CONTENT2_IDX {
                self.param_editor = CONTENT_IDX;
            }
            self.rebuild_scene_geometry();
            self.sync_parameters_pane();
        }
        let f = from as usize;
        self.dock_tabs[f].retain(|&s| s != slot);
        if self.dock_panes[f] == slot {
            self.dock_panes[f] = self.dock_tabs[f].first().copied().unwrap_or(NO_PANE);
        }
        self.after_dock_change();
    }

    /// Move the active tab of `slot`'s dock out to the first EMPTY dock —
    /// the corner menu's inverse of Add Tab. No empty dock, no move (with
    /// three panes on three docks, one is empty whenever any dock holds two).
    pub fn split_dock_tab(&mut self, slot: usize) {
        if self.tab_dock_of_pane(slot).is_none() {
            return;
        }
        let Some(empty) = [Dock::Left, Dock::Right, Dock::Bottom]
            .into_iter()
            .find(|&d| self.dock_tabs[d as usize].is_empty())
        else {
            return;
        };
        self.add_dock_tab(empty, slot);
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

    /// The node a path addresses, CLAMPED: each step that no longer exists
    /// ends the walk, so a stale second-editor path degrades to the deepest
    /// still-valid ancestor instead of indexing out of bounds after a
    /// structural edit made in the other view.
    pub fn dir_at(&self, path: &[usize]) -> &FsNode {
        let mut node = &self.fs_root;
        for &i in path {
            match node.children.get(i) {
                Some(child) => node = child,
                None => break,
            }
        }
        node
    }

    /// [`Self::dir_at`], mutable — same clamping walk.
    pub fn dir_at_mut(&mut self, path: &[usize]) -> &mut FsNode {
        let mut node = &mut self.fs_root;
        for &i in path {
            if i < node.children.len() {
                node = &mut node.children[i];
            } else {
                break;
            }
        }
        node
    }

    /// One editor's selected slot (CONTENT_IDX / CONTENT2_IDX).
    pub fn editor_selected_of(&self, editor: usize) -> Option<usize> {
        if editor == crate::slots::CONTENT2_IDX {
            use cce_ui::widget::GraphController as _;
            self.slots.content2.selected_node()
        } else {
            self.graph().selected_node()
        }
    }

    /// One editor's displayed level.
    pub fn editor_dir_of(&self, editor: usize) -> &FsNode {
        if editor == crate::slots::CONTENT2_IDX {
            self.dir_at(&self.current_path2)
        } else {
            self.current_dir()
        }
    }

    /// [`Self::editor_dir_of`], mutable.
    pub fn editor_dir_of_mut(&mut self, editor: usize) -> &mut FsNode {
        if editor == crate::slots::CONTENT2_IDX {
            let p2 = self.current_path2.clone();
            self.dir_at_mut(&p2)
        } else {
            self.current_dir_mut()
        }
    }

    /// The editor the parameters pane is bound to: its pin, else the active
    /// (last-clicked) editor.
    pub fn params_editor(&self) -> usize {
        self.params_pin.unwrap_or(self.param_editor)
    }

    /// The editor the spreadsheet is bound to — same resolution.
    pub fn spreadsheet_editor(&self) -> usize {
        self.spreadsheet_pin.unwrap_or(self.param_editor)
    }

    /// The selected slot in the editor the parameters pane follows.
    pub fn param_editor_selected(&self) -> Option<usize> {
        self.editor_selected_of(self.params_editor())
    }

    /// The level that editor is showing — where its selection resolves.
    pub fn param_editor_dir(&self) -> &FsNode {
        self.editor_dir_of(self.params_editor())
    }

    /// [`Self::param_editor_dir`], mutable — the param writeback target.
    pub fn param_editor_dir_mut(&mut self) -> &mut FsNode {
        self.editor_dir_of_mut(self.params_editor())
    }

    /// The editor whose level the VIEWPORT renders: the pin when set, else
    /// the active (last-clicked) editor.
    pub fn viewport_editor(&self) -> usize {
        self.viewport_pin.unwrap_or(self.param_editor)
    }

    /// The level the viewport renders — [`Self::viewport_editor`]'s dir.
    pub fn viewport_editor_dir(&self) -> &FsNode {
        if self.viewport_editor() == crate::slots::CONTENT2_IDX {
            self.dir_at(&self.current_path2)
        } else {
            self.current_dir()
        }
    }

    /// Truncate the second editor's path to its valid prefix — run after any
    /// structural edit, so `dir_at` clamping and the drawn breadcrumb agree.
    pub fn clamp_path2(&mut self) {
        let mut node = &self.fs_root;
        let mut valid = 0;
        for &i in &self.current_path2 {
            match node.children.get(i) {
                Some(child) => {
                    node = child;
                    valid += 1;
                }
                None => break,
            }
        }
        self.current_path2.truncate(valid);
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
            if let Some(slot_idx) = self.param_editor_selected() {
                let updated_params = self.param().node_params();
                // Live pane state, so a pane toggle only fires the visibility
                // action when it actually flips relative to what's on screen.
                let cur_show = (self.show_network, self.show_viewport, self.show_parameters, self.show_spreadsheet, self.show_playbar);
                let dir = self.param_editor_dir_mut();
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
    /// Write the selected Export node's input to its File.
    ///
    /// Evaluated fresh at the playbar's current frame rather than reusing the
    /// scene: the scene is what is VISIBLE, and an Export node whose geometry
    /// flag is off — which is the normal way to use one, since its input is
    /// already drawn — contributes nothing to it.
    pub fn run_export(&mut self) {
        let Some(slot_idx) = self.param_editor_selected() else { return };
        let dir = self.param_editor_dir();
        let Some(node) = dir.children.get(slot_idx).filter(|c| c.node_type == "export") else {
            return;
        };
        let node = node.clone();
        let file = crate::geometry::node_param_str(&node, "File", "");
        let file = file.trim().to_string();
        if file.is_empty() {
            self.update_status_text("Export: set a File first.");
            return;
        }
        let (format, scale) = crate::geometry::export_settings(&node);
        let path = std::path::PathBuf::from(shellexpand_home(&file));

        // A page is a different thing to get out of the computer, and the node
        // does not need to be told which it is holding: what reaches its input
        // decides. A page writes a PNG that carries its own physical size; the
        // Format parameter names a MESH format and has nothing to say here.
        if let Some(page) = crate::page::resolve_page(&self.fs_root, &node, &mut Vec::new()) {
            let (w, h) = (page.width, page.height);
            match page.write_png(&path) {
                Ok(()) => self.update_status_text(&format!(
                    "Exported {} as PNG ({}x{} at {} DPI) to {}",
                    node.name,
                    w,
                    h,
                    page.dpi,
                    path.display()
                )),
                Err(e) => self.update_status_text(&format!("Export failed: {e}")),
            }
            return;
        }

        let (frame, start) = (self.sim_frame(), self.sim_start_frame());
        let mut sim_cache = std::mem::take(&mut self.sim_cache);
        let geom = {
            let mut sim = crate::geometry::EvalSim::new(frame, start, &mut sim_cache);
            let mut visited = Vec::new();
            let mut err = None;
            crate::geometry::generate_single_node_geometry_with_errors(
                &self.fs_root,
                &node,
                &mut visited,
                &mut err,
                &mut sim,
            )
        };
        self.sim_cache = sim_cache;
        let Some(geom) = geom else {
            self.update_status_text("Export: the node has no input geometry.");
            return;
        };
        if geom.num_prims() == 0 {
            // Writing an empty file is a worse answer than saying so: a
            // zero-triangle STL is valid, and a slicer opening one reports
            // nothing wrong.
            self.update_status_text("Export: the geometry has no primitives.");
            return;
        }

        match crate::export::write(&geom, &path, format, scale) {
            Ok(bytes) => self.update_status_text(&format!(
                "Exported {} as {} ({} bytes) to {}",
                node.name,
                format.label(),
                bytes,
                path.display()
            )),
            Err(e) => self.update_status_text(&format!("Export failed: {e}")),
        }
    }

    pub fn execute_menu_action(&mut self, label: &str) -> bool {
        match label {
            // An Export node's button. Buttons dispatch by LABEL, which has no
            // node attached to it — but the pressed button can only be on the
            // node the pane is showing, so the selection is the node.
            "Export" => {
                self.run_export();
            }
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
            "Undo" => {
                self.execute_action(Action::Undo);
            }
            "Redo" => {
                self.execute_action(Action::Redo);
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
            // Both spellings reach the one action: "Show Network Plate" is
            // the View node's parameter name, "Network Plate" the menu row.
            "Show Network Plate" | "Network Plate" => {
                self.execute_action(Action::ToggleNetworkPlate);
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

    pub(crate) fn refresh_main_node_live_toggles(&mut self, slot_idx: usize) {
        let live_main: [(&str, bool); 2] = [
            ("Circular Pane", self.circular_network_pane),
            ("Ray Traced Preview", self.viewport().rt_mode),
        ];
        let live_view: [(&str, bool); 6] = [
            ("Show Network Plate", self.network_plate),
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
        let dir = self.param_editor_dir_mut();
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
        // Selection reads through the param-editor accessors: whichever
        // network editor took the last node click feeds the pane, at ITS
        // level — no matter the tab.
        if !self.is_detached_network {
            if let Some(slot_idx) = self.param_editor_selected() {
                self.refresh_main_node_live_toggles(slot_idx);
            }
        }
        let params = if !self.is_detached_network {
            if let Some(slot_idx) = self.param_editor_selected() {
                let dir = self.param_editor_dir();
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
            let Some(slot) = self.param_editor_selected() else { return params };
            let dir = self.param_editor_dir();
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
                    // Groups and attributes are separate namespaces now, so
                    // the menus read each directly instead of sifting a
                    // "group:" prefix out of one attribute map.
                    for g in geom.points().group_names() {
                        if !g.contains(',') {
                            groups.insert(g.to_string());
                        }
                    }
                    for a in geom.points().names() {
                        if !a.contains(',') {
                            attrs.insert(a.to_string());
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
        self.path_names_at(&self.current_path)
    }

    /// Segment names along `path` (skipping steps that no longer resolve —
    /// the same clamping `dir_at` walks with).
    pub fn path_names_at(&self, path: &[usize]) -> Vec<String> {
        let mut node = &self.fs_root;
        let mut names = Vec::new();
        for &i in path {
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

    /// Open the node right-click context menu at the cursor for `slot`. The
    /// items are contextual: Enter (dive into the subnet) for enterable nodes,
    /// Show/Hide Geometry for non-utility nodes, and Delete always.
    fn open_node_context_menu(&mut self, slot: usize) {
        let (is_utility, geom_visible, enterable, curve_editing) = {
            let dir = self.current_dir();
            let Some(node) = dir.children.get(slot) else { return };
            let enterable = node.is_enterable();
            // None: no viewer state for this node type; Some(bool): editable,
            // and whether it is being edited right now. The types that have a
            // state are whatever `source_for` accepts, so a new HandleSource
            // appears in this menu without touching it.
            let curve_editing = crate::viewer_state::source_for(&node.node_type).map(|_| {
                self.viewer_tool.as_ref().map(|t| t.node_id == node.id).unwrap_or(false)
            });
            (
                matches!(node.node_type.as_str(), "utility" | "session" | "meta"),
                node.geometry_visible,
                enterable,
                curve_editing,
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
        if let Some(editing) = curve_editing {
            // "Handles", not "Points": a soft transform's are a centre and a
            // tip, and only a curve's are points.
            options.push(if editing { "Stop Editing Handles" } else { "Edit Handles" }.to_string());
            actions.push(NodeMenuAction::EditCurve);
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
            self.viewport_mut().zoom = (dist / base_len).clamp(0.05, crate::viewport_3d::Viewport3D::MAX_ZOOM);
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

    /// One world unit in millimetres, per the Guides "World Unit".
    pub fn world_unit_mm(&self) -> f32 {
        let m = cce_ui::units::metric();
        cce_ui::units::Len::new(1.0, self.world_unit).convert(cce_ui::units::Unit::Mm, &m).value
    }

    /// Camera distance to the pivot plane at which the view is 1:1 — one
    /// world unit on that plane covers its real length on this display.
    /// `get_matrices` is a perspective with vertical FOV 0.9 rad, so the
    /// plane at distance D spans 2·D·tan(0.45) world units over the pane's
    /// height in logical px, and the display metric says how many mm each
    /// of those px is.
    fn one_to_one_distance(&self) -> f32 {
        let m = cce_ui::units::metric();
        let vh_logical = (self.last_viewport_height.max(1) as f32) / (self.scale as f32).max(0.001);
        let unit_mm = self.world_unit_mm().max(1e-6);
        m.mm_per_px() * vh_logical / (2.0 * 0.45f32.tan() * unit_mm)
    }

    /// The view's scale on the pivot plane: how many millimetres of world
    /// one millimetre of screen shows (1.0 = true size, 2.0 = half size).
    pub fn view_scale_ratio(&self) -> f32 {
        let m = cce_ui::units::metric();
        let vh_logical = (self.last_viewport_height.max(1) as f32) / (self.scale as f32).max(0.001);
        let base = self.last_viewport_camera_pos - self.last_viewport_pivot;
        let d = base.length().max(1e-4) * self.last_viewport_zoom;
        let world_mm_per_px = 2.0 * d * 0.45f32.tan() / vh_logical.max(1.0) * self.world_unit_mm();
        world_mm_per_px / m.mm_per_px().max(1e-6)
    }

    /// `View 1:1`: move the active camera along its own eye ray so the
    /// pivot plane sits at [`Self::one_to_one_distance`]. The default camera
    /// zooms (its eye ray is fixed through the origin); a camera node has its
    /// Position rewritten, as Frame All does. The zoom clamp can refuse a
    /// very large or small unit — then the readout shows what was reached.
    pub fn view_one_to_one(&mut self) {
        let dist = self.one_to_one_distance();
        if self.active_camera == "Default Camera" {
            let base_len = Vec3::new(2.5, 1.8, 2.5).length();
            self.viewport_mut().zoom = (dist / base_len).clamp(0.05, crate::viewport_3d::Viewport3D::MAX_ZOOM);
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
                let dir_unit = if offset.length() > 1e-4 { offset.normalize() } else { Vec3::new(2.5, 1.8, 2.5).normalize() };
                let new_pos = piv + dir_unit * dist;
                if let Some(p) = node.params.iter_mut().find(|p| p.name == "Position") {
                    p.default = format!("{:.3}:{:.3}:{:.3}", new_pos.x, new_pos.y, new_pos.z);
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
        let mut options = vec!["Frame All".to_string(), "View 1:1".to_string()];
        let mut actions = vec![ViewportMenuAction::FrameAll, ViewportMenuAction::OneToOne];
        // The viewport's editor binding, as a radio group: follow the active
        // editor, or pin to one. Pin rows appear only while a second editor
        // exists — with one editor, following IS pinned.
        if self.tab_dock_of_pane(crate::slots::NETWORK_PANEL2_IDX).is_some() {
            options.push("-".to_string());
            actions.push(ViewportMenuAction::Separator);
            let mark = |on: bool| if on { "●" } else { "○" };
            options.push(format!("{} Follow Active Editor", mark(self.viewport_pin.is_none())));
            actions.push(ViewportMenuAction::PinFollow);
            options.push(format!(
                "{} Pin: Network",
                mark(self.viewport_pin == Some(CONTENT_IDX))
            ));
            actions.push(ViewportMenuAction::PinTo(CONTENT_IDX));
            options.push(format!(
                "{} Pin: Network 2",
                mark(self.viewport_pin == Some(crate::slots::CONTENT2_IDX))
            ));
            actions.push(ViewportMenuAction::PinTo(crate::slots::CONTENT2_IDX));
        }
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
                    ViewportMenuAction::OneToOne => {
                        self.view_one_to_one();
                    }
                    ViewportMenuAction::PinFollow => {
                        self.viewport_pin = None;
                        self.rebuild_scene_geometry();
                    }
                    ViewportMenuAction::PinTo(e) => {
                        self.viewport_pin = Some(e);
                        self.rebuild_scene_geometry();
                    }
                    ViewportMenuAction::Separator => {}
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
            NodeMenuAction::EditCurve => {
                self.toggle_viewer_state(slot);
            }
            NodeMenuAction::Delete => {
                self.delete_node(slot);
            }
        }
    }


pub(crate) fn geometry_to_spreadsheet_data(geom: &Detail) -> (Vec<String>, Vec<Vec<String>>) {
    // One row per POINT, not per triangle corner. The soup listed the same
    // place once for every face touching it — a sphere came to 2304 rows for
    // 362 places — and the row number meant nothing a user could point at.
    // It is now the point index, which is also what the Point Numbers overlay
    // draws.
    let mut headers = vec![
        "Point".to_string(),
        "Pos.x".to_string(),
        "Pos.y".to_string(),
        "Pos.z".to_string(),
        "Col.r".to_string(),
        "Col.g".to_string(),
        "Col.b".to_string(),
    ];

    // Columnar storage means the columns are known up front, from the store
    // rather than from a scan of every element's map. `names()` is sorted, so
    // they hold still between frames.
    let attribs: Vec<(String, crate::detail::AttribType)> = geom
        .points()
        .names()
        .into_iter()
        .filter(|n| *n != crate::detail::CD)
        .filter_map(|n| geom.points().get(n).map(|a| (n.to_string(), a.ty())))
        .collect();

    // A Derivative attribute wears a trailing `~`: it resets at every step
    // boundary, and telling that apart from a value that persists is the first
    // question you ask when a solve misbehaves.
    let mark = |geom: &Detail, class: crate::detail::Class, name: &str| -> String {
        match geom.store(class).kind(name) {
            crate::detail::AttribKind::Derivative => format!("{}~", name),
            crate::detail::AttribKind::Live => name.to_string(),
        }
    };

    for (name, ty) in &attribs {
        let label = mark(geom, crate::detail::Class::Point, name);
        match ty.components() {
            1 => headers.push(label),
            n => {
                for c in ["x", "y", "z", "w"].iter().take(n) {
                    headers.push(format!("{}.{}", label, c));
                }
            }
        }
    }

    let groups = geom.points().group_names();
    for g in &groups {
        headers.push(format!("g:{}", g));
    }

    // Detail attributes ride along as `d:` columns, constant down the table —
    // which is what a detail attribute IS. Without this the Analysis node
    // would write its answers somewhere nothing could show them.
    let detail: Vec<(String, crate::detail::AttribType)> = geom
        .detail()
        .names()
        .into_iter()
        .filter_map(|n| geom.detail().get(n).map(|a| (n.to_string(), a.ty())))
        .collect();
    for (name, ty) in &detail {
        let label = mark(geom, crate::detail::Class::Detail, name);
        match ty.components() {
            1 => headers.push(format!("d:{}", label)),
            n => {
                for c in ["x", "y", "z", "w"].iter().take(n) {
                    headers.push(format!("d:{}.{}", label, c));
                }
            }
        }
    }

    let mut rows = Vec::new();
    for p in 0..geom.num_points() {
        let pos = geom.positions()[p];
        let col = geom.color(p);
        let mut row = vec![
            p.to_string(),
            format!("{:.4}", pos[0]),
            format!("{:.4}", pos[1]),
            format!("{:.4}", pos[2]),
            format!("{:.4}", col[0]),
            format!("{:.4}", col[1]),
            format!("{:.4}", col[2]),
        ];

        for (name, ty) in &attribs {
            // A column covers its whole class, so there is no "this element
            // does not have it" case left to render as a dash.
            match geom.points().value(name, p) {
                Some(crate::detail::AttribValue::Float(f)) => row.push(format!("{:.4}", f)),
                Some(crate::detail::AttribValue::Int(i)) => row.push(i.to_string()),
                Some(crate::detail::AttribValue::Float2(a)) => {
                    row.extend(a.iter().map(|v| format!("{:.4}", v)))
                }
                Some(crate::detail::AttribValue::Float3(a)) => {
                    row.extend(a.iter().map(|v| format!("{:.4}", v)))
                }
                Some(crate::detail::AttribValue::Float4(a)) => {
                    row.extend(a.iter().map(|v| format!("{:.4}", v)))
                }
                None => row.extend(std::iter::repeat("-".to_string()).take(ty.components())),
            }
        }

        for g in &groups {
            row.push(if geom.points().in_group(g, p) { "1".to_string() } else { String::new() });
        }

        for (name, ty) in &detail {
            match geom.detail().value(name, 0) {
                Some(crate::detail::AttribValue::Float(f)) => row.push(format!("{:.4}", f)),
                Some(crate::detail::AttribValue::Int(i)) => row.push(i.to_string()),
                Some(crate::detail::AttribValue::Float2(a)) => {
                    row.extend(a.iter().map(|v| format!("{:.4}", v)))
                }
                Some(crate::detail::AttribValue::Float3(a)) => {
                    row.extend(a.iter().map(|v| format!("{:.4}", v)))
                }
                Some(crate::detail::AttribValue::Float4(a)) => {
                    row.extend(a.iter().map(|v| format!("{:.4}", v)))
                }
                None => row.extend(std::iter::repeat("-".to_string()).take(ty.components())),
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

    /// One level's children as the graph widget's rows.
    fn graph_nodes_of(dir: &FsNode) -> Vec<GraphNode> {
        dir.children
            .iter()
            .map(|c| GraphNode {
                id: c.id.clone(),
                name: c.name.clone(),
                position: c.position,
                parameters: param_display(&c.params),
                geom_visible: c.geometry_visible,
                node_type: c.node_type.clone(),
                inputs: c.inputs,
                outputs: c.outputs,
            })
            .collect()
    }

    pub fn sync_nodes(&mut self) {
        let graph_nodes = Self::graph_nodes_of(self.current_dir());
        self.graph_mut().set_nodes(&graph_nodes);

        // The second network editor views ITS OWN level.
        self.clamp_path2();
        let nodes2 = Self::graph_nodes_of(self.dir_at(&self.current_path2.clone()));
        use cce_ui::widget::GraphController as _;
        self.slots.content2.set_nodes(&nodes2);
        let names2 = self.path_names_at(&self.current_path2.clone());
        use cce_ui::widget::PathController as _;
        self.slots.breadcrumb2.set_path(&names2);

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

        // The spreadsheet (and the group markers with it) read the
        // SPREADSHEET's binding: its pin when set, else the active editor —
        // exactly the parameters pane's rule with its own pin.
        let mut selected_node = None;
        if !self.is_detached_network {
            let se = self.spreadsheet_editor();
            if let Some(slot_idx) = self.editor_selected_of(se) {
                let dir = self.editor_dir_of(se);
                if slot_idx < dir.children.len() {
                    selected_node = Some(&dir.children[slot_idx]);
                }
            }
        }


        let sim_frame = self.sim_frame();
        let sim_start = self.sim_start_frame();
        let mut cache_hit = false;
        // Keyed by node ID, not name: names repeat across levels (every
        // Sphere subnet holds an "opencl1"), and with two editors selecting
        // at different levels a name key stale-hits across them.
        let mut current_name = None;
        let mut current_params = None;

        if let Some(node) = selected_node {
            current_name = Some(node.id.clone());
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
            left_menubar: MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("0: Network").with_label("Network Menu Bar").with_item("File", &["New", "Save", "Save As"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Network Plate", "Circular Pane", "Detach Pane", "Close Pane"]).with_context_options(context_opts.clone(), 0),
            right_menubar: MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("1: Viewport").with_label("Viewport Menu Bar").with_item("Camera", &["Perspective", "Orthographic"]).with_item("Display", &["Square Aspect"]).with_item("Guides", &["Show Grid", "Cube", "Origin", "Camera Pivot"]).with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 1),
            param_menubar: MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("2: Parameters").with_label("Parameters Menu Bar").with_item("Preset", &["Default", "Custom"]).with_item("Reset", &["All"]).with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 2),
            status: StatusBar::new().with_text("Ready"),
            breadcrumb: {
                let mut bc = Breadcrumb::new();
                // Raised, not the default trough: the segments float in front
                // of the network plate rather than reading as inset into it.
                bc.set_raised(true);
                bc
            },
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
            network_panel2: PassivePlate::new(),
            content2: Graph::new(),
            breadcrumb2: {
                let mut bc = Breadcrumb::new();
                bc.set_raised(true);
                bc
            },
            page_view: {
                // Contain, never crop: a page is a document, and a document
                // shown with its margins cut off is a different document. No
                // upscale past 1:1 either — a 72 DPI sheet blown up to fill
                // the pane would look like the composition is soft when it is
                // the preview that is.
                let mut v = ImageView::new()
                    .with_fit(cce_ui::scene::layout::FitMode::Contain { max_upscale: 1.0 })
                    .with_bg([0.12, 0.12, 0.13, 1.0]);
                v.set_visible(false);
                v
            },
            dialog: crate::dialog::Dialog::new(),
            dialog_params: {
                // Shown only while the dialog's Settings tab is up; laid out
                // inside the dialog's plate, so it draws no plate of its own.
                let mut p = ParametersBg::new();
                p.set_visible(false);
                p
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
        // defaulting to what the command registry declares; an invalid user
        // chord logs and falls back to the default instead of panicking.
        //
        // One loop over the registry, not a list kept beside it. The old hand-
        // written block was where a command went to be forgotten: undo was an
        // Action and an Edit-menu item and appeared here in neither, so the app
        // shipped with no Ctrl+Z and nothing that could have told you.
        let mut shortcut_manager = ShortcutManager::new();
        {
            let mut resolved: Vec<(&'static str, String)> = Vec::new();
            for cmd in crate::command::COMMANDS {
                let Some(default) = cmd.default_chord else { continue };
                let chord = cce_ui::input::app_chord(cmd.id, default);
                if shortcut_manager.register(&chord, cmd.id).is_err() {
                    eprintln!(
                        "[cce-designer] invalid chord {:?} for {}; using {:?}",
                        chord, cmd.id, default
                    );
                    let _ = shortcut_manager.register(default, cmd.id);
                    resolved.push((cmd.id, default.to_string()));
                } else {
                    resolved.push((cmd.id, chord));
                }
            }
            // Two commands on one chord make the second unreachable in silence
            // — it looks like a broken command rather than a broken binding —
            // so say which lost and to whom.
            for c in crate::command::conflicts(&resolved) {
                eprintln!(
                    "[cce-designer] chord {:?} is bound to {} and to {}; {} wins",
                    c.chord,
                    c.winner,
                    c.shadowed.join(", "),
                    c.winner
                );
            }
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
            current_path2: Vec::new(),
            param_editor: CONTENT_IDX,
            viewport_pin: None,
            params_pin: None,
            spreadsheet_pin: None,
            node_clipboard: None,
            last_click: None,
            shortcut_manager,
            pending_command: None,
            exit_requested: false,
            event_sender: None,
            slots,
            positions,
            dialog_settings_shown: Vec::new(),
            splitter_layout,
            node_menu_slot: None,
            node_menu_actions: Vec::new(),
            viewport_menu_active: false,
            viewport_menu_actions: Vec::new(),
            sim_cache: crate::geometry::SimCache::default(),
            page_image: None,
            seen_renderer: false,
            deselected_cell: None,
            orbit_drag: None,
            page_dirty: false,
            last_sim_frame: i32::MIN,
            plate_menu_slot: None,
            plate_menu_actions: Vec::new(),
            collapsed_panes: [false; WIDGET_COUNT],
            corner_press: None,
            dock_panes: [NETWORK_PANEL_IDX, PARAM_IDX, SPREADSHEET_IDX],
            dock_tabs: [
                vec![NETWORK_PANEL_IDX],
                vec![PARAM_IDX],
                vec![SPREADSHEET_IDX],
            ],
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
            network_plate: settings.viewport.network_plate,
            show_viewport: true,
            show_parameters: true,
            show_spreadsheet: false,
            show_playbar: false,
            last_spreadsheet_node_name: None,
            last_spreadsheet_node_params: None,
            grid_thickness: settings.viewport.grid_thickness,
            focused_pane: LEFT_MENUBAR_IDX,
            // Config-owned; update_inertial_settings overwrites these from
            // config.kdl's input.inertial right after construction. They
            // shape the drag-release fling and the unrouted wheel fallback
            // only — routed wheel/trackpad panning is tuned by cce-ui's
            // smooth-scroll settings (input.kdl) inside the Graph widget.
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
            // Cells and gaps render as ONE surface (the graph's bg_color is
            // the cell tint): the checkerboard grout is off by design; the
            // drop-target glow (render.rs) carries the only cell highlight.
            drop_glow: None,
            uniform_background: true,
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
            last_saved_layout_json: String::new(),
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
            world_unit: cce_ui::units::Unit::Mm,
            pick_cache: None,
            last_scene_mvp: None,
            last_scene_view_rect: (0.0, 0.0, 0.0, 0.0),
            viewer_tool: None,
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
        // The layout baseline waits for the layout pass above (it clamps the
        // plate fields), and the title computed earlier must be re-read
        // against it or a fresh window opens starred.
        state.last_saved_layout_json = state.pane_layout_json();
        state.update_window_title();
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

        // The second editor's graph reads the SAME display settings but its
        // OWN rect as origin (no pan of its own yet — the rect is the view).
        let (qx, qy, _, _) = self.positions[crate::slots::CONTENT2_IDX];
        {
            use cce_ui::widget::GraphController as _;
            let g2 = &mut *self.slots.content2;
            g2.set_show_network_grid(network_grid_visible);
            g2.set_grid_sizes(grid_size_x, grid_size_y);
            g2.set_skipped_sizes(gap_row_h, gap_col_w);
            g2.set_grid_origin(qx, qy);
            g2.set_grid_snap_enabled(grid_snap_enabled);
        }
        if let Some(g2) = self.slots.content2.as_any_mut().downcast_mut::<cce_ui::widget::Graph>() {
            g2.set_uniform_background(self.uniform_background);
            g2.set_network_opacity(self.network_opacity);
            g2.set_node_opacity(self.node_opacity);
            g2.set_cell_color(self.cell_color);
            g2.set_gap_color(self.gap_color);
        }
        if let Some(bc2) = self.slots.breadcrumb2.as_any_mut().downcast_mut::<cce_ui::widget::Breadcrumb>() {
            bc2.set_network_opacity(self.network_opacity);
        }
        if let Some(plate) = self.slots.network_panel2.as_any_mut().downcast_mut::<PassivePlate>() {
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
        // Graph-side these shape the drag-release fling and the unrouted
        // wheel fallback; the routed wheel/trackpad pan is the Graph widget's
        // ScrollMotion, tuned by cce-ui's smooth-scroll keys in input.kdl.
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

        // The second network editor exists only in the floating branch,
        // which re-lays it below; zeroing here keeps the circular and
        // detached branches (which predate it) from leaving stale rects.
        for slot in [
            crate::slots::NETWORK_PANEL2_IDX,
            crate::slots::CONTENT2_IDX,
            crate::slots::BREADCRUMB2_IDX,
        ] {
            self.positions[slot] = (0.0, 0.0, 0.0, 0.0);
            self.slots.get_dyn_mut(slot).set_visible(false);
        }

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
                // With the plate off the network is an overlay on the scene,
                // so it takes the whole body instead of its dock: there is no
                // surface left to bound it, and a graph confined to a
                // rectangle you cannot see is worse than one that spans what
                // it is drawn over. Everything below derives from these four
                // numbers — content, panel, breadcrumb — so overriding them
                // here keeps the pane's parts agreeing with each other.
                let (px, py, pw, ph) = if self.network_overlay() {
                    (0.0, HEADER_H, self.width, self.body_h())
                } else {
                    (px, py, pw, ph)
                };

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
                // The raised run floats clear of the plate's edge: offset past
                // the plate's rolled lip (bevel_width) plus the run's own boss
                // roll, so the two shadings never overlap — flush at the corner
                // they read as one clipped lump, not a part in front of a plate.
                let bc_pad = cce_ui::layout::bevel_width()
                    + cce_ui::layout::bevel_width().min(BREADCRUMB_H * 0.2);
                self.positions[BREADCRUMB_IDX] =
                    (px + bc_pad, py + mb_h + bc_pad, (pw - 2.0 * bc_pad).max(0.0), bc_h);
                self.positions[LEFT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);

                self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPREADSHEET_IDX] = rect_for(SPREADSHEET_IDX, self);
                self.slots.spreadsheet.set_rect(
                    self.positions[SPREADSHEET_IDX].0,
                    self.positions[SPREADSHEET_IDX].1,
                    self.positions[SPREADSHEET_IDX].2,
                    self.positions[SPREADSHEET_IDX].3,
                );

                // The second network editor: the pane-1 arrangement against
                // its own dock rect (plate, full-plate graph, hovering
                // breadcrumb at the same lip offset). Zero when unplaced or
                // waiting as a tab — rect_for already answers that.
                let (qx, qy, qw, qh) = rect_for(crate::slots::NETWORK_PANEL2_IDX, self);
                self.positions[crate::slots::NETWORK_PANEL2_IDX] = (qx, qy, qw, qh);
                self.slots.network_panel2.set_rect(qx, qy, qw, qh);
                if let Some(plate) =
                    self.slots.network_panel2.as_any_mut().downcast_mut::<PassivePlate>()
                {
                    plate.set_curved_circle(None);
                }
                self.positions[crate::slots::CONTENT2_IDX] = (qx, qy, qw, qh);
                let bc2 = if qw > 0.0 { BREADCRUMB_H } else { 0.0 };
                self.positions[crate::slots::BREADCRUMB2_IDX] =
                    (qx + bc_pad, qy + bc_pad, (qw - 2.0 * bc_pad).max(0.0), bc2);
                for (slot, on) in [
                    (crate::slots::NETWORK_PANEL2_IDX, qw > 0.0),
                    (crate::slots::CONTENT2_IDX, qw > 0.0),
                    (crate::slots::BREADCRUMB2_IDX, qw > 0.0),
                ] {
                    self.slots.get_dyn_mut(slot).set_visible(on);
                }

                self.positions[CANVAS_IDX] = (0.0, 0.0, self.width, body_h);
                self.positions[PARAM_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[STATUS_IDX] = (0.0, self.height - STATUS_H, self.width, STATUS_H);
                self.positions[PLAYBAR_IDX] = if self.show_playbar {
                    (gap, self.height - STATUS_H - gap - PLAYBAR_H, self.width - 2.0 * gap, PLAYBAR_H)
                } else {
                    (0.0, 0.0, 0.0, 0.0)
                };

                // Tab-aware visibility: a pane WAITING in a dock's tab list
                // is hidden regardless of its View flag — a zero rect alone
                // does not stop the text pass, so a waiting editor's labels
                // would paint over whichever pane fronted.
                let net_active = self.dock_of_pane(NETWORK_PANEL_IDX).is_some();
                let par_active = self.dock_of_pane(PARAM_IDX).is_some();
                let ss_active = self.dock_of_pane(SPREADSHEET_IDX).is_some();
                self.slots.header.set_visible(false);
                self.slots.status.set_visible(false);
                self.slots.playbar.set_visible(self.show_playbar);
                self.slots.content.set_visible(self.show_network && net_active);
                self.slots.network_panel.set_visible(self.show_network && net_active);
                self.slots.left_menubar.set_visible(false);
                self.slots.breadcrumb.set_visible(self.show_network && net_active);
                self.slots.splitter1.set_visible(false);
                self.slots.splitter2.set_visible(false);
                self.slots.viewport.set_visible(viewport_visible);
                self.slots.right_menubar.set_visible(false);
                self.slots.param.set_visible(right_visible && par_active);
                self.slots.param_menubar.set_visible(false);
                self.slots.spreadsheet.set_visible(spreadsheet_visible && ss_active);
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
        // The 2D page context takes the viewport's rect whenever the displayed
        // level holds a page, and the viewport stands down: one pane, one
        // thing in it. Placed here, after every layout branch has run, rather
        // than inside each of them — the rect it wants is always exactly the
        // viewport's, so there is nothing per-branch to decide.
        //
        // The ImageView's own image is the flag. Composing a page is
        // expensive and happens in rebuild_scene_geometry; layout runs on
        // every resize, and a second copy of "is a page showing" would be a
        // second thing to keep true.
        let showing_page = self.slots.page_view.image.is_some();
        self.positions[PAGE_IDX] = if showing_page {
            self.positions[VIEWPORT_IDX]
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        self.slots.page_view.set_visible(showing_page && self.slots.viewport.visible());
        if showing_page {
            self.slots.viewport.set_visible(false);
        }

        self.apply_collapsed_panes();
        // Last of all: the dialog floats over whatever the branches above
        // produced, so its rect depends on the window and nothing else.
        self.layout_dialog();
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

    /// Run a command by its registry id — the one entry point the chord, the
    /// palette and (soon) the menus all go through.
    ///
    /// Returns false for an id that names no command, which is what a stale
    /// `input.kdl` binding looks like.
    pub fn run_command(&mut self, id: &str) -> bool {
        let Some(cmd) = crate::command::by_id(id) else {
            self.update_status_text(&format!("No command named '{id}'"));
            return false;
        };
        match cmd.run {
            crate::command::Run::Key(action) => {
                self.execute_action(action);
                true
            }
            crate::command::Run::Menu(label) => self.execute_menu_action(label),
        }
    }

    /// The command context of the focused pane, for palette ranking.
    pub fn focused_context(&self) -> crate::command::Context {
        use crate::command::Context;
        match self.focused_pane {
            LEFT_MENUBAR_IDX => Context::Network,
            RIGHT_MENUBAR_IDX => Context::Viewport,
            PARAM_MENUBAR_IDX => Context::Parameters,
            SPREADSHEET_MENUBAR_IDX => Context::Spreadsheet,
            _ => Context::Always,
        }
    }

    /// Arrange the current level's nodes from their wiring.
    ///
    /// Reports how many moved: an arrange that did nothing — because the
    /// layout was already right — looks identical to one that is broken, and
    /// the status line is the only thing that can tell them apart.
    pub(crate) fn layout_current_level(&mut self) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        let nodes: Vec<crate::layout::LayoutNode> = self
            .current_dir()
            .children
            .iter()
            .map(|c| crate::layout::LayoutNode {
                name: c.name.clone(),
                input: c
                    .params
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case("input"))
                    .map(|p| p.default.clone()),
                position: c.position,
                // Utility trees stay where they were put; see the module doc.
                pinned: matches!(c.node_type.as_str(), "utility" | "session" | "meta"),
            })
            .collect();
        let moved = crate::layout::arrange(&nodes);
        let count = moved.len();
        {
            let dir = self.current_dir_mut();
            for (idx, pos) in moved {
                if let Some(child) = dir.children.get_mut(idx) {
                    child.position = pos;
                }
            }
        }
        if count > 0 {
            self.sync_nodes();
            self.sync_layout();
            // The cursor tracks the selection, which has just moved with its
            // node — otherwise the next keypress navigates from a cell the
            // selected node no longer occupies.
            let sel_pos = self
                .graph()
                .selected_node()
                .and_then(|sel| self.current_dir().children.get(sel).map(|c| c.position));
            if let Some((cx, cy)) = sel_pos {
                self.grid_cursor_col = cx as i32;
                self.grid_cursor_row = cy as i32;
            }
            self.keep_cursor_in_view();
            self.rebuild_positions();
            self.apply_layout();
            self.update_panel_bounds();
        }
        self.update_status_text(&match count {
            0 => "Layout: every node was already in place.".to_string(),
            1 => "Layout: moved 1 node.".to_string(),
            n => format!("Layout: moved {n} nodes."),
        });
        true
    }

    /// Clear the node selection, and make it stick.
    ///
    /// Returns false when nothing was selected, so Escape can fall through to
    /// meaning nothing rather than reporting that it did something.
    pub(crate) fn deselect_node(&mut self) -> bool {
        if self.graph().selected_node().is_none() {
            return false;
        }
        self.graph_mut().set_selected_node(None);
        self.deselected_cell = Some((self.grid_cursor_col, self.grid_cursor_row));
        if self.focused_widget == Some(CONTENT_IDX) {
            self.focused_widget = None;
        }
        self.sync_parameters_pane();
        true
    }

    /// Move the grid cursor one cell, taking the selection with it.
    ///
    /// The cursor is the network pane's keyboard position: `sync_cursor_and_
    /// selection` selects whatever node sits in its cell, so navigating IS
    /// selecting. Gated on the network pane having focus — plain h/j/k/l used
    /// to move it from any pane, which drifted the cursor invisibly while you
    /// were looking at the viewport and put it somewhere unexpected when you
    /// came back. Every other network key was already gated; this family was
    /// the one that was not.
    pub(crate) fn network_nav(&mut self, dc: i32, dr: i32) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.grid_cursor_col += dc;
        self.grid_cursor_row += dr;
        self.deselected_cell = None;
        self.sync_cursor_and_selection();
        self.keep_cursor_in_view();
        true
    }

    /// Move the node under the cursor one cell, and the cursor with it — so a
    /// run of alt+h drags a node across the sheet rather than leaving it
    /// behind on the first press.
    pub(crate) fn network_move_node(&mut self, dc: i32, dr: i32) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        let at_cursor = self.current_dir().children.iter().position(|child| {
            child.position.0 as i32 == self.grid_cursor_col
                && child.position.1 as i32 == self.grid_cursor_row
        });
        if let Some(idx) = at_cursor {
            let (x, y) = self.current_dir().children[idx].position;
            self.current_dir_mut().children[idx].position = (x + dc as f32, y + dr as f32);
            self.sync_nodes();
            self.sync_layout();
        }
        self.network_nav(dc, dr)
    }

    /// Pan the network view by one cell, leaving the cursor and the selection
    /// alone — the ctrl+hjkl family, which had no equivalent here.
    ///
    /// One cell, not a fixed pixel count, so a pan step means the same thing
    /// at every zoom level: the sheet moves by exactly one node.
    pub(crate) fn network_pan_view(&mut self, dc: i32, dr: i32) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.pan_x -= dc as f32 * (self.grid_size_x + self.gap_col_w);
        self.pan_y -= dr as f32 * (self.grid_size_y + self.gap_row_h);
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        true
    }

    /// Centre the view on the grid cursor, without changing the zoom — the
    /// counterpart of framing everything, and what `f` means in the plugin
    /// this is replacing.
    ///
    /// CENTRES rather than merely scrolling the cursor into view. The first
    /// version called `keep_cursor_in_view`, which pans only when the cursor
    /// has gone off the edge — so the command did nothing at all in the common
    /// case of a cursor that is already visible but off in a corner, which is
    /// exactly when you press it.
    pub(crate) fn frame_cursor(&mut self) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        let (_px, _py, pw, ph) = self.positions[CONTENT_IDX];
        if pw <= 0.0 || ph <= 0.0 {
            return true;
        }
        // Pan so the cursor CELL's centre lands at the pane's centre. The
        // cell's offset from the sheet origin is its column times the column
        // pitch, so the pan that centres it is the pane's half-extent minus
        // that offset, minus half a cell.
        self.pan_x = pw * 0.5
            - self.grid_cursor_col as f32 * (self.grid_size_x + self.gap_col_w)
            - self.grid_size_x * 0.5;
        self.pan_y = ph * 0.5
            - self.grid_cursor_row as f32 * (self.grid_size_y + self.gap_row_h)
            - self.grid_size_y * 0.5;
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.sync_grid_settings();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        true
    }

    /// Fit every node in the current level into the network pane.
    ///
    /// Extracted verbatim from the `f` key's inline arm so the command
    /// registry and the key run the same code — the point of the registry
    /// being that there is one implementation behind every way of asking.
    pub(crate) fn frame_all_nodes(&mut self) {
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
        self.sync_grid_settings();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
    }

    pub fn execute_action(&mut self, action: Action) {
        let mut settings_changed = false;
        match action {
            // Ctrl+P lands on Commands specifically, where Alt+D toggles the
            // dialog as a whole — the one difference between the two rows.
            Action::CommandPalette => {
                self.open_dialog();
                self.set_dialog_tab(crate::dialog::Tab::Commands);
            }
            Action::ToggleDialog => self.toggle_dialog(),
            // The network navigation families. Each returns false when the
            // network pane does not have focus, which is how one gate covers
            // all fourteen of them.
            Action::NetworkNav(dc, dr) => {
                self.network_nav(dc, dr);
            }
            Action::NetworkMove(dc, dr) => {
                self.network_move_node(dc, dr);
            }
            Action::NetworkPan(dc, dr) => {
                self.network_pan_view(dc, dr);
            }
            Action::ToggleNetworkPlate => {
                self.network_plate = !self.network_plate;
                self.rebuild_positions();
                self.apply_layout();
                self.update_status_text(if self.network_plate {
                    "Network plate on."
                } else {
                    "Network plate off — the graph overlays the scene."
                });
                // Saved through the same `settings_changed` path every other
                // viewport toggle uses, rather than a save call of its own.
                settings_changed = true;
            }
            Action::Deselect => {
                self.deselect_node();
            }
            Action::LayoutNodes => {
                self.layout_current_level();
            }
            Action::FrameCursor => {
                self.frame_cursor();
            }
            Action::FrameAll => {
                if self.focused_pane == LEFT_MENUBAR_IDX {
                    self.frame_all_nodes();
                }
            }
            Action::ToggleViewerState => {
                // The selected node, the same one the context menu's entry
                // would act on — so the command and the menu cannot disagree
                // about what "this node" means.
                match self.graph().selected_node() {
                    Some(slot) => {
                        self.toggle_viewer_state(slot);
                        if self.viewer_tool.is_none() {
                            self.update_status_text("Left the viewer state.");
                        }
                    }
                    None => self.update_status_text("Select a node to edit its handles."),
                }
            }
            Action::ToggleSnap => {
                if !self.toggle_viewer_snap() {
                    self.update_status_text("Snapping applies inside a viewer state.");
                }
            }
            // Undo/Redo reach whichever editing state owns a history. The
            // chords arrive through `Application::undo` / `redo` (the
            // toolkit routes them, after the focused text box's turn); the
            // Edit menu rows come here directly. The curve viewer state is
            // the only history so far; a project-wide one would be consulted
            // here after the tool declines.
            Action::Undo => {
                self.viewer_tool_undo();
            }
            Action::Redo => {
                self.viewer_tool_redo();
            }
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
            // Either play toggle PAUSES while anything is playing — direction
            // only chooses what starts from a stop. (Redirect-without-stopping
            // was tried and rejected: a moving timeline should always stop on
            // the first transport press.)
            Action::PlayPause | Action::PlayPauseReverse => {
                let pb = self.slots.playbar.inner_mut();
                if pb.playing {
                    pb.playing = false;
                } else {
                    pb.playing = true;
                    pb.reversed = action == Action::PlayPauseReverse;
                }
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
        // The grid cursor is PANE 1's concept: both network editors share
        // the LEFT_MENUBAR focus domain, so without the param_editor gate a
        // click in the second editor ran this and forced pane 1's selection
        // to whatever sat under its cursor cell — wiping the selection a
        // pinned params pane or spreadsheet was reading.
        if self.focused_pane != LEFT_MENUBAR_IDX || self.param_editor != CONTENT_IDX {
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

        // A selection that arrived from anywhere else — a click, a load, the
        // params pane — spends the remembered deselect. Otherwise clicking the
        // very node you deselected would clear itself again on this sync.
        if self.graph().selected_node().is_some() {
            self.deselected_cell = None;
        }

        // An explicit deselect holds for the cell it happened in, and for no
        // other: step away and selection resumes by itself.
        let node_at_cursor_idx = match self.deselected_cell {
            Some(cell) if cell == (self.grid_cursor_col, self.grid_cursor_row) => None,
            _ => node_at_cursor_idx,
        };

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
            WindowEvent::MouseWheel { delta } => {
                // The dialog is modal: a wheel over it scrolls it, and a wheel
                // anywhere else does nothing rather than scrolling — and
                // focusing — the pane it is covering.
                if self.dialog_visible() {
                    return self.dialog_mouse_wheel(*delta);
                }
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
                                // The Graph owns wheel→pan motion (cce-ui's
                                // ScrollMotion: notches glide, a trackpad
                                // tracks 1:1 and its flick coasts). Adopt
                                // wherever it moved the origin; the glide and
                                // coast advance in Graph::tick and are read
                                // back in tick_frame. A wheel also ends any
                                // drag-release fling still sliding.
                                self.pan_x = gx - active_node_area_x;
                                self.pan_y = gy - active_node_area_y;
                                self.pan_velocity_x = 0.0;
                                self.pan_velocity_y = 0.0;
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
                        // The pane hit but the Graph widget did not take the
                        // wheel (circular-pane hit shape wider than its rect,
                        // or a hidden graph): a plain instant pan.
                        let (dx, dy) = match delta {
                            MouseScrollDelta::LineDelta(x, y) => {
                                (*x * 30.0 * self.graph_scroll_speed, *y * 30.0 * self.graph_scroll_speed)
                            }
                            MouseScrollDelta::PixelDelta(pos) => (
                                (pos.x as f32 / self.scale as f32) * self.graph_scroll_speed,
                                (pos.y as f32 / self.scale as f32) * self.graph_scroll_speed,
                            ),
                        };
                        self.pan_x -= dx;
                        self.pan_y -= dy;
                        self.pan_velocity_x = 0.0;
                        self.pan_velocity_y = 0.0;
                        self.sync_grid_settings();
                        true
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

                // An in-flight curve-tool grab eats motion ahead of every
                // other drag: the grabbed control point tracks the cursor.
                if self.viewer_tool_drag_motion() {
                    return true;
                }

                // An in-flight camera orbit, likewise — it was armed by a press
                // on empty scene, so nothing else is competing for the motion.
                if let Some((lx, ly)) = self.orbit_drag {
                    let (dx, dy) = (self.cursor_x - lx, self.cursor_y - ly);
                    self.orbit_drag = Some((self.cursor_x, self.cursor_y));
                    if dx != 0.0 || dy != 0.0 {
                        self.orbit_camera_by(dx, dy);
                    }
                    return true;
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
                            } else if idx == crate::slots::DIALOG_PARAMS_IDX {
                                self.sync_dialog_settings_to_project();
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

                    self.viewport_mut().reset_velocity();
                }
                // The dialog is modal over what it covers, so it takes the
                // press before the pan trigger and before the whole
                // hit-target cascade. A press OUTSIDE it dismisses — the
                // convention every other floating surface in this app follows
                // (the node menu, the plate corner menu) — and is swallowed
                // rather than also acting on the pane it landed on.
                if self.dialog_visible() {
                    if let Some(handled) = self.dialog_mouse_input(*button, *btn_state) {
                        return handled;
                    }
                }
                let in_network_pane = self.in_network_pane();
                let node_area_x = self.positions[CONTENT_IDX].0;
                let node_area_y = self.positions[CONTENT_IDX].1;

                // Panning asks the AREA, not the nodes: with the plate off a
                // middle-drag over empty space still pans the graph, because
                // nothing else wants that gesture.
                let is_pan_trigger = self.in_network_area(self.cursor_x, self.cursor_y)
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
                    } else if state.network_overlay() && i == CONTENT_IDX {
                        // The graph's RECT spans the window in overlay mode,
                        // so the widget's own hit test would claim every press
                        // and there would be no way left to orbit the camera.
                        // It claims where it has drawn a node; the rest of the
                        // window is the scene. Same rule as `in_network_pane`,
                        // and the circular pane above refines its hit test the
                        // same way.
                        state.overlay_claims(x, y)
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

                        // The curve viewer state takes the viewport press
                        // ahead of the context menu and the click cascade:
                        // left grabs or adds a control point, right on a
                        // handle deletes it (right elsewhere still opens the
                        // viewport menu below).
                        if self.viewer_tool.is_some()
                            && self.cursor_in_viewport()
                            && !in_circle_network_pane
                        {
                            if *button == MouseButton::Left {
                                if self.viewer_tool_press() {
                                    return true;
                                }
                            } else if *button == MouseButton::Right
                                && self.viewer_tool_delete_at_cursor()
                            {
                                return true;
                            }
                        }

                        // A left press on empty scene arms a camera orbit.
                        // AFTER the viewer state above, so dragging a handle
                        // still edits it, and after the node hit tests, so a
                        // press on a node is still the node's — "empty" means
                        // the scene really is what is under the cursor. The
                        // camera otherwise turns only by scrolling, which is
                        // the trackpad gesture; this is the mouse's.
                        if *button == MouseButton::Left
                            && self.cursor_in_viewport()
                            && !in_circle_network_pane
                            && self.app_drag.is_none()
                        {
                            self.orbit_drag = Some((self.cursor_x, self.cursor_y));
                            self.focused_pane = RIGHT_MENUBAR_IDX;
                            if let Some(old) = self.focused_widget {
                                self.slots.get_dyn_mut(old).unfocus();
                                self.focused_widget = None;
                            }
                            self.sync_pane_focus();
                            return true;
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
                                let z = if i == NETWORK_PANEL_IDX || i == crate::slots::NETWORK_PANEL2_IDX {
                                    // Plates sort behind their content, or the
                                    // stable sort hands the plate every press
                                    // and the graph never hears a click.
                                    -5
                                } else if i == crate::slots::BREADCRUMB2_IDX {
                                    1
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
                            if i == LEFT_MENUBAR_IDX
                                || i == CONTENT_IDX
                                || i == CANVAS_IDX
                                || i == BREADCRUMB_IDX
                                || i == NETWORK_PANEL_IDX
                                || i == crate::slots::CONTENT2_IDX
                                || i == crate::slots::BREADCRUMB2_IDX
                                || i == crate::slots::NETWORK_PANEL2_IDX
                            {
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
                            if i == crate::slots::CONTENT2_IDX {
                                // A node click in the SECOND editor hands the
                                // parameters pane (and the viewport's level)
                                // to it.
                                if self.param_editor != crate::slots::CONTENT2_IDX {
                                    self.param_editor = crate::slots::CONTENT2_IDX;
                                    if self.viewport_pin.is_none() {
                                        self.rebuild_scene_geometry();
                                    }
                                }
                                self.sync_parameters_pane();
                            }
                            if i == CONTENT_IDX {
                                if self.param_editor != CONTENT_IDX {
                                    self.param_editor = CONTENT_IDX;
                                    if self.viewport_pin.is_none() {
                                        self.rebuild_scene_geometry();
                                    }
                                }
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
                        if self.orbit_drag.take().is_some() {
                            return true;
                        }
                        if self.viewer_tool_release() {
                            changed = true;
                        }
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
                            } else if idx == crate::slots::CONTENT2_IDX {
                                // The second editor's node drags write back to
                                // ITS level, or the next sync snaps them home.
                                {
                                    let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn WidgetHost + 'static);
                                    unsafe { (*ptr).handle_event(&cce_ui::widget::Event::DragEnd, &mut self.ui_context); }
                                }
                                use cce_ui::widget::GraphController as _;
                                let updated_nodes = self.slots.content2.get_nodes();
                                let p2 = self.current_path2.clone();
                                let dir = self.dir_at_mut(&p2);
                                for (i, node) in updated_nodes.iter().enumerate() {
                                    if let Some(child) = dir.children.get_mut(i) {
                                        child.position = node.position;
                                    }
                                }
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
                    self.current_dir_mut().set_child_geometry_visible(i, visible);
                    // The widget only flipped its own copy of the clicked node;
                    // the exclusivity rule may have cleared siblings (in both
                    // editors' views), so push the model back out.
                    self.sync_nodes();
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

                // A node dropped onto a wire splices in between its ends:
                // the dragged node inherits the wire's upstream as its
                // Input, and the wire's downstream node re-aims its Input
                // at the dragged node. Both rewires or neither — a splice
                // that only cut the wire would silently orphan downstream.
                if let Some((mid_id, src_name, dest_id)) = self.graph_mut().take_pending_splice() {
                    let dir = self.current_dir_mut();
                    let mid_name = dir
                        .children
                        .iter()
                        .find(|c| c.id == mid_id)
                        .map(|c| c.name.clone());
                    let both_rewirable = mid_name.is_some()
                        && dir.children.iter().any(|c| {
                            c.id == dest_id && c.params.iter().any(|p| p.name == "Input")
                        })
                        && dir.children.iter().any(|c| {
                            c.id == mid_id && c.params.iter().any(|p| p.name == "Input")
                        });
                    if let (Some(mid_name), true) = (mid_name, both_rewirable) {
                        if let Some(mid) = dir.children.iter_mut().find(|c| c.id == mid_id) {
                            if let Some(p) = mid.params.iter_mut().find(|p| p.name == "Input") {
                                p.default = src_name;
                            }
                        }
                        if let Some(dest) = dir.children.iter_mut().find(|c| c.id == dest_id) {
                            if let Some(p) = dest.params.iter_mut().find(|p| p.name == "Input") {
                                p.default = mid_name;
                            }
                        }
                        self.sync_nodes();
                        self.rebuild_scene_geometry();
                        self.sync_parameters_pane();
                        changed = true;
                    }
                }

                // The SECOND network editor's drains — the same handshakes,
                // against ITS OWN level (`current_path2` via the clamping
                // dir_at walk). Selection stays pane-1's affair for now: the
                // params pane follows the primary editor.
                {
                    use cce_ui::widget::GraphController as _;
                    if let Some((i, visible)) = self.slots.content2.take_node_geom_toggle() {
                        let p2 = self.current_path2.clone();
                        let dir = self.dir_at_mut(&p2);
                        if i < dir.children.len() {
                            dir.set_child_geometry_visible(i, visible);
                            self.sync_nodes();
                            self.rebuild_scene_geometry();
                            changed = true;
                        }
                    }
                    if let Some((input_node_id, output_node_name)) =
                        self.slots.content2.take_pending_connection()
                    {
                        let p2 = self.current_path2.clone();
                        let dir = self.dir_at_mut(&p2);
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
                    if let Some((mid_id, src_name, dest_id)) =
                        self.slots.content2.take_pending_splice()
                    {
                        let p2 = self.current_path2.clone();
                        let dir = self.dir_at_mut(&p2);
                        let mid_name = dir
                            .children
                            .iter()
                            .find(|c| c.id == mid_id)
                            .map(|c| c.name.clone());
                        let both = mid_name.is_some()
                            && dir.children.iter().any(|c| {
                                c.id == dest_id && c.params.iter().any(|p| p.name == "Input")
                            })
                            && dir.children.iter().any(|c| {
                                c.id == mid_id && c.params.iter().any(|p| p.name == "Input")
                            });
                        if let (Some(mid_name), true) = (mid_name, both) {
                            if let Some(mid) = dir.children.iter_mut().find(|c| c.id == mid_id) {
                                if let Some(p) = mid.params.iter_mut().find(|p| p.name == "Input") {
                                    p.default = src_name;
                                }
                            }
                            if let Some(dest) = dir.children.iter_mut().find(|c| c.id == dest_id) {
                                if let Some(p) = dest.params.iter_mut().find(|p| p.name == "Input") {
                                    p.default = mid_name;
                                }
                            }
                            self.sync_nodes();
                            self.rebuild_scene_geometry();
                            self.sync_parameters_pane();
                            changed = true;
                        }
                    }
                    if let Some(dir_idx) = self.slots.content2.double_clicked_node() {
                        self.slots.content2.clear_double_clicked_node();
                        let p2 = self.current_path2.clone();
                        let dir = self.dir_at(&p2);
                        if dir_idx < dir.children.len() && dir.children[dir_idx].is_enterable() {
                            self.current_path2.push(dir_idx);
                            self.sync_nodes();
                            // The viewport tracks its editor's level.
                            if self.viewport_editor() == crate::slots::CONTENT2_IDX {
                                self.rebuild_scene_geometry();
                            }
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

                // The dialog owns the keyboard outright while it is open —
                // ahead of the context chords, ahead of the params pane, ahead
                // of everything. It has a text field in it, and a modal whose
                // typing leaks into the pane behind it is worse than no modal:
                // typing "frame" into the filter would step the grid cursor
                // four times and toggle a node's geometry on the way past.
                if self.dialog_visible() {
                    return self.dialog_key_input(event);
                }

                // Context switching dispatches ahead of the widget key paths
                // (param pane, node palette) so the chord works from any pane.
                if event.state == ElementState::Pressed {
                    if let Some(id @ ("next_context" | "previous_context")) =
                        self.shortcut_manager.match_command(&self.modifiers, &event.logical_key)
                    {
                        self.run_command(id);
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
                // Steps auto-repeat (holding Right scrubs); the play toggles
                // fire once per physical press, or a held key would flicker
                // play/pause at the repeat rate.
                if event.state == ElementState::Pressed {
                    if let Some(id @ ("play_pause" | "play_pause_reverse" | "frame_next" | "frame_prev")) =
                        self.shortcut_manager.match_command(&self.modifiers, &event.logical_key)
                    {
                        let is_toggle = matches!(id, "play_pause" | "play_pause_reverse");
                        if !(is_toggle && event.repeat) {
                            self.run_command(id);
                        }
                        return true;
                    }
                }

                if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) {
                    // An open context menu — node, viewport, or plate-corner,
                    // all riding the shared context_menu thread-local — takes
                    // Escape ahead of connection-cancel. They were
                    // mouse-dismiss only, which left Escape wired to a
                    // cancel_connecting the user could not see happening.
                    if cce_ui::widget::context_menu::is_visible() {
                        if self.node_menu_open() {
                            self.close_node_menu();
                        } else if self.viewport_menu_open() {
                            self.close_viewport_menu();
                        } else {
                            self.close_plate_menu();
                        }
                        return true;
                    }
                    // The curve viewer state exits on Escape, ahead of
                    // connection-cancel — leaving point-edit mode is the
                    // more immediate "get me out" while it is active.
                    if self.viewer_tool.is_some() {
                        self.viewer_tool = None;
                        return true;
                    }
                    self.graph_mut().cancel_connecting();
                    // Last: clearing the selection. Escape is this app's one
                    // "get me out" key, and the things above are all more
                    // immediate than a selection — a menu you can see, a mode
                    // you are in, a wire you are dragging.
                    self.deselect_node();
                    return true;
                }
                // Delete/Backspace removes the curve tool's selected control
                // point. Ahead of the network pane's node-delete, which only
                // runs when no tool selection consumed the key; after the
                // param pane's shot above, so a focused text field keeps
                // Backspace for its caret.
                if event.state == ElementState::Pressed
                    && (event.logical_key == Key::Named(NamedKey::Delete)
                        || event.logical_key == Key::Named(NamedKey::Backspace))
                    && self.viewer_tool_delete_selected()
                {
                    return true;
                }
                let mut changed = false;
                if event.state == ElementState::Pressed {
                        let is_plain_key = !self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key();
                        let is_ctrl_only = self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key() && !self.modifiers.shift_key();

                        // The hjkl navigation families used to be decoded
                        // here, inline and unrebindable, with the bare form
                        // ungated so it drifted the cursor from any pane.
                        // They are registry commands now (nav_*, move_*,
                        // view_*, frame_*), matched with every other chord
                        // below.
                        {
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
                                                        self.current_dir_mut().set_child_geometry_visible(slot_idx, visible);
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
                                                regenerate_node_ids(&mut node);
                                                let start_x = self.grid_cursor_col as f32;
                                                let start_y = self.grid_cursor_row as f32;
                                                let (nx, ny) = self.find_empty_cell(start_x, start_y, None);
                                                node.position = (nx, ny);
                                                // Added = hidden, same as AddNode: a
                                                // paste of a displayed node must not
                                                // become a second visible sibling.
                                                node.geometry_visible = false;
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
                        if let Some(id) = self.shortcut_manager.match_command(&self.modifiers, &event.logical_key) {
                            self.pending_command = Some(id);
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

        // A replacement renderer left the page pane with no image; recompose
        // and re-upload it now that the frame has settled.
        if std::mem::take(&mut self.page_dirty) {
            self.rebuild_page();
        }

        // Drop-target glow animation: exponential smoothing toward the live
        // target — the position GLIDES between cells, alpha fades in while a
        // drag is in flight and out after it ends (lingering at the last
        // cell). Exponential rates are frame-rate independent.
        let mut glow_animating = false;
        {
            let target = self.graph().drop_target_cell_rect();
            let ease = |k: f32| 1.0 - (-k * dt.max(1e-4)).exp();
            match (&mut self.drop_glow, target) {
                (Some(g), Some((tx, ty, tw, th))) => {
                    let move_f = ease(18.0);
                    g.x += (tx - g.x) * move_f;
                    g.y += (ty - g.y) * move_f;
                    g.w += (tw - g.w) * move_f;
                    g.h += (th - g.h) * move_f;
                    g.alpha += (1.0 - g.alpha) * ease(14.0);
                    glow_animating = (tx - g.x).abs() > 0.3
                        || (ty - g.y).abs() > 0.3
                        || g.alpha < 0.995;
                    if !glow_animating {
                        g.x = tx;
                        g.y = ty;
                        g.alpha = 1.0;
                    }
                    // A settled glow still needs redraws only while the drag
                    // moves it — PointerMove frames cover that.
                }
                (Some(g), None) => {
                    g.alpha -= g.alpha * ease(10.0);
                    if g.alpha < 0.02 {
                        self.drop_glow = None;
                    }
                    glow_animating = true;
                }
                (None, Some((tx, ty, tw, th))) => {
                    self.drop_glow = Some(DropGlow { x: tx, y: ty, w: tw, h: th, alpha: 0.0 });
                    glow_animating = true;
                }
                (None, None) => {}
            }
        }

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

        let mut tick_changed = false;
        let mut param_ticked = false;
        let mut graph_ticked = false;
        let ctx = &mut self.ui_context;
        for i in 0..WIDGET_COUNT {
            if self.slots.get_dyn_mut(i).tick(dt, ctx) {
                tick_changed = true;
                if i == PARAM_IDX {
                    param_ticked = true;
                }
                if i == CONTENT_IDX {
                    graph_ticked = true;
                }
            }
        }
        // Every slot (and, through the adapter, its embedded children) was
        // just ticked by hand. The runner ticks the context's tick_receivers
        // right after Application::tick, which would tick the Graph,
        // Spreadsheet and every TextBox a second time per frame — their
        // ScrollMotion integrates dt, so a double tick runs each glide and
        // coast at double speed. Drain the roster so the runner's pass is a
        // no-op; paint re-registers the slots before the next frame.
        self.ui_context.tick_receivers.clear();
        if graph_ticked {
            // Graph::tick advanced its wheel glide / trackpad coast: adopt the
            // origin it moved to, as the wheel path does, so a later
            // sync_grid_settings cannot snap the canvas back to a stale pan.
            let (ax, ay, _, _) = self.positions[CONTENT_IDX];
            let (gx, gy) = self.graph().grid_origin();
            let (nx, ny) = (gx - ax, gy - ay);
            if nx != self.pan_x || ny != self.pan_y {
                self.pan_x = nx;
                self.pan_y = ny;
                self.sync_grid_settings();
                self.rebuild_positions();
                self.apply_layout();
                self.update_panel_bounds();
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

        // Drag-release fling: the velocity a middle / space+left drag had
        // when the button went up keeps the canvas sliding under friction.
        // (Wheel and trackpad motion coast inside the Graph widget instead.)
        if !self.is_panning && (self.pan_velocity_x.abs() > 0.01 || self.pan_velocity_y.abs() > 0.01) {
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

        tick_changed || panned || reclaimed || glow_animating
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
    /// Note that a renderer has been handed over, and invalidate anything
    /// that cannot survive a REPLACEMENT one. Returns whether this was a
    /// replacement.
    ///
    /// Split out of `renderer_init` so it can be tested: the callback needs a
    /// live `VkRenderer`, this needs nothing. There is no separate reconnect
    /// callback — the runner calls `renderer_init` once per renderer, so the
    /// first call is this process's own and every later one is a replacement,
    /// and remembering is the only way to tell them apart.
    ///
    /// Images uploaded outside that callback are not replayed into the new
    /// renderer, so a cached id names nothing and its draws are skipped in
    /// SILENCE — the page pane simply went blank. Freeing the stale id is
    /// safe (destroy_image returns early on an id the new table lacks, ids
    /// come from a counter that never resets, and free_image only queues), and
    /// the raster recomposes from the node graph cheaply, so dropping and
    /// re-uploading beats trying to preserve anything.
    pub fn renderer_handed_over(&mut self) -> bool {
        if !std::mem::replace(&mut self.seen_renderer, true) {
            return false;
        }
        if let Some(old) = self.page_image.take() {
            cce_ui::vk::free_image(old);
        }
        self.slots.page_view.set_image(None);
        // Re-uploaded on the next tick, not here: this runs before the frame
        // has settled, and rebuild_page relays the panes.
        self.page_dirty = true;
        true
    }

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

