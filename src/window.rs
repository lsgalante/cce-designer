//! The designer's window-event layer on the cce-ui engine.
//!
//! The Wayland plumbing (seat/pointer/keyboard handlers, configure, CSD)
//! lives in cce-ui's window runner; the `Application` impl (application.rs)
//! translates the runner's hooks into [`WindowEvent`]s. What remains here is
//! app policy: the post-event side-effect pass (`process_window_event`) and
//! MCP-action application (`apply_custom_event`).

use std::path::Path;

use cce_ui::widget::WidgetHost;
use crate::shortcut::Action;
use crate::app::{State, CustomEvent, McpAction, TouchPhase, LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX, HEADER_IDX, PARAM_IDX, WIDGET_COUNT, get_next_visible_pane, Project, ProjectViewState, ParamDef, param_display};

#[derive(Debug, Clone, Copy)]
pub struct LocalPosition {
    pub x: f64,
    pub y: f64,
}

pub enum WindowEvent {
    MouseWheel { delta: cce_ui::widget::MouseScrollDelta, phase: TouchPhase },
    CursorMoved { position: LocalPosition },
    MouseInput { state: cce_ui::widget::ElementState, button: cce_ui::widget::MouseButton },
    KeyboardInput { event: cce_ui::widget::KeyEvent },
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

            if let Some(action) = state.pending_action.take() {
                state.execute_action(action);
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

            if let Some((menu_idx, item_idx)) = state.menu_mut(HEADER_IDX).menu_click() {
                if menu_idx == 0 { // File
                    match item_idx {
                        0 => { // New Project
                            state.new_project();
                            changed = true;
                        }
                        1 => { // Open
                            state.open_file_chooser();
                            changed = true;
                        }
                        2 => { // Save
                            let path_opt = state.loaded_project_path.clone();
                            if let Some(path) = path_opt {
                                if let Err(e) = state.save_to_file(&path) {
                                    eprintln!("Failed to save project: {:?}", e);
                                    state.update_status_text(&format!("Failed to save: {:?}", e));
                                } else {
                                    state.update_status_text(&format!("Project saved to {}", path.display()));
                                    state.add_recent_file(path);
                                }
                            } else {
                                state.save_file_chooser();
                            }
                            changed = true;
                        }
                        3 => { // Save As
                            state.save_file_chooser();
                            changed = true;
                        }
                        4 => { // Exit
                            state.exit_requested = true;
                        }
                        _ => {}
                    }
                } else if menu_idx == 2 { // View
                    match item_idx {
                        0 => { // Zoom In
                            state.zoom(1.15, None);
                            changed = true;
                        }
                        1 => { // Zoom Out
                            state.zoom(1.0 / 1.15, None);
                            changed = true;
                        }
                        2 => { // Reset Zoom
                            state.grid_size_x = 150.0;
                            state.grid_size_y = 75.0;
                            state.skipped_col_w = 37.5;
                            state.skipped_row_h = 37.5;
                            state.sync_grid_settings();
                            changed = true;
                        }
                        3 => { // Detach Circular Window
                            state.execute_action(Action::DetachCircularWindow);
                            changed = true;
                        }
                        4 => { // Show Network Pane
                            state.show_network = !state.show_network;
                            state.slots.content.set_visible(state.show_network);
                            state.slots.left_menubar.set_visible(state.show_network);
                            state.slots.breadcrumb.set_visible(state.show_network);
                            let val = state.show_network;
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 4, val);
                            if !state.show_network && state.focused_pane == LEFT_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        5 => { // Show Viewport Pane
                            state.show_viewport = !state.show_viewport;
                            state.slots.viewport.set_visible(state.show_viewport);
                            state.slots.right_menubar.set_visible(state.show_viewport);
                            let val = state.show_viewport;
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 5, val);
                            if !state.show_viewport && state.focused_pane == RIGHT_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        6 => { // Show Parameters Pane
                            state.show_parameters = !state.show_parameters;
                            state.slots.param.set_visible(state.show_parameters);
                            state.slots.param_menubar.set_visible(state.show_parameters);
                            let val = state.show_parameters;
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 6, val);
                            if !state.show_parameters && state.focused_pane == PARAM_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        7 => { // Show Spreadsheet Pane
                            state.show_spreadsheet = !state.show_spreadsheet;
                            state.slots.spreadsheet.set_visible(state.show_spreadsheet);
                            state.slots.spreadsheet_menubar.set_visible(state.show_spreadsheet);
                            let val = state.show_spreadsheet;
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 7, val);
                            if !state.show_spreadsheet && state.focused_pane == SPREADSHEET_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.menu_mut(LEFT_MENUBAR_IDX).menu_click() {
                if menu_idx == 0 { // File
                    match item_idx {
                        0 => { // New
                            state.new_project();
                            changed = true;
                        }
                        1 => { // Open
                            state.open_file_chooser();
                            changed = true;
                        }
                        2 => { // Save
                            let path_opt = state.loaded_project_path.clone();
                            if let Some(path) = path_opt {
                                if let Err(e) = state.save_to_file(&path) {
                                    eprintln!("Failed to save project: {:?}", e);
                                    state.update_status_text(&format!("Failed to save: {:?}", e));
                                } else {
                                    state.update_status_text(&format!("Project saved to {}", path.display()));
                                    state.add_recent_file(path);
                                }
                            } else {
                                state.save_file_chooser();
                            }
                            changed = true;
                        }
                        3 => { // Save As
                            state.save_file_chooser();
                            changed = true;
                        }
                        _ => {}
                    }
                } else if menu_idx == 2 { // View
                    match item_idx {
                        0 => { // Zoom In
                            state.zoom(1.15, None);
                            changed = true;
                        }
                        1 => { // Zoom Out
                            state.zoom(1.0 / 1.15, None);
                            changed = true;
                        }
                        2 => {
                            state.circular_network_pane = !state.circular_network_pane;
                            let val = state.circular_network_pane;
                            state.menu_mut(LEFT_MENUBAR_IDX).set_item_checked(2, 2, val);
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_grid_settings();
                            changed = true;
                        }
                        3 => { // Detach Pane
                            state.execute_action(Action::DetachCircularWindow);
                            changed = true;
                        }
                        4 => { // Close Pane
                            state.show_network = false;
                            state.slots.content.set_visible(false);
                            state.slots.left_menubar.set_visible(false);
                            state.slots.breadcrumb.set_visible(false);
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 4, false);
                            if state.focused_pane == LEFT_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.menu_mut(RIGHT_MENUBAR_IDX).menu_click() {
                if menu_idx == 0 {
                    let camera_nodes: Vec<String> = state.current_dir().children.iter()
                        .filter(|c| c.node_type == "camera")
                        .map(|c| c.name.clone())
                        .collect();
                    let mut items = vec!["Default Camera".to_string()];
                    items.extend(camera_nodes);
                    if item_idx < items.len() {
                        state.active_camera = items[item_idx].clone();
                        let active_cam = state.active_camera.clone();
                        for (i, item) in items.iter().enumerate() {
                            state.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(0, i, item == &active_cam);
                        }
                        changed = true;
                    }
                } else if menu_idx == 3 { // View
                    if item_idx == 0 { // Close Pane
                        state.show_viewport = false;
                        state.slots.viewport.set_visible(false);
                        state.slots.right_menubar.set_visible(false);
                        state.menu_mut(HEADER_IDX).set_item_checked(2, 5, false);
                        if state.focused_pane == RIGHT_MENUBAR_IDX {
                            state.focused_pane = get_next_visible_pane(
                                state.focused_pane,
                                state.show_network,
                                state.show_viewport,
                                state.show_parameters,
                                state.show_spreadsheet,
                                false,
                            );
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
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

            if let Some((menu_idx, item_idx)) = state.menu_mut(PARAM_MENUBAR_IDX).menu_click() {
                if menu_idx == 0 { // Preset
                    if let Some(slot_idx) = state.graph().selected_node() {
                        let node_type = state.current_dir().children[slot_idx].node_type.clone();
                        let template_params = state.node_templates.iter()
                            .find(|t| t.node.node_type == node_type)
                            .map(|t| t.node.params.clone());
                        if let Some(params_to_reset) = template_params {
                            if item_idx == 0 { // Default
                                for template_param in &params_to_reset {
                                    if let Some(p) = state.current_dir_mut().children[slot_idx].params.iter_mut().find(|p| p.name == template_param.name) {
                                        p.default = template_param.default.clone();
                                    }
                                }
                            } else if item_idx == 1 { // Custom
                                for template_param in &params_to_reset {
                                    if let Some(p) = state.current_dir_mut().children[slot_idx].params.iter_mut().find(|p| p.name == template_param.name) {
                                        if let Ok(v) = template_param.default.parse::<f32>() {
                                            p.default = format!("{:.2}", v * 1.5);
                                        } else if let Ok(v) = template_param.default.parse::<i32>() {
                                            p.default = format!("{}", v * 2);
                                        } else if template_param.default.contains(':') {
                                            let parts: Vec<&str> = template_param.default.split(':').collect();
                                            let custom_parts: Vec<String> = parts.iter().map(|p_str| {
                                                if let Ok(v) = p_str.parse::<f32>() {
                                                    format!("{:.2}", v * 1.5)
                                                } else {
                                                    p_str.to_string()
                                                }
                                            }).collect();
                                            p.default = custom_parts.join(":");
                                        } else {
                                            p.default = template_param.default.clone();
                                        }
                                    }
                                }
                            }
                            state.sync_nodes();
                            state.rebuild_scene_geometry();
                            changed = true;
                        }
                    }
                } else if menu_idx == 1 { // Reset
                    if item_idx == 0 { // All
                        if let Some(slot_idx) = state.graph().selected_node() {
                            let node_type = state.current_dir().children[slot_idx].node_type.clone();
                            let template_params = state.node_templates.iter()
                                .find(|t| t.node.node_type == node_type)
                                .map(|t| t.node.params.clone());
                            if let Some(params_to_reset) = template_params {
                                for template_param in &params_to_reset {
                                    if let Some(p) = state.current_dir_mut().children[slot_idx].params.iter_mut().find(|p| p.name == template_param.name) {
                                        p.default = template_param.default.clone();
                                    }
                                }
                                state.sync_nodes();
                                state.rebuild_scene_geometry();
                                changed = true;
                            }
                        }
                    }
                } else if menu_idx == 2 { // View
                    if item_idx == 0 { // Close Pane
                        state.show_parameters = false;
                        state.slots.param.set_visible(false);
                        state.slots.param_menubar.set_visible(false);
                        state.menu_mut(HEADER_IDX).set_item_checked(2, 6, false);
                        if state.focused_pane == PARAM_MENUBAR_IDX {
                            state.focused_pane = get_next_visible_pane(
                                state.focused_pane,
                                state.show_network,
                                state.show_viewport,
                                state.show_parameters,
                                state.show_spreadsheet,
                                false,
                            );
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.menu_mut(SPREADSHEET_MENUBAR_IDX).menu_click() {
                if menu_idx == 0 { // View
                    if item_idx == 0 { // Close Pane
                        state.show_spreadsheet = false;
                        state.slots.spreadsheet.set_visible(false);
                        state.slots.spreadsheet_menubar.set_visible(false);
                        state.menu_mut(HEADER_IDX).set_item_checked(2, 7, false);
                        if state.focused_pane == SPREADSHEET_MENUBAR_IDX {
                            state.focused_pane = get_next_visible_pane(
                                state.focused_pane,
                                state.show_network,
                                state.show_viewport,
                                state.show_parameters,
                                state.show_spreadsheet,
                                false,
                            );
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
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

                // Sync Parameters pane with selected node
                let params = if !state.is_detached_network {
                    state.graph().selected_node().and_then(|sel_idx| {
                        let dir = state.current_dir();
                        if sel_idx < dir.children.len() {
                            Some(param_display(&dir.children[sel_idx].params))
                        } else { None }
                    }).unwrap_or_default()
                } else {
                    vec![]
                };
                state.param_mut().set_display_params(&params);

            }

            state.update_status_text(&format!(
                "col: {:.0}  vp: {:.0}  params: {:.0}",
                state.content_left_w(), state.viewport_w(), state.param_w(),
            ));

            if changed {
                state.update_window_title();
                if state.is_detached_network || state.detached_circular_network {
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
                        eprintln!("Action failed: {e}");
                    }
                }
                // Exit is handled by the Application::update wrapper
                // (autosave + engine exit) before this is reached.
                CustomEvent::Exit => {}
            }
        }
        if needs_redraw {
            self.update_window_title();
            if self.is_detached_network || self.detached_circular_network {
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
            view_state: ProjectViewState {
                active_camera: self.active_camera.clone(),
                pan: (self.pan_x, self.pan_y),
                current_path: self.current_path.clone(),
                selected_node: self.graph().selected_node(),
            },
        }
    }

    /// An MCP tool call: `get_state` returns the project snapshot; every
    /// other tool name is an `McpAction` tag — injected into the arguments
    /// and run through the shared action path.
    fn apply_mcp_call(
        &mut self,
        call: &cce_ui::mcp::McpToolCall,
        needs_redraw: &mut bool,
    ) -> Result<serde_json::Value, String> {
        if call.name == "get_state" {
            return serde_json::to_value(self.project_snapshot())
                .map_err(|e| format!("failed to serialize state: {e}"));
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
                    needs_redraw = true;
                    Ok("Moved up".to_string())
                } else {
                    Err("Already at root".to_string())
                }
            }
            McpAction::Enter { slot } => {
                let dir = state.current_dir();
                if slot < dir.children.len() && (dir.children[slot].node_type == "node" || dir.children[slot].node_type == "utility" || !dir.children[slot].children.is_empty()) {
                    state.current_path.push(slot);
                    state.on_path_changed();
                    needs_redraw = true;
                    Ok("Entered subnet".to_string())
                } else {
                    Err("Not a valid subnet".to_string())
                }
            }
            McpAction::SetParam { slot, name, value } => {
                let dir = state.current_dir_mut();
                if let Some(child) = dir.children.get_mut(slot) {
                    if let Some(p) = child.params.iter_mut().find(|p| p.name == name) {
                        p.default = value;
                        // Same sequence as the interactive param-pane
                        // path, so settings params (viewport flags,
                        // grid) actually take effect via automation.
                        state.apply_settings_from_menubar_subnets();
                        state.sync_grid_settings();
                        state.sync_nodes();
                        state.rebuild_scene_geometry();
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
            McpAction::ToggleGeometry { slot } => {
                let active_nodes = state.current_dir().children.len();
                if slot < active_nodes {
                    if state.current_dir().children[slot].node_type == "utility" {
                        Err("Cannot toggle geometry visibility on utility nodes".to_string())
                    } else {
                        let visible = !state.current_dir().children[slot].geometry_visible;
                        state.current_dir_mut().children[slot].geometry_visible = visible;
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
                    let mut allowed = true;
                    let is_in_utility = !state.current_path.is_empty() && state.fs_root.children[state.current_path[0]].node_type == "utility";
                    if is_in_utility {
                        if crate::geometry::is_geometry_node_type(&node.node_type) {
                            allowed = false;
                        }
                    }
                    if !allowed {
                        Err("Utility nodes cannot contain geometry.".to_string())
                    } else {
                        let (nx, ny) = state.find_empty_cell(x, y, None);
                        node.position = (nx, ny);
                        if let Some(n) = name {
                            node.name = n;
                        } else {
                            node.name = state.get_lowest_unused_name(&node.name);
                        }
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
                    needs_redraw = true;
                    Ok("Node deleted".to_string())
                } else {
                    Err("Slot out of bounds".to_string())
                }
            }
            McpAction::RenameNode { slot, new_name } => {
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
                if slot < len {
                    let param = ParamDef {
                        name,
                        label: String::new(),
                        param_type,
                        default,
                        options: vec![],
                        min: None,
                        max: None,
                        step: None,
                    };
                    state.current_dir_mut().children[slot].params.push(param);
                    state.sync_nodes();
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
                        needs_redraw = true;
                        Ok("Parameter deleted".to_string())
                    } else {
                        Err(format!("Parameter '{}' not found", name))
                    }
                } else {
                    Err("Slot out of bounds".to_string())
                }
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
                // used to reply "Menu clicked" while dispatching nowhere. NB the
                // pane-toggle items ("Show Spreadsheet Pane", ...) are NOT in these
                // menubars — they are toggle params in the menu pane, drained by
                // sync_parameters_to_project's label match, unreachable from here.
                let validated: Result<String, String> = if widget_idx >= WIDGET_COUNT {
                    Err(format!("widget_idx {widget_idx} out of range (widget slots: 0..{WIDGET_COUNT})"))
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
                    needs_redraw = true;
                    Ok(format!("Menu action executed: {}", label.replace(['"', '\\'], "'")))
                } else {
                    Err(format!("unknown menu action label: {}", label.replace(['"', '\\'], "'")))
                }
            }
            McpAction::MenuClosed { widget_idx, menu_idx } => {
                if state.active_menu_cloud_idx == Some((widget_idx, menu_idx)) {
                    state.active_menu_cloud_pid = None;
                    state.active_menu_cloud_idx = None;
                }
                Ok("Menu closed".to_string())
            }

        };
        if needs_redraw {
            *redraw = true;
        }
        res
    }
}


