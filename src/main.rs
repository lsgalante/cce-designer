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

use clear_ui::widget::{Breadcrumb, Canvas, ColorSelector, ContentBg, MenuBar, Node, ParametersBg, Spinbox, Splitter, Spreadsheet, StatusBar, TextLabel, Toggle, ViewportBg, Widget, GraphNode, Graph};
use clear_ui::colors;
use clear_ui::layout::{RenderTarget, Section};

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
const PARAM_IDX: usize = 5;
const CANVAS_IDX: usize = 6;
const LEFT_MENUBAR_IDX: usize = 7;
const RIGHT_MENUBAR_IDX: usize = 8;
const PARAM_MENUBAR_IDX: usize = 9;
const STATUS_IDX: usize = 10;
const BREADCRUMB_IDX: usize = 11;
const CONFIG_DIALOG_IDX: usize = 12;
const NODE_PALETTE_IDX: usize = 13;
const SPREADSHEET_IDX: usize = 14;
const SPREADSHEET_MENUBAR_IDX: usize = 15;


const HEADER_H: f32 = 26.0;
const STATUS_H: f32 = 28.0;
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
        }
    }
}

impl DesignSettings {
    fn file_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lsgalante".to_string());
        let mut path = std::path::PathBuf::from(home);
        path.push(".config");
        path.push("ccec");
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

enum ConfigControl {
    Header(&'static str),
    Text(&'static str),
    Toggle {
        #[allow(dead_code)]
        label: &'static str,
        #[allow(dead_code)]
        id: usize,
        #[allow(dead_code)]
        enabled: bool,
    },
    ColorSelector,
    Shortcut {
        key: &'static str,
        desc: &'static str,
    },
}

struct PageLayout {
    labels: Vec<TextLabel>,
}

struct SectionCollector {
    quads: Vec<(f32, f32, f32, f32, [f32; 4])>,
    labels: Vec<TextLabel>,
    active: bool,
}

impl RenderTarget for SectionCollector {
    fn rect(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32) {
        if self.active {
            self.quads.push((x, y, w, h, color));
        }
    }

    fn text(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4]) {
        if self.active {
            let u8_color = [
                (color[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                (color[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                (color[2] * 255.0).round().clamp(0.0, 255.0) as u8,
            ];
            self.labels.push(TextLabel {
                text: content.to_string(),
                x,
                y,
                font_size: size,
                color: u8_color,
            });
        }
    }
}

struct ConfigDialog {
    x: f32, y: f32, w: f32, h: f32,
    hovered: bool,
    visible: bool,
    close_hovered: bool,
    panel_w: f32,
    panel_h: f32,
    active_page: usize,
    hovered_tab: Option<usize>,
    toggle_grid_snap: Toggle,
    toggle_network_grid: Toggle,
    grid_size_x: f32,
    grid_size_y: f32,
    skipped_row_h: f32,
    skipped_col_w: f32,
    toggle_show_grid: Toggle,
    toggle_show_cube: Toggle,
    toggle_show_origin: Toggle,
    toggle_show_camera_pivot: Toggle,
    viewport_r: f32,
    viewport_g: f32,
    viewport_b: f32,
    color_selector: ColorSelector,
    last_sent_color: [u8; 3],
    node_color_selector: ColorSelector,
    last_sent_node_color: [u8; 3],
    origin_size: f32,
    camera_pivot_size: f32,
    grid_thickness: f32,
    spin_grid_x: Spinbox,
    spin_grid_y: Spinbox,
    spin_skipped_row_h: Spinbox,
    spin_skipped_col_w: Spinbox,
    spin_origin_size: Spinbox,
    spin_grid_thickness: Spinbox,
    spin_camera_pivot_size: Spinbox,
    scroll_y: f32,
    scroll_velocity: f32,
    section_quads: Vec<(f32, f32, f32, f32, [f32; 4])>,
    section_labels: Vec<TextLabel>,
}

impl ConfigDialog {
    fn new(settings: &DesignSettings) -> Self {
        let vr = settings.viewport_bg_color[0];
        let vg = settings.viewport_bg_color[1];
        let vb = settings.viewport_bg_color[2];
        let r = (vr * 255.0).round().clamp(0.0, 255.0) as u8;
        let g = (vg * 255.0).round().clamp(0.0, 255.0) as u8;
        let b = (vb * 255.0).round().clamp(0.0, 255.0) as u8;
        let nr = (settings.node_color[0] * 255.0).round().clamp(0.0, 255.0) as u8;
        let ng = (settings.node_color[1] * 255.0).round().clamp(0.0, 255.0) as u8;
        let nb = (settings.node_color[2] * 255.0).round().clamp(0.0, 255.0) as u8;

        let mut toggle_grid_snap = Toggle::new().with_label("Snap to Grid");
        toggle_grid_snap.set_toggled(settings.grid_snap_enabled);
        let mut toggle_network_grid = Toggle::new().with_label("Show Grid");
        toggle_network_grid.set_toggled(settings.network_grid_enabled);
        let mut toggle_show_grid = Toggle::new().with_label("Show Grid Guide");
        toggle_show_grid.set_toggled(settings.show_grid_enabled);
        let mut toggle_show_cube = Toggle::new().with_label("Show Reference Cube");
        toggle_show_cube.set_toggled(settings.show_cube_enabled);
        let mut toggle_show_origin = Toggle::new().with_label("Show Origin Axes");
        toggle_show_origin.set_toggled(settings.show_origin_enabled);
        let mut toggle_show_camera_pivot = Toggle::new().with_label("Show Camera Pivot");
        toggle_show_camera_pivot.set_toggled(settings.show_camera_pivot_enabled);

        let mut dialog = Self {
            x: 0.0, y: 0.0, w: 0.0, h: 0.0,
            hovered: false, visible: false,
            close_hovered: false,
            panel_w: 800.0, panel_h: 600.0,
            active_page: 0,
            hovered_tab: None,
            toggle_grid_snap,
            toggle_network_grid,
            grid_size_x: settings.grid_size_x,
            grid_size_y: settings.grid_size_y,
            skipped_row_h: settings.skipped_row_h,
            skipped_col_w: settings.skipped_col_w,
            toggle_show_grid,
            toggle_show_cube,
            toggle_show_origin,
            toggle_show_camera_pivot,
            viewport_r: vr,
            viewport_g: vg,
            viewport_b: vb,
            color_selector: ColorSelector::new([r, g, b]).with_label("Viewport Background"),
            last_sent_color: [r, g, b],
            node_color_selector: ColorSelector::new([nr, ng, nb]).with_label("Node Base Color"),
            last_sent_node_color: [nr, ng, nb],
            origin_size: settings.origin_size,
            camera_pivot_size: settings.camera_pivot_size,
            grid_thickness: settings.grid_thickness,
            spin_grid_x: Spinbox::new(settings.grid_size_x as i32, 10, 200, 5).with_label("Grid X:"),
            spin_grid_y: Spinbox::new(settings.grid_size_y as i32, 5, 100, 5).with_label("Grid Y:"),
            spin_skipped_row_h: Spinbox::new(settings.skipped_row_h as i32, 0, 150, 5).with_label("Skipped Row H:"),
            spin_skipped_col_w: Spinbox::new(settings.skipped_col_w as i32, 0, 150, 5).with_label("Skipped Col W:"),
            spin_origin_size: Spinbox::new((settings.origin_size * 10.0).round() as i32, 1, 50, 1).with_label("Origin Guide Size:").with_decimals(1),
            spin_grid_thickness: Spinbox::new((settings.grid_thickness * 1000.0).round() as i32, 2, 200, 5).with_label("Grid Thickness:").with_decimals(3),
            spin_camera_pivot_size: Spinbox::new((settings.camera_pivot_size * 10.0).round() as i32, 1, 50, 1).with_label("Camera Pivot Size:").with_decimals(1),
            scroll_y: 0.0,
            scroll_velocity: 0.0,
            section_quads: Vec::new(),
            section_labels: Vec::new(),
        };
        dialog.update_child_layouts();
        dialog
    }

    fn panel_rect(&self) -> (f32, f32, f32, f32) {
        let px = self.x + (self.w - self.panel_w) / 2.0;
        let py = self.y + (self.h - self.panel_h) / 2.0;
        (px, py, self.panel_w, self.panel_h)
    }

    fn close_rect(&self, px: f32, pw: f32, py: f32) -> (f32, f32, f32, f32) {
        (px + pw - 28.0, py + 8.0, 20.0, 20.0)
    }

    fn tab_rects(&self, px: f32, py: f32) -> [(f32, f32, f32, f32); 4] {
        let tab_y = py + 30.0;
        let tab_h = 22.0;
        let general_w = "General".len() as f32 * 7.5 + 16.0;
        let network_w = "Network".len() as f32 * 7.5 + 16.0;
        let viewport_w = "Viewport".len() as f32 * 7.5 + 16.0;
        let bindings_w = "Bindings".len() as f32 * 7.5 + 16.0;
        let gap = 4.0;
        [
            (px + 16.0, tab_y, general_w, tab_h),
            (px + 16.0 + general_w + gap, tab_y, network_w, tab_h),
            (px + 16.0 + general_w + gap + network_w + gap, tab_y, viewport_w, tab_h),
            (px + 16.0 + general_w + gap + network_w + gap + viewport_w + gap, tab_y, bindings_w, tab_h),
        ]
    }

    fn get_page_controls(&self, page: usize) -> Vec<ConfigControl> {
        match page {
            0 => vec![
                ConfigControl::Text("General settings for Clear Design Interface"),
            ],
            1 => vec![],
            2 => vec![],
            3 => vec![
                ConfigControl::Header("Keyboard Shortcuts"),
                ConfigControl::Shortcut { key: "Ctrl+G", desc: "Toggle Grid (viewport)" },
                ConfigControl::Shortcut { key: "Ctrl+E", desc: "Toggle Cube" },
                ConfigControl::Shortcut { key: "Ctrl+A", desc: "Square Viewport Aspect" },
                ConfigControl::Shortcut { key: "Ctrl+,", desc: "Configure Dialog" },
            ],
            _ => vec![],
        }
    }

    fn compute_layout(&self, px: f32, py: f32) -> PageLayout {
        let mut labels = Vec::new();

        let content_y = py + 60.0;
        let mut y = content_y;

        let controls = self.get_page_controls(self.active_page);
        for control in controls {
            match control {
                ConfigControl::Header(title) => {
                    labels.push(TextLabel {
                        text: title.to_string(),
                        x: px + 16.0,
                        y,
                        font_size: 13.0,
                        color: [0xcc, 0xcc, 0xd4],
                    });
                    y += 24.0;
                }
                ConfigControl::Text(msg) => {
                    labels.push(TextLabel {
                        text: msg.to_string(),
                        x: px + 16.0,
                        y,
                        font_size: 12.0,
                        color: [0x88, 0x88, 0x99],
                    });
                    y += 20.0;
                }
                ConfigControl::Toggle { .. } => {
                    // Handled as standard widget in update_child_layouts()
                }
                ConfigControl::ColorSelector => {
                    // Handled as standard widget in update_child_layouts()
                }
                ConfigControl::Shortcut { key, desc } => {
                    labels.push(TextLabel {
                        text: key.to_string(),
                        x: px + 32.0,
                        y,
                        font_size: 12.0,
                        color: [0xdd, 0xdd, 0x88],
                    });
                    labels.push(TextLabel {
                        text: desc.to_string(),
                        x: px + 130.0,
                        y,
                        font_size: 12.0,
                        color: [0xaa, 0xaa, 0xbb],
                    });
                    y += 22.0;
                }
            }
        }

        PageLayout {
            labels,
        }
    }

    fn update_child_layouts(&mut self) {
        let (px, py, _, _) = self.panel_rect();

        self.section_quads.clear();
        self.section_labels.clear();
        let mut collector = SectionCollector {
            quads: std::mem::take(&mut self.section_quads),
            labels: std::mem::take(&mut self.section_labels),
            active: true,
        };

        let mut max_scroll_y = 0.0;

        // --- Page 1 (Network) Layout ---
        if self.active_page == 1 {
            let y_start = py + 65.0;

            // Section 1: Network Configuration Section (aligned vertically)
            collector.active = true;
            let mut sec1 = Section::new(&mut collector, px + 20.0, y_start, 760.0, "Network Configuration");
            collector.active = false;
            sec1.widget(&mut collector, &mut self.toggle_grid_snap, 14.0, 44.0, 22.0);
            sec1.spacing(8.0);
            sec1.widget(&mut collector, &mut self.toggle_network_grid, 14.0, 44.0, 22.0);
            sec1.spacing(8.0);
            collector.active = true;
            let y1 = sec1.finish(&mut collector);

            // Section 2: Grid Spacing Section (aligned vertically under sec1)
            collector.active = true;
            let mut sec2 = Section::new(&mut collector, px + 20.0, y1 + 12.0, 760.0, "Grid Spacing");
            collector.active = false;
            sec2.widget(&mut collector, &mut self.spin_grid_x, 14.0, 240.0, 22.0);
            sec2.spacing(8.0);
            sec2.widget(&mut collector, &mut self.spin_grid_y, 14.0, 240.0, 22.0);
            sec2.spacing(8.0);
            sec2.widget(&mut collector, &mut self.spin_skipped_row_h, 14.0, 240.0, 22.0);
            sec2.spacing(8.0);
            sec2.widget(&mut collector, &mut self.spin_skipped_col_w, 14.0, 240.0, 22.0);
            sec2.spacing(8.0);
            collector.active = true;
            let y2 = sec2.finish(&mut collector);

            // Section 3: Node Styling Section (aligned vertically under sec2)
            collector.active = true;
            let mut sec3 = Section::new(&mut collector, px + 20.0, y2 + 12.0, 760.0, "Node Styling");
            collector.active = false;
            sec3.widget(&mut collector, &mut self.node_color_selector, 14.0, 240.0, 24.0);
            sec3.spacing(8.0);
            collector.active = true;
            let y3 = sec3.finish(&mut collector);

            let total_h = y3 - (py + 65.0);
            let visible_h = 510.0;
            max_scroll_y = (total_h - visible_h).max(0.0);
        }

        // --- Page 2 (Viewport) Layout ---
        if self.active_page == 2 {
            let y_start = py + 65.0;

            // Section 1: Guides & Display Section (aligned vertically)
            collector.active = true;
            let mut sec1 = Section::new(&mut collector, px + 20.0, y_start, 760.0, "Guides & Display");
            collector.active = false;
            sec1.widget(&mut collector, &mut self.toggle_show_grid, 14.0, 44.0, 22.0);
            sec1.spacing(8.0);
            sec1.widget(&mut collector, &mut self.toggle_show_cube, 14.0, 44.0, 22.0);
            sec1.spacing(8.0);
            sec1.widget(&mut collector, &mut self.toggle_show_origin, 14.0, 44.0, 22.0);
            sec1.spacing(8.0);
            sec1.widget(&mut collector, &mut self.toggle_show_camera_pivot, 14.0, 44.0, 22.0);
            sec1.spacing(8.0);
            collector.active = true;
            let y1 = sec1.finish(&mut collector);

            // Section 2: Viewport Settings Section (aligned vertically under sec1)
            collector.active = true;
            let mut sec2 = Section::new(&mut collector, px + 20.0, y1 + 12.0, 760.0, "Viewport Settings");
            collector.active = false;
            sec2.widget(&mut collector, &mut self.spin_grid_thickness, 14.0, 240.0, 22.0);
            sec2.spacing(8.0);
            sec2.widget(&mut collector, &mut self.spin_origin_size, 14.0, 240.0, 22.0);
            sec2.spacing(8.0);
            sec2.widget(&mut collector, &mut self.spin_camera_pivot_size, 14.0, 240.0, 22.0);
            sec2.spacing(12.0); // Slightly more space before color selector
            sec2.widget(&mut collector, &mut self.color_selector, 14.0, 240.0, 24.0);
            sec2.spacing(8.0);
            collector.active = true;
            let y2 = sec2.finish(&mut collector);

            let total_h = y2 - (py + 65.0);
            let visible_h = 510.0;
            max_scroll_y = (total_h - visible_h).max(0.0);
        }

        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll_y);

        // Apply scroll offset shifting to all layout elements
        if self.scroll_y > 0.0 {
            for quad in &mut collector.quads {
                quad.1 -= self.scroll_y;
            }
            for label in &mut collector.labels {
                label.y -= self.scroll_y;
            }

            let shift = self.scroll_y;
            let shift_widget = |w: &mut dyn Widget| {
                let (wx, wy, ww, wh) = w.rect();
                w.set_rect(wx, wy - shift, ww, wh);
            };

            if self.active_page == 1 {
                shift_widget(&mut self.toggle_grid_snap);
                shift_widget(&mut self.toggle_network_grid);
                shift_widget(&mut self.spin_grid_x);
                shift_widget(&mut self.spin_grid_y);
                shift_widget(&mut self.spin_skipped_row_h);
                shift_widget(&mut self.spin_skipped_col_w);
                shift_widget(&mut self.node_color_selector);
            } else if self.active_page == 2 {
                shift_widget(&mut self.toggle_show_grid);
                shift_widget(&mut self.toggle_show_cube);
                shift_widget(&mut self.toggle_show_origin);
                shift_widget(&mut self.toggle_show_camera_pivot);
                shift_widget(&mut self.spin_grid_thickness);
                shift_widget(&mut self.spin_origin_size);
                shift_widget(&mut self.spin_camera_pivot_size);
                shift_widget(&mut self.color_selector);
            }
        }

        self.section_quads = collector.quads;
        self.section_labels = collector.labels;
    }
}

impl Widget for ConfigDialog {
    fn rect(&self) -> (f32, f32, f32, f32) { (self.x, self.y, self.w, self.h) }
    fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.x = x; self.y = y; self.w = w; self.h = h;
        self.update_child_layouts();
    }
    fn color(&self) -> [f32; 4] { [0.0, 0.0, 0.0, 0.0] }
    fn set_hovered(&mut self, v: bool) { self.hovered = v; }
    fn hovered(&self) -> bool { self.hovered }

    fn set_visible(&mut self, v: bool) {
        self.visible = v;
        if v {
            self.scroll_y = 0.0;
            self.scroll_velocity = 0.0;
            self.update_child_layouts();
        }
    }
    fn visible(&self) -> bool { self.visible }

    fn mouse_wheel(&mut self, delta: &MouseScrollDelta, px: f32, py: f32) -> bool {
        if !self.visible { return false; }
        if self.hit_test(px, py) {
            let scroll_amount = match delta {
                MouseScrollDelta::LineDelta(_x, y) => *y * 24.0,
                MouseScrollDelta::PixelDelta(pos) => pos.y as f32,
            };
            self.scroll_velocity -= scroll_amount * 12.0;
            return true;
        }
        false
    }

    fn tick(&mut self, dt: f32) -> bool {
        if !self.visible { return false; }
        if self.scroll_velocity.abs() > 0.01 {
            let old_scroll_y = self.scroll_y;
            self.scroll_y += self.scroll_velocity * dt;
            let friction = 8.0;
            self.scroll_velocity *= (-friction * dt).exp();
            if self.scroll_velocity.abs() < 5.0 {
                self.scroll_velocity = 0.0;
            }
            self.update_child_layouts();
            (self.scroll_y - old_scroll_y).abs() > 0.01
        } else {
            false
        }
    }

    fn set_config_toggle(&mut self, id: usize, val: bool) {
        match id {
            0 => self.toggle_grid_snap.set_toggled(val),
            1 => self.toggle_network_grid.set_toggled(val),
            2 => self.toggle_show_grid.set_toggled(val),
            3 => self.toggle_show_cube.set_toggled(val),
            4 => self.toggle_show_origin.set_toggled(val),
            5 => self.toggle_show_camera_pivot.set_toggled(val),
            _ => {}
        }
    }
    fn take_config_toggle(&mut self) -> Option<(usize, bool)> {
        if self.toggle_grid_snap.take_click() {
            Some((0, self.toggle_grid_snap.toggled()))
        } else if self.toggle_network_grid.take_click() {
            Some((1, self.toggle_network_grid.toggled()))
        } else if self.toggle_show_grid.take_click() {
            Some((2, self.toggle_show_grid.toggled()))
        } else if self.toggle_show_cube.take_click() {
            Some((3, self.toggle_show_cube.toggled()))
        } else if self.toggle_show_origin.take_click() {
            Some((4, self.toggle_show_origin.toggled()))
        } else if self.toggle_show_camera_pivot.take_click() {
            Some((5, self.toggle_show_camera_pivot.toggled()))
        } else {
            None
        }
    }

    fn set_config_spin(&mut self, id: usize, val: f32) {
        match id {
            0 => {
                self.grid_size_x = val;
                self.spin_grid_x.value = val as i32;
            }
            1 => {
                self.grid_size_y = val;
                self.spin_grid_y.value = val as i32;
            }
            2 => {
                self.skipped_row_h = val;
                self.spin_skipped_row_h.value = val as i32;
            }
            3 => {
                self.skipped_col_w = val;
                self.spin_skipped_col_w.value = val as i32;
            }
            4 => {
                self.viewport_r = val;
                self.color_selector.color[0] = (val * 255.0).round().clamp(0.0, 255.0) as u8;
                self.last_sent_color[0] = self.color_selector.color[0];
            }
            5 => {
                self.viewport_g = val;
                self.color_selector.color[1] = (val * 255.0).round().clamp(0.0, 255.0) as u8;
                self.last_sent_color[1] = self.color_selector.color[1];
            }
            6 => {
                self.viewport_b = val;
                self.color_selector.color[2] = (val * 255.0).round().clamp(0.0, 255.0) as u8;
                self.last_sent_color[2] = self.color_selector.color[2];
            }
            7 => {
                self.origin_size = val;
                self.spin_origin_size.value = (val * 10.0).round() as i32;
            }
            8 => {
                self.grid_thickness = val;
                self.spin_grid_thickness.value = (val * 1000.0).round() as i32;
            }
            9 => {
                self.camera_pivot_size = val;
                self.spin_camera_pivot_size.value = (val * 10.0).round() as i32;
            }
            10 => {
                self.node_color_selector.color[0] = (val * 255.0).round().clamp(0.0, 255.0) as u8;
                self.last_sent_node_color[0] = self.node_color_selector.color[0];
            }
            11 => {
                self.node_color_selector.color[1] = (val * 255.0).round().clamp(0.0, 255.0) as u8;
                self.last_sent_node_color[1] = self.node_color_selector.color[1];
            }
            12 => {
                self.node_color_selector.color[2] = (val * 255.0).round().clamp(0.0, 255.0) as u8;
                self.last_sent_node_color[2] = self.node_color_selector.color[2];
            }
            _ => {}
        }
    }
    fn take_config_spin(&mut self) -> Option<(usize, f32)> {
        if self.spin_grid_x.value as f32 != self.grid_size_x {
            self.grid_size_x = self.spin_grid_x.value as f32;
            return Some((0, self.grid_size_x));
        }
        if self.spin_grid_y.value as f32 != self.grid_size_y {
            self.grid_size_y = self.spin_grid_y.value as f32;
            return Some((1, self.grid_size_y));
        }
        if self.spin_skipped_row_h.value as f32 != self.skipped_row_h {
            self.skipped_row_h = self.spin_skipped_row_h.value as f32;
            return Some((2, self.skipped_row_h));
        }
        if self.spin_skipped_col_w.value as f32 != self.skipped_col_w {
            self.skipped_col_w = self.spin_skipped_col_w.value as f32;
            return Some((3, self.skipped_col_w));
        }
        if self.spin_origin_size.value as f32 / 10.0 != self.origin_size {
            self.origin_size = self.spin_origin_size.value as f32 / 10.0;
            return Some((7, self.origin_size));
        }
        if self.spin_grid_thickness.value as f32 / 1000.0 != self.grid_thickness {
            self.grid_thickness = self.spin_grid_thickness.value as f32 / 1000.0;
            return Some((8, self.grid_thickness));
        }
        if self.spin_camera_pivot_size.value as f32 / 10.0 != self.camera_pivot_size {
            self.camera_pivot_size = self.spin_camera_pivot_size.value as f32 / 10.0;
            return Some((9, self.camera_pivot_size));
        }
        for i in 0..3 {
            if self.color_selector.color[i] != self.last_sent_color[i] {
                let val = self.color_selector.color[i] as f32 / 255.0;
                self.last_sent_color[i] = self.color_selector.color[i];
                match i {
                    0 => self.viewport_r = val,
                    1 => self.viewport_g = val,
                    2 => self.viewport_b = val,
                    _ => {}
                }
                return Some((4 + i, val));
            }
        }
        for i in 0..3 {
            if self.node_color_selector.color[i] != self.last_sent_node_color[i] {
                let val = self.node_color_selector.color[i] as f32 / 255.0;
                self.last_sent_node_color[i] = self.node_color_selector.color[i];
                return Some((10 + i, val));
            }
        }
        None
    }

    fn hit_test(&self, px: f32, py: f32) -> bool {
        if !self.visible { return false; }
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    fn on_cursor_moved(&mut self, px: f32, py: f32) -> bool {
        let was = self.hovered;
        self.hovered = self.hit_test(px, py);
        let (ppx, ppy, pw, _ph) = self.panel_rect();
        let old_close = self.close_hovered;
        let (cx, cy, cw, ch) = self.close_rect(ppx, pw, ppy);
        self.close_hovered = self.visible && px >= cx && px < cx + cw && py >= cy && py < cy + ch;

        let old_tab = self.hovered_tab;
        self.hovered_tab = None;
        if self.visible {
            for (i, (tx, ty, tw, th)) in self.tab_rects(ppx, ppy).iter().enumerate() {
                if px >= *tx && px < *tx + *tw && py >= *ty && py < *ty + *th {
                    self.hovered_tab = Some(i);
                    break;
                }
            }
        }

        let cs_changed = if self.visible && self.active_page == 2 {
            self.color_selector.cursor_moved(px, py)
        } else {
            let was = self.color_selector.hovered();
            self.color_selector.set_hovered(false);
            was
        };

        let cs_node_changed = if self.visible && self.active_page == 1 {
            self.node_color_selector.cursor_moved(px, py)
        } else {
            let was = self.node_color_selector.hovered();
            self.node_color_selector.set_hovered(false);
            was
        };

        let cs_changed = cs_changed || cs_node_changed;

        let mut widgets_changed = false;
        if self.visible {
            if self.active_page == 1 {
                widgets_changed |= self.toggle_grid_snap.cursor_moved(px, py);
                widgets_changed |= self.toggle_network_grid.cursor_moved(px, py);
                self.toggle_show_grid.set_hovered(false);
                self.toggle_show_cube.set_hovered(false);
                self.toggle_show_origin.set_hovered(false);
                self.toggle_show_camera_pivot.set_hovered(false);

                widgets_changed |= self.spin_grid_x.cursor_moved(px, py);
                widgets_changed |= self.spin_grid_y.cursor_moved(px, py);
                widgets_changed |= self.spin_skipped_row_h.cursor_moved(px, py);
                widgets_changed |= self.spin_skipped_col_w.cursor_moved(px, py);
                self.spin_grid_thickness.set_hovered(false);
                self.spin_origin_size.set_hovered(false);
                self.spin_camera_pivot_size.set_hovered(false);
            } else if self.active_page == 2 {
                self.toggle_grid_snap.set_hovered(false);
                self.toggle_network_grid.set_hovered(false);
                widgets_changed |= self.toggle_show_grid.cursor_moved(px, py);
                widgets_changed |= self.toggle_show_cube.cursor_moved(px, py);
                widgets_changed |= self.toggle_show_origin.cursor_moved(px, py);
                widgets_changed |= self.toggle_show_camera_pivot.cursor_moved(px, py);

                widgets_changed |= self.spin_grid_thickness.cursor_moved(px, py);
                widgets_changed |= self.spin_origin_size.cursor_moved(px, py);
                widgets_changed |= self.spin_camera_pivot_size.cursor_moved(px, py);
                self.spin_grid_x.set_hovered(false);
                self.spin_grid_y.set_hovered(false);
                self.spin_skipped_row_h.set_hovered(false);
                self.spin_skipped_col_w.set_hovered(false);
            } else {
                self.toggle_grid_snap.set_hovered(false);
                self.toggle_network_grid.set_hovered(false);
                self.toggle_show_grid.set_hovered(false);
                self.toggle_show_cube.set_hovered(false);
                self.toggle_show_origin.set_hovered(false);
                self.toggle_show_camera_pivot.set_hovered(false);

                self.spin_grid_x.set_hovered(false);
                self.spin_grid_y.set_hovered(false);
                self.spin_skipped_row_h.set_hovered(false);
                self.spin_skipped_col_w.set_hovered(false);
                self.spin_grid_thickness.set_hovered(false);
                self.spin_origin_size.set_hovered(false);
                self.spin_camera_pivot_size.set_hovered(false);
            }
        }

        was != self.hovered || old_close != self.close_hovered || old_tab != self.hovered_tab
            || cs_changed || widgets_changed
    }

    fn mouse_input(&mut self, button: MouseButton, state: ElementState, px: f32, py: f32) -> bool {
        if !self.visible || button != MouseButton::Left { return false; }
        let (ppx, ppy, pw, ph) = self.panel_rect();
        let in_panel = px >= ppx && px <= ppx + pw && py >= ppy && py <= ppy + ph;
        let (cx, cy, cw, ch) = self.close_rect(ppx, pw, ppy);
        let on_close = px >= cx && px < cx + cw && py >= cy && py < cy + ch;

        if state == ElementState::Pressed {
            if !in_panel || on_close {
                self.color_selector.unfocus();
                self.node_color_selector.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.spin_skipped_col_w.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.visible = false;
                return true;
            }
            for (i, (tx, ty, tw, th)) in self.tab_rects(ppx, ppy).iter().enumerate() {
                if px >= *tx && px < *tx + *tw && py >= *ty && py < *ty + *th {
                    self.color_selector.unfocus();
                    self.node_color_selector.unfocus();
                    self.spin_grid_x.unfocus();
                    self.spin_grid_y.unfocus();
                    self.spin_skipped_row_h.unfocus();
                    self.spin_skipped_col_w.unfocus();
                    self.spin_grid_thickness.unfocus();
                    self.spin_origin_size.unfocus();
                    self.spin_camera_pivot_size.unfocus();
                    self.toggle_grid_snap.unfocus();
                    self.toggle_network_grid.unfocus();
                    self.toggle_show_grid.unfocus();
                    self.toggle_show_cube.unfocus();
                    self.toggle_show_origin.unfocus();
                    self.toggle_show_camera_pivot.unfocus();
                    self.active_page = i;
                    self.scroll_y = 0.0;
                    self.scroll_velocity = 0.0;
                    self.update_child_layouts();
                    return true;
                }
            }
        }

        // Now dispatch to child widgets
        let mut handled = false;
        if self.active_page == 1 {
            if self.toggle_grid_snap.mouse_input(button, state, px, py) {
                self.toggle_network_grid.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.spin_skipped_col_w.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.toggle_network_grid.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.spin_skipped_col_w.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.spin_grid_x.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.spin_skipped_col_w.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.spin_grid_y.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.spin_skipped_col_w.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.spin_skipped_row_h.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_col_w.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.spin_skipped_col_w.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.node_color_selector.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.spin_skipped_col_w.unfocus();
                handled = true;
            }
        } else if self.active_page == 2 {
            if self.toggle_show_grid.mouse_input(button, state, px, py) {
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.toggle_show_cube.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.toggle_show_origin.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.toggle_show_camera_pivot.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.spin_grid_thickness.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.spin_origin_size.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.spin_camera_pivot_size.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.color_selector.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            } else if self.color_selector.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.node_color_selector.unfocus();
                handled = true;
            }
        }

        if handled {
            return true;
        }

        if state == ElementState::Pressed {
            // Unfocus everything if clicked elsewhere in the panel
            self.spin_grid_x.unfocus();
            self.spin_grid_y.unfocus();
            self.spin_skipped_row_h.unfocus();
            self.spin_skipped_col_w.unfocus();
            self.spin_grid_thickness.unfocus();
            self.spin_origin_size.unfocus();
            self.spin_camera_pivot_size.unfocus();
            self.color_selector.unfocus();
            self.node_color_selector.unfocus();
            self.toggle_grid_snap.unfocus();
            self.toggle_network_grid.unfocus();
            self.toggle_show_grid.unfocus();
            self.toggle_show_cube.unfocus();
            self.toggle_show_origin.unfocus();
            self.toggle_show_camera_pivot.unfocus();
        }

        false
    }

    fn keyboard_input(&mut self, event: &KeyEvent) -> bool {
        if !self.visible { return false; }
        if self.active_page == 1 {
            if self.spin_grid_x.keyboard_input(event) { return true; }
            if self.spin_grid_y.keyboard_input(event) { return true; }
            if self.spin_skipped_row_h.keyboard_input(event) { return true; }
            if self.spin_skipped_col_w.keyboard_input(event) { return true; }
            if self.node_color_selector.keyboard_input(event) { return true; }
        } else if self.active_page == 2 {
            if self.spin_grid_thickness.keyboard_input(event) { return true; }
            if self.spin_origin_size.keyboard_input(event) { return true; }
            if self.spin_camera_pivot_size.keyboard_input(event) { return true; }
            if self.color_selector.keyboard_input(event) { return true; }
        }
        if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) {
            self.color_selector.unfocus();
            self.node_color_selector.unfocus();
            self.spin_grid_x.unfocus();
            self.spin_grid_y.unfocus();
            self.spin_skipped_row_h.unfocus();
            self.spin_skipped_col_w.unfocus();
            self.spin_grid_thickness.unfocus();
            self.spin_origin_size.unfocus();
            self.spin_camera_pivot_size.unfocus();
            self.toggle_grid_snap.unfocus();
            self.toggle_network_grid.unfocus();
            self.toggle_show_grid.unfocus();
            self.toggle_show_cube.unfocus();
            self.toggle_show_origin.unfocus();
            self.toggle_show_camera_pivot.unfocus();
            self.visible = false;
            return true;
        }
        false
    }

    fn extra_quads(&self) -> Vec<(f32, f32, f32, f32, [f32; 4])> {
        if !self.visible { return vec![]; }
        let mut quads = Vec::new();
        quads.push((self.x, self.y, self.w, self.h, [0.0, 0.0, 0.0, 0.5]));
        let (px, py, pw, ph) = self.panel_rect();
        quads.push((px, py, pw, ph, colors::PANEL_MENU_BG));

        // Close button hover
        if self.close_hovered {
            let (cx, cy, cw, ch) = self.close_rect(px, pw, py);
            quads.push((cx, cy, cw, ch, colors::PANEL_MENU_HOVER));
        }

        // Tab bar background
        let tab_y = py + 30.0;
        let tab_h = 22.0;
        quads.push((px + 8.0, tab_y, pw - 16.0, tab_h, [0.15, 0.15, 0.20, 1.0]));

        // Tabs
        let tabs = self.tab_rects(px, py);
        for (i, &(tx, ty, tw, th)) in tabs.iter().enumerate() {
            let bg = if i == self.active_page {
                colors::PANEL_MENU_BG
            } else if Some(i) == self.hovered_tab {
                [0.22, 0.22, 0.30, 1.0]
            } else {
                [0.18, 0.18, 0.25, 1.0]
            };
            quads.push((tx, ty, tw, th, bg));
        }

        let y_min = py + 56.0;
        let y_max = py + 584.0;
        let filter_quads = |q: Vec<(f32, f32, f32, f32, [f32; 4])>| {
            q.into_iter().filter(|&(_, qy, _, qh, _)| qy >= y_min && qy + qh <= y_max).collect::<Vec<_>>()
        };

        if self.active_page == 1 || self.active_page == 2 {
            quads.extend(filter_quads(self.section_quads.clone()));
        }

        if self.active_page == 1 {
            quads.extend(filter_quads(self.toggle_grid_snap.extra_quads()));
            quads.extend(filter_quads(self.toggle_network_grid.extra_quads()));
            quads.extend(filter_quads(self.node_color_selector.extra_quads()));

            let draw_spin = |spin: &Spinbox, q: &mut Vec<(f32, f32, f32, f32, [f32; 4])>| {
                let (sx, sy, sw, sh) = spin.rect();
                if sy >= y_min && sy + sh <= y_max {
                    q.push((sx, sy, sw, sh, colors::SPINBOX_BG));
                    q.extend(spin.extra_quads().into_iter().filter(|&(_, qy, _, qh, _)| qy >= y_min && qy + qh <= y_max));
                }
            };
            draw_spin(&self.spin_grid_x, &mut quads);
            draw_spin(&self.spin_grid_y, &mut quads);
            draw_spin(&self.spin_skipped_row_h, &mut quads);
            draw_spin(&self.spin_skipped_col_w, &mut quads);
        } else if self.active_page == 2 {
            quads.extend(filter_quads(self.toggle_show_grid.extra_quads()));
            quads.extend(filter_quads(self.toggle_show_cube.extra_quads()));
            quads.extend(filter_quads(self.toggle_show_origin.extra_quads()));
            quads.extend(filter_quads(self.toggle_show_camera_pivot.extra_quads()));

            let draw_spin = |spin: &Spinbox, q: &mut Vec<(f32, f32, f32, f32, [f32; 4])>| {
                let (sx, sy, sw, sh) = spin.rect();
                if sy >= y_min && sy + sh <= y_max {
                    q.push((sx, sy, sw, sh, colors::SPINBOX_BG));
                    q.extend(spin.extra_quads().into_iter().filter(|&(_, qy, _, qh, _)| qy >= y_min && qy + qh <= y_max));
                }
            };
            draw_spin(&self.spin_grid_thickness, &mut quads);
            draw_spin(&self.spin_origin_size, &mut quads);
            draw_spin(&self.spin_camera_pivot_size, &mut quads);
            quads.extend(filter_quads(self.color_selector.extra_quads()));
        }

        quads
    }

    fn text_labels(&self) -> Vec<TextLabel> {
        if !self.visible { return vec![]; }
        let (px, py, pw, _) = self.panel_rect();
        let mut labels = Vec::new();

        // Title and close button
        labels.push(TextLabel { text: "Configure Clear Design Interface".into(), x: px + 16.0, y: py + 10.0, font_size: 14.0, color: [0xcc, 0xcc, 0xd4] });
        labels.push(TextLabel { text: "\u{2715}".into(), x: px + pw - 22.0, y: py + 10.0, font_size: 14.0, color: [0xaa, 0xaa, 0xbb] });

        // Tab labels
        let tabs = self.tab_rects(px, py);
        let tab_labels = ["General", "Network", "Viewport", "Bindings"];
        for (i, &(tx, ty, _, _)) in tabs.iter().enumerate() {
            let color = if i == self.active_page { [0xcc, 0xcc, 0xd4] } else { [0xaa, 0xaa, 0xbb] };
            labels.push(TextLabel {
                text: tab_labels[i].into(),
                x: tx + 8.0,
                y: ty + 4.0,
                font_size: 12.0,
                color,
            });
        }

        let layout = self.compute_layout(px, py);
        labels.extend(layout.labels);

        let y_min = py + 56.0;
        let y_max = py + 584.0;
        let filter_labels = |l: Vec<TextLabel>| {
            l.into_iter().filter(|lbl| lbl.y >= y_min && lbl.y <= y_max).collect::<Vec<_>>()
        };

        if self.active_page == 1 || self.active_page == 2 {
            labels.extend(self.section_labels.iter().filter(|lbl| lbl.y >= y_min && lbl.y <= y_max).map(|l| TextLabel {
                text: l.text.clone(),
                x: l.x,
                y: l.y,
                font_size: l.font_size,
                color: l.color,
            }));
        }

        if self.active_page == 1 {
            labels.extend(filter_labels(self.toggle_grid_snap.text_labels()));
            labels.extend(filter_labels(self.toggle_network_grid.text_labels()));
            labels.extend(filter_labels(self.node_color_selector.text_labels()));

            labels.extend(filter_labels(self.spin_grid_x.text_labels()));
            labels.extend(filter_labels(self.spin_grid_y.text_labels()));
            labels.extend(filter_labels(self.spin_skipped_row_h.text_labels()));
            labels.extend(filter_labels(self.spin_skipped_col_w.text_labels()));
        } else if self.active_page == 2 {
            labels.extend(filter_labels(self.toggle_show_grid.text_labels()));
            labels.extend(filter_labels(self.toggle_show_cube.text_labels()));
            labels.extend(filter_labels(self.toggle_show_origin.text_labels()));
            labels.extend(filter_labels(self.toggle_show_camera_pivot.text_labels()));

            labels.extend(filter_labels(self.spin_grid_thickness.text_labels()));
            labels.extend(filter_labels(self.spin_origin_size.text_labels()));
            labels.extend(filter_labels(self.spin_camera_pivot_size.text_labels()));
            labels.extend(filter_labels(self.color_selector.text_labels()));
        }

        labels
    }

    fn take_click(&mut self) -> bool {
        if !self.visible { return false; }
        true
    }

    fn z_index(&self) -> i32 {
        200
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

impl Widget for NodePalette {
    fn rect(&self) -> (f32, f32, f32, f32) { (self.x, self.y, self.w, self.h) }
    fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) { self.x = x; self.y = y; self.w = w; self.h = h; }
    fn color(&self) -> [f32; 4] { [0.0, 0.0, 0.0, 0.0] }
    fn hit_test(&self, px: f32, py: f32) -> bool {
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

#[derive(Clone, Copy, PartialEq)]
enum Action {
    ToggleGrid,
    ToggleCube,
    ToggleSquareViewport,
    ToggleConfigure,
    ToggleSpreadsheet,
    ToggleOrigin,
    ToggleCameraPivot,
    ToggleCircularPane,
}

struct KeyBind {
    ctrl: bool,
    shift: bool,
    alt: bool,
    super_: bool,
    key: Key,
    action: Action,
}

impl KeyBind {
    fn matches(&self, mods: &ModifiersState, key: &Key) -> bool {
        mods.control_key() == self.ctrl
            && mods.shift_key() == self.shift
            && mods.alt_key() == self.alt
            && mods.super_key() == self.super_
            && key == &self.key
    }
}

fn default_keybinds() -> Vec<KeyBind> {
    vec![
        KeyBind { ctrl: true, shift: false, alt: false, super_: false, key: Key::Character("g".into()), action: Action::ToggleGrid },
        KeyBind { ctrl: true, shift: false, alt: false, super_: false, key: Key::Character("e".into()), action: Action::ToggleCube },
        KeyBind { ctrl: true, shift: false, alt: false, super_: false, key: Key::Character("a".into()), action: Action::ToggleSquareViewport },
        KeyBind { ctrl: true, shift: false, alt: false, super_: false, key: Key::Character(",".into()), action: Action::ToggleConfigure },
        KeyBind { ctrl: false, shift: false, alt: false, super_: false, key: Key::Character("`".into()), action: Action::ToggleSpreadsheet },
        KeyBind { ctrl: true, shift: false, alt: false, super_: false, key: Key::Character("d".into()), action: Action::ToggleCircularPane },
    ]
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
    clip_circle: [f32; 3], // [cx, cy, r]
}

impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x4,
        2 => Float32x3,
    ];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}


fn quad_vertices(
    x: f32, y: f32, w: f32, h: f32,
    surface_w: f32, surface_h: f32,
    color: [f32; 4],
    clip_circle: [f32; 3],
) -> [Vertex; 6] {
    let x0 = (x / surface_w) * 2.0 - 1.0;
    let y0 = 1.0 - (y / surface_h) * 2.0;
    let x1 = ((x + w) / surface_w) * 2.0 - 1.0;
    let y1 = 1.0 - ((y + h) / surface_h) * 2.0;
    [
        Vertex { position: [x0, y0], color, clip_circle },
        Vertex { position: [x1, y0], color, clip_circle },
        Vertex { position: [x0, y1], color, clip_circle },
        Vertex { position: [x1, y0], color, clip_circle },
        Vertex { position: [x1, y1], color, clip_circle },
        Vertex { position: [x0, y1], color, clip_circle },
    ]
}

fn widget_vertices(w: &dyn Widget, sw: f32, sh: f32, clip_circle: [f32; 3]) -> Vec<Vertex> {
    let (x, y, ww, h) = w.rect();
    quad_vertices(x, y, ww, h, sw, sh, w.color(), clip_circle).to_vec()
}

fn quad_vertices_clipped(
    x: f32, y: f32, w: f32, h: f32,
    surface_w: f32, surface_h: f32,
    color: [f32; 4],
    clip: (f32, f32, f32, f32),
    clip_circle: [f32; 3],
) -> Vec<Vertex> {
    let (cx0, cy0, cx1, cy1) = clip;
    let ix0 = x.max(cx0);
    let iy0 = y.max(cy0);
    let ix1 = (x + w).min(cx1);
    let iy1 = (y + h).min(cy1);
    if ix1 <= ix0 || iy1 <= iy0 {
        return Vec::new();
    }
    quad_vertices(ix0, iy0, ix1 - ix0, iy1 - iy0, surface_w, surface_h, color, clip_circle).to_vec()
}

fn widget_vertices_clipped(w: &dyn Widget, sw: f32, sh: f32, clip: (f32, f32, f32, f32), clip_circle: [f32; 3]) -> Vec<Vertex> {
    let (x, y, ww, h) = w.rect();
    quad_vertices_clipped(x, y, ww, h, sw, sh, w.color(), clip, clip_circle)
}

fn circle_vertices(
    cx: f32, cy: f32, r: f32,
    sw: f32, sh: f32,
    color: [f32; 4],
    segments: usize,
    clip_circle: [f32; 3],
) -> Vec<Vertex> {
    let mut verts = Vec::new();
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
        
        verts.push(Vertex { position: [ndc_x0, ndc_y0], color, clip_circle });
        verts.push(Vertex { position: [ndc_x1, ndc_y1], color, clip_circle });
        verts.push(Vertex { position: [ndc_x2, ndc_y2], color, clip_circle });
    }
    verts
}

fn circle_border_vertices(
    cx: f32, cy: f32, r: f32,
    thickness: f32,
    sw: f32, sh: f32,
    color: [f32; 4],
    segments: usize,
    clip_circle: [f32; 3],
) -> Vec<Vertex> {
    let mut verts = Vec::new();
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
        
        verts.push(Vertex { position: [ndc_x0, ndc_y0], color, clip_circle });
        verts.push(Vertex { position: [ndc_x1, ndc_y1], color, clip_circle });
        verts.push(Vertex { position: [ndc_x2, ndc_y2], color, clip_circle });
        
        verts.push(Vertex { position: [ndc_x0, ndc_y0], color, clip_circle });
        verts.push(Vertex { position: [ndc_x2, ndc_y2], color, clip_circle });
        verts.push(Vertex { position: [ndc_x3, ndc_y3], color, clip_circle });
    }
    verts
}

fn arc_background_vertices(
    cx: f32, cy: f32, r: f32,
    thickness: f32,
    start_angle: f32, end_angle: f32,
    sw: f32, sh: f32,
    color: [f32; 4],
    segments: usize,
    clip_circle: [f32; 3],
) -> Vec<Vertex> {
    let mut verts = Vec::new();
    for i in 0..segments {
        let theta1 = start_angle + (i as f32) * (end_angle - start_angle) / (segments as f32);
        let theta2 = start_angle + ((i + 1) as f32) * (end_angle - start_angle) / (segments as f32);
        
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
        
        verts.push(Vertex { position: [ndc_x0, ndc_y0], color, clip_circle });
        verts.push(Vertex { position: [ndc_x1, ndc_y1], color, clip_circle });
        verts.push(Vertex { position: [ndc_x2, ndc_y2], color, clip_circle });
        
        verts.push(Vertex { position: [ndc_x0, ndc_y0], color, clip_circle });
        verts.push(Vertex { position: [ndc_x2, ndc_y2], color, clip_circle });
        verts.push(Vertex { position: [ndc_x3, ndc_y3], color, clip_circle });
    }
    verts
}

fn make_text_buffer(font_system: &mut FontSystem, text: &str, size: f32) -> Buffer {
    let metrics = Metrics::new(size, size * 1.4);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text(font_system, text, Attrs::new(), glyphon::Shaping::Advanced);
    buffer.shape_until_scroll(font_system, true);
    buffer
}

fn get_next_visible_pane(current_pane: usize, show_spreadsheet: bool, shift_pressed: bool) -> usize {
    let visible_panes = if show_spreadsheet {
        vec![LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX]
    } else {
        vec![LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX]
    };

    let current_pos = visible_panes.iter().position(|&x| x == current_pane).unwrap_or(0);
    let next_pos = if shift_pressed {
        (current_pos + visible_panes.len() - 1) % visible_panes.len()
    } else {
        (current_pos + 1) % visible_panes.len()
    };
    visible_panes[next_pos]
}

struct State {
    window: XdgWindow,
    wl_surface: wl_surface::WlSurface,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
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

    keybinds: Vec<KeyBind>,
    pending_action: Option<Action>,
    exit_requested: bool,

    widgets: Vec<Box<dyn Widget>>,
    positions: Vec<(f32, f32, f32, f32)>,
    splitter1_x: f32,
    splitter2_x: f32,
    node_palette_visible: bool,
    node_palette_query: String,
    node_palette_filtered: Vec<usize>,
    node_palette_selected: usize,

    font_system: FontSystem,
    swash_cache: SwashCache,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_viewport: Viewport,

    status_buffer: Buffer,

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
    last_config_read: Instant,
    circular_network_pane: bool,
    network_circle_x: f32,
    network_circle_y: f32,
    network_circle_radius: f32,
    is_dragging_network_circle: bool,
    circle_drag_ox: f32,
    circle_drag_oy: f32,
}

impl State {
    fn save_settings(&self) {
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
            square_viewport: self.square_viewport,
            grid_thickness: self.grid_thickness,
            show_camera_pivot_enabled: self.show_camera_pivot,
            camera_pivot_size: self.camera_pivot_size,
        };
        settings.save();
    }

    fn update_grid_geometry(&mut self) {
        let grid_verts = grid_vertices(self.grid_thickness);
        self.queue.write_buffer(&self.vertex_buffer_grid, 0, bytemuck::cast_slice(&grid_verts));
    }

    fn update_origin_geometry(&mut self) {
        let origin_verts = origin_vectors_vertices(self.origin_size);
        self.queue.write_buffer(&self.vertex_buffer_origin, 0, bytemuck::cast_slice(&origin_verts));
    }

    fn update_pivot_geometry(&mut self) {
        let pivot_verts = camera_pivot_vertices(self.camera_pivot_size);
        self.queue.write_buffer(&self.vertex_buffer_pivot, 0, bytemuck::cast_slice(&pivot_verts));
    }

    fn body_h(&self) -> f32 { self.height - HEADER_H - STATUS_H }

    fn content_left_w(&self) -> f32 { self.splitter1_x }

    fn content_right_x(&self) -> f32 { self.splitter1_x + SPLITTER_W }

    fn viewport_w(&self) -> f32 { self.splitter2_x - self.content_right_x() }

    fn param_x(&self) -> f32 { self.splitter2_x + SPLITTER_W }

    fn param_w(&self) -> f32 { self.width - self.param_x() }

    fn clamp_splitters(&mut self) {
        let min_s1 = MIN_COLUMN;
        let max_s1 = self.splitter2_x - SPLITTER_W - MIN_COLUMN;
        self.splitter1_x = self.splitter1_x.clamp(min_s1, max_s1);
        let min_s2 = self.splitter1_x + SPLITTER_W + MIN_COLUMN;
        let max_s2 = self.width - SPLITTER_W - MIN_COLUMN;
        self.splitter2_x = self.splitter2_x.clamp(min_s2, max_s2);
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

    fn on_path_changed(&mut self) {
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
        if self.focused_widget == Some(CONTENT_IDX) {
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
                }
            }

            self.widgets[SPREADSHEET_IDX].set_spreadsheet_data(headers, rows);
            self.last_spreadsheet_node_name = current_name;
            self.last_spreadsheet_node_params = current_params;
        }
    }

    fn save_to_file(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let proj = Project {
            name: "Project".to_string(),
            root: self.fs_root.clone(),
            view_state: ProjectViewState {
                active_camera: self.active_camera.clone(),
                pan: (self.pan_x, self.pan_y),
                current_path: self.current_path.clone(),
            },
        };
        let content = serde_json::to_string_pretty(&proj)?;
        fs::write(path, content)?;
        Ok(())
    }

    fn load_from_file(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
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
    }

    fn create_depth_texture(&self) -> (wgpu::Texture, wgpu::TextureView) {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
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

    async fn new(
        conn: &Connection,
        qh: &QueueHandle<AppState>,
        compositor_state: &CompositorState,
        xdg_shell_state: &XdgShell,
        pw: u32,
        ph: u32,
        scale: f64,
    ) -> Self {
        let settings = DesignSettings::load();
        let lw = pw as f32 / scale as f32;
        let lh = ph as f32 / scale as f32;
        let sw = lw;

        let wl_surface = compositor_state.create_surface(qh);
        wl_surface.set_buffer_scale(scale as i32);
        let window = xdg_shell_state.create_window(wl_surface.clone(), WindowDecorations::None, qh);
        window.set_title("Clear Design Interface");
        window.set_app_id("clear-design-interface");
        window.set_min_size(Some(((480.0 * scale) as u32, (320.0 * scale) as u32)));
        window.commit();

        let wayland_handle = Box::leak(Box::new(clear_ui::wayland::WaylandSurfaceHandle {
            display_ptr: conn.backend().display_id().as_ptr() as *mut std::ffi::c_void,
            surface_ptr: wl_surface.id().as_ptr() as *mut std::ffi::c_void,
        }));

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });

        let surface = instance
            .create_surface(wayland_handle)
            .expect("Failed to create surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("GPU Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await
            .expect("Failed to create device");

        let config = surface
            .get_default_config(&adapter, pw, ph)
            .expect("Failed to get surface config");
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
            bind_group_layouts: &[],
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
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        let grid_verts = grid_vertices(settings.grid_thickness);
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

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let mut text_atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let text_renderer = TextRenderer::new(&mut text_atlas, &device, wgpu::MultisampleState::default(), None);

        let mut text_viewport = Viewport::new(&device, &cache);
        text_viewport.update(&queue, Resolution { width: pw, height: ph });

        let status_buffer = make_text_buffer(&mut font_system, "Ready", 12.0);

        let splitter1_x = (sw - 2.0 * SPLITTER_W) / 3.0;
        let splitter2_x = splitter1_x + SPLITTER_W + (sw - 2.0 * SPLITTER_W) / 3.0;
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
        let mut widgets: Vec<Box<dyn Widget>> = vec![
            Box::new(MenuBar::new(0.0, 0.0, 0.0, HEADER_H).with_title("Clear Design Interface").with_item("File", &["New Project", "Open", "Save", "Configure", "Exit"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Reset Zoom"]).with_item("Help", &["About"]).with_z_index(110)),
            Box::new(Graph::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(ViewportBg::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(ParametersBg::new()),
            Box::new(Canvas::new()),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("0: Network").with_item("File", &["New", "Open", "Save"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Circular Pane"])),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("1: Viewport").with_item("Camera", &["Perspective", "Orthographic"]).with_item("Display", &["Square Aspect"]).with_item("Guides", &["Show Grid", "Cube", "Origin", "Camera Pivot"])),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("2: Parameters").with_item("Preset", &["Default", "Custom"]).with_item("Reset", &["All"])),
            Box::new(StatusBar::new()),
            Box::new(Breadcrumb::new()),
            Box::new(ConfigDialog::new(&settings)),
            Box::new(NodePalette::new()),
            Box::new(Spreadsheet::new()),
        ];
        
        let mut spreadsheet_menubar = MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("3: Spreadsheet");
        spreadsheet_menubar.visible = false;
        widgets.push(Box::new(spreadsheet_menubar));

        let mut positions = Vec::with_capacity(SPREADSHEET_MENUBAR_IDX + 1);
        positions.resize_with(SPREADSHEET_MENUBAR_IDX + 1, || (0.0, 0.0, 0.0, 0.0));

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vertex Buffer"),
            size: 1,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut state = Self {
            window,
            wl_surface,
            surface,
            device,
            queue,
            config,
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
            vertex_buffer_origin,
            vertex_count_origin,
            vertex_buffer_pivot,
            vertex_count_pivot,
            origin_size: settings.origin_size,
            camera_pivot_size: settings.camera_pivot_size,
            fs_root,
            node_templates,
            current_path,
            last_click: None,
            last_frame: Instant::now(),
            keybinds: default_keybinds(),
            pending_action: None,
            exit_requested: false,
            widgets,
            positions,
            splitter1_x,
            splitter2_x,
            node_palette_visible: false,
            node_palette_query: String::new(),
            node_palette_filtered: Vec::new(),
            node_palette_selected: 0,
            font_system,
            swash_cache,
            text_atlas,
            text_renderer,
            text_viewport,
            status_buffer,
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
            last_config_read: Instant::now(),
            circular_network_pane: false,
            network_circle_x: 250.0,
            network_circle_y: 300.0,
            network_circle_radius: 180.0,
            is_dragging_network_circle: false,
            circle_drag_ox: 0.0,
            circle_drag_oy: 0.0,
        };

        state.update_inertial_settings();
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

        state.rebuild_positions();
        state.apply_layout();
        state.update_panel_bounds();
        state.sync_pane_focus();
        state.upload_vertices();
        state
    }

    fn sync_grid_settings(&mut self) {
        let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
        let active_node_area_y = if self.circular_network_pane {
            self.network_circle_y - self.network_circle_radius + 45.0 + MENUBAR_H + BREADCRUMB_H
        } else {
            node_area_y
        };
        let active_node_area_x = if self.circular_network_pane {
            self.network_circle_x - self.network_circle_radius
        } else {
            0.0
        };

        self.widgets[CONTENT_IDX].set_show_network_grid(self.network_grid_visible);
        self.widgets[CONTENT_IDX].set_grid_sizes(self.grid_size_x, self.grid_size_y);
        self.widgets[CONTENT_IDX].set_skipped_sizes(self.skipped_row_h, self.skipped_col_w);
        self.widgets[CONTENT_IDX].set_grid_origin(active_node_area_x + self.pan_x, active_node_area_y + self.pan_y);
        self.widgets[CONTENT_IDX].set_grid_snap_enabled(self.grid_snap_enabled);
    }

    fn update_inertial_settings(&mut self) {
        self.last_config_read = Instant::now();
        let config_path = "/home/lsgalante/.config/ccec/config.toml";
        if let Ok(content) = std::fs::read_to_string(config_path) {
            #[derive(serde::Deserialize)]
            struct InertialSection {
                inertial_scroll: Option<bool>,
                scroll_friction: Option<u16>,
            }
            #[derive(serde::Deserialize)]
            struct Config {
                inertial: Option<InertialSection>,
            }
            if let Ok(cfg) = toml::from_str::<Config>(&content) {
                if let Some(inertial) = cfg.inertial {
                    if let Some(val) = inertial.inertial_scroll {
                        self.inertial_scroll_enabled = val;
                    }
                    if let Some(friction_val) = inertial.scroll_friction {
                        let friction_f = (friction_val as f32 / 1000.0).clamp(0.1, 0.999);
                        self.inertial_scroll_friction = friction_f;
                    }
                }
            }
        }
        // Defaults if file read or parsing fails
        self.inertial_scroll_enabled = true;
        self.inertial_scroll_friction = 0.90;
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

        let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
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
        let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
        let active_node_area_y = if self.circular_network_pane {
            self.network_circle_y - self.network_circle_radius + 45.0 + MENUBAR_H + BREADCRUMB_H
        } else {
            node_area_y
        };
        let active_node_area_x = if self.circular_network_pane {
            self.network_circle_x - self.network_circle_radius
        } else {
            0.0
        };

        let cx = active_node_area_x + self.grid_cursor_col as f32 * (self.grid_size_x + self.skipped_col_w) + self.pan_x;
        let cy = active_node_area_y + self.grid_cursor_row as f32 * (self.grid_size_y + self.skipped_row_h) + self.pan_y;
        let cw = self.grid_size_x;
        let ch = self.grid_size_y;

        let active_graph_w = if self.circular_network_pane {
            2.0 * self.network_circle_radius
        } else {
            self.content_left_w()
        };
        let active_max_y = if self.circular_network_pane {
            self.network_circle_y + self.network_circle_radius
        } else {
            self.height - STATUS_H
        };

        if cx < active_node_area_x {
            self.pan_x -= cx - active_node_area_x;
        } else if cx + cw > active_node_area_x + active_graph_w {
            self.pan_x -= cx + cw - (active_node_area_x + active_graph_w);
        }

        if cy < active_node_area_y {
            self.pan_y -= cy - active_node_area_y;
        } else if cy + ch > active_max_y {
            self.pan_y -= cy + ch - active_max_y;
        }
        self.sync_grid_settings();
    }

    fn rebuild_positions(&mut self) {
        self.clamp_splitters();
        self.widgets[LEFT_MENUBAR_IDX].set_center_items(self.circular_network_pane);

        let body_h = self.body_h();
        let clw = self.content_left_w();
        let crx = self.content_right_x();
        let vw = self.viewport_w();
        let px = self.param_x();

        let node_area_y = HEADER_H + MENUBAR_H;
        self.positions[0] = (0.0, 0.0, self.width, HEADER_H);

        if self.circular_network_pane {
            let cx = self.network_circle_x;
            let cy = self.network_circle_y;
            let r = self.network_circle_radius;

            self.positions[LEFT_MENUBAR_IDX] = (cx - r, cy - r, 2.0 * r, 35.0);
            self.widgets[LEFT_MENUBAR_IDX].set_curved_circle(Some((cx, cy, r)));
            self.positions[BREADCRUMB_IDX] = (cx - r, cy - r + 45.0 + MENUBAR_H, 2.0 * r, BREADCRUMB_H);
            self.positions[CONTENT_IDX] = (cx - r, cy - r + 45.0 + MENUBAR_H + BREADCRUMB_H, 2.0 * r, 2.0 * r - (45.0 + MENUBAR_H + BREADCRUMB_H));
            self.positions[SPLITTER1_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[SPLITTER2_IDX] = (self.splitter2_x, HEADER_H, SPLITTER_W, body_h);
            self.positions[PARAM_IDX] = (px, HEADER_H, self.param_w(), body_h);

            let viewport_w = self.splitter2_x;
            self.positions[VIEWPORT_IDX] = (0.0, HEADER_H, viewport_w, body_h);
            self.positions[RIGHT_MENUBAR_IDX] = (0.0, HEADER_H, viewport_w, MENUBAR_H);

            if self.show_spreadsheet {
                let viewport_h = body_h * 2.0 / 3.0;
                let spreadsheet_h = body_h - viewport_h;
                self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, HEADER_H + viewport_h, viewport_w, MENUBAR_H);
                self.positions[SPREADSHEET_IDX] = (0.0, HEADER_H + viewport_h + MENUBAR_H, viewport_w, spreadsheet_h - MENUBAR_H);
            } else {
                self.positions[SPREADSHEET_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
            }
        } else {
            self.widgets[LEFT_MENUBAR_IDX].set_curved_circle(None);
            self.positions[CONTENT_IDX] = (0.0, node_area_y + BREADCRUMB_H, clw, self.height - STATUS_H - (node_area_y + BREADCRUMB_H));
            self.positions[SPLITTER1_IDX] = (self.splitter1_x, HEADER_H, SPLITTER_W, body_h);
            self.positions[SPLITTER2_IDX] = (self.splitter2_x, HEADER_H, SPLITTER_W, body_h);
            self.positions[PARAM_IDX] = (px, HEADER_H, self.param_w(), body_h);

            self.positions[VIEWPORT_IDX] = (crx, HEADER_H, vw, body_h);
            self.positions[RIGHT_MENUBAR_IDX] = (crx, HEADER_H, vw, MENUBAR_H);
            self.positions[BREADCRUMB_IDX] = (0.0, node_area_y, clw, BREADCRUMB_H);
            self.positions[LEFT_MENUBAR_IDX] = (0.0, HEADER_H, clw, MENUBAR_H);

            if self.show_spreadsheet {
                let viewport_h = body_h * 2.0 / 3.0;
                let spreadsheet_h = body_h - viewport_h;
                self.positions[SPREADSHEET_MENUBAR_IDX] = (crx, HEADER_H + viewport_h, vw, MENUBAR_H);
                self.positions[SPREADSHEET_IDX] = (crx, HEADER_H + viewport_h + MENUBAR_H, vw, spreadsheet_h - MENUBAR_H);
            } else {
                self.positions[SPREADSHEET_IDX] = (0.0, 0.0, 0.0, 0.0);
                self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
            }
        }

        self.positions[CANVAS_IDX] = (0.0, HEADER_H, self.width, body_h);
        self.positions[PARAM_MENUBAR_IDX] = (px, HEADER_H, self.param_w(), MENUBAR_H);
        self.positions[STATUS_IDX] = (0.0, self.height - STATUS_H, self.width, STATUS_H);
        self.positions[CONFIG_DIALOG_IDX] = (0.0, 0.0, self.width, self.height);
        self.positions[NODE_PALETTE_IDX] = (0.0, 0.0, self.width, self.height);
    }


    fn apply_layout(&mut self) {
        for (i, pos) in self.positions.iter().enumerate() {
            if let Some(widget) = self.widgets.get_mut(i) {
                if widget.is_dragging() { continue; }
                let (x, y, w, h) = *pos;
                widget.set_rect(x, y, w, h);
            }
        }
    }

    fn update_panel_bounds(&mut self) {
        // Unbounded 2D canvas - no clamping
    }

    fn sync_pane_focus(&mut self) {
        for &menubar_idx in &[LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX] {
            self.widgets[menubar_idx].set_selected(menubar_idx == self.focused_pane);
        }
        if self.focused_pane != PARAM_MENUBAR_IDX {
            self.widgets[PARAM_IDX].unfocus();
        }
    }

    fn sync_layout(&mut self) {
        self.splitter1_x = self.widgets[SPLITTER1_IDX].rect().0;
        self.splitter2_x = self.widgets[SPLITTER2_IDX].rect().0;
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
                if self.widgets[CONFIG_DIALOG_IDX].take_click() {
                    self.widgets[CONFIG_DIALOG_IDX].set_visible(false);
                } else {
                    self.widgets[CONFIG_DIALOG_IDX].set_config_toggle(0, self.grid_snap_enabled);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_toggle(1, self.network_grid_visible);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_toggle(2, self.show_grid);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_toggle(3, self.show_cube);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_toggle(4, self.show_origin);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_toggle(5, self.show_camera_pivot);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(0, self.grid_size_x);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(1, self.grid_size_y);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(2, self.skipped_row_h);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(3, self.skipped_col_w);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(4, self.viewport_bg_color[0]);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(5, self.viewport_bg_color[1]);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(6, self.viewport_bg_color[2]);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(7, self.origin_size);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(8, self.grid_thickness);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(9, self.camera_pivot_size);
                    self.widgets[CONFIG_DIALOG_IDX].set_visible(true);
                }
                self.upload_vertices();
                return;
            }
            Action::ToggleSpreadsheet => {
                self.show_spreadsheet = !self.show_spreadsheet;
                self.widgets[SPREADSHEET_IDX].set_visible(self.show_spreadsheet);
                self.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(self.show_spreadsheet);
                if !self.show_spreadsheet && self.focused_pane == SPREADSHEET_MENUBAR_IDX {
                    self.focused_pane = RIGHT_MENUBAR_IDX;
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
        if let Some(old) = self.focused_widget {
            if old != CONTENT_IDX && old != SPREADSHEET_IDX {
                return;
            }
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

        let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
        let dialog_open = self.widgets[CONFIG_DIALOG_IDX].visible() || self.node_palette_visible;
        let show_cursor = self.drag_widget.is_none()
            && !dialog_open;

        let clip = if self.circular_network_pane {
            (
                self.network_circle_x - self.network_circle_radius,
                self.network_circle_y - self.network_circle_radius,
                self.network_circle_x + self.network_circle_radius,
                self.network_circle_y + self.network_circle_radius,
            )
        } else {
            (0.0, node_area_y, self.content_left_w(), self.height - STATUS_H)
        };

        let clip_circle_val = if self.circular_network_pane {
            [self.network_circle_x * self.scale as f32, self.network_circle_y * self.scale as f32, self.network_circle_radius * self.scale as f32]
        } else {
            [0.0, 0.0, 0.0]
        };

        let mut draw_order: Vec<usize> = (0..self.widgets.len()).collect();
        draw_order.sort_by_key(|&i| self.widgets[i].z_index());

        for &i in &draw_order {
            let w = &self.widgets[i];
            let is_network_part = i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX;
            let active_clip_circle = if is_network_part { clip_circle_val } else { [0.0, 0.0, 0.0] };

            if i == CONTENT_IDX {
                if self.circular_network_pane {
                    verts.extend(circle_vertices(
                        self.network_circle_x,
                        self.network_circle_y,
                        self.network_circle_radius,
                        sw,
                        sh,
                        [0.10, 0.10, 0.13, 0.95],
                        64,
                        active_clip_circle,
                    ));
                } else {
                    verts.extend(widget_vertices(w.as_ref(), sw, sh, active_clip_circle));
                }

                for (qx, qy, qw, qh, qc) in w.extra_quads() {
                    verts.extend(quad_vertices_clipped(qx, qy, qw, qh, sw, sh, qc, clip, active_clip_circle));
                }

                if self.circular_network_pane {
                    verts.extend(circle_border_vertices(
                        self.network_circle_x,
                        self.network_circle_y,
                        self.network_circle_radius,
                        3.0,
                        sw,
                        sh,
                        [0.35, 0.65, 0.95, 0.80],
                        64,
                        active_clip_circle,
                    ));
                }
            } else if i == LEFT_MENUBAR_IDX && self.circular_network_pane {
                let cx = self.network_circle_x;
                let cy = self.network_circle_y;
                let r = self.network_circle_radius;
                
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
                
                let border_color = [0.22, 0.22, 0.28, 0.90];
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
                    verts.extend(quad_vertices(qx, qy, qw, qh, sw, sh, qc, active_clip_circle));
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
                for (qx, qy, qw, qh, qc) in w.extra_quads() {
                    verts.extend(quad_vertices(qx, qy, qw, qh, sw, sh, qc, active_clip_circle));
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
                let cx = active_clip_circle[0] / self.scale as f32 - self.network_circle_radius + self.pan_x; // Wait, let's keep the exact cursor coordinates!
                let active_node_area_y = if self.circular_network_pane {
                    self.network_circle_y - self.network_circle_radius + 45.0 + MENUBAR_H + BREADCRUMB_H
                } else {
                    node_area_y
                };
                let active_node_area_x = if self.circular_network_pane {
                    self.network_circle_x - self.network_circle_radius
                } else {
                    0.0
                };
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
            self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Vertex Buffer"),
                size: needed,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue.write_buffer(&self.vertex_buffer, 0, data);
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
        if verts.is_empty() {
            self.vertex_buffer_spheres = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Sphere Node Vertex Buffer"),
                size: 1,
                usage: wgpu::BufferUsages::VERTEX,
                mapped_at_creation: false,
            });
        } else {
            self.vertex_buffer_spheres = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Sphere Node Vertex Buffer"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
        }
    }

    fn update_status_text(&mut self, text: &str) {
        self.status_buffer = make_text_buffer(&mut self.font_system, text, 12.0);
    }

    fn prepare_text(&mut self) {
        let splitter1_x = self.splitter1_x;
        let height = self.height;
        let Self {
            ref mut text_renderer, ref device, ref queue,
            ref mut font_system, ref mut text_atlas, ref mut text_viewport,
            ref mut swash_cache, ref status_buffer,
            physical_width, physical_height, scale,
            ref widgets, ..
        } = self;

        let viewport = Resolution { width: *physical_width, height: *physical_height };
        text_viewport.update(queue, viewport);
        let s = *scale as f32;

        let mut areas: Vec<TextArea> = vec![
            TextArea {
                buffer: status_buffer,
                left: 12.0 * s, top: *physical_height as f32 - 24.0 * s, scale: s,
                bounds: TextBounds { left: 0, top: 0, right: *physical_width as i32, bottom: *physical_height as i32 },
                default_color: glyphon::Color::rgb(0xaa, 0xaa, 0xbb),
                custom_glyphs: &[],
            },
        ];

        let mut widget_buffers: Vec<Buffer> = Vec::new();
        let mut widget_labels: Vec<TextLabel> = Vec::new();
        let mut widget_is_node: Vec<bool> = Vec::new();
        for (i, w) in widgets.iter().enumerate() {
            let is_node = i == CONTENT_IDX;
            let is_network_part = i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX;
            for label in w.text_labels() {
                if self.circular_network_pane && is_network_part && i != LEFT_MENUBAR_IDX {
                    let dx = label.x - self.network_circle_x;
                    let dy = label.y - self.network_circle_y;
                    let dist_sq = dx * dx + dy * dy;
                    if dist_sq > self.network_circle_radius * self.network_circle_radius {
                        continue;
                    }
                }
                widget_buffers.push(make_text_buffer(font_system, &label.text, label.font_size));
                widget_labels.push(label);
                widget_is_node.push(is_node);
            }
        }


        for ((buf, label), &is_node) in widget_buffers.iter().zip(widget_labels.iter()).zip(widget_is_node.iter()) {
            let bounds = if is_node {
                let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
                if self.circular_network_pane {
                    TextBounds {
                        left: ((self.network_circle_x - self.network_circle_radius) * s) as i32,
                        top: ((self.network_circle_y - self.network_circle_radius) * s) as i32,
                        right: ((self.network_circle_x + self.network_circle_radius) * s) as i32,
                        bottom: (((self.network_circle_y + self.network_circle_radius) * s) as i32).max(0),
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
            areas.push(TextArea {
                buffer: buf,
                left: label.x * s, top: label.y * s, scale: s,
                bounds,
                default_color: glyphon::Color::rgb(label.color[0], label.color[1], label.color[2]),
                custom_glyphs: &[],
            });
        }

        text_renderer.prepare(device, queue, font_system, text_atlas, text_viewport, areas, swash_cache).unwrap();
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            let old_width = self.width;
            self.physical_width = width;
            self.physical_height = height;
            self.width = width as f32 / self.scale as f32;
            self.height = height as f32 / self.scale as f32;
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);

            let (tex, view) = self.create_depth_texture();
            self.depth_texture = tex;
            self.depth_texture_view = view;

            if old_width > 0.0 {
                let r = self.width / old_width;
                self.splitter1_x *= r;
                self.splitter2_x *= r;
                let body_h = self.body_h();
                self.widgets[SPLITTER1_IDX].set_rect(self.splitter1_x, HEADER_H, SPLITTER_W, body_h);
                self.widgets[SPLITTER2_IDX].set_rect(self.splitter2_x, HEADER_H, SPLITTER_W, body_h);
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
                let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
                let dialog_open = self.widgets[CONFIG_DIALOG_IDX].visible() || self.node_palette_visible;
                let in_network_pane = self.cursor_x >= 0.0
                    && self.cursor_x < self.content_left_w()
                    && self.cursor_y >= node_area_y
                    && self.cursor_y < self.height - STATUS_H;

                let in_viewport = self.cursor_x >= self.content_right_x()
                    && self.cursor_x < self.splitter2_x
                    && self.cursor_y >= node_area_y
                    && self.cursor_y < self.height - STATUS_H;

                let mut focus_changed = false;
                let mut new_pane = None;
                if !dialog_open {
                    if in_network_pane {
                        new_pane = Some(LEFT_MENUBAR_IDX);
                    } else if in_viewport {
                        if self.show_spreadsheet && self.cursor_y >= self.positions[SPREADSHEET_MENUBAR_IDX].1 {
                            new_pane = Some(SPREADSHEET_MENUBAR_IDX);
                        } else {
                            new_pane = Some(RIGHT_MENUBAR_IDX);
                        }
                    } else if self.cursor_x > self.splitter2_x + SPLITTER_W && self.cursor_y >= node_area_y && self.cursor_y < self.height - STATUS_H {
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
                if self.widgets[CONFIG_DIALOG_IDX].visible() {
                    if self.widgets[CONFIG_DIALOG_IDX].mouse_wheel(delta, self.cursor_x, self.cursor_y) {
                        handled = true;
                    }
                } else if !dialog_open {
                    for w in &mut self.widgets {
                        if w.mouse_wheel(delta, self.cursor_x, self.cursor_y) {
                            handled = true;
                        }
                    }
                }

                let result = if handled {
                    if self.focused_widget == Some(CONTENT_IDX) {
                        if let Some(slot_idx) = self.widgets[CONTENT_IDX].selected_node() {
                            let updated_params = self.widgets[PARAM_IDX].node_params();
                            let dir = self.current_dir_mut();
                            if let Some(child) = dir.children.get_mut(slot_idx) {
                                let mut param_changed = false;
                                for (u_name, u_val, _type) in &updated_params {
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

                    // Sync Parameters pane with selected node
                    let params = if self.focused_widget == Some(CONTENT_IDX) {
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
                                let dx = *x * 30.0;
                                let dy = *y * 30.0;
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
                                let dx = pos.x as f32 / self.scale as f32;
                                let dy = pos.y as f32 / self.scale as f32;
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
                                self.rotation_y += dx;
                                self.rotation_x -= dy;

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

                                self.rotation_y += dx;
                                self.rotation_x -= dy;

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
                let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
                let dialog_open = self.widgets[CONFIG_DIALOG_IDX].visible() || self.node_palette_visible;
                let in_network_pane = self.cursor_x >= 0.0
                    && self.cursor_x < self.content_left_w()
                    && self.cursor_y >= node_area_y
                    && self.cursor_y < self.height - STATUS_H;

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
                println!("DEBUG: CursorMoved position=({:?}) scale={} => ({}, {})", position, self.scale, self.cursor_x, self.cursor_y);
                let _ = std::io::Write::flush(&mut std::io::stdout());
                let mut changed = false;

                if self.is_dragging_network_circle {
                    self.network_circle_x = self.cursor_x - self.circle_drag_ox;
                    self.network_circle_y = self.cursor_y - self.circle_drag_oy;
                    self.rebuild_positions();
                    self.apply_layout();
                    self.sync_grid_settings();
                    changed = true;
                } else if self.is_panning {
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
                        if self.widgets[idx].drag_update(self.cursor_x, self.cursor_y) {
                            changed = true;
                        }
                    }

                    if self.drag_widget.is_none() {
                        for i in 0..self.widgets.len() {
                            let (cx, cy) = (self.cursor_x, self.cursor_y);
                            let is_network_part = i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX;
                            let inside = if self.circular_network_pane && is_network_part {
                                let cx_c = self.network_circle_x;
                                let cy_c = self.network_circle_y;
                                let r = self.network_circle_radius;
                                if i == CONTENT_IDX {
                                    let dx = cx - cx_c;
                                    let dy = cy - cy_c;
                                    dx * dx + dy * dy <= r * r && cy >= cy_c - r + 45.0 + MENUBAR_H + BREADCRUMB_H
                                } else if i == LEFT_MENUBAR_IDX {
                                    self.widgets[LEFT_MENUBAR_IDX].hit_test(cx, cy)
                                } else if i == BREADCRUMB_IDX {
                                    cx >= cx_c - r && cx <= cx_c + r && cy >= cy_c - r + 45.0 + MENUBAR_H && cy <= cy_c - r + 45.0 + MENUBAR_H + BREADCRUMB_H
                                } else {
                                    false
                                }
                            } else {
                                self.widgets[i].hit_test(cx, cy)
                            };
                            let (tx, ty) = if inside { (cx, cy) } else { (-9999.0, -9999.0) };
                            if self.widgets[i].cursor_moved(tx, ty) {
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
                let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
                let dialog_open = self.widgets[CONFIG_DIALOG_IDX].visible() || self.node_palette_visible;
                let in_network_pane = self.cursor_x >= 0.0
                    && self.cursor_x < self.content_left_w()
                    && self.cursor_y >= node_area_y
                    && self.cursor_y < self.height - STATUS_H;

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
                        let cx_c = state.network_circle_x;
                        let cy_c = state.network_circle_y;
                        let r = state.network_circle_radius;
                        if i == CONTENT_IDX {
                            let dx = x - cx_c;
                            let dy = y - cy_c;
                            dx * dx + dy * dy <= r * r && y >= cy_c - r + 45.0 + MENUBAR_H + BREADCRUMB_H
                        } else if i == LEFT_MENUBAR_IDX {
                            state.widgets[LEFT_MENUBAR_IDX].hit_test(x, y)
                        } else if i == BREADCRUMB_IDX {
                            x >= cx_c - r && x <= cx_c + r && y >= cy_c - r + 45.0 + MENUBAR_H && y <= cy_c - r + 45.0 + MENUBAR_H + BREADCRUMB_H
                        } else {
                            false
                        }
                    } else {
                        state.widgets[i].hit_test(x, y)
                    }
                };

                let in_circle_network_pane = if self.circular_network_pane {
                    let dx = self.cursor_x - self.network_circle_x;
                    let dy = self.cursor_y - self.network_circle_y;
                    let r = self.network_circle_radius;
                    dx * dx + dy * dy <= r * r && self.cursor_y >= self.network_circle_y - r + 45.0 + MENUBAR_H + BREADCRUMB_H
                } else {
                    in_network_pane
                };

                match btn_state {
                    ElementState::Pressed => {
                        println!("DEBUG: Mouse Pressed button={:?} position=({}, {})", button, self.cursor_x, self.cursor_y);
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                        if *button == MouseButton::Left && self.circular_network_pane {
                            let dx = self.cursor_x - self.network_circle_x;
                            let dy = self.cursor_y - self.network_circle_y;
                            let dist = (dx * dx + dy * dy).sqrt();
                            let on_border = dist >= self.network_circle_radius - 12.0 && dist <= self.network_circle_radius;
                            let hit_menubar = self.widgets[LEFT_MENUBAR_IDX].hit_test(self.cursor_x, self.cursor_y);
                            let mut clicked_menu_item = false;
                            if hit_menubar {
                                clicked_menu_item = self.widgets[LEFT_MENUBAR_IDX].mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y);
                            }
                            if on_border || (hit_menubar && !clicked_menu_item) {
                                self.is_dragging_network_circle = true;
                                self.circle_drag_ox = self.cursor_x - self.network_circle_x;
                                self.circle_drag_oy = self.cursor_y - self.network_circle_y;
                                self.focused_pane = LEFT_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    self.widgets[old].unfocus();
                                    self.focused_widget = None;
                                }
                                self.widgets[PARAM_IDX].unfocus();
                                return true;
                            } else if clicked_menu_item {
                                self.focused_pane = LEFT_MENUBAR_IDX;
                                if let Some(old) = self.focused_widget {
                                    if old != LEFT_MENUBAR_IDX {
                                        self.widgets[old].unfocus();
                                    }
                                }
                                self.widgets[PARAM_IDX].unfocus();
                                self.widgets[LEFT_MENUBAR_IDX].focus();
                                self.focused_widget = Some(LEFT_MENUBAR_IDX);
                                if !self.widgets[LEFT_MENUBAR_IDX].is_menu_open() {
                                    self.widgets[LEFT_MENUBAR_IDX].unfocus();
                                    self.focused_widget = None;
                                }
                                return true;
                            }
                        }

                        if *button == MouseButton::Right {
                            if !dialog_open && in_circle_network_pane {
                                let col = ((self.cursor_x - self.pan_x) / (self.grid_size_x + self.skipped_col_w)).floor() as i32;
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
                            click_target = (0..self.widgets.len()).rev()
                                .find(|&i| hits_widget(self, i, self.cursor_x, self.cursor_y));
                        }

                        // Determine new focused pane
                        let mut new_pane = None;
                        if let Some(i) = click_target {
                            if i == LEFT_MENUBAR_IDX || i == CONTENT_IDX || i == CANVAS_IDX || i == BREADCRUMB_IDX {
                                new_pane = Some(LEFT_MENUBAR_IDX);
                            } else if i == RIGHT_MENUBAR_IDX || i == VIEWPORT_IDX {
                                new_pane = Some(RIGHT_MENUBAR_IDX);
                            } else if i == PARAM_MENUBAR_IDX || i == PARAM_IDX {
                                new_pane = Some(PARAM_MENUBAR_IDX);
                            } else if i == SPREADSHEET_MENUBAR_IDX || i == SPREADSHEET_IDX {
                                new_pane = Some(SPREADSHEET_MENUBAR_IDX);
                            }
                        } else {
                            if self.circular_network_pane {
                                if self.cursor_x > self.splitter2_x + SPLITTER_W {
                                    new_pane = Some(PARAM_MENUBAR_IDX);
                                } else {
                                    if self.show_spreadsheet && self.cursor_y >= self.positions[SPREADSHEET_MENUBAR_IDX].1 {
                                        new_pane = Some(SPREADSHEET_MENUBAR_IDX);
                                    } else {
                                        new_pane = Some(RIGHT_MENUBAR_IDX);
                                    }
                                }
                            } else {
                                if self.cursor_x < self.splitter1_x {
                                    new_pane = Some(LEFT_MENUBAR_IDX);
                                } else if self.cursor_x > self.splitter2_x + SPLITTER_W {
                                    new_pane = Some(PARAM_MENUBAR_IDX);
                                } else {
                                    if self.show_spreadsheet && self.cursor_y >= self.positions[SPREADSHEET_MENUBAR_IDX].1 {
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
                        }
                        if click_target.is_none() && !dialog_open && in_circle_network_pane {
                            let col = ((self.cursor_x - self.pan_x) / (self.grid_size_x + self.skipped_col_w)).floor() as i32;
                            let row = ((self.cursor_y - node_area_y - self.pan_y) / (self.grid_size_y + self.skipped_row_h)).floor() as i32;
                            self.grid_cursor_col = col;
                            self.grid_cursor_row = row;
                            changed = true;
                        }
                        if let Some(i) = click_target {
                            if self.widgets[i].mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y) {
                                changed = true;
                            }
                            if self.widgets[i].draggable() {
                                self.widgets[i].drag_begin(self.cursor_x, self.cursor_y);
                                self.drag_widget = Some(i);
                            }
                            if i != PARAM_IDX {
                                self.widgets[i].focus();
                                self.focused_widget = Some(i);
                                if self.widgets[i].is_menu_bar() && !self.widgets[i].is_menu_open() {
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
                                    let col = ((self.cursor_x - self.pan_x) / (self.grid_size_x + self.skipped_col_w)).floor() as i32;
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
                        if self.is_dragging_network_circle {
                            self.is_dragging_network_circle = false;
                            self.sync_layout();
                            self.read_panel_offsets();
                            self.upload_vertices();
                            return true;
                        }
                        if self.drag_widget.is_some() {
                            let idx = self.drag_widget.unwrap();
                            if idx == CONTENT_IDX {
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
                        for w in &mut self.widgets {
                            if w.mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y) {
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
                        self.focused_pane = get_next_visible_pane(self.focused_pane, self.show_spreadsheet, self.modifiers.shift_key());
                        self.sync_pane_focus();
                        return true;
                    }
                }

                if self.widgets[PARAM_IDX].keyboard_input(event) {
                    if self.focused_widget == Some(CONTENT_IDX) {
                        if let Some(slot_idx) = self.widgets[CONTENT_IDX].selected_node() {
                            let updated_params = self.widgets[PARAM_IDX].node_params();
                            let dir = self.current_dir_mut();
                            if let Some(child) = dir.children.get_mut(slot_idx) {
                                for (u_name, u_val, _) in &updated_params {
                                    if let Some(p) = child.params.iter_mut().find(|p| p.name == *u_name) {
                                        p.default = u_val.clone();
                                    }
                                }
                            }
                        }
                    }
                    self.rebuild_scene_geometry();
                    self.sync_nodes();
                    return true;
                }

                if self.node_palette_visible {
                    return self.handle_node_palette_key(event);
                }
                let mut changed = false;
                if event.state == ElementState::Pressed {
                    if !self.widgets[CONFIG_DIALOG_IDX].visible() {
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
                                                self.rotation_y = 0.0;
                                                self.rotation_x = 0.0;
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

                                                    let viewport_w = self.content_left_w();
                                                    let body_h = self.body_h();
                                                    let viewport_h = body_h - MENUBAR_H - BREADCRUMB_H;

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
                    }

                    if changed {
                        self.keep_cursor_in_view();
                    }

                    if !changed {
                        if event.logical_key == Key::Named(NamedKey::Tab) && !self.widgets[CONFIG_DIALOG_IDX].visible() {
                            self.open_node_palette();
                            return true;
                        }
                        for bind in &self.keybinds {
                            if bind.matches(&self.modifiers, &event.logical_key) {
                                self.pending_action = Some(bind.action);
                                return true;
                            }
                        }
                    }
                }
                if changed {
                    true
                } else if let Some(idx) = self.focused_widget {
                    self.widgets[idx].keyboard_input(event)
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
        for w in &mut self.widgets {
            if w.tick(dt) {
                tick_changed = true;
            }
        }

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
            self.rotation_y += self.rotate_velocity_yaw * dt;
            self.rotation_x += self.rotate_velocity_pitch * dt;

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
                let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
                let margin = 30.0_f32;
                let pan_speed = 5.0_f32;
                let clw = self.content_left_w();
                let max_y = self.height - STATUS_H;

                if self.cursor_x >= -50.0 && self.cursor_x < clw + 50.0
                    && self.cursor_y >= node_area_y - 50.0 && self.cursor_y < max_y + 50.0
                {
                    if self.cursor_x < margin {
                        self.pan_x += pan_speed;
                        panned = true;
                    } else if self.cursor_x > clw - margin {
                        self.pan_x -= pan_speed;
                        panned = true;
                    }

                    if self.cursor_y < node_area_y + margin {
                        self.pan_y += pan_speed;
                        panned = true;
                    } else if self.cursor_y > max_y - margin {
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
                self.widgets[idx].drag_update(self.cursor_x, self.cursor_y);
            }
            self.sync_layout();
            self.read_panel_offsets();
            self.upload_vertices();
        }

        self.prepare_text();

        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return false;
            }
            Err(wgpu::SurfaceError::Timeout) => return false,
            Err(e) => { eprintln!("Surface error: {e:?}"); return false; }
        };

        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Encoder"),
        });

        // 3D canvas render pass (background layer)
        {
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
                if self.active_camera != "Default Camera" {
                    if let Some(node) = self.current_dir().children.iter().find(|c| c.node_type == "camera" && c.name == self.active_camera) {
                        let mut cx = 2.5f32;
                        let mut cy = 1.8f32;
                        let mut cz = 2.5f32;
                        for p in &node.params {
                            if p.name == "X" {
                                if let Ok(val) = p.default.parse::<f32>() {
                                    cx = val;
                                }
                            } else if p.name == "Y" {
                                if let Ok(val) = p.default.parse::<f32>() {
                                    cy = val;
                                }
                            } else if p.name == "Z" {
                                if let Ok(val) = p.default.parse::<f32>() {
                                    cz = val;
                                }
                            }
                        }
                        camera_pos = Vec3::new(cx, cy, cz);
                    }
                }
                camera_pos *= self.viewport_zoom;
                let view_mat = Mat4::look_at_rh(camera_pos, Vec3::ZERO, Vec3::Y);
                let model = Mat4::from_rotation_y(self.rotation_y) * Mat4::from_rotation_x(self.rotation_x);
                let mvp = proj * view_mat * model;
                self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[mvp.to_cols_array_2d()]));

                let mvp_grid = proj * view_mat * model;
                self.queue.write_buffer(&self.uniform_buffer_grid, 0, bytemuck::cast_slice(&[mvp_grid.to_cols_array_2d()]));

                let cam_angle_y = camera_pos.x.atan2(camera_pos.z);
                let model_pivot = Mat4::from_rotation_y(self.rotation_y + cam_angle_y);
                let mvp_pivot = proj * view_mat * model_pivot;
                self.queue.write_buffer(&self.uniform_buffer_pivot, 0, bytemuck::cast_slice(&[mvp_pivot.to_cols_array_2d()]));

                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("3D Render Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
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
            }
        }

        // UI render pass (foreground layer)
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("UI Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.render_pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.vertex_count, 0..1);

            self.text_renderer.render(&self.text_atlas, &self.text_viewport, &mut pass).unwrap();
        }

        self.queue.submit(std::iter::once(encoder.finish()));
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
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
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
                        let ev = WindowEvent::CursorMoved {
                            position: LocalPosition {
                                x: event.position.0,
                                y: event.position.1,
                            },
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Press { button, .. } => {
                        let btn = match *button {
                            272 => clear_ui::widget::MouseButton::Left,
                            273 => clear_ui::widget::MouseButton::Right,
                            274 => clear_ui::widget::MouseButton::Middle,
                            _ => continue,
                        };
                        eprintln!("DEBUG MOUSE PRESS: button={:?}, pos={:?}, local=({}, {})", btn, event.position, cx, cy);
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
                        eprintln!("DEBUG MOUSE RELEASE: button={:?}, pos={:?}, local=({}, {})", btn, event.position, cx, cy);
                        let ev = WindowEvent::MouseInput {
                            state: clear_ui::widget::ElementState::Released,
                            button: btn,
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Axis { horizontal, vertical, .. } => {
                        let h_val = horizontal.absolute as f32;
                        let v_val = vertical.absolute as f32;
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
        _state: &mut Self,
        _proxy: &clear_ui::protocol::zclear_inspector_v1::ZclearInspectorV1,
        _event: clear_ui::protocol::zclear_inspector_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {}
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

        eprintln!("DEBUG KEY: keysym={:?}, state={:?}, logical_key={:?}", event.keysym, state, logical_key);

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

            if let Some((menu_idx, item_idx)) = state.widgets[HEADER_IDX].menu_click() {
                if menu_idx == 0 { // File
                    match item_idx {
                        0 => { // New Project
                            state.new_project();
                            changed = true;
                        }
                        1 => { // Open
                            let proj_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("project.json");
                            if let Err(e) = state.load_from_file(&proj_path) {
                                eprintln!("Failed to load project: {:?}", e);
                            }
                            changed = true;
                        }
                        2 => { // Save
                            let proj_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("project.json");
                            if let Err(e) = state.save_to_file(&proj_path) {
                                eprintln!("Failed to save project: {:?}", e);
                            }
                            changed = true;
                        }
                        3 => { // Configure
                            state.execute_action(Action::ToggleConfigure);
                            changed = true;
                        }
                        4 => { // Exit
                            state.exit_requested = true;
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
                            let proj_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("project.json");
                            if let Err(e) = state.load_from_file(&proj_path) {
                                eprintln!("Failed to load project: {:?}", e);
                            }
                            changed = true;
                        }
                        2 => { // Save
                            let proj_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("project.json");
                            if let Err(e) = state.save_to_file(&proj_path) {
                                eprintln!("Failed to save project: {:?}", e);
                            }
                            changed = true;
                        }
                        _ => {}
                    }
                } else if menu_idx == 2 { // View
                    match item_idx {
                        2 => {
                            state.circular_network_pane = !state.circular_network_pane;
                            state.widgets[LEFT_MENUBAR_IDX].set_item_checked(2, 2, state.circular_network_pane);
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_grid_settings();
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

            let mut settings_changed = false;
            while let Some((id, val)) = state.widgets[CONFIG_DIALOG_IDX].take_config_toggle() {
                match id {
                    0 => {
                        state.grid_snap_enabled = val;
                        state.widgets[CONTENT_IDX].set_grid_snap_enabled(val);
                    }
                    1 => {
                        state.network_grid_visible = val;
                        state.widgets[CONTENT_IDX].set_show_network_grid(val);
                    }
                    2 => {
                        state.show_grid = val;
                        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 0, val);
                    }
                    3 => {
                        state.show_cube = val;
                        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 1, val);
                    }
                    4 => {
                        state.show_origin = val;
                        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 2, val);
                    }
                    5 => {
                        state.show_camera_pivot = val;
                        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 3, val);
                    }
                    _ => {}
                }
                changed = true;
                settings_changed = true;
            }

            while let Some((id, val)) = state.widgets[CONFIG_DIALOG_IDX].take_config_spin() {
                match id {
                    0 => state.grid_size_x = val,
                    1 => state.grid_size_y = val,
                    2 => state.skipped_row_h = val,
                    3 => state.skipped_col_w = val,
                    4 => state.viewport_bg_color[0] = val,
                    5 => state.viewport_bg_color[1] = val,
                    6 => state.viewport_bg_color[2] = val,
                    7 => {
                        state.origin_size = val;
                        state.update_origin_geometry();
                    }
                    8 => {
                        state.grid_thickness = val;
                        state.update_grid_geometry();
                    }
                    9 => {
                        state.camera_pivot_size = val;
                        state.update_pivot_geometry();
                    }
                    10 => {
                        state.node_color[0] = val;
                        colors::set_node_color([state.node_color[0], state.node_color[1], state.node_color[2], 1.0]);
                    }
                    11 => {
                        state.node_color[1] = val;
                        colors::set_node_color([state.node_color[0], state.node_color[1], state.node_color[2], 1.0]);
                    }
                    12 => {
                        state.node_color[2] = val;
                        colors::set_node_color([state.node_color[0], state.node_color[1], state.node_color[2], 1.0]);
                    }
                    _ => {}
                }
                state.sync_grid_settings();
                changed = true;
                settings_changed = true;
            }

            if settings_changed {
                state.save_settings();
            }

            if changed {
                state.sync_layout();
                state.read_panel_offsets();
                state.sync_cursor_and_selection();

                if state.drag_widget == Some(PARAM_IDX) && state.widgets[PARAM_IDX].is_dragging() {
                    if state.focused_widget == Some(CONTENT_IDX) {
                        if let Some(slot_idx) = state.widgets[CONTENT_IDX].selected_node() {
                            let updated_params = state.widgets[PARAM_IDX].node_params();
                            let dir = state.current_dir_mut();
                            if let Some(child) = dir.children.get_mut(slot_idx) {
                                let mut param_changed = false;
                                for (u_name, u_val, _type) in &updated_params {
                                    if let Some(p) = child.params.iter_mut().find(|p| p.name == *u_name) {
                                        if p.default != *u_val {
                                            p.default = u_val.clone();
                                            param_changed = true;
                                        }
                                    }
                                }
                                if param_changed {
                                    state.rebuild_scene_geometry();
                                }
                            }
                        }
                    }
                }

                state.sync_nodes();

                // Sync Parameters pane with selected node
                let params = if state.focused_widget == Some(CONTENT_IDX) {
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

                state.upload_vertices();
            }

            state.update_status_text(&format!(
                "col: {:.0}  vp: {:.0}  params: {:.0}",
                state.content_left_w(), state.viewport_w(), state.param_w(),
            ));

            if changed {
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
                            state.rotation_y = 0.0;
                            state.rotation_x = 0.0;
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
            self.redraw = true;
        }
    }
}

fn main() {
    let conn = Connection::connect_to_env().unwrap();
    let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh).unwrap();
    let xdg_shell_state = XdgShell::bind(&globals, &qh).unwrap();
    let shm_state = Shm::bind(&globals, &qh).unwrap();
    let seat_state = SeatState::new(&globals, &qh);
    let output_state = OutputState::new(&globals, &qh);
    let inspector = globals.bind(&qh, 1..=1, ()).ok();

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
    };

    // Perform a roundtrip to populate output_state with active output scales
    event_queue.roundtrip(&mut app).unwrap();

    let scale = clear_ui::wayland::detect_scale_factor(&app.output_state);

    let pw = (1280.0 * scale) as u32;
    let ph = (800.0 * scale) as u32;

    let state = pollster::block_on(State::new(
        &conn,
        &qh,
        &app.compositor_state,
        &app.xdg_shell_state,
        pw, ph,
        scale,
    ));

    app.window = Some(state.window.clone());
    app.surface = Some(state.wl_surface.clone());
    app.state = Some(state);

    if let Some(ref inspector) = app.inspector {
        if let Some(ref surface) = app.surface {
            inspector.register_client(surface);
        }
    }

    let (sender, channel) = calloop::channel::channel::<CustomEvent>();

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
                    } else {
                        let body = "{\"error\":\"failed to send event to event loop\"}";
                        let response = format!(
                            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
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

                    let mut body_bytes = vec![0; content_length];
                    if reader.read_exact(&mut body_bytes).is_ok() {
                        if let Ok(body_str) = String::from_utf8(body_bytes) {
                            if let Ok(action) = serde_json::from_str::<HttpAction>(&body_str) {
                                let (tx, rx) = std::sync::mpsc::channel();
                                if server_sender.send(CustomEvent::PostAction(action, tx)).is_ok() {
                                    match rx.recv() {
                                        Ok(Ok(msg)) => {
                                            let body = format!("{{\"status\":\"success\",\"message\":\"{}\"}}", msg);
                                            let response = format!(
                                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                                body.len(),
                                                body
                                            );
                                            let _ = write_stream.write_all(response.as_bytes());
                                        }
                                        Ok(Err(err)) => {
                                            let body = format!("{{\"status\":\"error\",\"error\":\"{}\"}}", err);
                                            let response = format!(
                                                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                                body.len(),
                                                body
                                            );
                                            let _ = write_stream.write_all(response.as_bytes());
                                        }
                                        Err(_) => {
                                            let body = "{\"status\":\"error\",\"error\":\"internal receiver error\"}";
                                            let response = format!(
                                                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                                body.len(),
                                                body
                                            );
                                            let _ = write_stream.write_all(response.as_bytes());
                                        }
                                    }
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
                            let body = "{\"status\":\"error\",\"error\":\"body is not valid UTF-8\"}";
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
        let timeout = if app.redraw {
            std::time::Duration::from_millis(0)
        } else {
            std::time::Duration::from_millis(16)
        };
        event_loop.dispatch(timeout, &mut app).unwrap();

        if app.exit || app.state.as_ref().map(|s| s.exit_requested).unwrap_or(false) {
            break;
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
        assert_eq!(proj.root.children.len(), 2);
        assert_eq!(proj.root.children[0].name, "Camera 1");
        assert_eq!(proj.root.children[0].position, (1.0, 1.0));
        assert_eq!(proj.root.children[1].name, "Sphere 1");
        assert_eq!(proj.root.children[1].position, (4.0, 2.0));
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
        
        let serialized = serde_json::to_string(&settings).unwrap();
        let settings_roundtrip: DesignSettings = serde_json::from_str(&serialized).unwrap();
        assert_eq!(settings_roundtrip.show_camera_pivot_enabled, false);
        assert_eq!(settings_roundtrip.camera_pivot_size, 1.0);
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
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, false, false), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, false, false), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, false, false), LEFT_MENUBAR_IDX);

        // With spreadsheet (4 panes: LEFT, RIGHT, PARAM, SPREADSHEET)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, false), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, false), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, false), SPREADSHEET_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(SPREADSHEET_MENUBAR_IDX, true, false), LEFT_MENUBAR_IDX);

        // Reverse cycling with shift key (without spreadsheet)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, false, true), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, false, true), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, false, true), LEFT_MENUBAR_IDX);

        // Reverse cycling with shift key (with spreadsheet)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true), SPREADSHEET_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(SPREADSHEET_MENUBAR_IDX, true, true), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true), LEFT_MENUBAR_IDX);
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
}

