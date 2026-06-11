use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clear_ui::widget::{Button, Element};
use crate::app::{State, Project, FsNode, ProjectViewState, param_display, CONTENT_IDX, ParamDef};

impl State {


    pub(crate) fn update_window_title(&self) {
        let base_title = if self.is_detached_network {
            "Network Pane"
        } else {
            "Clear Design Interface"
        };
        
        let mut title = base_title.to_string();
        
        if let Some(path) = &self.loaded_project_path {
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                title.push_str(" - ");
                title.push_str(filename);
            }
        }
        
        if self.has_unsaved_changes() {
            title.push_str("*");
        }
        
        self.window.set_title(&title);
    }

    fn get_recent_files_path() -> Option<std::path::PathBuf> {
        std::env::var("HOME").ok().map(|h| {
            let mut path = std::path::PathBuf::from(h);
            path.push(".config");
            path.push("cce-design-interface");
            path.push("recent_files.json");
            path
        })
    }

    pub(crate) fn load_recent_files() -> Vec<std::path::PathBuf> {
        if let Some(path) = Self::get_recent_files_path() {
            if let Ok(file) = std::fs::File::open(path) {
                if let Ok(list) = serde_json::from_reader::<_, Vec<String>>(file) {
                    return list.into_iter().map(std::path::PathBuf::from).collect();
                }
            }
        }
        Vec::new()
    }

    fn save_recent_files(files: &[std::path::PathBuf]) {
        if let Some(path) = Self::get_recent_files_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(file) = std::fs::File::create(path) {
                let list: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();
                let _ = serde_json::to_writer_pretty(file, &list);
            }
        }
    }

    pub(crate) fn add_recent_file(&mut self, path: std::path::PathBuf) {
        let abs_path = std::fs::canonicalize(&path).unwrap_or(path);
        self.recent_files.retain(|p| p != &abs_path);
        self.recent_files.insert(0, abs_path);
        self.recent_files.truncate(10);
        Self::save_recent_files(&self.recent_files);
        self.rebuild_recent_buttons();
        self.update_paginator();
    }

    pub(crate) fn rebuild_recent_buttons(&mut self) {
        self.recent_files_buttons.clear();
        for file in &self.recent_files {
            let label = file.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| file.to_string_lossy().to_string());
            let btn = Button::new_list_row(0.0, 0.0, 0.0, 0.0).with_label(&label);
            self.recent_files_buttons.push(btn);
        }
    }


    pub(crate) fn save_to_file(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if path.file_name().map_or(false, |n| n == "default_project.json") {
            let proj = Project {
                name: "Default Project".to_string(),
                root: self.fs_root.clone(),
                view_state: ProjectViewState {
                    active_camera: self.active_camera.clone(),
                    pan: (self.pan_x, self.pan_y),
                    current_path: self.current_path.clone(),
                    selected_node: self.graph().selected_node(),
                },
            };
            let content = serde_json::to_string_pretty(&proj)?;
            fs::write(path, content)?;
            self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
            return Ok(());
        }

        let project_dir = path;
        let project_name = project_dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Default Project")
            .to_string();

        fs::create_dir_all(project_dir)?;

        let state_file_path = project_dir.join("state.json");

        let proj = Project {
            name: project_name,
            root: self.fs_root.clone(),
            view_state: ProjectViewState {
                active_camera: self.active_camera.clone(),
                pan: (self.pan_x, self.pan_y),
                current_path: self.current_path.clone(),
                selected_node: self.graph().selected_node(),
            },
        };
        let content = serde_json::to_string_pretty(&proj)?;
        fs::write(&state_file_path, content)?;
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        Ok(())
    }

    pub(crate) fn load_from_file(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        self.text_buffer_cache.clear();
        if path.file_name().map_or(false, |n| n == "default_project.json") {
            let content = fs::read_to_string(path)?;
            let proj: Project = serde_json::from_str(&content)?;
            self.fs_root = proj.root;
            self.ensure_menubar_subnets();
            self.apply_settings_from_menubar_subnets();
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

            let sel = proj.view_state.selected_node;
            self.graph_mut().set_selected_node(sel);
            if sel.is_some() {
                self.focused_widget = Some(CONTENT_IDX);
            } else {
                self.focused_widget = None;
            }
            self.drag_widget = None;
            self.last_click = None;

            self.sync_grid_settings();
            self.sync_nodes();

            let params = if !self.is_detached_network {
                self.graph().selected_node().and_then(|sel_idx| {
                    let dir = self.current_dir();
                    if sel_idx < dir.children.len() {
                        Some(param_display(&dir.children[sel_idx].params))
                    } else { None }
                }).unwrap_or_default()
            } else {
                vec![]
            };
            self.param_mut().set_display_params(&params);

            self.rebuild_scene_geometry();
            self.rebuild_positions();
            self.apply_layout();
            self.update_panel_bounds();
            self.upload_vertices();
            self.loaded_project_path = None;
            self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
            self.update_window_title();
            return Ok(());
        }

        let (state_file_path, project_dir) = if path.is_dir() {
            (path.join("state.json"), path.to_path_buf())
        } else {
            if path.file_name().map_or(false, |name| name == "state.json") {
                (path.to_path_buf(), path.parent().unwrap_or(path).to_path_buf())
            } else {
                (path.to_path_buf(), path.parent().unwrap_or(path).to_path_buf())
            }
        };

        let content = fs::read_to_string(&state_file_path)?;
        let proj: Project = serde_json::from_str(&content)?;
        self.fs_root = proj.root;
        self.ensure_menubar_subnets();
        self.apply_settings_from_menubar_subnets();
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

        let sel = proj.view_state.selected_node;
        self.graph_mut().set_selected_node(sel);
        if sel.is_some() {
            self.focused_widget = Some(CONTENT_IDX);
        } else {
            self.focused_widget = None;
        }
        self.drag_widget = None;
        self.last_click = None;

        self.sync_grid_settings();
        self.sync_nodes();

        // Sync Parameters pane with selected node
        let params = if !self.is_detached_network {
            self.graph().selected_node().and_then(|sel_idx| {
                let dir = self.current_dir();
                if sel_idx < dir.children.len() {
                    Some(param_display(&dir.children[sel_idx].params))
                } else { None }
            }).unwrap_or_default()
        } else {
            vec![]
        };
        self.param_mut().set_display_params(&params);

        self.rebuild_scene_geometry();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.upload_vertices();
        self.loaded_project_path = Some(project_dir);
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        self.update_window_title();
        Ok(())
    }

    pub(crate) fn new_project(&mut self) {
        self.text_buffer_cache.clear();
        self.fs_root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        self.ensure_menubar_subnets();
        self.apply_settings_from_menubar_subnets();
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
        self.loaded_project_path = None;
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        self.update_window_title();
    }

    pub(crate) fn ensure_menubar_subnets(&mut self) {
        let camera_nodes: Vec<String> = self.current_dir().children.iter()
            .filter(|c| c.node_type == "camera")
            .map(|c| c.name.clone())
            .collect();
        let mut camera_options = vec!["Default Camera".to_string()];
        camera_options.extend(camera_nodes);
        let camera_options_refs: Vec<&str> = camera_options.iter().map(|s| s.as_str()).collect();

        fn find_or_create_subnet<'a>(parent: &'a mut FsNode, name: &str, pos: (f32, f32)) -> &'a mut FsNode {
            if let Some(idx) = parent.children.iter().position(|c| c.name == name) {
                let node = &mut parent.children[idx];
                if node.position == (0.0, 0.0) {
                    node.position = pos;
                }
                node
            } else {
                let new_node = FsNode {
                    id: crate::app::generate_node_id(),
                    name: name.to_string(),
                    node_type: "node".to_string(),
                    children: vec![],
                    params: vec![],
                    geometry_visible: true,
                    position: pos,
                };
                parent.children.push(new_node);
                parent.children.last_mut().unwrap()
            }
        }

        fn ensure_param(node: &mut FsNode, name: &str, param_type: &str, default_val: &str, options: &[&str], min: Option<f32>, max: Option<f32>, step: Option<f32>) {
            if !node.params.iter().any(|p| p.name == name) {
                node.params.push(ParamDef {
                    name: name.to_string(),
                    label: name.to_string(),
                    param_type: param_type.to_string(),
                    default: default_val.to_string(),
                    options: options.iter().map(|s| s.to_string()).collect(),
                    min,
                    max,
                    step,
                });
            }
        }

        // 1. Main subnet
        let main_node = find_or_create_subnet(&mut self.fs_root, "Main", (0.0, 0.0));
        
        let file_node = find_or_create_subnet(main_node, "File", (0.0, 0.0));
        ensure_param(file_node, "New Project", "button", "", &[], None, None, None);
        ensure_param(file_node, "Open", "button", "", &[], None, None, None);
        ensure_param(file_node, "Save", "button", "", &[], None, None, None);
        ensure_param(file_node, "Save As", "button", "", &[], None, None, None);
        ensure_param(file_node, "Exit", "button", "", &[], None, None, None);

        let edit_node = find_or_create_subnet(main_node, "Edit", (2.0, 0.0));
        ensure_param(edit_node, "Undo", "button", "", &[], None, None, None);
        ensure_param(edit_node, "Redo", "button", "", &[], None, None, None);

        let view_node = find_or_create_subnet(main_node, "View", (4.0, 0.0));
        ensure_param(view_node, "Zoom In", "button", "", &[], None, None, None);
        ensure_param(view_node, "Zoom Out", "button", "", &[], None, None, None);
        ensure_param(view_node, "Reset Zoom", "button", "", &[], None, None, None);
        ensure_param(view_node, "Detach Circular Window", "button", "", &[], None, None, None);
        ensure_param(view_node, "Show Network Pane", "button", "", &[], None, None, None);
        ensure_param(view_node, "Show Viewport Pane", "button", "", &[], None, None, None);
        ensure_param(view_node, "Show Parameters Pane", "button", "", &[], None, None, None);
        ensure_param(view_node, "Show Spreadsheet Pane", "button", "", &[], None, None, None);

        let help_node = find_or_create_subnet(main_node, "Help", (6.0, 0.0));
        ensure_param(help_node, "About", "button", "", &[], None, None, None);

        // 2. Network subnet
        let net_node = find_or_create_subnet(&mut self.fs_root, "Network", (2.0, 0.0));

        let net_file_node = find_or_create_subnet(net_node, "File", (0.0, 0.0));
        ensure_param(net_file_node, "New", "button", "", &[], None, None, None);
        ensure_param(net_file_node, "Open", "button", "", &[], None, None, None);
        ensure_param(net_file_node, "Save", "button", "", &[], None, None, None);
        ensure_param(net_file_node, "Save As", "button", "", &[], None, None, None);

        let net_edit_node = find_or_create_subnet(net_node, "Edit", (2.0, 0.0));
        ensure_param(net_edit_node, "Undo", "button", "", &[], None, None, None);
        ensure_param(net_edit_node, "Redo", "button", "", &[], None, None, None);

        let net_view_node = find_or_create_subnet(net_node, "View", (4.0, 0.0));
        ensure_param(net_view_node, "Zoom In", "button", "", &[], None, None, None);
        ensure_param(net_view_node, "Zoom Out", "button", "", &[], None, None, None);
        ensure_param(net_view_node, "Circular Pane", "choice", if self.circular_network_pane { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(net_view_node, "Detach Pane", "button", "", &[], None, None, None);
        ensure_param(net_view_node, "Close Pane", "button", "", &[], None, None, None);

        let net_settings_node = find_or_create_subnet(net_node, "Settings", (6.0, 0.0));
        ensure_param(net_settings_node, "Snap to Grid", "choice", if self.grid_snap_enabled { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(net_settings_node, "Grid Visible", "choice", if self.network_grid_visible { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(net_settings_node, "Uniform Background", "choice", if self.uniform_background { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(net_settings_node, "Opacity", "slider", &format!("{:.2}", self.network_opacity), &[], Some(0.0), Some(1.0), None);
        ensure_param(net_settings_node, "Node Color R", "spinbox", &((self.node_color[0] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_settings_node, "Node Color G", "spinbox", &((self.node_color[1] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_settings_node, "Node Color B", "spinbox", &((self.node_color[2] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_settings_node, "Cell Color R", "spinbox", &((self.cell_color[0] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_settings_node, "Cell Color G", "spinbox", &((self.cell_color[1] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_settings_node, "Cell Color B", "spinbox", &((self.cell_color[2] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_settings_node, "Gap Color R", "spinbox", &((self.gap_color[0] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_settings_node, "Gap Color G", "spinbox", &((self.gap_color[1] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_settings_node, "Gap Color B", "spinbox", &((self.gap_color[2] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));

        // 3. Viewport subnet
        let vp_node = find_or_create_subnet(&mut self.fs_root, "Viewport", (4.0, 0.0));

        // Camera node - dynamically build options
        let camera_node = find_or_create_subnet(vp_node, "Camera", (0.0, 0.0));
        if let Some(p) = camera_node.params.iter_mut().find(|p| p.name == "Active Camera") {
            p.options = camera_options.clone();
            if !p.options.contains(&p.default) {
                p.default = "Default Camera".to_string();
            }
        } else {
            ensure_param(camera_node, "Active Camera", "choice", &self.active_camera, &camera_options_refs, None, None, None);
        }

        let display_node = find_or_create_subnet(vp_node, "Display", (2.0, 0.0));
        ensure_param(display_node, "Square Aspect", "choice", if self.square_viewport { "true" } else { "false" }, &["false", "true"], None, None, None);

        let guides_node = find_or_create_subnet(vp_node, "Guides", (4.0, 0.0));
        ensure_param(guides_node, "Show Grid", "choice", if self.show_grid { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(guides_node, "Cube", "choice", if self.show_cube { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(guides_node, "Origin", "choice", if self.show_origin { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(guides_node, "Camera Pivot", "choice", if self.show_camera_pivot { "true" } else { "false" }, &["false", "true"], None, None, None);

        let vp_view_node = find_or_create_subnet(vp_node, "View", (6.0, 0.0));
        ensure_param(vp_view_node, "Close Pane", "button", "", &[], None, None, None);

        let vp_settings_node = find_or_create_subnet(vp_node, "Settings", (8.0, 0.0));
        ensure_param(vp_settings_node, "Show Grid Guide", "choice", if self.show_grid { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_settings_node, "Show Reference Cube", "choice", if self.show_cube { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_settings_node, "Show Origin Axes", "choice", if self.show_origin { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_settings_node, "Show Camera Pivot", "choice", if self.show_camera_pivot { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_settings_node, "Grid Thickness", "spinbox", &((self.grid_thickness * 1000.0) as i32).to_string(), &[], Some(2.0), Some(200.0), Some(1.0));
        ensure_param(vp_settings_node, "Origin Guide Size", "spinbox", &((self.origin_size * 10.0) as i32).to_string(), &[], Some(1.0), Some(50.0), Some(1.0));
        ensure_param(vp_settings_node, "Camera Pivot Size", "spinbox", &((self.camera_pivot_size * 10.0) as i32).to_string(), &[], Some(1.0), Some(50.0), Some(1.0));
        ensure_param(vp_settings_node, "BG Color R", "spinbox", &((self.viewport_bg_color[0] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(vp_settings_node, "BG Color G", "spinbox", &((self.viewport_bg_color[1] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(vp_settings_node, "BG Color B", "spinbox", &((self.viewport_bg_color[2] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(vp_settings_node, "Grid Color R", "spinbox", &((self.grid_color[0] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(vp_settings_node, "Grid Color G", "spinbox", &((self.grid_color[1] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(vp_settings_node, "Grid Color B", "spinbox", &((self.grid_color[2] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));

        // 4. Parameters subnet
        let param_node = find_or_create_subnet(&mut self.fs_root, "Parameters", (6.0, 0.0));

        let preset_node = find_or_create_subnet(param_node, "Preset", (0.0, 0.0));
        ensure_param(preset_node, "Default", "button", "", &[], None, None, None);
        ensure_param(preset_node, "Custom", "button", "", &[], None, None, None);

        let reset_node = find_or_create_subnet(param_node, "Reset", (2.0, 0.0));
        ensure_param(reset_node, "All", "button", "", &[], None, None, None);

        let param_view_node = find_or_create_subnet(param_node, "View", (4.0, 0.0));
        ensure_param(param_view_node, "Close Pane", "button", "", &[], None, None, None);

        // 5. Spreadsheet subnet
        let ss_node = find_or_create_subnet(&mut self.fs_root, "Spreadsheet", (8.0, 0.0));

        let ss_view_node = find_or_create_subnet(ss_node, "View", (0.0, 0.0));
        ensure_param(ss_view_node, "Close Pane", "button", "", &[], None, None, None);
    }

    pub(crate) fn apply_settings_from_menubar_subnets(&mut self) {
        // Read Settings from Network subnet
        if let Some(net_idx) = self.fs_root.children.iter().position(|c| c.name == "Network") {
            let subnet = &self.fs_root.children[net_idx];
            if let Some(settings_idx) = subnet.children.iter().position(|c| c.name == "Settings") {
                let node = &subnet.children[settings_idx];
                for p in &node.params {
                    match p.name.as_str() {
                        "Snap to Grid" => if let Ok(val) = p.default.parse::<bool>() { self.grid_snap_enabled = val; }
                        "Grid Visible" => if let Ok(val) = p.default.parse::<bool>() { self.network_grid_visible = val; }
                        "Uniform Background" => if let Ok(val) = p.default.parse::<bool>() { self.uniform_background = val; }
                        "Opacity" => if let Ok(val) = p.default.parse::<f32>() { self.network_opacity = val; }
                        "Node Color R" => if let Ok(val) = p.default.parse::<f32>() { self.node_color[0] = val / 255.0; }
                        "Node Color G" => if let Ok(val) = p.default.parse::<f32>() { self.node_color[1] = val / 255.0; }
                        "Node Color B" => if let Ok(val) = p.default.parse::<f32>() { self.node_color[2] = val / 255.0; }
                        "Cell Color R" => if let Ok(val) = p.default.parse::<f32>() { self.cell_color[0] = val / 255.0; }
                        "Cell Color G" => if let Ok(val) = p.default.parse::<f32>() { self.cell_color[1] = val / 255.0; }
                        "Cell Color B" => if let Ok(val) = p.default.parse::<f32>() { self.cell_color[2] = val / 255.0; }
                        "Gap Color R" => if let Ok(val) = p.default.parse::<f32>() { self.gap_color[0] = val / 255.0; }
                        "Gap Color G" => if let Ok(val) = p.default.parse::<f32>() { self.gap_color[1] = val / 255.0; }
                        "Gap Color B" => if let Ok(val) = p.default.parse::<f32>() { self.gap_color[2] = val / 255.0; }
                        _ => {}
                    }
                }
            }
            if let Some(view_idx) = subnet.children.iter().position(|c| c.name == "View") {
                let node = &subnet.children[view_idx];
                for p in &node.params {
                    match p.name.as_str() {
                        "Circular Pane" => if let Ok(val) = p.default.parse::<bool>() { self.circular_network_pane = val; }
                        _ => {}
                    }
                }
            }
        }

        // Read Settings from Viewport subnet
        if let Some(vp_idx) = self.fs_root.children.iter().position(|c| c.name == "Viewport") {
            let subnet = &self.fs_root.children[vp_idx];
            if let Some(settings_idx) = subnet.children.iter().position(|c| c.name == "Settings") {
                let node = &subnet.children[settings_idx];
                for p in &node.params {
                    match p.name.as_str() {
                        "Show Grid Guide" => if let Ok(val) = p.default.parse::<bool>() { self.show_grid = val; }
                        "Show Reference Cube" => if let Ok(val) = p.default.parse::<bool>() { self.show_cube = val; }
                        "Show Origin Axes" => if let Ok(val) = p.default.parse::<bool>() { self.show_origin = val; }
                        "Show Camera Pivot" => if let Ok(val) = p.default.parse::<bool>() { self.show_camera_pivot = val; }
                        "Grid Thickness" => if let Ok(val) = p.default.parse::<f32>() { self.grid_thickness = val / 1000.0; }
                        "Origin Guide Size" => if let Ok(val) = p.default.parse::<f32>() { self.origin_size = val / 10.0; }
                        "Camera Pivot Size" => if let Ok(val) = p.default.parse::<f32>() { self.camera_pivot_size = val / 10.0; }
                        "BG Color R" => if let Ok(val) = p.default.parse::<f32>() { self.viewport_bg_color[0] = val / 255.0; }
                        "BG Color G" => if let Ok(val) = p.default.parse::<f32>() { self.viewport_bg_color[1] = val / 255.0; }
                        "BG Color B" => if let Ok(val) = p.default.parse::<f32>() { self.viewport_bg_color[2] = val / 255.0; }
                        "Grid Color R" => if let Ok(val) = p.default.parse::<f32>() { self.grid_color[0] = val / 255.0; }
                        "Grid Color G" => if let Ok(val) = p.default.parse::<f32>() { self.grid_color[1] = val / 255.0; }
                        "Grid Color B" => if let Ok(val) = p.default.parse::<f32>() { self.grid_color[2] = val / 255.0; }
                        _ => {}
                    }
                }
            }
            if let Some(disp_idx) = subnet.children.iter().position(|c| c.name == "Display") {
                let node = &subnet.children[disp_idx];
                for p in &node.params {
                    match p.name.as_str() {
                        "Square Aspect" => if let Ok(val) = p.default.parse::<bool>() { self.square_viewport = val; }
                        _ => {}
                    }
                }
            }
            if let Some(guides_idx) = subnet.children.iter().position(|c| c.name == "Guides") {
                let node = &subnet.children[guides_idx];
                for p in &node.params {
                    match p.name.as_str() {
                        "Show Grid" => if let Ok(val) = p.default.parse::<bool>() { self.show_grid = val; }
                        "Cube" => if let Ok(val) = p.default.parse::<bool>() { self.show_cube = val; }
                        "Origin" => if let Ok(val) = p.default.parse::<bool>() { self.show_origin = val; }
                        "Camera Pivot" => if let Ok(val) = p.default.parse::<bool>() { self.show_camera_pivot = val; }
                        _ => {}
                    }
                }
            }
            if let Some(camera_idx) = subnet.children.iter().position(|c| c.name == "Camera") {
                let node = &subnet.children[camera_idx];
                for p in &node.params {
                    match p.name.as_str() {
                        "Active Camera" => self.active_camera = p.default.clone(),
                        _ => {}
                    }
                }
            }
        }
    }
}
