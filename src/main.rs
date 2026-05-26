use std::sync::Arc;
use std::time::Instant;
use std::fs;
use std::path::Path;
use std::net::TcpListener;
use std::io::{BufRead, BufReader, Read, Write};

use serde::{Deserialize, Serialize};

use opencl3::platform::get_platforms;
use opencl3::device::{Device, CL_DEVICE_TYPE_GPU, CL_DEVICE_TYPE_CPU};
use opencl3::context::Context;
use opencl3::command_queue::CommandQueue;
use opencl3::program::Program;
use opencl3::kernel::{Kernel, ExecuteKernel};
use opencl3::memory::{Buffer as ClBuffer, CL_MEM_READ_WRITE};
use opencl3::types::{cl_float, cl_int, CL_TRUE};

use clear_ui::widget::{ElementState, MouseButton, MouseScrollDelta, KeyEvent, Key, NamedKey, Position};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window, delegate_output,
    registry::{ProvidesRegistryState, RegistryState},
    output::{OutputHandler, OutputState},
    seat::{
        keyboard::KeyboardHandler,
        pointer::PointerHandler,
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

use clear_ui::widget::{Breadcrumb, Canvas, ColorSelector, ContentBg, MenuBar, Node, ParametersBg, Spinbox, Splitter, Spreadsheet, StatusBar, TextLabel, Toggle, ViewportBg, Widget};
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
const NODE_SLOT_START: usize = 10;
const NODE_SLOT_COUNT: usize = 32;
const STATUS_IDX: usize = NODE_SLOT_START + NODE_SLOT_COUNT;
const BREADCRUMB_IDX: usize = STATUS_IDX + 1;
const CONFIG_DIALOG_IDX: usize = BREADCRUMB_IDX + 1;
const NODE_PALETTE_IDX: usize = CONFIG_DIALOG_IDX + 1;
const SPREADSHEET_IDX: usize = NODE_PALETTE_IDX + 1;
const SPREADSHEET_MENUBAR_IDX: usize = SPREADSHEET_IDX + 1;

const HEADER_H: f32 = 40.0;
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
        }
    }
}

impl DesignSettings {
    fn file_path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lsgalante".to_string());
        let mut path = std::path::PathBuf::from(home);
        path.push(".config");
        path.push("clearwm");
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

            let total_h = y2 - (py + 65.0);
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
        None
    }

    fn hit_test(&self, px: f32, py: f32) -> bool {
        if !self.visible { return false; }
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }

    fn cursor_moved(&mut self, px: f32, py: f32) -> bool {
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
                handled = true;
            } else if self.toggle_network_grid.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.spin_skipped_col_w.unfocus();
                handled = true;
            } else if self.spin_grid_x.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.spin_skipped_col_w.unfocus();
                handled = true;
            } else if self.spin_grid_y.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_skipped_row_h.unfocus();
                self.spin_skipped_col_w.unfocus();
                handled = true;
            } else if self.spin_skipped_row_h.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_col_w.unfocus();
                handled = true;
            } else if self.spin_skipped_col_w.mouse_input(button, state, px, py) {
                self.toggle_grid_snap.unfocus();
                self.toggle_network_grid.unfocus();
                self.spin_grid_x.unfocus();
                self.spin_grid_y.unfocus();
                self.spin_skipped_row_h.unfocus();
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
                handled = true;
            } else if self.toggle_show_cube.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                handled = true;
            } else if self.toggle_show_origin.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                handled = true;
            } else if self.toggle_show_camera_pivot.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                handled = true;
            } else if self.spin_grid_thickness.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                handled = true;
            } else if self.spin_origin_size.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_camera_pivot_size.unfocus();
                self.color_selector.unfocus();
                handled = true;
            } else if self.spin_camera_pivot_size.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.color_selector.unfocus();
                handled = true;
            } else if self.color_selector.mouse_input(button, state, px, py) {
                self.toggle_show_grid.unfocus();
                self.toggle_show_cube.unfocus();
                self.toggle_show_origin.unfocus();
                self.toggle_show_camera_pivot.unfocus();
                self.spin_grid_thickness.unfocus();
                self.spin_origin_size.unfocus();
                self.spin_camera_pivot_size.unfocus();
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
        } else if self.active_page == 2 {
            if self.spin_grid_thickness.keyboard_input(event) { return true; }
            if self.spin_origin_size.keyboard_input(event) { return true; }
            if self.spin_camera_pivot_size.keyboard_input(event) { return true; }
            if self.color_selector.keyboard_input(event) { return true; }
        }
        if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) {
            self.color_selector.unfocus();
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
    ]
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum GAttribute {
    Float(f32),
    Float2([f32; 2]),
    Float3([f32; 3]),
    Float4([f32; 4]),
}

#[derive(Clone, Debug)]
struct GVertex {
    pos: [f32; 3],
    col: [f32; 3],
    attributes: std::collections::HashMap<String, GAttribute>,
}

#[derive(Clone, Debug, Default)]
struct Geometry {
    vertices: Vec<GVertex>,
}

impl Geometry {
    fn new() -> Self {
        Geometry { vertices: Vec::new() }
    }

    fn merge(&mut self, other: Geometry) {
        self.vertices.extend(other.vertices);
    }

    fn to_vertex3d_vec(&self) -> Vec<Vertex3D> {
        self.vertices.iter().map(|v| Vertex3D {
            position: v.pos,
            color: v.col,
        }).collect()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex3D {
    position: [f32; 3],
    color: [f32; 3],
}

impl Vertex3D {
    const ATTRIBS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

fn cube_vertices() -> Vec<Vertex3D> {
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

fn sphere_vertices(center: Vec3, radius: f32) -> Geometry {
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

    let mut attributes = std::collections::HashMap::new();
    attributes.insert("Norm".to_string(), GAttribute::Float3(n.to_array()));
    attributes.insert("UV".to_string(), GAttribute::Float2([u, v]));

    GVertex {
        pos,
        col,
        attributes,
    }
}

fn line_vertices(start: Vec3, end: Vec3, thickness: f32) -> Geometry {
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
            let mut attributes = std::collections::HashMap::new();
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

fn node_param_f32(node: &FsNode, name: &str, fallback: f32) -> f32 {
    node.params.iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .and_then(|p| p.default.parse::<f32>().ok())
        .unwrap_or(fallback)
}

fn run_opencl_kernel(code: &str, geom: &mut Geometry) -> Result<(), String> {
    if geom.vertices.is_empty() {
        return Ok(());
    }

    let platforms = get_platforms().map_err(|e| format!("Failed to get platforms: {:?}", e))?;
    if platforms.is_empty() {
        return Err("No OpenCL platforms found".to_string());
    }

    // Try to find a GPU device first, then fallback to CPU
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
    let device_id = device_id.ok_or_else(|| "No OpenCL devices found".to_string())?;
    let device = Device::new(device_id);

    let context = Context::from_device(&device).map_err(|e| format!("Failed to create Context: {:?}", e))?;
    let queue = unsafe { CommandQueue::create(&context, device_id, 0) }
        .map_err(|e| format!("Failed to create CommandQueue: {:?}", e))?;

    let mut program = Program::create_from_source(&context, code).map_err(|e| format!("Failed to create Program: {:?}", e))?;
    if let Err(e) = program.build(&[device_id], "") {
        let log = program.get_build_log(device_id).unwrap_or_else(|_| "Failed to retrieve build log".to_string());
        return Err(format!("OpenCL JIT compilation error: {}\nLog:\n{}", e, log));
    }

    let kernel = Kernel::create(&program, "process").map_err(|e| format!("Failed to create kernel 'process': {:?}", e))?;

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
        ExecuteKernel::new(&kernel)
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

    Ok(())
}

fn network_sphere_vertices(root: &FsNode) -> Geometry {
    fn visit(node: &FsNode, count: &mut usize, out: &mut Geometry) {
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
        }
        for child in &node.children {
            visit(child, count, out);
        }
    }

    let mut out = Geometry::new();
    let mut count = 0;
    for child in &root.children {
        visit(child, &mut count, &mut out);
    }
    out
}

fn find_sphere_index(root: &FsNode, target: &FsNode) -> Option<usize> {
    fn visit(node: &FsNode, target: &FsNode, count: &mut usize) -> Option<usize> {
        let is_target = std::ptr::eq(node, target);
        if node.node_type.eq_ignore_ascii_case("sphere") || node.node_type.eq_ignore_ascii_case("line") {
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

fn origin_vectors_vertices(scale: f32) -> Vec<Vertex3D> {
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

fn camera_pivot_vertices(scale: f32) -> Vec<Vertex3D> {
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

fn grid_vertices(thickness: f32) -> Vec<Vertex3D> {
    let color = [0.35, 0.35, 0.40];
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

fn quad_vertices(
    x: f32, y: f32, w: f32, h: f32,
    surface_w: f32, surface_h: f32,
    color: [f32; 4],
) -> [Vertex; 6] {
    let x0 = (x / surface_w) * 2.0 - 1.0;
    let y0 = 1.0 - (y / surface_h) * 2.0;
    let x1 = ((x + w) / surface_w) * 2.0 - 1.0;
    let y1 = 1.0 - ((y + h) / surface_h) * 2.0;
    [
        Vertex { position: [x0, y0], color },
        Vertex { position: [x1, y0], color },
        Vertex { position: [x0, y1], color },
        Vertex { position: [x1, y0], color },
        Vertex { position: [x1, y1], color },
        Vertex { position: [x0, y1], color },
    ]
}

fn widget_vertices(w: &dyn Widget, sw: f32, sh: f32) -> Vec<Vertex> {
    let (x, y, ww, h) = w.rect();
    quad_vertices(x, y, ww, h, sw, sh, w.color()).to_vec()
}

fn quad_vertices_clipped(
    x: f32, y: f32, w: f32, h: f32,
    surface_w: f32, surface_h: f32,
    color: [f32; 4],
    clip: (f32, f32, f32, f32),
) -> Vec<Vertex> {
    let (cx0, cy0, cx1, cy1) = clip;
    let ix0 = x.max(cx0);
    let iy0 = y.max(cy0);
    let ix1 = (x + w).min(cx1);
    let iy1 = (y + h).min(cy1);
    if ix1 <= ix0 || iy1 <= iy0 {
        return Vec::new();
    }
    quad_vertices(ix0, iy0, ix1 - ix0, iy1 - iy0, surface_w, surface_h, color).to_vec()
}

fn widget_vertices_clipped(w: &dyn Widget, sw: f32, sh: f32, clip: (f32, f32, f32, f32)) -> Vec<Vertex> {
    let (x, y, ww, h) = w.rect();
    quad_vertices_clipped(x, y, ww, h, sw, sh, w.color(), clip)
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
    node_slots: Vec<usize>,
    splitter1_x: f32,
    splitter2_x: f32,
    left_offsets: Vec<(f32, f32)>,
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
        self.node_palette_visible = true;
        self.node_palette_query.clear();
        self.node_palette_selected = 0;
        self.refresh_node_palette();
        self.upload_vertices();
    }

    fn close_node_palette(&mut self) {
        self.node_palette_visible = false;
        self.refresh_node_palette();
        self.upload_vertices();
    }


    fn place_selected_node(&mut self) -> bool {
        let Some(&template_idx) = self.node_palette_filtered.get(self.node_palette_selected) else { return false; };
        let child_idx = self.current_dir().children.len();
        if child_idx >= self.node_slots.len() { return false; }
        let mut node = self.node_templates[template_idx].node.clone();
        let (nx, ny) = self.find_empty_cell(self.grid_cursor_col as f32, self.grid_cursor_row as f32, None);
        node.position = (nx, ny);
        self.current_dir_mut().children.push(node);
        self.left_offsets[child_idx] = (nx, ny);
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
        let node_infos: Vec<Option<(String, (f32, f32))>> = {
            let dir = self.current_dir();
            self.node_slots.iter().enumerate().map(|(i, _)| {
                dir.children.get(i).map(|c| (c.name.clone(), c.position))
            }).collect()
        };
        let params_list: Vec<Vec<(String, String, String)>> = {
            let dir = self.current_dir();
            self.node_slots.iter().enumerate().map(|(i, _)| {
                dir.children.get(i).map(|c| param_display(&c.params)).unwrap_or_default()
            }).collect()
        };
        let geom_visibles: Vec<bool> = {
            let dir = self.current_dir();
            self.node_slots.iter().enumerate().map(|(i, _)| {
                dir.children.get(i).map(|c| c.geometry_visible).unwrap_or(true)
            }).collect()
        };

        for (i, &slot) in self.node_slots.iter().enumerate() {
            if let Some((name, position)) = &node_infos[i] {
                if i < self.left_offsets.len() {
                    self.left_offsets[i] = *position;
                }
                self.widgets[slot].set_node_name(name);
                self.widgets[slot].set_display_params(&params_list[i]);
                self.widgets[slot].set_geom_visible(geom_visibles[i]);
            } else {
                self.widgets[slot].set_node_name("");
                self.widgets[slot].set_display_params(&[]);
                self.widgets[slot].set_geom_visible(true);
            }
        }
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
        if let Some(focused) = self.focused_widget {
            if self.node_slots.contains(&focused) {
                if let Some(slot_idx) = self.node_slots.iter().position(|&x| x == focused) {
                    let dir = self.current_dir();
                    if slot_idx < dir.children.len() {
                        selected_node = Some(&dir.children[slot_idx]);
                    }
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
        window.set_min_size(Some((pw, ph)));
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
                power_preference: wgpu::PowerPreference::HighPerformance,
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

        let splitter1_x = (sw - SPLITTER_W) * 0.25;
        let splitter2_x = (sw - SPLITTER_W) * 0.75;
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
        let node_slots: Vec<usize> = (NODE_SLOT_START..NODE_SLOT_START + NODE_SLOT_COUNT).collect();
        let left_offsets: Vec<(f32, f32)> = (0..NODE_SLOT_COUNT)
            .map(|i| (0.0, i as f32))
            .collect();

        let mut widgets: Vec<Box<dyn Widget>> = vec![
            Box::new(MenuBar::new(0.0, 0.0, 0.0, HEADER_H).with_title("Clear Design Interface").with_item("File", &["New Project", "Open", "Save", "Configure", "Exit"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Reset Zoom"]).with_item("Help", &["About"])),
            Box::new(ContentBg::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(ViewportBg::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(ParametersBg::new()),
            Box::new(Canvas::new()),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("0: Network").with_item("File", &["New", "Open", "Save"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out"])),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("1: Viewport").with_item("Camera", &["Perspective", "Orthographic"]).with_item("Display", &["Square Aspect"]).with_item("Guides", &["Show Grid", "Cube", "Origin", "Camera Pivot"])),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("2: Parameters").with_item("Preset", &["Default", "Custom"]).with_item("Reset", &["All"])),
        ];
        for _ in 0..NODE_SLOT_COUNT {
            widgets.push(Box::new(Node::new(0.0, 0.0, 0.0, 0.0, "")));
        }
        widgets.push(Box::new(StatusBar::new()));
        widgets.push(Box::new(Breadcrumb::new()));
        widgets.push(Box::new(ConfigDialog::new(&settings)));
        widgets.push(Box::new(NodePalette::new()));
        widgets.push(Box::new(Spreadsheet::new()));
        
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
            node_slots,
            splitter1_x,
            splitter2_x,
            left_offsets,
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
        };

        state.sync_nodes();
        state.rebuild_scene_geometry();
        state.sync_grid_settings();
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 0, state.show_grid);
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 1, state.show_cube);
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 2, state.show_origin);
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 3, state.show_camera_pivot);

        state.rebuild_positions();
        state.apply_layout();
        state.update_panel_bounds();
        state.sync_pane_focus();
        state.upload_vertices();
        state
    }

    fn sync_grid_settings(&mut self) {
        let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
        self.widgets[CONTENT_IDX].set_show_network_grid(self.network_grid_visible);
        self.widgets[CONTENT_IDX].set_grid_sizes(self.grid_size_x, self.grid_size_y);
        self.widgets[CONTENT_IDX].set_skipped_sizes(self.skipped_row_h, self.skipped_col_w);
        self.widgets[CONTENT_IDX].set_grid_origin(self.pan_x, node_area_y + self.pan_y);

        for &i in &self.node_slots {
            let gx = if self.grid_snap_enabled { self.grid_size_x + self.skipped_col_w } else { 0.0 };
            let gy = if self.grid_snap_enabled { self.grid_size_y + self.skipped_row_h } else { 0.0 };
            self.widgets[i].set_grid_snap(gx, gy);
            self.widgets[i].set_grid_origin(self.pan_x, node_area_y + self.pan_y);
        }
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
        for &i in &self.node_slots {
            let (x, y, _, _) = self.widgets[i].rect();
            self.widgets[i].set_rect(x, y, self.grid_size_x, self.grid_size_y);
        }
    }

    fn keep_cursor_in_view(&mut self) {
        let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
        let cx = self.grid_cursor_col as f32 * (self.grid_size_x + self.skipped_col_w) + self.pan_x;
        let cy = node_area_y + self.grid_cursor_row as f32 * (self.grid_size_y + self.skipped_row_h) + self.pan_y;
        let cw = self.grid_size_x;
        let ch = self.grid_size_y;
        let clw = self.content_left_w();
        let max_y = self.height - STATUS_H;

        if cx < 0.0 {
            self.pan_x -= cx;
        } else if cx + cw > clw {
            self.pan_x -= cx + cw - clw;
        }

        if cy < node_area_y {
            self.pan_y -= cy - node_area_y;
        } else if cy + ch > max_y {
            self.pan_y -= cy + ch - max_y;
        }
        self.sync_grid_settings();
    }

    fn rebuild_positions(&mut self) {
        self.clamp_splitters();

        let body_h = self.body_h();
        let clw = self.content_left_w();
        let crx = self.content_right_x();
        let vw = self.viewport_w();
        let px = self.param_x();

        self.positions[0] = (0.0, 0.0, self.width, HEADER_H);
        self.positions[CONTENT_IDX] = (0.0, HEADER_H, clw, body_h);
        self.positions[SPLITTER1_IDX] = (self.splitter1_x, HEADER_H, SPLITTER_W, body_h);
        self.positions[SPLITTER2_IDX] = (self.splitter2_x, HEADER_H, SPLITTER_W, body_h);
        self.positions[PARAM_IDX] = (px, HEADER_H, self.param_w(), body_h);

        self.positions[VIEWPORT_IDX] = (crx, HEADER_H, vw, body_h);
        self.positions[CANVAS_IDX] = (0.0, HEADER_H, self.width, body_h);

        if self.show_spreadsheet {
            let viewport_h = body_h * 2.0 / 3.0;
            let spreadsheet_h = body_h - viewport_h;

            self.positions[SPREADSHEET_MENUBAR_IDX] = (crx, HEADER_H + viewport_h, vw, MENUBAR_H);
            self.positions[SPREADSHEET_IDX] = (crx, HEADER_H + viewport_h + MENUBAR_H, vw, spreadsheet_h - MENUBAR_H);
        } else {
            self.positions[SPREADSHEET_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[SPREADSHEET_MENUBAR_IDX] = (0.0, 0.0, 0.0, 0.0);
        }

        self.positions[LEFT_MENUBAR_IDX] = (0.0, HEADER_H, clw, MENUBAR_H);
        self.positions[RIGHT_MENUBAR_IDX] = (crx, HEADER_H, vw, MENUBAR_H);
        self.positions[PARAM_MENUBAR_IDX] = (px, HEADER_H, self.param_w(), MENUBAR_H);

        let node_area_y = HEADER_H + MENUBAR_H;
        self.positions[BREADCRUMB_IDX] = (0.0, node_area_y, clw, BREADCRUMB_H);

        let active_nodes = self.current_dir().children.len().min(self.node_slots.len());
        for (slot_idx, (&idx, off)) in self.node_slots.iter().zip(self.left_offsets.iter()).enumerate() {
            if slot_idx < active_nodes {
                self.positions[idx] = (
                    off.0 * (self.grid_size_x + self.skipped_col_w) + self.pan_x,
                    node_area_y + BREADCRUMB_H + off.1 * (self.grid_size_y + self.skipped_row_h) + self.pan_y,
                    self.grid_size_x,
                    self.grid_size_y,
                );
            } else {
                self.positions[idx] = (0.0, 0.0, 0.0, 0.0);
            }
        }

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
            if menubar_idx == self.focused_pane {
                self.widgets[menubar_idx].focus();
            } else {
                self.widgets[menubar_idx].unfocus();
            }
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
        }
        if settings_changed {
            self.save_settings();
        }
    }

    fn read_panel_offsets(&mut self) {
        let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
        let active_nodes = self.current_dir().children.len().min(self.node_slots.len());
        let mut coords = Vec::with_capacity(active_nodes);
        for (&idx, off) in self.node_slots.iter().take(active_nodes).zip(self.left_offsets.iter_mut()) {
            let (px, py, _, _) = self.widgets[idx].rect();
            let c = ((px - self.pan_x) / (self.grid_size_x + self.skipped_col_w)).round();
            let r = ((py - node_area_y - self.pan_y) / (self.grid_size_y + self.skipped_row_h)).round();
            *off = (c, r);
            coords.push((c, r));
            if Some(idx) == self.focused_widget && self.drag_widget == Some(idx) {
                self.grid_cursor_col = c as i32;
                self.grid_cursor_row = r as i32;
            }
        }
        let dir = self.current_dir_mut();
        for (i, coord) in coords.into_iter().enumerate() {
            if let Some(child) = dir.children.get_mut(i) {
                child.position = coord;
            }
        }
    }

    fn sync_cursor_and_selection(&mut self) {
        if let Some(old) = self.focused_widget {
            if !self.node_slots.contains(&old) && old != SPREADSHEET_IDX {
                return;
            }
        }
        let active_nodes = self.current_dir().children.len().min(self.node_slots.len());
        let mut node_at_cursor = None;
        for (slot_idx, (&idx, off)) in self.node_slots.iter().zip(self.left_offsets.iter()).enumerate() {
            if slot_idx < active_nodes {
                if off.0 as i32 == self.grid_cursor_col && off.1 as i32 == self.grid_cursor_row {
                    node_at_cursor = Some(idx);
                    break;
                }
            }
        }

        if let Some(idx) = node_at_cursor {
            if self.focused_widget != Some(idx) {
                if let Some(old) = self.focused_widget {
                    self.widgets[old].unfocus();
                }
                self.widgets[idx].focus();
                self.focused_widget = Some(idx);
            }
        } else {
            if let Some(old) = self.focused_widget {
                if self.node_slots.contains(&old) {
                    self.widgets[old].unfocus();
                    self.focused_widget = None;
                }
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

        let clip = (0.0, node_area_y, self.content_left_w(), self.height - STATUS_H);

        for (i, w) in self.widgets.iter().enumerate() {
            if i == CONTENT_IDX {
                verts.extend(widget_vertices(w.as_ref(), sw, sh));
                for (qx, qy, qw, qh, qc) in w.extra_quads() {
                    verts.extend(quad_vertices_clipped(qx, qy, qw, qh, sw, sh, qc, clip));
                }
            } else if self.node_slots.contains(&i) {
                verts.extend(widget_vertices_clipped(w.as_ref(), sw, sh, clip));
                for (qx, qy, qw, qh, qc) in w.extra_quads() {
                    verts.extend(quad_vertices_clipped(qx, qy, qw, qh, sw, sh, qc, clip));
                }
            } else {
                verts.extend(widget_vertices(w.as_ref(), sw, sh));
                for (qx, qy, qw, qh, qc) in w.extra_quads() {
                    verts.extend(quad_vertices(qx, qy, qw, qh, sw, sh, qc));
                }
            }

            if i == CONTENT_IDX && show_cursor {
                let cx = self.grid_cursor_col as f32 * (self.grid_size_x + self.skipped_col_w) + self.pan_x;
                let cy = node_area_y + self.grid_cursor_row as f32 * (self.grid_size_y + self.skipped_row_h) + self.pan_y;
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
                ));

                // 4 border edges
                verts.extend(quad_vertices_clipped(cx, cy, cw, thickness, sw, sh, border_color, clip));
                verts.extend(quad_vertices_clipped(cx, cy + ch - thickness, cw, thickness, sw, sh, border_color, clip));
                verts.extend(quad_vertices_clipped(cx, cy + thickness, thickness, ch - 2.0 * thickness, sw, sh, border_color, clip));
                verts.extend(quad_vertices_clipped(cx + cw - thickness, cy + thickness, thickness, ch - 2.0 * thickness, sw, sh, border_color, clip));
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
            ref widgets, ref node_slots, ..
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
            let is_node = node_slots.contains(&i);
            for label in w.text_labels() {
                widget_buffers.push(make_text_buffer(font_system, &label.text, label.font_size));
                widget_labels.push(label);
                widget_is_node.push(is_node);
            }
        }

        for ((buf, label), &is_node) in widget_buffers.iter().zip(widget_labels.iter()).zip(widget_is_node.iter()) {
            let bounds = if is_node {
                let node_area_y = HEADER_H + MENUBAR_H + BREADCRUMB_H;
                TextBounds {
                    left: 0,
                    top: (node_area_y * s) as i32,
                    right: (splitter1_x * s) as i32,
                    bottom: (((height - STATUS_H) * s) as i32).max(0),
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
                    if let Some(focused) = self.focused_widget {
                        if self.node_slots.contains(&focused) {
                            if let Some(slot_idx) = self.node_slots.iter().position(|&x| x == focused) {
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
                    }

                    // Sync Parameters pane with selected node
                    let params = self.focused_widget.and_then(|f| {
                        if self.node_slots.contains(&f) { Some(f) } else { None }
                    }).map(|f| self.widgets[f].node_params()).unwrap_or_default();
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
                self.cursor_x = position.x as f32 / self.scale as f32;
                self.cursor_y = position.y as f32 / self.scale as f32;
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
                        if self.widgets[idx].drag_update(self.cursor_x, self.cursor_y) {
                            changed = true;
                        }
                    }

                    if self.drag_widget.is_none() {
                        for w in &mut self.widgets {
                            if w.cursor_moved(self.cursor_x, self.cursor_y) {
                                changed = true;
                            }
                        }
                    }
                }
                changed
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
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
                        || *button == MouseButton::Right
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

                if *button != MouseButton::Left { return false; }
                let mut changed = false;
                let old_focus = self.focused_widget;

                match btn_state {
                    ElementState::Pressed => {
                        let click_target = (0..self.widgets.len()).rev()
                            .find(|&i| self.widgets[i].hit_test(self.cursor_x, self.cursor_y));

                        // Determine new focused pane
                        let mut new_pane = None;
                        if let Some(i) = click_target {
                            if i == LEFT_MENUBAR_IDX || i == CONTENT_IDX || i == CANVAS_IDX || (i >= NODE_SLOT_START && i < NODE_SLOT_START + NODE_SLOT_COUNT) || i == BREADCRUMB_IDX {
                                new_pane = Some(LEFT_MENUBAR_IDX);
                            } else if i == RIGHT_MENUBAR_IDX || i == VIEWPORT_IDX {
                                new_pane = Some(RIGHT_MENUBAR_IDX);
                            } else if i == PARAM_MENUBAR_IDX || i == PARAM_IDX {
                                new_pane = Some(PARAM_MENUBAR_IDX);
                            } else if i == SPREADSHEET_MENUBAR_IDX || i == SPREADSHEET_IDX {
                                new_pane = Some(SPREADSHEET_MENUBAR_IDX);
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
                        if click_target.is_none() && !dialog_open && in_network_pane {
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
                            if self.node_slots.contains(&i) {
                                if let Some(slot_idx) = self.node_slots.iter().position(|&x| x == i) {
                                    let off = self.left_offsets[slot_idx];
                                    self.grid_cursor_col = off.0 as i32;
                                    self.grid_cursor_row = off.1 as i32;
                                }
                            }
                            // Double-click detection for node directory entry
                            if self.node_slots.contains(&i) {
                                if let Some((t, prev)) = self.last_click {
                                    if prev == i && t.elapsed() < std::time::Duration::from_millis(500) {
                                        let dir_idx = self.node_slots.iter().position(|&x| x == i).unwrap();
                                        let dir = self.current_dir();
                                        if dir_idx < dir.children.len() && !dir.children[dir_idx].children.is_empty() {
                                            self.current_path.push(dir_idx);
                                            self.on_path_changed();
                                            changed = true;
                                        }
                                    }
                                }
                                self.last_click = Some((Instant::now(), i));
                            } else {
                                self.last_click = None;
                            }
                        }
                        self.sync_pane_focus();
                    }
                    ElementState::Released => {
                        if self.drag_widget.is_some() {
                            let idx = self.drag_widget.unwrap();
                            if self.node_slots.contains(&idx) {
                                if let Some(slot_idx) = self.node_slots.iter().position(|&x| x == idx) {
                                    let pos = self.current_dir().children[slot_idx].position;
                                    let (nx, ny) = self.find_empty_cell(pos.0, pos.1, Some(slot_idx));
                                    self.current_dir_mut().children[slot_idx].position = (nx, ny);
                                    self.left_offsets[slot_idx] = (nx, ny);
                                    self.rebuild_positions();
                                    self.apply_layout();
                                    self.update_panel_bounds();
                                    self.upload_vertices();
                                }
                            }
                            self.widgets[idx].drag_end();
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

                let active_nodes = self.current_dir().children.len().min(self.node_slots.len());
                let mut visibility_updates = Vec::new();
                for (i, &slot) in self.node_slots.iter().enumerate().take(active_nodes) {
                    if self.widgets[slot].take_geom_toggle() {
                        let visible = self.widgets[slot].geom_visible();
                        visibility_updates.push((i, visible));
                    }
                }
                for (i, visible) in visibility_updates {
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
                    if let Some(focused) = self.focused_widget {
                        if self.node_slots.contains(&focused) {
                            if let Some(slot_idx) = self.node_slots.iter().position(|&x| x == focused) {
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
                                    "k" | "K" if is_plain_key || is_alt_key => {
                                        delta = Some((0, -1));
                                    }
                                    "j" | "J" if is_plain_key || is_alt_key => {
                                        delta = Some((0, 1));
                                    }
                                    "h" | "H" if is_plain_key || is_alt_key => {
                                        delta = Some((-1, 0));
                                    }
                                    "l" | "L" if is_plain_key || is_alt_key => {
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
                                let active_nodes = self.current_dir().children.len().min(self.node_slots.len());
                                let node_idx_at_cursor = self.current_dir().children.iter().take(active_nodes).position(|child| {
                                    child.position.0 as i32 == self.grid_cursor_col && child.position.1 as i32 == self.grid_cursor_row
                                });
                                if let Some(idx) = node_idx_at_cursor {
                                    let new_x = self.current_dir().children[idx].position.0 + dc as f32;
                                    let new_y = self.current_dir().children[idx].position.1 + dr as f32;
                                    self.current_dir_mut().children[idx].position = (new_x, new_y);
                                    self.left_offsets[idx] = (new_x, new_y);
                                    self.sync_nodes();
                                    self.sync_layout();
                                    self.upload_vertices();
                                }
                            }
                            self.grid_cursor_col += dc;
                            self.grid_cursor_row += dr;
                            changed = true;
                        } else {
                            if let Key::Character(s) = &event.logical_key {
                                if is_plain_key {
                                    match s.as_str() {
                                        "e" | "E" => {
                                            if let Some(idx) = self.focused_widget {
                                                if let Some(slot_idx) = self.node_slots.iter().position(|&x| x == idx) {
                                                    let active_nodes = self.current_dir().children.len().min(self.node_slots.len());
                                                    if slot_idx < active_nodes {
                                                        let visible = !self.widgets[idx].geom_visible();
                                                        self.widgets[idx].set_geom_visible(visible);
                                                        self.current_dir_mut().children[slot_idx].geometry_visible = visible;
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
                                                if let Some(focused) = self.focused_widget {
                                                    if let Some(slot_idx) = self.node_slots.iter().position(|&x| x == focused) {
                                                        let dir = self.current_dir();
                                                        if slot_idx < dir.children.len() && !dir.children[slot_idx].children.is_empty() {
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
            self.pan_x += self.pan_velocity_x * dt;
            self.pan_y += self.pan_velocity_y * dt;

            // Apply friction decay
            let friction = 5.0_f32;
            let decay = (-friction * dt).exp();
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
            if self.node_slots.contains(&idx) {
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

struct AppState {
    registry_state: RegistryState,
    compositor_state: CompositorState,
    xdg_shell_state: XdgShell,
    shm_state: Shm,
    seat_state: SeatState,
    output_state: OutputState,

    seats: Vec<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,

    window: Option<XdgWindow>,
    surface: Option<wl_surface::WlSurface>,

    state: Option<State>,
    exit: bool,
    redraw: bool,
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
            let pointer = self.seat_state.get_pointer(qh, &seat).unwrap();
            self.pointer = Some(pointer);
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
                match &event.kind {
                    PointerEventKind::Motion { .. } => {
                        let ev = WindowEvent::CursorMoved {
                            position: LocalPosition {
                                x: cx as f64,
                                y: cy as f64,
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
    ) {}

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
        _modifiers: smithay_client_toolkit::seat::keyboard::Modifiers,
        _layout: u32,
    ) {}
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
            _ => {
                if let Some(ref text) = event.utf8 {
                    Key::Character(text.clone())
                } else {
                    return;
                }
            }
        };

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
            };

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
                        for &i in &state.node_slots {
                            let gx = if val { state.grid_size_x + state.skipped_col_w } else { 0.0 };
                            let gy = if val { state.grid_size_y + state.skipped_row_h } else { 0.0 };
                            state.widgets[i].set_grid_snap(gx, gy);
                        }
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
                    _ => {}
                }
                for &i in &state.node_slots {
                    let (x, y, _, _) = state.widgets[i].rect();
                    state.widgets[i].set_rect(x, y, state.grid_size_x, state.grid_size_y);
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
                    if let Some(focused) = state.focused_widget {
                        if state.node_slots.contains(&focused) {
                            if let Some(slot_idx) = state.node_slots.iter().position(|&x| x == focused) {
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
                }

                state.sync_nodes();

                // Sync Parameters pane with selected node
                let params = state.focused_widget.and_then(|f| {
                    if state.node_slots.contains(&f) { Some(f) } else { None }
                }).map(|f| state.widgets[f].node_params()).unwrap_or_default();
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
                            if slot < dir.children.len() && !dir.children[slot].children.is_empty() {
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
                            let active_nodes = state.current_dir().children.len().min(state.node_slots.len());
                            if slot < active_nodes {
                                let idx = state.node_slots[slot];
                                let visible = !state.widgets[idx].geom_visible();
                                state.widgets[idx].set_geom_visible(visible);
                                state.current_dir_mut().children[slot].geometry_visible = visible;
                                state.rebuild_scene_geometry();
                                needs_redraw = true;
                                Ok(format!("Geometry visible: {}", visible))
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::AddNode { template_name, name, x, y } => {
                            let child_idx = state.current_dir().children.len();
                            if child_idx >= state.node_slots.len() {
                                Err("Max node slots reached".to_string())
                            } else {
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
                                    state.left_offsets[child_idx] = (nx, ny);
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
                        }
                        HttpAction::DeleteNode { slot } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                state.current_dir_mut().children.remove(slot);
                                if let Some(focused) = state.focused_widget {
                                    if let Some(pos) = state.node_slots.iter().position(|&x| x == focused) {
                                        if pos >= slot {
                                            state.widgets[focused].unfocus();
                                            state.focused_widget = None;
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
                                if slot < state.left_offsets.len() {
                                    state.left_offsets[slot] = (nx, ny);
                                }
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

        if app.redraw {
            app.redraw = false;
            if let Some(state) = &mut app.state {
                if state.render() {
                    app.redraw = true;
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
}

