//! `State::handle_event`, the one entry every window event takes into the
//! app (from `process_window_event` and the tests), split by the kind of
//! event: the wheel, the pointer moving, a button, a key. Each arm is the
//! whole of what that kind of event does, early returns included.
//!
//! A child module of `app` so it reaches `State`'s private fields and
//! methods as `app.rs` itself does.

use super::*;
use crate::window::LocalPosition;

impl State {
    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        // A press, a release, or a key that ends an entry ends the gesture
        // an undo step is: what the params pane writes next is a new one.
        match event {
            WindowEvent::MouseInput { .. } => self.doc.edit_history.break_group(),
            WindowEvent::KeyboardInput { event } if event.state == ElementState::Pressed => {
                if matches!(
                    event.logical_key,
                    Key::Named(NamedKey::Enter | NamedKey::Tab | NamedKey::Escape)
                ) {
                    self.doc.edit_history.break_group();
                }
            }
            _ => {}
        }
        match event {
            WindowEvent::MouseWheel { delta } => self.on_wheel(delta),
            WindowEvent::CursorMoved { position, .. } => self.on_pointer_moved(position),
            WindowEvent::MouseInput { state: btn_state, button, .. } => self.on_mouse_button(btn_state, button),
            WindowEvent::KeyboardInput { event, .. } => self.on_key(event),
        }
    }

    /// The wheel: the modal claims first, then the camera's shift pan,
    /// then whichever pane the wheel lands on — focused by it, offered it
    /// through the routed pass, and failing that the network's own zoom
    /// and pan or the viewport's ctrl zoom.
    fn on_wheel(&mut self, delta: &MouseScrollDelta) -> bool {
        if let Some(r) = self.wheel_modal(delta) {
            return r;
        }
        if let Some(r) = self.camera_wheel_pan(delta) {
            return r;
        }
        let (overlay, in_network_pane) = self.wheel_target();
        let in_viewport = self.cursor_in_viewport();
        let focus_changed = self.focus_pane_for_wheel(in_network_pane, in_viewport);
        let handled = self.route_wheel_to_slots(delta, overlay, in_network_pane);

        let result = if handled {
            self.sync_parameters_to_project();

            // Sync Parameters pane with selected node
            self.sync_parameters_pane();


            self.sync_layout();
            self.read_panel_offsets();
            true
        } else if in_network_pane {
            self.network_wheel(delta)
        } else if in_viewport && self.modifiers.control_key() {
            self.viewport_ctrl_wheel(delta)
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

    /// What claims a wheel ahead of every pane: the tail of a swipe that
    /// turned something, the dialog, an open menu.
    fn wheel_modal(&mut self, delta: &MouseScrollDelta) -> Option<bool> {
        // What is left of a swipe that turned a menu or the dialog is
        // the turn's, not the scene's or a list's under the new plate.
        if cce_ui::widget::side_swipe::swallow(delta) {
            return Some(true);
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
            return Some(self.dialog_mouse_wheel(*delta));
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
            return Some(true);
        }
        None
    }

    fn camera_wheel_pan(&mut self, delta: &MouseScrollDelta) -> Option<bool> {
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
            return Some(true);
        }
        None
    }

    /// Whether the network is laid over the window, and whether this
    /// wheel is the network's.
    fn wheel_target(&mut self) -> (bool, bool) {
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
        (overlay, in_network_pane)
    }

    /// The pane under a wheel takes the focus. True when it moved.
    fn focus_pane_for_wheel(&mut self, in_network_pane: bool, in_viewport: bool) -> bool {
        let node_area_y = self.positions[CONTENT_IDX].1;

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
        focus_changed
    }

    /// The routed pass: the wheel offered to every slot but the viewport,
    /// then to the viewport. True when one took it.
    fn route_wheel_to_slots(&mut self, delta: &MouseScrollDelta, overlay: bool, in_network_pane: bool) -> bool {
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
                let root = self.slots.get_dyn_mut(&mut self.ui_context, i).base().id();
                if self.ui_context.propagate_event(&wheel_ev, root) {
                    handled = true;
                    if i == CONTENT_IDX {
                        self.network_adopt_graph_origin();
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
        handled
    }

    /// The Graph took the wheel: the network's pan is wherever it moved
    /// the grid origin.
    fn network_adopt_graph_origin(&mut self) {
        let active_node_area_y = self.positions[CONTENT_IDX].1;
        let active_node_area_x = self.positions[CONTENT_IDX].0;
        let w = self.slots.get_dyn(&self.ui_context, CONTENT_IDX);
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
        self.net.pan_x = gx - active_node_area_x;
        self.net.pan_y = gy - active_node_area_y;
        self.net.pan_velocity_x = 0.0;
        self.net.pan_velocity_y = 0.0;
    }

    /// A wheel over the network that no widget took: ctrl zooms about
    /// the pointer, a plain one pans.
    fn network_wheel(&mut self, delta: &MouseScrollDelta) -> bool {
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
                    (*x * 30.0 * self.net.graph_scroll_speed, *y * 30.0 * self.net.graph_scroll_speed)
                }
                MouseScrollDelta::PixelDelta(pos) => (
                    (pos.x as f32 / self.scale as f32) * self.net.graph_scroll_speed,
                    (pos.y as f32 / self.scale as f32) * self.net.graph_scroll_speed,
                ),
            };
            self.net.pan_x -= dx;
            self.net.pan_y -= dy;
            self.net.pan_velocity_x = 0.0;
            self.net.pan_velocity_y = 0.0;
            self.sync_grid_settings();
            true
        }
    }

    fn viewport_ctrl_wheel(&mut self, delta: &MouseScrollDelta) -> bool {
        // Ctrl+wheel zoom over the 3D viewport. The routed pass
        // above deliberately skips wheels while ctrl is held, so
        // hand the event to the viewport widget here — its ctrl
        // branch is the camera zoom. (Trackpad pinches take the
        // direct `handle_pinch` path and never reach this.)
        self.slots.get_dyn_mut(&mut self.ui_context, VIEWPORT_IDX).set_modifiers(
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
    }

    /// The pointer moving: the menus' hover, then whatever capture holds
    /// the pointer (a menu slider, a handle, the camera, a grid drag),
    /// then the network's pan, an app drag or a widget drag, and the
    /// hover broadcast.
    fn on_pointer_moved(&mut self, position: &LocalPosition) -> bool {
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

        if self.menu_pointer_moved() {
            changed = true;
        }
        if let Some(r) = self.menu_slider_motion() {
            return r;
        }

        // An in-flight curve-tool grab eats motion ahead of every
        // other drag: the grabbed control point tracks the cursor.
        if self.viewer_tool_drag_motion() {
            return true;
        }

        if let Some(r) = self.camera_drag_motion() {
            return r;
        }

        // An expansion drag on the network grid, likewise armed by
        // its own press and competing with nothing: the cursor grows
        // from the pressed cell to the one under the pointer.
        if self.net.grid_cursor_drag.is_some() {
            return self.grid_cursor_drag_motion();
        }

        if self.net.is_panning {
            self.network_pan_motion();
            changed = true;
        } else {
            if let Some(drag) = self.app_drag {
                self.app_drag_motion(drag);
                changed = true;
            } else if let Some(idx) = self.drag_widget {
                if self.widget_drag_motion(idx) {
                    changed = true;
                }
            }

            if self.broadcast_pointer() {
                changed = true;
            }
        }
        changed
    }

    /// The open menus' and the dialog dropdown's highlights follow the
    /// pointer. True when one moved.
    fn menu_pointer_moved(&mut self) -> bool {
        let mut changed = false;
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
        changed
    }

    fn menu_slider_motion(&mut self) -> Option<bool> {
        // A held menu slider follows the pointer (cursor_moved moved
        // it); land the value, and let nothing else read this motion
        // as a drag of its own.
        if self.slider_menu_open() && cce_ui::widget::context_menu::slider_dragging() {
            self.drain_menu_slider(false);
            return Some(true);
        }
        None
    }

    fn camera_drag_motion(&mut self) -> Option<bool> {
        // An in-flight camera orbit, likewise — it was armed by a press
        // on empty scene, so nothing else is competing for the motion.
        if let Some((lx, ly)) = self.camera.pan_drag {
            let (dx, dy) = (self.cursor_x - lx, self.cursor_y - ly);
            self.camera.pan_drag = Some((self.cursor_x, self.cursor_y));
            if dx != 0.0 || dy != 0.0 {
                self.pan_camera_by(dx, dy);
            }
            return Some(true);
        }
        if let Some((lx, ly)) = self.camera.orbit_drag {
            let (dx, dy) = (self.cursor_x - lx, self.cursor_y - ly);
            self.camera.orbit_drag = Some((self.cursor_x, self.cursor_y));
            if dx != 0.0 || dy != 0.0 {
                self.orbit_camera_by(dx, dy);
            }
            return Some(true);
        }
        None
    }

    fn network_pan_motion(&mut self) {
        let dx = self.cursor_x - self.net.pan_start_cx;
        let dy = self.cursor_y - self.net.pan_start_cy;
        self.net.pan_x = self.net.pan_start_x + dx;
        self.net.pan_y = self.net.pan_start_y + dy;
        self.sync_grid_settings();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
    }

    /// A floating pane's edge, dragged.
    fn app_drag_motion(&mut self, drag: AppDrag) {
        match drag {
            AppDrag::HudResize { start_w, start_mouse_x } => {
                let dx = self.cursor_x - start_mouse_x;
                self.params_hud_width = (start_w - dx).max(PARAMS_HUD_MIN_W);
                self.params_hud_width = self.params_hud_rect().2;
                self.rebuild_positions();
                self.apply_layout();
            }
            AppDrag::SpreadsheetResize { start_h, start_mouse_y } => {
                let dy = self.cursor_y - start_mouse_y;
                let new_h = (start_h - dy).max(100.0);
                self.floating_spreadsheet_height = new_h;
                self.floating_spreadsheet_height = self.floating_spreadsheet_rect().3;
                self.rebuild_positions();
                self.apply_layout();
            }
        }
    }

    /// A widget drag: the slot's own Input drag hooks are driving.
    fn widget_drag_motion(&mut self, idx: usize) -> bool {
        let mut changed = false;
        self.slots.get_dyn_mut(&mut self.ui_context, idx).set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
        if {
            let ev = cce_ui::widget::Event::DragUpdate { dx: 0.0, dy: 0.0, x: self.cursor_x, y: self.cursor_y, local_x: self.cursor_x, local_y: self.cursor_y };
            self.deliver_to_slot(idx, &ev)
        } {
            changed = true;
            if idx == CONTENT_IDX && self.net.node_drag_group.is_some() {
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
    fn on_mouse_button(&mut self, btn_state: &ElementState, button: &MouseButton) -> bool {
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
            self.stop_inertia();
        }
        if let Some(r) = self.button_modal(btn_state, button) {
            return r;
        }
        let in_network_pane = self.in_network_pane();

        if let Some(r) = self.network_pan_button(btn_state, button) {
            return r;
        }
        if let Some(r) = self.camera_middle_button(btn_state, button) {
            return r;
        }

        if *button != MouseButton::Left && *button != MouseButton::Right { return false; }
        let mut changed = false;
        let old_focus = self.focused_widget;

        let in_circle_network_pane = if self.circular_network_pane {
            self.circular_network_layout.hit_test_content(self.cursor_x, self.cursor_y, 0.0, breadcrumb_h())
        } else {
            in_network_pane
        };

        match btn_state {
            ElementState::Pressed => {
                if let Some(r) = self.on_press(btn_state, button, in_network_pane, in_circle_network_pane, &mut changed) {
                    return r;
                }
            }
            ElementState::Released => {
                if let Some(r) = self.on_release(btn_state, button, &mut changed) {
                    return r;
                }
            }
        }

        if self.drain_graph_requests() {
            changed = true;
        }

        if self.focused_widget != old_focus {
            changed = true;
        }
        changed
    }

    /// A press stops what is coasting: the network's pan and the camera.
    fn stop_inertia(&mut self) {
        self.net.pan_velocity_x = 0.0;
        self.net.pan_velocity_y = 0.0;

        self.viewport_mut().reset_velocity();
    }

    fn button_modal(&mut self, btn_state: &ElementState, button: &MouseButton) -> Option<bool> {
        // The dialog is modal over what it covers, so it takes the
        // press before the pan trigger and before the whole
        // hit-target cascade. A press OUTSIDE it dismisses — the
        // convention every other floating surface in this app follows
        // (the node menu, the plate corner menu) — and is swallowed
        // rather than also acting on the pane it landed on.
        // A choice row's dropdown is in front of the dialog: it takes
        // the press first, and a press off it closes it alone.
        if *btn_state == ElementState::Pressed && self.dialog_dropdown_takes_press() {
            return Some(self.dialog_dropdown_press(*button));
        }
        if self.dialog_visible() {
            if let Some(handled) = self.dialog_mouse_input(*button, *btn_state) {
                return Some(handled);
            }
        }
        None
    }

    /// The network's pan: the middle button, or space and the left, over
    /// the network's area.
    fn network_pan_button(&mut self, btn_state: &ElementState, button: &MouseButton) -> Option<bool> {
        // Panning asks the AREA, not the nodes: in overlay mode a
        // middle-drag over empty space still pans the graph, because
        // nothing else wants that gesture.
        let is_pan_trigger = self.in_network_area(self.cursor_x, self.cursor_y)
            && (*button == MouseButton::Middle
                || (*button == MouseButton::Left && self.net.space_pressed));

        if is_pan_trigger {
            match btn_state {
                ElementState::Pressed => {
                    self.net.is_panning = true;
                    self.net.pan_start_cx = self.cursor_x;
                    self.net.pan_start_cy = self.cursor_y;
                    self.net.pan_start_x = self.net.pan_x;
                    self.net.pan_start_y = self.net.pan_y;
                    self.net.pan_velocity_x = 0.0;
                    self.net.pan_velocity_y = 0.0;
                    self.net.last_frame_pan_x = self.net.pan_x;
                    self.net.last_frame_pan_y = self.net.pan_y;
                    return Some(true);
                }
                ElementState::Released => {
                    if self.net.is_panning {
                        self.net.is_panning = false;
                        self.sync_layout();
                        self.read_panel_offsets();
                        return Some(true);
                    }
                }
            }
        }

        if self.net.is_panning && *btn_state == ElementState::Released {
            self.net.is_panning = false;
            self.sync_layout();
            self.read_panel_offsets();
            return Some(true);
        }
        None
    }

    fn camera_middle_button(&mut self, btn_state: &ElementState, button: &MouseButton) -> Option<bool> {
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
            self.camera.pan_drag = Some((self.cursor_x, self.cursor_y));
            self.focused_pane = RIGHT_MENUBAR_IDX;
            self.sync_pane_focus();
            return Some(true);
        }
        if *button == MouseButton::Middle && *btn_state == ElementState::Released && self.camera.pan_drag.take().is_some() {
            self.broadcast_pointer();
            return Some(true);
        }
        None
    }

    /// Whether a press at (x, y) lands on slot `i`, by the app's own hit
    /// shapes where the widget's rect would claim too much: the circular
    /// pane, the params HUD's rows, the overlaid network's nodes.
    fn hits_widget(&self, i: usize, x: f32, y: f32) -> bool {
        if self.circular_network_pane && (i == CONTENT_IDX || i == LEFT_MENUBAR_IDX || i == BREADCRUMB_IDX) {
            if i == CONTENT_IDX {
                self.circular_network_layout.hit_test_content(x, y, 0.0, breadcrumb_h())
            } else if i == LEFT_MENUBAR_IDX {
                false
            } else if i == BREADCRUMB_IDX {
                self.circular_network_layout.hit_test_breadcrumb(x, y, 0.0, breadcrumb_h())
            } else {
                false
            }
        } else if i == PARAM_IDX {
            // The HUD is under every plate, and without its own
            // plate it is its rows: a press anywhere else is the
            // plate's or the scene's.
            self.params_claims(x, y) && self.slots.get_dyn(&self.ui_context, i).hit_test(x, y, &self.ui_context)
        } else if self.network_overlay() && i == CONTENT_IDX {
            // The graph's RECT spans the window in overlay mode,
            // so the widget's own hit test would claim every press
            // and there would be no way left to orbit the camera.
            // It claims where it has drawn a node; the rest of the
            // window is the scene. Same rule as `in_network_pane`,
            // and the circular pane above refines its hit test the
            // same way.
            self.overlay_claims(x, y)
        } else {
            self.slots.get_dyn(&self.ui_context, i).hit_test(x, y, &self.ui_context)
        }
    }

    /// A left or right press: the menus, the network's chrome, the resize
    /// edges, the viewer state, the box select and the camera drags each
    /// get the first shot in turn; a right press that none took opens a
    /// context menu; anything else goes to the slot it lands on, which
    /// takes the focus.
    fn on_press(
        &mut self,
        btn_state: &ElementState,
        button: &MouseButton,
        in_network_pane: bool,
        in_circle_network_pane: bool,
        changed: &mut bool,
    ) -> Option<bool> {
        if let Some(r) = self.press_menus(button) {
            return Some(r);
        }
        if let Some(r) = self.press_network_chrome(btn_state, button) {
            return Some(r);
        }
        if let Some(r) = self.press_resize_edges(button) {
            return Some(r);
        }
        if let Some(r) = self.press_viewer_tool(button, in_circle_network_pane) {
            return Some(r);
        }
        if let Some(r) = self.press_network_box_select(button, in_network_pane) {
            return Some(r);
        }
        if let Some(r) = self.press_camera_drag(button, in_circle_network_pane) {
            return Some(r);
        }
        if *button == MouseButton::Right {
            return Some(self.press_right(in_circle_network_pane));
        }
        let click_target = self.click_target();
        if self.focus_for_press(click_target, in_circle_network_pane) {
            *changed = true;
        }
        if let Some(i) = click_target {
            if self.deliver_press(i, btn_state, button) {
                *changed = true;
            }
        }
        self.sync_pane_focus();
        None
    }

    /// The open menus take a press first, and a right press on a
    /// parameter row, the playbar or a plate opens its menu.
    fn press_menus(&mut self, button: &MouseButton) -> Option<bool> {
        // A left press on a page row or a back band of any
        // open menu turns it, ahead of the menus' own rows.
        if *button == MouseButton::Left && self.press_menu_turn() {
            return Some(true);
        }
        // The node context menu takes the first shot at a
        // press: a left click on it runs the item; any other
        // press (or a left click outside) dismisses it and
        // falls through to normal handling.
        if self.node_menu_open() {
            if *button == MouseButton::Left && self.handle_node_menu_click() {
                return Some(true);
            }
            self.close_node_menu();
        }
        // The plate corner menu, same contract as the node
        // menu above: it took the click, or it is dismissed.
        if self.plate_menu_open() {
            if *button == MouseButton::Left && self.handle_plate_menu_click() {
                return Some(true);
            }
            self.close_plate_menu();
            if *button == MouseButton::Left {
                return Some(true);
            }
        }
        // A plate drawn as its stub has nothing in it but its
        // title: a left press on a collapsed one expands it,
        // and a right press on either kind opens its plate
        // menu (Expand, or a detached pane's Reattach).
        if let Some(idx) = self.plate_at(self.cursor_x, self.cursor_y).filter(|&i| self.pane_is_stubbed(i)) {
            if *button == MouseButton::Left && !self.pane_is_detached(idx) {
                self.set_pane_collapsed(idx, false);
                return Some(true);
            }
            if *button == MouseButton::Right {
                self.close_node_menu();
                self.close_viewport_menu();
                self.close_network_menu();
                self.close_param_menu();
                self.open_plate_menu(idx);
                return Some(true);
            }
        }
        // The network editor's menu, same contract again.
        if self.network_menu_open() {
            if *button == MouseButton::Left && self.handle_network_menu_click() {
                return Some(true);
            }
            self.close_network_menu();
            if *button == MouseButton::Left {
                return Some(true);
            }
        }
        if self.viewport_menu_open() {
            if *button == MouseButton::Left && self.handle_viewport_menu_click() {
                return Some(true);
            }
            self.close_viewport_menu();
            // Swallow the dismissing left press: it should not
            // also orbit/click whatever sits underneath, and
            // returning true forces the redraw that actually
            // erases the menu — falling through here left the
            // menu painted until the next incidental redraw.
            if *button == MouseButton::Left {
                return Some(true);
            }
        }
        // The playbar's menu, same contract.
        if self.playbar_menu_open() {
            if *button == MouseButton::Left && self.handle_playbar_menu_click() {
                return Some(true);
            }
            self.close_playbar_menu();
            if *button == MouseButton::Left {
                return Some(true);
            }
        }
        // The parameter row menu, same contract again.
        if self.param_menu_open() {
            if *button == MouseButton::Left && self.handle_param_menu_click() {
                return Some(true);
            }
            self.close_param_menu();
            if *button == MouseButton::Left {
                return Some(true);
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
                return Some(true);
            }
            // A right press on the playbar opens its menu.
            if self.over_playbar(self.cursor_x, self.cursor_y) {
                self.close_node_menu();
                self.close_viewport_menu();
                self.close_network_menu();
                self.open_playbar_context_menu();
                return Some(true);
            }
            // A plate with no context menu of its own — the
            // params pane off a row, the spreadsheet — has
            // its plate menu.
            if let Some(idx) = self
                .plate_at(self.cursor_x, self.cursor_y)
                .filter(|&i| matches!(i, PARAM_IDX | SPREADSHEET_IDX))
            {
                self.close_node_menu();
                self.close_viewport_menu();
                self.close_network_menu();
                self.open_plate_menu(idx);
                return Some(true);
            }
        }
        None
    }

    /// The circular pane's border and the network's breadcrumb.
    fn press_network_chrome(&mut self, btn_state: &ElementState, button: &MouseButton) -> Option<bool> {
        if *button == MouseButton::Left && self.circular_network_pane {
            // A border press focuses the pane and consumes. It used to also arm
            // a NETWORK_PANEL_IDX widget drag, but PassivePlate has no drag
            // hooks (the panel move died with the 6as Plate dissolution), so
            // the drag scaffolding is gone.
            let on_border = self.circular_network_layout.hit_test_border(self.cursor_x, self.cursor_y, 12.0);
            if on_border {
                self.focused_pane = LEFT_MENUBAR_IDX;
                if let Some(old) = self.focused_widget {
                    self.ui_context.unfocus_id(self.slots.id(old));
                    self.focused_widget = None;
                }
                self.ui_context.unfocus_id(self.slots.param.id());
                self.sync_parameters_to_project();
                return Some(true);
            }
        }

        // The network's breadcrumb hovers over the graph and
        // claims its segments only. Asked where it is drawn:
        // until 2026-10-07 this tested the old left dock's
        // top strip, so a press in that band of the scene
        // focused the network and went nowhere.
        if *button == MouseButton::Left && !self.circular_network_pane && self.show_network {
            let (bx, by, bw, bh) = self.positions[BREADCRUMB_IDX];
            let (cx, cy) = (self.cursor_x, self.cursor_y);
            if cx >= bx && cx < bx + bw && cy >= by && cy < by + bh
                && self.ui_context.lend_h(self.slots.breadcrumb, |b, ui| b.mouse_input(*button, *btn_state, cx, cy, ui)).unwrap_or(false)
            {
                return Some(true);
            }
        }
        None
    }

    /// The params HUD's left edge and the spreadsheet's top edge.
    fn press_resize_edges(&mut self, button: &MouseButton) -> Option<bool> {
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
                    self.ui_context.unfocus_id(self.slots.id(old));
                    self.focused_widget = None;
                }
                return Some(true);
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
                    self.ui_context.unfocus_id(self.slots.id(old));
                    self.focused_widget = None;
                }
                return Some(true);
            }
        }
        None
    }

    fn press_viewer_tool(&mut self, button: &MouseButton, in_circle_network_pane: bool) -> Option<bool> {
        // The curve viewer state takes the viewport press
        // ahead of the context menu and the click cascade:
        // left grabs or adds a control point, right on a
        // handle deletes it (right elsewhere still opens the
        // viewport menu below).
        if self.camera.viewer_tool.is_some()
            && self.cursor_in_viewport()
            && !in_circle_network_pane
        {
            if *button == MouseButton::Left {
                if self.viewer_tool_press() {
                    return Some(true);
                }
            } else if *button == MouseButton::Right
                && self.viewer_tool_delete_at_cursor()
            {
                return Some(true);
            }
        }
        None
    }

    fn press_network_box_select(&mut self, button: &MouseButton, in_network_pane: bool) -> Option<bool> {
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
            self.net.grid_cursor_col = col;
            self.net.grid_cursor_row = row;
            self.net.grid_cursor_expanse = None;
            self.net.grid_cursor_drag = Some((col, row));
            self.focused_pane = LEFT_MENUBAR_IDX;
            if let Some(old) = self.focused_widget.take() {
                self.ui_context.unfocus_id(self.slots.id(old));
            }
            self.sync_pane_focus();
            return Some(true);
        }
        None
    }

    fn press_camera_drag(&mut self, button: &MouseButton, in_circle_network_pane: bool) -> Option<bool> {
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
                self.camera.pan_drag = Some((self.cursor_x, self.cursor_y));
            } else {
                self.camera.orbit_drag = Some((self.cursor_x, self.cursor_y));
            }
            self.focused_pane = RIGHT_MENUBAR_IDX;
            if let Some(old) = self.focused_widget {
                self.ui_context.unfocus_id(self.slots.id(old));
                self.focused_widget = None;
            }
            self.sync_pane_focus();
            return Some(true);
        }
        None
    }

    /// A right press nothing above took: the viewport's menu over the
    /// scene, a node's or the network's in the circular pane.
    fn press_right(&mut self, in_circle_network_pane: bool) -> bool {
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
            self.net.grid_cursor_col = col;
            self.net.grid_cursor_row = row;
            self.open_network_context_menu();
            return true;
        }
        false
    }

    /// The slot a press lands on: one whose menubar menu is open, else the
    /// topmost the press hits.
    fn click_target(&mut self) -> Option<usize> {
        let mut click_target = None;
        for i in 0..WIDGET_COUNT {
            if self.menubar_at(i).map(|m| m.is_menu_open()).unwrap_or(false)
                && self.hits_widget(i, self.cursor_x, self.cursor_y)
            {
                click_target = Some(i);
                break;
            }
        }
        if click_target.is_none() {
            let mut hit_order: Vec<usize> = (0..WIDGET_COUNT).collect();
            hit_order.sort_by_key(|&i| {
                let z = if i == NETWORK_PANEL_IDX {
                    // Plates sort behind their content, or the
                    // stable sort hands the plate every press
                    // and the graph never hears a click.
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
                    self.slots.get_dyn_mut(&mut self.ui_context, i).z_index()
                };
                -z
            });
            click_target = hit_order.into_iter()
                .find(|&i| self.hits_widget(i, self.cursor_x, self.cursor_y));
        }
        click_target
    }

    /// The pane a press focuses, and the focus it takes from the widget
    /// that had it. True when something changed.
    fn focus_for_press(&mut self, click_target: Option<usize>, in_circle_network_pane: bool) -> bool {
        let mut changed = false;
        // Determine new focused pane
        let mut new_pane = None;
        if let Some(i) = click_target {
            if i == LEFT_MENUBAR_IDX
                || i == CONTENT_IDX
                || i == CANVAS_IDX
                || i == BREADCRUMB_IDX
                || i == NETWORK_PANEL_IDX
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
                self.ui_context.unfocus_id(self.slots.id(old));
                self.focused_widget = None;
            }
        }
        if click_target != Some(PARAM_IDX) {
            self.ui_context.unfocus_id(self.slots.param.id());
            self.sync_parameters_to_project();
        }
        if click_target.is_none() && in_circle_network_pane {
            let (col, row) = self.cell_at(self.cursor_x, self.cursor_y);
            self.net.grid_cursor_col = col;
            self.net.grid_cursor_row = row;
            changed = true;
        }
        changed
    }

    /// The press delivered to the slot it landed on, which may arm a
    /// widget drag and takes the focus. True when something changed.
    fn deliver_press(&mut self, i: usize, btn_state: &ElementState, button: &MouseButton) -> bool {
        let mut changed = false;
        self.slots.get_dyn_mut(&mut self.ui_context, i).set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
        // Kept as a binding: the network's cursor-expansion
        // drag must not arm on a press the graph already
        // took. A press on a PORT is the case that bites —
        // it starts a connection and returns true without
        // selecting anything, so the "no selection" arm
        // below would read it as empty grid and swallow the
        // motion the rubber-band line is drawn from.
        let widget_took = {
            let ev = cce_ui::widget::Event::MouseButton { button: *button, state: *btn_state, x: self.cursor_x, y: self.cursor_y, local_x: self.cursor_x, local_y: self.cursor_y };
            self.deliver_to_slot(i, &ev)
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
        if self.slots.draggable(&self.ui_context, i) {
            self.slots.get_dyn_mut(&mut self.ui_context, i).set_modifiers(self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
            {
                let ev = cce_ui::widget::Event::DragStart { start_x: self.cursor_x, start_y: self.cursor_y };
                self.deliver_to_slot(i, &ev);
            }
            self.drag_widget = Some(i);
            self.drag_press_cursor = Some((self.cursor_x, self.cursor_y));
        }
        if i != PARAM_IDX {
            self.ui_context.focus_id(self.slots.id(i));
            self.focused_widget = Some(i);
            if self.menubar_at(i).map(|m| m.is_menu_bar()).unwrap_or(false)
                && !self.slots.get_dyn(&self.ui_context, i).focused(&self.ui_context)
            {
                self.ui_context.unfocus_id(self.slots.id(i));
                self.focused_widget = None;
            }
        }
        if i == CONTENT_IDX && self.network_after_press(widget_took) {
            changed = true;
        }
        changed
    }

    /// After a press on the network: a group drag armed or the cursor
    /// moved to the node, an expansion drag armed on empty grid, a
    /// double click dived into. True when something changed.
    fn network_after_press(&mut self, widget_took: bool) -> bool {
        let mut changed = false;
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
                    self.net.node_drag_group = Some(NodeDragGroup {
                        dragged: slot_idx,
                        from: (pos.0 as i32, pos.1 as i32),
                        others: cells,
                    });
                    // A group does not swap: one of
                    // it trading places would leave
                    // the rest where the offset put
                    // them, the selection scattered.
                    self.ui_context[self.slots.content].inner_mut().set_swap_on_drop(false);
                } else {
                    self.ui_context[self.slots.content].inner_mut().set_swap_on_drop(true);
                    self.net.grid_cursor_col = pos.0 as i32;
                    self.net.grid_cursor_row = pos.1 as i32;
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
            self.net.grid_cursor_col = col;
            self.net.grid_cursor_row = row;
            self.net.grid_cursor_expanse = None;
            if !widget_took {
                self.net.grid_cursor_drag = Some((col, row));
            }
            changed = true;
        }
        if let Some(dir_idx) = self.graph().double_clicked_node() {
            self.graph_mut().clear_double_clicked_node();
            let dir = self.current_dir();
            if dir_idx < dir.children.len() && dir.children[dir_idx].is_enterable() {
                self.doc.current_path.push(dir_idx);
                self.on_path_changed();
                changed = true;
            }
        }
        changed
    }

    /// A left or right release: every capture it ends comes down, then the
    /// release goes to every slot.
    fn on_release(&mut self, btn_state: &ElementState, button: &MouseButton, changed: &mut bool) -> Option<bool> {
        // The expansion drag ends by SETTLING onto what it
        // caught — see `settle_cursor_expansion`.
        if self.net.grid_cursor_drag.take().is_some() && self.settle_cursor_expansion() {
            *changed = true;
        }
        if let Some(r) = self.release_camera_drag() {
            return Some(r);
        }
        if self.viewer_tool_release() {
            *changed = true;
        }
        if self.release_app_drag() {
            *changed = true;
        }
        if self.release_widget_drag() {
            *changed = true;
        }
        // Every capture the release ends is down by here:
        // the pane under the pointer hovers again without
        // waiting for it to move.
        if self.broadcast_pointer() {
            *changed = true;
        }
        if self.broadcast_release(btn_state, button) {
            *changed = true;
        }
        None
    }

    fn release_camera_drag(&mut self) -> Option<bool> {
        if self.camera.orbit_drag.take().is_some() || self.camera.pan_drag.take().is_some() {
            self.broadcast_pointer();
            return Some(true);
        }
        None
    }

    fn release_app_drag(&mut self) -> bool {
        let mut changed = false;
        if let Some(drag) = self.app_drag.take() {
            // App-mode drag teardown. The DragEnd send is kept from the old
            // shared teardown for faithfulness — the pane widget never began
            // a drag in resize mode, so its commit/cancel hook is a no-op.
            let idx = match drag {
                AppDrag::HudResize { .. } => PARAM_IDX,
                AppDrag::SpreadsheetResize { .. } => SPREADSHEET_IDX,
            };
            {
                self.deliver_to_slot(idx, &cce_ui::widget::Event::DragEnd);
            }
            self.sync_layout();
            changed = true;
        }
        changed
    }

    fn release_widget_drag(&mut self) -> bool {
        let mut changed = false;
        if self.drag_widget.is_some() {
            let idx = self.drag_widget.unwrap();
            if idx == PARAM_IDX {
                {
                    self.deliver_to_slot(idx, &cce_ui::widget::Event::DragEnd);
                }
                self.sync_layout();
            } else if idx == SPREADSHEET_IDX {
                {
                    self.deliver_to_slot(idx, &cce_ui::widget::Event::DragEnd);
                }
                self.sync_layout();
            } else if idx == CONTENT_IDX {
                {
                    self.deliver_to_slot(idx, &cce_ui::widget::Event::DragEnd);
                }
                self.network_commit_drag();
            } else {
                {
                    self.deliver_to_slot(idx, &cce_ui::widget::Event::DragEnd);
                }
            }
            self.drag_widget = None;
            self.drag_press_cursor = None;
            changed = true;
        }
        changed
    }

    /// A node drag released: the positions the widget committed are
    /// written back to the level, a dragged group re-laid from where its
    /// node landed.
    fn network_commit_drag(&mut self) {
        let updated_nodes = self.graph().get_nodes();
        let dir = self.current_dir_mut();
        for (i, node) in updated_nodes.iter().enumerate() {
            if let Some(child) = dir.children.get_mut(i) {
                child.position = node.position;
            }
        }
        if let Some(group) = self.net.node_drag_group.take() {
            // Re-lay the companions from the cell the
            // dragged node actually COMMITTED to: the
            // widget walks it off an occupied cell, so
            // the preview's offset can be a cell out
            // from the one that landed.
            let dest = updated_nodes
                .get(group.dragged)
                .map(|n| (n.position.0 as i32, n.position.1 as i32))
                .unwrap_or(group.from);
            self.net.node_drag_group = Some(group.clone());
            self.drag_group_to(dest);
            self.net.node_drag_group = None;
            // The region travels with what it holds,
            // as it does for alt+hjkl — the anchor sat
            // still through the drag precisely so it
            // would still be there to move.
            self.shift_grid_cursor(dest.0 - group.from.0, dest.1 - group.from.1);
        } else if let Some(sel_idx) = self.graph().selected_node() {
            if let Some(node) = updated_nodes.get(sel_idx) {
                self.net.grid_cursor_col = node.position.0 as i32;
                self.net.grid_cursor_row = node.position.1 as i32;
            }
        }
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
    }

    /// The release, handed to every slot.
    fn broadcast_release(&mut self, btn_state: &ElementState, button: &MouseButton) -> bool {
        let mut changed = false;
        let mut sync_params = false;
        {
            let (ctrl, shift, alt) = (self.modifiers.control_key(), self.modifiers.shift_key(), self.modifiers.alt_key());
            let ev = cce_ui::widget::Event::MouseButton { button: *button, state: *btn_state, x: self.cursor_x, y: self.cursor_y, local_x: self.cursor_x, local_y: self.cursor_y };
            for i in 0..WIDGET_COUNT {
                self.slots.get_dyn_mut(&mut self.ui_context, i).set_modifiers(ctrl, shift, alt);
                if self.deliver_to_slot(i, &ev) {
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
        changed
    }

    /// What the graph widget asked for in the press or the release,
    /// drained after it: a geometry flag, a connection to a port, a swap,
    /// a splice. True when something changed.
    fn drain_graph_requests(&mut self) -> bool {
        let mut changed = false;
        if let Some((i, visible)) = self.graph_mut().take_node_geom_toggle() {
            self.current_dir_mut().set_child_geometry_visible(i, visible);
            // The widget only flipped its own copy of the clicked node;
            // the exclusivity rule may have cleared siblings, so push
            // the model back out.
            self.sync_nodes();
            self.rebuild_scene_geometry();
            changed = true;
        }

        if let Some((input_node_id, output_node_name, port)) = self.graph_mut().take_pending_connection_to_port() {
            let path = self.doc.current_path.clone();
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
        changed
    }

    /// A key: the dialog's, then what answers from any pane, Escape, the
    /// viewer state's Delete, the panes' own keys, Tab and the chords, and
    /// last the focused widget.
    fn on_key(&mut self, event: &KeyEvent) -> bool {
        if event.logical_key == Key::Named(NamedKey::Space) {
            self.net.space_pressed = event.state == ElementState::Pressed;
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

        if let Some(r) = self.global_keys(event) {
            return r;
        }

        if event.state == ElementState::Pressed && event.logical_key == Key::Named(NamedKey::Escape) {
            return self.on_escape();
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
            // The hjkl navigation families used to be decoded
            // here, inline and unrebindable, with the bare form
            // ungated so it drifted the cursor from any pane.
            // They are registry commands now (nav_*, move_*,
            // view_*, frame_*), matched with every other chord
            // below.
            if self.network_key(event) || self.viewport_key(event) || self.zoom_key(event) {
                changed = true;
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
            self.deliver_to_slot(idx, &kev)
        }
        } else { false }
    }

    /// What takes a key from any pane: the context chords, the params
    /// pane's focused field, the transport.
    fn global_keys(&mut self, event: &KeyEvent) -> Option<bool> {
        // Context switching dispatches ahead of the widget key paths
        // (param pane, node palette) so the chord works from any pane.
        if event.state == ElementState::Pressed {
            if let Some(id @ ("next_context" | "previous_context")) =
                self.shortcut_manager.match_command(&self.modifiers, &event.logical_key)
            {
                self.run_command(id);
                return Some(true);
            }
        }

        if self.ui_context.lend_h(self.slots.param, |p, ui| p.keyboard_input(event, ui)).unwrap_or(false) {
            self.sync_parameters_to_project();
            return Some(true);
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
                return Some(true);
            }
        }
        None
    }

    /// Escape, which is always taken: an open menu, else the viewer
    /// state, else a connection being dragged and the selection.
    fn on_escape(&mut self) -> bool {
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
        if self.camera.viewer_tool.is_some() {
            self.camera.viewer_tool = None;
            return true;
        }
        self.graph_mut().cancel_connecting();
        // Last: clearing the selection. Escape is this app's one
        // "get me out" key, and the things above are all more
        // immediate than a selection — a menu you can see, a mode
        // you are in, a wire you are dragging.
        self.deselect_node();
        true
    }

    /// The network's keys, which act on its selection while it has the
    /// focus: Delete, `e`, `u`, `i`, ctrl+c / x / v. True when something
    /// changed.
    fn network_key(&mut self, event: &KeyEvent) -> bool {
        let is_plain_key = !self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key();
        let is_ctrl_only = self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key() && !self.modifiers.shift_key();
        let mut changed = false;
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
                    "i" | "I" => {
                        if self.focused_pane == LEFT_MENUBAR_IDX {
                            if let Some(slot_idx) = self.graph().selected_node() {
                                let dir = self.current_dir();
                                 if slot_idx < dir.children.len() && dir.children[slot_idx].is_enterable() {
                                     self.doc.current_path.push(slot_idx);
                                     self.on_path_changed();
                                     changed = true;
                                 }
                            }
                        }
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
        changed
    }

    /// The viewport's key while it has the focus: `r` resets the camera.
    fn viewport_key(&mut self, event: &KeyEvent) -> bool {
        let is_plain_key = !self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key();
        let mut changed = false;
        if let Key::Character(s) = &event.logical_key {
            if is_plain_key {
                match s.as_str() {
                    "r" | "R" => {
                        if self.focused_pane == RIGHT_MENUBAR_IDX {
                            if self.camera.active_camera != "Default Camera" {
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
                    _ => {}
                }
            }
        }
        changed
    }

    /// `-` and `=` zoom the network, from any pane.
    fn zoom_key(&mut self, event: &KeyEvent) -> bool {
        let is_plain_key = !self.modifiers.control_key() && !self.modifiers.alt_key() && !self.modifiers.super_key();
        let mut changed = false;
        if let Key::Character(s) = &event.logical_key {
            if is_plain_key {
                match s.as_str() {
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
        changed
    }
}
