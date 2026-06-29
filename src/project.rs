use std::fs;
use std::path::Path;

use cce_ui::widget::Button;
use crate::app::{State, Project, FsNode, ProjectViewState, CONTENT_IDX, ParamDef};

fn color_to_hex(rgb: [f32; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}",
        (rgb[0] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb[1] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb[2] * 255.0).round().clamp(0.0, 255.0) as u8
    )
}

fn hex_to_color(hex: &str) -> Option<[f32; 3]> {
    let s = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&s[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&s[4..6], 16).ok()? as f32 / 255.0;
        Some([r, g, b])
    } else {
        None
    }
}

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
            path.push("cce");
            path.push("cce-designer");
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
        self.ensure_menubar_subnets();
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
            self.sync_cursor_and_selection_from_loaded();
            self.sync_cursor_and_selection();
            self.sync_parameters_pane();

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
        self.sync_cursor_and_selection_from_loaded();
        self.sync_cursor_and_selection();
        self.add_recent_file(project_dir.clone());
        self.sync_parameters_pane();

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
            inputs: 0,
            outputs: 0,
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
        self.sync_cursor_and_selection();
        self.sync_parameters_pane();
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

        let mut recent_options = vec!["- Select -".to_string()];
        for path in &self.recent_files {
            recent_options.push(path.to_string_lossy().to_string());
        }
        let recent_options_refs: Vec<&str> = recent_options.iter().map(|s| s.as_str()).collect();

        fn find_or_create_subnet<'a>(parent: &'a mut FsNode, name: &str, node_type: &str, pos: (f32, f32)) -> &'a mut FsNode {
            if let Some(idx) = parent.children.iter().position(|c| c.name == name) {
                let node = &mut parent.children[idx];
                node.node_type = node_type.to_string();
                if node.position == (0.0, 0.0) {
                    node.position = pos;
                }
                node
            } else {
                let new_node = FsNode {
                    id: crate::app::generate_node_id(),
                    name: name.to_string(),
                    node_type: node_type.to_string(),
                    children: vec![],
                    params: vec![],
                    geometry_visible: true,
                    position: pos,
                    inputs: 1,
                    outputs: 1,
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
        let main_node = find_or_create_subnet(&mut self.fs_root, "Main", "utility", (0.0, 0.0));
        main_node.children.clear();

        ensure_param(main_node, "File", "section", "", &[], None, None, None);
        ensure_param(main_node, "New Project", "button", "", &[], None, None, None);
        ensure_param(main_node, "Open", "button", "", &[], None, None, None);

        if let Some(p) = main_node.params.iter_mut().find(|p| p.name == "Open Recent") {
            p.options = recent_options.clone();
            if !p.options.contains(&p.default) {
                p.default = "- Select -".to_string();
            }
        } else {
            ensure_param(main_node, "Open Recent", "choice", "- Select -", &recent_options_refs, None, None, None);
        }

        ensure_param(main_node, "Save", "button", "", &[], None, None, None);
        ensure_param(main_node, "Save As", "button", "", &[], None, None, None);
        ensure_param(main_node, "Exit", "button", "", &[], None, None, None);

        ensure_param(main_node, "Edit", "section", "", &[], None, None, None);
        ensure_param(main_node, "Undo", "button", "", &[], None, None, None);
        ensure_param(main_node, "Redo", "button", "", &[], None, None, None);

        ensure_param(main_node, "View", "section", "", &[], None, None, None);
        ensure_param(main_node, "Zoom In", "button", "", &[], None, None, None);
        ensure_param(main_node, "Zoom Out", "button", "", &[], None, None, None);
        ensure_param(main_node, "Reset Zoom", "button", "", &[], None, None, None);
        ensure_param(main_node, "Detach Circular Window", "button", "", &[], None, None, None);
        ensure_param(main_node, "Show Network Pane", "button", "", &[], None, None, None);
        ensure_param(main_node, "Show Viewport Pane", "button", "", &[], None, None, None);
        ensure_param(main_node, "Show Parameters Pane", "button", "", &[], None, None, None);
        ensure_param(main_node, "Show Spreadsheet Pane", "button", "", &[], None, None, None);

        ensure_param(main_node, "Help", "section", "", &[], None, None, None);
        ensure_param(main_node, "About", "button", "", &[], None, None, None);

        // 2. Network subnet
        let net_node = find_or_create_subnet(&mut self.fs_root, "Network", "utility", (2.0, 0.0));
        net_node.children.clear();

        ensure_param(net_node, "File", "section", "", &[], None, None, None);
        ensure_param(net_node, "New", "button", "", &[], None, None, None);
        ensure_param(net_node, "Open", "button", "", &[], None, None, None);
        ensure_param(net_node, "Save", "button", "", &[], None, None, None);
        ensure_param(net_node, "Save As", "button", "", &[], None, None, None);

        ensure_param(net_node, "Edit", "section", "", &[], None, None, None);
        ensure_param(net_node, "Undo", "button", "", &[], None, None, None);
        ensure_param(net_node, "Redo", "button", "", &[], None, None, None);

        ensure_param(net_node, "View", "section", "", &[], None, None, None);
        ensure_param(net_node, "Zoom In", "button", "", &[], None, None, None);
        ensure_param(net_node, "Zoom Out", "button", "", &[], None, None, None);
        ensure_param(net_node, "Circular Pane", "choice", if self.circular_network_pane { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(net_node, "Detach Pane", "button", "", &[], None, None, None);
        ensure_param(net_node, "Close Pane", "button", "", &[], None, None, None);

        ensure_param(net_node, "Settings", "section", "", &[], None, None, None);
        ensure_param(net_node, "Node Color R", "spinbox", &((self.node_color[0] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_node, "Node Color G", "spinbox", &((self.node_color[1] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));
        ensure_param(net_node, "Node Color B", "spinbox", &((self.node_color[2] * 255.0) as i32).to_string(), &[], Some(0.0), Some(255.0), Some(1.0));

        // 3. Viewport subnet
        let vp_node = find_or_create_subnet(&mut self.fs_root, "Viewport", "utility", (4.0, 0.0));
        vp_node.children.clear();

        ensure_param(vp_node, "Camera", "section", "", &[], None, None, None);
        if let Some(p) = vp_node.params.iter_mut().find(|p| p.name == "Active Camera") {
            p.options = camera_options.clone();
            if !p.options.contains(&p.default) {
                p.default = "Default Camera".to_string();
            }
        } else {
            ensure_param(vp_node, "Active Camera", "choice", &self.active_camera, &camera_options_refs, None, None, None);
        }

        ensure_param(vp_node, "Display", "section", "", &[], None, None, None);
        ensure_param(vp_node, "Square Aspect", "choice", if self.square_viewport { "true" } else { "false" }, &["false", "true"], None, None, None);

        ensure_param(vp_node, "Guides", "section", "", &[], None, None, None);
        ensure_param(vp_node, "Show Grid", "choice", if self.show_grid { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_node, "Cube", "choice", if self.show_cube { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_node, "Origin", "choice", if self.show_origin { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_node, "Camera Pivot", "choice", if self.show_camera_pivot { "true" } else { "false" }, &["false", "true"], None, None, None);

        ensure_param(vp_node, "View", "section", "", &[], None, None, None);
        ensure_param(vp_node, "Close Pane", "button", "", &[], None, None, None);

        ensure_param(vp_node, "Settings", "section", "", &[], None, None, None);
        ensure_param(vp_node, "Show Grid Guide", "choice", if self.show_grid { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_node, "Show Reference Cube", "choice", if self.show_cube { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_node, "Show Origin Axes", "choice", if self.show_origin { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_node, "Show Camera Pivot", "choice", if self.show_camera_pivot { "true" } else { "false" }, &["false", "true"], None, None, None);
        ensure_param(vp_node, "Grid Thickness", "spinbox", &((self.grid_thickness * 1000.0) as i32).to_string(), &[], Some(2.0), Some(200.0), Some(1.0));
        ensure_param(vp_node, "Origin Guide Size", "spinbox", &((self.origin_size * 10.0) as i32).to_string(), &[], Some(1.0), Some(50.0), Some(1.0));
        ensure_param(vp_node, "Camera Pivot Size", "spinbox", &((self.camera_pivot_size * 10.0) as i32).to_string(), &[], Some(1.0), Some(50.0), Some(1.0));
        ensure_param(vp_node, "Background Color", "color", &color_to_hex(self.viewport_bg_color), &[], None, None, None);
        ensure_param(vp_node, "Grid Color", "color", &color_to_hex(self.grid_color), &[], None, None, None);

        // 4. Parameters subnet
        let param_node = find_or_create_subnet(&mut self.fs_root, "Parameters", "utility", (6.0, 0.0));
        param_node.children.clear();

        ensure_param(param_node, "Preset", "section", "", &[], None, None, None);
        ensure_param(param_node, "Default", "button", "", &[], None, None, None);
        ensure_param(param_node, "Custom", "button", "", &[], None, None, None);

        ensure_param(param_node, "Reset", "section", "", &[], None, None, None);
        ensure_param(param_node, "All", "button", "", &[], None, None, None);

        ensure_param(param_node, "View", "section", "", &[], None, None, None);
        ensure_param(param_node, "Close Pane", "button", "", &[], None, None, None);

        // 5. Spreadsheet subnet
        let ss_node = find_or_create_subnet(&mut self.fs_root, "Spreadsheet", "utility", (8.0, 0.0));
        ss_node.children.clear();

        ensure_param(ss_node, "View", "section", "", &[], None, None, None);
        ensure_param(ss_node, "Close Pane", "button", "", &[], None, None, None);
    }

    pub(crate) fn apply_settings_from_menubar_subnets(&mut self) {
        // Read Settings from Network subnet
        if let Some(net_idx) = self.fs_root.children.iter().position(|c| c.name == "Network") {
            let node = &self.fs_root.children[net_idx];
            for p in &node.params {
                match p.name.as_str() {
                    "Node Color R" => if let Ok(val) = p.default.parse::<f32>() { self.node_color[0] = val / 255.0; }
                    "Node Color G" => if let Ok(val) = p.default.parse::<f32>() { self.node_color[1] = val / 255.0; }
                    "Node Color B" => if let Ok(val) = p.default.parse::<f32>() { self.node_color[2] = val / 255.0; }
                    "Circular Pane" => if let Ok(val) = p.default.parse::<bool>() { self.circular_network_pane = val; }
                    _ => {}
                }
            }
        }

        // Read Settings from Viewport subnet
        if let Some(vp_idx) = self.fs_root.children.iter().position(|c| c.name == "Viewport") {
            let node = &self.fs_root.children[vp_idx];
            for p in &node.params {
                match p.name.as_str() {
                    "Show Grid Guide" => if let Ok(val) = p.default.parse::<bool>() { self.show_grid = val; }
                    "Show Reference Cube" => if let Ok(val) = p.default.parse::<bool>() { self.show_cube = val; }
                    "Show Origin Axes" => if let Ok(val) = p.default.parse::<bool>() { self.show_origin = val; }
                    "Show Camera Pivot" => if let Ok(val) = p.default.parse::<bool>() { self.show_camera_pivot = val; }
                    "Grid Thickness" => if let Ok(val) = p.default.parse::<f32>() { self.grid_thickness = val / 1000.0; }
                    "Origin Guide Size" => if let Ok(val) = p.default.parse::<f32>() { self.origin_size = val / 10.0; }
                    "Camera Pivot Size" => if let Ok(val) = p.default.parse::<f32>() { self.camera_pivot_size = val / 10.0; }
                    "Background Color" => if let Some(col) = hex_to_color(&p.default) { self.viewport_bg_color = col; }
                    "Grid Color" => if let Some(col) = hex_to_color(&p.default) { self.grid_color = col; }
                    "Square Aspect" => if let Ok(val) = p.default.parse::<bool>() { self.square_viewport = val; }
                    "Show Grid" => if let Ok(val) = p.default.parse::<bool>() { self.show_grid = val; }
                    "Cube" => if let Ok(val) = p.default.parse::<bool>() { self.show_cube = val; }
                    "Origin" => if let Ok(val) = p.default.parse::<bool>() { self.show_origin = val; }
                    "Camera Pivot" => if let Ok(val) = p.default.parse::<bool>() { self.show_camera_pivot = val; }
                    "Active Camera" => self.active_camera = p.default.clone(),
                    _ => {}
                }
            }
        }
    }
}
