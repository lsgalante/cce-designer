use std::sync::Arc;
use std::time::Instant;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes};

use wgpu::util::DeviceExt;

use clear_ui::widget::{Breadcrumb, Canvas, ContentBg, MenuBar, Node, ParametersBg, Splitter, Spreadsheet, StatusBar, TextLabel, ViewportBg, Widget};
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

struct ConfigDialog {
    x: f32, y: f32, w: f32, h: f32,
    hovered: bool,
    visible: bool,
    close_hovered: bool,
    panel_w: f32,
    panel_h: f32,
    active_page: usize,
    hovered_tab: Option<usize>,
    grid_snap_enabled: bool,
    grid_snap_pending: Option<bool>,
    grid_snap_hovered: bool,
    network_grid_enabled: bool,
    network_grid_pending: Option<bool>,
    network_grid_hovered: bool,
    grid_size_x: f32,
    grid_size_y: f32,
    skipped_row_h: f32,
    skipped_col_w: f32,
    grid_size_pending: Option<(usize, f32)>,
    hovered_spin_btn: Option<usize>,
}

impl ConfigDialog {
    fn new() -> Self {
        Self {
            x: 0.0, y: 0.0, w: 0.0, h: 0.0,
            hovered: false, visible: false,
            close_hovered: false,
            panel_w: 400.0, panel_h: 300.0,
            active_page: 0,
            hovered_tab: None,
            grid_snap_enabled: true,
            grid_snap_pending: None,
            grid_snap_hovered: false,
            network_grid_enabled: true,
            network_grid_pending: None,
            network_grid_hovered: false,
            grid_size_x: 80.0,
            grid_size_y: 40.0,
            skipped_row_h: 20.0,
            skipped_col_w: 20.0,
            grid_size_pending: None,
            hovered_spin_btn: None,
        }
    }

    fn panel_rect(&self) -> (f32, f32, f32, f32) {
        let px = self.x + (self.w - self.panel_w) / 2.0;
        let py = self.y + (self.h - self.panel_h) / 2.0;
        (px, py, self.panel_w, self.panel_h)
    }

    fn close_rect(&self, px: f32, pw: f32, py: f32) -> (f32, f32, f32, f32) {
        (px + pw - 28.0, py + 8.0, 20.0, 20.0)
    }

    fn spin_btns(&self, px: f32, py: f32) -> Vec<(usize, f32, f32)> {
        let row0_y = py + 156.0; // Grid X
        let row1_y = py + 178.0; // Grid Y
        let row2_y = py + 200.0; // Skipped Row H
        let row3_y = py + 222.0; // Skipped Col W
        vec![
            (0, px + 160.0, row0_y - 2.0),
            (1, px + 210.0, row0_y - 2.0),
            (2, px + 160.0, row1_y - 2.0),
            (3, px + 210.0, row1_y - 2.0),
            (4, px + 160.0, row2_y - 2.0),
            (5, px + 210.0, row2_y - 2.0),
            (6, px + 160.0, row3_y - 2.0),
            (7, px + 210.0, row3_y - 2.0),
        ]
    }

    fn tab_rects(&self, px: f32, py: f32) -> [(f32, f32, f32, f32); 3] {
        let tab_y = py + 30.0;
        let tab_h = 22.0;
        let general_w = "General".len() as f32 * 7.5 + 16.0;
        let network_w = "Network".len() as f32 * 7.5 + 16.0;
        let bindings_w = "Bindings".len() as f32 * 7.5 + 16.0;
        let gap = 4.0;
        [
            (px + 16.0, tab_y, general_w, tab_h),
            (px + 16.0 + general_w + gap, tab_y, network_w, tab_h),
            (px + 16.0 + general_w + gap + network_w + gap, tab_y, bindings_w, tab_h),
        ]
    }
}

impl Widget for ConfigDialog {
    fn rect(&self) -> (f32, f32, f32, f32) { (self.x, self.y, self.w, self.h) }
    fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) { self.x = x; self.y = y; self.w = w; self.h = h; }
    fn color(&self) -> [f32; 4] { [0.0, 0.0, 0.0, 0.0] }
    fn set_hovered(&mut self, v: bool) { self.hovered = v; }
    fn hovered(&self) -> bool { self.hovered }

    fn set_visible(&mut self, v: bool) { self.visible = v; }
    fn visible(&self) -> bool { self.visible }

    fn set_config_toggle(&mut self, id: usize, val: bool) {
        match id {
            0 => self.grid_snap_enabled = val,
            1 => self.network_grid_enabled = val,
            _ => {}
        }
    }
    fn take_config_toggle(&mut self) -> Option<(usize, bool)> {
        if let Some(v) = self.grid_snap_pending.take() {
            Some((0, v))
        } else if let Some(v) = self.network_grid_pending.take() {
            Some((1, v))
        } else {
            None
        }
    }

    fn set_config_spin(&mut self, id: usize, val: f32) {
        match id {
            0 => self.grid_size_x = val,
            1 => self.grid_size_y = val,
            2 => self.skipped_row_h = val,
            3 => self.skipped_col_w = val,
            _ => {}
        }
    }
    fn take_config_spin(&mut self) -> Option<(usize, f32)> {
        self.grid_size_pending.take()
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

        let old_tg = self.grid_snap_hovered;
        let old_ng = self.network_grid_hovered;
        let old_sb = self.hovered_spin_btn;
        self.grid_snap_hovered = false;
        self.network_grid_hovered = false;
        self.hovered_spin_btn = None;
        if self.visible && self.active_page == 1 {
            let row1_y = ppy + 84.0;
            self.grid_snap_hovered = px >= ppx + 16.0 && px < ppx + 200.0
                && py >= row1_y - 2.0 && py < row1_y + 18.0;
            let row2_y = ppy + 108.0;
            self.network_grid_hovered = px >= ppx + 16.0 && px < ppx + 200.0
                && py >= row2_y - 2.0 && py < row2_y + 18.0;
            for &(btn_id, bx, by) in &self.spin_btns(ppx, ppy) {
                if px >= bx && px < bx + 22.0 && py >= by && py < by + 20.0 {
                    self.hovered_spin_btn = Some(btn_id);
                    break;
                }
            }
        }

        was != self.hovered || old_close != self.close_hovered || old_tab != self.hovered_tab
            || old_tg != self.grid_snap_hovered || old_ng != self.network_grid_hovered
            || old_sb != self.hovered_spin_btn
    }

    fn mouse_input(&mut self, button: MouseButton, state: ElementState, px: f32, py: f32) -> bool {
        if !self.visible || button != MouseButton::Left || state != ElementState::Pressed { return false; }
        let (ppx, ppy, pw, ph) = self.panel_rect();
        let in_panel = px >= ppx && px <= ppx + pw && py >= ppy && py <= ppy + ph;
        let (cx, cy, cw, ch) = self.close_rect(ppx, pw, ppy);
        let on_close = px >= cx && px < cx + cw && py >= cy && py < cy + ch;
        if !in_panel || on_close {
            self.visible = false;
            return true;
        }
        for (i, (tx, ty, tw, th)) in self.tab_rects(ppx, ppy).iter().enumerate() {
            if px >= *tx && px < *tx + *tw && py >= *ty && py < *ty + *th {
                self.active_page = i;
                return true;
            }
        }
        if self.active_page == 1 {
            let row1_y = ppy + 84.0;
            if px >= ppx + 16.0 && px < ppx + 200.0
                && py >= row1_y - 2.0 && py < row1_y + 18.0
            {
                self.grid_snap_enabled = !self.grid_snap_enabled;
                self.grid_snap_pending = Some(self.grid_snap_enabled);
                return true;
            }
            let row2_y = ppy + 108.0;
            if px >= ppx + 16.0 && px < ppx + 200.0
                && py >= row2_y - 2.0 && py < row2_y + 18.0
            {
                self.network_grid_enabled = !self.network_grid_enabled;
                self.network_grid_pending = Some(self.network_grid_enabled);
                return true;
            }
            for &(btn_id, bx, by) in &self.spin_btns(ppx, ppy) {
                if px >= bx && px < bx + 22.0 && py >= by && py < by + 20.0 {
                    let (val, delta, min_val, max_val, pending_id) = match btn_id {
                        0 => (&mut self.grid_size_x, -5.0, 10.0, 200.0, 0),
                        1 => (&mut self.grid_size_x, 5.0, 10.0, 200.0, 0),
                        2 => (&mut self.grid_size_y, -5.0, 5.0, 100.0, 1),
                        3 => (&mut self.grid_size_y, 5.0, 5.0, 100.0, 1),
                        4 => (&mut self.skipped_row_h, -5.0, 0.0, 150.0, 2),
                        5 => (&mut self.skipped_row_h, 5.0, 0.0, 150.0, 2),
                        6 => (&mut self.skipped_col_w, -5.0, 0.0, 150.0, 3),
                        7 => (&mut self.skipped_col_w, 5.0, 0.0, 150.0, 3),
                        _ => unreachable!(),
                    };
                    let new = (*val + delta).clamp(min_val, max_val);
                    if (new - *val).abs() > 0.01 {
                        *val = new;
                        self.grid_size_pending = Some((pending_id, *val));
                    }
                    return true;
                }
            }
        }
        false
    }

    fn keyboard_input(&mut self, event: &KeyEvent) -> bool {
        if !self.visible { return false; }
        if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) {
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

        // Toggle and spin hover highlights on Network page
        if self.visible && self.active_page == 1 {
            if self.grid_snap_hovered {
                let row1_y = py + 84.0;
                quads.push((px + 14.0, row1_y - 2.0, 186.0, 20.0, [0.25, 0.25, 0.35, 0.4]));
            }
            if self.network_grid_hovered {
                let row2_y = py + 108.0;
                quads.push((px + 14.0, row2_y - 2.0, 186.0, 20.0, [0.25, 0.25, 0.35, 0.4]));
            }
            if let Some(sb) = self.hovered_spin_btn {
                for &(btn_id, bx, by) in &self.spin_btns(px, py) {
                    if btn_id == sb {
                        quads.push((bx, by, 22.0, 20.0, [0.35, 0.35, 0.45, 0.5]));
                        break;
                    }
                }
            }
        }

        quads
    }

    fn text_labels(&self) -> Vec<TextLabel> {
        if !self.visible { return vec![]; }
        let (px, py, pw, _) = self.panel_rect();
        let mut labels = Vec::new();

        // Title and close button
        labels.push(TextLabel { text: "Configure Clear Designer".into(), x: px + 16.0, y: py + 10.0, font_size: 14.0, color: [0xcc, 0xcc, 0xd4] });
        labels.push(TextLabel { text: "\u{2715}".into(), x: px + pw - 22.0, y: py + 10.0, font_size: 14.0, color: [0xaa, 0xaa, 0xbb] });

        // Tab labels
        let tabs = self.tab_rects(px, py);
        let tab_labels = ["General", "Network", "Bindings"];
        for (i, &(tx, ty, _, _)) in tabs.iter().enumerate() {
            let color = if i == self.active_page { [0xcc, 0xcc, 0xd4] } else { [0x88, 0x88, 0x99] };
            labels.push(TextLabel {
                text: tab_labels[i].into(),
                x: tx + 8.0,
                y: ty + 4.0,
                font_size: 12.0,
                color,
            });
        }

        // Page content
        let content_y = py + 60.0;
        if self.active_page == 0 {
            labels.push(TextLabel { text: "General settings for Clear Designer".into(), x: px + 16.0, y: content_y, font_size: 12.0, color: [0x88, 0x88, 0x99] });
        } else if self.active_page == 1 {
            labels.push(TextLabel { text: "Network Configuration".into(), x: px + 16.0, y: content_y, font_size: 13.0, color: [0xcc, 0xcc, 0xd4] });
            let snap_text = if self.grid_snap_enabled { "[\u{2713}] Snap to Grid" } else { "[ ] Snap to Grid" };
            labels.push(TextLabel { text: snap_text.into(), x: px + 20.0, y: content_y + 26.0, font_size: 12.0, color: [0xaa, 0xaa, 0xbb] });
            let grid_text = if self.network_grid_enabled { "[\u{2713}] Show Grid" } else { "[ ] Show Grid" };
            labels.push(TextLabel { text: grid_text.into(), x: px + 20.0, y: content_y + 50.0, font_size: 12.0, color: [0xaa, 0xaa, 0xbb] });
            labels.push(TextLabel { text: "Grid Spacing".into(), x: px + 16.0, y: content_y + 78.0, font_size: 12.0, color: [0x88, 0x88, 0x99] });
            let gx = self.grid_size_x as i32;
            labels.push(TextLabel { text: "Grid X:".into(), x: px + 16.0, y: content_y + 100.0, font_size: 12.0, color: [0xbb, 0xbb, 0xcc] });
            labels.push(TextLabel { text: "\u{2212}".into(), x: px + 170.0, y: content_y + 100.0, font_size: 12.0, color: [0xcc, 0xcc, 0xd4] });
            labels.push(TextLabel { text: format!("{}", gx), x: px + 188.0, y: content_y + 100.0, font_size: 12.0, color: [0xdd, 0xdd, 0x88] });
            labels.push(TextLabel { text: "+".into(), x: px + 220.0, y: content_y + 100.0, font_size: 12.0, color: [0xcc, 0xcc, 0xd4] });
            let gy = self.grid_size_y as i32;
            labels.push(TextLabel { text: "Grid Y:".into(), x: px + 16.0, y: content_y + 122.0, font_size: 12.0, color: [0xbb, 0xbb, 0xcc] });
            labels.push(TextLabel { text: "\u{2212}".into(), x: px + 170.0, y: content_y + 122.0, font_size: 12.0, color: [0xcc, 0xcc, 0xd4] });
            labels.push(TextLabel { text: format!("{}", gy), x: px + 188.0, y: content_y + 122.0, font_size: 12.0, color: [0xdd, 0xdd, 0x88] });
            labels.push(TextLabel { text: "+".into(), x: px + 220.0, y: content_y + 122.0, font_size: 12.0, color: [0xcc, 0xcc, 0xd4] });

            let srh = self.skipped_row_h as i32;
            labels.push(TextLabel { text: "Skipped Row H:".into(), x: px + 16.0, y: content_y + 144.0, font_size: 12.0, color: [0xbb, 0xbb, 0xcc] });
            labels.push(TextLabel { text: "\u{2212}".into(), x: px + 170.0, y: content_y + 144.0, font_size: 12.0, color: [0xcc, 0xcc, 0xd4] });
            labels.push(TextLabel { text: format!("{}", srh), x: px + 188.0, y: content_y + 144.0, font_size: 12.0, color: [0xdd, 0xdd, 0x88] });
            labels.push(TextLabel { text: "+".into(), x: px + 220.0, y: content_y + 144.0, font_size: 12.0, color: [0xcc, 0xcc, 0xd4] });

            let scw = self.skipped_col_w as i32;
            labels.push(TextLabel { text: "Skipped Col W:".into(), x: px + 16.0, y: content_y + 166.0, font_size: 12.0, color: [0xbb, 0xbb, 0xcc] });
            labels.push(TextLabel { text: "\u{2212}".into(), x: px + 170.0, y: content_y + 166.0, font_size: 12.0, color: [0xcc, 0xcc, 0xd4] });
            labels.push(TextLabel { text: format!("{}", scw), x: px + 188.0, y: content_y + 166.0, font_size: 12.0, color: [0xdd, 0xdd, 0x88] });
            labels.push(TextLabel { text: "+".into(), x: px + 220.0, y: content_y + 166.0, font_size: 12.0, color: [0xcc, 0xcc, 0xd4] });
        } else {
            labels.push(TextLabel { text: "Keyboard Shortcuts".into(), x: px + 16.0, y: content_y, font_size: 13.0, color: [0xcc, 0xcc, 0xd4] });
            let shortcuts = [
                ("Ctrl+G", "Toggle Grid (viewport)"),
                ("Ctrl+E", "Toggle Cube"),
                ("Ctrl+A", "Square Viewport Aspect"),
                ("Ctrl+,", "Configure Dialog"),
            ];
            for (i, (key, desc)) in shortcuts.iter().enumerate() {
                let y = content_y + 28.0 + i as f32 * 22.0;
                labels.push(TextLabel { text: (*key).into(), x: px + 32.0, y, font_size: 12.0, color: [0xdd, 0xdd, 0x88] });
                labels.push(TextLabel { text: (*desc).into(), x: px + 130.0, y, font_size: 12.0, color: [0xaa, 0xaa, 0xbb] });
            }
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

fn node_param_f32(node: &FsNode, name: &str, fallback: f32) -> f32 {
    node.params.iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .and_then(|p| p.default.parse::<f32>().ok())
        .unwrap_or(fallback)
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
        if node.node_type.eq_ignore_ascii_case("sphere") {
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

fn grid_vertices() -> Vec<Vertex3D> {
    let half_w = 0.015;
    let color = [0.35, 0.35, 0.40];
    let range = 4.0;
    let step = 1.0;
    let y = 0.0;
    let mut verts = Vec::new();

    let mut z = -range;
    while z <= range {
        let z0 = z - half_w;
        let z1 = z + half_w;
        verts.push(Vertex3D { position: [-range, y, z0], color });
        verts.push(Vertex3D { position: [range, y, z0], color });
        verts.push(Vertex3D { position: [-range, y, z1], color });
        verts.push(Vertex3D { position: [range, y, z0], color });
        verts.push(Vertex3D { position: [range, y, z1], color });
        verts.push(Vertex3D { position: [-range, y, z1], color });
        z += step;
    }

    let mut x = -range;
    while x <= range {
        let x0 = x - half_w;
        let x1 = x + half_w;
        verts.push(Vertex3D { position: [x0, y, -range], color });
        verts.push(Vertex3D { position: [x1, y, -range], color });
        verts.push(Vertex3D { position: [x0, y, range], color });
        verts.push(Vertex3D { position: [x1, y, -range], color });
        verts.push(Vertex3D { position: [x1, y, range], color });
        verts.push(Vertex3D { position: [x0, y, range], color });
        x += step;
    }

    verts
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

struct State {
    window: Arc<Window>,
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
    vertex_buffer_3d: wgpu::Buffer,
    vertex_count_3d: u32,
    vertex_buffer_spheres: wgpu::Buffer,
    vertex_count_spheres: u32,
    vertex_buffer_grid: wgpu::Buffer,
    vertex_count_grid: u32,
    depth_texture: wgpu::Texture,
    depth_texture_view: wgpu::TextureView,
    rotation: f32,
    show_grid: bool,
    show_cube: bool,

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
}

impl State {
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

    fn next_node_offset(&self, _idx: usize) -> (f32, f32) {
        (self.grid_cursor_col as f32, self.grid_cursor_row as f32)
    }

    fn place_selected_node(&mut self) -> bool {
        let Some(&template_idx) = self.node_palette_filtered.get(self.node_palette_selected) else { return false; };
        let child_idx = self.current_dir().children.len();
        if child_idx >= self.node_slots.len() { return false; }
        let mut node = self.node_templates[template_idx].node.clone();
        node.position = (self.grid_cursor_col as f32, self.grid_cursor_row as f32);
        self.current_dir_mut().children.push(node);
        self.left_offsets[child_idx] = self.next_node_offset(child_idx);
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
        "Pos X".to_string(),
        "Pos Y".to_string(),
        "Pos Z".to_string(),
        "Col R".to_string(),
        "Col G".to_string(),
        "Col B".to_string(),
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

    async fn new(window: Arc<Window>) -> Self {
        let scale = window.scale_factor();
        let physical_size = window.inner_size();
        let pw = physical_size.width.max(1);
        let ph = physical_size.height.max(1);
        let lw = pw as f32 / scale as f32;
        let lh = ph as f32 / scale as f32;
        let sw = lw;

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
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

        let grid_verts = grid_vertices();
        let vertex_count_grid = grid_verts.len() as u32;
        let vertex_buffer_grid = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Grid Vertex Buffer"),
            contents: bytemuck::cast_slice(&grid_verts),
            usage: wgpu::BufferUsages::VERTEX,
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
            Box::new(MenuBar::new(0.0, 0.0, 0.0, HEADER_H).with_title("Clear Designer").with_item("File", &["New Project", "Open", "Save", "Configure", "Exit"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out", "Reset Zoom"]).with_item("Help", &["About"])),
            Box::new(ContentBg::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(ViewportBg::new()),
            Box::new(Splitter::new(SPLITTER_W)),
            Box::new(ParametersBg::new()),
            Box::new(Canvas::new()),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("Network").with_item("File", &["New", "Open", "Save"]).with_item("Edit", &["Undo", "Redo"]).with_item("View", &["Zoom In", "Zoom Out"])),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("Viewport").with_item("Camera", &["Perspective", "Orthographic"]).with_item("Display", &["Square Aspect"]).with_item("Guides", &["Show Grid", "Cube"])),
            Box::new(MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("Parameters").with_item("Preset", &["Default", "Custom"]).with_item("Reset", &["All"])),
        ];
        for _ in 0..NODE_SLOT_COUNT {
            widgets.push(Box::new(Node::new(0.0, 0.0, 0.0, 0.0, "")));
        }
        widgets.push(Box::new(StatusBar::new()));
        widgets.push(Box::new(Breadcrumb::new()));
        widgets.push(Box::new(ConfigDialog::new()));
        widgets.push(Box::new(NodePalette::new()));
        widgets.push(Box::new(Spreadsheet::new()));
        
        let mut spreadsheet_menubar = MenuBar::new(0.0, 0.0, 0.0, MENUBAR_H).with_title("spreadsheed");
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
            vertex_buffer_3d,
            vertex_count_3d: cube_vertices().len() as u32,
            vertex_buffer_spheres,
            vertex_count_spheres: 0,
            vertex_buffer_grid,
            vertex_count_grid,
            depth_texture,
            depth_texture_view,
            rotation: 0.0,
            show_grid: true,
            show_cube: false,
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
            square_viewport: false,
            grid_snap_enabled: true,
            network_grid_visible: true,
            grid_size_x: 80.0,
            grid_size_y: 40.0,
            skipped_row_h: 20.0,
            skipped_col_w: 20.0,

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
        };

        state.sync_nodes();
        state.rebuild_scene_geometry();
        state.sync_grid_settings();
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 0, state.show_grid);
        state.widgets[RIGHT_MENUBAR_IDX].set_item_checked(2, 1, state.show_cube);

        state.rebuild_positions();
        state.apply_layout();
        state.update_panel_bounds();
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

    fn sync_layout(&mut self) {
        self.splitter1_x = self.widgets[SPLITTER1_IDX].rect().0;
        self.splitter2_x = self.widgets[SPLITTER2_IDX].rect().0;
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
    }

    fn execute_action(&mut self, action: Action) {
        match action {
            Action::ToggleGrid => self.show_grid = !self.show_grid,
            Action::ToggleCube => self.show_cube = !self.show_cube,
            Action::ToggleSquareViewport => self.square_viewport = !self.square_viewport,
            Action::ToggleConfigure => {
                if self.widgets[CONFIG_DIALOG_IDX].take_click() {
                    self.widgets[CONFIG_DIALOG_IDX].set_visible(false);
                } else {
                    self.widgets[CONFIG_DIALOG_IDX].set_config_toggle(0, self.grid_snap_enabled);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_toggle(1, self.network_grid_visible);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(0, self.grid_size_x);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(1, self.grid_size_y);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(2, self.skipped_row_h);
                    self.widgets[CONFIG_DIALOG_IDX].set_config_spin(3, self.skipped_col_w);
                    self.widgets[CONFIG_DIALOG_IDX].set_visible(true);
                }
                self.upload_vertices();
                return;
            }
            Action::ToggleSpreadsheet => {
                self.show_spreadsheet = !self.show_spreadsheet;
                self.widgets[SPREADSHEET_IDX].set_visible(self.show_spreadsheet);
                self.widgets[SPREADSHEET_MENUBAR_IDX].set_visible(self.show_spreadsheet);
                self.rebuild_positions();
                self.apply_layout();
                self.sync_nodes();
            }
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
        let geom = network_sphere_vertices(&self.fs_root);
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

    fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.physical_width = new_size.width;
            self.physical_height = new_size.height;
            self.width = new_size.width as f32 / self.scale as f32;
            self.height = new_size.height as f32 / self.scale as f32;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
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

                let mut handled = false;
                if !dialog_open {
                    for w in &mut self.widgets {
                        if w.mouse_wheel(delta, self.cursor_x, self.cursor_y) {
                            handled = true;
                        }
                    }
                }

                if handled {
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
                            winit::event::MouseScrollDelta::LineDelta(_x, y) => {
                                if *y > 0.0 { 1.15 } else if *y < 0.0 { 1.0 / 1.15 } else { 1.0 }
                            }
                            winit::event::MouseScrollDelta::PixelDelta(pos) => {
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
                            winit::event::MouseScrollDelta::LineDelta(x, y) => {
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
                            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                                let dx = pos.x as f32 / self.scale as f32;
                                let dy = pos.y as f32 / self.scale as f32;
                                self.pan_x -= dx;
                                self.pan_y -= dy;
                                self.is_scrolling_trackpad = match phase {
                                    winit::event::TouchPhase::Started | winit::event::TouchPhase::Moved => true,
                                    winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => false,
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
                                            changed = true;
                                        }
                                    }
                                }
                                self.last_click = Some((Instant::now(), i));
                            } else {
                                self.last_click = None;
                            }
                        }
                    }
                    ElementState::Released => {
                        if self.drag_widget.is_some() {
                            let idx = self.drag_widget.unwrap();
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
            _ => false,
        }
    }

    fn render(&mut self) {
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
            self.window.request_redraw();
        }

        self.prepare_text();

        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(wgpu::SurfaceError::Timeout) => return,
            Err(e) => { eprintln!("Surface error: {e:?}"); return; }
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

                self.rotation += 0.015;
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
                let view_mat = Mat4::look_at_rh(camera_pos, Vec3::ZERO, Vec3::Y);
                let model = Mat4::from_rotation_y(self.rotation) * Mat4::from_rotation_x(self.rotation * 0.2);
                let mvp = proj * view_mat * model;
                self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[mvp.to_cols_array_2d()]));

                let mvp_grid = proj * view_mat;
                self.queue.write_buffer(&self.uniform_buffer_grid, 0, bytemuck::cast_slice(&[mvp_grid.to_cols_array_2d()]));

                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("3D Render Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.05, b: 0.10, a: 1.0 }),
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
        self.window.pre_present_notify();
        output.present();
    }
}

struct App { state: Option<State> }

impl App { fn new() -> Self { Self { state: None } } }

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() { return; }

        let window = Arc::new(
            event_loop.create_window(
                WindowAttributes::default()
                    .with_title("Clear Designer")
                    .with_inner_size(winit::dpi::LogicalSize::new(1280, 800)),
            ).unwrap(),
        );

        let state = pollster::block_on(State::new(window));
        self.state = Some(state);
        self.state.as_ref().unwrap().window.request_redraw();
    }

    fn window_event(
        &mut self, event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId, event: WindowEvent,
    ) {
        let needs_redraw = match event {
            WindowEvent::CloseRequested => { event_loop.exit(); true }
            WindowEvent::Resized(size) => {
                if let Some(state) = &mut self.state { state.resize(size); }
                true
            }
            WindowEvent::RedrawRequested => {
                if let Some(state) = &mut self.state {
                    state.render();
                    state.window.request_redraw();
                }
                true
            }
            WindowEvent::ScaleFactorChanged { scale_factor, mut inner_size_writer } => {
                if let Some(state) = &mut self.state {
                    let new_physical = winit::dpi::PhysicalSize::new(
                        (state.width as f64 * scale_factor) as u32,
                        (state.height as f64 * scale_factor) as u32,
                    );
                    let _ = inner_size_writer.request_inner_size(new_physical);
                    state.scale = scale_factor;
                    state.resize(new_physical);
                    state.window.request_redraw();
                }
                true
            }
            _ => {
                if let Some(state) = &mut self.state {
                    let mut changed = state.handle_event(&event);

                    if let Some(seg) = state.widgets[BREADCRUMB_IDX].path_click() {
                        if seg < state.current_path.len() {
                            state.current_path.truncate(seg);
                            state.pan_x = 0.0;
                            state.pan_y = 0.0;
                            state.pan_velocity_x = 0.0;
                            state.pan_velocity_y = 0.0;
                            state.last_frame_pan_x = 0.0;
                            state.last_frame_pan_y = 0.0;
                            state.is_scrolling_trackpad = false;
                            state.scroll_accum_x = 0.0;
                            state.scroll_accum_y = 0.0;
                            state.grid_cursor_col = 0;
                            state.grid_cursor_row = 0;
                            state.sync_grid_settings();
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
                                    state.widgets[CONFIG_DIALOG_IDX].set_config_toggle(0, state.grid_snap_enabled);
                                    state.widgets[CONFIG_DIALOG_IDX].set_config_toggle(1, state.network_grid_visible);
                                    state.widgets[CONFIG_DIALOG_IDX].set_config_spin(0, state.grid_size_x);
                                    state.widgets[CONFIG_DIALOG_IDX].set_config_spin(1, state.grid_size_y);
                                    state.widgets[CONFIG_DIALOG_IDX].set_config_spin(2, state.skipped_row_h);
                                    state.widgets[CONFIG_DIALOG_IDX].set_config_spin(3, state.skipped_col_w);
                                    state.widgets[CONFIG_DIALOG_IDX].set_visible(true);
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
                                    _ => None,
                                }
                            } else { None };
                            if let Some(a) = action {
                                state.execute_action(a);
                                changed = true;
                            }
                        }
                    }

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
                            _ => {}
                        }
                        changed = true;
                    }

                    while let Some((id, val)) = state.widgets[CONFIG_DIALOG_IDX].take_config_spin() {
                        match id {
                            0 => state.grid_size_x = val,
                            1 => state.grid_size_y = val,
                            2 => state.skipped_row_h = val,
                            3 => state.skipped_col_w = val,
                            _ => {}
                        }
                        for &i in &state.node_slots {
                            let (x, y, _, _) = state.widgets[i].rect();
                            state.widgets[i].set_rect(x, y, state.grid_size_x, state.grid_size_y);
                        }
                        state.sync_grid_settings();
                        changed = true;
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
                    changed
                } else { false }
            }
        };
        if needs_redraw {
            if let Some(state) = &mut self.state { state.window.request_redraw(); }
        }
        if let Some(state) = &self.state {
            if state.exit_requested {
                event_loop.exit();
            }
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
    event_loop.run_app(&mut app).unwrap();
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
            "Pos X".to_string(),
            "Pos Y".to_string(),
            "Pos Z".to_string(),
            "Col R".to_string(),
            "Col G".to_string(),
            "Col B".to_string(),
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
}
