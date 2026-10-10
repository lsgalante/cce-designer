//! The app's settings: `ViewportSettings`, `RenderSettings` and
//! `DesignSettings` with their defaults, the grid geometry, the GPU
//! preference, and their KDL load and save. Split out of app.rs on
//! 2026-10-10; app.rs re-exports it.
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

pub(crate) fn default_grid_thickness() -> f32 { 0.03 }
pub(crate) fn default_grid_color() -> [f32; 3] { [0.35, 0.35, 0.40] }

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

pub(crate) fn default_point_marker_size() -> f32 {
    0.02
}

pub(crate) fn default_point_marker_color() -> [f32; 3] {
    [0.85, 0.85, 1.0]
}

pub(crate) fn default_world_unit() -> String {
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
pub(crate) struct StoredRenderSettings {
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

pub(crate) fn default_group_marker_size() -> f32 {
    0.025
}

/// The far end of Group Marker Size's range, in world units.
pub(crate) const GROUP_MARKER_SIZE_MAX: f32 = 0.2;

pub(crate) fn default_pull_arrow_scale() -> f32 {
    1.0
}

pub(crate) fn default_wire_color() -> [f32; 3] {
    [0.0, 0.0, 0.0]
}

pub(crate) fn default_wire_width() -> f32 {
    1.0
}

pub(crate) fn default_geo_opacity() -> f32 {
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

pub(crate) fn default_params_plate() -> bool {
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

pub(crate) fn default_playbar_fps() -> f32 {
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

pub(crate) fn default_true() -> bool {
    true
}

/// The GPU setting's options. "integrated" is cce-ui's own default
/// (`CCE_VK_DEVICE` unset); "discrete" asks for the dedicated card.
pub const GPU_CHOICES: &[&str] = &["integrated", "discrete"];

pub(crate) fn default_gpu() -> String {
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

pub(crate) fn float_array_to_hex(rgb: &[f32; 3]) -> String {
    let r = (rgb[0] * 255.0).clamp(0.0, 255.0).round() as u8;
    let g = (rgb[1] * 255.0).clamp(0.0, 255.0).round() as u8;
    let b = (rgb[2] * 255.0).clamp(0.0, 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

pub(crate) fn hex_to_float_array(hex: &str) -> Option<[f32; 3]> {
    cce_ui::color::parse_hex_rgb(hex)
}

pub(crate) fn hex_to_float_array4(hex: &str) -> Option<[f32; 4]> {
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

    pub(crate) fn load() -> Self {
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

    pub(crate) fn save(&self) {
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
