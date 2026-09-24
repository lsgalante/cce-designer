//! The designer on the cce-ui engine: `impl Application for State`.
//!
//! The engine owns the Wayland plumbing, event loop, and renderer; these
//! hooks translate its callbacks into the designer's `WindowEvent`s, stage
//! the 3D scene each frame, and run the detached circular window's
//! non-rectangular CSD (radial border resize, top-arc move).

use std::time::{Duration, Instant};

use cce_ui::engine::{
    xdg_toplevel::ResizeEdge, Application, CursorIcon, EngineState, LogicalPosition, LogicalSize,
    WindowAction, WindowSettings,
};
use cce_ui::vk::VkRenderer;
use cce_ui::widget::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use wayland_client::QueueHandle;

use crate::api::start_mcp_server;
use crate::app::{CustomEvent, PendingWindowDrag, State};
use crate::slots::LEFT_MENUBAR_IDX;
use crate::window::{LocalPosition, WindowEvent};

/// Pointer travel (logical px) before a chrome press becomes an interactive
/// move/resize, so a plain click on the border doesn't start a grab.
const DRAG_THRESHOLD: f32 = 4.0;

impl State {
    /// What the detached circular window's chrome at (lx, ly) would do:
    /// radial border band → resize, top menubar arc → move, an open menu
    /// always wins. `None` outside the chrome.
    fn circular_chrome_at(&self, lx: f32, ly: f32) -> Option<WindowAction> {
        let dx = lx - self.circular_network_layout.x;
        let dy = ly - self.circular_network_layout.y;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist <= f32::EPSILON {
            return None;
        }
        let r = self.circular_network_layout.r;
        let on_border = dist >= r - 12.0 && dist <= r;
        let in_menubar_bg = dy < 0.0 && dist >= r - 35.0 && dist <= r;
        if !(on_border || in_menubar_bg)
            || self.menu(LEFT_MENUBAR_IDX).get_menu_items_at(lx, ly).is_some()
        {
            return None;
        }
        if on_border {
            let nx = dx / dist;
            let ny = dy / dist;
            let edge = if ny < -0.382 {
                if nx < -0.382 {
                    ResizeEdge::TopLeft
                } else if nx > 0.382 {
                    ResizeEdge::TopRight
                } else {
                    ResizeEdge::Top
                }
            } else if ny > 0.382 {
                if nx < -0.382 {
                    ResizeEdge::BottomLeft
                } else if nx > 0.382 {
                    ResizeEdge::BottomRight
                } else {
                    ResizeEdge::Bottom
                }
            } else if nx < -0.382 {
                ResizeEdge::Left
            } else {
                ResizeEdge::Right
            };
            Some(WindowAction::Resize(edge))
        } else {
            Some(WindowAction::Move)
        }
    }

    /// The engine syncs modifier state into the UiContext (on key and wheel
    /// events); mirror it into the designer's own `ModifiersState`.
    fn sync_modifiers_from_ctx(&mut self) {
        self.modifiers.ctrl = self.ui_context.ctrl_pressed;
        self.modifiers.shift = self.ui_context.shift_pressed;
        self.modifiers.alt = self.ui_context.alt_pressed;
        self.modifiers.logo = self.ui_context.logo_pressed;
    }

    fn default_project_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json")
    }

    /// Detached-window sync (both directions): debounced autosave of the
    /// shared `default_project.json`, and mtime-polled reload when the other
    /// window wrote it. Returns true when a reload happened.
    fn poll_shared_project(&mut self) -> bool {
        let mut redraw = false;
        let syncing = self.is_detached_network
            || self.detached_circular_network
            || self.detached_pane.is_some()
            || self.detached_panes.iter().any(|d| *d);
        if !syncing {
            return false;
        }

        if self.needs_autosave {
            let now = Instant::now();
            if now.duration_since(self.last_autosave_time) >= Duration::from_millis(200) {
                self.needs_autosave = false;
                self.last_autosave_time = now;
                let path = Self::default_project_path();
                if let Err(e) = self.save_to_file(&path) {
                    eprintln!("Failed to auto-save default project: {:?}", e);
                } else if let Ok(m) = std::fs::metadata(&path) {
                    if let Ok(mod_time) = m.modified() {
                        self.last_project_mod_time = Some(mod_time);
                    }
                }
            }
        }

        let now = Instant::now();
        if now.duration_since(self.last_project_check) >= Duration::from_millis(100) {
            self.last_project_check = now;
            let path = Self::default_project_path();
            if let Ok(m) = std::fs::metadata(&path) {
                if let Ok(mod_time) = m.modified() {
                    if Some(mod_time) != self.last_project_mod_time {
                        self.last_project_mod_time = Some(mod_time);
                        if let Err(e) = self.load_from_file(&path) {
                            eprintln!("Failed to auto-reload project: {:?}", e);
                        } else {
                            redraw = true;
                        }
                    }
                }
            }
        }
        redraw
    }

    pub(crate) fn autosave_on_exit(&mut self) {
        if self.needs_autosave
            && (self.is_detached_network
                || self.detached_circular_network
                || self.detached_pane.is_some()
                || self.detached_panes.iter().any(|d| *d))
        {
            let _ = self.save_to_file(&Self::default_project_path());
        }
    }
}

impl Application for State {
    type Message = CustomEvent;

    fn new(
        _qh: &QueueHandle<EngineState<Self>>,
        sender: calloop::channel::Sender<CustomEvent>,
    ) -> Self {
        let is_detached_network = std::env::args().any(|arg| arg == "--detached-network");
        let detached_pane = std::env::args()
            .find_map(|arg| crate::plate_corner::pane_from_detach_flag(&arg));
        let mut state = State::new(is_detached_network);
        if let Some(idx) = detached_pane {
            // Set after construction, so the layout that `State::new` already
            // ran has to be redone against the detached shape.
            state.detached_pane = Some(idx);
            state.rebuild_positions();
            state.apply_layout();
        }
        state.event_sender = Some(sender.clone());
        // One MCP server per project: the detached windows are satellites of the
        // main one and would only collide on the port.
        if !is_detached_network && detached_pane.is_none() {
            // The user's configured startup project, over the bundled default
            // State::new seeded. Main window only: the detached windows read
            // default_project.json as their sync channel and must keep it.
            state.load_default_project_setting();
            start_mcp_server(sender);
        }
        state
    }

    fn settings(&self) -> WindowSettings {
        let (app_id, min_size) = if self.is_detached_network {
            ("circular-network-pane", (200, 200))
        } else if let Some(idx) = self.detached_pane {
            (crate::plate_corner::pane_app_id(idx), (240, 160))
        } else {
            ("cce-designer", (480, 320))
        };
        WindowSettings {
            title: self.title.clone(),
            app_id: app_id.to_string(),
            width: self.width as u32,
            height: self.height as u32,
            fullscreen: false,
            min_size: Some(min_size),
        }
    }

    fn update(&mut self, msg: CustomEvent, needs_rebuild: &mut bool, exit: &mut bool) {
        if matches!(msg, CustomEvent::Exit) {
            self.autosave_on_exit();
            *exit = true;
            return;
        }
        if self.apply_custom_event(msg) {
            *needs_rebuild = true;
        }
        // MCP can reach File > Exit through menu_action.
        if self.exit_requested {
            self.autosave_on_exit();
            *exit = true;
        }
    }

    /// `poll_shared_project` watches the project file's mtime for a detached
    /// pane's edits — nothing the runner can be woken by — so while a pane is
    /// detached the loop may not sleep past this between ticks.
    fn idle_poll_interval(&self) -> Option<std::time::Duration> {
        let syncing = self.is_detached_network
            || self.detached_circular_network
            || self.detached_pane.is_some()
            || self.detached_panes.iter().any(|d| *d);
        syncing.then(|| std::time::Duration::from_millis(250))
    }

    fn tick(&mut self, dt: f32, needs_rebuild: &mut bool) {
        if self.tick_frame(dt) {
            *needs_rebuild = true;
        }
        if self.poll_shared_project() {
            *needs_rebuild = true;
        }
    }

    fn ui_context(&self) -> Option<&cce_ui::context::UiContext> {
        Some(&self.ui_context)
    }

    fn ui_context_mut(&mut self) -> Option<&mut cce_ui::context::UiContext> {
        Some(&mut self.ui_context)
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        self.sync_modifiers_from_ctx();
        if let Some(pending) = self.pending_window_drag {
            let dx = pos.x - pending.start_x;
            let dy = pos.y - pending.start_y;
            if (dx * dx + dy * dy).sqrt() > DRAG_THRESHOLD {
                self.window_action = Some(pending.action);
                self.pending_window_drag = None;
            }
        }
        let ev = WindowEvent::CursorMoved {
            position: LocalPosition { x: pos.x as f64, y: pos.y as f64 },
        };
        if self.process_window_event(ev) {
            *needs_rebuild = true;
        }
    }

    fn handle_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
        pos: LogicalPosition,
        needs_rebuild: &mut bool,
    ) -> Option<CustomEvent> {
        self.sync_modifiers_from_ctx();
        self.cursor_x = pos.x;
        self.cursor_y = pos.y;
        if self.is_detached_network && button == MouseButton::Left {
            match state {
                ElementState::Pressed => {
                    if let Some(action) = self.circular_chrome_at(pos.x, pos.y) {
                        self.pending_window_drag = Some(PendingWindowDrag {
                            start_x: pos.x,
                            start_y: pos.y,
                            action,
                        });
                        return None; // consumed by the window chrome
                    }
                }
                ElementState::Released => {
                    self.pending_window_drag = None;
                }
            }
        }
        let ev = WindowEvent::MouseInput { state, button };
        if self.process_window_event(ev) {
            *needs_rebuild = true;
        }
        if self.exit_requested {
            return Some(CustomEvent::Exit);
        }
        None
    }

    fn handle_mouse_wheel(
        &mut self,
        delta: &MouseScrollDelta,
        pos: LogicalPosition,
        needs_rebuild: &mut bool,
    ) {
        self.sync_modifiers_from_ctx();
        self.cursor_x = pos.x;
        self.cursor_y = pos.y;
        let ev = WindowEvent::MouseWheel { delta: delta.clone() };
        if self.process_window_event(ev) {
            *needs_rebuild = true;
        }
    }

    fn handle_pinch(&mut self, factor: f32, pos: LogicalPosition, needs_rebuild: &mut bool) -> bool {
        self.cursor_x = pos.x;
        self.cursor_y = pos.y;
        // 1:1 camera zoom over the 3D viewport; anywhere else falls back to
        // the engine's ctrl+wheel synthesis (which is what zooms the graph).
        if self.is_detached_network || !self.cursor_in_viewport() {
            return false;
        }
        self.viewport_mut().pinch_zoom(factor);
        *needs_rebuild = true;
        true
    }

    /// The toolkit's undo/redo routing lands here once no focused text box
    /// wanted the chord. Only the curve viewer state has a history today.
    fn undo(&mut self, needs_rebuild: &mut bool) -> bool {
        // A code row being edited owns the chord: its typing is the thing to
        // undo, ahead of a viewer tool that may also be active.
        let taken = self.code_editor_action(cce_ui::widget::ContextAction::Undo) || self.viewer_tool_undo();
        if taken {
            *needs_rebuild = true;
        }
        taken
    }

    fn redo(&mut self, needs_rebuild: &mut bool) -> bool {
        let taken = self.code_editor_action(cce_ui::widget::ContextAction::Redo) || self.viewer_tool_redo();
        if taken {
            *needs_rebuild = true;
        }
        taken
    }

    fn handle_key_input(&mut self, event: &KeyEvent, needs_rebuild: &mut bool) -> Option<CustomEvent> {
        self.sync_modifiers_from_ctx();
        let ev = WindowEvent::KeyboardInput { event: event.clone() };
        if self.process_window_event(ev) {
            *needs_rebuild = true;
        }
        if self.exit_requested {
            return Some(CustomEvent::Exit);
        }
        None
    }

    fn display_list(&mut self, _size: LogicalSize, _scale: f64) -> Option<cce_ui::scene::paint::DisplayList> {
        // The single paint path: the whole 2D frame — geometry and text — rebuilt
        // every drawn frame (the engine only draws on demand). The 3D scene / RT
        // panes stay in stage_renderer.
        Some(self.collect_display_list())
    }

    fn display_list_text(&self) -> bool {
        true
    }

    fn renderer_init(&mut self, renderer: &mut VkRenderer) {
        self.init_renderer(renderer);
        // Everything that cannot survive a REPLACEMENT renderer, which this
        // may be — see `renderer_handed_over`.
        self.renderer_handed_over();
    }

    fn stage_renderer(&mut self, renderer: &mut VkRenderer, _size: LogicalSize, _scale: f64) -> bool {
        self.stage_frame(renderer)
    }

    fn handle_resize(&mut self, width: f32, height: f32, scale: f64) {
        self.resize(width, height, scale);
    }

    fn standard_csd(&self) -> bool {
        // The detached window's chrome is the circle, not the rect.
        !self.is_detached_network
    }

    fn cursor_icon(&self, x: f32, y: f32) -> Option<CursorIcon> {
        if !self.is_detached_network {
            // Main window: resize cursor over the pane edge-resize hotspots and
            // for the duration of a pane-edge drag; `None` elsewhere so the
            // engine's standard CSD edge cursors still apply.
            return self.pane_resize_cursor(x, y);
        }
        Some(match self.circular_chrome_at(x, y) {
            Some(WindowAction::Resize(edge)) => match edge {
                ResizeEdge::TopLeft => CursorIcon::NwResize,
                ResizeEdge::Top => CursorIcon::NResize,
                ResizeEdge::TopRight => CursorIcon::NeResize,
                ResizeEdge::Left => CursorIcon::WResize,
                ResizeEdge::Right => CursorIcon::EResize,
                ResizeEdge::BottomLeft => CursorIcon::SwResize,
                ResizeEdge::Bottom => CursorIcon::SResize,
                ResizeEdge::BottomRight => CursorIcon::SeResize,
                _ => CursorIcon::Default,
            },
            _ => CursorIcon::Default,
        })
    }

    fn take_window_action(&mut self) -> Option<WindowAction> {
        self.window_action.take()
    }

    fn on_exit(&mut self) {
        self.autosave_on_exit();
    }
}
