use std::fs;
use std::path::Path;

use crate::app::{State, Project, FsNode, ProjectViewState, CONTENT_IDX, ParamDef};

fn color_to_hex(rgb: [f32; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}",
        (rgb[0] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb[1] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb[2] * 255.0).round().clamp(0.0, 255.0) as u8
    )
}

fn hex_to_color(hex: &str) -> Option<[f32; 3]> {
    cce_ui::color::parse_hex_rgb(hex)
}

fn color_to_hex8(rgba: [f32; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}",
        (rgba[0] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgba[1] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgba[2] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgba[3] * 255.0).round().clamp(0.0, 255.0) as u8
    )
}

/// 6- or 8-digit hex → RGBA (alpha 1.0 when absent).
fn hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    cce_ui::color::parse_hex_rgba(hex)
}

impl State {


    pub(crate) fn update_window_title(&mut self) {
        let base_title = if self.is_detached_network {
            "Network Pane"
        } else {
            "Designer"
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

        // The engine polls `Application::settings` and applies title changes.
        self.title = title;
    }

    pub(crate) fn load_recent_files() -> Vec<std::path::PathBuf> {
        cce_ui::config::load_recent_files()
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect()
    }

    fn save_recent_files(files: &[std::path::PathBuf]) {
        let string_files: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();
        cce_ui::config::save_recent_files(&string_files);
    }

    pub(crate) fn add_recent_file(&mut self, path: std::path::PathBuf) {
        let abs_path = std::fs::canonicalize(&path).unwrap_or(path);
        self.recent_files.retain(|p| p != &abs_path);
        self.recent_files.insert(0, abs_path);
        self.recent_files.truncate(10);
        Self::save_recent_files(&self.recent_files);
        self.ensure_menubar_subnets();
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
            self.app_drag = None;
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
        self.app_drag = None;
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
        self.loaded_project_path = Some(project_dir);
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        self.update_window_title();
        Ok(())
    }

    pub(crate) fn new_project(&mut self) {
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
        self.app_drag = None;
        self.last_click = None;

        self.sync_grid_settings();
        self.sync_nodes();
        self.sync_cursor_and_selection();
        self.sync_parameters_pane();
        self.rebuild_scene_geometry();
        self.rebuild_positions();
        self.apply_layout();
        self.update_panel_bounds();
        self.loaded_project_path = None;
        self.last_saved_root_json = serde_json::to_string(&self.fs_root).unwrap_or_default();
        self.update_window_title();
    }

    pub(crate) fn ensure_menubar_subnets(&mut self) {
        let vp_show_grid = self.viewport().show_grid;
        let vp_show_cube = self.viewport().show_cube;
        let vp_show_origin = self.viewport().show_origin;
        let vp_show_camera_pivot = self.viewport().show_camera_pivot;
        let vp_bg_color = self.viewport().bg_color;
        let vp_grid_color = self.viewport().grid_color;
        let vp_rt_mode = self.viewport().rt_mode;
        let show_network = self.show_network;
        let show_viewport = self.show_viewport;
        let show_parameters = self.show_parameters;
        let show_spreadsheet = self.show_spreadsheet;
        let show_playbar = self.show_playbar;
        let wireframe = self.wireframe;
        let wire_single_color = self.wire_single_color;
        let wire_color = self.wire_color;
        let wire_width = self.wire_width;
        let geo_opacity = self.geo_opacity;
        let render_points = self.render_points;
        let point_size = self.point_size;
        let point_color = self.point_color;
        let square_viewport = self.square_viewport;
        let bool_str = |b: bool| if b { "true" } else { "false" };

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
        recent_options.push("Other".to_string());
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

        fn set_toggle(p: &mut ParamDef, on: bool) {
            p.param_type = "toggle".to_string();
            p.options.clear();
            p.default = if on { "true" } else { "false" }.to_string();
        }

        // Retain only the Main utility subnet, removing the rest
        self.fs_root.children.retain(|c| c.name != "Network" && c.name != "Viewport" && c.name != "Parameters" && c.name != "Spreadsheet");

        // 1. Main subnet
        let main_node = find_or_create_subnet(&mut self.fs_root, "Main", "utility", (0.0, 0.0));
        main_node.children.clear();

        ensure_param(main_node, "File", "section", "", &[], None, None, None);
        ensure_param(main_node, "New Project", "button", "", &[], None, None, None);

        if let Some(p) = main_node.params.iter_mut().find(|p| p.name == "Open") {
            p.options = recent_options.clone();
            if !p.options.contains(&p.default) {
                p.default = "- Select -".to_string();
            }
        } else {
            ensure_param(main_node, "Open", "choice", "- Select -", &recent_options_refs, None, None, None);
        }

        ensure_param(main_node, "Save", "button", "", &[], None, None, None);
        ensure_param(main_node, "Save As", "button", "", &[], None, None, None);
        ensure_param(main_node, "Exit", "button", "", &[], None, None, None);

        ensure_param(main_node, "Edit", "section", "", &[], None, None, None);
        ensure_param(main_node, "Undo", "button", "", &[], None, None, None);
        ensure_param(main_node, "Redo", "button", "", &[], None, None, None);

        ensure_param(main_node, "View", "section", "", &[], None, None, None);
        ensure_param(main_node, "Show Viewport Pane", "toggle", bool_str(show_viewport), &[], None, None, None);
        ensure_param(main_node, "Show Parameters Pane", "toggle", bool_str(show_parameters), &[], None, None, None);
        ensure_param(main_node, "Show Spreadsheet Pane", "toggle", bool_str(show_spreadsheet), &[], None, None, None);
        ensure_param(main_node, "Show Playbar Pane", "toggle", bool_str(show_playbar), &[], None, None, None);

        // Network — renamed from the retired "Network Settings" (migrate older
        // saves' section param in place so its position survives the reorder).
        if let Some(sec) = main_node.params.iter_mut().find(|p| p.name == "Network Settings") {
            sec.name = "Network".to_string();
            sec.label = "Network".to_string();
        }
        ensure_param(main_node, "Network", "section", "", &[], None, None, None);
        ensure_param(main_node, "Show Network Pane", "toggle", bool_str(show_network), &[], None, None, None);
        ensure_param(main_node, "Zoom In", "button", "", &[], None, None, None);
        ensure_param(main_node, "Zoom Out", "button", "", &[], None, None, None);
        ensure_param(main_node, "Reset Zoom", "button", "", &[], None, None, None);
        ensure_param(main_node, "Detach Circular Window", "button", "", &[], None, None, None);
        ensure_param(main_node, "Circular Pane", "toggle", bool_str(self.circular_network_pane), &[], None, None, None);
        // Node color is config-owned (style.surface.graph.node.color in
        // config.kdl) — drop the retired per-project params from older saves.
        main_node.params.retain(|p| !matches!(p.name.as_str(), "Node Color R" | "Node Color G" | "Node Color B"));

        // Viewport — renamed from the retired "Viewport Settings" (migrate
        // older saves' section param in place).
        if let Some(sec) = main_node.params.iter_mut().find(|p| p.name == "Viewport Settings") {
            sec.name = "Viewport".to_string();
            sec.label = "Viewport".to_string();
        }
        ensure_param(main_node, "Viewport", "section", "", &[], None, None, None);
        if let Some(p) = main_node.params.iter_mut().find(|p| p.name == "Active Camera") {
            p.options = camera_options.clone();
            if !p.options.contains(&p.default) {
                p.default = "Default Camera".to_string();
            }
        } else {
            ensure_param(main_node, "Active Camera", "choice", &self.active_camera, &camera_options_refs, None, None, None);
        }

        ensure_param(main_node, "Ray Traced Preview", "toggle", bool_str(vp_rt_mode), &[], None, None, None);
        ensure_param(main_node, "Camera Pivot Size", "spinbox", &((self.camera_pivot_size * 10.0) as i32).to_string(), &[], Some(1.0), Some(50.0), Some(1.0));
        ensure_param(main_node, "Background Color", "color", &color_to_hex(vp_bg_color), &[], None, None, None);

        // Style — the DE-chrome styling this instance renders with. The bevel
        // profile ramp reshapes every recess/boss wall live (the identity 0→1
        // smooth curve IS the analytic default the toolkit ships).
        ensure_param(main_node, "Style", "section", "", &[], None, None, None);
        ensure_param(main_node, "Bevel Profile", "ramp", "smooth;0.000:0.000,1.000:1.000", &[], None, None, None);
        // The plate perimeter roll's descent curve (face join → silhouette).
        // Unlike the carve ramp — whose identity curve IS its analytic
        // default — the roll's identity would be a straight chamfer, so the
        // untouched identity spec means "analytic quadrant" (apply below
        // clears the profile for it) and any edited curve takes over.
        ensure_param(main_node, "Edge Profile", "ramp", "smooth;0.000:0.000,1.000:1.000", &[], None, None, None);
        // The params plate's tint, rgba — alpha doubles as the frost strength
        // under plate blur. Seeded from the live cce-ui color (linear → sRGB
        // for the hex; alpha is stored linear on both sides).
        let plate_hex = {
            let c = cce_ui::color::param_bg_color();
            format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                (cce_ui::color::linear_to_srgb(c[0]) * 255.0).round().clamp(0.0, 255.0) as u8,
                (cce_ui::color::linear_to_srgb(c[1]) * 255.0).round().clamp(0.0, 255.0) as u8,
                (cce_ui::color::linear_to_srgb(c[2]) * 255.0).round().clamp(0.0, 255.0) as u8,
                (c[3] * 255.0).round().clamp(0.0, 255.0) as u8,
            )
        };
        ensure_param(main_node, "Plate Color", "rgba", &plate_hex, &[], None, None, None);

        // The Help section is retired — its only row was an About button nothing
        // dispatched. Drop it from older saves too.
        main_node.params.retain(|p| !matches!(p.name.as_str(), "Help" | "About"));

        // The viewport guide params moved to the Guides utility node: retire
        // Main's copies, keeping an older save's values as the seeds.
        let migrated_grid = main_node.params.iter()
            .find(|p| p.name == "Show Grid Guide")
            .and_then(|p| p.default.parse::<bool>().ok());
        let migrated_cube = main_node.params.iter()
            .find(|p| p.name == "Show Reference Cube")
            .and_then(|p| p.default.parse::<bool>().ok());
        let migrated_origin = main_node.params.iter()
            .find(|p| p.name == "Show Origin Axes")
            .and_then(|p| p.default.parse::<bool>().ok());
        let migrated_thickness = main_node.params.iter()
            .find(|p| p.name == "Grid Thickness")
            .map(|p| p.default.clone());
        let migrated_origin_size = main_node.params.iter()
            .find(|p| p.name == "Origin Guide Size")
            .map(|p| p.default.clone());
        let migrated_grid_color = main_node.params.iter()
            .find(|p| p.name == "Grid Color")
            .map(|p| p.default.clone());
        main_node.params.retain(|p| {
            !matches!(
                p.name.as_str(),
                "Show Grid Guide" | "Show Reference Cube" | "Show Origin Axes"
                    | "Grid Thickness" | "Origin Guide Size" | "Grid Color"
            )
        });

        // Square Aspect / Show Camera Pivot moved to the camera nodes
        // (per-camera display params): retire Main's copies, keeping an older
        // save's values as the seed for the cameras below.
        let migrated_square = main_node.params.iter()
            .find(|p| p.name == "Square Aspect")
            .and_then(|p| p.default.parse::<bool>().ok());
        let migrated_pivot = main_node.params.iter()
            .find(|p| p.name == "Show Camera Pivot")
            .and_then(|p| p.default.parse::<bool>().ok());
        main_node.params.retain(|p| !matches!(p.name.as_str(), "Square Aspect" | "Show Camera Pivot"));

        // Boolean settings and pane-visibility items render as toggles. Older
        // saves stored these as choice dropdowns / buttons; retype them and
        // reflect live pane state so a reopened project shows real switches.
        for p in main_node.params.iter_mut() {
            match p.name.as_str() {
                "Circular Pane" | "Ray Traced Preview" => {
                    p.param_type = "toggle".to_string();
                    p.options.clear();
                    if p.default != "true" { p.default = "false".to_string(); }
                }
                "Show Network Pane" => set_toggle(p, show_network),
                "Show Viewport Pane" => set_toggle(p, show_viewport),
                "Show Parameters Pane" => set_toggle(p, show_parameters),
                "Show Spreadsheet Pane" => set_toggle(p, show_spreadsheet),
                "Show Playbar Pane" => set_toggle(p, show_playbar),
                _ => {}
            }
        }

        const MAIN_PARAM_ORDER: [&str; 30] = [
            "File", "New Project", "Open", "Save", "Save As", "Exit",
            "Edit", "Undo", "Redo",
            "View", "Show Viewport Pane", "Show Parameters Pane",
            "Show Spreadsheet Pane", "Show Playbar Pane",
            "Network", "Show Network Pane", "Zoom In", "Zoom Out",
            "Reset Zoom", "Detach Circular Window", "Circular Pane",
            "Viewport", "Active Camera",
            "Ray Traced Preview",
            "Camera Pivot Size", "Background Color",
            "Style", "Bevel Profile", "Edge Profile", "Plate Color",
        ];
        main_node.params.sort_by_key(|p| {
            MAIN_PARAM_ORDER
                .iter()
                .position(|n| *n == p.name)
                .unwrap_or(MAIN_PARAM_ORDER.len())
        });

        // Display labels only — the `name` stays the dispatch identity used by
        // execute_menu_action and the live-toggle refresh. The params pane keys
        // off `label` when set (param_display), and sync_parameters_to_project
        // resolves a click back to its param by that same display key.
        for p in main_node.params.iter_mut() {
            if p.name == "New Project" {
                p.label = "New".to_string();
            } else if p.name == "Show Network Pane" {
                // In the Network section the pane toggle reads as "Visible".
                p.label = "Visible".to_string();
            } else if p.param_type == "toggle" {
                if let Some(rest) = p.name.strip_prefix("Show ") {
                    p.label = rest.to_string();
                }
            }
        }

        // 2. Guides subnet — viewport guide toggles (home of the grid toggle,
        // migrated off Main), sitting in the cell Render vacated.
        let guides_node = find_or_create_subnet(&mut self.fs_root, "Guides", "utility", (0.0, 1.0));
        guides_node.children.clear();
        ensure_param(guides_node, "Guides", "section", "", &[], None, None, None);
        ensure_param(guides_node, "Show Grid Guide", "toggle", bool_str(migrated_grid.unwrap_or(vp_show_grid)), &[], None, None, None);
        ensure_param(guides_node, "Show Reference Cube", "toggle", bool_str(migrated_cube.unwrap_or(vp_show_cube)), &[], None, None, None);
        ensure_param(guides_node, "Show Origin Axes", "toggle", bool_str(migrated_origin.unwrap_or(vp_show_origin)), &[], None, None, None);
        let thickness_seed = migrated_thickness
            .unwrap_or_else(|| ((self.grid_thickness * 1000.0) as i32).to_string());
        ensure_param(guides_node, "Grid Thickness", "spinbox", &thickness_seed, &[], Some(2.0), Some(200.0), Some(1.0));
        let origin_size_seed = migrated_origin_size
            .unwrap_or_else(|| ((self.origin_size * 10.0) as i32).to_string());
        ensure_param(guides_node, "Origin Guide Size", "spinbox", &origin_size_seed, &[], Some(1.0), Some(50.0), Some(1.0));
        let grid_color_seed = migrated_grid_color.unwrap_or_else(|| color_to_hex(vp_grid_color));
        ensure_param(guides_node, "Grid Color", "color", &grid_color_seed, &[], None, None, None);
        for p in guides_node.params.iter_mut() {
            match p.name.as_str() {
                "Show Grid Guide" => set_toggle(p, vp_show_grid),
                "Show Reference Cube" => set_toggle(p, vp_show_cube),
                "Show Origin Axes" => set_toggle(p, vp_show_origin),
                _ => {}
            }
            if p.param_type == "toggle" {
                if let Some(rest) = p.name.strip_prefix("Show ") {
                    p.label = rest.to_string();
                }
            }
        }

        // 3. Render subnet — render/display controls, present by default like
        // Main. Toggles reflect live state so a reopened project shows real
        // switches. One cell below Guides; older saves parked at the old
        // default (0,1) slide down to make room.
        let render_node = find_or_create_subnet(&mut self.fs_root, "Render", "utility", (0.0, 2.0));
        if render_node.position == (0.0, 1.0) {
            render_node.position = (0.0, 2.0);
        }
        render_node.children.clear();

        // The overlay toggle is retired — drop it from older saves so the
        // pane doesn't resurrect it.
        render_node.params.retain(|p| p.name != "Wireframe Overlay");

        ensure_param(render_node, "Render Settings", "section", "", &[], None, None, None);
        ensure_param(render_node, "Show Wireframe", "toggle", bool_str(wireframe), &[], None, None, None);
        ensure_param(render_node, "Wire Single Color", "toggle", bool_str(wire_single_color), &[], None, None, None);
        // rgba: the alpha channel is the wireframe's own opacity (the
        // geometry Opacity slider deliberately leaves wires alone).
        ensure_param(render_node, "Wire Color", "rgba", &color_to_hex8(wire_color), &[], None, None, None);
        ensure_param(render_node, "Wire Thickness", "slider:1.0:8.0:1", &format!("{:.1}", wire_width), &[], Some(1.0), Some(8.0), None);
        ensure_param(render_node, "Opacity", "slider:0.00:1.00", &format!("{:.2}", geo_opacity), &[], Some(0.0), Some(1.0), None);
        ensure_param(render_node, "Render Points", "toggle", bool_str(render_points), &[], None, None, None);
        ensure_param(render_node, "Point Size", "slider:0.000:0.100:3", &format!("{:.3}", point_size), &[], Some(0.0), Some(0.10), None);
        ensure_param(render_node, "Point Color", "color", &color_to_hex(point_color), &[], None, None, None);

        for p in render_node.params.iter_mut() {
            match p.name.as_str() {
                "Show Wireframe" => set_toggle(p, wireframe),
                "Wire Single Color" => set_toggle(p, wire_single_color),
                "Wire Color" => {
                    // Type migration: early saves carried a plain rgb color.
                    p.param_type = "rgba".to_string();
                    p.default = color_to_hex8(wire_color);
                }
                "Wire Thickness" => p.default = format!("{:.1}", wire_width),
                "Opacity" => p.default = format!("{:.2}", geo_opacity),
                "Render Points" => set_toggle(p, render_points),
                "Point Size" => {
                    // Range/precision migration: older saves carried
                    // slider:0.01:0.30 (2 decimals).
                    p.param_type = "slider:0.000:0.100:3".to_string();
                    p.min = Some(0.0);
                    p.max = Some(0.10);
                    p.default = format!("{:.3}", point_size);
                }
                "Point Color" => p.default = color_to_hex(point_color),
                _ => {}
            }
            if p.param_type == "toggle" {
                if let Some(rest) = p.name.strip_prefix("Show ") {
                    p.label = rest.to_string();
                }
            }
        }

        // 3. Camera display params — Square Aspect / Show Camera Pivot live on
        // the camera nodes (applied from the ACTIVE camera). Ensured on every
        // camera in the tree, seeded from the retired Main copies (older
        // saves) or the live values.
        fn ensure_camera_display_params(node: &mut FsNode, square: bool, pivot: bool) {
            if node.node_type == "camera" {
                let bool_str = |b: bool| if b { "true" } else { "false" };
                if !node.params.iter().any(|p| p.name == "Square Aspect") {
                    node.params.push(ParamDef {
                        name: "Square Aspect".to_string(),
                        label: "Square Aspect".to_string(),
                        param_type: "toggle".to_string(),
                        default: bool_str(square).to_string(),
                        options: Vec::new(),
                        min: None,
                        max: None,
                        step: None,
                    });
                }
                if !node.params.iter().any(|p| p.name == "Show Camera Pivot") {
                    node.params.push(ParamDef {
                        name: "Show Camera Pivot".to_string(),
                        label: "Camera Pivot".to_string(),
                        param_type: "toggle".to_string(),
                        default: bool_str(pivot).to_string(),
                        options: Vec::new(),
                        min: None,
                        max: None,
                        step: None,
                    });
                }
            }
            for child in &mut node.children {
                ensure_camera_display_params(child, square, pivot);
            }
        }
        let square_seed = migrated_square.unwrap_or(square_viewport);
        let pivot_seed = migrated_pivot.unwrap_or(vp_show_camera_pivot);
        ensure_camera_display_params(&mut self.fs_root, square_seed, pivot_seed);
    }

    pub(crate) fn apply_settings_from_menubar_subnets(&mut self) {
        if let Some(guides_idx) = self.fs_root.children.iter().position(|c| c.name == "Guides") {
            let params = self.fs_root.children[guides_idx].params.clone();
            for p in &params {
                match p.name.as_str() {
                    "Show Grid Guide" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_grid = val; }
                    "Show Reference Cube" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_cube = val; }
                    "Show Origin Axes" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_origin = val; }
                    "Grid Thickness" => if let Ok(val) = p.default.parse::<f32>() { self.grid_thickness = val / 1000.0; }
                    "Origin Guide Size" => if let Ok(val) = p.default.parse::<f32>() { self.origin_size = val / 10.0; }
                    "Grid Color" => if let Some(col) = hex_to_color(&p.default) { self.viewport_mut().grid_color = col; }
                    _ => {}
                }
            }
        }
        if let Some(main_idx) = self.fs_root.children.iter().position(|c| c.name == "Main") {
            let params = self.fs_root.children[main_idx].params.clone();
            for p in &params {
                match p.name.as_str() {
                    // Network Settings
                    "Circular Pane" => if let Ok(val) = p.default.parse::<bool>() { self.circular_network_pane = val; }

                    // Viewport Settings
                    "Show Grid Guide" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_grid = val; }
                    "Show Reference Cube" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_cube = val; }
                    "Show Origin Axes" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_origin = val; }
                    "Ray Traced Preview" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().rt_mode = val; }
                    "Grid Thickness" => if let Ok(val) = p.default.parse::<f32>() { self.grid_thickness = val / 1000.0; }
                    "Origin Guide Size" => if let Ok(val) = p.default.parse::<f32>() { self.origin_size = val / 10.0; }
                    "Camera Pivot Size" => if let Ok(val) = p.default.parse::<f32>() { self.camera_pivot_size = val / 10.0; }
                    "Background Color" => if let Some(col) = hex_to_color(&p.default) { self.viewport_mut().bg_color = col; }
                    "Grid Color" => if let Some(col) = hex_to_color(&p.default) { self.viewport_mut().grid_color = col; }
                    "Show Grid" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_grid = val; }
                    "Cube" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_cube = val; }
                    "Origin" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_origin = val; }
                    "Active Camera" => {
                        let cam = p.default.clone();
                        self.active_camera = cam.clone();
                        self.viewport_mut().active_camera = cam;
                    }

                    // Style
                    "Bevel Profile" => {
                        if let Some((keys, smooth)) = cce_ui::widget::parse_ramp_spec(&p.default) {
                            cce_ui::layout::set_bevel_profile_keys(&keys, smooth);
                        } else {
                            cce_ui::layout::clear_bevel_profile();
                        }
                    }
                    "Edge Profile" => {
                        // The untouched identity spec means "analytic
                        // quadrant", not a straight chamfer (see the
                        // ensure_param note).
                        if p.default == "smooth;0.000:0.000,1.000:1.000" {
                            cce_ui::layout::clear_roll_profile();
                        } else if let Some((keys, smooth)) = cce_ui::widget::parse_ramp_spec(&p.default) {
                            cce_ui::layout::set_roll_profile_keys(&keys, smooth);
                        } else {
                            cce_ui::layout::clear_roll_profile();
                        }
                    }
                    "Plate Color" => {
                        if let Some(c) = cce_ui::color::parse_hex_bytes(&p.default) {
                            cce_ui::color::set_param_bg_color([
                                cce_ui::color::srgb_to_linear(c[0] as f32 / 255.0),
                                cce_ui::color::srgb_to_linear(c[1] as f32 / 255.0),
                                cce_ui::color::srgb_to_linear(c[2] as f32 / 255.0),
                                c[3] as f32 / 255.0,
                            ]);
                        }
                    }
                    _ => {}
                }
            }
        }

        // The ACTIVE camera's display params (per-camera). Default Camera has
        // no node — the live values stand.
        if self.active_camera != "Default Camera" {
            let active = self.active_camera.clone();
            let cam_params = self.current_dir().children.iter()
                .find(|c| c.node_type == "camera" && c.name == active)
                .map(|c| c.params.clone());
            if let Some(params) = cam_params {
                for p in &params {
                    match p.name.as_str() {
                        "Square Aspect" => if let Ok(val) = p.default.parse::<bool>() { self.square_viewport = val; }
                        "Show Camera Pivot" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_camera_pivot = val; }
                        _ => {}
                    }
                }
            }
        }

        if let Some(render_idx) = self.fs_root.children.iter().position(|c| c.name == "Render") {
            let params = self.fs_root.children[render_idx].params.clone();
            for p in &params {
                match p.name.as_str() {
                    "Show Wireframe" => if let Ok(val) = p.default.parse::<bool>() { self.wireframe = val; }
                    "Wire Single Color" => if let Ok(val) = p.default.parse::<bool>() { self.wire_single_color = val; }
                    "Wire Color" => if let Some(col) = hex_to_rgba(&p.default) { self.wire_color = col; }
                    "Wire Thickness" => if let Ok(val) = p.default.parse::<f32>() { self.wire_width = val.clamp(1.0, 8.0); }
                    "Opacity" => if let Ok(val) = p.default.parse::<f32>() { self.geo_opacity = val.clamp(0.0, 1.0); }
                    "Render Points" => if let Ok(val) = p.default.parse::<bool>() { self.render_points = val; }
                    "Point Size" => if let Ok(val) = p.default.parse::<f32>() { self.point_size = val.clamp(0.0, 0.1); }
                    "Point Color" => if let Some(col) = hex_to_color(&p.default) { self.point_color = col; }
                    _ => {}
                }
            }
        }
    }
}
