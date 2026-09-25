use std::fs;
use std::path::Path;

use crate::app::{State, Project, FsNode, ProjectViewState, PlateGeometry, ParamDef};
use crate::slots::CONTENT_IDX;

pub(crate) fn color_to_hex(rgb: [f32; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}",
        (rgb[0] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb[1] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgb[2] * 255.0).round().clamp(0.0, 255.0) as u8
    )
}

pub(crate) fn hex_to_color(hex: &str) -> Option<[f32; 3]> {
    cce_ui::color::parse_hex_rgb(hex)
}

pub(crate) fn color_to_hex8(rgba: [f32; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}",
        (rgba[0] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgba[1] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgba[2] * 255.0).round().clamp(0.0, 255.0) as u8,
        (rgba[3] * 255.0).round().clamp(0.0, 255.0) as u8
    )
}

/// 6- or 8-digit hex → RGBA (alpha 1.0 when absent).
pub(crate) fn hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    cce_ui::color::parse_hex_rgba(hex)
}

/// "network" -> "Network", for rebuilding an old save's param names.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
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
        self.migrate_meta_settings_node();
    }



    /// The view-state block every save and snapshot shares — the pane state
    /// (visibility, collapse, splitter proportions, docks, pins) beside the
    /// camera/pan fields. Visibility rode the root meta node's View subnet
    /// params into the file until that node was retired.
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
            visible_panes: Some(
                Self::PANE_FLAGS
                    .iter()
                    .filter(|(_, get, _)| get(self))
                    .map(|(name, _, _)| name.to_string())
                    .collect(),
            ),
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
            display: Some(self.display_settings()),
        }
    }

    /// The saved Default Camera view onto the live state — after the
    /// active camera and the path are known. The orbit, zoom and pivot are
    /// the view and always restore; the square aspect, pivot marker and its
    /// size are a camera NODE's own params when one is active in the
    /// current directory, so those restore only for a view with no node.
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
            self.migrate_meta_settings_node();
        }
        if path.file_name().map_or(false, |n| n == "default_project.json") {
            let proj = Project {
                name: "Default Project".to_string(),
                root: self.fs_root.clone(),
                format: crate::app::PROJECT_FORMAT,
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
            format: crate::app::PROJECT_FORMAT,
            view_state: self.project_view_state(),
        };
        let content = serde_json::to_string_pretty(&proj)?;
        fs::write(&state_file_path, content)?;
        self.mark_saved();
        Ok(())
    }

    /// The five pane-visibility flags by their saved name, with the menu
    /// action that flips each — the one table the save and the load share,
    /// so a pane cannot be written under a name the loader does not know.
    const PANE_FLAGS: [(&'static str, fn(&State) -> bool, &'static str); 5] = [
        ("network", |s| s.show_network, "Show Network Pane"),
        ("viewport", |s| s.show_viewport, "Show Viewport Pane"),
        ("parameters", |s| s.show_parameters, "Show Parameters Pane"),
        ("spreadsheet", |s| s.show_spreadsheet, "Show Spreadsheet Pane"),
        ("playbar", |s| s.show_playbar, "Show Playbar Pane"),
    ];

    /// Apply a loaded project's pane state: visibility diffs fire the same
    /// menu actions the View toggles use (slots, checkmarks, focus fixup all
    /// included), then collapse and splitter proportions. Main window only —
    /// detached windows own their single-pane layout, and the sync channel
    /// must not re-shape them.
    fn apply_pane_state_from_project(&mut self, vs: &ProjectViewState) {
        if self.is_detached_network || self.detached_pane.is_some() {
            return;
        }
        // Absent (an older save, or one written before pane state moved off
        // the meta node) keeps the live layout — the same rule the collapse
        // list and the splitters follow.
        if let Some(open) = &vs.visible_panes {
            for (name, get, action) in Self::PANE_FLAGS {
                let desired = open.iter().any(|n| n == name);
                if get(self) != desired {
                    self.execute_menu_action(action);
                }
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
            proj.migrate_param_refs();
            crate::app::merge_template_defs(&mut proj.root, &self.node_templates);
            self.fs_root = proj.root;
            // A load is not a colour change: an older save's wire colour is
            // the baseline, so the auto-enable of single-colour mode stays
            // quiet while the migration reads it.
            self.last_applied_wire_color = None;
            self.migrate_meta_settings_node();
            // Before the default view, whose camera-node rule has the last
            // word on the square aspect and the pivot marker.
            if let Some(d) = &proj.view_state.display {
                self.apply_display_settings(d);
            }
            self.apply_pane_state_from_project(&proj.view_state);
            self.set_active_camera(proj.view_state.active_camera);
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
        proj.migrate_param_refs();
        crate::app::merge_template_defs(&mut proj.root, &self.node_templates);
        self.fs_root = proj.root;
        // As in the default-project branch.
        self.last_applied_wire_color = None;
        self.migrate_meta_settings_node();
        // As in the default-project branch.
        if let Some(d) = &proj.view_state.display {
            self.apply_display_settings(d);
        }
        self.apply_pane_state_from_project(&proj.view_state);
        self.set_active_camera(proj.view_state.active_camera);
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
        self.migrate_meta_settings_node();
        self.set_active_camera("Default Camera");
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

    /// Pull an older project's settings off its root `meta` node, then take
    /// the node out.
    ///
    /// Until 2026-09-23 session-wide display settings lived as params on four
    /// utility subnets (`main`, `view`, `guides`, `render`) under a permanent
    /// root `meta` node, and that node tree was the STORE OF RECORD:
    /// `ensure_menubar_subnets` rebuilt it from live state and
    /// `apply_settings_from_menubar_subnets` copied it back over live state
    /// after every parameter edit anywhere. A display preference was
    /// therefore a piece of project data, carried in the file, reset by
    /// opening someone else's scene — and editable only by selecting the
    /// right node in the right utility subnet.
    ///
    /// They are settings, and they are set from the command palette now: the
    /// live fields are the values, `DesignSettings` persists them to
    /// `state.kdl`, and the dialog's Settings half edits them (see
    /// `SETTINGS` in `src/dialog.rs`). This runs once per load to carry a
    /// saved project's values across rather than dropping them on the floor —
    /// a user who set a grid colour two years ago keeps it.
    ///
    /// Pane visibility is NOT read here: it is genuinely project state and
    /// has moved to `ProjectViewState::visible_panes`, which `load_from_file`
    /// applies. An old save's `view` subnet is read for it, though, or every
    /// project saved before the move would open with the default layout.
    pub(crate) fn migrate_meta_settings_node(&mut self) {
        let meta_idx = self.fs_root.children.iter().position(|c| {
            matches!(c.node_type.as_str(), "session" | "meta") || c.name == "Session"
        });
        // Pre-Session saves parked the four subnets FLAT at the root, with no
        // container above them, so the sweep below runs whether or not a meta
        // node was found — an early return on the container alone left that
        // whole generation of file carrying four dead nodes forever.
        let mut subnets: Vec<FsNode> = match meta_idx {
            Some(i) => self.fs_root.children.remove(i).children,
            None => Vec::new(),
        };
        let mut i = 0;
        while i < self.fs_root.children.len() {
            let c = &self.fs_root.children[i];
            if c.node_type == "utility"
                && matches!(c.name.as_str(), "main" | "view" | "guides" | "render")
            {
                subnets.push(self.fs_root.children.remove(i));
            } else {
                i += 1;
            }
        }
        // Anything else that was living under the meta node is the user's,
        // not ours: adding a non-geometry node in there was allowed, so a
        // migration that quietly ate one would be eating their work. Re-home
        // it at the root, where the level it was in used to be.
        let mut i = 0;
        while i < subnets.len() {
            if matches!(subnets[i].name.as_str(), "main" | "view" | "guides" | "render") {
                i += 1;
            } else {
                let mut node = subnets.remove(i);
                let (nx, ny) = self.find_empty_cell(node.position.0, node.position.1, None);
                node.position = (nx, ny);
                self.fs_root.children.push(node);
            }
        }
        if subnets.is_empty() {
            return;
        }
        let params = |name: &str| -> Vec<ParamDef> {
            subnets
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(name))
                .map(|n| n.params.clone())
                .unwrap_or_default()
        };
        let as_bool = |p: &ParamDef| p.default.parse::<bool>().ok();
        let as_f32 = |p: &ParamDef| p.default.parse::<f32>().ok();

        for p in params("guides").iter().chain(params("main").iter()) {
            match p.name.as_str() {
                "Show Grid Guide" | "Show Grid" => {
                    if let Some(v) = as_bool(p) { self.viewport_mut().show_grid = v; }
                }
                // The reference cube guide was removed on 2026-09-25; an old
                // save's value for it has nowhere to go and is dropped.
                "Show Reference Cube" | "Cube" => {}
                "Show Origin Axes" | "Origin" => {
                    if let Some(v) = as_bool(p) { self.viewport_mut().show_origin = v; }
                }
                "Grid Thickness" => if let Some(v) = as_f32(p) { self.grid_thickness = v / 1000.0; },
                "Origin Guide Size" => if let Some(v) = as_f32(p) { self.origin_size = v / 10.0; },
                "Camera Pivot Size" => if let Some(v) = as_f32(p) { self.camera_pivot_size = v / 10.0; },
                "Grid Color" => if let Some(c) = hex_to_color(&p.default) { self.viewport_mut().grid_color = c; },
                "Background Color" => if let Some(c) = hex_to_color(&p.default) { self.viewport_mut().bg_color = c; },
                "Point Marker Size" => if let Some(v) = as_f32(p) { self.point_marker_size = v / 1000.0; },
                "Point Marker Color" => if let Some(c) = hex_to_color(&p.default) { self.point_marker_color = c; },
                "World Unit" => if let Some(u) = cce_ui::units::Unit::parse(&p.default) { self.world_unit = u; },
                "Circular Pane" => if let Some(v) = as_bool(p) { self.circular_network_pane = v; },
                "Ray Traced Preview" => if let Some(v) = as_bool(p) { self.viewport_mut().rt_mode = v; },
                _ => {}
            }
        }
        for p in params("render") {
            match p.name.as_str() {
                "Show Wireframe" => if let Some(v) = as_bool(&p) { self.wireframe = v; },
                "Wire Single Color" => if let Some(v) = as_bool(&p) { self.wire_single_color = v; },
                "Wire Color" => if let Some(c) = hex_to_rgba(&p.default) { self.wire_color = c; },
                "Wire Thickness" => if let Some(v) = as_f32(&p) { self.wire_width = v.clamp(1.0, 8.0); },
                "Opacity" => if let Some(v) = as_f32(&p) { self.geo_opacity = v.clamp(0.0, 1.0); },
                "Render Points" => if let Some(v) = as_bool(&p) { self.render_points = v; },
                "Point Size" => if let Some(v) = as_f32(&p) { self.point_size = v.clamp(0.0, 0.1); },
                "Point Color" => if let Some(c) = hex_to_color(&p.default) { self.point_color = c; },
                _ => {}
            }
        }
        // The pane layout an old save carried on its `view` subnet, in the
        // shape `apply_pane_state_from_project` now reads.
        let view = params("view");
        if !view.is_empty() && !(self.is_detached_network || self.detached_pane.is_some()) {
            for (name, get, action) in Self::PANE_FLAGS {
                let Some(p) = view
                    .iter()
                    .find(|p| p.param_type == "toggle" && p.name == format!("Show {} Pane", capitalize(name)))
                else {
                    continue;
                };
                if let Some(desired) = as_bool(p) {
                    if get(self) != desired {
                        self.execute_menu_action(action);
                    }
                }
            }
        }
        // The values are live state now, so they belong in state.kdl — this
        // is the one write that makes the migration stick.
        self.save_settings();
    }
}
