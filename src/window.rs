//! The designer's window-event layer on the cce-ui engine.
//!
//! The Wayland plumbing (seat/pointer/keyboard handlers, configure, CSD)
//! lives in cce-ui's window runner; the `Application` impl (application.rs)
//! translates the runner's hooks into [`WindowEvent`]s. What remains here is
//! app policy: the post-event side-effect pass (`process_window_event`) and
//! MCP-action application (`apply_custom_event`).

use std::path::Path;

use cce_ui::widget::WidgetHost;
use crate::app::{State, CustomEvent, McpAction, Project, ParamDef};
use crate::slots::{LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX, HEADER_IDX, PARAM_IDX, WIDGET_COUNT};

#[derive(Debug, Clone, Copy)]
pub struct LocalPosition {
    pub x: f64,
    pub y: f64,
}

pub enum WindowEvent {
    MouseWheel { delta: cce_ui::widget::MouseScrollDelta },
    CursorMoved { position: LocalPosition },
    MouseInput { state: cce_ui::widget::ElementState, button: cce_ui::widget::MouseButton },
    KeyboardInput { event: cce_ui::widget::KeyEvent },
}

/// The menubar menus `process_window_event` still dispatches by index: the
/// viewport's Camera menu and the parameters' Preset and Reset, which run
/// what the camera and preset commands run. The rest of what the menubars
/// list is reached as a registry command.
pub(crate) fn menu_is_dispatched(widget_idx: usize, menu_idx: usize) -> bool {
    match widget_idx {
        RIGHT_MENUBAR_IDX => menu_idx == 0,
        PARAM_MENUBAR_IDX => menu_idx <= 1,
        _ => false,
    }
}

impl State {
    /// Route a window event through `handle_event`, then run the post-event
    /// side-effect pass (menu clicks, pane toggles, pending actions).
    /// Returns true when a redraw is needed.
    pub(crate) fn process_window_event(&mut self, ev: WindowEvent) -> bool {
        let mut result = false;
        {
            let state = &mut *self;
            let mut changed = state.handle_event(&ev);

            if let Some(seg) = state.path_mut().path_click() {
                if seg < state.current_path.len() {
                    let exited_idx = state.current_path.get(seg).copied();
                    state.current_path.truncate(seg);
                    state.on_path_changed();
                    if let Some(idx) = exited_idx {
                        let pos = {
                            let dir = state.current_dir();
                            if idx < dir.children.len() {
                                Some(dir.children[idx].position)
                            } else {
                                None
                            }
                        };
                        if let Some((pos_x, pos_y)) = pos {
                            state.grid_cursor_col = pos_x as i32;
                            state.grid_cursor_row = pos_y as i32;
                            state.sync_cursor_and_selection();
                        }
                    }
                    changed = true;
                }
            }

            // The second network editor's breadcrumb navigates ITS path.
            {
                use cce_ui::widget::PathController as _;
                if let Some(seg) = state.slots.breadcrumb2.path_click() {
                    if seg < state.current_path2.len() {
                        state.current_path2.truncate(seg);
                        state.sync_nodes();
                        // The viewport tracks its editor's level.
                        if state.viewport_editor() == crate::slots::CONTENT2_IDX {
                            state.rebuild_scene_geometry();
                        }
                        changed = true;
                    }
                }
            }

            if let Some(id) = state.pending_command.take() {
                state.run_command(id);
                changed = true;
            }


            // Check context switcher dropdown changes
            for &widget_idx in &[HEADER_IDX, LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX] {
                if let Some(new_sel) = state.menu_mut(widget_idx).take_context_change() {
                    let target_pane = match new_sel {
                        0 => LEFT_MENUBAR_IDX,
                        1 => RIGHT_MENUBAR_IDX,
                        2 => PARAM_MENUBAR_IDX,
                        3 => SPREADSHEET_MENUBAR_IDX,
                        4 => HEADER_IDX,
                        _ => continue,
                    };
                    if state.focused_pane != target_pane {
                        state.focused_pane = target_pane;
                        // Make sure the switched pane is visible!
                        match target_pane {
                            LEFT_MENUBAR_IDX => {
                                if !state.show_network {
                                    state.show_network = true;
                                    state.slots.content.set_visible(true);
                                    state.slots.left_menubar.set_visible(true);
                                    state.slots.breadcrumb.set_visible(true);
                                    state.menu_mut(HEADER_IDX).set_item_checked(2, 4, true);
                                }
                            }
                            RIGHT_MENUBAR_IDX => {
                                if !state.show_viewport {
                                    state.show_viewport = true;
                                    state.slots.viewport.set_visible(true);
                                    state.slots.right_menubar.set_visible(true);
                                    state.menu_mut(HEADER_IDX).set_item_checked(2, 5, true);
                                }
                            }
                            PARAM_MENUBAR_IDX => {
                                if !state.show_parameters {
                                    state.show_parameters = true;
                                    state.slots.param.set_visible(true);
                                    state.slots.param_menubar.set_visible(true);
                                    state.menu_mut(HEADER_IDX).set_item_checked(2, 6, true);
                                }
                            }
                            SPREADSHEET_MENUBAR_IDX => {
                                if !state.show_spreadsheet {
                                    state.show_spreadsheet = true;
                                    state.slots.spreadsheet.set_visible(true);
                                    state.slots.spreadsheet_menubar.set_visible(true);
                                    state.menu_mut(HEADER_IDX).set_item_checked(2, 7, true);
                                }
                            }
                            _ => {}
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
                }
            }

            // The menubars are not drawn (their bars have no height), so a
            // click reaches these only through MCP's `menu_click`. What is
            // dispatched here is the two menus whose items are chosen by
            // position: the cameras, and the parameter presets. Both run what
            // their commands run. Everything else the menubars list is
            // reached as a command.
            if let Some((menu_idx, item_idx)) = state.menu_mut(RIGHT_MENUBAR_IDX).menu_click() {
                if menu_idx == 0 {
                    if let Some(name) = state.camera_names().get(item_idx).cloned() {
                        changed |= state.choose_camera(&name);
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.menu_mut(PARAM_MENUBAR_IDX).menu_click() {
                // Preset's Default and Reset's All are one thing.
                if menu_idx <= 1 && item_idx == 0 {
                    changed |= state.reset_parameters();
                }
            }

            if changed {
                state.sync_layout();
                state.read_panel_offsets();
                state.sync_cursor_and_selection();

                if state.drag_widget == Some(PARAM_IDX) && state.slots.param.is_dragging() {
                    state.sync_parameters_to_project();
                }

                state.sync_nodes();

                // Sync Parameters pane with selected node — through the one
                // pane-sync path, so textpick rows survive this rebuild.
                state.sync_parameters_pane();

            }

            if changed {
                state.update_window_title();
                if state.syncing_windows() {
                    state.needs_autosave = true;
                }
                result = true;
            }
        }
        result
    }

    /// Apply an automation event (the engine `update` hook). Returns true
    /// when a redraw is needed.
    pub(crate) fn apply_custom_event(&mut self, event: CustomEvent) -> bool {
        let mut needs_redraw = false;
        {
            let state = &mut *self;
            match event {
                CustomEvent::McpCall(call) => {
                    let res = state.apply_mcp_call(&call, &mut needs_redraw);
                    let _ = call.reply.send(res);
                }
                CustomEvent::RunAction(action) => {
                    if let Err(e) = state.apply_action(action, &mut needs_redraw) {
                        state.update_status_text(&e);
                        needs_redraw = true;
                    }
                }
                // Exit is handled by the Application::update wrapper
                // (autosave + engine exit) before this is reached.
                CustomEvent::Exit => {}
            }
        }
        if needs_redraw {
            self.update_window_title();
            if self.syncing_windows() {
                self.needs_autosave = true;
            }
        }
        needs_redraw
    }

    /// Snapshot the project (node tree + view state) for state queries.
    fn project_snapshot(&self) -> Project {
        Project {
            name: "Project".to_string(),
            root: self.fs_root.clone(),
            view_state: self.project_view_state(),
            format: crate::app::PROJECT_FORMAT,
        }
    }

    /// An MCP tool call: `get_state` returns the project snapshot; every
    /// other tool name is an `McpAction` tag — injected into the arguments
    /// and run through the shared action path.
    pub(crate) fn apply_mcp_call(
        &mut self,
        call: &cce_ui::mcp::McpToolCall,
        needs_redraw: &mut bool,
    ) -> Result<serde_json::Value, String> {
        if call.name == "get_state" {
            let mut v = serde_json::to_value(self.project_snapshot())
                .map_err(|e| format!("failed to serialize state: {e}"))?;
            // Additive sibling of the project fields: the playbar is app
            // state, not project state, so it must not enter the Project
            // struct (the save format) — but automation needs to read it.
            let pb = self.slots.playbar.inner();
            v["playbar"] = serde_json::json!({
                "frame": pb.current_frame.round() as i64,
                "playing": pb.playing,
                "reversed": pb.reversed,
                "repeat": pb.repeat,
                "start_frame": pb.start_frame.round() as i64,
                "end_frame": pb.end_frame.round() as i64,
            });
            // The network grid as it is right now, beside what config says
            // it is at 100%: the one way to check, from outside, that a
            // config edit reached the lattice on screen.
            let cfg = crate::app::configured_grid_geometry();
            v["grid"] = serde_json::json!({
                "pitch": [self.grid_pitch_x, self.grid_pitch_y],
                "node_size": [self.node_w, self.node_h],
                "zoom_percent": self.zoom_percent(),
                "configured_pitch": [cfg.pitch_x, cfg.pitch_y],
                "configured_node_size": [cfg.node_w, cfg.node_h],
            });
            // The status line as shown — the load report, a node error, a
            // refused edit. The window may be anywhere, or off-screen; this
            // is how to read it from outside.
            v["status"] = serde_json::Value::String(self.last_status_text.clone());
            return Ok(v);
        }
        let mut req = if call.arguments.is_object() {
            call.arguments.clone()
        } else {
            serde_json::json!({})
        };
        req["action"] = serde_json::Value::String(call.name.clone());
        match serde_json::from_value::<McpAction>(req) {
            Ok(action) => self
                .apply_action(action, needs_redraw)
                .map(serde_json::Value::String),
            Err(e) => Err(format!("invalid arguments for '{}': {e}", call.name)),
        }
    }

    /// Apply one automation action (an MCP tool call, or an app-internal
    /// fire-and-forget `RunAction`).
    pub(crate) fn apply_action(
        &mut self,
        action: McpAction,
        redraw: &mut bool,
    ) -> Result<String, String> {
        let mut needs_redraw = false;
        let state = self;
        let res = match action {
            McpAction::Up => {
                if state.move_up() {
                    // on_path_changed clears the selection but leaves the param
                    // pane to process_window_event's tail, which MCP bypasses.
                    state.sync_parameters_pane();
                    needs_redraw = true;
                    Ok("Moved up".to_string())
                } else {
                    Err("Already at root".to_string())
                }
            }
            McpAction::Enter { slot } => {
                let dir = state.current_dir();
                if slot < dir.children.len() && (dir.children[slot].node_type == "node" || !dir.children[slot].children.is_empty()) {
                    state.current_path.push(slot);
                    state.on_path_changed();
                    state.sync_parameters_pane();
                    needs_redraw = true;
                    Ok("Entered subnet".to_string())
                } else {
                    Err("Not a valid subnet".to_string())
                }
            }
            McpAction::Select { slot } => {
                if slot < state.current_dir().children.len() {
                    // Same effect as clicking the node: it becomes the selected node and
                    // its params populate the parameter pane. A click also reaches
                    // sync_nodes via process_window_event's changed-path, which is
                    // what refreshes the spreadsheet — this path must call it itself.
                    state.graph_mut().set_selected_node(Some(slot));
                    state.sync_parameters_pane();
                    state.sync_nodes();
                    needs_redraw = true;
                    Ok("Node selected".to_string())
                } else {
                    Err("Slot index out of bounds".to_string())
                }
            }
            McpAction::SetParam { slot, name, value } => {
                let dir = state.current_dir_mut();
                if let Some(child) = dir.children.get_mut(slot) {
                    if let Some(p) = child.params.iter_mut().find(|p| p.name == name) {
                        // A value that reads as a reference becomes an
                        // expression, as one typed into the pane does; an
                        // expression is checked when it evaluates. Anything
                        // else must fit the kind, or nothing is written.
                        let as_expr = p.is_expr() || (p.takes_expressions() && crate::expr::looks_like_expression(&value));
                        if !as_expr {
                            if let Err(why) = p.check(&value) {
                                return Err(format!("{name}: {why}"));
                            }
                        }
                        p.set_text(value);
                        if as_expr {
                            p.set_expr(true);
                        }
                        // Same sequence as the interactive param-pane
                        // path, so settings params (viewport flags,
                        // grid) actually take effect via automation.
                        state.sync_grid_settings();
                        state.sync_nodes();
                        state.rebuild_scene_geometry();
                        // Interactively the edit originates IN the param pane;
                        // here it must be pushed back or a selected node's pane
                        // keeps showing the old value.
                        state.sync_parameters_pane();
                        needs_redraw = true;
                        Ok("Parameter updated".to_string())
                    } else {
                        Err(format!("Parameter {} not found", name))
                    }
                } else {
                    Err("Slot index out of bounds".to_string())
                }
            }
            McpAction::ResetCamera => {
                if state.active_camera != "Default Camera" {
                    state.update_active_camera_rotation_reset();
                } else {
                    state.viewport_mut().rotation_y = 0.0;
                    state.viewport_mut().rotation_x = 0.0;
                }
                state.viewport_mut().zoom = 1.0;
                state.viewport_mut().reset_velocity();
                needs_redraw = true;
                Ok("Camera reset".to_string())
            }
            McpAction::Load { path } => {
                if let Err(e) = state.load_from_file(Path::new(&path)) {
                    Err(format!("Load failed: {:?}", e))
                } else {
                    needs_redraw = true;
                    Ok("Project loaded".to_string())
                }
            }
            McpAction::Save { path } => {
                if let Err(e) = state.save_to_file(Path::new(&path)) {
                    Err(format!("Save failed: {:?}", e))
                } else {
                    let path_buf = Path::new(&path).to_path_buf();
                    state.loaded_project_path = Some(path_buf.clone());
                    state.add_recent_file(path_buf);
                    needs_redraw = true;
                    Ok("Project saved".to_string())
                }
            }
            McpAction::ToggleBypass { slot } => {
                match state.current_dir().children.get(slot).map(|n| !n.bypassed) {
                    Some(bypassed) => {
                        state.set_bypassed(&[slot], bypassed);
                        needs_redraw = true;
                        Ok(format!("Bypassed: {}", bypassed))
                    }
                    None => Err("Slot out of bounds".to_string()),
                }
            }
            McpAction::ToggleGeometry { slot } => {
                let active_nodes = state.current_dir().children.len();
                if slot < active_nodes {
                    {
                        let visible = !state.current_dir().children[slot].geometry_visible;
                        state.current_dir_mut().set_child_geometry_visible(slot, visible);
                        state.sync_nodes();
                        state.rebuild_scene_geometry();
                        needs_redraw = true;
                        Ok(format!("Geometry visible: {}", visible))
                    }
                } else {
                    Err("Slot out of bounds".to_string())
                }
            }
            McpAction::AddNode { template_name, name, x, y } => {
                let template_idx = state.node_templates.iter().position(|t| {
                    t.label.to_lowercase() == template_name.to_lowercase()
                        || t.node.name.to_lowercase() == template_name.to_lowercase()
                });
                if let Some(idx) = template_idx {
                    let mut node = state.node_templates[idx].node.clone();
                    // Fresh ids, like paste: a verbatim clone shares the
                    // template's ids across every instance.
                    crate::app::regenerate_node_ids(&mut node);
                    {
                        let (nx, ny) = state.find_empty_cell(x, y, None);
                        node.position = (nx, ny);
                        if let Some(n) = name {
                            node.name = crate::app::sanitize_node_name(&n);
                        } else {
                            node.name = state.get_lowest_unused_name(&node.name);
                        }
                        // New nodes arrive with their display flag OFF: the
                        // one-visible-per-directory rule means showing is an
                        // explicit act ('e', the click toggle), never a side
                        // effect of adding. Top-level flag only — a subnet
                        // template's internal chain keeps its own flags.
                        node.geometry_visible = false;
                        state.current_dir_mut().children.push(node);
                        state.sync_nodes();
                        state.rebuild_positions();
                        state.apply_layout();
                        state.update_panel_bounds();
                        state.rebuild_scene_geometry();
                        needs_redraw = true;
                        Ok("Node added".to_string())
                    }
                } else {
                    Err(format!("Template '{}' not found", template_name))
                }
            }
            McpAction::DeleteNode { slot } => {
                if state.delete_node(slot) {
                    // delete_node clears/shifts the selection; the param pane
                    // resync normally comes from process_window_event's tail.
                    state.sync_parameters_pane();
                    needs_redraw = true;
                    Ok("Node deleted".to_string())
                } else {
                    Err("Slot out of bounds".to_string())
                }
            }
            McpAction::RenameNode { slot, new_name } => {
                let len = state.current_dir().children.len();
                if slot < len {
                    let new_name = crate::app::sanitize_node_name(&new_name);
                    let (id, old_name) = {
                        let n = &state.current_dir().children[slot];
                        (n.id.clone(), n.name.clone())
                    };
                    // Everything that names the node follows it: the wires,
                    // the expressions anywhere in the tree, the active camera.
                    crate::geometry::rename_node_in_tree(&mut state.fs_root, &id, &new_name);
                    if state.active_camera == old_name {
                        state.active_camera = new_name.clone();
                    }
                    state.sync_nodes();
                    // Connections reference nodes by name (Input params), so a
                    // rename changes downstream evaluation.
                    state.rebuild_scene_geometry();
                    needs_redraw = true;
                    Ok("Node renamed".to_string())
                } else {
                    Err("Slot out of bounds".to_string())
                }
            }
            McpAction::MoveNode { slot, x, y } => {
                let len = state.current_dir().children.len();
                if slot < len {
                    let (nx, ny) = state.find_empty_cell(x, y, Some(slot));
                    state.current_dir_mut().children[slot].position = (nx, ny);
                    state.sync_nodes();
                    state.rebuild_positions();
                    state.apply_layout();
                    state.update_panel_bounds();
                    needs_redraw = true;
                    Ok("Node moved".to_string())
                } else {
                    Err("Slot out of bounds".to_string())
                }
            }
            McpAction::AddParam { slot, name, param_type, default } => {
                let len = state.current_dir().children.len();
                if crate::app::ParamKind::parse(&param_type).is_none() {
                    Err(format!(
                        "Unknown param_type '{param_type}'; expected one of: {}",
                        crate::app::ParamKind::NAMES.join(", ")
                    ))
                } else if let Some(why) = ParamDef::new(name.clone(), param_type.clone(), default.clone()).invalid() {
                    Err(format!("{name}: {why}"))
                } else if slot < len {
                    let param = ParamDef::new(name, param_type, default);
                    state.current_dir_mut().children[slot].params.push(param);
                    state.sync_nodes();
                    // Params feed kernel evaluation and the param pane shows
                    // the selected node's list — same rationale as SetParam.
                    state.rebuild_scene_geometry();
                    state.sync_parameters_pane();
                    needs_redraw = true;
                    Ok("Parameter added".to_string())
                } else {
                    Err("Slot out of bounds".to_string())
                }
            }
            McpAction::DeleteParam { slot, name } => {
                let len = state.current_dir().children.len();
                if slot < len {
                    let params = &mut state.current_dir_mut().children[slot].params;
                    if let Some(pos) = params.iter().position(|p| p.name == name) {
                        params.remove(pos);
                        state.sync_nodes();
                        state.rebuild_scene_geometry();
                        state.sync_parameters_pane();
                        needs_redraw = true;
                        Ok("Parameter deleted".to_string())
                    } else {
                        Err(format!("Parameter '{}' not found", name))
                    }
                } else {
                    Err("Slot out of bounds".to_string())
                }
            }
            McpAction::SetPaneCollapsed { pane, collapsed } => {
                let idx = crate::plate_corner::pane_slot_from_name(&pane)
                    .ok_or_else(|| format!("unknown pane: {pane}"))?;
                state.set_pane_collapsed(idx, collapsed);
                needs_redraw = true;
                Ok(format!("{pane} collapsed={collapsed}"))
            }
            McpAction::SetPaneDetached { pane, detached } => {
                let idx = crate::plate_corner::pane_slot_from_name(&pane)
                    .ok_or_else(|| format!("unknown pane: {pane}"))?;
                state.set_pane_detached(idx, detached);
                needs_redraw = true;
                Ok(format!("{pane} detached={}", state.pane_is_detached(idx)))
            }
            McpAction::SetFrame { frame } => {
                let clamped = {
                    let pb = state.slots.playbar.inner_mut();
                    pb.current_frame = frame.clamp(pb.start_frame, pb.end_frame).round();
                    pb.current_frame
                };
                needs_redraw = true;
                Ok(format!("frame={clamped}"))
            }
            McpAction::CurveSetPoints { slot, points } => {
                if points.iter().flatten().any(|c| !c.is_finite()) {
                    return Err("Points must be finite numbers".to_string());
                }
                let dir = state.current_dir_mut();
                let Some(child) = dir.children.get_mut(slot) else {
                    return Err("Slot index out of bounds".to_string());
                };
                if !child.node_type.eq_ignore_ascii_case("curve") {
                    return Err(format!(
                        "Node in slot {slot} is '{}', not a curve",
                        child.node_type
                    ));
                }
                let pts: Vec<glam::Vec3> =
                    points.iter().map(|p| glam::Vec3::new(p[0], p[1], p[2])).collect();
                let Some(p) = child.params.iter_mut().find(|p| p.name == "Points") else {
                    return Err("Curve node has no Points param".to_string());
                };
                p.set_text(crate::geometry::format_curve_points(&pts));
                // Same resync sequence as SetParam / the viewer state.
                state.sync_nodes();
                state.rebuild_scene_geometry();
                state.sync_parameters_pane();
                needs_redraw = true;
                Ok(format!("Curve points set ({})", pts.len()))
            }
            McpAction::ToggleCircularPane => {
                state.circular_network_pane = !state.circular_network_pane;
                let val = state.circular_network_pane;
                state.menu_mut(LEFT_MENUBAR_IDX).set_item_checked(2, 2, val);
                state.rebuild_positions();
                state.apply_layout();
                state.sync_grid_settings();
                needs_redraw = true;
                Ok(format!("Circular pane: {}", state.circular_network_pane))
            }
            McpAction::MenuClick { widget_idx, menu_idx, item_idx } => {
                // Validate before touching menu_mut(): a non-menubar widget_idx
                // panics its MenuBar downcast, and out-of-range menu/item indices
                // used to reply "Menu clicked" while dispatching nowhere.
                let validated: Result<String, String> = if widget_idx >= WIDGET_COUNT {
                    Err(format!("widget_idx {widget_idx} out of range (widget slots: 0..{WIDGET_COUNT})"))
                } else if state.menubar_at(widget_idx).is_some() && !menu_is_dispatched(widget_idx, menu_idx) {
                    // Everything else a menubar lists is a registry command;
                    // a click that was accepted here would dispatch nowhere.
                    Err(format!(
                        "menu {menu_idx} of menubar {widget_idx} is not dispatched by index: use run_command"
                    ))
                } else if let Some(menubar) = state.menubar_at(widget_idx) {
                    match menubar.menu_dropdowns.get(menu_idx) {
                        None => Err(format!(
                            "menu_idx {menu_idx} out of range: menubar {widget_idx} has {} menus",
                            menubar.menu_dropdowns.len()
                        )),
                        // The reply body is interpolated into JSON unescaped, so
                        // keep these messages free of quotes/backslashes.
                        Some(items) => items.get(item_idx).cloned().ok_or_else(|| format!(
                            "item_idx {item_idx} out of range: menu {menu_idx} has {} items: [{}]",
                            items.len(), items.join(", ")
                        )),
                    }
                } else {
                    Err(format!("widget_idx {widget_idx} is not a menubar"))
                };
                match validated {
                    Ok(label) => {
                        state.menu_mut(widget_idx).trigger_menu_click(menu_idx, item_idx);
                        let _ = state.process_window_event(WindowEvent::CursorMoved { position: LocalPosition { x: -9999.0, y: -9999.0 } });
                        needs_redraw = true;
                        Ok(format!("Menu clicked: {label}"))
                    }
                    Err(e) => Err(e),
                }
            }
            McpAction::MenuAction { label } => {
                if state.execute_menu_action(&label) {
                    // The arms relayout themselves but render() draws the last
                    // uploaded buffer (same ritual as ToggleCircularPane).
                    // The interactive menu dispatch follows its pane-show arms
                    // with sync_nodes; execute_menu_action's copies don't, so a
                    // spreadsheet shown here would keep stale contents without
                    // this (the cache makes it a no-op when nothing changed).
                    state.sync_nodes();
                    needs_redraw = true;
                    Ok(format!("Menu action executed: {}", label.replace(['"', '\\'], "'")))
                } else {
                    Err(format!("unknown menu action label: {}", label.replace(['"', '\\'], "'")))
                }
            }
            McpAction::RunCommand { id } => {
                if state.run_command(&id) {
                    state.sync_nodes();
                    needs_redraw = true;
                    Ok(format!("Command run: {}", id.replace(['"', '\\'], "'")))
                } else {
                    Err(format!("unknown command: {}", id.replace(['"', '\\'], "'")))
                }
            }
        };
        if needs_redraw {
            *redraw = true;
        }
        res
    }
}


