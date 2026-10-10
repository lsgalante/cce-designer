//! The node tree and the project file: `FsNode` (one node and its
//! children), `Project` (what a save holds, its load, migrations and
//! template merge), node ids, the wire and splice operations on a directory
//! of nodes, parameter rows as the panes list them, and `load_fs_tree`.
//! Plain data and functions over it — no window, no GPU. Split out of
//! app.rs on 2026-10-10; app.rs re-exports it, so `crate::app::FsNode` and
//! the rest still resolve.
#![allow(unused_imports)]
use std::time::Instant;
use std::fs;
use std::path::Path;
use std::net::TcpListener;
use std::io::BufReader;

use serde::{Deserialize, Serialize};

use cce_ui::widget::{ElementState, MouseButton, MouseScrollDelta, KeyEvent, Key, NamedKey, WidgetHostExt};

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
use crate::detail::Detail;
use crate::slots::*;
use crate::shortcut::{ShortcutManager, Action};
use cce_ui::vk::{SceneDraw, TextSpan};
use cce_ui::engine::Vertex;
use crate::window::WindowEvent;

use crate::app::*;

pub(crate) static NODE_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn generate_node_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = NODE_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("node_{:x}_{:x}", now, count)
}

pub(crate) fn default_node_inputs() -> usize { 1 }
pub(crate) fn default_node_outputs() -> usize { 1 }

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
    /// Bypassed: the node is in the graph and does nothing. What reads it
    /// gets what it reads — its `Input`, untouched — and a node with no
    /// input gives nothing. Written only when set, so a file that never
    /// bypassed anything is the file it was.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bypassed: bool,
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
        matches!(self.node_type.as_str(), "node" | "simnet" | "repeat" | crate::context::GEOMETRY)
            || !self.children.is_empty()
    }

    /// Set one child's geometry visibility. Enabling is EXCLUSIVE within the
    /// directory — at most one node per path shows its geometry, so turning a
    /// node on turns every sibling off (a display flag, not a per-node render
    /// flag). Disabling touches only the named child. Every toggle route
    /// (keyboard `e`, the graph widgets' click toggles, MCP/context-menu
    /// ToggleGeometry) must go through here or the invariant silently rots.
    ///
    /// Exclusive within its CONTEXT, since 2026-09-29: the page nodes and the
    /// geometry nodes each have a display flag of their own, so a level shows
    /// one image and one geometry — a picture behind the model drawn over
    /// it. Until then a page took the viewport's pane whole and one flag did.
    ///
    /// A GEOMETRY CONTAINER's flag is its own (since 2026-10-02): the root is
    /// the object level, where each geometry node is shown or not, as
    /// Houdini's objects are, and several draw at once. Showing one leaves
    /// its siblings as they are, and showing anything else leaves it.
    pub fn set_child_geometry_visible(&mut self, slot: usize, visible: bool) {
        if slot >= self.children.len() {
            return;
        }
        if visible && crate::context::is_geometry_container(&self.children[slot].node_type) {
            self.children[slot].geometry_visible = true;
        } else if visible {
            let page = crate::page::is_page_node(&self.children[slot].node_type);
            for (i, child) in self.children.iter_mut().enumerate() {
                if crate::context::is_geometry_container(&child.node_type) {
                    continue;
                }
                if crate::page::is_page_node(&child.node_type) == page {
                    child.geometry_visible = i == slot;
                }
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


pub(crate) fn default_node_type() -> String { "node".to_string() }
pub(crate) fn default_node_geometry_visible() -> bool { true }
pub(crate) fn is_false(flag: &bool) -> bool { !*flag }
pub(crate) fn default_node_position() -> (f32, f32) { (0.0, 0.0) }

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
    /// Which panes are OPEN, by name ("network", "viewport", "parameters",
    /// "spreadsheet", "playbar").
    ///
    /// Pane visibility rode the root meta node's `view` subnet params into
    /// the file until 2026-09-23 — five toggles on a node that existed to
    /// hold them. It is pane state like the collapse list and the splitter
    /// proportions below, so it sits with them. `None` (older saves, and
    /// saves written before the move) keeps the live layout, which is what
    /// the absent-value case always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_panes: Option<Vec<String>>,
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
    // `dock_tabs`, what each dock held, rode here until 2026-10-07, when
    // the docks went; an older save's is ignored.
    // `current_path2` and the viewport / params / spreadsheet pins rode
    // here until 2026-10-07, for the second network editor; an older save's
    // are ignored.
    /// The floating layout's plate geometry — every edge the user can drag
    /// — as window fractions, so a project restores its plate sizes and
    /// positions at any window size (the splitter convention). Absent in
    /// older saves keeps the live geometry.
    #[serde(default)]
    pub plates: Option<PlateGeometry>,
    /// The Default Camera view — the viewport settings that have no node
    /// to live on when no camera node is active: the square aspect, the
    /// camera-pivot marker and its size, and the view itself (orbit, zoom,
    /// pivot). Absent in older saves keeps the live values. A named camera's
    /// own params still win over these when it is active and in the
    /// directory.
    #[serde(default)]
    pub default_view: Option<DefaultCameraView>,
    /// The display settings the project was saved with — the grid and the
    /// other guides, the wireframe, shading, opacity, points and colours:
    /// everything `DesignSettings` persists but the startup pointer. A load
    /// applies them over the live state (`State::apply_display_settings`),
    /// so a project opens looking the way it was left. Absent in saves from
    /// before 2026-09-24 keeps the live settings, as every block here does.
    ///
    /// These were deliberately app-wide from 2026-09-23 — "a display
    /// setting belongs to the view" — and came back into the file by the
    /// user's choice the next day. state.kdl still holds them too, as the
    /// last-used look: what a new project and an older save open with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<DisplaySettings>,
    /// The playbar's frame range, `(start, end)` — project state, since a
    /// simulation is made for a length; absent in an older save, which
    /// keeps the live range. Set from the playbar menu's Start Frame and
    /// End Frame sliders (2026-09-30).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_range: Option<(i32, i32)>,
}

/// The display half of `DesignSettings` — see [`ProjectViewState::display`].
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DisplaySettings {
    #[serde(default)]
    pub viewport: ViewportSettings,
    #[serde(default)]
    pub render: RenderSettings,
}

/// See [`ProjectViewState::default_view`].
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct DefaultCameraView {
    pub square: bool,
    pub show_pivot: bool,
    pub pivot_size: f32,
    pub rotation: (f32, f32),
    pub zoom: f32,
    pub pivot: [f32; 3],
}

/// The user-dragged plate edges, each as a fraction of the window
/// dimension it spans: the params HUD's width and the spreadsheet's height.
/// Everything else about where the plates stand is derived by
/// `rebuild_positions`. (Until 2026-10-07 this also held the left and
/// right docks' widths and the spreadsheet's tucks under them; an older
/// save's are ignored.)
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct PlateGeometry {
    pub spreadsheet_height: f32,
    /// The params HUD's width (since 2026-10-06, when it left the right
    /// dock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hud_width: Option<f32>,
    /// The right dock's width in an older save: read as the HUD's when the
    /// save has no `hud_width` (one from before the HUD left the dock, when
    /// it was the params pane's). Never written.
    #[serde(default, skip_serializing)]
    pub params_width: Option<f32>,
}

pub(crate) fn default_camera() -> String {
    "Default Camera".to_string()
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Project {
    pub name: String,
    pub root: FsNode,
    #[serde(default)]
    pub view_state: ProjectViewState,
    /// The file format this project was saved in, so a load can tell an old
    /// meaning from a new one. 0 (absent) is every save before 2026-09-24,
    /// when a bare `ch("Name")` meant the PARENT's parameter; 1 is Houdini's
    /// semantics, where it means the node's own; 2 (2026-10-01) is the
    /// generators' normal attribute called `N` where it was `Norm`, and 3
    /// (the same day) their texture coordinates `uv` where they were `UV`;
    /// 4 (the same day) parameter names as identifiers; 5 (2026-10-02) the
    /// root as the object level, its geometry inside a `geometry` node
    /// (`context::wrap_root_geometry`); 6 (2026-10-06) a range's two ends as
    /// one `float2` (`Project::migrate_range_rows`); 7 (the same day)
    /// Composite's Length as the length of Name, not of Source B
    /// (`Project::migrate_composite_length`); 8 (the same day) Develop's
    /// Direction as an attribute name (`Project::migrate_develop_direction`).
    /// `migrate_format` takes a file through each step it is behind, and a
    /// step must not run twice.
    #[serde(default)]
    pub format: u32,
}

/// The format `Project` saves in — see its `format` field.
pub const PROJECT_FORMAT: u32 = 8;


/// A node's wires as the network draws them: every parameter of the `node`
/// kind, in order — the k-th is input port k — as (name, the node it names).
/// A wire whose row is hidden (`show_when`) names nothing here: it keeps its
/// port and draws no line, since what is not on screen should not be. An
/// expression wire names what it EVALUATES to at `frame` — the Remesh
/// subnet's transfer reads its From through `if(chs("../from"), …,
/// "input1")`, and is drawn from input1 — and nothing when it fails.
/// Auto-layout reads the same wires.
pub fn node_wires_at(root: &FsNode, node: &FsNode, frame: i32) -> Vec<(String, String)> {
    if !node.params.iter().any(|p| p.kind() == ParamKind::Node && p.is_expr()) {
        return node_wires(node);
    }
    match crate::geometry::resolve_param_refs(root, node, frame, &mut None) {
        Some(resolved) => node_wires(&resolved),
        None => node_wires(node),
    }
}

/// Rewire around the child at `slot` of `dir`, ahead of its removal: every
/// sibling's wire that names it — `Input` or a second operand, any shown,
/// plain `node` parameter — takes the name the child's own `Input` wire
/// carries. Nothing changes when that wire is empty, an expression, hidden
/// or names the child itself, and a sibling is never wired to itself.
pub(crate) fn splice_out(dir: &mut FsNode, slot: usize) {
    let Some(gone) = dir.children.get(slot) else { return };
    let name = gone.name.clone();
    let Some(upstream) = gone
        .params
        .iter()
        .find(|p| p.name == "input" && p.kind() == ParamKind::Node && !p.is_expr() && param_visible(&gone.params, &p.show_when))
        .map(|p| p.text().trim().to_string())
        .filter(|u| !u.is_empty() && *u != name)
    else {
        return;
    };
    for (i, sibling) in dir.children.iter_mut().enumerate() {
        if i == slot || sibling.name == upstream {
            continue;
        }
        for p in sibling.params.iter_mut() {
            if p.kind() == ParamKind::Node && !p.is_expr() && p.text().trim() == name {
                p.set_text(upstream.clone());
            }
        }
    }
}

/// Splice the child `mid_id` of `dir` into the wire from `src_name` to
/// `dest_id`: the middle node takes the wire's upstream as its Input, and
/// the downstream node re-aims its Input at the middle one. Both rewires or
/// neither — a splice that only cut the wire would orphan downstream — so
/// false, and nothing written, when either node has no Input. What a node
/// dropped onto a wire runs, and Add Node on a cell a wire runs through.
pub(crate) fn splice_into_wire(dir: &mut FsNode, mid_id: &str, src_name: String, dest_id: &str) -> bool {
    splice_chain_into_wire(dir, mid_id, mid_id, src_name, dest_id)
}

/// [`splice_into_wire`] for a chain: its `head` takes the wire's upstream,
/// and the downstream node reads its `tail`. A pasted chain goes in whole.
pub(crate) fn splice_chain_into_wire(dir: &mut FsNode, head_id: &str, tail_id: &str, src_name: String, dest_id: &str) -> bool {
    let has_input = |id: &str| dir.children.iter().any(|c| c.id == id && c.params.iter().any(|p| p.name == "input"));
    let Some(tail_name) = dir.children.iter().find(|c| c.id == tail_id).map(|c| c.name.clone()) else { return false };
    if !has_input(head_id) || !has_input(dest_id) {
        return false;
    }
    for (id, wire) in [(head_id, src_name), (dest_id, tail_name)] {
        if let Some(p) = dir.children.iter_mut().find(|c| c.id == id).and_then(|c| c.params.iter_mut().find(|p| p.name == "input")) {
            p.set_text(wire);
        }
    }
    true
}

/// Swap the places of the children `a_id` and `b_id` of `dir` in the
/// graph: each takes the other's wires, and every wire that named the one
/// names the other. The positions are the widget's to trade (a node dropped
/// on a node, `Graph::set_swap_on_drop`); this trades the connections, so
/// in I → A → B → C dragging B onto A gives I → B → A → C — the chain's
/// order, not just the picture of it. Written as a renaming σ (A ↔ B) of
/// what each wire holds: a third node's wire w becomes σ(w); A's k-th wire
/// becomes σ of B's k-th and B's σ of A's, port for port, so a wire between
/// the two turns round. A port only one of them has keeps its own wire,
/// σ'd. An expression wire is moved as it is, unrewritten: what it names
/// is not a text to rename. False, and nothing written, when either is not
/// there.
pub(crate) fn swap_places(dir: &mut FsNode, a_id: &str, b_id: &str) -> bool {
    let (Some(ai), Some(bi)) = (
        dir.children.iter().position(|c| c.id == a_id),
        dir.children.iter().position(|c| c.id == b_id),
    ) else {
        return false;
    };
    if ai == bi {
        return false;
    }
    let (a, b) = (dir.children[ai].name.clone(), dir.children[bi].name.clone());
    let sigma = |w: &str| -> String {
        let t = w.trim();
        if t == a {
            b.clone()
        } else if t == b {
            a.clone()
        } else {
            w.to_string()
        }
    };
    // Each wire port of a node, in order: (param index, text, expression).
    let ports = |n: &FsNode| -> Vec<(usize, String, bool)> {
        n.params
            .iter()
            .enumerate()
            .filter(|(_, p)| p.kind() == ParamKind::Node)
            .map(|(i, p)| (i, p.text().to_string(), p.is_expr()))
            .collect()
    };
    let (pa, pb) = (ports(&dir.children[ai]), ports(&dir.children[bi]));
    let moved = |(text, expr): (&String, bool)| if expr { (text.clone(), true) } else { (sigma(text), false) };
    // What each of the two will hold, port for port, from the wires as
    // they stood.
    let new_for = |own: &[(usize, String, bool)], other: &[(usize, String, bool)]| -> Vec<(usize, String, bool)> {
        own.iter()
            .enumerate()
            .map(|(k, (i, text, expr))| {
                let (t, e) = match other.get(k) {
                    Some((_, ot, oe)) => moved((ot, *oe)),
                    None => moved((text, *expr)),
                };
                (*i, t, e)
            })
            .collect()
    };
    let (na, nb) = (new_for(&pa, &pb), new_for(&pb, &pa));
    for (slot, child) in dir.children.iter_mut().enumerate() {
        if slot == ai || slot == bi {
            continue;
        }
        for p in child.params.iter_mut() {
            if p.kind() == ParamKind::Node && !p.is_expr() {
                let t = sigma(p.text());
                if t != p.text() {
                    p.set_text(t);
                }
            }
        }
    }
    for (slot, wires) in [(ai, na), (bi, nb)] {
        for (i, text, expr) in wires {
            let p = &mut dir.children[slot].params[i];
            p.set_text(text);
            p.set_expr(expr);
        }
    }
    true
}

/// [`node_wires_at`] with nothing evaluated: an expression wire names
/// nothing.
pub fn node_wires(node: &FsNode) -> Vec<(String, String)> {
    node.params
        .iter()
        .filter(|p| p.kind() == ParamKind::Node)
        .map(|p| {
            let shown = param_visible(&node.params, &p.show_when) && !p.is_expr();
            (p.name.clone(), if shown { p.text().trim().to_string() } else { String::new() })
        })
        .collect()
}

/// The run a parameter belongs to, for the pane's separators: the
/// template's `group`, or for the LEADING run of wires — the node's inputs
/// at the top of its list — a group of their own, so every node with an
/// input has a line under it without a template saying so. A wire further
/// down (Relax's Rest, under its Mode) is part of whatever run it is in.
pub fn param_group(params: &[ParamDef], i: usize) -> String {
    let p = &params[i];
    if !p.group.is_empty() {
        return p.group.clone();
    }
    let leading_wire = params[..=i].iter().all(|q| q.kind() == ParamKind::Node && q.group.is_empty());
    if leading_wire { PARAM_INPUTS_GROUP.to_string() } else { String::new() }
}

/// The group the leading wires are in when a template names none.
pub const PARAM_INPUTS_GROUP: &str = "inputs";

/// A separator row in `param_display`'s output: cce-ui's `separator` row,
/// with no key and no value, so the write-back finds no parameter by it.
pub const PARAM_SEPARATOR: &str = "separator";

/// The params pane's rows: every SHOWN parameter as (display key, value,
/// row type), with a separator row wherever two shown rows belong to
/// different groups ([`param_group`]) — between the inputs and the rest,
/// and between runs of parameters about different things. Never first or
/// last, and never two in a row, since one is only ever placed between two
/// shown parameters.
pub fn param_display(params: &[ParamDef]) -> Vec<(String, String, String)> {
    // Rows whose condition does not hold are not shown. Write-back resolves a
    // row by its display key rather than by position, so a hidden parameter
    // simply is not reported and keeps whatever value it had.
    let mut out = Vec::new();
    let mut last_group: Option<String> = None;
    for (i, p) in params.iter().enumerate() {
        if !param_visible(params, &p.show_when) {
            continue;
        }
        let group = param_group(params, i);
        if last_group.as_ref().is_some_and(|g| *g != group) {
            out.push((String::new(), String::new(), PARAM_SEPARATOR.to_string()));
        }
        last_group = Some(group);
        out.push(param_row(p));
    }
    out
}

/// One parameter as the pane's row: its display key, its value as the row
/// shows it, and the row type.
pub(crate) fn param_row(p: &ParamDef) -> (String, String, String) {
    {
        let key = p.shown_name();
        let kind = p.kind();
        let value = if p.ty() == "choice" && !p.options().is_empty() && p.text().is_empty() {
            p.options()[0].clone()
        } else {
            p.text().to_string()
        };
        // An expression (`ch("../sphere1/radius") * 2`) is shown as the text
        // it is: a slider cannot hold it, and a spinbox would show zero and
        // then write zero back over it.
        let ptype = if p.is_expr() {
            "text".to_string()
        } else if matches!(kind, ParamKind::Text | ParamKind::Float | ParamKind::Node | ParamKind::Attribute | ParamKind::Group) {
            // The pane has no numeric-text or node-picker row; both are a
            // text box there. `string` (an absent type) is one too — the
            // pane does not know that word and would draw nothing. An
            // attribute or group name is a text box as well, upgraded to a
            // `textpick` row by `add_pick_lists` when the input has names
            // to offer.
            "text".to_string()
        } else if kind == ParamKind::Float2 {
            // Two sliders over a soft range around the value, as the
            // Attribute node's Value row is; the pane's own pass keeps the
            // span a row already has (`State::present_float2_rows`). A text
            // that is not two numbers stays a text box, to be put right.
            match p.value() {
                Some(ParamValue::Vec2(v)) => float2_row(value_row_span(v, None)),
                _ => "text".to_string(),
            }
        } else if p.ty() == "slider" {
            let (min, max, _) = p.range().expect("a slider has a range");
            format!("slider:{}:{}", min, max)
        } else if p.ty() == "float3" {
            // A position or a size is set a component at a time: the ball
            // is there for the asking (the row menu), not by default.
            let (min, max, _) = p.range().expect("a float3 has a range");
            float3_row(min, max, p.wants_trackball(false))
        } else if p.ty() == "spinbox" {
            let (min, max, step) = p.range().expect("a spinbox has a range");
            format!("spinbox:{}:{}:{}", min as i32, max as i32, step.unwrap_or(1.0) as i32)
        } else if p.ty() == "choice" {
            format!("choice:{}", p.options().join(","))
        } else {
            p.ty().to_string()
        };
        (key.to_string(), value, ptype)
    }
}

pub fn flatten_node_templates(root: &FsNode) -> Vec<NodeTemplate> {
    let mut out = Vec::new();
    for child in &root.children {
        out.push(NodeTemplate { label: child.name.clone(), node: child.clone() });
    }
    out
}

/// The template an instance came from: a native node by TYPE, a subnet
/// instance by NAME with its index stripped ("sphere3" → "Sphere",
/// case-insensitively). The one rule the template merge, and everything
/// that asks what a node's template says (the row menu's `Default:`),
/// resolve by.
pub fn template_for<'a>(node: &FsNode, templates: &'a [NodeTemplate]) -> Option<&'a FsNode> {
    if node.node_type.eq_ignore_ascii_case("node") {
        let base = node
            .name
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .trim_end_matches(|c: char| c == '_' || c.is_whitespace());
        // Case-insensitively: the template is "Sphere", its instances
        // are "sphere1".
        templates.iter().map(|t| &t.node).find(|t| {
            t.node_type.eq_ignore_ascii_case("node")
                && (t.name.eq_ignore_ascii_case(&node.name)
                    || (!base.is_empty() && t.name.eq_ignore_ascii_case(base)))
        })
    } else {
        templates.iter().map(|t| &t.node).find(|t| {
            !t.node_type.eq_ignore_ascii_case("node")
                && t.node_type.eq_ignore_ascii_case(&node.node_type)
        })
    }
}

/// Strip the per-node `meta` (preferences) children from a loaded tree.
///
/// Every geometry node used to carry one, holding four display switches —
/// Point Markers, Point Numbers, Point Normals, Wireframe. They were
/// retired on 2026-09-23: what they controlled is a property of the VIEW,
/// not of the scene, so it belongs to the viewport's own settings and the
/// command palette (`toggle_point_markers` and friends), not to a hidden
/// child you had to dive into a node to find and set one node at a time.
///
/// A migration rather than a no-op because those children ride saved
/// projects: left in place they would show in every network as a child that
/// does nothing, and the loader would keep them alive forever. Their VALUES
/// are deliberately dropped — four per-node booleans do not reduce to one
/// global switch, and inferring one ("any node asked for markers") would
/// turn one node's preference into a setting over the whole scene.
///
/// The root `meta` node is a different thing entirely and is not touched
/// here: it is the session-settings container, reached through `fs_root`'s
/// own children rather than as a child of a placed node.
pub fn strip_meta_children(root: &mut FsNode) {
    fn strip(node: &mut FsNode) {
        node.children.retain(|c| c.node_type != "meta");
        for c in &mut node.children {
            strip(c);
        }
    }
    // Entered through the root's children, exactly as `ensure_meta_children`
    // was: the root is a container, not a placed node, and its own direct
    // `meta` child is the SESSION node, which this must not take.
    for c in &mut root.children {
        strip(c);
    }
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
/// ("sphere3" → "Sphere" — and "sphere_3", should someone type one — so a
/// renamed instance simply keeps its saved
/// shape), and only merge when EVERY template child is present by name and
/// type — a hand-built subnet that happens to share the name is left alone,
/// and nothing is ever injected or deleted. Simnet children (the user's sim
/// chain) are out of scope by construction: simnet is a native type.
/// A node name as this app will keep it: lowercase, no whitespace.
///
/// A node's name is a segment of its path — `/sphere1/opencl1` is how the
/// breadcrumb, the MCP tools and every `Input` wire name it — and a path
/// with spaces in it is a path that has to be quoted everywhere it goes.
/// So names do not carry them. The one space that was CONVENTIONAL, the one
/// between a template's name and its index ("Sphere 1"), simply goes, so a
/// migrated save reads like a fresh one; any other whitespace becomes an
/// underscore, so "My Region" keeps its two words. And the whole thing is
/// lowercased, as Houdini names its nodes (`sphere1`, `camera1`), since a
/// path convention with exceptions is two conventions. Empty comes back as
/// `node`, since a node with no name has no path at all.
/// The playbar's cache strip, frame by frame over `start..=end` and run
/// together: a frame is CACHED when every simnet in the tree has it in hand
/// (at or before its start it shows its seed, which every simnet has), STALE
/// when every one has it and one of them has it from the chain as it was —
/// before an edit it went on across (`stale_to`), or before an edit not yet
/// solved (`chains` differing from the solve's) — and in no run otherwise.
/// A solve of a simnet not in `chains` (deleted since) does not count, and
/// with none that does there is nothing to show.
pub fn playbar_cache_runs(
    solved: &[crate::geometry::SolvedRange],
    chains: &std::collections::HashMap<String, u64>,
    start: i32,
    end: i32,
) -> Vec<crate::playbar::CacheRun> {
    use crate::playbar::{CacheRun, CacheState};
    let sims: Vec<(&crate::geometry::SolvedRange, bool)> =
        solved.iter().filter_map(|r| chains.get(&r.id).map(|c| (r, *c != r.chain))).collect();
    let mut runs: Vec<CacheRun> = Vec::new();
    if sims.is_empty() {
        return runs;
    }
    for f in start..=end {
        let mut held = Some(CacheState::Cached);
        for (r, edited) in &sims {
            let this = if f <= r.start {
                Some(CacheState::Cached)
            } else if f > r.reach {
                None
            } else if *edited || r.stale_to.is_some_and(|t| f <= t) {
                Some(CacheState::Stale)
            } else {
                Some(CacheState::Cached)
            };
            held = match (held, this) {
                (None, _) | (_, None) => None,
                (Some(CacheState::Stale), _) | (_, Some(CacheState::Stale)) => Some(CacheState::Stale),
                _ => Some(CacheState::Cached),
            };
        }
        let Some(state) = held else { continue };
        match runs.last_mut() {
            Some(last) if last.state == state && last.to == f - 1 => last.to = f,
            _ => runs.push(CacheRun { from: f, to: f, state }),
        }
    }
    runs
}

pub fn sanitize_node_name(name: &str) -> String {
    let lowered = name.to_lowercase();
    let trimmed = lowered.trim();
    // "Sphere 1" → "Sphere1": drop the whitespace between a base and a
    // trailing run of digits.
    let digits = trimmed.trim_end_matches(|c: char| c.is_ascii_digit());
    let (base, index) = trimmed.split_at(digits.len());
    let base = if index.is_empty() { base } else { base.trim_end() };
    let mut out = String::with_capacity(trimmed.len());
    let mut in_space = false;
    for c in base.chars() {
        if c.is_whitespace() {
            in_space = true;
        } else {
            if in_space {
                out.push('_');
                in_space = false;
            }
            out.push(c);
        }
    }
    out.push_str(index);
    if out.is_empty() {
        "node".to_string()
    } else {
        out
    }
}

/// The parameter MCP means by `name`: the one of that name, else the one
/// whose label it is (in any case — what the pane shows is what a person
/// reads off it), else the one its [`param_name_of`] names, so a script
/// written against the old names (`Base Resolution`) still reaches
/// `base_resolution`.
pub fn param_by_name_or_label<'a>(params: &'a mut [ParamDef], name: &str) -> Option<&'a mut ParamDef> {
    let i = params
        .iter()
        .position(|p| p.name == name)
        .or_else(|| params.iter().position(|p| !p.label.is_empty() && p.label.eq_ignore_ascii_case(name)))
        .or_else(|| {
            let snake = param_name_of(name);
            params.iter().position(|p| p.name == snake)
        })?;
    params.get_mut(i)
}

/// A channel path with its parameter segment — the last, less a `.x` /
/// `.y` / `.z` component — renamed by [`param_name_of`]: format 4's
/// rewrite of what an expression names. None when nothing changes.
pub(crate) fn param_path_renamed(path: &str) -> Option<String> {
    let (head, last) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let (name, comp) = match last.rfind('.') {
        Some(i) if i > 0 && matches!(&last[i..], ".x" | ".y" | ".z") => (&last[..i], &last[i..]),
        _ => (last, ""),
    };
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let renamed = param_name_of(name);
    (renamed != name && !renamed.is_empty()).then(|| format!("{head}{renamed}{comp}"))
}

impl Project {
    /// Bring a loaded project's node names under [`sanitize_node_name`],
    /// and follow every reference to a renamed node.
    ///
    /// Saves from before 2026-09-21 carry "Sphere 1" and "Camera 1", and
    /// wires are BY NAME — a node's `Input` (and `With`, `Rest`, `Target`,
    /// `Source`, `Collider`) holds the name of the node it reads — so a
    /// rename that left the references alone would cut every wire in the
    /// file. Names are unique within a level and references stay within a
    /// level, so each level is handled on its own: rename its children, then
    /// rewrite any sibling parameter whose value was one of the old names.
    /// The view state's active camera is the one reference outside the tree.
    /// Runs before the template merge on every load path, so what the merge
    /// matches ("Sphere1" → "Sphere") is already the kept spelling.
    pub fn sanitize_node_names(&mut self) {
        fn walk(dir: &mut FsNode, renamed: &mut Vec<(String, String)>) {
            // The names as loaded: a sanitized name must not land on a
            // sibling's — "Sphere 1" next to a hand-named "Sphere1" — or two
            // nodes share one name and every wire to them is ambiguous. A
            // later sibling still carries its loaded name; an earlier one
            // carries what this pass gave it.
            let loaded: Vec<String> = dir.children.iter().map(|c| c.name.clone()).collect();
            let mut taken: Vec<String> = Vec::new();
            let mut map: Vec<(String, String)> = Vec::new();
            for (i, child) in dir.children.iter_mut().enumerate() {
                let mut name = sanitize_node_name(&child.name);
                let clashes = |n: &str| taken.iter().any(|t| t == n) || loaded[i + 1..].iter().any(|t| t == n);
                if name != child.name && clashes(&name) {
                    let mut n = 2;
                    while clashes(&format!("{name}_{n}")) {
                        n += 1;
                    }
                    name = format!("{name}_{n}");
                }
                if name != child.name {
                    map.push((child.name.clone(), name.clone()));
                    child.name = name.clone();
                }
                taken.push(name);
            }
            if !map.is_empty() {
                for child in &mut dir.children {
                    for p in &mut child.params {
                        if let Some((_, new)) = map.iter().find(|(old, _)| *old == p.text()) {
                            p.set_text(new.clone());
                        }
                    }
                }
                renamed.extend(map);
            }
            for child in &mut dir.children {
                walk(child, &mut Vec::new());
            }
        }
        let mut root_renamed = Vec::new();
        walk(&mut self.root, &mut root_renamed);
        if let Some((_, new)) = root_renamed.iter().find(|(old, _)| *old == self.view_state.active_camera) {
            self.view_state.active_camera = new.clone();
        }
    }
}

impl Project {
    /// Take a loaded project through every format step it is behind, then
    /// call it current. Runs beside `sanitize_node_names` on every load
    /// path; a step must not run twice, which is what the version is for.
    ///
    /// Format 0 → 1: every parameter that was a pre-expression reference —
    /// the whole value `ch("Name")`, `chf` / `chi` / `chb`, with `../` per
    /// level — becomes an expression with Houdini's semantics. A bare name
    /// meant the parent, so it gains `../`; an explicit `../` already meant
    /// what it means now. Runs beside `sanitize_node_names` on every load
    /// path, and on a format-1 file does nothing, which is what the version
    /// is for: a bare name in a NEW file is the node's own parameter and
    /// must not be rewritten.
    pub fn migrate_format(&mut self) {
        if self.format < 1 {
            self.migrate_param_refs();
        }
        if self.format < 2 {
            self.rename_attribute("Norm", "N");
        }
        if self.format < 3 {
            self.rename_attribute("UV", "uv");
        }
        if self.format < 4 {
            self.migrate_param_names();
        }
        if self.format < 5 {
            crate::context::wrap_root_geometry(self);
        }
        if self.format < 6 {
            self.migrate_range_rows();
        }
        if self.format < 7 {
            self.migrate_composite_length();
        }
        if self.format < 8 {
            self.migrate_develop_direction();
        }
        self.format = PROJECT_FORMAT;
    }

    /// Format 7 → 8: Develop's Direction names the vector attribute points
    /// move along, where it was a choice — Normal, or Attribute with the
    /// attribute in a Source row. Normal becomes `N` (the normal, worked out
    /// when the input carries no N, as before), Attribute becomes what Source
    /// named, its text and expression flag both, and the Source row goes. A
    /// node from before the choice, with neither row, is left to the merge,
    /// which gives it `N`: what it moved along.
    fn migrate_develop_direction(&mut self) {
        fn walk(node: &mut FsNode) {
            if node.node_type.eq_ignore_ascii_case("develop") {
                if let Some(si) = node.params.iter().position(|p| p.name == "source") {
                    let source = node.params.remove(si);
                    if let Some(d) = node.params.iter_mut().find(|p| p.name == "direction") {
                        let by_attr = !d.is_expr() && d.text().trim().eq_ignore_ascii_case("attribute");
                        if by_attr && !source.text().trim().is_empty() {
                            d.set_text(source.text().to_string());
                            d.set_expr(source.is_expr());
                        } else {
                            d.set_text("N".to_string());
                            d.set_expr(false);
                        }
                    }
                }
            }
            for c in &mut node.children {
                walk(c);
            }
        }
        walk(&mut self.root);
    }

    /// Format 6 → 7: Composite's Length is the length of NAME, where it was
    /// the length of Source B written into Name (or into Result, for the
    /// day Result existed before this). So a saved Length node computes
    /// what it did with Source B moved into Name and the attribute it wrote
    /// named as Result — which, existing, keeps its type and takes the
    /// length in every component, as before. Text and expression flag move
    /// together. A node whose Source B is empty failed before and is left
    /// as it is. A channel path elsewhere that reads one of these rows is
    /// not followed: they hold attribute names, which nothing reads that way.
    fn migrate_composite_length(&mut self) {
        fn walk(node: &mut FsNode) {
            let is = |n: &FsNode, row: &str, v: &str| node_param_str(n, row, "").trim().eq_ignore_ascii_case(v);
            if node.node_type.eq_ignore_ascii_case("attribute")
                && is(node, "operation", "composite")
                && is(node, "combine_op", "length")
            {
                let row = |n: &FsNode, r: &str| n.params.iter().find(|p| p.name == r).cloned();
                if let (Some(name), Some(b)) = (row(node, "attribute_name"), row(node, "source_b")) {
                    if !b.text().trim().is_empty() {
                        let result_empty = row(node, "result").is_none_or(|r| r.text().trim().is_empty());
                        let set = |node: &mut FsNode, r: &str, from: &ParamDef| {
                            match node.params.iter_mut().find(|p| p.name == r) {
                                Some(p) => {
                                    p.set_text(from.text().to_string());
                                    p.set_expr(from.is_expr());
                                }
                                None => {
                                    let mut p = ParamDef::new(r, "attribute", from.text());
                                    p.set_expr(from.is_expr());
                                    node.params.push(p);
                                }
                            }
                        };
                        set(node, "attribute_name", &b);
                        if result_empty {
                            set(node, "result", &name);
                        }
                        set(node, "source_b", &ParamDef::new("source_b", "attribute", ""));
                    }
                }
            }
            for c in &mut node.children {
                walk(c);
            }
        }
        walk(&mut self.root);
    }

    /// Format 5 → 6: a range is one `float2` row where it was two numbers.
    /// The Attribute node's From Min / From Max become From, To Min / To
    /// Max become To, and To Max is also Normalize To, the goal Normalize
    /// read off it; Visualize's From / To become Manual Range. Each pair's
    /// two texts are joined as they were written (`0.00:1.00`), and when
    /// either was an expression so is the whole — a float2's components are
    /// each an expression, as a float3's are. A half the save lacks takes
    /// the default. What names the old rows follows: a channel path to
    /// `from_min` reads `from.x`, to `to_max` `to.y` (or `normalize_to` on
    /// a node set to Normalize, which is what it meant there), Visualize's
    /// `from` `manual_range.x`. Paths are RESOLVED, from where their holder
    /// stands — `from` and `to` are wires on Transfer, Copy and Distance,
    /// and those are not touched.
    fn migrate_range_rows(&mut self) {
        // What a retired row is now, on the node that has it.
        fn now(node: &FsNode, param: &str) -> Option<&'static str> {
            if !node.params.iter().any(|p| p.name == param) {
                return None;
            }
            let normalize = node_param_str(node, "operation", "").eq_ignore_ascii_case("normalize");
            Some(match (node.node_type.to_ascii_lowercase().as_str(), param) {
                ("attribute", "from_min") => "from.x",
                ("attribute", "from_max") => "from.y",
                ("attribute", "to_min") => "to.x",
                ("attribute", "to_max") if normalize => "normalize_to",
                ("attribute", "to_max") => "to.y",
                ("visualize", "from") if visualize_pair(node) => "manual_range.x",
                ("visualize", "to") if visualize_pair(node) => "manual_range.y",
                _ => return None,
            })
        }
        // Visualize's From and To are the numbers this step joins, not a
        // later row of the same name.
        fn visualize_pair(node: &FsNode) -> bool {
            !node.params.iter().any(|p| p.name == "manual_range")
        }
        // Pass one, over the tree as it stands: every path that names one.
        fn collect(root: &FsNode, node: &FsNode, edits: &mut Vec<(String, String, String)>) {
            for p in node.params.iter().filter(|p| p.is_expr() || p.kind() == ParamKind::Code) {
                let text = crate::expr::rewrite_paths(p.text(), |path| {
                    let (id, _, param) = crate::geometry::ref_path_target(root, node, path)?;
                    let target = crate::viewer_state::find_node_by_id(root, &id)?;
                    let new = now(target, &param)?;
                    Some(match path.trim().rsplit_once('/') {
                        Some((head, _)) => format!("{head}/{new}"),
                        None => new.to_string(),
                    })
                });
                if text != p.text() {
                    edits.push((node.id.clone(), p.name.clone(), text));
                }
            }
            for c in &node.children {
                collect(root, c, edits);
            }
        }
        let mut edits = Vec::new();
        collect(&self.root, &self.root, &mut edits);
        for (id, name, text) in edits {
            let node = if self.root.id == id { Some(&mut self.root) } else { crate::viewer_state::find_node_by_id_mut(&mut self.root, &id) };
            if let Some(p) = node.and_then(|n| n.params.iter_mut().find(|p| p.name == name)) {
                p.set_text(text);
            }
        }

        // Pass two: the rows themselves.
        fn join(node: &mut FsNode, lo: &str, hi: &str, name: &str, keep_hi: Option<&str>) {
            let Some(at) = node.params.iter().position(|p| p.name == lo || p.name == hi) else { return };
            let half = |node: &FsNode, n: &str, default: &str| {
                node.params.iter().find(|p| p.name == n).map_or((default.to_string(), false), |p| (p.text().trim().to_string(), p.is_expr()))
            };
            let (a, ae) = half(node, lo, "0.00");
            let (b, be) = half(node, hi, "1.00");
            let mut joined = ParamDef::new(name, "float2", format!("{a}:{b}"));
            if ae || be {
                joined.set_expr(true);
            }
            let mut extra = None;
            if let Some(extra_name) = keep_hi {
                if let Some(p) = node.params.iter().find(|p| p.name == hi) {
                    let mut q = ParamDef::new(extra_name, "float", p.text());
                    q.set_expr(p.is_expr());
                    extra = Some(q);
                }
            }
            node.params.retain(|p| p.name != lo && p.name != hi);
            let at = at.min(node.params.len());
            node.params.insert(at, joined);
            if let Some(q) = extra {
                node.params.insert(at + 1, q);
            }
        }
        fn walk(node: &mut FsNode) {
            match node.node_type.to_ascii_lowercase().as_str() {
                "attribute" => {
                    join(node, "from_min", "from_max", "from", None);
                    join(node, "to_min", "to_max", "to", Some("normalize_to"));
                }
                "visualize" if visualize_pair(node) && node.params.iter().any(|p| p.name == "from" || p.name == "to") => {
                    join(node, "from", "to", "manual_range", None);
                }
                _ => {}
            }
            for c in &mut node.children {
                walk(c);
            }
        }
        walk(&mut self.root);
    }

    /// Format 3 → 4: a parameter's NAME is an identifier
    /// (`base_resolution`), where it was the text the pane showed (`Base
    /// Resolution`) — that is the template's label now. Every parameter is
    /// renamed by [`param_name_of`], and what spells a name follows: each
    /// channel path in an expression and in a wrangle's Code, whose last
    /// segment is a parameter (`chf("../sphere1/Radius")` →
    /// `chf("../sphere1/radius")`). `show_when` is left alone: the template
    /// merge replaces it, and `page::migrate_preset_rows` recognises an old
    /// page by the condition as it was. The retired settings nodes are
    /// skipped — `Project::migrate_meta_settings_node` reads them by the
    /// names they were saved with, after this.
    fn migrate_param_names(&mut self) {
        fn walk(node: &mut FsNode) {
            if matches!(node.node_type.as_str(), "session" | "meta" | "utility") {
                return;
            }
            for p in &mut node.params {
                let name = param_name_of(&p.name);
                if !name.is_empty() {
                    p.name = name;
                }
                if p.is_expr() || p.kind() == ParamKind::Code {
                    let text = crate::expr::rewrite_paths(p.text(), param_path_renamed);
                    if text != p.text() {
                        p.set_text(text);
                    }
                }
            }
            for c in &mut node.children {
                walk(c);
            }
        }
        walk(&mut self.root);
    }

    /// Format 0 → 1, as above.
    fn migrate_param_refs(&mut self) {
        fn walk(node: &mut FsNode) {
            for p in &mut node.params {
                if !p.is_expr() {
                    if let Some(new) = crate::expr::migrate_legacy_ref(p.text()) {
                        p.set_text(new);
                        p.set_expr(true);
                    }
                }
            }
            for c in &mut node.children {
                walk(c);
            }
        }
        walk(&mut self.root);
    }

    /// A step that renames an attribute the generators write, `old` to
    /// `new` — format 1 → 2 `Norm` → `N` (as the Normal node, the exporter
    /// and a wrangle's `@N` already named it), 2 → 3 `UV` → `uv` (the
    /// lowercase every other built-in name has) — and what names it in a
    /// save follows: a parameter naming an attribute
    /// (`ParamKind::Attribute`) that says `old`, a name in a comma list of
    /// attributes (Transfer's and the Remesh's `Attributes`), and `@old` in
    /// a wrangle's Code. A choice row is not touched (the Sphere's Method
    /// keeps its `UV` option). Once, by the version — an attribute someone
    /// names `old` after this is theirs.
    fn rename_attribute(&mut self, old: &str, new: &str) {
        fn walk(node: &mut FsNode, old: &str, new: &str) {
            for p in &mut node.params {
                if p.is_expr() {
                    continue;
                }
                let text = p.text().to_string();
                let renamed = if p.kind() == ParamKind::Attribute && text.trim() == old {
                    Some(new.to_string())
                } else if p.name == "Attributes" && text.split(',').any(|a| a.trim() == old) {
                    Some(text.split(',').map(|a| if a.trim() == old { a.replace(old, new) } else { a.to_string() }).collect::<Vec<_>>().join(","))
                } else if p.kind() == ParamKind::Code && text.contains(&format!("@{old}")) {
                    Some(rename_at_attribute(&text, old, new))
                } else {
                    None
                };
                if let Some(renamed) = renamed.filter(|n| *n != text) {
                    p.set_text(renamed);
                }
            }
            for c in &mut node.children {
                walk(c, old, new);
            }
        }
        walk(&mut self.root, old, new);
    }
}

/// `code` with every `@old` that is a whole name (not `@Normal` for
/// `@Norm`) written `@new`.
pub(crate) fn rename_at_attribute(code: &str, old: &str, new: &str) -> String {
    let pat = format!("@{old}");
    let renamed = format!("@{new}");
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(i) = rest.find(&pat) {
        let after = &rest[i + pat.len()..];
        let whole = !after.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
        out.push_str(&rest[..i]);
        out.push_str(if whole { &renamed } else { &pat });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// A template's parameters whose defaults READ as expressions become ones:
/// `chf("../radius")` in `embryo.json` is a reference by any reading, and a
/// template author should not have to say `"expr": true` beside it (though
/// they may). `looks_like_expression` is the inference, and it is the same
/// one a typed or scripted value gets — arithmetic alone is not enough.
pub fn infer_template_exprs(node: &mut FsNode) {
    for p in &mut node.params {
        if !p.is_expr() && p.takes_expressions() && crate::expr::looks_like_expression(p.text()) {
            p.set_expr(true);
        }
    }
    for c in &mut node.children {
        infer_template_exprs(c);
    }
}

pub fn merge_template_defs(root: &mut FsNode, templates: &[NodeTemplate]) {
    // The retired per-node `meta` children go first, before any matching:
    // the merge compares a subnet instance's children against its template's
    // name-for-name, and a stale `meta` on one side and not the other is a
    // mismatch that costs the instance its refresh. Here rather than at the
    // call sites because this is the one function EVERY deserialization runs
    // — file load, the sync-channel reload, the thumbnail and the export CLI
    // — and a migration missed at one load path is the whole failure mode.
    strip_meta_children(root);

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

    // A NATIVE embryo (node type "embryo", 2026-09-21 only) is recomposed as
    // an instance of the Embryo template, which is the same pipeline as a
    // network: id, name, position, display flag and every parameter value
    // carry over by name, and the template's children arrive with fresh ids. Wholesale rather than through the
    // merge below, which never injects children.
    //
    // The Remesh the same way (since 2026-09-30): a native `remesh` becomes
    // the Remesh subnet. Its Split, Collapse, Flip and Project switches are
    // not rows of the subnet — the passes are nodes inside it — so one that
    // was off BYPASSES its node there, which is what it meant.
    fn recompose_native_embryo(node: &mut FsNode, templates: &[NodeTemplate]) {
        for c in &mut node.children {
            if c.node_type.eq_ignore_ascii_case("remesh") {
                if let Some(t) = templates.iter().find(|t| t.node.name == "Remesh" && t.node.node_type == "node") {
                    let mut fresh = t.node.clone();
                    regenerate_node_ids(&mut fresh);
                    fresh.id = c.id.clone();
                    fresh.name = c.name.clone();
                    fresh.position = c.position;
                    fresh.geometry_visible = c.geometry_visible;
                    fresh.bypassed = c.bypassed;
                    for p in &c.params {
                        if let Some(fp) = fresh.params.iter_mut().find(|fp| fp.name == p.name) {
                            fp.set_text(p.text().to_string());
                            fp.set_expr(p.is_expr());
                        }
                    }
                    for (switch, pass) in crate::geometry::REMESH_PASS_SWITCHES {
                        let on = c.params.iter().find(|p| p.name == *switch).map_or(true, |p| {
                            !["false", "0", "off"].contains(&p.text().trim().to_ascii_lowercase().as_str())
                        });
                        if !on {
                            if let Some(n) = fresh.children.iter_mut().find(|k| k.node_type == "repeat").and_then(|r| r.children.iter_mut().find(|k| k.name == *pass)) {
                                n.bypassed = true;
                            }
                        }
                    }
                    *c = fresh;
                }
            }
            if c.node_type.eq_ignore_ascii_case("embryo") {
                if let Some(t) = templates.iter().find(|t| t.node.name == "Embryo") {
                    let mut fresh = t.node.clone();
                    regenerate_node_ids(&mut fresh);
                    fresh.id = c.id.clone();
                    fresh.name = c.name.clone();
                    fresh.position = c.position;
                    fresh.geometry_visible = c.geometry_visible;
                    fresh.bypassed = c.bypassed;
                    for p in &c.params {
                        if let Some(fp) = fresh.params.iter_mut().find(|fp| fp.name == p.name) {
                            fp.set_text(p.text().to_string());
                        }
                    }
                    *c = fresh;
                }
            }
            recompose_native_embryo(c, templates);
        }
    }
    recompose_native_embryo(root, templates);

    // The Remesh subnet's switch was `result1` for its first day
    // (2026-09-30), and is `transfer_switch1`: a saved Remesh is renamed to
    // match, its output's wire with it, or it would stop matching the
    // template by its children's names and take no template change again.
    fn rename_remesh_switch(node: &mut FsNode) {
        for c in &mut node.children {
            if c.node_type == "node"
                && c.children.iter().any(|k| k.name == "result1" && k.node_type == "switch")
                && c.children.iter().any(|k| k.name == "repeat1" && k.node_type == "repeat")
                && !c.children.iter().any(|k| k.name == "transfer_switch1")
            {
                for k in &mut c.children {
                    if k.name == "result1" {
                        k.name = "transfer_switch1".into();
                    }
                    for p in &mut k.params {
                        if p.kind() == ParamKind::Node && p.text().trim() == "result1" {
                            p.set_text("transfer_switch1".to_string());
                        }
                    }
                }
            }
            rename_remesh_switch(c);
        }
    }
    rename_remesh_switch(root);

    // A KERNEL SUBNET — a Sphere, Box, Plane or Extrude instance saved while
    // those templates were `input → opencl → output` subnets (until
    // 2026-09-24) — becomes the native node of that type: id, name,
    // position, flag and parameter values stay, the children go, and a
    // parameter the native template does not have (the Box's unused Input)
    // goes with them. Matched by the same base-name rule `template_for`
    // used to match them to their templates, and only when an `opencl`
    // child is actually there, so a subnet someone built by hand and
    // happened to call "sphere2" keeps whatever is inside it.
    fn nativize_kernel_subnets(node: &mut FsNode, templates: &[NodeTemplate]) {
        for c in &mut node.children {
            if c.node_type.eq_ignore_ascii_case("node")
                && c.children.iter().any(|k| k.node_type.eq_ignore_ascii_case("opencl"))
            {
                let base = c
                    .name
                    .trim_end_matches(|ch: char| ch.is_ascii_digit())
                    .trim_end_matches(|ch: char| ch == '_' || ch.is_whitespace())
                    .to_lowercase();
                if ["sphere", "box", "plane", "extrude"].contains(&base.as_str()) {
                    c.node_type = base.clone();
                    c.children.clear();
                    if let Some(t) = templates.iter().find(|t| t.node.node_type.eq_ignore_ascii_case(&base)) {
                        c.params.retain(|p| t.node.params.iter().any(|tp| tp.name == p.name));
                        c.inputs = t.node.inputs;
                        c.outputs = t.node.outputs;
                    }
                }
            }
            nativize_kernel_subnets(c, templates);
        }
    }
    nativize_kernel_subnets(root, templates);

    fn merge_params(node: &mut FsNode, template: &FsNode) {
        // A missing parameter goes where the TEMPLATE puts it — after the
        // last template parameter the instance already has — not at the
        // end. The pane's order is the template's statement of what matters
        // first (the Sphere's Method sits above the Radius it governs), and
        // an old save that appended every later control below Color was
        // showing a different node from a fresh one.
        let mut cursor = 0;
        for tp in &template.params {
            if let Some(i) = node.params.iter().position(|p| p.name == tp.name) {
                cursor = i + 1;
                // Type, label, options, range, step and the show-when
                // condition — the condition is UI metadata like the rest:
                // the template owns when a control applies, the instance
                // owns its value. Without it a saved project keeps the pane
                // it had on the day it was made, and a node that later
                // learned to hide its irrelevant rows would not hide them
                // there. The value is re-parsed under the template's kind,
                // so an old save's text wire is a node wire from here on.
                node.params[i].adopt_ui_from(tp);
            } else {
                node.params.insert(cursor, tp.clone());
                cursor += 1;
            }
        }
    }
    fn merge_node(node: &mut FsNode, templates: &[NodeTemplate]) {
        // A camera's Square Aspect and Show Camera Pivot were written when
        // the viewport's toggles flipped under it and read by nothing: the
        // settings are the viewport's, and ride the project's view state.
        // Retired, and dropped from a save that still has them.
        if node.node_type == "camera" {
            node.params.retain(|p| p.name != "square_aspect" && p.name != "show_camera_pivot");
        }
        // A page's size is its Width and Height now, Preset has no Custom
        // and there is no Orientation row: a save from before is carried
        // over once, ahead of the merge that would take the old rows'
        // conditions away.
        if node.node_type == "page" {
            crate::page::migrate_preset_rows(node);
        }
        // Visualize's Mix blend was Set under another name (Opacity fades
        // every blend alike) and is retired; a save holding it is Set, or
        // it would load as a choice the row no longer offers.
        if node.node_type == "visualize" {
            if let Some(p) = node.params.iter_mut().find(|p| p.name == "blend" && p.text().trim().eq_ignore_ascii_case("mix")) {
                p.set_text("Set".to_string());
            }
        }
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
                        if let Some(t_code) = tc.params.iter().find(|p| p.name == "code") {
                            if let Some(i_code) = ic.params.iter_mut().find(|p| p.name == "code") {
                                i_code.set_text(t_code.text().to_string());
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
        // A template that cannot be read or parsed is DROPPED, and saying so
        // is the whole point of these two arms. It used to be an `if let Ok`
        // pair: the node simply left the palette, which looks nothing like a
        // parse error and nothing like an I/O error either. That silence cost
        // an afternoon on 2026-09-23, when a stray `close()` from the OpenCL
        // ICD (see `has_opencl_platform` in geometry.rs) was handing these
        // reads EBADF at random and the only symptom was a template count
        // that came up one short in a test far away.
        for path in paths {
            match fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<FsNode>(&content) {
                    // A type that names no kind is dropped like a parse
                    // error, and for the same reason: loaded, the row would
                    // read as text and look like it worked.
                    Ok(node) if !unknown_param_kinds(&node).is_empty() => eprintln!(
                        "cce-designer: dropping node template {} — unknown parameter type: {}",
                        path.display(),
                        unknown_param_kinds(&node)
                            .iter()
                            .map(|(at, name, ty)| format!("{at} '{name}' is \"{ty}\""))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    // So is a parameter whose name is not one: the name is
                    // what a path spells, and the pane shows the label.
                    Ok(node) if !misnamed_params(&node).is_empty() => eprintln!(
                        "cce-designer: dropping node template {} — a parameter name must be lowercase letters, digits and underscores: {}",
                        path.display(),
                        misnamed_params(&node)
                            .iter()
                            .map(|(at, name)| format!("{at} '{name}' (say '{}', with the text as its label)", param_name_of(name)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    Ok(mut node) => {
                        infer_template_exprs(&mut node);
                        raw_nodes.push(node);
                    }
                    Err(e) => eprintln!(
                        "cce-designer: dropping node template {} — it does not parse: {e}",
                        path.display()
                    ),
                },
                Err(e) => eprintln!(
                    "cce-designer: dropping node template {} — it could not be read: {e}",
                    path.display()
                ),
            }
        }

        // A child names its base template by type or by name, and takes
        // the base's params with its own overrides on top. RECURSIVELY: a
        // base that is itself a subnet (the Embryo's sphere1 is a Sphere)
        // brings raw children of its own, and those resolve the same way,
        // or the nested sphere's kernel node would arrive with only the
        // params its template override named. Depth-bounded, because a
        // template that contained itself would otherwise never finish.
        fn resolve_children(node: &mut FsNode, raw_nodes: &[FsNode], depth: usize, owner: &str) {
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
                    // The display flag too: a composed subnet's internals
                    // are switched off in the template (only its output
                    // draws), or viewing the subnet from outside draws the
                    // chain's intermediate stages on top of its result.
                    resolved_child.geometry_visible = child.geometry_visible;
                    resolved_child.bypassed = child.bypassed;
                    // Merge parameters
                    for override_p in &child.params {
                        if let Some(base_p) = resolved_child.params.iter_mut().find(|p| p.name == override_p.name) {
                            base_p.set_text(override_p.text().to_string());
                            // The flag travels with the value: an override
                            // that is a reference (the Embryo's sphere1
                            // reading `chf("../radius")`) stays one.
                            base_p.set_expr(override_p.is_expr());
                        }
                    }
                    // A child that lists children of its own brings THOSE
                    // rather than its base's: the Remesh subnet's repeat1
                    // is a Repeat holding the remesh's passes, not the
                    // empty input-to-output loop the Repeat template ships.
                    if !child.children.is_empty() {
                        resolved_child.children = child.children.clone();
                    }
                    if depth < 8 {
                        resolve_children(&mut resolved_child, raw_nodes, depth + 1, owner);
                    }
                    resolved_children.push(resolved_child);
                } else {
                    panic!(
                        "Node template of type '{}' not found for child '{}' in template '{}'",
                        child.node_type, child.name, owner
                    );
                }
            }
            node.children = resolved_children;
        }
        for mut node in raw_nodes.clone() {
            let owner = node.name.clone();
            resolve_children(&mut node, &raw_nodes, 0, &owner);
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
        bypassed: false,
        position: (0.0, 0.0),
        inputs: 0,
        outputs: 0,
    }
}
