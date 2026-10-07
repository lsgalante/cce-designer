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
/// The network editor's breadcrumb strip: a run of segment buttons, so the
/// toolkit's button height (the toolkit has no breadcrumb key of its own).
pub fn breadcrumb_h() -> f32 {
    cce_ui::layout::button_height()
}
pub const PLAYBAR_H: f32 = 36.0;

/// The attached playbar's whole height: the transport's [`PLAYBAR_H`] over
/// the window's bottom lip, which the playbar's shelf runs down into (see
/// `Playbar::frame`).
pub fn playbar_shelf_h() -> f32 {
    PLAYBAR_H + cce_ui::layout::bevel_width()
}

pub use crate::param::{invalid_params, is_param_name, misnamed_params, param_name_of, unknown_param_kinds, ParamDef, ParamKind, ParamSlot, ParamValue};

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
///
/// ` || ` joins alternatives and binds looser than ` && `, so
/// `operation == Clip || operation == Remap && from_range == Manual` is
/// Clip, or a Manual Remap.
pub fn param_visible(params: &[ParamDef], cond: &str) -> bool {
    let cond = cond.trim();
    if cond.is_empty() {
        return true;
    }
    cond.split("||").any(|alt| alt.split("&&").all(|clause| {
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
        let Some(sibling) = params.iter().find(|p| p.name == name) else {
            return false;
        };
        let matches = wanted
            .split('|')
            .any(|w| w.trim().eq_ignore_ascii_case(sibling.text().trim()));
        matches != negated
    }))
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

/// Which node the params pane is showing: the editor feeding it, that
/// editor's level, the slot there, and the node's id. All four, because no
/// one of them is an identity alone — an id is empty on nodes a bundled file
/// was saved without, a slot is only meaningful at a level, and a level is
/// only meaningful per editor.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ParamPaneTarget {
    editor: usize,
    path: Vec<usize>,
    slot: usize,
    id: String,
}

fn default_node_type() -> String { "node".to_string() }
fn default_node_geometry_visible() -> bool { true }
fn is_false(flag: &bool) -> bool { !*flag }
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
    /// The params HUD's width (since 2026-10-06, when it left the right
    /// dock). Absent in an older save, whose `params_width` was the params
    /// pane's and is read as the HUD's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hud_width: Option<f32>,
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

/// One entry in a node's right-click context menu, parallel to the visible
/// labels shown via `context_menu::show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeMenuAction {
    /// Dive into the node's subnet (the double-click behavior).
    Enter,
    /// Flip the node's geometry visibility.
    ToggleGeometry,
    ToggleBypass,
    /// Enter/exit the curve viewer state (curve nodes only).
    EditCurve,
    /// Open the dialog on the node's name.
    Rename,
    /// Remove the node.
    Delete,
}

/// The 3D viewport's right-click context menu actions.
/// One entry in a parameter row's right-click menu — Houdini's Copy
/// Parameter / Paste Relative References, and the switch between a value
/// and an expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMenuAction {
    CopyParameter,
    PasteRelative,
    PasteAbsolute,
    /// Keep the value, but as an expression: the row turns to text so
    /// arithmetic can be typed around it.
    EditExpression,
    /// Houdini's Delete Channels: the expression's CURRENT value, as a value.
    DeleteExpression,
    /// Show a float3 row's trackball beside its sliders, or the sliders
    /// alone — the parameter's `view`, kept with the instance.
    ShowTrackball,
    HideTrackball,
    Separator,
    /// A header row that reads something out — the control's kind, the
    /// value's type — and runs nothing.
    Info,
}

/// The viewport menubar's Guides menu and its items, by position — the one
/// place the order `with_item("Guides", …)` builds is spelled out, for the
/// checkmark writes and the click dispatch in `window.rs`. The Cube item
/// sat second until the guide was removed (2026-09-25).
pub const GUIDES_MENU: usize = 2;
pub const GUIDE_GRID: usize = 0;
pub const GUIDE_ORIGIN: usize = 1;
pub const GUIDE_CAMERA_PIVOT: usize = 2;

/// A page of the viewport menu: a row of the menu that the menu turns into
/// (see `crate::menu_page`), with a way back to the menu at its top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportMenuPage {
    /// How the geometry itself is drawn: the wireframe and the surface.
    Style,
    /// What is drawn ON it: the points, and the markers, numbers and
    /// normals of its points, primitives and vertices.
    Markers,
}

impl ViewportMenuPage {
    pub fn label(self) -> &'static str {
        match self {
            ViewportMenuPage::Style => "Style",
            ViewportMenuPage::Markers => "Markers",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewportMenuAction {
    /// A row that turns the menu into one of its pages.
    Page(ViewportMenuPage),
    /// The Network row, while the network has no plate: a page turning the
    /// menu into the network's own menu, which a right press on empty graph
    /// space can no longer reach — that space is the scene's.
    NetworkPage,
    /// Move the active camera so the visible node geometry fills the view.
    FrameAll,
    /// Put the pivot plane at true size: one world unit (the Guides "World
    /// Unit") spans its real length on this display.
    OneToOne,
    /// Follow whichever editor took the last node click (the default).
    PinFollow,
    /// Lock the viewport to one editor's level (CONTENT_IDX / CONTENT2_IDX).
    PinTo(usize),
    /// Run a registry command — the display toggles, so the menu's rows are
    /// the palette's and a row is exactly as scriptable as its command.
    Command(&'static str),
    /// Set the shading mode: smooth (true) or flat. A radio pair over the
    /// one `toggle_smooth_shading` flag, running the toggle only when the
    /// pick differs, so picking the mode already on is a no-op rather than
    /// a flip.
    Shading(bool),
    /// The polygon (geometry fill) opacity, as a SLIDER row (cce-ui's
    /// `context_menu::MenuSlider`): the wheel steps it by 5%, a press on its
    /// band drags it, and the menu stays open. Picking the row runs nothing
    /// — the slider's changes arrive through `drain_viewport_menu_slider`.
    OpacitySlider,
    /// The wire pass's thickness in px, a slider row under Show Wireframe:
    /// 1–8 like the palette's Wire Thickness row, half a pixel a notch.
    WireThicknessSlider,
    /// The wire pass's own opacity, in percent like the polygon Opacity row
    /// and stepped the same 5%: the two are independent, so a translucent
    /// fill can carry a solid lattice and the other way round.
    WireOpacitySlider,
    /// Point Marker Size in world units, the Show Point Markers overlay's
    /// radius: the palette row's 0.005–0.1.
    PointMarkerSizeSlider,
    /// Group Marker Size in world units, the Selected-Group markers'
    /// radius: the palette row's 0–0.2.
    GroupMarkerSizeSlider,
    /// Pull Arrow Scale, the pull arrows' length as a multiple of the true
    /// displacement: the palette row's 0.25–10.
    PullArrowScaleSlider,
    /// Camera Pivot Size, the pivot marker's scale, under its switch: 0–1
    /// by a twentieth. The palette row is the coarse one, in tenths up
    /// to 5.
    CameraPivotSizeSlider,
    /// A "-" row: engraved, inert.
    Separator,
}


/// The playbar's right-click menu (2026-09-30): the transport's commands,
/// its Repeat switch, and the settings that are the timeline's — the
/// playback rate and the frame range — as slider rows, the viewport menu's
/// shape. Rows dispatch as the network menu's do, by command id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybarMenuAction {
    /// Run `command::by_id(id)`; a toggle command's row carries its mark.
    Command(&'static str),
    /// Playback Rate, in frames a second: 1–120 by one.
    FpsSlider,
    /// The frame range's near end, 1–999 by one; kept below the far end.
    StartFrameSlider,
    /// The frame range's far end, 2–1000 by one; kept above the near end.
    EndFrameSlider,
    /// The Plate row: a page turning the menu into the playbar's plate rows
    /// (Collapse, Detach).
    PlatePage,
    /// A "-" row: engraved, inert.
    Separator,
}

/// The network editor's right-click context menu (on empty space — a press on
/// a node still opens that node's menu). Every row but the separator names a
/// COMMAND ID rather than a piece of work, so the labels cannot drift from the
/// palette's and a row is exactly as scriptable as the command behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMenuAction {
    /// Run `command::by_id(id)` — the row's label came from the same row.
    Command(&'static str),
    /// The Plate row: a page turning the menu into the network pane's
    /// plate rows — collapse, detach, its dock's tabs, Move To.
    PlatePage,
    /// A "-" row: engraved, inert.
    Separator,
}

/// The target a scroll gesture over the plateless network was given at its
/// first event (`State::overlay_wheel_to_graph`).
#[derive(Debug, Clone, Copy)]
pub struct OverlayWheel {
    /// The graph pans; otherwise the scene's camera orbits.
    pub graph: bool,
    pub at: (f32, f32),
    pub last: Instant,
}

/// How long a pause ends a scroll gesture for `OverlayWheel`: longer than
/// the gap between a trackpad's events, shorter than a deliberate second
/// swipe.
pub const OVERLAY_WHEEL_GAP: std::time::Duration = std::time::Duration::from_millis(250);

/// A mouse drag that moves the whole selection, not just the node under the
/// pointer.
///
/// The widget drags ONE node — it has one `dragging_idx` — so the companions
/// are moved here, rigidly, by the offset the dragged node has travelled.
/// Their ORIGINAL cells are kept rather than stepped each frame: a drag is a
/// continuous gesture reported in whole cells, so accumulating the steps would
/// drift the group apart the first time two motion events resolved to the same
/// cell.
#[derive(Debug, Clone)]
pub struct NodeDragGroup {
    /// The slot the pointer grabbed — the widget's own dragged node.
    pub dragged: usize,
    /// The cell it started on; every offset is measured from here.
    pub from: (i32, i32),
    /// The rest of the selection, with the cells they started on.
    pub others: Vec<(usize, (f32, f32))>,
}

/// The command ids the network context menu offers, in order; `None` is a
/// separator. Add Node leads because right-clicking empty space USED to open
/// the add-node palette outright, and that is still the common reason to come
/// here.
/// How far under its last row a plateless params pane still claims the
/// pointer: a press just below a row is the row's, not the scene's.
pub const PARAMS_CLAIM_PAD: f32 = 8.0;

/// The narrowest the params HUD may be dragged.
pub const PARAMS_HUD_MIN_W: f32 = 150.0;

/// The params plate's size collapsed: a circle this wide at the HUD's top
/// right corner, while there are no rows to show.
pub const PARAMS_DOT_D: f32 = 36.0;

pub const NETWORK_MENU_COMMANDS: &[Option<&'static str>] = &[
    Some("add_node"),
    None,
    Some("layout_nodes"),
    Some("frame_all"),
    Some("frame_cursor"),
    None,
    Some("reset_zoom"),
    Some("toggle_circular_pane"),
];

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
    ToggleBypass { slot: usize },
    AddNode { template_name: String, name: Option<String>, x: f32, y: f32 },
    DeleteNode { slot: usize },
    RenameNode { slot: usize, new_name: String },
    MoveNode { slot: usize, x: f32, y: f32 },
    AddParam {
        slot: usize,
        name: String,
        param_type: String,
        default: String,
        /// What the params pane shows for it; the name when absent.
        #[serde(default)]
        label: String,
    },
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

/// The row menu's `Control:` and `Type:` readouts for a row the pane shows
/// as `shown` (a display type string) over a parameter of `kind`: the
/// control drawn, and the type of value it sets. The control follows what
/// is DRAWN — a float3 row is sliders, an expression a text box — and the
/// type what is SET: a presented float3 sets a float3 whatever the
/// parameter's kind, and otherwise the kind says (a `float` parameter in a
/// text box still sets a float).
pub fn control_and_type(shown: &str, kind: ParamKind) -> (&'static str, &'static str) {
    let head = shown.split(':').next().unwrap_or("");
    let control = match head {
        "float3" if shown.split(':').nth(3) == Some("trackball") => "trackball and sliders",
        "slider" | "float2" | "float3" | "float4" => "slider",
        "spinbox" => "spinbox",
        "choice" => "dropdown",
        "toggle" | "checkbox" => "toggle",
        "button" => "button",
        "code" => "code editor",
        "textpick" => "text box with picker",
        "ramp" => "ramp",
        "color" | "rgb" | "rgba" => "color picker",
        _ => "text box",
    };
    let ty = match head {
        "float2" => "float2",
        "float3" => "float3",
        "float4" => "float4",
        // A text parameter presented as a slider (the Attribute node's
        // Value, one wide) sets a number.
        "slider" => "float",
        _ => match kind {
            ParamKind::Slider | ParamKind::Float => "float",
            ParamKind::Spin => "integer",
            ParamKind::Float2 => "float2",
            ParamKind::Float3 => "float3",
            ParamKind::Toggle => "boolean",
            ParamKind::Choice => "enum",
            ParamKind::Text | ParamKind::Code => "string",
            ParamKind::Node => "node",
            ParamKind::Attribute => "attribute",
            ParamKind::Group => "group",
            ParamKind::Button => "none",
        },
    };
    (control, ty)
}

/// How wide a line of a parameter's description is in its row menu, in
/// characters. The menu is as wide as its widest row, so an unwrapped
/// sentence would stretch it across the window.
pub const PARAM_DESCRIPTION_WIDTH: usize = 44;

/// `text` broken into lines of at most `width` characters, at spaces; a
/// word longer than a line has one to itself.
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The range a presented row carries in its display type (`slider:lo:hi`,
/// `float3:lo:hi`), for a row whose parameter declares none of its own.
fn shown_row_range(shown: &str) -> Option<(f32, f32, Option<f32>)> {
    let mut parts = shown.split(':');
    let head = parts.next()?;
    if !matches!(head, "slider" | "float2" | "float3" | "float4") {
        return None;
    }
    let lo = parts.next()?.parse::<f32>().ok()?;
    let hi = parts.next()?.parse::<f32>().ok()?;
    Some((lo, hi, None))
}

/// The half-span of the Attribute node's Value row: the row runs from
/// minus this to plus it. It ADAPTS to the value (since 2026-10-01; until
/// then a fixed ±1000, which a drag crossed in hundreds): the smallest power
/// of ten, and at least one, whose middle half holds every component — so
/// 1.00 drags over ±10 and 9.6 over ±100. A span already in use (`kept`)
/// stands while the value stays between a twentieth of it and nineteen
/// twentieths, so a value moving inside the row does not re-scale it under
/// the pointer; one that reaches an end, or falls far inside, is given a
/// new one. The row's range is SOFT: a value typed past an end widens it
/// rather than being clamped, and the next span holds it.
pub fn value_row_span(values: &[f32], kept: Option<f32>) -> f32 {
    let m = values.iter().filter(|v| v.is_finite()).fold(0.0f32, |a, v| a.max(v.abs()));
    if let Some(r) = kept {
        if m <= 0.95 * r && (r <= 1.0 || m >= 0.05 * r) {
            return r;
        }
    }
    let mut r = 1.0f32;
    while m > 0.5 * r && r < 1e9 {
        r *= 10.0;
    }
    r
}

/// What an Attribute node's `Value` is aimed at, as `add_pick_lists` needs
/// it: a width the node itself decides, or the name of the input attribute
/// whose width has to be read off the evaluated input.
enum ValueTarget {
    Width(usize),
    Named(String),
}

/// What the params pane reads off a node's evaluated INPUT to build its
/// controls: group names, point attribute names (plus the Pos/Col
/// built-ins), and each attribute's width in components.
#[derive(Clone, Default)]
pub struct PickLists {
    pub groups: Vec<String>,
    pub attrs: Vec<String>,
    pub widths: Vec<(String, usize)>,
}

/// A float3 row's display type: `float3:lo:hi`, with `:trackball` for the
/// ball beside the sliders (cce-ui's `Float3::set_trackball`).
pub fn float3_row(min: f32, max: f32, trackball: bool) -> String {
    format!("float3:{}:{}{}", min, max, if trackball { ":trackball" } else { "" })
}

/// A `float2` parameter's display type over ± `span`: two sliders, with a
/// soft range, so a value typed past an end widens it rather than being
/// clamped.
pub fn float2_row(span: f32) -> String {
    format!("float2:{}:{}:soft", -span, span)
}

/// The control the Attribute node's Value row is presented as, for a target
/// `width` components wide, the row's text as that control shows it, and
/// the half-span it runs over: a slider for one, the float group with two,
/// three or four rows for more (cce-ui's `float2` / `float3` / `float4`),
/// over ± [`value_row_span`] — `kept` is the span the row has now, kept
/// outright while `hold` (a drag in the pane) — with a soft range. The text must hold one number or `width` of them; one is
/// SPREAD to every component, which is what a single number means to the
/// node (`fit` in `apply_attribute`), so the controls show it as it acts.
/// `None` — a text box, as before — for a width it does not fit.
pub fn value_row_control(text: &str, width: usize, ball: bool, kept: Option<f32>, hold: bool) -> Option<(String, String, f32)> {
    let parts: Vec<&str> = text.split(|c| c == ':' || c == ',' || c == ' ').filter(|s| !s.is_empty()).collect();
    if !(1..=4).contains(&width) || parts.iter().any(|p| p.parse::<f32>().is_err()) {
        return None;
    }
    let shown = match parts.len() {
        n if n == width => parts.join(":"),
        1 => vec![parts[0]; width].join(":"),
        _ => return None,
    };
    let values: Vec<f32> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    // Held (a drag in the pane), the span in use stands whatever the value.
    let r = match (hold, kept) {
        (true, Some(r)) => r,
        _ => value_row_span(&values, kept),
    };
    let ty = match width {
        1 => format!("slider:{}:{}:2:soft", -r, r),
        3 => format!("{}:soft", float3_row(-r, r, ball)),
        n => format!("float{n}:{}:{}:soft", -r, r),
    };
    Some((ty, shown, r))
}

/// Whether `shown` is `kept` as a value row presents it — the same numbers,
/// or one number of `kept` spread over every component of `shown` — so the
/// write-back leaves a row the user did not touch alone rather than
/// rewriting `1.00` as `1.00:1.00:1.00` the first time the pane syncs.
pub fn same_value_row_text(kept: &str, shown: &str) -> bool {
    let nums = |t: &str| -> Option<Vec<f32>> {
        t.split(|c| c == ':' || c == ',' || c == ' ').filter(|s| !s.is_empty()).map(|s| s.parse::<f32>().ok()).collect()
    };
    let (Some(a), Some(b)) = (nums(kept), nums(shown)) else { return false };
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.len() == b.len() {
        return a == b;
    }
    a.len() == 1 && b.iter().all(|v| *v == a[0])
}

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
fn param_row(p: &ParamDef) -> (String, String, String) {
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
/// One attribute value as spreadsheet cells: a cell a component, `-` for
/// each when there is none.
fn push_cells(row: &mut Vec<String>, value: Option<crate::detail::AttribValue>, components: usize) {
    match value {
        Some(crate::detail::AttribValue::Float(f)) => row.push(fmt4(f)),
        Some(crate::detail::AttribValue::Int(i)) => row.push(i.to_string()),
        Some(crate::detail::AttribValue::Float2(a)) => row.extend(a.iter().map(|v| fmt4(*v))),
        Some(crate::detail::AttribValue::Float3(a)) => row.extend(a.iter().map(|v| fmt4(*v))),
        Some(crate::detail::AttribValue::Float4(a)) => row.extend(a.iter().map(|v| fmt4(*v))),
        None => row.extend(std::iter::repeat("-".to_string()).take(components)),
    }
}

/// `format!("{:.4}", x)`, character for character, several times faster:
/// an f32 times ten thousand is exact in an f64 (24 bits of mantissa and
/// 14), so rounding it half to even is rounding the exact decimal value,
/// which is what the formatter does. What does not fit an integer goes to
/// the formatter. `fmt4_is_format_4` holds the two equal.
pub(crate) fn fmt4(x: f32) -> String {
    if !x.is_finite() || x.abs() >= 1.0e14 {
        return format!("{:.4}", x);
    }
    use std::fmt::Write;
    let y = (x.abs() as f64 * 10000.0).round_ties_even() as u64;
    let mut s = String::with_capacity(12);
    if x.is_sign_negative() {
        s.push('-');
    }
    let _ = write!(s, "{}.{:04}", y / 10000, y % 10000);
    s
}

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
fn param_path_renamed(path: &str) -> Option<String> {
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
fn rename_at_attribute(code: &str, old: &str, new: &str) -> String {
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



fn default_grid_thickness() -> f32 { 0.03 }
fn default_grid_color() -> [f32; 3] { [0.35, 0.35, 0.40] }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ViewportSettings {
    pub bg_color: [f32; 3],
    pub square: bool,
    pub show_camera_pivot_enabled: bool,
    pub camera_pivot_size: f32,
    pub show_grid_enabled: bool,
    // `show_cube_enabled` was the reference cube guide, removed 2026-09-25.
    // Older state.kdl files and project display blocks still carry the key;
    // serde ignores it, so they load unchanged.
    pub show_origin_enabled: bool,
    pub origin_size: f32,
    #[serde(default = "default_grid_thickness")]
    pub grid_thickness: f32,
    #[serde(default = "default_grid_color")]
    pub grid_color: [f32; 3],
    /// Whether the params HUD draws its plate — one FITTED to its rows,
    /// not to the HUD's rect. On by default; off, the rows stand on the
    /// scene. Either way the HUD claims only the band its rows cover.
    #[serde(default = "default_params_plate")]
    pub params_plate: bool,
    /// How the network's node wires run (`cce_ui::widget::display::WireStyle`
    /// by name). Empty follows `style.surface.graph.node.wire_style` in
    /// config.kdl, which is every file from before the row.
    #[serde(default)]
    pub node_wire_style: String,
    /// The three point overlays. Display settings like the guide toggles
    /// above, and persisted in the same place: they were per-node `meta`
    /// child preferences until 2026-09-23, which made a view choice into a
    /// property of the scene. Absent from older files — off, as the meta
    /// defaults were.
    #[serde(default)]
    pub show_point_markers: bool,
    #[serde(default)]
    pub show_point_numbers: bool,
    #[serde(default)]
    pub show_point_normals: bool,
    /// The same for the other two element classes. Absent from older
    /// files — off.
    #[serde(default)]
    pub show_prim_numbers: bool,
    #[serde(default)]
    pub show_prim_normals: bool,
    #[serde(default)]
    pub show_vertex_numbers: bool,
    #[serde(default)]
    pub show_vertex_markers: bool,
    #[serde(default)]
    pub show_vertex_normals: bool,
    /// The point groups whose members wear a marker in the scene — the
    /// Group Markers dialog's switches, by group name, joined by commas.
    /// One string rather than a list: the KDL writer puts a list of one
    /// back as a bare string, which a `Vec` refuses, and a settings file
    /// that fails to parse is read as the DEFAULTS. Absent from older
    /// files — none. `State::marked_groups_of` / `join_marked_groups` are
    /// the two ends.
    #[serde(default)]
    pub marked_groups: String,
    /// The attribute visualizers, as one JSON string — see
    /// `crate::visualizer::encode` for why one string. Absent from older
    /// files — none.
    #[serde(default)]
    pub visualizers: String,
    /// World-unit radius and colour of the Show Point Markers overlay.
    #[serde(default = "default_point_marker_size")]
    pub point_marker_size: f32,
    #[serde(default = "default_point_marker_color")]
    pub point_marker_color: [f32; 3],
    /// What one world unit IS (mm / cm / m / in). A DECLARATION — geometry
    /// never converts; it feeds `View 1:1`.
    #[serde(default = "default_world_unit")]
    pub world_unit: String,
    /// The path-traced preview.
    #[serde(default)]
    pub rt_mode: bool,
    /// The network pane's circular shape.
    #[serde(default)]
    pub circular_pane: bool,
}

fn default_point_marker_size() -> f32 {
    0.02
}

fn default_point_marker_color() -> [f32; 3] {
    [0.85, 0.85, 1.0]
}

fn default_world_unit() -> String {
    "mm".to_string()
}

/// How the geometry itself is drawn — the wire pass and the point display.
///
/// These lived on the root meta node's `render` utility subnet until
/// 2026-09-23, which made them per-PROJECT: opening someone else's scene
/// reset how you looked at geometry. They are display preferences like the
/// guides, so they persist here, and the dialog's Settings half is where
/// they are edited.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(from = "StoredRenderSettings")]
pub struct RenderSettings {
    pub wireframe: bool,
    pub wire_single_color: bool,
    /// RGB; used in single-colour mode only.
    pub wire_color: [f32; 3],
    /// The wire pass's own opacity, in both colour modes — `geo_opacity` is
    /// the polygons'. Until 2026-09-25 it was the wire colour's ALPHA, which
    /// made it a setting reachable only through a colour picker's alpha
    /// channel; [`StoredRenderSettings`] moves an old alpha here.
    pub wire_opacity: f32,
    pub wire_width: f32,
    pub geo_opacity: f32,
    /// The Selected-Group markers' radius, in world units. Until
    /// 2026-09-29 it was Point Size times Group Marker Scale, Point Size
    /// being the radius of the Show Points display, which went that day as
    /// a double of Show Point Markers; [`StoredRenderSettings`] multiplies
    /// an old pair out.
    pub group_marker_size: f32,
    /// The pull arrows' length as a multiple of the displacement they show.
    /// 1 draws the true vector; a longer arrow is legible when the pull is
    /// small beside the model. Display only — the pull itself is untouched.
    pub pull_arrow_scale: f32,
    /// Smooth (vertex-normal) shading of the scene fill, where off is the
    /// faceted look the raster pass has always had. Absent in older files:
    /// flat.
    pub smooth_shading: bool,
    /// See-through fill: below full opacity the fill draws with no culling
    /// and no depth writes, triangles sorted back to front for the eye, so
    /// what it occludes — its own far side, the wires, the scene behind —
    /// shows through. Absent in older files: off.
    pub show_occluded: bool,
}

/// [`RenderSettings`] as READ, from state.kdl and from a project's display
/// block alike: every field optional in the file, and the wire colour taken
/// with three components or four. A fourth is the wire opacity as it was
/// stored before `wire_opacity` existed, and becomes it when the file names
/// no `wire_opacity` of its own — dropping it would make every translucent
/// wireframe opaque on the first load. The group markers' size is read the
/// same way: its own key, else the `point_size` x `group_marker_scale` it
/// was until 2026-09-29. `render_points` and `point_color`, the rest of the
/// retired Show Points display, are in older files and not read.
#[derive(Deserialize)]
struct StoredRenderSettings {
    #[serde(default)]
    wireframe: bool,
    #[serde(default)]
    wire_single_color: bool,
    #[serde(default)]
    wire_color: Option<Vec<f32>>,
    #[serde(default)]
    wire_opacity: Option<f32>,
    #[serde(default = "default_wire_width")]
    wire_width: f32,
    #[serde(default = "default_geo_opacity")]
    geo_opacity: f32,
    #[serde(default)]
    group_marker_size: Option<f32>,
    #[serde(default)]
    point_size: Option<f32>,
    #[serde(default)]
    group_marker_scale: Option<f32>,
    #[serde(default = "default_pull_arrow_scale")]
    pull_arrow_scale: f32,
    #[serde(default)]
    smooth_shading: bool,
    #[serde(default)]
    show_occluded: bool,
}

impl From<StoredRenderSettings> for RenderSettings {
    fn from(s: StoredRenderSettings) -> Self {
        let c = s.wire_color.unwrap_or_default();
        let (wire_color, old_alpha) = match c[..] {
            [r, g, b] => ([r, g, b], None),
            [r, g, b, a, ..] => ([r, g, b], Some(a)),
            _ => (default_wire_color(), None),
        };
        Self {
            wireframe: s.wireframe,
            wire_single_color: s.wire_single_color,
            wire_color,
            wire_opacity: s.wire_opacity.or(old_alpha).unwrap_or(1.0).clamp(0.0, 1.0),
            wire_width: s.wire_width,
            geo_opacity: s.geo_opacity,
            group_marker_size: s
                .group_marker_size
                .unwrap_or_else(|| s.point_size.unwrap_or(0.02) * s.group_marker_scale.unwrap_or(1.25))
                .clamp(0.0, GROUP_MARKER_SIZE_MAX),
            pull_arrow_scale: s.pull_arrow_scale,
            smooth_shading: s.smooth_shading,
            show_occluded: s.show_occluded,
        }
    }
}

fn default_group_marker_size() -> f32 {
    0.025
}

/// The far end of Group Marker Size's range, in world units.
pub(crate) const GROUP_MARKER_SIZE_MAX: f32 = 0.2;

fn default_pull_arrow_scale() -> f32 {
    1.0
}

fn default_wire_color() -> [f32; 3] {
    [0.0, 0.0, 0.0]
}

fn default_wire_width() -> f32 {
    1.0
}

fn default_geo_opacity() -> f32 {
    1.0
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            wireframe: false,
            wire_single_color: false,
            wire_color: default_wire_color(),
            wire_opacity: 1.0,
            wire_width: default_wire_width(),
            geo_opacity: default_geo_opacity(),
            group_marker_size: default_group_marker_size(),
            pull_arrow_scale: default_pull_arrow_scale(),
            smooth_shading: false,
            show_occluded: false,
        }
    }
}

fn default_params_plate() -> bool {
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
            show_origin_enabled: true,
            params_plate: true,
            node_wire_style: String::new(),
            show_point_markers: false,
            show_point_numbers: false,
            show_point_normals: false,
            show_prim_numbers: false,
            show_prim_normals: false,
            show_vertex_numbers: false,
            show_vertex_markers: false,
            show_vertex_normals: false,
            marked_groups: String::new(),
            visualizers: String::new(),
            point_marker_size: default_point_marker_size(),
            point_marker_color: default_point_marker_color(),
            world_unit: default_world_unit(),
            rt_mode: false,
            circular_pane: false,
            origin_size: 1.0,
            grid_thickness: default_grid_thickness(),
            grid_color: default_grid_color(),
        }
    }
}

/// The network grid's geometry as configured, at 100% zoom: the pitch
/// (`style.surface.graph.spacing_x` / `spacing_y` — the distance from the
/// centre of one grid line to the centre of the next, the grid's ONE size;
/// nodes are centred on the lattice intersections) and the node body's own
/// size (`style.surface.graph.node.width` / `height`), which the pitch does
/// not touch: a denser grid moves nodes closer, it does not shrink them.
/// Until 2026-09-22 the grid was a cell size plus a gap, with a node
/// filling its cell.
///
/// Config-owned, NOT state: it is user-authored, so the app reads it and
/// never writes it back — the same split `../CLAUDE.md` describes for scroll
/// behavior. Zoom scales all four in memory together; the configured values
/// are the 100% baseline that Reset Zoom returns to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridGeometry {
    pub pitch_x: f32,
    pub pitch_y: f32,
    pub node_w: f32,
    pub node_h: f32,
}

#[cfg(not(test))]
pub fn configured_grid_geometry() -> GridGeometry {
    GridGeometry {
        pitch_x: cce_ui::layout::graph_spacing_x(),
        pitch_y: cce_ui::layout::graph_spacing_y(),
        node_w: cce_ui::layout::graph_node_width(),
        node_h: cce_ui::layout::graph_node_height(),
    }
}

/// The lattice the SUITE runs on, fixed rather than read from
/// `~/.config/cce/config.kdl`.
///
/// The grid tests press at pixel coordinates derived from `cell_center` and
/// assert which node the press landed on, so the pitch decides whether they
/// pass — and it was coming from whichever config.kdl happened to be on the
/// machine. `dragging_a_selected_node_carries_the_selection` is the one that
/// showed it: at cce-ui's own defaults (187.5 x 112.5) its row 11 lands at
/// 1237 px in a 900 px test window, so the press misses the node and the
/// drag never arms. It passed only because the author's config set 140 x 70.
/// A fresh clone, a second machine or CI would all have failed it, and the
/// failure would have read as a broken drag rather than a borrowed lattice.
///
/// These ARE those numbers — the lattice the grid tests were written against,
/// kept so that pinning them changes no test's meaning. They are deliberately
/// not cce-ui's defaults: matching those would mean rewriting the cell
/// arithmetic of a subtle drag test to fit a coarser grid, which is a real
/// change to what it checks, made for the sake of a number that is arbitrary
/// either way. What matters is that the number is the suite's own.
///
/// `graph_grid_snap` needs no pin: this app does not read it, a dragged node
/// always snapping to a cell it can land on (`State::grid_snap_enabled`).
#[cfg(test)]
pub fn configured_grid_geometry() -> GridGeometry {
    GridGeometry { pitch_x: 140.0, pitch_y: 70.0, node_w: 80.0, node_h: 40.0 }
}

/// Zoom limits on the pitch — the old 30..500 x 15..250 limits on the node
/// body, expressed on the pitch that body used to be a share of.
pub const MIN_PITCH_X: f32 = 37.5;
pub const MAX_PITCH_X: f32 = 625.0;
pub const MIN_PITCH_Y: f32 = 22.5;
pub const MAX_PITCH_Y: f32 = 375.0;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DesignSettings {
    #[serde(default)]
    pub viewport: ViewportSettings,
    #[serde(default)]
    pub render: RenderSettings,
    /// Project to open at startup instead of the bundled default — the Main
    /// node's "Set As Default" button. A path string (what
    /// `loaded_project_path` held when it was set); absent = the bundled
    /// `default_project.json`. Deliberately NOT the project file itself:
    /// that file is versioned AND is the detached-window sync channel, so
    /// "make this the default" must not rewrite it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_project: Option<String>,
    /// Which GPU the window's renderer asks Vulkan for: one of
    /// [`GPU_CHOICES`]. Read ONCE, at launch, by [`apply_gpu_preference`],
    /// because the device is chosen when the renderer is created and a
    /// renderer is not rebuilt on a whim — so a change takes effect on the
    /// next start. Here rather than in the render block because that block
    /// rides the project file, and which GPU a machine has is not a property
    /// of a scene.
    #[serde(default = "default_gpu")]
    pub gpu: String,
    /// Whether playback wraps at the end of the frame range (the default)
    /// or stops on the last frame — `toggle_playbar_repeat`. Top-level like
    /// `gpu`: how the transport behaves is how you like to work, not a
    /// property of a scene, so it does not ride the project file.
    #[serde(default = "default_true")]
    pub playbar_repeat: bool,
    /// The playback rate in frames a second — the playbar menu's Playback
    /// Rate. Top-level like `playbar_repeat`, and for the same reason.
    #[serde(default = "default_playbar_fps")]
    pub playbar_fps: f32,
    /// Whether the playbar shows its Previous / Next Frame buttons —
    /// `toggle_playbar_step_buttons`. Top-level like `playbar_repeat`.
    #[serde(default = "default_true")]
    pub playbar_step_buttons: bool,
}

fn default_playbar_fps() -> f32 {
    24.0
}

impl Default for DesignSettings {
    fn default() -> Self {
        Self {
            viewport: ViewportSettings::default(),
            render: RenderSettings::default(),
            default_project: None,
            gpu: default_gpu(),
            playbar_repeat: true,
            playbar_fps: default_playbar_fps(),
            playbar_step_buttons: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// The GPU setting's options. "integrated" is cce-ui's own default
/// (`CCE_VK_DEVICE` unset); "discrete" asks for the dedicated card.
pub const GPU_CHOICES: &[&str] = &["integrated", "discrete"];

fn default_gpu() -> String {
    GPU_CHOICES[0].to_string()
}

/// What `CCE_VK_DEVICE` should be set to for a saved GPU preference, given
/// what the environment already says — `None` to leave it alone.
///
/// An explicit `CCE_VK_DEVICE` at launch wins: it is a per-run override,
/// and a setting that silently undid it would make it useless for trying
/// the other card once. "integrated" sets NOTHING rather than
/// "integrated": cce-ui treats any explicit request as licence to lift a
/// session-wide ICD pin (`VK_DRIVER_FILES`), which loads every vendor's
/// driver and can wake a sleeping discrete GPU just to enumerate it — the
/// opposite of what asking for the integrated one means.
pub(crate) fn gpu_env_for(pref: &str, existing: Option<&str>) -> Option<&'static str> {
    if existing.is_some_and(|v| !v.is_empty()) {
        return None;
    }
    (pref == "discrete").then_some("discrete")
}

/// Put the saved GPU preference into the environment, where cce-ui's
/// renderer — and the compute device `gpu.rs` opens — read it. Called from
/// `main` before the engine starts, while the process has one thread; the
/// detached windows inherit it, since the main window spawns them.
pub(crate) fn apply_gpu_preference() {
    let pref = DesignSettings::load().gpu;
    let existing = std::env::var("CCE_VK_DEVICE").ok();
    if let Some(v) = gpu_env_for(&pref, existing.as_deref()) {
        std::env::set_var("CCE_VK_DEVICE", v);
    }
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

fn hex_to_float_array4(hex: &str) -> Option<[f32; 4]> {
    let h = hex.trim_start_matches('#');
    if h.len() == 8 {
        let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|b| b as f32 / 255.0);
        Some([v(0)?, v(2)?, v(4)?, v(6)?])
    } else {
        let rgb = cce_ui::color::parse_hex_rgb(hex)?;
        Some([rgb[0], rgb[1], rgb[2], 1.0])
    }
}

/// Where `cfg(test)` builds keep the files the installed app keeps under
/// `<config home>/cce/cce-designer/` — a directory of this TEST's own,
/// created on first use.
///
/// Per process, so two suites running at once (another session's, a second
/// terminal's) cannot read each other's writes. And per TEST within a
/// process, because settings are shared mutable state and libtest runs tests
/// in parallel threads: `the_suite_does_not_write_the_users_own_settings`
/// ran the network plate toggle (retired since), which SAVED `network_plate = false`, and with one
/// file between them every `State::new` racing it loaded that and came up
/// with the plate switched off — `test_the_network_plate_is_an_option` (retired with it)
/// failing perhaps one run in six, in an assertion about a row in the View
/// node. Nothing in the suite had ever toggled a setting before
/// 2026-09-23, so this was not a pre-existing race so much as one that
/// arrived with the test that could trigger it.
///
/// libtest names each thread after the test running on it, which is what
/// makes the split possible without every test having to opt in. A thread
/// with no name — a helper the test spawned — shares the process-wide
/// directory, which is the old behaviour and is right: it belongs to
/// whichever test spawned it.
#[cfg(test)]
pub(crate) fn test_config_dir() -> std::path::PathBuf {
    static ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let root = ROOT
        .get_or_init(|| std::env::temp_dir().join(format!("cce-designer-test-{}", std::process::id())))
        .clone();
    let dir = match std::thread::current().name() {
        Some(t) => root.join(t.replace(|c: char| !c.is_ascii_alphanumeric(), "_")),
        None => root,
    };
    let _ = fs::create_dir_all(&dir);
    dir
}

impl DesignSettings {
    /// `<config home>/cce/cce-designer/state.kdl`, resolved through the
    /// toolkit's own base directory so `$XDG_CONFIG_HOME` is honored. Every
    /// other app in the workspace already goes through `cce_config_dir`;
    /// this one hardcoded `$HOME/.config` and was the only holdout.
    #[cfg(not(test))]
    pub(crate) fn file_path() -> std::path::PathBuf {
        cce_ui::config::cce_config_dir().join("cce-designer").join("state.kdl")
    }

    /// The same file under test, in a temp directory — and that redirect is
    /// not a convenience.
    ///
    /// `State::new` loads the BUNDLED project, whose meta subnets overwrote
    /// the live viewport flags on every parameter change (the meta node is
    /// retired, but the hazard was real and the redirect is what caught it).
    /// Any test that then reached `save_settings` — a plate toggle,
    /// the dialog's toggle rows — wrote the bundled project's
    /// show_grid / show_cube / show_origin over the user's own state.kdl.
    /// (The cube guide has since been removed.)
    /// `cargo test` reset three of the user's toggles on every run, and
    /// nothing about the run looked wrong afterwards.
    ///
    /// The redirect lives in the path itself rather than in an environment
    /// variable the test module sets, because that would leave the guarantee
    /// resting on every future test remembering to set the variable before
    /// touching `State` — and the one test that forgets destroys real
    /// settings silently. There is nothing here to remember.
    #[cfg(test)]
    pub(crate) fn file_path() -> std::path::PathBuf {
        test_config_dir().join("state.kdl")
    }

    fn load_kdl(path: &std::path::Path) -> Option<Self> {
        let content = fs::read_to_string(path).ok()?;
        Some(Self::from_kdl_str(&content))
    }

    /// Every colour field, as `(block, field, components read)`. KDL carries
    /// them as `#rrggbb` hex strings, so both directions walk this one table.
    /// It was a hand-written pair of `if let`s per colour, which is why only
    /// two of the five were ever converted once the render block arrived.
    ///
    /// Every colour is WRITTEN as three components. The wire colour is READ
    /// as four, because until 2026-09-25 it was `#rrggbbaa` with the alpha
    /// as the wire opacity; `StoredRenderSettings` moves that alpha into
    /// `wire_opacity`, which a six-digit hex (alpha 1) never overrides since
    /// the file then names `wire_opacity` itself.
    const COLOR_FIELDS: &'static [(&'static str, &'static str, usize)] = &[
        ("viewport", "bg_color", 3),
        ("viewport", "grid_color", 3),
        ("viewport", "point_marker_color", 3),
        ("render", "wire_color", 4),
    ];

    pub(crate) fn from_kdl_str(content: &str) -> Self {
        let mut json_val = cce_ui::config::parse_kdl_to_json(content);
        if let Some(obj) = json_val.as_object_mut() {
            for &(block, field, n) in Self::COLOR_FIELDS {
                let Some(b) = obj.get_mut(block).and_then(|v| v.as_object_mut()) else { continue };
                let Some(serde_json::Value::String(hex)) = b.get(field) else { continue };
                let parsed = if n == 4 {
                    hex_to_float_array4(hex).and_then(|a| serde_json::to_value(a).ok())
                } else {
                    hex_to_float_array(hex).and_then(|a| serde_json::to_value(a).ok())
                };
                if let Some(v) = parsed {
                    b.insert(field.to_string(), v);
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
            if let Some(obj) = json_val.as_object_mut() {
                for &(block, field, _) in Self::COLOR_FIELDS {
                    let Some(b) = obj.get_mut(block).and_then(|v| v.as_object_mut()) else { continue };
                    let Some(val) = b.get(field).cloned() else { continue };
                    if let Some(hex) = serde_json::from_value::<[f32; 3]>(val).ok().map(|a| float_array_to_hex(&a)) {
                        b.insert(field.to_string(), serde_json::Value::String(hex));
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
    /// The right dock's left edge — the plate docked there, if any.
    RightDockResize { start_w: f32, start_mouse_x: f32 },
    /// The params HUD's left edge: its own width, which no plate shares.
    HudResize { start_w: f32, start_mouse_x: f32 },
    SpreadsheetResize { start_h: f32, start_mouse_y: f32 },
    /// The spreadsheet's left edge drag, as an inset past the flush position
    /// beside the network pane: a positive inset tucks the spreadsheet UNDER
    /// the pane, whose bottom the layout raises to make room.
    SpreadsheetResizeLeft { start_inset: f32, start_mouse_x: f32 },
    /// The spreadsheet's right edge, symmetrically, tucking under the parameter pane.
    SpreadsheetResizeRight { start_inset: f32, start_mouse_x: f32 },
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
    pub viewport_bg: cce_ui::vk::MeshId,
    pub spheres: cce_ui::vk::MeshId,
    /// LINE_LIST edge expansion of `spheres` (vertex pairs per triangle
    /// edge) — the wire pass draws real line primitives, never
    /// PolygonMode::LINE (driver-broken; see cce-ui's scene stage).
    pub sphere_edges: cce_ui::vk::MeshId,
    pub grid: cce_ui::vk::MeshId,
    pub origin: cce_ui::vk::MeshId,
    pub pivot: cce_ui::vk::MeshId,
    /// The marker spheres the marker draws instance (`geometry::marker_sphere`,
    /// white): the group markers' at Group Marker Size — the selected group,
    /// the marked groups and the spreadsheet's rows share it — the points'
    /// at Point Marker Size and the vertices' at `VERTEX_MARKER_SCALE` of
    /// it. Re-uploaded when a size moves (`State::marker_sphere_radii`),
    /// which is 240 vertices whatever the scene.
    pub group_sphere: cce_ui::vk::MeshId,
    pub point_sphere: cce_ui::vk::MeshId,
    pub vertex_sphere: cce_ui::vk::MeshId,
    /// Selected-Group membership markers: while a Group node is selected, one
    /// marker per vertex it tags, so the selection SHOWS the group. This and
    /// the other marker meshes below hold INSTANCES (cce-ui's
    /// `SceneDraw::instances`) — a marker's place and colour — drawn over
    /// their sphere.
    pub group_points: cce_ui::vk::MeshId,
    /// Markers on the points whose rows are selected in the spreadsheet.
    pub row_points: cce_ui::vk::MeshId,
    /// Markers on the members of the marked groups (the Group Markers dialog).
    pub marked_points: cce_ui::vk::MeshId,
    /// The Show Point Markers overlay.
    pub overlay_points: cce_ui::vk::MeshId,
    /// The Show Vertex Markers overlay.
    pub vertex_points: cce_ui::vk::MeshId,
    /// The Show Point Normals overlay (LINE_LIST whiskers).
    pub overlay_normals: cce_ui::vk::MeshId,
    /// Pull arrows (LINE_LIST): while a point-moving Attribute node is
    /// selected, how far and which way it moves a spread of the points.
    pub pull_arrows: cce_ui::vk::MeshId,
}

/// Which markers [`State::drawn_markers`] reads.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    /// The selected Group node's members.
    Group,
    /// The spreadsheet's selected rows.
    Row,
    /// The marked groups' members.
    Marked,
    /// Show Point Markers and Show Vertex Markers.
    Overlay,
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
    /// The network lattice's line colour (`style.surface.graph.grid_color`);
    /// `grid_color` above is the viewport's ground grid.
    pub graph_grid_color: [f32; 3],
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
    /// The node the params pane's rows were loaded FROM — set by
    /// `sync_parameters_pane`, checked by `sync_parameters_to_project`, which
    /// writes nothing when the selection has moved on since. See
    /// [`State::param_pane_target`].
    pub(crate) param_pane_source: Option<ParamPaneTarget>,
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
    /// The copied nodes, with the positions they were copied FROM — a paste
    /// lays them back out in the same shape, offset to the cursor. A Vec
    /// rather than one node because an expanded cursor selects many, and a
    /// copy that silently took one of them would be a trap.
    pub node_clipboard: Vec<FsNode>,
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
    /// Which page of the viewport menu is up: `None` for the menu itself.
    pub viewport_menu_page: Option<ViewportMenuPage>,
    /// The menu the dialog was turned to from by a page row (Add Node,
    /// Rename, Attribute Visualizers), which a swipe back from it shows
    /// again — see `crate::menu_page`.
    pub dialog_from: Option<crate::menu_page::MenuOrigin>,
    /// The dialog's own modes it was turned through on the way to the one
    /// up, nearest last: Commands under Group Markers, the visualizer list
    /// under one visualizer.
    pub dialog_trail: Vec<crate::dialog::Mode>,
    /// The menu the plate menu's Add Tab page was turned to from.
    pub plate_page_from: Option<crate::menu_page::MenuOrigin>,
    /// The plate PAGE shown from another menu's Plate row: the plate's slot
    /// and that menu. Add Tab turned to from the page goes back to the page,
    /// and the page back to the menu (`open_plate_page`).
    pub plate_page_root: Option<(usize, crate::menu_page::MenuOrigin)>,
    /// A parameter row's right-click menu — the same thread-local; the
    /// target is (node id, parameter name) rather than a slot and a row, so
    /// it holds across a re-layout of the pane.
    pub param_menu_active: bool,
    pub param_menu_actions: Vec<ParamMenuAction>,
    /// The playbar's right-click menu, the same contract.
    pub playbar_menu_active: bool,
    pub playbar_menu_actions: Vec<PlaybarMenuAction>,
    pub param_menu_target: Option<(String, String)>,
    /// Copy Parameter's clipboard: (node id, parameter name). An id, so a
    /// rename between the copy and the paste still pastes the right path.
    pub copied_param: Option<(String, String)>,
    /// The node the rename dialog is open on, by id.
    pub rename_target: Option<String>,
    /// Undo for edits to the node tree, parameters and structure — see
    /// `src/edit_history.rs`. Consulted after a code row and a viewer
    /// state have had their turn.
    pub edit_history: crate::edit_history::EditHistory,
    /// The tree as it stood when its structure was last looked at, which
    /// the next look is compared with. None until the first.
    pub structure_base: Option<FsNode>,
    /// The network editor's right-click menu — the same thread-local again,
    /// with the flag saying the open menu is this one.
    pub network_menu_active: bool,
    pub network_menu_actions: Vec<NetworkMenuAction>,
    /// The menu the network menu was turned to from — the viewport's, while
    /// the network overlays the scene — which its back band returns to. None when a right
    /// press on the graph opened it.
    pub network_menu_from: Option<crate::menu_page::MenuOrigin>,
    /// The grid cell under the viewport menu's right press, while the
    /// network has no plate and the press was over its area: where the
    /// grid cursor goes if the menu is turned to the network's, so Add Node
    /// places at the cell pointed at, as the plated network's press does.
    pub network_menu_cell: Option<(i32, i32)>,
    /// Which of the two a scroll gesture over the plateless network goes
    /// to — the graph (a node was under the pointer when it began) or the
    /// scene — with where and when its last event came. Held for the whole
    /// gesture: a pan slides the node out from under a pointer that does
    /// not move, and the rest of the swipe must not become an orbit.
    pub overlay_wheel: Option<OverlayWheel>,
    /// The plate corner menu — same `context_menu` thread-local again; the slot
    /// says which plate's control opened it (and doubles as the pressed state
    /// the corner control paints with).
    /// Solved simulation states, kept across frames so playing forward costs one
    /// step per frame instead of re-solving from the start frame every redraw.
    pub sim_cache: crate::geometry::SimCache,
    /// What the playbar's cache strip was last worked out from: the sim
    /// cache's revision, the geometry version (an edit moves it, and may
    /// make what is cached stale without solving anything) and the frame
    /// range. See [`State::sync_playbar_cache`].
    pub playbar_cache_key: Option<(u64, u64, i32, i32)>,
    /// The GPU image of the page the viewport shows. Owned here, so
    /// replacing a page frees the one it replaces.
    pub page_image: Option<u32>,
    /// The page that image is of: what the scene pass places it by and the
    /// framing commands fit the camera to. None when the level shows none.
    pub page_shown: Option<crate::page::PageShown>,
    /// What the page on show was composed from ([`crate::page::chain_key`])
    /// and the status line that announced it. A geometry rebuild — any edit
    /// at the level, every frame of a playing sim — recomposes the page, and
    /// a Letter sheet at 300 DPI is ~90 ms of compose and conversion plus a
    /// 34 MB upload; with this the ones that change nothing it reads cost a
    /// hash of its chain.
    pub page_composed: Option<(u64, String)>,
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
    /// An in-flight camera PAN drag (middle button, or shift and the left):
    /// the cursor position the last motion was measured from.
    pub pan_drag: Option<(f32, f32)>,
    /// The camera node a pan last wrote, with the Pivot and Position it
    /// wrote IN FULL. The node holds them as text, to four decimals; a pan
    /// is hundreds of small moves, and each read back from the text would
    /// lose what the text could not hold. Used while the node still says
    /// what was written.
    pub pan_exact: Option<(String, Vec3, Vec3)>,
    /// Set when the page raster must be re-uploaded — after a replacement
    /// renderer drops the old id. Consumed on the next tick rather than acted
    /// on in `renderer_init`, which runs before the frame has settled and
    /// where relaying the panes would be premature.
    pub page_dirty: bool,
    /// The window title wants re-deriving (`update_window_title`), at most
    /// once a frame: deriving it serializes the whole tree to compare with
    /// the save, and the event path asked on every changed pointer event.
    pub title_dirty: bool,
    /// A dialog setting changed during a slider drag and is saved when the
    /// drag ends (`apply_setting`), not on every motion of it.
    pub settings_save_pending: bool,
    /// Frame the scene was last built at, so the timeline moving can invalidate it.
    pub last_sim_frame: i32,
    pub plate_menu_slot: Option<usize>,
    pub plate_menu_actions: Vec<crate::plate_menu::PlateMenuAction>,
    /// Panes shrunk to their title stub, indexed by slot. Only the
    /// `plate_menu::PLATE_SLOTS` entries are ever set.
    pub collapsed_panes: [bool; WIDGET_COUNT],
    /// Dock occupancy, indexed Left/Right/Bottom. Swapped by the plate
    /// menu's Move To rows.
    /// With tabs this names each dock's ACTIVE pane — always a member of the
    /// dock's `dock_tabs` list — or [`NO_PANE`] for a dock whose tabs were
    /// all pulled elsewhere.
    pub dock_panes: [usize; 3],
    /// The panes tabbed into each dock, indexed Left/Right/Bottom. One dock
    /// rect, several panes: only the active one (`dock_panes`) is laid out;
    /// the rest wait as tabs, switched and moved through the plate
    /// menus. Every docked pane lives in exactly ONE dock's list.
    pub dock_tabs: [Vec<usize>; 3],

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
    /// The grid cursor EXPANDED over a region: the cell it was anchored at
    /// when a drag began, and the far cell that drag reached.
    ///
    /// Read through [`grid_cursor_region`](State::grid_cursor_region), which
    /// hands it back only while its anchor is still the live cursor cell.
    /// That is the whole collapse rule: every OTHER way the cursor moves — a
    /// nav key, a click, a load, a selection following a node — leaves the
    /// anchor behind and the region goes with it, without a line in any of
    /// those places. A flag reset by hand at fifteen call sites is a flag
    /// that gets missed at one of them, and a cursor left stretched across
    /// the sheet is not a subtle wrong.
    pub grid_cursor_expanse: Option<((i32, i32), (i32, i32))>,
    /// A node drag that is carrying the whole selection. `None` for an
    /// ordinary one-node drag, which the widget handles by itself.
    pub node_drag_group: Option<NodeDragGroup>,
    /// The anchor of a LIVE expansion drag — a left press on empty grid,
    /// cleared on release. `Some` is what makes motion grow the region
    /// rather than do nothing; the region it leaves behind outlives it.
    pub grid_cursor_drag: Option<(i32, i32)>,
    pub modifiers: ModifiersState,

    pub width: f32,
    pub height: f32,
    pub physical_width: u32,
    pub physical_height: u32,
    pub scale: f64,
    pub square_viewport: bool,
    /// Whether a dragged node snaps to cells: always, in this app — to a
    /// cell it can land on (cce-ui's `Graph::drag_update`). Not read from
    /// `style.surface.graph.grid_snap`.
    pub grid_snap_enabled: bool,
    pub network_grid_visible: bool,
    /// The network grid's pitch at the current zoom — centre of one grid
    /// line to the centre of the next, per axis. The grid's one size; a
    /// node's (col, row) is the intersection its centre sits on.
    /// The configured grid geometry (`configured_grid_geometry`) the live
    /// one is a zoom of — the 100% baseline as it was last read. A config
    /// reload that moves it re-applies the grid at the zoom in hand
    /// (`update_graph_settings_from_config`).
    pub grid_base: GridGeometry,
    pub grid_pitch_x: f32,
    pub grid_pitch_y: f32,
    /// The node body's size at the current zoom — its own, not the pitch's;
    /// zoom scales the two together and nothing else relates them.
    pub node_w: f32,
    pub node_h: f32,

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
    /// Whether the params pane draws its PLATE. With it off the rows stand
    /// directly over the scene, and the pane claims only what its rows
    /// cover (`params_claim`), so the scene under the rest of its rect
    /// orbits, takes the wheel and shows its point numbers.
    pub params_plate: bool,
    pub show_viewport: bool,
    pub show_parameters: bool,
    pub show_spreadsheet: bool,
    pub show_playbar: bool,
    pub last_spreadsheet_node_name: Option<String>,
    pub last_spreadsheet_node_params: Option<Vec<(String, String)>>,
    /// The frame and geometry version the spreadsheet's rows were read at.
    pub last_spreadsheet_read_at: (i32, u64),
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
    /// The params plate as drawn: `[x, y, w, h, round]`, eased each tick
    /// toward `params_plate_target` — `round` 0 is the plate fitted to the
    /// rows, 1 the small circle it collapses to with no rows to show. `None`
    /// with no plate, and before the first tick (the target is drawn then).
    pub params_plate_shown: Option<[f32; 5]>,
    pub network_opacity: f32,
    /// Node-domain opacity (style.surface.graph.node.opacity) — independent of
    /// the pane's network_opacity; fades node bodies/wires/ports and node text.
    pub node_opacity: f32,
    /// The node bodies' own backdrop compression (`style.surface.graph.
    /// node_compression`, 0..1), overriding the pane material's for nodes
    /// only — the plates can stay clear while the tablets sitting on them
    /// pull the view toward the tint's key and read as solid. None = the
    /// pane's.
    pub node_compression: Option<f32>,
    /// The node bodies' own tint (`style.surface.graph.node_tint`, rgba,
    /// LINEAR once read), overriding the pane material's for the bodies and
    /// the name floors — what compression pulls the backdrop toward, so a
    /// light tint is a light node. None = the pane's. See
    /// [`State::node_ink`] for what it does to the names.
    pub node_tint: Option<[f32; 4]>,
    pub last_design_mod_time: Option<std::time::SystemTime>,
    pub last_config_mod_time: Option<std::time::SystemTime>,
    pub floating_network_layout: (f32, f32, f32, f32),
    /// The active app-mode drag (pane edge resize), if any. See [`AppDrag`].
    pub app_drag: Option<AppDrag>,
    pub floating_param_width: f32,
    /// The params HUD's width as asked for (`params_hud_rect` fits it to
    /// the viewport). Its own, apart from the right dock's: the HUD lives
    /// on the scene, not in a dock.
    pub params_hud_width: f32,
    pub floating_spreadsheet_height: f32,
    /// How far the spreadsheet's left/right edge reaches INTO the neighboring
    /// pane's span past its flush position (0 = glued beside the neighbor).
    /// A positive inset tucks the spreadsheet UNDER that neighbor: the layout
    /// raises the neighbor's bottom edge to the spreadsheet's top.
    pub floating_spreadsheet_inset_left: f32,
    pub floating_spreadsheet_inset_right: f32,
    /// Whether the compositor has told us the window's size yet. Until it
    /// has, `width`/`height` are `State::new`'s 1280x800 placeholder.
    pub window_configured: bool,
    /// Plate fractions a load applied against that placeholder, held so the
    /// first real `resize` can apply them again at the real size. The
    /// startup project loads before the first configure, and `resize`
    /// keeps plates at their pixel size, so without this every launch in a
    /// window other than 1280x800 opened with the plates scaled by
    /// 1280/width — and a save then stored the scaled fractions.
    pub pending_plates: Option<PlateGeometry>,
    pub loaded_project_path: Option<std::path::PathBuf>,
    /// The configured startup project (`DesignSettings::default_project`),
    /// mirrored live so "Set As Default" can rewrite it and `save_settings` —
    /// which reconstructs DesignSettings from live state — can carry it.
    pub default_project_setting: Option<String>,
    /// The GPU setting (`DesignSettings::gpu`), mirrored live like the
    /// startup project so `save_settings` carries it. Takes effect at the
    /// next launch; `gpu_at_launch` is what this process is running on.
    pub gpu_preference: String,
    /// `CCE_VK_DEVICE` as the renderer read it — "integrated" when unset —
    /// so the setting row can say whether a change is still pending.
    pub gpu_at_launch: String,
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
    /// RGB, used in single-colour mode only.
    pub wire_color: [f32; 3],
    /// The wire pass's opacity ("Wire Opacity"), in both colour modes — the
    /// geometry Opacity slider affects only the polygons.
    pub wire_opacity: f32,
    /// Wire line width in framebuffer pixels ("Wire Thickness" slider).
    pub wire_width: f32,
    pub last_viewport_wire_single_color: bool,
    pub last_viewport_wire_color: [f32; 3],
    pub last_viewport_wire_opacity: f32,
    pub last_viewport_wire_width: f32,
    /// Opacity of the rendered node geometry (the Render node's "Opacity"
    /// slider): 1.0 opaque, straight-alpha blended toward the viewport bg.
    pub geo_opacity: f32,
    pub last_viewport_geo_opacity: f32,
    /// Selected-Group marker radius in world units (a setting row of the
    /// dialog and a viewport-menu slider; persisted in the render block).
    pub group_marker_size: f32,
    /// Pull arrow length as a multiple of the true displacement (a setting
    /// row of the dialog and a viewport-menu slider; persisted in the render
    /// block).
    pub pull_arrow_scale: f32,
    /// Smooth shading of the scene fill (`toggle_smooth_shading`). While on,
    /// `scene_smooth_verts` holds the lit fill and the draw is `prelit`.
    pub smooth_shading: bool,
    /// The scene fill with smooth shading baked into its colours
    /// (`geometry::smooth_lit_vertices`), built by `rebuild_scene_geometry`
    /// while `smooth_shading` is on and empty otherwise. The raster pass
    /// uploads this in place of `rt_sphere_verts`, which the path tracer
    /// keeps reading unlit.
    pub scene_smooth_verts: Vec<Vertex3D>,
    /// The scene's light, as the Environment node at the root says (or its
    /// defaults) — read by `present_scene` and `sync_environment`, handed
    /// to both views by the stage pass. See `crate::environment`.
    pub environment: crate::environment::Environment,
    /// See-through fill (`toggle_show_occluded`), in effect only below full
    /// opacity — at 100% there is nothing to see through, and the ordinary
    /// depth-writing fill is exact.
    pub show_occluded: bool,
    /// What the fill mesh was last SORTED for: (geometry version, smooth,
    /// eye in mesh space). The stage pass re-sorts and re-uploads when any
    /// of the three moves while see-through is in effect — which during an
    /// orbit is every frame — and clears it when see-through ends, so the
    /// next entry sorts afresh.
    pub sorted_fill_key: Option<(u64, bool, [f32; 3])>,
    /// Selected-Group membership markers: marker vertices staged CPU-side by
    /// `sync_nodes` whenever the selection is a Group node (empty otherwise),
    /// flushed to `meshes.group_points`; `group_point_count` gates the
    /// draw. The key — (node id, params, geometry version) — spares the
    /// re-evaluation on unrelated `sync_nodes` runs.
    pub group_point_instances: Vec<Vertex3D>,
    /// The spreadsheet's selected rows, shown in the scene: a marker on the
    /// point each row is. `spreadsheet_points` is where the rows' points
    /// were when the table was last filled, kept so a selection stages its
    /// markers without an evaluation; the markers are staged by
    /// `rebuild_row_markers` and flushed to `meshes.row_points`.
    pub spreadsheet_points: Vec<[f32; 3]>,
    pub row_marker_instances: Vec<Vertex3D>,
    pub row_markers_dirty: bool,
    pub row_marker_count: u32,
    pub group_points_dirty: bool,
    pub group_point_count: u32,
    pub last_group_points_key: Option<(String, Vec<(String, String)>, u64)>,
    /// Pull arrows: while the params pane shows an Attribute node that moves
    /// points (`geometry::moves_points`), an arrow from where each of a
    /// spread of its points was to where the node puts it — staged by
    /// `sync_pull_arrows`, flushed to `meshes.pull_arrows`. Keyed like the
    /// group markers, by (node id, params, geometry version).
    pub pull_arrow_verts: Vec<Vertex3D>,
    /// The sampled `(before, after)` pairs the arrows were built from, kept
    /// so Pull Arrow Scale can re-draw them without re-evaluating the node
    /// (`rebuild_pull_arrow_verts`).
    pub pull_arrow_pairs: Vec<(glam::Vec3, glam::Vec3)>,
    pub pull_arrows_dirty: bool,
    pub pull_arrow_count: u32,
    pub last_pull_arrows_key: Option<(String, Vec<(String, String)>, u64)>,
    /// The selected Group's member positions, kept from the evaluation so
    /// the markers can be re-SIZED without re-evaluating the node — a
    /// marker size change (the viewport menu's slider, per motion of a
    /// drag) only rebuilds the spheres (`rebuild_group_markers`).
    pub group_members: Vec<Vertex3D>,
    /// The marker radius `group_point_instances` was built at.
    pub last_group_marker_size: f32,
    /// The point overlays on the visible scene, rebuilt with it: marker
    /// geometry for Show Point Markers, and (position, vertex index) labels
    /// for Show Point Numbers — the labels project through `last_scene_mvp`
    /// into 2D text each frame.
    ///
    /// These were per-node preferences on a hidden `meta` child until
    /// 2026-09-23, so seeing the point numbering of what was on screen meant
    /// diving into each node and flipping its own switch. They are display
    /// settings, and display settings belong to the view: three commands in
    /// the palette (`toggle_point_markers` / `_numbers` / `_normals`) over
    /// the flags below, persisted in `ViewportSettings` beside Show Grid.
    pub overlay_marker_instances: Vec<Vertex3D>,
    /// The scene's point positions, kept by `rebuild_scene_geometry` while
    /// Show Point Markers is on (empty otherwise), so the markers can be
    /// re-SIZED without re-evaluating the graph — the viewport menu's Point
    /// Marker Size slider does that on every motion of a drag
    /// (`rebuild_overlay_markers`).
    pub overlay_marker_points: Vec<Vertex3D>,
    /// The same for Show Vertex Markers: where each vertex's marker
    /// stands, inset from its point as its number is.
    pub overlay_vertex_marker_points: Vec<Vertex3D>,
    pub overlay_dirty: bool,
    pub overlay_point_count: u32,
    /// The Show Vertex Markers instances, built beside the points'
    /// (`rebuild_overlay_markers`) and flushed with them.
    pub vertex_marker_instances: Vec<Vertex3D>,
    pub vertex_marker_count: u32,
    /// The radii the marker spheres were last uploaded at — group, point,
    /// vertex — so the flush re-uploads them when a size moves.
    pub marker_sphere_radii: Option<[f32; 3]>,
    pub overlay_number_labels: Vec<([f32; 3], u32)>,
    /// How much of each label above shows through the fill in front of its
    /// point (`geometry::point_transmittance`), worked out by the stage
    /// pass for the eye it staged. The labels are 2D text, so this is the
    /// only occlusion they get. Emptied with every rebuild of the labels; a
    /// label with no entry draws whole.
    pub overlay_number_alpha: Vec<f32>,
    /// Show Primitive Numbers and Show Vertex Numbers, the same two lists
    /// each: a primitive's label stands at its centroid, a vertex's inside
    /// its primitive, part of the way from its point to that centroid, so
    /// the vertices that share a point are told apart.
    pub overlay_prim_labels: Vec<([f32; 3], u32)>,
    pub overlay_prim_alpha: Vec<f32>,
    pub overlay_vertex_labels: Vec<([f32; 3], u32)>,
    pub overlay_vertex_alpha: Vec<f32>,
    /// Show Point Normals: LINE_LIST whiskers from each distinct point along
    /// its smooth vertex normal (computed from topology — the kernel outputs
    /// carry only a default up-normal attribute).
    pub overlay_normal_verts: Vec<Vertex3D>,
    pub overlay_normal_count: u32,
    /// The three overlay switches, flipped by their palette commands and
    /// persisted in `ViewportSettings`.
    pub show_point_markers: bool,
    pub show_point_numbers: bool,
    pub show_point_normals: bool,
    pub show_prim_numbers: bool,
    pub show_prim_normals: bool,
    pub show_vertex_numbers: bool,
    pub show_vertex_markers: bool,
    pub show_vertex_normals: bool,
    /// The point groups whose members wear a marker in the scene, by name
    /// — the Group Markers dialog's switches (`group_markers`), persisted
    /// in the viewport block. A name the scene has no group for stays on
    /// the list and marks nothing, so a switch set for a group that comes
    /// and goes with a frame or an edit is not lost with it.
    pub marked_groups: Vec<String>,
    /// The attribute visualizers, applied in order to the displayed scene
    /// (`crate::visualizer`). Persisted in the viewport block.
    pub visualizers: Vec<crate::visualizer::Visualizer>,
    /// The params HUD shows the attribute visualizers, editing visualizer
    /// `i` (0 with none yet), in place of the selected node's parameters —
    /// `visualizer::State::open_visualizers_hud`. `None` is the node.
    pub vis_hud: Option<usize>,
    /// What the HUD would have shown when the visualizers took it: picking
    /// another node hands the HUD back to that node's parameters.
    pub(crate) vis_hud_from: Option<ParamPaneTarget>,
    /// The half-span the Attribute node's Value row runs over, with the
    /// node it is for (`value_row_span`): kept between pane syncs so the row
    /// re-scales only when its value leaves it.
    pub value_row_span: Option<(String, f32)>,
    /// The same for the shown node's `float2` rows, by parameter name, with
    /// the node they are for (`present_float2_rows`).
    pub float2_spans: Option<(String, Vec<(String, f32)>)>,
    /// The displayed scene's point attributes as last built, with their
    /// ranges: what the visualizer editor offers.
    pub scene_attributes: Vec<crate::visualizer::SceneAttribute>,
    /// The displayed scene as last EVALUATED, before the visualizers: what a
    /// visualizer edit re-presents (`State::revisualize`) without running
    /// the graph again.
    pub scene_base: Option<crate::detail::Detail>,
    /// Every point group of the scene as last built, with its members'
    /// positions: what the dialog lists and what the markers are built
    /// from, so a switch flipped evaluates nothing.
    pub scene_groups: Vec<(String, Vec<[f32; 3]>)>,
    /// The marked groups' markers, staged by `rebuild_marked_group_markers`,
    /// flushed to `meshes.marked_points`.
    pub marked_group_instances: Vec<Vertex3D>,
    pub marked_groups_dirty: bool,
    pub marked_group_count: u32,
    /// The visible scene's own edges for the wire pass (LINE_LIST pairs),
    /// rebuilt with the scene while Show Wireframe is on and empty while it
    /// is off. Topological — see `render::scene_edge_verts`.
    pub scene_edge_verts: Vec<Vertex3D>,
    /// World-unit radius of the Show Point Markers overlay.
    pub point_marker_size: f32,
    /// sRGB color of the Show Point Markers overlay.
    pub point_marker_color: [f32; 3],
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
    pub pick_cache: Option<((String, u64), PickLists)>,
    /// The raster scene's model-view-projection and the viewport pane rect in
    /// LOGICAL px, cached at staging so the 2D pass can project 3D overlays.
    pub last_scene_mvp: Option<Mat4>,
    /// What the numbers' dimming was last worked out for: the view, the
    /// geometry, the opacity, whether the fill is seen through, how many.
    pub number_alpha_key: Option<([u32; 16], u64, u32, bool, usize)>,
    /// The eye the scene was last staged for, in mesh space, beside the
    /// matrix: what a scene rebuild dims the numbers by until the stage
    /// pass has staged the new geometry.
    pub last_scene_eye: Vec3,
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
    /// The `rt_geometry_version` the RT scene was last built from.
    /// The traced scene as it was last handed over: the geometry's version,
    /// the image's, and the world unit's bits, which size the image.
    pub last_rt_scene_key: Option<(u64, u64, u32)>,
    /// Counts the recompositions of the shown image, as
    /// `rt_geometry_version` counts the geometry's.
    pub page_version: u64,
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
            self.params_hud_width.round(),
            self.floating_spreadsheet_height.round(),
            self.floating_spreadsheet_inset_left.round(),
            self.floating_spreadsheet_inset_right.round(),
        );
        // The display settings ride the file now, so changing one is an
        // edit the title's asterisk should show.
        let display = serde_json::to_string(&vs.display).unwrap_or_default();
        serde_json::to_string(&(
            vs.collapsed_panes,
            splitters,
            vs.dock_tabs,
            vs.frame_range,
            vs.viewport_pin,
            vs.params_pin,
            vs.spreadsheet_pin,
            plates,
            display,
        ))
        .unwrap_or_default()
    }

    /// Record the live tree and pane layout as the saved baseline — every
    /// save and load path calls this, so the title's asterisk clears.
    pub fn mark_saved(&mut self) {
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        self.last_saved_layout_json = self.pane_layout_json();
    }






    /// Whether the fill draws see-through this frame: Show Occluded is on
    /// and the fill is translucent. At full opacity the ordinary fill is
    /// exact and cheaper — no sort, no re-upload.
    pub fn see_through_active(&self) -> bool {
        self.show_occluded && self.geo_opacity < 0.999
    }

    /// The live display settings — what state.kdl and a project's
    /// `display` block both carry.
    pub fn display_settings(&self) -> DisplaySettings {
        DisplaySettings {
            viewport: ViewportSettings {
                bg_color: self.viewport().bg_color,
                square: self.square_viewport,
                show_camera_pivot_enabled: self.viewport().show_camera_pivot,
                camera_pivot_size: self.camera_pivot_size,
                show_grid_enabled: self.viewport().show_grid,
                show_origin_enabled: self.viewport().show_origin,
                origin_size: self.origin_size,
                grid_thickness: self.grid_thickness,
                grid_color: self.viewport().grid_color,
                params_plate: self.params_plate,
                node_wire_style: self.slots.content.inner().chosen_wire_style().map(|w| w.name().to_string()).unwrap_or_default(),
                show_point_markers: self.show_point_markers,
                show_point_numbers: self.show_point_numbers,
                show_point_normals: self.show_point_normals,
                show_prim_numbers: self.show_prim_numbers,
                show_prim_normals: self.show_prim_normals,
                show_vertex_numbers: self.show_vertex_numbers,
                show_vertex_markers: self.show_vertex_markers,
                show_vertex_normals: self.show_vertex_normals,
                marked_groups: Self::join_marked_groups(&self.marked_groups),
                visualizers: crate::visualizer::encode(&self.visualizers),
                point_marker_size: self.point_marker_size,
                point_marker_color: self.point_marker_color,
                world_unit: self.world_unit.suffix().to_string(),
                rt_mode: self.viewport().rt_mode,
                circular_pane: self.circular_network_pane,
            },
            render: RenderSettings {
                wireframe: self.wireframe,
                wire_single_color: self.wire_single_color,
                wire_color: self.wire_color,
                wire_opacity: self.wire_opacity,
                wire_width: self.wire_width,
                geo_opacity: self.geo_opacity,
                group_marker_size: self.group_marker_size,
                pull_arrow_scale: self.pull_arrow_scale,
                smooth_shading: self.smooth_shading,
                show_occluded: self.show_occluded,
            },
        }
    }

    pub fn save_settings(&mut self) {
        let display = self.display_settings();
        let settings = DesignSettings {
            viewport: display.viewport,
            render: display.render,
            default_project: self.default_project_setting.clone(),
            gpu: self.gpu_preference.clone(),
            playbar_repeat: self.slots.playbar.inner().repeat,
            playbar_fps: self.slots.playbar.inner().fps,
            playbar_step_buttons: self.slots.playbar.inner().step_buttons,
        };
        settings.save();
        self.last_design_mod_time = {
            let design_path = DesignSettings::file_path();
            std::fs::metadata(&design_path).and_then(|m| m.modified()).ok()
        };
    }



    /// Put a project's display settings onto the live state: every field
    /// `display_settings` reads, then the regeneration the dialog's apply
    /// runs (the viewport meshes bake sizes and colours in), the layout for
    /// the two pane-shaped ones, the menus' checkmarks, and state.kdl — so
    /// the last-used look follows the project that was opened.
    ///
    /// Main window only, like the pane state: a detached window has no
    /// viewport, and it reloads the sync channel on every write, so letting
    /// it apply and persist would race the main window's own state.kdl.
    pub(crate) fn apply_display_settings(&mut self, d: &DisplaySettings) {
        if self.is_detached_network || self.detached_pane.is_some() {
            return;
        }
        let (v, r) = (&d.viewport, &d.render);
        {
            let vp = self.viewport_mut();
            vp.bg_color = v.bg_color;
            vp.show_grid = v.show_grid_enabled;
            vp.show_origin = v.show_origin_enabled;
            vp.show_camera_pivot = v.show_camera_pivot_enabled;
            vp.grid_color = v.grid_color;
            vp.rt_mode = v.rt_mode;
        }
        self.grid_color = v.grid_color;
        self.square_viewport = v.square;
        self.camera_pivot_size = v.camera_pivot_size;
        self.origin_size = v.origin_size;
        self.grid_thickness = v.grid_thickness;
        self.params_plate = v.params_plate;
        self.set_node_wire_style(cce_ui::widget::display::WireStyle::parse(&v.node_wire_style));
        self.circular_network_pane = self.is_detached_network || v.circular_pane;
        self.show_point_markers = v.show_point_markers;
        self.show_point_numbers = v.show_point_numbers;
        self.show_point_normals = v.show_point_normals;
        self.show_prim_numbers = v.show_prim_numbers;
        self.show_prim_normals = v.show_prim_normals;
        self.show_vertex_numbers = v.show_vertex_numbers;
        self.show_vertex_markers = v.show_vertex_markers;
        self.show_vertex_normals = v.show_vertex_normals;
        self.marked_groups = Self::marked_groups_of(&v.marked_groups);
        self.visualizers = crate::visualizer::decode(&v.visualizers);
        self.point_marker_size = v.point_marker_size;
        self.point_marker_color = v.point_marker_color;
        if let Some(u) = cce_ui::units::Unit::parse(&v.world_unit) {
            self.world_unit = u;
        }
        self.wireframe = r.wireframe;
        self.wire_single_color = r.wire_single_color;
        self.wire_color = r.wire_color;
        self.wire_opacity = r.wire_opacity;
        self.wire_width = r.wire_width;
        self.geo_opacity = r.geo_opacity;
        self.group_marker_size = r.group_marker_size;
        self.pull_arrow_scale = r.pull_arrow_scale;
        self.rebuild_pull_arrow_verts();
        self.smooth_shading = r.smooth_shading;
        self.show_occluded = r.show_occluded;

        // The checkmarks the guide and pane toggles keep in step by hand.
        let (sg, so, cp) = {
            let vp = self.viewport();
            (vp.show_grid, vp.show_origin, vp.show_camera_pivot)
        };
        self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(GUIDES_MENU, GUIDE_GRID, sg);
        self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(GUIDES_MENU, GUIDE_ORIGIN, so);
        self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(GUIDES_MENU, GUIDE_CAMERA_PIVOT, cp);
        let cnp = self.circular_network_pane;
        self.menu_mut(LEFT_MENUBAR_IDX).set_item_checked(2, 2, cnp);

        self.update_grid_geometry();
        self.update_origin_geometry();
        self.update_pivot_geometry();
        self.update_viewport_bg_geometry();
        self.sync_grid_settings();
        self.rebuild_positions();
        self.apply_layout();
        // The visualizers came with the block: shown on the scene in hand.
        self.revisualize();
        self.viewport_dirty = true;
        self.save_settings();
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

    /// Hand the playbar what the simulations hold ([`playbar_cache_runs`]),
    /// worked out again only when the cache, the geometry or the frame
    /// range has moved. Only simnets still in the tree count, and a
    /// solve whose simnet has been edited since it ran is stale whole —
    /// the edit is in the tree but not solved, as for a simnet nothing on
    /// screen reads. Returns whether the strip changed.
    pub fn sync_playbar_cache(&mut self) -> bool {
        let (start, end) = {
            let pb = self.slots.playbar.inner();
            (pb.start_frame.round() as i32, pb.end_frame.round() as i32)
        };
        let key = (self.sim_cache.revision(), self.rt_geometry_version, start, end);
        if self.playbar_cache_key == Some(key) {
            return false;
        }
        self.playbar_cache_key = Some(key);
        let solved = self.sim_cache.solved();
        let mut chains = std::collections::HashMap::new();
        if !solved.is_empty() {
            fn walk(n: &FsNode, solved: &[crate::geometry::SolvedRange], out: &mut std::collections::HashMap<String, u64>) {
                if n.node_type.eq_ignore_ascii_case("simnet") && solved.iter().any(|r| r.id == n.id) {
                    out.insert(n.id.clone(), crate::geometry::chain_hash(n));
                }
                for c in &n.children {
                    walk(c, solved, out);
                }
            }
            walk(&self.fs_root, &solved, &mut chains);
        }
        let runs = playbar_cache_runs(&solved, &chains, start, end);
        let pb = self.slots.playbar.inner_mut();
        if pb.cache == runs {
            return false;
        }
        pb.cache = runs;
        true
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
    
    /// How far a wheel notch slides the camera under shift, in logical px.
    pub(crate) const PAN_PX_PER_LINE: f32 = 40.0;

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
    /// Is the pointer CAPTURED — owned by a gesture or a modal rather than
    /// free to hover whatever it is over? A widget drag, an app drag (pane
    /// edges, the dock), a camera orbit, a network pan, a grid expansion
    /// drag, a viewer-tool handle grab, a held viewport-menu slider, or the
    /// dialog. While it is, no pane hovers (`broadcast_pointer`).
    pub(crate) fn pointer_captured(&self) -> bool {
        self.drag_widget.is_some()
            || self.app_drag.is_some()
            || self.orbit_drag.is_some()
            || self.pan_drag.is_some()
            || self.is_panning
            || self.grid_cursor_drag.is_some()
            || self.viewer_tool.as_ref().map_or(false, |t| t.drag.is_some())
            || (self.slider_menu_open() && cce_ui::widget::context_menu::slider_dragging())
            || self.dialog_visible()
    }

    /// Hand every pane the pointer: its real position when the pane is
    /// under it and the pointer is free, an off-screen one otherwise, so a
    /// control's hover tracks the pointer exactly while the pointer can
    /// reach it. Returns whether any pane changed.
    ///
    /// Until 2026-09-28 this ran only while no widget or app drag was live,
    /// and the arm's early returns skipped it for the other gestures — so a
    /// control hovered at a press stayed lit for the length of an orbit, a
    /// node drag or a pane resize, and lit again only when the pointer next
    /// crossed it. A captured pointer now clears every pane, and the
    /// release re-broadcasts (`MouseInput`) so what is under the pointer
    /// hovers at once. The slot driving a widget drag is left alone: its
    /// `DragUpdate` stream is its motion. The dialog is modal, so while it
    /// is up only its own slot sees the pointer, and `close_dialog` hands
    /// it back.
    pub(crate) fn broadcast_pointer(&mut self) -> bool {
        let captured = self.pointer_captured();
        let dialog = self.dialog_visible();
        let mut changed = false;
        for i in 0..WIDGET_COUNT {
            if self.drag_widget == Some(i) {
                continue;
            }
            let (cx, cy) = (self.cursor_x, self.cursor_y);
            let free = if dialog { i == crate::slots::DIALOG_IDX } else { !captured };
            let is_network_part = i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX || i == NETWORK_PANEL_IDX;
            let inside = free
                && if self.circular_network_pane && is_network_part {
                    if i == CONTENT_IDX {
                        self.circular_network_layout.hit_test_content(cx, cy, 0.0, breadcrumb_h())
                    } else if i == LEFT_MENUBAR_IDX {
                        false
                    } else if i == BREADCRUMB_IDX {
                        self.circular_network_layout.hit_test_breadcrumb(cx, cy, 0.0, breadcrumb_h())
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
        changed
    }

    /// Slide the camera across its own view by a pointer delta, in logical
    /// px: the pivot and the eye move together, in the plane of the screen,
    /// so the view turns nowhere and the scene follows the pointer. What is
    /// on the PIVOT's plane moves exactly as far as the pointer did — the
    /// projection is a perspective, so what is nearer moves further and
    /// what is beyond it less.
    ///
    /// The Default Camera's eye hangs off its pivot, so its pivot is all
    /// that moves. A camera node has its Pivot and Position rewritten, as
    /// Frame All rewrites them.
    /// The scene's pane and the view through it AS THEY ARE NOW: the pane's
    /// rect in physical px, the projection and the view. What the stage
    /// pass stages the scene by, and `None` where there is no scene to
    /// stage — a detached window, a hidden viewport, a pane of no size.
    pub(crate) fn scene_view(&self) -> Option<((u32, u32, u32, u32), Mat4, Mat4)> {
        if self.is_detached_network || self.detached_pane.is_some() || !self.show_viewport {
            return None;
        }
        let s = self.scale as f32;
        let (mut sx, mut sy) = (0u32, (HEADER_H * s) as u32);
        let (mut cw, mut ch) = ((self.width * s) as u32, (self.body_h() * s) as u32);
        if self.square_viewport {
            let side = cw.min(ch);
            sx += (cw - side) / 2;
            sy += (ch - side) / 2;
            (cw, ch) = (side, side);
        }
        if cw == 0 || ch == 0 {
            return None;
        }
        let (pos, rot, pivot) = self.active_camera_pose();
        let (proj, view, model) = self.viewport().get_matrices(cw as f32 / ch as f32, Some(pos), Some(rot), Some(pivot));
        Some(((sx, sy, cw, ch), proj, view * model))
    }

    /// Bring what the 2D frame projects by — `last_scene_mvp`, the pane's
    /// rect, the eye — up to the camera as it is now, ahead of painting.
    ///
    /// The 2D frame is painted BEFORE the stage pass stages the scene, and
    /// until 2026-09-29 the stage pass was the one place these were set: so
    /// everything the 2D frame draws over the scene — the point, primitive
    /// and vertex numbers, a viewer state's handles —
    /// was placed by the camera of the frame BEFORE, and trailed the
    /// geometry and its markers by a frame whenever the camera moved. When
    /// it stopped they stood a frame's move off their points until
    /// something else drew. Not in the traced mode, which keeps the view
    /// the raster pass last staged, as it did.
    pub(crate) fn refresh_scene_view(&mut self) {
        if self.viewport().rt_mode {
            return;
        }
        let Some(((sx, sy, cw, ch), proj, view)) = self.scene_view() else { return };
        let s = (self.scale as f32).max(0.001);
        let mvp = proj * view;
        self.last_scene_mvp = Some(mvp);
        self.last_scene_view_rect = (sx as f32 / s, sy as f32 / s, cw as f32 / s, ch as f32 / s);
        self.last_scene_eye = view.inverse().transform_point3(Vec3::ZERO);
        self.sync_point_number_alpha(mvp, self.last_scene_eye);
    }

    pub(crate) fn pan_camera_by(&mut self, dx_px: f32, dy_px: f32) {
        let (pos, rot, pivot) = self.active_camera_pose();
        let (_, view, _) = self.viewport().get_matrices(1.0, Some(pos), Some(rot), Some(pivot));
        let inv = view.inverse();
        let (right, up, eye) = (inv.x_axis.truncate(), inv.y_axis.truncate(), inv.w_axis.truncate());
        let pane_h = self.last_scene_view_rect.3.max(1.0);
        // World units a logical px covers on the pivot's plane: the
        // projection's vertical field of view is 0.9 rad.
        let per_px = 2.0 * (eye - pivot).length() * (0.45f32).tan() / pane_h;
        let by = (up * dy_px - right * dx_px) * per_px;
        if by.length_squared() == 0.0 || !by.is_finite() {
            return;
        }
        let active = self.active_camera.clone();
        let fmt3 = |v: Vec3| format!("{:.4}:{:.4}:{:.4}", v.x, v.y, v.z);
        let exact = self.pan_exact.take();
        let node = (active != "Default Camera")
            .then(|| self.camera_level_mut().children.iter_mut().find(|c| c.node_type == "camera" && c.name == active))
            .flatten();
        match node {
            Some(node) => {
                let text = |node: &FsNode, name: &str| node.params.iter().find(|p| p.name == name).map(|p| p.text().to_string());
                // From what the last pan wrote in full, while the node
                // still says it; from the node otherwise.
                let (from_pivot, from_pos) = match exact {
                    Some((name, p, e)) if name == active && text(node, "pivot") == Some(fmt3(p)) && text(node, "position") == Some(fmt3(e)) => (p, e),
                    _ => (pivot, pos),
                };
                let (to_pivot, to_pos) = (from_pivot + by, from_pos + by);
                for (name, v) in [("pivot", to_pivot), ("position", to_pos)] {
                    if let Some(p) = node.params.iter_mut().find(|p| p.name == name) {
                        p.set_text(fmt3(v));
                    }
                }
                self.pan_exact = Some((active, to_pivot, to_pos));
                self.sync_parameters_pane();
            }
            None => self.viewport_mut().pivot = pivot + by,
        }
        self.viewport_mut().reset_velocity();
        self.viewport_dirty = true;
    }

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

    /// Whether the network is drawn as an OVERLAY on the scene: shown, in the
    /// ordinary layout. It has no plate (since 2026-10-06; it was a switch),
    /// so its nodes stand on the scene and it spans the window.
    ///
    /// The circular pane and a detached network window have their own
    /// geometry and their own hit tests, and neither is a thing to overlay.
    pub fn network_overlay(&self) -> bool {
        self.show_network
            && !self.circular_network_pane
            && !self.is_detached_network
    }

    /// Whether the cursor is over a pane that FLOATS above the network —
    /// which, when the network spans the whole window, is the only thing
    /// keeping a node drawn under the params pane from stealing its clicks.
    pub fn over_floating_pane_at(&self, px: f32, py: f32) -> bool {
        self.params_claims(px, py)
            || [SPREADSHEET_IDX, PLAYBAR_IDX].iter().any(|&idx| {
                let (x, y, w, h) = self.positions[idx];
                w > 0.0 && h > 0.0 && px >= x && px < x + w && py >= y && py < y + h
            })
    }

    /// The part of the params HUD that is the HUD's: the band from its top
    /// to just under its last row — the ROWS are the HUD, and the scene
    /// below them is the viewport's, to orbit, scroll and number, as the
    /// network's overlay claims only its nodes. With the plate on this is
    /// the PLATE, fitted to the rows: as far under the last row as the
    /// first row stands under the top, so it is padded alike above and
    /// below; without it, `PARAMS_CLAIM_PAD` under. Rows overflowing the
    /// HUD fill it, and a HUD with no rows (nothing selected) claims
    /// nothing and draws no plate.
    pub fn params_claim(&self) -> (f32, f32, f32, f32) {
        let (x, y, w, h) = self.positions[PARAM_IDX];
        if w <= 0.0 || h <= 0.0 || self.pane_is_stubbed(PARAM_IDX) {
            return (x, y, w, h);
        }
        let rows: Vec<(f32, f32, f32, f32)> = self.param_row_rects().into_iter().filter(|r| r.3 > 0.0).collect();
        let Some(first) = rows.iter().map(|r| r.1).reduce(f32::min) else {
            // No rows: the plate is the small circle it collapses to, and
            // with no plate there is nothing at all.
            return if self.params_plate { self.params_dot_rect() } else { (x, y, w, 0.0) };
        };
        let last = rows.iter().map(|r| r.1 + r.3).fold(y, f32::max);
        let pad = if self.params_plate { (first - y).max(PARAMS_CLAIM_PAD) } else { PARAMS_CLAIM_PAD };
        (x, y, w, (last + pad).min(y + h) - y)
    }

    /// The circle the params plate collapses to while there are no rows to
    /// show: `PARAMS_DOT_D` wide, in the HUD's top right corner.
    pub fn params_dot_rect(&self) -> (f32, f32, f32, f32) {
        let (x, y, w, _) = self.positions[PARAM_IDX];
        let d = PARAMS_DOT_D.min(w.max(0.0));
        (x + w - d, y, d, d)
    }

    /// Whether the params HUD has rows to show.
    pub fn params_have_rows(&self) -> bool {
        self.param_row_rects().iter().any(|r| r.3 > 0.0)
    }

    /// Where the params plate is heading: `[x, y, w, h, round]`, the plate
    /// fitted to the rows (`round` 0) or, with none, the circle (`round`
    /// 1). `None` with the plate off or the HUD not shown.
    pub fn params_plate_target(&self) -> Option<[f32; 5]> {
        let (_, _, w, h) = self.positions[PARAM_IDX];
        if !self.params_plate || w <= 0.0 || h <= 0.0 || self.pane_is_stubbed(PARAM_IDX) || !self.slots.param.visible() {
            return None;
        }
        let (cx, cy, cw, ch) = self.params_claim();
        let round = if self.params_have_rows() { 0.0 } else { 1.0 };
        (cw > 0.0 && ch > 0.0).then_some([cx, cy, cw, ch, round])
    }

    /// Ease the drawn plate toward its target, as the drop glow eases:
    /// exponential, so frame-rate independent. True while it moves.
    pub fn animate_params_plate(&mut self, dt: f32) -> bool {
        let Some(target) = self.params_plate_target() else {
            return self.params_plate_shown.take().is_some();
        };
        let Some(shown) = self.params_plate_shown.as_mut() else {
            self.params_plate_shown = Some(target);
            return true;
        };
        let k = 1.0 - (-16.0 * dt.max(1e-4)).exp();
        let mut moving = false;
        for (s, t) in shown.iter_mut().zip(target) {
            *s += (t - *s) * k;
            if (t - *s).abs() > 0.3 {
                moving = true;
            }
        }
        if !moving {
            *shown = target;
        }
        moving
    }

    /// The params plate as it is drawn this frame, and its corner radii.
    pub fn params_plate_drawn(&self) -> Option<([f32; 5], f32)> {
        let shown = self.params_plate_shown.or_else(|| self.params_plate_target())?;
        let [_, _, w, h, round] = shown;
        let plate_r = cce_ui::layout::plate_corner_radius();
        let r = plate_r + ((w.min(h) * 0.5) - plate_r) * round.clamp(0.0, 1.0);
        Some((shown, r.max(0.0)))
    }

    /// Whether (px, py) is the params pane's: inside `params_claim` where
    /// no plate covers it, or on a dropdown it has open, which grows past
    /// its rows and is drawn over everything.
    pub fn params_claims(&self, px: f32, py: f32) -> bool {
        if !self.slots.param.visible() {
            return false;
        }
        let inside = |(x, y, w, h): (f32, f32, f32, f32)| w > 0.0 && h > 0.0 && px >= x && px < x + w && py >= y && py < y + h;
        (inside(self.params_claim()) && !self.plate_over_params_at(px, py))
            || self.slots.param.popover_rect().is_some_and(inside)
    }

    /// Whether (px, py) is under a plate drawn over the scene: a floating
    /// pane, or the network's while it has one.
    pub fn under_a_plate(&self, px: f32, py: f32) -> bool {
        if self.over_floating_pane_at(px, py) {
            return true;
        }
        if !self.show_network || self.is_detached_network {
            return false;
        }
        if self.circular_network_pane {
            return self.circular_network_layout.hit_test_content(px, py, 0.0, breadcrumb_h());
        }
        false
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

    /// Whether this wheel event over the plateless network is the graph's
    /// (it pans) or the scene's (the camera orbits): the graph's when a
    /// node was under the pointer as the GESTURE began, as a click is.
    /// Decided at the gesture's first event and held to its end — the lift,
    /// a pause of `OVERLAY_WHEEL_GAP`, or the pointer moving off where it
    /// was — since panning slides the node from under the pointer.
    pub(crate) fn overlay_wheel_to_graph(&mut self, phase: cce_ui::widget::scroll_motion::ScrollPhase) -> bool {
        let now = Instant::now();
        let at = (self.cursor_x, self.cursor_y);
        let held = self.overlay_wheel.filter(|w| {
            now.duration_since(w.last) < OVERLAY_WHEEL_GAP && (w.at.0 - at.0).abs() < 4.0 && (w.at.1 - at.1).abs() < 4.0
        });
        let graph = held.map_or_else(|| self.overlay_claims(at.0, at.1), |w| w.graph);
        self.overlay_wheel = if phase == cce_ui::widget::scroll_motion::ScrollPhase::FingerEnd {
            None
        } else {
            Some(OverlayWheel { graph, at, last: now })
        };
        graph
    }

    /// Whether (px, py) is inside the network's AREA — the region it is laid
    /// out over, whatever it has drawn there.
    ///
    /// Distinct from [`in_network_pane`](Self::in_network_pane), which in
    /// overlay mode narrows to the nodes so a click can reach the scene. The
    /// two differ only in overlay mode, and the difference is the point:
    /// a CLICK on empty space is not the network's, but a PAN gesture over
    /// that same space is — middle-drag and space+left mean nothing to the
    /// scene, and a graph you cannot pan by dragging because its own surface
    /// stopped being drawn would be a strange thing to ship.
    pub fn in_network_area(&self, px: f32, py: f32) -> bool {
        // A hidden network has no area: its rect is still laid out, and a
        // middle-drag panned the graph no one could see.
        if !self.show_network {
            return false;
        }
        if self.circular_network_pane {
            return self.circular_network_layout.hit_test_content(px, py, 0.0, breadcrumb_h());
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
            self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, 0.0, breadcrumb_h())
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
    /// Make `name` the active camera — on the app AND on the viewport
    /// widget, which keeps a copy of the name because its wheel handler
    /// routes by it: the Default Camera's orbit lands on the widget's own
    /// `rotation_x`/`rotation_y`, a camera NODE's accumulates into
    /// `pending_yaw`/`pending_pitch` for `tick_frame` to write onto the
    /// node. Until 2026-09-24 the widget's copy was written once, at
    /// construction, from whichever project `State::new` loaded — so after
    /// opening a project whose active camera differed (the startup default
    /// project pointer, Open, New, the viewport menu), the two disagreed:
    /// the widget parked every wheel into the pending pair, the drain saw
    /// the Default Camera active and threw it away, and trackpad scrolling
    /// in the viewport did nothing while a drag (which reads `State`'s
    /// copy) still orbited. Every site that changes the camera goes
    /// through here; nothing else writes either field.
    pub fn set_active_camera(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.viewport_mut().active_camera = name.clone();
        self.active_camera = name;
    }

    /// The level camera nodes stand on: the root, the object level
    /// (`context`). One level for every view, since 2026-10-02 — a camera
    /// was looked up on the CURRENT level until then, so diving into a
    /// subnet lost it, and with the geometry inside a Geometry node that
    /// would be every working view.
    pub fn camera_level(&self) -> &FsNode {
        &self.fs_root
    }

    pub fn camera_level_mut(&mut self) -> &mut FsNode {
        &mut self.fs_root
    }

    /// The cameras the scene offers, the Default Camera first: what the
    /// camera commands step through and the palette's camera rows list.
    pub fn camera_names(&self) -> Vec<String> {
        let mut names = vec!["Default Camera".to_string()];
        names.extend(
            self.camera_level()
                .children
                .iter()
                .filter(|c| c.node_type == "camera")
                .map(|c| c.name.clone()),
        );
        names
    }

    /// Look through the named camera, if this level has it. The one entry
    /// the camera commands, the palette's camera rows and the menubar's
    /// Camera menu share.
    pub fn choose_camera(&mut self, name: &str) -> bool {
        if !self.camera_names().iter().any(|n| n == name) {
            self.update_status_text(&format!("No camera named '{name}' here"));
            return false;
        }
        self.set_active_camera(name);
        // Re-checks the menubar's marks and the readouts keyed on the view.
        self.sync_nodes();
        self.update_status_text(&format!("Camera: {name}"));
        true
    }

    /// Step to the next camera (or the previous), wrapping.
    pub fn cycle_camera(&mut self, step: i32) -> bool {
        let names = self.camera_names();
        let at = names.iter().position(|n| *n == self.active_camera).unwrap_or(0) as i32;
        let next = (at + step).rem_euclid(names.len() as i32) as usize;
        self.choose_camera(&names[next])
    }

    /// Set every parameter of the node the params pane shows back to its
    /// template's default — the text AND whether it is an expression, so a
    /// default that is a reference is one again.
    pub fn reset_parameters(&mut self) -> bool {
        let Some(slot) = self.param_editor_selected() else {
            self.update_status_text("No node selected");
            return false;
        };
        let dir = self.param_editor_dir();
        let Some(node) = dir.children.get(slot) else { return false };
        // A subnet template's override for a child inside an instance wins,
        // as it does for the row menu's Default.
        let defaults: Vec<(String, String, bool)> = node
            .params
            .iter()
            .filter_map(|p| {
                self.template_default(dir, node, &p.name)
                    .map(|d| (p.name.clone(), d.text().to_string(), d.is_expr()))
            })
            .collect();
        let name = node.name.clone();
        if defaults.is_empty() {
            self.update_status_text(&format!("{name} has no template to reset to"));
            return false;
        }
        let node = &mut self.param_editor_dir_mut().children[slot];
        let was = node.params.clone();
        for (pname, text, is_expr) in defaults {
            if let Some(p) = node.params.iter_mut().find(|p| p.name == pname) {
                p.set_text(text);
                p.set_expr(is_expr);
            }
        }
        let before = crate::edit_history::ParamSnapshot {
            node_id: node.id.clone(),
            params: was
                .into_iter()
                .zip(node.params.iter())
                .filter(|(a, b)| !crate::edit_history::same(a, b))
                .map(|(a, _)| a)
                .collect(),
            what: "Reset Parameters".to_string(),
        };
        if !before.params.is_empty() {
            self.record_params(before, false);
        }
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
        self.update_status_text(&format!("{name}: parameters reset"));
        true
    }

    pub fn cursor_in_viewport(&self) -> bool {
        let (px, py) = (self.cursor_x, self.cursor_y);
        let inside = |(x, y, w, h): (f32, f32, f32, f32)| w > 0.0 && h > 0.0 && px >= x && px < x + w && py >= y && py < y + h;
        if self.network_overlay() {
            // The complement of the overlay: everything in the viewport the
            // network is not holding and no floating pane covers — the
            // second editor, docked, holds its rect. Bounded by the
            // viewport's rect; until 2026-10-06 it stopped at the old
            // column split (`splitter2_x`), so the scene under the params
            // HUD's rows, right of it, was nobody's.
            return inside(self.positions[VIEWPORT_IDX])
                && !self.in_network_pane()
                && !self.over_floating_pane()
                && !inside(self.positions[crate::slots::NETWORK_PANEL2_IDX]);
        }
        // Minus the floating panes here too. The spreadsheet and the playbar
        // sit INSIDE the centre column, over the full-bleed scene, and this
        // test used to count them as viewport — so a left press on a column
        // header armed the camera orbit and returned before any widget was
        // asked, and the spreadsheet's own header-click sort never fired.
        // Same for the right-click menu and the pinch zoom, which gate on
        // this test as well.
        //
        // Bounded by the viewport's OWN rect, minus the network plates by
        // their laid-out rects. Until 2026-09-25 this carved out the old
        // column layout instead — left of `splitter1_x + SPLITTER_W`, above
        // the network content's top — which the floating plates stopped
        // following long ago, so a band right of the network plate and a
        // strip along the top were neither the network's nor the scene's,
        // and a right-click there opened nothing. The circular pane is
        // excluded by its callers (`in_circle_network_pane`): its rect is
        // the circle's bounding box, whose corners are scene.
        let over_network = !self.circular_network_pane
            && [NETWORK_PANEL_IDX, crate::slots::NETWORK_PANEL2_IDX].iter().any(|&idx| inside(self.positions[idx]));
        inside(self.positions[VIEWPORT_IDX]) && !over_network && !self.over_floating_pane()
    }

    // --- Pane edge-resize hotspots. Each is the single source of truth for its zone:
    // the press handlers arm the matching `AppDrag` off it, and `pane_resize_cursor`
    // shows the resize cursor over it, so the two can't drift apart.

    /// The floating network pane's edge-resize hotspot at (cx, cy) — only the right
    /// edge resizes. `None` while the pane is circular or hidden.
    pub fn network_resize_edge_at(&self, cx: f32, cy: f32) -> Option<ResizeDirection> {
        // The overlay spans the window and has no edge to drag.
        if self.circular_network_pane || !self.show_network || self.network_overlay() {
            return None;
        }
        let (fx, fy, _, fh) = self.floating_network_layout;
        let fw = self.left_dock_width();
        let margin = 8.0_f32;
        let on_right = cx >= fx + fw - margin && cx <= fx + fw + margin && cy >= fy - margin && cy <= fy + fh + margin;
        if on_right {
            Some(ResizeDirection { left: false, right: true, top: false, bottom: false })
        } else {
            None
        }
    }

    /// Whether (cx, cy) is on the params HUD's left edge-resize hotspot:
    /// the edge as far down as the HUD claims (its rows, without its
    /// plate), and nowhere a plate covers it.
    pub fn on_param_resize_edge(&self, cx: f32, cy: f32) -> bool {
        // Collapsed to its circle, the HUD has no edge to drag.
        if !self.show_parameters || self.pane_is_stubbed(PARAM_IDX) || !self.params_have_rows() {
            return false;
        }
        let (x, y, w, h) = self.params_claim();
        let margin = 8.0_f32;
        w > 0.0
            && cx >= x - margin
            && cx <= x + margin
            && cy >= y - margin
            && cy <= y + h + margin
            && !self.plate_over_params_at(cx, cy)
    }

    /// Whether (cx, cy) is on the right dock's left edge-resize hotspot —
    /// the edge of whatever plate is docked there.
    pub fn on_right_dock_resize_edge(&self, cx: f32, cy: f32) -> bool {
        if self.circular_network_pane || !self.dock_shown(Dock::Right) {
            return false;
        }
        let pane = self.pane_in_dock(Dock::Right);
        if self.pane_is_stubbed(pane) {
            return false;
        }
        let (x, y, w, h) = self.positions[pane];
        let margin = 8.0_f32;
        w > 0.0 && cx >= x - margin && cx <= x + margin && cy >= y - margin && cy <= y + h + margin
    }

    /// The params HUD's rect: laid out from the VIEWPORT — its top right
    /// corner, a gap in, as wide as `params_hud_width` asks — and as tall as
    /// the viewport, except that it stops a gap above the spreadsheet or the
    /// playbar when one lies below it (since 2026-10-06): rows under a plate
    /// along the bottom could be neither seen nor reached, and with the HUD
    /// stopped short the pane scrolls them instead. A plate beside or over
    /// the HUD's top (the right dock's) sizes nothing; it is drawn over the
    /// HUD (`plates_over_params`).
    pub fn params_hud_rect(&self) -> (f32, f32, f32, f32) {
        let gap = 18.0_f32;
        let (vx, vy, vw, vh) = self.positions[VIEWPORT_IDX];
        let (vx, vy, vw, vh) = if vw > 0.0 && vh > 0.0 { (vx, vy, vw, vh) } else { (0.0, 0.0, self.width, self.height - STATUS_H) };
        let w = self.params_hud_width.clamp(PARAMS_HUD_MIN_W, (vw - 2.0 * gap).max(PARAMS_HUD_MIN_W));
        let (x, top) = (vx + vw - gap - w, vy + gap);
        let mut bottom = vy + vh - gap;
        for idx in [SPREADSHEET_IDX, PLAYBAR_IDX] {
            let (px, py, pw, ph) = self.positions[idx];
            let below = pw > 0.0 && ph > 0.0 && self.slots.get_dyn(idx).visible() && px < x + w && px + pw > x && py > top + PARAMS_DOT_D;
            if below {
                bottom = bottom.min(py - gap);
            }
        }
        (x, top, w, (bottom - top).max(PARAMS_DOT_D))
    }

    /// The plates drawn over the params HUD, as rects: the network's
    /// (while it has one), the second editor's, the spreadsheet's, the
    /// playbar's — collapsed and detached stubs included, since those are
    /// the slots' rects too. The HUD is UNDER all of them: what it shows,
    /// and what it takes of the pointer, is what they leave.
    pub fn plates_over_params(&self) -> Vec<(f32, f32, f32, f32)> {
        let mut out = Vec::new();
        let mut take = |idx: usize| {
            let r = self.positions[idx];
            if r.2 > 0.0 && r.3 > 0.0 && self.slots.get_dyn(idx).visible() {
                out.push(r);
            }
        };
        take(SPREADSHEET_IDX);
        take(PLAYBAR_IDX);
        if self.circular_network_pane && self.show_network && !self.detached_circular_network {
            let l = &self.circular_network_layout;
            out.push((l.x - l.r, l.y - l.r, 2.0 * l.r, 2.0 * l.r));
        }
        out
    }

    /// Whether a plate covers the HUD at (px, py).
    pub fn plate_over_params_at(&self, px: f32, py: f32) -> bool {
        self.plates_over_params()
            .iter()
            .any(|&(x, y, w, h)| px >= x && px < x + w && py >= y && py < y + h)
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

    /// The dock a split would move a tab to: the first EMPTY one, Left,
    /// Right, Bottom. There are four tab candidates for three docks, so two
    /// docks can each hold two with none left free — the corner menu reads
    /// this to leave Move To Own Plate out rather than offer a dead row.
    pub fn first_empty_dock(&self) -> Option<Dock> {
        [Dock::Left, Dock::Right, Dock::Bottom]
            .into_iter()
            .find(|&d| self.dock_tabs[d as usize].is_empty())
    }

    /// Move `slot` out of its shared dock to the first EMPTY dock — the
    /// corner menu's inverse of Add Tab. No empty dock, no move.
    pub fn split_dock_tab(&mut self, slot: usize) {
        if self.tab_dock_of_pane(slot).is_none() {
            return;
        }
        let Some(empty) = self.first_empty_dock() else { return };
        self.add_dock_tab(empty, slot);
    }

    pub fn floating_spreadsheet_rect(&self) -> (f32, f32, f32, f32) {
        let gap = 18.0_f32;
        let fx = gap;
        let fw = self.left_dock_width();
        let param_w = self.right_dock_width();
        let param_x = self.width - gap - param_w;
        let flush_left = if self.dock_shown(Dock::Left) { fx + fw + gap } else { gap };
        let flush_right = if self.dock_shown(Dock::Right) { param_x - gap } else { self.width - gap };
        let ss_x = (flush_left - self.floating_spreadsheet_inset_left.max(0.0)).max(gap);
        let ss_end = (flush_right + self.floating_spreadsheet_inset_right.max(0.0)).min(self.width - gap);
        let ss_w = (ss_end - ss_x).max(150.0);
        // The playbar is attached to the bottom edge: what stands above it
        // stops a gap short of its top, the gap the bottom edge gives it.
        let pb_off = if self.show_playbar { playbar_shelf_h() } else { 0.0 };
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

    /// The left dock's width as drawn: `floating_network_layout.2` (the
    /// width asked for) fitted to this window. Read this, not the field,
    /// wherever the plate's ON-SCREEN width matters.
    pub fn left_dock_width(&self) -> f32 {
        let gap = 18.0_f32;
        self.floating_network_layout.2.clamp(150.0, (self.width - 2.0 * gap).max(150.0))
    }

    /// The right dock's width as drawn — `floating_param_width` fitted to
    /// this window, as [`Self::left_dock_width`] is for the left.
    pub fn right_dock_width(&self) -> f32 {
        let gap = 18.0_f32;
        self.floating_param_width.clamp(150.0, (self.width - 2.0 * gap).max(150.0))
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
        if self.dock_shown(Dock::Right) && in_v && cx >= ss_x + ss_w - margin && cx <= ss_x + ss_w + margin {
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
                AppDrag::RightDockResize { .. } | AppDrag::HudResize { .. } => CursorIcon::EwResize,
                AppDrag::SpreadsheetResize { .. } => CursorIcon::NsResize,
                AppDrag::SpreadsheetResizeLeft { .. } | AppDrag::SpreadsheetResizeRight { .. } => {
                    CursorIcon::EwResize
                }
            });
        }
        if let Some(dir) = self.network_resize_edge_at(cx, cy) {
            return Some(dir_cursor(dir));
        }
        if self.on_right_dock_resize_edge(cx, cy) || self.on_param_resize_edge(cx, cy) {
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

    /// The name a new node gets: the template's name and the lowest free
    /// index run together — `Sphere1`, not `Sphere 1`. A node's name is a
    /// segment of its path, and a path with spaces in it is a path you have
    /// to quote everywhere it goes (see [`sanitize_node_name`]).
    pub fn get_lowest_unused_name(&self, base_name: &str) -> String {
        let dir = self.current_dir();
        let base = sanitize_node_name(base_name);
        let mut index = 1;
        loop {
            let candidate = format!("{}{}", base, index);
            if !dir.children.iter().any(|c| c.name == candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    /// The node a params-pane load would show right now — see
    /// [`ParamPaneTarget`]. None when nothing is selected.
    pub(crate) fn param_pane_target(&self) -> Option<ParamPaneTarget> {
        let editor = self.params_editor();
        let slot = self.param_editor_selected()?;
        let node = self.param_editor_dir().children.get(slot)?;
        let path = if editor == crate::slots::CONTENT2_IDX { &self.current_path2 } else { &self.current_path };
        Some(ParamPaneTarget { editor, path: path.clone(), slot, id: node.id.clone() })
    }

    /// Write the params pane's rows back into the node they were loaded from.
    ///
    /// Rows are matched to params by NAME, so rows from one node written into
    /// another land wherever the two share a name — and every node has an
    /// `Input`. The pane is not reloaded the instant the selection moves:
    /// `sync_layout` reaches here through `sync_pane_focus` before the
    /// post-event pass gets to `sync_parameters_pane`, so a cursor step from
    /// output1 onto detangle1 wrote output1's `Input` into detangle1, and the
    /// next step carried detangle1's `Input` and `Iterations` into relax1
    /// (2026-09-25, a simnet rewired by pressing k twice). Hence the guard:
    /// rows that were not loaded from the selected node are stale, and
    /// writing nothing is the only right thing to do with them.
    pub fn sync_parameters_to_project(&mut self) {
        // The HUD showing the visualizers writes back to them, not a node.
        if self.vis_hud.is_some() {
            self.sync_visualizer_hud_back();
            return;
        }
        let mut file_to_open = None;
        if !self.is_detached_network && self.param_pane_source == self.param_pane_target() {
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
                    // Rows whose new text does not fit the parameter's kind
                    // (`abc` typed into a threshold): (display key, the text
                    // kept, why). Nothing is written; the row shows the kept
                    // text again and the status line says why.
                    let mut rejected: Vec<(String, String, String)> = Vec::new();
                    let mut pane_actions = Vec::new();
                    // What each changed parameter was, for undo.
                    let mut was: Vec<ParamDef> = Vec::new();
                    for (u_name, u_val, _) in &updated_params {
                        // The params pane reports its display key (label when
                        // set, else name), so resolve back to the param by that
                        // key rather than by name alone.
                        if let Some(p) = child.params.iter_mut().find(|p| {
                            let key = p.shown_name();
                            key == u_name
                        }) {
                            // A value row presents a single number spread
                            // over its components; read back unchanged, it
                            // is the text it was, not an edit.
                            let presented = p.name == "value" && !p.is_expr() && same_value_row_text(p.text(), u_val);
                            if p.text() != *u_val && !presented {
                                // A reference typed into a plain row becomes
                                // an expression — the one way to make one
                                // without the row menu — and an expression is
                                // checked when it evaluates. Anything else
                                // must fit the kind.
                                let as_expr = p.is_expr()
                                    || (p.takes_expressions() && crate::expr::looks_like_expression(u_val));
                                if !as_expr {
                                    if let Err(why) = p.check(u_val) {
                                        rejected.push((u_name.clone(), p.text().to_string(), format!("{}: {why}", p.shown_name())));
                                        continue;
                                    }
                                }
                                was.push(p.clone());
                                p.set_text(u_val.clone());
                                param_changed = true;
                                if as_expr {
                                    p.set_expr(true);
                                }
                                if p.kind() == ParamKind::Button && p.text() == "clicked" {
                                    triggered_buttons.push((p.name.clone(), p.shown_name().to_string()));
                                    p.set_text("".to_string());
                                }
                                if p.kind() == ParamKind::Toggle {
                                    let desired = p.text() == "true";
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
                                if p.name == "Open" && p.text() != "- Select -" && !p.text().is_empty() {
                                    file_to_open = Some(p.text().to_string());
                                    p.set_text("- Select -".to_string());
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

                    // A page's Preset and Units set its Width
                    // and Height; what they overwrite is part of the step.
                    let mut followed = false;
                    if child.node_type == "page" {
                        let setters: Vec<ParamDef> = was.iter().filter(|w| matches!(w.name.as_str(), "preset" | "units")).cloned().collect();
                        for w in setters {
                            for r in crate::page::follow_page_rows(child, &w) {
                                followed = true;
                                if !was.iter().any(|x| x.name == r.name) {
                                    was.push(r);
                                }
                            }
                        }
                    }

                    // One step per gesture: a drag writes back on every
                    // motion. A button or the Open dropdown ends as it
                    // began, and is no edit.
                    was.retain(|w| {
                        child.params.iter().find(|p| p.name == w.name).is_some_and(|p| !crate::edit_history::same(p, w))
                    });
                    let edit = (!was.is_empty()).then(|| crate::edit_history::ParamSnapshot {
                        node_id: child.id.clone(),
                        what: was.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", "),
                        params: was,
                    });
                    if let Some(edit) = edit {
                        self.record_params(edit, true);
                    }

                    if !triggered_buttons.is_empty() || !display_resets.is_empty() || !rejected.is_empty() {
                        let mut disp_params = self.param().node_params();
                        for btn_name in triggered_buttons.iter().map(|(_, key)| key).chain(display_resets.iter()) {
                            if let Some(pos) = disp_params.iter().position(|p| p.0 == *btn_name) {
                                disp_params[pos].1 = if btn_name == "Open" { "- Select -".to_string() } else { "".to_string() };
                            }
                        }
                        for (key, kept, _) in &rejected {
                            if let Some(pos) = disp_params.iter().position(|p| p.0 == *key) {
                                disp_params[pos].1 = kept.clone();
                            }
                        }
                        self.param_mut().set_display_params(&disp_params);
                    }
                    if let Some((_, _, why)) = rejected.first() {
                        self.update_status_text(&format!("Not applied — {why}"));
                    }

                    if param_changed {
                        self.sync_grid_settings();
                        self.rebuild_scene_geometry();
                        self.sync_nodes();
                        // Width and Height moved under the pane's feet.
                        if followed {
                            self.sync_parameters_pane();
                        }

                        for (btn_name, _) in triggered_buttons {
                            self.run_param_button(&btn_name);
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
        let file = crate::geometry::node_param_str(&node, "file", "");
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

    /// A button row of the params pane, pressed. By the parameter's NAME:
    /// the pane reports its label, and dispatching that — as this did until
    /// the names became identifiers — sent `export` to `execute_menu_action`,
    /// which knows no such label, and the Export button did nothing. A
    /// button carries no node, but the pressed one can only be on the node
    /// the pane is showing, so the selection is the node.
    pub fn run_param_button(&mut self, name: &str) {
        match name {
            "export" => self.run_export(),
            "detect_range" => self.detect_remap_range(),
            _ => {
                self.execute_menu_action(name);
            }
        }
    }

    /// The Attribute node's Detect Range: its input measured once, at the
    /// current frame and as the scene shows it, and the lowest and highest
    /// value of its Name over its Group written into From. An undoable
    /// edit; From then stays as written, where From Range Auto follows the
    /// input.
    pub fn detect_remap_range(&mut self) {
        let Some(slot) = self.param_editor_selected() else { return };
        let Some(node) = self.param_editor_dir().children.get(slot).filter(|c| c.node_type == "attribute") else {
            return;
        };
        let node = node.clone();
        let (frame, start) = (self.sim_frame(), self.sim_start_frame());
        let mut err = None;
        // Name and Group may be expressions; read them as they evaluate.
        let resolved = crate::geometry::resolve_param_refs(&self.fs_root, &node, frame, &mut err).unwrap_or_else(|| node.clone());
        let Some(input) = crate::geometry::param_node(&self.fs_root, &node, "input") else {
            self.update_status_text(&format!("{}: Detect Range needs an input", node.name));
            return;
        };
        let mut sim_cache = std::mem::take(&mut self.sim_cache);
        let geom = {
            let mut sim = crate::geometry::EvalSim::new(frame, start, &mut sim_cache);
            crate::geometry::node_geometry_as_shown(&self.fs_root, input, &mut err, &mut sim)
        };
        self.sim_cache = sim_cache;
        let Some(geom) = geom else {
            self.update_status_text(&format!("{}: Detect Range: the input has no geometry", node.name));
            return;
        };
        let [lo, hi] = match crate::geometry::remap_input_range(&geom, &resolved) {
            Ok(r) => r,
            Err(why) => {
                self.update_status_text(&format!("{}: Detect Range: {why}", node.name));
                return;
            }
        };
        let dir = self.param_editor_dir_mut();
        let Some(p) = dir.children[slot].params.iter_mut().find(|p| p.name == "from") else { return };
        let was = p.clone();
        p.set_value(ParamValue::Vec2([lo, hi]));
        p.set_expr(false);
        let text = p.text().to_string();
        if !crate::edit_history::same(&was, p) {
            let before = crate::edit_history::ParamSnapshot {
                node_id: node.id.clone(),
                params: vec![was],
                what: "Detect Range".to_string(),
            };
            self.record_params(before, false);
        }
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
        self.update_status_text(&format!("{}: From set to {}", node.name, text.replace(':', " .. ")));
    }

    pub fn execute_menu_action(&mut self, label: &str) -> bool {
        match label {
            // An Export node's button, reached by its label from MCP's
            // `menu_action`; the pane's press goes through `run_param_button`.
            "Export" => {
                self.run_export();
            }
            "Set As Default" => {
                self.set_current_as_default();
            }
            "Next Camera" => {
                self.cycle_camera(1);
            }
            "Previous Camera" => {
                self.cycle_camera(-1);
            }
            "Default Camera" => {
                self.choose_camera("Default Camera");
            }
            "Reset Parameters" => {
                self.reset_parameters();
            }
            "Group Markers" => self.open_group_markers_dialog(),
            "Attribute Visualizers" => self.open_visualizers_hud(),
            "Rename Node" => match self.selected_slots().first().copied() {
                Some(slot) => self.open_rename_dialog(slot),
                None => self.update_status_text("Select a node to rename."),
            },
            "New Project" | "New" => {
                self.new_project();
                self.update_status_text("New project");
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
            "Add Node" => {
                self.open_node_palette();
            }
            "Zoom In" => {
                self.zoom(1.15, None);
            }
            "Zoom Out" => {
                self.zoom(1.0 / 1.15, None);
            }
            "Reset Zoom" => {
                self.set_grid_geometry(configured_grid_geometry());
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

    pub fn sync_parameters_pane(&mut self) {
        // The attribute visualizers, when the HUD shows them — until another
        // node is picked, which takes the HUD back.
        if self.vis_hud.is_some() && self.param_pane_target() != self.vis_hud_from {
            self.vis_hud = None;
        }
        if self.vis_hud.is_some() && !self.is_detached_network {
            let rows = param_display(&self.visualizer_hud_params());
            self.param_mut().set_display_params(&rows);
            self.param_pane_source = None;
            return;
        }
        // Selection reads through the param-editor accessors: whichever
        // network editor took the last node click feeds the pane, at ITS
        // level — no matter the tab.
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
        let params = self.present_float2_rows(params);
        self.param_mut().set_display_params(&params);
        self.param_pane_source = if self.is_detached_network { None } else { self.param_pane_target() };
    }

    /// The shown node's `float2` rows over the span each already has:
    /// `param_display` chooses one from the value alone, and this keeps the
    /// span in use while the value stays inside it, and outright through a
    /// drag in the pane — a new span is a new row type, which rebuilds the
    /// pane and would drop the slider being held. The Value row's rule
    /// ([`value_row_span`]), kept per parameter.
    fn present_float2_rows(&mut self, mut params: Vec<(String, String, String)>) -> Vec<(String, String, String)> {
        let rows: Vec<(String, String, [f32; 2])> = {
            if self.is_detached_network {
                return params;
            }
            let Some(slot) = self.param_editor_selected() else { return params };
            let Some(node) = self.param_editor_dir().children.get(slot) else { return params };
            let rows = node
                .params
                .iter()
                .filter(|p| !p.is_expr())
                .filter_map(|p| match p.value() {
                    Some(ParamValue::Vec2(v)) => Some((p.shown_name().to_string(), p.name.clone(), *v)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if rows.is_empty() {
                return params;
            }
            let id = node.id.clone();
            if self.float2_spans.as_ref().is_none_or(|(n, _)| *n != id) {
                self.float2_spans = Some((id, Vec::new()));
            }
            rows
        };
        let held = self.drag_widget == Some(crate::slots::PARAM_IDX);
        let Some((_, spans)) = self.float2_spans.as_mut() else { return params };
        for (key, name, v) in rows {
            let Some(row) = params.iter_mut().find(|r| r.0 == key && r.2.starts_with("float2")) else { continue };
            let kept = spans.iter().find(|(n, _)| *n == name).map(|(_, r)| *r);
            let span = match (held, kept) {
                (true, Some(r)) => r,
                _ => value_row_span(&v, kept),
            };
            row.2 = float2_row(span);
            spans.retain(|(n, _)| *n != name);
            spans.push((name, span));
        }
        params
    }

    /// Upgrade the selected node's `attribute`- and `group`-kind text rows
    /// to `textpick` rows carrying the candidates read off the node's INPUT
    /// geometry (the Houdini attribute/group chooser). Rows stay plain text
    /// when there is no input, evaluation fails, or the list is empty — the
    /// picker degrades to nothing rather than an empty menu.
    ///
    /// By KIND, not by name: until 2026-09-28 this knew four rows on three
    /// node types by their names (Attribute's Attribute Name and Group, the
    /// Group node's Group Name, Relax's Pin Group), and the other thirty-odd
    /// rows that name an attribute or a group — every Visualize, Cull,
    /// Neighbour and Wrangle Group, every operator's output Attribute — were
    /// text boxes you typed into blind. The template says what a row names
    /// now (`ParamKind::Attribute` / `Group`), so a row gets the picker by
    /// declaring it, and a new template needs no entry here.
    ///
    /// The same pass PRESENTS the Attribute node's `Value` as a float3 row
    /// when its target is three wide (since 2026-09-28). The parameter
    /// itself stays `text`, because its width is the target's — the Type
    /// row under Create, the named attribute under Modify — which no fixed
    /// kind can say (see "Parameter kinds" in CLAUDE.md); what changes is
    /// the control, as the pickers change it. Three wide means Create with
    /// Type Float3, or Modify aimed at Pos, Col or an input attribute the
    /// evaluated input holds as a Float3. The text must already hold three
    /// numbers: a single number BROADCASTS to every component, and a row
    /// that showed it as `(n, 0, 0)` would write that triple back on the
    /// first drag; an expression is shown as its text like any other. The
    /// row's range adapts to the value ([`value_row_span`]) and is soft.
    /// Since 2026-10-01 every width is presented, a single number spread
    /// over the components ([`value_row_control`]).
    fn add_pick_lists(
        &mut self,
        mut params: Vec<(String, String, String)>,
    ) -> Vec<(String, String, String)> {
        // Rows are keyed as `param_display` keys them — by label when
        // there is one, by name otherwise — so the kind is looked up the
        // same way the pane's write-back resolves a row.
        let (node_id, kinds, value_row) = {
            if self.is_detached_network {
                return params;
            }
            let Some(slot) = self.param_editor_selected() else { return params };
            let dir = self.param_editor_dir();
            let Some(node) = dir.children.get(slot) else { return params };
            let kinds: Vec<(String, ParamKind)> = node
                .params
                .iter()
                .filter(|p| matches!(p.kind(), ParamKind::Attribute | ParamKind::Group))
                .map(|p| (p.shown_name().to_string(), p.kind()))
                .collect();
            let value_row = Self::attribute_value_target(node);
            if kinds.is_empty() && value_row.is_none() {
                return params;
            }
            (node.id.clone(), kinds, value_row)
        };
        let lists = self.input_pick_lists(&node_id);
        for row in params.iter_mut() {
            let Some((_, kind)) = kinds.iter().find(|(key, _)| *key == row.0) else { continue };
            let list = match kind {
                ParamKind::Attribute => &lists.attrs,
                _ => &lists.groups,
            };
            if row.2 == "text" && !list.is_empty() {
                row.2 = format!("textpick:{}", list.join(","));
            }
        }
        if let Some((key, target, ball)) = value_row {
            let width = match target {
                ValueTarget::Width(w) => w,
                ValueTarget::Named(name) => lists
                    .widths
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(&name))
                    .map_or(0, |(_, w)| *w),
            };
            // The span the row already has stands through a drag: a new
            // one rebuilds the pane, which would drop the slider being held.
            let kept = self.value_row_span.as_ref().filter(|(id, _)| *id == node_id).map(|(_, r)| *r);
            let held = self.drag_widget == Some(crate::slots::PARAM_IDX);
            if let Some(row) = params.iter_mut().find(|r| r.0 == key && r.2 == "text") {
                if let Some((ty, text, r)) = value_row_control(&row.1, width, ball, kept, held) {
                    row.2 = ty;
                    row.1 = text;
                    self.value_row_span = Some((node_id.clone(), r));
                }
            }
        }
        params
    }

    /// What an Attribute node's `Value` is aimed at — its display key and
    /// the target's width, or the attribute name the width has to be read
    /// off the input for — when the row is one the float3 presentation can
    /// take: a plain (non-expression) text holding three numbers. `None`
    /// for any other node, operation, or text.
    ///
    /// The third member is whether the row shows the TRACKBALL: the
    /// parameter's own choice (`ParamDef::view`), else on for a vector —
    /// a displacement of Pos, a Float3 attribute — and off for Col, whose
    /// three numbers are a colour and point nowhere.
    fn attribute_value_target(node: &FsNode) -> Option<(String, ValueTarget, bool)> {
        if !node.node_type.eq_ignore_ascii_case("attribute") {
            return None;
        }
        let p = node.params.iter().find(|p| p.name == "value")?;
        if p.is_expr() {
            return None;
        }
        // Read from an attribute, the row is not shown at all.
        if node_param_str(node, "value_from", "Constant").eq_ignore_ascii_case("attribute") {
            return None;
        }
        let comps = p
            .text()
            .split(|c| c == ':' || c == ',' || c == ' ')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<f32>().ok())
            .count();
        let raw = p.text().split(|c| c == ':' || c == ',' || c == ' ').filter(|s| !s.is_empty()).count();
        if comps == 0 || comps > 4 || raw != comps {
            return None;
        }
        let key = p.shown_name().to_string();
        let name = node_param_str(node, "attribute_name", "");
        let name = name.trim().to_string();
        let ball = p.wants_trackball(!name.eq_ignore_ascii_case("Col"));
        let target = match node_param_str(node, "operation", "Create").to_lowercase().as_str() {
            "create" => ValueTarget::Width(match node_param_str(node, "type", "Float").to_lowercase().as_str() {
                "float3" => 3,
                "float2" => 2,
                "float4" => 4,
                _ => 1,
            }),
            "modify" if name.eq_ignore_ascii_case("Pos") || name.eq_ignore_ascii_case("Col") => ValueTarget::Width(3),
            "modify" => ValueTarget::Named(name),
            _ => return None,
        };
        Some((key, target, ball))
    }

    /// The (groups, attributes) present on the evaluated geometry of node
    /// `node_id`'s Input, cached on (input node id, geometry version) —
    /// both empty when the Input is unconnected or names nothing. The input
    /// resolves sibling-first like the wire itself (`geometry::param_node`);
    /// a whole-tree search by name offered the groups of a same-named node in
    /// some other subnet. Attribute names get the Pos/Col built-ins appended
    /// (the Attribute node can Modify them); names carrying a comma are
    /// dropped — they cannot ride the type spec-string.
    /// Beside the names, each point attribute's WIDTH in components, which
    /// is what decides the control the Attribute node's Value row gets.
    fn input_pick_lists(&mut self, node_id: &str) -> PickLists {
        let input_id = crate::viewer_state::find_node_by_id(&self.fs_root, node_id)
            .and_then(|n| crate::geometry::param_node(&self.fs_root, n, "input"))
            .map(|n| n.id.clone());
        let Some(input_id) = input_id else { return PickLists::default() };
        let key = (input_id.clone(), self.rt_geometry_version);
        if let Some((k, lists)) = &self.pick_cache {
            if *k == key {
                return lists.clone();
            }
        }
        let (frame, start) = (self.sim_frame(), self.sim_start_frame());
        let mut groups = std::collections::BTreeSet::new();
        let mut attrs = std::collections::BTreeSet::new();
        let mut widths: Vec<(String, usize)> = Vec::new();
        let mut sim_cache = std::mem::take(&mut self.sim_cache);
        {
            let mut sim = crate::geometry::EvalSim::new(frame, start, &mut sim_cache);
            if let Some(input_node) = crate::viewer_state::find_node_by_id(&self.fs_root, &input_id) {
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
                        let width = match geom.points().get(a) {
                            Some(crate::detail::AttribData::Float2(_)) => 2,
                            Some(crate::detail::AttribData::Float3(_)) => 3,
                            Some(crate::detail::AttribData::Float4(_)) => 4,
                            _ => 1,
                        };
                        widths.push((a.to_string(), width));
                    }
                }
            }
        }
        self.sim_cache = sim_cache;
        let mut attrs: Vec<String> = attrs.into_iter().collect();
        attrs.push("Pos".to_string());
        attrs.push("Col".to_string());
        let lists = PickLists { groups: groups.into_iter().collect(), attrs, widths };
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
    /// Show/Hide Geometry, and Delete.
    /// The rows of a node's right-click menu and the action each runs.
    /// Split from the open so a test reads them.
    pub(crate) fn node_menu_rows(&self, slot: usize) -> (Vec<String>, Vec<NodeMenuAction>) {
        let bypassed = self.current_dir().children.get(slot).is_some_and(|n| n.bypassed);
        let (is_utility, geom_visible, enterable, curve_editing) = {
            let dir = self.current_dir();
            let Some(node) = dir.children.get(slot) else { return (Vec::new(), Vec::new()) };
            let enterable = node.is_enterable();
            // None: no viewer state for this node type; Some(bool): editable,
            // and whether it is being edited right now. The types that have a
            // state are whatever `source_for` accepts, so a new HandleSource
            // appears in this menu without touching it.
            let curve_editing = crate::viewer_state::source_for(&node.node_type).map(|_| {
                self.viewer_tool.as_ref().map(|t| t.node_id == node.id).unwrap_or(false)
            });
            (
                false,
                node.geometry_visible,
                enterable,
                curve_editing,
            )
        };
        let deletable = {
            let dir = self.current_dir();
            dir.children
                .get(slot)
                .map(|_| true)
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
            options.push(if bypassed { "Stop Bypassing" } else { "Bypass" }.to_string());
            actions.push(NodeMenuAction::ToggleBypass);
        }
        if let Some(editing) = curve_editing {
            // "Handles", not "Points": a soft transform's are a centre and a
            // tip, and only a curve's are points.
            options.push(if editing { "Stop Editing Handles" } else { "Edit Handles" }.to_string());
            actions.push(NodeMenuAction::EditCurve);
        }
        if deletable {
            options.push("Rename".to_string());
            actions.push(NodeMenuAction::Rename);
            options.push("Delete".to_string());
            actions.push(NodeMenuAction::Delete);
        }

        (options, actions)
    }

    fn open_node_context_menu(&mut self, slot: usize) {
        self.open_node_context_menu_at(slot, None);
    }

    /// A node's menu at the pointer, or with its top-left at `at`.
    pub(crate) fn open_node_context_menu_at(&mut self, slot: usize, at: Option<(f32, f32)>) {
        let (options, actions) = self.node_menu_rows(slot);
        if options.is_empty() {
            return;
        }
        let target = self.slots.get_dyn(CONTENT_IDX).base().id();
        self.put_up_menu(at, None, options, 0, target);
        crate::menu_page::mark_page_rows(&actions, |a| a == NodeMenuAction::Rename);
        self.node_menu_slot = Some(slot);
        self.node_menu_actions = actions;
    }

    fn node_menu_open(&self) -> bool {
        cce_ui::widget::context_menu::is_visible() && self.node_menu_slot.is_some()
    }

    pub(crate) fn close_node_menu(&mut self) {
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
            let idx = cce_ui::widget::context_menu::row_at(self.cursor_x, self.cursor_y);
            let picked = self.node_menu_slot.zip(idx.and_then(|i| self.node_menu_actions.get(i).copied()));
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
        // The image the level shows is part of the scene, so it is part of
        // what Frame All holds: its four corners, beside the geometry.
        let image = self.image_world_corners().map(|c| c.map(Vec3::from_array));
        let points = || {
            self.rt_sphere_verts
                .iter()
                .map(|v| Vec3::from_array(v.position))
                .chain(image.into_iter().flatten())
        };
        if points().next().is_none() {
            return;
        }
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for p in points() {
            min = min.min(p);
            max = max.max(p);
        }
        let center = (min + max) * 0.5;
        let mut radius = 0.0f32;
        for p in points() {
            radius = radius.max((p - center).length());
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

        // A camera node stands at the root and applies from every level;
        // a named camera that is not there is the Default Camera view, and
        // is framed as one.
        let camera_name = self.active_camera.clone();
        let mut framed_node = false;
        if self.active_camera != "Default Camera" {
            let dir = self.camera_level_mut();
            if let Some(node) = dir.children.iter_mut().find(|c| c.node_type == "camera" && c.name == camera_name) {
                framed_node = true;
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
                let pos = node.params.iter().find(|p| p.name == "position")
                    .and_then(|p| parse3(p.text()))
                    .unwrap_or(Vec3::new(2.5, 1.8, 2.5));
                let piv = node.params.iter().find(|p| p.name == "pivot")
                    .and_then(|p| parse3(p.text()))
                    .unwrap_or(Vec3::ZERO);
                let offset = pos - piv;
                let dir_unit = if offset.length() > 1e-4 {
                    offset.normalize()
                } else {
                    Vec3::new(2.5, 1.8, 2.5).normalize()
                };
                let new_pos = center + dir_unit * dist;
                let fmt3 = |v: Vec3| format!("{:.2}:{:.2}:{:.2}", v.x, v.y, v.z);
                if let Some(p) = node.params.iter_mut().find(|p| p.name == "pivot") {
                    p.set_text(fmt3(center));
                }
                if let Some(p) = node.params.iter_mut().find(|p| p.name == "position") {
                    p.set_text(fmt3(new_pos));
                }
                self.viewport_mut().zoom = 1.0;
                self.viewport_mut().reset_velocity();
            }
        }
        if !framed_node {
            // The Default Camera view: its pivot moves to the geometry's
            // centre and the fixed eye ray is fitted with zoom, so the
            // geometry is centred AND sized — the pivot used to be pinned to
            // the origin, which framed off-centre geometry out of the pane.
            let base_len = Vec3::new(2.5, 1.8, 2.5).length();
            let vp = self.viewport_mut();
            vp.pivot = center;
            vp.zoom = (dist / base_len).clamp(0.05, crate::viewport_3d::Viewport3D::MAX_ZOOM);
            vp.reset_velocity();
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
        // As in `frame_all`: a named camera that is not there is the
        // Default Camera view, and is fitted as one.
        let camera_name = self.active_camera.clone();
        let mut fitted_node = false;
        if self.active_camera != "Default Camera" {
            let dir = self.camera_level_mut();
            if let Some(node) = dir.children.iter_mut().find(|c| c.node_type == "camera" && c.name == camera_name) {
                fitted_node = true;
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
                let pos = node.params.iter().find(|p| p.name == "position")
                    .and_then(|p| parse3(p.text()))
                    .unwrap_or(Vec3::new(2.5, 1.8, 2.5));
                let piv = node.params.iter().find(|p| p.name == "pivot")
                    .and_then(|p| parse3(p.text()))
                    .unwrap_or(Vec3::ZERO);
                let offset = pos - piv;
                let dir_unit = if offset.length() > 1e-4 { offset.normalize() } else { Vec3::new(2.5, 1.8, 2.5).normalize() };
                let new_pos = piv + dir_unit * dist;
                if let Some(p) = node.params.iter_mut().find(|p| p.name == "position") {
                    p.set_text(format!("{:.3}:{:.3}:{:.3}", new_pos.x, new_pos.y, new_pos.z));
                }
                self.viewport_mut().zoom = 1.0;
                self.viewport_mut().reset_velocity();
            }
        }
        if !fitted_node {
            let base_len = Vec3::new(2.5, 1.8, 2.5).length();
            self.viewport_mut().zoom = (dist / base_len).clamp(0.05, crate::viewport_3d::Viewport3D::MAX_ZOOM);
            self.viewport_mut().reset_velocity();
        }
        self.viewport_dirty = true;
        self.sync_parameters_pane();
    }

    /// The parameter row under a window point in the params pane: the slot
    /// the pane shows and the parameter's NAME — the row's display key
    /// resolved back, as the write-back resolves it. None off a row, off the
    /// pane, or on a section header.
    pub fn param_row_at(&self, x: f32, y: f32) -> Option<(usize, String)> {
        if !self.show_parameters || self.is_detached_network {
            return None;
        }
        // A row under a plate is the plate's, not the HUD's.
        if !self.params_claims(x, y) {
            return None;
        }
        let slot = self.param_editor_selected()?;
        let rects = self.param_row_rects();
        let child = self.param_editor_dir().children.get(slot)?;
        let rows = param_display(&child.params);
        let i = rects
            .iter()
            .position(|&(rx, ry, rw, rh)| rh > 0.0 && x >= rx && x <= rx + rw && y >= ry && y <= ry + rh)?;
        let row = rows.get(i)?;
        if row.2 == "section" {
            return None;
        }
        let p = child.params.iter().find(|p| {
            let key = p.shown_name();
            *key == row.0
        })?;
        Some((slot, p.name.clone()))
    }

    /// The params pane's row rects, index-parallel to `param_display` of the
    /// node it shows — the one geometry the row menu's hit test and the
    /// expression tint both read.
    pub fn param_row_rects(&self) -> Vec<(f32, f32, f32, f32)> {
        self.slots.param.as_any().downcast_ref::<ParametersBg>().map(|pb| pb.get_param_rects()).unwrap_or_default()
    }

    /// What the template says `pname` on `node` defaults to, where `dir` is
    /// the level `node` sits in. A child of a subnet instance (the Embryo's
    /// `sphere1`) takes the SUBNET template's word for it — its override
    /// (`chf("../radius")`) is the default that instance was built with,
    /// and the merge refreshes the child from there — and any other node
    /// its own template's. `None` for a parameter no template names, such
    /// as one added over MCP.
    pub fn template_default(&self, dir: &FsNode, node: &FsNode, pname: &str) -> Option<&ParamDef> {
        let from_subnet = template_for(dir, &self.node_templates)
            .and_then(|t| t.children.iter().find(|c| c.name.eq_ignore_ascii_case(&node.name)))
            .and_then(|c| c.params.iter().find(|p| p.name == pname));
        from_subnet.or_else(|| {
            template_for(node, &self.node_templates).and_then(|t| t.params.iter().find(|p| p.name == pname))
        })
    }

    /// The rows of a parameter's right-click menu: labels, the action each
    /// runs, and how many leading rows are HEADERS. Split from the open so
    /// a test reads them.
    ///
    /// The headers read the parameter out. `Control:` is the control the
    /// pane DRAWS for the row and `Type:` the type of value that control
    /// sets, in a programmer's terms — both read off the row as the pane
    /// shows it (`display_row_type`), not off the parameter's kind alone,
    /// because the two part ways: the Attribute node's Value is a text
    /// parameter the pane presents as three sliders over a float3
    /// (`add_pick_lists`), and an expression is a text box whatever its
    /// kind. Until 2026-09-28 Control was the kind's name, Type the raw
    /// type string and a third `Value:` row the type of the text held, so
    /// that Value row read `Control: text`, `Type: text`, `Value: string`
    /// over a control that was plainly a slider setting a vector. The raw
    /// type string's content is the rows below it (the range, the options).
    /// `Expression:` is the row's expression FLAG, `true` or `false` — the
    /// thing `ParamDef::expr` stores, which is what Edit Expression sets
    /// and Delete Expression clears. `Invalid:` appears only for a text the
    /// kind refuses, with the reason. `Default:` is the template's value
    /// for the row (`template_default`, as written there — an expression
    /// shows as the expression), left out for a parameter no template
    /// names. A control with a range shows `Min:` / `Max:` / `Step:` as the
    /// template DECLARES them (`ParamDef::declared_range`, an inline
    /// `slider:-2:2` included), `none` where it declares nothing, then
    /// `Range: lo..hi` as the pane APPLIES it, with its step when one is
    /// set — the parameter's own range, or the presented row's when the
    /// parameter has none (the Value row's adaptive span). A choice
    /// adds `Options: a, b, c`. Around those, `Name:` heads the list — the
    /// parameter's name, which is what a `ch()` path and a wire spell —
    /// with `Label:` after it only when the template gives one (the pane
    /// shows the name otherwise, and a Label row repeating it would say
    /// there is one) and the parameter's DESCRIPTION under them, ahead of
    /// `Control:` — what it does, from the template
    /// (`ParamDef::description`), wrapped to `PARAM_DESCRIPTION_WIDTH` over
    /// as many rows as it takes and unprefixed, since it reads as prose and
    /// not as a field — and `Shown when:` closes the list with the row's
    /// `show_when` condition when it has one.
    pub fn param_menu_rows(&self, slot: usize, pname: &str) -> (Vec<String>, Vec<ParamMenuAction>, usize) {
        let dir = self.param_editor_dir();
        let child = &dir.children[slot];
        let param = child.params.iter().find(|p| p.name == pname);
        let is_expr = param.is_some_and(|p| p.is_expr());
        let shown = param.map(|p| self.display_row_type(slot, p)).unwrap_or_default();
        let (control, ty) = param
            .map(|p| control_and_type(&shown, p.kind()))
            .unwrap_or(("?", "?"));
        let mut options = vec![format!("Name: {pname}")];
        if let Some(label) = param.map(|p| p.label.as_str()).filter(|l| !l.is_empty()) {
            options.push(format!("Label: {label}"));
        }
        if let Some(p) = param {
            options.extend(wrap_words(&p.description, PARAM_DESCRIPTION_WIDTH));
        }
        options.push(format!("Control: {control}"));
        options.push(format!("Type: {ty}"));
        options.push(format!("Expression: {is_expr}"));
        if let Some(why) = param.and_then(|p| p.invalid()) {
            options.push(format!("Invalid: {why}"));
        }
        if let Some(d) = self.template_default(dir, child, pname) {
            options.push(format!("Default: {}", d.text()));
        }
        // The range the pane applies: the parameter's own, or — for a text
        // parameter presented as a ranged control — the presented row's.
        let applied = param.and_then(|p| p.range()).or_else(|| shown_row_range(&shown));
        if let Some((lo, hi, step)) = applied {
            let fmt = |v: f32| crate::expr::fmt_num(v as f64);
            let declared = |v: Option<f32>| v.map(fmt).unwrap_or_else(|| "none".to_string());
            let (min, max, dstep) = param.map(|p| p.declared_range()).unwrap_or_default();
            options.push(format!("Min: {}", declared(min)));
            options.push(format!("Max: {}", declared(max)));
            options.push(format!("Step: {}", declared(dstep)));
            let step = step.map(|s| format!(", step {}", fmt(s))).unwrap_or_default();
            options.push(format!("Range: {}..{}{step}", fmt(lo), fmt(hi)));
        }
        if let Some(p) = param.filter(|p| p.kind() == ParamKind::Choice) {
            options.push(format!("Options: {}", p.choice_options().join(", ")));
        }
        if let Some(cond) = param.map(|p| p.show_when.as_str()).filter(|c| !c.is_empty()) {
            options.push(format!("Shown when: {cond}"));
        }
        let headers = options.len();
        let mut actions = vec![ParamMenuAction::Info; headers];
        options.push("-".to_string());
        options.push("Copy Parameter".to_string());
        actions.push(ParamMenuAction::Separator);
        actions.push(ParamMenuAction::CopyParameter);
        if self.copied_param.is_some() {
            options.push("Paste Relative Reference".to_string());
            actions.push(ParamMenuAction::PasteRelative);
            options.push("Paste Absolute Reference".to_string());
            actions.push(ParamMenuAction::PasteAbsolute);
        }
        options.push("-".to_string());
        actions.push(ParamMenuAction::Separator);
        // A row shown as a float3 can carry the trackball; the entry names
        // what picking it does.
        if shown.starts_with("float3") {
            if shown.split(':').nth(3) == Some("trackball") {
                options.push("Hide Trackball".to_string());
                actions.push(ParamMenuAction::HideTrackball);
            } else {
                options.push("Show Trackball".to_string());
                actions.push(ParamMenuAction::ShowTrackball);
            }
        }
        if is_expr {
            options.push("Delete Expression".to_string());
            actions.push(ParamMenuAction::DeleteExpression);
        } else {
            options.push("Edit Expression".to_string());
            actions.push(ParamMenuAction::EditExpression);
        }
        (options, actions, headers)
    }

    /// The row type the pane shows `param` of node `slot` under — the
    /// pane's own row when it is showing that node (so the picker and
    /// float3 presentations `add_pick_lists` makes are seen), what
    /// `param_display` alone would give otherwise.
    fn display_row_type(&self, slot: usize, param: &ParamDef) -> String {
        let key = param.shown_name();
        if self.param_editor_selected() == Some(slot) {
            if let Some(row) = self.param().node_params().into_iter().find(|r| r.0 == *key) {
                return row.2;
            }
        }
        param_display(std::slice::from_ref(param)).into_iter().next().map(|r| r.2).unwrap_or_default()
    }

    /// Open a parameter row's right-click menu.
    fn open_param_context_menu(&mut self, slot: usize, pname: String) {
        let node_id = self.param_editor_dir().children[slot].id.clone();
        let (options, actions, headers) = self.param_menu_rows(slot, &pname);
        let target = self.slots.get_dyn(crate::slots::PARAM_IDX).base().id();
        cce_ui::widget::context_menu::show(self.cursor_x, self.cursor_y, options, headers, target);
        self.param_menu_active = true;
        self.param_menu_actions = actions;
        self.param_menu_target = Some((node_id, pname));
    }

    pub fn param_menu_open(&self) -> bool {
        cce_ui::widget::context_menu::is_visible() && self.param_menu_active
    }

    fn close_param_menu(&mut self) {
        cce_ui::widget::context_menu::hide();
        self.param_menu_active = false;
        self.param_menu_actions.clear();
        self.param_menu_target = None;
    }

    /// Route a left press while the row menu is open — same contract as
    /// `handle_viewport_menu_click`.
    fn handle_param_menu_click(&mut self) -> bool {
        if !self.param_menu_open() {
            return false;
        }
        if cce_ui::widget::context_menu::hit_test(self.cursor_x, self.cursor_y) {
            let idx = cce_ui::widget::context_menu::row_at(self.cursor_x, self.cursor_y);
            let picked = idx.and_then(|i| self.param_menu_actions.get(i).copied());
            let target = self.param_menu_target.clone();
            self.close_param_menu();
            if let (Some(action), Some((node_id, pname))) = (picked, target) {
                self.run_param_action(&node_id, &pname, action);
            }
            return true;
        }
        self.close_param_menu();
        false
    }

    /// The playbar menu's rows and what each does: the transport (Play /
    /// Pause, Play / Pause Reverse, Go To Start Frame); the Repeat switch,
    /// marked from the live flag; then the timeline's settings as slider
    /// rows — Playback Rate, Start Frame, End Frame. Split from the open so
    /// a test can read it, as the viewport menu's is.
    pub(crate) fn playbar_menu_rows(&self) -> (Vec<String>, Vec<PlaybarMenuAction>) {
        let mut options = Vec::new();
        let mut actions = Vec::new();
        let mark = |on: bool| if on { cce_ui::widget::context_menu::MARK_ON } else { cce_ui::widget::context_menu::MARK_OFF };
        let mut command = |options: &mut Vec<String>, actions: &mut Vec<PlaybarMenuAction>, id: &'static str| {
            let Some(c) = crate::command::by_id(id) else { return };
            let label = match self.command_toggle_state(id) {
                Some(on) => format!("{}{}", mark(on), c.label),
                None => c.label.to_string(),
            };
            options.push(label);
            actions.push(PlaybarMenuAction::Command(id));
        };
        let mut row = |options: &mut Vec<String>, actions: &mut Vec<PlaybarMenuAction>, text: &str, a: PlaybarMenuAction| {
            options.push(text.to_string());
            actions.push(a);
        };
        command(&mut options, &mut actions, "play_pause");
        command(&mut options, &mut actions, "play_pause_reverse");
        command(&mut options, &mut actions, "frame_start");
        row(&mut options, &mut actions, "-", PlaybarMenuAction::Separator);
        command(&mut options, &mut actions, "toggle_playbar_repeat");
        command(&mut options, &mut actions, "toggle_playbar_step_buttons");
        row(&mut options, &mut actions, "-", PlaybarMenuAction::Separator);
        row(&mut options, &mut actions, "Playback Rate", PlaybarMenuAction::FpsSlider);
        row(&mut options, &mut actions, "Start Frame", PlaybarMenuAction::StartFrameSlider);
        row(&mut options, &mut actions, "End Frame", PlaybarMenuAction::EndFrameSlider);
        // The playbar's plate rows, as a page: one Plate row.
        if !self.plate_menu_rows(PLAYBAR_IDX).0.is_empty() {
            row(&mut options, &mut actions, "-", PlaybarMenuAction::Separator);
            row(&mut options, &mut actions, "Plate", PlaybarMenuAction::PlatePage);
        }
        (options, actions)
    }

    /// The slider a playbar menu row carries, from the live value.
    pub(crate) fn playbar_menu_slider(&self, action: PlaybarMenuAction) -> Option<cce_ui::widget::context_menu::MenuSlider> {
        use cce_ui::widget::context_menu::MenuSlider;
        let pb = self.slots.playbar.inner();
        Some(match action {
            PlaybarMenuAction::FpsSlider => MenuSlider { value: pb.fps.clamp(1.0, 120.0).round(), min: 1.0, max: 120.0, step: 1.0, decimals: 0, suffix: " fps" },
            PlaybarMenuAction::StartFrameSlider => MenuSlider { value: pb.start_frame.round().clamp(1.0, 999.0), min: 1.0, max: 999.0, step: 1.0, decimals: 0, suffix: "" },
            PlaybarMenuAction::EndFrameSlider => MenuSlider { value: pb.end_frame.round().clamp(2.0, 1000.0), min: 2.0, max: 1000.0, step: 1.0, decimals: 0, suffix: "" },
            _ => return None,
        })
    }

    /// Land a playbar slider: the rate, or an end of the range, the other
    /// end kept a frame clear of it and the playhead kept inside. The rate
    /// is a setting and saved with them; the range is the project's and
    /// dirties it.
    fn land_playbar_menu_slider(&mut self, action: PlaybarMenuAction, v: f32) {
        let pb = self.slots.playbar.inner_mut();
        match action {
            PlaybarMenuAction::FpsSlider => pb.fps = v.round().clamp(1.0, 120.0),
            PlaybarMenuAction::StartFrameSlider => {
                pb.start_frame = v.round().clamp(1.0, 999.0);
                pb.end_frame = pb.end_frame.max(pb.start_frame + 1.0);
            }
            PlaybarMenuAction::EndFrameSlider => {
                pb.end_frame = v.round().clamp(2.0, 1000.0);
                pb.start_frame = pb.start_frame.min(pb.end_frame - 1.0);
            }
            _ => return,
        }
        pb.current_frame = pb.current_frame.clamp(pb.start_frame, pb.end_frame);
        self.viewport_dirty = true;
        self.update_window_title();
    }

    pub(crate) fn open_playbar_context_menu(&mut self) {
        self.open_playbar_context_menu_at(None);
    }

    /// The playbar's menu at the pointer, or with its top-left at `at`.
    pub(crate) fn open_playbar_context_menu_at(&mut self, at: Option<(f32, f32)>) {
        let (options, actions) = self.playbar_menu_rows();
        let target = self.slots.get_dyn(PLAYBAR_IDX).base().id();
        self.put_up_menu(at, None, options, 0, target);
        crate::menu_page::mark_page_rows(&actions, |a| a == PlaybarMenuAction::PlatePage);
        for (i, a) in actions.iter().enumerate() {
            if let Some(slider) = self.playbar_menu_slider(*a) {
                cce_ui::widget::context_menu::set_row_slider(i, slider);
            }
        }
        self.playbar_menu_actions = actions;
        self.playbar_menu_active = true;
    }

    pub fn playbar_menu_open(&self) -> bool {
        cce_ui::widget::context_menu::is_visible() && self.playbar_menu_active
    }

    pub(crate) fn close_playbar_menu(&mut self) {
        cce_ui::widget::context_menu::hide();
        self.playbar_menu_active = false;
        self.playbar_menu_actions.clear();
    }

    /// Whether the pointer is over the playbar's plate.
    pub fn over_playbar(&self, px: f32, py: f32) -> bool {
        let (x, y, w, h) = self.positions[PLAYBAR_IDX];
        self.show_playbar && w > 0.0 && h > 0.0 && px >= x && px < x + w && py >= y && py < y + h
    }

    /// Route a left press while the playbar menu is open — the viewport
    /// menu's contract: a slider row is worked and keeps the menu up, any
    /// other row runs and closes it.
    fn handle_playbar_menu_click(&mut self) -> bool {
        if !self.playbar_menu_open() {
            return false;
        }
        if cce_ui::widget::context_menu::slider_press(self.cursor_x, self.cursor_y) {
            self.drain_playbar_menu_slider(false);
            return true;
        }
        if cce_ui::widget::context_menu::hit_test(self.cursor_x, self.cursor_y) {
            let idx = cce_ui::widget::context_menu::row_at(self.cursor_x, self.cursor_y);
            let picked = idx.and_then(|i| self.playbar_menu_actions.get(i).copied());
            self.close_playbar_menu();
            if let Some(action) = picked {
                self.run_playbar_menu_action(action);
            }
            return true;
        }
        self.close_playbar_menu();
        false
    }

    pub(crate) fn run_playbar_menu_action(&mut self, action: PlaybarMenuAction) {
        match action {
            PlaybarMenuAction::Command(id) => {
                self.run_command(id);
            }
            _ => {}
        }
    }

    /// Land what a playbar menu slider did; `persist` saves the rate with
    /// the settings, as the viewport menu's drain does.
    pub(crate) fn drain_playbar_menu_slider(&mut self, persist: bool) -> bool {
        let Some((idx, v)) = cce_ui::widget::context_menu::take_slider_change() else {
            if persist {
                self.save_settings();
            }
            return false;
        };
        if let Some(action) = self.playbar_menu_actions.get(idx).copied() {
            self.land_playbar_menu_slider(action, v);
            if persist {
                self.save_settings();
            }
        }
        true
    }

    /// Whichever menu with slider rows is open — the viewport's or the
    /// playbar's — drained: the four slider hooks in `handle_event` ask
    /// this so neither menu has to be named there.
    fn drain_menu_slider(&mut self, persist: bool) -> bool {
        if self.playbar_menu_open() {
            self.drain_playbar_menu_slider(persist)
        } else {
            self.drain_viewport_menu_slider(persist)
        }
    }

    /// A menu whose rows carry sliders is open.
    fn slider_menu_open(&self) -> bool {
        self.viewport_menu_open() || self.playbar_menu_open()
    }

    /// One row-menu action on one parameter, by node id and name — the
    /// entry the menu, a test and any future command share.
    pub fn run_param_action(&mut self, node_id: &str, pname: &str, action: ParamMenuAction) {
        let before = crate::viewer_state::find_node_by_id(&self.fs_root, node_id)
            .and_then(|n| n.params.iter().find(|p| p.name == pname))
            .cloned();
        self.run_param_action_unrecorded(node_id, pname, action);
        if let Some(before) = before {
            self.record_param_edit(node_id, before);
        }
    }

    fn run_param_action_unrecorded(&mut self, node_id: &str, pname: &str, action: ParamMenuAction) {
        let node_label = crate::geometry::node_path_names(&self.fs_root, node_id)
            .map(|n| format!("/{}", n.join("/")))
            .unwrap_or_else(|| node_id.to_string());
        match action {
            ParamMenuAction::Separator | ParamMenuAction::Info => return,
            ParamMenuAction::CopyParameter => {
                self.copied_param = Some((node_id.to_string(), pname.to_string()));
                self.update_status_text(&format!("Copied {node_label}/{pname} — paste it as a reference on another parameter."));
                return;
            }
            ParamMenuAction::PasteRelative | ParamMenuAction::PasteAbsolute => {
                let Some((src_id, src_p)) = self.copied_param.clone() else {
                    self.update_status_text("Nothing copied — Copy Parameter first.");
                    return;
                };
                let path = if action == ParamMenuAction::PasteRelative {
                    crate::geometry::relative_ref_path(&self.fs_root, node_id, &src_id)
                } else {
                    crate::geometry::absolute_ref_path(&self.fs_root, &src_id)
                };
                let Some(path) = path else {
                    self.update_status_text("The copied parameter's node is gone.");
                    return;
                };
                // `chs` for a row that holds text, `ch` for one that holds a
                // number — by the TARGET, since that is what the value has
                // to fit: a choice pasted onto a switch's Index wants the
                // option's index, pasted onto a text row its name.
                let target_kind = crate::viewer_state::find_node_by_id(&self.fs_root, node_id)
                    .and_then(|n| n.params.iter().find(|p| p.name == pname))
                    .map(|p| p.kind())
                    .unwrap_or(ParamKind::Text);
                let func = match target_kind {
                    ParamKind::Text | ParamKind::Node | ParamKind::Attribute | ParamKind::Group | ParamKind::Choice | ParamKind::Code => "chs",
                    _ => "ch",
                };
                let full = if path.is_empty() { src_p.clone() } else { format!("{path}/{src_p}") };
                let value = format!("{func}(\"{full}\")");
                if let Some(p) = crate::viewer_state::find_node_by_id_mut(&mut self.fs_root, node_id)
                    .and_then(|n| n.params.iter_mut().find(|p| p.name == pname))
                {
                    p.set_text(value.clone());
                    p.set_expr(true);
                }
                self.update_status_text(&format!("{pname} = {value}"));
            }
            ParamMenuAction::ShowTrackball | ParamMenuAction::HideTrackball => {
                let show = action == ParamMenuAction::ShowTrackball;
                if let Some(p) = crate::viewer_state::find_node_by_id_mut(&mut self.fs_root, node_id)
                    .and_then(|n| n.params.iter_mut().find(|p| p.name == pname))
                {
                    p.view = if show { "trackball" } else { "sliders" }.to_string();
                }
                self.update_status_text(&if show {
                    format!("{pname}: drag the ball to turn the vector; its length is kept.")
                } else {
                    format!("{pname}: sliders only.")
                });
            }
            ParamMenuAction::EditExpression => {
                if let Some(p) = crate::viewer_state::find_node_by_id_mut(&mut self.fs_root, node_id)
                    .and_then(|n| n.params.iter_mut().find(|p| p.name == pname))
                {
                    p.set_expr(true);
                }
                self.update_status_text(&format!("{pname} is an expression — type ch(\"../node/param\"), $F, arithmetic."));
            }
            ParamMenuAction::DeleteExpression => {
                // The expression's value NOW becomes the value, as Houdini's
                // Delete Channels keeps what the channel was showing. One that
                // does not evaluate keeps its text, no longer as an expression.
                let frame = self.sim_frame();
                let evaluated = crate::viewer_state::find_node_by_id(&self.fs_root, node_id).and_then(|node| {
                    let mut err = None;
                    let resolved = crate::geometry::resolve_param_refs(&self.fs_root, node, frame, &mut err)?;
                    resolved.params.into_iter().find(|p| p.name == pname).map(|p| (p.text().to_string(), err))
                });
                let mut note = None;
                if let Some(p) = crate::viewer_state::find_node_by_id_mut(&mut self.fs_root, node_id)
                    .and_then(|n| n.params.iter_mut().find(|p| p.name == pname))
                {
                    p.set_expr(false);
                    match evaluated {
                        Some((value, None)) => {
                            p.bake(value.clone());
                            note = Some(format!("{pname} = {value}, no longer an expression."));
                        }
                        Some((_, Some(e))) => note = Some(format!("{pname} kept as text — it did not evaluate: {e}")),
                        None => {}
                    }
                }
                if let Some(n) = note {
                    self.update_status_text(&n);
                }
            }
        }
        // The SetParam resync sequence.
        self.sync_grid_settings();
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
    }

    /// Open the viewport right-click context menu at the cursor.
    pub(crate) fn open_viewport_context_menu(&mut self) {
        self.show_viewport_menu_page(None, None);
    }

    /// Put up the viewport menu (`None`) or one of its pages: at the
    /// pointer, or with its top-left at `at` in place of the plate that
    /// stood there — a page under a back band to the menu.
    pub(crate) fn show_viewport_menu_page(&mut self, page: Option<ViewportMenuPage>, at: Option<(f32, f32)>) {
        let (options, actions) = self.viewport_menu_rows_of(page);
        let target = self.slots.viewport.id();
        let back = page.map(|_| crate::menu_page::MenuOrigin::Viewport);
        self.put_up_menu(at, back, options, 0, target);
        for (i, a) in actions.iter().enumerate() {
            if let Some(slider) = self.viewport_menu_slider(*a) {
                cce_ui::widget::context_menu::set_row_slider(i, slider);
            }
        }
        crate::menu_page::mark_page_rows(&actions, |a| a.leads_to_page());
        self.viewport_menu_actions = actions;
        self.viewport_menu_page = page;
        self.viewport_menu_active = true;
    }

    /// Re-read the shown viewport menu page's labels and slider values in
    /// place, after a row of it ran and it stays up: how a switch's mark
    /// follows the switch.
    pub(crate) fn refill_viewport_menu(&mut self) {
        let (options, actions) = self.viewport_menu_rows_of(self.viewport_menu_page);
        let sliders: Vec<_> = actions.iter().map(|a| self.viewport_menu_slider(*a)).collect();
        if cce_ui::widget::context_menu::refill(options, &sliders) {
            self.viewport_menu_actions = actions;
        } else {
            let at = (cce_ui::widget::context_menu::x(), cce_ui::widget::context_menu::y());
            self.show_viewport_menu_page(self.viewport_menu_page, Some(at));
        }
    }

    /// The slider a viewport menu row carries, read from the live value —
    /// `None` for an action row. The one table of the menu's sliders:
    /// `open_viewport_context_menu` sets each from here and
    /// `land_viewport_menu_slider` writes each back, so a slider row is
    /// added in those two matches and the row list.
    ///
    /// Both opacities read in percent, stepped by 5; Wire Thickness in px over the
    /// palette row's own 1–8, by half a pixel. The palette rows are the fine
    /// controls.
    pub(crate) fn viewport_menu_slider(&self, action: ViewportMenuAction) -> Option<cce_ui::widget::context_menu::MenuSlider> {
        use cce_ui::widget::context_menu::MenuSlider;
        Some(match action {
            ViewportMenuAction::OpacitySlider => MenuSlider {
                value: (self.geo_opacity.clamp(0.0, 1.0) * 100.0).round(),
                min: 0.0,
                max: 100.0,
                step: 5.0,
                decimals: 0,
                suffix: "%",
            },
            ViewportMenuAction::WireOpacitySlider => MenuSlider {
                value: (self.wire_opacity.clamp(0.0, 1.0) * 100.0).round(),
                min: 0.0,
                max: 100.0,
                step: 5.0,
                decimals: 0,
                suffix: "%",
            },
            ViewportMenuAction::WireThicknessSlider => MenuSlider {
                value: self.wire_width.clamp(1.0, 8.0),
                min: 1.0,
                max: 8.0,
                step: 0.5,
                decimals: 1,
                suffix: " px",
            },
            // World units, so no suffix — the World Unit declaration is what
            // names them, and a readout saying "mm" under a cm declaration
            // would be wrong.
            // The palette row's spin is 5–100 thousandths; the same range in
            // world units here, where the readout has room for the decimals.
            ViewportMenuAction::PointMarkerSizeSlider => MenuSlider {
                value: self.point_marker_size.clamp(0.005, 0.1),
                min: 0.005,
                max: 0.1,
                step: 0.005,
                decimals: 3,
                suffix: "",
            },
            // World units, as the point markers' size is.
            ViewportMenuAction::GroupMarkerSizeSlider => MenuSlider {
                value: self.group_marker_size.clamp(0.0, GROUP_MARKER_SIZE_MAX),
                min: 0.0,
                max: GROUP_MARKER_SIZE_MAX,
                step: 0.005,
                decimals: 3,
                suffix: "",
            },
            // A multiple of the true displacement, read as one ("1.25x"): a
            // plain x rather than a multiplication sign, which the menu face
            // may not carry and which a fallback glyph would then
            // under-measure.
            ViewportMenuAction::CameraPivotSizeSlider => MenuSlider {
                value: self.camera_pivot_size.clamp(0.0, 1.0),
                min: 0.0,
                max: 1.0,
                step: 0.05,
                decimals: 2,
                suffix: "x",
            },
            ViewportMenuAction::PullArrowScaleSlider => MenuSlider {
                value: self.pull_arrow_scale.clamp(0.25, 10.0),
                min: 0.25,
                max: 10.0,
                step: 0.25,
                decimals: 2,
                suffix: "x",
            },
            _ => return None,
        })
    }

    /// Write a slider row's value onto the live field, and redo only what
    /// that value feeds. The opacities and wire thickness are draw-time values (a
    /// uniform, a line width and the fill's matching depth bias). Group
    /// Marker Size re-sizes the group markers from their kept members,
    /// Point Marker Size re-sizes the overlay from
    /// the scene positions its rebuild kept. None of it re-evaluates the
    /// graph.
    fn land_viewport_menu_slider(&mut self, action: ViewportMenuAction, v: f32) {
        // The menu's opacity rows are in percent; the field is a fraction.
        if action == ViewportMenuAction::CameraPivotSizeSlider {
            // The live field itself: the palette row writes whole tenths,
            // which is coarser than this slider's step.
            self.camera_pivot_size = v.clamp(0.0, 1.0);
            self.update_pivot_geometry();
            self.viewport_dirty = true;
            return;
        }
        let (key, v) = match action {
            ViewportMenuAction::OpacitySlider => ("geo_opacity", v / 100.0),
            ViewportMenuAction::WireThicknessSlider => ("wire_width", v),
            ViewportMenuAction::WireOpacitySlider => ("wire_opacity", v / 100.0),
            ViewportMenuAction::PointMarkerSizeSlider => ("point_marker_size", v),
            ViewportMenuAction::GroupMarkerSizeSlider => ("group_marker_size", v),
            ViewportMenuAction::PullArrowScaleSlider => ("pull_arrow_scale", v),
            _ => return,
        };
        self.land_draw_time_setting(key, v);
    }

    /// Land a DRAW-TIME display setting by its `DesignSettings` field key —
    /// the one landing behind the viewport menu's sliders AND the dialog's
    /// slider and spin rows (`land_dialog_slider`), so the two cannot
    /// disagree about a clamp or about which mesh a size feeds. The three
    /// guide sizes re-bake their own small mesh (a grid, three axes, a
    /// pivot) and nothing else. Returns false for a key that is not one of
    /// these, which the dialog takes as "run the full apply".
    ///
    /// Until 2026-09-28 the dialog's sliders went through `apply_setting` on
    /// every motion of a drag: a whole graph evaluation, a second one for
    /// the group markers and a third for the params pane's pickers (both
    /// keyed on the geometry version the first had just bumped), a restart
    /// of the path tracer's refine, and a synchronous state.kdl write — per
    /// pointer event, for six values none of which the graph reads.
    pub(crate) fn land_draw_time_setting(&mut self, key: &str, v: f32) -> bool {
        match key {
            "geo_opacity" => self.geo_opacity = v.clamp(0.0, 1.0),
            "wire_width" => self.wire_width = v.clamp(1.0, 8.0),
            "wire_opacity" => self.wire_opacity = v.clamp(0.0, 1.0),
            "point_marker_size" => {
                self.point_marker_size = v.clamp(0.005, 0.1);
                self.rebuild_overlay_markers();
            }
            "group_marker_size" => {
                self.group_marker_size = v.clamp(0.0, GROUP_MARKER_SIZE_MAX);
                self.rebuild_group_markers();
            }
            "pull_arrow_scale" => {
                self.pull_arrow_scale = v.clamp(0.25, 10.0);
                self.rebuild_pull_arrow_verts();
            }
            // The spin rows' ranges, in world units (the rows read in
            // thousandths and tenths).
            "grid_thickness" => {
                self.grid_thickness = v.clamp(0.002, 0.2);
                self.update_grid_geometry();
            }
            "origin_size" => {
                self.origin_size = v.clamp(0.1, 5.0);
                self.update_origin_geometry();
            }
            _ => return false,
        }
        self.viewport_dirty = true;
        true
    }

    /// Land what a viewport menu slider did. During a drag this is called
    /// on every motion, so it only sets the value and asks for a redraw —
    /// every menu slider is a draw-time value, and `apply_setting`'s full
    /// regenerate-and-rebuild pass would re-evaluate the graph per pixel of
    /// drag. `persist` saves state.kdl, which the wheel does per step and a
    /// drag does once, on the release.
    pub(crate) fn drain_viewport_menu_slider(&mut self, persist: bool) -> bool {
        use cce_ui::widget::context_menu;
        let changed = context_menu::take_slider_change()
            .and_then(|(idx, v)| Some((self.viewport_menu_actions.get(idx).copied()?, v)));
        let Some((action, v)) = changed else {
            if persist {
                self.save_settings();
            }
            return false;
        };
        {
            if self.viewport_menu_slider(action).is_some() {
                self.land_viewport_menu_slider(action, v);
                if persist {
                    self.save_settings();
                }
            }
        }
        true
    }

    /// The viewport menu's rows and what each does, in groups a separator
    /// apart: framing; the GUIDES (Show Grid, Show Origin — the scene
    /// furniture that is not the geometry); the WIREFRAME (its switch,
    /// thickness and opacity); the
    /// SELECTION FEEDBACK
    /// (the group markers' size and the pull arrows' scale); the OVERLAYS (Show Point Markers and its size, Show
    /// Point Numbers, Show Point Normals — the annotations drawn over the
    /// scene's points); the SURFACE (flat or smooth shading as a radio pair,
    /// the polygon opacity, Show Occluded — the three that decide how the
    /// fill itself reads); then the editor pin. Split from the open so a
    /// test can read it. Marks are the ●/○ the pin rows and the network menu
    /// use.
    pub(crate) fn viewport_menu_rows(&self) -> (Vec<String>, Vec<ViewportMenuAction>) {
        self.viewport_menu_rows_of(None)
    }

    /// Turn the open viewport menu to the page that holds `action`, as a
    /// press on its row does, and hand back that page's actions.
    #[cfg(test)]
    pub(crate) fn open_viewport_page_with(&mut self, action: ViewportMenuAction) -> Vec<ViewportMenuAction> {
        let page = self.viewport_menu_page_of(action).expect("a page holds the row");
        self.run_viewport_menu_action(ViewportMenuAction::Page(page));
        assert_eq!(self.viewport_menu_page, Some(page), "the menu turned");
        self.viewport_menu_actions.clone()
    }

    /// The page of the viewport menu that holds `action`, `None` for the
    /// menu's own rows — how a test finds a row without knowing the
    /// menu's layout.
    #[cfg(test)]
    pub(crate) fn viewport_menu_page_of(&self, action: ViewportMenuAction) -> Option<ViewportMenuPage> {
        [ViewportMenuPage::Style, ViewportMenuPage::Markers]
            .into_iter()
            .find(|page| self.viewport_menu_rows_of(Some(*page)).1.contains(&action))
    }

    /// The rows of the viewport menu (`None`) or of one of its pages.
    /// The menu holds what is DONE (framing), the guides, and a row for each
    /// page; the STYLE page holds how the geometry is drawn
    /// (wireframe, then surface) and the MARKERS page what is drawn on
    /// it (the points, then the overlays of each element class).
    pub(crate) fn viewport_menu_rows_of(&self, page: Option<ViewportMenuPage>) -> (Vec<String>, Vec<ViewportMenuAction>) {
        let mut options: Vec<String> = Vec::new();
        let mut actions: Vec<ViewportMenuAction> = Vec::new();
        let mark = |on: bool| if on { cce_ui::widget::context_menu::MARK_ON } else { cce_ui::widget::context_menu::MARK_OFF };
        let label = |id: &str, fallback: &'static str| crate::command::by_id(id).map(|c| c.label).unwrap_or(fallback);
        let row = |options: &mut Vec<String>, actions: &mut Vec<ViewportMenuAction>, text: String, a: ViewportMenuAction| {
            options.push(text);
            actions.push(a);
        };
        let sep = ViewportMenuAction::Separator;
        let toggle = |options: &mut Vec<String>, actions: &mut Vec<ViewportMenuAction>, id: &'static str| {
            let on = self.command_toggle_state(id).unwrap_or(false);
            options.push(format!("{}{}", mark(on), label(id, id)));
            actions.push(ViewportMenuAction::Command(id));
        };

        match page {
            Some(ViewportMenuPage::Style) => {
                // Wireframe.
                toggle(&mut options, &mut actions, "toggle_wireframe");
                row(&mut options, &mut actions, "Wire Thickness".into(), ViewportMenuAction::WireThicknessSlider);
                row(&mut options, &mut actions, "Wire Opacity".into(), ViewportMenuAction::WireOpacitySlider);

                // Surface.
                row(&mut options, &mut actions, "-".into(), sep);
                row(&mut options, &mut actions, format!("{}Flat Shading", mark(!self.smooth_shading)), ViewportMenuAction::Shading(false));
                row(&mut options, &mut actions, format!("{}Smooth Shading", mark(self.smooth_shading)), ViewportMenuAction::Shading(true));
                row(&mut options, &mut actions, "Opacity".into(), ViewportMenuAction::OpacitySlider);
                toggle(&mut options, &mut actions, "toggle_show_occluded");
                return (options, actions);
            }
            Some(ViewportMenuPage::Markers) => {
                // The selection feedback: the group markers' size and the
                // pull arrows' length.
                row(&mut options, &mut actions, "Group Marker Size".into(), ViewportMenuAction::GroupMarkerSizeSlider);
                row(&mut options, &mut actions, "Pull Arrow Scale".into(), ViewportMenuAction::PullArrowScaleSlider);

                // The overlays, a class at a time: points, primitives,
                // vertices.
                row(&mut options, &mut actions, "-".into(), sep);
                toggle(&mut options, &mut actions, "toggle_point_markers");
                row(&mut options, &mut actions, "Point Marker Size".into(), ViewportMenuAction::PointMarkerSizeSlider);
                toggle(&mut options, &mut actions, "toggle_point_numbers");
                toggle(&mut options, &mut actions, "toggle_point_normals");
                row(&mut options, &mut actions, "-".into(), sep);
                toggle(&mut options, &mut actions, "toggle_prim_numbers");
                toggle(&mut options, &mut actions, "toggle_prim_normals");
                row(&mut options, &mut actions, "-".into(), sep);
                toggle(&mut options, &mut actions, "toggle_vertex_markers");
                toggle(&mut options, &mut actions, "toggle_vertex_numbers");
                toggle(&mut options, &mut actions, "toggle_vertex_normals");
                return (options, actions);
            }
            None => {}
        }

        // While the network overlays the scene, empty graph space is the
        // scene's, so the network's menu is a page of this one, at its head.
        if self.network_overlay() {
            row(&mut options, &mut actions, "Network".into(), ViewportMenuAction::NetworkPage);
            row(&mut options, &mut actions, "-".into(), sep);
        }

        row(&mut options, &mut actions, "Frame All".into(), ViewportMenuAction::FrameAll);
        row(&mut options, &mut actions, "View 1:1".into(), ViewportMenuAction::OneToOne);

        // The image's two camera rows, under the scene's, while one shows.
        if self.page_shown.is_some() {
            row(&mut options, &mut actions, label("frame_image", "Frame Image").to_string(), ViewportMenuAction::Command("frame_image"));
            row(&mut options, &mut actions, label("view_image_pixels", "View Image Pixels 1:1").to_string(), ViewportMenuAction::Command("view_image_pixels"));
        }

        // Guides: the scene furniture that is not the geometry.
        row(&mut options, &mut actions, "-".into(), sep);
        toggle(&mut options, &mut actions, "toggle_grid");
        toggle(&mut options, &mut actions, "toggle_origin");
        toggle(&mut options, &mut actions, "toggle_camera_pivot");
        row(&mut options, &mut actions, "Camera Pivot Size".into(), ViewportMenuAction::CameraPivotSizeSlider);

        // The display settings, a page each, and the visualizers' editor,
        // which is the dialog: three rows the menu turns into what they
        // name.
        row(&mut options, &mut actions, "-".into(), sep);
        for page in [ViewportMenuPage::Style, ViewportMenuPage::Markers] {
            row(&mut options, &mut actions, page.label().into(), ViewportMenuAction::Page(page));
        }
        row(
            &mut options,
            &mut actions,
            label("attribute_visualizers", "Attribute Visualizers").to_string(),
            ViewportMenuAction::Command("attribute_visualizers"),
        );

        // The viewport's editor binding, as a radio group: follow the active
        // editor, or pin to one. Pin rows appear only while a second editor
        // exists — with one editor, following IS pinned.
        if self.tab_dock_of_pane(crate::slots::NETWORK_PANEL2_IDX).is_some() {
            options.push("-".to_string());
            actions.push(ViewportMenuAction::Separator);
            let mark = |on: bool| if on { cce_ui::widget::context_menu::MARK_ON } else { cce_ui::widget::context_menu::MARK_OFF };
            options.push(format!("{}Follow Active Editor", mark(self.viewport_pin.is_none())));
            actions.push(ViewportMenuAction::PinFollow);
            options.push(format!(
                "{}Pin: Network",
                mark(self.viewport_pin == Some(CONTENT_IDX))
            ));
            actions.push(ViewportMenuAction::PinTo(CONTENT_IDX));
            options.push(format!(
                "{}Pin: Network 2",
                mark(self.viewport_pin == Some(crate::slots::CONTENT2_IDX))
            ));
            actions.push(ViewportMenuAction::PinTo(crate::slots::CONTENT2_IDX));
        }
        (options, actions)
    }

    /// Run one viewport menu row — the click path, and the tests'.
    pub(crate) fn run_viewport_menu_action(&mut self, action: ViewportMenuAction) {
        match action {
            ViewportMenuAction::FrameAll => {
                self.frame_all();
            }
            ViewportMenuAction::OneToOne => {
                self.view_one_to_one();
            }
            // The menu turns into the page where it stands.
            ViewportMenuAction::Page(page) => {
                let at = (cce_ui::widget::context_menu::x(), cce_ui::widget::context_menu::y());
                self.show_viewport_menu_page(Some(page), Some(at));
            }
            ViewportMenuAction::NetworkPage => {
                let at = (cce_ui::widget::context_menu::x(), cce_ui::widget::context_menu::y());
                self.close_viewport_menu();
                self.open_network_menu_from_viewport(at);
            }
            ViewportMenuAction::PinFollow => {
                self.viewport_pin = None;
                self.rebuild_scene_geometry();
            }
            ViewportMenuAction::PinTo(e) => {
                self.viewport_pin = Some(e);
                self.rebuild_scene_geometry();
            }
            ViewportMenuAction::Command(id) => {
                self.run_command(id);
            }
            ViewportMenuAction::Shading(smooth) => {
                if self.smooth_shading != smooth {
                    self.run_command("toggle_smooth_shading");
                }
            }
            // The slider row is worked, not picked.
            ViewportMenuAction::OpacitySlider
            | ViewportMenuAction::WireThicknessSlider
            | ViewportMenuAction::WireOpacitySlider
            | ViewportMenuAction::PointMarkerSizeSlider
            | ViewportMenuAction::GroupMarkerSizeSlider
            | ViewportMenuAction::PullArrowScaleSlider
            | ViewportMenuAction::CameraPivotSizeSlider => {}
            ViewportMenuAction::Separator => {}
        }
    }

    pub fn viewport_menu_open(&self) -> bool {
        cce_ui::widget::context_menu::is_visible() && self.viewport_menu_active
    }

    pub(crate) fn close_viewport_menu(&mut self) {
        cce_ui::widget::context_menu::hide();
        self.viewport_menu_active = false;
        self.viewport_menu_actions.clear();
        self.viewport_menu_page = None;
    }

    /// Route a left press while the viewport menu is open — same contract as
    /// `handle_node_menu_click`.
    fn handle_viewport_menu_click(&mut self) -> bool {
        if !self.viewport_menu_open() {
            return false;
        }
        // A press on a slider row is the slider's: it jumps (on the band)
        // and keeps the menu open, where every other row fires and closes.
        if cce_ui::widget::context_menu::slider_press(self.cursor_x, self.cursor_y) {
            self.drain_viewport_menu_slider(false);
            return true;
        }
        use cce_ui::widget::context_menu;
        if context_menu::hit_test(self.cursor_x, self.cursor_y) {
            let idx = context_menu::row_at(self.cursor_x, self.cursor_y);
            let picked = idx.and_then(|i| self.viewport_menu_actions.get(i).copied());
            // A row of a PAGE runs and the page stays up, re-marked: a page
            // is a panel of settings, opened to set several, and a switch
            // that closed it would cost a right-click and a turn per
            // setting. A row of the menu itself runs and closes it.
            if self.viewport_menu_page.is_some() {
                if let Some(action) = picked {
                    self.run_viewport_menu_action(action);
                    if self.viewport_menu_open() {
                        self.refill_viewport_menu();
                    }
                }
                return true;
            }
            self.close_viewport_menu();
            if let Some(action) = picked {
                self.run_viewport_menu_action(action);
            }
            return true;
        }
        self.close_viewport_menu();
        false
    }

    /// Open the network editor's right-click context menu at the cursor. It is
    /// what a press on EMPTY graph space opens; a press on a node still opens
    /// that node's menu, which is the more specific thing under the pointer.
    ///
    /// Until 2026-09-22 this press opened the add-node palette outright, which
    /// left the network the one pane whose right-click was not a context menu
    /// — and left every other graph-wide command reachable only by chord or
    /// through the palette. Add Node is the first row instead.
    ///
    /// Rows are built from `NETWORK_MENU_COMMANDS` through `command::by_id`,
    /// so a label is the registry's label and a row cannot name work the
    /// palette spells differently. A toggle command carries the same ●/○ mark
    /// the viewport menu's radio rows use, read through
    /// `command_toggle_state` — the one table the dialog's switches read too.
    fn open_network_context_menu(&mut self) {
        self.network_menu_from = None;
        self.open_network_context_menu_at(None);
    }

    /// Turn the viewport menu, standing at `at`, into the network's: what
    /// its Network row does while the network has no plate. The network
    /// takes focus — its menu's commands are the network's, and the
    /// cursor ones act only on a focused network — and the grid cursor goes
    /// to the cell the menu was opened over, when it was opened over one.
    pub(crate) fn open_network_menu_from_viewport(&mut self, at: (f32, f32)) {
        if let Some((col, row)) = self.network_menu_cell.take() {
            self.grid_cursor_col = col;
            self.grid_cursor_row = row;
        }
        if self.focused_pane != LEFT_MENUBAR_IDX {
            self.focused_pane = LEFT_MENUBAR_IDX;
            if let Some(old) = self.focused_widget.take() {
                self.slots.get_dyn_mut(old).unfocus();
            }
            self.sync_pane_focus();
        }
        self.network_menu_from = Some(crate::menu_page::MenuOrigin::Viewport);
        self.open_network_context_menu_at(Some(at));
    }

    /// The network menu at the pointer, or with its top-left at `at` in
    /// place of a page it is turned back to from.
    pub(crate) fn open_network_context_menu_at(&mut self, at: Option<(f32, f32)>) {
        let mut options: Vec<String> = Vec::new();
        let mut actions: Vec<NetworkMenuAction> = Vec::new();
        for entry in NETWORK_MENU_COMMANDS {
            match entry {
                None => {
                    if options.is_empty() || options.last().map(String::as_str) == Some("-") {
                        continue;
                    }
                    options.push("-".to_string());
                    actions.push(NetworkMenuAction::Separator);
                }
                Some(id) => {
                    let Some(cmd) = crate::command::by_id(id) else { continue };
                    options.push(match self.command_toggle_state(id) {
                        Some(on) => format!("{}{}", if on { cce_ui::widget::context_menu::MARK_ON } else { cce_ui::widget::context_menu::MARK_OFF }, cmd.label),
                        None => cmd.label.to_string(),
                    });
                    actions.push(NetworkMenuAction::Command(cmd.id));
                }
            }
        }
        if options.last().map(String::as_str) == Some("-") {
            options.pop();
            actions.pop();
        }
        // The network pane's plate rows, below the graph's own, as a page:
        // one Plate row the menu turns into them (`open_plate_page`).
        if !self.plate_menu_rows(NETWORK_PANEL_IDX).0.is_empty() {
            options.push("-".to_string());
            actions.push(NetworkMenuAction::Separator);
            options.push("Plate".to_string());
            actions.push(NetworkMenuAction::PlatePage);
        }

        let target = self.slots.get_dyn(CONTENT_IDX).base().id();
        let back = self.network_menu_from.filter(|_| at.is_some());
        self.put_up_menu(at, back, options, 0, target);
        crate::menu_page::mark_page_rows(&actions, |a| a.leads_to_page());
        self.network_menu_active = true;
        self.network_menu_actions = actions;
    }

    fn network_menu_open(&self) -> bool {
        cce_ui::widget::context_menu::is_visible() && self.network_menu_active
    }

    pub(crate) fn close_network_menu(&mut self) {
        cce_ui::widget::context_menu::hide();
        self.network_menu_active = false;
        self.network_menu_actions.clear();
    }

    /// Route a left press while the network menu is open — same contract as
    /// `handle_node_menu_click`. The menu is closed BEFORE the command runs,
    /// since Add Node opens the dialog and a menu still standing over it would
    /// be painted on top of the thing it asked for.
    fn handle_network_menu_click(&mut self) -> bool {
        if !self.network_menu_open() {
            return false;
        }
        if cce_ui::widget::context_menu::hit_test(self.cursor_x, self.cursor_y) {
            let idx = cce_ui::widget::context_menu::row_at(self.cursor_x, self.cursor_y);
            let picked = idx.and_then(|i| self.network_menu_actions.get(i).copied());
            // The rows that turn the menu (Add Node, Plate) were taken by
            // the press's page turn ahead of this.
            self.close_network_menu();
            match picked {
                Some(NetworkMenuAction::Command(id)) => {
                    self.run_command(id);
                }
                _ => {}
            }
            return true;
        }
        self.close_network_menu();
        false
    }

    /// Run a row of a node's menu: what a click on it runs.
    pub(crate) fn run_node_menu_action(&mut self, slot: usize, action: NodeMenuAction) {
        self.dispatch_node_menu(slot, action);
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
            NodeMenuAction::ToggleBypass => {
                self.set_bypassed(&[slot], !self.current_dir().children.get(slot).is_some_and(|n| n.bypassed));
            }
            NodeMenuAction::EditCurve => {
                self.toggle_viewer_state(slot);
            }
            NodeMenuAction::Rename => {
                self.open_rename_dialog(slot);
            }
            NodeMenuAction::Delete => {
                self.delete_node(slot);
            }
        }
    }

    /// The name a rename of `id` to `typed` would write, or why it would
    /// write none. A name is a segment of a path: it is sanitized as every
    /// name is, and a sibling's is refused, since wires are by name and
    /// two nodes of one name leave every wire to either naming both.
    pub fn rename_check(&self, id: &str, typed: &str) -> Result<String, String> {
        let node = crate::viewer_state::find_node_by_id(&self.fs_root, id).ok_or("The node is gone")?;
        if typed.trim().is_empty() {
            return Err("A node needs a name".to_string());
        }
        let new = sanitize_node_name(typed);
        if new == node.name {
            return Err(format!("{new} is its name already"));
        }
        let taken = crate::geometry::find_parent_node(&self.fs_root, id)
            .is_some_and(|p| p.children.iter().any(|c| c.id != id && c.name == new));
        if taken {
            return Err(format!("{new} is another node's name"));
        }
        Ok(new)
    }

    /// Rename the node `id`, and everything that names it with it: the
    /// wires, the expression paths anywhere in the tree, the active
    /// camera. The one entry the node menu, the command and MCP share.
    pub fn rename_node(&mut self, id: &str, typed: &str) -> Result<String, String> {
        let new = self.rename_check(id, typed)?;
        let old = crate::viewer_state::find_node_by_id(&self.fs_root, id).map(|n| n.name.clone()).unwrap_or_default();
        // The active camera is looked up where it stands, the root, and is
        // this node only if this node is there.
        let is_camera = self
            .camera_level()
            .children
            .iter()
            .any(|c| c.id == id && c.node_type == "camera" && c.name == self.active_camera);
        crate::geometry::rename_node_in_tree(&mut self.fs_root, id, &new);
        if is_camera {
            // Both copies of the name: the viewport routes the wheel by
            // its own.
            self.set_active_camera(new.clone());
        }
        self.sync_nodes();
        // Connections reference nodes by name (Input params), so a rename
        // changes downstream evaluation.
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
        Ok(format!("Renamed {old} to {new}"))
    }


pub(crate) fn geometry_to_spreadsheet_data(geom: &Detail) -> (Vec<String>, Vec<Vec<String>>) {
    // One row per POINT, not per triangle corner. The soup listed the same
    // place once for every face touching it — a sphere came to 2304 rows for
    // 362 places — and the row number meant nothing a user could point at.
    // It is now the point index, which is also what the Point Numbers overlay
    // draws.
    // The groups stand FIRST after the point's number, a column each,
    // `group:<name>`, 1 for a member and 0 for the rest. Until 2026-09-29
    // they were `g:` columns after every attribute — the thirteenth column
    // of a sphere's table, off the right of any pane — and blank for a
    // point not in the group, so a group of one point among five hundred
    // was a column that looked empty. Sorting the column, descending,
    // brings the members to the top.
    let groups = geom.points().group_names();
    let mut headers = vec!["Point".to_string()];
    headers.extend(groups.iter().map(|g| format!("group:{}", g)));
    headers.extend([
        "Pos.x".to_string(),
        "Pos.y".to_string(),
        "Pos.z".to_string(),
        "Col.r".to_string(),
        "Col.g".to_string(),
        "Col.b".to_string(),
    ]);

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

    // Each column's store looked up once, not once a row: a playing
    // simulation refills the table every frame, and at ten thousand points
    // the lookups and `format!` were most of a frame (see `fmt4`).
    let point_cols: Vec<(Option<&crate::detail::AttribData>, usize)> =
        attribs.iter().map(|(name, ty)| (geom.points().get(name), ty.components())).collect();
    let detail_cells: Vec<String> = {
        let mut cells = Vec::new();
        for (name, ty) in &detail {
            push_cells(&mut cells, geom.detail().value(name, 0), ty.components());
        }
        cells
    };
    let width = headers.len();
    let mut rows = Vec::with_capacity(geom.num_points());
    for p in 0..geom.num_points() {
        let pos = geom.positions()[p];
        let col = geom.color(p);
        let mut row = Vec::with_capacity(width);
        row.push(p.to_string());
        row.extend(groups.iter().map(|g| if geom.points().in_group(g, p) { "1" } else { "0" }.to_string()));
        row.extend([fmt4(pos[0]), fmt4(pos[1]), fmt4(pos[2]), fmt4(col[0]), fmt4(col[1]), fmt4(col[2])]);
        // A column covers its whole class, so there is no "this element
        // does not have it" case left to render as a dash.
        for (data, components) in &point_cols {
            push_cells(&mut row, data.and_then(|d| d.get(p)), *components);
        }
        row.extend(detail_cells.iter().cloned());
        rows.push(row);
    }

    (headers, rows)
}

    pub fn update_active_camera_rotation(&mut self, d_yaw: f32, d_pitch: f32) -> bool {
        if self.active_camera == "Default Camera" {
            return false;
        }
        let camera_name = self.active_camera.clone();
        let dir = self.camera_level_mut();
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
            let pos = node.params.iter().find(|p| p.name == "position")
                .and_then(|p| parse3(p.text()))
                .unwrap_or(Vec3::new(2.5, 1.8, 2.5));
            let piv = node.params.iter().find(|p| p.name == "pivot")
                .and_then(|p| parse3(p.text()))
                .unwrap_or(Vec3::ZERO);
            let offset = pos - piv;
            let pitch0_deg = (offset.y / offset.length().max(1e-5)).asin().to_degrees();
            let max_pitch_deg = crate::viewport_3d::Viewport3D::MAX_PITCH.to_degrees();

            if let Some(p) = node.params.iter_mut().find(|p| p.name == "rotation") {
                let mut rx = 0.0f32;
                let mut ry = 0.0f32;
                let mut rz = 0.0f32;
                if let Some(v) = parse3(p.text()) {
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
                p.set_text(format!("{:.2}:{:.2}:{:.2}", rx, ry, rz));

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
        let dir = self.camera_level_mut();
        if let Some(node) = dir.children.iter_mut().find(|c| c.node_type == "camera" && c.name == camera_name) {
            if let Some(p) = node.params.iter_mut().find(|p| p.name == "rotation") {
                p.set_text("0.00:0.00:0.00".to_string());
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

    /// Delete the node at `slot` of the current level, splicing it out of
    /// its chain: every sibling wire that named it is rewired to what it
    /// read through its `Input`, so deleting B from A → B → C leaves A → C.
    /// A node with no Input (a generator) leaves those wires as they were.
    /// The rewiring is part of the same undo step as the deletion, since a
    /// structure step holds the wires of every node it touches.
    pub fn delete_node(&mut self, slot: usize) -> bool {
        let len = self.current_dir().children.len();
        if slot < len {
            splice_out(self.current_dir_mut(), slot);
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

    /// Bypass the nodes at `slots` of the current level, or stop: the one
    /// way the flag changes, whichever of the key, the node's menu and MCP
    /// asked. Returns how many nodes it changed.
    pub fn set_bypassed(&mut self, slots: &[usize], bypassed: bool) -> usize {
        let mut changed = Vec::new();
        for &slot in slots {
            if let Some(node) = self.current_dir_mut().children.get_mut(slot) {
                if node.bypassed != bypassed {
                    node.bypassed = bypassed;
                    changed.push(node.name.clone());
                }
            }
        }
        if changed.is_empty() {
            return 0;
        }
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
        self.viewport_dirty = true;
        let what = if bypassed { "Bypassed" } else { "No longer bypassing" };
        self.update_status_text(&format!("{what} {}.", changed.join(", ")));
        changed.len()
    }

    /// One level's children as the graph widget's rows. What the widget
    /// reads of a row's parameters is its WIRES — every `node` parameter,
    /// typed `node`, the k-th into input port k ([`node_wires`]) — so that
    /// is what it is handed, and a node has as many input ports as it has
    /// wires where its template declared fewer (Relax's Rest, Collision's
    /// Collider, the Remesh's From).
    fn graph_nodes_of(root: &FsNode, dir: &FsNode, frame: i32) -> Vec<GraphNode> {
        dir.children
            .iter()
            .map(|c| {
                let wires = node_wires_at(root, c, frame);
                GraphNode {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    position: c.position,
                    inputs: c.inputs.max(wires.len()),
                    parameters: wires.into_iter().map(|(name, source)| (name, source, "node".to_string())).collect(),
                    geom_visible: c.geometry_visible,
                    node_type: c.node_type.clone(),
                    outputs: c.outputs,
                }
            })
            .collect()
    }

    /// The pointer connected `output` into input port `port` of the node
    /// `input_id` on the level at `path`: the port's wire is set. False when
    /// the node or the port is not there.
    pub(crate) fn connect_port(&mut self, path: &[usize], input_id: &str, output: String, port: usize) -> bool {
        let dir = self.dir_at_mut(path);
        let Some(child) = dir.children.iter_mut().find(|c| c.id == input_id) else { return false };
        // A node with no wire parameter at all (none that says so) takes
        // it as its Input, as a connection always did.
        let slot = child.params.iter().enumerate().filter(|(_, p)| p.kind() == ParamKind::Node).map(|(i, _)| i).nth(port)
            .or_else(|| child.params.iter().position(|p| p.name == "input"));
        let Some(slot) = slot else { return false };
        child.params[slot].set_text(output);
        self.sync_nodes();
        self.rebuild_scene_geometry();
        self.sync_parameters_pane();
        true
    }

    pub fn sync_nodes(&mut self) {
        let frame = self.sim_frame();
        let graph_nodes = Self::graph_nodes_of(&self.fs_root, self.current_dir(), frame);
        self.graph_mut().set_nodes(&graph_nodes);

        // The second network editor views ITS OWN level.
        self.clamp_path2();
        let nodes2 = Self::graph_nodes_of(&self.fs_root, self.dir_at(&self.current_path2.clone()), frame);
        use cce_ui::widget::GraphController as _;
        self.slots.content2.set_nodes(&nodes2);
        let names2 = self.path_names_at(&self.current_path2.clone());
        use cce_ui::widget::PathController as _;
        self.slots.breadcrumb2.set_path(&names2);

        let camera_nodes: Vec<String> = self.camera_level().children.iter()
            .filter(|c| c.node_type == "camera")
            .map(|c| c.name.clone())
            .collect();
        let mut items = vec!["Default Camera".to_string()];
        items.extend(camera_nodes);
        if !items.contains(&self.active_camera) {
            self.set_active_camera("Default Camera");
        }
        self.menu_mut(RIGHT_MENUBAR_IDX).set_menu_items(0, &items);
        for (i, item) in items.iter().enumerate() {
            let checked = item == &self.active_camera;
            self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(0, i, checked);
        }
        let path_strs = self.current_path_names();
        self.path_mut().set_path(&path_strs);

        self.sync_selection_readouts();
        self.sync_pull_arrows();
    }

    /// The readouts of what is SELECTED: the spreadsheet's rows and the
    /// selected Group's viewport markers. Each evaluates the selected node
    /// when its key moves, and the key is everything the answer depends on
    /// — the node and its parameters, the geometry version (which every
    /// scene rebuild bumps, so an edit anywhere upstream is in it) and the
    /// frame. Until 2026-09-29 the spreadsheet's key was the node and its
    /// OWN parameters alone, so it showed the frame and the upstream values
    /// it had been opened on until the selection itself was touched; and
    /// both ran from `sync_nodes` only, which a frame change does not call.
    /// From there and from the end of every scene rebuild now, as the pull
    /// arrows are.
    pub(crate) fn sync_selection_readouts(&mut self) {
        // The shared sim cache, taken out BEFORE the selected node is
        // borrowed off self and put back once that borrow is dead: the two
        // evaluations below are of whatever is selected, and when that is
        // a simnet, or anything downstream of one, the scene rebuild has
        // already solved this frame. Each used a throwaway cache until
        // 2026-09-29 — "the shared one cannot be reached from here" — so a
        // spreadsheet or a selected group downstream of a simulation solved
        // it again from the seed at every refresh, at frame 240 of the
        // project this was found on two and a half times what the frame
        // itself cost.
        let mut sim_cache = std::mem::take(&mut self.sim_cache);

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

        // What the rows were read at: a frame and a geometry version.
        let read_at = (sim_frame, self.rt_geometry_version);
        if let Some(node) = selected_node {
            current_name = Some(node.id.clone());
            current_params = Some(node.params.iter().map(|p| (p.name.clone(), p.text().to_string())).collect::<Vec<_>>());
            if self.last_spreadsheet_node_name == current_name
                && self.last_spreadsheet_node_params == current_params
                && self.last_spreadsheet_read_at == read_at
            {
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
            // Where each row's point is: a row is a point, by index.
            let mut points: Vec<[f32; 3]> = Vec::new();

            if let Some(node) = selected_node {
                let mut ocl_error = None;
                let mut sim = crate::geometry::EvalSim::new(sim_frame, sim_start, &mut sim_cache);
                if let Some(geom) = crate::geometry::node_geometry_as_shown(&self.fs_root, node, &mut ocl_error, &mut sim) {
                    let (h, r) = Self::geometry_to_spreadsheet_data(&geom);
                    headers = h;
                    rows = r;
                    points = geom.positions().to_vec();
                }
            }
            spreadsheet_update = Some((headers, rows, points));
        }

        // Selected-Group viewport markers: while the selection is a Group
        // node, evaluate it and stage a marker at every vertex it tags, so
        // selecting the node SHOWS the group in the viewport — independent of
        // the Highlight bake and of which node holds the display flag. The
        // geometry version keeps the key honest against upstream edits (the
        // scene rebuild bumps it); a non-group selection clears the markers.
        // The version and the frame both: a frame change rebuilds the
        // scene, and bumps the version, only when the graph holds a simnet.
        let group_key = selected_node.filter(|n| n.node_type.eq_ignore_ascii_case("group")).map(|n| {
            (
                n.id.clone(),
                n.params.iter().map(|p| (p.name.clone(), p.text().to_string())).collect::<Vec<_>>(),
                self.rt_geometry_version.wrapping_add((sim_frame as u64).wrapping_mul(0x9E3779B97F4A7C15)),
            )
        });
        let mut group_update = None;
        if group_key != self.last_group_points_key {
            let mut member_verts = Vec::new();
            if let Some(node) = selected_node.filter(|n| n.node_type.eq_ignore_ascii_case("group")) {
                let group_name = node_param_str(node, "group_name", "group1");
                let mut ocl_error = None;
                let mut sim = crate::geometry::EvalSim::new(sim_frame, sim_start, &mut sim_cache);
                if let Some(geom) = crate::geometry::node_geometry_as_shown(&self.fs_root, node, &mut ocl_error, &mut sim) {
                    member_verts = crate::geometry::group_member_positions(&geom, &group_name);
                }
            }
            group_update = Some(member_verts);
        }
        // `selected_node` is not read past here.
        self.sim_cache = sim_cache;

        if let Some((headers, rows, points)) = spreadsheet_update {
            // The selection is of rows by index, which are points of ONE
            // node's output: it stands across a frame or an edit, and goes
            // when the table becomes another node's.
            if current_name != self.last_spreadsheet_node_name {
                self.spreadsheet_mut().set_selected_rows(&[]);
            }
            self.spreadsheet_mut().set_spreadsheet_data(headers, rows);
            self.spreadsheet_points = points;
            self.rebuild_row_markers();
            self.last_spreadsheet_node_name = current_name;
            self.last_spreadsheet_node_params = current_params;
            self.last_spreadsheet_read_at = read_at;
        }
        if let Some(members) = group_update {
            self.group_members = members;
            self.last_group_points_key = group_key;
            self.rebuild_group_markers();
        } else if (self.group_marker_size - self.last_group_marker_size).abs() > f32::EPSILON {
            // Same members, new size (the palette's Group Marker Size):
            // re-size without re-evaluating.
            self.rebuild_group_markers();
        }
    }

    /// How many points of a pull get an arrow. A dozen show the direction
    /// and the reach of the pull across the region it covers; one per point
    /// buries the mesh under them.
    pub const PULL_ARROWS_MAX: usize = 12;

    /// Stage the pull arrows for the node the params pane shows, when it is
    /// an Attribute node that moves points; clear them otherwise. Evaluates
    /// only when the key moves — a different node, an edited parameter, or
    /// a new geometry version (which every scene rebuild, and so every frame
    /// of a playing simnet, bumps).
    ///
    /// The shared sim cache is borrowed for the evaluation rather than a
    /// throwaway one: the scene rebuild has just solved this frame, so the
    /// step feedback a node inside a simnet needs is a cache hit instead of
    /// a solve from the seed on every frame of playback.
    pub(crate) fn sync_pull_arrows(&mut self) {
        let node = if self.is_detached_network {
            None
        } else {
            self.param_editor_selected()
                .and_then(|slot| self.param_editor_dir().children.get(slot))
                .filter(|n| crate::geometry::moves_points(n))
                .cloned()
        };
        let key = node.as_ref().map(|n| {
            (
                n.id.clone(),
                n.params.iter().map(|p| (p.name.clone(), p.text().to_string())).collect::<Vec<_>>(),
                self.rt_geometry_version,
            )
        });
        if key == self.last_pull_arrows_key {
            return;
        }
        self.last_pull_arrows_key = key;
        self.pull_arrow_pairs = match node {
            None => Vec::new(),
            Some(node) => {
                let (frame, start) = (self.sim_frame(), self.sim_start_frame());
                let mut cache = std::mem::take(&mut self.sim_cache);
                let moved = {
                    let mut sim = crate::geometry::EvalSim::new(frame, start, &mut cache);
                    crate::geometry::point_displacements(&self.fs_root, &node, &mut sim)
                };
                self.sim_cache = cache;
                let bases: Vec<glam::Vec3> = moved.iter().map(|(a, _)| *a).collect();
                crate::geometry::spread_sample(&bases, Self::PULL_ARROWS_MAX)
                    .into_iter()
                    .map(|i| moved[i])
                    .collect()
            }
        };
        self.rebuild_pull_arrow_verts();
    }

    /// Build the pull arrows from the kept pairs at the current Pull Arrow
    /// Scale — the cheap half, with no evaluation, so the menu slider can run
    /// it on every motion of a drag. Each arrow keeps its base on the point's
    /// position before the pull and stretches along the pull by the scale.
    pub(crate) fn rebuild_pull_arrow_verts(&mut self) {
        let scale = self.pull_arrow_scale;
        let shown: Vec<(glam::Vec3, glam::Vec3)> =
            self.pull_arrow_pairs.iter().map(|&(a, b)| (a, a + (b - a) * scale)).collect();
        // The group markers' warm accent: both are "what the selected node
        // does", and they read as one feature.
        self.pull_arrow_verts =
            crate::geometry::arrow_vertices(&shown, cce_ui::colors::to_linear_rgb([1.0, 0.78, 0.20]));
        self.pull_arrows_dirty = true;
    }

    /// Works out how much of each point number shows through the fill, for
    /// the eye the stage pass is staging (`eye` in mesh space, the space of
    /// the labels and the triangles).
    pub(crate) fn sync_point_number_alpha(&mut self, mvp: Mat4, eye: Vec3) {
        // Asked for the same view of the same scene twice in a frame — by
        // the 2D paint and again by the stage pass — and worked out once.
        let key = (
            mvp.to_cols_array().map(f32::to_bits),
            self.rt_geometry_version,
            self.geo_opacity.to_bits(),
            self.see_through_active(),
            self.overlay_number_labels.len() + self.overlay_prim_labels.len() + self.overlay_vertex_labels.len(),
        );
        let worked_out = self.overlay_number_alpha.len() == self.overlay_number_labels.len()
            && self.overlay_prim_alpha.len() == self.overlay_prim_labels.len()
            && self.overlay_vertex_alpha.len() == self.overlay_vertex_labels.len();
        if self.number_alpha_key == Some(key) && worked_out {
            return;
        }
        self.number_alpha_key = Some(key);
        // One pass over the three lists, so the mesh is binned once.
        let (points_n, prims_n) = (self.overlay_number_labels.len(), self.overlay_prim_labels.len());
        let at: Vec<[f32; 3]> = self
            .overlay_number_labels
            .iter()
            .chain(&self.overlay_prim_labels)
            .chain(&self.overlay_vertex_labels)
            .map(|(p, _)| *p)
            .collect();
        if at.is_empty() {
            self.overlay_number_alpha.clear();
            self.overlay_prim_alpha.clear();
            self.overlay_vertex_alpha.clear();
            return;
        }
        let mut alpha = crate::geometry::point_transmittance(
            &self.rt_sphere_verts,
            mvp,
            eye,
            &at,
            self.geo_opacity,
            self.see_through_active(),
        );
        self.overlay_vertex_alpha = alpha.split_off(points_n + prims_n);
        self.overlay_prim_alpha = alpha.split_off(points_n);
        self.overlay_number_alpha = alpha;
    }

    /// Build the marker overlays' instances from the kept scene positions,
    /// without the evaluation that produced the positions: the points' in
    /// the marker colour, and the vertices' in the vertex overlays' green —
    /// drawn over a sphere `VERTEX_MARKER_SCALE` the size of the points',
    /// so that a point's marker is not lost among the markers of the
    /// vertices around it. A SIZE change needs none of this: the instances
    /// stand where they stood, and the flush re-uploads the sphere they are
    /// drawn over (`marker_sphere_radii`).
    pub(crate) fn rebuild_overlay_markers(&mut self) {
        self.overlay_marker_instances = crate::geometry::marker_instances(
            &self.overlay_marker_points,
            cce_ui::colors::to_linear_rgb(self.point_marker_color),
        );
        self.vertex_marker_instances = crate::geometry::marker_instances(
            &self.overlay_vertex_marker_points,
            cce_ui::colors::to_linear_rgb(crate::render::VERTEX_LABEL_COLOR.map(|c| c as f32 / 255.0)),
        );
        self.overlay_dirty = true;
    }

    /// What the marker draws show, as vertices: each kind's instances over
    /// its sphere at its size — what the renderer draws, expanded. For the
    /// tests, which read a marker's size and place.
    #[cfg(test)]
    pub(crate) fn drawn_markers(&self, kind: MarkerKind) -> Vec<Vertex3D> {
        use crate::geometry::{expand_instances, marker_sphere};
        let group = || marker_sphere(self.group_marker_size);
        match kind {
            MarkerKind::Group => expand_instances(&group(), &self.group_point_instances),
            MarkerKind::Row => expand_instances(&group(), &self.row_marker_instances),
            MarkerKind::Marked => expand_instances(&group(), &self.marked_group_instances),
            MarkerKind::Overlay => {
                let mut out = expand_instances(&marker_sphere(self.point_marker_size), &self.overlay_marker_instances);
                out.extend(expand_instances(
                    &marker_sphere(self.point_marker_size * crate::render::VERTEX_MARKER_SCALE),
                    &self.vertex_marker_instances,
                ));
                out
            }
        }
    }

    /// Stage a marker on the point of every row selected in the
    /// spreadsheet, at the group markers' size and in the highlight colour
    /// the rows themselves wear. From the positions the table was filled
    /// from, so a press on a row evaluates nothing.
    pub(crate) fn rebuild_row_markers(&mut self) {
        let _ = self.spreadsheet_mut().take_selection_change();
        let rows = self.spreadsheet_mut().selected_rows();
        let at: Vec<Vertex3D> = rows
            .iter()
            .filter_map(|&r| self.spreadsheet_points.get(r))
            .map(|&position| Vertex3D { position, color: [0.0; 3] })
            .collect();
        let [r, g, b, _] = cce_ui::colors::highlight_primary_color();
        self.row_marker_instances = crate::geometry::marker_instances(&at, cce_ui::colors::to_linear_rgb([r, g, b]));
        self.row_markers_dirty = true;
        self.viewport_dirty = true;
    }

    /// The points whose rows are selected in the spreadsheet, by index.
    pub(crate) fn selected_spreadsheet_points(&mut self) -> Vec<usize> {
        self.spreadsheet_mut().selected_rows()
    }

    /// Stage a marker on every member of every marked group, at Group
    /// Marker Size in the group markers' amber, from the positions the
    /// last scene rebuild kept — a switch flipped evaluates nothing.
    pub(crate) fn rebuild_marked_group_markers(&mut self) {
        let at: Vec<Vertex3D> = self
            .scene_groups
            .iter()
            .filter(|(name, _)| self.marked_groups.contains(name))
            .flat_map(|(_, members)| members.iter().map(|&position| Vertex3D { position, color: [0.0; 3] }))
            .collect();
        self.marked_group_instances = crate::geometry::marker_instances(&at, cce_ui::colors::to_linear_rgb([1.0, 0.78, 0.20]));
        self.marked_groups_dirty = true;
        self.viewport_dirty = true;
    }

    /// The marked groups as the settings hold them: names joined by commas.
    pub fn join_marked_groups(groups: &[String]) -> String {
        groups.join(",")
    }

    /// The marked groups out of the settings' one string.
    pub fn marked_groups_of(joined: &str) -> Vec<String> {
        let mut out: Vec<String> = joined.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Whether `group` is marked.
    pub fn group_marked(&self, group: &str) -> bool {
        self.marked_groups.iter().any(|g| g == group)
    }

    /// Mark or unmark a point group: the Group Markers dialog's switch.
    /// Persisted with the display settings, which the project file carries
    /// too.
    pub fn set_group_marked(&mut self, group: &str, on: bool) {
        let was = self.group_marked(group);
        if on && !was {
            self.marked_groups.push(group.to_string());
            self.marked_groups.sort();
        } else if !on && was {
            self.marked_groups.retain(|g| g != group);
        }
        if on != was {
            self.rebuild_marked_group_markers();
            self.save_settings();
        }
    }

    pub(crate) fn rebuild_group_markers(&mut self) {
        // One size for every kind of marker on a group's members.
        self.rebuild_row_markers();
        self.rebuild_marked_group_markers();
        self.group_point_instances =
            crate::geometry::marker_instances(&self.group_members, cce_ui::colors::to_linear_rgb([1.0, 0.78, 0.20]));
        self.group_points_dirty = true;
        self.last_group_marker_size = self.group_marker_size;
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
                    proj.sanitize_node_names();
                    proj.migrate_format();
                    merge_template_defs(&mut proj.root, &node_templates);
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
                bypassed: false,
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
            right_menubar: MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("1: Viewport").with_label("Viewport Menu Bar").with_item("Camera", &["Perspective", "Orthographic"]).with_item("Display", &["square_aspect"]).with_item("Guides", &["Show Grid", "Origin", "Camera Pivot"]).with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 1),
            param_menubar: MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("2: Parameters").with_label("Parameters Menu Bar").with_item("Preset", &["Default"]).with_item("Reset", &["All"]).with_item("View", &["Close Pane"]).with_context_options(context_opts.clone(), 2),
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
            dialog: crate::dialog::Dialog::new(),
        });

        slots.playbar.inner_mut().repeat = settings.playbar_repeat;
        slots.playbar.inner_mut().step_buttons = settings.playbar_step_buttons;
        let wire_style = cce_ui::widget::display::WireStyle::parse(&settings.viewport.node_wire_style);
        slots.content.inner_mut().set_wire_style(wire_style);
        slots.content2.inner_mut().set_wire_style(wire_style);
        // A node dropped on a node swaps places with it, connections and all
        // (`swap_places`).
        slots.content.inner_mut().set_swap_on_drop(true);
        slots.content2.inner_mut().set_swap_on_drop(true);
        slots.playbar.inner_mut().fps = settings.playbar_fps.clamp(1.0, 120.0);
        if let Some(viewport) = slots.viewport.as_any_mut().downcast_mut::<Viewport3D>() {
            viewport.show_grid = settings.viewport.show_grid_enabled;
            viewport.show_origin = settings.viewport.show_origin_enabled;
            viewport.show_camera_pivot = settings.viewport.show_camera_pivot_enabled;
            viewport.bg_color = settings.viewport.bg_color;
            viewport.grid_color = settings.viewport.grid_color;
            viewport.rt_mode = settings.viewport.rt_mode;
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
            graph_grid_color: cce_ui::color::graph_grid_color(),
            origin_size: settings.viewport.origin_size,
            camera_pivot_size: settings.viewport.camera_pivot_size,
            fs_root: fs_root.clone(),
            node_templates,
            current_path,
            current_path2: Vec::new(),
            param_editor: CONTENT_IDX,
            param_pane_source: None,
            viewport_pin: None,
            params_pin: None,
            spreadsheet_pin: None,
            node_clipboard: Vec::new(),
            last_click: None,
            shortcut_manager,
            pending_command: None,
            exit_requested: false,
            event_sender: None,
            slots,
            positions,
            splitter_layout,
            node_menu_slot: None,
            node_menu_actions: Vec::new(),
            viewport_menu_active: false,
            viewport_menu_actions: Vec::new(),
            viewport_menu_page: None,
            dialog_from: None,
            dialog_trail: Vec::new(),
            plate_page_from: None,
            plate_page_root: None,
            param_menu_active: false,
            param_menu_actions: Vec::new(),
            playbar_menu_active: false,
            playbar_menu_actions: Vec::new(),
            param_menu_target: None,
            copied_param: None,
            rename_target: None,
            edit_history: Default::default(),
            structure_base: None,
            network_menu_active: false,
            network_menu_actions: Vec::new(),
            network_menu_from: None,
            network_menu_cell: None,
            overlay_wheel: None,
            sim_cache: crate::geometry::SimCache::default(),
            playbar_cache_key: None,
            page_image: None,
            page_shown: None,
            page_composed: None,
            seen_renderer: false,
            deselected_cell: None,
            orbit_drag: None,
            pan_drag: None,
            pan_exact: None,
            page_dirty: false,
            title_dirty: false,
            settings_save_pending: false,
            last_sim_frame: i32::MIN,
            plate_menu_slot: None,
            plate_menu_actions: Vec::new(),
            collapsed_panes: [false; WIDGET_COUNT],
            // The right dock starts empty: the params HUD is not a dock
            // pane, and a plate moved there is drawn over it.
            dock_panes: [NETWORK_PANEL_IDX, NO_PANE, SPREADSHEET_IDX],
            dock_tabs: [
                vec![NETWORK_PANEL_IDX],
                Vec::new(),
                vec![SPREADSHEET_IDX],
            ],
            drag_widget: None,
            drag_press_cursor: None,
            focused_widget: None,
            cursor_x: 0.0,
            cursor_y: 0.0,
            grid_cursor_col: 0,
            grid_cursor_row: 0,
            grid_cursor_expanse: None,
            grid_cursor_drag: None,
            node_drag_group: None,
            modifiers: ModifiersState::default(),
            width: lw,
            height: lh,
            physical_width: pw,
            physical_height: ph,
            scale,
            square_viewport: settings.viewport.square,
            grid_snap_enabled: true,
            network_grid_visible: true,
            grid_base: cfg_grid,
            grid_pitch_x: cfg_grid.pitch_x,
            grid_pitch_y: cfg_grid.pitch_y,
            node_w: cfg_grid.node_w,
            node_h: cfg_grid.node_h,
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
            params_plate: settings.viewport.params_plate,
            show_viewport: true,
            show_parameters: true,
            show_spreadsheet: false,
            show_playbar: false,
            last_spreadsheet_node_name: None,
            last_spreadsheet_node_params: None,
            last_spreadsheet_read_at: (i32::MIN, 0),
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
            circular_network_pane: is_detached_network || settings.viewport.circular_pane,
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
            params_plate_shown: None,
            network_opacity: 0.95,
            node_opacity: 1.0,
            node_compression: None,
            node_tint: None,
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
            params_hud_width: 300.0,
            floating_spreadsheet_height: 250.0,
            floating_spreadsheet_inset_left: 0.0,
            floating_spreadsheet_inset_right: 0.0,
            window_configured: false,
            pending_plates: None,
            loaded_project_path: None,
            default_project_setting: settings.default_project.clone(),
            gpu_preference: settings.gpu.clone(),
            gpu_at_launch: std::env::var("CCE_VK_DEVICE")
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| v.to_lowercase())
                .unwrap_or_else(default_gpu),
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
            last_viewport_show_origin: false,
            last_viewport_show_camera_pivot: false,
            last_viewport_width: 0,
            last_viewport_height: 0,
            last_viewport_active_camera: String::new(),
            last_viewport_show_viewport: false,
            wireframe: settings.render.wireframe,
            last_viewport_wireframe: false,
            wire_single_color: settings.render.wire_single_color,
            wire_color: settings.render.wire_color,
            wire_opacity: settings.render.wire_opacity,
            wire_width: settings.render.wire_width,
            last_viewport_wire_single_color: false,
            last_viewport_wire_color: [1.0, 1.0, 1.0],
            last_viewport_wire_opacity: 1.0,
            last_viewport_wire_width: 1.0,
            geo_opacity: settings.render.geo_opacity,
            last_viewport_geo_opacity: 1.0,
            group_marker_size: settings.render.group_marker_size,
            pull_arrow_scale: settings.render.pull_arrow_scale,
            pull_arrow_pairs: Vec::new(),
            smooth_shading: settings.render.smooth_shading,
            show_occluded: settings.render.show_occluded,
            sorted_fill_key: None,
            scene_smooth_verts: Vec::new(),
            environment: crate::environment::Environment::default(),
            group_point_instances: Vec::new(),
            spreadsheet_points: Vec::new(),
            row_marker_instances: Vec::new(),
            row_markers_dirty: false,
            row_marker_count: 0,
            group_points_dirty: false,
            group_point_count: 0,
            last_group_points_key: None,
            pull_arrow_verts: Vec::new(),
            pull_arrows_dirty: false,
            pull_arrow_count: 0,
            last_pull_arrows_key: None,
            group_members: Vec::new(),
            last_group_marker_size: 0.0,
            overlay_marker_instances: Vec::new(),
            overlay_marker_points: Vec::new(),
            overlay_vertex_marker_points: Vec::new(),
            overlay_dirty: false,
            vertex_marker_instances: Vec::new(),
            vertex_marker_count: 0,
            marker_sphere_radii: None,
            overlay_point_count: 0,
            overlay_number_labels: Vec::new(),
            overlay_number_alpha: Vec::new(),
            overlay_prim_labels: Vec::new(),
            overlay_prim_alpha: Vec::new(),
            overlay_vertex_labels: Vec::new(),
            overlay_vertex_alpha: Vec::new(),
            overlay_normal_verts: Vec::new(),
            overlay_normal_count: 0,
            show_point_markers: settings.viewport.show_point_markers,
            show_point_numbers: settings.viewport.show_point_numbers,
            show_point_normals: settings.viewport.show_point_normals,
            show_prim_numbers: settings.viewport.show_prim_numbers,
            show_prim_normals: settings.viewport.show_prim_normals,
            show_vertex_numbers: settings.viewport.show_vertex_numbers,
            show_vertex_markers: settings.viewport.show_vertex_markers,
            show_vertex_normals: settings.viewport.show_vertex_normals,
            marked_groups: Self::marked_groups_of(&settings.viewport.marked_groups),
            visualizers: crate::visualizer::decode(&settings.viewport.visualizers),
            vis_hud: None,
            vis_hud_from: None,
            value_row_span: None,
            float2_spans: None,
            scene_attributes: Vec::new(),
            scene_base: None,
            scene_groups: Vec::new(),
            marked_group_instances: Vec::new(),
            marked_groups_dirty: false,
            marked_group_count: 0,
            scene_edge_verts: Vec::new(),
            point_marker_size: settings.viewport.point_marker_size,
            point_marker_color: settings.viewport.point_marker_color,
            world_unit: cce_ui::units::Unit::parse(&settings.viewport.world_unit)
                .unwrap_or(cce_ui::units::Unit::Mm),
            pick_cache: None,
            last_scene_mvp: None,
            number_alpha_key: None,
            last_scene_eye: Vec3::ZERO,
            last_scene_view_rect: (0.0, 0.0, 0.0, 0.0),
            viewer_tool: None,
            last_viewport_rt_mode: false,
            rt_sphere_verts: Vec::new(),
            rt_geometry_version: 0,
            last_rt_scene_key: None,
            page_version: 0,
            ui_context: cce_ui::context::UiContext::new(),
        };

        state.update_inertial_settings();
        state.update_graph_settings_from_config();
        state.update_window_title();
        colors::set_node_color(state.node_color);
        state.migrate_meta_settings_node();
        state.sync_nodes();
        state.rebuild_scene_geometry();
        state.sync_grid_settings();
        let sg = state.viewport().show_grid;
        let so = state.viewport().show_origin;
        let cp = state.viewport().show_camera_pivot;
        let cnp = state.circular_network_pane;
        let dcn = state.detached_circular_network;
        let sn = state.show_network;
        let sv = state.show_viewport;
        let sp = state.show_parameters;
        let ss = state.show_spreadsheet;

        state.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(GUIDES_MENU, GUIDE_GRID, sg);
        state.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(GUIDES_MENU, GUIDE_ORIGIN, so);
        state.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(GUIDES_MENU, GUIDE_CAMERA_PIVOT, cp);
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

    /// The node body's size at the current zoom — what `sync_grid_settings`
    /// hands the widget, so the cursor drawn here and the nodes the widget
    /// draws cannot disagree.
    pub fn node_size(&self) -> (f32, f32) {
        (self.node_w, self.node_h)
    }

    /// Put the grid at a geometry outright (Reset Zoom, Frame All's fit).
    pub fn set_grid_geometry(&mut self, g: GridGeometry) {
        self.grid_pitch_x = g.pitch_x;
        self.grid_pitch_y = g.pitch_y;
        self.node_w = g.node_w;
        self.node_h = g.node_h;
    }

    /// Scale the grid geometry by one factor — the pitch and the node body
    /// together, which is the only way the two are related.
    pub fn scale_grid_geometry(&mut self, f: f32) {
        self.grid_pitch_x *= f;
        self.grid_pitch_y *= f;
        self.node_w *= f;
        self.node_h *= f;
    }

    /// The window-space centre of lattice cell (col, row) in the network
    /// pane: the pane origin, plus the pan, plus the cell's pitches. This is
    /// where a node at that position is centred, and what the grid origin
    /// handed to the widget means.
    pub fn cell_center(&self, col: i32, row: i32) -> (f32, f32) {
        let (px, py, _, _) = self.positions[CONTENT_IDX];
        (
            px + self.pan_x + col as f32 * self.grid_pitch_x,
            py + self.pan_y + row as f32 * self.grid_pitch_y,
        )
    }

    /// The window-space rect a node body occupies on lattice cell
    /// (col, row) — centred on the intersection.
    pub fn cell_rect(&self, col: i32, row: i32) -> (f32, f32, f32, f32) {
        let (cx, cy) = self.cell_center(col, row);
        let (w, h) = self.node_size();
        (cx - w * 0.5, cy - h * 0.5, w, h)
    }

    /// The lattice cell nearest a window-space point in the network pane.
    /// Nearest, not "the cell whose span contains it": a cell is centred on
    /// its intersection, so a click between two nodes belongs to the closer.
    pub fn cell_at(&self, x: f32, y: f32) -> (i32, i32) {
        let (px, py, _, _) = self.positions[CONTENT_IDX];
        let col = if self.grid_pitch_x > 0.0 { ((x - px - self.pan_x) / self.grid_pitch_x).round() } else { 0.0 };
        let row = if self.grid_pitch_y > 0.0 { ((y - py - self.pan_y) / self.grid_pitch_y).round() } else { 0.0 };
        (col as i32, row as i32)
    }

    /// The lattice region the cursor covers: `(col, row, cols, rows)`, never
    /// smaller than one cell. The expanse a drag left behind counts only
    /// while its anchor is still where the cursor is — see
    /// [`grid_cursor_expanse`](State::grid_cursor_expanse).
    pub fn grid_cursor_region(&self) -> (i32, i32, i32, i32) {
        let anchor = (self.grid_cursor_col, self.grid_cursor_row);
        let far = match self.grid_cursor_expanse {
            Some((a, far)) if a == anchor => far,
            _ => anchor,
        };
        let (c0, c1) = (anchor.0.min(far.0), anchor.0.max(far.0));
        let (r0, r1) = (anchor.1.min(far.1), anchor.1.max(far.1));
        (c0, r0, c1 - c0 + 1, r1 - r0 + 1)
    }

    /// The window-space rect the cursor outline is drawn on: the union of the
    /// region's corner cells, which for the usual one-cell cursor is exactly
    /// `cell_rect` of it.
    pub fn grid_cursor_rect(&self) -> (f32, f32, f32, f32) {
        let (col, row, cols, rows) = self.grid_cursor_region();
        let (x0, y0, _, _) = self.cell_rect(col, row);
        let (x1, y1, w, h) = self.cell_rect(col + cols - 1, row + rows - 1);
        (x0, y0, x1 - x0 + w, y1 - y0 + h)
    }

    /// Whether the cursor's region stands on lattice cell `(col, row)`.
    pub fn grid_cursor_covers(&self, col: i32, row: i32) -> bool {
        let (c, r, cols, rows) = self.grid_cursor_region();
        col >= c && col < c + cols && row >= r && row < r + rows
    }

    /// Whether the cursor is expanded past its one cell — what makes a
    /// selection a MULTIPLE selection, and the one test the paint and the
    /// operations share.
    pub fn grid_cursor_expanded(&self) -> bool {
        let (_, _, cols, rows) = self.grid_cursor_region();
        cols > 1 || rows > 1
    }

    /// The nodes the cursor selects, as slots in the current level, in slot
    /// order.
    ///
    /// One cell — the ordinary cursor — defers to the graph's own
    /// `selected_node`, so nothing about a single selection changes here:
    /// that one answer already carries the deselect memory, a click that
    /// arrived from another pane, and a selection made while the network was
    /// not focused. An EXPANDED cursor names every node standing inside it
    /// instead; its anchor is empty grid by construction (a press on a node
    /// drags the node), so there is no single selection to defer to.
    pub fn selected_slots(&self) -> Vec<usize> {
        if !self.grid_cursor_expanded() {
            return self.graph().selected_node().into_iter().collect();
        }
        self.current_dir()
            .children
            .iter()
            .enumerate()
            .filter(|(_, c)| self.grid_cursor_covers(c.position.0 as i32, c.position.1 as i32))
            .map(|(i, _)| i)
            .collect()
    }

    /// Move the cursor AND the region it carries by whole cells — what a
    /// selection being dragged across the sheet needs, since stepping the
    /// anchor alone is precisely the thing that collapses the region.
    pub fn shift_grid_cursor(&mut self, dc: i32, dr: i32) {
        self.grid_cursor_col += dc;
        self.grid_cursor_row += dr;
        if let Some((anchor, far)) = self.grid_cursor_expanse {
            self.grid_cursor_expanse =
                Some(((anchor.0 + dc, anchor.1 + dr), (far.0 + dc, far.1 + dr)));
        }
    }

    /// Grow the live expansion drag to the cell under the cursor. `true` when
    /// the region actually changed, which is the redraw.
    fn grid_cursor_drag_motion(&mut self) -> bool {
        let Some(anchor) = self.grid_cursor_drag else { return false };
        let far = self.cell_at(self.cursor_x, self.cursor_y);
        let next = (anchor != far).then_some((anchor, far));
        if self.grid_cursor_expanse == next {
            return false;
        }
        self.grid_cursor_expanse = next;
        true
    }

    pub fn sync_grid_settings(&mut self) {
        let active_node_area_y = self.positions[CONTENT_IDX].1;
        let active_node_area_x = self.positions[CONTENT_IDX].0;

        let network_grid_visible = self.network_grid_visible;
        let grid_pitch_x = self.grid_pitch_x;
        let grid_pitch_y = self.grid_pitch_y;
        let (node_w, node_h) = self.node_size();
        let pan_x = self.pan_x;
        let pan_y = self.pan_y;
        let grid_snap_enabled = self.grid_snap_enabled;
        let graph = self.graph_mut();
        graph.set_show_network_grid(network_grid_visible);
        graph.set_grid_pitch(grid_pitch_x, grid_pitch_y);
        graph.set_node_size(node_w, node_h);
        graph.set_grid_origin(active_node_area_x + pan_x, active_node_area_y + pan_y);
        graph.set_grid_snap_enabled(grid_snap_enabled);
        if let Some(graph) = self.slots.content.as_any_mut().downcast_mut::<cce_ui::widget::Graph>() {
            graph.set_network_opacity(self.network_opacity);
            graph.set_node_opacity(self.node_opacity);
            graph.set_grid_color(self.graph_grid_color);
        }
        if let Some(menubar) = self.slots.left_menubar.as_any_mut().downcast_mut::<cce_ui::widget::MenuBar>() {
            menubar.set_network_opacity(self.network_opacity);
        }
        if let Some(breadcrumb) = self.slots.breadcrumb.as_any_mut().downcast_mut::<cce_ui::widget::Breadcrumb>() {
            breadcrumb.set_network_opacity(self.network_opacity);
        }

        // The second editor's graph reads the SAME display settings but its
        // OWN rect as origin (no pan of its own yet — the rect is the view).
        let (qx, qy, _, _) = self.positions[crate::slots::CONTENT2_IDX];
        {
            use cce_ui::widget::GraphController as _;
            let g2 = &mut *self.slots.content2;
            g2.set_show_network_grid(network_grid_visible);
            g2.set_grid_pitch(grid_pitch_x, grid_pitch_y);
            g2.set_node_size(node_w, node_h);
            g2.set_grid_origin(qx, qy);
            g2.set_grid_snap_enabled(grid_snap_enabled);
        }
        if let Some(g2) = self.slots.content2.as_any_mut().downcast_mut::<cce_ui::widget::Graph>() {
            g2.set_network_opacity(self.network_opacity);
            g2.set_node_opacity(self.node_opacity);
            g2.set_grid_color(self.graph_grid_color);
        }
        if let Some(bc2) = self.slots.breadcrumb2.as_any_mut().downcast_mut::<cce_ui::widget::Breadcrumb>() {
            bc2.set_network_opacity(self.network_opacity);
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
        // The viewport's orbit and zoom are ScrollMotions too: they take the
        // speed and the on/off switch from here, and how long a coast runs
        // from input.kdl's `scroll_friction`, as the network pan does.
        self.graph_scroll_speed = speed;
        self.graph_inertial_scroll = enabled;
        self.graph_scroll_friction = friction;
        self.viewport_mut().scroll_speed = speed;
        self.viewport_mut().inertial_scroll = enabled;
    }

    pub fn update_graph_settings_from_config(&mut self) {
        cce_ui::layout::reload_config();
        
        let opacity = cce_ui::color::graph_opacity();
        let node_opacity = cce_ui::color::graph_node_opacity();
        let graph_grid_color = cce_ui::color::graph_grid_color();
        let node_color = cce_ui::color::graph_node_color();
        let node_compression = cce_ui::config::cached_config()
            .pointer("/style/surface/graph/node_compression")
            .and_then(|v| v.as_f64())
            .map(|k| (k as f32).clamp(0.0, 1.0));
        let node_tint = cce_ui::config::cached_config()
            .pointer("/style/surface/graph/node_tint")
            .and_then(|v| v.as_str())
            .and_then(cce_ui::color::parse_hex_rgba_linear);

        let mut changed = false;

        if self.node_tint != node_tint {
            self.node_tint = node_tint;
            changed = true;
        }

        if self.node_compression != node_compression {
            self.node_compression = node_compression;
            changed = true;
        }
        if (self.network_opacity - opacity).abs() > 0.001 {
            self.network_opacity = opacity;
            changed = true;
        }
        if (self.node_opacity - node_opacity).abs() > 0.001 {
            self.node_opacity = node_opacity;
            changed = true;
        }
        if self.graph_grid_color != graph_grid_color {
            self.graph_grid_color = graph_grid_color;
            changed = true;
        }
        if self.node_color != node_color {
            self.node_color = node_color;
            colors::set_node_color(node_color);
            changed = true;
        }
        // `graph_grid_snap` is not read: a dragged node always snaps to a
        // cell it can land on (`grid_snap_enabled`, always on — since
        // 2026-10-06; the config's switch, off on the user's machine, had a
        // dragged node float freely and land somewhere else).
        // The grid's spacing and node size: re-applied at the zoom in hand,
        // so a config.kdl edit to `spacing_y` shows at once. Until
        // 2026-10-06 the live geometry was set at startup and only zoomed
        // after it, so an edit waited for Reset Zoom or a restart.
        let base = configured_grid_geometry();
        if base != self.grid_base {
            let zoom = if self.grid_base.pitch_x > 0.0 { self.grid_pitch_x / self.grid_base.pitch_x } else { 1.0 };
            self.set_grid_geometry(GridGeometry {
                pitch_x: base.pitch_x * zoom,
                pitch_y: base.pitch_y * zoom,
                node_w: base.node_w * zoom,
                node_h: base.node_h * zoom,
            });
            self.grid_base = base;
            changed = true;
        }

        if changed {
            self.sync_grid_settings();
            self.viewport_dirty = true;
        }
    }



    /// Zoom the network view about a window-space point (the cursor cell's
    /// centre when none is given): the pitch scales, and the pan moves so the
    /// lattice point under the anchor stays under it.
    pub fn zoom(&mut self, factor: f32, center: Option<(f32, f32)>) {
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        let old_px = self.grid_pitch_x;
        let old_py = self.grid_pitch_y;

        // The x pitch is clamped and the y pitch follows by the SAME factor,
        // so the node body — scaled by that factor too — keeps its shape.
        let new_px = (old_px * factor).clamp(MIN_PITCH_X, MAX_PITCH_X);
        let factor = new_px / old_px;
        let new_py = old_py * factor;

        if (new_px - old_px).abs() < 0.01 {
            return;
        }

        let (area_x, area_y, _, _) = self.positions[CONTENT_IDX];
        let (cx, cy) = match center {
            Some(pt) => pt,
            None => self.cell_center(self.grid_cursor_col, self.grid_cursor_row),
        };

        // The anchor's lattice coordinate, in pitches, is what must not move.
        let col_f = (cx - area_x - self.pan_x) / old_px;
        let row_f = (cy - area_y - self.pan_y) / old_py;

        self.pan_x = cx - area_x - col_f * new_px;
        self.pan_y = cy - area_y - row_f * new_py;

        self.scale_grid_geometry(factor);

        self.sync_grid_settings();
    }


    pub fn keep_cursor_in_view(&mut self) {
        self.keep_cell_in_view(self.grid_cursor_col, self.grid_cursor_row);
    }

    /// Pan the least that brings one lattice cell fully into the pane. The
    /// cursor's own is the usual one; an EXTEND walks the region's far corner
    /// instead, and following the anchor there would scroll the wrong end of
    /// the selection into view — the anchor is the end that is not moving.
    pub fn keep_cell_in_view(&mut self, col: i32, row: i32) {
        let (px, py, pw, ph) = self.positions[CONTENT_IDX];
        let (cx, cy, cw, ch) = self.cell_rect(col, row);

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
            let bc_h = breadcrumb_h();
            self.positions[BREADCRUMB_IDX] = (cx - r, cy - r + 45.0, 2.0 * r, bc_h);
            self.positions[CONTENT_IDX] = (cx - r, cy - r + 45.0 + bc_h, 2.0 * r, 2.0 * r - (45.0 + bc_h));
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
            self.slots.playbar.inner_mut().frame = 0.0;

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
                let bc_h = breadcrumb_h();
                self.positions[BREADCRUMB_IDX] = (cx - r, cy - r + 45.0, 2.0 * r, bc_h);
                self.positions[CONTENT_IDX] = (cx - r, cy - r + 45.0 + bc_h, 2.0 * r, 2.0 * r - (45.0 + bc_h));
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
                self.slots.playbar.inner_mut().frame = 0.0;

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
                // The playbar is attached to the bottom edge (below), so the
                // plates above stop a gap short of its top.
                let pb_off = if self.show_playbar { playbar_shelf_h() } else { 0.0 };
                // The stored widths and height are what the user ASKED for;
                // the clamps below fit them to this window for drawing and are
                // never written back. Storing the clamp made every transient
                // shrink permanent — a window briefly narrower than a plate
                // left it that narrow when the window grew again.
                let fw = self.left_dock_width();
                let fx = paginator_w + gap;
                let fy = gap;
                let mut fh = (self.height - STATUS_H - pb_off - 2.0 * gap).max(100.0);
                self.floating_network_layout.0 = fx;
                self.floating_network_layout.1 = fy;
                self.floating_network_layout.3 = fh;

                let param_w = self.right_dock_width();
                let param_x = self.width - gap - param_w;
                let param_y = gap;
                let mut param_h = (self.height - STATUS_H - pb_off - 2.0 * gap).max(100.0);

                // The spreadsheet rect (shared derivation with the edge hotspots —
                // reads the clamped network width written back above). A side inset
                // tucks the spreadsheet UNDER that neighbor: the neighbor's bottom
                // rises to the spreadsheet's top edge to make room.
                let (ss_x, ss_y, ss_w, ss_h) = self.floating_spreadsheet_rect();
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
                // The network has no plate (since 2026-10-06): it is an
                // overlay on the scene, so it takes the whole body instead of
                // its dock — there is no surface to bound it, and a graph confined to a
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
                let bc_h = if self.show_network { breadcrumb_h() } else { 0.0 };
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
                self.positions[VIEWPORT_IDX] = (col_c_x, vp_y, col_c_w, vp_h);
                // The params HUD is not docked: it lives on the scene, laid
                // out from the viewport alone, under every plate.
                let p_rect = if right_visible { self.params_hud_rect() } else { (0.0, 0.0, 0.0, 0.0) };
                self.positions[PARAM_IDX] = p_rect;
                self.slots.param.set_rect(p_rect.0, p_rect.1, p_rect.2, p_rect.3);
                self.positions[RIGHT_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
                // The raised run floats clear of the plate's edge: offset past
                // the plate's rolled lip (bevel_width) plus the run's own boss
                // roll, so the two shadings never overlap — flush at the corner
                // they read as one clipped lump, not a part in front of a plate.
                let bc_pad = cce_ui::layout::bevel_width()
                    + cce_ui::layout::bevel_width().min(breadcrumb_h() * 0.2);
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
                let bc2 = if qw > 0.0 { breadcrumb_h() } else { 0.0 };
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
                    // A SHELF of the window's bottom edge, the full width and
                    // down into the window's lip: its sides and bottom are the
                    // window's own edge, drawn over it, and only its top is a
                    // plate's (the PLAYBAR_IDX render arm). The transport
                    // stands clear of the lip (`Playbar::frame`), and the
                    // plates above stop a gap short of its top (`pb_off`). It
                    // floated a gap in from the sides and the bottom until
                    // 2026-10-06, and was a plate of its own laid on the
                    // window's edge, rolled all round, until later that day.
                    let h = playbar_shelf_h();
                    (0.0, self.height - STATUS_H - h, self.width, h)
                } else {
                    (0.0, 0.0, 0.0, 0.0)
                };
                self.slots.playbar.inner_mut().frame = cce_ui::layout::bevel_width();

                // Tab-aware visibility: a pane WAITING in a dock's tab list
                // is hidden regardless of its View flag — a zero rect alone
                // does not stop the text pass, so a waiting editor's labels
                // would paint over whichever pane fronted.
                let net_active = self.dock_of_pane(NETWORK_PANEL_IDX).is_some();
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
                self.slots.param.set_visible(right_visible);
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
        self.apply_collapsed_panes();
        // The params HUD stops above the spreadsheet and the playbar as
        // they finally stand — a collapse or a detach above moved them —
        // in the floating layout that lays it out from the viewport.
        if !self.is_detached_network
            && !self.detached_circular_network
            && !self.circular_network_pane
            && self.detached_pane.is_none()
            && self.show_parameters
            && !self.pane_is_stubbed(PARAM_IDX)
        {
            let r = self.params_hud_rect();
            self.positions[PARAM_IDX] = r;
            self.slots.param.set_rect(r.0, r.1, r.2, r.3);
        }
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
        let frame = self.sim_frame();
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
                    .map(|p| p.text().to_string()),
                // The rest of the wires the network draws.
                reads: node_wires_at(&self.fs_root, c, frame).into_iter().filter(|(name, _)| !name.eq_ignore_ascii_case("input")).map(|(_, src)| src).collect(),
                position: c.position,
                // Utility trees stay where they were put; see the module doc.
                pinned: false,
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
        // An expanded cursor IS a selection, and of the two this is the one
        // Escape has to be able to clear: the single selection under a plain
        // cursor comes back on the next sync anyway, while a region would
        // stand until the cursor was moved off its anchor.
        if self.grid_cursor_expanded() {
            self.grid_cursor_expanse = None;
            self.grid_cursor_drag = None;
            self.sync_parameters_pane();
            return true;
        }
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

    /// Lay the drag group's companions out around `dest`, the cell the dragged
    /// node is on (or heading for), keeping the shape the selection had when
    /// the drag began.
    ///
    /// They translate rigidly and are NOT walked off occupied cells the way
    /// the widget walks the node it drags: a whole selection that rearranged
    /// itself around whatever it passed over would not be the selection you
    /// picked up. It is the same bargain `network_move_node` has always made
    /// with alt+hjkl.
    fn drag_group_to(&mut self, dest: (i32, i32)) {
        let Some(group) = self.node_drag_group.clone() else { return };
        let (dc, dr) = (dest.0 - group.from.0, dest.1 - group.from.1);
        let len = self.current_dir().children.len();
        for (slot, origin) in group.others {
            if slot < len {
                self.current_dir_mut().children[slot].position =
                    (origin.0 + dc as f32, origin.1 + dr as f32);
            }
        }
        self.sync_nodes();
    }

    /// Close an expansion drag by shrinking the region onto what it caught:
    /// the bounding box of the selected nodes, or — with nothing caught — one
    /// cell at the middle of where the region stood.
    ///
    /// A region is a way of POINTING at nodes, and once the pointing is done
    /// the empty margin the pointer swept through is noise: it hides nothing,
    /// it selects nothing, and it makes the next alt+hjkl or Add Node read off
    /// an anchor out in open grid. Settling it also makes the region say what
    /// was selected — a box drawn loosely around two nodes comes back fitted
    /// to them, which is the selection made visible.
    ///
    /// Nothing caught settles to the MIDDLE rather than to the anchor: the
    /// anchor is where the gesture began, and a drag that selected nothing is
    /// most likely aimed at the space it ended up circling. Even spans round
    /// down, toward the region's own first cell.
    ///
    /// The selection is unchanged by all of this — the bounding box of the
    /// selected nodes contains no cell the region did not — which is what
    /// lets it run unconditionally at the end of every drag.
    pub(crate) fn settle_cursor_expansion(&mut self) -> bool {
        if !self.grid_cursor_expanded() {
            return false;
        }
        let (col, row, cols, rows) = self.grid_cursor_region();
        let cells: Vec<(i32, i32)> = self
            .selected_slots()
            .into_iter()
            .map(|i| {
                let p = self.current_dir().children[i].position;
                (p.0 as i32, p.1 as i32)
            })
            .collect();
        match cells.split_first() {
            None => {
                self.grid_cursor_col = col + (cols - 1) / 2;
                self.grid_cursor_row = row + (rows - 1) / 2;
                self.grid_cursor_expanse = None;
            }
            Some((first, rest)) => {
                let (mut c0, mut r0, mut c1, mut r1) = (first.0, first.1, first.0, first.1);
                for &(c, r) in rest {
                    c0 = c0.min(c);
                    r0 = r0.min(r);
                    c1 = c1.max(c);
                    r1 = r1.max(r);
                }
                self.grid_cursor_col = c0;
                self.grid_cursor_row = r0;
                self.grid_cursor_expanse =
                    ((c1, r1) != (c0, r0)).then_some(((c0, r0), (c1, r1)));
            }
        }
        // A settle that lands on a node selects it, and one that lands on
        // empty grid clears the single selection — the ordinary cursor rules,
        // which is what the cursor is again whenever the region collapsed.
        self.deselected_cell = None;
        self.sync_cursor_and_selection();
        true
    }

    /// Grow (or shrink) the cursor's region by one cell — the shift+hjkl
    /// family, the plugin's extend-the-selection scheme.
    ///
    /// The ANCHOR never moves: it is the fixed end of the region, exactly as
    /// it is for a drag, so shift+l then shift+h returns to where it started
    /// rather than walking the whole region right and back. A far corner that
    /// meets the anchor again drops the expanse outright, so a shrunk-to-
    /// nothing region is the plain one-cell cursor and not a 1x1 region that
    /// merely behaves like one.
    ///
    /// Extending from a cursor that sits ON a node keeps that node in the
    /// selection — the anchor's cell is part of its own region — which is what
    /// makes the family an EXTEND rather than a second way to start one.
    pub(crate) fn network_extend(&mut self, dc: i32, dr: i32) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        let anchor = (self.grid_cursor_col, self.grid_cursor_row);
        let far = match self.grid_cursor_expanse {
            Some((a, far)) if a == anchor => far,
            _ => anchor,
        };
        let far = (far.0 + dc, far.1 + dr);
        self.grid_cursor_expanse = (far != anchor).then_some((anchor, far));
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.keep_cell_in_view(far.0, far.1);
        true
    }

    /// Copy the selected nodes, positions and all.
    pub(crate) fn copy_selected_nodes(&mut self) {
        let slots = self.selected_slots();
        let dir = self.current_dir();
        self.node_clipboard = slots
            .into_iter()
            .filter_map(|i| dir.children.get(i).cloned())
            .collect();
    }

    /// Paste the clipboard at the grid cursor, KEEPING THE SHAPE it was
    /// copied in: the set's top-left corner lands on the cursor cell and
    /// every node keeps its offset from it, so a pasted chain arrives wired
    /// the way it was drawn rather than stacked in a column.
    ///
    /// A node whose cell is taken steps aside to the nearest free one, which
    /// is the one case where the shape gives: a paste that silently sat two
    /// nodes on one crossing would be worse than a paste that is a cell out.
    ///
    /// A pasted node whose name is taken takes the next free one
    /// (`transform1` beside a `transform1` becomes `transform2`), and the
    /// wires between pasted nodes follow, so a pasted chain reads itself
    /// and not the chain it was copied from. A wire to a node that was not
    /// copied still names that node.
    ///
    /// Pasted on a free cursor cell a wire runs through, the paste is
    /// spliced into that wire, as Add Node there would be: one node, or a
    /// chain with one head and one tail, goes in whole.
    pub(crate) fn paste_nodes(&mut self) -> bool {
        if self.node_clipboard.is_empty() {
            return false;
        }
        // Whole or not at all: a set that half belongs here is pasted
        // nowhere, rather than as the part that does with its wires cut.
        let here = crate::context::context_at(&self.current_path);
        if let Some(why) = self
            .node_clipboard
            .iter()
            .find_map(|n| crate::context::refusal(&n.name, &n.node_type, here))
        {
            self.update_status_text(&format!("Not pasted: {why}"));
            return false;
        }
        let origin = self.node_clipboard.iter().fold((f32::MAX, f32::MAX), |(x, y), n| {
            (x.min(n.position.0), y.min(n.position.1))
        });
        let (cx, cy) = (self.grid_cursor_col as f32, self.grid_cursor_row as f32);
        // Asked before the paste, whose nodes' own wires would touch the cell.
        let free = !self.current_dir().children.iter().any(|c| c.position == (cx, cy));
        let wire = if free { self.graph().input_wire_through_cell(cx, cy) } else { None };
        let mut last = (cx, cy);
        let mut renamed: Vec<(String, String)> = Vec::new();
        let mut pasted: Vec<String> = Vec::new();
        for source in self.node_clipboard.clone() {
            let mut node = source;
            regenerate_node_ids(&mut node);
            let want = (cx + node.position.0 - origin.0, cy + node.position.1 - origin.1);
            let (nx, ny) = self.find_empty_cell(want.0, want.1, None);
            node.position = (nx, ny);
            // Added = hidden, same as AddNode: a paste of a displayed node
            // must not become a second visible sibling.
            node.geometry_visible = false;
            let old = node.name.clone();
            if self.current_dir().children.iter().any(|c| c.name == old) {
                node.name = self.get_lowest_unused_name(old.trim_end_matches(|c: char| c.is_ascii_digit()));
            }
            renamed.push((old, node.name.clone()));
            pasted.push(node.id.clone());
            self.current_dir_mut().children.push(node);
            last = (nx, ny);
        }
        let dir = self.current_dir_mut();
        for node in dir.children.iter_mut().filter(|c| pasted.contains(&c.id)) {
            for p in node.params.iter_mut().filter(|p| p.kind() == ParamKind::Node && !p.is_expr()) {
                let wired = p.text().trim().to_string();
                if let Some((_, new)) = renamed.iter().find(|(old, _)| *old == wired) {
                    p.set_text(new.clone());
                }
            }
        }
        if let Some((src_id, dest_id)) = wire {
            self.splice_paste(&pasted, &src_id, &dest_id);
        }
        // The cursor lands on the last node pasted, collapsed — the pasted
        // nodes are new, and the region that selected the originals means
        // nothing about them.
        self.grid_cursor_col = last.0 as i32;
        self.grid_cursor_row = last.1 as i32;
        self.grid_cursor_expanse = None;
        self.sync_nodes();
        self.sync_cursor_and_selection();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.rebuild_scene_geometry();
        self.viewport_dirty = true;
        true
    }

    /// Splice the nodes just pasted (`ids`) into the wire from `src_id` to
    /// `dest_id`: as one chain, its head the pasted node whose Input reads
    /// no other pasted node, its tail the one no other pasted node reads.
    /// A paste that is not one such chain is left beside the wire.
    fn splice_paste(&mut self, ids: &[String], src_id: &str, dest_id: &str) {
        let dir = self.current_dir();
        let nodes: Vec<&FsNode> = dir.children.iter().filter(|c| ids.contains(&c.id)).collect();
        let input = |n: &FsNode| crate::geometry::node_param_node(n, "input");
        let heads: Vec<&FsNode> = nodes.iter().copied().filter(|n| !input(n).is_some_and(|i| nodes.iter().any(|m| m.name == i))).collect();
        let tails: Vec<&FsNode> = nodes.iter().copied().filter(|n| !nodes.iter().any(|m| input(m).as_deref() == Some(n.name.as_str()))).collect();
        let ([head], [tail], Some(src)) = (heads.as_slice(), tails.as_slice(), dir.children.iter().find(|c| c.id == src_id)) else { return };
        let (head_id, tail_id, src_name) = (head.id.clone(), tail.id.clone(), src.name.clone());
        if splice_chain_into_wire(self.current_dir_mut(), &head_id, &tail_id, src_name.clone(), dest_id) {
            let dest = self.current_dir().children.iter().find(|c| c.id == dest_id).map(|c| c.name.clone()).unwrap_or_default();
            self.update_status_text(&format!("Pasted between {src_name} and {dest}."));
        }
    }

    /// Move the SELECTED nodes one cell, and the cursor with them — so a run
    /// of alt+h drags them across the sheet rather than leaving them behind on
    /// the first press.
    ///
    /// With an expanded cursor that is every node inside it, and the region
    /// travels too: stepping the anchor alone is exactly what collapses a
    /// region, so a selection would be dropped by the first press that moved
    /// it. `network_nav` cannot do that job — for the plain cursor, stepping
    /// off IS the point.
    pub(crate) fn network_move_node(&mut self, dc: i32, dr: i32) -> bool {
        if self.focused_pane != LEFT_MENUBAR_IDX {
            return false;
        }
        let moving = self.selected_slots();
        if !moving.is_empty() {
            for idx in moving {
                let (x, y) = self.current_dir().children[idx].position;
                self.current_dir_mut().children[idx].position = (x + dc as f32, y + dr as f32);
            }
            self.sync_nodes();
            self.sync_layout();
        }
        if self.grid_cursor_expanded() {
            self.pan_velocity_x = 0.0;
            self.pan_velocity_y = 0.0;
            self.shift_grid_cursor(dc, dr);
            self.deselected_cell = None;
            self.keep_cursor_in_view();
            return true;
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
        self.pan_x -= dc as f32 * self.grid_pitch_x;
        self.pan_y -= dr as f32 * self.grid_pitch_y;
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
        // Pan so the cursor cell's centre — the lattice intersection it names
        // — lands at the pane's centre: the pane's half-extent minus the
        // cell's offset from the origin, which is its column times the pitch.
        self.pan_x = pw * 0.5 - self.grid_cursor_col as f32 * self.grid_pitch_x;
        self.pan_y = ph * 0.5 - self.grid_cursor_row as f32 * self.grid_pitch_y;
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.sync_grid_settings();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        true
    }

    /// The extent a node takes up on the sheet at a grid geometry, relative
    /// to the (0, 0) crossing — `(x_min, x_max, y_min, y_max)`. The body,
    /// AND the name label the widget draws to its right: framing the bodies
    /// alone left the labels of the right-hand column cut off, which is what
    /// Frame All exists to avoid.
    ///
    /// The label's placement is `Graph::node_labels`'s rule, repeated here
    /// because the widget offers no query for it: an 8 px gap and a 14 px
    /// font, both scaled with the body against its 80 px baseline, the font
    /// clamped to 6..48, the width the widget's own estimate — the number it
    /// culls the label against, so the two cannot disagree about where a
    /// label ends. The label is centred on the body and shorter than it at
    /// every zoom Frame All can choose, but its own height is folded in
    /// anyway rather than argued away.
    fn node_extent(name: &str, (col, row): (f32, f32), g: &GridGeometry) -> (f32, f32, f32, f32) {
        let x_min = col * g.pitch_x - g.node_w * 0.5;
        let y_min = row * g.pitch_y - g.node_h * 0.5;
        let scale_f = g.node_w / 80.0;
        let font_size = (14.0 * scale_f).clamp(6.0, 48.0);
        let label_w = TextLabel::estimate_width(name, font_size);
        let cy = row * g.pitch_y;
        (
            x_min,
            x_min + g.node_w + 8.0 * scale_f + label_w,
            y_min.min(cy - font_size * 0.5),
            (y_min + g.node_h).max(cy + font_size * 0.5),
        )
    }

    /// The union of `node_extent` over the current level, or None when the
    /// level is empty.
    fn level_extent(&self, g: &GridGeometry) -> Option<(f32, f32, f32, f32)> {
        self.current_dir().children.iter().fold(None, |acc, child| {
            let (x0, x1, y0, y1) = Self::node_extent(&child.name, child.position, g);
            Some(match acc {
                None => (x0, x1, y0, y1),
                Some((ax0, ax1, ay0, ay1)) => (ax0.min(x0), ax1.max(x1), ay0.min(y0), ay1.max(y1)),
            })
        })
    }

    /// Fit every node in the current level — body and name label — into the
    /// network pane.
    ///
    /// Extracted verbatim from the `f` key's inline arm so the command
    /// registry and the key run the same code — the point of the registry
    /// being that there is one implementation behind every way of asking.
    pub(crate) fn frame_all_nodes(&mut self) {
        // The framing baseline is the configured geometry (100% zoom),
        // scaled down until everything fits — Frame All never zooms in
        // past 100%.
        let base = configured_grid_geometry();
        let at = |f: f32| GridGeometry {
            pitch_x: base.pitch_x * f,
            pitch_y: base.pitch_y * f,
            node_w: base.node_w * f,
            node_h: base.node_h * f,
        };
        let (_px, _py, pw, ph) = self.positions[CONTENT_IDX];
        let padding = 40.0;
        let padded_w = (pw - 2.0 * padding).max(10.0);
        let padded_h = (ph - 2.0 * padding).max(10.0);

        if self.level_extent(&base).is_none() {
            self.set_grid_geometry(base);
            self.pan_x = 20.0;
            self.pan_y = 20.0;
        } else {
            // The extent is not linear in the zoom: a label's font stops
            // shrinking at 6 px and its width is rounded up, so the fit
            // computed at 100% overstates how much a small zoom saves. Each
            // pass refits at the zoom the last one chose; the factor only
            // ever falls, and a pass that changes nothing ends it.
            let mut f = 1.0f32;
            for _ in 0..4 {
                let (x0, x1, y0, y1) = self.level_extent(&at(f)).unwrap();
                let fit = (padded_w / (x1 - x0)).min(padded_h / (y1 - y0)).min(1.0);
                if fit >= 1.0 {
                    break;
                }
                f *= fit;
            }
            f = f.max(MIN_PITCH_X / base.pitch_x);

            self.set_grid_geometry(at(f));
            let (x0, x1, y0, y1) = self.level_extent(&at(f)).unwrap();
            self.pan_x = (pw - (x1 - x0)) / 2.0 - x0;
            self.pan_y = (ph - (y1 - y0)) / 2.0 - y0;
        }

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
            Action::CommandPalette => self.open_dialog(),
            Action::ToggleDialog => self.toggle_dialog(),
            // The network navigation families. Each returns false when the
            // network pane does not have focus, which is how one gate covers
            // all eighteen of them.
            Action::NetworkNav(dc, dr) => {
                self.network_nav(dc, dr);
            }
            Action::NetworkExtend(dc, dr) => {
                self.network_extend(dc, dr);
            }
            Action::NetworkMove(dc, dr) => {
                self.network_move_node(dc, dr);
            }
            Action::NetworkPan(dc, dr) => {
                self.network_pan_view(dc, dr);
            }
            Action::ToggleParamsPlate => {
                self.params_plate = !self.params_plate;
                self.update_status_text(if self.params_plate {
                    "Parameters plate on, fitted to the rows."
                } else {
                    "Parameters plate off — the controls stand on the scene."
                });
                settings_changed = true;
            }
            Action::Deselect => {
                self.deselect_node();
            }
            Action::ToggleBypass => {
                // The network's, like the rest of its bare-letter family:
                // `b` typed anywhere else is a letter. The whole selection
                // follows the FIRST node's flag, as `e` has it follow the
                // first node's geometry flag.
                if self.focused_pane == LEFT_MENUBAR_IDX {
                    let selected = self.selected_slots();
                    if let Some(&first) = selected.first() {
                        let bypassed = !self.current_dir().children.get(first).is_some_and(|n| n.bypassed);
                        self.set_bypassed(&selected, bypassed);
                    }
                }
            }
            Action::LayoutNodes => {
                self.layout_current_level();
            }
            Action::FrameImage => {
                self.frame_image();
            }
            Action::ViewImagePixels => {
                self.view_image_pixels();
            }
            Action::NewImage => {
                self.new_image();
            }
            Action::AddToImage(layer) => {
                self.add_to_image(layer);
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
            // Edit menu rows come here directly. A viewer state's history
            // first, then the edit history (`src/edit_history.rs`);
            // a project-wide one would be consulted after both decline.
            Action::Undo => {
                let _ = self.viewer_tool_undo() || self.history_step(true);
            }
            Action::Redo => {
                let _ = self.viewer_tool_redo() || self.history_step(false);
            }
            Action::ToggleGrid => {
                let val = !self.viewport().show_grid;
                self.viewport_mut().show_grid = val;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(GUIDES_MENU, GUIDE_GRID, val);
                settings_changed = true;
            }
            Action::ToggleOrigin => {
                let val = !self.viewport().show_origin;
                self.viewport_mut().show_origin = val;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(GUIDES_MENU, GUIDE_ORIGIN, val);
                settings_changed = true;
            }
            Action::ToggleCameraPivot => {
                let val = !self.viewport().show_camera_pivot;
                self.viewport_mut().show_camera_pivot = val;
                self.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(GUIDES_MENU, GUIDE_CAMERA_PIVOT, val);
                settings_changed = true;
            }
            Action::ToggleWireframe => {
                let val = !self.wireframe;
                self.wireframe = val;
                // The edge list is collected with the scene and dropped
                // while the wireframe is off, so switching it on has to
                // rebuild — there is nothing staged to draw otherwise.
                self.rebuild_scene_geometry();
            }
            // The smooth-lit fill is baked with the scene and dropped while
            // flat, so the flip rebuilds, as the wireframe's does.
            Action::ToggleSmoothShading => {
                self.smooth_shading = !self.smooth_shading;
                self.rebuild_scene_geometry();
                settings_changed = true;
            }
            // A draw-time switch: the stage pass sorts and picks the
            // see-through pipeline; nothing to rebuild. Said out loud at full
            // opacity, where it has nothing to show and would look broken.
            Action::ToggleShowOccluded => {
                self.show_occluded = !self.show_occluded;
                self.viewport_dirty = true;
                settings_changed = true;
                if self.show_occluded && self.geo_opacity >= 0.999 {
                    self.update_status_text("Show Occluded takes effect below 100% opacity.");
                }
            }
            // The three point overlays. Each is collected in
            // `rebuild_scene_geometry` off the scene's own Detail, so the
            // flip has to re-run it: the meshes are built from the flags,
            // not filtered at draw time.
            Action::TogglePointMarkers => {
                self.show_point_markers = !self.show_point_markers;
                self.rebuild_scene_geometry();
                settings_changed = true;
            }
            Action::TogglePointNumbers => {
                self.show_point_numbers = !self.show_point_numbers;
                self.rebuild_scene_geometry();
                settings_changed = true;
            }
            Action::TogglePointNormals => {
                self.show_point_normals = !self.show_point_normals;
                self.rebuild_scene_geometry();
                settings_changed = true;
            }
            Action::TogglePrimNumbers => {
                self.show_prim_numbers = !self.show_prim_numbers;
                self.rebuild_scene_geometry();
                settings_changed = true;
            }
            Action::TogglePrimNormals => {
                self.show_prim_normals = !self.show_prim_normals;
                self.rebuild_scene_geometry();
                settings_changed = true;
            }
            Action::ToggleVertexMarkers => {
                self.show_vertex_markers = !self.show_vertex_markers;
                self.rebuild_scene_geometry();
                settings_changed = true;
            }
            Action::ToggleVertexNormals => {
                self.show_vertex_normals = !self.show_vertex_normals;
                self.rebuild_scene_geometry();
                settings_changed = true;
            }
            Action::ToggleVertexNumbers => {
                self.show_vertex_numbers = !self.show_vertex_numbers;
                self.rebuild_scene_geometry();
                settings_changed = true;
            }
            Action::ToggleWireSingleColor => {
                self.wire_single_color = !self.wire_single_color;
                self.viewport_dirty = true;
                settings_changed = true;
            }
            Action::ToggleRayTracedPreview => {
                let val = !self.viewport().rt_mode;
                self.viewport_mut().rt_mode = val;
                self.viewport_dirty = true;
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
                // The Main node owns this one, as Guides owns the guide
                // toggles above: without the write, the next parameter edit
                // put the pane back the way the node said.
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
                    pb.begin(action == Action::PlayPauseReverse);
                }
            }
            // Repeat is a setting, not a transport press: the timeline keeps
            // doing whatever it is doing, and the next run-off honours it.
            Action::TogglePlaybarRepeat => {
                let pb = self.slots.playbar.inner_mut();
                pb.repeat = !pb.repeat;
                settings_changed = true;
            }
            Action::TogglePlaybarStepButtons => {
                let pb = self.slots.playbar.inner_mut();
                pb.step_buttons = !pb.step_buttons;
                settings_changed = true;
            }
            // Whole-frame stepping (`Playbar::step`, which the playbar's
            // own step buttons share). The scene rebuild follows from
            // tick_frame's last_sim_frame diff.
            Action::FrameNext | Action::FramePrev => {
                let step = if action == Action::FrameNext { 1.0 } else { -1.0 };
                self.slots.playbar.inner_mut().step(step);
            }
            // Rewind: pause whatever is playing and land on the start frame.
            // The scene rebuild follows from tick_frame's last_sim_frame
            // diff, as for the steps above.
            Action::FrameStart => {
                let pb = self.slots.playbar.inner_mut();
                pb.playing = false;
                pb.current_frame = pb.start_frame;
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
        // Not while a group drag is carrying the selection: the anchor is
        // holding the region still on purpose, and yanking it onto the
        // dragged node's cell would collapse the selection mid-gesture.
        if self.node_drag_group.is_some() {
            return;
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

            self.window_configured = true;
            if let Some(pg) = self.pending_plates.take() {
                // The load took its saved baseline with the plates on the
                // placeholder size. Landing them on the real one is the load
                // finishing, not an edit — unless something was already
                // unsaved, which stays so.
                let clean = !self.has_unsaved_changes();
                self.apply_plate_geometry(pg);
                if clean {
                    self.last_saved_layout_json = self.pane_layout_json();
                    self.update_window_title();
                }
            }

            self.sync_layout();
            self.read_panel_offsets();
            self.keep_cursor_in_view();
            self.viewport_dirty = true;
        }
    }

    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        // A press, a release, or a key that ends an entry ends the gesture
        // an undo step is: what the params pane writes next is a new one.
        match event {
            WindowEvent::MouseInput { .. } => self.edit_history.break_group(),
            WindowEvent::KeyboardInput { event } if event.state == ElementState::Pressed => {
                if matches!(
                    event.logical_key,
                    Key::Named(NamedKey::Enter | NamedKey::Tab | NamedKey::Escape)
                ) {
                    self.edit_history.break_group();
                }
            }
            _ => {}
        }
        match event {
            WindowEvent::MouseWheel { delta } => {
                // What is left of a swipe that turned a menu or the dialog is
                // the turn's, not the scene's or a list's under the new plate.
                if cce_ui::widget::side_swipe::swallow(delta) {
                    return true;
                }
                // The dialog is modal: a wheel over it scrolls it, and a wheel
                // anywhere else does nothing rather than scrolling — and
                // focusing — the pane it is covering.
                // A wheel closes the dialog's open dropdown, whose list
                // would otherwise ride a scroll it was not laid out for.
                if self.dialog_dropdown_open() {
                    self.close_dialog_dropdown();
                }
                if self.dialog_visible() {
                    return self.dialog_mouse_wheel(*delta);
                }
                // Over an open menu the wheel is the menu's: a side swipe
                // turns it (`crate::menu_page`), a slider row steps, and
                // nothing scrolls or orbits beneath.
                if self.open_menu_origin().is_some()
                    && cce_ui::widget::context_menu::hit_test(self.cursor_x, self.cursor_y)
                {
                    if cce_ui::widget::context_menu::mouse_wheel(delta, self.cursor_x, self.cursor_y) {
                        if !self.take_menu_turn() && self.slider_menu_open() {
                            self.drain_menu_slider(true);
                        }
                    }
                    return true;
                }
                // Shift and a scroll over the scene slide the camera, as the
                // drag does: the scene follows the fingers.
                if self.modifiers.shift_key() && !self.modifiers.control_key() && self.cursor_in_viewport() && !self.in_network_pane() {
                    let (dx, dy) = match delta {
                        MouseScrollDelta::PixelDelta(p) => {
                            let s = (self.scale as f32).max(0.001);
                            (p.x as f32 / s, p.y as f32 / s)
                        }
                        MouseScrollDelta::LineDelta(x, y) => (*x * Self::PAN_PX_PER_LINE, *y * Self::PAN_PX_PER_LINE),
                    };
                    if dx != 0.0 || dy != 0.0 {
                        self.pan_camera_by(dx, dy);
                    }
                    return true;
                }
                // Over the plateless network the gesture's target decides,
                // not what is under the pointer now (`overlay_wheel_to_graph`):
                // a scroll begun on a node pans the graph, one begun on empty
                // space orbits the camera.
                let overlay = self.network_overlay();
                let in_network_pane = if overlay {
                    self.overlay_wheel_to_graph(cce_ui::widget::scroll_motion::current_scroll_phase())
                } else {
                    self.in_network_pane()
                };
                // eprintln!("DEBUG MOUSEWHEEL: delta={:?}, phase={:?}, cursor=({}, {}), in_network_pane={}", delta, phase, self.cursor_x, self.cursor_y, in_network_pane);
                let node_area_y = self.positions[CONTENT_IDX].1;

                let in_viewport = self.cursor_in_viewport();

                let mut focus_changed = false;
                let mut new_pane = None;
                {
                    if in_network_pane {
                        new_pane = Some(LEFT_MENUBAR_IDX);
                    } else if self.show_spreadsheet && {
                        let (sx, sy, sw, sh) = self.positions[SPREADSHEET_IDX];
                        sw > 0.0 && sh > 0.0
                            && self.cursor_x >= sx && self.cursor_x < sx + sw
                            && self.cursor_y >= sy && self.cursor_y < sy + sh
                    } {
                        // Asked before the viewport: `cursor_in_viewport` no
                        // longer counts the spreadsheet as scene, so this can
                        // no longer be a refinement of that answer.
                        new_pane = Some(SPREADSHEET_MENUBAR_IDX);
                    } else if in_viewport {
                        new_pane = Some(RIGHT_MENUBAR_IDX);
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
                        // The plateless network spans the window: it takes
                        // only a gesture begun on one of its nodes.
                        if overlay && !in_network_pane && (i == CONTENT_IDX || i == NETWORK_PANEL_IDX) {
                            continue;
                        }
                        // Under its rows a plateless params pane is scene.
                        if i == PARAM_IDX && !self.params_claims(self.cursor_x, self.cursor_y) {
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

                // A captured pointer reaches no pane: the returns below
                // (a menu slider, a handle grab, an orbit, a grid drag) each
                // leave before the broadcast at the end of this arm, and a
                // pane hovered at the press stayed lit for the whole gesture.
                if self.pointer_captured() && self.broadcast_pointer() {
                    changed = true;
                }

                // Track hover on the node/viewport/network context menus so
                // the highlight follows.
                if self.open_menu_origin().is_some()
                    && cce_ui::widget::context_menu::cursor_moved(self.cursor_x, self.cursor_y)
                {
                    changed = true;
                }
                // The dialog's open dropdown follows the pointer with its
                // highlight, ahead of the dialog.
                if self.dialog_dropdown_open() {
                    let ev = cce_ui::widget::Event::PointerMove { x: self.cursor_x, y: self.cursor_y, local_x: self.cursor_x, local_y: self.cursor_y };
                    if self.dialog_dropdown_event(&ev) {
                        changed = true;
                    }
                }
                // A held menu slider follows the pointer (cursor_moved moved
                // it); land the value, and let nothing else read this motion
                // as a drag of its own.
                if self.slider_menu_open() && cce_ui::widget::context_menu::slider_dragging() {
                    self.drain_menu_slider(false);
                    return true;
                }

                // An in-flight curve-tool grab eats motion ahead of every
                // other drag: the grabbed control point tracks the cursor.
                if self.viewer_tool_drag_motion() {
                    return true;
                }

                // An in-flight camera orbit, likewise — it was armed by a press
                // on empty scene, so nothing else is competing for the motion.
                if let Some((lx, ly)) = self.pan_drag {
                    let (dx, dy) = (self.cursor_x - lx, self.cursor_y - ly);
                    self.pan_drag = Some((self.cursor_x, self.cursor_y));
                    if dx != 0.0 || dy != 0.0 {
                        self.pan_camera_by(dx, dy);
                    }
                    return true;
                }
                if let Some((lx, ly)) = self.orbit_drag {
                    let (dx, dy) = (self.cursor_x - lx, self.cursor_y - ly);
                    self.orbit_drag = Some((self.cursor_x, self.cursor_y));
                    if dx != 0.0 || dy != 0.0 {
                        self.orbit_camera_by(dx, dy);
                    }
                    return true;
                }

                // An expansion drag on the network grid, likewise armed by
                // its own press and competing with nothing: the cursor grows
                // from the pressed cell to the one under the pointer.
                if self.grid_cursor_drag.is_some() {
                    return self.grid_cursor_drag_motion();
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
                                // A drag commits the width it SHOWS: storing an
                                // overshoot past the window's limit would leave
                                // the edge dead on the way back until the pointer
                                // had unwound it.
                                self.floating_network_layout.2 = self.left_dock_width();
                                self.rebuild_positions();
                                self.apply_layout();
                                self.sync_grid_settings();
                                changed = true;
                            }
                            AppDrag::RightDockResize { start_w, start_mouse_x } => {
                                let dx = self.cursor_x - start_mouse_x;
                                let new_w = (start_w - dx).max(150.0);
                                self.floating_param_width = new_w;
                                self.floating_param_width = self.right_dock_width();
                                self.rebuild_positions();
                                self.apply_layout();
                                changed = true;
                            }
                            AppDrag::HudResize { start_w, start_mouse_x } => {
                                let dx = self.cursor_x - start_mouse_x;
                                self.params_hud_width = (start_w - dx).max(PARAMS_HUD_MIN_W);
                                self.params_hud_width = self.params_hud_rect().2;
                                self.rebuild_positions();
                                self.apply_layout();
                                changed = true;
                            }
                            AppDrag::SpreadsheetResize { start_h, start_mouse_y } => {
                                let dy = self.cursor_y - start_mouse_y;
                                let new_h = (start_h - dy).max(100.0);
                                self.floating_spreadsheet_height = new_h;
                                self.floating_spreadsheet_height = self.floating_spreadsheet_rect().3;
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
                                let fx = self.floating_network_layout.0;
                                let flush_left = fx + self.left_dock_width() + gap;
                                let max_inset = (flush_left - gap).max(0.0);
                                self.floating_spreadsheet_inset_left =
                                    (start_inset - dx).clamp(0.0, max_inset);
                                self.rebuild_positions();
                                self.apply_layout();
                                self.sync_grid_settings();
                                changed = true;
                            }
                            AppDrag::SpreadsheetResizeRight { start_inset, start_mouse_x } => {
                                // The right edge tucks under the parameter pane, symmetrically.
                                let dx = self.cursor_x - start_mouse_x;
                                let gap = 18.0_f32;
                                let param_x = self.width - gap - self.right_dock_width();
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
                            if idx == CONTENT_IDX && self.node_drag_group.is_some() {
                                // The companions follow in whole cells, from
                                // where the widget says the dragged body will
                                // LAND (`drop_target_cell_rect` runs
                                // commit_drag's own resolution) — so the
                                // selection previews the same arrangement the
                                // release will make, rather than one measured
                                // off the free-floating ghost.
                                if let Some((rx, ry, rw, rh)) = self.graph().drop_target_cell_rect() {
                                    let dest = self.cell_at(rx + rw * 0.5, ry + rh * 0.5);
                                    self.drag_group_to(dest);
                                }
                            }
                            if idx == PARAM_IDX {
                                self.sync_parameters_to_project();
                            } else if idx == crate::slots::DIALOG_IDX {
                                // A dialog slider, mid-drag.
                                self.drain_dialog_clicks();
                            }
                        }
                    }

                    if self.broadcast_pointer() {
                        changed = true;
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
                // The release that ends a menu slider drag — wherever the
                // pointer is — commits it and is nobody else's.
                if *btn_state == ElementState::Released
                    && *button == MouseButton::Left
                    && cce_ui::widget::context_menu::slider_release()
                {
                    self.drain_menu_slider(true);
                    return true;
                }
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
                // A choice row's dropdown is in front of the dialog: it takes
                // the press first, and a press off it closes it alone.
                if *btn_state == ElementState::Pressed && self.dialog_dropdown_takes_press() {
                    return self.dialog_dropdown_press(*button);
                }
                if self.dialog_visible() {
                    if let Some(handled) = self.dialog_mouse_input(*button, *btn_state) {
                        return handled;
                    }
                }
                let in_network_pane = self.in_network_pane();

                // Panning asks the AREA, not the nodes: in overlay mode a
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

                // A middle press on the scene slides the camera. After the
                // network's own pan above, which the middle button is
                // wherever the network is laid out — in overlay mode the
                // whole window, where shift and the left button pan
                // the camera instead.
                if *button == MouseButton::Middle
                    && *btn_state == ElementState::Pressed
                    && self.cursor_in_viewport()
                    && !(self.circular_network_pane && self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, 0.0, breadcrumb_h()))
                    && self.app_drag.is_none()
                {
                    self.pan_drag = Some((self.cursor_x, self.cursor_y));
                    self.focused_pane = RIGHT_MENUBAR_IDX;
                    self.sync_pane_focus();
                    return true;
                }
                if *button == MouseButton::Middle && *btn_state == ElementState::Released && self.pan_drag.take().is_some() {
                    self.broadcast_pointer();
                    return true;
                }

                if *button != MouseButton::Left && *button != MouseButton::Right { return false; }
                let mut changed = false;
                let old_focus = self.focused_widget;

                let hits_widget = |state: &State, i: usize, x: f32, y: f32| -> bool {
                    if state.circular_network_pane && (i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX) {
                        if i == CONTENT_IDX {
                            state.circular_network_layout.hit_test_content(x, y, 0.0, breadcrumb_h())
                        } else if i == LEFT_MENUBAR_IDX {
                            false
                        } else if i == BREADCRUMB_IDX {
                            state.circular_network_layout.hit_test_breadcrumb(x, y, 0.0, breadcrumb_h())
                        } else {
                            false
                        }
                    } else if i == PARAM_IDX {
                        // The HUD is under every plate, and without its own
                        // plate it is its rows: a press anywhere else is the
                        // plate's or the scene's.
                        state.params_claims(x, y) && state.slots.get_dyn(i).hit_test(x, y, &state.ui_context)
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
                    self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, 0.0, breadcrumb_h())
                } else {
                    in_network_pane
                };

                match btn_state {
                    ElementState::Pressed => {
                        // A left press on a page row or a back band of any
                        // open menu turns it, ahead of the menus' own rows.
                        if *button == MouseButton::Left && self.press_menu_turn() {
                            return true;
                        }
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
                        // A plate drawn as its stub has nothing in it but its
                        // title: a left press on a collapsed one expands it,
                        // and a right press on either kind opens its plate
                        // menu (Expand, or a detached pane's Reattach).
                        if let Some(idx) = self.plate_at(self.cursor_x, self.cursor_y).filter(|&i| self.pane_is_stubbed(i)) {
                            if *button == MouseButton::Left && !self.pane_is_detached(idx) {
                                self.set_pane_collapsed(idx, false);
                                return true;
                            }
                            if *button == MouseButton::Right {
                                self.close_node_menu();
                                self.close_viewport_menu();
                                self.close_network_menu();
                                self.close_param_menu();
                                self.open_plate_menu(idx);
                                return true;
                            }
                        }
                        // The network editor's menu, same contract again.
                        if self.network_menu_open() {
                            if *button == MouseButton::Left && self.handle_network_menu_click() {
                                return true;
                            }
                            self.close_network_menu();
                            if *button == MouseButton::Left {
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
                        // The playbar's menu, same contract.
                        if self.playbar_menu_open() {
                            if *button == MouseButton::Left && self.handle_playbar_menu_click() {
                                return true;
                            }
                            self.close_playbar_menu();
                            if *button == MouseButton::Left {
                                return true;
                            }
                        }
                        // The parameter row menu, same contract again.
                        if self.param_menu_open() {
                            if *button == MouseButton::Left && self.handle_param_menu_click() {
                                return true;
                            }
                            self.close_param_menu();
                            if *button == MouseButton::Left {
                                return true;
                            }
                        }
                        // A right press on a parameter row opens its menu,
                        // ahead of everything else a press in that pane could
                        // mean — the pane's widgets have no right-click of
                        // their own to lose.
                        if *button == MouseButton::Right {
                            if let Some((slot, pname)) = self.param_row_at(self.cursor_x, self.cursor_y) {
                                self.close_node_menu();
                                self.close_viewport_menu();
                                self.close_network_menu();
                                self.open_param_context_menu(slot, pname);
                                return true;
                            }
                            // A right press on the playbar opens its menu.
                            if self.over_playbar(self.cursor_x, self.cursor_y) {
                                self.close_node_menu();
                                self.close_viewport_menu();
                                self.close_network_menu();
                                self.open_playbar_context_menu();
                                return true;
                            }
                            // A plate with no context menu of its own — the
                            // params pane off a row, the spreadsheet, the
                            // second network editor — has its plate menu.
                            if let Some(idx) = self
                                .plate_at(self.cursor_x, self.cursor_y)
                                .filter(|&i| matches!(i, PARAM_IDX | SPREADSHEET_IDX | crate::slots::NETWORK_PANEL2_IDX))
                            {
                                self.close_node_menu();
                                self.close_viewport_menu();
                                self.close_network_menu();
                                self.open_plate_menu(idx);
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
                            let (fx, fy, _, _fh) = self.floating_network_layout;
                            let fw = self.left_dock_width();
                            let cx = self.cursor_x;
                            let cy = self.cursor_y;

                            if let Some(dir) = self.network_resize_edge_at(cx, cy) {
                                self.app_drag = Some(AppDrag::NetworkResize {
                                    dir,
                                    start_rect: (fx, fy, fw, self.floating_network_layout.3),
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
                            } else if cx >= fx && cx < fx + fw && cy >= fy && cy < fy + (if self.show_network { breadcrumb_h() } else { 0.0 }) {
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

                        // The right dock's plate is over the HUD, so its edge
                        // is asked first.
                        if *button == MouseButton::Left && self.on_right_dock_resize_edge(self.cursor_x, self.cursor_y) {
                            self.app_drag = Some(AppDrag::RightDockResize {
                                start_w: self.right_dock_width(),
                                start_mouse_x: self.cursor_x,
                            });
                            if let Some(old) = self.focused_widget {
                                self.slots.get_dyn_mut(old).unfocus();
                                self.focused_widget = None;
                            }
                            return true;
                        }
                        if *button == MouseButton::Left && self.show_parameters {
                            let cx = self.cursor_x;
                            let cy = self.cursor_y;

                            if self.on_param_resize_edge(cx, cy) {
                                self.app_drag = Some(AppDrag::HudResize {
                                    start_w: self.params_hud_rect().2,
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

                        // Ctrl and a left press on empty space over the
                        // overlaid network box-selects: the press is the
                        // empty-grid press of the network's own cascade — the
                        // cursor to the cell pressed, an expansion drag armed
                        // from it, settled on the release — which a plain
                        // press there cannot be, empty space being the
                        // scene's to orbit. The network takes focus, as a
                        // press on its grid gives it.
                        if *button == MouseButton::Left
                            && self.modifiers.control_key()
                            && self.network_overlay()
                            && !in_network_pane
                            && self.in_network_area(self.cursor_x, self.cursor_y)
                            && self.app_drag.is_none()
                        {
                            let (col, row) = self.cell_at(self.cursor_x, self.cursor_y);
                            self.grid_cursor_col = col;
                            self.grid_cursor_row = row;
                            self.grid_cursor_expanse = None;
                            self.grid_cursor_drag = Some((col, row));
                            self.focused_pane = LEFT_MENUBAR_IDX;
                            if let Some(old) = self.focused_widget.take() {
                                self.slots.get_dyn_mut(old).unfocus();
                            }
                            self.sync_pane_focus();
                            return true;
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
                            // With shift the drag slides the camera
                            // where without it the drag turns it.
                            if self.modifiers.shift_key() {
                                self.pan_drag = Some((self.cursor_x, self.cursor_y));
                            } else {
                                self.orbit_drag = Some((self.cursor_x, self.cursor_y));
                            }
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
                            self.close_network_menu();
                            self.close_param_menu();
                            if self.cursor_in_viewport() && !in_circle_network_pane {
                                self.network_menu_cell = (self.network_overlay()
                                    && self.in_network_area(self.cursor_x, self.cursor_y))
                                .then(|| self.cell_at(self.cursor_x, self.cursor_y));
                                self.open_viewport_context_menu();
                                return true;
                            }
                            if in_circle_network_pane {
                                // On a node → its context menu; empty space →
                                // the network's own, whose first row is Add
                                // Node. The grid cursor moves to the clicked
                                // cell FIRST, because that is where Add Node
                                // will place what it adds — the menu is opened
                                // over the cell the user pointed at, and the
                                // cursor is the only thing carrying it there.
                                if let Some(slot) = self.graph().node_at(self.cursor_x, self.cursor_y) {
                                    self.graph_mut().set_selected_node(Some(slot));
                                    self.sync_parameters_pane();
                                    self.open_node_context_menu(slot);
                                    return true;
                                }
                                let (col, row) = self.cell_at(self.cursor_x, self.cursor_y);
                                self.grid_cursor_col = col;
                                self.grid_cursor_row = row;
                                self.open_network_context_menu();
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
                            let (col, row) = self.cell_at(self.cursor_x, self.cursor_y);
                            self.grid_cursor_col = col;
                            self.grid_cursor_row = row;
                            changed = true;
                        }
                        if let Some(i) = click_target {
                            self.slots.get_dyn_mut(i).set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
                            // Kept as a binding: the network's cursor-expansion
                            // drag must not arm on a press the graph already
                            // took. A press on a PORT is the case that bites —
                            // it starts a connection and returns true without
                            // selecting anything, so the "no selection" arm
                            // below would read it as empty grid and swallow the
                            // motion the rubber-band line is drawn from.
                            let widget_took = {
                                let ev = cce_ui::widget::Event::MouseButton { button: *button, state: *btn_state, x: self.cursor_x, y: self.cursor_y, local_x: self.cursor_x, local_y: self.cursor_y };
                                let ptr = self.slots.get_dyn_mut(i) as *mut (dyn WidgetHost + 'static);
                                unsafe { (*ptr).handle_event(&ev, &mut self.ui_context) }
                            };
                            if widget_took {
                                changed = true;
                                if i == PARAM_IDX {
                                    self.sync_parameters_to_project();
                                }
                                // A press on a row selects it, and what is
                                // selected is marked in the scene.
                                if i == SPREADSHEET_IDX {
                                    self.rebuild_row_markers();
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
                                        // A press on a node INSIDE the
                                        // selection picks up the whole of it
                                        // and leaves the cursor alone: moving
                                        // the anchor onto the pressed node is
                                        // exactly what collapses a region, so
                                        // the selection would be gone before
                                        // the drag began. A press on a node
                                        // outside it does move the anchor, and
                                        // that collapse is the right one —
                                        // clicking an unselected node selects
                                        // that node.
                                        let selected = self.selected_slots();
                                        if selected.len() > 1 && selected.contains(&slot_idx) {
                                            let cells: Vec<(usize, (f32, f32))> = selected
                                                .iter()
                                                .copied()
                                                .filter(|&i| i != slot_idx)
                                                .map(|i| (i, dir.children[i].position))
                                                .collect();
                                            self.node_drag_group = Some(NodeDragGroup {
                                                dragged: slot_idx,
                                                from: (pos.0 as i32, pos.1 as i32),
                                                others: cells,
                                            });
                                            // A group does not swap: one of
                                            // it trading places would leave
                                            // the rest where the offset put
                                            // them, the selection scattered.
                                            self.slots.content.inner_mut().set_swap_on_drop(false);
                                        } else {
                                            self.slots.content.inner_mut().set_swap_on_drop(true);
                                            self.grid_cursor_col = pos.0 as i32;
                                            self.grid_cursor_row = pos.1 as i32;
                                        }
                                    }
                                } else {
                                    // Empty grid: the cursor goes to the cell
                                    // pressed, on the PRESS, and the press arms
                                    // an expansion drag from it. The graph
                                    // widget is only `draggable` while it is
                                    // moving a node, so nothing else wants this
                                    // gesture — a press that never moves simply
                                    // leaves a one-cell cursor behind.
                                    let (col, row) = self.cell_at(self.cursor_x, self.cursor_y);
                                    self.grid_cursor_col = col;
                                    self.grid_cursor_row = row;
                                    self.grid_cursor_expanse = None;
                                    if !widget_took {
                                        self.grid_cursor_drag = Some((col, row));
                                    }
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
                        // The expansion drag ends by SETTLING onto what it
                        // caught — see `settle_cursor_expansion`.
                        if self.grid_cursor_drag.take().is_some() && self.settle_cursor_expansion() {
                            changed = true;
                        }
                        if self.orbit_drag.take().is_some() || self.pan_drag.take().is_some() {
                            self.broadcast_pointer();
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
                                AppDrag::HudResize { .. } => PARAM_IDX,
                                AppDrag::RightDockResize { .. } => self.pane_in_dock(Dock::Right),
                                AppDrag::SpreadsheetResize { .. }
                                | AppDrag::SpreadsheetResizeLeft { .. }
                                | AppDrag::SpreadsheetResizeRight { .. } => SPREADSHEET_IDX,
                            };
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
                            ) {
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
                                if let Some(group) = self.node_drag_group.take() {
                                    // Re-lay the companions from the cell the
                                    // dragged node actually COMMITTED to: the
                                    // widget walks it off an occupied cell, so
                                    // the preview's offset can be a cell out
                                    // from the one that landed.
                                    let dest = updated_nodes
                                        .get(group.dragged)
                                        .map(|n| (n.position.0 as i32, n.position.1 as i32))
                                        .unwrap_or(group.from);
                                    self.node_drag_group = Some(group.clone());
                                    self.drag_group_to(dest);
                                    self.node_drag_group = None;
                                    // The region travels with what it holds,
                                    // as it does for alt+hjkl — the anchor sat
                                    // still through the drag precisely so it
                                    // would still be there to move.
                                    self.shift_grid_cursor(dest.0 - group.from.0, dest.1 - group.from.1);
                                } else if let Some(sel_idx) = self.graph().selected_node() {
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
                        // Every capture the release ends is down by here:
                        // the pane under the pointer hovers again without
                        // waiting for it to move.
                        if self.broadcast_pointer() {
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

                if let Some((input_node_id, output_node_name, port)) = self.graph_mut().take_pending_connection_to_port() {
                    let path = self.current_path.clone();
                    changed |= self.connect_port(&path, &input_node_id, output_node_name, port);
                }

                // A node dropped onto a node swaps places with it: the widget
                // traded their cells (written back with the drag above), and
                // the connections are traded here.
                if let Some((a_id, b_id)) = self.graph_mut().take_pending_swap() {
                    if swap_places(self.current_dir_mut(), &a_id, &b_id) {
                        self.sync_nodes();
                        self.rebuild_scene_geometry();
                        self.sync_parameters_pane();
                        changed = true;
                    }
                }

                // A node dropped onto a wire splices in between its ends:
                // the dragged node inherits the wire's upstream as its
                // Input, and the wire's downstream node re-aims its Input
                // at the dragged node. Both rewires or neither — a splice
                // that only cut the wire would silently orphan downstream.
                if let Some((mid_id, src_name, dest_id)) = self.graph_mut().take_pending_splice() {
                    if splice_into_wire(self.current_dir_mut(), &mid_id, src_name, &dest_id) {
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
                    if let Some((input_node_id, output_node_name, port)) =
                        self.slots.content2.take_pending_connection_to_port()
                    {
                        let p2 = self.current_path2.clone();
                        changed |= self.connect_port(&p2, &input_node_id, output_node_name, port);
                    }
                    if let Some((a_id, b_id)) = self.slots.content2.take_pending_swap() {
                        let p2 = self.current_path2.clone();
                        if swap_places(self.dir_at_mut(&p2), &a_id, &b_id) {
                            self.sync_nodes();
                            self.rebuild_scene_geometry();
                            self.sync_parameters_pane();
                            changed = true;
                        }
                    }
                    if let Some((mid_id, src_name, dest_id)) =
                        self.slots.content2.take_pending_splice()
                    {
                        let p2 = self.current_path2.clone();
                        if splice_into_wire(self.dir_at_mut(&p2), &mid_id, src_name, &dest_id) {
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
                    // An open context menu — node, viewport, network or
                    // plate-corner,
                    // all riding the shared context_menu thread-local — takes
                    // Escape ahead of connection-cancel. They were
                    // mouse-dismiss only, which left Escape wired to a
                    // cancel_connecting the user could not see happening.
                    if cce_ui::widget::context_menu::is_visible() {
                        if self.node_menu_open() {
                            self.close_node_menu();
                        } else if self.viewport_menu_open() {
                            self.close_viewport_menu();
                        } else if self.param_menu_open() {
                            self.close_param_menu();
                        } else if self.playbar_menu_open() {
                            self.close_playbar_menu();
                        } else if self.network_menu_open() {
                            self.close_network_menu();
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
                                    // Every selected node, highest slot first:
                                    // removing one shifts the slots above it,
                                    // so any other order deletes the wrong
                                    // nodes from the second one on.
                                    for slot_idx in self.selected_slots().into_iter().rev() {
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
                                                // The whole selection follows the
                                                // FIRST node's flag rather than
                                                // each flipping its own: a toggle
                                                // over a mixed selection should
                                                // settle it, not shuffle it.
                                                let selected = self.selected_slots();
                                                let target = selected
                                                    .first()
                                                    .map(|&i| !self.current_dir().children[i].geometry_visible);
                                                for slot_idx in selected {
                                                    let visible = target.unwrap_or(false);
                                                    let dir = self.current_dir();
                                                    if slot_idx < dir.children.len() {
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
                                            self.copy_selected_nodes();
                                        }
                                        "x" | "X" => {
                                            self.copy_selected_nodes();
                                            // Highest slot first, as the Delete
                                            // key does and for the same reason.
                                            for slot_idx in self.selected_slots().into_iter().rev() {
                                                if self.delete_node(slot_idx) {
                                                    changed = true;
                                                }
                                            }
                                        }
                                        "v" | "V" => {
                                            if self.paste_nodes() {
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
        // Work the event path deferred to once a frame.
        if std::mem::take(&mut self.title_dirty) {
            self.update_window_title();
        }
        if self.settings_save_pending && !self.slots.dialog.slider_dragging() {
            self.settings_save_pending = false;
            self.save_settings();
        }
        let light_moved = self.sync_environment();

        // A replacement renderer left the page pane with no image; recompose
        // and re-upload it now that the frame has settled.
        if std::mem::take(&mut self.page_dirty) {
            self.rebuild_page();
        }

        // Drop-target glow animation: exponential smoothing toward the live
        // target — the position GLIDES between cells, alpha fades in while a
        // drag is in flight and out after it ends (lingering at the last
        // cell). Exponential rates are frame-rate independent.
        // The params plate eases between the plate fitted to its rows and
        // the circle it collapses to with none.
        let plate_animating = self.animate_params_plate(dt);
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

        // A detached window the user closed hands its pane back here, so a
        // closed window cannot strand the pane as a stub nothing can revive.
        let reclaimed = self.poll_detached_children();

        // A config.kdl edit repaints: until 2026-09-30 it waited for whatever
        // drew next, so an edit to the wire style showed on the next hover.
        let mut config_changed = false;
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
                config_changed = true;
                cce_ui::layout::reload_config();
                self.update_inertial_settings();
                self.update_graph_settings_from_config();
                self.rebuild_positions();
                self.apply_layout();
            } else {
                // Unchanged: nothing to re-read. Until 2026-10-06 this read
                // and parsed config.kdl every two seconds regardless.
                self.last_config_read = Instant::now();
            }
            let design_path = DesignSettings::file_path();
            if let Ok(m) = std::fs::metadata(&design_path) {
                if let Ok(mod_time) = m.modified() {
                    if Some(mod_time) != self.last_design_mod_time {
                        self.last_design_mod_time = Some(mod_time);
                         let settings = DesignSettings::load();
                         self.default_project_setting = settings.default_project.clone();
                         self.gpu_preference = settings.gpu.clone();
                         self.slots.playbar.inner_mut().repeat = settings.playbar_repeat;
                         self.slots.playbar.inner_mut().step_buttons = settings.playbar_step_buttons;
                         self.slots.playbar.inner_mut().fps = settings.playbar_fps.clamp(1.0, 120.0);
                         self.square_viewport = settings.viewport.square;
                         self.grid_thickness = settings.viewport.grid_thickness;
                         self.viewport_mut().show_grid = settings.viewport.show_grid_enabled;
                         self.viewport_mut().show_origin = settings.viewport.show_origin_enabled;
                         self.viewport_mut().show_camera_pivot = settings.viewport.show_camera_pivot_enabled;
                         self.viewport_mut().bg_color = settings.viewport.bg_color;
                         self.node_color = cce_ui::color::graph_node_color();
                         self.viewport_mut().grid_color = settings.viewport.grid_color;
                         self.origin_size = settings.viewport.origin_size;
                         self.camera_pivot_size = settings.viewport.camera_pivot_size;
                         self.graph_grid_color = cce_ui::color::graph_grid_color();

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
        // A simnet's geometry is a function of the frame, so advancing the
        // timeline invalidates the scene the way editing a node does. Gated on
        // the graph actually containing one: without this, every frame of
        // playback would rebuild the scene for a graph that cannot have
        // changed. AFTER the widget ticks, because the Playbar's tick is what
        // advances the frame during playback: until 2026-09-28 this ran
        // before them, so every tick rebuilt the scene for the frame the
        // playbar showed LAST tick and then advanced the readout — the whole
        // scene a frame behind the number on the playbar, for as long as it
        // played.
        let frame_now = self.sim_frame();
        let frame_moved = frame_now != self.last_sim_frame;
        if frame_moved {
            self.last_sim_frame = frame_now;
            if crate::geometry::contains_simnet(&self.fs_root) {
                self.rebuild_scene_geometry();
                self.viewport_dirty = true;
            } else {
                // No simulation, so no rebuild — but a node whose value is
                // an expression of the frame still reads differently, and
                // what is selected is read again.
                self.sync_selection_readouts();
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
        // ...but only when a VALUE changed: the pane's tick also reports
        // change for every frame of a scroll glide / coast and the scrollbar
        // reveal window, and syncing the whole parameter list on each of
        // those (clone + compare every param) made the params scroll choppy.
        if param_ticked {
            let streamed = self
                .slots
                .param
                .as_any_mut()
                .downcast_mut::<cce_ui::widget::ParametersBg>()
                .map_or(false, |p| p.take_tick_value_change());
            if streamed {
                self.sync_parameters_to_project();
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

        let cache_moved = self.sync_playbar_cache();

        tick_changed || panned || reclaimed || glow_animating || plate_animating || frame_moved || config_changed || light_moved || cache_moved
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
            let fill = if self.smooth_shading { &self.scene_smooth_verts } else { &self.rt_sphere_verts };
            renderer.update_mesh(meshes.spheres, bytemuck::cast_slice(fill));
            // Edge mesh for the wire pass, collected with the scene.
            renderer.update_mesh(meshes.sphere_edges, bytemuck::cast_slice(&self.scene_edge_verts));
        }

        // The spheres the markers are drawn over, at the sizes as they are.
        let radii = [
            self.group_marker_size,
            self.point_marker_size,
            self.point_marker_size * crate::render::VERTEX_MARKER_SCALE,
        ];
        if self.marker_sphere_radii != Some(radii) {
            self.marker_sphere_radii = Some(radii);
            for (mesh, r) in [meshes.group_sphere, meshes.point_sphere, meshes.vertex_sphere].into_iter().zip(radii) {
                renderer.update_mesh(mesh, bytemuck::cast_slice(&crate::geometry::marker_sphere(r)));
            }
            self.viewport_dirty = true;
        }

        // Selected-Group markers, staged by sync_nodes.
        if self.group_points_dirty {
            self.group_points_dirty = false;
            renderer.update_mesh(meshes.group_points, bytemuck::cast_slice(&self.group_point_instances));
            self.group_point_count = self.group_point_instances.len() as u32;
            self.viewport_dirty = true;
        }

        // The marked groups' markers, staged by the dialog's switches and
        // by every scene rebuild.
        if self.marked_groups_dirty {
            self.marked_groups_dirty = false;
            renderer.update_mesh(meshes.marked_points, bytemuck::cast_slice(&self.marked_group_instances));
            self.marked_group_count = self.marked_group_instances.len() as u32;
            self.viewport_dirty = true;
        }

        // The spreadsheet's selected rows, staged by their selection.
        if self.row_markers_dirty {
            self.row_markers_dirty = false;
            renderer.update_mesh(meshes.row_points, bytemuck::cast_slice(&self.row_marker_instances));
            self.row_marker_count = self.row_marker_instances.len() as u32;
            self.viewport_dirty = true;
        }

        // The point overlays, staged by rebuild_scene_geometry.
        if self.overlay_dirty {
            self.overlay_dirty = false;
            renderer.update_mesh(meshes.overlay_points, bytemuck::cast_slice(&self.overlay_marker_instances));
            self.overlay_point_count = self.overlay_marker_instances.len() as u32;
            renderer.update_mesh(meshes.vertex_points, bytemuck::cast_slice(&self.vertex_marker_instances));
            self.vertex_marker_count = self.vertex_marker_instances.len() as u32;
            renderer
                .update_mesh(meshes.overlay_normals, bytemuck::cast_slice(&self.overlay_normal_verts));
            self.overlay_normal_count = self.overlay_normal_verts.len() as u32;
            self.viewport_dirty = true;
        }

        // Pull arrows, staged by sync_pull_arrows.
        if self.pull_arrows_dirty {
            self.pull_arrows_dirty = false;
            renderer.update_mesh(meshes.pull_arrows, bytemuck::cast_slice(&self.pull_arrow_verts));
            self.pull_arrow_count = self.pull_arrow_verts.len() as u32;
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
        // Re-uploaded on the next tick, not here: this runs before the frame
        // has settled.
        self.page_dirty = true;
        true
    }

    pub fn init_renderer(&mut self, renderer: &mut cce_ui::vk::VkRenderer) {
        let linear_grid_color = cce_ui::colors::to_linear_rgb(self.grid_color);
        let grid_verts = grid_vertices(self.grid_thickness, linear_grid_color);
        let origin_verts = origin_vectors_vertices(self.origin_size);
        let pivot_verts = camera_pivot_vertices(self.camera_pivot_size);
        let bg_verts =
            Self::viewport_bg_vertices(cce_ui::colors::to_linear_rgb(self.viewport().bg_color));
        self.meshes = Some(SceneMeshes {
            viewport_bg: renderer.create_mesh(bytemuck::cast_slice(&bg_verts)),
            spheres: renderer.create_mesh(&[]),
            sphere_edges: renderer.create_mesh(&[]),
            grid: renderer.create_mesh(bytemuck::cast_slice(&grid_verts)),
            origin: renderer.create_mesh(bytemuck::cast_slice(&origin_verts)),
            pivot: renderer.create_mesh(bytemuck::cast_slice(&pivot_verts)),
            group_sphere: renderer.create_mesh(&[]),
            point_sphere: renderer.create_mesh(&[]),
            vertex_sphere: renderer.create_mesh(&[]),
            group_points: renderer.create_mesh(&[]),
            row_points: renderer.create_mesh(&[]),
            marked_points: renderer.create_mesh(&[]),
            overlay_points: renderer.create_mesh(&[]),
            vertex_points: renderer.create_mesh(&[]),
            overlay_normals: renderer.create_mesh(&[]),
            // Seeded with what is staged: a replacement renderer gets the
            // arrows back without waiting for the selection to change.
            pull_arrows: renderer.create_mesh(bytemuck::cast_slice(&self.pull_arrow_verts)),
        });
        self.pull_arrow_count = self.pull_arrow_verts.len() as u32;
        // Scene geometry built during `State::new` (before the renderer
        // existed) uploads on the first frame's flush, the markers with it:
        // the spheres at their sizes and the instances already staged.
        self.spheres_dirty = !self.rt_sphere_verts.is_empty();
        self.marker_sphere_radii = None;
        self.group_points_dirty = true;
        self.marked_groups_dirty = true;
        self.row_markers_dirty = true;
        self.overlay_dirty = true;
        self.viewport_dirty = true;
    }

    /// Frame staging (engine `stage_renderer` hook): pending meshes, text, and
    /// the 3D scene / RT pane. Returns true while the path tracer is still
    /// refining, to keep frames coming. The renderer's window-corner clip is
    /// left at the engine default (0) — the compositor rounds the window.
    /// The pose of the camera the viewport looks through, as
    /// `(position, rotation in degrees, pivot)`: the active camera node's
    /// Position, Rotation and Pivot when it lives in the current directory,
    /// the Default Camera's fixed eye ray from the viewport's own pivot
    /// otherwise. The Default Camera's ORBIT is not in here — it is the
    /// viewport widget's `rotation_x` / `rotation_y`, which `get_matrices`
    /// folds in. Split out of the stage pass so a window with no 3D canvas
    /// (a detached pane) can ask too.
    pub fn active_camera_pose(&self) -> (Vec3, Vec3, Vec3) {
        // The Default Camera: the fixed eye ray from the viewport's own
        // pivot (`Viewport3D::pivot`, the origin until Frame All moves
        // it). Also what a NAMED camera that is not in this directory
        // resolves to — a camera node applies where it lives.
        let mut pivot = self.viewport().pivot;
        let mut camera_pos = pivot + Vec3::new(2.5, 1.8, 2.5);
        let mut rx = 0.0f32;
        let mut ry = 0.0f32;
        let mut rz = 0.0f32;
        if self.active_camera != "Default Camera" {
            if let Some(node) = self.camera_level().children.iter().find(|c| c.node_type == "camera" && c.name == self.active_camera) {
                let mut cx = 2.5f32;
                let mut cy = 1.8f32;
                let mut cz = 2.5f32;
                for p in &node.params {
                    if p.name == "position" {
                        let parts: Vec<&str> = p.text()
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
                    } else if p.name == "rotation" {
                        let parts: Vec<&str> = p.text()
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
                    } else if p.name == "pivot" {
                        let parts: Vec<&str> = p.text()
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

        (camera_pos, Vec3::new(rx, ry, rz), pivot)
    }

    /// Whether this window shares the project with another over the sync
    /// channel — a detached window, or the main one with a pane or the
    /// circular network detached. The one test the autosave requests, the
    /// poll and the exit save share: until 2026-09-29 the REQUESTS named
    /// only the circular network, so with the parameters, spreadsheet or
    /// playbar detached neither window ever wrote the channel again after
    /// the detach, and the detached window showed what it started with.
    pub fn syncing_windows(&self) -> bool {
        self.is_detached_network
            || self.detached_circular_network
            || self.detached_pane.is_some()
            || self.detached_panes.iter().any(|d| *d)
    }

    /// [`Self::sync_trackball_view`] from the camera this window knows
    /// of, and the word to the other windows when it moved: the MAIN
    /// window asks for an autosave of the sync channel, which carries the
    /// camera (the active camera's name and node, the Default Camera's
    /// orbit), so a detached parameters window's trackballs turn with the
    /// viewport they cannot see. A detached window only reads: it has no
    /// camera of its own to tell anyone about.
    pub fn sync_trackball_view_from_camera(&mut self) -> bool {
        let (position, rotation, pivot) = self.active_camera_pose();
        let moved = self.sync_trackball_view(position, rotation, pivot);
        if moved && self.syncing_windows() && !self.is_detached_network && self.detached_pane.is_none() {
            self.needs_autosave = true;
        }
        moved
    }

    /// What a detached window does once, at startup: take the sync channel
    /// WHOLE — the tree, the selection, the camera. `State::new` seeds the
    /// tree, camera name, pan and path from the bundled file and records
    /// its mtime, so without this the window waited for a change that the
    /// file it had just been handed was never going to have, and a detached
    /// parameters window opened with no node selected: an empty pane.
    pub fn seed_detached_window(&mut self, channel: &std::path::Path) {
        if let Err(e) = self.load_sync_channel(channel, false) {
            eprintln!("Failed to read the sync channel at startup: {e:?}");
        }
        self.sync_trackball_view_from_camera();
    }

    /// See the params pane's trackballs from the camera the viewport is
    /// looking through: the view matrix's rotation, as the camera's right,
    /// its up and the direction toward it, in the scene's space — which is
    /// the space a node's vector is in, the model matrix being the
    /// identity. The vector on the ball then lies as the pull arrows do in
    /// the viewport beside it, and rolling the ball to the right swings
    /// the vector to the right of the screen. Returns whether the view
    /// moved, so the stage pass can ask for the frame that shows it.
    pub fn sync_trackball_view(&mut self, camera_pos: Vec3, rotation: Vec3, pivot: Vec3) -> bool {
        let (_, view, model) = self.viewport().get_matrices(1.0, Some(camera_pos), Some(rotation), Some(pivot));
        let m = view * model;
        // Row i of the matrix is the view's axis i, as a direction of the
        // scene: x right, y up, z back toward the camera (a right-handed
        // view looks down its own -Z).
        let row = |i: usize| [m.x_axis[i], m.y_axis[i], m.z_axis[i]];
        self.slots.param.inner_mut().set_trackball_view([row(0), row(1), row(2)])
    }

    pub fn stage_frame(&mut self, renderer: &mut cce_ui::vk::VkRenderer) -> bool {
        self.flush_pending_meshes(renderer);
        let meshes = self.meshes.expect("stage_frame before renderer_init");
        // Whether the trackballs' view moved this pass. The pane may have
        // been painted for this frame already, so a moved view asks for one
        // more — the ball trails the camera by a frame, never by more.
        let mut trackball_moved = false;
        // A detached pane has no 3D canvas to resolve a camera for, but the
        // sync channel gave it the main window's: see the balls from that.
        if self.detached_pane.is_some() {
            trackball_moved = self.sync_trackball_view_from_camera();
        }

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
                let (camera_pos, rotation, pivot) = self.active_camera_pose();
                let (rx, ry, rz) = (rotation.x, rotation.y, rotation.z);
                trackball_moved = self.sync_trackball_view_from_camera();

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
                    || self.last_viewport_wire_opacity != self.wire_opacity
                    || self.last_viewport_wire_width != self.wire_width;

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
                    self.last_scene_eye = (view_mat * model).inverse().transform_point3(Vec3::ZERO);
                    self.sync_point_number_alpha(mvp_mat, self.last_scene_eye);

                    // The camera-pivot marker is WORLD-FIXED at the pivot point, like
                    // the origin gizmo. Its old yaw rotation existed to keep it glued
                    // to the model-matrix orbit's rotating world; with the camera
                    // doing the moving it must not rotate, or its axis beams read as
                    // scene geometry spinning with the camera over the stationary grid.
                    let model_pivot = Mat4::from_translation(pivot);
                    let mvp_pivot = (proj * view_mat * model_pivot).to_cols_array_2d();

                    // Same draw order as the wgpu pass: bg quad, grid, origin,
                    // pivot, spheres. The node geometry (points +
                    // spheres) carries the Render node's Opacity; scene
                    // furniture stays opaque.
                    const NO_TINT: [f32; 4] = [0.0; 4];
                    let geo_opacity = self.geo_opacity.clamp(0.0, 1.0);

                    // See-through fill: re-sort the triangles back to front
                    // for THIS eye and upload them over the flush's order,
                    // whenever the geometry, the shading or the eye moved.
                    // The eye is taken in mesh space (the inverse of view *
                    // model), the space the triangles are in.
                    let see_through = self.see_through_active();
                    if see_through && self.vertex_count_spheres > 0 {
                        let eye = (view_mat * model).inverse().transform_point3(Vec3::ZERO);
                        let key = (self.rt_geometry_version, self.smooth_shading, eye.to_array());
                        if self.sorted_fill_key != Some(key) {
                            let src = if self.smooth_shading { &self.scene_smooth_verts } else { &self.rt_sphere_verts };
                            let sorted = crate::geometry::sort_triangles_back_to_front(src, eye);
                            renderer.update_mesh(meshes.spheres, bytemuck::cast_slice(&sorted));
                            self.sorted_fill_key = Some(key);
                        }
                    } else {
                        // The sorted order is still a valid opaque mesh, so
                        // nothing re-uploads; the next entry just sorts anew.
                        self.sorted_fill_key = None;
                    }
                    let mut draws = vec![SceneDraw { mesh: meshes.viewport_bg, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: false, instances: None }];
                    if self.viewport().show_grid {
                        draws.push(SceneDraw { mesh: meshes.grid, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: false, instances: None });
                    }
                    if self.viewport().show_origin {
                        draws.push(SceneDraw { mesh: meshes.origin, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: false, instances: None });
                    }
                    if self.viewport().show_camera_pivot {
                        draws.push(SceneDraw { mesh: meshes.pivot, mvp: mvp_pivot, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: false, instances: None });
                    }
                    // Selected-Group markers: full-opacity selection feedback,
                    // deliberately outside the Render node's Opacity.
                    if self.group_point_count > 0 {
                        draws.push(SceneDraw { mesh: meshes.group_sphere, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: false, instances: Some(meshes.group_points) });
                    }
                    // The marked groups: the same amber as a selected
                    // group's markers, and the same tier.
                    if self.marked_group_count > 0 {
                        draws.push(SceneDraw { mesh: meshes.group_sphere, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: false, instances: Some(meshes.marked_points) });
                    }
                    // The spreadsheet's selected rows, while it is shown.
                    if self.show_spreadsheet && self.row_marker_count > 0 {
                        draws.push(SceneDraw { mesh: meshes.group_sphere, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: false, instances: Some(meshes.row_points) });
                    }
                    // Show Point Markers and Show Vertex Markers, the same
                    // full-opacity tier, each its sphere instanced.
                    if self.overlay_point_count > 0 {
                        draws.push(SceneDraw { mesh: meshes.point_sphere, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: false, instances: Some(meshes.overlay_points) });
                    }
                    if self.vertex_marker_count > 0 {
                        draws.push(SceneDraw { mesh: meshes.vertex_sphere, mvp, wireframe: false, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: false, instances: Some(meshes.vertex_points) });
                    }
                    // The page the level shows stands in the scene as an
                    // image: after the furniture and the markers, which are
                    // opaque and may show through it, and before the
                    // geometry, whose fill may be translucent over it.
                    let image_slot = draws.len() as u32;
                    if self.vertex_count_spheres > 0 {
                        // The line annotations go UNDER the geometry, as
                        // the markers above do, and write depth (the wire
                        // draw's `see_through` selects the depth-writing
                        // line pipeline): what is nearer then blends over
                        // them, so a whisker behind a translucent face or
                        // wire is dimmed by it, and what is farther fails
                        // the test and leaves them whole. Drawn after the
                        // fill they were hidden outright behind one that
                        // writes depth, and painted at full strength over
                        // the near faces of one that does not.
                        //
                        // Show Point Normals: thin cyan whiskers,
                        // width deliberately fixed (a chunky Wire Width is a
                        // wireframe styling choice, not a normals one).
                        if self.overlay_normal_count > 0 {
                            draws.push(SceneDraw { mesh: meshes.overlay_normals, mvp, wireframe: true, wire_tint: NO_TINT, opacity: 1.0, line_width: 1.0, wire_base_width: 0.0, prelit: false, see_through: true, instances: None });
                        }
                        // Pull arrows: selection feedback, like the group
                        // markers — full opacity, a little heavier than the
                        // whiskers.
                        if self.pull_arrow_count > 0 {
                            draws.push(SceneDraw { mesh: meshes.pull_arrows, mvp, wireframe: true, wire_tint: NO_TINT, opacity: 1.0, line_width: 2.0, wire_base_width: 0.0, prelit: false, see_through: true, instances: None });
                        }
                        // With wires coming, the fill is pushed back by its
                        // slope-scaled offset so the lattice reads solid.
                        let base = if self.wireframe { self.wire_width } else { 0.0 };
                        let mut fill = Some(SceneDraw { mesh: meshes.spheres, mvp, wireframe: false, wire_tint: NO_TINT, opacity: geo_opacity, line_width: 1.0, wire_base_width: base, prelit: self.smooth_shading, see_through, instances: None });
                        // A see-through fill writes no depth, so wires drawn
                        // AFTER it pass everywhere and the far side's paint
                        // over the near faces. Seen through, the wires go
                        // FIRST and write depth instead: each fill layer then
                        // lands only where it is nearer than the wire under
                        // it, so a far wire is dimmed by the layers in front
                        // of it and a near wire by none (the offset above
                        // puts it ahead of its own face).
                        if !see_through {
                            draws.extend(fill.take());
                        }
                        if self.wireframe {
                            // The wire pass rides ON TOP of the fill (never
                            // replaces it). Single-color mode replaces the
                            // fragment color outright; geometry-color mode
                            // draws the raw vertex colors — the wire pass is
                            // unlit, so they read brighter than the shaded
                            // fill beneath. Far-side wires that clear the
                            // depth test near the limb show as their own
                            // (complementary) colors — a soft x-ray read.
                            // The wires' opacity is Wire Opacity in both
                            // modes; the geometry Opacity slider is
                            // polygons-only.
                            let tint = if self.wire_single_color {
                                [self.wire_color[0], self.wire_color[1], self.wire_color[2], 1.0]
                            } else {
                                [0.0, 0.0, 0.0, 0.0]
                            };
                            let wire_alpha = self.wire_opacity.clamp(0.0, 1.0);
                            draws.push(SceneDraw { mesh: meshes.sphere_edges, mvp, wireframe: true, wire_tint: tint, opacity: wire_alpha, line_width: self.wire_width, wire_base_width: 0.0, prelit: false, see_through, instances: None });
                        }
                        draws.extend(fill);
                    }
                    // One light for both views: the environment's sun.
                    renderer.set_scene_light(self.environment.sun_direction.to_array());
                    renderer.stage_scene((sx, sy, cw, ch), draws);
                    if let (Some(image), Some(shown)) = (self.page_image, &self.page_shown) {
                        renderer.stage_scene_images(vec![cce_ui::vk::SceneImage {
                            image,
                            corners: shown.world_corners(self.world_unit_mm()),
                            mvp,
                            opacity: 1.0,
                            before: image_slot,
                        }]);
                    }
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
                    self.last_viewport_wire_opacity = self.wire_opacity;
                    self.last_viewport_wire_width = self.wire_width;
                    self.last_viewport_rt_mode = rt_mode;
                    self.viewport_dirty = false;
                }

                // Path-traced mode: staged EVERY frame (each one adds a
                // sample); the renderer resets the accumulation itself when
                // the camera/pane changes, so camera drags stay interactive
                // (1-spp noise) and stillness converges.
                if rt_mode {
                    let unit_mm = self.world_unit_mm();
                    let key = (self.rt_geometry_version, self.page_version, unit_mm.to_bits());
                    if self.last_rt_scene_key != Some(key) {
                        let (rt_tris, rt_mats) = self.collect_rt_scene();
                        // The image the raster pass draws, traced: the same
                        // upload, at the same corners.
                        let image = self.page_image.zip(self.page_shown.as_ref()).map(
                            |(image, shown)| cce_ui::vk::RtImage {
                                image,
                                corners: shown.world_corners(unit_mm),
                                opacity: 1.0,
                            },
                        );
                        renderer.set_rt_scene_with_image(&rt_tris, &rt_mats, image);
                        self.last_rt_scene_key = Some(key);
                    }
                    let aspect = cw as f32 / ch as f32;
                    let (proj, view_mat, model) = self.viewport().get_matrices(aspect, Some(camera_pos), Some(Vec3::new(rx, ry, rz)), Some(pivot));
                    let inv_mvp = (proj * view_mat * model).inverse().to_cols_array_2d();
                    // Behind the scene, the Background Color, as the raster
                    // pass draws it (since 2026-10-02; it was the tracer's
                    // sky). The sky still lights the scene.
                    renderer.set_rt_background(Some(cce_ui::colors::to_linear_rgb(self.viewport().bg_color)));
                    renderer.set_rt_environment(self.environment.to_rt());
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
        // tracer is still refining, and for the one that shows a trackball
        // its new view.
        trackball_moved
            || (!self.is_detached_network
                && self.detached_pane.is_none()
                && self.show_viewport
                && self.viewport().rt_mode
                && renderer.rt_accumulating())
    }
}

