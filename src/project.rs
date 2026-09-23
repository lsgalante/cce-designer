use std::fs;
use std::path::Path;

use crate::app::{State, Project, FsNode, ProjectViewState, PlateGeometry, ParamDef};
use crate::slots::CONTENT_IDX;

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

    /// The recent list, from `<config home>/cce/<app>/recent-files.kdl`.
    ///
    /// Empty under test, and the write below is skipped there for the same
    /// reason [`DesignSettings::file_path`](crate::app::DesignSettings) is
    /// redirected: the toolkit derives that path from the EXE's basename, so
    /// a test binary wrote a real `~/.config/cce/cce_designer-<hash>/` of its
    /// own — seven of them had accumulated by 2026-09-23. Reading is no safer
    /// than writing, either: a test that loaded the real list would assert
    /// against whatever projects happen to be on the machine running it.
    pub(crate) fn load_recent_files() -> Vec<std::path::PathBuf> {
        if cfg!(test) {
            return Vec::new();
        }
        cce_ui::config::load_recent_files()
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect()
    }

    fn save_recent_files(files: &[std::path::PathBuf]) {
        if cfg!(test) {
            return;
        }
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



    /// The view-state block every save and snapshot shares. Pane visibility
    /// is NOT here — it lives in the root meta node's View subnet params,
    /// which ride `fs_root` into the file; this carries the rest of the pane
    /// state (collapse + splitter proportions) beside the camera/pan fields.
    pub(crate) fn project_view_state(&self) -> ProjectViewState {
        let collapsed_panes = crate::plate_corner::PLATE_SLOTS
            .iter()
            .filter(|&&i| self.collapsed_panes[i])
            .filter_map(|&i| crate::plate_corner::pane_name_from_slot(i))
            .map(str::to_string)
            .collect();
        let splitters = if self.width > 1.0 {
            Some((
                self.splitter_layout.splitter1_x / self.width,
                self.splitter_layout.splitter2_x / self.width,
            ))
        } else {
            None
        };
        // Each dock's tabs by name, active first — the order the loader
        // reads back (first = front).
        let dock_tabs = (0..3)
            .map(|d| {
                let active = self.dock_panes[d];
                let mut names: Vec<String> = Vec::new();
                if let Some(n) = crate::plate_corner::pane_name_from_slot(active) {
                    names.push(n.to_string());
                }
                for &t in &self.dock_tabs[d] {
                    if t != active {
                        if let Some(n) = crate::plate_corner::pane_name_from_slot(t) {
                            names.push(n.to_string());
                        }
                    }
                }
                names
            })
            .collect();
        let plates = if self.width > 1.0 && self.height > 1.0 {
            Some(PlateGeometry {
                network_width: self.floating_network_layout.2 / self.width,
                params_width: self.floating_param_width / self.width,
                spreadsheet_height: self.floating_spreadsheet_height / self.height,
                spreadsheet_inset_left: self.floating_spreadsheet_inset_left / self.width,
                spreadsheet_inset_right: self.floating_spreadsheet_inset_right / self.width,
            })
        } else {
            None
        };
        ProjectViewState {
            active_camera: self.active_camera.clone(),
            pan: (self.pan_x, self.pan_y),
            current_path: self.current_path.clone(),
            selected_node: self.graph().selected_node(),
            collapsed_panes,
            splitters,
            dock_tabs,
            current_path2: self.current_path2.clone(),
            viewport_pin: Self::pin_name(self.viewport_pin),
            params_pin: Self::pin_name(self.params_pin),
            spreadsheet_pin: Self::pin_name(self.spreadsheet_pin),
            plates,
            default_view: Some(crate::app::DefaultCameraView {
                square: self.square_viewport,
                show_pivot: self.viewport().show_camera_pivot,
                pivot_size: self.camera_pivot_size,
                rotation: (self.viewport().rotation_x, self.viewport().rotation_y),
                zoom: self.viewport().zoom,
                pivot: self.viewport().pivot.to_array(),
            }),
        }
    }

    /// The saved Default Camera view onto the live state — after the
    /// active camera and the path are known. The orbit, zoom and pivot are
    /// the view and always restore; the square aspect, pivot marker and its
    /// size are a camera NODE's own params when one is active in the
    /// current directory (`apply_settings_from_menubar_subnets` reads them
    /// off it), so those restore only for a view with no node.
    fn apply_default_view_from_project(&mut self, view: Option<crate::app::DefaultCameraView>) {
        let Some(v) = view else { return };
        let active = self.active_camera.clone();
        let has_node = active != "Default Camera"
            && self.current_dir().children.iter().any(|c| c.node_type == "camera" && c.name == active);
        if !has_node {
            self.square_viewport = v.square;
            self.camera_pivot_size = v.pivot_size;
            self.viewport_mut().show_camera_pivot = v.show_pivot;
        }
        let vp = self.viewport_mut();
        vp.rotation_x = v.rotation.0;
        vp.rotation_y = v.rotation.1;
        vp.zoom = v.zoom.clamp(0.05, crate::viewport_3d::Viewport3D::MAX_ZOOM);
        vp.pivot = glam::Vec3::from_array(v.pivot);
        vp.reset_velocity();
    }

    /// A pin as its saved pane name.
    fn pin_name(pin: Option<usize>) -> Option<String> {
        match pin {
            Some(crate::slots::CONTENT2_IDX) => Some("network2".to_string()),
            Some(_) => Some("network".to_string()),
            None => None,
        }
    }

    /// The inverse: a saved pin name, honored only when its editor exists.
    fn pin_from_name(&self, name: Option<&str>) -> Option<usize> {
        match name {
            Some("network") => Some(crate::slots::CONTENT_IDX),
            Some("network2")
                if self.tab_dock_of_pane(crate::slots::NETWORK_PANEL2_IDX).is_some() =>
            {
                Some(crate::slots::CONTENT2_IDX)
            }
            _ => None,
        }
    }

    pub(crate) fn save_to_file(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        // The View subnet params mirror the live pane flags, but nothing
        // refreshes them on a pane toggle — sync the mirror now so the saved
        // tree carries the pane state that is actually on screen. Main window
        // only: a detached pane window writing the sync channel would stamp
        // its single-pane layout into the file, and the main window's next
        // reload would apply it (the load side is gated the same way).
        if !self.is_detached_network && self.detached_pane.is_none() {
            self.ensure_menubar_subnets();
        }
        if path.file_name().map_or(false, |n| n == "default_project.json") {
            let proj = Project {
                name: "Default Project".to_string(),
                root: self.fs_root.clone(),
                view_state: self.project_view_state(),
            };
            let content = serde_json::to_string_pretty(&proj)?;
            fs::write(path, content)?;
            self.mark_saved();
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
            view_state: self.project_view_state(),
        };
        let content = serde_json::to_string_pretty(&proj)?;
        fs::write(&state_file_path, content)?;
        self.mark_saved();
        Ok(())
    }

    /// The pane-visibility toggles as saved in a project tree's meta→View
    /// subnet. Read them off the LOADED tree before `ensure_menubar_subnets`
    /// runs — it refreshes those params from live state, clobbering what the
    /// file said.
    fn project_pane_visibility(root: &FsNode) -> Vec<(String, bool)> {
        root.children
            .iter()
            .find(|c| c.node_type == "meta")
            .and_then(|m| m.children.iter().find(|c| c.name == "view"))
            .map(|v| {
                v.params
                    .iter()
                    .filter(|p| p.param_type == "toggle" && p.name.starts_with("Show ") && p.name.ends_with(" Pane"))
                    .filter_map(|p| p.default.parse::<bool>().ok().map(|b| (p.name.clone(), b)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Apply a loaded project's pane state: visibility diffs fire the same
    /// menu actions the View toggles use (slots, checkmarks, focus fixup all
    /// included), then collapse and splitter proportions. Main window only —
    /// detached windows own their single-pane layout, and the sync channel
    /// must not re-shape them.
    fn apply_pane_state_from_project(&mut self, visibility: &[(String, bool)], vs: &ProjectViewState) {
        if self.is_detached_network || self.detached_pane.is_some() {
            return;
        }
        for (name, desired) in visibility {
            let cur = match name.as_str() {
                "Show Network Pane" => Some(self.show_network),
                "Show Viewport Pane" => Some(self.show_viewport),
                "Show Parameters Pane" => Some(self.show_parameters),
                "Show Spreadsheet Pane" => Some(self.show_spreadsheet),
                "Show Playbar Pane" => Some(self.show_playbar),
                _ => None,
            };
            if cur == Some(!*desired) {
                self.execute_menu_action(name);
            }
        }
        // Absent names expand: an older save (no collapse list) loads with
        // every pane open rather than inheriting this session's collapses.
        for &idx in crate::plate_corner::PLATE_SLOTS.iter() {
            let desired = crate::plate_corner::pane_name_from_slot(idx)
                .map_or(false, |n| vs.collapsed_panes.iter().any(|c| c == n));
            self.set_pane_collapsed(idx, desired);
        }
        // Dock tab groups: accepted only whole — three lists whose names
        // resolve, cover each CORE docked pane exactly once, and carry the
        // second network editor at most once (its presence in a list is what
        // recreates it; absent, it stays closed — including replacing a live
        // one, since the file's arrangement is the arrangement). Anything
        // else (older saves' empty list included) keeps the current layout
        // rather than loading half of one.
        if vs.dock_tabs.len() == 3 {
            let resolved: Vec<Vec<usize>> = vs
                .dock_tabs
                .iter()
                .map(|names| {
                    names
                        .iter()
                        .filter_map(|n| crate::plate_corner::pane_slot_from_name(n))
                        .collect()
                })
                .collect();
            let all: Vec<usize> = resolved.iter().flatten().copied().collect();
            let n2 = crate::slots::NETWORK_PANEL2_IDX;
            let n2_count = all.iter().filter(|&&s| s == n2).count();
            let mut core: Vec<usize> = all.iter().copied().filter(|&s| s != n2).collect();
            core.sort_unstable();
            let mut expected = vec![
                crate::slots::NETWORK_PANEL_IDX,
                crate::slots::PARAM_IDX,
                crate::slots::SPREADSHEET_IDX,
            ];
            expected.sort_unstable();
            if core == expected && n2_count <= 1 {
                for d in 0..3 {
                    self.dock_tabs[d] = resolved[d].clone();
                    self.dock_panes[d] =
                        resolved[d].first().copied().unwrap_or(crate::app::NO_PANE);
                }
                self.rebuild_positions();
                self.apply_layout();
            }
        }
        // The second editor's own path, clamped against the loaded tree —
        // a stale save must degrade to the deepest valid ancestor.
        self.current_path2 = vs.current_path2.clone();
        self.clamp_path2();
        // Pins: "network2" only holds if the loaded arrangement actually
        // carries the second editor.
        self.viewport_pin = self.pin_from_name(vs.viewport_pin.as_deref());
        self.params_pin = self.pin_from_name(vs.params_pin.as_deref());
        self.spreadsheet_pin = self.pin_from_name(vs.spreadsheet_pin.as_deref());
        if let Some((f1, f2)) = vs.splitters {
            if self.width > 1.0 && f1 > 0.02 && f2 < 0.98 && f1 < f2 {
                self.splitter_layout.splitter1_x = f1 * self.width;
                self.splitter_layout.splitter2_x = f2 * self.width;
                self.rebuild_positions();
                self.apply_layout();
            }
        }
        // Plate geometry: the fractions scale back onto this window, and
        // the layout pass clamps them exactly as a drag would (minimum
        // widths, the spreadsheet's tuck limits). A save with a nonsense
        // value keeps the live geometry rather than loading half of one.
        if let Some(pg) = vs.plates {
            let sane = |f: f32| f.is_finite() && (0.0..=1.0).contains(&f);
            let all = [pg.network_width, pg.params_width, pg.spreadsheet_height, pg.spreadsheet_inset_left, pg.spreadsheet_inset_right];
            if self.width > 1.0 && self.height > 1.0 && all.iter().all(|&f| sane(f)) {
                self.floating_network_layout.2 = pg.network_width * self.width;
                self.floating_param_width = pg.params_width * self.width;
                self.floating_spreadsheet_height = pg.spreadsheet_height * self.height;
                self.floating_spreadsheet_inset_left = pg.spreadsheet_inset_left * self.width;
                self.floating_spreadsheet_inset_right = pg.spreadsheet_inset_right * self.width;
                self.rebuild_positions();
                self.apply_layout();
            }
        }
    }

    pub(crate) fn load_from_file(&mut self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if path.file_name().map_or(false, |n| n == "default_project.json") {
            let content = fs::read_to_string(path)?;
            let mut proj: Project = serde_json::from_str(&content)?;
            proj.sanitize_node_names();
            crate::app::merge_template_defs(&mut proj.root, &self.node_templates);
            crate::app::ensure_meta_children(&mut proj.root);
            let saved_pane_vis = Self::project_pane_visibility(&proj.root);
            self.fs_root = proj.root;
            // The project's viewport settings — the Guides and Render
            // nodes' values, Main's background — onto the live state FIRST:
            // `ensure_menubar_subnets` re-seeds those params from live state
            // (so a chord-flipped toggle shows on the node), which on a load
            // stamped the preferences file's values over the file's and lost
            // them before the apply below could read them (2026-09-21).
            // A load is not a colour change: the loaded tree's value is the
            // baseline, so the auto-enable of single-colour mode stays quiet.
            self.last_applied_wire_color = None;
            self.apply_settings_from_menubar_subnets();
            self.ensure_menubar_subnets();
            self.apply_settings_from_menubar_subnets();
            self.apply_pane_state_from_project(&saved_pane_vis, &proj.view_state);
            self.active_camera = proj.view_state.active_camera;
            self.pan_x = proj.view_state.pan.0;
            self.pan_y = proj.view_state.pan.1;
            self.pan_velocity_x = 0.0;
            self.pan_velocity_y = 0.0;
            self.last_frame_pan_x = self.pan_x;
            self.last_frame_pan_y = self.pan_y;
            self.current_path = proj.view_state.current_path;
            self.apply_default_view_from_project(proj.view_state.default_view);

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
            self.mark_saved();
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
        let mut proj: Project = serde_json::from_str(&content)?;
        proj.sanitize_node_names();
        crate::app::merge_template_defs(&mut proj.root, &self.node_templates);
        crate::app::ensure_meta_children(&mut proj.root);
        let saved_pane_vis = Self::project_pane_visibility(&proj.root);
        self.fs_root = proj.root;
        // As in the default-project branch: the file's viewport settings
        // land on the live state before ensure re-seeds the nodes from it.
        // A load is not a colour change: the loaded tree's value is the
        // baseline, so the auto-enable of single-colour mode stays quiet.
        self.last_applied_wire_color = None;
        self.apply_settings_from_menubar_subnets();
        self.ensure_menubar_subnets();
        self.apply_settings_from_menubar_subnets();
        self.apply_pane_state_from_project(&saved_pane_vis, &proj.view_state);
        self.active_camera = proj.view_state.active_camera;
        self.pan_x = proj.view_state.pan.0;
        self.pan_y = proj.view_state.pan.1;
        self.pan_velocity_x = 0.0;
        self.pan_velocity_y = 0.0;
        self.last_frame_pan_x = self.pan_x;
        self.last_frame_pan_y = self.pan_y;
        self.current_path = proj.view_state.current_path;
        self.apply_default_view_from_project(proj.view_state.default_view);

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
        self.mark_saved();
        self.update_window_title();
        Ok(())
    }

    /// The Main node's "Set As Default": remember the currently-loaded project
    /// as what the app opens at startup. A pointer in state.kdl — NOT a rewrite
    /// of the bundled default_project.json, which is versioned and doubles as
    /// the detached-window sync channel. A scratch (never-saved) project has no
    /// path to point at, so the click is a no-op with a note.
    pub(crate) fn set_current_as_default(&mut self) {
        match &self.loaded_project_path {
            Some(path) => {
                self.default_project_setting = Some(path.to_string_lossy().into_owned());
                self.save_settings();
            }
            None => {
                eprintln!("Set As Default: no project file is loaded — save the project first");
            }
        }
    }

    /// Open the configured startup project, if any. Main window only — the
    /// detached windows must keep seeding from default_project.json, which is
    /// their sync channel with the parent.
    ///
    /// A default that cannot be opened — gone, or unreadable — falls back to
    /// what `State::new` already loaded and **keeps the setting**, saying so
    /// on the status line. It used to DELETE the pointer on a path that did
    /// not exist, reasoning that a dead default should not fail on every
    /// launch. The trade is the wrong way round: failing costs one line of
    /// stderr and a fallback that already works, while forgetting costs the
    /// user a setting they cannot get back without reopening the project and
    /// pressing the button again. And a path is absent for reasons that pass
    /// — a cloud-synced folder the daemon has not mounted yet, an external
    /// drive, a machine that autostarts the app before the network is up —
    /// so the one launch that raced the filesystem took the setting with it.
    pub(crate) fn load_default_project_setting(&mut self) {
        let Some(configured) = self.default_project_setting.clone() else { return };
        let path = std::path::PathBuf::from(&configured);
        if !path.exists() {
            eprintln!("Default project is not there right now: {configured}");
            self.update_status_text(&format!("Default project not found: {configured}"));
            return;
        }
        if let Err(e) = self.load_from_file(&path) {
            eprintln!("Failed to load default project {configured}: {e:?}");
            self.update_status_text(&format!("Default project would not open: {configured}"));
        }
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
        self.mark_saved();
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
        let network_plate = self.network_plate;
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
                    show_when: String::new(),
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

        // The root meta node: the permanent root container for the
        // session-wide settings nodes (Main/View/Guides/Render) — the root
        // network's counterpart of every node's per-node `meta` child, and
        // still a subnet. It began life as the "Session" node; older saves
        // carry it typed "session" (or the four settings nodes flat at the
        // root) and are migrated — retyped/renamed, params intact. The node
        // itself is undeletable (delete_node refuses the "meta" type).
        let mut migrated: Vec<FsNode> = Vec::new();
        {
            let mut idx = 0;
            while idx < self.fs_root.children.len() {
                let c = &self.fs_root.children[idx];
                if c.node_type == "utility"
                    && matches!(c.name.as_str(), "main" | "view" | "guides" | "render")
                {
                    migrated.push(self.fs_root.children.remove(idx));
                } else {
                    idx += 1;
                }
            }
        }
        let session_idx = match self
            .fs_root
            .children
            .iter()
            .position(|c| matches!(c.node_type.as_str(), "session" | "meta") || c.name == "Session")
        {
            Some(i) => {
                self.fs_root.children[i].node_type = "meta".to_string();
                self.fs_root.children[i].name = "meta".to_string();
                i
            }
            None => {
                self.fs_root.children.push(FsNode {
                    id: crate::app::generate_node_id(),
                    name: "meta".to_string(),
                    node_type: "meta".to_string(),
                    children: vec![],
                    params: vec![],
                    geometry_visible: true,
                    position: (0.0, 0.0),
                    inputs: 0,
                    outputs: 0,
                });
                self.fs_root.children.len() - 1
            }
        };
        for node in migrated {
            let session = &mut self.fs_root.children[session_idx];
            if !session.children.iter().any(|c| c.name == node.name) {
                session.children.push(node);
            }
        }
        let session = &mut self.fs_root.children[session_idx];

        // 1. Main subnet
        let main_node = find_or_create_subnet(session, "main", "utility", (0.0, 0.0));
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
        ensure_param(main_node, "Set As Default", "button", "", &[], None, None, None);
        ensure_param(main_node, "Exit", "button", "", &[], None, None, None);

        ensure_param(main_node, "Edit", "section", "", &[], None, None, None);
        ensure_param(main_node, "Undo", "button", "", &[], None, None, None);
        ensure_param(main_node, "Redo", "button", "", &[], None, None, None);

        // The pane-visibility toggles moved to the View utility node (below):
        // retire Main's copies and its now-empty View section from older saves.
        // No value migration — pane state is session-owned, never applied from
        // the project, so the View node seeds from live state.
        main_node.params.retain(|p| {
            !matches!(
                p.name.as_str(),
                "View" | "Show Network Pane" | "Show Viewport Pane" | "Show Parameters Pane"
                    | "Show Spreadsheet Pane" | "Show Playbar Pane"
            )
        });

        // Network — renamed from the retired "Network Settings" (migrate older
        // saves' section param in place so its position survives the reorder).
        if let Some(sec) = main_node.params.iter_mut().find(|p| p.name == "Network Settings") {
            sec.name = "Network".to_string();
            sec.label = "Network".to_string();
        }
        ensure_param(main_node, "Network", "section", "", &[], None, None, None);
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
        ensure_param(main_node, "Background Color", "color", &color_to_hex(vp_bg_color), &[], None, None, None);
        // Mirrors the live value, as the toggles do: a background set on the
        // viewport rather than through the node still reaches the saved tree.
        if let Some(p) = main_node.params.iter_mut().find(|p| p.name == "Background Color") {
            p.default = color_to_hex(vp_bg_color);
        }

        // The Style section is retired — DE chrome is config-owned, not
        // per-project: the wall and edge relief curves are
        // `style.surface.relief.profile` / `.edge_profile` and the params
        // plate tint is `style.surface.param.color` in config.kdl, which
        // cce-ui already applies for every client. Main's copies shadowed
        // those on load, so a project file silently outranked the user's
        // config. Drop them from older saves.
        main_node.params.retain(|p| {
            !matches!(
                p.name.as_str(),
                "Style" | "Bevel Profile" | "Edge Profile" | "Plate Color"
            )
        });

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
        let migrated_pivot_size = main_node.params.iter()
            .find(|p| p.name == "Camera Pivot Size")
            .map(|p| p.default.clone());
        main_node.params.retain(|p| {
            !matches!(p.name.as_str(), "Square Aspect" | "Show Camera Pivot" | "Camera Pivot Size")
        });

        // Boolean settings render as toggles. Older saves stored these as
        // choice dropdowns / buttons; retype them so a reopened project shows
        // real switches.
        for p in main_node.params.iter_mut() {
            match p.name.as_str() {
                "Circular Pane" | "Ray Traced Preview" => {
                    p.param_type = "toggle".to_string();
                    p.options.clear();
                    if p.default != "true" { p.default = "false".to_string(); }
                }
                _ => {}
            }
        }

        const MAIN_PARAM_ORDER: [&str; 20] = [
            "File", "New Project", "Open", "Save", "Save As", "Set As Default", "Exit",
            "Edit", "Undo", "Redo",
            "Network", "Zoom In", "Zoom Out",
            "Reset Zoom", "Detach Circular Window", "Circular Pane",
            "Viewport", "Active Camera",
            "Ray Traced Preview",
            "Background Color",
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
            } else if p.param_type == "toggle" {
                if let Some(rest) = p.name.strip_prefix("Show ") {
                    p.label = rest.to_string();
                }
            }
        }

        // 2. View subnet — the pane-visibility switches, migrated off Main's
        // View section (the Guides pattern: a setting's home is a utility
        // node; the header menu items stay as command access). The toggles
        // refresh from live state — mid-session, the live flags are the
        // authority — but they are ALSO the persisted pane state: save_to_file
        // syncs this mirror before cloning the tree, and load_from_file reads
        // the loaded values (before this refresh clobbers them) and applies
        // the diffs via apply_pane_state_from_project.
        let view_node = find_or_create_subnet(&mut self.fs_root.children[session_idx], "view", "utility", (0.0, 2.0));
        view_node.children.clear();
        ensure_param(view_node, "Panes", "section", "", &[], None, None, None);
        ensure_param(view_node, "Show Network Pane", "toggle", bool_str(show_network), &[], None, None, None);
        ensure_param(view_node, "Show Viewport Pane", "toggle", bool_str(show_viewport), &[], None, None, None);
        ensure_param(view_node, "Show Parameters Pane", "toggle", bool_str(show_parameters), &[], None, None, None);
        ensure_param(view_node, "Show Spreadsheet Pane", "toggle", bool_str(show_spreadsheet), &[], None, None, None);
        ensure_param(view_node, "Show Playbar Pane", "toggle", bool_str(show_playbar), &[], None, None, None);
        // Appearance, not visibility — hence its own section. The pane
        // toggles above say which panes EXIST; this one says whether the
        // network draws a surface under its graph or lets the scene through.
        ensure_param(view_node, "Network", "section", "", &[], None, None, None);
        ensure_param(
            view_node,
            "Show Network Plate",
            "toggle",
            bool_str(network_plate),
            &[],
            None,
            None,
            None,
        );
        for p in view_node.params.iter_mut() {
            match p.name.as_str() {
                "Show Network Pane" => set_toggle(p, show_network),
                "Show Viewport Pane" => set_toggle(p, show_viewport),
                "Show Parameters Pane" => set_toggle(p, show_parameters),
                "Show Spreadsheet Pane" => set_toggle(p, show_spreadsheet),
                "Show Playbar Pane" => set_toggle(p, show_playbar),
                "Show Network Plate" => set_toggle(p, network_plate),
                _ => {}
            }
            // "Show Network Pane" -> "Network": inside the Panes section the
            // toggles read by pane name alone. The `name` stays the dispatch
            // identity execute_menu_action fires on.
            if p.param_type == "toggle" {
                if let Some(rest) = p.name.strip_prefix("Show ").and_then(|r| r.strip_suffix(" Pane")) {
                    p.label = rest.to_string();
                } else if p.name == "Show Network Plate" {
                    // Under its own "Network" section the row reads as
                    // "Plate", the same way the pane rows read as their pane.
                    p.label = "Plate".to_string();
                }
            }
        }

        // 3. Guides subnet — viewport guide toggles (home of the grid toggle,
        // migrated off Main). The utility column keeps one empty cell between
        // nodes: Main (0,0), View (0,2), Guides (0,4), Render (0,6); older
        // saves parked at prior defaults slide to the spaced slots.
        let guides_node = find_or_create_subnet(&mut self.fs_root.children[session_idx], "guides", "utility", (0.0, 4.0));
        if guides_node.position == (0.0, 1.0) || guides_node.position == (0.0, 2.0) {
            guides_node.position = (0.0, 4.0);
        }
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
        // Size of the per-node meta "Point Markers" overlay, in thousandths
        // (the Grid Thickness convention): 20 = 0.02 world units.
        let marker_size_seed = ((self.meta_marker_size * 1000.0).round() as i32).to_string();
        ensure_param(guides_node, "Point Marker Size", "spinbox", &marker_size_seed, &[], Some(5.0), Some(100.0), Some(1.0));
        let marker_color_seed = color_to_hex(self.meta_marker_color);
        ensure_param(guides_node, "Point Marker Color", "color", &marker_color_seed, &[], None, None, None);
        // What a world unit is in the real world. The geometry never
        // converts; the viewport's scale readout and `View 1:1` do.
        ensure_param(guides_node, "World Unit", "choice", self.world_unit.suffix(), &["mm", "cm", "m", "in"], None, None, None);
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

        // 4. Render subnet — render/display controls, present by default like
        // Main. Toggles reflect live state so a reopened project shows real
        // switches. Two rows below Guides (the spaced column); older saves
        // parked at the prior defaults slide down.
        let render_node = find_or_create_subnet(&mut self.fs_root.children[session_idx], "render", "utility", (0.0, 6.0));
        if render_node.position == (0.0, 1.0) || render_node.position == (0.0, 2.0) || render_node.position == (0.0, 4.0) {
            render_node.position = (0.0, 6.0);
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

        // 3. Camera display params — Square Aspect / Show Camera Pivot /
        // Camera Pivot Size live on the camera nodes (applied from the
        // ACTIVE camera). Ensured on every camera in the tree, seeded from
        // the retired Main copies (older saves) or the live values.
        fn ensure_camera_display_params(node: &mut FsNode, square: bool, pivot: bool, pivot_size: &str) {
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
                        show_when: String::new(),
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
                        show_when: String::new(),
                    });
                }
                if !node.params.iter().any(|p| p.name == "Camera Pivot Size") {
                    node.params.push(ParamDef {
                        name: "Camera Pivot Size".to_string(),
                        label: "Camera Pivot Size".to_string(),
                        param_type: "spinbox".to_string(),
                        default: pivot_size.to_string(),
                        options: Vec::new(),
                        min: Some(1.0),
                        max: Some(50.0),
                        step: Some(1.0),
                        show_when: String::new(),
                    });
                }
            }
            for child in &mut node.children {
                ensure_camera_display_params(child, square, pivot, pivot_size);
            }
        }
        let square_seed = migrated_square.unwrap_or(square_viewport);
        let pivot_seed = migrated_pivot.unwrap_or(vp_show_camera_pivot);
        let pivot_size_seed = migrated_pivot_size
            .unwrap_or_else(|| ((self.camera_pivot_size * 10.0) as i32).to_string());
        ensure_camera_display_params(&mut self.fs_root, square_seed, pivot_seed, &pivot_size_seed);
    }

    pub(crate) fn apply_settings_from_menubar_subnets(&mut self) {
        let session_params = |root: &FsNode, name: &str| -> Option<Vec<ParamDef>> {
            root.children
                .iter()
                .find(|c| c.node_type == "meta")
                .and_then(|s| s.children.iter().find(|c| c.name == name))
                .map(|n| n.params.clone())
        };
        if let Some(params) = session_params(&self.fs_root, "guides") {
            for p in &params {
                match p.name.as_str() {
                    "Show Grid Guide" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_grid = val; }
                    "Show Reference Cube" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_cube = val; }
                    "Show Origin Axes" => if let Ok(val) = p.default.parse::<bool>() { self.viewport_mut().show_origin = val; }
                    "Grid Thickness" => if let Ok(val) = p.default.parse::<f32>() { self.grid_thickness = val / 1000.0; }
                    "Origin Guide Size" => if let Ok(val) = p.default.parse::<f32>() { self.origin_size = val / 10.0; }
                    "Grid Color" => if let Some(col) = hex_to_color(&p.default) { self.viewport_mut().grid_color = col; }
                    "Point Marker Size" => if let Ok(val) = p.default.parse::<f32>() {
                        let size = val / 1000.0;
                        if (size - self.meta_marker_size).abs() > 1e-6 {
                            self.meta_marker_size = size;
                            // The marker geometry bakes the radius in, so a
                            // size change re-collects the overlays.
                            self.rebuild_scene_geometry();
                        }
                    }
                    "Point Marker Color" => if let Some(col) = hex_to_color(&p.default) {
                        if col != self.meta_marker_color {
                            self.meta_marker_color = col;
                            // Baked into the marker verts, like the radius.
                            self.rebuild_scene_geometry();
                        }
                    }
                    "World Unit" => if let Some(u) = cce_ui::units::Unit::parse(&p.default) {
                        if u != self.world_unit {
                            self.world_unit = u;
                            self.viewport_dirty = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(params) = session_params(&self.fs_root, "main") {
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
                        "Camera Pivot Size" => if let Ok(val) = p.default.parse::<f32>() { self.camera_pivot_size = val / 10.0; }
                        _ => {}
                    }
                }
            }
        }

        if let Some(params) = session_params(&self.fs_root, "render") {
            let before = self.wire_color;
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
            // Setting a wire colour means wanting to see it: a CHANGE to the
            // colour (not a load — the first read of a tree sets the
            // baseline, and a load's value is what it is) turns single-colour
            // mode on if it was off, on the node as well as live, so the
            // switch shows moved. Off, the wires carry the geometry's own
            // colours and the colour row is their alpha alone — which twice
            // read as "the colour did not take" (2026-09-21).
            let changed = self.last_applied_wire_color.is_some_and(|last| last != self.wire_color)
                && before != self.wire_color;
            self.last_applied_wire_color = Some(self.wire_color);
            if changed && !self.wire_single_color {
                self.wire_single_color = true;
                self.write_render_toggle("Wire Single Color", true);
            }
        }
    }
}
