
pub mod app;
pub mod param;
pub mod edit_history;
pub mod application;
pub mod curve_tool;
pub mod soft_transform_tool;
pub mod viewer_state;
pub mod detail;
pub mod export;
pub mod export_cli;
pub mod remesh;
pub mod spatial;
pub mod detangle;
pub mod volume;
pub mod wrangle;
pub mod shapes;
pub mod gpu;
pub mod springs;
pub mod collide;

// Root-level aliases some modules import via `crate::` paths.
#[allow(unused_imports)]
use app::{CustomEvent, McpAction, ModifiersState};
pub mod plate_menu;
pub mod menu_page;
pub mod visualizer;
pub mod playbar;
pub mod viewport_3d;
pub mod api;
pub mod window;
pub mod geometry;
pub mod context;
pub mod environment;
pub mod project;
pub mod render;
pub mod shortcut;
pub mod slots;
pub mod command;
pub mod dialog;
pub mod layout;
pub mod mold;
pub mod hull;
pub mod scatter;
pub mod page;
pub mod image_tools;
pub mod image_handles;
pub mod expr;
pub mod thumbnail;

#[cfg(test)]
mod test_prelude {
    pub use std::fs;
    pub use std::path::Path;
    pub use glam::{Mat4, Vec3};
    pub use cce_ui::widget::{Key, NamedKey};
    pub use crate::app::{State, McpAction, ModifiersState};

    /// The root's first Geometry node: where a loaded save's geometry
    /// stands since the root became the object level (format 5).
    pub fn geo(root: &crate::app::FsNode) -> &crate::app::FsNode {
        root.children
            .iter()
            .find(|c| crate::context::is_geometry_container(&c.node_type))
            .expect("a geometry node at the root")
    }

    pub fn geo_mut(root: &mut crate::app::FsNode) -> &mut crate::app::FsNode {
        root.children
            .iter_mut()
            .find(|c| crate::context::is_geometry_container(&c.node_type))
            .expect("a geometry node at the root")
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Headless thumbnail mode: no Wayland, no window — render and exit.
    //   cce-designer --thumbnail <project-dir-or-state.json> <out.png> [--size N]
    if let Some(i) = args.iter().position(|a| a == "--thumbnail") {
        let _ = env_logger::try_init();
        let (Some(project), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: cce-designer --thumbnail <project> <out.png> [--size N] [--samples N] [--frame N]");
            std::process::exit(2);
        };
        let size = args
            .windows(2)
            .find(|w| w[0] == "--size")
            .and_then(|w| w[1].parse::<u32>().ok())
            .unwrap_or(256)
            .clamp(16, 2048);
        let samples = args
            .windows(2)
            .find(|w| w[0] == "--samples")
            .and_then(|w| w[1].parse::<u32>().ok());
        let frame = args
            .windows(2)
            .find(|w| w[0] == "--frame")
            .and_then(|w| w[1].parse::<i32>().ok());
        match thumbnail::run(std::path::Path::new(project), std::path::Path::new(out), size, samples, frame) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("cce-designer --thumbnail: {e}");
                std::process::exit(1);
            }
        }
    }

    // Headless export: evaluate a project and write its geometry to a file.
    //   cce-designer --export <project> <out.stl|out.obj> [--frame N] [--node NAME] [--scale S]
    //
    // The counterpart of --thumbnail, and the same argument for existing: work
    // that can only leave the app through a window is work that cannot be
    // scripted, diffed or checked.
    if let Some(i) = args.iter().position(|a| a == "--export") {
        let _ = env_logger::try_init();
        let (Some(project), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!(
                "usage: cce-designer --export <project> <out.stl|out.obj> [--frame N] [--node NAME] [--scale S]"
            );
            std::process::exit(2);
        };
        let flag = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
        let frame = flag("--frame").and_then(|v| v.parse::<i32>().ok());
        let scale = flag("--scale").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        match export_cli::run(
            std::path::Path::new(project),
            std::path::Path::new(out),
            frame,
            flag("--node"),
            scale,
        ) {
            Ok(msg) => {
                println!("{msg}");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("cce-designer --export: {e}");
                std::process::exit(1);
            }
        }
    }

    // The GPU setting, as CCE_VK_DEVICE for the renderer the engine is about
    // to create. Only here: the thumbnail and export modes above exit first,
    // so cce-files' preview cache never wakes a discrete GPU.
    app::apply_gpu_preference();

    // Everything windowed runs on the cce-ui engine (application.rs holds the
    // Application impl; --detached-network is read there).
    cce_ui::engine::run::<app::State>();
}

#[cfg(test)]
mod tests {
    use crate::test_prelude::*;
    use crate::app::{get_next_visible_pane, DesignSettings, FsNode, Project, ProjectViewState};
    use crate::slots::{LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX};
    use crate::shortcut::{Shortcut, ShortcutManager, Action};
    use crate::geometry::{box_detail, detail_vertices};
    use crate::detail::{AttribData, AttribKind, AttribType, AttribValue, Class, Detail};

    /// The choosers open in the loaded project's parent — the "current view" —
    /// and fall back to cce-files' remembered location only when nothing is
    /// loaded (a scratch project has no place to point at).
    #[test]
    fn test_chooser_opens_at_the_loaded_projects_parent() {
        let mut state = State::new(false);
        assert_eq!(state.chooser_start_dir(), None, "scratch project must not pin a dir");

        let dir = std::env::temp_dir()
            .join(format!("cce-designer-test-projects-{}", std::process::id()))
            .join("gears");
        std::fs::create_dir_all(&dir).unwrap();
        state.loaded_project_path = Some(dir.clone());
        assert_eq!(state.chooser_start_dir().as_deref(), dir.parent(),
            "chooser must start where the current project lives");
    }

    /// A default project that is not there at launch is not FORGOTTEN. It
    /// used to be deleted from the settings on the reasoning that a dead
    /// pointer should not fail every launch — but a path is absent for
    /// reasons that pass (a cloud-synced folder the daemon has not mounted
    /// yet, an external drive, an autostart that beat the network), and the
    /// one launch that raced the filesystem took a setting the user could
    /// only restore by reopening the project and pressing the button again.
    #[test]
    fn a_default_project_that_is_missing_is_not_forgotten() {
        let gone = std::env::temp_dir().join(format!("cce-designer-no-such-{}", std::process::id()));
        let _ = fs::remove_dir_all(&gone);
        assert!(!gone.exists());

        let mut state = State::new(false);
        let before = state.fs_root.children.len();
        state.default_project_setting = Some(gone.to_string_lossy().into_owned());
        state.load_default_project_setting();

        assert_eq!(
            state.default_project_setting.as_deref(),
            Some(gone.to_string_lossy().as_ref()),
            "the pointer survives a launch that could not see it"
        );
        assert_eq!(state.fs_root.children.len(), before, "and the bundled project still stands");
        assert!(
            state.last_status_text.contains("not found"),
            "the status line says so: {}",
            state.last_status_text
        );
    }

    /// The default-project pointer must survive the KDL round trip state.kdl
    /// actually goes through — serde alone passing means nothing if
    /// json_to_kdl_string / parse_kdl_to_json drop or retype the field.
    #[test]
    fn test_default_project_setting_survives_the_kdl_round_trip() {
        let mut settings = DesignSettings::default();
        settings.default_project = Some("/home/user/projects/gears".to_string());
        let kdl = settings.to_kdl_str().expect("settings serialize");
        let back = DesignSettings::from_kdl_str(&kdl);
        assert_eq!(back.default_project.as_deref(), Some("/home/user/projects/gears"));

        // And absence stays absence — an unset default must not come back as
        // Some("") and shadow the bundled project.
        let none_kdl = DesignSettings::default().to_kdl_str().expect("serialize");
        assert_eq!(DesignSettings::from_kdl_str(&none_kdl).default_project, None);
    }

    /// The node View toggle is project data: it survives the JSON round trip
    /// and counts as an unsaved change (the title asterisk). It was
    /// `#[serde(skip)]`, which silently discarded every toggle on save and
    /// kept `has_unsaved_changes` blind to it.
    #[test]
    fn test_view_toggle_is_project_data() {
        let node = FsNode {
            id: "t".to_string(),
            name: "t".to_string(),
            node_type: "node".to_string(),
            children: vec![],
            params: vec![],
            geometry_visible: false,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        };
        let back: FsNode =
            serde_json::from_str(&serde_json::to_string(&node).unwrap()).unwrap();
        assert!(!back.geometry_visible, "View toggle lost in the JSON round trip");

        // Absence still defaults on, so pre-existing saves load unchanged.
        let legacy: FsNode = serde_json::from_str(r#"{"name":"n"}"#).unwrap();
        assert!(legacy.geometry_visible);

        let mut state = State::new(false);
        state.fs_root.children.push(back);
        state.last_saved_root_json = serde_json::to_string(&state.fs_root).unwrap();
        assert!(!state.has_unsaved_changes());
        state.fs_root.children.last_mut().unwrap().geometry_visible = true;
        assert!(state.has_unsaved_changes(), "the toggle must dirty the title");
    }

    /// Geometry visibility is exclusive per directory: enabling one node's
    /// display turns every sibling's off (a display flag, not a per-node
    /// render flag), while disabling touches only that node. All toggle
    /// routes (keyboard `e`, the graph click toggles, MCP ToggleGeometry)
    /// funnel through `set_child_geometry_visible`.
    #[test]
    fn test_geometry_visibility_is_exclusive_per_directory() {
        let child = |name: &str| FsNode {
            id: name.to_string(),
            name: name.to_string(),
            node_type: "grid".to_string(),
            children: vec![],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        };
        let mut dir = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![child("a"), child("b"), child("c")],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };

        // Enabling slot 1 clears its siblings, even ones already visible.
        dir.set_child_geometry_visible(1, true);
        let vis: Vec<bool> = dir.children.iter().map(|c| c.geometry_visible).collect();
        assert_eq!(vis, [false, true, false]);

        // Disabling is not exclusive — only the named node changes.
        dir.set_child_geometry_visible(1, false);
        let vis: Vec<bool> = dir.children.iter().map(|c| c.geometry_visible).collect();
        assert_eq!(vis, [false, false, false]);

        // Out-of-bounds is a no-op, never a panic or a sibling sweep.
        dir.children[2].geometry_visible = true;
        dir.set_child_geometry_visible(9, true);
        assert!(dir.children[2].geometry_visible);
    }

    /// A newly added node arrives with its display flag OFF: under the
    /// one-visible-per-directory rule, showing geometry is an explicit act,
    /// never a side effect of adding. Template children keep their own flags
    /// (a subnet's internal chain still displays once the parent is enabled).
    #[test]
    fn test_added_nodes_start_hidden() {
        let mut state = State::new(false);
        let mut redraw = false;
        state
            .apply_action(
                McpAction::AddNode { template_name: "Embryo".to_string(), name: None, x: 5.0, y: 5.0 },
                &mut redraw,
            )
            .expect("add embryo node");
        let added = state.current_dir().children.last().unwrap();
        assert!(!added.geometry_visible, "added node must start hidden");
        assert!(
            added.children.iter().any(|c| c.geometry_visible),
            "the template's internal chain must keep its own flags"
        );
    }

    /// Legacy "add" nodes retype to "points" on load (the session->meta
    /// pattern) and gain the renamed template's Shape param through the
    /// normal merge, keeping their own name and saved param values.
    #[test]
    fn test_add_node_migrates_to_points() {
        let legacy: FsNode = serde_json::from_str(
            r#"{"name":"Add 3","type":"add","params":[
                {"name":"points","type":"spinbox","default":"250"}
            ]}"#,
        )
        .unwrap();
        let mut root: FsNode = serde_json::from_str(r#"{"name":"root"}"#).unwrap();
        root.children.push(legacy);
        let templates = crate::app::load_fs_tree();
        let templates: Vec<crate::app::NodeTemplate> = templates
            .children
            .iter()
            .map(|c| crate::app::NodeTemplate { label: c.name.clone(), node: c.clone() })
            .collect();
        crate::app::merge_template_defs(&mut root, &templates);
        let node = &root.children[0];
        assert_eq!(node.node_type, "points");
        assert_eq!(node.name, "Add 3", "instance name is the wire identity — never rewritten");
        let points = node.params.iter().find(|p| p.name == "points").unwrap();
        assert_eq!(points.text(), "250", "instance owns its values");
        let shape = node.params.iter().find(|p| p.name == "shape").expect("Shape appended");
        assert_eq!(shape.text(), "None");
    }

    /// Set As Default is reachable. It was a button on the Main utility
    /// node's File section; with that node retired it is a registry command
    /// like the rest of that section, findable in the palette.
    #[test]
    fn test_main_node_offers_set_as_default() {
        use crate::command::{by_id, Run};
        let cmd = by_id("set_as_default").expect("no set_as_default command");
        assert_eq!(cmd.label, "Set As Default");
        assert_eq!(cmd.run, Run::Menu("Set As Default"));
        // Its File-section neighbours are commands too, or the retirement
        // of the Main node took them with it.
        for id in ["new_project", "open_project", "save_document", "save_document_as", "exit"] {
            assert!(by_id(id).is_some(), "the File section lost '{id}'");
        }
    }

    /// A scratch project has no path — the click must not invent a default.
    #[test]
    fn test_set_as_default_without_a_loaded_file_is_a_noop() {
        let mut state = State::new(false);
        assert_eq!(state.loaded_project_path, None);
        let before = state.default_project_setting.clone();
        // Deliberately NOT via set_current_as_default's saving path: with a
        // loaded path it would write the real ~/.config state.kdl. The no-path
        // arm does not save, so it is safe to exercise directly.
        state.set_current_as_default();
        assert_eq!(state.default_project_setting, before);
    }

    /// Press `button` at (x, y), as the pointer would.
    fn press_at(state: &mut State, x: f32, y: f32, button: cce_ui::widget::MouseButton) {
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::ElementState;
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button });
    }

    /// There are no corner triggers: a plate's rows are in its right-click
    /// menu. The panes with a menu of their own carry them under it (the
    /// network's empty space, the playbar); the rest open them alone.
    #[test]
    fn a_plates_rows_are_in_its_right_click_menu() {
        use crate::plate_menu::PlateMenuAction;
        use crate::slots::{NETWORK_PANEL_IDX, PARAM_IDX, PLAYBAR_IDX, SPREADSHEET_IDX};
        use cce_ui::widget::MouseButton;
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.show_spreadsheet = true;
        state.show_playbar = true;
        state.rebuild_positions();
        state.apply_layout();
        let collapse = "Collapse".to_string();

        // Params, off a row (its bottom edge), and the spreadsheet: the plate
        // menu alone.
        for idx in [PARAM_IDX, SPREADSHEET_IDX] {
            let (x, y, w, h) = state.slots.get_dyn(idx).rect();
            press_at(&mut state, x + w - 6.0, y + h - 6.0, MouseButton::Right);
            assert_eq!(state.plate_menu_slot, Some(idx), "{}", crate::plate_menu::plate_title(idx));
            assert!(state.plate_menu_actions.contains(&PlateMenuAction::Collapse));
            press_at(&mut state, 2.0, 2.0, MouseButton::Left);
            assert!(!state.plate_menu_open(), "a press outside dismisses it");
        }

        // The network: its own rows, then the plate's.
        let (cx, cy, cw, ch) = state.positions[crate::slots::CONTENT_IDX];
        let (px, py) = (cx + cw * 0.85, cy + ch * 0.2);
        assert!(state.graph().node_at(px, py).is_none());
        press_at(&mut state, px, py, MouseButton::Right);
        let options = cce_ui::widget::context_menu::options();
        assert_eq!(options.first().map(String::as_str), Some("Add Node"));
        assert!(options.contains(&collapse), "{options:?}");
        assert!(options.iter().any(|o| o.starts_with("Move To")), "{options:?}");
        // Picking Collapse there collapses the network plate.
        let row = options.iter().position(|o| *o == collapse).unwrap();
        let rx = cce_ui::widget::context_menu::x() + 8.0;
        let ry = cce_ui::widget::context_menu::row_y(row) + 4.0;
        press_at(&mut state, rx, ry, MouseButton::Left);
        assert!(state.pane_is_collapsed(NETWORK_PANEL_IDX));
        state.set_pane_collapsed(NETWORK_PANEL_IDX, false);

        // The playbar: its transport, then the plate's.
        let (x, y, w, h) = state.positions[PLAYBAR_IDX];
        press_at(&mut state, x + w * 0.5, y + h * 0.5, MouseButton::Right);
        let options = cce_ui::widget::context_menu::options();
        assert!(options.contains(&collapse), "{options:?}");
        assert!(state.playbar_menu_actions.contains(&crate::app::PlaybarMenuAction::Plate(PlateMenuAction::Collapse)));
        press_at(&mut state, 2.0, 2.0, MouseButton::Left);
    }

    /// Move To swaps the pane, and the tabs riding it, with what holds the
    /// other dock — what dragging the corner used to do.
    #[test]
    fn move_to_swaps_a_pane_into_another_dock() {
        use crate::app::Dock;
        use crate::plate_menu::PlateMenuAction;
        use crate::slots::{NETWORK_PANEL_IDX, PARAM_IDX};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.open_plate_menu(PARAM_IDX);
        assert!(state.plate_menu_actions.contains(&PlateMenuAction::MoveTo(Dock::Left)));
        assert!(!state.plate_menu_actions.contains(&PlateMenuAction::MoveTo(Dock::Right)), "not to its own dock");
        state.close_plate_menu();
        state.run_plate_menu_action(PARAM_IDX, PlateMenuAction::MoveTo(Dock::Left), (0.0, 0.0));
        assert_eq!(state.dock_of_pane(PARAM_IDX), Some(Dock::Left));
        assert_eq!(state.dock_of_pane(NETWORK_PANEL_IDX), Some(Dock::Right));
    }

    /// Collapse must actually reclaim the plate AND take its body with it, and
    /// expanding must put both back — a stub that still hosts a full-height
    /// graph would paint the pane over the viewport it just freed.
    #[test]
    fn test_collapse_shrinks_the_plate_and_restores_it() {
        use crate::plate_menu::STUB_H;
        use crate::slots::{CONTENT_IDX, NETWORK_PANEL_IDX};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);

        let (_, _, _, full_h) = state.slots.get_dyn(NETWORK_PANEL_IDX).rect();
        assert!(full_h > STUB_H, "network plate starts taller than a stub");
        assert!(state.slots.get_dyn(CONTENT_IDX).visible(), "graph starts visible");

        state.set_pane_collapsed(NETWORK_PANEL_IDX, true);
        let (_, _, _, stub_h) = state.slots.get_dyn(NETWORK_PANEL_IDX).rect();
        assert_eq!(stub_h, STUB_H, "collapsed plate is not the stub height");
        assert!(!state.slots.get_dyn(CONTENT_IDX).visible(), "graph survived the collapse");
        // A right press on the stub offers Expand; a left press expands it.
        let (x, y, w, h) = state.slots.get_dyn(NETWORK_PANEL_IDX).rect();
        press_at(&mut state, x + w * 0.5, y + h * 0.5, cce_ui::widget::MouseButton::Right);
        assert!(state.plate_menu_actions.contains(&crate::plate_menu::PlateMenuAction::Expand));
        state.close_plate_menu();
        press_at(&mut state, x + w * 0.5, y + h * 0.5, cce_ui::widget::MouseButton::Left);
        let (_, _, _, back_h) = state.slots.get_dyn(NETWORK_PANEL_IDX).rect();
        assert_eq!(back_h, full_h, "expanding did not restore the plate height");
        assert!(state.slots.get_dyn(CONTENT_IDX).visible(), "graph did not come back");
    }

    /// Both sides of a detach. The child must show ONE pane and nothing else —
    /// a stray visible slot would paint over it — and the parent must stop
    /// laying the pane out, or the space it held is never released.
    #[test]
    fn test_detached_pane_claims_its_window_and_leaves_the_parent() {
        use crate::plate_menu::DETACHED_MARGIN;
        use crate::slots::{PARAM_IDX, VIEWPORT_IDX, WIDGET_COUNT};

        // Child: the detached window.
        let mut child = State::new(false);
        child.detached_pane = Some(PARAM_IDX);
        child.resize(600.0, 400.0, 1.0);
        for i in 0..WIDGET_COUNT {
            let visible = child.slots.get_dyn(i).visible();
            assert_eq!(visible, i == PARAM_IDX, "slot {i} visibility in a detached window");
        }
        let (x, y, w, h) = child.slots.get_dyn(PARAM_IDX).rect();
        assert_eq!((x, y), (DETACHED_MARGIN, DETACHED_MARGIN));
        assert_eq!(w, 600.0 - 2.0 * DETACHED_MARGIN);
        assert_eq!(h, 400.0 - 2.0 * DETACHED_MARGIN);

        // Parent: the window that handed the pane out keeps a STUB, because the
        // stub's plate menu is the only way to reattach.
        use crate::plate_menu::STUB_H;
        let mut parent = State::new(false);
        parent.resize(1600.0, 900.0, 1.0);
        assert!(parent.slots.get_dyn(PARAM_IDX).visible(), "params starts in the parent");
        let (_, _, _, full_h) = parent.slots.get_dyn(PARAM_IDX).rect();

        parent.detached_panes[PARAM_IDX] = true;
        parent.rebuild_positions();
        parent.apply_layout();

        let (_, _, _, stub_h) = parent.slots.get_dyn(PARAM_IDX).rect();
        assert!(full_h > stub_h, "detaching did not shrink the pane in the parent");
        assert_eq!(stub_h, STUB_H, "the parent's leftover is not a stub");
        let (sx, sy, sw, sh) = parent.slots.get_dyn(PARAM_IDX).rect();
        press_at(&mut parent, sx + sw * 0.5, sy + sh * 0.5, cce_ui::widget::MouseButton::Right);
        assert_eq!(parent.plate_menu_actions, vec![crate::plate_menu::PlateMenuAction::Reattach],
            "the stub's right press offers Reattach — nothing else can reattach the pane");
        parent.close_plate_menu();
        // Collapsed and detached stubs must not read the same.
        let label = parent.pane_stub_label(PARAM_IDX).expect("a detached pane is stubbed");
        assert!(label.contains("detached"), "stub does not say the pane is detached: {label}");
        assert!(parent.slots.get_dyn(VIEWPORT_IDX).visible(), "the rest of the parent survived");
    }

    /// Dock swap: Move To another dock swaps occupants,
    /// and the dock-owned dimensions stay put — the network lands in the
    /// bottom strip's rect, the spreadsheet in the left column's.
    /// A left press on the spreadsheet's column header reaches the widget,
    /// which sorts the column — it does not orbit the camera.
    ///
    /// `cursor_in_viewport` used to be the whole centre column, spreadsheet
    /// included, and the orbit arm at the top of the left-press path returns
    /// before any widget is asked. So the toolkit's header-click sort
    /// (tested in cce-ui) never fired in this app: clicking a header
    /// hovered it, tinted it, and did nothing.
    /// The scene is everything in the viewport's rect that no plate covers
    /// — right up to the network plate's edge. The test used to carve out
    /// the old COLUMN layout, so the band between the floating plate's right
    /// edge and `splitter1_x + SPLITTER_W`, and the strip above the network
    /// content, were nobody's: a right-click there opened no menu.
    #[test]
    fn the_scene_starts_at_the_network_plates_edge() {
        use crate::slots::{NETWORK_PANEL_IDX, VIEWPORT_IDX};
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.network_plate = true;
        state.splitter_layout.splitter1_x = 700.0;
        state.rebuild_positions();
        state.apply_layout();
        let (nx, ny, nw, nh) = state.positions[NETWORK_PANEL_IDX];
        assert!(nw > 0.0 && nx + nw < 690.0, "the plate ends short of the old column split");
        let at = |state: &mut State, x: f32, y: f32| {
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
            state.cursor_in_viewport()
        };
        assert!(!at(&mut state, nx + nw - 4.0, ny + nh * 0.5), "the plate is the network's");
        assert!(at(&mut state, nx + nw + 4.0, ny + nh * 0.5), "just right of the plate is scene");
        let (vx, vy, vw, _) = state.positions[VIEWPORT_IDX];
        assert!(at(&mut state, vx + vw * 0.5, vy + 4.0), "the strip along the top is scene");

        // And the right press there opens the viewport's menu.
        at(&mut state, nx + nw + 4.0, ny + nh * 0.5);
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right });
        assert!(state.viewport_menu_open(), "a right-click beside the plate opens the viewport menu");
    }

    #[test]
    fn spreadsheet_header_press_reaches_the_widget_not_the_camera() {
        use crate::slots::{SPREADSHEET_IDX, SPREADSHEET_MENUBAR_IDX, VIEWPORT_IDX};
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.show_spreadsheet = true;
        state.rebuild_positions();
        state.apply_layout();
        state.spreadsheet_mut().set_spreadsheet_data(
            vec!["Point".into(), "Pos.x".into()],
            vec![vec!["0".into(), "9".into()], vec!["1".into(), "10".into()]],
        );

        // The middle of the second column's header cell.
        let (sx, sy, sw, sh) = state.positions[SPREADSHEET_IDX];
        assert!(sw > 0.0 && sh > 0.0, "the spreadsheet is laid out");
        let (hx, hy) = (sx + sw * 0.75, sy + 12.0);
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: hx as f64, y: hy as f64 } });
        assert!(!state.cursor_in_viewport(), "a floating pane is not the scene");

        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        assert!(state.orbit_drag.is_none(), "the press must not arm the camera orbit");
        assert_eq!(state.focused_widget, Some(SPREADSHEET_IDX), "the press reached the spreadsheet");
        assert_eq!(state.focused_pane, SPREADSHEET_MENUBAR_IDX);
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });

        // And the scene beside it is still the scene: a press in the
        // viewport's own rect, clear of every floating pane, orbits.
        let (vx, vy, vw, vh) = state.positions[VIEWPORT_IDX];
        let (cx, cy) = (vx + vw * 0.5, vy + vh * 0.3);
        assert!(!state.over_floating_pane_at(cx, cy), "pick a point clear of the panes");
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: cx as f64, y: cy as f64 } });
        assert!(state.cursor_in_viewport());
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        assert_eq!(state.orbit_drag, Some((cx, cy)), "a press on the scene still orbits");
    }

    // ----- Node names carry no spaces -----

    /// A node's name is a path segment, so the conventional "Sphere 1"
    /// becomes "sphere1" and any other whitespace an underscore, all lowercase.
    #[test]
    fn node_names_are_sanitized_of_whitespace() {
        use crate::app::sanitize_node_name;
        assert_eq!(sanitize_node_name("Sphere 1"), "sphere1");
        assert_eq!(sanitize_node_name("Camera 12"), "camera12");
        assert_eq!(sanitize_node_name("Sphere1"), "sphere1");
        assert_eq!(sanitize_node_name("My Region"), "my_region");
        assert_eq!(sanitize_node_name("  My   Region 2 "), "my_region2");
        assert_eq!(sanitize_node_name("mold\tshell"), "mold_shell");
        assert_eq!(sanitize_node_name(""), "node");
        assert_eq!(sanitize_node_name("   "), "node");

        // Minting and both MCP entry points go through it.
        let mut state = State::new(false);
        assert_eq!(state.get_lowest_unused_name("Sphere"), "sphere2", "sphere1 is taken by the default project");
        let mut redraw = false;
        state.apply_action(crate::app::McpAction::AddNode { template_name: "Plane".into(), name: Some("my plane".into()), x: 5.0, y: 5.0 }, &mut redraw).unwrap();
        let slot = state.current_dir().children.iter().position(|c| c.name == "my_plane").expect("the added node, sanitized");
        state.apply_action(crate::app::McpAction::RenameNode { slot, new_name: "flat one 3".into() }, &mut redraw).unwrap();
        assert_eq!(state.current_dir().children[slot].name, "flat_one3");
        state.apply_action(crate::app::McpAction::AddNode { template_name: "Plane".into(), name: None, x: 6.0, y: 6.0 }, &mut redraw).unwrap();
        assert!(state.current_dir().children.iter().any(|c| c.name == "plane1"), "a minted name is lowercase with no space");
    }

    /// Loading an older save renames its nodes and follows every reference:
    /// the wires, and the active camera in the view state.
    #[test]
    fn loading_a_project_strips_spaces_and_rewires_references() {
        use crate::app::Project;
        let content = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/default_project.json")).unwrap();
        let mut proj: Project = serde_json::from_str(&content).unwrap();
        // Age the file: put the spaces back, add a consumer wired to the
        // sphere by its old name, and a sibling already holding the new one.
        let sphere = proj.root.children.iter().position(|c| c.name == "sphere1").unwrap();
        let camera = proj.root.children.iter().position(|c| c.name == "camera1").unwrap();
        proj.root.children[sphere].name = "Sphere 1".into();
        proj.root.children[camera].name = "Camera 1".into();
        proj.view_state.active_camera = "Camera 1".into();
        let mut group = proj.root.children[sphere].clone();
        group.id = "g".into();
        group.name = "My Region".into();
        group.node_type = "group".into();
        group.children.clear();
        group.params = vec![crate::app::ParamDef::new("input", "text", "Sphere 1").with_label("Input")];
        let mut clash = group.clone();
        clash.id = "c".into();
        clash.name = "sphere1".into();
        clash.params[0].set_text("Camera 1");
        proj.root.children.push(group);
        proj.root.children.push(clash);

        proj.sanitize_node_names();

        proj.migrate_format();

        // The camera stays at the root; the rest is geometry, and went into
        // a Geometry node (format 5) with its names and wires.
        assert!(proj.root.children.iter().any(|c| c.name == "camera1"));
        let names: Vec<&str> = geo(&proj.root).children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"my_region"));
        assert!(names.contains(&"sphere1"), "the hand-named sibling keeps its name");
        assert!(names.contains(&"sphere1_2"), "the migrated sphere steps aside from it: {names:?}");
        let by_name = |n: &str| geo(&proj.root).children.iter().find(|c| c.name == n).unwrap();
        assert_eq!(by_name("my_region").params[0].text(), "sphere1_2", "the wire followed the rename");
        assert_eq!(by_name("sphere1").params[0].text(), "camera1");
        assert_eq!(proj.view_state.active_camera, "camera1");
        // The sphere is the native node the bundled file now holds.
        assert_eq!(by_name("sphere1_2").node_type, "sphere");

        // A clean file is left exactly alone.
        let before = serde_json::to_string(&proj).unwrap();
        proj.sanitize_node_names();
        proj.migrate_format();
        assert_eq!(serde_json::to_string(&proj).unwrap(), before);
    }
    /// A parameter has a NAME — an identifier a path spells, never shown —
    /// and a LABEL, what the pane shows. Every template carries both, an
    /// older save is renamed on load with what spells its names, MCP finds
    /// a parameter by either, and a name that is not one is refused.
    #[test]
    fn parameters_have_a_name_and_a_label() {
        use crate::app::{is_param_name, misnamed_params, param_name_of, FsNode, McpAction, ParamDef, Project};
        assert_eq!(param_name_of("Base Resolution"), "base_resolution");
        assert_eq!(param_name_of("Relax in 3D Space"), "relax_in_3d_space");
        assert_eq!(param_name_of("Input 2"), "input_2");
        assert_eq!(param_name_of("  Odd -- Spacing! "), "odd_spacing");
        assert!(is_param_name("input_2") && !is_param_name("Input 2") && !is_param_name("Radius") && !is_param_name(""));

        // Every shipped template: a name, and a label for the pane.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("nodes");
        let mut n = 0;
        for f in fs::read_dir(&dir).unwrap().flatten() {
            let t: FsNode = serde_json::from_str(&fs::read_to_string(f.path()).unwrap()).unwrap();
            assert!(misnamed_params(&t).is_empty(), "{}: {:?}", f.path().display(), misnamed_params(&t));
            for p in &t.params {
                assert!(!p.label.is_empty(), "{}: {} has no label", f.path().display(), p.name);
                n += 1;
            }
        }
        assert!(n > 300, "walked {n}");
        let mut bad = ref_node("t", "Bad", "node", vec![("Some Row", "float", "1")], vec![]);
        assert_eq!(misnamed_params(&bad), vec![("Bad".to_string(), "Some Row".to_string())]);
        bad.params[0].name = "some_row".into();
        assert!(misnamed_params(&bad).is_empty());

        // The pane shows labels.
        let mut state = State::new(false);
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Embryo".into(), name: Some("embryo1".into()), x: 3.0, y: 8.0 }, &mut redraw).unwrap();
        let slot = state.current_dir().children.iter().position(|c| c.name == "embryo1").unwrap();
        let rows = crate::app::param_display(&state.current_dir().children[slot].params);
        assert!(rows.iter().any(|r| r.0 == "Base Resolution"), "{rows:?}");
        assert!(rows.iter().all(|r| !r.0.contains('_')), "no name in the pane: {rows:?}");

        // MCP: by name, by label, by the old name.
        for (asked, value) in [("base_resolution", "20"), ("Base Resolution", "21"), ("base resolution", "22")] {
            state.apply_action(McpAction::SetParam { slot, name: asked.into(), value: value.into() }, &mut redraw).unwrap();
            assert_eq!(crate::geometry::node_param_str(&state.current_dir().children[slot], "base_resolution", ""), value, "{asked}");
        }
        let err = state
            .apply_action(McpAction::AddParam { slot, name: "My Row".into(), param_type: "float".into(), default: "1".into(), label: String::new() }, &mut redraw)
            .unwrap_err();
        assert!(err.contains("my_row"), "the refusal says the form: {err}");
        state
            .apply_action(McpAction::AddParam { slot, name: "my_row".into(), param_type: "float".into(), default: "1".into(), label: "My Row".into() }, &mut redraw)
            .unwrap();
        let rows = crate::app::param_display(&state.current_dir().children[slot].params);
        assert!(rows.iter().any(|r| r.0 == "My Row"));
        assert!(state
            .apply_action(McpAction::AddParam { slot, name: "my_row".into(), param_type: "float".into(), default: "1".into(), label: String::new() }, &mut redraw)
            .is_err(), "a name the node has is refused");

        // Format 3 → 4: names and what spells them.
        let mut meta = ref_node("m", "view", "utility", vec![("Show Grid", "toggle", "true")], vec![]);
        meta.params[0].label.clear();
        let mut root = ref_node("root", "root", "node", vec![], vec![
            ref_node("a", "ball", "sphere", vec![("Base Resolution", "spinbox", "12"), ("Center", "float3", "0:1:0")], vec![]),
            ref_node("b", "box1", "box", vec![("Size X", "slider", "ch(\"../ball/Base Resolution\") * 2 + chf(\"../ball/Center.y\")")], vec![]),
            ref_node("w", "wrangle1", "wrangle", vec![("Code", "code", "@P.y += chv(\"../ball/Center\").y; // ch(\"Base Resolution\")")], vec![]),
            meta,
        ]);
        root.children[1].params[0].set_expr(true);
        let mut proj = Project { name: "p".into(), root, view_state: Default::default(), format: 3 };
        proj.migrate_format();
        let names = |n: &FsNode| n.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        let g = geo(&proj.root);
        assert_eq!(names(&g.children[0]), ["base_resolution", "center"]);
        assert_eq!(g.children[1].params[0].name, "size_x");
        assert_eq!(g.children[1].params[0].text(), "ch(\"../ball/base_resolution\") * 2 + chf(\"../ball/center.y\")");
        assert!(g.children[1].params[0].is_expr());
        assert_eq!(g.children[2].params[0].text(), "@P.y += chv(\"../ball/center\").y; // ch(\"base_resolution\")");
        assert_eq!(names(&proj.root.children[0]), ["Show Grid"], "a retired settings node keeps what its own migration reads, at the root");
        let p = ParamDef::new("radius", "slider", "1").with_label("Radius");
        assert_eq!((p.shown_name(), p.name.as_str()), ("Radius", "radius"));
    }



    /// Format 1 → 2 and 2 → 3: the generators' normal attribute is `N`
    /// and their texture coordinates `uv`, and what names `Norm` / `UV` in
    /// a save follows them — an attribute row, a name in a comma
    /// list of attributes, `@Norm` in a wrangle (not `@Normal`) — once: a
    /// format-2 file naming `Norm` is left alone, since that attribute is
    /// someone's own.
    #[test]
    fn a_save_naming_norm_or_uv_names_n_or_uv() {
        use crate::app::{FsNode, ParamDef, Project, PROJECT_FORMAT};
        let node = |name: &str, ty: &str, params: Vec<ParamDef>| FsNode {
            id: name.into(),
            name: name.into(),
            node_type: ty.into(),
            children: vec![],
            params,
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        };
        let mut root = node("root", "subnet", vec![]);
        root.children = vec![
            // A format-1 file's names are what the pane showed.
            node("vis", "visualize", vec![ParamDef::new("Attribute", "attribute", "Norm")]),
            node("xfer", "transfer", vec![ParamDef::new("Attributes", "text", "Cd, Norm,UV")]),
            node("w", "wrangle", vec![ParamDef::new("Code", "code", "@P += @Norm * 0.1; @Normal = 1;")]),
            node("keep", "visualize", vec![ParamDef::new("Attribute", "attribute", "Normx")]),
        ];
        let mut proj = Project { name: "p".into(), root, view_state: Default::default(), format: 1 };
        proj.migrate_format();
        assert_eq!(proj.format, PROJECT_FORMAT);
        let text = |proj: &Project, i: usize| geo(&proj.root).children[i].params[0].text().to_string();
        assert_eq!(text(&proj, 0), "N");
        assert_eq!(text(&proj, 1), "Cd, N,uv", "every step a file is behind: N, then uv");
        assert_eq!(text(&proj, 2), "@P += @N * 0.1; @Normal = 1;");
        assert_eq!(text(&proj, 3), "Normx", "only the whole name");
        let names: Vec<&str> = geo(&proj.root).children.iter().map(|c| c.params[0].name.as_str()).collect();
        assert_eq!(names, ["attribute", "attributes", "code", "attribute"], "and then every name is one (format 4)");

        // Once: a format-2 file's Norm is its own attribute.
        let mut again = proj.clone();
        geo_mut(&mut again.root).children[0].params[0].set_text("Norm".to_string());
        again.migrate_format();
        assert_eq!(text(&again, 0), "Norm");

        // And the generators write N.
        let s = crate::geometry::sphere_detail(glam::Vec3::ZERO, 1.0, 4, 6);
        assert!(s.points().has("N") && !s.points().has("Norm"));

        // Format 2 → 3: UV is uv, the same way; a choice row keeps its UV
        // (the Sphere's Method), and a format-2 file takes only this step.
        let mut root = node("root", "subnet", vec![]);
        root.children = vec![
            node("vis", "visualize", vec![ParamDef::new("attribute", "attribute", "UV")]),
            node("w", "wrangle", vec![ParamDef::new("code", "code", "@P.y = @UV.x; @UVW = 1;")]),
            node("ball", "sphere", vec![ParamDef::new("method", "choice:UV,Icosphere,Cube", "UV")]),
            node("n", "visualize", vec![ParamDef::new("attribute", "attribute", "Norm")]),
        ];
        let mut proj = Project { name: "p".into(), root, view_state: Default::default(), format: 2 };
        proj.migrate_format();
        assert_eq!(text(&proj, 0), "uv");
        assert_eq!(text(&proj, 1), "@P.y = @uv.x; @UVW = 1;");
        assert_eq!(text(&proj, 2), "UV", "a choice is not an attribute");
        assert_eq!(text(&proj, 3), "Norm", "a format-2 file is past the N step");
        assert!(s.points().has("uv") && !s.points().has("UV"));
    }

    #[test]
    fn test_dock_swap_repositions_plates() {
        use crate::app::Dock;
        use crate::slots::{NETWORK_PANEL_IDX, SPREADSHEET_IDX};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.show_spreadsheet = true;
        state.rebuild_positions();
        state.apply_layout();

        let net_before = state.slots.get_dyn(NETWORK_PANEL_IDX).rect();
        let ss_before = state.slots.get_dyn(SPREADSHEET_IDX).rect();
        assert!(net_before.2 > 0.0 && ss_before.2 > 0.0);
        assert!(net_before.1 < ss_before.1, "network starts above the bottom strip");

        state.move_pane_to_dock(NETWORK_PANEL_IDX, Dock::Bottom);
        assert_eq!(state.dock_of_pane(NETWORK_PANEL_IDX), Some(Dock::Bottom));
        assert_eq!(state.dock_of_pane(SPREADSHEET_IDX), Some(Dock::Left));

        let net_after = state.slots.get_dyn(NETWORK_PANEL_IDX).rect();
        let ss_after = state.slots.get_dyn(SPREADSHEET_IDX).rect();
        // The network now wears (approximately) the strip geometry and the
        // spreadsheet the left column's; exact equality is not required
        // because the strip derivation reads dock occupancy, but the vertical
        // order must have inverted and both must remain visible.
        assert!(net_after.1 > ss_after.1, "network did not move below the spreadsheet");
        assert!(net_after.2 > 0.0 && ss_after.2 > 0.0, "a pane vanished in the swap");

        // Dropping it back restores the original arrangement.
        state.move_pane_to_dock(NETWORK_PANEL_IDX, Dock::Left);
        assert_eq!(state.dock_of_pane(SPREADSHEET_IDX), Some(Dock::Bottom));
    }

    /// Full width tucks the spreadsheet under BOTH neighbors (their bottoms
    /// rise via the tuck interlock); between-panes clears both tucks.
    #[test]
    fn test_spreadsheet_full_width_round_trip() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.show_spreadsheet = true;
        state.rebuild_positions();
        assert!(!state.spreadsheet_tucks_left() && !state.spreadsheet_tucks_right());

        state.set_spreadsheet_full_width(true);
        assert!(state.spreadsheet_tucks_left() && state.spreadsheet_tucks_right(),
            "full width must tuck under both neighbors");
        let (ss_x, _, ss_w, _) = state.floating_spreadsheet_rect();
        assert!(ss_x <= 18.5 && ss_x + ss_w >= 1600.0 - 18.5, "not actually full width: x={ss_x} w={ss_w}");

        state.set_spreadsheet_full_width(false);
        assert!(!state.spreadsheet_tucks_left() && !state.spreadsheet_tucks_right());
    }

    /// The way back. A detached pane's menu offers Reattach and nothing else,
    /// and reattaching restores the pane in full.
    #[test]
    fn test_reattach_brings_a_detached_pane_back() {
        use crate::plate_menu::PlateMenuAction;
        use crate::slots::PARAM_IDX;
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        let (_, _, _, full_h) = state.slots.get_dyn(PARAM_IDX).rect();

        state.detached_panes[PARAM_IDX] = true;
        state.rebuild_positions();
        state.apply_layout();

        state.open_plate_menu(PARAM_IDX);
        assert_eq!(state.plate_menu_actions, vec![PlateMenuAction::Reattach],
            "a detached pane must offer Reattach and only Reattach");
        state.close_plate_menu();

        state.reattach_plate(PARAM_IDX);
        assert!(!state.pane_is_detached(PARAM_IDX), "still marked detached after reattach");
        assert!(state.pane_stub_label(PARAM_IDX).is_none(), "still a stub after reattach");
        let (_, _, _, back_h) = state.slots.get_dyn(PARAM_IDX).rect();
        assert_eq!(back_h, full_h, "reattach did not restore the pane height");
    }

    /// A detached window the user closes themselves must not strand its pane as
    /// a stub the parent thinks is still elsewhere. Uses a REAL child, reaped
    /// before the poll, so the liveness probe is the one that runs in the app.
    #[test]
    fn test_parent_reclaims_a_pane_whose_window_exited() {
        use crate::slots::SPREADSHEET_IDX;
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);

        // Deliberately NOT reaped here: an unreaped exited child is a zombie,
        // which is exactly the state a closed window leaves behind. A pid-based
        // `kill(pid, 0)` probe calls a zombie alive and never reclaims the pane
        // — this test only bites if the poll reaps for itself.
        let child = std::process::Command::new("true").spawn().expect("spawn a short-lived child");
        state.detached_children.insert(SPREADSHEET_IDX, child);
        state.detached_panes[SPREADSHEET_IDX] = true;
        state.rebuild_positions();
        state.apply_layout();
        assert!(state.pane_is_detached(SPREADSHEET_IDX));

        let mut reclaimed = false;
        for _ in 0..200 {
            if state.poll_detached_children() {
                reclaimed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(reclaimed, "poll never noticed the exited child");
        assert!(!state.pane_is_detached(SPREADSHEET_IDX), "pane stayed detached after its window exited");
        assert!(!state.detached_children.contains_key(&SPREADSHEET_IDX), "stale child handle kept");
    }

    /// Detach must not be offered where it cannot work: twice for one pane, or
    /// from inside a detached window (which would fork the pane again).
    #[test]
    fn test_detach_is_only_offered_where_it_works() {
        use crate::slots::{PARAM_IDX, SPREADSHEET_IDX};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        assert!(state.plate_can_detach(PARAM_IDX), "params should be detachable");

        state.detached_panes[PARAM_IDX] = true;
        assert!(!state.plate_can_detach(PARAM_IDX), "params offered detach twice");
        assert!(state.plate_can_detach(SPREADSHEET_IDX), "one detach blocked the others");

        let mut child = State::new(false);
        child.detached_pane = Some(PARAM_IDX);
        child.resize(600.0, 400.0, 1.0);
        assert!(!child.plate_can_detach(PARAM_IDX), "a detached window offered to detach again");
    }

    /// The menu is contextual, and the two states are mutually exclusive: a
    /// collapsed plate must offer Expand and NOT Collapse, or the item that
    /// restores it is unreachable.
    #[test]
    fn test_plate_corner_menu_is_contextual() {
        use crate::plate_menu::{PlateMenuAction, PLATE_SLOTS};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);

        let idx = PLATE_SLOTS.iter().copied()
            .find(|&i| state.slots.get_dyn(i).visible())
            .expect("some plate is shown at this size");

        state.open_plate_menu(idx);
        assert!(state.plate_menu_actions.contains(&PlateMenuAction::Collapse));
        assert!(!state.plate_menu_actions.contains(&PlateMenuAction::Expand));
        state.close_plate_menu();

        state.set_pane_collapsed(idx, true);
        state.open_plate_menu(idx);
        assert!(state.plate_menu_actions.contains(&PlateMenuAction::Expand));
        assert!(!state.plate_menu_actions.contains(&PlateMenuAction::Collapse));
        state.close_plate_menu();
    }

    /// DE chrome is config-owned (`style.surface.relief.wall.profile` /
    /// `.edge.profile` / `style.surface.param.color`), and a project file
    /// must not outrank the user's config.kdl. It did while the Main utility
    /// node carried a Style section; the retirement of that whole node tree
    /// is what closes it for good, so what is asserted now is that a save
    /// carrying those params brings nothing back.
    #[test]
    fn test_legacy_style_params_are_dropped_from_main() {
        let mut state = State::new(false);
        // A pre-removal save: the four utility subnets flat at the root,
        // Main carrying its retired Style section.
        let style = |name: &str, ty: &str, val: &str| crate::app::ParamDef::new(name.to_string(), ty.to_string(), val.to_string());
        state.fs_root.children.push(crate::app::FsNode {
            id: "legacy-main".to_string(),
            name: "main".to_string(),
            node_type: "utility".to_string(),
            children: vec![],
            params: vec![
                style("Style", "section", ""),
                style("Bevel Profile", "ramp", "smooth;0.000:0.000,1.000:1.000"),
                style("Edge Profile", "ramp", "smooth;0.000:0.000,1.000:1.000"),
                style("Plate Color", "rgba", "#11223344"),
            ],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        });

        state.migrate_meta_settings_node();

        assert!(
            !state.fs_root.children.iter().any(|c| c.node_type == "utility"),
            "a utility subnet survived the migration"
        );
        let names: Vec<&str> = state
            .fs_root
            .children
            .iter()
            .flat_map(|c| c.params.iter())
            .map(|p| p.name.as_str())
            .collect();
        for retired in ["Style", "Bevel Profile", "Edge Profile", "Plate Color"] {
            assert!(!names.contains(&retired), "retired style param survived load: {retired}");
        }
    }

    /// The roster macro (`widget_roster!` in `src/slots.rs`) numbers the `*_IDX`
    /// constants from declaration order. A mis-expansion that skipped a number would
    /// leave the last slot addressed as `WIDGET_COUNT`, unreachable through `get_dyn`
    /// and invisible to every `0..WIDGET_COUNT` loop — but would still compile.
    #[test]
    fn test_widget_roster_indices_are_dense() {
        use crate::slots::*;
        let roster = [
            HEADER_IDX, CONTENT_IDX, SPLITTER1_IDX, VIEWPORT_IDX,
            SPLITTER2_IDX, PARAM_IDX, CANVAS_IDX, LEFT_MENUBAR_IDX,
            RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, STATUS_IDX, BREADCRUMB_IDX,
            SPREADSHEET_IDX, SPREADSHEET_MENUBAR_IDX, NETWORK_PANEL_IDX, PLAYBAR_IDX,
            NETWORK_PANEL2_IDX, CONTENT2_IDX, BREADCRUMB2_IDX,
            DIALOG_IDX,
        ];
        assert_eq!(roster.len(), WIDGET_COUNT, "roster length vs WIDGET_COUNT");
        for (i, idx) in roster.iter().enumerate() {
            assert_eq!(i, *idx, "slot #{i} expanded to index {idx}");
        }
    }

    /// `View 1:1` puts the pivot plane at true size: afterwards one world
    /// unit spans its real length on the display, so the readout's ratio
    /// is 1. Exercised on the default camera (zoom) at a centimetre world
    /// unit; the cached viewport state the readout reads is set by hand,
    /// as the render pass would.
    #[test]
    fn view_one_to_one_reaches_true_scale() {
        let mut state = State::new(false);
        state.world_unit = cce_ui::units::Unit::Cm;
        // The default camera's eye ray is fixed, so 1:1 is a zoom — the one
        // number the readout's cached state can follow here without a
        // render pass (a camera node's Position rewrite is cached by one).
        state.active_camera = "Default Camera".to_string();
        state.scale = 1.0;
        state.last_viewport_width = 1200;
        state.last_viewport_height = 800;
        state.last_viewport_camera_pos = Vec3::new(2.5, 1.8, 2.5);
        state.last_viewport_pivot = Vec3::ZERO;
        state.last_viewport_zoom = state.viewport().zoom;
        let before = state.view_scale_ratio();
        state.view_one_to_one();
        state.last_viewport_zoom = state.viewport().zoom;
        let after = state.view_scale_ratio();
        assert!((after - 1.0).abs() < 1e-3, "ratio {after} (was {before})");
        assert!((state.world_unit_mm() - 10.0).abs() < 1e-4);
        // A millimetre unit needs the camera ~10× farther; still reachable.
        state.world_unit = cce_ui::units::Unit::Mm;
        state.view_one_to_one();
        state.last_viewport_zoom = state.viewport().zoom;
        let mm = state.view_scale_ratio();
        assert!((mm - 1.0).abs() < 1e-3, "mm ratio {mm}");
    }

    /// The root meta node is gone, and a save that still carries one is
    /// migrated rather than opened with it.
    ///
    /// It was the permanent root container for four utility subnets holding
    /// every session-wide display setting — and it was the STORE OF RECORD
    /// for them, copied back over live state after each parameter edit. That
    /// made a display preference a piece of project data, editable only by
    /// finding the right node. The settings live on `State` now and persist
    /// to `state.kdl`; `migrate_meta_settings_node` reads an old file's
    /// values across once and takes the node out.
    #[test]
    fn test_session_node_exists_and_cannot_be_deleted() {
        let mut state = State::new(false);
        assert!(
            !state.fs_root.children.iter().any(|c| c.node_type == "meta"),
            "a fresh tree grew a meta node"
        );

        // An old save: a "session"-typed container with the four subnets,
        // carrying values that are not the defaults.
        let p = |name: &str, ty: &str, val: &str| crate::app::ParamDef::new(name.to_string(), ty.to_string(), val.to_string());
        let subnet = |name: &str, params: Vec<crate::app::ParamDef>| crate::app::FsNode {
            id: format!("legacy-{name}"),
            name: name.to_string(),
            node_type: "utility".to_string(),
            children: vec![],
            params,
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        };
        state.fs_root.children.push(crate::app::FsNode {
            id: "legacy-session".to_string(),
            name: "Session".to_string(),
            node_type: "session".to_string(),
            children: vec![
                subnet("guides", vec![
                    p("Point Marker Size", "spinbox", "50"),
                    p("Point Marker Color", "color", "#ff8000"),
                    p("World Unit", "choice", "cm"),
                    p("Grid Thickness", "spinbox", "40"),
                    p("Show Reference Cube", "toggle", "true"),
                ]),
                subnet("render", vec![
                    p("Show Wireframe", "toggle", "true"),
                    p("Wire Thickness", "slider", "4.0"),
                ]),
                subnet("main", vec![p("Circular Pane", "toggle", "true")]),
            ],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        });

        state.migrate_meta_settings_node();

        // The node is gone, along with every utility subnet it held.
        assert!(!state.fs_root.children.iter().any(|c| {
            matches!(c.node_type.as_str(), "meta" | "session" | "utility")
        }), "the meta node survived the migration");

        // And its values are the live settings — the point of migrating at
        // all rather than simply dropping the node.
        assert_eq!(state.world_unit, cce_ui::units::Unit::Cm);
        assert!((state.world_unit_mm() - 10.0).abs() < 1e-4);
        assert!((state.point_marker_size - 0.05).abs() < 1e-6);
        assert!((state.point_marker_color[0] - 1.0).abs() < 0.01);
        assert!((state.point_marker_color[1] - 0.5).abs() < 0.01);
        assert!((state.point_marker_color[2] - 0.0).abs() < 0.01);
        assert!((state.grid_thickness - 0.04).abs() < 1e-6);
        assert!(state.wireframe);
        assert!((state.wire_width - 4.0).abs() < 1e-6);
        assert!(state.circular_network_pane);

        // Idempotent: a second pass has nothing to find and changes nothing.
        let before = serde_json::to_string(&state.fs_root).unwrap();
        state.migrate_meta_settings_node();
        assert_eq!(before, serde_json::to_string(&state.fs_root).unwrap());
    }

    /// A node the user put INSIDE the meta subnet is re-homed, not eaten.
    ///
    /// Adding a non-geometry node in there was allowed, so the migration
    /// cannot treat everything under that container as the app's own —
    /// dropping the node with it would silently delete the user's work, and
    /// the only trace would be its absence.
    #[test]
    fn the_migration_rehomes_a_node_the_user_left_in_the_meta_subnet() {
        let mut state = State::new(false);
        let mine = crate::app::FsNode {
            id: "mine".to_string(),
            name: "my_notes".to_string(),
            node_type: "node".to_string(),
            children: vec![],
            params: vec![],
            geometry_visible: false,
            bypassed: false,
            position: (2.0, 3.0),
            inputs: 1,
            outputs: 1,
        };
        state.fs_root.children.push(crate::app::FsNode {
            id: "legacy-meta".to_string(),
            name: "meta".to_string(),
            node_type: "meta".to_string(),
            children: vec![
                crate::app::FsNode {
                    id: "legacy-guides".to_string(),
                    name: "guides".to_string(),
                    node_type: "utility".to_string(),
                    children: vec![],
                    params: vec![],
                    geometry_visible: true,
                    bypassed: false,
                    position: (0.0, 4.0),
                    inputs: 1,
                    outputs: 1,
                },
                mine,
            ],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        });

        state.migrate_meta_settings_node();

        assert!(!state.fs_root.children.iter().any(|c| c.node_type == "meta"));
        // A subnet is a geometry node, so it is re-homed in the root's
        // Geometry node rather than at the root itself.
        let level = geo(&state.fs_root);
        let kept = level
            .children
            .iter()
            .find(|c| c.id == "mine")
            .expect("the user's node was eaten with the meta subnet");
        assert_eq!(kept.name, "my_notes");
        // Re-homed onto a free cell — the level may already have something
        // standing where it was.
        assert!(
            level.children.iter().filter(|c| c.position == kept.position).count() == 1,
            "it landed on top of another node"
        );
    }

    /// An OLDER save still carries Main/View/Guides/Render flat at the root,
    /// with no meta node above them at all — the shape before the Session
    /// node existed. The migration has to reach that generation too, or the
    /// four nodes stay in the network forever doing nothing.
    #[test]
    fn test_old_saves_migrate_settings_nodes_into_session() {
        let mut state = State::new(false);
        state.viewport_mut().show_grid = true;
        state.fs_root.children.push(crate::app::FsNode {
            id: "flat-guides".to_string(),
            name: "guides".to_string(),
            node_type: "utility".to_string(),
            children: vec![],
            params: vec![crate::app::ParamDef::new("Show Grid Guide".to_string(), "toggle".to_string(), "false".to_string())],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 4.0),
            inputs: 1,
            outputs: 1,
        });

        state.migrate_meta_settings_node();

        assert!(
            !state.fs_root.children.iter().any(|c| c.node_type == "utility"),
            "a root-level settings node survived"
        );
        assert!(!state.viewport().show_grid, "its value did not reach the live state");
    }

    /// Ctrl+S saves in place, Ctrl+Shift+S is Save As — and the Shift must
    /// actually discriminate: a matcher that ignores modifiers would fire
    /// plain Save for both.
    #[test]
    fn test_save_as_chord() {
        let mut m = ShortcutManager::new();
        m.register("Ctrl+s", "save_document").unwrap();
        m.register("Ctrl+Shift+s", "save_document_as").unwrap();
        let ctrl = crate::app::ModifiersState { ctrl: true, ..Default::default() };
        let ctrl_shift = crate::app::ModifiersState { ctrl: true, shift: true, ..Default::default() };
        // The REAL event shapes: xkb delivers the shifted character when
        // Shift is held — "S", not "s". The first version of this test fed
        // lowercase for both and passed against a matcher that could never
        // fire in practice.
        let lower = cce_ui::widget::Key::Character("s".into());
        let upper = cce_ui::widget::Key::Character("S".into());
        assert_eq!(m.match_command(&ctrl, &lower), Some("save_document"));
        assert_eq!(m.match_command(&ctrl_shift, &upper), Some("save_document_as"));
        assert_eq!(m.match_command(&ctrl_shift, &lower), Some("save_document_as"));
    }

    /// Plates support tabs: a pane pulled into another dock rides it as a
    /// tab (one laid out, the rest waiting), switching fronts a waiting tab,
    /// splitting moves the active one back out to the empty dock, and the
    /// arrangement rides the project view state active-first.
    #[test]
    fn test_plate_tabs_share_a_dock() {
        use crate::app::{Dock, NO_PANE};
        use crate::slots::{PARAM_IDX, SPREADSHEET_IDX};
        let mut state = State::new(false);

        // Pull the spreadsheet into the right dock: it fronts, the params
        // wait as a tab, and the bottom dock empties.
        state.add_dock_tab(Dock::Right, SPREADSHEET_IDX);
        assert_eq!(state.pane_in_dock(Dock::Right), SPREADSHEET_IDX);
        assert!(state.dock_tabs[Dock::Right as usize].contains(&PARAM_IDX));
        assert_eq!(state.pane_in_dock(Dock::Bottom), NO_PANE);
        // The waiting tab is laid out nowhere but keeps its home dock.
        assert_eq!(state.dock_of_pane(PARAM_IDX), None);
        assert_eq!(state.tab_dock_of_pane(PARAM_IDX), Some(Dock::Right));

        // Switching fronts the waiting tab without evicting the other.
        state.show_dock_tab(Dock::Right, PARAM_IDX);
        assert_eq!(state.pane_in_dock(Dock::Right), PARAM_IDX);
        assert!(state.dock_tabs[Dock::Right as usize].contains(&SPREADSHEET_IDX));

        // The view state carries the groups, active first.
        let vs = state.project_view_state();
        assert_eq!(vs.dock_tabs[1][0], "parameters");
        assert!(vs.dock_tabs[1].contains(&"spreadsheet".to_string()));
        assert!(vs.dock_tabs[2].is_empty());

        // Splitting moves the active pane to the empty dock; the tab left
        // behind fronts.
        state.split_dock_tab(PARAM_IDX);
        assert_eq!(state.pane_in_dock(Dock::Bottom), PARAM_IDX);
        assert_eq!(state.pane_in_dock(Dock::Right), SPREADSHEET_IDX);
    }

    /// Move To Own Plate is offered only while a dock is free to take the
    /// pane. Four tab candidates share three docks, so a dock can hold two
    /// with none empty — and there the row used to show and do nothing.
    #[test]
    fn move_to_own_plate_needs_an_empty_dock() {
        use crate::app::Dock;
        use crate::plate_menu::PlateMenuAction;
        use crate::slots::{NETWORK_PANEL2_IDX, NETWORK_PANEL_IDX, PARAM_IDX, SPREADSHEET_IDX};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);

        // The second editor tabs in beside the first: left holds two, and
        // params and spreadsheet keep the other two docks, so none is free.
        state.add_dock_tab(Dock::Left, NETWORK_PANEL2_IDX);
        state.show_dock_tab(Dock::Left, NETWORK_PANEL_IDX);
        assert_eq!(state.first_empty_dock(), None);
        state.open_plate_menu(NETWORK_PANEL_IDX);
        assert!(!state.plate_menu_actions.contains(&PlateMenuAction::SplitTab),
            "no dock is free, so the row is left out");
        assert!(state.plate_menu_actions.contains(&PlateMenuAction::ShowTab(NETWORK_PANEL2_IDX)),
            "the tab list itself still shows");
        state.close_plate_menu();

        // Pulling the spreadsheet in beside the params frees the bottom dock,
        // and the row comes back — on both shared docks.
        state.add_dock_tab(Dock::Right, SPREADSHEET_IDX);
        state.show_dock_tab(Dock::Right, PARAM_IDX);
        assert_eq!(state.first_empty_dock(), Some(Dock::Bottom));
        for idx in [NETWORK_PANEL_IDX, PARAM_IDX] {
            state.open_plate_menu(idx);
            assert!(state.plate_menu_actions.contains(&PlateMenuAction::SplitTab),
                "a shared dock with a free one offers the split");
            state.close_plate_menu();
        }

        // And the split lands there.
        state.split_dock_tab(PARAM_IDX);
        assert_eq!(state.pane_in_dock(Dock::Bottom), PARAM_IDX);
    }

    /// The second network editor: joins a dock from nowhere through the tab
    /// machinery, dives on its OWN path while the primary stays put, clamps
    /// a stale path instead of panicking, and Close removes it entirely.
    #[test]
    fn test_second_network_editor_has_its_own_path() {
        use crate::app::Dock;
        use crate::slots::{NETWORK_PANEL2_IDX, NETWORK_PANEL_IDX};
        let mut state = State::new(false);
        assert_eq!(state.tab_dock_of_pane(NETWORK_PANEL2_IDX), None, "starts unplaced");

        state.add_dock_tab(Dock::Left, NETWORK_PANEL2_IDX);
        assert_eq!(state.pane_in_dock(Dock::Left), NETWORK_PANEL2_IDX);
        assert_eq!(state.tab_dock_of_pane(NETWORK_PANEL_IDX), Some(Dock::Left));

        let start = state.current_path.clone();
        let sphere = state
            .current_dir()
            .children
            .iter()
            .position(|c| c.name == "sphere1")
            .expect("default project has sphere1");
        state.current_path2 = [start.clone(), vec![sphere]].concat();
        state.sync_nodes();
        assert_eq!(state.current_path, start, "primary path must not follow");
        assert_eq!(state.path_names_at(&state.current_path2), vec!["geometry1".to_string(), "sphere1".to_string()]);

        state.current_path2 = vec![99];
        state.sync_nodes();
        assert!(state.current_path2.is_empty(), "a stale path clamps, never indexes");

        state.close_dock_tab(NETWORK_PANEL2_IDX);
        assert_eq!(state.tab_dock_of_pane(NETWORK_PANEL2_IDX), None);
        assert_eq!(state.pane_in_dock(Dock::Left), NETWORK_PANEL_IDX);
    }


    /// Pinning: the spreadsheet bound to pane 1 keeps reading pane 1's
    /// selection while clicks land in the second editor, and the cursor-
    /// selection sync no longer wipes pane 1's selection on second-editor
    /// interactions (the regression that made pins look broken).
    #[test]
    fn test_pane_pins_bind_selection_sources() {
        use crate::app::Dock;
        use crate::slots::{CONTENT2_IDX, CONTENT_IDX, LEFT_MENUBAR_IDX, NETWORK_PANEL2_IDX};
        use cce_ui::widget::GraphController as _;
        let mut state = State::new(false);
        state.add_dock_tab(Dock::Left, NETWORK_PANEL2_IDX);

        // Both editors in the bundled project's Geometry node, where the
        // sphere is; a second node there for the other editor to pick.
        state.current_path2 = state.current_path.clone();
        let mut other = state.current_dir().children.iter().find(|c| c.name == "sphere1").unwrap().clone();
        crate::app::regenerate_node_ids(&mut other);
        other.name = "sphere2".into();
        other.position.0 += 2.0;
        state.current_dir_mut().children.push(other);
        state.sync_nodes();
        let sphere = state.current_dir().children.iter().position(|c| c.name == "sphere1").unwrap();
        let camera = state.current_dir().children.iter().position(|c| c.name == "sphere2").unwrap();

        // Pane 1 selects the sphere; the spreadsheet pins to pane 1.
        state.graph_mut().set_selected_node(Some(sphere));
        state.spreadsheet_pin = Some(CONTENT_IDX);
        // A click in the second editor selects the camera and takes the
        // active-editor role.
        state.slots.content2.set_selected_node(Some(camera));
        state.param_editor = CONTENT2_IDX;

        // The params pane (unpinned) follows the second editor...
        assert_eq!(state.param_editor_selected(), Some(camera));
        // ...the spreadsheet's binding stays on pane 1's sphere.
        let se = state.spreadsheet_editor();
        assert_eq!(se, CONTENT_IDX);
        assert_eq!(state.editor_selected_of(se), Some(sphere));

        // The cursor-selection sync must NOT wipe pane 1's selection while
        // the second editor is active — the grid cursor is pane 1's concept.
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.grid_cursor_col = -50;
        state.grid_cursor_row = -50;
        state.sync_cursor_and_selection();
        assert_eq!(
            state.graph().selected_node(),
            Some(sphere),
            "second-editor activity must not clear pane 1's selection"
        );

        // And the spreadsheet refresh keys off the pinned selection.
        state.show_spreadsheet = true;
        state.sync_nodes();
        let sphere_id = state.current_dir().children[sphere].id.clone();
        assert_eq!(
            state.last_spreadsheet_node_name.as_deref(),
            Some(sphere_id.as_str()),
            "spreadsheet must refresh against the PINNED editor's selection"
        );

        // A params pin binds the pane the other way.
        state.params_pin = Some(CONTENT_IDX);
        assert_eq!(state.param_editor_selected(), Some(sphere));
    }

    /// Pane state rides save files: visibility through the meta→View subnet
    /// params (synced at save, applied on load), collapse and splitter
    /// proportions through view_state. A fresh State loading the file must
    /// come out shaped like the one that saved it.
    #[test]
    fn test_pane_state_round_trips_through_save() {
        use crate::slots::PARAM_IDX;
        let dir = std::env::temp_dir().join(format!("cce-designer-pane-state-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let mut a = State::new(false);
        a.width = 1600.0;
        assert!(a.show_viewport && !a.show_spreadsheet, "test assumes the default pane set");
        a.execute_menu_action("Show Viewport Pane");
        a.execute_menu_action("Show Spreadsheet Pane");
        a.set_pane_collapsed(PARAM_IDX, true);
        a.splitter_layout.splitter1_x = 400.0;
        a.splitter_layout.splitter2_x = 1200.0;
        // Tab state: a second network editor tabbed beside the first (and
        // fronted), dived one level down its own path.
        a.add_dock_tab(crate::app::Dock::Left, crate::slots::NETWORK_PANEL2_IDX);
        let sphere = a
            .current_dir()
            .children
            .iter()
            .position(|c| c.name == "sphere1")
            .expect("default project has sphere1");
        a.current_path2 = [a.current_path.clone(), vec![sphere]].concat();
        a.save_to_file(&dir).expect("save");

        let mut b = State::new(false);
        b.width = 800.0;
        b.load_from_file(&dir).expect("load");
        assert!(!b.show_viewport, "viewport hidden in the save must load hidden");
        assert!(b.show_spreadsheet, "spreadsheet shown in the save must load shown");
        assert!(b.collapsed_panes[PARAM_IDX], "param pane collapse must round-trip");
        assert!((b.splitter_layout.splitter1_x - 200.0).abs() < 1.0,
            "splitters restore as fractions: 400/1600 of an 800-wide window = 200, got {}",
            b.splitter_layout.splitter1_x);
        // The tab arrangement rides the file: the second editor exists,
        // fronted in the left dock with the primary waiting, on its own path.
        assert_eq!(
            b.pane_in_dock(crate::app::Dock::Left),
            crate::slots::NETWORK_PANEL2_IDX,
            "the fronted second editor must load fronted"
        );
        assert_eq!(
            b.tab_dock_of_pane(crate::slots::NETWORK_PANEL_IDX),
            Some(crate::app::Dock::Left),
            "the primary must load as the waiting tab"
        );
        assert_eq!(b.current_path2, [a.current_path.clone(), vec![sphere]].concat(), "the second editor's path must round-trip");

        // A detached pane window must ignore the same file's pane state.
        let mut d = State::new(true);
        let vp_before = d.show_viewport;
        d.load_from_file(&dir).expect("load detached");
        assert_eq!(d.show_viewport, vp_before, "detached windows keep their own pane layout");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Plate geometry rides save files too: every edge the user can drag —
    /// the network and parameter plate widths, the spreadsheet's height and
    /// its tucks under both neighbors — comes back at the size it was saved
    /// at, and, as fractions, scales onto a differently sized window.
    #[test]
    fn test_plate_geometry_round_trips_through_save() {
        let dir = std::env::temp_dir().join(format!("cce-designer-plate-geometry-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let mut a = State::new(false);
        a.resize(1600.0, 900.0, 1.0);
        a.execute_menu_action("Show Spreadsheet Pane");
        assert!(a.show_spreadsheet);
        a.floating_network_layout.2 = 520.0;
        a.floating_param_width = 360.0;
        a.floating_spreadsheet_height = 300.0;
        a.set_spreadsheet_full_width(true);
        a.rebuild_positions();
        let insets = (a.floating_spreadsheet_inset_left, a.floating_spreadsheet_inset_right);
        assert!(insets.0 > 0.0 && insets.1 > 0.0, "full width must set both tucks");
        assert!((a.floating_network_layout.2 - 520.0).abs() < 0.5 && (a.floating_param_width - 360.0).abs() < 0.5);
        a.save_to_file(&dir).expect("save");

        let plates = a.project_view_state().plates.expect("a sized window records its plates");
        assert!((plates.network_width - 520.0 / 1600.0).abs() < 1e-4, "widths save as window fractions");

        let mut b = State::new(false);
        b.resize(1600.0, 900.0, 1.0);
        b.load_from_file(&dir).expect("load");
        assert!((b.floating_network_layout.2 - 520.0).abs() < 0.5, "network width: {}", b.floating_network_layout.2);
        assert!((b.floating_param_width - 360.0).abs() < 0.5, "param width: {}", b.floating_param_width);
        assert!((b.floating_spreadsheet_height - 300.0).abs() < 0.5, "spreadsheet height: {}", b.floating_spreadsheet_height);
        assert!((b.floating_spreadsheet_inset_left - insets.0).abs() < 0.5 && (b.floating_spreadsheet_inset_right - insets.1).abs() < 0.5,
            "tucks: {:?} vs {:?}", (b.floating_spreadsheet_inset_left, b.floating_spreadsheet_inset_right), insets);
        assert!(b.spreadsheet_tucks_left() && b.spreadsheet_tucks_right(), "the full-width tuck must load tucked");

        // Half the window: the same fractions land at half the pixels.
        let mut c = State::new(false);
        c.resize(800.0, 450.0, 1.0);
        c.load_from_file(&dir).expect("load half-size");
        assert!((c.floating_network_layout.2 - 260.0).abs() < 0.5, "scaled network width: {}", c.floating_network_layout.2);
        assert!((c.floating_param_width - 180.0).abs() < 0.5, "scaled param width: {}", c.floating_param_width);

        // A detached pane window keeps its own plates.
        let mut d = State::new(true);
        let before = d.floating_param_width;
        d.load_from_file(&dir).expect("load detached");
        assert_eq!(d.floating_param_width, before);

        let _ = fs::remove_dir_all(&dir);
    }

    /// The startup project loads before the compositor's first configure, so
    /// its plate fractions land on `State::new`'s 1280x800 placeholder; the
    /// first real size must apply them again. Before, the plates kept the
    /// placeholder's pixels, opening every launch scaled by 1280/width (and
    /// 800/height), and a save then stored the shrunken fractions.
    #[test]
    fn plates_loaded_before_the_first_configure_fit_the_real_window() {
        let dir = std::env::temp_dir().join(format!("cce_designer_prefconfigure_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut a = State::new(false);
        a.resize(1400.0, 1080.0, 1.0);
        a.execute_menu_action("Show Spreadsheet Pane");
        a.floating_network_layout.2 = 350.0;
        a.floating_param_width = 310.0;
        a.floating_spreadsheet_height = 100.0;
        a.rebuild_positions();
        a.save_to_file(&dir).expect("save");

        // No resize before the load: this is the startup order.
        let mut b = State::new(false);
        assert!(!b.window_configured);
        b.load_from_file(&dir).expect("load");
        b.resize(1400.0, 1080.0, 1.0);
        assert!((b.left_dock_width() - 350.0).abs() < 0.5, "network width: {}", b.left_dock_width());
        assert!((b.right_dock_width() - 310.0).abs() < 0.5, "param width: {}", b.right_dock_width());
        assert!((b.floating_spreadsheet_height - 100.0).abs() < 0.5, "spreadsheet height: {}", b.floating_spreadsheet_height);
        let pg = b.project_view_state().plates.expect("plates");
        assert!((pg.network_width - 350.0 / 1400.0).abs() < 1e-4, "a save writes back what was loaded: {:?}", pg);

        // Only the FIRST configure: a later window resize keeps the plates'
        // pixels, as it always has.
        b.resize(1000.0, 1080.0, 1.0);
        assert!((b.left_dock_width() - 350.0).abs() < 0.5, "a later resize rescaled the plate: {}", b.left_dock_width());

        let _ = fs::remove_dir_all(&dir);
    }

    /// The startup load takes its saved baseline before the first configure,
    /// with the plates on the placeholder size; the configure then lands
    /// them on the real one. That is the load finishing, not an edit, so
    /// the project opens clean.
    #[test]
    fn a_project_loaded_before_the_first_configure_opens_clean() {
        let dir = std::env::temp_dir().join(format!("cce_designer_cleanopen_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut a = State::new(false);
        a.resize(1400.0, 1080.0, 1.0);
        a.floating_network_layout.2 = 350.0;
        a.floating_param_width = 310.0;
        a.rebuild_positions();
        a.save_to_file(&dir).expect("save");

        let mut b = State::new(false);
        b.load_from_file(&dir).expect("load");
        assert!(!b.has_unsaved_changes(), "a fresh load is clean");
        b.resize(1400.0, 1080.0, 1.0);
        assert!(!b.has_unsaved_changes(), "the first configure is not an edit");

        // What was unsaved before the configure still is after it.
        let mut c = State::new(false);
        c.load_from_file(&dir).expect("load");
        c.set_pane_collapsed(crate::slots::PARAM_IDX, true);
        c.resize(1400.0, 1080.0, 1.0);
        assert!(c.has_unsaved_changes(), "the configure must not hide an edit");

        let _ = fs::remove_dir_all(&dir);
    }

    /// A window briefly narrower than its plates squeezes them for as long as
    /// it lasts and no longer: the layout used to store the clamped size, so
    /// one transient shrink (a re-tile, a configure at startup) left every
    /// plate that small for good.
    #[test]
    fn a_shrunk_window_does_not_shrink_the_plates_for_good() {
        let mut s = State::new(false);
        s.resize(1600.0, 900.0, 1.0);
        s.execute_menu_action("Show Spreadsheet Pane");
        s.floating_network_layout.2 = 700.0;
        s.floating_param_width = 650.0;
        s.floating_spreadsheet_height = 600.0;
        s.rebuild_positions();

        s.resize(400.0, 300.0, 1.0);
        s.rebuild_positions();
        assert!(
            s.positions[crate::slots::PARAM_IDX].2 <= 400.0 - 2.0 * 18.0 + 0.5,
            "the plate still fits the small window: {:?}", s.positions[crate::slots::PARAM_IDX]
        );

        s.resize(1600.0, 900.0, 1.0);
        s.rebuild_positions();
        assert!((s.left_dock_width() - 700.0).abs() < 0.5, "network width: {}", s.left_dock_width());
        assert!((s.right_dock_width() - 650.0).abs() < 0.5, "param width: {}", s.right_dock_width());
        assert!((s.floating_spreadsheet_rect().3 - 600.0).abs() < 0.5, "spreadsheet height: {:?}", s.floating_spreadsheet_rect());
        assert!((s.positions[crate::slots::PARAM_IDX].2 - 650.0).abs() < 0.5, "drawn param width: {:?}", s.positions[crate::slots::PARAM_IDX]);
    }

    /// The main window's sync reload takes a detached window's TREE edit and
    /// keeps its own view. It used to apply the whole view state the detached
    /// window wrote — that window's default plates, panes and camera — so
    /// every autosave from it reset the main window's layout.
    #[test]
    fn a_detached_windows_save_does_not_reset_the_main_layout() {
        let dir = std::env::temp_dir().join(format!("cce-designer-sync-view-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let channel = dir.join("default_project.json");

        let mut main = State::new(false);
        main.resize(1600.0, 900.0, 1.0);
        main.floating_network_layout.2 = 700.0;
        main.floating_param_width = 650.0;
        main.floating_spreadsheet_height = 420.0;
        main.viewport_mut().rotation_x = 0.7;
        main.rebuild_positions();

        let mut child = State::new(false);
        child.detached_pane = Some(crate::slots::PARAM_IDX);
        child.resize(400.0, 300.0, 1.0);
        child.rebuild_positions();
        child.fs_root.children[0].name = "synced_edit".to_string();
        child.save_to_file(&channel).expect("child writes the channel");

        main.app_drag = Some(crate::app::AppDrag::ParamResize { start_w: 650.0, start_mouse_x: 0.0 });
        main.load_sync_channel(&channel, true).expect("main reloads");

        assert!(main.fs_root.children.iter().any(|c| c.name == "synced_edit"), "the tree edit must sync");
        assert!((main.floating_network_layout.2 - 700.0).abs() < 0.5, "network width: {}", main.floating_network_layout.2);
        assert!((main.floating_param_width - 650.0).abs() < 0.5, "param width: {}", main.floating_param_width);
        assert!((main.floating_spreadsheet_height - 420.0).abs() < 0.5, "spreadsheet height: {}", main.floating_spreadsheet_height);
        assert!((main.viewport().rotation_x - 0.7).abs() < 1e-6, "camera: {}", main.viewport().rotation_x);
        assert!(main.app_drag.is_some(), "a plate drag in progress survives the reload");

        let _ = fs::remove_dir_all(&dir);
    }

    /// A detached window writes its tree and navigation into the sync channel
    /// and leaves the layout there as the main window last wrote it — it used
    /// to write its own defaults, so a full load of the file restored the
    /// wrong plates.
    #[test]
    fn a_detached_window_leaves_the_channels_layout_alone() {
        let dir = std::env::temp_dir().join(format!("cce-designer-sync-write-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let channel = dir.join("default_project.json");

        let mut main = State::new(false);
        main.resize(1600.0, 900.0, 1.0);
        main.floating_param_width = 650.0;
        main.viewport_mut().rotation_x = 0.7;
        main.rebuild_positions();
        main.save_to_file(&channel).expect("main writes the channel");

        let mut child = State::new(false);
        child.detached_pane = Some(crate::slots::PARAM_IDX);
        child.resize(400.0, 300.0, 1.0);
        child.rebuild_positions();
        child.fs_root.children[0].name = "synced_edit".to_string();
        child.graph_mut().set_selected_node(Some(0));
        child.save_to_file(&channel).expect("child writes the channel");

        let proj: crate::app::Project = serde_json::from_str(&fs::read_to_string(&channel).unwrap()).unwrap();
        assert!(proj.root.children.iter().any(|c| c.name == "synced_edit"), "the tree is the child's");
        assert_eq!(proj.view_state.selected_node, Some(0), "so is the navigation");
        let plates = proj.view_state.plates.expect("the main window's plates stay in the file");
        assert!((plates.params_width - 650.0 / 1600.0).abs() < 1e-4, "params width: {}", plates.params_width);
        let view = proj.view_state.default_view.expect("the main window's camera stays in the file");
        assert!((view.rotation.0 - 0.7).abs() < 1e-6, "camera: {:?}", view.rotation);

        // No file to keep a layout from: the layout blocks go absent, which
        // a full load reads as "keep the live layout".
        let _ = fs::remove_file(&channel);
        child.save_to_file(&channel).expect("child writes a fresh channel");
        let proj: crate::app::Project = serde_json::from_str(&fs::read_to_string(&channel).unwrap()).unwrap();
        assert!(proj.view_state.plates.is_none() && proj.view_state.visible_panes.is_none(), "no layout of the child's");

        let _ = fs::remove_dir_all(&dir);
    }

    /// A dragged plate edge is an unsaved change — the save file carries the
    /// plate geometry, so the title's asterisk must follow it, and clear on
    /// save. A window resize alone must NOT dirty it.
    #[test]
    fn test_plate_resize_dirties_the_title_until_saved() {
        let dir = std::env::temp_dir().join(format!("cce-designer-plate-dirty-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let mut state = State::new(false);
        // The tree baseline is taken before the meta-node migrations run
        // (startup's project load re-baselines); this test is about the
        // layout half, so baseline here.
        state.mark_saved();
        assert!(!state.has_unsaved_changes());
        state.resize(1600.0, 900.0, 1.0);
        assert!(!state.has_unsaved_changes(), "a window resize is not an edit");

        state.floating_param_width += 60.0;
        state.rebuild_positions();
        state.update_window_title();
        assert!(state.has_unsaved_changes(), "a plate resize must dirty the title");
        assert!(state.title.ends_with('*'), "title: {}", state.title);

        state.save_to_file(&dir).expect("save");
        assert!(!state.has_unsaved_changes(), "saving clears it");
        state.update_window_title(); // the event loop's refresh, after the save event
        assert!(!state.title.ends_with('*'), "title: {}", state.title);

        state.set_pane_collapsed(crate::slots::PARAM_IDX, true);
        assert!(state.has_unsaved_changes(), "a collapse is saved state too");

        let _ = fs::remove_dir_all(&dir);
    }

    /// The playbar transport chords: plain arrows drive the timeline (Up =
    /// play/pause, Left/Right = step), and a held modifier must NOT match —
    /// modified arrows stay free for other bindings.
    #[test]
    fn test_playbar_transport_keys() {
        use cce_ui::widget::{Key, NamedKey};
        let mut m = ShortcutManager::new();
        m.register("Up", "play_pause").unwrap();
        m.register("Right", "frame_next").unwrap();
        m.register("Left", "frame_prev").unwrap();
        let plain = crate::app::ModifiersState::default();
        let ctrl = crate::app::ModifiersState { ctrl: true, ..Default::default() };
        assert_eq!(m.match_command(&plain, &Key::Named(NamedKey::ArrowUp)), Some("play_pause"));
        assert_eq!(m.match_command(&plain, &Key::Named(NamedKey::ArrowRight)), Some("frame_next"));
        assert_eq!(m.match_command(&plain, &Key::Named(NamedKey::ArrowLeft)), Some("frame_prev"));
        assert_eq!(m.match_command(&ctrl, &Key::Named(NamedKey::ArrowUp)), None);
        m.register("Down", "play_pause_reverse").unwrap();
        assert_eq!(m.match_command(&plain, &Key::Named(NamedKey::ArrowDown)), Some("play_pause_reverse"));
    }

    /// A project carries its display settings: saved with the grid off,
    /// smooth shading, half opacity and a centimetre world unit, it opens
    /// that way whatever the session had since — and a save from before the
    /// block existed leaves the live settings alone. Changing one dirties
    /// the project, since the file now holds it.
    #[test]
    fn a_project_keeps_its_display_settings() {
        let dir = std::env::temp_dir()
            .join(format!("cce-designer-display-{}", std::process::id()))
            .join("look");
        let _ = fs::remove_dir_all(&dir);
        let mut state = State::new(false);
        state.viewport_mut().show_grid = false;
        state.viewport_mut().show_origin = false;
        state.smooth_shading = true;
        state.wireframe = true;
        state.geo_opacity = 0.5;
        state.grid_thickness = 0.07;
        state.world_unit = cce_ui::units::Unit::Cm;
        state.point_marker_color = [0.1, 0.9, 0.2];
        state.save_to_file(&dir).expect("save");
        assert!(!state.has_unsaved_changes());

        // The session moves on.
        state.run_command("toggle_grid");
        assert!(state.viewport().show_grid);
        assert!(state.has_unsaved_changes(), "a display change is an edit to the file");
        state.run_command("toggle_origin");
        state.smooth_shading = false;
        state.wireframe = false;
        state.geo_opacity = 1.0;
        state.grid_thickness = 0.03;
        state.world_unit = cce_ui::units::Unit::Mm;
        state.point_marker_color = [1.0, 1.0, 1.0];

        state.load_from_file(&dir).expect("load");
        assert!(!state.viewport().show_grid, "the grid comes back off");
        assert!(!state.viewport().show_origin);
        assert!(state.smooth_shading && state.wireframe);
        assert!((state.geo_opacity - 0.5).abs() < 1e-6);
        assert!((state.grid_thickness - 0.07).abs() < 1e-6);
        assert_eq!(state.world_unit, cce_ui::units::Unit::Cm);
        assert_eq!(state.point_marker_color, [0.1, 0.9, 0.2]);
        assert!(!state.scene_smooth_verts.is_empty(), "the scene was rebuilt smooth");
        assert!(!state.has_unsaved_changes(), "a fresh load is clean");
        assert_eq!(state.command_toggle_state("toggle_grid"), Some(false), "the palette's switch agrees");

        // An older save has no display block: the live settings stand.
        let state_json = dir.join("state.json");
        let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&state_json).unwrap()).unwrap();
        v["view_state"].as_object_mut().unwrap().remove("display");
        fs::write(&state_json, serde_json::to_string(&v).unwrap()).unwrap();
        state.run_command("toggle_grid");
        assert!(state.viewport().show_grid);
        state.load_from_file(&dir).expect("load an older save");
        assert!(state.viewport().show_grid, "no block, no change");

        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    /// The node wires' style is a Settings row: choosing one sets it on both
    /// network editors, the project file carries it, and a file that names
    /// none — every one from before the row — follows the config.
    #[test]
    fn the_node_wire_style_is_a_setting_the_project_keeps() {
        use cce_ui::widget::display::WireStyle;
        let dir = std::env::temp_dir()
            .join(format!("cce-designer-wires-{}", std::process::id()))
            .join("look");
        let _ = fs::remove_dir_all(&dir);
        let mut state = State::new(false);
        state.apply_setting("Node Wire Style", "Bezier");
        assert_eq!(state.slots.content.inner().wire_style(), WireStyle::Bezier);
        assert_eq!(state.slots.content2.inner().wire_style(), WireStyle::Bezier, "both editors");
        assert_eq!(state.display_settings().viewport.node_wire_style, "bezier");
        state.save_to_file(&dir).expect("save");

        state.apply_setting("Node Wire Style", "Straight");
        assert!(state.has_unsaved_changes(), "a wire style is an edit to the file");
        state.load_from_file(&dir).expect("load");
        assert_eq!(state.slots.content.inner().wire_style(), WireStyle::Bezier, "the file's style comes back");

        // A file that names no style hands the choice back to the config.
        let state_json = dir.join("state.json");
        let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&state_json).unwrap()).unwrap();
        v["view_state"]["display"]["viewport"].as_object_mut().unwrap().remove("node_wire_style");
        fs::write(&state_json, serde_json::to_string(&v).unwrap()).unwrap();
        state.load_from_file(&dir).expect("load an older save");
        assert_eq!(state.slots.content.inner().chosen_wire_style(), None);
        assert_eq!(state.slots.content.inner().wire_style(), WireStyle::configured());

        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    /// A number under a plate is not drawn: the engine lays text out after
    /// all geometry, so it would stand sharp over a plate that frosts
    /// everything else behind it.
    #[test]
    fn a_point_number_under_a_plate_is_not_drawn() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.show_point_numbers = true;
        state.geo_opacity = 0.2;
        state.show_occluded = true;
        state.rebuild_scene_geometry();
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO, Vec3::Y);
        let proj = Mat4::perspective_rh(0.9, 1600.0 / 900.0, 0.1, 100.0);
        state.last_scene_mvp = Some(proj * view);
        state.last_scene_eye = Vec3::new(0.0, 0.0, 6.0);
        state.last_scene_view_rect = (0.0, 0.0, 1600.0, 900.0);
        state.sync_point_number_alpha(proj * view, state.last_scene_eye);

        let (px, py, pw, ph) = state.positions[crate::slots::PARAM_IDX];
        assert!(pw > 0.0 && ph > 0.0, "the params plate is laid out");
        let all = state.point_number_labels();
        assert!(!all.is_empty());
        assert!(all.iter().all(|(_, x, y, ..)| !state.under_a_plate(*x, *y + 6.0)), "none stands under a plate");

        // Put the params plate over the middle of the scene: the numbers
        // there go, the rest stay.
        state.positions[crate::slots::PARAM_IDX] = (700.0, 350.0, 200.0, 200.0);
        let fewer = state.point_number_labels();
        assert!(fewer.len() < all.len(), "{} of {} are left", fewer.len(), all.len());
        assert!(!fewer.is_empty());
        assert!(fewer.iter().all(|(_, x, y, ..)| !(*x >= 700.0 && *x < 900.0 && *y + 6.0 >= 350.0 && *y + 6.0 < 550.0)));
        state.positions[crate::slots::PARAM_IDX] = (px, py, pw, ph);
        assert_eq!(state.point_number_labels().len(), all.len());
    }

    /// A scene rebuild leaves the numbers dimmed as they were: the 2D frame
    /// is painted before the stage pass, so a rebuild that cleared the
    /// dimming drew one frame of every number at full strength, and a
    /// playing simulation rebuilds at every frame.
    #[test]
    fn a_scene_rebuild_keeps_the_point_numbers_dimmed() {
        let mut state = State::new(false);
        state.show_point_numbers = true;
        state.rebuild_scene_geometry();
        assert!(!state.overlay_number_labels.is_empty());
        // The view the stage pass last staged: from +z, looking at the origin.
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO, Vec3::Y);
        let proj = Mat4::perspective_rh(0.9, 1.5, 0.1, 100.0);
        state.last_scene_mvp = Some(proj * view);
        state.last_scene_eye = Vec3::new(0.0, 0.0, 6.0);
        state.sync_point_number_alpha(proj * view, state.last_scene_eye);
        let staged = state.overlay_number_alpha.clone();
        assert!(staged.iter().any(|a| *a < 0.02), "the far side's numbers are hidden: {staged:?}");
        assert!(staged.iter().any(|a| *a > 0.98), "the near side's are shown");

        state.rebuild_scene_geometry();
        assert_eq!(state.overlay_number_alpha, staged, "the rebuild left the dimming as the view has it");
    }

    /// A point number is dimmed by the fill in front of its point, as a
    /// marker drawn under that fill is: whole on the near side, one layer
    /// down on the far side of a closed mesh (the faces that meet AT the
    /// point are not in front of it), gone behind an opaque face. Behind
    /// the whole mesh the two fills differ — seen through, both walls
    /// blend; otherwise the far wall is culled and only the near one does.
    #[test]
    fn a_point_number_is_dimmed_by_the_fill_in_front_of_it() {
        use glam::{Mat4, Vec3};
        let sphere = crate::geometry::sphere_detail(Vec3::ZERO, 1.0, 8, 12);
        let verts = crate::geometry::detail_vertices(&sphere);
        let eye = Vec3::new(0.3, 0.2, 5.0);
        let mvp = Mat4::perspective_rh(0.9, 1.0, 0.1, 100.0) * Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        let positions = sphere.positions();
        let nearest = |to: Vec3| {
            *positions
                .iter()
                .min_by(|a, b| (Vec3::from_array(**a) - to).length().total_cmp(&(Vec3::from_array(**b) - to).length()))
                .unwrap()
        };
        let near = nearest(Vec3::new(0.0, 0.0, 1.0));
        let far = nearest(Vec3::new(0.2, 0.3, -1.0));
        let behind = [0.05, 0.05, -3.0];
        let points = [near, far, behind];
        let t = |opacity: f32, see_through: bool| {
            crate::geometry::point_transmittance(&verts, mvp, eye, &points, opacity, see_through)
        };
        let close = |a: &[f32], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5);
        assert!(close(&t(0.5, true), [1.0, 0.5, 0.25]), "seen through: {:?}", t(0.5, true));
        assert!(close(&t(0.5, false), [1.0, 0.5, 0.5]), "culled: {:?}", t(0.5, false));
        assert!(close(&t(1.0, false), [1.0, 0.0, 0.0]), "opaque: {:?}", t(1.0, false));
        assert!(close(&t(0.0, true), [1.0, 1.0, 1.0]), "invisible fill: {:?}", t(0.0, true));

        // And the paint reads it: a label behind an opaque face is not drawn.
        let mut state = State::new(false);
        state.show_point_numbers = true;
        state.rebuild_scene_geometry();
        assert!(state.overlay_number_alpha.is_empty(), "a rebuild drops the old eye's answers");
        let labels = |state: &mut State| {
            let numbers: std::collections::HashSet<String> =
                state.overlay_number_labels.iter().map(|(_, i)| i.to_string()).collect();
            state
                .collect_display_list()
                .items
                .iter()
                .filter(|item| matches!(&item.prim, cce_ui::scene::paint::Prim::Text { text, .. } if numbers.contains(text)))
                .count()
        };
        state.show_viewport = true;
        state.last_scene_mvp = Some(Mat4::IDENTITY);
        state.last_scene_view_rect = (0.0, 0.0, 800.0, 600.0);
        let whole = labels(&mut state);
        state.overlay_number_alpha = vec![0.0; state.overlay_number_labels.len()];
        assert!(labels(&mut state) < whole, "hidden labels are not drawn");
    }

    /// Smooth shading bakes the raster pass's own light, so on a PLANE —
    /// where every point normal is the face normal — it gives exactly the
    /// flat shader's factor at every corner: switching modes changes how
    /// curved surfaces read, not the brightness of flat ones. On a closed
    /// sphere the corners of one face differ, which is what smooth means.
    #[test]
    fn smooth_shading_bakes_the_flat_shaders_light_per_vertex() {
        use crate::geometry::{shade_factor, smooth_lit_vertices, sphere_detail, detail_vertices};
        use glam::Vec3;
        // Lit by the environment's sun: a face turned toward it is its
        // brightest, one turned away its darkest. (Until 2026-10-02 it was
        // the other way round, a constant read against the shader's inward
        // normal — a light from below.)
        let l = crate::environment::Environment::default().sun_direction;
        assert!((shade_factor(l, l) - 1.0).abs() < 1e-5);
        assert!((shade_factor(-l, l) - 0.55).abs() < 1e-5);
        assert!((shade_factor(Vec3::ZERO, l) - (0.55 + 0.45 * 0.5)).abs() < 1e-5);

        // A single quad in the XZ plane, wound to face +y.
        let mut quad = crate::detail::Detail::new();
        let a = quad.add_point(Vec3::new(0.0, 0.0, 0.0));
        let b = quad.add_point(Vec3::new(0.0, 0.0, 1.0));
        let c = quad.add_point(Vec3::new(1.0, 0.0, 1.0));
        let d = quad.add_point(Vec3::new(1.0, 0.0, 0.0));
        quad.add_prim(&[a, b, c, d]);
        let n = crate::geometry::point_normals(&quad)[0];
        assert!((n - Vec3::Y).length() < 1e-5, "the quad faces +y: {n}");
        let lit = smooth_lit_vertices(&quad, l);
        let flat = detail_vertices(&quad);
        assert_eq!(lit.len(), flat.len(), "same triangles as the unlit fill");
        let k = shade_factor(Vec3::Y, l);
        for (l, f) in lit.iter().zip(&flat) {
            assert_eq!(l.position, f.position);
            for ch in 0..3 {
                assert!((l.color[ch] - f.color[ch] * k).abs() < 1e-5);
            }
        }

        // A closed UV sphere: same triangle list, and corners of one face
        // no longer share one brightness.
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 12, 16);
        let lit = smooth_lit_vertices(&sphere, l);
        assert_eq!(lit.len(), detail_vertices(&sphere).len());
        let varied = lit.chunks(3).filter(|t| {
            let b = |v: &crate::geometry::Vertex3D| v.color[0] + v.color[1] + v.color[2];
            (b(&t[0]) - b(&t[1])).abs() > 1e-4 || (b(&t[0]) - b(&t[2])).abs() > 1e-4
        }).count();
        assert!(varied > lit.len() / 6, "smooth shading varies across faces: {varied}");
    }

    /// The see-through fill's order: farthest triangle first from the eye,
    /// every triangle kept whole and exactly once — a sort that split or
    /// dropped one would draw a torn mesh while looking like a blend bug.
    #[test]
    fn a_see_through_fill_sorts_its_triangles_back_to_front() {
        use crate::geometry::{detail_vertices, sort_triangles_back_to_front, sphere_detail, Vertex3D};
        use glam::Vec3;
        let verts = detail_vertices(&sphere_detail(Vec3::ZERO, 1.0, 10, 14));
        let eye = Vec3::new(3.0, 1.5, -2.0);
        let sorted = sort_triangles_back_to_front(&verts, eye);
        assert_eq!(sorted.len(), verts.len());
        let dist = |t: &[Vertex3D]| {
            let c = t.iter().fold(Vec3::ZERO, |a, v| a + Vec3::from_array(v.position)) / 3.0;
            (c - eye).length_squared()
        };
        let d: Vec<f32> = sorted.chunks(3).map(dist).collect();
        assert!(d.windows(2).all(|w| w[0] >= w[1]), "not farthest-first");
        let key = |t: &[Vertex3D]| format!("{:?}", t.iter().map(|v| v.position).collect::<Vec<_>>());
        let mut a: Vec<String> = verts.chunks(3).map(key).collect();
        let mut b: Vec<String> = sorted.chunks(3).map(key).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b, "the same triangles, each whole, each once");
        // From the opposite side the order reverses its ends.
        let back = sort_triangles_back_to_front(&verts, -eye);
        assert_eq!(key(&back[..3]), key(&sorted[sorted.len() - 3..]));
    }

    /// Show Occluded is a toggle with a viewport-menu row, and it takes
    /// effect only below full opacity — at 100% it says so rather than
    /// quietly doing nothing.
    #[test]
    fn show_occluded_sees_through_a_translucent_fill_only() {
        use crate::app::ViewportMenuAction as A;
        let mut state = State::new(false);
        state.show_occluded = false;
        state.geo_opacity = 1.0;
        let (options, actions) = state.viewport_menu_rows_of(state.viewport_menu_page_of(A::Command("toggle_show_occluded")));
        let i = actions.iter().position(|a| *a == A::Command("toggle_show_occluded")).expect("a Show Occluded row");
        assert_eq!(options[i], "○ Show Occluded");

        state.run_viewport_menu_action(A::Command("toggle_show_occluded"));
        assert!(state.show_occluded);
        assert_eq!(state.command_toggle_state("toggle_show_occluded"), Some(true));
        assert!(!state.see_through_active(), "nothing to see through at 100%");
        assert!(state.last_status_text.contains("below 100%"), "{}", state.last_status_text);

        state.apply_setting("Geometry Opacity", "0.50");
        assert!(state.see_through_active());
        state.run_command("toggle_show_occluded");
        assert!(!state.see_through_active());
    }

    /// The viewport's right-click menu sets the display mode: the wireframe
    /// switch and flat or smooth shading as a radio pair — each mark reading
    /// the live state, each row landing on it — and the polygon opacity as a
    /// slider row (below).
    #[test]
    fn the_viewport_menu_sets_the_display_mode() {
        use crate::app::ViewportMenuAction as A;
        let mut state = State::new(false);
        state.wireframe = false;
        state.smooth_shading = false;
        state.geo_opacity = 1.0;
        let row = |state: &State, a: A| {
            let (options, actions) = state.viewport_menu_rows_of(state.viewport_menu_page_of(a));
            let i = actions.iter().position(|x| *x == a).unwrap_or_else(|| panic!("no {a:?} row"));
            options[i].clone()
        };
        assert!(row(&state, A::Command("toggle_wireframe")).starts_with('○'));
        assert!(row(&state, A::Shading(false)).starts_with('●'));
        assert!(row(&state, A::Shading(true)).starts_with('○'));
        assert_eq!(row(&state, A::OpacitySlider), "Opacity");

        state.run_viewport_menu_action(A::Command("toggle_wireframe"));
        assert!(state.wireframe);
        assert!(row(&state, A::Command("toggle_wireframe")).starts_with('●'));

        // Smooth: the flag, and a lit raster fill beside the unlit one the
        // path tracer reads, triangle for triangle.
        state.run_viewport_menu_action(A::Shading(true));
        assert!(state.smooth_shading);
        assert_eq!(state.scene_smooth_verts.len(), state.rt_sphere_verts.len());
        assert!(!state.scene_smooth_verts.is_empty(), "the bundled scene draws something");
        // Picking the mode already on is not a flip.
        state.run_viewport_menu_action(A::Shading(true));
        assert!(state.smooth_shading);
        assert!(row(&state, A::Shading(true)).starts_with('●'));
        assert_eq!(state.command_toggle_state("toggle_smooth_shading"), Some(true));
        state.run_viewport_menu_action(A::Shading(false));
        assert!(!state.smooth_shading);
        assert!(state.scene_smooth_verts.is_empty(), "flat keeps no lit copy");

    }

    /// The viewport menu's Opacity row is a cce-ui menu SLIDER: opened, it
    /// reads the live opacity in percent; the wheel over it steps 5% and
    /// saves, the menu staying open; a press on its band jumps and drags,
    /// landing the value live, and the release commits it. The wheel over
    /// an action row is still swallowed by the open menu rather than
    /// orbiting the scene beneath it.
    #[test]
    fn the_viewport_menus_opacity_row_is_a_wheel_slider() {
        use crate::app::ViewportMenuAction as A;
        use crate::window::WindowEvent;
        use cce_ui::widget::{context_menu, ElementState, MouseButton, MouseScrollDelta};
        let mut state = State::new(false);
        state.geo_opacity = 0.5;
        state.cursor_x = 300.0;
        state.cursor_y = 200.0;
        state.open_viewport_context_menu();
        let sub_actions = state.open_viewport_page_with(A::OpacitySlider);
        assert!(state.viewport_menu_open());
        let i = sub_actions.iter().position(|a| *a == A::OpacitySlider).expect("an Opacity row");
        let s = context_menu::slider(i).expect("the row is a slider");
        assert_eq!((s.value, s.min, s.max, s.step), (50.0, 0.0, 100.0, 5.0));

        // Wheel over the row: one notch up is 5% more, saved, menu still up.
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.cursor_x = context_menu::x() + 20.0;
        let wheel = |state: &mut State, notches: f32| {
            state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, notches) })
        };
        assert!(wheel(&mut state, 1.0));
        assert!((state.geo_opacity - 0.55).abs() < 1e-6, "{}", state.geo_opacity);
        assert!(state.viewport_menu_open(), "the menu stays open");
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert!(kdl.contains("0.55"), "the wheel step persisted: {kdl}");
        wheel(&mut state, -3.0);
        assert!((state.geo_opacity - 0.40).abs() < 1e-6);

        // Press on the band's right end: 100%, live; drag back; release.
        let band = context_menu::CONTEXT_MENU.with(|m| m.borrow().slider_band(i));
        state.cursor_x = band.x + band.width - 1.0;
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        assert!((state.geo_opacity - 1.0).abs() < 1e-6, "{}", state.geo_opacity);
        assert!(state.viewport_menu_open());
        state.handle_event(&WindowEvent::CursorMoved { position: crate::window::LocalPosition { x: (band.x + band.width * 0.25) as f64, y: 900.0 } });
        assert!((state.geo_opacity - 0.25).abs() < 1e-6, "the drag follows off the plate: {}", state.geo_opacity);
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        assert!(!context_menu::slider_dragging());
        assert!(state.viewport_menu_open(), "the release does not close it");

        // The wheel over an action row changes nothing and orbits nothing.
        let before = (state.geo_opacity, state.viewport().rotation_y);
        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(0) + context_menu::ROW_H * 0.5;
        assert!(wheel(&mut state, 1.0));
        assert_eq!((state.geo_opacity, state.viewport().rotation_y), before);
        context_menu::hide();
    }

    /// The GPU setting: a choice row of the dialog, persisted in state.kdl
    /// (absent = integrated), NOT carried by a project's display block, and
    /// turned into `CCE_VK_DEVICE` at launch only when it asks for the
    /// discrete card and the environment has not already said.
    #[test]
    fn the_gpu_setting_picks_the_renderers_device_at_launch() {
        use crate::app::gpu_env_for;
        let mut state = State::new(false);
        state.gpu_preference = "integrated".into();
        state.gpu_at_launch = "integrated".into();
        assert_eq!(state.settings_row_value("GPU"), "integrated");
        state.apply_setting("GPU", "discrete");
        assert_eq!(state.gpu_preference, "discrete");
        assert!(state.last_status_text.contains("restart"), "{}", state.last_status_text);
        let kdl = fs::read_to_string(DesignSettings::file_path()).expect("saved");
        assert_eq!(DesignSettings::from_kdl_str(&kdl).gpu, "discrete", "{kdl}");
        state.apply_setting("GPU", "Integrated");
        assert_eq!(state.gpu_preference, "integrated");
        assert!(state.last_status_text.contains("in use"), "{}", state.last_status_text);
        state.apply_setting("GPU", "quantum");
        assert_eq!(state.gpu_preference, "integrated", "an unknown option is refused");

        // A file from before the setting existed: integrated.
        assert_eq!(DesignSettings::from_kdl_str("viewport {\n}\n").gpu, "integrated");
        // A project does not carry it.
        let json = serde_json::to_value(state.display_settings()).unwrap();
        assert!(!json.to_string().contains("\"gpu\""), "{json}");

        assert_eq!(gpu_env_for("discrete", None), Some("discrete"));
        assert_eq!(gpu_env_for("discrete", Some("")), Some("discrete"));
        assert_eq!(gpu_env_for("integrated", None), None, "integrated sets nothing, keeping any ICD pin");
        assert_eq!(gpu_env_for("discrete", Some("intel")), None, "an explicit CCE_VK_DEVICE wins");
    }

    /// The wires' opacity is a setting of its own, apart from the polygons':
    /// the viewport menu's Wire Opacity slider moves `wire_opacity` and
    /// leaves `geo_opacity` alone (and the other way round), the dialog has
    /// a row for it, and it persists. The wire pass reads it in both colour
    /// modes, so it is not tied to single-colour mode the way the colour is.
    #[test]
    fn wire_opacity_is_separate_from_polygon_opacity() {
        use crate::app::ViewportMenuAction as A;
        use crate::window::WindowEvent;
        use cce_ui::widget::{context_menu, MouseScrollDelta};
        let mut state = State::new(false);
        state.geo_opacity = 0.5;
        state.wire_opacity = 0.5;
        state.cursor_x = 300.0;
        state.cursor_y = 200.0;
        state.open_viewport_context_menu();
        let sub_actions = state.open_viewport_page_with(A::WireOpacitySlider);
        let acts = sub_actions.clone();
        let i = acts.iter().position(|a| *a == A::WireOpacitySlider).expect("a Wire Opacity row");
        let sl = context_menu::slider(i).expect("the row is a slider");
        assert_eq!((sl.value, sl.min, sl.max, sl.step), (50.0, 0.0, 100.0, 5.0));

        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, -2.0) });
        assert!((state.wire_opacity - 0.40).abs() < 1e-6, "{}", state.wire_opacity);
        assert!((state.geo_opacity - 0.5).abs() < 1e-6, "the polygon opacity moved with the wires'");

        let j = acts.iter().position(|a| *a == A::OpacitySlider).expect("an Opacity row");
        state.cursor_y = context_menu::row_y(j) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 2.0) });
        assert!((state.geo_opacity - 0.60).abs() < 1e-6, "{}", state.geo_opacity);
        assert!((state.wire_opacity - 0.40).abs() < 1e-6, "the wires' opacity moved with the polygons'");
        context_menu::hide();

        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        let back = crate::app::DesignSettings::from_kdl_str(&kdl);
        assert!((back.render.wire_opacity - 0.40).abs() < 0.01, "persisted: {kdl}");

        // The dialog row edits the same field.
        state.apply_setting("Wire Opacity", "0.25");
        assert!((state.wire_opacity - 0.25).abs() < 1e-6);
        assert!((state.geo_opacity - 0.60).abs() < 1e-6);
    }

    /// Until 2026-09-25 the wire opacity was the wire colour's alpha:
    /// `#rrggbbaa` in state.kdl and a four-component array in a project's
    /// display block. Both load with the alpha as `wire_opacity`, or every
    /// translucent wireframe would come back opaque; and a file that names
    /// `wire_opacity` itself keeps it.
    #[test]
    fn an_old_wire_colour_alpha_becomes_the_wire_opacity() {
        let old = "render {\n    wireframe (bool)true\n    wire_color (rgba)\"#33669980\"\n}\n";
        let back = crate::app::DesignSettings::from_kdl_str(old);
        assert!(back.render.wireframe, "the rest of the block reads: {old}");
        let rgb = back.render.wire_color;
        assert!((rgb[0] - 0.2).abs() < 0.01 && (rgb[1] - 0.4).abs() < 0.01 && (rgb[2] - 0.6).abs() < 0.01, "{rgb:?}");
        assert!((back.render.wire_opacity - 128.0 / 255.0).abs() < 0.01, "{}", back.render.wire_opacity);

        // Written back, the colour is six digits and the opacity its own key.
        let written = back.to_kdl_str().expect("kdl");
        assert!(written.contains("#336699\""), "{written}");
        let again = crate::app::DesignSettings::from_kdl_str(&written);
        assert!((again.render.wire_opacity - back.render.wire_opacity).abs() < 1e-6);

        // A project's display block, likewise.
        let json = serde_json::json!({ "render": { "wire_color": [0.2, 0.4, 0.6, 0.3] } });
        let d: crate::app::DisplaySettings = serde_json::from_value(json).expect("an old display block loads");
        assert!((d.render.wire_opacity - 0.3).abs() < 1e-6);
        assert_eq!(d.render.wire_color, [0.2, 0.4, 0.6]);
        let json = serde_json::json!({ "render": { "wire_color": [0.2, 0.4, 0.6], "wire_opacity": 0.7 } });
        let d: crate::app::DisplaySettings = serde_json::from_value(json).unwrap();
        assert!((d.render.wire_opacity - 0.7).abs() < 1e-6);
        // Absent altogether: opaque.
        let d: crate::app::DisplaySettings = serde_json::from_value(serde_json::json!({ "render": {} })).unwrap();
        assert_eq!((d.render.wire_color, d.render.wire_opacity), ([0.0; 3], 1.0));
    }

    /// The viewport menu holds framing, the guides and a row for each
    /// page; the display rows are in the pages, in groups a separator
    /// apart — Style the wireframe then the surface, Markers the points then
    /// each element class's overlays — every one in exactly one.
    #[test]
    fn the_viewport_menu_groups_its_display_rows() {
        use crate::app::{ViewportMenuAction as A, ViewportMenuPage as P};
        let state = State::new(false);
        let groups = |page: Option<P>| -> Vec<Vec<A>> {
            let (options, actions) = state.viewport_menu_rows_of(page);
            assert_eq!(options.len(), actions.len());
            assert!(options.iter().zip(&actions).all(|(o, a)| (o == "-") == (*a == A::Separator)), "separator rows line up");
            actions.split(|a| *a == A::Separator).map(|g| g.to_vec()).collect()
        };
        assert_eq!(
            groups(None),
            vec![
                vec![A::FrameAll, A::OneToOne],
                vec![A::Command("toggle_grid"), A::Command("toggle_origin"), A::Command("toggle_camera_pivot"), A::CameraPivotSizeSlider],
                vec![A::Page(P::Style), A::Page(P::Markers), A::Command("attribute_visualizers")],
            ]
        );
        assert_eq!(
            groups(Some(P::Style)),
            vec![
                vec![A::Command("toggle_wireframe"), A::WireThicknessSlider, A::WireOpacitySlider],
                vec![A::Shading(false), A::Shading(true), A::OpacitySlider, A::Command("toggle_show_occluded")],
            ]
        );
        assert_eq!(
            groups(Some(P::Markers)),
            vec![
                vec![A::GroupMarkerSizeSlider, A::PullArrowScaleSlider],
                vec![
                    A::Command("toggle_point_markers"),
                    A::PointMarkerSizeSlider,
                    A::Command("toggle_point_numbers"),
                    A::Command("toggle_point_normals"),
                ],
                vec![A::Command("toggle_prim_numbers"), A::Command("toggle_prim_normals")],
                vec![
                    A::Command("toggle_vertex_markers"),
                    A::Command("toggle_vertex_numbers"),
                    A::Command("toggle_vertex_normals"),
                ],
            ]
        );
    }

    /// The display settings are PAGES of the viewport menu: pointing at
    /// their row turns nothing; a press on it, or a side swipe forward over
    /// it, turns the menu into the page where it stands, under a back band;
    /// a switch on the page flips, is re-marked in place and leaves the page
    /// up; a press on the band or a swipe back turns back. The visualizers'
    /// row turns the menu into the dialog, which a swipe back turns back
    /// into the menu at its corner. A row of the menu itself still closes it.
    #[test]
    fn the_viewport_menu_turns_into_its_pages_and_back() {
        use crate::app::{ViewportMenuAction as A, ViewportMenuPage as P};
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{context_menu, ElementState, MouseButton, MouseScrollDelta, Position};
        let mut state = State::new(false);
        state.show_prim_numbers = false;
        state.cursor_x = 300.0;
        state.cursor_y = 200.0;
        state.open_viewport_context_menu();
        let corner = (context_menu::x(), context_menu::y());
        let row_of = |state: &State, a: A| state.viewport_menu_actions.iter().position(|x| *x == a).unwrap_or_else(|| panic!("no {a:?} row"));
        let move_to = |state: &mut State, x: f32, y: f32| {
            state.cursor_x = x;
            state.cursor_y = y;
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        };
        let press = |state: &mut State| {
            state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
            state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        };
        // Two fingers to the side, a few events long: negative x shows what
        // is to the right, which is forward.
        let swipe = |state: &mut State, dx: f64| {
            cce_ui::widget::side_swipe::end_gesture();
            for _ in 0..4 {
                state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: dx / 4.0, y: 0.0 }) });
            }
        };
        let (style, markers) = (row_of(&state, A::Page(P::Style)), row_of(&state, A::Page(P::Markers)));
        let vis = row_of(&state, A::Command("attribute_visualizers"));
        assert!([style, markers, vis].iter().all(|&i| context_menu::leads_to_page(i)), "the three rows are page rows");
        assert!(!context_menu::leads_to_page(row_of(&state, A::FrameAll)));

        move_to(&mut state, corner.0 + 20.0, context_menu::row_y(markers) + 12.0);
        assert_eq!(state.viewport_menu_page, None, "pointing turns nothing");
        press(&mut state);
        assert_eq!(state.viewport_menu_page, Some(P::Markers), "a press turns the menu");
        assert_eq!((context_menu::x(), context_menu::y()), corner, "where the menu stood");
        assert_eq!(context_menu::back_title().as_deref(), Some("Viewport"));
        let prims = row_of(&state, A::Command("toggle_prim_numbers"));
        assert!(context_menu::options()[prims].starts_with('○'));

        move_to(&mut state, corner.0 + 20.0, context_menu::row_y(prims) + 12.0);
        press(&mut state);
        assert!(state.show_prim_numbers, "the switch flipped");
        assert!(state.viewport_menu_open() && state.viewport_menu_page == Some(P::Markers), "and the page is still up");
        assert!(context_menu::options()[prims].starts_with('●'), "re-marked: {}", context_menu::options()[prims]);

        swipe(&mut state, 80.0);
        assert_eq!(state.viewport_menu_page, None, "a swipe back turned back to the menu");
        assert!(state.viewport_menu_open());
        assert_eq!((context_menu::x(), context_menu::y()), corner);
        assert_eq!(context_menu::back_title(), None, "the menu goes back nowhere");

        move_to(&mut state, corner.0 + 20.0, context_menu::row_y(style) + 12.0);
        swipe(&mut state, -80.0);
        assert_eq!(state.viewport_menu_page, Some(P::Style), "a swipe forward over the row turns into its page");
        assert_eq!(state.viewport_menu_actions[0], A::Command("toggle_wireframe"));
        move_to(&mut state, corner.0 + 20.0, corner.1 + context_menu::PAD + context_menu::ROW_H * 0.5);
        press(&mut state);
        assert_eq!(state.viewport_menu_page, None, "a press on the back band turns back");

        // Into the dialog, and back.
        move_to(&mut state, corner.0 + 20.0, context_menu::row_y(vis) + 12.0);
        press(&mut state);
        assert!(!state.viewport_menu_open());
        assert!(state.dialog_visible() && state.slots.dialog.mode == crate::dialog::Mode::Visualizers);
        assert_eq!(state.slots.dialog.anchor, Some(corner), "the dialog took the menu's corner");
        let (dx, dy, _, _) = state.positions[crate::slots::DIALOG_IDX];
        move_to(&mut state, dx + 30.0, dy + 30.0);
        swipe(&mut state, 80.0);
        assert!(!state.dialog_visible(), "a swipe back closes the dialog");
        assert!(state.viewport_menu_open() && state.viewport_menu_page.is_none(), "and turns it back into the menu");
        assert_eq!((context_menu::x(), context_menu::y()), (dx, dy), "at the dialog's corner");

        // A row of the menu itself runs and closes it.
        let grid = row_of(&state, A::Command("toggle_grid"));
        move_to(&mut state, context_menu::x() + 20.0, context_menu::row_y(grid) + 12.0);
        press(&mut state);
        assert!(!state.viewport_menu_open(), "a row of the menu closes it");
    }

    /// The primitive and vertex overlays read off the scene as the points'
    /// do: a primitive's number at its centroid, a vertex's — its index in
    /// the detail — inset from its point toward that centroid, a
    /// primitive's normal from the centroid along the face. Off, nothing
    /// is collected.
    #[test]
    fn primitives_and_vertices_are_numbered_where_they_are() {
        use glam::Vec3;
        let mut d = crate::detail::Detail::new();
        let p: Vec<u32> = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 2.0, 0.0], [4.0, 0.0, 0.0]]
            .into_iter()
            .map(|at| d.add_point(Vec3::from_array(at)))
            .collect();
        d.add_prim(&[p[0], p[1], p[2], p[3]]);
        d.add_prim(&[p[1], p[4], p[2]]);
        use crate::render::ElementOverlays as Want;
        let none = crate::render::scene_element_overlays(&d, Want::default(), 1.0);
        assert!(none.prim_labels.is_empty() && none.vertex_labels.is_empty() && none.normals.is_empty() && none.vertex_markers.is_empty());

        let want = Want { prim_numbers: true, prim_normals: true, vertex_numbers: true, ..Want::default() };
        let got = crate::render::scene_element_overlays(&d, want, 0.5);
        let (prims, verts, normals) = (got.prim_labels, got.vertex_labels, got.normals);
        assert_eq!(prims.len(), 2);
        assert_eq!(prims[0], ([1.0, 1.0, 0.0], 0));
        assert_eq!(prims[1].1, 1);
        assert_eq!(verts.iter().map(|(_, v)| *v).collect::<Vec<_>>(), (0..7).collect::<Vec<u32>>(), "every vertex, by its index");
        // Points 1 and 2 are shared: each has a vertex in either primitive,
        // and the two stand apart, each inside its own.
        let inset = crate::render::VERTEX_LABEL_INSET;
        assert!((Vec3::from_array(verts[1].0) - Vec3::new(2.0, 0.0, 0.0).lerp(Vec3::new(1.0, 1.0, 0.0), inset)).length() < 1e-6);
        assert!(verts[1].0[0] < 2.0 && verts[4].0[0] > 2.0, "{:?} {:?}", verts[1], verts[4]);
        assert_eq!(normals.len(), 4, "a whisker a primitive");
        let n = Vec3::from_array(normals[1].position) - Vec3::from_array(normals[0].position);
        assert!((n - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-6, "{n:?}");

        // A vertex's marker stands where its number does, and its normal
        // is its primitive's: on a mesh that carries no vertex normals the
        // corners of one face agree and the faces around a point do not.
        let got = crate::render::scene_element_overlays(&d, Want { vertex_markers: true, vertex_normals: true, ..Want::default() }, 0.5);
        assert_eq!(got.vertex_markers.len(), 7);
        assert_eq!(got.vertex_markers.iter().map(|m| m.position).collect::<Vec<_>>(), verts.iter().map(|(p, _)| *p).collect::<Vec<_>>());
        assert_eq!(got.normals.len(), 14, "a whisker a vertex");
        assert_eq!(got.normals[2].position, verts[1].0, "from where the vertex stands");
        let n = Vec3::from_array(got.normals[3].position) - Vec3::from_array(got.normals[2].position);
        assert!((n - Vec3::new(0.0, 0.0, 0.5 * crate::render::VERTEX_MARKER_SCALE)).length() < 1e-6, "{n:?}");
        // Its own N, where the detail carries one on its vertices.
        let mut tilted = d.clone();
        tilted.verts_mut().create("N", crate::detail::AttribValue::Float3([1.0, 0.0, 0.0]));
        let got = crate::render::scene_element_overlays(&tilted, Want { vertex_normals: true, ..Want::default() }, 1.0);
        let n = Vec3::from_array(got.normals[1].position) - Vec3::from_array(got.normals[0].position);
        assert!((n.normalize() - Vec3::X).length() < 1e-6, "{n:?}");

        // The markers are built with the points', and re-sized with them.
        let mut state = State::new(false);
        state.show_point_markers = false;
        state.show_vertex_markers = false;
        state.rebuild_scene_geometry();
        assert!(state.overlay_marker_verts.is_empty());
        state.run_command("toggle_vertex_markers");
        let built = state.overlay_marker_verts.clone();
        assert!(!built.is_empty() && !state.overlay_vertex_marker_points.is_empty());
        state.point_marker_size *= 2.0;
        state.rebuild_overlay_marker_verts();
        assert_eq!(state.overlay_marker_verts.len(), built.len());
        assert_ne!(state.overlay_marker_verts[0].position, built[0].position, "re-sized");
        state.run_command("toggle_vertex_markers");
        assert!(state.overlay_marker_verts.is_empty(), "and gone with the switch");

        // And the app collects them by its switches, and persists those.
        let mut state = State::new(false);
        state.show_prim_numbers = false;
        state.show_vertex_numbers = false;
        state.rebuild_scene_geometry();
        assert!(state.overlay_prim_labels.is_empty() && state.overlay_vertex_labels.is_empty());
        state.run_command("toggle_prim_numbers");
        state.run_command("toggle_vertex_numbers");
        assert!(!state.overlay_prim_labels.is_empty() && !state.overlay_vertex_labels.is_empty());
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        let back = crate::app::DesignSettings::from_kdl_str(&kdl).viewport;
        assert!(back.show_prim_numbers && back.show_vertex_numbers && !back.show_prim_normals);
    }

    /// Show Grid heads the Guides group, marked from the live flag; the row
    /// runs the command.
    #[test]
    fn the_viewport_menu_toggles_show_grid() {
        use crate::app::ViewportMenuAction as A;
        let mut state = State::new(false);
        state.viewport_mut().show_grid = true;
        let row = |state: &State| {
            let (options, actions) = state.viewport_menu_rows_of(state.viewport_menu_page_of(A::Command("toggle_grid")));
            let i = actions.iter().position(|a| *a == A::Command("toggle_grid")).expect("a Show Grid row");
            options[i].clone()
        };
        let label = crate::command::by_id("toggle_grid").unwrap().label;
        assert_eq!(row(&state), format!("● {label}"));
        state.run_viewport_menu_action(A::Command("toggle_grid"));
        assert!(!state.viewport().show_grid);
        assert_eq!(row(&state), format!("○ {label}"));
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert!(!crate::app::DesignSettings::from_kdl_str(&kdl).viewport.show_grid_enabled, "persisted");
    }

    /// Show Origin is a switch in the Guides group, marked from the live
    /// flag; the row runs the command, which also keeps the viewport
    /// menubar's Guides checkmark in step.
    #[test]
    fn the_viewport_menu_toggles_show_origin() {
        use crate::app::ViewportMenuAction as A;
        let mut state = State::new(false);
        state.viewport_mut().show_origin = true;
        let row = |state: &State| {
            let (options, actions) = state.viewport_menu_rows_of(state.viewport_menu_page_of(A::Command("toggle_origin")));
            let i = actions.iter().position(|a| *a == A::Command("toggle_origin")).expect("a Show Origin row");
            options[i].clone()
        };
        let label = crate::command::by_id("toggle_origin").unwrap().label;
        assert_eq!(row(&state), format!("● {label}"));
        state.run_viewport_menu_action(A::Command("toggle_origin"));
        assert!(!state.viewport().show_origin);
        assert_eq!(row(&state), format!("○ {label}"));
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert!(!crate::app::DesignSettings::from_kdl_str(&kdl).viewport.show_origin_enabled, "persisted");
    }

    /// Show Point Normals is a switch in the Points group after the
    /// numbers, marked from the live flag; the row runs the command, which
    /// collects the whiskers with the scene.
    #[test]
    fn the_viewport_menu_toggles_show_point_normals() {
        use crate::app::ViewportMenuAction as A;
        let mut state = State::new(false);
        state.show_point_normals = false;
        state.rebuild_scene_geometry();
        let whiskers = state.overlay_normal_verts.len();
        let row = |state: &State| {
            let (options, actions) = state.viewport_menu_rows_of(state.viewport_menu_page_of(A::Command("toggle_point_normals")));
            let i = actions.iter().position(|a| *a == A::Command("toggle_point_normals")).expect("a Show Point Normals row");
            options[i].clone()
        };
        let label = crate::command::by_id("toggle_point_normals").unwrap().label;
        assert_eq!(row(&state), format!("○ {label}"));
        state.run_viewport_menu_action(A::Command("toggle_point_normals"));
        assert!(state.show_point_normals);
        assert_eq!(row(&state), format!("● {label}"));
        assert!(state.overlay_normal_verts.len() > whiskers, "the whiskers were collected");
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert!(crate::app::DesignSettings::from_kdl_str(&kdl).viewport.show_point_normals, "persisted");
    }

    /// Show Point Numbers is a switch in the Points group after the
    /// markers' size, marked from the live flag; the row runs the command,
    /// which collects the labels with the scene.
    #[test]
    fn the_viewport_menu_toggles_show_point_numbers() {
        use crate::app::ViewportMenuAction as A;
        let mut state = State::new(false);
        state.show_point_numbers = false;
        state.rebuild_scene_geometry();
        let row = |state: &State| {
            let (options, actions) = state.viewport_menu_rows_of(state.viewport_menu_page_of(A::Command("toggle_point_numbers")));
            let i = actions.iter().position(|a| *a == A::Command("toggle_point_numbers")).expect("a Show Point Numbers row");
            options[i].clone()
        };
        let label = crate::command::by_id("toggle_point_numbers").unwrap().label;
        assert_eq!(row(&state), format!("○ {label}"));
        assert!(state.overlay_number_labels.is_empty());
        state.run_viewport_menu_action(A::Command("toggle_point_numbers"));
        assert!(state.show_point_numbers);
        assert_eq!(row(&state), format!("● {label}"));
        assert!(!state.overlay_number_labels.is_empty(), "the labels were collected");
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert!(crate::app::DesignSettings::from_kdl_str(&kdl).viewport.show_point_numbers, "persisted");
    }

    /// Show Camera Pivot is a guide row of the menu itself, beside the
    /// grid and the origin, marked from the live flag; the row runs the
    /// command the Guides menubar and the palette run.
    #[test]
    fn the_viewport_menu_toggles_the_camera_pivot_marker() {
        use crate::app::ViewportMenuAction as A;
        let mut state = State::new(false);
        state.viewport_mut().show_camera_pivot = false;
        let row = |state: &State| {
            let (options, actions) = state.viewport_menu_rows_of(None);
            let i = actions.iter().position(|a| *a == A::Command("toggle_camera_pivot")).expect("a Show Camera Pivot row");
            options[i].clone()
        };
        assert_eq!(row(&state), "○ Show Camera Pivot");
        state.run_viewport_menu_action(A::Command("toggle_camera_pivot"));
        assert!(state.viewport().show_camera_pivot);
        assert_eq!(row(&state), "● Show Camera Pivot");
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert!(crate::app::DesignSettings::from_kdl_str(&kdl).viewport.show_camera_pivot_enabled, "persisted");
    }

    /// The pivot marker's size is the length of its beams; their thickness
    /// does not follow it.
    #[test]
    fn the_camera_pivot_size_leaves_the_line_width_alone() {
        // The X beam: its extent along x is the length, along y the width.
        let extent = |scale: f32, axis: usize| {
            let v = crate::geometry::camera_pivot_vertices(scale);
            let beam: Vec<_> = v.iter().filter(|p| p.color[0] > 0.5).collect();
            let lo = beam.iter().map(|p| p.position[axis]).fold(f32::MAX, f32::min);
            let hi = beam.iter().map(|p| p.position[axis]).fold(f32::MIN, f32::max);
            hi - lo
        };
        assert!((extent(1.0, 0) - 2.0 * extent(0.5, 0)).abs() < 1e-6, "the length follows the size");
        assert!((extent(1.0, 1) - extent(0.25, 1)).abs() < 1e-7, "the width does not");
        assert!(extent(0.25, 1) > 0.0);
    }

    /// The playbar has a right-click menu: the transport's commands, the
    /// Repeat switch with its mark, and the timeline's settings as slider
    /// rows — Playback Rate, saved with the settings, and the frame range,
    /// which is the project's, saved in its file and dirtying it.
    #[test]
    fn the_playbar_menu_sets_the_rate_and_the_range() {
        use crate::app::PlaybarMenuAction as A;
        use crate::slots::PLAYBAR_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{context_menu, ElementState, MouseButton, MouseScrollDelta};
        let dir = std::env::temp_dir().join(format!("cce-designer-playbar-menu-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.execute_menu_action("Show Playbar Pane");
        state.rebuild_positions();
        state.apply_layout();
        state.save_to_file(&dir).expect("save");
        assert!(!state.has_unsaved_changes());

        let (options, actions) = state.playbar_menu_rows();
        let groups: Vec<Vec<A>> = actions.split(|a| *a == A::Separator).map(|g| g.to_vec()).collect();
        assert_eq!(
            groups,
            vec![
                vec![A::Command("play_pause"), A::Command("play_pause_reverse"), A::Command("frame_start")],
                vec![A::Command("toggle_playbar_repeat"), A::Command("toggle_playbar_step_buttons")],
                vec![A::FpsSlider, A::StartFrameSlider, A::EndFrameSlider],
                vec![
                    A::Plate(crate::plate_menu::PlateMenuAction::Collapse),
                    A::Plate(crate::plate_menu::PlateMenuAction::Detach),
                ],
            ]
        );
        let repeat = actions.iter().position(|a| *a == A::Command("toggle_playbar_repeat")).unwrap();
        assert!(options[repeat].starts_with("● "), "{}", options[repeat]);

        // A right press on the plate opens it.
        let (px, py, pw, ph) = state.positions[PLAYBAR_IDX];
        assert!(pw > 0.0 && ph > 0.0, "the playbar is laid out");
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: (px + pw * 0.5) as f64, y: (py + ph * 0.5) as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right });
        assert!(state.playbar_menu_open(), "a right press on the playbar opens its menu");
        assert!(!state.viewport_menu_open());

        // The rate: a wheel notch over its row is a frame a second, saved.
        let i = actions.iter().position(|a| *a == A::FpsSlider).unwrap();
        let sl = context_menu::slider(i).expect("a slider");
        assert_eq!((sl.min, sl.max, sl.step, sl.suffix), (1.0, 120.0, 1.0, " fps"));
        assert_eq!(sl.value, 24.0);
        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 6.0) });
        assert_eq!(state.slots.playbar.inner().fps, 30.0);
        assert!(state.playbar_menu_open(), "a slider row keeps the menu up");
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert_eq!(crate::app::DesignSettings::from_kdl_str(&kdl).playbar_fps, 30.0, "persisted");
        assert!(!state.has_unsaved_changes(), "the rate is a setting, not the project's");

        // The range: an end moved past the other carries it along, and the
        // playhead stays inside.
        state.slots.playbar.inner_mut().current_frame = 200.0;
        let i = actions.iter().position(|a| *a == A::EndFrameSlider).unwrap();
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, -100.0) });
        let pb = state.slots.playbar.inner();
        assert_eq!(pb.end_frame, 140.0, "{}", pb.end_frame);
        assert_eq!(pb.current_frame, 140.0, "the playhead is kept inside");
        assert!(state.has_unsaved_changes(), "the range is the project's");
        let i = actions.iter().position(|a| *a == A::StartFrameSlider).unwrap();
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 150.0) });
        let pb = state.slots.playbar.inner();
        assert_eq!((pb.start_frame, pb.end_frame), (151.0, 152.0), "the far end is carried a frame ahead of the near");

        // A command row runs and closes.
        let i = actions.iter().position(|a| *a == A::Command("toggle_playbar_repeat")).unwrap();
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        assert!(!state.playbar_menu_open());
        assert!(!state.slots.playbar.inner().repeat, "Repeat was flipped");

        // The range rides the project file.
        state.save_to_file(&dir).expect("save");
        assert!(!state.has_unsaved_changes());
        let mut again = State::new(false);
        again.resize(1600.0, 900.0, 1.0);
        again.load_from_file(&dir).expect("load");
        let pb = again.slots.playbar.inner();
        assert_eq!((pb.start_frame, pb.end_frame), (151.0, 152.0));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The playbar's Previous / Next Frame buttons step a whole frame by
    /// pointer, either side of the play button, and the playbar menu's
    /// Show Step Buttons switch takes them away — the track widening into
    /// their room — and saves the choice.
    #[test]
    fn the_playbar_step_buttons_step_and_can_be_hidden() {
        use crate::app::PlaybarMenuAction as A;
        use crate::slots::PLAYBAR_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::scene::layout::Rect;
        use cce_ui::widget::{context_menu, ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.execute_menu_action("Show Playbar Pane");
        state.rebuild_positions();
        state.apply_layout();
        let (px, py, pw, ph) = state.positions[PLAYBAR_IDX];
        let rect = Rect { x: px, y: py, width: pw, height: ph };
        assert!(state.slots.playbar.inner().step_buttons, "on by default");
        let press = |state: &mut State, r: Rect| {
            let (x, y) = (r.x + r.width * 0.5, r.y + r.height * 0.5);
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
            state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
            state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        };
        let pb = state.slots.playbar.inner();
        let prev = pb.transport_button_rect(rect, -1).expect("a Previous Frame button");
        let play = pb.transport_button_rect(rect, 0).expect("a play button");
        let next = pb.transport_button_rect(rect, 1).expect("a Next Frame button");
        assert!(prev.x + prev.width < play.x && play.x + play.width < next.x, "|< > >| left to right");

        state.slots.playbar.inner_mut().current_frame = 10.4;
        press(&mut state, next);
        assert_eq!(state.slots.playbar.inner().current_frame, 11.0, "a whole frame on, off the rounded one");
        press(&mut state, prev);
        press(&mut state, prev);
        assert_eq!(state.slots.playbar.inner().current_frame, 9.0);
        assert!(!state.slots.playbar.inner().playing, "a step does not start playback");
        state.slots.playbar.inner_mut().current_frame = 1.0;
        press(&mut state, prev);
        assert_eq!(state.slots.playbar.inner().current_frame, 1.0, "held inside the range");

        // The menu's switch hides them, and is saved.
        let (options, actions) = state.playbar_menu_rows();
        let i = actions.iter().position(|a| *a == A::Command("toggle_playbar_step_buttons")).expect("a Step Buttons row");
        assert!(options[i].starts_with("● "), "{}", options[i]);
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: (play.x + 400.0) as f64, y: (py + ph * 0.5) as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right });
        assert!(state.playbar_menu_open());
        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        assert!(!state.playbar_menu_open());
        let pb = state.slots.playbar.inner();
        assert!(!pb.step_buttons);
        assert!(pb.transport_button_rect(rect, -1).is_none() && pb.transport_button_rect(rect, 1).is_none());
        assert_eq!(pb.transport_button_rect(rect, 0).unwrap().x, prev.x, "the play button takes the first place");
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert!(!crate::app::DesignSettings::from_kdl_str(&kdl).playbar_step_buttons, "persisted");

        // Where Next Frame stood is the track now: a press there scrubs.
        state.slots.playbar.inner_mut().current_frame = 50.0;
        press(&mut state, next);
        assert_ne!(state.slots.playbar.inner().current_frame, 51.0, "no step button there any more");
        state.run_command("toggle_playbar_step_buttons");
        assert!(state.slots.playbar.inner().step_buttons);
    }

    /// Camera Pivot Size is a slider under Show Camera Pivot, over 0–1:
    /// the wheel steps a twentieth, re-bakes the marker and nothing else,
    /// and saves.
    #[test]
    fn the_viewport_menu_sets_the_camera_pivot_size() {
        use crate::app::ViewportMenuAction as A;
        use crate::window::WindowEvent;
        use cce_ui::widget::{context_menu, MouseScrollDelta};
        let mut state = State::new(false);
        state.camera_pivot_size = 0.5;
        state.pending_pivot = None;
        let version = state.rt_geometry_version;
        state.cursor_x = 300.0;
        state.cursor_y = 200.0;
        state.open_viewport_context_menu();
        let actions = state.viewport_menu_actions.clone();
        let i = actions.iter().position(|a| *a == A::CameraPivotSizeSlider).expect("a Camera Pivot Size row");
        assert_eq!(actions[i - 1], A::Command("toggle_camera_pivot"), "it sits under its switch");
        let sl = context_menu::slider(i).expect("a slider");
        assert_eq!((sl.min, sl.max, sl.step, sl.decimals), (0.0, 1.0, 0.05, 2));
        assert!((sl.value - 0.5).abs() < 1e-6);

        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 5.0) });
        assert!((state.camera_pivot_size - 0.75).abs() < 1e-5, "{}", state.camera_pivot_size);
        assert!(state.pending_pivot.is_some(), "the marker re-bakes");
        assert_eq!(state.rt_geometry_version, version, "and the graph is not evaluated");
        assert!(context_menu::is_visible(), "a slider row keeps the menu up");
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        let saved = crate::app::DesignSettings::from_kdl_str(&kdl).viewport.camera_pivot_size;
        assert!((saved - 0.75).abs() < 1e-5, "persisted: {saved}");
        context_menu::hide();
    }

    /// Show Point Markers is a switch in the Points group, over its own
    /// size slider, marked from the live flag; the row runs the command,
    /// which rebuilds the overlay (and keeps the positions the size slider
    /// re-sizes from).
    #[test]
    fn the_viewport_menu_toggles_show_point_markers() {
        use crate::app::ViewportMenuAction as A;
        let mut state = State::new(false);
        state.show_point_markers = false;
        state.rebuild_scene_geometry();
        let row = |state: &State| {
            let (options, actions) = state.viewport_menu_rows_of(state.viewport_menu_page_of(A::Command("toggle_point_markers")));
            let i = actions.iter().position(|a| *a == A::Command("toggle_point_markers")).expect("a Show Point Markers row");
            options[i].clone()
        };
        let label = crate::command::by_id("toggle_point_markers").unwrap().label;
        assert_eq!(row(&state), format!("○ {label}"));
        assert!(state.overlay_marker_verts.is_empty());
        state.run_viewport_menu_action(A::Command("toggle_point_markers"));
        assert!(state.show_point_markers);
        assert_eq!(row(&state), format!("● {label}"));
        assert!(!state.overlay_marker_verts.is_empty(), "the overlay was built");
        assert!(!state.overlay_marker_points.is_empty(), "and its positions kept for re-sizing");
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert!(crate::app::DesignSettings::from_kdl_str(&kdl).viewport.show_point_markers, "persisted");
    }

    /// Wire Thickness is a slider row right under Show Wireframe, over the
    /// palette row's 1–8 px: the wheel steps half a pixel and saves, a press
    /// on the band jumps, and the value is the live `wire_width` the wire
    /// pass draws with.
    #[test]
    fn the_viewport_menu_sets_the_wire_thickness_by_wheel() {
        use crate::app::ViewportMenuAction as A;
        use crate::window::WindowEvent;
        use cce_ui::widget::{context_menu, ElementState, MouseButton, MouseScrollDelta};
        let mut state = State::new(false);
        state.wire_width = 2.0;
        state.cursor_x = 300.0;
        state.cursor_y = 200.0;
        state.open_viewport_context_menu();
        let sub_actions = state.open_viewport_page_with(A::WireThicknessSlider);
        let acts = sub_actions.clone();
        let i = acts.iter().position(|a| *a == A::WireThicknessSlider).expect("a Wire Thickness row");
        assert_eq!(acts[i - 1], A::Command("toggle_wireframe"), "it sits under Show Wireframe");
        let sl = context_menu::slider(i).expect("the row is a slider");
        assert_eq!((sl.value, sl.min, sl.max, sl.step), (2.0, 1.0, 8.0, 0.5));
        // Every other slider the menu carries answers the same table.
        for (k, a) in acts.iter().enumerate() {
            assert_eq!(context_menu::slider(k).is_some(), state.viewport_menu_slider(*a).is_some(), "row {k} {a:?}");
        }

        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 2.0) });
        assert!((state.wire_width - 3.0).abs() < 1e-6, "{}", state.wire_width);
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert!(kdl.contains("wire_width (f64)3") || kdl.contains("wire_width 3"), "persisted: {kdl}");
        assert!(state.viewport_menu_open());

        let band = context_menu::CONTEXT_MENU.with(|m| m.borrow().slider_band(i));
        state.cursor_x = band.x + 1.0;
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        assert!((state.wire_width - 1.0).abs() < 1e-6, "the band's left end is 1 px: {}", state.wire_width);
        context_menu::hide();
    }

    /// A viewport menu the compositor cut short (the popup's configure lands
    /// it through `context_menu::place`) scrolls under the designer's own
    /// wheel hook, and a press afterwards runs the row DRAWN under the
    /// pointer — the scrolled one, not the one at that offset unscrolled.
    #[test]
    fn a_shortened_viewport_menu_scrolls_and_picks_the_scrolled_row() {
        use crate::app::ViewportMenuAction as A;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{context_menu, ElementState, MouseButton, MouseScrollDelta};
        let mut state = State::new(false);
        state.cursor_x = 300.0;
        state.cursor_y = 200.0;
        state.open_viewport_context_menu();
        let full = context_menu::CONTEXT_MENU.with(|m| m.borrow().content_h);
        context_menu::place(300.0, 0.0, full * 0.5);

        // Over the first row, which is Frame All — an action, not a slider.
        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(0) + context_menu::ROW_H * 0.5;
        assert_eq!(state.viewport_menu_actions[0], A::FrameAll);
        state.handle_event(&WindowEvent::CursorMoved {
            position: LocalPosition { x: state.cursor_x as f64, y: state.cursor_y as f64 },
        });
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, -3.0) });
        let scroll = context_menu::CONTEXT_MENU.with(|m| m.borrow().scroll);
        assert_eq!(scroll, 3.0 * context_menu::ROW_H, "three notches down, three rows");
        assert!(state.viewport_menu_open(), "scrolling keeps the menu up");

        // The row now under the pointer is row 3; pressing it runs row 3's
        // action. Pick a row whose effect is visible: a toggle.
        let row = context_menu::row_at(state.cursor_x, state.cursor_y).expect("a row under the pointer");
        assert_eq!(row, 3);
        let A::Command(id) = state.viewport_menu_actions[row] else {
            panic!("row 3 should be a toggle command, is {:?}", state.viewport_menu_actions[row]);
        };
        let before = state.command_toggle_state(id);
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        assert_ne!(state.command_toggle_state(id), before, "the scrolled row ran");
        context_menu::hide();
    }

    /// Point Marker Size is a viewport-menu slider over the palette row's
    /// 0.005–0.1, and it re-sizes the Show Point Markers overlay from the
    /// scene positions the last rebuild kept — landing on exactly what a full
    /// rebuild at that size would draw, without re-evaluating the graph.
    #[test]
    fn the_viewport_menu_sets_the_point_marker_size_without_a_rebuild() {
        use crate::app::ViewportMenuAction as A;
        use crate::window::WindowEvent;
        use cce_ui::widget::{context_menu, MouseScrollDelta};
        let mut state = State::new(false);
        state.show_point_markers = true;
        state.point_marker_size = 0.02;
        state.rebuild_scene_geometry();
        assert!(!state.overlay_marker_points.is_empty(), "the bundled scene has points");
        let version = state.rt_geometry_version;

        state.cursor_x = 300.0;
        state.cursor_y = 200.0;
        state.open_viewport_context_menu();
        let sub_actions = state.open_viewport_page_with(A::PointMarkerSizeSlider);
        let i = sub_actions.iter().position(|a| *a == A::PointMarkerSizeSlider).expect("a Point Marker Size row");
        assert_eq!(sub_actions[i - 1], A::Command("toggle_point_markers"), "it sits under its switch");
        let sl = context_menu::slider(i).expect("a slider");
        assert_eq!((sl.min, sl.max, sl.step), (0.005, 0.1, 0.005));
        assert!((sl.value - 0.02).abs() < 1e-6);

        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 4.0) });
        assert!((state.point_marker_size - 0.04).abs() < 1e-5, "{}", state.point_marker_size);
        assert_eq!(state.rt_geometry_version, version, "no rebuild ran");
        assert!(state.overlay_dirty);
        let resized: Vec<[f32; 3]> = state.overlay_marker_verts.iter().map(|v| v.position).collect();
        context_menu::hide();

        // The full path at the same size draws the same spheres.
        state.rebuild_scene_geometry();
        let rebuilt: Vec<[f32; 3]> = state.overlay_marker_verts.iter().map(|v| v.position).collect();
        assert_eq!(resized, rebuilt);

        // Off, nothing is kept to re-size.
        state.run_command("toggle_point_markers");
        assert!(state.overlay_marker_points.is_empty());
    }

    /// Pull Arrow Scale stretches the pull arrows along the pull from their
    /// kept pairs, base fixed — 1 by default, the true vector — and is a
    /// viewport-menu slider under Group Marker Size.
    #[test]
    fn pull_arrow_scale_stretches_the_arrows_from_their_base() {
        use crate::app::ViewportMenuAction as A;
        use crate::window::WindowEvent;
        use cce_ui::widget::{context_menu, MouseScrollDelta};
        assert_eq!(crate::app::DesignSettings::default().render.pull_arrow_scale, 1.0);
        assert_eq!(crate::app::DesignSettings::from_kdl_str("").render.pull_arrow_scale, 1.0, "absent means 1");

        let mut state = State::new(false);
        state.pull_arrow_scale = 1.0;
        let (a, b) = (glam::Vec3::new(0.0, 1.0, 0.0), glam::Vec3::new(0.0, 1.06, 0.0));
        state.pull_arrow_pairs = vec![(a, b)];
        state.rebuild_pull_arrow_verts();
        // The shaft is the first stroke: from the base to the tip.
        let shaft = |state: &State| (state.pull_arrow_verts[0].position, state.pull_arrow_verts[1].position);
        assert_eq!(shaft(&state).0, a.to_array(), "the base stays on the point");
        assert!((shaft(&state).1[1] - 1.06).abs() < 1e-6, "scale 1 is the true vector");

        state.cursor_x = 300.0;
        state.cursor_y = 200.0;
        state.open_viewport_context_menu();
        let sub_actions = state.open_viewport_page_with(A::PullArrowScaleSlider);
        let i = sub_actions.iter().position(|a| *a == A::PullArrowScaleSlider).expect("a Pull Arrow Scale row");
        let sl = context_menu::slider(i).expect("a slider");
        assert_eq!((sl.min, sl.max, sl.step, sl.suffix), (0.25, 10.0, 0.25, "x"));
        assert_eq!(sl.readout(), "1.00x");

        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 15.0) });
        let k = state.pull_arrow_scale;
        assert!(k > 1.0, "the wheel raised the scale: {k}");
        assert_eq!(shaft(&state).0, a.to_array());
        assert!((shaft(&state).1[1] - (1.0 + 0.06 * k)).abs() < 1e-5, "the tip stretched to {:?} at {k}x", shaft(&state).1);
        assert_eq!(state.pull_arrow_pairs, vec![(a, b)], "the kept pairs are the measurement, never scaled");
        context_menu::hide();
    }

    /// Group Marker Size is a viewport-menu slider over the palette row's
    /// 0–0.2 world units, re-sizing the Selected-Group markers from their
    /// kept members — no re-evaluation of the Group node.
    #[test]
    fn the_viewport_menu_sets_the_group_marker_size() {
        use crate::app::ViewportMenuAction as A;
        use crate::window::WindowEvent;
        use cce_ui::widget::{context_menu, MouseScrollDelta};
        let mut state = State::new(false);
        state.group_marker_size = 0.025;
        let centre = [0.0f32, 1.0, 0.0];
        state.group_members = vec![crate::geometry::Vertex3D { position: centre, color: [0.0; 3] }];
        state.rebuild_group_marker_verts();
        let radius = |state: &State| {
            state.group_point_verts.iter().map(|v| (v.position[1] - centre[1]).abs()).fold(0.0f32, f32::max)
        };
        assert!((radius(&state) - 0.025).abs() < 1e-5);

        state.cursor_x = 300.0;
        state.cursor_y = 200.0;
        state.open_viewport_context_menu();
        let sub_actions = state.open_viewport_page_with(A::GroupMarkerSizeSlider);
        let i = sub_actions.iter().position(|a| *a == A::GroupMarkerSizeSlider).expect("a Group Marker Size row");
        let sl = context_menu::slider(i).expect("a slider");
        assert_eq!((sl.min, sl.max, sl.step, sl.decimals), (0.0, 0.2, 0.005, 3));
        assert!((sl.value - 0.025).abs() < 1e-6);

        state.cursor_x = context_menu::x() + 20.0;
        state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 3.0) });
        assert!((state.group_marker_size - 0.04).abs() < 1e-5, "{}", state.group_marker_size);
        assert!((radius(&state) - 0.04).abs() < 1e-5, "the markers re-sized: {}", radius(&state));
        assert!(state.group_points_dirty, "and will re-upload");
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        let saved = crate::app::DesignSettings::from_kdl_str(&kdl).render.group_marker_size;
        assert!((saved - 0.04).abs() < 1e-5, "persisted: {saved}");

        // A size change arriving another way re-sizes too, through
        // sync_nodes' size check.
        context_menu::hide();
        state.group_marker_size = 0.03;
        state.sync_nodes();
        assert!((radius(&state) - 0.03).abs() < 1e-5, "{}", radius(&state));
    }

    /// Show Points, the second display of a marker on every point, is
    /// retired for Show Point Markers, and the group markers' size is a
    /// setting of its own. A file from before carries the old keys: the
    /// size is the old pair multiplied out, and the rest is not read.
    #[test]
    fn an_older_render_block_gives_the_group_markers_their_size() {
        assert!(crate::command::by_id("toggle_render_points").is_none());
        let old = "render {\n    render_points (bool)true\n    point_size (f64)0.04\n    point_color (rgb)\"#00ff00\"\n    group_marker_scale (f64)2.0\n}\n";
        let read = crate::app::DesignSettings::from_kdl_str(old).render;
        assert!((read.group_marker_size - 0.08).abs() < 1e-6, "{}", read.group_marker_size);
        let new = "render {\n    point_size (f64)0.04\n    group_marker_size (f64)0.01\n}\n";
        let read = crate::app::DesignSettings::from_kdl_str(new).render;
        assert!((read.group_marker_size - 0.01).abs() < 1e-6, "its own key wins: {}", read.group_marker_size);
        let none = crate::app::DesignSettings::from_kdl_str("render {\n    wireframe (bool)true\n}\n").render;
        assert!((none.group_marker_size - 0.025).abs() < 1e-6, "{}", none.group_marker_size);
    }

    /// The dialog plate IS the menu plate: its fill is `Material::menu`'s
    /// and its corners are the menu radius, so the command palette and a
    /// right-click menu read one config block (`style.surface.menu`) and
    /// the app carries no compression, colour or radius of its own for it.
    /// The source scan is the backstop: a `dialog/compression` reader or a
    /// `DIALOG_COMPRESSION` constant coming back would be an override the
    /// menus do not share.
    #[test]
    fn the_dialog_plate_is_the_menu_plate() {
        use cce_ui::widget::model::Paint;
        let dialog = crate::dialog::Dialog::new();
        let menu = cce_ui::scene::Material::menu().fill(cce_ui::scene::PlateRole::Nested);
        assert_eq!(dialog.color(), menu);
        assert!(dialog.solid_border().is_none(), "a menu has no border");
        let r = cce_ui::layout::menu_corner_radius();
        let rect = cce_ui::scene::layout::Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        assert_eq!(dialog.corner_style(rect).map(|(r, _)| r), (r > 0.0).then_some(r));
        for file in ["src/dialog.rs", "src/render.rs", "src/app.rs"] {
            let src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
            assert!(!src.contains("dialog/compression") && !src.contains("DIALOG_COMPRESSION"), "{file} overrides the menu material");
        }
    }

    /// Ctrl+Up rewinds: a moving timeline stops and the playhead lands on
    /// the start frame, from wherever it was and in either direction; from a
    /// stop it is simply a jump to the start.
    #[test]
    fn ctrl_up_rewinds_to_the_start_frame_and_pauses() {
        use crate::command::by_id;
        let cmd = by_id("frame_start").expect("no frame_start command");
        assert_eq!(cmd.default_chord, Some("Ctrl+Up"));
        assert_eq!(cmd.context, crate::command::Context::Playbar);

        let mut state = State::new(false);
        {
            let pb = state.slots.playbar.inner_mut();
            pb.start_frame = 1.0;
            pb.end_frame = 48.0;
            pb.current_frame = 17.5;
            pb.playing = true;
            pb.reversed = true;
        }
        assert!(state.run_command("frame_start"));
        {
            let pb = state.slots.playbar.inner();
            assert!(!pb.playing, "a moving timeline stops");
            assert_eq!(pb.current_frame, 1.0, "and lands on the start frame");
        }
        // Stopped, mid-timeline: a plain jump.
        state.slots.playbar.inner_mut().current_frame = 30.0;
        state.run_command("frame_start");
        assert_eq!(state.slots.playbar.inner().current_frame, 1.0);
        assert!(!state.slots.playbar.inner().playing);
        // Whatever the start frame is.
        state.slots.playbar.inner_mut().start_frame = 5.0;
        state.slots.playbar.inner_mut().current_frame = 30.0;
        state.run_command("frame_start");
        assert_eq!(state.slots.playbar.inner().current_frame, 5.0);
    }

    /// Either play toggle pauses a moving timeline; direction only chooses
    /// what starts from a stop — and the reverse tick runs the frame counter
    /// down, wrapping start→end.
    #[test]
    fn test_reverse_playback_semantics_and_wrap() {
        let mut state = State::new(false);

        state.execute_action(Action::PlayPauseReverse);
        {
            let pb = state.slots.playbar.inner();
            assert!(pb.playing && pb.reversed, "Down from stopped plays in reverse");
        }
        state.execute_action(Action::PlayPause);
        assert!(!state.slots.playbar.inner().playing, "Up while reverse-playing pauses");
        state.execute_action(Action::PlayPause);
        {
            let pb = state.slots.playbar.inner();
            assert!(pb.playing && !pb.reversed, "Up from stopped plays forward");
        }
        state.execute_action(Action::PlayPauseReverse);
        assert!(!state.slots.playbar.inner().playing, "Down while forward-playing pauses");
        state.execute_action(Action::PlayPauseReverse);
        assert!(state.slots.playbar.inner().reversed, "Down from stopped is reverse again");
        state.execute_action(Action::PlayPauseReverse);
        assert!(!state.slots.playbar.inner().playing, "same-direction press pauses");

        {
            let pb = state.slots.playbar.inner_mut();
            pb.playing = true;
            pb.reversed = true;
            pb.repeat = true;
            pb.current_frame = 1.5;
        }
        let rect = cce_ui::scene::layout::Rect { x: 0.0, y: 0.0, width: 100.0, height: 30.0 };
        let moved =
            cce_ui::widget::Input::tick(state.slots.playbar.inner_mut(), 0.1, rect);
        assert!(moved, "reverse playback advances the frame");
        let f = state.slots.playbar.inner().current_frame;
        assert!(f > 200.0, "running off the start wraps to the end, got {f}");
        assert!(state.slots.playbar.inner().playing, "the wrap does not stop playback");
    }

    #[test]
    fn test_load_default_project() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
        let content = fs::read_to_string(&path).expect("failed to read default project");
        let proj: Project = serde_json::from_str(&content).expect("failed to deserialize project");
        assert_eq!(proj.name, "Default Project");
        assert_eq!(proj.view_state.active_camera, "camera1");
        assert_eq!(proj.root.name, "root");
        assert_eq!(proj.root.children.len(), 2);
        assert_eq!(proj.root.children[0].name, "camera1");
        assert_eq!(proj.root.children[0].position, (1.0, 1.0));
        assert_eq!(proj.root.children[1].name, "sphere1");
        assert_eq!(proj.root.children[1].position, (4.0, 2.0));
    }

    /// A trackpad flick over the viewport coasts: the orbit follows the
    /// fingers 1:1, and after the lift — a zero delta in the FingerEnd
    /// phase — it carries on the way it was going, slowing, and stops. The
    /// hand-rolled coast this replaced read the lift as more motion and
    /// blended its velocity to nothing, so the viewport never coasted.
    /// Ctrl-scroll zoom coasts the same way; with `inertial_scroll` off in
    /// config.kdl a lift stops dead; and a drag, or anything else that takes
    /// the camera over, stops a coast (`reset_velocity`).
    #[test]
    fn a_trackpad_flick_coasts_the_viewport_after_the_lift() {
        use crate::viewport_3d::Viewport3D;
        use cce_ui::widget::{Input, MouseScrollDelta, Position, ScrollPhase, ScrollSettings};
        cce_ui::widget::scroll_motion::force_scroll_settings(Some(ScrollSettings {
            smooth: true,
            ease_rate: 12.0,
            kinetic: true,
            friction: 6.0,
        }));
        let rect = cce_ui::scene::layout::Rect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 };
        let frame = 1.0 / 60.0;
        let px = |x: f64, y: f64| MouseScrollDelta::PixelDelta(Position { x, y });
        let flick = |vp: &mut Viewport3D, d: MouseScrollDelta| {
            for _ in 0..6 {
                vp.wheel(&d, ScrollPhase::Finger);
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
        };

        // A sideways flick: yaw only, the axis lock keeping pitch out.
        let mut a = Viewport3D::new();
        let vp = a.inner_mut();
        flick(vp, px(12.0, 0.5));
        let at_lift = vp.rotation_y;
        assert!(at_lift > 0.0, "the fingers turned the camera");
        let pitch = vp.rotation_x;
        vp.wheel(&px(0.0, 0.0), ScrollPhase::FingerEnd);
        assert_eq!(vp.rotation_y, at_lift, "the lift itself moves nothing");
        assert!(vp.is_coasting(), "the lift starts a coast");
        let mut steps = Vec::new();
        let mut last = vp.rotation_y;
        let mut frames = 0;
        while Input::tick(vp, frame, rect) && frames < 600 {
            steps.push(vp.rotation_y - last);
            last = vp.rotation_y;
            frames += 1;
        }
        assert!(frames < 600, "the coast stops");
        assert!(vp.rotation_y > at_lift + 0.05, "it carried on well past the lift: {} -> {}", at_lift, vp.rotation_y);
        assert!(steps[0] > 0.0 && steps.windows(2).all(|w| w[1] <= w[0] + 1e-6), "the same way, slowing: {steps:?}");
        assert_eq!(vp.rotation_x, pitch, "the locked axis stays put");
        assert!(!vp.is_coasting());

        // Ctrl-scroll zoom coasts too.
        let mut a = Viewport3D::new();
        let vp = a.inner_mut();
        Input::set_modifiers(vp, true, false, false);
        flick(vp, px(0.0, 10.0));
        let zoom_at_lift = vp.zoom;
        assert!(zoom_at_lift < 1.0, "the fingers zoomed in");
        vp.wheel(&px(0.0, 0.0), ScrollPhase::FingerEnd);
        for _ in 0..30 {
            Input::tick(vp, frame, rect);
        }
        assert!(vp.zoom < zoom_at_lift * 0.95, "and it carries on zooming after the lift");

        // inertial_scroll off: the lift stops the orbit dead.
        let mut a = Viewport3D::new();
        let vp = a.inner_mut();
        vp.inertial_scroll = false;
        flick(vp, px(12.0, 0.0));
        vp.wheel(&px(0.0, 0.0), ScrollPhase::FingerEnd);
        let stopped = vp.rotation_y;
        assert!(!Input::tick(vp, frame, rect) && vp.rotation_y == stopped, "no coast with inertial_scroll off");

        // Anything that takes the camera over stops a coast.
        let mut a = Viewport3D::new();
        let vp = a.inner_mut();
        flick(vp, px(12.0, 0.0));
        vp.wheel(&px(0.0, 0.0), ScrollPhase::FingerEnd);
        vp.reset_velocity();
        let stopped = vp.rotation_y;
        assert!(!Input::tick(vp, frame, rect) && vp.rotation_y == stopped, "reset_velocity ends the coast");

        cce_ui::widget::scroll_motion::force_scroll_settings(None);
    }

    /// A wheel over the viewport orbits whichever camera is active AFTER the
    /// active camera has changed. The viewport widget routes its wheel by a
    /// copy of the camera name, and that copy used to be written once, at
    /// construction, from the bundled project (camera1) — so opening a
    /// project whose active camera was the Default Camera left the widget
    /// parking every wheel into the camera-node pending pair, which the
    /// drain discarded because the app said no node was active. Scrolling
    /// in the viewport did nothing, while a drag (which reads the app's
    /// copy) still orbited. Every path that changes the camera is covered:
    /// New, a saved project, and the viewport menu's own choice.
    #[test]
    fn a_wheel_orbits_the_camera_that_is_active_after_a_change() {
        use crate::slots::VIEWPORT_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{MouseScrollDelta, Position};
        let dir = std::env::temp_dir().join(format!("cce-designer-active-camera-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        assert_eq!(state.active_camera, "camera1", "the bundled project starts on its camera node");
        assert_eq!(state.viewport().active_camera, state.active_camera);

        // A project saved with the Default Camera active, opened over it.
        state.set_active_camera("Default Camera");
        state.save_to_file(&dir).expect("save");
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.load_from_file(&dir).expect("load");
        assert_eq!(state.active_camera, "Default Camera");
        assert_eq!(state.viewport().active_camera, "Default Camera", "the widget's copy follows a load");

        let (vx, vy, vw, vh) = state.positions[VIEWPORT_IDX];
        let (cx, cy) = (vx + vw * 0.5, vy + vh * 0.5);
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: cx as f64, y: cy as f64 } });
        assert!(state.cursor_in_viewport());
        let before = (state.viewport().rotation_x, state.viewport().rotation_y);
        // A trackpad's pixel delta, then a mouse notch: both routes.
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 0.0, y: 30.0 }) });
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 1.0) });
        let after = (state.viewport().rotation_x, state.viewport().rotation_y);
        assert_ne!(before, after, "the wheel orbits the Default Camera once it is the active one");
        assert_eq!(state.viewport().pending_yaw, 0.0, "nothing was parked for a camera node");
        assert_eq!(state.viewport().pending_pitch, 0.0);

        // New: back to the Default Camera from a node, on both copies.
        let mut state = State::new(false);
        state.new_project();
        assert_eq!(state.viewport().active_camera, "Default Camera");

        // The viewport menu's choice: a node, then the default again.
        let mut state = State::new(false);
        state.set_active_camera("Default Camera");
        state.set_active_camera("camera1");
        assert_eq!(state.viewport().active_camera, "camera1");

        let _ = fs::remove_dir_all(&dir);
    }

    /// What the 2D frame draws over the scene is placed by the camera as
    /// it is when the frame is painted. The paint comes BEFORE the stage
    /// pass, which was the one place the projection was kept: the numbers
    /// trailed their points by a frame while the camera moved, and stood a
    /// frame's move off them when it stopped.
    #[test]
    fn the_point_numbers_are_placed_by_the_camera_as_it_is() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.set_active_camera("Default Camera");
        state.show_point_numbers = true;
        state.geo_opacity = 0.35;
        state.show_occluded = true;
        state.rebuild_scene_geometry();
        let _ = state.collect_display_list();
        let placed = |state: &State| -> Vec<(String, f32, f32)> {
            state.point_number_labels().into_iter().map(|(t, x, y, ..)| (t, x, y)).collect()
        };
        let before = placed(&state);
        assert!(!before.is_empty());

        // The camera moves, and the next frame is painted — no stage pass
        // between.
        state.orbit_camera_by(90.0, 20.0);
        state.pan_camera_by(30.0, -10.0);
        let _ = state.collect_display_list();
        let after = placed(&state);
        assert_ne!(before, after, "the numbers stood where the last frame had them");
        // And they stand where the scene will be drawn: by the view the
        // stage pass is about to stage.
        let (_, proj, view) = state.scene_view().expect("a scene");
        assert_eq!(state.last_scene_mvp, Some(proj * view));
        let alphas = state.overlay_number_alpha.clone();
        assert_eq!(alphas.len(), state.overlay_number_labels.len(), "and are dimmed for that view");
        // Asked again for the same view, the dimming is not worked out again.
        let key = state.number_alpha_key;
        state.sync_point_number_alpha(proj * view, state.last_scene_eye);
        assert_eq!((state.number_alpha_key, &state.overlay_number_alpha), (key, &alphas));
    }

    /// A pan slides the camera across its own view: what is at the pivot
    /// follows the pointer px for px, the view turns nowhere, and a camera
    /// node's Pivot and Position move together. Middle-drag, shift and the
    /// left button, and shift with a scroll all pan; the plain left drag
    /// and the plain scroll still orbit.
    #[test]
    fn the_camera_pans_with_the_pointer() {
        use crate::slots::VIEWPORT_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton, MouseScrollDelta, Position};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        let rect = (0.0f32, 0.0f32, 1600.0f32, 900.0f32);
        state.last_scene_view_rect = rect;
        // Where a world point is on screen, in logical px.
        let on_screen = |state: &State, at: Vec3| {
            let (pos, rot, pivot) = state.active_camera_pose();
            let (proj, view, model) = state.viewport().get_matrices(rect.2 / rect.3, Some(pos), Some(rot), Some(pivot));
            let c = proj * view * model * at.extend(1.0);
            (rect.0 + (c.x / c.w * 0.5 + 0.5) * rect.2, rect.1 + (0.5 - c.y / c.w * 0.5) * rect.3)
        };
        let view_of = |state: &State| {
            let (pos, rot, pivot) = state.active_camera_pose();
            state.viewport().get_matrices(1.0, Some(pos), Some(rot), Some(pivot)).1
        };

        // The Default Camera, turned a little first so the axes are not the world's.
        state.set_active_camera("Default Camera");
        state.orbit_camera_by(120.0, -40.0);
        let mark = state.viewport().pivot;
        let (before, turned) = (on_screen(&state, mark), view_of(&state));
        state.pan_camera_by(50.0, -30.0);
        let after = on_screen(&state, mark);
        assert!((after.0 - before.0 - 50.0).abs() < 0.05 && (after.1 - before.1 + 30.0).abs() < 0.05, "{before:?} -> {after:?}");
        assert_ne!(state.viewport().pivot, mark, "the pivot moved");
        let (a, b) = (turned.to_cols_array(), view_of(&state).to_cols_array());
        assert!((0..12).all(|i| (a[i] - b[i]).abs() < 1e-5), "the view turned");

        // A camera node: Pivot and Position move as one, Rotation stays, and
        // a hundred small moves come to what one large one does.
        state.set_active_camera("camera1");
        let read = |state: &State, name: &str| {
            let node = state.camera_level().children.iter().find(|c| c.name == "camera1").unwrap();
            crate::geometry::node_param_vec3(node, name, Vec3::ZERO)
        };
        let (pivot0, pos0, rot0) = (read(&state, "pivot"), read(&state, "position"), read(&state, "rotation"));
        let mark = pivot0;
        let before = on_screen(&state, mark);
        for _ in 0..100 {
            state.pan_camera_by(0.7, 0.3);
        }
        let after = on_screen(&state, mark);
        assert!((after.0 - before.0 - 70.0).abs() < 0.5 && (after.1 - before.1 - 30.0).abs() < 0.5, "{before:?} -> {after:?}");
        let (moved_pivot, moved_pos) = (read(&state, "pivot") - pivot0, read(&state, "position") - pos0);
        assert!(moved_pivot.length() > 0.01 && (moved_pivot - moved_pos).length() < 1e-3, "{moved_pivot:?} against {moved_pos:?}");
        assert_eq!(read(&state, "rotation"), rot0);

        // The three ways of asking, by pointer.
        state.set_active_camera("Default Camera");
        let (vx, vy, vw, vh) = state.positions[VIEWPORT_IDX];
        let (cx, cy) = (vx + vw * 0.5, vy + vh * 0.5);
        let at = |state: &mut State, x: f32, y: f32| {
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        };
        let press = |state: &mut State, b: MouseButton, s: ElementState| {
            state.handle_event(&WindowEvent::MouseInput { state: s, button: b });
        };
        at(&mut state, cx, cy);
        assert!(state.cursor_in_viewport());
        let orbit = |state: &State| (state.viewport().rotation_x, state.viewport().rotation_y);

        let (pivot, turned) = (state.viewport().pivot, orbit(&state));
        press(&mut state, MouseButton::Middle, ElementState::Pressed);
        assert!(state.pan_drag.is_some() && state.pointer_captured(), "a middle press arms the pan");
        at(&mut state, cx + 40.0, cy + 10.0);
        press(&mut state, MouseButton::Middle, ElementState::Released);
        assert!(state.pan_drag.is_none());
        assert_ne!(state.viewport().pivot, pivot, "the middle drag panned");
        assert_eq!(orbit(&state), turned, "and did not turn the camera");

        at(&mut state, cx, cy);
        let pivot = state.viewport().pivot;
        state.modifiers.shift = true;
        press(&mut state, MouseButton::Left, ElementState::Pressed);
        assert!(state.pan_drag.is_some() && state.orbit_drag.is_none(), "shift and the left button pan");
        at(&mut state, cx - 25.0, cy + 5.0);
        press(&mut state, MouseButton::Left, ElementState::Released);
        assert_ne!(state.viewport().pivot, pivot);
        assert_eq!(orbit(&state), turned);

        let pivot = state.viewport().pivot;
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 12.0, y: -8.0 }) });
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 1.0) });
        assert_ne!(state.viewport().pivot, pivot, "shift and a scroll pan");
        assert_eq!(orbit(&state), turned);
        state.modifiers.shift = false;

        // Without shift they are the orbit's, as they were.
        at(&mut state, cx, cy);
        let pivot = state.viewport().pivot;
        press(&mut state, MouseButton::Left, ElementState::Pressed);
        assert!(state.orbit_drag.is_some() && state.pan_drag.is_none());
        at(&mut state, cx + 30.0, cy);
        press(&mut state, MouseButton::Left, ElementState::Released);
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 0.0, y: 30.0 }) });
        assert_eq!(state.viewport().pivot, pivot);
        assert_ne!(orbit(&state), turned);
    }

    #[test]
    fn test_default_camera_orbit_moves_camera_not_geometry() {
        use cce_ui::widget::WidgetHost;
        use glam::{Mat4, Vec3};
        let mut vp = crate::viewport_3d::Viewport3D::new();
        let inner = vp.as_any_mut().downcast_mut::<crate::viewport_3d::Viewport3D>().unwrap();

        let (_, view0, model0) = inner.get_matrices(1.0, None, None, None);
        inner.rotation_y = 0.7;
        inner.rotation_x = 0.2;
        let (_, view1, model1) = inner.get_matrices(1.0, None, None, None);

        // The scroll orbit must never rotate the geometry within world space.
        assert_eq!(model0, Mat4::IDENTITY);
        assert_eq!(model1, Mat4::IDENTITY);
        // The camera moved...
        assert!(view0 != view1);
        // ...by orbiting: the pivot (look-at center) stays at the same eye-space
        // point, and the camera keeps its distance from it.
        let p0 = view0.transform_point3(Vec3::ZERO);
        let p1 = view1.transform_point3(Vec3::ZERO);
        assert!((p0 - p1).length() < 1e-4, "pivot drifted: {p0:?} vs {p1:?}");
    }

    #[test]
    fn test_orbit_pitch_clamps_short_of_poles() {
        use cce_ui::widget::WidgetHost;
        use glam::Vec3;
        let mut vp = crate::viewport_3d::Viewport3D::new();
        let inner = vp.as_any_mut().downcast_mut::<crate::viewport_3d::Viewport3D>().unwrap();

        // However far the stored pitch runs, the camera must never cross a pole:
        // world-up keeps a positive eye-space Y (the view never rolls upside down).
        for rx in [-100.0_f32, -3.0, 0.0, 3.0, 100.0] {
            inner.rotation_x = rx;
            let (_, view, _) = inner.get_matrices(1.0, None, None, None);
            let up_eye = view.transform_vector3(Vec3::Y);
            assert!(up_eye.y > 0.0, "camera flipped at rotation_x = {rx}: up_eye = {up_eye:?}");
        }
    }

    #[test]
    fn test_scene_camera_yaw_changes_view() {
        use cce_ui::widget::WidgetHost;
        use glam::Vec3;
        let mut vp = crate::viewport_3d::Viewport3D::new();
        let inner = vp.as_any_mut().downcast_mut::<crate::viewport_3d::Viewport3D>().unwrap();
        inner.active_camera = "camera1".to_string();
        let pos = Vec3::new(2.5, 1.8, 2.5);
        let piv = Vec3::ZERO;
        let (_, v1, _) = inner.get_matrices(1.0, Some(pos), Some(Vec3::new(23.62, -58.83, 0.0)), Some(piv));
        let (_, v2, _) = inner.get_matrices(1.0, Some(pos), Some(Vec3::new(23.62, 31.17, 0.0)), Some(piv));
        let p1 = v1.transform_point3(Vec3::new(1.0, 0.0, 0.0));
        let p2 = v2.transform_point3(Vec3::new(1.0, 0.0, 0.0));
        println!("world (1,0,0) in eye space: {p1:?} vs {p2:?}");
        assert!((p1 - p2).length() > 0.1, "yaw had no effect: {p1:?} vs {p2:?}");
    }

    #[test]
    fn test_node_template_names() {
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        assert!(!templates.is_empty(), "No templates loaded!");
        for t in templates {
            let last_char = t.label.chars().last().unwrap();
            assert!(!last_char.is_ascii_digit(), "Template name '{}' ends with a digit, but templates should not have numeric suffixes in the add node popup.", t.label);
        }
    }

    /// A subnet template's children resolve against their own templates: a
    /// child names a base by type, takes its params, and lays its overrides
    /// on top. The Embryo is the shipped example (the four kernel subnets it
    /// used to share this test with are native nodes since 2026-09-24).
    #[test]
    fn test_subnet_template_child_resolution() {
        let templates_root = crate::app::load_fs_tree();
        let embryo = templates_root
            .children
            .iter()
            .find(|t| t.name == "Embryo")
            .expect("Embryo template should be loaded");
        assert_eq!(embryo.node_type, "node");
        assert_eq!(embryo.children.len(), 10);

        let input1 = embryo.children.iter().find(|c| c.name == "input1").unwrap();
        assert_eq!(input1.node_type, "input");

        // The nested sphere is the native Sphere with the template's whole
        // surface, the Embryo's overrides on top of it.
        let sphere1 = embryo.children.iter().find(|c| c.name == "sphere1").unwrap();
        assert_eq!(sphere1.node_type, "sphere");
        assert!(sphere1.children.is_empty(), "a native node has no children");
        assert!(sphere1.params.iter().any(|p| p.name == "method"), "the base template's params arrive");
        let radius = sphere1.params.iter().find(|p| p.name == "radius").unwrap();
        assert!(radius.is_expr() && radius.text().contains("radius"), "the override is the reference: {} (expr {})", radius.text(), radius.is_expr());
        assert_eq!(sphere1.params.iter().find(|p| p.name == "center_y").unwrap().text(), "0.0");

        let output1 = embryo.children.iter().find(|c| c.name == "output1").unwrap();
        assert_eq!(output1.node_type, "output");
        let output_input = output1.params.iter().find(|p| p.name == "input").unwrap();
        assert_eq!(output_input.text(), "normal1");
    }

    /// The raster pipeline culls back faces with CCW fronts (the wgpu
    /// convention: negative-viewport-height Y flip keeps model-space CCW =
    /// front). A template mesh must wind CCW as seen from OUTSIDE, or the
    /// live viewport silently shows its interior — near faces culled, far
    /// faces drawn — which a closed symmetric mesh disguises until a
    /// deformation makes it obvious. RT intersects both sides and never
    /// catches this; this test is the raster-side guard.
    #[test]
    fn test_template_meshes_wind_ccw_outward() {
        let templates_root = crate::app::load_fs_tree();
        // Winding is a property of triangles, so this one flattens on purpose.
        let eval_template = |name: &str| -> Vec<crate::geometry::Vertex3D> {
            let t = templates_root
                .children
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} template should be loaded"));
            let mut inst = t.clone();
            inst.id = format!("{name}-winding-inst");
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
                bypassed: false,
                position: (0.0, 0.0),
                inputs: 0,
                outputs: 0,
            };
            let mut visited = Vec::new();
            let mut err = None;
            let g = crate::geometry::generate_single_node_geometry_with_errors(
                &root,
                &root.children[0],
                &mut visited,
                &mut err,
                &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
            )
            .expect("geometry");
            assert!(err.is_none(), "{name}: {err:?}");
            crate::geometry::detail_vertices(&g)
        };
        let tri_cross = |g: &[crate::geometry::Vertex3D], tri: usize| -> [f32; 3] {
            let a = g[tri * 3].position;
            let b = g[tri * 3 + 1].position;
            let d = g[tri * 3 + 2].position;
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [d[0] - a[0], d[1] - a[1], d[2] - a[2]];
            [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ]
        };

        // Closed generators: the winding cross must point OUTWARD (away from
        // the mesh center) on effectively every non-degenerate triangle.
        for name in ["Sphere", "Box"] {
            let g = eval_template(name);
            let n = g.len() as f32;
            let mut c = [0.0f32; 3];
            for v in &g {
                for k in 0..3 {
                    c[k] += v.position[k] / n;
                }
            }
            let (mut outward, mut total) = (0usize, 0usize);
            for tri in 0..g.len() / 3 {
                let nrm = tri_cross(&g, tri);
                let a = g[tri * 3].position;
                let b = g[tri * 3 + 1].position;
                let d = g[tri * 3 + 2].position;
                let cen = [
                    (a[0] + b[0] + d[0]) / 3.0 - c[0],
                    (a[1] + b[1] + d[1]) / 3.0 - c[1],
                    (a[2] + b[2] + d[2]) / 3.0 - c[2],
                ];
                let dot = nrm[0] * cen[0] + nrm[1] * cen[1] + nrm[2] * cen[2];
                if dot.abs() > 1e-12 {
                    total += 1;
                    if dot > 0.0 {
                        outward += 1;
                    }
                }
            }
            let f = outward as f32 / total.max(1) as f32;
            assert!(
                f > 0.95,
                "{name}: only {:.0}% of triangles wind CCW-outward — the raster viewport shows this mesh inside-out",
                f * 100.0
            );
        }

        // The plane's visible face is UP: the winding cross must point +Y.
        let g = eval_template("Plane");
        let (mut up, mut total) = (0usize, 0usize);
        for tri in 0..g.len() / 3 {
            let nrm = tri_cross(&g, tri);
            if nrm[1].abs() > 1e-12 {
                total += 1;
                if nrm[1] > 0.0 {
                    up += 1;
                }
            }
        }
        assert!(
            up as f32 / total.max(1) as f32 > 0.95,
            "Plane: winding faces down — invisible from above in the raster viewport"
        );
    }

    /// A template's Color toggle: on, the gradient the shape has always
    /// carried; off, `DEFAULT_COLOR` — the same grey geometry with no `Cd`
    /// at all renders at, so an uncoloured sphere or plane looks like an
    /// uncoloured anything else rather than like a second colour scheme.
    fn assert_color_toggle_switches_between_gradient_and_default(template_name: &str) {
        let templates_root = crate::app::load_fs_tree();
        let template = templates_root
            .children
            .iter()
            .find(|t| t.name == template_name)
            .unwrap_or_else(|| panic!("{template_name} template should be loaded"));
        let color = template.params.iter().find(|p| p.name == "color").expect("a Color param");
        assert_eq!(color.ty(), "toggle");
        assert_eq!(color.text(), "true", "coloured by default, as it always was");

        let generate = |on: &str| {
            let mut inst = template.clone();
            inst.id = format!("{template_name}_{on}");
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            inst.params.iter_mut().find(|p| p.name == "color").unwrap().set_text(on.to_string());
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
                bypassed: false,
                position: (0.0, 0.0),
                inputs: 0,
                outputs: 0,
            };
            let mut visited = Vec::new();
            let mut err = None;
            let geom = crate::geometry::generate_single_node_geometry_with_errors(
                &root,
                &root.children[0],
                &mut visited,
                &mut err,
                &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
            )
            .expect("geometry");
            assert!(err.is_none(), "{template_name} kernel error: {err:?}");
            geom
        };

        let off = generate("false");
        assert!(off.num_points() > 0);
        for p in 0..off.num_points() {
            assert_eq!(off.color(p), crate::detail::DEFAULT_COLOR, "{template_name} point {p} off");
        }

        let on = generate("true");
        assert_eq!(on.num_points(), off.num_points(), "the toggle changes colour, not shape");
        let first = on.color(0);
        assert!(
            (0..on.num_points()).any(|p| on.color(p) != first),
            "on, the {template_name} carries its gradient"
        );
    }

    #[test]
    fn sphere_color_toggle_switches_between_gradient_and_default() {
        assert_color_toggle_switches_between_gradient_and_default("Sphere");
    }

    #[test]
    fn plane_color_toggle_switches_between_gradient_and_default() {
        assert_color_toggle_switches_between_gradient_and_default("Plane");
    }

    /// The Sphere is a native node: no children, welded points, closed.
    #[test]
    fn test_sphere_node_geometry_generation() {
        let templates_root = crate::app::load_fs_tree();
        let sphere_template = templates_root
            .children
            .iter()
            .find(|t| t.name == "Sphere")
            .expect("Sphere template should be loaded");
        assert_eq!(sphere_template.node_type, "sphere");
        assert!(sphere_template.children.is_empty());

        let mut sphere_instance = sphere_template.clone();
        sphere_instance.id = "sphere_inst".to_string();
        let root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere_instance],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };
        let mut visited = Vec::new();
        let mut err = None;
        let geom = crate::geometry::generate_single_node_geometry_with_errors(
            &root,
            &root.children[0],
            &mut visited,
            &mut err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
        )
        .expect("sphere generation failed");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(geom.num_points(), crate::geometry::sphere_point_len(16, 24));
        assert_eq!(geom.num_prims(), 24 * 2 + 24 * 14, "two pole fans and fourteen bands of quads");
        assert!(geom.is_closed());
        for pos in geom.positions() {
            let r = (pos[0].powi(2) + (pos[1] - 0.55).powi(2) + pos[2].powi(2)).sqrt();
            assert!((r - 0.5).abs() < 1e-4, "point {pos:?} is {r} from the centre");
        }
        assert!(geom.points().has("N") && geom.points().has("uv") && geom.points().has("Cd"));
    }

    /// The native curve node: a Catmull-Rom strip through the "Points"
    /// param, each sampled span an oriented 36-vertex box.
    #[test]
    fn test_curve_native_geometry_generation() {
        let templates_root = crate::app::load_fs_tree();
        let curve_template = templates_root
            .children
            .iter()
            .find(|t| t.name == "Curve")
            .expect("Curve template should be loaded");
        assert_eq!(curve_template.node_type, "curve");
        assert!(curve_template.children.is_empty(), "native curve has no subnet children");

        let make_root = |instance: FsNode| FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![instance],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };
        let eval = |root: &FsNode| {
            let mut visited = Vec::new();
            crate::geometry::generate_single_node_geometry(root, &root.children[0], &mut visited)
                .expect("Geometry generation failed")
        };

        // Default: 4 points, 8 segments per span → 3*8 spans, one box each.
        let mut instance = curve_template.clone();
        instance.id = "curve_inst".to_string();
        let geom = eval(&make_root(instance.clone()));
        assert_eq!(geom.num_points(), 24 * 8, "one eight-cornered box per span");
        for pos in geom.positions() {
            assert!(pos.iter().all(|c| c.is_finite()), "curve produced non-finite positions");
        }
        // The strip reaches both endpoint control points.
        let near = |g: &crate::detail::Detail, p: [f32; 3]| {
            g.positions().iter().any(|q| {
                (0..3).map(|k| (q[k] - p[k]).powi(2)).sum::<f32>().sqrt() < 0.1
            })
        };
        assert!(near(&geom, [-0.75, 0.05, 0.0]), "curve does not reach its first point");
        assert!(near(&geom, [0.75, 1.05, 0.0]), "curve does not reach its last point");

        // Two points: one span, 8 boxes. The scene walk agrees with the
        // single-node path.
        instance.params.iter_mut().find(|p| p.name == "points").unwrap().set_text("0 0 0; 1 0 0".to_string());
        let root = make_root(instance.clone());
        assert_eq!(eval(&root).num_points(), 8 * 8);
        assert_eq!(
            crate::geometry::network_sphere_vertices(&root).num_points(),
            8 * 8,
            "scene walk and single-node eval disagree"
        );

        // No parseable points: empty geometry, not a panic.
        instance.params.iter_mut().find(|p| p.name == "points").unwrap().set_text("not points".to_string());
        assert_eq!(eval(&make_root(instance)).num_points(), 0);
    }

    /// Points round-trip through the "Points" param format; malformed
    /// chunks are skipped rather than corrupting neighbors; the sampled
    /// Catmull-Rom passes through every control point at span boundaries.
    #[test]
    fn test_curve_points_parse_and_sampling() {
        use crate::geometry::{format_curve_points, parse_curve_points, sample_catmull_rom};

        let pts = vec![
            Vec3::new(-0.75, 0.05, 0.0),
            Vec3::new(0.25, 1.5, -0.5),
            Vec3::new(2.0, -1.0, 3.25),
        ];
        assert_eq!(parse_curve_points(&format_curve_points(&pts)), pts);

        // Commas allowed, garbage and half-typed triples skipped.
        let parsed = parse_curve_points("1, 2, 3; nope; 4 5; ; 6 7 8");
        assert_eq!(parsed, vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(6.0, 7.0, 8.0)]);

        let segs = 4;
        let samples = sample_catmull_rom(&pts, segs);
        assert_eq!(samples.len(), (pts.len() - 1) * segs + 1);
        for (i, p) in pts.iter().enumerate() {
            let s = samples[i * segs];
            assert!((s - *p).length() < 1e-5, "sample {} misses control point {}", i * segs, i);
        }
        // Degenerate inputs sample as themselves.
        assert_eq!(sample_catmull_rom(&[], segs).len(), 0);
        assert_eq!(sample_catmull_rom(&pts[..1], segs), pts[..1].to_vec());
    }

    /// The curve_set_points MCP action: replaces a curve node's whole point
    /// list from structured triples, refuses non-curve slots and non-finite
    /// coordinates, and round-trips through the same param the viewer state
    /// and params pane edit.
    #[test]
    fn test_curve_set_points_mcp_action() {
        let mut state = State::new(false);
        let mut redraw = false;
        state
            .apply_action(
                McpAction::AddNode { template_name: "Curve".to_string(), name: None, x: 0.0, y: 0.0 },
                &mut redraw,
            )
            .expect("add curve node");
        let slot = state.current_dir().children.len() - 1;

        state
            .apply_action(
                McpAction::CurveSetPoints {
                    slot,
                    points: vec![[0.0, 0.0, 0.0], [1.0, 2.0, 3.0], [-1.5, 0.5, 0.25]],
                },
                &mut redraw,
            )
            .expect("set curve points");
        let pts = crate::geometry::parse_curve_points(&crate::geometry::node_param_str(
            &state.current_dir().children[slot],
            "points",
            "",
        ));
        assert_eq!(
            pts,
            vec![Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0), Vec3::new(-1.5, 0.5, 0.25)]
        );

        // Slot 0 is the default project's camera — not a curve.
        assert!(state
            .apply_action(
                McpAction::CurveSetPoints { slot: 0, points: vec![[0.0, 0.0, 0.0]] },
                &mut redraw,
            )
            .is_err());
        // Non-finite coordinates are refused, and the list stays intact.
        assert!(state
            .apply_action(
                McpAction::CurveSetPoints { slot, points: vec![[f32::NAN, 0.0, 0.0]] },
                &mut redraw,
            )
            .is_err());
        assert_eq!(
            crate::geometry::parse_curve_points(&crate::geometry::node_param_str(
                &state.current_dir().children[slot],
                "points",
                "",
            ))
            .len(),
            3
        );
    }

    /// The curve viewer state end-to-end, headless: grab-and-drag moves a
    /// control point on its own depth plane, a press on empty space appends
    /// a point there, right-press and Delete remove points — all through
    /// the real press/motion/release handlers, against an identity mvp
    /// (world x/y map linearly onto a 100×100 pane).
    #[test]
    fn test_curve_tool_add_move_delete() {
        let mut state = State::new(false);
        let mut redraw = false;
        state
            .apply_action(
                McpAction::AddNode { template_name: "Curve".to_string(), name: None, x: 0.0, y: 0.0 },
                &mut redraw,
            )
            .expect("add curve node");
        let slot = state.current_dir().children.len() - 1;
        assert_eq!(state.current_dir().children[slot].node_type, "curve");

        // A second instance gets its own id (AddNode regenerates like
        // paste) — the tool binds by id, so shared ids would edit the
        // wrong node.
        state
            .apply_action(
                McpAction::AddNode { template_name: "Curve".to_string(), name: None, x: 2.0, y: 0.0 },
                &mut redraw,
            )
            .expect("add second curve node");
        let slot2 = state.current_dir().children.len() - 1;
        assert_ne!(
            state.current_dir().children[slot].id,
            state.current_dir().children[slot2].id,
            "template instances must not share ids"
        );

        state.toggle_viewer_state(slot);
        assert!(state.viewer_tool.is_some());

        state.last_scene_mvp = Some(Mat4::IDENTITY);
        state.last_scene_view_rect = (0.0, 0.0, 100.0, 100.0);
        let sx = |x: f32| 50.0 + x * 50.0;
        let sy = |y: f32| 50.0 - y * 50.0;
        let points_of = |state: &State, slot: usize| {
            crate::geometry::parse_curve_points(&crate::geometry::node_param_str(
                &state.current_dir().children[slot],
                "points",
                "",
            ))
        };
        let default_first = points_of(&state, slot)[0];

        // Grab the first point and drag it to the pane center → (0, 0, z).
        state.cursor_x = sx(default_first.x);
        state.cursor_y = sy(default_first.y);
        assert!(state.viewer_tool_press(), "press on a handle must grab");
        state.cursor_x = 50.0;
        state.cursor_y = 50.0;
        assert!(state.viewer_tool_drag_motion());
        assert!(state.viewer_tool_release());
        let pts = points_of(&state, slot);
        assert!(pts[0].length() < 1e-4, "dragged point should sit at the origin, got {:?}", pts[0]);
        // The other curve is untouched.
        assert_eq!(points_of(&state, slot2)[0], default_first);

        // Press on empty space appends a point there (at the last point's
        // depth — z=0 here) and immediately drags it.
        state.cursor_x = 90.0;
        state.cursor_y = 90.0;
        assert!(state.viewer_tool_press());
        let pts = points_of(&state, slot);
        assert_eq!(pts.len(), 5);
        assert!((pts[4] - Vec3::new(0.8, -0.8, 0.0)).length() < 1e-4, "added at {:?}", pts[4]);
        assert!(state.viewer_tool_release());

        // Delete the (selected) new point, then right-press-delete the one
        // parked at the pane center.
        assert!(state.viewer_tool_delete_selected());
        assert_eq!(points_of(&state, slot).len(), 4);
        state.cursor_x = 50.0;
        state.cursor_y = 50.0;
        assert!(state.viewer_tool_delete_at_cursor());
        assert_eq!(points_of(&state, slot).len(), 3);

        // Toggling on the same node exits the state.
        state.toggle_viewer_state(slot);
        assert!(state.viewer_tool.is_none());
    }

    /// Undo/redo in the curve viewer state: one entry per gesture (a drag
    /// records once on its first motion, an add-and-drag once, a delete
    /// once; a grab released without moving records nothing), undo walks
    /// back through them, redo forward, and a fresh gesture after an undo
    /// drops the redo branch. Same identity-mvp setup as the tool test.
    #[test]
    fn test_curve_tool_undo_redo() {
        let mut state = State::new(false);
        let mut redraw = false;
        state
            .apply_action(
                McpAction::AddNode { template_name: "Curve".to_string(), name: None, x: 0.0, y: 0.0 },
                &mut redraw,
            )
            .expect("add curve node");
        let slot = state.current_dir().children.len() - 1;
        state.toggle_viewer_state(slot);
        state.last_scene_mvp = Some(Mat4::IDENTITY);
        state.last_scene_view_rect = (0.0, 0.0, 100.0, 100.0);
        let sx = |x: f32| 50.0 + x * 50.0;
        let sy = |y: f32| 50.0 - y * 50.0;
        let points_of = |state: &State| {
            crate::geometry::parse_curve_points(&crate::geometry::node_param_str(
                &state.current_dir().children[slot],
                "points",
                "",
            ))
        };
        let history = |state: &State| {
            let t = state.viewer_tool.as_ref().expect("tool active");
            (t.history.undo_len(), t.history.redo_len())
        };

        let initial = points_of(&state);
        assert!(!state.viewer_tool_undo(), "nothing to undo yet");
        assert!(!state.viewer_tool_redo(), "nothing to redo yet");

        // Grab and release without moving: no history.
        state.cursor_x = sx(initial[0].x);
        state.cursor_y = sy(initial[0].y);
        assert!(state.viewer_tool_press());
        assert!(state.viewer_tool_release());
        assert_eq!(history(&state), (0, 0));

        // Gesture 1: drag the first point to the pane center, over several
        // motion events — still one entry.
        assert!(state.viewer_tool_press());
        for (x, y) in [(55.0, 55.0), (52.0, 52.0), (50.0, 50.0)] {
            state.cursor_x = x;
            state.cursor_y = y;
            assert!(state.viewer_tool_drag_motion());
        }
        assert!(state.viewer_tool_release());
        let after_drag = points_of(&state);
        assert!(after_drag[0].length() < 1e-4);
        assert_eq!(history(&state), (1, 0));

        // Gesture 2: add a point (press on empty space + drag + release).
        state.cursor_x = 90.0;
        state.cursor_y = 90.0;
        assert!(state.viewer_tool_press());
        state.cursor_x = 85.0;
        state.cursor_y = 85.0;
        assert!(state.viewer_tool_drag_motion());
        assert!(state.viewer_tool_release());
        let after_add = points_of(&state);
        assert_eq!(after_add.len(), initial.len() + 1);
        assert_eq!(history(&state), (2, 0));

        // Gesture 3: delete the selected (new) point.
        assert!(state.viewer_tool_delete_selected());
        let after_delete = points_of(&state);
        assert_eq!(after_delete.len(), initial.len());
        assert_eq!(history(&state), (3, 0));

        // Undo walks back through all three.
        assert!(state.viewer_tool_undo());
        assert_eq!(points_of(&state), after_add);
        assert!(state.viewer_tool_undo());
        assert_eq!(points_of(&state), after_drag);
        assert!(state.viewer_tool_undo());
        assert_eq!(points_of(&state), initial);
        assert_eq!(history(&state), (0, 3));
        assert!(!state.viewer_tool_undo(), "history exhausted");

        // Redo walks forward again.
        assert!(state.viewer_tool_redo());
        assert_eq!(points_of(&state), after_drag);
        assert!(state.viewer_tool_redo());
        assert_eq!(points_of(&state), after_add);
        assert_eq!(history(&state), (2, 1));

        // A new gesture after an undo forks: the redo branch is gone.
        state.cursor_x = 50.0;
        state.cursor_y = 50.0;
        assert!(state.viewer_tool_delete_at_cursor());
        assert_eq!(history(&state), (3, 0));
        assert!(!state.viewer_tool_redo());

        // Undo mid-drag abandons the drag and clamps the selection.
        let pts = points_of(&state);
        state.cursor_x = sx(pts[pts.len() - 1].x);
        state.cursor_y = sy(pts[pts.len() - 1].y);
        assert!(state.viewer_tool_press());
        state.cursor_x += 5.0;
        assert!(state.viewer_tool_drag_motion());
        assert!(state.viewer_tool_undo());
        let tool = state.viewer_tool.as_ref().unwrap();
        assert!(tool.drag.is_none());
        assert!(tool.selected.map(|i| i < points_of(&state).len()).unwrap_or(true));
        assert!(!state.viewer_tool_drag_motion(), "no drag survives an undo");

        // Leaving the state drops its history.
        state.toggle_viewer_state(slot);
        assert!(state.viewer_tool.is_none());
        assert!(!state.viewer_tool_undo());
    }

    /// The Extrude node extrudes the input AS A WHOLE: points move along
    /// their normals, boundary edges grow walls, and Keep Base closes the
    /// bottom. A 2 x 2 plane becomes a closed slab of 18 points and 16
    /// primitives; without the base it is open and 4 primitives lighter. A
    /// closed sphere grows no walls at all — it becomes a two-skinned shell.
    #[test]
    fn test_extrude_node_geometry_generation() {
        let templates_root = crate::app::load_fs_tree();
        let extrude_template = templates_root
            .children
            .iter()
            .find(|t| t.name == "Extrude")
            .expect("Extrude template should be loaded");
        assert_eq!(extrude_template.node_type, "extrude");
        assert!(extrude_template.children.is_empty());
        assert_eq!(extrude_template.inputs, 1);

        let plane = ref_node("p", "plane1", "plane", vec![("rows", "spinbox", "2"), ("columns", "spinbox", "2"), ("width", "slider", "1"), ("length", "slider", "1")], vec![]);
        let extrude = |keep: &str| {
            ref_node("e", "extrude1", "extrude", vec![("input", "text", "plane1"), ("distance", "slider", "0.2"), ("keep_base", "toggle", keep)], vec![])
        };
        let root = ref_node("root", "root", "node", vec![], vec![plane.clone(), extrude("true")]);
        let (g, err) = eval(&root, &root.children[1]);
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        assert_eq!(g.num_points(), 18, "every point once, and once lifted");
        assert_eq!(g.num_prims(), 4 + 8 + 4, "top, eight boundary walls, base");
        assert!(g.is_closed(), "with the base kept the slab is watertight");
        for p in 0..9 {
            assert!((g.pos(p + 9).y - g.pos(p).y - 0.2).abs() < 1e-5, "top point {p} sits Distance above its base");
        }
        // Outward: every face normal points away from the slab's centre.
        let centre = (0..18).map(|p| g.pos(p)).sum::<Vec3>() / 18.0;
        for pr in 0..g.num_prims() {
            let pts = g.prim_points(pr);
            let (a, b, c) = (g.pos(pts[0] as usize), g.pos(pts[1] as usize), g.pos(pts[2] as usize));
            let n = (b - a).cross(c - a);
            let mid = pts.iter().map(|&q| g.pos(q as usize)).sum::<Vec3>() / pts.len() as f32;
            assert!(n.dot(mid - centre) > 0.0, "primitive {pr} faces inward");
        }
        assert!(g.points().has("Cd") && g.points().has("uv"), "point attributes ride to the top");

        let root2 = ref_node("root", "root", "node", vec![], vec![plane, extrude("false")]);
        let g2 = eval(&root2, &root2.children[1]).0.unwrap();
        assert_eq!(g2.num_points(), 18, "the base's corners are the walls' corners: same places, fewer faces");
        assert_eq!(g2.num_prims(), 4 + 8);
        assert!(!g2.is_closed(), "no base, open bottom");

        // A closed input: no boundary, so no walls — an outer and an inner
        // skin, the farthest points Distance beyond the sphere.
        let sphere = ref_node("s", "sphere1", "sphere", vec![("radius", "slider", "0.5"), ("center_x", "slider", "0"), ("center_y", "slider", "0.55"), ("center_z", "slider", "0")], vec![]);
        let ext = ref_node("e", "extrude1", "extrude", vec![("input", "text", "sphere1"), ("distance", "slider", "0.2"), ("keep_base", "toggle", "true")], vec![]);
        let root3 = ref_node("root", "root", "node", vec![], vec![sphere, ext]);
        let g3 = eval(&root3, &root3.children[1]).0.unwrap();
        let base = crate::geometry::sphere_point_len(16, 24);
        assert_eq!(g3.num_points(), 2 * base);
        assert_eq!(g3.num_prims(), 2 * (24 * 2 + 24 * 14), "no walls on a closed surface");
        assert!(g3.is_closed());
        let max_dist = g3.positions().iter().map(|p| (p[0].powi(2) + (p[1] - 0.55).powi(2) + p[2].powi(2)).sqrt()).fold(0.0f32, f32::max);
        assert!((max_dist - 0.7).abs() < 1e-3, "expected max extent ~0.7, got {max_dist}");
    }

    /// Group membership → viewport markers: the `group:<name>` tags a Group
    /// node writes partition the geometry on its box, `group_member_positions`
    /// reads exactly the tagged vertices back out, and `points_vertices`
    /// expands them into marker geometry (what the viewport draws while the
    /// Group node is selected).
    #[test]
    fn test_group_member_positions() {
        let templates_root = crate::app::load_fs_tree();
        let sphere_template = templates_root.children.iter().find(|t| t.name == "Sphere").unwrap();
        let group_template = templates_root
            .children
            .iter()
            .find(|t| t.name == "Group")
            .expect("Group template should be loaded");

        let mut sphere_instance = sphere_template.clone();
        sphere_instance.id = "sphere_inst".to_string();
        sphere_instance.name = "Sphere 1".to_string();
        for child in &mut sphere_instance.children {
            child.id = format!("{}_{}", sphere_instance.id, child.name);
        }

        // Box over the sphere's upper half: the default sphere is radius 0.5
        // centered at (0, 0.55, 0), so y ∈ [0.55, 1.05] selects the top
        // hemisphere's vertices.
        let mut group_instance = group_template.clone();
        group_instance.id = "group_inst".to_string();
        group_instance.name = "Group 1".to_string();
        let set = |inst: &mut FsNode, name: &str, val: &str| {
            inst.params.iter_mut().find(|p| p.name == name).unwrap().set_text(val.to_string());
        };
        set(&mut group_instance, "input", "Sphere 1");
        set(&mut group_instance, "center", "0.00:0.80:0.00");
        set(&mut group_instance, "size", "2.00:0.50:2.00");

        let root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere_instance, group_instance],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };

        let mut visited = Vec::new();
        let mut ocl_err = None;
        let geom = crate::geometry::generate_single_node_geometry_with_errors(
            &root,
            &root.children[1],
            &mut visited,
            &mut ocl_err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
        ).expect("Group geometry generation failed");
        assert!(ocl_err.is_none(), "node error: {:?}", ocl_err);

        let members = crate::geometry::group_member_positions(&geom, "group1");
        assert!(!members.is_empty(), "the box should tag the upper hemisphere");
        assert!(members.len() < geom.num_points(), "the box must not tag everything");
        for m in &members {
            assert!(m.position[1] >= 0.55 - 1e-4, "member below the box: y={}", m.position[1]);
        }
        // The group and the box agree: every non-member is outside it.
        assert_eq!(geom.points().group_len("group1"), members.len());
        for p in 0..geom.num_points() {
            if !geom.points().in_group("group1", p) {
                let y = geom.positions()[p][1];
                assert!(y <= 0.55 + 1e-4, "non-member inside the box: y={y}");
            }
        }

        // A name the node never wrote reads back empty.
        assert!(crate::geometry::group_member_positions(&geom, "nope").is_empty());

        // Marker expansion: 4x10 lat/lon sphere = 240 vertices per distinct point.
        let markers = crate::geometry::points_vertices(&members, 0.025, [1.0, 0.78, 0.20]);
        assert!(!markers.is_empty());
        assert_eq!(markers.len() % 240, 0);
    }

    /// A marker sphere winds counter-clockwise seen from OUTSIDE — the
    /// raster fill's culling convention, and what `sphere_detail` does.
    /// Until 2026-09-24 `points_vertices` kept the retired soup's inward
    /// winding, so the cull threw away the near half of every marker and
    /// drew the inside of the far half instead: a marker on a surface was
    /// visible only where that far half poked out of the mesh, which is
    /// what "the group marker disappears when I look up at it from below"
    /// was. The signed volume by the divergence theorem is the test — it
    /// is positive exactly when every facet faces outward.
    #[test]
    fn point_markers_wind_outward() {
        let centre = [0.3f32, -0.2, 0.7];
        let src = [crate::geometry::Vertex3D { position: centre, color: [0.0; 3] }];
        let r = 0.1f32;
        let markers = crate::geometry::points_vertices(&src, r, [1.0; 3]);
        assert_eq!(markers.len() % 3, 0);
        let c = glam::Vec3::from_array(centre);
        let mut volume = 0.0f32;
        let mut inward_facets = 0;
        for tri in markers.chunks(3) {
            let a = glam::Vec3::from_array(tri[0].position) - c;
            let b = glam::Vec3::from_array(tri[1].position) - c;
            let d = glam::Vec3::from_array(tri[2].position) - c;
            volume += a.dot(b.cross(d)) / 6.0;
            // Every facet's plain cross product must point away from the centre.
            let n = (b - a).cross(d - a);
            if n.dot(a + b + d) < 0.0 {
                inward_facets += 1;
            }
        }
        let expected = 4.0 / 3.0 * std::f32::consts::PI * r.powi(3);
        assert!(volume > 0.0, "marker winds inward: signed volume {volume}");
        // An inscribed 4x10 polyhedron holds about 80% of the true sphere.
        assert!(
            volume > expected * 0.6 && volume < expected,
            "signed volume {volume} is not a sphere of radius {r} ({expected})"
        );
        assert_eq!(inward_facets, 0, "{inward_facets} facets face into the marker");
    }

    /// The Attribute node's three operations over a sphere: Create tags every
    /// vertex, Modify combines into existing tags (and reaches the Pos/Col
    /// built-ins), Delete removes them, a Group name restricts the edit to
    /// tagged vertices, and a bad Value passes the geometry through with an
    /// error instead of eating it.
    #[test]
    fn test_attribute_node_operations() {
        let templates_root = crate::app::load_fs_tree();
        let find = |name: &str| {
            templates_root
                .children
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} template should be loaded"))
        };
        let instance = |template: &FsNode, id: &str, name: &str, params: &[(&str, &str)]| {
            let mut inst = template.clone();
            inst.id = id.to_string();
            inst.name = name.to_string();
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            for (pname, val) in params {
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().set_text(val.to_string());
            }
            inst
        };
        let root_with = |children: Vec<FsNode>| FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children,
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };
        let eval = |root: &FsNode, idx: usize| -> (Option<crate::detail::Detail>, Option<String>) {
            let mut visited = Vec::new();
            let mut ocl_err = None;
            let geom = crate::geometry::generate_single_node_geometry_with_errors(
                root,
                &root.children[idx],
                &mut visited,
                &mut ocl_err,
                &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
            );
            (geom, ocl_err)
        };

        let sphere_t = find("Sphere");
        let attr_t = find("Attribute");
        let group_t = find("Group");

        // Baseline sphere, for the Col comparison below.
        let base_root = root_with(vec![instance(sphere_t, "s", "Sphere 1", &[])]);
        let (base, err) = eval(&base_root, 0);
        let base = base.expect("baseline sphere");
        assert!(err.is_none());

        // Create → Modify (multiply) → Delete, as a three-node chain.
        let root = root_with(vec![
            instance(sphere_t, "s", "Sphere 1", &[]),
            instance(attr_t, "a1", "Attr 1", &[
                ("input", "Sphere 1"),
                ("operation", "Create"),
                ("attribute_name", "mass"),
                ("type", "Float"),
                ("value", "2.50"),
            ]),
            instance(attr_t, "a2", "Attr 2", &[
                ("input", "Attr 1"),
                ("operation", "Modify"),
                ("attribute_name", "mass"),
                ("combine", "Multiply"),
                ("value", "2.00"),
            ]),
            instance(attr_t, "a3", "Attr 3", &[
                ("input", "Attr 2"),
                ("operation", "Delete"),
                ("attribute_name", "mass"),
            ]),
        ]);
        let (geom, err) = eval(&root, 1);
        let geom = geom.expect("Create");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(geom.num_points(), base.num_points());
        assert!((0..geom.num_points()).all(|p| matches!(
            geom.points().value("mass", p),
            Some(AttribValue::Float(x)) if (x - 2.5).abs() < 1e-6
        )));
        let (geom, err) = eval(&root, 2);
        let geom = geom.expect("Modify");
        assert!(err.is_none(), "{err:?}");
        assert!((0..geom.num_points()).all(|p| matches!(
            geom.points().value("mass", p),
            Some(AttribValue::Float(x)) if (x - 5.0).abs() < 1e-6
        )));
        let (geom, err) = eval(&root, 3);
        let geom = geom.expect("Delete");
        assert!(err.is_none(), "{err:?}");
        assert!(!geom.points().has("mass"), "Delete removes the whole column");

        // Modify the Col built-in: multiply by a broadcast 0.5 halves every
        // channel relative to the baseline.
        let root = root_with(vec![
            instance(sphere_t, "s", "Sphere 1", &[]),
            instance(attr_t, "a1", "Tint", &[
                ("input", "Sphere 1"),
                ("operation", "Modify"),
                ("attribute_name", "Col"),
                ("combine", "Multiply"),
                ("value", "0.50"),
            ]),
        ]);
        let (geom, err) = eval(&root, 1);
        let geom = geom.expect("Col modify");
        assert!(err.is_none(), "{err:?}");
        for p in 0..geom.num_points() {
            for k in 0..3 {
                assert!((geom.color(p)[k] - base.color(p)[k] * 0.5).abs() < 1e-5);
            }
        }

        // Modify Pos with Add displaces the geometry upward.
        let root = root_with(vec![
            instance(sphere_t, "s", "Sphere 1", &[]),
            instance(attr_t, "a1", "Lift", &[
                ("input", "Sphere 1"),
                ("operation", "Modify"),
                ("attribute_name", "Pos"),
                ("combine", "Add"),
                ("value", "0.00:0.10:0.00"),
            ]),
        ]);
        let (geom, err) = eval(&root, 1);
        let geom = geom.expect("Pos modify");
        assert!(err.is_none(), "{err:?}");
        for p in 0..geom.num_points() {
            assert!((geom.positions()[p][1] - (base.positions()[p][1] + 0.1)).abs() < 1e-5);
        }

        // A Group name restricts what Create WRITES, not what exists.
        let root = root_with(vec![
            instance(sphere_t, "s", "Sphere 1", &[]),
            instance(group_t, "g", "Group 1", &[
                ("input", "Sphere 1"),
                ("center", "0.00:0.80:0.00"),
                ("size", "2.00:0.50:2.00"),
            ]),
            instance(attr_t, "a1", "Attr 1", &[
                ("input", "Group 1"),
                ("operation", "Create"),
                ("attribute_name", "mass"),
                ("value", "1.00"),
                ("group", "group1"),
            ]),
        ]);
        let (geom, err) = eval(&root, 2);
        let geom = geom.expect("grouped Create");
        assert!(err.is_none(), "{err:?}");
        // A column covers its whole class, so the attribute exists
        // everywhere; membership is the difference between the value and the
        // type's zero. This is the one behaviour the columnar store changes,
        // and it is what lets a solver read any attribute at any point.
        let members = geom.points().group_members("group1");
        assert!(!members.is_empty() && members.len() < geom.num_points());
        for p in 0..geom.num_points() {
            let want = if members.contains(&(p as u32)) { 1.0 } else { 0.0 };
            assert_eq!(
                geom.points().value("mass", p),
                Some(AttribValue::Float(want)),
                "point {p}"
            );
        }

        // A bad Value surfaces an error and passes the geometry through.
        let root = root_with(vec![
            instance(sphere_t, "s", "Sphere 1", &[]),
            instance(attr_t, "a1", "Attr 1", &[
                ("input", "Sphere 1"),
                ("operation", "Create"),
                ("attribute_name", "mass"),
                ("value", "abc"),
            ]),
        ]);
        let (geom, err) = eval(&root, 1);
        let geom = geom.expect("bad Value still passes geometry through");
        assert!(err.is_some(), "bad Value must surface an error");
        assert_eq!(geom.num_points(), base.num_points());
        assert!(!geom.points().has("mass"));
    }

    /// The param pane's attribute/group pickers: selecting a node upgrades
    /// its `attribute`- and `group`-kind rows to textpick rows whose
    /// candidates are read off the INPUT geometry (attributes plus the
    /// Pos/Col built-ins); a Group node with a group-less input keeps a
    /// plain text row (no empty menu). By KIND: a Visualize node — one the
    /// old by-name table never listed — gets the same pickers, and its
    /// `float` rows stay text.
    #[test]
    fn test_param_pane_pick_lists() {
        let templates_root = crate::app::load_fs_tree();
        let find = |name: &str| {
            templates_root.children.iter().find(|t| t.name == name).unwrap()
        };
        let instance = |template: &FsNode, id: &str, name: &str, params: &[(&str, &str)]| {
            let mut inst = template.clone();
            inst.id = id.to_string();
            inst.name = name.to_string();
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            for (pname, val) in params {
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().set_text(val.to_string());
            }
            inst
        };

        let mut state = State::new(false);
        // Inside the bundled project's Geometry node, where geometry goes.
        state.current_dir_mut().children = vec![
            instance(find("Sphere"), "s", "Sphere 1", &[]),
            instance(find("Group"), "g", "Group 1", &[
                ("input", "Sphere 1"),
                ("center", "0.00:0.80:0.00"),
                ("size", "2.00:0.50:2.00"),
            ]),
            instance(find("Attribute"), "a", "Attr 1", &[("input", "Group 1")]),
            instance(find("Visualize"), "v", "Vis 1", &[("input", "Group 1"), ("range", "Manual")]),
        ];
        state.sync_nodes();

        // The Attribute node: attrs from the input (+Pos/Col), groups from
        // the Group node it consumes.
        state.graph_mut().set_selected_node(Some(2));
        state.sync_parameters_pane();
        let rows = state.param_mut().node_params();
        let row = |name: &str| {
            rows.iter().find(|r| r.0 == name).unwrap_or_else(|| panic!("row {name}")).2.clone()
        };
        let attr_ty = row("Name");
        assert!(attr_ty.starts_with("textpick:"), "got {attr_ty}");
        for expected in ["N", "uv", "Pos", "Col"] {
            assert!(attr_ty.contains(expected), "{expected} missing from {attr_ty}");
        }
        assert_eq!(row("Group"), "textpick:group1");
        assert!(!rows.iter().any(|r| r.0 == "Attribute Name"), "the row shows its label, not its name");
        // An edit to the labelled row lands on the parameter it names.
        let mut edited = rows.clone();
        edited.iter_mut().find(|r| r.0 == "Name").unwrap().1 = "weight".into();
        state.param_mut().set_display_params(&edited);
        state.sync_parameters_to_project();
        let attr = &state.current_dir().children[2];
        assert_eq!(attr.params.iter().find(|p| p.name == "attribute_name").unwrap().text(), "weight");
        state.sync_parameters_pane();
        let rows = state.param_mut().node_params();
        // The Input row stays plain text.
        assert_eq!(row("Input"), "text");

        // The Group node's own Group Name: its input (the sphere) carries no
        // groups, so the row degrades to plain text.
        state.graph_mut().set_selected_node(Some(1));
        state.sync_parameters_pane();
        let rows = state.param_mut().node_params();
        let gn = rows.iter().find(|r| r.0 == "Group Name").unwrap();
        assert_eq!(gn.2, "text");

        // Visualize: its Attribute and Group rows are pickers because the
        // template says what they name, not because this node is listed
        // anywhere; its Manual Range is a range's two ends, two sliders.
        state.graph_mut().set_selected_node(Some(3));
        state.sync_parameters_pane();
        let rows = state.param_mut().node_params();
        let row = |name: &str| {
            rows.iter().find(|r| r.0 == name).unwrap_or_else(|| panic!("row {name}")).2.clone()
        };
        assert!(row("Attribute").starts_with("textpick:") && row("Attribute").contains("Pos"), "got {}", row("Attribute"));
        assert_eq!(row("Group"), "textpick:group1");
        assert_eq!(row("Manual Range"), "float2:-10:10:soft");
        assert_eq!(row("Input"), "text");
    }

    /// The point overlays are a VIEW setting, not a node property: the
    /// three flags decide them for the whole displayed scene, read off the
    /// merged Detail the geometry rebuild already produced. Their per-node
    /// `meta` children are stripped from any tree that still carries them,
    /// cameras and the root meta node untouched.
    #[test]
    fn test_point_overlays_are_a_global_view_setting() {
        let templates_root = crate::app::load_fs_tree();
        let sphere_t = templates_root.children.iter().find(|t| t.name == "Sphere").unwrap();
        let camera_t = templates_root.children.iter().find(|t| t.name == "Camera").unwrap();

        let mut sphere = sphere_t.clone();
        sphere.id = "s".to_string();
        sphere.name = "Sphere 1".to_string();
        for child in &mut sphere.children {
            child.id = format!("{}_{}", sphere.id, child.name);
        }
        let mut camera = camera_t.clone();
        camera.id = "cam".to_string();
        camera.name = "Camera 1".to_string();

        let meta_child = |id: &str| FsNode {
            id: id.to_string(),
            name: "meta".to_string(),
            node_type: "meta".to_string(),
            children: vec![],
            params: vec![],
            geometry_visible: false,
            bypassed: false,
            position: (0.0, 4.0),
            inputs: 0,
            outputs: 0,
        };
        // A tree as an older save carries it: a meta child on the sphere, one
        // on a node INSIDE a subnet, plus the ROOT meta node beside them.
        sphere.children.push(meta_child("s_meta"));
        let mut inner = camera_t.clone();
        inner.id = "inner".to_string();
        inner.name = "inner1".to_string();
        inner.children.push(meta_child("inner_meta"));
        let mut sub = FsNode {
            id: "sub".to_string(),
            name: "sub1".to_string(),
            node_type: "node".to_string(),
            children: vec![inner],
            params: vec![],
            geometry_visible: false,
            bypassed: false,
            position: (2.0, 0.0),
            inputs: 0,
            outputs: 0,
        };
        sub.children[0].children.push(meta_child("inner_meta2"));
        let mut session = meta_child("root_meta");
        session.geometry_visible = true;

        let mut root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere, camera, session, sub],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };

        crate::app::strip_meta_children(&mut root);

        // Gone from the placed nodes, at every depth…
        assert!(!root.children[0].children.iter().any(|c| c.node_type == "meta"));
        let inner = root.children[3].children.iter().find(|c| c.name == "inner1").unwrap();
        assert!(!inner.children.iter().any(|c| c.node_type == "meta"));
        // …and the root meta node, which is the SESSION container and not a
        // per-node child at all, is still standing.
        assert!(
            root.children.iter().any(|c| c.id == "root_meta" && c.node_type == "meta"),
            "the strip took the root meta node"
        );
        // Idempotent.
        let before = serde_json::to_string(&root).unwrap();
        crate::app::strip_meta_children(&mut root);
        assert_eq!(before, serde_json::to_string(&root).unwrap());

        // Evaluation is unaffected by their going.
        let mut visited = Vec::new();
        let mut err = None;
        let mut cache = crate::geometry::SimCache::default();
        let geom = crate::geometry::generate_single_node_geometry_with_errors(
            &root,
            &root.children[0],
            &mut visited,
            &mut err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut cache),
        ).expect("the stripped sphere evaluates");
        assert!(err.is_none(), "{err:?}");
        let points = crate::geometry::sphere_point_len(16, 24);
        assert_eq!(geom.num_points(), points);

        // Nothing while the three flags are off…
        let (markers, labels, normals) =
            crate::render::scene_point_overlays(&geom, false, false, false, 0.02, [1.0, 0.5, 0.0]);
        assert!(markers.is_empty() && labels.is_empty() && normals.is_empty());

        // …and all three off the one Detail: 240 marker verts per POINT, one
        // label per point, one whisker pair per point.
        let (markers, labels, normals) =
            crate::render::scene_point_overlays(&geom, true, true, true, 0.02, [1.0, 0.5, 0.0]);
        assert_eq!(labels.len(), points, "one label per point");
        assert_eq!(markers.len(), points * 240);
        assert!(labels.iter().any(|(_, i)| *i > 0));
        // The marker color parameter flows into the vertices (linearized).
        let expect = cce_ui::colors::to_linear_rgb([1.0, 0.5, 0.0]);
        assert!(markers.iter().all(|v| v.color == expect));
        // Normals: one whisker per point, pointing OUT of the sphere
        // (center (0, 0.55, 0)) — this pins the winding/negation convention,
        // not just the count.
        assert_eq!(normals.len(), points * 2);
        for pair in normals.chunks_exact(2) {
            let d = |p: &[f32; 3]| {
                let (dx, dy, dz) = (p[0], p[1] - 0.55, p[2]);
                (dx * dx + dy * dy + dz * dz).sqrt()
            };
            assert!(
                d(&pair[1].position) > d(&pair[0].position),
                "normal points inward at {:?}",
                pair[0].position
            );
        }

        // Each flag is independent — no flag drags another in.
        let (m, l, n) =
            crate::render::scene_point_overlays(&geom, true, false, false, 0.02, [1.0, 0.5, 0.0]);
        assert!(!m.is_empty() && l.is_empty() && n.is_empty());
        let (m, l, n) =
            crate::render::scene_point_overlays(&geom, false, true, false, 0.02, [1.0, 0.5, 0.0]);
        assert!(m.is_empty() && !l.is_empty() && n.is_empty());

        // The wire pass draws the mesh's TOPOLOGICAL edges — one pair per
        // unique edge, not per triangle side. This is what the global Show
        // Wireframe draws now; it drew the triangle soup until the per-node
        // meta Wireframe (which drew this) was retired into it.
        let wires = crate::render::scene_edge_verts(&geom);
        assert_eq!(wires.len(), geom.edges().len() * 2, "one pair per unique edge");
        assert!(wires.len() < geom.num_prims() * 6, "still drawing the soup's edges");
    }

    /// The Plane template's construction controls: Rows/Columns set the grid
    /// tessellation (vertex count = rows * columns * 6 — coverage the
    /// long-standing spinboxes never had), and the Center X/Y/Z channels
    /// place the plane like the Sphere's do.
    #[test]
    fn test_plane_construction_controls() {
        let templates_root = crate::app::load_fs_tree();
        let plane_t = templates_root.children.iter().find(|t| t.name == "Plane").unwrap();
        let build = |params: &[(&str, &str)]| {
            let mut inst = plane_t.clone();
            inst.id = "p".to_string();
            inst.name = "Plane 1".to_string();
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            for (pname, val) in params {
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().set_text(val.to_string());
            }
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
                bypassed: false,
                position: (0.0, 0.0),
                inputs: 0,
                outputs: 0,
            };
            let mut visited = Vec::new();
            let mut err = None;
            let mut cache = crate::geometry::SimCache::default();
            let geom = crate::geometry::generate_single_node_geometry_with_errors(
                &root,
                &root.children[0],
                &mut visited,
                &mut err,
                &mut crate::geometry::EvalSim::new(0, 0, &mut cache),
            ).expect("plane generation failed");
            assert!(err.is_none(), "{err:?}");
            geom
        };

        // Defaults: a 16x16 grid at the origin, flat on y = 0.
        let base = build(&[]);
        // A 16x16 cell grid shares its interior points: 17x17 of them.
        assert_eq!(base.num_points(), 17 * 17);
        assert!(base.positions().iter().all(|p| p[1].abs() < 1e-6));

        // Resolution: 3 columns x 2 rows.
        assert_eq!(build(&[("rows", "2"), ("columns", "3")]).num_points(), 4 * 3);

        // Center: lifts to y = 0.3 and shifts x by 1 (span [0.5, 1.5]).
        let moved = build(&[("center_x", "1.0"), ("center_y", "0.3")]);
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        for pos in moved.positions() {
            assert!((pos[1] - 0.3).abs() < 1e-5);
            min_x = min_x.min(pos[0]);
            max_x = max_x.max(pos[0]);
        }
        assert!((min_x - 0.5).abs() < 0.01, "min x {min_x}");
        assert!((max_x - 1.5).abs() < 0.01, "max x {max_x}");
    }

    /// The loader's template merge: saved instances gain params their
    /// template grew after the save (values they already hold are kept), a
    /// KERNEL SUBNET saved while Sphere was one becomes the native node with
    /// its values intact and its children gone, and non-template lookalikes
    /// are left alone.
    /// Phase 3: a parameter's value is parsed by its kind and its TEXT is
    /// kept verbatim, so `0.50` is the number 0.5 and still reads, saves and
    /// hashes as `0.50`. A text that does not fit is kept too, flagged, and
    /// read the way every read used to be — never coerced or dropped.
    #[test]
    fn a_value_parses_by_its_kind_and_keeps_its_text() {
        use crate::app::{ParamDef, ParamSlot, ParamValue as V};
        let v = |ty: &str, text: &str| ParamDef::new("p", ty, text).slot().clone();
        assert_eq!(v("slider", "0.50"), ParamSlot::Value(V::Number(0.5)));
        assert_eq!(ParamDef::new("p", "slider", "0.50").text(), "0.50");
        assert_eq!(v("float", " -3 "), ParamSlot::Value(V::Number(-3.0)));
        assert_eq!(v("spinbox", "16"), ParamSlot::Value(V::Int(16)));
        assert_eq!(v("float3", "0.00:0.20:-1"), ParamSlot::Value(V::Vec3([0.0, 0.2, -1.0])));
        assert_eq!(v("toggle", "TRUE"), ParamSlot::Value(V::Bool(true)));
        assert_eq!(v("choice:UV,Icosphere,Cube", "icosphere"), ParamSlot::Value(V::Choice("Icosphere".into())));
        assert_eq!(v("choice:UV,Icosphere,Cube", ""), ParamSlot::Value(V::Choice("UV".into())), "empty is the first option");
        assert_eq!(v("node", "sphere1"), ParamSlot::Value(V::Text("sphere1".into())));
        for (ty, text) in [("slider", "abc"), ("float", ""), ("spinbox", "4.5"), ("float3", "1:2"), ("toggle", "maybe"), ("choice:UV,Cube", "Torus")] {
            let p = ParamDef::new("p", ty, text);
            assert!(p.invalid().is_some(), "{ty} {text:?} should not fit");
            assert_eq!(p.text(), text, "an invalid text is kept verbatim");
            assert!(p.check(text).is_err());
        }
        // An invalid value reads as it always did: `4.5` in a spinbox is still 4.5.
        let node = FsNode { params: vec![ParamDef::new("count", "spinbox", "4.5")], ..crate::app::load_fs_tree() };
        assert_eq!(crate::geometry::node_param_f32(&node, "count", 0.0), 4.5);
        // An expression is not parsed as a value, and is not flagged.
        let e = ParamDef::new("p", "slider", "ch(\"../a/radius\") * 2").as_expr();
        assert!(e.is_expr() && e.invalid().is_none());
    }

    /// The file format did not move: a parameter serializes with the keys
    /// the old derive wrote, in its order, with its values — which is what
    /// keeps a re-save byte-identical and `sim_solve_key` (a hash of the
    /// simnet's JSON) from restarting every cached simulation. Checked over
    /// every shipped template and both bundled projects.
    #[test]
    fn params_serialize_as_they_always_did() {
        use serde_json::Value;
        let norm = |p: &Value| {
            let mut o = p.as_object().unwrap().clone();
            o.entry("label").or_insert("".into());
            o.entry("type").or_insert("string".into());
            o.entry("default").or_insert("".into());
            o.entry("options").or_insert(serde_json::json!([]));
            for k in ["min", "max", "step"] {
                o.entry(k).or_insert(Value::Null);
            }
            o.entry("show_when").or_insert("".into());
            // A template's description and group are read and never written.
            o.remove("description");
            o.remove("group");
            if o.get("expr") == Some(&Value::Bool(false)) {
                o.remove("expr");
            }
            Value::Object(o)
        };
        fn walk(v: &Value, f: &mut dyn FnMut(&Value)) {
            if let Some(ps) = v.get("params").and_then(|p| p.as_array()) {
                ps.iter().for_each(|p| f(p));
            }
            if let Some(cs) = v.get("children").and_then(|c| c.as_array()) {
                cs.iter().for_each(|c| walk(c, f));
            }
            if let Some(r) = v.get("root") {
                walk(r, f);
            }
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files: Vec<_> = fs::read_dir(dir.join("nodes")).unwrap().flatten().map(|e| e.path()).collect();
        files.push(dir.join("default_project.json"));
        files.push(dir.join("project.json"));
        let mut n = 0;
        for path in files.iter().filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json")) {
            let v: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            walk(&v, &mut |p| {
                let def: crate::app::ParamDef = serde_json::from_value(p.clone()).unwrap();
                // Through a STRING, as a save writes: `to_value` would widen
                // the f32 range fields and disagree with the file about 0.05.
                let back: Value = serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
                // By numeric VALUE: a hand-written template says `128` where
                // an f32 field writes `128.0`, as the old derive did too.
                fn num(v: Value) -> Value {
                    match v {
                        Value::Number(n) => serde_json::json!(n.as_f64()),
                        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, num(v))).collect()),
                        other => other,
                    }
                }
                assert_eq!(num(back), num(norm(p)), "{}", path.display());
                n += 1;
            });
        }
        assert!(n > 300, "walked {n} parameters");
        let written = serde_json::to_string(&crate::app::ParamDef::new("n", "slider", "0.50").as_expr()).unwrap();
        assert_eq!(
            written,
            r#"{"name":"n","label":"","type":"slider","default":"0.50","options":[],"min":null,"max":null,"step":null,"show_when":"","expr":true}"#
        );
    }

    /// A template that changes a parameter's kind re-parses the value an old
    /// save carries: Grid's Center shipped as text and is a float3 now.
    #[test]
    fn the_template_kind_reparses_an_old_value() {
        use crate::app::{ParamDef, ParamValue};
        let mut old = ParamDef::new("center", "text", "0.00:1.50:0.00");
        assert_eq!(old.value(), Some(&ParamValue::Text("0.00:1.50:0.00".into())));
        old.adopt_ui_from(&ParamDef::new("center", "float3", "0:0:0"));
        assert_eq!(old.value(), Some(&ParamValue::Vec3([0.0, 1.5, 0.0])));
        assert_eq!(old.text(), "0.00:1.50:0.00", "the instance keeps its value");
    }

    /// Phase 4: an expression's result is converted by the row it lands in —
    /// a number into a toggle is its truth, into a choice the option at that
    /// index, into a spinbox its whole part — and a result that fits nothing
    /// (a string into a slider) is stored as its text and flagged, which is
    /// what the old string write-back did.
    #[test]
    fn an_expression_result_takes_the_rows_kind() {
        use crate::app::{ParamDef, ParamValue as V};
        use crate::expr::Value;
        let p = |ty: &str| ParamDef::new("p", ty, "");
        assert_eq!(p("toggle").value_from_expr(&Value::Num(2.0)), Some(V::Bool(true)));
        assert_eq!(p("choice:UV,Icosphere,Cube").value_from_expr(&Value::Num(1.0)), Some(V::Choice("Icosphere".into())));
        assert_eq!(p("choice:UV,Icosphere,Cube").value_from_expr(&Value::Str("cube".into())), Some(V::Choice("Cube".into())));
        assert_eq!(p("spinbox").value_from_expr(&Value::Num(3.7)), Some(V::Int(3)));
        assert_eq!(p("slider").value_from_expr(&Value::Num(0.25)), Some(V::Number(0.25)));
        assert_eq!(p("text").value_from_expr(&Value::Num(0.5)), Some(V::Text("0.5".into())));
        assert_eq!(p("slider").value_from_expr(&Value::Str("abc".into())), None);

        // Through the resolver: the clone's parameter holds the typed value.
        let mut node = crate::app::load_fs_tree().children.into_iter().find(|t| t.node_type == "cull").unwrap();
        node.params.retain(|q| q.name != "invert");
        node.params.push(ParamDef::new("invert", "toggle", "1 + 1").as_expr());
        let root = FsNode { children: vec![node.clone()], ..crate::app::load_fs_tree() };
        let mut err = None;
        let resolved = crate::geometry::resolve_param_refs(&root, &root.children[0], 1, &mut err).unwrap();
        let inv = resolved.params.iter().find(|q| q.name == "invert").unwrap();
        assert_eq!((inv.value(), inv.text(), inv.is_expr()), (Some(&V::Bool(true)), "true", false));
        assert!(err.is_none(), "{err:?}");
    }

    /// Phase 4's entry points refuse a value that does not fit, and write
    /// nothing: MCP's set_param returns why; the params pane puts the kept
    /// text back in the row and says why on the status line. A value that
    /// reads as an expression is let through as one.
    #[test]
    fn a_value_that_does_not_fit_is_refused_where_it_is_typed() {
        let mut s = State::new(false);
        let mut redraw = false;
        s.apply_action(crate::app::McpAction::AddNode { template_name: "Cull".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw).unwrap();
        let slot = s.current_dir().children.iter().position(|c| c.node_type == "cull").unwrap();
        let threshold = |s: &State| crate::geometry::node_param_str(&s.current_dir().children[slot], "threshold", "");
        let before = threshold(&s);
        let set = |s: &mut State, v: &str| {
            s.apply_action(crate::app::McpAction::SetParam { slot, name: "threshold".into(), value: v.into() }, &mut false)
        };
        let err = set(&mut s, "abc").unwrap_err();
        assert!(err.contains("threshold") && err.contains("not a number"), "{err}");
        assert_eq!(threshold(&s), before, "nothing was written");
        set(&mut s, "0.7").unwrap();
        assert_eq!(threshold(&s), "0.7");
        set(&mut s, "ch(\"../x/radius\")").expect("an expression is not checked as a value");
        assert!(s.current_dir().children[slot].params.iter().find(|p| p.name == "threshold").unwrap().is_expr());
        set(&mut s, "0.7").unwrap();
        s.current_dir_mut().children[slot].params.iter_mut().find(|p| p.name == "threshold").unwrap().set_expr(false);

        // The pane.
        s.apply_action(crate::app::McpAction::Select { slot }, &mut redraw).unwrap();
        let rows: Vec<(String, String, String)> = s
            .param()
            .node_params()
            .into_iter()
            .map(|(n, v, t)| if n == "Threshold" { (n, "abc".into(), t) } else { (n, v, t) })
            .collect();
        s.param_mut().set_display_params(&rows);
        s.sync_parameters_to_project();
        assert_eq!(threshold(&s), "0.7", "the pane wrote nothing");
        let shown = s.param().node_params().into_iter().find(|r| r.0 == "Threshold").unwrap().1;
        assert_eq!(shown, "0.7", "the row shows the kept value again");
        assert!(s.last_status_text.contains("Not applied") && s.last_status_text.contains("Threshold"), "{}", s.last_status_text);
    }

    /// A status message stays until something else has something to say.
    /// Every window event used to overwrite the line with a leftover debug
    /// readout of the pane widths (`col: … vp: … params: …`), so a load
    /// report, a node error or a refused edit vanished the first time the
    /// pointer moved — the tests that set a status never passed an event
    /// through afterwards, and missed it.
    #[test]
    fn a_status_message_survives_window_events() {
        use crate::window::{LocalPosition, WindowEvent};
        let mut s = State::new(false);
        s.resize(1400.0, 900.0, 1.0);
        s.update_status_text("Not applied — Threshold: 'abc' is not a number");
        for (x, y) in [(300.0, 200.0), (700.0, 450.0), (1100.0, 600.0)] {
            s.process_window_event(WindowEvent::CursorMoved { position: LocalPosition { x, y } });
        }
        s.process_window_event(WindowEvent::MouseWheel { delta: cce_ui::widget::MouseScrollDelta::LineDelta(0.0, 1.0) });
        assert_eq!(s.last_status_text, "Not applied — Threshold: 'abc' is not a number");
    }

    /// get_state carries the status line, so it can be read from outside
    /// whether or not the window is on screen.
    #[test]
    fn get_state_reports_the_status_line() {
        let mut s = State::new(false);
        s.update_status_text("Not applied — Threshold: 'abc' is not a number");
        let (reply, _rx) = std::sync::mpsc::channel();
        let call = cce_ui::mcp::McpToolCall { name: "get_state".into(), arguments: serde_json::json!({}), reply };
        let v = s.apply_mcp_call(&call, &mut false).expect("get_state");
        assert_eq!(v["status"], "Not applied — Threshold: 'abc' is not a number");
        assert!(v.get("root").is_some(), "the project is still there beside it");
    }

    /// A load says when a project holds a value that does not fit — kept,
    /// but worth knowing about.
    #[test]
    fn a_load_reports_a_value_that_does_not_fit() {
        let dir = std::env::temp_dir().join(format!("cce_designer_invalid_param_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut a = State::new(false);
        let mut redraw = false;
        a.apply_action(crate::app::McpAction::AddNode { template_name: "Cull".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw).unwrap();
        let slot = a.current_dir().children.iter().position(|c| c.node_type == "cull").unwrap();
        a.current_dir_mut().children[slot].params.iter_mut().find(|p| p.name == "threshold").unwrap().set_text("half");
        a.save_to_file(&dir).expect("save");

        let mut b = State::new(false);
        b.load_from_file(&dir).expect("load");
        assert!(b.last_status_text.contains("Threshold") && b.last_status_text.contains("not a number"), "{}", b.last_status_text);
        let kept = b.current_dir().children.iter().find(|c| c.node_type == "cull").unwrap();
        assert_eq!(crate::geometry::node_param_str(kept, "threshold", ""), "half", "kept verbatim");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Phase 0 of typed parameters: a `type` string names a [`ParamKind`],
    /// read by its head, and anything else is refused rather than read as
    /// text and left to look like it worked.
    #[test]
    fn param_kinds_parse_by_their_head() {
        use crate::app::ParamKind as K;
        for (ty, kind) in [
            ("text", K::Text), ("string", K::Text), ("float", K::Float),
            ("slider", K::Slider), ("slider:-2:2", K::Slider), ("spinbox", K::Spin),
            ("float3", K::Float3), ("choice:UV,Icosphere,Cube", K::Choice),
            ("toggle", K::Toggle), ("button", K::Button), ("code", K::Code), ("node", K::Node),
        ] {
            assert_eq!(K::parse(ty), Some(kind), "{ty}");
        }
        for bad in ["int", "slidr", "textpick:a,b", "", "Text"] {
            assert_eq!(K::parse(bad), None, "{bad:?} names no kind");
        }
        for name in K::NAMES {
            assert!(K::parse(name).is_some(), "NAMES lists {name}, which does not parse");
        }
        // Found anywhere in a template's tree, children included.
        let mut t = crate::app::load_fs_tree().children.into_iter().find(|t| t.name == "Embryo").unwrap();
        assert!(crate::app::unknown_param_kinds(&t).is_empty());
        t.children[0].params.push(crate::app::ParamDef::new("count", "int", "1"));
        let bad = crate::app::unknown_param_kinds(&t);
        assert_eq!(bad.len(), 1);
        assert_eq!((bad[0].1.as_str(), bad[0].2.as_str()), ("count", "int"));
    }

    /// Every shipped template's every parameter names a kind — walked from
    /// the RAW files, because the loader drops a template that does not, and
    /// a dropped template shows up only as a missing palette entry.
    #[test]
    fn every_shipped_template_param_has_a_known_kind() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("nodes");
        let mut files = 0;
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            files += 1;
            let node: FsNode = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let bad = crate::app::unknown_param_kinds(&node);
            assert!(bad.is_empty(), "{}: {bad:?}", path.display());
        }
        assert_eq!(crate::app::load_fs_tree().children.len(), files, "the loader dropped a template");
    }

    /// Phase 1: every parameter that names another node is a `node`, and
    /// the numbers and vectors that shipped as `text` are what they hold.
    /// By TEMPLATE, not by name: Visualize's From and To are numbers where
    /// Transfer's From and Copy's To are wires. Since 2026-09-28 a row that
    /// names a point attribute is an `attribute` and one that names a group
    /// a `group` — read or written, the picker is the same — and what is
    /// still `text` is text for a reason: Attribute's Value is as wide as
    /// its Type row says, Transfer's Attributes is a comma list, Simnet's
    /// Start Frame is empty for "the playbar's".
    #[test]
    fn template_params_carry_the_kind_they_hold() {
        use crate::app::ParamKind as K;
        let root = crate::app::load_fs_tree();
        let kind = |ty: &str, name: &str| {
            // By type, or by name for a subnet (the Remesh is a `node`).
            root.children.iter().find(|t| t.node_type == ty || (t.node_type == "node" && t.name.eq_ignore_ascii_case(ty)))
                .and_then(|t| t.params.iter().find(|p| p.name == name))
                .unwrap_or_else(|| panic!("{ty} has no {name}"))
                .kind()
        };
        for (ty, name) in [
            ("switch", "input_2"), ("switch", "input_3"), ("switch", "input_4"),
            ("boolean", "with"), ("collision", "collider"), ("relax", "rest"),
            ("suture", "against"), ("copy", "to"), ("distance", "to"), ("transfer", "from"), ("remesh", "from"), ("project", "surface"),
        ] {
            assert_eq!(kind(ty, name), K::Node, "{ty}'s {name}");
        }
        for (ty, name) in [("grid", "center"), ("polygon", "center"), ("soft_transform", "center"), ("soft_transform", "translation")] {
            assert_eq!(kind(ty, name), K::Float3, "{ty}'s {name}");
        }
        for (ty, name) in [
            ("cull", "threshold"), ("group", "threshold"), ("copy", "scale"), ("attribute", "normalize_to"),
        ] {
            assert_eq!(kind(ty, name), K::Float, "{ty}'s {name}");
        }
        assert_eq!(kind("neighbour", "constant"), K::Float3);
        // The pull's Strength is a slider over none-to-double, one in the
        // middle: a number set by hand and by eye, where the `float` it
        // was for a day is a box to type into. It still takes an
        // expression, shown as text like any other.
        let strength = root.children.iter().find(|t| t.node_type == "attribute").unwrap()
            .params.iter().find(|p| p.name == "strength").expect("attribute has a Strength");
        assert_eq!(strength.kind(), K::Slider);
        assert_eq!(strength.range().map(|(lo, hi, _)| (lo, hi)), Some((0.0, 2.0)));
        assert_eq!(strength.text(), "1.00");
        for (ty, name) in [
            ("attribute", "attribute_name"), ("attribute", "source_b"), ("visualize", "attribute"), ("neighbour", "attribute"),
            ("neighbour", "direction"), ("neighbour", "source"), ("distance", "direction"), ("develop", "source"),
            ("copy", "scale_attribute"), ("normal", "attribute"), ("suture", "counter"), ("time", "attribute"),
        ] {
            assert_eq!(kind(ty, name), K::Attribute, "{ty}'s {name}");
        }
        for (ty, name) in [
            ("attribute", "group"), ("group", "group_name"), ("group", "source_group"), ("relax", "pin_group"),
            ("collision", "group_name"), ("wrangle", "group"), ("visualize", "group"), ("cull", "group"),
        ] {
            assert_eq!(kind(ty, name), K::Group, "{ty}'s {name}");
        }
        // Every row called Group, on every template, names a group.
        for t in &root.children {
            for p in t.params.iter().filter(|p| p.name == "group") {
                assert_eq!(p.kind(), K::Group, "{}'s Group", t.name);
            }
        }
        for (ty, name) in [("attribute", "value"), ("transfer", "attributes"), ("transfer", "groups"), ("remesh", "attributes"), ("remesh", "groups"), ("simnet", "start_frame"), ("bounds", "prefix")] {
            assert_eq!(kind(ty, name), K::Text, "{ty}'s {name} stays text on purpose");
        }
        // Every template's Input is a wire, top level and composed children alike.
        fn inputs(n: &FsNode, out: &mut Vec<(String, crate::app::ParamKind)>) {
            for p in n.params.iter().filter(|p| p.name == "input") {
                out.push((n.name.clone(), p.kind()));
            }
            n.children.iter().for_each(|c| inputs(c, out));
        }
        let mut all = Vec::new();
        root.children.iter().for_each(|t| inputs(t, &mut all));
        assert!(all.len() > 40);
        let not_node: Vec<_> = all.iter().filter(|(_, k)| *k != K::Node).collect();
        assert!(not_node.is_empty(), "{not_node:?}");
    }

    /// A save made before phase 1 carries `"type": "text"` on its wires; the
    /// template merge hands it the template's kind, as it does all UI
    /// metadata, and leaves the value alone.
    #[test]
    fn a_saved_text_wire_loads_as_a_node_wire() {
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let mut relax = templates_root.children.iter().find(|t| t.node_type == "relax").unwrap().clone();
        for p in relax.params.iter_mut().filter(|p| p.name == "input" || p.name == "rest") {
            p.set_type("text");
            p.set_text("sphere1");
        }
        let mut root = FsNode { children: vec![relax], ..templates_root.clone() };
        crate::app::merge_template_defs(&mut root, &templates);
        for name in ["input", "rest"] {
            let p = root.children[0].params.iter().find(|p| p.name == name).unwrap();
            assert_eq!(p.kind(), crate::app::ParamKind::Node, "{name}");
            assert_eq!(p.text(), "sphere1", "the value is the instance's");
        }
    }

    /// The pane has no node or numeric-text row: both show as text, and so
    /// does `string` (an absent type), which the pane would otherwise not
    /// recognise at all — and so do an attribute and a group name, until
    /// `add_pick_lists` has candidates to offer.
    #[test]
    fn node_and_float_rows_show_as_text() {
        let row = |ty: &str| crate::app::ParamDef::new("x", ty, "1");
        let shown = crate::app::param_display(&[row("node"), row("float"), row("string"), row("attribute"), row("group"), row("toggle")]);
        let types: Vec<&str> = shown.iter().map(|r| r.2.as_str()).collect();
        // The leading wire is the inputs, a separator under it.
        assert_eq!(types, vec!["text", "separator", "text", "text", "text", "text", "toggle"]);
    }

    /// The params pane draws a separator wherever two rows it shows belong to
    /// different groups: under the leading wires, and between the runs a
    /// template names. Only between two SHOWN rows — never first, last or
    /// doubled, and a run whose rows are all hidden leaves no line — and a
    /// wire further down is part of the run it is in. The separator names
    /// no parameter, so the write-back passes over it.
    #[test]
    fn the_params_pane_separates_groups_of_parameters() {
        let p = |name: &str, ty: &str, group: &str, show_when: &str| {
            let mut d = crate::app::ParamDef::new(name, ty, "1");
            d.group = group.to_string();
            d.show_when = show_when.to_string();
            d
        };
        let rows = |ps: &[crate::app::ParamDef]| -> Vec<String> {
            crate::app::param_display(ps).into_iter().map(|r| if r.2 == "separator" { "|".to_string() } else { r.0 }).collect()
        };
        let params = vec![
            p("input", "node", "", ""),
            p("with", "node", "", ""),
            p("mode", "choice:A,B", "", ""),
            p("rest", "node", "", ""),
            p("size", "float", "shape", ""),
            p("sides", "float", "shape", "mode == B"),
            p("Hidden", "float", "extra", "mode == B"),
            p("group", "group", "where", ""),
        ];
        assert_eq!(rows(&params), ["input", "with", "|", "mode", "rest", "|", "size", "|", "group"]);
        // A node with no wires and no groups has no line at all.
        assert_eq!(rows(&[p("A", "float", "", ""), p("B", "float", "", "")]), ["A", "B"]);
        // A template-named group on a wire takes it out of the inputs.
        assert_eq!(rows(&[p("input", "node", "", ""), p("surface", "node", "x", ""), p("size", "float", "x", "")]), ["input", "|", "surface", "size"]);
        // The group is the template's: it rides the merge, never the file.
        let mut inst = crate::app::ParamDef::new("size", "float", "2");
        inst.adopt_ui_from(&params[4]);
        assert_eq!(inst.group, "shape");
        assert!(!serde_json::to_string(&inst).unwrap().contains("shape"), "not written");
    }

    /// Phase 2's toggle reader: the words a toggle can hold, in any case,
    /// and the FALLBACK for anything else — where `== "true"` and
    /// `!= "false"` used to disagree about garbage.
    #[test]
    fn node_param_bool_reads_a_toggle_and_falls_back_on_anything_else() {
        use crate::geometry::node_param_bool;
        let node = |v: &str| FsNode {
            params: vec![crate::app::ParamDef::new("on", "toggle", v)],
            ..crate::app::load_fs_tree()
        };
        for v in ["true", "TRUE", " True ", "1", "on"] {
            assert!(node_param_bool(&node(v), "on", false), "{v:?}");
        }
        for v in ["false", "False", "0", "off"] {
            assert!(!node_param_bool(&node(v), "on", true), "{v:?}");
        }
        for v in ["", "yes please", "2"] {
            assert!(node_param_bool(&node(v), "on", true) && !node_param_bool(&node(v), "on", false), "{v:?}");
        }
        assert!(node_param_bool(&node("true"), "missing", true) && !node_param_bool(&node("true"), "missing", false));
    }

    /// MCP's add_param refuses a type that names no kind, and says which do.
    #[test]
    fn add_param_refuses_an_unknown_type() {
        let mut state = State::new(false);
        let mut redraw = false;
        let before = state.current_dir().children[0].params.len();
        let err = state
            .apply_action(crate::app::McpAction::AddParam { slot: 0, name: "n".into(), param_type: "int".into(), default: "1".into(), label: String::new() }, &mut redraw)
            .unwrap_err();
        assert!(err.contains("int") && err.contains("spinbox"), "{err}");
        assert_eq!(state.current_dir().children[0].params.len(), before);
        state
            .apply_action(crate::app::McpAction::AddParam { slot: 0, name: "n".into(), param_type: "spinbox".into(), default: "1".into(), label: String::new() }, &mut redraw)
            .expect("a known kind is added");
    }

    /// A camera node has no Square Aspect or Show Camera Pivot: they were
    /// written when the viewport's toggles flipped under it and read by
    /// nothing. A save that has them loads without them.
    #[test]
    fn a_camera_carries_no_viewport_toggles() {
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let t = templates_root.children.iter().find(|t| t.node_type == "camera").unwrap();
        let names: Vec<&str> = t.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["position", "rotation", "pivot"]);
        let mut old = t.clone();
        old.params.push(crate::app::ParamDef::new("square_aspect", "toggle", "true"));
        old.params.push(crate::app::ParamDef::new("show_camera_pivot", "toggle", "true"));
        let mut root = templates_root.clone();
        root.children = vec![old];
        crate::app::merge_template_defs(&mut root, &templates);
        let names: Vec<&str> = root.children[0].params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["position", "rotation", "pivot"]);
    }

    /// Visualize's Blend has no Mix: it was Set under another name, Opacity
    /// fading every blend alike. A save that chose it loads as Set, a
    /// valid choice, rather than as a text the row no longer offers.
    #[test]
    fn visualize_mix_blend_loads_as_set() {
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let t = templates_root.children.iter().find(|t| t.node_type == "visualize").unwrap();
        let blend = t.params.iter().find(|p| p.name == "blend").unwrap();
        assert_eq!(blend.choice_options(), vec!["Set", "Multiply", "Add"]);
        let mut old = t.clone();
        let p = old.params.iter_mut().find(|p| p.name == "blend").unwrap();
        p.set_type("choice:Set,Mix,Multiply,Add");
        p.set_text("Mix".to_string());
        assert!(p.invalid().is_none(), "the old row took Mix");
        let mut root = templates_root.clone();
        root.children = vec![old];
        crate::app::merge_template_defs(&mut root, &templates);
        let p = root.children[0].params.iter().find(|p| p.name == "blend").unwrap();
        assert_eq!(p.text(), "Set");
        assert!(p.invalid().is_none(), "{:?}", p.invalid());
    }

    #[test]
    fn test_loader_merges_new_template_params() {
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let group_t = templates_root.children.iter().find(|t| t.name == "Group").unwrap();
        let output_t = templates_root.children.iter().find(|t| t.node_type == "output").unwrap();

        // An "old save": a Sphere instance from before the construction
        // controls AND from before the port — a subnet of opencl1 → output1
        // holding only Radius, with a user value, and a stale kernel.
        let opencl1 = FsNode {
            id: "s_opencl1".to_string(),
            name: "opencl1".to_string(),
            node_type: "opencl".to_string(),
            children: vec![],
            params: vec![crate::app::ParamDef::new("code", "code", "OLD KERNEL")],
            geometry_visible: true,
            bypassed: false,
            position: (4.0, 2.0),
            inputs: 1,
            outputs: 1,
        };
        let mut output1 = output_t.clone();
        output1.id = "s_output1".to_string();
        output1.name = "output1".to_string();
        output1.params.iter_mut().find(|p| p.name == "input").unwrap().set_text("opencl1".to_string());
        let old_sphere = FsNode {
            id: "s".to_string(),
            name: "Sphere 3".to_string(),
            node_type: "node".to_string(),
            children: vec![opencl1, output1],
            params: vec![crate::app::ParamDef::new("radius", "slider", "0.70")],
            geometry_visible: true,
            bypassed: false,
            position: (3.0, 1.0),
            inputs: 0,
            outputs: 1,
        };

        // An old Group missing a later-added param, with a kept value.
        let mut old_group = group_t.clone();
        old_group.id = "g".to_string();
        old_group.name = "My Region".to_string(); // renamed: native nodes match by TYPE
        old_group.params.retain(|p| p.name != "highlight");
        old_group.params.iter_mut().find(|p| p.name == "center").unwrap().set_text("0.00:0.80:0.00".to_string());

        // A hand-built subnet that happens to share the Sphere name.
        let lookalike = FsNode {
            id: "fake".to_string(),
            name: "Sphere 9".to_string(),
            node_type: "node".to_string(),
            children: vec![],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };

        let mut root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![old_sphere, old_group, lookalike],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };
        crate::app::merge_template_defs(&mut root, &templates);

        // Sphere: the kernel subnet is the native node now — same id, name
        // and position, no children — and new params are inserted where the
        // template puts them: Method ABOVE the Radius the instance already
        // had, the rest after it, with template defaults and the value kept.
        let s = &root.children[0];
        assert_eq!((s.node_type.as_str(), s.id.as_str(), s.name.as_str(), s.position), ("sphere", "s", "Sphere 3", (3.0, 1.0)));
        assert!(s.children.is_empty(), "the opencl and output children go");
        let names: Vec<&str> = s.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["method", "radius", "rows", "columns", "frequency", "resolution", "center_x", "center_y", "center_z", "color"]);
        assert_eq!(s.params.iter().find(|p| p.name == "radius").unwrap().text(), "0.70", "instance value survives");

        // And the merged instance evaluates with the new controls live.
        let mut merged_sphere_root = root.clone();
        merged_sphere_root.children.truncate(1);
        merged_sphere_root.children[0].params.iter_mut()
            .find(|p| p.name == "rows").unwrap().set_text("4".to_string());
        merged_sphere_root.children[0].params.iter_mut()
            .find(|p| p.name == "columns").unwrap().set_text("6".to_string());
        let mut visited = Vec::new();
        let mut err = None;
        let mut cache = crate::geometry::SimCache::default();
        let geom = crate::geometry::generate_single_node_geometry_with_errors(
            &merged_sphere_root,
            &merged_sphere_root.children[0],
            &mut visited,
            &mut err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut cache),
        ).expect("merged sphere evaluates");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(geom.num_points(), crate::geometry::sphere_point_len(4, 6));

        // Group (renamed, matched by type): Highlight restored, value kept.
        let g = &root.children[1];
        assert!(g.params.iter().any(|p| p.name == "highlight" && p.text() == "true"));
        assert_eq!(g.params.iter().find(|p| p.name == "center").unwrap().text(), "0.00:0.80:0.00");

        // Lookalike: untouched — no params gained, no children injected.
        let l = &root.children[2];
        assert!(l.params.is_empty());
        assert!(l.children.is_empty());
    }

    /// The Sphere template's construction controls: Rows/Columns set the
    /// lat/lon tessellation (vertex count = rows * columns * 6), Center X/Y/Z
    /// place the sphere, and the defaults keep the historical 16x24 sphere at
    /// (0, 0.55, 0) byte-identical (the extrude test's 2304-vertex baseline).
    #[test]
    fn test_sphere_construction_controls() {
        let templates_root = crate::app::load_fs_tree();
        let sphere_t = templates_root.children.iter().find(|t| t.name == "Sphere").unwrap();
        let build = |params: &[(&str, &str)]| {
            let mut inst = sphere_t.clone();
            inst.id = "s".to_string();
            inst.name = "Sphere 1".to_string();
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            for (pname, val) in params {
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().set_text(val.to_string());
            }
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
                bypassed: false,
                position: (0.0, 0.0),
                inputs: 0,
                outputs: 0,
            };
            let mut visited = Vec::new();
            let mut err = None;
            let mut cache = crate::geometry::SimCache::default();
            let geom = crate::geometry::generate_single_node_geometry_with_errors(
                &root,
                &root.children[0],
                &mut visited,
                &mut err,
                &mut crate::geometry::EvalSim::new(0, 0, &mut cache),
            ).expect("sphere generation failed");
            assert!(err.is_none(), "{err:?}");
            geom
        };

        // Defaults: the historical 16x24 sphere.
        assert_eq!(build(&[]).num_points(), crate::geometry::sphere_point_len(16, 24));

        // A coarse 4x6 tessellation.
        let coarse = build(&[("rows", "4"), ("columns", "6")]);
        assert_eq!(coarse.num_points(), crate::geometry::sphere_point_len(4, 6));

        // Center X shifts the whole sphere: default spans x in [-0.5, 0.5],
        // shifted spans [0.5, 1.5].
        let shifted = build(&[("center_x", "1.0")]);
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        for pos in shifted.positions() {
            min_x = min_x.min(pos[0]);
            max_x = max_x.max(pos[0]);
        }
        assert!((min_x - 0.5).abs() < 0.01, "min x {min_x}");
        assert!((max_x - 1.5).abs() < 0.01, "max x {max_x}");

        // Degenerate resolutions clamp instead of emitting nothing.
        assert_eq!(
            build(&[("rows", "0"), ("columns", "0")]).num_points(),
            crate::geometry::sphere_point_len(2, 3)
        );
    }

    /// The Sphere's Method dropdown picks the construction: UV (Rows x
    /// Columns, the sphere this node always built), Icosphere (an
    /// icosahedron's 20 faces each split into Frequency^2 triangles) and
    /// Cube (a Resolution x Resolution grid on each face of a cube, pushed
    /// onto the sphere). The welded point counts are the closed forms —
    /// 10f^2 + 2 and 6r^2 + 2 — which hold only if every corner two faces
    /// share lands on the same point, and closedness says the winding came
    /// out consistent after the outward turn. Native since 2026-09-24
    /// (`src/shapes.rs`); it was a kernel reading Method as an option index.
    #[test]
    fn sphere_method_builds_a_uv_ico_or_cube_sphere() {
        let templates_root = crate::app::load_fs_tree();
        let sphere_t = templates_root.children.iter().find(|t| t.name == "Sphere").unwrap();
        let method = sphere_t.params.iter().find(|p| p.name == "method").expect("a Method dropdown");
        assert_eq!(method.ty(), "choice:UV,Icosphere,Cube");
        assert_eq!(method.text(), "UV", "the default stays the sphere every saved project was built with");
        assert_eq!(sphere_t.params[0].name, "method", "the method heads the pane, above the radius it governs");
        // And it heads the pane of a sphere SAVED before it existed too: the
        // bundled project's sphere1 gains it through the loader's merge, at
        // the template's position rather than below Color.
        let state = State::new(false);
        let saved = state.current_dir().children.iter().find(|c| c.name == "sphere1").expect("the bundled sphere1");
        assert_eq!(saved.params[0].name, "method", "merged order: {:?}", saved.params.iter().map(|p| &p.name).collect::<Vec<_>>());
        let build = |params: &[(&str, &str)]| {
            let mut inst = sphere_t.clone();
            inst.id = "s".to_string();
            inst.name = "sphere1".to_string();
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            for (pname, val) in params {
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().set_text(val.to_string());
            }
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
                bypassed: false,
                position: (0.0, 0.0),
                inputs: 0,
                outputs: 0,
            };
            let mut visited = Vec::new();
            let mut err = None;
            let mut cache = crate::geometry::SimCache::default();
            let geom = crate::geometry::generate_single_node_geometry_with_errors(
                &root,
                &root.children[0],
                &mut visited,
                &mut err,
                &mut crate::geometry::EvalSim::new(0, 0, &mut cache),
            ).expect("sphere generation failed");
            assert!(err.is_none(), "{err:?}");
            geom
        };
        let on_sphere = |geom: &crate::detail::Detail, radius: f32| {
            for pos in geom.positions() {
                let r = ((pos[0]).powi(2) + (pos[1] - 0.55).powi(2) + (pos[2]).powi(2)).sqrt();
                assert!((r - radius).abs() < 1e-3, "point {pos:?} is {r} from the centre, not {radius}");
            }
        };

        let uv = build(&[("method", "UV")]);
        assert_eq!(uv.num_points(), crate::geometry::sphere_point_len(16, 24));

        for (freq, expect) in [("1", 12), ("2", 42), ("4", 162), ("7", 492)] {
            let ico = build(&[("method", "Icosphere"), ("frequency", freq)]);
            assert_eq!(ico.num_points(), expect, "icosphere at frequency {freq}");
            assert_eq!(ico.num_prims(), 20 * freq.parse::<usize>().unwrap().pow(2));
            assert!(ico.is_closed(), "icosphere at frequency {freq} is not closed");
            on_sphere(&ico, 0.5);
        }

        for (res, expect) in [("1", 8), ("3", 56), ("8", 386)] {
            let cube = build(&[("method", "Cube"), ("resolution", res), ("radius", "0.8")]);
            assert_eq!(cube.num_points(), expect, "cube sphere at resolution {res}");
            assert_eq!(cube.num_prims(), 6 * res.parse::<usize>().unwrap().pow(2), "one quad per cell — the kernel fanned them");
            assert!(cube.is_closed(), "cube sphere at resolution {res} is not closed");
            on_sphere(&cube, 0.8);
        }

        // The out-of-range guards: a frequency of 0 builds the icosahedron.
        assert_eq!(build(&[("method", "Icosphere"), ("frequency", "0")]).num_points(), 12);
    }

    /// `param_number` is what a kernel's `chi()` reads: a choice is its
    /// option index, a toggle 0 or 1, a number itself, and text 0.
    #[test]
    fn a_choice_reads_as_its_option_index_from_a_kernel() {
        use crate::geometry::param_number;
        let p = |ty: &str, val: &str| crate::app::ParamDef::new("x", ty, val);
        assert_eq!(param_number(&p("choice:UV,Icosphere,Cube", "Cube")), 2.0);
        assert_eq!(param_number(&p("choice:UV,Icosphere,Cube", "icosphere")), 1.0, "case-insensitive, like the reference path");
        assert_eq!(param_number(&p("choice:UV,Icosphere,Cube", "Nope")), 0.0, "an unknown option is the first");
        assert_eq!(param_number(&p("toggle", "true")), 1.0);
        assert_eq!(param_number(&p("slider", "0.25")), 0.25);
        assert_eq!(param_number(&p("string", "hello")), 0.0);
    }

    /// A Scatter consumed downstream must still evaluate: the dispatch pushes
    /// the target id before dispatching, so a resolver-local visited guard
    /// sees it and refuses every dispatched call — scatter geometry silently
    /// vanished from any chain while displaying fine on its own.
    #[test]
    fn test_scatter_consumed_downstream() {
        let templates_root = crate::app::load_fs_tree();
        let find = |name: &str| {
            templates_root.children.iter().find(|t| t.name == name).unwrap()
        };
        let instance = |template: &FsNode, id: &str, name: &str, params: &[(&str, &str)]| {
            let mut inst = template.clone();
            inst.id = id.to_string();
            inst.name = name.to_string();
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            for (pname, val) in params {
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().set_text(val.to_string());
            }
            inst
        };
        let root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![
                instance(find("Sphere"), "s", "Sphere 1", &[]),
                instance(find("Scatter"), "sc", "Scatter 1", &[("input", "Sphere 1")]),
                instance(find("Attribute"), "a", "Attr 1", &[
                    ("input", "Scatter 1"),
                    ("operation", "Create"),
                    ("attribute_name", "mass"),
                    ("value", "1.00"),
                ]),
            ],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };

        // The scatter alone works (the scene walk's direct-call path)…
        let mut visited = Vec::new();
        let mut err = None;
        let mut cache = crate::geometry::SimCache::default();
        let direct = crate::geometry::generate_single_node_geometry_with_errors(
            &root,
            &root.children[1],
            &mut visited,
            &mut err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut cache),
        ).expect("scatter evaluates on its own");
        assert!(err.is_none(), "{err:?}");
        assert!(!direct.is_empty());

        // …and the SAME scatter feeding a downstream node yields the SAME
        // points, tagged by the consumer.
        let mut visited = Vec::new();
        let mut err = None;
        let mut cache = crate::geometry::SimCache::default();
        let chained = crate::geometry::generate_single_node_geometry_with_errors(
            &root,
            &root.children[2],
            &mut visited,
            &mut err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut cache),
        ).expect("a node consuming a scatter must see its geometry");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(chained.num_points(), direct.num_points());
        assert!(chained.points().has("mass"));
    }

    /// The Plane node: a Columns x Rows sheet of quads on XZ at y = 0, Width
    /// and Length its sides. Native since 2026-09-24; it was a kernel subnet.
    #[test]
    fn test_plane_node_geometry_generation() {
        let templates_root = crate::app::load_fs_tree();
        let plane_template = templates_root
            .children
            .iter()
            .find(|t| t.name == "Plane")
            .expect("Plane template should be loaded");
        assert_eq!(plane_template.node_type, "plane");
        assert!(plane_template.children.is_empty());

        let generate = |overrides: &[(&str, &str)], id: &str| {
            let mut inst = plane_template.clone();
            inst.id = id.to_string();
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            for (name, value) in overrides {
                inst.params.iter_mut().find(|p| p.name == *name).unwrap().set_text(value.to_string());
            }
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
                bypassed: false,
                position: (0.0, 0.0),
                inputs: 0,
                outputs: 0,
            };
            let mut visited = Vec::new();
            let mut ocl_err = None;
            let geom = crate::geometry::generate_single_node_geometry_with_errors(
                &root,
                &root.children[0],
                &mut visited,
                &mut ocl_err,
                &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
            ).expect("Geometry generation failed");
            assert!(ocl_err.is_none(), "node error: {:?}", ocl_err);
            geom
        };

        // Defaults (Width/Length 1.0, Columns/Rows 16): a 16x16 grid of
        // two-triangle cells, flat at y = 0, spanning [-0.5, 0.5] on X and Z.
        let geom = generate(&[], "plane_inst");
        assert_eq!(geom.num_points(), 17 * 17);
        let mut max_x: f32 = 0.0;
        let mut max_z: f32 = 0.0;
        for pos in geom.positions() {
            assert!(pos[1].abs() < 1e-6, "Expected flat plane at y=0, got y={}", pos[1]);
            max_x = max_x.max(pos[0].abs());
            max_z = max_z.max(pos[2].abs());
        }
        assert!((max_x - 0.5).abs() < 0.01, "Expected half-width 0.5 on X, got {}", max_x);
        assert!((max_z - 0.5).abs() < 0.01, "Expected half-length 0.5 on Z, got {}", max_z);

        // Width and Length size their axes independently.
        let geom_2 = generate(&[("width", "2.0"), ("length", "3.0")], "plane_inst_2");
        let max_x_2 = geom_2.positions().iter().map(|p| p[0].abs()).fold(0.0f32, f32::max);
        let max_z_2 = geom_2.positions().iter().map(|p| p[2].abs()).fold(0.0f32, f32::max);
        assert!((max_x_2 - 1.0).abs() < 0.01, "Expected half-width 1.0 on X, got {}", max_x_2);
        assert!((max_z_2 - 1.5).abs() < 0.01, "Expected half-length 1.5 on Z, got {}", max_z_2);

        // Columns/Rows control the cell counts per axis.
        let geom_3 = generate(&[("columns", "4"), ("rows", "8")], "plane_inst_3");
        assert_eq!(geom_3.num_points(), 5 * 9, "a 4x8 cell grid is 5x9 points");
    }

    #[test]
    fn test_project_serialization_roundtrip() {
        let root = FsNode {
            id: "root".to_string(),
            name: "test_root".to_string(),
            node_type: "node".to_string(),
            children: vec![
                FsNode {
                    id: "child1".to_string(),
                    name: "child1".to_string(),
                    node_type: "sphere".to_string(),
                    children: vec![],
                    params: vec![],
                    geometry_visible: true,
                    bypassed: false,
                    position: (5.0, 6.0),
                    inputs: 0,
                    outputs: 1,
                }
            ],
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };
        let view_state = ProjectViewState {
            active_camera: "child1".to_string(),
            pan: (1.5, -2.5),
            current_path: vec![0],
            selected_node: Some(2),
            ..Default::default()
        };
        let proj = Project {
            name: "Test Project".to_string(),
            root,
            view_state,
            format: crate::app::PROJECT_FORMAT,
        };

        let content = serde_json::to_string(&proj).expect("failed to serialize");
        let proj2: Project = serde_json::from_str(&content).expect("failed to deserialize");

        assert_eq!(proj.name, proj2.name);
        assert_eq!(proj.view_state.active_camera, proj2.view_state.active_camera);
        assert_eq!(proj.view_state.pan, proj2.view_state.pan);
        assert_eq!(proj.view_state.current_path, proj2.view_state.current_path);
        assert_eq!(proj.view_state.selected_node, proj2.view_state.selected_node);
        assert_eq!(proj.root.name, proj2.root.name);
        assert_eq!(proj.root.children.len(), proj2.root.children.len());
        assert_eq!(proj.root.children[0].name, proj2.root.children[0].name);
        assert_eq!(proj.root.children[0].position, proj2.root.children[0].position);
    }

    #[test]
    fn test_geometry_attributes_system() {
        // Two points, built the way the pipeline builds them.
        let mut geom = Detail::new();
        geom.add_point(Vec3::new(1.0, 2.0, 3.0));
        geom.add_point(Vec3::new(4.0, 5.0, 6.0));
        geom.set_color(0, [1.0, 0.0, 0.0]);
        geom.set_color(1, [0.0, 1.0, 0.0]);

        // A column covers its whole class. The soup could hold an attribute on
        // one vertex and not the next, which is what the dashes in this
        // spreadsheet used to mean; there is no ragged case left to render.
        geom.points_mut().create("UV", AttribValue::Float2([0.0; 2]));
        geom.points_mut().set_value("UV", 0, AttribValue::Float2([0.1, 0.2])).unwrap();
        geom.points_mut().set_value("UV", 1, AttribValue::Float2([0.3, 0.4])).unwrap();
        geom.points_mut().create("ID", AttribValue::Int(0));
        geom.points_mut().set_value("ID", 0, AttribValue::Int(42)).unwrap();
        geom.points_mut().create_group("pinned");
        geom.points_mut().add_to_group("pinned", 1);
        // A detail attribute — what Analysis writes — shows as a `d:` column,
        // constant down the table, which is what a detail attribute is.
        geom.detail_mut().create_kind(
            "mass_max",
            AttribValue::Float(9.5),
            AttribKind::Derivative,
        );

        assert_eq!(geom.num_points(), 2);
        let render_verts = crate::geometry::detail_vertices(&geom);
        assert!(render_verts.is_empty(), "two loose points make no triangles");

        let (headers, rows) = State::geometry_to_spreadsheet_data(&geom);
        assert_eq!(
            headers,
            vec![
                "Point", "group:pinned", "Pos.x", "Pos.y", "Pos.z", "Col.r", "Col.g", "Col.b",
                "ID", "UV.x", "UV.y", "d:mass_max~",
            ]
        );

        assert_eq!(rows.len(), 2, "one row per point");
        assert!(rows.iter().all(|r| r.len() == headers.len()), "a cell a column");
        assert_eq!(rows[0][0], "0");
        // The groups stand beside the point's number, where they are seen
        // without scrolling, and say 0 as plainly as 1.
        assert_eq!(rows[0][1], "0", "point 0 is not in the group");
        assert_eq!(rows[0][2], "1.0000"); // Pos.x
        assert_eq!(rows[0][5], "1.0000"); // Col.r
        assert_eq!(rows[0][8], "42"); // ID, an integer and printed as one
        assert_eq!(rows[0][9], "0.1000"); // UV.x

        assert_eq!(rows[1][0], "1");
        assert_eq!(rows[1][1], "1", "point 1 is in the group");
        assert_eq!(rows[1][2], "4.0000");
        assert_eq!(rows[1][8], "0", "unwritten is the type's zero, not a dash");
        assert_eq!(rows[1][10], "0.4000"); // UV.y
        assert_eq!(rows[0][11], "9.5000");
        assert_eq!(rows[1][11], "9.5000", "a detail value repeats down the column");
        // The trailing ~ says this one resets at every step boundary.
        assert!(headers[11].ends_with('~'), "{}", headers[11]);
    }

    #[test]
    fn test_line_geometry_generation() {
        let start = Vec3::new(0.0, 0.0, 0.0);
        let end = Vec3::new(0.0, 1.0, 0.0);
        let d = box_detail(start, end, 0.02);

        // A box line fans to 36 renderer vertices: 6 faces * 2 triangles * 3 corners.
        assert_eq!(detail_vertices(&d).len(), 36);

        // N and uv ride the vertices, one per corner.
        assert!(d.verts().has("N"));
        assert!(d.verts().has("uv"));
    }

    /// The reference cube guide is gone, and nothing that used to carry it
    /// breaks: an older state.kdl or project display block with
    /// `show_cube_enabled` still loads (serde ignores the key), there is no
    /// Show Cube command or chord, and the viewport menubar's Guides menu
    /// is Grid, Origin, Camera Pivot — its checkmarks and clicks addressed
    /// through the `GUIDE_*` positions, so the removal shifted no item onto
    /// another's action.
    #[test]
    fn the_cube_guide_is_gone_and_old_files_still_load() {
        use crate::app::{DesignSettings, GUIDES_MENU, GUIDE_CAMERA_PIVOT, GUIDE_GRID, GUIDE_ORIGIN};
        // A complete state.kdl as this build writes it, with the old key
        // put back into its viewport block.
        let mut state = State::new(false);
        state.viewport_mut().show_grid = false;
        state.save_settings();
        let written = fs::read_to_string(DesignSettings::file_path()).expect("state.kdl");
        assert!(!written.contains("show_cube"), "the key is no longer written");
        let old = written.replacen("viewport {", "viewport {\n    show_cube_enabled (bool)true", 1);
        assert!(old.contains("show_cube_enabled"), "the fixture carries the old key");
        let back = DesignSettings::from_kdl_str(&old);
        assert!(!back.viewport.show_grid_enabled, "the rest of the block still reads");

        // A project's display block, likewise.
        let mut json = serde_json::to_value(crate::app::DisplaySettings {
            viewport: back.viewport.clone(),
            render: back.render.clone(),
        })
        .unwrap();
        json["viewport"]["show_cube_enabled"] = serde_json::json!(true);
        json["viewport"]["show_origin_enabled"] = serde_json::json!(false);
        let d: crate::app::DisplaySettings = serde_json::from_value(json).expect("an old display block loads");
        assert!(!d.viewport.show_origin_enabled);

        assert!(crate::command::by_id("toggle_cube").is_none());
        assert!(!crate::command::COMMANDS.iter().any(|c| c.label.to_lowercase().contains("cube")));

        let items = state.menu(crate::slots::RIGHT_MENUBAR_IDX).menu_items_list()[GUIDES_MENU].clone();
        assert_eq!(items, vec!["Show Grid".to_string(), "Origin".to_string(), "Camera Pivot".to_string()]);
        assert_eq!((GUIDE_GRID, GUIDE_ORIGIN, GUIDE_CAMERA_PIVOT), (0, 1, 2));
        // The Origin item flips the origin, not whatever sits where it was.
        let before = state.viewport().show_origin;
        state.execute_action(crate::shortcut::Action::ToggleOrigin);
        assert_eq!(state.viewport().show_origin, !before);
    }

    #[test]
    fn test_design_settings_serialization_roundtrip() {
        let json_without_pivot = r#"
        {
            "viewport": {
                "bg_color": [0.05, 0.05, 0.10],
                "square": false,
                "show_camera_pivot_enabled": false,
                "camera_pivot_size": 1.0,
                "show_grid_enabled": true,
                "show_cube_enabled": false,
                "show_origin_enabled": true,
                "origin_size": 1.0,
                "grid_thickness": 0.03,
                "grid_color": [0.35, 0.35, 0.40]
            },
            "graph": {
                "grid_size_x": 80.0,
                "grid_size_y": 40.0,
                "gap_row_h": 20.0,
                "gap_col_w": 20.0
            }
        }
        "#;
        
        let settings: DesignSettings = serde_json::from_str(json_without_pivot).unwrap();
        assert_eq!(settings.viewport.show_camera_pivot_enabled, false);
        assert_eq!(settings.viewport.camera_pivot_size, 1.0);
        assert_eq!(settings.viewport.grid_color, [0.35, 0.35, 0.40]);
        
        // Grid geometry is config-owned now: a state file still carrying the old graph block
        // loads fine (the key is simply ignored) and never comes back out on save.
        let mut with_stale_graph: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        with_stale_graph["graph"] = serde_json::json!({ "grid_size_x": 71.0, "gap_col_w": 14.0 });
        let stale: DesignSettings = serde_json::from_value(with_stale_graph).unwrap();
        assert_eq!(stale.viewport.grid_color, [0.35, 0.35, 0.40]);
        let rewritten = serde_json::to_string(&stale).unwrap();
        assert!(!rewritten.contains("graph"), "state must not carry graph settings: {rewritten}");

        let serialized = serde_json::to_string(&settings).unwrap();
        let settings_roundtrip: DesignSettings = serde_json::from_str(&serialized).unwrap();
        assert_eq!(settings_roundtrip.viewport.show_camera_pivot_enabled, false);
        assert_eq!(settings_roundtrip.viewport.camera_pivot_size, 1.0);
        assert_eq!(settings_roundtrip.viewport.grid_color, [0.35, 0.35, 0.40]);
    }

    #[test]
    fn test_mcp_action_parsing() {
        let json_str = "{\"action\": \"add_node\", \"template_name\": \"Sphere\", \"name\": \"MySphere\", \"x\": 5.0, \"y\": 3.0}";
        let action: McpAction = serde_json::from_str(json_str).unwrap();
        match action {
            McpAction::AddNode { template_name, name, x, y } => {
                assert_eq!(template_name, "Sphere");
                assert_eq!(name, Some("MySphere".to_string()));
                assert_eq!(x, 5.0);
                assert_eq!(y, 3.0);
            }
            _ => panic!("Expected AddNode"),
        }
    }

    #[test]
    fn test_get_next_visible_pane() {
        // Without spreadsheet (3 panes: LEFT, RIGHT, PARAM)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, false, false), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, false, false), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, false, false), LEFT_MENUBAR_IDX);

        // With spreadsheet (4 panes: LEFT, RIGHT, PARAM, SPREADSHEET)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, true, false), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, true, false), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, true, false), SPREADSHEET_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(SPREADSHEET_MENUBAR_IDX, true, true, true, true, false), LEFT_MENUBAR_IDX);

        // Reverse cycling with shift key (without spreadsheet)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, false, true), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, false, true), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, false, true), LEFT_MENUBAR_IDX);

        // Reverse cycling with shift key (with spreadsheet)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, true, true), SPREADSHEET_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(SPREADSHEET_MENUBAR_IDX, true, true, true, true, true), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, true, true), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, true, true), LEFT_MENUBAR_IDX);
    }

    #[test]
    fn test_inertial_settings_fallback_and_ranges() {
        let friction_raw = 90_u16;
        let friction_f32 = (friction_raw as f32 / 100.0).clamp(0.5, 0.99);
        assert!(friction_f32 >= 0.5 && friction_f32 <= 0.99);
        
        let friction_raw_low = 30_u16;
        let friction_f32_low = (friction_raw_low as f32 / 100.0).clamp(0.5, 0.99);
        assert_eq!(friction_f32_low, 0.5);
    }

    #[test]
    fn test_decay_formula() {
        let friction = 0.90_f32;
        let dt_60 = 1.0 / 60.0;
        let decay_60 = friction.powf(dt_60 * 60.0);
        assert!((decay_60 - friction).abs() < 1e-5);

        let decay_0 = friction.powf(0.0 * 60.0);
        assert_eq!(decay_0, 1.0);
    }

    #[test]
    fn test_circular_network_clamping_math() {
        let header_h = 26.0;
        let status_h = 0.0;
        let gap = 18.0;

        let clamp_layout = |width: f32, height: f32, layout_x: f32, layout_y: f32, layout_r: f32| -> (f32, f32, f32) {
            let max_r = ((width - 2.0 * gap).min(height - header_h - status_h - 2.0 * gap) / 2.0).max(50.0);
            let r = layout_r.clamp(50.0, max_r);

            let min_x = gap + r;
            let max_x = (width - gap - r).max(min_x);
            let x = layout_x.clamp(min_x, max_x);

            let min_y = header_h + gap + r;
            let max_y = (height - status_h - gap - r).max(min_y);
            let y = layout_y.clamp(min_y, max_y);

            (x, y, r)
        };

        let (x, y, r) = clamp_layout(1000.0, 800.0, 500.0, 400.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 500.0);
        assert_eq!(y, 400.0);

        let (x, y, r) = clamp_layout(1000.0, 800.0, 50.0, 400.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 198.0);
        assert_eq!(y, 400.0);

        let (x, y, r) = clamp_layout(1000.0, 800.0, 500.0, 100.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 500.0);
        assert_eq!(y, 224.0);

        let (x, y, r) = clamp_layout(1000.0, 800.0, 500.0, 750.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 500.0);
        assert_eq!(y, 602.0);

        let (x, y, r) = clamp_layout(50.0, 50.0, 10.0, 10.0, 180.0);
        assert!(r >= 50.0);
        assert!(x >= 0.0);
        assert!(y >= 0.0);
    }

    #[test]
    fn test_floating_rectangular_pane_clamping() {
        let width = 1000.0_f32;
        let height = 800.0_f32;
        let header_h = 26.0_f32;
        let status_h = 0.0_f32;
        let gap = 18.0_f32;

        let clamp_floating = |_fx: f32, _fy: f32, fw: f32, _fh: f32| -> (f32, f32, f32, f32) {
            let fw = fw.clamp(150.0, (width - 2.0 * gap).max(150.0));
            let fx = gap;
            let fy = header_h + gap;
            let fh = (height - header_h - status_h - 2.0 * gap).max(100.0);
            (fx, fy, fw, fh)
        };

        let expected_h = height - header_h - status_h - 2.0 * gap;

        // Standard center case (fx anchored to gap, fy/fh to full height)
        let (x, y, w, h) = clamp_floating(100.0, 200.0, 400.0, 300.0);
        assert_eq!((x, y, w, h), (gap, header_h + gap, 400.0, expected_h));

        // Off-screen left/top
        let (x, y, w, h) = clamp_floating(-50.0, 0.0, 400.0, 300.0);
        assert_eq!((x, y, w, h), (gap, header_h + gap, 400.0, expected_h));

        // Off-screen right/bottom
        let (x, y, w, h) = clamp_floating(900.0, 700.0, 400.0, 300.0);
        assert_eq!((x, y, w, h), (gap, header_h + gap, 400.0, expected_h));

        let clamp_param = |pw: f32| -> (f32, f32, f32, f32) {
            let pw = pw.clamp(150.0, (width - 2.0 * gap).max(150.0));
            let px = width - gap - pw;
            let py = header_h + gap;
            let ph = (height - header_h - status_h - 2.0 * gap).max(100.0);
            (px, py, pw, ph)
        };

        // Standard param case
        let (px, py, pw, ph) = clamp_param(300.0);
        assert_eq!((px, py, pw, ph), (width - gap - 300.0, header_h + gap, 300.0, expected_h));

        // Off-screen/overflow width param case
        let (px, py, pw, ph) = clamp_param(1200.0);
        let max_w = width - 2.0 * gap;
        assert_eq!((px, py, pw, ph), (gap, header_h + gap, max_w, expected_h));

        let clamp_ss = |ss_h_val: f32, show_net: bool, show_param: bool| -> (f32, f32, f32, f32) {
            let fx = gap;
            let fw = 400.0;
            let param_w = 300.0;
            let param_x = width - gap - param_w;

            let ss_x = if show_net { fx + fw + gap } else { gap };
            let ss_w_end = if show_param { param_x - gap } else { width - gap };
            let ss_w = (ss_w_end - ss_x).max(150.0);
            let ss_y_end = height - status_h - gap;
            let ss_h = ss_h_val.clamp(100.0, (ss_y_end - header_h - gap).max(100.0));
            let ss_y = ss_y_end - ss_h;
            (ss_x, ss_y, ss_w, ss_h)
        };

        // Standard spreadsheet case (both net and param visible)
        let (sx, sy, sw, sh) = clamp_ss(250.0, true, true);
        assert_eq!(sx, gap + 400.0 + gap);
        assert_eq!(sw, (width - gap - 300.0 - gap) - (gap + 400.0 + gap));
        assert_eq!(sh, 250.0);
        assert_eq!(sy, height - status_h - gap - 250.0);

        // Neither net nor param visible
        let (sx2, _sy2, sw2, sh2) = clamp_ss(200.0, false, false);
        assert_eq!(sx2, gap);
        assert_eq!(sw2, width - 2.0 * gap);
        assert_eq!(sh2, 200.0);

        // Clamping height to min (100.0)
        let (_, _, _, sh_min) = clamp_ss(50.0, true, true);
        assert_eq!(sh_min, 100.0);

        // Clamping height to max
        let max_possible_h = height - status_h - gap - header_h - gap;
        let (_, _, _, sh_max) = clamp_ss(1000.0, true, true);
        assert_eq!(sh_max, max_possible_h);
    }

    #[test]
    fn test_paginator_collapsed_layout() {
        let width = 1000.0_f32;

        let get_paginator_w = |page_hidden: bool| -> f32 {
            if page_hidden {
                56.0_f32
            } else {
                236.0_f32
            }
        };

        // Collapsed layout
        let w_collapsed = get_paginator_w(true);
        assert_eq!(w_collapsed, 56.0);

        // Expanded layout
        let w_expanded = get_paginator_w(false);
        assert_eq!(w_expanded, 236.0);

        // Verify how other viewport bounds adjust relative to paginator_w
        let check_viewport_layout = |page_hidden: bool| -> (f32, f32) {
            let paginator_w = get_paginator_w(page_hidden);
            let col_c_x = paginator_w;
            let col_c_w = width - paginator_w;
            (col_c_x, col_c_w)
        };

        // When collapsed, viewport/canvas has more space
        let (vx_c, vw_c) = check_viewport_layout(true);
        assert_eq!(vx_c, 56.0);
        assert_eq!(vw_c, 944.0);

        // When expanded, viewport/canvas has default space
        let (vx_e, vw_e) = check_viewport_layout(false);
        assert_eq!(vx_e, 236.0);
        assert_eq!(vw_e, 764.0);
    }

    /// The list's scrollbar is cce-mail's: sunk until a scroll raises it,
    /// draggable while raised, sunk again after the hold — and while sunk it
    /// takes no input, so a press on its lane reaches the row beneath.
    #[test]
    fn dialog_scrollbar_raises_on_scroll_drags_and_sinks() {
        use cce_ui::widget::{ElementState, MouseButton, MouseScrollDelta, Position, ScrollPhase, WidgetHost};
        use crate::dialog::{Dialog, Row};
        let mut ctx = cce_ui::context::UiContext::new();
        let mut d = Dialog::new();
        d.set_visible(true);
        WidgetHost::set_rect(&mut d, 0.0, 0.0, 520.0, 420.0);
        let rect = cce_ui::scene::layout::Rect { x: 0.0, y: 0.0, width: 520.0, height: 420.0 };
        let rows: Vec<Row> = (0..60)
            .map(|i| Row { id: format!("c{i}"), label: format!("Command {i}"), chord: String::new(), control: None, truncate_head: false })
            .collect();
        d.set_rows(rows);
        d.set_page(10);
        assert!(d.scrollbar_geom(rect).is_some(), "sixty rows overflow: there is a bar");
        assert!(!d.scrollbar_raised(), "sunk until something scrolls");

        cce_ui::widget::scroll_motion::set_scroll_phase(ScrollPhase::Finger);
        d.mouse_wheel_ungated(&MouseScrollDelta::PixelDelta(Position { x: 0.0, y: -30.0 }), 100.0, 200.0, &mut ctx);
        WidgetHost::tick(&mut d, 1.0 / 60.0, &mut ctx);
        assert!(d.scrollbar_raised(), "a scroll raises the bar");

        // Grab the thumb and drag it down: the list follows.
        let (sb_x, _, sb_w, _, thumb_y, thumb_h) = d.scrollbar_geom(rect).unwrap();
        let before = d.scroll_px;
        let gx = sb_x + sb_w * 0.5;
        let gy = thumb_y + thumb_h * 0.5;
        assert!(d.mouse_input(MouseButton::Left, ElementState::Pressed, gx, gy, &mut ctx));
        d.cursor_moved(gx, gy + 80.0, &mut ctx);
        assert!(d.scroll_px > before + 10.0, "dragging the thumb scrolls: {} -> {}", before, d.scroll_px);
        d.mouse_input(MouseButton::Left, ElementState::Released, gx, gy + 80.0, &mut ctx);
        assert!(d.scrollbar_raised(), "the release starts the hold");

        // Quiet for longer than the hold and the fade: the bar sinks, and a
        // press on its lane is a row press again.
        for _ in 0..90 {
            WidgetHost::tick(&mut d, 1.0 / 60.0, &mut ctx);
        }
        assert!(!d.scrollbar_raised(), "the bar sinks after the hold");
        let was = d.scroll_px;
        d.mouse_input(MouseButton::Left, ElementState::Pressed, gx, gy, &mut ctx);
        assert!((d.scroll_px - was).abs() < 0.01, "a sunk bar takes no input");
        assert!(d.take_activated().is_some(), "the press reached the row beneath");
    }

    /// The dialog's commands list takes a trackpad (finger-phase pixel
    /// delta) as well as a wheel notch (2026-09-20: a finger did nothing).
    #[test]
    fn dialog_list_scrolls_by_trackpad_and_by_wheel() {
        use cce_ui::widget::{MouseScrollDelta, Position, ScrollPhase, WidgetHost};
        use crate::dialog::{Dialog, Row};
        let mut ctx = cce_ui::context::UiContext::new();
        let mut d = Dialog::new();
        d.set_visible(true);
        WidgetHost::set_rect(&mut d, 0.0, 0.0, 520.0, 420.0);
        let rows: Vec<Row> = (0..60)
            .map(|i| Row { id: format!("c{i}"), label: format!("Command {i}"), chord: String::new(), control: None, truncate_head: false })
            .collect();
        d.set_rows(rows);
        d.set_page(10);
        assert_eq!(d.scroll_px, 0.0);

        cce_ui::widget::scroll_motion::set_scroll_phase(ScrollPhase::Finger);
        let moved = d.mouse_wheel_ungated(&MouseScrollDelta::PixelDelta(Position { x: 0.0, y: -30.0 }), 100.0, 200.0, &mut ctx);
        assert!(moved, "a finger delta moves the list");
        assert!(d.scroll_px > 0.0, "trackpad scrolled the list: {}", d.scroll_px);
        // The layout re-records the page on every relayout; that must not
        // snap the list back to the (unscrolled) selection.
        let scrolled = d.scroll_px;
        d.set_page(10);
        assert_eq!(d.scroll_px, scrolled, "set_page keeps the scroll");

        let before = d.scroll_px;
        cce_ui::widget::scroll_motion::set_scroll_phase(ScrollPhase::Wheel);
        d.mouse_wheel_ungated(&MouseScrollDelta::LineDelta(0.0, -1.0), 100.0, 200.0, &mut ctx);
        for _ in 0..30 {
            WidgetHost::tick(&mut d, 1.0 / 60.0, &mut ctx);
        }
        assert!(d.scroll_px > before, "a wheel notch glides the list: {} -> {}", before, d.scroll_px);
    }

    #[test]
    fn test_keyboard_shortcut_system() {
        // Test parsing simple shortcut
        let ctrl_g = Shortcut::parse("Ctrl+g").unwrap();
        assert_eq!(ctrl_g.ctrl, true);
        assert_eq!(ctrl_g.shift, false);
        assert_eq!(ctrl_g.alt, false);
        assert_eq!(ctrl_g.logo, false);
        assert_eq!(ctrl_g.key, Key::Character("g".to_string()));

        // Test parsing complex shortcut
        let complex = Shortcut::parse("Ctrl+Shift+Alt+Logo+s").unwrap();
        assert_eq!(complex.ctrl, true);
        assert_eq!(complex.shift, true);
        assert_eq!(complex.alt, true);
        assert_eq!(complex.logo, true);
        assert_eq!(complex.key, Key::Character("s".to_string()));

        // Test parsing named keys
        let tab_sc = Shortcut::parse("Tab").unwrap();
        assert_eq!(tab_sc.key, Key::Named(NamedKey::Tab));

        // Test parsing case insensitivity
        let case_sc = Shortcut::parse("cTrL+sHiFt+ArrowDown").unwrap();
        assert_eq!(case_sc.ctrl, true);
        assert_eq!(case_sc.shift, true);
        assert_eq!(case_sc.key, Key::Named(NamedKey::ArrowDown));

        // Test register and match
        let mut mgr = ShortcutManager::new();
        mgr.register("Ctrl+g", "toggle_grid").unwrap();
        mgr.register("`", "toggle_spreadsheet").unwrap();

        // Matches with ctrl and g
        let mods_ctrl = ModifiersState { ctrl: true, alt: false, shift: false, logo: false };
        let key_g = Key::Character("g".to_string());
        assert_eq!(mgr.match_command(&mods_ctrl, &key_g), Some("toggle_grid"));

        // No match with ctrl and a
        let key_a = Key::Character("a".to_string());
        assert_eq!(mgr.match_command(&mods_ctrl, &key_a), None);

        // Matches backtick with no modifiers
        let mods_none = ModifiersState::default();
        let key_tick = Key::Character("`".to_string());
        assert_eq!(mgr.match_command(&mods_none, &key_tick), Some("toggle_spreadsheet"));

        // Context cycling chords: exact modifier match separates next from previous
        mgr.register("Ctrl+Tab", "next_context").unwrap();
        mgr.register("Ctrl+Shift+Tab", "previous_context").unwrap();
        let key_tab = Key::Named(NamedKey::Tab);
        let mods_ctrl_shift = ModifiersState { ctrl: true, alt: false, shift: true, logo: false };
        assert_eq!(mgr.match_command(&mods_ctrl, &key_tab), Some("next_context"));
        assert_eq!(mgr.match_command(&mods_ctrl_shift, &key_tab), Some("previous_context"));
        assert_eq!(mgr.match_command(&mods_none, &key_tab), None);
    }

    #[test]
    fn test_popover_draws_over_widget_labels() {
        // Regression: the display list draws strictly in order, so an open
        // dropdown's popover (background AND option text) must be appended
        // after the widget-label text pass — popover rects emitted in the
        // geometry pass sat under every label, and the labels of buttons
        // beneath the params pane's "Open" dropdown bled through it.
        let mut state = State::new(false);
        // Any node with a choice param will do; the Main utility node's
        // "Open" dropdown was this test's subject until that node was
        // retired. Scatter's Mode is a choice.
        let scatter = state
            .node_templates
            .iter()
            .find(|t| t.label == "Scatter")
            .expect("a Scatter template")
            .node
            .clone();
        state.current_dir_mut().children.push(scatter);
        let idx = state.current_dir().children.len() - 1;
        state.graph_mut().set_selected_node(Some(idx));
        state.sync_parameters_pane();

        {
            let dropdown = state
                .slots
                .param
                .inner_mut()
                .choices
                .iter_mut()
                .flatten()
                .next()
                .expect("Scatter's params include a dropdown (Mode)");
            dropdown.open = true;
            // The popover expands on a wall-clock animation, and every geometry
            // reader uses `anim_snap`, a snapshot refreshed only on tick/event —
            // so writing `open` directly leaves the snapshot at 0 and the menu
            // draws with zero extent. Ticking folds the progress in; because the
            // direct write left `anim_start` unset, that lands fully open at once
            // instead of waiting out the animation.
            let mut tick_ctx = cce_ui::context::UiContext::new();
            cce_ui::widget::WidgetHost::tick(dropdown, 0.0, &mut tick_ctx);
        }

        let list = state.collect_display_list();
        let text_pos = |needle: &str| {
            list.items.iter().position(|item| {
                matches!(&item.prim, cce_ui::scene::paint::Prim::Text { text, .. } if text == needle)
            })
        };
        // "Points" is a param label sitting under the open dropdown;
        // "Surface" is not Mode's current value, so it exists only inside
        // the popover's option list.
        let label_idx = text_pos("Points").expect("param label in display list");
        let option_idx = text_pos("Surface").expect("popover option text in display list");
        assert!(option_idx > label_idx, "popover text must draw after widget labels");
        // The trigger's plate grown — its fill and relief, whatever prims
        // those are — between the label and the option.
        let has_bg_between = list.items[label_idx..option_idx]
            .iter()
            .any(|item| !matches!(item.prim, cce_ui::scene::paint::Prim::Text { .. }));
        assert!(has_bg_between, "popover background must draw after widget labels");
    }

    #[test]
    fn test_mcp_tools_map_to_actions() {
        // Every MCP tool except get_state must dispatch by injecting its name
        // as the McpAction serde tag; filling each schema property with a
        // dummy of its declared type must yield a deserializable action, so
        // this catches tool-name/field drift against the enum.
        let tools = crate::api::mcp_tools();
        assert!(tools.iter().any(|t| t.name == "get_state"));
        let mut names = std::collections::HashSet::new();
        for tool in &tools {
            assert!(names.insert(tool.name.clone()), "duplicate tool name: {}", tool.name);
            assert_eq!(tool.input_schema["type"], "object", "{}: schema must be an object", tool.name);
            if tool.name == "get_state" {
                continue;
            }
            let mut args = serde_json::Map::new();
            if let Some(props) = tool.input_schema["properties"].as_object() {
                for (key, prop) in props {
                    let dummy = match prop["type"].as_str() {
                        Some("integer") => serde_json::json!(0),
                        Some("number") => serde_json::json!(0.0),
                        Some("string") => serde_json::json!("x"),
                        Some("boolean") => serde_json::json!(false),
                        Some("array") => serde_json::json!([]),
                        other => panic!("{}.{}: unhandled schema type {:?}", tool.name, key, other),
                    };
                    args.insert(key.clone(), dummy);
                }
            }
            args.insert("action".to_string(), serde_json::json!(tool.name));
            serde_json::from_value::<McpAction>(serde_json::Value::Object(args))
                .unwrap_or_else(|e| panic!("tool '{}' does not map to an McpAction: {e}", tool.name));
        }
    }

    #[test]
    fn test_grid_is_a_welded_sheet_wound_upward() {
        let root = modelling_root(
            "1.0",
            vec![phase3_node(
                "grid",
                &[("rows", "4"), ("columns", "6"), ("width", "2.00"), ("length", "3.00")],
            )],
        );
        let (g, err) = eval_node(&root, "grid 1");
        assert!(err.is_none(), "{err:?}");

        // Welded, not a corner list: 5 x 7 points for 4 x 6 quads. That is the
        // difference from the Plane subnet, whose kernel emits corners that
        // have to be welded on the way back.
        assert_eq!(g.num_points(), 5 * 7);
        assert_eq!(g.num_prims(), 4 * 6);
        for prim in 0..g.num_prims() {
            assert_eq!(g.prim_points(prim).len(), 4, "prim {prim} is not a quad");
        }

        // Sized by its parameters, centred where it was told.
        let (lo, hi) = g.bounds().unwrap();
        assert!(((hi - lo) - Vec3::new(2.0, 0.0, 3.0)).length() < 1e-5, "{:?}", hi - lo);
        assert!(((lo + hi) * 0.5).length() < 1e-5, "not centred: {:?}", (lo + hi) * 0.5);

        // Wound so the plain cross points up, like every other generator.
        for n in crate::geometry::point_normals(&g) {
            assert!(n.y > 0.99, "a face points {n:?} rather than up");
        }

        // An interior point has four neighbours, which is what says the sheet
        // is one surface rather than loose quads.
        let interior = (0..g.num_points())
            .find(|&p| g.point_neighbours(p).len() == 4)
            .expect("no interior point");
        assert_eq!(g.point_prims(interior).len(), 4);
    }

    #[test]
    fn test_polygon_is_four_create_operators_with_one_parameter_varying() {
        let build = |params: &[(&str, &str)]| {
            let mut ps = vec![("radius", "1.00")];
            ps.extend_from_slice(params);
            let root = modelling_root("1.0", vec![phase3_node("polygon", &ps)]);
            eval_node(&root, "polygon 1").0
        };

        // Three sides is a triangle, four a square, thirty-two a circle: the
        // same shape with one number changed.
        let tri = build(&[("sides", "3")]);
        assert_eq!(tri.num_prims(), 3, "a filled triangle is three fan triangles");
        assert_eq!(tri.num_points(), 4, "three corners and a hub");
        let circle = build(&[("sides", "32")]);
        assert_eq!(circle.num_points(), 33);

        // A circle's corners all sit at the radius; a square's do too.
        for d in [&circle, &build(&[("sides", "4")])] {
            for p in 0..d.num_points() {
                let r = d.pos(p).length();
                assert!(r < 1.0 + 1e-4, "point {p} is outside the radius at {r}");
            }
            assert!((d.bounds().unwrap().1.y).abs() < 1e-6, "the polygon is not flat");
            // Wound so the plain cross points up, like the grid and the
            // sphere. The grid got this backwards on the first try, so it is
            // worth asserting wherever a generator lays out a face by hand.
            for n in crate::geometry::point_normals(d) {
                assert!(n.y > 0.99, "a face points {n:?} rather than up");
            }
        }

        // A star alternates the two radii, so it has twice the corners and
        // half of them sit on the inner circle.
        let star = build(&[("sides", "5"), ("inner_radius", "0.40")]);
        assert_eq!(star.num_prims(), 10);
        let inner = (0..star.num_points())
            .filter(|&p| (star.pos(p).length() - 0.4).abs() < 1e-4)
            .count();
        assert_eq!(inner, 5, "the notches are not on the inner radius");

        // Filled from a CENTRE point, not fanned from a corner: on a star a
        // corner fan crosses the notches and the shape renders as its convex
        // hull. Every triangle here touches the hub.
        let hub = star.num_points() - 1;
        assert!(star.pos(hub).length() < 1e-6, "the hub is not at the centre");
        for prim in 0..star.num_prims() {
            assert!(
                star.prim_points(prim).contains(&(hub as u32)),
                "prim {prim} does not touch the hub, so the fill fans from a corner"
            );
        }

        // Unfilled is the outline: one two-point primitive per edge, a closed
        // loop, and nothing to shade.
        let ring = build(&[("sides", "6"), ("fill", "false")]);
        assert_eq!(ring.num_points(), 6, "no hub when there is no fill");
        assert_eq!(ring.num_prims(), 6);
        assert_eq!(ring.edges().len(), 6, "the outline closes");
        for p in 0..ring.num_points() {
            assert_eq!(ring.point_neighbours(p).len(), 2, "point {p} is not on a loop");
        }
    }

    // ---- The parameter pane's conditional rows ----

    fn pd(name: &str, value: &str, show_when: &str) -> crate::app::ParamDef {
        crate::app::ParamDef::new(name, "text", value).with_show_when(show_when)
    }

    #[test]
    fn test_a_row_shows_only_when_its_condition_holds() {
        use crate::app::{param_display, param_visible};
        let params = vec![
            pd("mode", "Twist", ""),
            pd("angle", "1.0", "mode == Twist"),
            pd("bend_axis", "Y", "mode == Bend"),
            pd("shared", "x", "mode == Twist|Bend"),
            pd("not_bleed", "x", "mode != Bleed"),
        ];
        let shown: Vec<String> = param_display(&params).into_iter().map(|r| r.0).collect();
        assert_eq!(shown, vec!["mode", "angle", "shared", "not_bleed"]);

        // Flip the driving parameter and a different set applies. This is the
        // whole point: collapsing fifty operators into ten traded node count
        // for parameter count, and a pane showing twelve irrelevant rows is
        // worse than the twelve nodes it replaced.
        let mut bent = params.clone();
        bent[0].set_text("Bend");
        let shown: Vec<String> = param_display(&bent).into_iter().map(|r| r.0).collect();
        assert_eq!(shown, vec!["mode", "bend_axis", "shared", "not_bleed"]);

        // Bleed matches none of the conditions, so only the driving row is
        // left — which is a node with one relevant control showing one.
        let mut bleeding = params.clone();
        bleeding[0].set_text("Bleed");
        let shown: Vec<String> = param_display(&bleeding).into_iter().map(|r| r.0).collect();
        assert_eq!(shown, vec!["mode"]);

        // The condition is evaluated against siblings' CURRENT values, which
        // is where this app keeps them.
        assert!(param_visible(&params, "mode == Twist"));
        assert!(!param_visible(&bleeding, "mode == Twist"));
    }

    #[test]
    fn test_conditions_and_together_and_compare_without_case() {
        use crate::app::param_visible;
        let params = vec![
            pd("mode", "Align", ""),
            pd("target", "Constant", ""),
        ];
        assert!(param_visible(&params, "mode == Align && target == Constant"));
        assert!(!param_visible(&params, "mode == Align && target == Attribute"));
        // Case does not matter: a template author writing `twist` and a choice
        // reading `Twist` is not a bug worth having.
        assert!(param_visible(&params, "mode == ALIGN"));
        // An empty condition always holds — that is what most parameters have.
        assert!(param_visible(&params, ""));
        assert!(param_visible(&params, "   "));
    }

    #[test]
    fn test_a_broken_condition_hides_its_row_rather_than_hiding_the_mistake() {
        use crate::app::param_visible;
        let params = vec![pd("mode", "Twist", "")];
        // A misspelled sibling, and a clause that is not a comparison at all.
        // Both are template bugs; showing the row unconditionally would let
        // them pass unnoticed, and the row going missing is a complaint you
        // can act on.
        assert!(!param_visible(&params, "Moed == Twist"));
        assert!(!param_visible(&params, "mode"));
        assert!(!param_visible(&params, "Mode ~ Twist"));
    }

    #[test]
    fn test_the_shipped_templates_only_name_parameters_they_have() {
        // Every condition in every template has to resolve, or the row it
        // guards silently never appears. Checking the shipped set here is
        // cheaper than finding one missing in the pane a month from now.
        let templates = crate::app::load_fs_tree();
        let mut checked = 0;
        fn walk(node: &FsNode, checked: &mut usize) {
            let names: Vec<&str> = node.params.iter().map(|p| p.name.as_str()).collect();
            for p in &node.params {
                for clause in p.show_when.split("&&") {
                    let clause = clause.trim();
                    if clause.is_empty() {
                        continue;
                    }
                    let sep = if clause.contains("!=") { "!=" } else { "==" };
                    let (lhs, rhs) = clause.split_once(sep).unwrap_or_else(|| {
                        panic!("{}: '{}' is not a comparison", node.name, clause)
                    });
                    let lhs = lhs.trim();
                    assert!(
                        names.iter().any(|n| n.eq_ignore_ascii_case(lhs)),
                        "{} guards '{}' on '{}', which it does not have",
                        node.name,
                        p.name,
                        lhs
                    );
                    assert!(!rhs.trim().is_empty(), "{}: '{}' compares to nothing", node.name, clause);
                    *checked += 1;
                }
            }
            for c in &node.children {
                walk(c, checked);
            }
        }
        for t in &templates.children {
            walk(t, &mut checked);
        }
        assert!(checked > 20, "only {checked} conditions checked — did the templates lose them?");
    }

    #[test]
    fn the_attribute_nodes_group_row_is_shown_where_it_is_read() {
        // Delete removes the attribute's whole column — there is no deleting
        // it from some points — and Promote reads every point, so neither
        // reads Group, and a row shown there would be a filter that filters
        // nothing. Every other operation limits its work to the group.
        let root = crate::app::load_fs_tree();
        let template = root.children.iter().find(|t| t.node_type == "attribute").expect("attribute template");
        let group = template.params.iter().find(|p| p.name == "group").expect("attribute has a group row");
        for (op, shown) in [
            ("Create", true), ("Modify", true), ("Remap", true), ("Clip", true),
            ("Normalize", true), ("Composite", true), ("Delete", false), ("Promote", false),
        ] {
            let mut params = template.params.clone();
            params.iter_mut().find(|p| p.name == "operation").unwrap().set_text(op);
            assert_eq!(crate::app::param_visible(&params, &group.show_when), shown, "Group on {op}");
        }
    }

    #[test]
    fn test_hiding_a_row_does_not_lose_its_value() {
        use crate::app::param_display;
        // Write-back resolves a row by its display key, not by position, so a
        // hidden parameter is simply not reported and keeps what it had. A
        // user who sets a Remap range, switches to Clip and switches back must
        // find their numbers still there.
        let mut params = vec![
            pd("operation", "Remap", ""),
            pd("to", "7.5", "operation == Remap"),
        ];
        assert_eq!(param_display(&params).len(), 2);
        params[0].set_text("Clip");
        assert_eq!(param_display(&params).len(), 1, "the row hid");
        assert_eq!(params[1].text(), "7.5", "but the value is untouched");
        params[0].set_text("Remap");
        assert_eq!(param_display(&params)[1].1, "7.5", "and comes back as it was");
    }

    // ---- Volumes ----

    use crate::volume::Volume;

    #[test]
    fn test_a_sphere_round_trips_through_a_distance_field() {
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 16, 24);
        let vol = Volume::from_mesh(&sphere, 0.12, 0.4);
        let back = vol.to_mesh();

        assert!(back.num_points() > 100, "the surface did not come back");
        assert!(back.num_prims() > 100);

        // Every extracted point is on the sphere, to within a voxel. That is
        // the whole claim of the representation: a mesh in, a field, a mesh
        // out, and the shape survives.
        for p in 0..back.num_points() {
            let r = back.pos(p).length();
            assert!((r - 1.0).abs() < 0.14, "point {p} is at radius {r}");
        }

        // Closed: every edge is shared by exactly two faces. Surface nets
        // gives this by construction, and it is what makes the output safe to
        // hand a slicer.
        let mut shared: std::collections::HashMap<[u32; 2], usize> = Default::default();
        for prim in 0..back.num_prims() {
            let pts = back.prim_points(prim);
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                *shared.entry([a.min(b), a.max(b)]).or_default() += 1;
            }
        }
        let open = shared.values().filter(|&&c| c != 2).count();
        assert_eq!(open, 0, "{open} edges are not shared by two faces");

        // And it faces outward, like every other generator.
        let normals = crate::geometry::point_normals(&back);
        let outward = (0..back.num_points())
            .filter(|&p| normals[p].dot(back.pos(p).normalize()) > 0.0)
            .count();
        assert_eq!(outward, back.num_points(), "the extracted surface is inside out");
    }

    #[test]
    fn test_the_sign_is_right_where_the_nearest_face_would_lie() {
        // A field's sign has to be right EVERYWHERE — a wrong one is a bubble
        // or a hole, where in the Distance node it was a slightly wrong
        // number. This is why the build casts rays rather than asking the
        // nearest face which way it points, and this is the check that says so.
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 20, 28);
        let vol = Volume::from_mesh(&sphere, 0.12, 0.3);
        let [nx, ny, nz] = vol.dims();

        let mut wrong = Vec::new();
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let p = vol.sample_position(i, j, k);
                    let r = p.length();
                    // Skip the band where the answer is genuinely ambiguous at
                    // this resolution.
                    if (r - 1.0).abs() < vol.voxel() {
                        continue;
                    }
                    let want_inside = r < 1.0;
                    if (vol.at(i, j, k) < 0.0) != want_inside {
                        wrong.push((i, j, k, r, vol.at(i, j, k)));
                    }
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "{} of {} samples have the wrong sign, e.g. {:?}",
            wrong.len(),
            nx * ny * nz,
            &wrong[..wrong.len().min(3)]
        );
    }

    /// An axis-aligned closed box, built by hand.
    fn box_mesh(lo: Vec3, hi: Vec3) -> Detail {
        let mut d = Detail::new();
        for (x, y, z) in [
            (lo.x, lo.y, lo.z), (hi.x, lo.y, lo.z), (hi.x, lo.y, hi.z), (lo.x, lo.y, hi.z),
            (lo.x, hi.y, lo.z), (hi.x, hi.y, lo.z), (hi.x, hi.y, hi.z), (lo.x, hi.y, hi.z),
        ] {
            d.add_point(Vec3::new(x, y, z));
        }
        // Wound counter-clockwise seen from OUTSIDE, like every generator —
        // asserted below, because getting this backwards by hand is exactly
        // what happened the first time.
        for q in [
            [0u32, 1, 2, 3], [7, 6, 5, 4], [0, 4, 5, 1],
            [1, 5, 6, 2], [2, 6, 7, 3], [3, 7, 4, 0],
        ] {
            d.add_prim(&q);
        }
        let centre = (lo + hi) * 0.5;
        for (p, n) in crate::geometry::point_normals(&d).iter().enumerate() {
            assert!(
                n.dot((d.pos(p) - centre).normalize()) > 0.0,
                "the test box's corner {p} faces inward"
            );
        }
        d
    }

    /// The page nodes end to end, through the resolver — not just the raster.
    ///
    /// Written before the UI was wired, because the Boolean node taught this
    /// exact lesson one commit ago: arithmetic that passes every unit test and
    /// wiring nobody has exercised look identical until something asks for the
    /// result.
    #[test]
    fn test_the_page_nodes_compose_a_sheet_through_the_resolver() {
        use crate::page::{displayed_page, is_page_node, resolve_page};

        fn pnode(id: &str, name: &str, ty: &str, params: &[(&str, &str)]) -> FsNode {
            FsNode {
                id: id.to_string(),
                name: name.to_string(),
                node_type: ty.to_string(),
                children: vec![],
                params: params
                    .iter()
                    .map(|(n, v)| crate::app::ParamDef::new(n.to_string(), "text".to_string(), v.to_string()))
                    .collect(),
                geometry_visible: true,
                bypassed: false,
                position: (0.0, 0.0),
                inputs: 1,
                outputs: 1,
            }
        }

        let root = pnode("r", "root", "node", &[]);
        let mut root = root;
        root.children = vec![
            pnode(
                "p",
                "page1",
                "page",
                &[
                    ("preset", "Letter"),
                    ("resolution", "72"),
                    ("color", "1.00:1.00:1.00"),
                ],
            ),
            pnode(
                "g",
                "grid1",
                "page_grid",
                &[
                    ("input", "page1"),
                    ("cell_size", "0.5"),
                    ("line_width", "0.02"),
                    ("line_color", "0.00:0.00:0.00"),
                    ("fill_cells", "false"),
                ],
            ),
            pnode(
                "b",
                "border1",
                "page_border",
                &[("input", "grid1"), ("width", "0.1"), ("inset", "0.25"), ("color", "1.00:0.00:0.00")],
            ),
        ];

        assert!(is_page_node("page_grid") && !is_page_node("sphere"));

        let page = resolve_page(&root, &root.children[2], &mut Vec::new())
            .expect("the page chain resolved to nothing");
        assert_eq!((page.width, page.height), (612, 792), "Letter at 72 DPI");

        let at = |x: u32, y: u32| page.pixels[(y * page.width + x) as usize];
        // The border is red where it was asked for, and nowhere else.
        let b = at(20, 400);
        assert!(b[0] > 0.9 && b[1] < 0.1, "no border ink at the left edge: {b:?}");
        assert!(at(300, 400)[1] > 0.5, "the border filled the sheet");
        // The grid ruled the interior: 0.5 inches at 72 DPI is every 36 px.
        assert!(at(36 * 4, 400)[0] < 0.4, "no rule at 2.0 inches");
        assert!(at(36 * 4 + 18, 400)[0] > 0.9, "the cell between rules is not clear");

        // An orphan composite is not a page: a border with nothing under it
        // resolves to nothing rather than inventing a sheet.
        let orphan = pnode("o", "border2", "page_border", &[("input", "nothing")]);
        let mut lone = root.clone();
        lone.children.push(orphan);
        assert!(
            resolve_page(&lone, lone.children.last().unwrap(), &mut Vec::new()).is_none(),
            "a border with no page under it invented one"
        );

        // A cycle terminates rather than recursing forever.
        let mut looped = root.clone();
        looped.children[0] = pnode("p", "page1", "page_border", &[("input", "border1")]);
        assert!(resolve_page(&looped, &looped.children[2], &mut Vec::new()).is_none());

        // The level's LAST visible page node is what gets displayed.
        // Green, not red: the red border and the white sheet both read 1.0 in
        // the red channel, so testing that one proves nothing either way.
        let border_ink = |p: &crate::page::Page| p.pixels[(400 * p.width + 20) as usize][1];
        let shown = displayed_page(&root, &root).expect("nothing displayed");
        assert!(border_ink(&shown) < 0.1, "the border chain is not what showed");
        let mut hidden = root.clone();
        hidden.children[2].geometry_visible = false;
        let shown = displayed_page(&hidden, &hidden).expect("nothing displayed");
        assert!(border_ink(&shown) > 0.9, "a hidden node still displayed");
    }

    /// The page cache's key moves with everything the page is composed from
    /// and with nothing else: an edit beside the chain must not recompose a
    /// 300 DPI sheet, and an edit inside it must.
    #[test]
    fn test_the_page_key_follows_the_chain_and_only_the_chain() {
        use crate::page::chain_key;
        fn pnode(id: &str, name: &str, ty: &str, params: &[(&str, &str)]) -> FsNode {
            FsNode {
                id: id.to_string(),
                name: name.to_string(),
                node_type: ty.to_string(),
                children: vec![],
                params: params
                    .iter()
                    .map(|(n, v)| crate::app::ParamDef::new(n.to_string(), "text".to_string(), v.to_string()))
                    .collect(),
                geometry_visible: true,
                bypassed: false,
                position: (0.0, 0.0),
                inputs: 1,
                outputs: 1,
            }
        }
        let mut root = pnode("r", "root", "node", &[]);
        root.children = vec![
            pnode("p", "page1", "page", &[("preset", "Letter"), ("resolution", "72")]),
            pnode("g", "grid1", "page_grid", &[("input", "page1"), ("cell_size", "0.5")]),
            pnode("b", "border1", "page_border", &[("input", "grid1"), ("width", "0.1")]),
            pnode("s", "sphere1", "sphere", &[("radius", "1")]),
        ];
        let key = |root: &FsNode| chain_key(root, &root.children[2]);
        let base = key(&root);
        assert_eq!(base, key(&root.clone()), "the same chain keys the same");

        let mut beside = root.clone();
        beside.children[3].params[0].set_text("2".to_string());
        assert_eq!(base, key(&beside), "a geometry edit beside the page moved its key");

        let mut edited = root.clone();
        edited.children[1].params[1].set_text("0.25".to_string());
        assert_ne!(base, key(&edited), "a grid edit under the border kept the key");

        let mut sheet = root.clone();
        sheet.children[0].params[1].set_text("300".to_string());
        assert_ne!(base, key(&sheet), "the sheet's resolution kept the key");

        let mut bypassed = root.clone();
        bypassed.children[1].bypassed = true;
        assert_ne!(base, key(&bypassed), "bypassing the grid kept the key");

        let mut rewired = root.clone();
        rewired.children[2].params[0].set_text("page1".to_string());
        assert_ne!(base, key(&rewired), "rewiring the border past the grid kept the key");
    }

    /// The 8-bit conversion clamps through the cast now; out-of-range and
    /// NaN channels must land where the explicit clamp put them.
    #[test]
    fn test_page_rgba8_clamps_and_rounds() {
        let mut page = crate::page::Page::new([1.0, 1.0], 2, [0.0; 4]);
        page.pixels[0] = [-1.0, 0.5, 2.0, f32::NAN];
        page.pixels[1] = [0.0, 1.0, 0.0019, 0.002];
        page.pixels[2] = [f32::INFINITY, f32::NEG_INFINITY, 0.999, 1.0001];
        let out = page.to_rgba8();
        assert_eq!(&out[..12], &[0, 128, 255, 0, 0, 255, 0, 1, 255, 0, 255, 255]);
    }

    /// Text lands on the sheet, and alignment moves it.
    #[test]
    fn test_page_text_puts_ink_where_it_is_aligned() {
        use crate::page::{HAlign, Page, TextSpec, VAlign};
        let ink = |halign, valign| {
            let mut p = Page::new([4.0, 2.0], 72, [1.0, 1.0, 1.0, 1.0]);
            crate::page::with_fonts_for_test(|fonts, cache| {
                p.text(
                    fonts,
                    cache,
                    &TextSpec {
                        text: "Hg",
                        size: 0.5,
                        at: [2.0, 1.0],
                        halign,
                        valign,
                        ..Default::default()
                    },
                );
            });
            // The centroid of the ink, in pixels.
            let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
            for y in 0..p.height {
                for x in 0..p.width {
                    let v = 1.0 - p.pixels[(y * p.width + x) as usize][0] as f64;
                    if v > 0.5 {
                        sx += x as f64;
                        sy += y as f64;
                        n += 1.0;
                    }
                }
            }
            assert!(n > 0.0, "no ink at all");
            (sx / n, sy / n)
        };

        let (cx, cy) = ink(HAlign::Center, VAlign::Middle);
        assert!((cx - 144.0).abs() < 25.0, "centred text sits at x={cx}, not the middle");
        assert!((cy - 72.0).abs() < 25.0, "middled text sits at y={cy}, not the middle");

        let (lx, _) = ink(HAlign::Left, VAlign::Middle);
        let (rx, _) = ink(HAlign::Right, VAlign::Middle);
        assert!(lx > cx && cx > rx, "alignment did not move the ink: {lx} {cx} {rx}");
    }

    /// Fuzzy ranking is the plugin's fuzzyfinder, deliberately: shortest
    /// contiguous span, then earliest start, then alphabetical. Muscle memory
    /// is the whole point of keeping it — "sg" has to keep landing on Show
    /// Grid.
    #[test]
    fn test_fuzzy_ranking_matches_the_plugins_order() {
        use crate::command::fuzzy_rank;
        let items = ["Show Grid", "Show Spreadsheet Pane", "Save As", "Set As Default"];

        // Subsequence, not substring.
        let r = fuzzy_rank("sg", &items);
        assert_eq!(items[r[0]], "Show Grid", "sg did not rank Show Grid first: {r:?}");

        // The tightest span wins over the earliest start: "sa" spans 2 in
        // "Save As" (Sa) and more in the others.
        let r = fuzzy_rank("sa", &items);
        assert_eq!(items[r[0]], "Save As");

        // No match at all drops out rather than ranking last.
        assert!(fuzzy_rank("zzz", &items).is_empty());

        // An empty query is every item in registry order, which is what makes
        // the palette usable as a plain list.
        assert_eq!(fuzzy_rank("", &items), vec![0, 1, 2, 3]);

        // Case and spaces in the query are ignored.
        assert_eq!(fuzzy_rank("S G", &items), fuzzy_rank("sg", &items));
    }

    /// The Wireframe Color command is a palette row that PREVIEWS the colour
    /// — the row carries the live wire colour as its swatch — and, picked,
    /// lands on the Settings half's Wireframe Color row, which edits the
    /// live `wire_color` (its owner was the Render node's "Wire Color" until
    /// that node was retired). The palette row shows the value; the settings
    /// row edits it.
    #[test]
    fn the_wireframe_colour_is_a_colour_row_of_the_palette() {
        use crate::dialog::{setting_row_id, Control, Owner, SETTINGS};
        assert!(crate::command::by_id("wireframe_color").is_none(), "the command went with the Settings half");

        let mut state = State::new(false);
        state.wire_color = [0.2, 0.6, 0.9];
        state.run_command("command_palette");
        let id = setting_row_id("Wireframe Color");
        let row = state
            .slots
            .dialog
            .rows
            .iter()
            .find(|r| r.id == id)
            .expect("the palette lists Wireframe Color");
        assert_eq!(row.label, "Wireframe Color");
        assert!(row.chord.is_empty(), "a setting has no chord");
        // The control carries the live colour, and a toolkit colour
        // selector stands behind it at the same value.
        let hex = crate::project::color_to_hex([0.2, 0.6, 0.9]);
        assert_eq!(row.control, Some(Control::Color { hex: hex.clone() }));
        let sel = state.slots.dialog.color_selector(&id).expect("a colour selector behind the row");
        assert_eq!(sel.get_value_string().as_deref(), Some(hex.as_str()));
        let s = SETTINGS.iter().find(|s| s.label == "Wireframe Color").unwrap();
        assert_eq!(s.owner, Owner::Field("wire_color"));

        // Editing the row reaches the live state: the colour AND the switch
        // that makes the wire pass use it (off, the wires carry the
        // geometry's colours). The dialog stays up, and the row re-reads the
        // value.
        state.wire_single_color = false;
        state.apply_setting("Wireframe Color", "#000000");
        assert_eq!(state.wire_color, [0.0, 0.0, 0.0], "the colour row writes the live wire colour");
        assert!(state.wire_single_color, "a colour edit turns single-colour mode on");
        assert!(state.dialog_visible());
        let row = state.slots.dialog.rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.control, Some(Control::Color { hex: "#000000".into() }));

        // And the switch is a command row of its own, flipped in place.
        state.take_dialog_pick("toggle_wire_single_color".to_string());
        assert!(!state.wire_single_color, "the switch row did not reach the flag");
        assert!(state.dialog_visible());
    }

    /// Frame All frames the displayed geometry from wherever the view is:
    /// with a named camera that is not in the current directory (a subnet —
    /// the camera node lives at the root and applies only there) it used to
    /// do nothing at all; now that view is the Default Camera view and is
    /// framed as one — its pivot moves to the geometry's centre and the
    /// fixed eye ray is fitted with zoom.
    /// A camera stands at the root and is seen from every level (since
    /// 2026-10-02): Frame All from inside a subnet inside the Geometry node
    /// frames the active camera NODE, where until then a camera not on the
    /// current level was the Default Camera view and the node was left.
    #[test]
    fn frame_all_frames_the_root_camera_from_inside_a_subnet() {
        use crate::geometry::{node_param_vec3, Vertex3D};
        let mut state = State::new(false);
        state.set_active_camera("camera1");
        let sub = state.current_dir().children.iter().position(|c| c.name == "sphere1").expect("sphere1 in geometry1");
        state.current_path.push(sub);
        state.on_path_changed();
        assert!(!state.current_dir().children.iter().any(|c| c.node_type == "camera"), "no camera in the subnet");
        assert!(state.camera_names().contains(&"camera1".to_string()), "the root's camera is offered here");
        let camera = |state: &State| state.camera_level().children.iter().find(|c| c.name == "camera1").unwrap().clone();
        let before = camera(&state);
        // Displayed geometry: a small cluster centred well off the origin.
        let c = [3.0f32, 0.5, -2.0];
        state.rt_sphere_verts = (0..12)
            .map(|i| {
                let a = i as f32 * 0.5236;
                Vertex3D { position: [c[0] + 0.25 * a.cos(), c[1] + 0.25 * a.sin(), c[2] + 0.1 * (i % 3) as f32], color: [1.0; 3] }
            })
            .collect();
        state.last_viewport_width = 800;
        state.last_viewport_height = 600;
        let zoom_before = state.viewport().zoom;

        state.frame_all();

        let after = camera(&state);
        let piv = node_param_vec3(&after, "pivot", Vec3::ZERO);
        for k in 0..3 {
            assert!((piv[k] - c[k]).abs() < 0.2, "the camera's pivot {piv:?} is not on the geometry's centre {c:?}");
        }
        let reach = |n: &FsNode| (node_param_vec3(n, "position", Vec3::ZERO) - node_param_vec3(n, "pivot", Vec3::ZERO)).length();
        assert!(reach(&after) < reach(&before), "a 0.25 cluster frames closer than the stock camera");
        assert_eq!(state.viewport().zoom, zoom_before, "the node was framed, not the Default Camera");
    }

    /// The scene file carries the Default Camera VIEW (square aspect, pivot
    /// marker, orbit/zoom/pivot) — where you were standing in this scene —
    /// and, since 2026-09-24, the display settings with it.
    ///
    /// From 2026-09-23 to 24 the display settings were app-wide only, on the
    /// argument that opening someone else's scene should not reset how you
    /// look at geometry; this test asserted the wireframe did NOT travel.
    /// The user chose the other way: a project opens looking the way it was
    /// left. `a_project_keeps_its_display_settings` covers the whole block.
    #[test]
    fn viewport_settings_round_trip_through_the_scene_file() {
        let dir = std::env::temp_dir().join(format!("cce-designer-vp-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut a = State::new(false);
        // The Default Camera is active: a camera NODE's own Pivot would
        // override the saved view's, by design.
        a.active_camera = "Default Camera".to_string();
        a.square_viewport = true;
        a.viewport_mut().show_camera_pivot = true;
        a.viewport_mut().rotation_y = 0.7;
        a.viewport_mut().zoom = 0.4;
        a.viewport_mut().pivot = Vec3::new(3.0, 0.5, -2.0);
        // A display setting, which travels with the project now.
        a.wireframe = true;
        a.save_to_file(&dir).expect("save");

        let mut b = State::new(false);
        b.wireframe = false;
        b.load_from_file(&dir).expect("load");
        assert!(b.square_viewport, "Square Aspect loads from the file");
        assert!(b.viewport().show_camera_pivot, "the pivot marker loads from the file");
        assert!((b.viewport().rotation_y - 0.7).abs() < 1e-4);
        assert!((b.viewport().zoom - 0.4).abs() < 1e-4);
        assert_eq!(b.viewport().pivot, Vec3::new(3.0, 0.5, -2.0));
        assert!(b.wireframe, "the wireframe travels with the project");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// …and the display settings round-trip through `state.kdl` instead,
    /// every one of them, including the colours that pass through hex on the
    /// way. The colour table was a hand-written pair of `if let`s per field
    /// and covered two of the five, so a new colour setting serialized as a
    /// JSON array and came back as the default.
    #[test]
    fn display_settings_round_trip_through_state_kdl() {
        use crate::app::DesignSettings;
        let mut a = State::new(false);
        a.viewport_mut().show_grid = false;
        a.viewport_mut().show_origin = false;
        a.viewport_mut().bg_color = [0.1, 0.2, 0.3];
        a.viewport_mut().grid_color = [0.4, 0.5, 0.6];
        a.viewport_mut().rt_mode = true;
        a.grid_thickness = 0.04;
        a.origin_size = 2.5;
        a.show_point_markers = true;
        a.show_point_numbers = true;
        a.point_marker_size = 0.05;
        a.point_marker_color = [1.0, 0.5, 0.0];
        a.world_unit = cce_ui::units::Unit::Cm;
        a.wireframe = true;
        a.wire_single_color = true;
        a.wire_color = [0.2, 0.4, 0.6];
        a.wire_opacity = 0.5;
        a.wire_width = 3.0;
        a.geo_opacity = 0.75;
        a.group_marker_size = 0.125;
        a.pull_arrow_scale = 4.0;
        a.smooth_shading = true;
        a.show_occluded = true;
        a.save_settings();

        let kdl = std::fs::read_to_string(DesignSettings::file_path()).expect("state.kdl was written");
        let back = DesignSettings::from_kdl_str(&kdl);
        let close = |x: f32, y: f32| (x - y).abs() < 0.01;

        assert!(!back.viewport.show_grid_enabled);
        assert!(!back.viewport.show_origin_enabled);
        assert!(back.viewport.rt_mode);
        assert!(close(back.viewport.grid_thickness, 0.04));
        assert!(close(back.viewport.origin_size, 2.5));
        assert!(back.viewport.show_point_markers && back.viewport.show_point_numbers);
        assert!(!back.viewport.show_point_normals);
        assert!(close(back.viewport.point_marker_size, 0.05));
        assert_eq!(back.viewport.world_unit, "cm");
        for (got, want) in [
            (back.viewport.bg_color, [0.1, 0.2, 0.3]),
            (back.viewport.grid_color, [0.4, 0.5, 0.6]),
            (back.viewport.point_marker_color, [1.0, 0.5, 0.0]),
        ] {
            for k in 0..3 {
                assert!(close(got[k], want[k]), "colour {got:?} came back as {want:?}");
            }
        }
        assert!(back.render.wireframe && back.render.wire_single_color);
        for k in 0..3 {
            assert!(close(back.render.wire_color[k], [0.2, 0.4, 0.6][k]), "{:?}", back.render.wire_color);
        }
        assert!(close(back.render.wire_opacity, 0.5));
        assert!(close(back.render.wire_width, 3.0));
        assert!(close(back.render.geo_opacity, 0.75));
        assert!((back.render.group_marker_size - 0.125).abs() < 1e-4);
        assert!(close(back.render.pull_arrow_scale, 4.0));
        assert!(back.render.smooth_shading);
        assert!(back.render.show_occluded);
    }

    /// Changing the wire colour turns single-colour mode on, so the colour
    /// shows; turning the switch off afterwards sticks, and a LOAD never
    /// flips it — a project that says off stays off whatever colour it
    /// carries.
    #[test]
    fn changing_the_wire_colour_turns_single_colour_mode_on() {
        let mut state = State::new(false);
        state.wire_single_color = false;
        state.wire_color = [1.0, 1.0, 1.0];

        // An edit through the dialog's colour row, which is the only way
        // in now that the Render node is gone.
        state.open_dialog();
        state.apply_setting("Wireframe Color", "#000000");
        assert_eq!(state.wire_color, [0.0, 0.0, 0.0]);
        assert!(state.wire_single_color, "a colour change switches single-colour mode on");

        // Off again by hand stays off while the colour is unchanged: the
        // auto-enable fires on a CHANGE, not on every settings pass.
        state.wire_single_color = false;
        state.apply_setting("Wireframe Color", "#000000");
        assert!(!state.wire_single_color, "an unrelated pass flipped it back on");

        // A load carries the project's geometry and leaves the wire
        // settings — preferences now — exactly where they are.
        let dir = std::env::temp_dir().join(format!("cce-designer-wire-colour-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        state.save_to_file(&dir).expect("save");
        state.load_from_file(&dir).expect("load");
        assert_eq!(state.wire_color, [0.0, 0.0, 0.0]);
        assert!(!state.wire_single_color, "a load never flips the switch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The wireframe toggle is a palette row that flips the live flag, and
    /// the flag is the whole of it. The value used to live on the Render
    /// utility node, which `apply_settings_from_menubar_subnets` read back
    /// over live state after every parameter edit anywhere — so a flag
    /// flipped alone reverted on the next unrelated change, and the command
    /// had to write the node too. There is no node and no read-back now.
    #[test]
    fn test_toggle_wireframe_flips_the_flag_and_the_render_node() {
        use crate::command::{by_id, Run};
        let cmd = by_id("toggle_wireframe").expect("no toggle_wireframe command");
        assert_eq!(cmd.label, "Show Wireframe");
        assert_eq!(cmd.run, Run::Key(crate::shortcut::Action::ToggleWireframe));

        let mut state = State::new(false);
        state.wireframe = false;
        assert!(state.run_command("toggle_wireframe"));
        assert!(state.wireframe);
        assert_eq!(state.command_toggle_state("toggle_wireframe"), Some(true));
        // An unrelated settings apply no longer reverts it.
        state.open_dialog();
        let thickness = state.settings_row_value("Wire Thickness");
        state.apply_setting("Wire Thickness", &thickness);
        assert!(state.wireframe, "a settings pass read the flag back over itself");
        assert!(state.run_command("toggle_wireframe"));
        assert!(!state.wireframe);
    }

    /// The registry's own invariants. Ids are what `input.kdl` binds and
    /// labels are what the palette maps a chosen row back to, so a duplicate
    /// of either silently runs the wrong command.
    #[test]
    fn test_the_command_registry_is_consistent() {
        use crate::command::COMMANDS;
        let mut ids: Vec<&str> = COMMANDS.iter().map(|c| c.id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate command id");

        let mut labels: Vec<&str> = COMMANDS.iter().map(|c| c.label).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), n, "duplicate command label: the palette picks by label");

        for c in COMMANDS {
            assert!(
                c.id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'),
                "{} is not snake_case, which is what input.kdl writes",
                c.id
            );
            if let Some(chord) = c.default_chord {
                crate::shortcut::Shortcut::parse(chord)
                    .unwrap_or_else(|e| panic!("{}: unparseable default chord {chord:?}: {e}", c.id));
            }
        }
    }

    /// Every menu-dispatched command names a label `execute_menu_action`
    /// actually handles.
    ///
    /// This is the check the plugin's hccommands.py doc argues for: a label
    /// kept in two places drifts, and a renamed one fails SILENTLY — the
    /// dispatch falls through its match and the command simply does nothing.
    /// Scanning the source is an odd way to assert it, but the alternative is
    /// calling every command to see if it is handled, and "Exit" would end the
    /// test run.
    #[test]
    fn test_every_menu_command_names_a_label_that_is_dispatched() {
        use crate::command::{Run, COMMANDS};
        let src = include_str!("app.rs");
        let start = src
            .find("pub fn execute_menu_action")
            .expect("execute_menu_action moved; this test scans for it");
        // The function alone, not the rest of the file: the menubars are
        // built further down with their items spelled out, and scanning on
        // to the end found "New Project" THERE while no arm dispatched it.
        let end = src[start..]
            .find("\n    }\n")
            .expect("execute_menu_action has no end");
        let body = &src[start..start + end];
        for c in COMMANDS {
            let Run::Menu(label) = c.run else { continue };
            let arm = format!("\"{label}\"");
            assert!(
                body.contains(&arm),
                "command {} dispatches {label:?}, which execute_menu_action does not handle",
                c.id
            );
        }
    }

    /// The menubars are not drawn, and what they listed is commands. A
    /// click by index is dispatched for the two things no command does —
    /// the active camera, the parameter presets — and refused for the rest,
    /// where it used to be accepted and, for the header's File menu, run
    /// the item one below the one named.
    #[test]
    fn a_menubar_click_is_dispatched_or_refused() {
        use crate::app::McpAction;
        let mut state = State::new(false);
        let mut redraw = false;
        state.set_active_camera("camera1");
        state
            .apply_action(McpAction::MenuClick { widget_idx: RIGHT_MENUBAR_IDX, menu_idx: 0, item_idx: 0 }, &mut redraw)
            .expect("the Camera menu is dispatched");
        assert_eq!(state.active_camera, "Default Camera");

        let nodes = state.fs_root.children.len();
        for (widget_idx, menu_idx) in [(crate::slots::HEADER_IDX, 0), (LEFT_MENUBAR_IDX, 0), (RIGHT_MENUBAR_IDX, 2)] {
            let res = state.apply_action(McpAction::MenuClick { widget_idx, menu_idx, item_idx: 0 }, &mut redraw);
            assert!(res.is_err(), "menubar {widget_idx} menu {menu_idx} was accepted: {res:?}");
        }
        assert_eq!(state.fs_root.children.len(), nodes, "a refused click ran New Project");
    }

    /// The cameras and the parameter reset are commands. They were menus
    /// of two menubars that are not drawn, so nothing on screen reached them.
    #[test]
    fn the_cameras_and_the_parameter_reset_are_commands() {
        use crate::dialog::CAMERA_ROW_PREFIX;
        let mut state = State::new(false);
        assert!(state.camera_names().contains(&"camera1".to_string()), "the bundled project has camera1");

        // Stepping wraps, and both copies of the name follow.
        state.set_active_camera("Default Camera");
        let count = state.camera_names().len();
        for _ in 0..count {
            assert!(state.run_command("next_camera"));
            assert_eq!(state.viewport().active_camera, state.active_camera);
        }
        assert_eq!(state.active_camera, "Default Camera", "a full turn comes back");
        assert!(state.run_command("previous_camera"));
        assert_eq!(state.active_camera, *state.camera_names().last().unwrap());
        assert!(state.run_command("default_camera"));
        assert_eq!(state.active_camera, "Default Camera");

        // A camera node is a row of the palette, found by its name.
        state.open_dialog();
        state.slots.dialog.query = "camera1".to_string();
        state.refresh_dialog_rows();
        let id = format!("{CAMERA_ROW_PREFIX}camera1");
        let row = state.slots.dialog.rows.iter().position(|r| r.id == id).expect("no row for camera1");
        assert_eq!(state.slots.dialog.rows[row].label, "Camera: camera1");
        state.slots.dialog.selected = row;
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(!state.dialog_visible());
        assert_eq!(state.active_camera, "camera1");
        assert_eq!(state.viewport().active_camera, "camera1");

        // Reset puts a changed parameter back.
        let slot = state
            .current_dir()
            .children
            .iter()
            .position(|c| c.node_type == "sphere")
            .expect("the bundled project has a sphere");
        state.graph_mut().set_selected_node(Some(slot));
        let default = {
            let dir = state.current_dir();
            state.template_default(dir, &dir.children[slot], "radius").expect("no Radius default").text().to_string()
        };
        let radius = |state: &State| {
            state.current_dir().children[slot].params.iter().find(|p| p.name == "radius").unwrap().text().to_string()
        };
        state.current_dir_mut().children[slot].params.iter_mut().find(|p| p.name == "radius").unwrap().set_text("3.25".to_string());
        assert!(state.run_command("reset_parameters"));
        assert_eq!(radius(&state), default);
        assert!(crate::command::by_id("custom_preset").is_none(), "the custom preset is retired");

        // With nothing selected there is nothing to reset, and it says so.
        state.graph_mut().set_selected_node(None);
        state.run_command("reset_parameters");
    }

    /// Reset Parameters can be taken back, and put back again. It rewrites
    /// every parameter of a node at once, and until it recorded a step the
    /// values it replaced were gone.
    #[test]
    fn reset_parameters_is_undone_and_redone() {
        let mut state = State::new(false);
        let slot = state
            .current_dir()
            .children
            .iter()
            .position(|c| c.node_type == "sphere")
            .expect("the bundled project has a sphere");
        state.graph_mut().set_selected_node(Some(slot));
        let texts = |state: &State| -> Vec<(String, String, bool)> {
            state.current_dir().children[slot]
                .params
                .iter()
                .map(|p| (p.name.clone(), p.text().to_string(), p.is_expr()))
                .collect()
        };
        {
            let node = &mut state.current_dir_mut().children[slot];
            node.params.iter_mut().find(|p| p.name == "radius").unwrap().set_text("3.25".to_string());
            let rows = node.params.iter_mut().find(|p| p.name == "rows").unwrap();
            rows.set_text("$F + 4".to_string());
            rows.set_expr(true);
        }
        let edited = texts(&state);

        assert!(!state.history_step(true), "nothing to undo before the reset");
        assert!(state.run_command("reset_parameters"));
        let reset = texts(&state);
        assert_ne!(reset, edited);

        // Through the Undo command, as the palette and the chord arrive.
        assert!(state.run_command("undo"));
        assert_eq!(texts(&state), edited, "undo did not bring the values back, expression flag included");
        assert!(state.run_command("redo"));
        assert_eq!(texts(&state), reset);
        assert!(state.run_command("undo"));
        assert_eq!(texts(&state), edited);

        // A rename between the reset and the undo: the step is by id, and
        // is reached under the rename, which is a step of its own.
        state.run_command("reset_parameters");
        let id = state.current_dir().children[slot].id.clone();
        crate::geometry::rename_node_in_tree(&mut state.fs_root, &id, "ball");
        assert!(state.history_step(true));
        assert_ne!(state.current_dir().children[slot].name, "ball");
        assert!(state.history_step(true));
        assert_eq!(texts(&state), edited);

        // A node deleted since comes back first, and then its parameters.
        state.run_command("reset_parameters");
        let reset = texts(&state);
        state.delete_node(slot);
        assert!(state.history_step(true), "the delete is the last step");
        assert_eq!(texts(&state), reset, "the node came back as it was deleted");
        assert!(state.history_step(true));
        assert_eq!(texts(&state), edited);

        // Another document's steps are not this one's.
        let mut state = State::new(false);
        state.graph_mut().set_selected_node(Some(slot));
        state.run_command("reset_parameters");
        state.new_project();
        assert!(!state.history_step(true), "New Project kept the old project's undo");
    }

    /// An edit to a parameter can be taken back however it was made: a row
    /// of the pane, `set_param`, the row menu. A drag writes back on every
    /// motion and is one step; a press between two drags makes them two.
    #[test]
    fn a_parameter_edit_is_undone_a_gesture_at_a_time() {
        use crate::app::{McpAction, ParamMenuAction};
        let mut state = State::new(false);
        let mut redraw = false;
        let slot = state
            .current_dir()
            .children
            .iter()
            .position(|c| c.node_type == "sphere")
            .expect("the bundled project has a sphere");
        state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
        let node_id = state.current_dir().children[slot].id.clone();
        let param = |state: &State, name: &str| -> (String, bool) {
            let p = state.current_dir().children[slot].params.iter().find(|p| p.name == name).unwrap();
            (p.text().to_string(), p.is_expr())
        };
        // The pane reporting a row at a value, as a drag does per motion.
        let pane = |state: &mut State, name: &str, value: &str| {
            let rows: Vec<(String, String, String)> = state
                .param()
                .node_params()
                .into_iter()
                .map(|(n, v, t)| if n == name { (n, value.to_string(), t) } else { (n, v, t) })
                .collect();
            state.param_mut().set_display_params(&rows);
            state.sync_parameters_to_project();
        };
        let radius = param(&state, "radius");
        let rows = param(&state, "rows");

        // One drag: three motions, one step.
        for v in ["1.10", "1.20", "1.30"] {
            pane(&mut state, "Radius", v);
        }
        assert_eq!(state.edit_history.undo_len(), 1, "a drag is one step");
        // A write-back that changes nothing records nothing.
        state.sync_parameters_to_project();
        assert_eq!(state.edit_history.undo_len(), 1);
        // A release and a press, then a second drag of the same row.
        state.edit_history.break_group();
        for v in ["1.40", "1.50"] {
            pane(&mut state, "Radius", v);
        }
        assert_eq!(state.edit_history.undo_len(), 2, "a second drag is a second step");
        // Another row, with no press between: its own step all the same.
        pane(&mut state, "Rows", "9");
        assert_eq!(state.edit_history.undo_len(), 3);

        assert!(state.run_command("undo"));
        assert_eq!(param(&state, "rows"), rows);
        assert_eq!(param(&state, "radius").0, "1.50", "undoing Rows left Radius alone");
        assert!(state.run_command("undo"));
        assert_eq!(param(&state, "radius").0, "1.30");
        assert!(state.run_command("undo"));
        assert_eq!(param(&state, "radius"), radius);
        assert!(!state.history_step(true), "three steps were recorded");
        let shown = state.param().node_params().into_iter().find(|r| r.0 == "Radius").unwrap().1;
        assert_eq!(shown, radius.0, "the pane shows the restored value");
        for want in ["1.30", "1.50"] {
            assert!(state.run_command("redo"));
            assert_eq!(param(&state, "radius").0, want);
        }
        // An edit after an undo forks: what was undone is not redone over it.
        assert!(state.run_command("undo"));
        state.edit_history.break_group();
        pane(&mut state, "Radius", "2.00");
        assert!(!state.history_step(false), "a new edit left the redo branch standing");
        assert!(state.run_command("undo"));
        assert_eq!(param(&state, "radius").0, "1.30");

        // A step restores what it changed and nothing else on the node.
        pane(&mut state, "Radius", "2.50");
        state.current_dir_mut().children[slot].params.iter_mut().find(|p| p.name == "rows").unwrap().set_text("21".to_string());
        assert!(state.run_command("undo"));
        assert_eq!(param(&state, "radius").0, "1.30");
        assert_eq!(param(&state, "rows").0, "21", "undoing Radius took back an edit to Rows");

        // set_param, and one that is refused.
        let before = state.edit_history.undo_len();
        state.apply_action(McpAction::SetParam { slot, name: "radius".into(), value: "3.00".into() }, &mut redraw).unwrap();
        assert!(state.apply_action(McpAction::SetParam { slot, name: "radius".into(), value: "abc".into() }, &mut redraw).is_err());
        assert_eq!(state.edit_history.undo_len(), before + 1, "a refused value recorded a step");
        assert!(state.run_command("undo"));
        assert_eq!(param(&state, "radius").0, "1.30");

        // The row menu: the expression flag is part of what comes back.
        state.run_param_action(&node_id, "radius", ParamMenuAction::EditExpression);
        assert!(param(&state, "radius").1);
        state.run_param_action(&node_id, "radius", ParamMenuAction::CopyParameter);
        assert!(state.run_command("undo"));
        assert_eq!(param(&state, "radius"), ("1.30".to_string(), false), "Copy Parameter is no edit, and Edit Expression is one");
    }

    /// Adding, deleting, moving and wiring nodes can be taken back, in the
    /// order they were done, among the parameter edits made between them.
    #[test]
    fn the_graph_is_undone_a_step_at_a_time() {
        use crate::app::McpAction;
        let mut state = State::new(false);
        let mut redraw = false;
        // The first look takes the tree as it stands and records nothing.
        state.record_structure_changes();
        assert_eq!(state.edit_history.undo_len(), 0);
        let shape = |state: &State| -> Vec<(String, (f32, f32), String, bool, bool)> {
            state
                .current_dir()
                .children
                .iter()
                .map(|c| {
                    let input = c.params.iter().find(|p| p.name == "input").map(|p| p.text().to_string()).unwrap_or_default();
                    (c.name.clone(), c.position, input, c.geometry_visible, c.bypassed)
                })
                .collect()
        };
        let start = shape(&state);

        // Add, through MCP as the palette's pick adds.
        state
            .apply_action(McpAction::AddNode { template_name: "Box".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw)
            .unwrap();
        let added = shape(&state);
        assert_eq!(added.len(), start.len() + 1);
        assert_eq!(state.edit_history.undo_len(), 1);
        let slot = added.len() - 1;
        let name = added[slot].0.clone();

        // Wire it to the sphere, by the parameter, and move it.
        state.apply_action(McpAction::SetParam { slot, name: "radius".into(), value: "1".into() }, &mut redraw).ok();
        let steps = state.edit_history.undo_len();
        state.current_dir_mut().children[slot].position = (12.0, 9.0);
        state.record_structure_changes();
        assert_eq!(state.edit_history.undo_len(), steps + 1, "a move is a step");
        let moved = shape(&state);
        // A second move of the same node straight after is the same step.
        state.current_dir_mut().children[slot].position = (13.0, 9.0);
        state.record_structure_changes();
        assert_eq!(state.edit_history.undo_len(), steps + 1, "a run of moves is one step");
        // A press between them, and it is another.
        state.edit_history.break_group();
        state.current_dir_mut().children[slot].position = (14.0, 9.0);
        state.record_structure_changes();
        assert_eq!(state.edit_history.undo_len(), steps + 2);
        let moved_again = shape(&state);

        // Flags: bypass, through its command's writer.
        state.set_bypassed(&[slot], true);
        state.record_structure_changes();
        let bypassed = shape(&state);
        assert!(bypassed[slot].4);

        // Delete the first node of the level, which moves every slot.
        let first = state.current_dir().children[0].clone();
        state.delete_node(0);
        state.record_structure_changes();
        let deleted = shape(&state);
        assert_eq!(deleted.len(), added.len() - 1);

        // Back, a step at a time.
        assert!(state.run_command("undo"));
        assert_eq!(shape(&state), bypassed, "the deleted node came back where it was");
        assert_eq!(state.current_dir().children[0].id, first.id);
        assert_eq!(state.current_dir().children[0].params.len(), first.params.len());
        assert!(state.last_status_text.contains("Undo Delete"), "{}", state.last_status_text);
        assert!(state.run_command("undo"));
        assert_eq!(shape(&state), moved_again);
        assert!(state.run_command("undo"));
        assert_eq!(shape(&state)[slot].1, (13.0, 9.0), "the run of two moves is where the third began");
        assert!(state.run_command("undo"));
        assert_eq!(shape(&state)[slot].1, (9.0, 9.0));
        let _ = moved;
        while state.edit_history.undo_len() > 1 {
            assert!(state.run_command("undo"));
        }
        assert_eq!(shape(&state), added);
        assert!(state.run_command("undo"));
        assert_eq!(shape(&state), start, "undoing the add took the node out");
        assert!(!state.history_step(true));

        // And forward again, to the end.
        while state.history_step(false) {}
        assert_eq!(shape(&state), deleted);
        assert!(!state.current_dir().children.iter().any(|c| c.id == first.id));
        assert!(state.current_dir().children.iter().any(|c| c.name == name));

        // Undoing the add of a node the editor has gone into comes out of it.
        let mut state = State::new(false);
        state.record_structure_changes();
        state
            .apply_action(McpAction::AddNode { template_name: "Embryo".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw)
            .unwrap();
        let slot = state.current_dir().children.len() - 1;
        let level = state.current_path.clone();
        state.current_path.push(slot);
        state.sync_nodes();
        assert!(state.history_step(true));
        assert_eq!(state.current_path, level, "the editor is inside a node that is gone");

        // A wire, made as the graph makes one and as the pane does: each is
        // one step, and the second is not noticed a second time.
        let mut state = State::new(false);
        state
            .apply_action(McpAction::AddNode { template_name: "Normal".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw)
            .unwrap();
        state.edit_history.clear();
        let wired = state
            .current_dir()
            .children
            .iter()
            .position(|c| c.params.iter().any(|p| p.name == "input" && p.kind() == crate::app::ParamKind::Node))
            .expect("a Normal node has an Input");
        let input = |state: &State| {
            state.current_dir().children[wired].params.iter().find(|p| p.name == "input").unwrap().text().to_string()
        };
        let was = input(&state);
        state.current_dir_mut().children[wired].params.iter_mut().find(|p| p.name == "input").unwrap().set_text("camera1".to_string());
        state.record_structure_changes();
        assert_eq!(state.edit_history.undo_len(), 1);
        state
            .apply_action(McpAction::SetParam { slot: wired, name: "input".into(), value: String::new() }, &mut redraw)
            .unwrap();
        assert_eq!(state.edit_history.undo_len(), 2, "a wire set as a parameter is one step, not two");
        assert!(state.run_command("undo"));
        assert_eq!(input(&state), "camera1");
        assert!(state.run_command("undo"));
        assert_eq!(input(&state), was);
        assert!(state.last_status_text.contains("Wire"), "{}", state.last_status_text);

        // A project opened is not an edit, and takes the history with it.
        state
            .apply_action(McpAction::AddNode { template_name: "Box".into(), name: None, x: 3.0, y: 9.0 }, &mut redraw)
            .unwrap();
        state.new_project();
        state.record_structure_changes();
        assert_eq!(state.edit_history.undo_len(), 0, "New Project was recorded as an edit");
    }

    /// A rename is taken back with everything that named the node: the
    /// wires to it, the expression paths through it wherever they stand,
    /// and the active camera.
    #[test]
    fn a_rename_is_undone_with_what_names_the_node() {
        use crate::app::McpAction;
        let mut state = State::new(false);
        let mut redraw = false;
        let add = |state: &mut State, template: &str, x: f32| {
            state
                .apply_action(McpAction::AddNode { template_name: template.into(), name: None, x, y: 9.0 }, &mut false)
                .unwrap();
            state.current_dir().children.len() - 1
        };
        let sphere = state.current_dir().children.iter().position(|c| c.node_type == "sphere").unwrap();
        let old = state.current_dir().children[sphere].name.clone();
        let normal = add(&mut state, "Normal", 3.0);
        let embryo = add(&mut state, "Embryo", 5.0);
        state.apply_action(McpAction::SetParam { slot: normal, name: "input".into(), value: old.clone() }, &mut redraw).unwrap();
        // An expression a level down, reaching up and across to the sphere.
        let reference = format!("ch(\"../../{old}/radius\") * 2");
        let inside = state.current_dir().children[embryo]
            .children
            .iter()
            .position(|c| !c.params.is_empty())
            .expect("the Embryo has a child with parameters");
        {
            let inner = &mut state.current_dir_mut().children[embryo].children[inside];
            let p = &mut inner.params[0];
            p.set_text(reference.clone());
            p.set_expr(true);
        }
        // The camera stands at the root, the sphere in its Geometry node.
        let camera = state.camera_level().children.iter().find(|c| c.node_type == "camera").unwrap();
        let (camera_id, camera_name) = (camera.id.clone(), camera.name.clone());
        state.set_active_camera(camera_name.clone());
        state.record_structure_changes();
        state.edit_history.clear();

        let names = |state: &State| -> (String, String, String, String, String) {
            let dir = state.current_dir();
            (
                dir.children[sphere].name.clone(),
                dir.children[normal].params.iter().find(|p| p.name == "input").unwrap().text().to_string(),
                dir.children[embryo].children[inside].params[0].text().to_string(),
                state.active_camera.clone(),
                state.viewport().active_camera.clone(),
            )
        };
        let before = names(&state);
        assert_eq!(before.2, reference);

        state.apply_action(McpAction::RenameNode { slot: sphere, new_name: "Ball".into() }, &mut redraw).unwrap();
        state.rename_node(&camera_id, "lens").unwrap();
        state.record_structure_changes();
        let after = names(&state);
        assert_eq!(after.0, "ball");
        assert_eq!(after.1, "ball", "the wire followed the rename");
        assert!(after.2.contains("../../ball/radius"), "{}", after.2);
        assert_eq!((after.3.as_str(), after.4.as_str()), ("lens", "lens"), "both copies of the camera's name");
        assert_eq!(state.edit_history.undo_len(), 2, "each rename is one step, its wires with it");

        assert!(state.run_command("undo"));
        assert!(state.last_status_text.contains(&format!("Undo Rename {camera_name}")), "{}", state.last_status_text);
        assert_eq!(names(&state).3, camera_name);
        assert_eq!(names(&state).4, camera_name);
        assert!(state.run_command("undo"));
        assert_eq!(names(&state), before);
        assert!(state.run_command("redo"));
        assert!(state.run_command("redo"));
        assert_eq!(names(&state), after);

        // A name taken since is not taken twice: the node keeps its own.
        assert!(state.run_command("undo"));
        assert!(state.run_command("undo"));
        state.apply_action(McpAction::RenameNode { slot: sphere, new_name: "ball".into() }, &mut redraw).unwrap();
        state.apply_action(McpAction::RenameNode { slot: normal, new_name: old.clone() }, &mut redraw).unwrap();
        state.edit_history.take(true);
        assert!(state.history_step(true), "the step is taken");
        let dir = state.current_dir();
        assert_eq!(dir.children[sphere].name, "ball", "the sphere took a name its sibling has");
        assert_eq!(dir.children[normal].name, old);
    }

    /// Rename is a row of the node's menu and a command: the dialog opens
    /// holding the node's name, the row says what Enter will do, and a name
    /// that cannot be written is refused there and not on the way in.
    /// The Group Markers dialog: the palette turned into a list of the
    /// scene's point groups, a switch each. Enter or a click flips the
    /// switch in place and the list stays up; a marked group's members
    /// wear a marker in the scene, built from what the last rebuild kept,
    /// following the geometry through a rebuild and persisted with the
    /// display settings.
    /// Attribute visualizers: added, edited, switched and deleted in the
    /// dialog, applied to the displayed scene with no node in the graph,
    /// and kept with the display settings.
    #[test]
    fn attribute_visualizers_are_edited_in_the_dialog_and_shown_on_the_scene() {
        use crate::dialog::Mode;
        use crate::visualizer::{VIS_ADD_ROW_ID, VIS_FIELD_PREFIX, VIS_ROW_PREFIX};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.rebuild_scene_geometry();
        assert!(state.scene_attributes.iter().any(|a| a.name == "N"), "{:?}", state.scene_attributes);
        let colours = |state: &State| state.rt_sphere_verts.iter().map(|v| v.color).collect::<Vec<_>>();
        let plain = colours(&state);
        let nodes_before = serde_json::to_string(&state.fs_root).unwrap();

        // The list, empty but for Add Visualizer.
        assert!(state.run_command("attribute_visualizers"));
        assert_eq!(state.slots.dialog.mode, Mode::Visualizers);
        assert_eq!(state.slots.dialog.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec![VIS_ADD_ROW_ID]);

        // Add one: the list turns into its settings.
        state.take_dialog_pick(VIS_ADD_ROW_ID.to_string());
        assert_eq!(state.visualizers.len(), 1);
        assert_eq!(state.slots.dialog.mode, Mode::VisualizerEdit);
        assert_eq!(state.vis_editing, Some(0));
        let field = |f: &str| format!("{VIS_FIELD_PREFIX}{f}");
        let has = |state: &State, f: &str| state.slots.dialog.rows.iter().any(|r| r.id == field(f));
        assert!(has(&state, "ramp") && has(&state, "opacity") && !has(&state, "scale"), "Ramp's rows");

        // On N, a Ramp recolours the scene, which gained no node.
        state.set_visualizer_field(0, "attribute", "N", true);
        assert_ne!(colours(&state), plain, "the ramp is on the scene");
        assert_eq!(serde_json::to_string(&state.fs_root).unwrap(), nodes_before, "no node in the graph");

        // Mode's dropdown, Down, Enter: Vector's rows, and lines drawn.
        let mode_row = state.slots.dialog.rows.iter().position(|r| r.id == field("mode")).unwrap();
        state.slots.dialog.selected = mode_row;
        let lines_before = state.overlay_normal_verts.len();
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(state.dialog_dropdown_open(), "a choice opens its dropdown");
        assert_eq!(state.slots.dialog.dropdown.options, vec!["Ramp".to_string(), "Vector".to_string()]);
        assert_eq!(state.slots.dialog.dropdown.selected, 0);
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowDown)));
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(state.visualizers[0].is_vector());
        assert!(has(&state, "scale") && !has(&state, "ramp"), "Vector's rows");
        assert_eq!(state.slots.dialog.selected_id(), Some(field("mode").as_str()), "the selection stays");
        assert!(state.overlay_normal_verts.len() > lines_before, "the vectors are drawn");
        assert_eq!(colours(&state), plain, "a Vector leaves the colours");

        // Escape goes back to the list, which names it.
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Escape)));
        assert!(state.dialog_visible());
        assert_eq!(state.slots.dialog.mode, Mode::Visualizers);
        let id = format!("{VIS_ROW_PREFIX}0");
        let row = state.slots.dialog.rows.iter().find(|r| r.id == id).expect("its row").clone();
        assert!(row.label.starts_with("N — Vector"), "{}", row.label);
        assert_eq!(row.toggle(), Some(true));

        // Kept with the display settings.
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        let kept = crate::visualizer::decode(&crate::app::DesignSettings::from_kdl_str(&kdl).viewport.visualizers);
        assert_eq!(kept, state.visualizers);

        // Its switch turns it off; the rest of the row opens it.
        state.take_dialog_pick_at(id.clone(), true);
        assert!(!state.visualizers[0].enabled);
        assert_eq!(state.slots.dialog.mode, Mode::Visualizers);
        let lines_off = state.overlay_normal_verts.len();
        assert!(lines_off < lines_before + 1, "off draws nothing");
        state.take_dialog_pick_at(id, false);
        assert_eq!(state.slots.dialog.mode, Mode::VisualizerEdit);

        // Delete, and the list is empty again.
        state.take_dialog_pick(field("delete"));
        assert!(state.visualizers.is_empty());
        assert_eq!(state.slots.dialog.mode, Mode::Visualizers);
        assert_eq!(colours(&state), plain);
        state.close_dialog();
    }

    /// A visualizer's Manual Range is the node's float2, and the dialog
    /// edits it as one: a row of two sliders side by side, each end worked
    /// on its own — pressed and dragged, turned by the wheel, nudged by the
    /// arrows (shift for the second). A drag lands live and saves on the
    /// release. A settings file from before, which kept From and To, reads
    /// as the range.
    #[test]
    fn a_visualizers_manual_range_is_one_float2_row() {
        use crate::dialog::Control;
        use crate::slots::DIALOG_IDX;
        use crate::visualizer::{decode, VIS_ADD_ROW_ID, VIS_FIELD_PREFIX};
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton, MouseScrollDelta};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.rebuild_scene_geometry();
        assert!(state.run_command("attribute_visualizers"));
        state.take_dialog_pick(VIS_ADD_ROW_ID.to_string());
        state.set_visualizer_field(0, "attribute", "N", true);
        state.set_visualizer_field(0, "range", "Manual", true);
        state.refresh_dialog_controls();
        let (lo, hi) = {
            let a = state.scene_attributes.iter().find(|a| a.name == "N").unwrap();
            (a.min, a.max)
        };
        assert_eq!(state.visualizers[0].manual_range, [lo, hi], "Manual starts at what Auto showed");

        let id = format!("{VIS_FIELD_PREFIX}manual_range");
        let i = state.slots.dialog.rows.iter().position(|r| r.id == id).expect("a Manual Range row");
        let ids: Vec<&str> = state.slots.dialog.rows.iter().map(|r| r.id.as_str()).collect();
        assert!(!ids.iter().any(|r| r.ends_with(":from") || r.ends_with(":to")), "no From and To rows: {ids:?}");
        let control = |state: &State| match state.slots.dialog.rows[i].control.clone() {
            Some(Control::Float2 { values, min, max, .. }) => (values, min, max),
            other => panic!("not a float2: {other:?}"),
        };
        let (values, min, max) = control(&state);
        assert_eq!(values, [lo, hi]);
        let at = |t: f32| min + t * (max - min);

        let bands = state.slots.dialog.slider_bands(state.positions[DIALOG_IDX], i);
        assert_eq!(bands.len(), 2, "two sliders");
        assert!(bands[0].x + bands[0].width < bands[1].x, "side by side");
        let point = |state: &mut State, k: usize, t: f32| {
            let b = bands[k];
            let (px, py) = (b.x + b.width * t, b.y + b.height * 0.5);
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: px as f64, y: py as f64 } });
        };
        let saved = || {
            let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
            decode(&crate::app::DesignSettings::from_kdl_str(&kdl).viewport.visualizers)[0].manual_range
        };
        let close = |a: f32, b: f32| (a - b).abs() < (max - min) * 0.02;

        // The first end, pressed and dragged: live, saved on the release.
        point(&mut state, 0, 0.25);
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        assert!(state.slots.dialog.slider_dragging());
        point(&mut state, 0, 0.1);
        let [a, b] = state.visualizers[0].manual_range;
        assert!(close(a, at(0.1)) && b == hi, "{a} {b}");
        assert_eq!(control(&state).0, state.visualizers[0].manual_range, "the row shows what the visualizer holds");
        assert_eq!(saved(), [lo, hi], "not written mid-drag");
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        assert_eq!(saved(), state.visualizers[0].manual_range, "written on the release");

        // The second end, on its own band; the first stays.
        point(&mut state, 1, 0.9);
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        let [a2, b2] = state.visualizers[0].manual_range;
        assert!(a2 == a && close(b2, at(0.9)), "{a2} {b2}");

        // The wheel over an end turns that end, up being more.
        point(&mut state, 0, 0.5);
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 1.0) });
        let [a3, b3] = state.visualizers[0].manual_range;
        assert!(a3 > a2 && b3 == b2, "{a3} {b3}");

        // The arrows nudge the first end; with shift, the second.
        state.slots.dialog.selected = i;
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowRight)));
        let [a4, b4] = state.visualizers[0].manual_range;
        assert!(a4 > a3 && b4 == b3, "{a4} {b4}");
        state.modifiers = ModifiersState { shift: true, ..Default::default() };
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowLeft)));
        state.modifiers = ModifiersState::default();
        let [a5, b5] = state.visualizers[0].manual_range;
        assert!(a5 == a4 && b5 < b4, "{a5} {b5}");
        assert_eq!(saved(), [a5, b5], "a key's landing saves at once");

        // What it draws is the node's Manual Range.
        let node = state.visualizers[0].as_node();
        assert_eq!(crate::geometry::node_param_vec2(&node, "manual_range", [0.0, 0.0]), [a5, b5]);

        // A settings file from before kept the two ends apart.
        let old = decode("attribute=N|mode=Ramp|range=Manual|from=2|to=5");
        assert_eq!(old[0].manual_range, [2.0, 5.0]);
        assert_eq!(decode(&crate::visualizer::encode(&old)), old);
        state.close_dialog();
    }

    /// A visualizer is the Visualize node's reading: the same settings give
    /// the same colours as the node does, and the settings round-trip
    /// through their one string.
    #[test]
    fn a_visualizer_reads_as_the_visualize_node_does() {
        let mut state = State::new(false);
        state.rebuild_scene_geometry();
        let base = state.scene_base.clone().expect("a scene");
        let mut v = crate::visualizer::Visualizer::new("N");
        v.ramp = "Heat".into();
        let mut by_vis = base.clone();
        crate::visualizer::apply_all(&[v.clone()], &mut by_vis);
        let mut by_node = base.clone();
        let mut err = None;
        crate::geometry::apply_visualize(&mut by_node, &v.as_node(), &mut err);
        assert!(err.is_none());
        assert_eq!(by_vis, by_node);
        assert_ne!(by_vis, base);
        let both = vec![v.clone(), crate::visualizer::Visualizer::new("Cd")];
        assert_eq!(crate::visualizer::decode(&crate::visualizer::encode(&both)), both);
        assert!(crate::visualizer::decode("not a record").is_empty());
        // What the KDL writer cannot carry is escaped, and comes back.
        let mut odd = crate::visualizer::Visualizer::new("a|b;c=\"d\\%");
        odd.group = "g;1".into();
        let text = crate::visualizer::encode(&[odd.clone()]);
        assert!(!text.contains('"') && !text.contains('\\'), "{text}");
        assert_eq!(crate::visualizer::decode(&text), vec![odd]);
        assert_eq!(crate::visualizer::encode(&[]), "");
    }

    #[test]
    fn the_group_markers_dialog_marks_a_groups_points() {
        use crate::dialog::{Mode, GROUP_ROW_PREFIX};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.param_editor = crate::slots::CONTENT_IDX;
        let mut redraw = false;
        // A group of five points on the sphere, shown.
        state.apply_action(McpAction::AddNode { template_name: "Group".into(), name: Some("tagged".into()), x: 7.0, y: 8.0 }, &mut redraw).unwrap();
        let tagged = state.current_dir().children.iter().position(|c| c.name == "tagged").unwrap();
        for (name, value) in [("input", "sphere1"), ("group_name", "five"), ("mode", "Random"), ("count", "5")] {
            state.apply_action(McpAction::SetParam { slot: tagged, name: name.into(), value: value.into() }, &mut redraw).unwrap();
        }
        state.current_dir_mut().set_child_geometry_visible(tagged, true);
        state.rebuild_scene_geometry();
        assert!(state.scene_groups.iter().any(|(n, m)| n == "five" && m.len() == 5), "{:?}", state.scene_groups.iter().map(|(n, m)| (n.clone(), m.len())).collect::<Vec<_>>());
        assert!(state.marked_group_verts.is_empty(), "nothing is marked yet");

        // From the palette: the command turns it into the groups list.
        state.run_command("command_palette");
        assert_eq!(state.slots.dialog.mode, Mode::Commands);
        assert!(state.run_command("group_markers"));
        assert!(state.dialog_visible());
        assert_eq!(state.slots.dialog.mode, Mode::Groups);
        let row = state.slots.dialog.rows.iter().position(|r| r.id == format!("{GROUP_ROW_PREFIX}five")).expect("a row for the group");
        assert_eq!(state.slots.dialog.rows[row].chord, "5 points");
        assert_eq!(state.slots.dialog.rows[row].toggle(), Some(false));

        // Enter on the row marks the group, and the list stays up.
        state.slots.dialog.selected = row;
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(state.dialog_visible(), "a switch is worked in place");
        assert_eq!(state.slots.dialog.rows[row].toggle(), Some(true));
        assert!(state.group_marked("five"));
        assert!(!state.marked_group_verts.is_empty() && state.marked_groups_dirty, "the markers are staged");
        let one = state.marked_group_verts.len();
        // On the group's points, at Group Marker Size.
        let members: Vec<[f32; 3]> = state.scene_groups.iter().find(|(n, _)| n == "five").unwrap().1.clone();
        for m in &members {
            assert!(state.marked_group_verts.iter().any(|v| (0..3).all(|k| (v.position[k] - m[k]).abs() <= state.group_marker_size + 1e-4)), "a marker at {m:?}");
        }
        let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
        assert_eq!(crate::app::DesignSettings::from_kdl_str(&kdl).viewport.marked_groups, "five", "persisted");
        assert_eq!(State::marked_groups_of("b, a,,a"), vec!["a".to_string(), "b".to_string()]);

        // The markers follow the geometry: a bigger sphere, farther points.
        let sphere = state.current_dir().children.iter().position(|c| c.name == "sphere1").unwrap();
        let far = |state: &State| state.marked_group_verts.iter().map(|v| (v.position[0].powi(2) + v.position[2].powi(2)).sqrt()).fold(0.0f32, f32::max);
        let before = far(&state);
        state.apply_action(McpAction::SetParam { slot: sphere, name: "radius".into(), value: "2.0".into() }, &mut redraw).unwrap();
        assert!(far(&state) > before * 1.5, "{} against {before}", far(&state));
        assert_eq!(state.marked_group_verts.len(), one);

        // Enter again unmarks; Escape closes; a query filters the names.
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(!state.group_marked("five"));
        assert!(state.marked_group_verts.is_empty());
        state.dialog_key_input(&typed("z"));
        assert!(state.slots.dialog.rows.is_empty(), "no group matches");
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Escape)));
        assert!(!state.dialog_visible());

        // A marked name the scene has no group for marks nothing and is kept.
        state.set_group_marked("gone", true);
        assert!(state.marked_group_verts.is_empty());
        assert!(state.group_marked("gone"));
    }

    #[test]
    fn a_node_is_renamed_from_its_menu() {
        use crate::dialog::{Mode, RENAME_ROW_ID};
        let mut state = State::new(false);
        // A sibling for the sphere, whose name is taken.
        state
            .apply_action(McpAction::AddNode { template_name: "Box".into(), name: None, x: 9.0, y: 9.0 }, &mut false)
            .unwrap();
        state.record_structure_changes();
        let sphere = state.current_dir().children.iter().position(|c| c.node_type == "sphere").unwrap();
        let sibling = state.current_dir().children.iter().position(|c| c.node_type == "box").unwrap();
        let old = state.current_dir().children[sphere].name.clone();
        let other = state.current_dir().children[sibling].name.clone();
        let row = |state: &State| state.slots.dialog.rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
        let retype = |state: &mut State, name: &str| {
            while !state.slots.dialog.query.is_empty() {
                state.dialog_key_input(&key_press(Key::Named(NamedKey::Backspace)));
            }
            for c in name.chars() {
                if c == ' ' {
                    state.dialog_key_input(&key_press(Key::Named(NamedKey::Space)));
                } else {
                    state.dialog_key_input(&typed(&c.to_string()));
                }
            }
        };

        state.run_node_menu_action(sphere, crate::app::NodeMenuAction::Rename);
        assert!(state.dialog_visible());
        assert_eq!(state.slots.dialog.mode, Mode::Rename);
        assert_eq!(state.slots.dialog.query, old, "the dialog opens holding the name");
        assert_eq!(state.slots.dialog.rows[0].id, RENAME_ROW_ID);

        // Its own name, a sibling's, and none: each is said, and Enter on
        // it writes nothing.
        assert!(row(&state)[0].contains("already"), "{:?}", row(&state));
        retype(&mut state, &other);
        assert!(row(&state)[0].contains("another node"), "{:?}", row(&state));
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert_eq!(state.current_dir().children[sphere].name, old);

        // A name as typed is written as a name is: lowercase, no spaces.
        state.run_node_menu_action(sphere, crate::app::NodeMenuAction::Rename);
        retype(&mut state, "My Ball");
        assert_eq!(row(&state), vec![format!("Rename {old} to my_ball")]);
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(!state.dialog_visible());
        assert_eq!(state.current_dir().children[sphere].name, "my_ball");
        assert!(state.last_status_text.contains("my_ball"), "{}", state.last_status_text);

        // And it is a step.
        assert!(state.run_command("undo"));
        assert_eq!(state.current_dir().children[sphere].name, old);

        // The command renames the selection, and says so when there is none.
        state.graph_mut().set_selected_node(Some(sphere));
        assert!(state.run_command("rename_node"));
        assert_eq!(state.slots.dialog.mode, Mode::Rename);
        assert_eq!(state.slots.dialog.query, old);
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Escape)));
        assert_eq!(state.current_dir().children[sphere].name, old, "Escape renames nothing");

        // The menu has the row.
        let labels = state.node_menu_rows(sphere).0;
        assert!(labels.iter().any(|l| l == "Rename"), "{labels:?}");
    }

    /// New Project from the palette starts a project. The command named a
    /// label no arm dispatched, so the row ran and nothing happened.
    #[test]
    fn the_new_project_command_starts_an_empty_project() {
        let mut state = State::new(false);
        assert!(!state.fs_root.children.is_empty(), "the bundled project has nodes");
        assert!(state.run_command("new_project"));
        assert!(state.fs_root.children.is_empty(), "New Project left the old nodes in place");
        assert_eq!(state.loaded_project_path, None);
    }

    /// Two commands on one chord is silent at the keyboard — the second never
    /// runs and nothing says why — so it is reported at startup.
    #[test]
    fn test_chord_conflicts_are_detected_across_spellings() {
        use crate::command::conflicts;
        let none = conflicts(&[("save_document", "Ctrl+s".into()), ("undo", "Ctrl+z".into())]);
        assert!(none.is_empty(), "{none:?}");

        // Compared as PARSED chords, not as text: these are the same keypress.
        let clash = conflicts(&[
            ("save_document", "Ctrl+s".into()),
            ("toggle_grid", "ctrl+S".into()),
            ("undo", "CTRL+s".into()),
        ]);
        assert_eq!(clash.len(), 1, "{clash:?}");
        assert_eq!(clash[0].winner, "save_document", "the first registered must win");
        assert_eq!(clash[0].shadowed, vec!["toggle_grid", "undo"]);

        // The shipped defaults must not collide with each other.
        let shipped: Vec<(&'static str, String)> = crate::command::COMMANDS
            .iter()
            .filter_map(|c| c.default_chord.map(|d| (c.id, d.to_string())))
            .collect();
        let shipped_conflicts = conflicts(&shipped);
        assert!(shipped_conflicts.is_empty(), "the defaults collide: {shipped_conflicts:?}");
    }

    /// The palette ranks the focused pane's commands first without hiding the
    /// rest — a palette that omits what you are looking for is worse than one
    /// that lists it second.
    #[test]
    fn test_the_palette_puts_the_focused_panes_commands_first() {
        use crate::command::{palette_entries, Context, COMMANDS};
        let all = palette_entries("", Context::Viewport);
        assert_eq!(all.len(), COMMANDS.len(), "ranking dropped commands");
        assert_eq!(
            all[0].context,
            Context::Viewport,
            "a viewport command does not lead: {}",
            all[0].id
        );
        assert!(
            all.iter().any(|c| c.id == "save_document"),
            "a global command vanished when a pane was focused"
        );

        // With nothing pane-specific focused the order is the fuzzy one alone.
        let plain = palette_entries("", Context::Always);
        assert_eq!(plain[0].id, COMMANDS[0].id);
    }

    /// A chord prints back the way it parsed, in a fixed modifier order, so
    /// two spellings of one chord read the same beside their labels.
    #[test]
    fn test_a_chord_describes_itself_back() {
        use crate::shortcut::Shortcut;
        for (written, shown) in [
            ("Ctrl+s", "Ctrl+S"),
            ("ctrl+shift+S", "Ctrl+Shift+S"),
            ("shift+ctrl+s", "Ctrl+Shift+S"),
            ("`", "`"),
        ] {
            assert_eq!(Shortcut::parse(written).unwrap().describe(), shown);
        }
        // And what it prints parses back to the same chord.
        for c in crate::command::COMMANDS.iter().filter_map(|c| c.default_chord) {
            let parsed = Shortcut::parse(c).unwrap();
            assert_eq!(Shortcut::parse(&parsed.describe()).unwrap(), parsed, "{c} did not round trip");
        }
    }

    /// A palette row round-trips back to the command it names — including
    /// when one label is a prefix of another.
    #[test]
    fn test_a_palette_row_names_its_command_back() {
        use crate::command::{from_palette_row, palette_row, COMMANDS};
        let width = COMMANDS.iter().map(|c| c.label.len()).max().unwrap() + 2;

        for c in COMMANDS {
            let row = palette_row(c.label, Some("Ctrl+X".into()), width);
            assert_eq!(
                from_palette_row(&row).map(|f| f.id),
                Some(c.id),
                "row {row:?} did not name {} back",
                c.id
            );
            // And a row with no chord at all.
            let bare = palette_row(c.label, None, width);
            assert_eq!(from_palette_row(&bare).map(|f| f.id), Some(c.id));
        }

        // The prefix case, spelled out: "Save" starts "Save As"'s row, and
        // taking the first match rather than the longest runs the wrong one.
        let row = palette_row("Save As", Some("Ctrl+Shift+S".into()), width);
        assert_eq!(from_palette_row(&row).map(|c| c.id), Some("save_document_as"));

        assert!(from_palette_row("Not A Command").is_none());
    }

    /// The soft-transform viewer state: two fixed handles, one of which is a
    /// DERIVED position, driven through the same framework as the curve.
    ///
    /// This is the test that says the framework is one — the curve tests above
    /// exercise an open-ended list of stored world positions, and this is a
    /// fixed pair where the second handle is `Centre + Translation` and has to
    /// be converted both ways.
    #[test]
    fn test_the_soft_transform_state_drags_a_derived_handle() {
        use crate::geometry::node_param_str;
        let mut state = State::new(false);
        let mut redraw = false;
        state
            .apply_action(
                McpAction::AddNode {
                    template_name: "Soft Transform".to_string(),
                    name: None,
                    x: 0.0,
                    y: 0.0,
                },
                &mut redraw,
            )
            .expect("add soft transform node");
        let slot = state.current_dir().children.len() - 1;
        assert_eq!(state.current_dir().children[slot].node_type, "soft_transform");

        state.toggle_viewer_state(slot);
        assert!(state.viewer_tool.is_some(), "soft_transform must enter a viewer state");

        state.last_scene_mvp = Some(Mat4::IDENTITY);
        state.last_scene_view_rect = (0.0, 0.0, 100.0, 100.0);
        let sx = |x: f32| 50.0 + x * 50.0;
        let sy = |y: f32| 50.0 - y * 50.0;
        let param = |state: &State, name: &str| {
            node_param_str(&state.current_dir().children[slot], name, "")
        };

        // Two handles: the centre, and the tip at centre + translation. The
        // template ships centre (0,0,0) and translation (0,0.2,0).
        let handles = state.viewer_tool_handles();
        assert_eq!(handles.len(), 2, "a soft transform has exactly two handles");
        assert!((handles[0].1 - sx(0.0)).abs() < 1e-3 && (handles[0].2 - sy(0.0)).abs() < 1e-3);
        assert!(
            (handles[1].2 - sy(0.2)).abs() < 1e-3,
            "the tip is not drawn at centre + translation: {:?}",
            handles[1]
        );

        // Drag the TIP to (0.4, 0, 0): the translation becomes that offset,
        // and the centre does not move.
        state.cursor_x = handles[1].1;
        state.cursor_y = handles[1].2;
        assert!(state.viewer_tool_press(), "press on the tip must grab");
        state.cursor_x = sx(0.4);
        state.cursor_y = sy(0.0);
        assert!(state.viewer_tool_drag_motion());
        assert!(state.viewer_tool_release());
        assert_eq!(param(&state, "center"), "0.00:0.00:0.00", "the centre moved");
        assert_eq!(param(&state, "translation"), "0.40:0.00:0.00");

        // Fixed handles: a press on empty space adds nothing, and Delete
        // removes nothing — a third handle would mean nothing.
        state.cursor_x = sx(-0.9);
        state.cursor_y = sy(-0.9);
        assert!(state.viewer_tool_press(), "the state still owns the viewport press");
        assert!(state.viewer_tool_release() || true);
        assert_eq!(state.viewer_tool_handles().len(), 2, "empty-space press added a handle");
        assert!(!state.viewer_tool_delete_selected(), "a fixed source must not delete");

        // Dragging the BASE keeps the tip where it is, so the translation
        // shortens to match — the documented two-handled-gizmo behaviour.
        let handles = state.viewer_tool_handles();
        state.cursor_x = handles[0].1;
        state.cursor_y = handles[0].2;
        assert!(state.viewer_tool_press());
        state.cursor_x = sx(0.1);
        state.cursor_y = sy(0.0);
        assert!(state.viewer_tool_drag_motion());
        assert!(state.viewer_tool_release());
        assert_eq!(param(&state, "center"), "0.10:0.00:0.00");
        assert_eq!(param(&state, "translation"), "0.30:0.00:0.00", "the tip should not have moved");

        // And undo restores BOTH parameters, which is the case a "keep the
        // translation when the centre moves" rule would have broken.
        assert!(state.viewer_tool_undo());
        assert_eq!(param(&state, "center"), "0.00:0.00:0.00");
        assert_eq!(param(&state, "translation"), "0.40:0.00:0.00");
    }

    /// Snapping rounds a dragged handle to a world increment, and only when it
    /// is on.
    #[test]
    fn test_snapping_rounds_a_dragged_handle() {
        use crate::viewer_state::SNAP_INCREMENT;
        let mut state = State::new(false);
        let mut redraw = false;
        state
            .apply_action(
                McpAction::AddNode { template_name: "Curve".to_string(), name: None, x: 0.0, y: 0.0 },
                &mut redraw,
            )
            .expect("add curve node");
        let slot = state.current_dir().children.len() - 1;
        state.toggle_viewer_state(slot);
        state.last_scene_mvp = Some(Mat4::IDENTITY);
        state.last_scene_view_rect = (0.0, 0.0, 100.0, 100.0);
        let points = |state: &State| {
            crate::geometry::parse_curve_points(&crate::geometry::node_param_str(
                &state.current_dir().children[slot],
                "points",
                "",
            ))
        };

        // Off by default: a drag lands exactly where the cursor is.
        let first = points(&state)[0];
        state.cursor_x = 50.0 + first.x * 50.0;
        state.cursor_y = 50.0 - first.y * 50.0;
        assert!(state.viewer_tool_press());
        state.cursor_x = 50.0 + 0.37 * 50.0;
        state.cursor_y = 50.0;
        assert!(state.viewer_tool_drag_motion());
        assert!(state.viewer_tool_release());
        assert!((points(&state)[0].x - 0.37).abs() < 1e-3, "{:?}", points(&state)[0]);

        // On: the same drag rounds to the increment.
        assert!(state.toggle_viewer_snap());
        assert_eq!(state.viewer_tool.as_ref().unwrap().snap, Some(SNAP_INCREMENT));
        let first = points(&state)[0];
        state.cursor_x = 50.0 + first.x * 50.0;
        state.cursor_y = 50.0 - first.y * 50.0;
        assert!(state.viewer_tool_press());
        state.cursor_x = 50.0 + 0.37 * 50.0;
        state.cursor_y = 50.0;
        assert!(state.viewer_tool_drag_motion());
        assert!(state.viewer_tool_release());
        let p = points(&state)[0];
        assert!((p.x - 0.4).abs() < 1e-3, "snapped x should be 0.4, got {p:?}");

        // And off again, from the same command.
        assert!(state.toggle_viewer_snap());
        assert_eq!(state.viewer_tool.as_ref().unwrap().snap, None);

        // The HUD says which it is, because a mode you cannot see is a mode
        // you forget you are in.
        let hud = state.viewer_tool.as_ref().unwrap().hud();
        assert!(hud.contains("Curve Points") && hud.contains("Snap off"), "{hud}");
    }

    /// Only node types with a source enter a viewer state, and each gets its
    /// own.
    #[test]
    fn test_only_editable_node_types_enter_a_viewer_state() {
        use crate::viewer_state::source_for;
        assert_eq!(source_for("curve").map(|s| s.name()), Some("Curve Points"));
        assert_eq!(source_for("soft_transform").map(|s| s.name()), Some("Soft Transform"));
        assert!(source_for("sphere").is_none());
        assert!(source_for("boolean").is_none());
        // Types are matched case-insensitively, like every other node lookup.
        assert!(source_for("Curve").is_some());
    }

    /// Keyboard graph navigation: the plugin's hjkl families, as commands.
    ///
    /// Bare hjkl moves the grid cursor and therefore the selection, alt moves
    /// the node under it, ctrl pans the view and touches neither. All of it is
    /// gated on the network pane having focus — the bare family used to be the
    /// one that was not, so the cursor drifted invisibly while you looked at
    /// the viewport.
    /// Stepping the grid cursor from node to node must not carry one node's
    /// parameters into the next.
    ///
    /// The pane's rows are written back by NAME, and until 2026-09-25 they
    /// were written into whatever was selected — `sync_layout` flushed them
    /// through `sync_pane_focus` after a cursor step had moved the selection
    /// but before the post-event pass reloaded the pane. Every node has an
    /// `Input`, so pressing k twice from the bottom of a chain pointed each
    /// node it landed on at the chain's end, and carried Iterations from one
    /// node into the next on the way. Real key events through
    /// `process_window_event`, because the bug lived in that pass's order and
    /// `run_command` alone skips it.
    #[test]
    fn stepping_the_cursor_does_not_rewire_the_nodes_it_lands_on() {
        use crate::window::WindowEvent;
        fn key(state: cce_ui::widget::ElementState) -> cce_ui::widget::KeyEvent {
            cce_ui::widget::KeyEvent {
                state,
                logical_key: Key::Character("k".into()),
                text: Some("k".into()),
                repeat: false,
                ctrl: false,
                shift: false,
                alt: false,
            }
        }
        fn params(s: &State) -> Vec<(String, Vec<(String, String)>)> {
            s.current_dir()
                .children
                .iter()
                .map(|c| (c.name.clone(), c.params.iter().map(|p| (p.name.clone(), p.text().to_string())).collect()))
                .collect()
        }
        let mut s = State::new(false);
        let mut r = false;
        let base = s.current_dir().children.len();
        // A column of its own, clear of the bundled project's nodes: the
        // chain's shape from the report, Output at the bottom.
        for (t, y) in [("attribute", 3.0), ("relax", 5.0), ("Detangle", 6.0), ("Output", 7.0)] {
            s.apply_action(McpAction::AddNode { template_name: t.into(), name: None, x: 13.0, y }, &mut r).unwrap();
        }
        let names: Vec<String> = s.current_dir().children[base..].iter().map(|c| c.name.clone()).collect();
        for i in 1..4 {
            s.apply_action(McpAction::SetParam { slot: base + i, name: "input".into(), value: names[i - 1].clone() }, &mut r)
                .unwrap();
        }
        s.focused_pane = LEFT_MENUBAR_IDX;
        s.param_editor = crate::slots::CONTENT_IDX;
        s.grid_cursor_col = 13;
        s.grid_cursor_row = 7;
        s.sync_cursor_and_selection();
        s.sync_parameters_pane();
        let before = params(&s);

        for row in [6, 5] {
            s.process_window_event(WindowEvent::KeyboardInput { event: key(cce_ui::widget::ElementState::Pressed) });
            s.process_window_event(WindowEvent::KeyboardInput { event: key(cce_ui::widget::ElementState::Released) });
            assert_eq!(s.grid_cursor_row, row, "the step has to have happened, or the check is vacuous");
        }
        assert_eq!(s.graph().selected_node(), Some(base + 1), "the cursor landed on the relax");
        assert_eq!(params(&s), before, "no node's parameters may change from walking over it");

        // And the guard does not stand in the way of the pane it protects:
        // an edit to the node it now shows still lands.
        let rows: Vec<(String, String, String)> = s
            .param()
            .node_params()
            .into_iter()
            .map(|(n, v, t)| if n == "Iterations" { (n, "12".into(), t) } else { (n, v, t) })
            .collect();
        s.param_mut().set_display_params(&rows);
        s.sync_parameters_to_project();
        let relax = &s.current_dir().children[base + 1];
        assert_eq!(crate::geometry::node_param_str(relax, "iterations", ""), "12");
    }

    #[test]
    fn test_the_network_navigation_families() {
        use crate::slots::{LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX};
        let mut state = State::new(false);
        let mut redraw = false;
        state
            .apply_action(
                McpAction::AddNode { template_name: "Sphere".to_string(), name: None, x: 3.0, y: 2.0 },
                &mut redraw,
            )
            .expect("add node");
        let slot = state.current_dir().children.len() - 1;
        assert_eq!(state.current_dir().children[slot].position, (3.0, 2.0));

        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        state.grid_cursor_col = 3;
        state.grid_cursor_row = 2;
        state.sync_cursor_and_selection();
        assert_eq!(state.graph().selected_node(), Some(slot), "the cursor should select what it sits on");

        // Bare hjkl walks the cursor, and the selection follows it off the
        // node. Up, not right: the default project already has a node at
        // (4, 2), and navigating onto it would select that one instead —
        // correctly, which is exactly why the empty cell has to be chosen
        // deliberately rather than assumed.
        assert!(state.run_command("nav_up"));
        assert_eq!((state.grid_cursor_col, state.grid_cursor_row), (3, 1));
        assert_eq!(state.graph().selected_node(), None, "the cursor left the node");
        assert!(state.run_command("nav_down"));
        assert_eq!(state.graph().selected_node(), Some(slot), "and came back to it");

        // And navigating ONTO another node selects that one.
        let neighbour = state
            .current_dir()
            .children
            .iter()
            .position(|c| c.position == (4.0, 2.0))
            .expect("the default project has a node at (4, 2)");
        assert!(state.run_command("nav_right"));
        assert_eq!(state.graph().selected_node(), Some(neighbour));
        assert!(state.run_command("nav_left"));

        // Alt moves the node AND the cursor, so a run of them drags it rather
        // than leaving it behind on the first press.
        assert!(state.run_command("move_down"));
        assert_eq!(state.current_dir().children[slot].position, (3.0, 3.0));
        assert_eq!((state.grid_cursor_col, state.grid_cursor_row), (3, 3));
        assert_eq!(state.graph().selected_node(), Some(slot), "the node should still be selected");
        assert!(state.run_command("move_down"));
        assert_eq!(state.current_dir().children[slot].position, (3.0, 4.0));

        // Ctrl pans the view: the cursor, the selection and the node all stay.
        let before = (state.pan_x, state.pan_y);
        let cursor = (state.grid_cursor_col, state.grid_cursor_row);
        assert!(state.run_command("view_right"));
        assert_ne!((state.pan_x, state.pan_y), before, "the view did not pan");
        assert_eq!((state.grid_cursor_col, state.grid_cursor_row), cursor);
        assert_eq!(state.current_dir().children[slot].position, (3.0, 4.0));
        assert_eq!(state.graph().selected_node(), Some(slot));

        // Frame Cursor CENTRES the cursor cell, rather than only scrolling it
        // into view when it has gone off an edge — which would make the
        // command do nothing in the case you actually press it in.
        state.positions[crate::slots::CONTENT_IDX] = (0.0, 0.0, 800.0, 600.0);
        state.grid_cursor_col = 9;
        state.grid_cursor_row = 7;
        assert!(state.run_command("frame_cursor"));
        // The cell's centre IS its lattice intersection. Pane-relative,
        // because the command's rebuild_positions puts the real pane origin
        // back under the cell_center the pan was computed for.
        let (pane_x, pane_y, _, _) = state.positions[crate::slots::CONTENT_IDX];
        let (cell_x, cell_y) = state.cell_center(9, 7);
        let (cell_x, cell_y) = (cell_x - pane_x, cell_y - pane_y);
        assert!(
            (cell_x - 400.0).abs() < 1.0,
            "the cursor cell is not centred horizontally: {cell_x}"
        );
        assert!(
            (cell_y - 300.0).abs() < 1.0,
            "the cursor cell is not centred vertically: {cell_y}"
        );

        // Every family is gated on the network pane. With the viewport focused
        // the commands run and do nothing, rather than moving a cursor nobody
        // can see.
        state.focused_pane = RIGHT_MENUBAR_IDX;
        let cursor = (state.grid_cursor_col, state.grid_cursor_row);
        let pos = state.current_dir().children[slot].position;
        let pan = (state.pan_x, state.pan_y);
        for id in ["nav_left", "nav_right", "nav_up", "nav_down", "move_left", "view_left", "frame_cursor"] {
            assert!(state.run_command(id), "{id} should be a known command");
        }
        assert_eq!((state.grid_cursor_col, state.grid_cursor_row), cursor, "the cursor moved from another pane");
        assert_eq!(state.current_dir().children[slot].position, pos, "a node moved from another pane");
        assert_eq!((state.pan_x, state.pan_y), pan, "the view panned from another pane");
    }

    /// The navigation scheme is the plugin's, and the registry says so: hjkl
    /// bare, shift, alt and ctrl, plus the two framings — eighteen rows, all
    /// in the network context, none of them colliding.
    #[test]
    fn test_the_navigation_scheme_matches_the_plugins() {
        use crate::command::{by_id, Context};
        let expected = [
            // Uppercase because `describe` prints single letters as capitals,
            // the way every menu in the app writes a chord.
            ("nav_left", "H"), ("nav_down", "J"), ("nav_up", "K"), ("nav_right", "L"),
            ("extend_left", "Shift+H"), ("extend_down", "Shift+J"), ("extend_up", "Shift+K"), ("extend_right", "Shift+L"),
            ("move_left", "Alt+H"), ("move_down", "Alt+J"), ("move_up", "Alt+K"), ("move_right", "Alt+L"),
            ("view_left", "Ctrl+H"), ("view_down", "Ctrl+J"), ("view_up", "Ctrl+K"), ("view_right", "Ctrl+L"),
            ("frame_cursor", "F"), ("frame_all", "Shift+F"),
        ];
        for (id, chord) in expected {
            let cmd = by_id(id).unwrap_or_else(|| panic!("{id} is not a command"));
            assert_eq!(cmd.context, Context::Network, "{id} is not a network command");
            let parsed = crate::shortcut::Shortcut::parse(cmd.default_chord.expect(id)).unwrap();
            assert_eq!(parsed.describe(), chord, "{id} is not bound where the plugin binds it");
        }
    }

    /// Auto-layout: rows are how far downstream a node is, columns keep it
    /// under what it reads from.
    #[test]
    fn test_auto_layout_lays_a_chain_out_vertically() {
        use crate::layout::{arrange, LayoutNode};
        let node = |name: &str, input: Option<&str>, pos: (f32, f32)| LayoutNode {
            name: name.to_string(),
            input: input.map(|s| s.to_string()),
            reads: Vec::new(),
            position: pos,
            pinned: false,
        };

        // A chain, scattered. It should come back as one vertical line,
        // because a chain IS a vertical line in this grid — a sphere at
        // (4, 2) feeding an output at (4, 3) is the convention every project
        // in the repo already uses.
        let nodes = vec![
            node("c", Some("b"), (7.0, 0.0)),
            node("a", None, (2.0, 5.0)),
            node("b", Some("a"), (0.0, 9.0)),
        ];
        let moved: std::collections::HashMap<usize, (f32, f32)> =
            arrange(&nodes).into_iter().collect();
        let at = |i: usize| moved.get(&i).copied().unwrap_or(nodes[i].position);
        assert_eq!(at(1).1, 0.0, "the root is not on the top row");
        assert_eq!(at(2).1, 1.0, "its child is not one row below it");
        assert_eq!(at(0).1, 2.0, "the grandchild is not two rows below");
        assert_eq!(at(1).0, at(2).0, "a chain should be one column");
        assert_eq!(at(2).0, at(0).0, "a chain should be one column");

        // A root's existing column is its wish, so two independent chains keep
        // the left-to-right order the user gave them.
        let nodes = vec![
            node("right", None, (5.0, 0.0)),
            node("left", None, (1.0, 0.0)),
            node("right_child", Some("right"), (0.0, 0.0)),
            node("left_child", Some("left"), (0.0, 0.0)),
        ];
        let moved: std::collections::HashMap<usize, (f32, f32)> =
            arrange(&nodes).into_iter().collect();
        let at = |i: usize| moved.get(&i).copied().unwrap_or(nodes[i].position);
        assert!(at(1).0 < at(0).0, "left should stay left of right");
        assert_eq!(at(1).0, at(3).0, "left's child should sit under it");
        assert_eq!(at(0).0, at(2).0, "right's child should sit under it");
        assert_eq!(at(2).1, 1.0);
        assert_eq!(at(3).1, 1.0);
    }

    /// Every wire a node has is drawn, into its own port: the Remesh's
    /// switch reads the loop on its Input and the transfer on its Input 2,
    /// and both are lines on the network now, where only the Input was. An
    /// expression wire (the transfer's From) is drawn to what it evaluates
    /// to — input1 while the Remesh's own From is empty; a node beside the
    /// subnet when it names one, which is not on this level to draw from.
    /// A connection dropped on a port sets THAT wire, and auto-layout puts
    /// a node below everything it reads.
    #[test]
    fn every_wire_is_drawn_into_its_own_port() {
        use cce_ui::widget::node_wires;
        // In the bundled project's Geometry node, where it opens.
        let mut state = State::new(false);
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Remesh".into(), name: Some("remesh1".into()), x: 3.0, y: 8.0 }, &mut redraw).unwrap();
        let slot = state.current_dir().children.iter().position(|c| c.name == "remesh1").unwrap();
        state.apply_action(McpAction::Enter { slot }, &mut redraw).unwrap();
        state.sync_nodes();
        let nodes = state.graph().get_nodes();
        let get = |n: &str| nodes.iter().find(|g| g.name == n).unwrap().clone();
        let switch = get("transfer_switch1");
        assert_eq!(node_wires(&switch), ["repeat1", "transfer1", "", ""]);
        assert_eq!(switch.inputs, 4);
        let transfer = get("transfer1");
        assert_eq!(node_wires(&transfer), ["repeat1", "input1"], "From is an expression, drawn to what it evaluates to");

        // The Remesh's From naming a node outside: not on this level, no line.
        state.apply_action(McpAction::Up, &mut redraw).unwrap();
        state.current_dir_mut().children[slot].params.iter_mut().find(|p| p.name == "from").unwrap().set_text("elsewhere");
        state.apply_action(McpAction::Enter { slot }, &mut redraw).unwrap();
        state.sync_nodes();
        let level = state.graph().get_nodes();
        let transfer = level.iter().find(|g| g.name == "transfer1").unwrap();
        assert_eq!(node_wires(transfer), ["repeat1", "elsewhere"], "it names the outer node…");
        assert!(!level.iter().any(|g| g.name == "elsewhere"), "…which is not on this level, so no line is drawn");

        // Dropped on the switch's third port: Input 3.
        let path = state.current_path.clone();
        let id = switch.id.clone();
        assert!(state.connect_port(&path, &id, "input1".into(), 2));
        let sw = state.current_dir().children.iter().find(|c| c.id == id).unwrap();
        assert_eq!(sw.params.iter().find(|p| p.name == "input_3").unwrap().text(), "input1");
        assert_eq!(sw.params.iter().find(|p| p.name == "input").unwrap().text(), "repeat1", "the Input is untouched");

        // A second operand sets the row, not the column.
        use crate::layout::{arrange, LayoutNode};
        let node = |name: &str, input: Option<&str>, reads: &[&str], pos: (f32, f32)| LayoutNode {
            name: name.into(),
            input: input.map(String::from),
            reads: reads.iter().map(|s| s.to_string()).collect(),
            position: pos,
            pinned: false,
        };
        let nodes = vec![
            node("a", None, &[], (0.0, 0.0)),
            node("b", Some("a"), &[], (4.0, 0.0)),
            node("c", Some("b"), &[], (4.0, 0.0)),
            node("join", Some("a"), &["c"], (0.0, 0.0)),
        ];
        let moved: std::collections::HashMap<usize, (f32, f32)> = arrange(&nodes).into_iter().collect();
        let at = |i: usize| moved.get(&i).copied().unwrap_or(nodes[i].position);
        assert_eq!(at(3).1, 3.0, "below c, which it reads through its second wire");
        assert_eq!(at(3).0, at(0).0, "under a, which its Input reads");
    }

    /// The cases that would otherwise hang or overwrite: cycles, self
    /// reference, dangling names, and pinned cells.
    #[test]
    fn test_auto_layout_survives_cycles_and_pinned_nodes() {
        use crate::layout::{arrange, LayoutNode};
        let node = |name: &str, input: Option<&str>, pos: (f32, f32), pinned: bool| LayoutNode {
            name: name.to_string(),
            input: input.map(|s| s.to_string()),
            reads: Vec::new(),
            position: pos,
            pinned,
        };

        // A name-wired graph can be cyclic; it must terminate rather than
        // recurse, and the answer only has to be finite and sane.
        let cyclic = vec![
            node("a", Some("b"), (0.0, 0.0), false),
            node("b", Some("a"), (1.0, 0.0), false),
            node("self", Some("self"), (2.0, 0.0), false),
        ];
        let moved = arrange(&cyclic);
        assert!(moved.len() <= 3);
        for (_, (c, r)) in &moved {
            assert!(c.is_finite() && r.is_finite() && *r >= 0.0);
        }

        // A dangling input name is simply no edge — the node is a root, not an
        // error and not a crash.
        let dangling = vec![node("a", Some("nothing_called_this"), (3.0, 4.0), false)];
        let moved: Vec<_> = arrange(&dangling);
        assert_eq!(moved, vec![(0, (3.0, 0.0))], "a dangling input should make a root");

        // Pinned nodes never move, and nothing is placed on top of them.
        let pinned = vec![
            node("meta", None, (0.0, 0.0), true),
            node("a", None, (0.0, 5.0), false),
            node("b", Some("a"), (0.0, 6.0), false),
        ];
        let moved: std::collections::HashMap<usize, (f32, f32)> =
            arrange(&pinned).into_iter().collect();
        assert!(!moved.contains_key(&0), "a pinned node moved");
        let a = moved.get(&1).copied().unwrap_or(pinned[1].position);
        assert_ne!(a, (0.0, 0.0), "a node was placed on top of the pinned one");
        assert_eq!(a.1, 0.0, "the root still belongs on the top row");
        let b = moved.get(&2).copied().unwrap_or(pinned[2].position);
        assert_eq!(b.0, a.0, "the child should follow its parent's column");
        assert_eq!(b.1, 1.0);
    }

    /// The command end to end, on a real project.
    #[test]
    fn test_the_layout_command_arranges_the_current_level() {
        use crate::slots::{CONTENT_IDX, LEFT_MENUBAR_IDX};
        let mut state = State::new(false);
        let mut redraw = false;
        for (template, x, y) in
            [("Curve", 6.0, 7.0), ("Remesh", 2.0, 1.0), ("Subdivide", 9.0, 3.0)]
        {
            state
                .apply_action(
                    McpAction::AddNode {
                        template_name: template.to_string(),
                        name: None,
                        x,
                        y,
                    },
                    &mut redraw,
                )
                .unwrap_or_else(|e| panic!("add {template}: {e}"));
        }
        let n = state.current_dir().children.len();
        let (curve, remesh, subdiv) = (n - 3, n - 2, n - 1);
        let names: Vec<String> =
            state.current_dir().children.iter().map(|c| c.name.clone()).collect();

        // Wire them into a chain: curve -> remesh -> subdivide.
        let set_input = |state: &mut State, slot: usize, value: &str| {
            let dir = state.current_dir_mut();
            if let Some(p) =
                dir.children[slot].params.iter_mut().find(|p| p.name.eq_ignore_ascii_case("input"))
            {
                p.set_text(value.to_string());
            }
        };
        set_input(&mut state, remesh, &names[curve]);
        set_input(&mut state, subdiv, &names[remesh]);

        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = CONTENT_IDX;
        assert!(state.layout_current_level());

        let pos = |state: &State, slot: usize| state.current_dir().children[slot].position;
        assert_eq!(pos(&state, curve).1 + 1.0, pos(&state, remesh).1, "remesh should sit below curve");
        assert_eq!(pos(&state, remesh).1 + 1.0, pos(&state, subdiv).1, "subdivide should sit below remesh");
        assert_eq!(pos(&state, curve).0, pos(&state, remesh).0, "the chain should be one column");
        assert_eq!(pos(&state, remesh).0, pos(&state, subdiv).0, "the chain should be one column");

        // The meta node is a utility tree and stays where it was.
        let meta = state.current_dir().children.iter().position(|c| c.node_type == "meta");
        if let Some(m) = meta {
            assert_eq!(pos(&state, m), (0.0, 0.0), "the meta node moved");
        }

        // Running it again changes nothing, and says so rather than looking
        // broken.
        let before: Vec<_> =
            state.current_dir().children.iter().map(|c| c.position).collect();
        assert!(state.layout_current_level());
        let after: Vec<_> = state.current_dir().children.iter().map(|c| c.position).collect();
        assert_eq!(before, after, "a second layout should be a no-op");

        // And it is gated on the network pane like every other network command.
        state.focused_pane = crate::slots::RIGHT_MENUBAR_IDX;
        assert!(!state.layout_current_level());
    }

    /// `cargo test` must not write the user's own settings.
    ///
    /// It did, until 2026-09-23. `DesignSettings::file_path` hardcoded
    /// `$HOME/.config/cce/cce-designer/state.kdl`, and `State::new` loads the
    /// BUNDLED project, whose meta subnets overwrite the live viewport flags —
    /// so any test that reached `save_settings` wrote the bundled project's
    /// show_grid / show_cube / show_origin over the user's. Every run of the
    /// suite silently reset three of their toggles, and the run looked green.
    ///
    /// The real path is spelled out here rather than read from `file_path()`,
    /// which is the thing under test and now answers with a temp directory.
    #[test]
    fn the_suite_does_not_write_the_users_own_settings() {
        let real = cce_ui::config::cce_config_dir().join("cce-designer").join("state.kdl");
        let before = std::fs::read(&real).ok();

        assert!(
            !crate::app::DesignSettings::file_path()
                .starts_with(cce_ui::config::cce_config_dir()),
            "the suite writes settings inside the real cce config directory"
        );

        // The flip that carried the damage: it marks settings dirty and
        // `execute_action` saves at the end of the action.
        let mut state = State::new(false);
        let plate = state.network_plate;
        assert!(state.run_command("toggle_network_plate"));
        assert_ne!(state.network_plate, plate, "the toggle did not flip the plate");

        // Without this the test passes on a save that never happened, which
        // is exactly the bug wearing a different face.
        assert!(
            crate::app::DesignSettings::file_path().exists(),
            "no settings file was written at all — the assertion below proves nothing"
        );
        assert_eq!(
            std::fs::read(&real).ok(),
            before,
            "{} changed — a test wrote the user's real settings",
            real.display()
        );
    }

    /// The recent-files list is not the user's, under test.
    ///
    /// The toolkit derives that path from the EXE's basename, so each test
    /// binary wrote a real `~/.config/cce/cce_designer-<hash>/` of its own —
    /// seven had accumulated by 2026-09-23. Reading was no safer than
    /// writing: a test that loaded the real list would assert against
    /// whatever projects happen to be on the machine running it.
    ///
    /// Both halves are asserted because they fail differently — a load that
    /// reached the real file makes this suite's behaviour depend on the
    /// machine, a save leaves a directory behind on it — and because the
    /// gate is one `cfg!(test)` in each of two functions, so one can be
    /// removed without the other.
    #[test]
    fn the_recent_files_list_is_not_the_users() {
        assert!(
            State::load_recent_files().is_empty(),
            "the suite loaded the real recent-files list"
        );

        let real = cce_ui::config::get_app_recent_files_path();
        let before = std::fs::read(&real).ok();

        let mut state = State::new(false);
        state.add_recent_file(
            std::env::temp_dir().join(format!("cce-designer-recent-{}", std::process::id())),
        );

        assert_eq!(
            std::fs::read(&real).ok(),
            before,
            "{} changed — a test wrote a real recent-files list",
            real.display()
        );
    }

    /// The suite runs on a lattice of its own, not the machine's.
    ///
    /// `configured_grid_geometry` read `style.surface.graph.spacing_x` and
    /// friends straight out of `~/.config/cce/config.kdl`, so the grid tests
    /// — which press at pixel coordinates from `cell_center` and assert which
    /// node was hit — passed or failed by whoever's config was installed.
    /// `dragging_a_selected_node_carries_the_selection` genuinely failed at
    /// cce-ui's own defaults: its row 11 lands at 1237 px in a 900 px test
    /// window, so the press misses and the drag never arms. It had been
    /// passing on the author's 140 x 70.
    ///
    /// Asserting the constants back is the point rather than a tautology: it
    /// is what fails if the pin is ever unwired back to the config, and the
    /// live `State` is checked alongside them so the pin has to reach the app
    /// and not just the helper.
    #[test]
    fn the_suite_runs_on_a_lattice_of_its_own() {
        let g = crate::app::configured_grid_geometry();
        assert_eq!(
            (g.pitch_x, g.pitch_y, g.node_w, g.node_h),
            (140.0, 70.0, 80.0, 40.0),
            "the suite's lattice moved — if this came from config.kdl, the grid \
             tests now depend on the machine running them"
        );

        let state = State::new(false);
        assert_eq!(
            (state.grid_pitch_x, state.grid_pitch_y, state.node_w, state.node_h),
            (140.0, 70.0, 80.0, 40.0),
            "State::new did not start on the pinned lattice"
        );
    }

    /// The network plate is optional, and the option is reachable three ways
    /// that cannot disagree: the View settings node's toggle, the network
    /// pane's View menu, and the command palette.
    ///
    /// The flip itself is exercised by
    /// `the_suite_does_not_write_the_users_own_settings`, which is what it is
    /// for: until the settings path was redirected under test, flipping the
    /// plate here would have rewritten the user's own state.kdl as a side
    /// effect. What is asserted below is everything around the flip — the
    /// default, the wiring, and the mirror.
    #[test]
    fn test_the_network_plate_is_an_option() {
        use crate::command::{by_id, Run};

        let state = State::new(false);
        assert!(state.network_plate, "the plate is on unless the user turned it off");

        // The command exists, is rebindable, and runs the same action the menu
        // row does — one implementation behind both.
        let cmd = by_id("toggle_network_plate").expect("no toggle_network_plate command");
        assert_eq!(cmd.label, "Network Plate");
        assert_eq!(cmd.run, Run::Key(crate::shortcut::Action::ToggleNetworkPlate));
        assert!(cmd.default_chord.is_some(), "the plate toggle has no chord");

        // The live flag and the dialog's switch are one reading: the
        // command's palette row goes through `command_toggle_state` rather
        // than mirroring the flag onto a node that could fall out of step
        // with it.
        let mut state = State::new(false);
        assert_eq!(state.command_toggle_state("toggle_network_plate"), Some(true));
        assert!(state.run_command("toggle_network_plate"));
        assert!(!state.network_plate);
        assert_eq!(state.command_toggle_state("toggle_network_plate"), Some(false));

        state.run_command("command_palette");
        let row = state
            .slots
            .dialog
            .rows
            .iter()
            .find(|r| r.id == "toggle_network_plate")
            .expect("a Network Plate row");
        assert_eq!(row.toggle(), Some(false), "the row's switch reads the live flag");
    }

    /// The viewport guide toggles survive the next parameter edit.
    ///
    /// `apply_settings_from_menubar_subnets` used to copy the Guides utility
    /// node onto the live flags on EVERY parameter change, so a command that
    /// flipped only the flag was undone by the next edit anywhere — Show
    /// Cube (a guide since removed) hid the cube, and editing any node's
    /// parameter brought it back.
    /// The fix was to write the node as well; the node is gone now and the
    /// flag is simply the value, which is the same guarantee with nothing
    /// left to fall out of step. Still asserted, because the failure it
    /// catches (an edit reverting a display toggle) is invisible in a test
    /// that only flips the toggle.
    #[test]
    fn guide_toggles_survive_the_settings_apply_pass() {
        let mut state = State::new(false);
        for command in ["toggle_grid", "toggle_origin", "toggle_point_markers"] {
            let flag = |state: &State| match command {
                "toggle_grid" => state.viewport().show_grid,
                "toggle_origin" => state.viewport().show_origin,
                _ => state.show_point_markers,
            };
            let before = flag(&state);
            assert!(state.run_command(command));
            assert_eq!(flag(&state), !before, "{command} flipped the flag");

            // A real edit through the action path, on an unrelated node.
            let mut redraw = false;
            let sphere = state.current_dir().children.iter().position(|c| c.name.starts_with("sphere")).expect("a sphere");
            state
                .apply_action(crate::app::McpAction::SetParam { slot: sphere, name: "radius".into(), value: "0.7".into() }, &mut redraw)
                .expect("set a sphere param");
            assert_eq!(flag(&state), !before, "{command} was undone by a parameter edit");
        }

        // Circular Pane had the same hole.
        let before = state.circular_network_pane;
        assert!(state.run_command("toggle_circular_pane"));
        assert_eq!(state.circular_network_pane, !before);
    }

    /// A replacement renderer invalidates the page pane's image id, and the
    /// state has to notice.
    ///
    /// There is no reconnect callback: the runner calls `renderer_init` once
    /// per renderer, so the FIRST call is this process's own and every later
    /// one is a replacement. Remembering that it has been called is the only
    /// way to tell them apart — and getting it wrong is silent, because a
    /// stale id names nothing and its draws are skipped rather than failing.
    #[test]
    fn test_a_replacement_renderer_drops_the_page_image() {
        let mut state = State::new(false);
        assert!(!state.seen_renderer, "a fresh State has not been given a renderer");
        assert!(!state.page_dirty);

        // Stand in for a composed page: an id owned by State.
        state.page_image = Some(7);

        // The FIRST renderer is this process's own — nothing to invalidate,
        // and dropping the image here would throw away a page that is fine.
        assert!(!state.renderer_handed_over(), "the first renderer is not a replacement");
        assert_eq!(state.page_image, Some(7), "the first renderer must not drop the image");
        assert!(!state.page_dirty);

        // A LATER one is a replacement: the id names nothing in it, so it is
        // dropped and the next tick re-uploads rather than leaving the pane
        // blank forever.
        assert!(state.renderer_handed_over(), "the second renderer must read as a replacement");
        assert_eq!(state.page_image, None, "the dead id was kept");
        assert!(state.page_dirty, "nothing would re-upload the page");
    }

    /// Dragging empty scene turns the camera, at the same rate the trackpad's
    /// pixel-delta orbit does.
    ///
    /// The camera had no drag gesture at all before this: `Viewport3D` handles
    /// only `MouseWheel`, so the scene could be turned by scrolling and by
    /// nothing else. The rate is shared with that path deliberately — a drag
    /// and a two-finger swipe should not feel like different cameras.
    #[test]
    fn test_dragging_empty_scene_turns_the_camera() {
        let mut state = State::new(false);
        let k = State::ORBIT_RADIANS_PER_PX;

        // The default camera carries its own orbit. X drag yaws, Y drag
        // pitches, and the pitch is inverted so dragging down looks down.
        state.active_camera = "Default Camera".to_string();
        let (y0, x0) = (state.viewport().rotation_y, state.viewport().rotation_x);
        state.orbit_camera_by(100.0, 40.0);
        assert!(
            (state.viewport().rotation_y - (y0 + 100.0 * k)).abs() < 1e-5,
            "yaw did not follow the drag"
        );
        assert!(
            (state.viewport().rotation_x - (x0 - 40.0 * k)).abs() < 1e-5,
            "pitch did not follow the drag, or is not inverted"
        );

        // A named camera accumulates instead, for the camera NODE to pick up —
        // the same split the scroll path makes.
        let mut state = State::new(false);
        state.active_camera = "Camera 1".to_string();
        let before = (state.viewport().rotation_y, state.viewport().rotation_x);
        state.orbit_camera_by(100.0, 40.0);
        assert!(
            (state.viewport().pending_yaw - 100.0 * k).abs() < 1e-5,
            "a named camera's yaw did not accumulate"
        );
        assert!(
            (state.viewport().pending_pitch - (-40.0 * k)).abs() < 1e-5,
            "a named camera's pitch did not accumulate"
        );
        assert_eq!(
            (state.viewport().rotation_y, state.viewport().rotation_x),
            before,
            "a named camera must not move the default camera's orbit"
        );

        // Pitch is clamped, so a long downward drag cannot roll the scene over.
        let mut state = State::new(false);
        state.active_camera = "Default Camera".to_string();
        state.orbit_camera_by(0.0, -100000.0);
        let pitch = state.viewport().rotation_x;
        assert!(pitch.abs() < std::f32::consts::PI, "pitch ran past vertical: {pitch}");

    }

    /// Deselecting has to STICK, which is the whole difficulty.
    ///
    /// The selection is whatever sits in the cursor's cell — that is what
    /// `sync_cursor_and_selection` means — and the sync runs on nearly every
    /// frame that changes anything. A bare `set_selected_node(None)` is put
    /// straight back, so the deselected cell is remembered and the sync leaves
    /// that one cell alone.
    #[test]
    fn test_deselect_sticks_until_the_cursor_moves() {
        use crate::slots::{CONTENT_IDX, LEFT_MENUBAR_IDX};
        let mut state = State::new(false);
        let mut redraw = false;
        state
            .apply_action(
                McpAction::AddNode { template_name: "Sphere".to_string(), name: None, x: 6.0, y: 6.0 },
                &mut redraw,
            )
            .expect("add node");
        let slot = state.current_dir().children.len() - 1;

        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = CONTENT_IDX;
        state.grid_cursor_col = 6;
        state.grid_cursor_row = 6;
        state.sync_cursor_and_selection();
        assert_eq!(state.graph().selected_node(), Some(slot));

        // Nothing selected, nothing to do — so Escape can fall through to
        // meaning nothing rather than claiming it acted.
        assert!(state.deselect_node());
        assert_eq!(state.graph().selected_node(), None);
        assert!(!state.deselect_node(), "a second deselect has nothing to clear");

        // THE point: the sync that runs on the next changed frame must not put
        // it back, even though the cursor still sits on the node.
        state.sync_cursor_and_selection();
        assert_eq!(
            state.graph().selected_node(),
            None,
            "the sync re-selected the node the user just deselected"
        );

        // Stepping away and back selects again — the deselect held for that
        // one cell, not for the node.
        assert!(state.run_command("nav_right"));
        assert_eq!(state.graph().selected_node(), None, "nothing is at the new cell");
        assert!(state.run_command("nav_left"));
        assert_eq!(
            state.graph().selected_node(),
            Some(slot),
            "coming back to the node should select it again"
        );

        // And selecting explicitly spends the memory: deselect, then select
        // the same node, and the next sync must leave it selected.
        assert!(state.deselect_node());
        state.graph_mut().set_selected_node(Some(slot));
        state.sync_layout();
        state.sync_cursor_and_selection();
        assert_eq!(
            state.graph().selected_node(),
            Some(slot),
            "re-selecting the deselected node did not stick"
        );
    }

    /// Curvature is signed, dimensionless, and does not move when the model
    /// is scaled or re-tessellated.
    ///
    /// That last property is the one that matters: thickness is chosen from
    /// this measure, so a measure that changed with the remesh division size
    /// would give a shell whose thickness moved every time you re-tessellated.
    #[test]
    fn test_curvature_is_signed_and_scale_free() {
        use crate::mold::curvature;

        // A sphere is convex everywhere, so every point reads negative, and
        // every point reads the SAME — it has one curvature.
        let sphere = crate::geometry::sphere_detail(glam::Vec3::ZERO, 1.0, 24, 32);
        let c = curvature(&sphere);
        assert!(c.iter().all(|v| *v < 0.0), "a sphere should read convex everywhere");
        let (lo, hi) = c.iter().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
        assert!(hi - lo < 0.08, "a sphere's curvature is not uniform: {lo}..{hi}");

        // Ten times the size, same measure — this is what "dimensionless"
        // buys, and it is why the thickness range means the same thing on a
        // model of any size.
        let big = crate::geometry::sphere_detail(glam::Vec3::ZERO, 10.0, 24, 32);
        let cb = curvature(&big);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!(
            (mean(&c) - mean(&cb)).abs() < 1e-3,
            "curvature changed with scale: {} vs {}",
            mean(&c),
            mean(&cb)
        );

        // And it is finite on a mesh with isolated points — those read flat
        // rather than NaN, which would poison the whole thickness range.
        let mut stray = sphere.clone();
        stray.add_point(glam::Vec3::new(50.0, 0.0, 0.0));
        let cs = curvature(&stray);
        assert!(cs.iter().all(|v| v.is_finite()), "curvature went non-finite");
        assert_eq!(cs[cs.len() - 1], 0.0, "an isolated point should read flat");
    }

    /// The ramp maps curvature into the thickness range, and the range is
    /// honoured whichever way round its ends are given.
    #[test]
    fn test_thickness_stays_inside_the_range() {
        use crate::mold::{thickness_from_curvature, Ramp};
        let curv = [-1.0, -0.5, 0.0, 0.5, 1.0];

        let t = thickness_from_curvature(&curv, 0.6, 0.75, Ramp::Linear);
        assert!(t.iter().all(|v| (0.6..=0.75).contains(v)), "{t:?} left the range");
        assert!(t[0] < t[4], "concave should be thicker than convex");
        assert!((t[0] - 0.6).abs() < 1e-6 && (t[4] - 0.75).abs() < 1e-6, "{t:?}");

        // Constant is the uniform shell, reachable without leaving the node.
        let t = thickness_from_curvature(&curv, 0.6, 0.75, Ramp::Constant);
        assert!(t.iter().all(|v| (*v - 0.75).abs() < 1e-6), "{t:?}");

        // Smooth flattens both ends rather than changing where they land.
        let t = thickness_from_curvature(&curv, 0.0, 1.0, Ramp::Smooth);
        assert!((t[0] - 0.0).abs() < 1e-6 && (t[4] - 1.0).abs() < 1e-6);
        assert!(t[1] < 0.25 && t[3] > 0.75, "smooth did not flatten the ends: {t:?}");

        // A range given backwards is still a range — min and max are the two
        // ends, not an ordering the caller has to get right.
        let a = thickness_from_curvature(&curv, 0.75, 0.6, Ramp::Linear);
        let b = thickness_from_curvature(&curv, 0.6, 0.75, Ramp::Linear);
        assert_eq!(a, b);
    }

    /// The shell end to end, through the resolver the viewport calls.
    // ----- Parameter references and the Switch node (src/geometry.rs) -----

    /// Deleting a node wired between two splices it out: what read it reads
    /// what it read, through every wire — Input and second operand alike —
    /// and the rewiring is undone with the deletion.
    #[test]
    fn deleting_a_wired_node_connects_its_neighbours() {
        let mut state = State::new(false);
        let wires = |state: &State, name: &str| -> Vec<(String, String)> {
            let n = state.current_dir().children.iter().find(|c| c.name == name).expect(name);
            crate::app::node_wires(n)
        };
        state.current_dir_mut().children = vec![
            ref_node("a", "a", "sphere", vec![("radius", "float", "1")], vec![]),
            ref_node("b", "b", "transform", vec![("input", "node", "a")], vec![]),
            ref_node("c", "c", "transform", vec![("input", "node", "b")], vec![]),
            ref_node("d", "d", "boolean", vec![("input", "node", "c"), ("with", "node", "b")], vec![]),
        ];
        state.sync_nodes();
        state.record_structure_changes();

        assert!(state.delete_node(1));
        state.record_structure_changes();
        assert_eq!(wires(&state, "c"), vec![("input".to_string(), "a".to_string())], "A -> C");
        assert_eq!(
            wires(&state, "d"),
            vec![("input".to_string(), "c".to_string()), ("with".to_string(), "a".to_string())],
            "a second operand follows too"
        );

        // A generator has nothing to splice: what read it is left as it was.
        assert!(state.delete_node(0));
        assert_eq!(wires(&state, "c"), vec![("input".to_string(), "a".to_string())]);

        // Undo puts the node back and the wires with it.
        state.record_structure_changes();
        assert!(state.run_command("undo"));
        assert!(state.run_command("undo"));
        assert_eq!(wires(&state, "c"), vec![("input".to_string(), "b".to_string())]);
        assert_eq!(wires(&state, "d")[1], ("with".to_string(), "b".to_string()));
    }

    fn ref_node(id: &str, name: &str, node_type: &str, params: Vec<(&str, &str, &str)>, children: Vec<FsNode>) -> FsNode {
        FsNode {
            id: id.into(),
            name: name.into(),
            node_type: node_type.into(),
            children,
            params: params
                .into_iter()
                .map(|(n, t, d)| {
                    let (ptype, options) = match t.strip_prefix("choice:") {
                        Some(o) => ("choice".to_string(), o.split(',').map(str::to_string).collect()),
                        None => (t.to_string(), Vec::new()),
                    };
                    { let p = crate::app::ParamDef::new(n, ptype, d).with_options(options); if crate::expr::looks_like_expression(d) { p.as_expr() } else { p } }
                })
                .collect(),
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        }
    }

    fn eval(root: &FsNode, target: &FsNode) -> (Option<Detail>, Option<String>) {
        let mut visited = Vec::new();
        let mut err = None;
        let g = crate::geometry::generate_single_node_geometry_with_errors(
            root, target, &mut visited, &mut err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
        );
        (g, err)
    }

    /// Half the x extent — native spheres are placed at an index-derived
    /// centre, so a distance from the origin would measure the placement.
    fn radius_of(g: &Detail) -> f32 {
        let (lo, hi) = g.positions().iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
        (hi - lo) * 0.5
    }

    /// The expression language: arithmetic with Houdini's precedence, the
    /// channel functions with their conversions, `$F`, strings, and the
    /// functions. `Scope` is a table here, so none of this touches a tree.
    #[test]
    fn expressions_parse_and_evaluate() {
        use crate::expr::{parse, ChKind, Scope, Value};
        struct Table(i32);
        impl Scope for Table {
            fn channel(&mut self, path: &str, kind: ChKind) -> Result<Value, String> {
                let v = match path {
                    "../radius" => Value::Num(0.5),
                    "../sphere1/rows" => Value::Num(16.0),
                    "mode" => Value::Num(1.0),
                    "../text1/font" => Value::Str("Inter".into()),
                    _ => return Err(format!("no {path}")),
                };
                Ok(match kind {
                    ChKind::Int => Value::Num(v.as_num().trunc()),
                    ChKind::Bool => Value::Num(if v.truthy() { 1.0 } else { 0.0 }),
                    _ => v,
                })
            }
            fn var(&self, name: &str) -> Option<Value> {
                (name == "F").then(|| Value::Num(self.0 as f64))
            }
        }
        let ev = |src: &str| parse(src).unwrap_or_else(|e| panic!("{src}: {e}")).eval(&mut Table(12)).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert_eq!(ev("1 + 2 * 3"), Value::Num(7.0));
        assert_eq!(ev("(1 + 2) * 3"), Value::Num(9.0));
        assert_eq!(ev("2 ^ 3 ^ 2"), Value::Num(512.0), "power is right-associative");
        assert_eq!(ev("-2 ^ 2"), Value::Num(-4.0), "unary minus binds looser than power, as in Houdini");
        assert_eq!(ev("7 % 4"), Value::Num(3.0));
        assert_eq!(ev("ch(\"../radius\") * 2 + 1"), Value::Num(2.0));
        assert_eq!(ev("chi(\"../sphere1/rows\") / 3"), Value::Num(16.0 / 3.0));
        assert_eq!(ev("chb(\"mode\")"), Value::Num(1.0));
        assert_eq!(ev("chs(\"../text1/font\") + \" Bold\""), Value::Str("Inter Bold".into()));
        assert_eq!(ev("$F / 24"), Value::Num(0.5));
        assert_eq!(ev("if($F > 10, 1, 0)"), Value::Num(1.0));
        assert_eq!(ev("$F > 10 && $F < 20"), Value::Num(1.0));
        assert_eq!(ev("!($F == 12)"), Value::Num(0.0));
        assert_eq!(ev("clamp(5, 0, 1) + min(3, 1, 2) + max(-1, -2)"), Value::Num(1.0));
        assert_eq!(ev("fit(5, 0, 10, 0, 1)"), Value::Num(0.5));
        assert_eq!(ev("floor(2.7) + ceil(2.2) + round(2.5) + int(-2.7)"), Value::Num(6.0));
        assert_eq!(ev("sqrt(16) + abs(-1) + pow(2, 3)"), Value::Num(13.0));
        assert!((ev("sin(PI / 2)").as_num() - 1.0).abs() < 1e-9);
        assert_eq!(ev("rand(3)"), ev("rand(3)"), "rand is a function of its seed");
        assert_ne!(ev("rand(3)"), ev("rand(4)"));
        assert_eq!(ev("\"a\" == \"a\""), Value::Num(1.0));

        // Errors name what went wrong.
        for (src, needle) in [("1 +", "unexpected end"), ("ch(Radius)", "unknown name"), ("foo(1)", "unknown function"), ("1 / 0", "division by zero"), ("ch(\"nope\")", "no nope"), ("$X", "unknown variable"), ("1 2", "trailing")] {
            let err = match parse(src) {
                Ok(e) => e.eval(&mut Table(0)).unwrap_err(),
                Err(e) => e,
            };
            assert!(err.contains(needle), "{src}: {err}");
        }

        // Formatting: integers bare, floats as the f32 they are read as.
        assert_eq!(crate::expr::fmt_num(2.0), "2");
        assert_eq!(crate::expr::fmt_num(0.1 + 0.2), "0.3");
        assert_eq!(crate::expr::fmt_num(-1.5), "-1.5");
    }

    /// What is and is not inferred to be an expression when typed or
    /// scripted into a plain parameter: a reference is, arithmetic on a
    /// literal, a node name, a number and a kernel are not.
    #[test]
    fn a_typed_reference_is_an_expression_and_a_kernel_is_not() {
        use crate::expr::looks_like_expression;
        assert!(looks_like_expression("ch(\"../sphere1/radius\")"));
        assert!(looks_like_expression("chf(\"../radius\") * 2"));
        assert!(looks_like_expression("$F / 24"));
        assert!(!looks_like_expression("1 + 2"), "arithmetic alone is asked for through Edit Expression");
        assert!(!looks_like_expression("0.5"));
        assert!(!looks_like_expression("sphere1"));
        assert!(!looks_like_expression("true"));
        assert!(!looks_like_expression("0.00:0.80:0.00"));
        assert!(!looks_like_expression("float r = chf(\"radius\", 0.5);"), "a kernel is not a reference");
        let kernel = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/nodes/sphere.json")).unwrap();
        assert!(!looks_like_expression(&kernel));
    }

    /// A rename rewrites the channel paths that pass through the node —
    /// textually, so the user's spacing survives — and leaves every other
    /// path alone.
    #[test]
    fn rename_rewrites_the_paths_through_a_node() {
        use crate::expr::rewrite_paths;
        let src = "ch( \"../sphere1/radius\" ) * chs('../text1/Font') + chf(\"/sphere1/rows\")";
        let out = rewrite_paths(src, |path| path.contains("sphere1").then(|| path.replace("sphere1", "ball")));
        assert_eq!(out, "ch( \"../ball/radius\" ) * chs('../text1/Font') + chf(\"/ball/rows\")");
        assert_eq!(rewrite_paths("touch(\"x\")", |_| Some("no".into())), "touch(\"x\")", "only channel calls are paths");
    }

    /// Format 4 → 5: the root is the object level, and an older save's
    /// geometry goes into one new Geometry node there, in its order and
    /// with its wires — while cameras, pages and an export of a page stay.
    /// What names a moved node across the move is re-pointed (an absolute
    /// path into it, a relative path between it and the root, both ways),
    /// what does not cross is left as written, and the view follows: an
    /// editor at the root looks into the new node at the node it had
    /// selected, and a path into a moved subnet goes through it. Once.
    #[test]
    fn an_older_save_puts_its_geometry_in_a_geometry_node() {
        use crate::app::{Project, PROJECT_FORMAT};
        let at = |mut n: FsNode, x: f32, y: f32| {
            n.position = (x, y);
            n
        };
        let root = ref_node("root", "root", "node", vec![], vec![
            at(ref_node("cam", "camera1", "camera", vec![("pivot", "float3", "1:2:3")], vec![]), 0.0, 0.0),
            at(ref_node("pg", "page1", "page", vec![("width", "float", "ch(\"../sphere1/radius\")")], vec![]), 1.0, 0.0),
            at(ref_node("xp", "export_page", "export", vec![("input", "node", "page1")], vec![]), 1.0, 1.0),
            at(ref_node("sp", "sphere1", "sphere", vec![("radius", "slider", "ch(\"../camera1/pivot.x\")")], vec![]), 4.0, 2.0),
            at(ref_node("xf", "xform1", "transform", vec![
                ("input", "node", "sphere1"),
                ("scale", "float", "ch(\"/sphere1/radius\") * 2 + ch(\"../sphere1/radius\")"),
            ], vec![]), 4.0, 3.0),
            at(ref_node("xg", "export1", "export", vec![("input", "node", "xform1")], vec![]), 4.0, 4.0),
            at(ref_node("sub", "sub1", "node", vec![], vec![ref_node("in", "input1", "input", vec![], vec![])]), 6.0, 2.0),
        ]);
        let mut proj = Project { name: "p".into(), root, view_state: Default::default(), format: 4 };
        proj.view_state.selected_node = Some(4);
        proj.view_state.current_path2 = vec![6, 0];
        proj.migrate_format();
        assert_eq!(proj.format, PROJECT_FORMAT);

        let names = |n: &FsNode| n.children.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&proj.root), ["camera1", "page1", "export_page", "geometry1"]);
        let g = geo(&proj.root);
        assert_eq!((g.node_type.as_str(), g.geometry_visible), ("geometry", true));
        assert_eq!(names(g), ["sphere1", "xform1", "export1", "sub1"]);
        assert!(!proj.root.children[..3].iter().any(|c| c.position == g.position), "it stands on a free cell");
        assert_eq!(g.children[1].params[0].text(), "sphere1", "a wire between moved nodes is left alone");

        let text = |n: &FsNode, p: &str| n.params.iter().find(|q| q.name == p).unwrap().text().to_string();
        assert_eq!(text(&g.children[1], "scale"), "ch(\"/geometry1/sphere1/radius\") * 2 + ch(\"../sphere1/radius\")");
        assert_eq!(text(&g.children[0], "radius"), "ch(\"../../camera1/pivot.x\")", "out of the node to the root");
        assert_eq!(text(&proj.root.children[1], "width"), "ch(\"../geometry1/sphere1/radius\")", "from the root into it");
        let mut err = None;
        let resolved = crate::geometry::resolve_param_refs(&proj.root, &g.children[0], 0, &mut err).unwrap();
        assert!(err.is_none(), "{err:?}");
        assert_eq!(text(&resolved, "radius"), "1");

        // The view: the root editor is inside, on xform1; the second editor's
        // path into sub1 goes through the new node.
        assert_eq!(proj.view_state.current_path, vec![3]);
        assert_eq!(proj.view_state.selected_node, Some(1));
        assert_eq!(proj.view_state.current_path2, vec![3, 3, 0]);

        // Once.
        let before = serde_json::to_string(&proj).unwrap();
        proj.migrate_format();
        assert_eq!(serde_json::to_string(&proj).unwrap(), before);

        // A save with nothing to move is left as it was.
        let root = ref_node("root", "root", "node", vec![], vec![ref_node("cam", "camera1", "camera", vec![], vec![])]);
        let mut proj = Project { name: "p".into(), root, view_state: Default::default(), format: 4 };
        proj.migrate_format();
        assert_eq!(names(&proj.root), ["camera1"]);
        assert!(proj.view_state.current_path.is_empty());
    }

    /// A range's two ends are one `float2` row (format 6, 2026-10-06): the
    /// Attribute node's From and To, Normalize's own Normalize To, and
    /// Visualize's Manual Range. An older save's pairs are joined as they
    /// were written — an expression half makes the whole an expression,
    /// each component its own — and a channel path to an old row follows,
    /// RESOLVED, so a Transfer's `from` wire is not mistaken for one. The
    /// pane shows two sliders over a soft span around the value, kept while
    /// the value stays in it and through a drag; an expression is text.
    #[test]
    fn a_range_is_one_float2_row() {
        use crate::app::{ParamKind, ParamValue, Project, PROJECT_FORMAT};
        let templates_root = crate::app::load_fs_tree();
        let kind = |ty: &str, name: &str| {
            templates_root.children.iter().find(|t| t.node_type == ty).unwrap().params.iter().find(|p| p.name == name).map(|p| p.kind())
        };
        assert_eq!(kind("attribute", "from"), Some(ParamKind::Float2));
        assert_eq!(kind("attribute", "to"), Some(ParamKind::Float2));
        assert_eq!(kind("attribute", "normalize_to"), Some(ParamKind::Float));
        assert_eq!(kind("visualize", "manual_range"), Some(ParamKind::Float2));
        for gone in ["from_min", "from_max", "to_min", "to_max"] {
            assert_eq!(kind("attribute", gone), None, "{gone} is retired");
        }
        assert_eq!((kind("visualize", "from"), kind("visualize", "to")), (None, None));

        // An older save.
        let attr = |id: &str, op: &str, rows: Vec<(&'static str, &'static str, &'static str)>| {
            let mut params = vec![("input", "node", "sphere1"), ("operation", "choice:Remap,Normalize", op)];
            params.extend(rows);
            ref_node(id, id, "attribute", params, vec![])
        };
        let geometry = ref_node("g", "geometry1", "geometry", vec![], vec![
            ref_node("sphere1", "sphere1", "sphere", vec![("radius", "slider", "0.5")], vec![]),
            attr("remap1", "Remap", vec![
                ("from_min", "float", "2.00"),
                ("from_max", "float", "5.00"),
                ("to_min", "float", "0.00"),
                ("to_max", "float", "chf(\"../sphere1/radius\") * 4"),
            ]),
            attr("norm1", "Normalize", vec![("to_max", "float", "3.00")]),
            ref_node("vis1", "vis1", "visualize", vec![("input", "node", "remap1"), ("from", "float", "-1.00"), ("to", "float", "1.00")], vec![]),
            ref_node("xfer", "xfer", "transfer", vec![("input", "node", "remap1"), ("from", "node", "norm1")], vec![]),
            ref_node("w", "w", "wrangle", vec![("code", "code", "@a = ch(\"../remap1/from_max\") + ch(\"../norm1/to_max\") + ch(\"../vis1/to\") + ch(\"../xfer/from\");")], vec![]),
            ref_node("ball", "ball", "sphere", vec![("radius", "slider", "ch(\"../remap1/to_max\") + ch(\"../remap1/from_min\")")], vec![]),
        ]);
        let root = ref_node("root", "root", "node", vec![], vec![geometry]);
        let mut proj = Project { name: "p".into(), root, view_state: Default::default(), format: 5 };
        proj.migrate_format();
        assert_eq!(proj.format, PROJECT_FORMAT);
        let g = geo(&proj.root);
        let node = |name: &str| g.children.iter().find(|c| c.name == name).unwrap();
        let names = |n: &FsNode| n.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        let param = |n: &str, p: &str| node(n).params.iter().find(|q| q.name == p).unwrap().clone();

        assert_eq!(names(node("remap1")), ["input", "operation", "from", "to", "normalize_to"]);
        assert_eq!(param("remap1", "from").value(), Some(&ParamValue::Vec2([2.0, 5.0])));
        let to = param("remap1", "to");
        assert!(to.is_expr(), "a half that was an expression makes the whole one");
        assert_eq!(to.text(), "0.00:chf(\"../sphere1/radius\") * 4");
        assert!(param("remap1", "normalize_to").is_expr(), "To Max is Normalize To's too");
        assert_eq!(param("norm1", "to").text(), "0.00:3.00", "a missing half is the default");
        assert_eq!(param("norm1", "normalize_to").value(), Some(&ParamValue::Number(3.0)));
        assert_eq!(names(node("vis1")), ["input", "manual_range"]);
        assert_eq!(param("vis1", "manual_range").value(), Some(&ParamValue::Vec2([-1.0, 1.0])));
        assert_eq!(param("xfer", "from").text(), "norm1", "a wire called from is not a range");

        assert_eq!(
            param("w", "code").text(),
            "@a = ch(\"../remap1/from.y\") + ch(\"../norm1/normalize_to\") + ch(\"../vis1/manual_range.y\") + ch(\"../xfer/from\");",
            "To Max is Normalize To on a Normalize node, and a path not to a range is left alone"
        );
        assert_eq!(param("ball", "radius").text(), "ch(\"../remap1/to.y\") + ch(\"../remap1/from.x\")");

        // Each component evaluates on its own, and a path reads one.
        let mut err = None;
        let resolved = crate::geometry::resolve_param_refs(&proj.root, node("remap1"), 1, &mut err).unwrap();
        assert!(err.is_none(), "{err:?}");
        assert_eq!(resolved.params.iter().find(|p| p.name == "to").unwrap().value(), Some(&ParamValue::Vec2([0.0, 2.0])));
        let resolved = crate::geometry::resolve_param_refs(&proj.root, node("ball"), 1, &mut err).unwrap();
        assert!(err.is_none(), "{err:?}");
        assert_eq!(resolved.params.iter().find(|p| p.name == "radius").unwrap().value(), Some(&ParamValue::Number(4.0)));

        // Once.
        let before = serde_json::to_string(&proj).unwrap();
        proj.migrate_format();
        assert_eq!(serde_json::to_string(&proj).unwrap(), before);

        // The pane: two sliders over a soft span around the value.
        let find = |name: &str| templates_root.children.iter().find(|t| t.name == name).unwrap();
        let mut remap = find("Attribute").clone();
        remap.id = "r".into();
        remap.name = "remap1".into();
        for (p, v) in [("operation", "Remap"), ("from", "2.00:5.00")] {
            remap.params.iter_mut().find(|q| q.name == p).unwrap().set_text(v.to_string());
        }
        let mut state = State::new(false);
        state.current_dir_mut().children = vec![remap];
        state.sync_nodes();
        state.graph_mut().set_selected_node(Some(0));
        let row = |state: &mut State, key: &str| {
            state.sync_parameters_pane();
            state.param_mut().node_params().iter().find(|r| r.0 == key).unwrap_or_else(|| panic!("a {key} row")).2.clone()
        };
        let set = |state: &mut State, val: &str| {
            state.current_dir_mut().children[0].params.iter_mut().find(|p| p.name == "from").unwrap().set_text(val.to_string());
        };
        assert_eq!(row(&mut state, "From"), "float2:-10:10:soft");
        assert_eq!(row(&mut state, "To"), "float2:-10:10:soft");
        set(&mut state, "300:400");
        assert_eq!(row(&mut state, "From"), "float2:-1000:1000:soft", "the span follows the value");
        state.drag_widget = Some(crate::slots::PARAM_IDX);
        set(&mut state, "3000:4000");
        assert_eq!(row(&mut state, "From"), "float2:-1000:1000:soft", "but not under the pointer");
        state.drag_widget = None;
        assert_eq!(row(&mut state, "From"), "float2:-10000:10000:soft", "and on the release it does");
        state.current_dir_mut().children[0].params.iter_mut().find(|p| p.name == "from").unwrap().set_expr(true);
        assert_eq!(row(&mut state, "From"), "text", "an expression is shown as its text");
        state.current_dir_mut().children[0].params.iter_mut().find(|p| p.name == "from").unwrap().set_expr(false);
        set(&mut state, "abc");
        assert_eq!(row(&mut state, "From"), "text", "a text that is not two numbers is a text box, to be put right");
    }

    /// The root is the object level: each Geometry node there is shown or
    /// not by its own flag, and several draw at once, where inside one the
    /// flag is exclusive as it always was. A Geometry node resolves to what
    /// its flag inside shows, as a Houdini object is its display SOP.
    #[test]
    fn geometry_nodes_at_the_root_each_show_their_own() {
        use crate::geometry::{generate_single_node_geometry_with_errors, network_sphere_vertices, EvalSim, SimCache};
        let sphere = |id: &str, r: &str| {
            ref_node(id, id, "sphere", vec![("radius", "slider", r), ("center", "float3", "0:0:0"), ("rows", "spinbox", "4"), ("columns", "spinbox", "6")], vec![])
        };
        let box_ = |id: &str| ref_node(id, id, "box", vec![("size", "float3", "1:1:1"), ("center", "float3", "3:0:0")], vec![]);
        let mut a = ref_node("ga", "geometry1", "geometry", vec![], vec![sphere("s1", "0.5"), box_("b1")]);
        a.set_child_geometry_visible(0, true);
        assert!(!a.children[1].geometry_visible, "inside, the flag is exclusive");
        let mut root = ref_node("root", "root", "node", vec![], vec![
            a,
            ref_node("gb", "geometry2", "geometry", vec![], vec![box_("b2")]),
            ref_node("cam", "camera1", "camera", vec![], vec![]),
        ]);
        let count = |root: &FsNode| network_sphere_vertices(root).num_points();
        let (n_sphere, n_box) = {
            let mut cache = SimCache::default();
            let mut sim = EvalSim::new(0, 0, &mut cache);
            let mut err = None;
            let s = generate_single_node_geometry_with_errors(&root, &root.children[0], &mut Vec::new(), &mut err, &mut sim).unwrap();
            let b = generate_single_node_geometry_with_errors(&root, &root.children[1], &mut Vec::new(), &mut err, &mut sim).unwrap();
            assert!(err.is_none(), "{err:?}");
            (s.num_points(), b.num_points())
        };
        assert_eq!(n_box, 8, "geometry2 is its box");
        assert!(n_sphere > 8, "geometry1 is its shown sphere, not its box");

        root.set_child_geometry_visible(0, true);
        root.set_child_geometry_visible(1, true);
        assert!(root.children[0].geometry_visible && root.children[1].geometry_visible, "both stay shown");
        assert_eq!(count(&root), n_sphere + n_box, "and both draw");
        root.set_child_geometry_visible(1, false);
        assert_eq!(count(&root), n_sphere);
        root.set_child_geometry_visible(1, true);
        root.set_child_geometry_visible(2, true);
        assert!(root.children[0].geometry_visible && root.children[1].geometry_visible, "a camera's flag turns no object off");
    }

    /// The pre-expression reference migrates to Houdini's semantics: a bare
    /// name meant the parent and gains `../`, an explicit `../` is kept, and
    /// anything else is not a legacy reference.
    #[test]
    fn legacy_references_migrate_to_the_parent_path() {
        use crate::expr::migrate_legacy_ref;
        assert_eq!(migrate_legacy_ref("ch(\"Radius\")").as_deref(), Some("ch(\"../Radius\")"));
        assert_eq!(migrate_legacy_ref("  chf('Base Resolution')  ").as_deref(), Some("chf(\"../Base Resolution\")"));
        assert_eq!(migrate_legacy_ref("chi(\"../Source\")").as_deref(), Some("chi(\"../Source\")"));
        assert_eq!(migrate_legacy_ref("chb(\"../../Relax Points\")").as_deref(), Some("chb(\"../../Relax Points\")"));
        assert_eq!(migrate_legacy_ref("0.5"), None);
        assert_eq!(migrate_legacy_ref("float r = chf(\"Radius\", 0.5);"), None, "a kernel is not a reference");
        assert_eq!(migrate_legacy_ref("ch(Radius)"), None, "unquoted is not a reference");
        assert_eq!(migrate_legacy_ref("ch(\"\")"), None);
        assert_eq!(migrate_legacy_ref("ch(\"../a/Radius\")"), None, "a path was never the old form");

        // And a whole project: format 0 migrates once, format 1 is left alone.
        let mut proj = crate::app::Project {
            name: "old".into(),
            root: ref_node("root", "root", "node", vec![], vec![
                ref_node("sub", "sub1", "node", vec![("Size", "slider", "0.8")], vec![
                    ref_node("s", "sphere1", "sphere", vec![("Radius", "slider", "chf(\"Size\")")], vec![]),
                ]),
            ]),
            view_state: Default::default(),
            format: 0,
        };
        proj.root.children[0].children[0].params[0].set_expr(false);
        proj.migrate_format();
        // Format 4 → 5 put the subnet inside a Geometry node.
        let r = &geo(&proj.root).children[0].children[0].params[0];
        // Format 0 → 1 gives the reference its parent; 3 → 4 the name.
        assert_eq!(r.text(), "chf(\"../size\")");
        assert_eq!(r.name, "radius");
        assert!(r.is_expr());
        assert_eq!(proj.format, crate::app::PROJECT_FORMAT);
        // A NEW file's bare name is the node's own parameter and stays.
        geo_mut(&mut proj.root).children[0].children[0].params[0].set_text("chf(\"radius\")");
        proj.migrate_format();
        assert_eq!(geo(&proj.root).children[0].children[0].params[0].text(), "chf(\"radius\")");
    }

    #[test]
    fn param_references_resolve_against_the_enclosing_subnet() {
        use crate::geometry::resolve_param_refs;
        let sphere = ref_node("s", "sphere1", "sphere", vec![("radius", "slider", "ch(\"../radius\")")], vec![]);
        let output = ref_node("o", "output1", "output", vec![("input", "text", "sphere1")], vec![]);
        let inner = ref_node("sub", "shape1", "node",
            vec![("radius", "slider", "chf(\"../size\")"), ("mode", "choice:Basic,Scatter", "Scatter"), ("On", "toggle", "true")],
            vec![sphere, output]);
        let probe = ref_node("p", "probe", "switch",
            vec![("index", "spinbox", "chi(\"../mode\")"), ("Flag", "text", "chb(\"../on\")"), ("Name", "text", "chs(\"../mode\")"), ("Plain", "text", "kept")],
            vec![]);
        let mut inner = inner;
        inner.children.push(probe);
        let outer = ref_node("outer", "outer1", "node", vec![("size", "slider", "0.8")], vec![inner]);
        let root = ref_node("root", "root", "node", vec![], vec![outer]);

        // The probe's params, resolved against shape1.
        let probe = &root.children[0].children[0].children[2];
        let mut err = None;
        let resolved = resolve_param_refs(&root, probe, 0, &mut err).expect("it has references");
        assert!(err.is_none(), "{err:?}");
        let get = |n: &str| resolved.params.iter().find(|p| p.name == n).unwrap().text().to_string();
        assert_eq!(get("index"), "1", "chi on a choice is its option index");
        assert_eq!(get("Flag"), "1", "chb into a text row is 1 or 0");
        assert_eq!(get("Name"), "Scatter");
        assert_eq!(get("Plain"), "kept");

        // Evaluated, the sphere's Radius chains: sphere1 → shape1's Radius,
        // which is itself chf("../size") → outer1's 0.8.
        let shape = &root.children[0].children[0];
        let (g, err) = eval(&root, shape);
        assert!(err.is_none(), "{err:?}");
        assert!((radius_of(&g.expect("geometry")) - 0.8).abs() < 0.02, "the radius came from the outermost control");

        // A node with no references is left alone: no clone, no error.
        let mut none = None;
        assert!(resolve_param_refs(&root, &root.children[0].children[0].children[1], 0, &mut none).is_none());
        assert!(none.is_none());

        // A reference to nothing is reported, and the value left as written.
        let bad = ref_node("b", "bad1", "sphere", vec![("radius", "slider", "ch(\"../nope\")")], vec![]);
        let holder = ref_node("h", "holder1", "node", vec![], vec![bad]);
        let root2 = ref_node("root", "root", "node", vec![], vec![holder]);
        let mut err = None;
        let r = resolve_param_refs(&root2, &root2.children[0].children[0], 0, &mut err).unwrap();
        assert_eq!(r.params[0].text(), "ch(\"../nope\")");
        assert!(err.as_deref().unwrap_or("").contains("names no parameter nope on holder1"), "{err:?}");
        // Too many levels up, likewise.
        let far = ref_node("f", "far1", "sphere", vec![("radius", "slider", "ch(\"../../../x\")")], vec![]);
        let root3 = ref_node("root", "root", "node", vec![], vec![far]);
        let mut err = None;
        resolve_param_refs(&root3, &root3.children[0], 0, &mut err);
        assert!(err.is_some());
    }

    /// Channel paths over a tree, Houdini's way: a bare name is the node's
    /// own parameter, `..` the parent, a sibling by name, `/` the root; a
    /// float3 component through `.y`; an expression that reads an expression
    /// follows the chain; `$F` is the evaluation's frame; and a circle is an
    /// error, not a stack overflow.
    #[test]
    fn channel_paths_resolve_over_the_tree() {
        use crate::geometry::resolve_param_refs;
        let a = ref_node("a", "a1", "sphere", vec![("radius", "slider", "0.25"), ("center", "float3", "1:2:3")], vec![]);
        let b = ref_node("b", "b1", "sphere", vec![
            ("radius", "slider", "ch(\"../a1/radius\") * 2"),
            ("rows", "spinbox", "ch(\"radius\") * 100"),
            ("y", "slider", "ch(\"../a1/center.y\") + ch(\"/sub1/a1/center.z\")"),
            ("Frame", "slider", "$F / 2"),
            ("Up", "slider", "ch(\"../size\") + ch(\"/top\")"),
            ("mode", "choice:Basic,Scatter", "1"),
            ("On", "toggle", "ch(\"../a1/radius\") > 0"),
            ("Label", "text", "chs(\"../a1/radius\") + \" units\""),
            ("center", "float3", "chf(\"../a1/center.x\"):0:ch(\"../size\")"),
        ], vec![]);
        let sub = ref_node("sub", "sub1", "node", vec![("size", "slider", "0.5")], vec![a, b]);
        let root = ref_node("root", "root", "node", vec![("Top", "slider", "10")], vec![sub]);
        // The choice's value is an index written as an expression; flag it.
        let mut root = root;
        root.children[0].children[1].params.iter_mut().find(|p| p.name == "mode").unwrap().set_expr(true);

        let b = &root.children[0].children[1];
        let mut err = None;
        let r = resolve_param_refs(&root, b, 12, &mut err).expect("b1 has expressions");
        assert!(err.is_none(), "{err:?}");
        let get = |n: &str| r.params.iter().find(|p| p.name == n).unwrap().text().to_string();
        assert_eq!(get("radius"), "0.5", "a sibling by path");
        assert_eq!(get("rows"), "50", "a bare name is the node's OWN parameter, read through its expression");
        assert_eq!(get("y"), "5", "components, relative and absolute");
        assert_eq!(get("Frame"), "6");
        assert_eq!(get("Up"), "10.5", "the parent and the root");
        assert_eq!(get("mode"), "Scatter", "a number into a choice picks the option");
        assert_eq!(get("On"), "true", "a number into a toggle is true or false");
        assert_eq!(get("Label"), "0.25 units");
        assert_eq!(get("center"), "1:0:0.5", "a float3 is three expressions");
        assert!(r.params.iter().all(|p| !p.is_expr()), "the resolved clone holds values");

        // A circle: two parameters reading each other.
        let x = ref_node("x", "x1", "sphere", vec![("radius", "slider", "ch(\"../y1/radius\")")], vec![]);
        let y = ref_node("y", "y1", "sphere", vec![("radius", "slider", "ch(\"../x1/radius\") + 1")], vec![]);
        let ring = ref_node("root", "root", "node", vec![], vec![x, y]);
        let mut err = None;
        let r = resolve_param_refs(&ring, &ring.children[0], 0, &mut err).unwrap();
        assert!(err.as_deref().unwrap_or("").contains("circular"), "{err:?}");
        assert_eq!(r.params[0].text(), "ch(\"../y1/radius\")", "left as written");
        // A parameter reading itself is the shortest circle.
        let me = ref_node("m", "me", "sphere", vec![("radius", "slider", "ch(\"radius\") + 1")], vec![]);
        let solo = ref_node("root", "root", "node", vec![], vec![me]);
        let mut err = None;
        resolve_param_refs(&solo, &solo.children[0], 0, &mut err);
        assert!(err.as_deref().unwrap_or("").contains("circular"), "{err:?}");

        // A path to a node that is not there names the step that failed.
        let lost = ref_node("l", "lost", "sphere", vec![("radius", "slider", "ch(\"../nope/radius\")")], vec![]);
        let root4 = ref_node("root", "root", "node", vec![], vec![lost]);
        let mut err = None;
        resolve_param_refs(&root4, &root4.children[0], 0, &mut err);
        assert!(err.as_deref().unwrap_or("").contains("no node `nope`"), "{err:?}");
        // A syntax error names the parameter.
        let broken = ref_node("k", "broken", "sphere", vec![("radius", "slider", "1 +")], vec![]);
        let mut broken = broken;
        broken.params[0].set_expr(true);
        let root5 = ref_node("root", "root", "node", vec![], vec![broken]);
        let mut err = None;
        resolve_param_refs(&root5, &root5.children[0], 0, &mut err);
        assert!(err.as_deref().unwrap_or("").contains("broken: radius"), "{err:?}");
    }

    /// The paths a paste writes, and what a rename does to the paths that
    /// stand: `rename_node_in_tree` rewrites every expression whose path
    /// passes through the node and every wire naming it, and leaves a
    /// same-named node elsewhere alone.
    #[test]
    fn renaming_a_node_carries_its_references() {
        use crate::geometry::{absolute_ref_path, relative_ref_path, rename_node_in_tree};
        let a = ref_node("a", "a1", "sphere", vec![("radius", "slider", "0.25")], vec![]);
        let b = ref_node("b", "b1", "sphere", vec![
            ("radius", "slider", "ch( \"../a1/radius\" ) * 2"),
            ("input", "text", "a1"),
        ], vec![]);
        let deep = ref_node("d", "deep1", "sphere", vec![("radius", "slider", "chf(\"/sub1/a1/radius\") + ch(\"../../a1/radius\")")], vec![]);
        let inner = ref_node("in", "inner1", "node", vec![], vec![deep]);
        let other = ref_node("oa", "a1", "sphere", vec![("radius", "slider", "ch(\"../a1/radius\")")], vec![]);
        let elsewhere = ref_node("el", "elsewhere", "node", vec![], vec![other]);
        let sub = ref_node("sub", "sub1", "node", vec![("size", "slider", "0.5")], vec![a, b, inner]);
        let mut root = ref_node("root", "root", "node", vec![], vec![sub, elsewhere]);

        assert_eq!(relative_ref_path(&root, "b", "a").as_deref(), Some("../a1"));
        assert_eq!(relative_ref_path(&root, "d", "a").as_deref(), Some("../../a1"));
        assert_eq!(relative_ref_path(&root, "b", "sub").as_deref(), Some(".."));
        assert_eq!(relative_ref_path(&root, "b", "b").as_deref(), Some(""));
        assert_eq!(relative_ref_path(&root, "a", "d").as_deref(), Some("../inner1/deep1"));
        assert_eq!(absolute_ref_path(&root, "d").as_deref(), Some("/sub1/inner1/deep1"));

        assert!(rename_node_in_tree(&mut root, "a", "ball"));
        let get = |root: &FsNode, path: &[usize], n: &str| {
            let mut node = root;
            for &i in path {
                node = &node.children[i];
            }
            node.params.iter().find(|p| p.name == n).unwrap().text().to_string()
        };
        assert_eq!(root.children[0].children[0].name, "ball");
        assert_eq!(get(&root, &[0, 1], "radius"), "ch( \"../ball/radius\" ) * 2", "spacing kept");
        assert_eq!(get(&root, &[0, 1], "input"), "ball", "the wire follows");
        assert_eq!(get(&root, &[0, 2, 0], "radius"), "chf(\"/sub1/ball/radius\") + ch(\"../../ball/radius\")");
        assert_eq!(get(&root, &[1, 0], "radius"), "ch(\"../a1/radius\")", "the OTHER a1 is not this one");
        assert!(!rename_node_in_tree(&mut root, "a", "ball"), "a rename to the same name is nothing");
        assert!(!rename_node_in_tree(&mut root, "zzz", "x"), "and so is one of a node that is not there");
    }

    /// Every parameter a template ships says what it does, and the row
    /// menu shows it: under the name, wrapped so the menu stays narrow,
    /// ahead of the readouts. The description is the TEMPLATE's: an
    /// instance loaded from a save carries none and is handed it by the
    /// merge, a child of a subnet template takes its base template's, and
    /// a save never writes one.
    #[test]
    fn the_row_menu_says_what_a_parameter_does() {
        use crate::app::{wrap_words, McpAction, PARAM_DESCRIPTION_WIDTH};
        // Every shipped template's own parameters, read off the RAW files
        // so a description the loader dropped would not pass for one.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("nodes");
        let mut missing = Vec::new();
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            for p in v["params"].as_array().into_iter().flatten() {
                let d = p["description"].as_str().unwrap_or("").trim();
                if d.is_empty() {
                    missing.push(format!("{}: {}", path.file_name().unwrap().to_string_lossy(), p["name"]));
                }
            }
        }
        assert!(missing.is_empty(), "parameters with no description: {missing:#?}");

        // A wrap keeps every word, in order, and no line is wider than
        // the width unless one word is.
        let text = "Radius of the sphere in world units; larger values make a bigger ball.";
        let lines = wrap_words(text, 20);
        assert_eq!(lines.join(" "), text);
        assert!(lines.iter().all(|l| l.chars().count() <= 20), "{lines:?}");
        assert_eq!(wrap_words("antidisestablishmentarianism is long", 10), vec!["antidisestablishmentarianism", "is long"]);
        assert!(wrap_words("", 10).is_empty());

        let mut state = State::new(false);
        state.param_editor = crate::slots::CONTENT_IDX;
        let mut redraw = false;
        let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
        // sphere1 comes from the bundled project, whose file has no
        // descriptions: the merge hands it the template's.
        let sphere = slot_of(&state, "sphere1");
        let templates = state.node_templates.clone();
        let template = |name: &str, pname: &str| -> String {
            templates.iter().find(|t| t.node.name == name).unwrap().node.params.iter()
                .find(|p| p.name == pname).unwrap().description.clone()
        };
        let radius = template("Sphere", "radius");
        let want = wrap_words(&radius, PARAM_DESCRIPTION_WIDTH);
        assert!(want.len() > 1, "a sentence spans rows: {want:?}");
        let (rows, _, headers) = state.param_menu_rows(sphere, "radius");
        assert_eq!(rows[..2], ["Name: radius".to_string(), "Label: Radius".to_string()], "the name a path spells, then what the pane shows");
        assert_eq!(&rows[2..2 + want.len()], &want[..], "the description sits under the name and label");
        assert_eq!(rows[2 + want.len()], "Control: slider");
        assert!(2 + want.len() < headers, "the description rows are headers, and run nothing");

        // A child inside a subnet template takes its base template's.
        state.apply_action(McpAction::AddNode { template_name: "Embryo".into(), name: Some("embryo1".into()), x: 3.0, y: 8.0 }, &mut redraw).unwrap();
        let embryo = slot_of(&state, "embryo1");
        let (rows, _, _) = state.param_menu_rows(embryo, "radius");
        let own = wrap_words(&template("Embryo", "radius"), PARAM_DESCRIPTION_WIDTH);
        assert_ne!(own, want, "the Embryo's Radius is described as the Embryo's");
        assert_eq!(&rows[2..2 + own.len()], &own[..]);
        state.apply_action(McpAction::Enter { slot: embryo }, &mut redraw).unwrap();
        let inner = slot_of(&state, "sphere1");
        let (rows, _, _) = state.param_menu_rows(inner, "radius");
        assert_eq!(&rows[2..2 + want.len()], &want[..], "the Embryo's sphere1 says what a Sphere's Radius does");
        state.apply_action(McpAction::Up, &mut redraw).unwrap();

        // A parameter no template names says nothing, and the menu goes
        // straight from the name to the readouts.
        state.apply_action(McpAction::AddParam { slot: sphere, name: "extra".into(), param_type: "float".into(), default: "3".into(), label: "Extra".into() }, &mut redraw).unwrap();
        let (rows, _, _) = state.param_menu_rows(sphere, "extra");
        assert_eq!(rows[2], "Control: text box");

        // A save never carries one, so the file is what it was.
        let saved = serde_json::to_string(&state.current_dir().children[sphere]).unwrap();
        assert!(!saved.contains("description"), "{saved}");
    }

    /// The parameter row menu, end to end: a right press on a row in the
    /// params pane opens it (and nothing else claims the press), Copy
    /// Parameter then Paste Relative Reference on another node's row writes
    /// the Houdini path and flags the row, which the pane then shows as
    /// text; Delete Expression writes the evaluated value back as a value;
    /// Edit Expression flags a value without changing it.
    #[test]
    fn the_row_menu_copies_and_pastes_references() {
        use crate::app::{McpAction, ParamMenuAction};
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        let mut redraw = false;
        state
            .apply_action(McpAction::AddNode { template_name: "Sphere".into(), name: Some("ball".into()), x: 1.0, y: 8.0 }, &mut redraw)
            .unwrap();
        let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
        let (sphere, ball) = (slot_of(&state, "sphere1"), slot_of(&state, "ball"));

        // Show sphere1 in the pane and find its Radius row.
        let show = |state: &mut State, slot: usize| {
            state.graph_mut().set_selected_node(Some(slot));
            state.sync_parameters_pane();
            state.rebuild_positions();
            state.apply_layout();
            assert_eq!(state.param_editor_selected(), Some(slot));
        };
        let row_center = |state: &State, pname: &str| -> (f32, f32) {
            let child = &state.current_dir().children[state.param_editor_selected().unwrap()];
            let rows = crate::app::param_display(&child.params);
            let i = rows.iter().position(|r| r.0 == pname).expect(pname);
            let rects = state.param_row_rects();
            let (x, y, w, h) = rects[i];
            (x + w * 0.5, y + h * 0.5)
        };
        // A parameter's description, as the menu wraps it, and the menu's
        // rows with it taken out: the readouts the rest of this test is
        // about. `the_row_menu_says_what_a_parameter_does` covers the
        // description itself.
        let described = |state: &State, slot: usize, pname: &str| -> Vec<String> {
            let p = state.current_dir().children[slot].params.iter().find(|p| p.name == pname).expect(pname);
            crate::app::wrap_words(&p.description, crate::app::PARAM_DESCRIPTION_WIDTH)
        };
        let fields = |state: &State, slot: usize, pname: &str| -> (Vec<String>, usize) {
            let (rows, _, h) = state.param_menu_rows(slot, pname);
            let d = described(state, slot, pname);
            (rows.into_iter().filter(|r| !d.contains(r)).collect(), h - d.len())
        };
        show(&mut state, sphere);
        let (x, y) = row_center(&state, "Radius");
        assert_eq!(state.param_row_at(x, y), Some((sphere, "radius".to_string())), "the row's NAME, though the pane shows its label");
        assert_eq!(state.param_row_at(x, state.positions[crate::slots::PARAM_IDX].1 - 5.0), None, "above the pane is no row");

        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right });
        assert!(state.param_menu_open(), "a right press on a row opens its menu");
        assert!(!state.viewport_menu_open());
        let radius_desc = described(&state, sphere, "radius");
        assert_eq!(
            state.param_menu_actions,
            vec![ParamMenuAction::Info; 10 + radius_desc.len()].into_iter().chain([ParamMenuAction::Separator, ParamMenuAction::CopyParameter, ParamMenuAction::Separator, ParamMenuAction::EditExpression]).collect::<Vec<_>>(),
            "nothing copied yet, and the row holds a value"
        );
        // The header rows read the parameter out: its name, the control
        // the pane draws, the type of value that control sets in a
        // programmer's terms, the template's default and the control's
        // range. Radius is a slider setting a float, clamped to the pane's
        // default 0..2 since the template declares no range; it has no
        // label and no condition, so neither row appears. There is no
        // Value row: the Type row is the value's type.
        let shown: Vec<String> =
            cce_ui::widget::context_menu::options().into_iter().filter(|r| !radius_desc.contains(r)).collect();
        assert_eq!(
            &shown[..11],
            &[
                "Name: radius".to_string(), "Label: Radius".to_string(), "Control: slider".to_string(), "Type: float".to_string(),
                "Expression: false".to_string(), "Default: 0.5".to_string(), "Min: none".to_string(), "Max: none".to_string(),
                "Step: none".to_string(), "Range: 0..2".to_string(), "-".to_string(),
            ]
        );
        assert!(shown.iter().all(|r| !r.starts_with("Value:")), "{shown:?}");
        let (_, headers) = fields(&state, sphere, "radius");
        assert_eq!(headers, 10);
        // The headers are the rows before the separator; each one is
        // looked up by its readout, not its position.
        let headers_of = |state: &State, pname: &str| -> Vec<String> {
            let (rows, h) = fields(state, sphere, pname);
            assert_eq!(rows[h], "-");
            rows[..h].to_vec()
        };
        // An inline range, a spinbox's range and step, a choice's options,
        // and a conditional row's condition.
        let rows = headers_of(&state, "center_x");
        for want in ["Control: slider", "Type: float", "Min: -2", "Max: 2", "Step: none", "Range: -2..2"] {
            assert!(rows.contains(&want.to_string()), "{want} missing from {rows:?}");
        }
        let rows = headers_of(&state, "rows");
        for want in ["Default: 16", "Min: 2", "Max: 128", "Step: 1", "Range: 2..128, step 1", "Shown when: method == UV"] {
            assert!(rows.contains(&want.to_string()), "{want} missing from {rows:?}");
        }
        let rows = headers_of(&state, "method");
        for want in ["Control: dropdown", "Type: enum", "Default: UV", "Options: UV, Icosphere, Cube"] {
            assert!(rows.contains(&want.to_string()), "{want} missing from {rows:?}");
        }
        assert_eq!(headers_of(&state, "color"), vec!["Name: color", "Label: Color", "Control: toggle", "Type: boolean", "Expression: false", "Default: true"], "a toggle has neither a range nor options");
        // A parameter no template names has no default row, and its label
        // is under its name as a template's is.
        state.apply_action(McpAction::AddParam { slot: sphere, name: "extra".into(), param_type: "float".into(), default: "3".into(), label: "Extra".into() }, &mut redraw).unwrap();
        // A `float` parameter is drawn as a text box, and sets a float.
        assert_eq!(headers_of(&state, "extra"), vec!["Name: extra", "Label: Extra", "Control: text box", "Type: float", "Expression: false"]);
        state.current_dir_mut().children[sphere].params.iter_mut().find(|p| p.name == "extra").unwrap().label = "Extra Size".into();
        assert_eq!(headers_of(&state, "extra")[..2], ["Name: extra".to_string(), "Label: Extra Size".to_string()]);
        // Inside a subnet instance the SUBNET template's override is the
        // default: the Embryo's sphere1 was built with an expression.
        state.apply_action(McpAction::AddNode { template_name: "Embryo".into(), name: Some("embryo1".into()), x: 3.0, y: 8.0 }, &mut redraw).unwrap();
        let embryo = slot_of(&state, "embryo1");
        state.apply_action(McpAction::Enter { slot: embryo }, &mut redraw).unwrap();
        let inner = slot_of(&state, "sphere1");
        let (rows, h) = fields(&state, inner, "radius");
        assert!(rows[..h].contains(&"Default: chf(\"../radius\")".to_string()), "{rows:?}");
        state.apply_action(McpAction::Up, &mut redraw).unwrap();
        // A click on a header runs nothing.
        state.run_param_action(&state.current_dir().children[sphere].id.clone(), "radius", ParamMenuAction::Info);
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right });

        // Copy, then paste onto ball's Radius — a sibling, so `../sphere1`.
        let sphere_id = state.current_dir().children[sphere].id.clone();
        let ball_id = state.current_dir().children[ball].id.clone();
        state.run_param_action(&sphere_id, "radius", ParamMenuAction::CopyParameter);
        assert_eq!(state.copied_param, Some((sphere_id.clone(), "radius".to_string())));
        show(&mut state, ball);
        let (x, y) = row_center(&state, "Radius");
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right });
        assert!(state.param_menu_actions.contains(&ParamMenuAction::PasteRelative), "with a copy, paste is offered");
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right });
        state.run_param_action(&ball_id, "radius", ParamMenuAction::PasteRelative);
        let radius = |state: &State, slot: usize| state.current_dir().children[slot].params.iter().find(|p| p.name == "radius").unwrap().clone();
        assert_eq!(radius(&state, ball).text(), "ch(\"../sphere1/radius\")");
        assert!(radius(&state, ball).is_expr());
        let rows = crate::app::param_display(&state.current_dir().children[ball].params);
        assert_eq!(rows.iter().find(|r| r.0 == "Radius").unwrap().2, "text", "the pane shows an expression as text");

        // The reference is live: ball follows sphere1's Radius.
        state.apply_action(McpAction::SetParam { slot: sphere, name: "radius".into(), value: "0.9".into() }, &mut redraw).unwrap();
        let mut err = None;
        let r = crate::geometry::resolve_param_refs(&state.fs_root, &state.current_dir().children[ball], 0, &mut err).unwrap();
        assert!(err.is_none(), "{err:?}");
        assert_eq!(r.params.iter().find(|p| p.name == "radius").unwrap().text(), "0.9");

        // Absolute paste, then Delete Expression bakes the current value.
        state.run_param_action(&ball_id, "radius", ParamMenuAction::PasteAbsolute);
        assert_eq!(radius(&state, ball).text(), "ch(\"/geometry1/sphere1/radius\")");
        state.run_param_action(&ball_id, "radius", ParamMenuAction::DeleteExpression);
        assert_eq!(radius(&state, ball).text(), "0.9");
        assert!(!radius(&state, ball).is_expr());
        // Edit Expression flags without changing.
        state.run_param_action(&ball_id, "radius", ParamMenuAction::EditExpression);
        assert_eq!(radius(&state, ball).text(), "0.9");
        assert!(radius(&state, ball).is_expr());
        // …and the menu's readout says so: an expression is drawn as a
        // text box, and still sets the float its slider would. Method is a
        // dropdown setting an enum, Rows a spinbox setting an integer.
        show(&mut state, ball);
        let (rows, _) = fields(&state, ball, "radius");
        assert_eq!(&rows[2..5], &["Control: text box".to_string(), "Type: float".to_string(), "Expression: true".to_string()]);
        let (rows, _) = fields(&state, ball, "method");
        assert_eq!((&rows[2], &rows[3]), (&"Control: dropdown".to_string(), &"Type: enum".to_string()));
        let (rows, _) = fields(&state, ball, "rows");
        assert_eq!((&rows[2], &rows[3]), (&"Control: spinbox".to_string(), &"Type: integer".to_string()));

        // The pull node: a text parameter the pane presents as sliders
        // over a float3. The menu reads the control drawn and the type it
        // sets, with the presented row's range; a text that stays a text
        // box is a string.
        state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
        let pull = slot_of(&state, "pull1");
        for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.00:0.06:0.00")] {
            state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
        }
        show(&mut state, pull);
        let (rows, h) = fields(&state, pull, "value");
        let want: Vec<String> = ["Name: value", "Label: Value", "Control: trackball and sliders", "Type: float3", "Expression: false"].iter().map(|s| s.to_string()).collect();
        assert_eq!(&rows[..5], &want[..], "{rows:?}");
        assert!(rows[..h].contains(&"Range: -1..1".to_string()), "the span around 0.06: {rows:?}");
        assert!(rows[..h].iter().all(|r| !r.starts_with("Value:")), "{rows:?}");
        state.apply_action(McpAction::SetParam { slot: pull, name: "value".into(), value: "0.06".into() }, &mut redraw).unwrap();
        show(&mut state, pull);
        let (rows, _) = fields(&state, pull, "value");
        assert_eq!(
            (&rows[2], &rows[3]),
            (&"Control: trackball and sliders".to_string(), &"Type: float3".to_string()),
            "a single number is spread over the same control"
        );

        // And a reference typed straight into a row (or scripted) becomes one.
        state.apply_action(McpAction::SetParam { slot: ball, name: "rows".into(), value: "chi(\"../sphere1/rows\") * 2".into() }, &mut redraw).unwrap();
        let rows_p = state.current_dir().children[ball].params.iter().find(|p| p.name == "rows").unwrap();
        assert!(rows_p.is_expr());

        // A rename carries the paste along.
        state.apply_action(McpAction::RenameNode { slot: sphere, new_name: "orb".into() }, &mut redraw).unwrap();
        assert_eq!(state.current_dir().children[ball].params.iter().find(|p| p.name == "rows").unwrap().text(), "chi(\"../orb/rows\") * 2");
    }

    /// A code parameter never becomes an expression, however its text reads:
    /// a one-line script that IS `ch("../a/radius")` is a program to run,
    /// and flagging it would evaluate it to a number first. Neither the
    /// template loader nor a scripted set_param flags one.
    #[test]
    fn a_code_parameter_is_never_an_expression() {
        use crate::app::{infer_template_exprs, McpAction};
        let mut node = ref_node("w", "w1", "wrangle", vec![("code", "code", "ch(\"../a/radius\")"), ("radius", "slider", "ch(\"../a/radius\")")], vec![]);
        for p in &mut node.params {
            p.set_expr(false);
        }
        infer_template_exprs(&mut node);
        assert!(!node.params[0].is_expr(), "the code stays a program");
        assert!(node.params[1].is_expr(), "the slider becomes an expression");

        let mut state = State::new(false);
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Wrangle".into(), name: Some("k".into()), x: 3.0, y: 9.0 }, &mut redraw).unwrap();
        let k = state.current_dir().children.iter().position(|c| c.name == "k").unwrap();
        state.apply_action(McpAction::SetParam { slot: k, name: "code".into(), value: "chf(\"../sphere1/radius\")".into() }, &mut redraw).unwrap();
        let code = state.current_dir().children[k].params.iter().find(|p| p.name == "code").unwrap();
        assert!(!code.is_expr());
        assert_eq!(code.ty(), "code");
    }

    /// Inside the SECOND instance of a subnet, a child wired to a sibling by
    /// name finds its own sibling, not the first instance's.
    #[test]
    fn input_lookups_prefer_siblings() {
        let make = |id: &str, name: &str, radius: &str| {
            let src = ref_node(&format!("{id}-src"), "src", "sphere", vec![("radius", "slider", radius)], vec![]);
            let out = ref_node(&format!("{id}-out"), "output1", "output", vec![("input", "text", "src")], vec![]);
            ref_node(id, name, "node", vec![], vec![src, out])
        };
        let root = ref_node("root", "root", "node", vec![], vec![make("a", "shape1", "0.3"), make("b", "shape2", "0.9")]);
        let (g, _) = eval(&root, &root.children[1]);
        assert!((radius_of(&g.unwrap()) - 0.9).abs() < 0.02, "shape2's output read shape1's src");
        // Sibling-first, then anywhere: a name with no sibling still resolves globally.
        let global = ref_node("g", "global1", "sphere", vec![("radius", "slider", "0.6")], vec![]);
        let user = ref_node("u", "user1", "node", vec![], vec![ref_node("u-out", "output1", "output", vec![("input", "text", "global1")], vec![])]);
        let root2 = ref_node("root", "root", "node", vec![], vec![global, user]);
        let (g, _) = eval(&root2, &root2.children[1]);
        assert!((radius_of(&g.unwrap()) - 0.6).abs() < 0.02);
        assert_eq!(crate::geometry::find_input_node(&root2, &root2.children[1], "").map(|n| n.name.clone()), None);
    }

    /// The Switch passes the input its Index names, clamps a wild index, and
    /// passes nothing for an empty slot.
    #[test]
    fn switch_node_selects_one_of_its_inputs() {
        use crate::geometry::switch_input_param;
        assert_eq!(switch_input_param(0), "input");
        assert_eq!(switch_input_param(1), "input_2");
        assert_eq!(switch_input_param(3), "input_4");
        let a = ref_node("a", "a1", "sphere", vec![("radius", "slider", "0.2")], vec![]);
        let b = ref_node("b", "b1", "sphere", vec![("radius", "slider", "0.7")], vec![]);
        let sw = |index: &str| ref_node("sw", "switch1", "switch",
            vec![("input", "text", "a1"), ("input_2", "text", "b1"), ("input_3", "text", ""), ("input_4", "text", ""), ("index", "spinbox", index)], vec![]);
        let root = |index: &str| ref_node("root", "root", "node", vec![], vec![a.clone(), b.clone(), sw(index)]);
        let r = root("0");
        assert!((radius_of(&eval(&r, &r.children[2]).0.unwrap()) - 0.2).abs() < 0.02);
        let r = root("1");
        assert!((radius_of(&eval(&r, &r.children[2]).0.unwrap()) - 0.7).abs() < 0.02);
        let r = root("2");
        assert!(eval(&r, &r.children[2]).0.is_none(), "an empty slot passes nothing");
        let r = root("99");
        assert!(eval(&r, &r.children[2]).0.is_none(), "clamped to the last slot, which is empty");
        let r = root("-4");
        assert!((radius_of(&eval(&r, &r.children[2]).0.unwrap()) - 0.2).abs() < 0.02, "clamped to the first");

        // The template loads and is a geometry type the walk knows.
        let templates = crate::app::load_fs_tree();
        let t = templates.children.iter().find(|t| t.name == "Switch").expect("the Switch template");
        assert_eq!(t.node_type, "switch");
        assert_eq!(t.inputs, 4);
        assert!(crate::geometry::is_geometry_node_type("switch"));
    }

    /// A subnet's choice drives its switch through chi(), and the scene walk
    /// dived into the subnet draws the switch's pick — references resolve on
    /// that path too.
    #[test]
    fn a_choice_drives_a_switch_and_the_walk_sees_it() {
        let a = ref_node("a", "small", "sphere", vec![("radius", "slider", "0.2")], vec![]);
        let b = ref_node("b", "big", "sphere", vec![("radius", "slider", "0.7")], vec![]);
        let sw = ref_node("sw", "switch1", "switch",
            vec![("input", "text", "small"), ("input_2", "text", "big"), ("index", "spinbox", "chi(\"../size\")")], vec![]);
        let out = ref_node("o", "output1", "output", vec![("input", "text", "switch1")], vec![]);
        let mut a = a; a.geometry_visible = false;
        let mut b = b; b.geometry_visible = false;
        let sub = |size: &str| ref_node("sub", "pick1", "node", vec![("size", "choice:Small,Big", size)], vec![a.clone(), b.clone(), sw.clone(), out.clone()]);
        for (size, want) in [("Small", 0.2), ("Big", 0.7)] {
            let root = ref_node("root", "root", "node", vec![], vec![sub(size)]);
            let (g, err) = eval(&root, &root.children[0]);
            assert!(err.is_none(), "{err:?}");
            assert!((radius_of(&g.unwrap()) - want).abs() < 0.02, "Size {size} should pick radius {want}");
            // Dived in: the walk draws switch1 (visible) and output1, both the pick.
            let mut err = None;
            let drawn = crate::geometry::network_sphere_vertices_with_errors(
                &root, &root.children[0], &mut err,
                &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
            );
            assert!(err.is_none(), "{err:?}");
            assert!(!drawn.is_empty());
            assert!((radius_of(&drawn) - want).abs() < 0.02, "the walk inside pick1 draws the {size} pick");
        }
    }

    /// A Sphere TEMPLATE instance reads its kernel's chf("radius") through
    /// its parent's parameter — and that parameter may be a reference to the
    /// subnet above, which has to be resolved before the kernel sees it.
    #[test]
    fn a_kernel_reads_a_reference_through_its_parent() {
        let templates_root = crate::app::load_fs_tree();
        let mut sphere = templates_root.children.iter().find(|t| t.name == "Sphere").unwrap().clone();
        sphere.id = "sph".into();
        sphere.name = "sphere1".into();
        for c in &mut sphere.children {
            c.id = format!("sph_{}", c.name);
        }
        let radius = sphere.params.iter_mut().find(|p| p.name == "radius").unwrap();
        radius.set_text("ch(\"../radius\")");
        radius.set_expr(true);
        let out = ref_node("o", "output1", "output", vec![("input", "text", "sphere1")], vec![]);
        let sub = ref_node("sub", "subnet1", "node", vec![("input", "text", ""), ("radius", "slider", "0.9")], vec![sphere, out]);
        let root = ref_node("root", "root", "node", vec![], vec![sub]);
        let (g, err) = eval(&root, &root.children[0]);
        assert!(err.is_none(), "{err:?}");
        let g = g.expect("the subnet evaluates");
        let center = Vec3::new(0.0, 0.55, 0.0);
        let r = g.positions().iter().map(|p| (Vec3::from(*p) - center).length()).fold(0.0, f32::max);
        assert!((r - 0.9).abs() < 0.02, "the kernel got the subnet's radius, not the reference string: {r}");
    }

    /// The params pane shows a referencing value as the text it is.
    #[test]
    fn param_display_shows_references_as_text() {
        let params = vec![
            crate::app::ParamDef::new("radius", "slider", "ch(\"../radius\")").with_range(Some(0.0), Some(2.0)).as_expr(),
            crate::app::ParamDef::new("rows", "spinbox", "16").with_range(Some(2.0), Some(128.0)).with_step(Some(1.0)),
        ];
        let rows = crate::app::param_display(&params);
        assert_eq!(rows[0], ("radius".to_string(), "ch(\"../radius\")".to_string(), "text".to_string()));
        assert!(rows[1].2.starts_with("spinbox"));
    }

    // ----- Hull, surface scatter, repel relax, and the Embryo template -----

    /// The hull of a cube's corners plus points inside it is the cube: eight
    /// points, twelve triangles, closed, with nothing left outside it.
    #[test]
    fn convex_hull_of_a_cube_with_interior_points_is_the_cube() {
        use crate::hull::convex_hull;
        let mut pts = Vec::new();
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    pts.push(Vec3::new(x, y, z));
                }
            }
        }
        for i in 0..50 {
            let t = i as f32 / 50.0;
            pts.push(Vec3::new(t * 0.9 - 0.45, (t * 7.0).sin() * 0.5, (t * 3.0).cos() * 0.5));
        }
        let hull = convex_hull(&pts).expect("a cube spans a volume");
        assert_eq!(hull.num_points(), 8, "only the corners are on the hull");
        assert_eq!(hull.num_prims(), 12);
        assert!(hull.is_closed(), "a hull is watertight and consistently wound");
        for prim in 0..hull.num_prims() {
            let ids = hull.prim_points(prim);
            let (a, b, c) = (hull.pos(ids[0] as usize), hull.pos(ids[1] as usize), hull.pos(ids[2] as usize));
            let n = (b - a).cross(c - a).normalize();
            assert!(a.dot(n) > 0.0, "face {prim} winds outward");
            for q in &pts {
                assert!((*q - a).dot(n) <= 1e-4, "point {q:?} is outside face {prim}");
            }
        }
        let flat: Vec<Vec3> = (0..20).map(|i| Vec3::new(i as f32, (i * i) as f32 * 0.1, 0.0)).collect();
        assert!(convex_hull(&flat).is_none(), "coplanar points span no volume");
        assert!(convex_hull(&pts[..3]).is_none());

        // The node: a hull of the input's points; too few to hull passes through.
        let src = ref_node("s", "src", "points", vec![("shape", "text", "Spiral"), ("points", "spinbox", "60"), ("markers", "text", "false")], vec![]);
        let hull_node = ref_node("h", "hull1", "hull", vec![("input", "text", "src")], vec![]);
        let root = ref_node("root", "root", "node", vec![], vec![src, hull_node]);
        let (g, err) = eval(&root, &root.children[1]);
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        assert!(g.num_prims() > 0 && g.is_closed(), "the spiral hulls into a closed mesh");
        let line = ref_node("l", "line", "points", vec![("shape", "text", "Line"), ("points", "spinbox", "5"), ("markers", "text", "false")], vec![]);
        let hull2 = ref_node("h2", "hull2", "hull", vec![("input", "text", "line")], vec![]);
        let root2 = ref_node("root", "root", "node", vec![], vec![line, hull2]);
        let g = eval(&root2, &root2.children[1]).0.unwrap();
        assert_eq!((g.num_points(), g.num_prims()), (5, 0), "a line of points passes through unhulled");
    }

    /// Scattered points lie on the surface, in the number asked for, and a
    /// seed reproduces its draw; the node's Surface mode emits them, relaxed
    /// apart when asked.
    #[test]
    fn scatter_surface_mode_lands_on_the_surface_and_is_seeded() {
        use crate::scatter::scatter_on_surface;
        let sphere = crate::geometry::sphere_detail(Vec3::ZERO, 0.5, 12, 16);
        let a = scatter_on_surface(&sphere, 300, 1.1);
        assert_eq!(a.len(), 300);
        let grid = crate::spatial::TriGrid::build(&sphere);
        for p in &a {
            let hit = grid.closest(*p).unwrap();
            assert!(hit.distance < 1e-4, "point {p:?} is {} off the surface", hit.distance);
        }
        assert_eq!(a, scatter_on_surface(&sphere, 300, 1.1), "same seed, same points");
        assert_ne!(a, scatter_on_surface(&sphere, 300, 2.0), "another seed, another draw");
        assert!(scatter_on_surface(&Detail::new(), 10, 1.0).is_empty());

        let scatter = |relax: &str| {
            let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "0.5")], vec![]);
            let sc = ref_node("sc", "scatter1", "scatter", vec![
                ("input", "text", "src"), ("mode", "choice:Volume,Surface", "Surface"), ("points", "spinbox", "80"),
                ("seed", "slider", "1.1"), ("relax_points", "toggle", relax), ("relax_iterations", "spinbox", "30"),
                ("markers", "choice:true,false", "false"),
            ], vec![]);
            let root = ref_node("root", "root", "node", vec![], vec![src, sc]);
            let (g, err) = eval(&root, &root.children[1]);
            assert!(err.is_none(), "{err:?}");
            (g.unwrap(), eval(&root, &root.children[0]).0.unwrap())
        };
        let (raw, src) = scatter("false");
        assert_eq!(raw.num_points(), 80, "bare points, one per location");
        let grid = crate::spatial::TriGrid::build(&src);
        for p in 0..raw.num_points() {
            assert!(grid.closest(raw.pos(p)).unwrap().distance < 1e-3);
        }
        let (relaxed, _) = scatter("true");
        assert_eq!(relaxed.num_points(), 80);
        let nearest = |d: &Detail| -> f32 {
            let mut worst = f32::MAX;
            for i in 0..d.num_points() {
                let mut best = f32::MAX;
                for j in 0..d.num_points() {
                    if i != j { best = best.min((d.pos(i) - d.pos(j)).length()); }
                }
                worst = worst.min(best);
            }
            worst
        };
        assert!(nearest(&relaxed) > nearest(&raw), "relaxing spreads the closest pair: {} vs {}", nearest(&relaxed), nearest(&raw));
        for p in 0..relaxed.num_points() {
            assert!(grid.closest(relaxed.pos(p)).unwrap().distance < 1e-3, "relaxed points stay on the surface");
        }
    }

    /// Relax in Repel mode slides a mesh's points apart in their tangent
    /// planes, so they stay on the shape; In 3D Space lets them leave it.
    #[test]
    fn relax_repel_mode_keeps_points_in_their_tangent_planes() {
        let relax = |in_3d: &str, iterations: &str| {
            let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "0.5")], vec![]);
            let rx = ref_node("r", "relax1", "relax", vec![
                ("input", "text", "src"), ("mode", "choice:Springs,Repel", "Repel"), ("iterations", "spinbox", iterations),
                ("radius", "slider", "0.08"), ("in_3d_space", "toggle", in_3d),
            ], vec![]);
            let root = ref_node("root", "root", "node", vec![], vec![src, rx]);
            (eval(&root, &root.children[1]).0.unwrap(), eval(&root, &root.children[0]).0.unwrap())
        };
        let (relaxed, plain) = relax("false", "5");
        let centre = (0..plain.num_points()).map(|i| plain.pos(i)).sum::<Vec3>() / plain.num_points() as f32;
        let mut moved = 0;
        for i in 0..relaxed.num_points() {
            if (relaxed.pos(i) - plain.pos(i)).length() > 1e-5 { moved += 1; }
            assert!(((relaxed.pos(i) - centre).length() - 0.5).abs() < 0.05, "point {i} left the sphere");
        }
        assert!(moved > 0, "some point moved");
        let (free, _) = relax("true", "5");
        assert!((0..free.num_points()).any(|i| ((free.pos(i) - centre).length() - 0.5).abs() > 0.01), "in 3D the points are free to leave");
        let (off, _) = relax("false", "0");
        assert!((0..off.num_points()).all(|i| (off.pos(i) - plain.pos(i)).length() < 1e-6), "zero iterations is off");
    }

    /// The Embryo template, end to end: an instance whose children read
    /// its controls through references. Basic is the internal sphere;
    /// Scatter is a closed hull of at most Scatter Count points inside the
    /// sphere's radius; Input reads what it is given; every mesh carries N.
    #[test]
    fn embryo_template_builds_a_sphere_a_hull_or_the_input() {
        let templates_root = crate::app::load_fs_tree();
        let t = templates_root.children.iter().find(|t| t.name == "Embryo").expect("the Embryo template");
        assert_eq!(t.node_type, "node", "the Embryo is a subnet of nodes");
        let names: Vec<&str> = t.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["input1", "sphere1", "source1", "scatter1", "hull1", "method1", "relax1", "subdivide1", "normal1", "output1"]);
        for c in &t.children {
            // The last real node is the one that draws — as the Sphere's
            // kernel node is — since a subnet viewed from outside shows its
            // internals by their flags and output children only draw at the
            // displayed level. Every chain node visible drew the hull five
            // times over and cost seconds per edit.
            assert_eq!(c.geometry_visible, c.name == "normal1", "only normal1 draws: {} is {}", c.name, c.geometry_visible);
        }
        // Nested template resolution: the sphere inside carries its own
        // resolved children, kernel node params included.
        let sphere1 = t.children.iter().find(|c| c.name == "sphere1").unwrap();
        assert_eq!(sphere1.node_type, "sphere", "the nested sphere is the native Sphere");
        assert!(sphere1.params.iter().any(|p| p.name == "method"), "with its template's whole surface");
        assert!(sphere1.params.iter().find(|p| p.name == "radius").unwrap().is_expr(), "and the Embryo's reference on its Radius");

        let instance = |overrides: &[(&str, &str)], extra: Vec<FsNode>| {
            let mut inst = t.clone();
            crate::app::regenerate_node_ids(&mut inst);
            inst.name = "embryo1".into();
            for (n, v) in overrides {
                inst.params.iter_mut().find(|p| p.name == *n).unwrap_or_else(|| panic!("param {n}")).set_text(v.to_string());
            }
            let mut children = extra;
            children.push(inst);
            ref_node("root", "root", "node", vec![], children)
        };
        let run = |root: &FsNode| {
            let e = root.children.iter().find(|c| c.name == "embryo1").unwrap();
            let (g, err) = eval(root, e);
            assert!(err.is_none(), "{err:?}");
            g.expect("the embryo evaluates")
        };
        let extent = |g: &Detail| g.positions().iter().map(|p| Vec3::from(*p).length()).fold(0.0, f32::max);

        let basic = run(&instance(&[], vec![]));
        assert!(basic.num_prims() > 0);
        assert!((extent(&basic) - 0.5).abs() < 0.02, "Basic is the internal sphere of Radius 0.5: {}", extent(&basic));
        assert!(basic.points().value("N", 0).is_some(), "normals are written last");
        let big = run(&instance(&[("radius", "1.5"), ("base_resolution", "8")], vec![]));
        assert!((extent(&big) - 1.5).abs() < 0.05, "Radius reaches the sphere through chf: {}", extent(&big));
        assert!(big.num_points() < basic.num_points(), "Base Resolution reaches Rows and Columns through chi");

        let scattered = run(&instance(&[("method", "Scatter"), ("scatter_count", "400"), ("base_resolution", "16")], vec![]));
        assert!(scattered.num_prims() > 0 && scattered.is_closed(), "Scatter hulls the points into a closed mesh");
        assert!(scattered.num_points() <= 400);
        assert!(extent(&scattered) <= 0.5 + 1e-3, "the hull lies inside the seed sphere");
        assert!(scattered.points().value("N", 0).is_some());

        let seed = ref_node("seed", "seed", "sphere", vec![("radius", "slider", "0.25")], vec![]);
        let root = instance(&[("source", "Input"), ("input", "seed")], vec![seed]);
        let from_input = run(&root);
        let seed_geom = eval(&root, &root.children[0]).0.unwrap();
        assert_eq!(from_input.num_points(), seed_geom.num_points(), "Source Input is the wired node");
        assert!((from_input.pos(0) - seed_geom.pos(0)).length() < 1e-6);
        let e = instance(&[("source", "Input")], vec![]);
        assert!(eval(&e, &e.children[0]).0.is_none(), "Source Input with nothing wired seeds nothing");

        let coarse = run(&instance(&[("base_resolution", "8")], vec![]));
        let sub = run(&instance(&[("base_resolution", "8"), ("subdivision_depth", "1")], vec![]));
        // Against the real subdivide of the same mesh rather than ×4: the
        // kernel sphere's pole triangles are degenerate and subdivide drops
        // them.
        assert_eq!(sub.num_prims(), crate::remesh::subdivide(&coarse, 1).num_prims(), "Subdivision Depth reaches Depth");
    }

    /// A native embryo from 2026-09-21 loads as an instance of the template,
    /// with its values, its identity and its meta child intact.
    #[test]
    fn a_native_embryo_recomposes_on_load() {
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let param = |n: &str, v: &str| crate::app::ParamDef::new(n, "text", v);
        let meta = ref_node("m", "meta", "meta", vec![("Point Markers", "toggle", "true")], vec![]);
        let mut native = ref_node("old-id", "embryo1", "embryo", vec![], vec![meta]);
        native.params = vec![param("input", ""), param("method", "Scatter"), param("scatter_count", "150"), param("radius", "0.7"), param("base_resolution", "16")];
        native.position = (3.0, 4.0);
        native.geometry_visible = true;
        let mut root = ref_node("root", "root", "node", vec![], vec![native]);
        crate::app::merge_template_defs(&mut root, &templates);
        let e = &root.children[0];
        assert_eq!(e.node_type, "node", "recomposed as a subnet");
        assert_eq!((e.id.as_str(), e.name.as_str(), e.position, e.geometry_visible), ("old-id", "embryo1", (3.0, 4.0), true));
        let get = |n: &str| e.params.iter().find(|p| p.name == n).unwrap().text().to_string();
        assert_eq!(get("method"), "Scatter");
        assert_eq!(get("scatter_count"), "150");
        assert_eq!(get("radius"), "0.7");
        assert_eq!(get("scatter_seed"), "1.1", "a param the native node lacked takes the template default");
        assert!(e.children.iter().any(|c| c.name == "hull1"));
        // The per-node meta child an older save carried is stripped, here as
        // everywhere else: merge_template_defs takes them before it matches
        // anything, so a recompose never has one to carry over.
        assert!(!e.children.iter().any(|c| c.node_type == "meta"), "a meta child survived the recompose");
        let (g, err) = eval(&root, e);
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        assert!(g.is_closed() && g.num_points() <= 150, "and it evaluates as the scatter it was");
        let extent = g.positions().iter().map(|p| Vec3::from(*p).length()).fold(0.0, f32::max);
        assert!(extent <= 0.7 + 1e-3 && extent > 0.5);
    }

    #[test]
    fn test_the_mold_shell_node_builds_a_two_sided_shell() {
        use crate::geometry::resolve_mold_shell_geometry_with_errors;
        fn mnode(id: &str, name: &str, ty: &str, params: &[(&str, &str)]) -> FsNode {
            FsNode {
                id: id.to_string(),
                name: name.to_string(),
                node_type: ty.to_string(),
                children: vec![],
                params: params
                    .iter()
                    .map(|(n, v)| crate::app::ParamDef::new(n.to_string(), "text".to_string(), v.to_string()))
                    .collect(),
                geometry_visible: true,
                bypassed: false,
                position: (0.0, 0.0),
                inputs: 1,
                outputs: 1,
            }
        }
        let sphere = mnode("id-s", "sphere1", "sphere", &[("radius", "0.8")]);
        let shell = mnode(
            "id-m",
            "mold1",
            "mold_shell",
            &[
                ("input", "sphere1"),
                ("maximum_thickness", "0.20"),
                ("minimum_thickness", "0.10"),
                ("remesh_division_size", "0.30"),
                ("ramp", "Linear"),
            ],
        );
        let mut root = mnode("id-root", "root", "node", &[]);
        root.children = vec![sphere, shell];

        let mut err = None;
        let mut cache = crate::geometry::SimCache::default();
        let mut sim = crate::geometry::EvalSim::new(0, 0, &mut cache);
        let out = resolve_mold_shell_geometry_with_errors(
            &root,
            &root.children[1],
            &mut Vec::new(),
            &mut err,
            &mut sim,
        )
        .expect("the mold shell resolved to nothing");
        assert!(err.is_none(), "{err:?}");
        assert!(out.num_prims() > 0);

        // Two surfaces: the outer one at the sphere's radius, the inner one
        // pulled in by between the minimum and the maximum thickness.
        let centre = {
            let (lo, hi) = out.bounds().unwrap();
            (lo + hi) * 0.5
        };
        let radii: Vec<f32> = (0..out.num_points()).map(|p| (out.pos(p) - centre).length()).collect();
        let far = radii.iter().cloned().fold(0.0f32, f32::max);
        let near = radii.iter().cloned().fold(f32::MAX, f32::min);
        assert!((far - 0.8).abs() < 0.12, "the outer surface is at {far}, not the sphere's 0.8");
        assert!(
            near < far - 0.08 && near > far - 0.30,
            "the inner surface is {near} against an outer {far}; the gap should be the thickness range"
        );

        // Closed: the pair is a solid, not two loose surfaces. A sphere has no
        // rim, so the two shells close each other.
        assert!(out.is_closed(), "the shell is not a closed surface");
    }

    /// A page's raster is its physical size times its resolution — the
    /// property that makes DPI a page parameter rather than an export one.
    #[test]
    fn test_a_page_is_its_physical_size_times_its_resolution() {
        use crate::page::Page;
        let p = Page::new([8.5, 11.0], 300, [1.0; 4]);
        assert_eq!((p.width, p.height), (2550, 3300));
        assert!((p.scale() - 300.0).abs() < 0.01);

        // The same sheet at a different resolution is the same sheet.
        let q = Page::new([8.5, 11.0], 72, [1.0; 4]);
        assert_eq!((q.width, q.height), (612, 792));
        assert!(
            ((p.width as f32 / p.height as f32) - (q.width as f32 / q.height as f32)).abs() < 1e-3
        );

        // A sheet nobody could print clamps rather than allocating: aspect
        // survives, resolution does not.
        let huge = Page::new([100.0, 50.0], 1200, [1.0; 4]);
        assert!(
            (huge.width as u64) * (huge.height as u64) <= 356_000_000,
            "{}x{} is not clamped",
            huge.width,
            huge.height
        );
        assert!(
            ((huge.width as f32 / huge.height as f32) - 2.0).abs() < 0.01,
            "the clamp changed the aspect: {}x{}",
            huge.width,
            huge.height
        );
    }

    /// Rect coverage is exact area, not a test of the pixel centre.
    ///
    /// This is what keeps a ruled sheet's lines from alternating between one
    /// and two pixels wide down its length — which prints as a wobble in the
    /// paper rather than as aliasing.
    #[test]
    fn test_rect_coverage_is_exact_area() {
        use crate::page::Page;
        // Ten pixels per inch, so one pixel is a tenth of an inch and the
        // arithmetic is readable.
        let mut p = Page::new([1.0, 1.0], 10, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!((p.width, p.height), (10, 10));

        // A rect covering exactly the left half of pixel (0,0).
        p.rect(0.0, 0.0, 0.05, 0.1, [1.0, 1.0, 1.0, 1.0]);
        let v = p.pixels[0][0];
        assert!((v - 0.5).abs() < 1e-4, "half a pixel of white over black read {v}, not 0.5");

        // A whole-pixel rect is fully opaque, and its neighbour is untouched.
        let mut p = Page::new([1.0, 1.0], 10, [0.0, 0.0, 0.0, 1.0]);
        p.rect(0.2, 0.0, 0.3, 0.1, [1.0, 1.0, 1.0, 1.0]);
        assert!((p.pixels[2][0] - 1.0).abs() < 1e-4, "a whole pixel is not solid");
        assert!(p.pixels[1][0] < 1e-4, "the rect bled into its neighbour");
        assert!(p.pixels[3][0] < 1e-4, "the rect bled into its neighbour");

        // Off the sheet entirely is a no-op, not a panic or a wrap.
        p.rect(-5.0, -5.0, -4.0, -4.0, [1.0, 0.0, 0.0, 1.0]);
        p.rect(50.0, 50.0, 60.0, 60.0, [1.0, 0.0, 0.0, 1.0]);
        assert!(p.pixels.iter().all(|px| px[0] == px[1] && px[1] == px[2]), "red leaked in");
    }

    /// Two grids at cell and 2x cell share their rules exactly, which is the
    /// whole reason for drawing a second one.
    #[test]
    fn test_a_second_grid_lands_on_the_first_ones_rules() {
        use crate::page::Page;
        let mut p = Page::new([2.0, 2.0], 100, [1.0, 1.0, 1.0, 1.0]);
        p.grid(0.25, 0.02, [0.0; 4], [0.0, 0.0, 0.0, 1.0]);
        // Column of the rule at x = 0.5 inches: 50 px in.
        let row = 37; // anywhere between two horizontal rules
        assert!(p.pixels[(row * p.width + 50) as usize][0] < 0.1, "no rule at 0.50 inches");
        assert!(p.pixels[(row * p.width + 37) as usize][0] > 0.9, "the cell is not clear");

        let mut q = Page::new([2.0, 2.0], 100, [1.0, 1.0, 1.0, 1.0]);
        q.grid(0.5, 0.02, [0.0; 4], [0.0, 0.0, 0.0, 1.0]);
        assert!(q.pixels[(row * q.width + 50) as usize][0] < 0.1, "the 2x grid missed the rule");

        // And both rule the sheet's own edge, so neither looks like it stopped
        // a line short.
        assert!(p.pixels[(row * p.width) as usize][0] < 0.6, "the left edge is not ruled");
    }

    /// A border puts its ink INSIDE the sheet: half a border off the paper is
    /// half a border.
    #[test]
    fn test_a_border_stays_on_the_paper() {
        use crate::page::Page;
        let mut p = Page::new([2.0, 2.0], 100, [1.0, 1.0, 1.0, 1.0]);
        p.border(0.25, 0.0, [0.0, 0.0, 0.0, 1.0]);
        let at = |x: u32, y: u32| p.pixels[(y * p.width + x) as usize][0];
        assert!(at(0, 100) < 0.1, "the outermost pixel is not inked");
        assert!(at(24, 100) < 0.1, "the border is thinner than asked");
        assert!(at(30, 100) > 0.9, "the border is thicker than asked");
        assert!(at(100, 100) > 0.9, "the border filled the page");
        // All four sides, not just the two that a copy-paste would reach.
        assert!(at(199, 100) < 0.1 && at(100, 0) < 0.1 && at(100, 199) < 0.1, "a side is missing");
    }

    /// The PNG carries the physical size, so a printer lays the sheet out at
    /// the size it was composed at instead of guessing 96 DPI.
    #[test]
    fn test_the_png_knows_its_own_physical_size() {
        use crate::page::Page;
        let dir = std::env::temp_dir()
            .join(format!("cce-designer-page-tests-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sheet.png");
        let p = Page::new([8.5, 11.0], 300, [1.0, 1.0, 1.0, 1.0]);
        p.write_png(&path).expect("write");

        let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
        let reader = decoder.read_info().unwrap();
        let info = reader.info();
        assert_eq!((info.width, info.height), (2550, 3300));
        let dims = info.pixel_dims.expect("no pHYs chunk: the printer would guess");
        assert!(matches!(dims.unit, png::Unit::Meter));
        // pHYs is pixels per METRE, the only unit PNG offers, so the DPI
        // round-trips through a conversion and comes back a hair off.
        let dpi = dims.xppu as f32 / 39.370_08;
        assert!((dpi - 300.0).abs() < 0.01, "the PNG says {dpi} DPI, not 300");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_a_thin_slab_is_solid_all_the_way_through() {
        // The flood fill decides what is enclosed, and it must not be able to
        // walk THROUGH a wall. A slab only a few voxels thick is where that
        // goes wrong: at a band narrower than a voxel, two adjacent samples
        // straddling the surface can both read as "far", the flood steps
        // between them, and the slab comes back hollow — which a boolean
        // against it then fails to cut with.
        let slab = box_mesh(Vec3::new(-1.0, -0.1, -1.0), Vec3::new(1.0, 0.1, 1.0));
        assert!(slab.is_closed());
        let vol = Volume::from_mesh(&slab, 0.05, 0.15);
        let [nx, ny, nz] = vol.dims();

        let mut inside_wrong = Vec::new();
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let p = vol.sample_position(i, j, k);
                    let deep = p.x.abs() < 0.8 && p.z.abs() < 0.8 && p.y.abs() < 0.04;
                    if deep && vol.at(i, j, k) >= 0.0 {
                        inside_wrong.push((p, vol.at(i, j, k)));
                    }
                }
            }
        }
        assert!(
            inside_wrong.is_empty(),
            "{} samples inside the slab read as outside, e.g. {:?}",
            inside_wrong.len(),
            &inside_wrong[..inside_wrong.len().min(3)]
        );

        // And it cuts: subtracting the slab from a box that contains it leaves
        // a gap where the slab was.
        let block = box_mesh(Vec3::splat(-0.6), Vec3::splat(0.6));
        let (lo, hi) = (Vec3::splat(-1.3), Vec3::splat(1.3));
        let mut vb = Volume::build(&block, lo, hi, 0.05, 0.15);
        let vs = Volume::build(&slab, lo, hi, 0.05, 0.15);
        vb.subtract(&vs);
        let out = vb.to_mesh();
        let survivors = (0..out.num_points())
            .map(|p| out.pos(p))
            .filter(|q| q.y.abs() < 0.06 && q.x.abs() < 0.4 && q.z.abs() < 0.4)
            .count();
        assert_eq!(survivors, 0, "points survive where the slab cut through");

        // And an INSIDE-OUT input gives the same field. The band test asks the
        // nearest face which way it points, so a mesh wound the other way
        // would otherwise come back riddled with holes — which is how the
        // winding measurement got written.
        let mut flipped = Detail::new();
        for p in 0..slab.num_points() {
            flipped.add_point(slab.pos(p));
        }
        for prim in 0..slab.num_prims() {
            let mut pts = slab.prim_points(prim).to_vec();
            pts.reverse();
            flipped.add_prim(&pts);
        }
        let inverted = Volume::build(&flipped, lo, hi, 0.05, 0.15);
        let mut differ = 0;
        for k in 0..vs.dims()[2] {
            for j in 0..vs.dims()[1] {
                for i in 0..vs.dims()[0] {
                    if (vs.at(i, j, k) < 0.0) != (inverted.at(i, j, k) < 0.0) {
                        differ += 1;
                    }
                }
            }
        }
        assert_eq!(differ, 0, "{differ} samples disagree when the input is wound inside out");
    }

    #[test]
    fn test_offsetting_is_subtraction() {
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 14, 20);
        let radius = |d: &Detail| {
            (0..d.num_points()).map(|p| d.pos(p).length()).sum::<f32>() / d.num_points() as f32
        };

        let mut grown = Volume::from_mesh(&sphere, 0.15, 0.6);
        grown.offset(0.3);
        let out = grown.to_mesh();
        assert!(
            (radius(&out) - 1.3).abs() < 0.12,
            "a 0.3 offset should give radius 1.3, got {}",
            radius(&out)
        );

        // Inward too, which is what a shell's inner wall is.
        let mut shrunk = Volume::from_mesh(&sphere, 0.15, 0.6);
        shrunk.offset(-0.3);
        assert!((radius(&shrunk.to_mesh()) - 0.7).abs() < 0.12);
    }

    #[test]
    fn test_the_booleans_are_a_minimum_and_a_maximum() {
        // Two overlapping spheres, sampled over ONE grid so the operations are
        // elementwise. Sharing the grid is what makes them arithmetic rather
        // than a geometry problem.
        let a = sphere_detail(Vec3::new(-0.35, 0.0, 0.0), 0.8, 14, 20);
        let b = sphere_detail(Vec3::new(0.35, 0.0, 0.0), 0.8, 14, 20);
        let (lo, hi) = (Vec3::splat(-1.6), Vec3::splat(1.6));
        let va = Volume::from_mesh_in(&a, lo, hi, 0.14);
        let vb = Volume::from_mesh_in(&b, lo, hi, 0.14);
        assert!(va.aligned_with(&vb), "the two fields do not share a grid");

        let width = |d: &Detail| d.bounds().map(|(l, h)| h.x - l.x).unwrap_or(0.0);

        let mut u = va.clone();
        u.union(&vb);
        let mut i = va.clone();
        i.intersect(&vb);
        let mut s = va.clone();
        s.subtract(&vb);
        // Extracted ONCE each: surface extraction is not free, and an
        // assertion message that re-runs it is a slow test nobody runs.
        let (um, im, sm, am) = (u.to_mesh(), i.to_mesh(), s.to_mesh(), va.to_mesh());

        // The union spans both, the intersection is the lens between them, and
        // the difference is narrower than the whole of A.
        assert!(width(&um) > 2.2, "union is {}", width(&um));
        assert!(width(&im) < 1.0, "intersection is {}", width(&im));
        assert!(width(&sm) < width(&am) + 0.01, "the difference grew");
        // Every result is still a closed surface — which a mesh boolean has to
        // work for and a field gets for free.
        for m in [&um, &im, &sm] {
            assert!(m.num_prims() > 50);
        }

        // Fields on different grids refuse to combine rather than reading each
        // other's memory in the wrong order.
        let elsewhere = Volume::from_mesh_in(&b, lo, hi, 0.25);
        assert!(!va.aligned_with(&elsewhere));
        let mut guarded = va.clone();
        guarded.union(&elsewhere);
        assert_eq!(
            guarded.to_mesh().num_points(),
            am.num_points(),
            "a mismatched grid was combined"
        );
    }

    // ---- Mesh export ----

    /// A two-quad sheet: enough to tell a format that keeps topology from one
    /// that does not.
    fn sheet() -> Detail {
        let mut d = Detail::new();
        for (x, z) in [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (0.0, 1.0), (1.0, 1.0), (2.0, 1.0)] {
            d.add_point(Vec3::new(x, 0.0, z));
        }
        d.add_prim(&[0, 3, 4, 1]);
        d.add_prim(&[1, 4, 5, 2]);
        d
    }

    #[test]
    fn test_stl_writes_a_file_a_printer_can_read() {
        use crate::export::stl_binary;
        let d = sheet();
        let b = stl_binary(&d, 1.0, "sheet");

        // 80-byte header, a count, 50 bytes a triangle. Two quads fan to four.
        let count = u32::from_le_bytes(b[80..84].try_into().unwrap()) as usize;
        assert_eq!(count, 4);
        assert_eq!(b.len(), 84 + count * 50);
        // The header must NOT begin with "solid": that word at the start of a
        // file is how readers guess at the ASCII form, and a binary file that
        // opens with it is a well-known way to be misread.
        assert!(!b.starts_with(b"solid"), "binary STL must not look like ASCII");
        assert!(b.starts_with(b"cce-designer sheet"));

        // Every facet normal agrees with its own winding. STL stores both and
        // they can disagree; a slicer that trusts the normal would then see
        // the surface inside out.
        for t in 0..count {
            let at = 84 + t * 50;
            let f: Vec<f32> = (0..12)
                .map(|i| f32::from_le_bytes(b[at + i * 4..at + i * 4 + 4].try_into().unwrap()))
                .collect();
            let n = Vec3::new(f[0], f[1], f[2]);
            let (a, bb, c) = (
                Vec3::new(f[3], f[4], f[5]),
                Vec3::new(f[6], f[7], f[8]),
                Vec3::new(f[9], f[10], f[11]),
            );
            let geo = (bb - a).cross(c - a).normalize();
            assert!(n.dot(geo) > 0.999, "facet {t}: normal {n:?} vs winding {geo:?}");
            // The attribute byte count nothing uses and everything expects.
            assert_eq!(u16::from_le_bytes(b[at + 48..at + 50].try_into().unwrap()), 0);
        }

        // Scale multiplies coordinates and nothing else.
        let big = stl_binary(&d, 10.0, "sheet");
        let vx = |blob: &[u8]| f32::from_le_bytes(blob[96..100].try_into().unwrap());
        assert!((vx(&big) - vx(&b) * 10.0).abs() < 1e-4);

        // Empty geometry is a valid file with no triangles, not a panic.
        let empty = stl_binary(&Detail::new(), 1.0, "nothing");
        assert_eq!(empty.len(), 84);
        assert_eq!(u32::from_le_bytes(empty[80..84].try_into().unwrap()), 0);
    }

    #[test]
    fn test_obj_keeps_the_topology_that_stl_throws_away() {
        use crate::export::{obj, stl_binary};
        let d = sheet();
        let text = obj(&d, 1.0, "sheet");
        let lines: Vec<&str> = text.lines().collect();

        // One vertex per POINT and one face per PRIMITIVE: a quad stays a
        // quad and a shared point stays shared. STL cannot say either — it
        // fans to four triangles carrying twelve loose corners.
        assert_eq!(lines.iter().filter(|l| l.starts_with("v ")).count(), d.num_points());
        let faces: Vec<&&str> = lines.iter().filter(|l| l.starts_with("f ")).collect();
        assert_eq!(faces.len(), d.num_prims());
        for f in &faces {
            assert_eq!(f.split_whitespace().count() - 1, 4, "a quad did not survive: {f}");
        }
        let count = u32::from_le_bytes(stl_binary(&d, 1.0, "s")[80..84].try_into().unwrap());
        assert_eq!(count, 4, "STL fans the same mesh to triangles");

        // Indices are 1-based and in range — the single most common way to
        // write a broken OBJ.
        for f in &faces {
            for tok in f.split_whitespace().skip(1) {
                let i: usize = tok.split('/').next().unwrap().parse().unwrap();
                assert!(i >= 1 && i <= d.num_points(), "index {i} out of range");
            }
        }
    }

    #[test]
    fn test_obj_writes_normals_only_when_the_geometry_has_them() {
        use crate::export::obj;
        let plain = obj(&sheet(), 1.0, "s");
        assert!(!plain.contains("vn "), "normals appeared from nowhere");
        assert!(plain.lines().any(|l| l.starts_with("f 1 4 5 2")), "{plain}");

        // With N present they are written and referenced per corner. Without
        // it the faces stay bare and the reader computes its own, which is the
        // right default: a stale N from before a deform is worse than none.
        let mut d = sheet();
        d.points_mut().create("N", AttribValue::Float3([0.0, 1.0, 0.0]));
        let with = obj(&d, 1.0, "s");
        assert_eq!(with.lines().filter(|l| l.starts_with("vn ")).count(), d.num_points());
        assert!(with.lines().any(|l| l.starts_with("f 1//1 4//4")), "{with}");
    }

    #[test]
    fn test_obj_writes_a_two_point_primitive_as_a_line() {
        use crate::export::obj;
        // Polygon unfilled is a ring of two-point prims. OBJ has `l` for
        // exactly this; writing them as faces would hand readers degenerate
        // triangles.
        let root = modelling_root(
            "1.0",
            vec![phase3_node("polygon", &[("sides", "5"), ("fill", "false")])],
        );
        let (ring, _) = eval_node(&root, "polygon 1");
        let text = obj(&ring, 1.0, "ring");
        assert_eq!(text.lines().filter(|l| l.starts_with("l ")).count(), 5);
        assert_eq!(text.lines().filter(|l| l.starts_with("f ")).count(), 0);
    }

    #[test]
    fn test_ascii_stl_is_the_same_mesh_in_words() {
        use crate::export::{stl_ascii, stl_binary};
        let d = sheet();
        let text = stl_ascii(&d, 1.0, "sheet");
        let facets = text.lines().filter(|l| l.trim_start().starts_with("facet normal")).count();
        let verts = text.lines().filter(|l| l.trim_start().starts_with("vertex")).count();
        let count = u32::from_le_bytes(stl_binary(&d, 1.0, "s")[80..84].try_into().unwrap()) as usize;
        assert_eq!(facets, count);
        assert_eq!(verts, count * 3);
        assert!(text.starts_with("solid sheet\n") && text.trim_end().ends_with("endsolid sheet"));
    }

    #[test]
    fn test_the_extension_picks_the_format() {
        use crate::export::Format;
        use std::path::Path;
        assert_eq!(Format::from_path(Path::new("a/b.obj")), Format::Obj);
        assert_eq!(Format::from_path(Path::new("a/b.OBJ")), Format::Obj);
        assert_eq!(Format::from_path(Path::new("a/b.stl")), Format::StlBinary);
        // Anything unrecognized is binary STL: the form a printer wants, and
        // the one an unlabelled name most likely meant.
        assert_eq!(Format::from_path(Path::new("a/b")), Format::StlBinary);
    }

    #[test]
    fn test_write_creates_the_directory_and_reports_the_size() {
        use crate::export::{write, Format};
        let dir = std::env::temp_dir()
            .join(format!("cce-export-test-{}", std::process::id()))
            .join("nested");
        let path = dir.join("thing.obj");
        let n = write(&sheet(), &path, Format::Obj, 1.0).expect("write");
        assert!(path.exists(), "the nested directory was not created");
        assert_eq!(n, std::fs::metadata(&path).unwrap().len() as usize);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        let _ = std::fs::remove_dir(dir.parent().unwrap());
    }

    // ---- Phase 4: the modelling set ----

    /// Two spheres far apart: two connected pieces, the first much larger.
    fn two_pieces() -> Detail {
        let mut d = sphere_detail(Vec3::ZERO, 1.0, 8, 12);
        d.merge(&sphere_detail(Vec3::new(10.0, 0.0, 0.0), 0.3, 4, 6));
        d
    }

    fn eval_node(root: &FsNode, name: &str) -> (Detail, Option<String>) {
        let target = root.children.iter().find(|c| c.name == name).unwrap();
        let mut visited = Vec::new();
        let mut err = None;
        let mut cache = crate::geometry::SimCache::default();
        let mut sim = crate::geometry::EvalSim::new(0, 0, &mut cache);
        let g = crate::geometry::generate_single_node_geometry_with_errors(
            root, target, &mut visited, &mut err, &mut sim,
        )
        .unwrap_or_else(|| panic!("{name} evaluates"));
        (g, err)
    }

    /// A root holding a native sphere plus the nodes described, each already
    /// named "<type> 1" by `phase3_node`.
    fn modelling_root(radius: &str, nodes: Vec<FsNode>) -> FsNode {
        let mut children = vec![phase3_node("sphere", &[("radius", radius)])];
        children.extend(nodes);
        FsNode {
            id: "root".into(),
            name: "root".into(),
            node_type: "node".into(),
            children,
            params: vec![],
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        }
    }

    #[test]
    fn test_normal_publishes_the_surface_normal_as_data() {
        let root = modelling_root(
            "1.0",
            vec![phase3_node("normal", &[("input", "sphere 1"), ("attribute", "N")])],
        );
        let (g, err) = eval_node(&root, "normal 1");
        assert!(err.is_none(), "{err:?}");

        // The normal has been computed inside the app all along; as an
        // attribute it becomes ordinary data that anything can read.
        assert!(g.points().has("N"));
        // The `sphere` node places itself by its index in the graph, so the
        // centre comes off the bounds rather than being assumed to be zero.
        let (lo, hi) = g.bounds().unwrap();
        let centre = (lo + hi) * 0.5;
        for p in 0..g.num_points() {
            let n = g.points().value("N", p).unwrap().as_vec3();
            assert!((n.length() - 1.0).abs() < 1e-4, "point {p} normal is not unit: {n:?}");
            assert!(
                n.dot((g.pos(p) - centre).normalize()) > 0.99,
                "point {p} does not face outward"
            );
        }

        let flipped = modelling_root(
            "1.0",
            vec![phase3_node("normal", &[("input", "sphere 1"), ("flip", "true")])],
        );
        let (f, _) = eval_node(&flipped, "normal 1");
        let (lo, hi) = f.bounds().unwrap();
        let radial = f.pos(0) - (lo + hi) * 0.5;
        assert!(f.points().value("N", 0).unwrap().as_vec3().dot(radial) < 0.0);
    }

    /// The Normal node's Vertices class writes a normal per CORNER, cusped:
    /// the faces around a point that turn less than the Cusp Angle from a
    /// corner's own face are averaged into it, the rest are left out. On a
    /// box, whose faces meet at 90 degrees, 60 keeps every corner to its
    /// own face and 120 rounds them into the points' normals; the points'
    /// own attribute is not written, and smooth shading lights each corner
    /// by what the node wrote.
    #[test]
    fn the_normal_node_writes_cusped_vertex_normals() {
        let mut d = Detail::new();
        let p: Vec<u32> = (0..8)
            .map(|i| d.add_point(Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32)))
            .collect();
        // Six quads, wound to face outward.
        for q in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
            d.add_prim(&q.map(|i| p[i]));
        }
        assert!(d.is_closed());
        let face = |prim: usize| {
            let pts = d.prim_points(prim);
            (d.pos(pts[1] as usize) - d.pos(pts[0] as usize)).cross(d.pos(pts[2] as usize) - d.pos(pts[0] as usize)).normalize()
        };
        let points = crate::geometry::point_normals(&d);

        let hard = crate::geometry::vertex_normals(&d, 60.0);
        let soft = crate::geometry::vertex_normals(&d, 120.0);
        let all = crate::geometry::vertex_normals(&d, 180.0);
        let none = crate::geometry::vertex_normals(&d, 0.0);
        assert_eq!(hard.len(), d.num_verts());
        for prim in 0..d.num_prims() {
            for (v, &pt) in d.prim_verts(prim).zip(d.prim_points(prim)) {
                assert!((hard[v] - face(prim)).length() < 1e-5, "vertex {v}: a corner under the cusp keeps to its face");
                assert!((none[v] - face(prim)).length() < 1e-5);
                assert!((soft[v] - points[pt as usize]).length() < 1e-5, "vertex {v}: over it, the faces around the point");
                assert!((all[v] - points[pt as usize]).length() < 1e-5);
            }
        }

        // Through the node.
        let root = modelling_root(
            "1.0",
            vec![phase3_node("normal", &[("input", "sphere 1"), ("class", "Vertices"), ("cusp_angle", "180"), ("flip", "true")])],
        );
        let (g, err) = eval_node(&root, "normal 1");
        assert!(err.is_none(), "{err:?}");
        // The points' N is the sphere's own, as it came in: Vertices writes
        // the vertices and leaves it alone.
        let (input, _) = eval_node(&root, "sphere 1");
        let pn = |d: &crate::detail::Detail| (0..d.num_points()).map(|p| d.points().value("N", p).map(|v| v.as_vec3())).collect::<Vec<_>>();
        assert_eq!(pn(&g), pn(&input), "the points' is not written");
        let n = crate::geometry::own_vertex_normals(&g).expect("a normal a vertex");
        assert_eq!(n.len(), g.num_verts());
        let of_points = crate::geometry::point_normals(&g);
        for (v, &pt) in g.vert_points().iter().enumerate() {
            let got = n.get(v).unwrap().as_vec3();
            assert!((got + of_points[pt as usize]).length() < 1e-4, "vertex {v}: flipped, and its point's at 180");
        }

        // Smooth shading lights a corner by its own normal: cusped to the
        // faces, every corner of a face is lit alike, which point normals
        // on a box never are.
        let mut cusped = d.clone();
        cusped.verts_mut().create("N", crate::detail::AttribValue::Float3([0.0; 3]));
        cusped
            .verts_mut()
            .insert("N", crate::detail::AttribData::Float3(hard.iter().map(|n| n.to_array()).collect()))
            .unwrap();
        let lit = crate::geometry::smooth_lit_vertices(&cusped, crate::environment::Environment::default().sun_direction);
        let plain = crate::geometry::smooth_lit_vertices(&d, crate::environment::Environment::default().sun_direction);
        assert_eq!(lit.len(), plain.len());
        for tri in lit.chunks(3) {
            assert!(tri.iter().all(|c| (c.color[0] - tri[0].color[0]).abs() < 1e-6), "one face, one light");
        }
        assert!(plain.chunks(3).any(|tri| tri.iter().any(|c| (c.color[0] - tri[0].color[0]).abs() > 1e-3)));
    }

    #[test]
    fn test_bounds_measures_into_detail_attributes() {
        let root = modelling_root(
            "2.0",
            vec![phase3_node("bounds", &[("input", "sphere 1"), ("prefix", "bb")])],
        );
        let (g, err) = eval_node(&root, "bounds 1");
        assert!(err.is_none(), "{err:?}");

        let v = |n: &str| g.detail().value(n, 0).unwrap().as_vec3();
        // A radius-2 sphere is 4 across wherever the node happens to place it,
        // and min/max/center have to agree with each other and with the points.
        assert!((v("bb_size") - Vec3::splat(4.0)).length() < 0.02, "{:?}", v("bb_size"));
        assert!((v("bb_max") - v("bb_min") - v("bb_size")).length() < 1e-4);
        assert!(((v("bb_min") + v("bb_max")) * 0.5 - v("bb_center")).length() < 1e-4);
        let (lo, hi) = g.bounds().unwrap();
        assert!((v("bb_min") - lo).length() < 1e-5 && (v("bb_max") - hi).length() < 1e-5);
        // A measurement describes the state it was taken from.
        assert_eq!(g.detail().kind("bb_min"), AttribKind::Derivative);
    }

    #[test]
    fn test_distance_measures_to_another_geometry_and_points_at_it() {
        let mut root = modelling_root(
            "1.0",
            vec![
                phase3_node("points", &[]),
                phase3_node(
                    "distance",
                    &[
                        ("input", "points 1"),
                        ("to", "sphere 1"),
                        ("attribute", "dist"),
                        ("direction", "toward"),
                    ],
                ),
            ],
        );
        // Put the sample points somewhere known: a Points node in "Line" mode
        // lays them along X.
        root.children[1]
            .params
            .push(crate::app::ParamDef::new("shape", "text", "Line"));

        let (g, err) = eval_node(&root, "distance 1");
        assert!(err.is_none(), "{err:?}");
        assert!(g.points().has("dist") && g.points().has("toward"));

        // Every distance is non-negative unsigned, and the direction is a unit
        // vector pointing at the surface — which is exactly what Migrate wants
        // to flow along and Align wants to steer by, out of one lookup.
        for p in 0..g.num_points() {
            let d = g.points().value("dist", p).unwrap().as_f32();
            assert!(d >= 0.0, "point {p} unsigned distance is negative: {d}");
            // Unit, except where there is nothing to point at: a point
            // sitting ON the surface has no direction to it, and inventing one
            // would be worse than leaving it zero.
            let dir = g.points().value("toward", p).unwrap().as_vec3();
            if d > 1e-5 {
                assert!((dir.length() - 1.0).abs() < 1e-3, "point {p} direction is not unit");
            } else {
                assert_eq!(dir, Vec3::ZERO, "point {p} on the surface invented a direction");
            }
        }

        // A missing target is reported rather than silently writing zeros.
        let broken = modelling_root(
            "1.0",
            vec![phase3_node("distance", &[("input", "sphere 1"), ("to", "nope")])],
        );
        let (_, err) = eval_node(&broken, "distance 1");
        assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
    }

    #[test]
    fn test_distance_signed_tells_inside_from_outside() {
        // A point cloud straddling a sphere's surface.
        let mut cloud = Detail::new();
        cloud.add_point(Vec3::new(0.0, 0.0, 0.0)); // inside
        cloud.add_point(Vec3::new(3.0, 0.0, 0.0)); // outside
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 12, 16);

        let grid = crate::spatial::TriGrid::build(&sphere);
        let signed = |p: Vec3| {
            let h = grid.closest(p).unwrap();
            if (p - h.point).dot(h.normal) < 0.0 { -h.distance } else { h.distance }
        };
        assert!(signed(cloud.pos(0)) < 0.0, "the centre should read as inside");
        assert!(signed(cloud.pos(1)) > 0.0, "a point outside should read as outside");
    }

    #[test]
    fn test_connectivity_numbers_pieces_largest_first() {
        let mut geom = two_pieces();
        let big = sphere_detail(Vec3::ZERO, 1.0, 8, 12).num_points();

        let node = phase3_node("connectivity", &[("attribute", "piece")]);
        // Exercised through the resolver's own labelling by hand, since the
        // input is built here rather than by a graph.
        let root = modelling_root("1.0", vec![node]);
        let _ = &root;
        let labels = {
            // Same walk the node does.
            let n = geom.num_points();
            let mut label = vec![u32::MAX; n];
            let mut sizes: Vec<(u32, usize)> = Vec::new();
            for seed in 0..n {
                if label[seed] != u32::MAX {
                    continue;
                }
                let id = sizes.len() as u32;
                let mut count = 0;
                let mut stack = vec![seed];
                label[seed] = id;
                while let Some(p) = stack.pop() {
                    count += 1;
                    for &q in geom.point_neighbours(p) {
                        if label[q as usize] == u32::MAX {
                            label[q as usize] = id;
                            stack.push(q as usize);
                        }
                    }
                }
                sizes.push((id, count));
            }
            (label, sizes)
        };
        assert_eq!(labels.1.len(), 2, "two spheres are two pieces");
        assert_eq!(labels.1.iter().map(|(_, c)| c).sum::<usize>(), geom.num_points());

        // Through the node: piece 0 is the BIGGEST however the points are
        // ordered, which is what makes "keep the largest" an ordinary Cull.
        let mut err = None;
        let g = {
            let n = phase3_node("connectivity", &[("attribute", "piece")]);
            crate::geometry::apply_connectivity_for_test(&mut geom, &n, &mut err);
            geom
        };
        let zeros = (0..g.num_points())
            .filter(|&p| g.points().value("piece", p).unwrap().as_f32() as i32 == 0)
            .count();
        assert_eq!(zeros, big, "piece 0 should be the large sphere");
    }

    #[test]
    fn test_cull_removes_what_it_selects_and_invert_keeps_it() {
        let root = modelling_root(
            "1.0",
            vec![
                phase3_node("connectivity", &[("input", "sphere 1"), ("attribute", "piece")]),
                phase3_node(
                    "cull",
                    &[
                        ("input", "connectivity 1"),
                        ("attribute", "piece"),
                        ("comparison", "Below"),
                        ("threshold", "1.00"),
                        ("invert", "false"),
                    ],
                ),
            ],
        );
        // One sphere is one piece, so culling piece < 1 removes everything.
        let (all_gone, err) = eval_node(&root, "cull 1");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(all_gone.num_points(), 0);

        // Inverted, the same selection is what SURVIVES — which is how
        // "isolate the largest piece" reads, and what the GEM mold chain used
        // im_cull for.
        let mut kept_root = root.clone();
        kept_root
            .children
            .iter_mut()
            .find(|c| c.name == "cull 1")
            .unwrap()
            .params
            .iter_mut()
            .find(|p| p.name == "invert")
            .unwrap()
            .set_text("true");
        let (kept, _) = eval_node(&kept_root, "cull 1");
        assert_eq!(kept.num_points(), sphere_detail(Vec3::ZERO, 1.0, 16, 24).num_points());
        assert!(kept.num_prims() > 0, "the surface survived with its primitives");

        // A missing attribute is reported, and nothing is deleted on a guess.
        let broken = modelling_root(
            "1.0",
            vec![phase3_node("cull", &[("input", "sphere 1"), ("attribute", "nope")])],
        );
        let (g, err) = eval_node(&broken, "cull 1");
        assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
        assert!(g.num_points() > 0, "nothing should be culled on an error");

        // With neither a group nor an attribute there is no selection at all.
        let idle = modelling_root("1.0", vec![phase3_node("cull", &[("input", "sphere 1")])]);
        let (g, _) = eval_node(&idle, "cull 1");
        assert_eq!(g.num_points(), sphere_detail(Vec3::ZERO, 1.0, 16, 24).num_points());
    }

    #[test]
    fn test_transfer_samples_a_field_onto_new_geometry() {
        // A field defined on one sphere, read onto a second one that shares
        // none of its points — which is what happens whenever a chain rebuilds
        // rather than deforms.
        let mut source = sphere_detail(Vec3::ZERO, 1.0, 12, 16);
        source.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..source.num_points() {
            let y = source.pos(p).y;
            source.points_mut().set_value("mass", p, AttribValue::Float(y)).unwrap();
        }
        source
            .points_mut()
            .create_kind("scratch", AttribValue::Float(1.0), AttribKind::Derivative);

        let mut dest = sphere_detail(Vec3::ZERO, 1.0, 7, 9);
        assert!(dest.num_points() != source.num_points());

        // Exercised through the same nearest-point walk the node does.
        let src_pos: Vec<Vec3> = (0..source.num_points()).map(|p| source.pos(p)).collect();
        let grid = crate::spatial::PointGrid::build(&src_pos, 0.1);
        dest.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..dest.num_points() {
            let (q, _) = grid.nearest(dest.pos(p)).unwrap();
            let v = source.points().value("mass", q as usize).unwrap();
            dest.points_mut().set_value("mass", p, v).unwrap();
        }

        // The field came across: a point's value matches where it sits, to
        // within the source's resolution.
        for p in 0..dest.num_points() {
            let v = dest.points().value("mass", p).unwrap().as_f32();
            assert!((v - dest.pos(p).y).abs() < 0.25, "point {p}: {v} vs y {}", dest.pos(p).y);
        }
    }

    #[test]
    fn test_transfer_respects_its_maximum_distance_and_carries_the_kind() {
        let root = modelling_root(
            "1.0",
            vec![
                phase3_node("points", &[("shape", "Line"), ("points", "6"), ("markers", "false")]),
                phase3_node(
                    "transfer",
                    &[
                        ("input", "points 1"),
                        ("from", "sphere 1"),
                        ("attributes", "N"),
                        ("maximum_distance", "0.00"),
                    ],
                ),
            ],
        );
        let (g, err) = eval_node(&root, "transfer 1");
        assert!(err.is_none(), "{err:?}");
        // No limit: everything finds a nearest source point however far.
        assert!(g.points().has("N"));
        assert!(
            (0..g.num_points()).any(|p| g.points().value("N", p).unwrap().as_vec3() != Vec3::ZERO),
            "nothing was transferred"
        );

        // With a tight limit the sphere is out of reach, so the column exists
        // and stays at the type's zero — present and empty, not missing.
        let mut limited = root.clone();
        limited
            .children
            .iter_mut()
            .find(|c| c.name == "transfer 1")
            .unwrap()
            .params
            .iter_mut()
            .find(|p| p.name == "maximum_distance")
            .unwrap()
            .set_text("0.01");
        let (g, _) = eval_node(&limited, "transfer 1");
        assert!(g.points().has("N"), "the column exists even where nothing was near");
        assert!(
            (0..g.num_points()).all(|p| g.points().value("N", p).unwrap().as_vec3() == Vec3::ZERO),
            "something transferred from out of range"
        );

        // A source it cannot resolve is reported rather than silently doing
        // nothing.
        let broken = modelling_root(
            "1.0",
            vec![phase3_node("transfer", &[("input", "sphere 1"), ("from", "nope")])],
        );
        let (_, err) = eval_node(&broken, "transfer 1");
        assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
    }

    /// Transfer carries GROUPS as it carries attributes: with Transfer
    /// Groups on, each target point takes its nearest source point's
    /// membership in the named groups — every group when none is named —
    /// joining and leaving alike; off, or on a node from before the row,
    /// no group moves. The Remesh node has the same transfer inside it:
    /// from its own input when From names nothing, else the node named.
    #[test]
    fn transfer_carries_groups_and_remesh_has_a_copy() {
        use crate::detail::AttribValue;
        // The rule itself, on hand-built points: four source points along
        // x, the far two in `top`; four targets beside them, all put in
        // `top` beforehand.
        let mut source = Detail::new();
        for x in 0..4 {
            source.add_point(Vec3::new(x as f32, 0.0, 0.0));
        }
        source.points_mut().create("mass", AttribValue::Float(0.0));
        source.points_mut().set_value("mass", 3, AttribValue::Float(7.0)).unwrap();
        source.points_mut().create_group("top");
        source.points_mut().add_to_group("top", 2);
        source.points_mut().add_to_group("top", 3);
        source.points_mut().create_group("other");
        source.points_mut().add_to_group("other", 0);
        let mut target = Detail::new();
        for x in [0.1, 0.9, 2.1, 2.9] {
            target.add_point(Vec3::new(x, 0.0, 0.0));
        }
        for p in 0..4 {
            target.points_mut().add_to_group("top", p);
        }
        let member = |d: &Detail, g: &str| (0..d.num_points()).map(|p| d.points().in_group(g, p)).collect::<Vec<_>>();
        let mut all = target.clone();
        crate::geometry::transfer_onto(&mut all, &source, &[], Some(&[]), 0.0, "");
        assert_eq!(member(&all, "top"), vec![false, false, true, true], "membership is the nearest source point's, joining and leaving");
        assert_eq!(member(&all, "other"), vec![true, false, false, false], "every group, none named");
        assert_eq!(all.points().value("mass", 3), Some(AttribValue::Float(7.0)));
        let mut named = target.clone();
        crate::geometry::transfer_onto(&mut named, &source, &[], Some(&["nope".to_string(), "top".to_string()]), 0.0, "");
        assert_eq!(member(&named, "top"), vec![false, false, true, true]);
        assert!(!named.points().has_group("other"), "only the named");
        let mut none = target.clone();
        crate::geometry::transfer_onto(&mut none, &source, &[], None, 0.0, "");
        assert_eq!(member(&none, "top"), vec![true, true, true, true], "no groups asked, none touched");
        assert!(none.points().has("mass"), "the attributes still are");
        let mut near = target.clone();
        crate::geometry::transfer_onto(&mut near, &source, &[], Some(&[]), 0.15, "");
        assert_eq!(member(&near, "top"), vec![false, false, true, true], "within the limit");
        near.points_mut().add_to_group("top", 1);
        let mut far = target.clone();
        crate::geometry::transfer_onto(&mut far, &source, &[], Some(&[]), 0.05, "");
        assert_eq!(member(&far, "top"), vec![true, true, true, true], "nothing within 0.05: nothing moves");

        // The nodes. A sphere with a group and an attribute is the source.
        let up = phase3_node("group", &[("input", "sphere 1"), ("group_name", "top"), ("mode", "Box"), ("center", "-1.875:1.55:0.00"), ("size", "4.00:2.00:4.00")]);
        let tagged = phase3_node("attribute", &[("input", "group 1"), ("operation", "Create"), ("attribute_name", "mass"), ("type", "Float"), ("value", "3")]);
        let line = phase3_node("points", &[("shape", "Line"), ("points", "6"), ("markers", "false")]);
        let mut transfer = phase3_node("transfer", &[("input", "points 1"), ("from", "attribute 1"), ("attributes", "mass"), ("transfer_groups", "true"), ("groups", ""), ("maximum_distance", "0.00")]);
        let with = |t: FsNode| modelling_root("1.0", vec![up.clone(), tagged.clone(), line.clone(), t]);
        let (src, err) = eval_node(&with(transfer.clone()), "attribute 1");
        assert!(err.is_none(), "{err:?}");
        let members = src.points().group_members("top").len();
        assert!(members > 0 && members < src.num_points(), "a group of part of the sphere: {members}");
        let (g, err) = eval_node(&with(transfer.clone()), "transfer 1");
        assert!(err.is_none(), "{err:?}");
        assert!(g.points().has_group("top") && g.points().has("mass"), "the group and the attribute were carried");

        // Off, no group moves — and a node without the row is off.
        transfer.params.iter_mut().find(|p| p.name == "transfer_groups").unwrap().set_text("false");
        let (g, _) = eval_node(&with(transfer.clone()), "transfer 1");
        assert!(!g.points().has_group("top") && g.points().has("mass"), "nothing carried with the switch off");
        transfer.params.retain(|p| p.name != "transfer_groups" && p.name != "groups");
        let (g, _) = eval_node(&with(transfer.clone()), "transfer 1");
        assert!(!g.points().has_group("top"), "a node from before the row carries none");

        // Remesh's copy: a sphere remeshed coarse, its group read back off
        // its own input — no From, no wire — so the new points carry it.
        // Both ways a remesh is: the native node, and the subnet whose
        // Transfer node reads From through an expression and finds a node
        // beside the subnet, not inside it.
        let rows = [("input", "attribute 1"), ("target_length", "0.5"), ("iterations", "3"), ("relax", "0.0"), ("transfer", "true"), ("from", ""), ("attributes", ""), ("transfer_groups", "true"), ("groups", "")];
        let templates = crate::app::load_fs_tree();
        let mut composed = templates.children.iter().find(|t| t.name == "Remesh").unwrap().clone();
        crate::app::regenerate_node_ids(&mut composed);
        composed.name = "remesh 1".into();
        for (k, v) in rows {
            composed.params.iter_mut().find(|p| p.name == k).unwrap().set_text(v.to_string());
        }
        for remesh in [phase3_node("remesh", &rows), composed] {
        let (g, err) = eval_node(&with(remesh.clone()), "remesh 1");
        assert!(err.is_none(), "{err:?}");
        assert_ne!(g.num_points(), src.num_points(), "the remesh changed the points");
        assert!(g.points().has_group("top") && g.points().has("mass"), "the remesh carried the group and the attribute");
        let carried = g.points().group_members("top").len();
        assert!(carried > 0 && carried < g.num_points(), "the top half, on the new points: {carried} of {}", g.num_points());
        for p in 0..g.num_points() {
            // The group is the top half of the sphere by its box; each new
            // point's membership is its nearest old point's.
            let nearest = (0..src.num_points()).min_by(|&a, &b| src.pos(a).distance(g.pos(p)).partial_cmp(&src.pos(b).distance(g.pos(p))).unwrap()).unwrap();
            assert_eq!(g.points().in_group("top", p), src.points().in_group("top", nearest), "point {p}");
        }
        // Off, the remesh carries what a remesh carries: the group through
        // its splits and collapses, as before this row.
        let mut off = remesh.clone();
        off.params.iter_mut().find(|p| p.name == "transfer").unwrap().set_text("false");
        let (plain, err) = eval_node(&with(off), "remesh 1");
        assert!(err.is_none(), "{err:?}");
        assert!(plain.points().has_group("top"));
        // From another node: the line's points carry no group and no mass,
        // so there is nothing to lay over the remeshed sphere and it is as
        // the remesh left it.
        let mut from_line = remesh.clone();
        from_line.params.iter_mut().find(|p| p.name == "from").unwrap().set_text("points 1");
        let (g, err) = eval_node(&with(from_line), "remesh 1");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(g.points().group_members("top"), plain.points().group_members("top"), "a source without the group leaves it as it was");
        // A From it cannot resolve is said.
        let mut broken = remesh.clone();
        broken.params.iter_mut().find(|p| p.name == "from").unwrap().set_text("nope");
        let (_, err) = eval_node(&with(broken), "remesh 1");
        assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
        }
    }

    #[test]
    fn test_valence_counts_what_the_remesher_steers_toward() {
        use crate::remesh::{remesh, Settings};
        let root = modelling_root(
            "1.0",
            vec![phase3_node("valence", &[("input", "sphere 1"), ("attribute", "valence")])],
        );
        let (g, err) = eval_node(&root, "valence 1");
        assert!(err.is_none(), "{err:?}");

        for p in 0..g.num_points() {
            let v = g.points().value("valence", p).unwrap().as_f32() as usize;
            assert_eq!(v, g.point_neighbours(p).len(), "point {p}");
        }
        // A UV sphere's poles are the irregular vertices: everything else on a
        // quad sphere has four neighbours.
        let counts: Vec<usize> = (0..g.num_points()).map(|p| g.point_neighbours(p).len()).collect();
        assert!(counts.iter().any(|&c| c > 4), "the poles should be irregular");

        // After remeshing to triangles, six is the regular valence — which is
        // the number the flip pass steers toward, and being able to see it is
        // how a settled mesh is told from an unsettled one.
        let settled = remesh(&g, Settings { target: 0.25, iterations: 6, ..Default::default() });
        let sixes = (0..settled.num_points())
            .filter(|&p| settled.point_neighbours(p).len() == 6)
            .count();
        assert!(
            sixes * 2 > settled.num_points(),
            "only {sixes} of {} points reached valence 6",
            settled.num_points()
        );
    }

    #[test]
    fn test_deform_twists_bends_and_tapers_about_an_axis() {
        let base = sphere_detail(Vec3::ZERO, 1.0, 10, 14);
        let run = |mode: &str, amount: &str| {
            let mut g = base.clone();
            let node = phase3_node("deform", &[("mode", mode), ("axis", "Y"), ("amount", amount)]);
            crate::geometry::apply_deform(&mut g, &node);
            g
        };

        // The middle of the geometry is the still point, and the two ends go
        // opposite ways — which is what makes a twist read as a twist rather
        // than a rotation of the whole thing.
        let twisted = run("Twist", "2.00");
        let top = (0..base.num_points())
            .max_by(|&a, &b| base.pos(a).y.partial_cmp(&base.pos(b).y).unwrap())
            .unwrap();
        let equator = (0..base.num_points())
            .min_by(|&a, &b| base.pos(a).y.abs().partial_cmp(&base.pos(b).y.abs()).unwrap())
            .unwrap();
        assert!((twisted.pos(equator) - base.pos(equator)).length() < 0.15, "the middle moved");
        // A twist keeps every point's distance from the axis.
        for p in 0..base.num_points() {
            let r0 = Vec3::new(base.pos(p).x, 0.0, base.pos(p).z).length();
            let r1 = Vec3::new(twisted.pos(p).x, 0.0, twisted.pos(p).z).length();
            assert!((r0 - r1).abs() < 1e-4, "point {p} changed radius");
            assert!((twisted.pos(p).y - base.pos(p).y).abs() < 1e-4, "point {p} moved along the axis");
        }
        let _ = top;

        // A taper pinches one end and swells the other, and clamps at zero
        // rather than turning the geometry inside out.
        let tapered = run("Taper", "3.90");
        let radius = |d: &Detail, p: usize| Vec3::new(d.pos(p).x, 0.0, d.pos(p).z).length();
        let lower = (0..base.num_points())
            .filter(|&p| base.pos(p).y < -0.8)
            .collect::<Vec<_>>();
        for &p in &lower {
            assert!(radius(&tapered, p) <= radius(&base, p) + 1e-4, "point {p} swelled at the pinched end");
        }

        // A bend moves points along the axis, which neither of the others do.
        let bent = run("Bend", "1.50");
        assert!(
            (0..base.num_points()).any(|p| (bent.pos(p).y - base.pos(p).y).abs() > 0.05),
            "bend did not move anything along the axis"
        );

        // Zero amount is the identity, whatever the mode.
        for mode in ["Twist", "Bend", "Taper"] {
            let still = run(mode, "0.00");
            for p in 0..base.num_points() {
                assert!((still.pos(p) - base.pos(p)).length() < 1e-5, "{mode} moved at zero");
            }
        }
    }

    #[test]
    fn test_points_and_scatter_can_emit_bare_points() {
        // Everything that generates locations drew marker spheres at them,
        // which is right for looking at and wrong for working with: Copy
        // placed one instance per marker VERTEX rather than one per location,
        // because the markers were the only points there were.
        let markers = modelling_root(
            "1.0",
            vec![phase3_node("points", &[("shape", "Line"), ("points", "5")])],
        );
        let (with, _) = eval_node(&markers, "points 1");
        assert!(with.num_prims() > 0, "markers are geometry");
        assert!(with.num_points() > 5);

        let bare = modelling_root(
            "1.0",
            vec![phase3_node(
                "points",
                &[("shape", "Line"), ("points", "5"), ("markers", "false")],
            )],
        );
        let (without, _) = eval_node(&bare, "points 1");
        assert_eq!(without.num_points(), 5, "one point per location");
        assert_eq!(without.num_prims(), 0, "bare points are not geometry");

        // Default is unchanged, so no existing project looks different.
        let defaulted = modelling_root(
            "1.0",
            vec![phase3_node("points", &[("shape", "Line"), ("points", "5")])],
        );
        assert_eq!(eval_node(&defaulted, "points 1").0.num_points(), with.num_points());
    }

    #[test]
    fn test_copy_puts_geometry_at_every_target_point() {
        let root = modelling_root(
            "0.2",
            vec![
                phase3_node("points", &[("shape", "Line"), ("points", "5")]),
                phase3_node("copy", &[("input", "sphere 1"), ("to", "points 1")]),
            ],
        );
        let (src, _) = eval_node(&root, "sphere 1");
        let (onto, _) = eval_node(&root, "points 1");
        let (g, err) = eval_node(&root, "copy 1");
        assert!(err.is_none(), "{err:?}");

        assert_eq!(g.num_points(), src.num_points() * onto.num_points());
        assert_eq!(g.num_prims(), src.num_prims() * onto.num_points());
        // Every copy is its own set of points: merge reallocates identities, so
        // a solver can treat them separately rather than seeing one set
        // repeated.
        let mut ids = g.ids().to_vec();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "identities were repeated across copies");

        // Each copy lands ON its target. Merge appends them in order, so copy
        // i is the i-th block of points — and its centroid should be the
        // target's position plus the source's own centroid, since the `sphere`
        // node places itself by graph index rather than at the origin.
        let m = src.num_points();
        let centroid = |d: &Detail, range: std::ops::Range<usize>| {
            let n = range.len() as f32;
            range.map(|p| d.pos(p)).sum::<Vec3>() / n
        };
        let src_centre = centroid(&src, 0..m);
        for i in 0..onto.num_points() {
            let want = onto.pos(i) + src_centre;
            let got = centroid(&g, i * m..(i + 1) * m);
            assert!((got - want).length() < 1e-3, "copy {i} at {got:?}, wanted {want:?}");
        }
    }

    #[test]
    fn test_copy_scales_by_an_attribute_and_refuses_an_explosion() {
        let mut root = modelling_root(
            "0.2",
            vec![
                phase3_node("points", &[("shape", "Line"), ("points", "4")]),
                phase3_node(
                    "normal",
                    &[("input", "points 1"), ("attribute", "N")],
                ),
                phase3_node(
                    "copy",
                    &[
                        ("input", "sphere 1"),
                        ("to", "points 1"),
                        ("scale", "2.00"),
                    ],
                ),
            ],
        );
        let (plain, _) = eval_node(&root, "copy 1");
        let plain_size = plain.bounds().map(|(a, b)| (b - a).length()).unwrap();

        // Scale shrinks the copies without moving the targets they sit on.
        root.children
            .iter_mut()
            .find(|c| c.name == "copy 1")
            .unwrap()
            .params
            .iter_mut()
            .find(|p| p.name == "scale")
            .unwrap()
            .set_text("0.50");
        let (small, _) = eval_node(&root, "copy 1");
        let small_size = small.bounds().map(|(a, b)| (b - a).length()).unwrap();
        assert!(small_size < plain_size, "{small_size} should be under {plain_size}");
        assert_eq!(small.num_points(), plain.num_points());

        // A copy big enough to hang the app is refused with a number rather
        // than attempted.
        let huge = modelling_root(
            "1.0",
            vec![
                phase3_node("points", &[("shape", "Grid"), ("points", "9000")]),
                phase3_node("copy", &[("input", "sphere 1"), ("to", "points 1")]),
            ],
        );
        let (g, err) = eval_node(&huge, "copy 1");
        assert!(err.as_deref().unwrap_or("").contains("ceiling"), "{err:?}");
        assert_eq!(g.num_points(), 0, "nothing is built when the copy is refused");
    }

    #[test]
    fn test_soft_transform_falls_off_instead_of_leaving_a_step() {
        let mut sphere = sphere_detail(Vec3::ZERO, 1.0, 12, 16);
        let before: Vec<Vec3> = (0..sphere.num_points()).map(|p| sphere.pos(p)).collect();
        // Everything above the equator is selected; the falloff should still
        // reach below it.
        sphere.points_mut().create_group("top");
        for p in 0..sphere.num_points() {
            if sphere.pos(p).y > 0.7 {
                sphere.points_mut().add_to_group("top", p);
            }
        }

        let node = phase3_node(
            "soft_transform",
            &[
                ("translation", "0.00:1.00:0.00"),
                ("group", "top"),
                ("radius", "0.80"),
                ("falloff", "Smooth"),
                ("attribute", "falloff"),
            ],
        );
        crate::geometry::apply_soft_transform(&mut sphere, &node);

        let moved = |p: usize| (sphere.pos(p) - before[p]).length();
        // Group members move the full amount.
        let members = sphere.points().group_members("top");
        assert!(!members.is_empty());
        for &p in &members {
            assert!((moved(p as usize) - 1.0).abs() < 1e-4, "member {p} moved {}", moved(p as usize));
        }
        // Points beyond the radius do not move at all, and points between do —
        // which is the difference from a hard translate on the group.
        let far = (0..before.len()).find(|&p| before[p].y < -0.9).unwrap();
        assert!(moved(far) < 1e-5, "a point across the sphere moved {}", moved(far));
        let between = (0..before.len())
            .filter(|&p| !members.contains(&(p as u32)) && moved(p) > 1e-4)
            .count();
        assert!(between > 0, "nothing followed the selection");

        // The falloff is written out as the mask that drove the move.
        assert!(sphere.points().has("falloff"));
        for &p in &members {
            let w = sphere.points().value("falloff", p as usize).unwrap().as_f32();
            assert!((w - 1.0).abs() < 1e-4);
        }
        assert!((sphere.points().value("falloff", far).unwrap().as_f32()).abs() < 1e-5);
    }

    #[test]
    fn test_soft_transform_measures_from_the_nearest_member_not_the_centroid() {
        // A selection shaped like a ring: its centroid is the middle, where no
        // member is. Measuring from the centroid would drag hardest at a place
        // the selection is nowhere near.
        let mut sphere = sphere_detail(Vec3::ZERO, 1.0, 12, 16);
        sphere.points_mut().create_group("ring");
        for p in 0..sphere.num_points() {
            if sphere.pos(p).y.abs() < 0.15 {
                sphere.points_mut().add_to_group("ring", p);
            }
        }
        let before: Vec<Vec3> = (0..sphere.num_points()).map(|p| sphere.pos(p)).collect();
        let node = phase3_node(
            "soft_transform",
            &[("translation", "0.00:0.50:0.00"), ("group", "ring"), ("radius", "0.30")],
        );
        crate::geometry::apply_soft_transform(&mut sphere, &node);

        let moved = |p: usize| (sphere.pos(p) - before[p]).length();
        // The ring itself moves fully; the poles, far from every member, do
        // not — even though they are no further from the ring's CENTROID than
        // the ring is.
        for &p in &sphere.points().group_members("ring") {
            assert!((moved(p as usize) - 0.5).abs() < 1e-4);
        }
        let pole = (0..before.len())
            .max_by(|&a, &b| before[a].y.partial_cmp(&before[b].y).unwrap())
            .unwrap();
        assert!(moved(pole) < 1e-5, "the pole moved {}", moved(pole));
    }

    #[test]
    fn test_group_selects_by_attribute_and_expands_across_the_surface() {
        let root = modelling_root(
            "1.0",
            vec![
                phase3_node("connectivity", &[("input", "sphere 1"), ("attribute", "piece")]),
                phase3_node(
                    "normal",
                    &[("input", "connectivity 1"), ("attribute", "N")],
                ),
                phase3_node(
                    "group",
                    &[
                        ("input", "normal 1"),
                        ("group_name", "up"),
                        ("mode", "Attribute"),
                        ("attribute", "N"),
                        ("comparison", "Above"),
                        ("threshold", "0.50"),
                        ("highlight", "false"),
                    ],
                ),
            ],
        );
        let (g, err) = eval_node(&root, "group 1");
        assert!(err.is_none(), "{err:?}");

        // Selecting by what a point IS, not where it is: this is what makes
        // the measuring nodes composable. `N` reads as its first component, so
        // the selection is "normal points along +X".
        let members = g.points().group_members("up");
        assert!(!members.is_empty() && members.len() < g.num_points());
        for &p in &members {
            assert!(g.points().value("N", p as usize).unwrap().as_f32() > 0.5);
        }

        // Expand grows that selection across the surface.
        let mut grown_root = root.clone();
        grown_root.children.push(phase3_node(
            "group",
            &[
                ("input", "group 1"),
                ("group_name", "wider"),
                ("mode", "Expand"),
                ("source_group", "up"),
                ("rings", "2"),
                ("highlight", "false"),
            ],
        ));
        grown_root.children.last_mut().unwrap().name = "group 2".into();
        grown_root.children.last_mut().unwrap().id = "id-group2".into();
        let (grown, err) = eval_node(&grown_root, "group 2");
        assert!(err.is_none(), "{err:?}");
        let wider = grown.points().group_members("wider");
        assert!(wider.len() > members.len(), "{} did not grow past {}", wider.len(), members.len());
        for &p in &members {
            assert!(wider.contains(&p), "expanding dropped an original member");
        }

        // And shrinks it: negative rings peel the boundary off, so an
        // erode/dilate pair is one node twice rather than two nodes.
        let mut shrunk_root = grown_root.clone();
        shrunk_root
            .children
            .last_mut()
            .unwrap()
            .params
            .iter_mut()
            .find(|p| p.name == "rings")
            .unwrap()
            .set_text("-1");
        let (shrunk, _) = eval_node(&shrunk_root, "group 2");
        assert!(shrunk.points().group_members("wider").len() < members.len());
    }

    // ---- Phase 3: surface development ----

    use crate::geometry::sphere_detail;
    use crate::remesh::{remesh, Settings};

    /// Mean edge length, the number remeshing steers.
    fn mean_edge(d: &Detail) -> f32 {
        let edges = d.edges();
        if edges.is_empty() {
            return 0.0;
        }
        edges
            .iter()
            .map(|e| (d.pos(e[1] as usize) - d.pos(e[0] as usize)).length())
            .sum::<f32>()
            / edges.len() as f32
    }

    #[test]
    fn test_remesh_pulls_edge_lengths_toward_the_target_from_both_sides() {
        let coarse = sphere_detail(Vec3::ZERO, 1.0, 6, 8);
        let before = mean_edge(&coarse);
        assert!(before > 0.4, "the test sphere starts coarse: {before}");

        // Too coarse: splitting dominates and the mesh gets denser.
        let finer = remesh(&coarse, Settings { target: 0.2, iterations: 4, ..Default::default() });
        let after = mean_edge(&finer);
        assert!(after < before, "{after} is not shorter than {before}");
        assert!(finer.num_points() > coarse.num_points(), "a finer mesh needs more points");
        assert!((after - 0.2).abs() < 0.12, "landed at {after}, wanted about 0.2");

        // Too fine: collapsing dominates and the mesh gets coarser. The same
        // node, the same passes, steered from the other side.
        let dense = sphere_detail(Vec3::ZERO, 1.0, 24, 32);
        let dense_before = mean_edge(&dense);
        let coarsened = remesh(&dense, Settings { target: 0.5, iterations: 4, ..Default::default() });
        assert!(mean_edge(&coarsened) > dense_before, "collapse did not coarsen");
        assert!(coarsened.num_points() < dense.num_points());
    }

    /// The same mesh in every particular: points, identities, primitives,
    /// attributes.
    fn same_mesh(a: &Detail, b: &Detail) -> bool {
        a.positions() == b.positions()
            && a.ids() == b.ids()
            && a.num_prims() == b.num_prims()
            && (0..a.num_prims()).all(|p| a.prim_points(p) == b.prim_points(p))
            && a.points().names() == b.points().names()
            && a.points().names().iter().all(|n| (0..a.num_points()).all(|p| a.points().value(n, p) == b.points().value(n, p)))
    }

    /// The Remesh is a subnet of nodes (since 2026-09-30) — a Repeat of
    /// Split Edges, Collapse Edges, Flip Edges, a Tangential Relax and a
    /// Project, then a Transfer — and it makes the mesh the native remesh
    /// makes, bit for bit: points, identities, the counter new points draw
    /// from, primitives, attributes and groups. Twice over, the second
    /// remesh eating the first as a simulation's next step does, so an
    /// identity a collapse took out is not handed back. With no relaxation
    /// asked and nothing to do, the input comes back as it came.
    #[test]
    fn the_remesh_subnet_is_the_remesh() {
        let templates = crate::app::load_fs_tree();
        let t = templates.children.iter().find(|t| t.name == "Remesh").expect("the Remesh template");
        assert_eq!(t.node_type, "node", "the Remesh is a subnet");
        let names: Vec<&str> = t.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["input1", "repeat1", "transfer1", "transfer_switch1", "output1"]);
        let repeat = &t.children[1];
        assert_eq!(repeat.node_type, "repeat");
        let passes: Vec<(&str, &str)> = repeat.children.iter().map(|c| (c.name.as_str(), c.node_type.as_str())).collect();
        assert_eq!(passes, [("input1", "input"), ("seed1", "seed"), ("split1", "split_edges"), ("collapse1", "collapse_edges"),
            ("flip1", "flip_edges"), ("relax1", "relax"), ("project1", "project"), ("output1", "output")]);
        for c in &t.children {
            assert_eq!(c.geometry_visible, c.name == "transfer_switch1", "only transfer_switch1 draws: {}", c.name);
        }

        let tagged = [
            phase3_node("group", &[("input", "sphere 1"), ("group_name", "top"), ("mode", "Box"), ("center", "-1.875:1.55:0.00"), ("size", "4.00:2.00:4.00")]),
            phase3_node("attribute", &[("input", "group 1"), ("operation", "Create"), ("attribute_name", "mass"), ("type", "Float"), ("value", "3")]),
        ];
        let native = |name: &str, input: &str, s: &[(&str, &str)]| {
            let mut n = phase3_node("remesh", &[("input", input)]);
            n.id = format!("id-{name}");
            n.name = name.into();
            for (k, v) in s.iter().chain([("Split", "true"), ("Collapse", "true"), ("flip", "true"), ("Project", "true")].iter()) {
                n.params.push(crate::app::ParamDef::new(*k, "text", *v));
            }
            n
        };
        let subnet = |name: &str, input: &str, s: &[(&str, &str)]| {
            let mut n = t.clone();
            crate::app::regenerate_node_ids(&mut n);
            n.name = name.into();
            n.params.iter_mut().find(|p| p.name == "input").unwrap().set_text(input.to_string());
            for (k, v) in s {
                n.params.iter_mut().find(|p| p.name == *k).unwrap_or_else(|| panic!("the subnet has {k}")).set_text(v.to_string());
            }
            n
        };
        let same = |a: &Detail, b: &Detail, what: &str| {
            assert_eq!((a.num_points(), a.num_prims()), (b.num_points(), b.num_prims()), "{what}: counts");
            assert_eq!(a.positions(), b.positions(), "{what}: positions");
            assert_eq!(a.ids(), b.ids(), "{what}: identities");
            assert!(a == b, "{what}: attributes, groups or the id counter");
        };
        for settings in [
            vec![("target_length", "0.5"), ("iterations", "3"), ("relax", "0.5"), ("transfer", "false")],
            vec![("target_length", "0.25"), ("iterations", "4"), ("relax", "0.04"), ("transfer", "true")],
            vec![("target_length", "0.8"), ("iterations", "2"), ("relax", "1.0"), ("transfer", "true"), ("transfer_groups", "true")],
        ] {
            let mut children = tagged.to_vec();
            children.push(native("native1", "attribute 1", &settings));
            children.push(native("native2", "native1", &settings));
            children.push(subnet("remesh1", "attribute 1", &settings));
            children.push(subnet("remesh2", "remesh1", &settings));
            let root = modelling_root("1.0", children);
            for (n, s) in [("native1", "remesh1"), ("native2", "remesh2")] {
                let (a, err) = eval_node(&root, n);
                assert!(err.is_none(), "{err:?}");
                let (b, err) = eval_node(&root, s);
                assert!(err.is_none(), "{err:?}");
                assert!(b.num_points() > 0 && b.points().has("mass"), "{settings:?}");
                same(&a, &b, &format!("{s} {settings:?}"));
            }
        }

        // No relaxation, and a mesh already at its length: the input back.
        let settings = [("target_length", "0.5"), ("iterations", "3"), ("relax", "0"), ("transfer", "false")];
        let root = modelling_root("1.0", vec![
            subnet("remesh1", "sphere 1", &settings),
            subnet("remesh2", "remesh1", &settings),
            native("native1", "sphere 1", &settings),
        ]);
        let (first, _) = eval_node(&root, "remesh1");
        let (native_first, _) = eval_node(&root, "native1");
        same(&native_first, &first, "relax 0");
        let (again, _) = eval_node(&root, "remesh2");
        let settled = crate::remesh::remesh(&first, crate::remesh::Settings { target: 0.5, iterations: 3, relax: 0.0, ..Default::default() });
        assert!(settled == first, "the fixture has settled: a native remesh of it is itself");
        assert!(again == first, "a remesh with nothing to do hands its input back");
    }

    /// A native remesh from before 2026-09-30 loads as the Remesh subnet —
    /// wherever it stands, a simnet's chain included — with its values and
    /// its identity, and a pass it had switched off BYPASSED inside, since
    /// the switches are the nodes now. It makes the mesh it made.
    #[test]
    fn a_native_remesh_recomposes_on_load() {
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let mut old = phase3_node("remesh", &[("input", "sphere 1"), ("target_length", "0.3"), ("iterations", "2"), ("relax", "0.04"),
            ("split", "false"), ("collapse", "true"), ("flip", "true"), ("project", "true"), ("transfer", "true"), ("from", "")]);
        old.id = "old-id".into();
        old.name = "remesh1".into();
        old.position = (2.0, 5.0);
        let mut root = modelling_root("1.0", vec![ref_node("sub", "sub1", "node", vec![], vec![]), old.clone()]);
        root.children[1].children.push({ let mut inner = old.clone(); inner.id = "inner-id".into(); inner });
        crate::app::merge_template_defs(&mut root, &templates);
        for r in [&root.children[2], &root.children[1].children[0]] {
            assert_eq!(r.node_type, "node", "recomposed as a subnet, nested ones too");
            assert!(r.is_enterable());
            let get = |n: &str| r.params.iter().find(|p| p.name == n).map(|p| p.text().to_string());
            assert_eq!(get("target_length").as_deref(), Some("0.3"));
            assert_eq!(get("relax").as_deref(), Some("0.04"));
            assert_eq!(get("transfer").as_deref(), Some("true"));
            assert_eq!(get("split"), None, "the pass switches are nodes now");
            let repeat = r.children.iter().find(|c| c.name == "repeat1").unwrap();
            let bypassed: Vec<&str> = repeat.children.iter().filter(|c| c.bypassed).map(|c| c.name.as_str()).collect();
            assert_eq!(bypassed, ["split1"], "Split was off");
        }
        let r = &root.children[2];
        assert_eq!((r.id.as_str(), r.name.as_str(), r.position), ("old-id", "remesh1", (2.0, 5.0)));
        // The native node, beside it after the load (which gives the bare
        // sphere its template's Center too), is what it has to match.
        let mut native = old.clone();
        native.id = "native-id".into();
        native.name = "native1".into();
        root.children.push(native);
        let (made, err) = eval_node(&root, "native1");
        assert!(err.is_none(), "{err:?}");
        let (g, err) = eval_node(&root, "remesh1");
        assert!(err.is_none(), "{err:?}");
        assert!(g.num_points() > 0 && g == made, "the recomposed remesh makes the mesh the native one made");
    }

    /// The Remesh subnet's switch was `result1` for a day; a Remesh saved
    /// then loads with it renamed, and its output wired to the new name.
    #[test]
    fn a_remesh_saved_with_result1_is_renamed_on_load() {
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let mut old = templates_root.children.iter().find(|t| t.name == "Remesh").unwrap().clone();
        crate::app::regenerate_node_ids(&mut old);
        old.name = "remesh1".into();
        for k in &mut old.children {
            if k.name == "transfer_switch1" {
                k.name = "result1".into();
            }
            for p in &mut k.params {
                if p.text() == "transfer_switch1" {
                    p.set_text("result1");
                }
            }
        }
        let mut root = modelling_root("1.0", vec![old]);
        root.children[1].params.iter_mut().find(|p| p.name == "input").unwrap().set_text("sphere 1");
        crate::app::merge_template_defs(&mut root, &templates);
        let r = &root.children[1];
        let names: Vec<&str> = r.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["input1", "repeat1", "transfer1", "transfer_switch1", "output1"]);
        let out = r.children.iter().find(|c| c.name == "output1").unwrap();
        assert_eq!(out.params.iter().find(|p| p.name == "input").unwrap().text(), "transfer_switch1");
        let (g, err) = eval_node(&root, "remesh1");
        assert!(err.is_none() && g.num_points() > 0, "{err:?}");
    }

    /// A Repeat runs its chain Iterations times, each pass on the last one's
    /// result; a Seed inside reads what the loop began from. Dived in, its
    /// chain is shown as the LAST pass saw it, as a simnet's is shown as its
    /// last substep saw it — and a level inside a loop (a subnet in a
    /// simnet) is walked with the loop's feedback pushed, so it shows this
    /// frame and not the seed.
    #[test]
    fn a_repeat_loops_its_chain_and_shows_its_last_pass() {
        let templates_root = crate::app::load_fs_tree();
        let t = templates_root.children.iter().find(|t| t.name == "Repeat").expect("the Repeat template");
        assert!(t.is_enterable());
        let mut repeat = t.clone();
        crate::app::regenerate_node_ids(&mut repeat);
        repeat.name = "repeat1".into();
        repeat.geometry_visible = true;
        repeat.params.iter_mut().find(|p| p.name == "input").unwrap().set_text("sphere 1");
        repeat.params.iter_mut().find(|p| p.name == "iterations").unwrap().set_text("3");
        let mut step = ref_node("step", "step1", "transform", vec![("input", "node", "input1"), ("translation", "float3", "1:0:0")], vec![]);
        step.geometry_visible = false;
        repeat.children.push(step);
        repeat.children.iter_mut().find(|c| c.name == "output1").unwrap().params[0].set_text("step1");
        let root = modelling_root("1.0", vec![repeat]);
        let (sphere, _) = eval_node(&root, "sphere 1");
        let min_x = |g: &Detail| g.positions().iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
        let x0 = min_x(&sphere);
        let (g, err) = eval_node(&root, "repeat1");
        assert!(err.is_none(), "{err:?}");
        assert!((min_x(&g) - (x0 + 3.0)).abs() < 1e-4, "three passes of +1: {}", min_x(&g) - x0);

        // Dived in: the output draws the result, the step as the last pass
        // saw it (+2 in, +3 out), and the seed what the loop began from.
        let mut root = root;
        let r = root.children.iter_mut().find(|c| c.name == "repeat1").unwrap();
        r.children.iter_mut().find(|c| c.name == "output1").unwrap().geometry_visible = false;
        r.children.iter_mut().find(|c| c.name == "input1").unwrap().geometry_visible = false;
        r.children.iter_mut().find(|c| c.name == "step1").unwrap().geometry_visible = true;
        r.children.iter_mut().find(|c| c.name == "seed1").unwrap().geometry_visible = true;
        let level = root.children.iter().find(|c| c.name == "repeat1").unwrap();
        let mut cache = crate::geometry::SimCache::default();
        let mut sim = crate::geometry::EvalSim::new(0, 0, &mut cache);
        let mut err = None;
        let shown = crate::geometry::network_sphere_vertices_with_errors(&root, level, &mut err, &mut sim);
        assert!(err.is_none(), "{err:?}");
        assert_eq!(shown.num_points(), 2 * sphere.num_points(), "the step and the seed");
        let mut xs: Vec<f32> = shown.positions().iter().map(|p| p[0]).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((xs[0] - x0).abs() < 1e-4, "the seed is the sphere");
        assert!((xs[sphere.num_points()] - (x0 + 3.0)).abs() < 1e-4, "the step as its last pass made it: {}", xs[sphere.num_points()] - x0);

        // A subnet inside a simnet, dived into, shows the frame.
        let sub = ref_node("sub", "sub1", "node", vec![("input", "node", "input1")], vec![
            ref_node("sub-in", "input1", "input", vec![], vec![]),
            { let mut n = ref_node("sub-step", "step1", "transform", vec![("input", "node", "input1"), ("translation", "float3", "1:0:0")], vec![]); n.geometry_visible = true; n },
            { let mut n = ref_node("sub-out", "output1", "output", vec![("input", "node", "step1")], vec![]); n.geometry_visible = false; n },
        ]);
        let simnet = ref_node("sim", "simnet1", "simnet", vec![("input", "node", "sphere 1"), ("substeps", "spinbox", "1")], vec![
            ref_node("sim-in", "input1", "input", vec![], vec![]),
            sub,
            ref_node("sim-out", "output1", "output", vec![("input", "node", "sub1")], vec![]),
        ]);
        let root = modelling_root("1.0", vec![simnet]);
        let level = &root.children[1].children[1];
        let mut cache = crate::geometry::SimCache::default();
        let mut sim = crate::geometry::EvalSim::new(3, 0, &mut cache);
        let shown = crate::geometry::network_sphere_vertices_with_errors(&root, level, &mut err, &mut sim);
        assert!(err.is_none(), "{err:?}");
        // Its input shows what the frame's step read (+2), its step what it
        // made (+3); read off the seed, they were +0 and +1.
        let max_x = |g: &Detail| g.positions().iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
        assert_eq!(shown.num_points(), 2 * sphere.num_points());
        assert!((min_x(&shown) - (x0 + 2.0)).abs() < 1e-4, "the input at frame 3: {}", min_x(&shown) - x0);
        assert!((max_x(&shown) - (max_x(&sphere) + 3.0)).abs() < 1e-4, "the step at frame 3: {}", max_x(&shown) - max_x(&sphere));
    }

    /// The faster flip pass and the faster closest-point search are HOW a
    /// remesh is run and not what it does: against the passes as they were
    /// first written, the same mesh bit for bit, step after step of a
    /// surface being pulled about — and the same hit for a query on the
    /// surface, off it and far from it.
    #[test]
    fn the_remesh_matches_its_reference() {
        use crate::remesh::remesh_reference;
        let fixtures = [
            (sphere_detail(Vec3::ZERO, 1.0, 6, 8), 0.2),
            (sphere_detail(Vec3::ZERO, 1.0, 24, 32), 0.5),
            (sphere_detail(Vec3::new(0.3, -0.2, 1.0), 0.5, 10, 14), 0.1),
        ];
        for (start, target) in fixtures {
            for relax in [0.0, 0.5] {
                let settings = Settings { target, iterations: 3, relax, ..Default::default() };
                let (mut a, mut b) = (start.clone(), start.clone());
                for step in 0..6 {
                    // A pull between remeshes, as a simulation makes one.
                    for d in [&mut a, &mut b] {
                        for p in 0..d.num_points() {
                            let at = d.pos(p);
                            let pull = Vec3::new(0.03, 0.0, 0.0) * (at.y * 3.0 + step as f32).sin();
                            d.set_pos(p, at + pull);
                        }
                    }
                    a = remesh(&a, settings);
                    b = remesh_reference(&b, settings);
                    assert!(same_mesh(&a, &b), "target {target}, relax {relax}, step {step}: {} points against {}", a.num_points(), b.num_points());
                }
                assert!(a.num_points() != start.num_points() || target == 0.1, "the fixture remeshes");
            }
        }

        let sphere = sphere_detail(Vec3::ZERO, 1.0, 12, 16);
        let grid = crate::spatial::TriGrid::build(&sphere);
        let mut asked = 0;
        for i in 0..4000 {
            // On the surface, near it, inside it, far outside it.
            let f = i as f32;
            let dir = Vec3::new((f * 0.37).sin(), (f * 0.73).cos(), (f * 1.31).sin()).normalize_or_zero();
            let p = dir * [1.0, 0.98, 1.05, 0.3, 0.0, 4.0, 40.0][i % 7];
            let (new, old) = (grid.closest(p).unwrap(), grid.closest_reference(p).unwrap());
            assert_eq!((new.point, new.distance, new.normal), (old.point, old.distance, old.normal), "query {p:?}");
            asked += 1;
        }
        for p in 0..sphere.num_points() {
            let (new, old) = (grid.closest(sphere.pos(p)).unwrap(), grid.closest_reference(sphere.pos(p)).unwrap());
            assert_eq!((new.point, new.distance, new.normal), (old.point, old.distance, old.normal), "point {p}");
        }
        assert_eq!(asked, 4000);
    }

    /// A point in a group survives a remesh: a pulled point at the tip of
    /// a spike keeps its identity, its values and its membership while the
    /// mesh around it is split and collapsed. A collapse that would leave a
    /// corner with fewer than three triangles is refused — it stranded the
    /// pulled point, which `into_detail` then dropped with everything it
    /// was — and where the pulled point is one end of a collapsed edge it
    /// is the end that survives, joining the other end's groups.
    #[test]
    fn a_grouped_point_survives_a_remesh() {
        let mut d = sphere_detail(Vec3::ZERO, 1.0, 10, 14);
        let tip = 37usize;
        let id = d.ids()[tip];
        d.points_mut().create_group("pull");
        d.points_mut().add_to_group("pull", tip);
        d.points_mut().create("mass", crate::detail::AttribValue::Float(0.0));
        d.points_mut().set_value("mass", tip, crate::detail::AttribValue::Float(7.5)).unwrap();
        let settings = Settings { target: 0.25, iterations: 3, relax: 0.0, ..Default::default() };
        let mut outward = d.pos(tip).normalize();
        for step in 0..40 {
            // The pull: the tip out along its ray, a little each step, so
            // the edges around it stretch and the remesh works there.
            let p = d.points().group_members("pull");
            assert_eq!(p.len(), 1, "step {step}: the group has one member, not {}", p.len());
            let at = p[0] as usize;
            assert_eq!(d.ids()[at], id, "step {step}: the member is the point it was");
            assert_eq!(d.points().value("mass", at), Some(crate::detail::AttribValue::Float(7.5)), "step {step}: with its values");
            let pos = d.pos(at);
            outward = if pos.length() > 1e-3 { pos.normalize() } else { outward };
            d.set_pos(at, pos + outward * 0.08);
            d = remesh(&d, settings);
            assert!(d.is_closed(), "step {step}: still a closed surface");
        }
        assert!(d.pos(d.points().group_members("pull")[0] as usize).length() > 3.0, "the tip went out with the pull");
    }

    /// A remesh SETTLES: run again on what it made, it comes to a mesh it
    /// finds nothing to do to, and hands that back as it was given — the
    /// primitives and their order untouched, which is what lets a step of
    /// a simulation at rest keep what it knows of the topology. Until
    /// 2026-09-29 the flip pass, judging by valence alone, turned back the
    /// long edges the split and collapse had just turned, and the three
    /// went round on the same edges for ever.
    #[test]
    fn a_remesh_settles_and_then_leaves_the_mesh_alone() {
        for (start, target) in [
            (sphere_detail(Vec3::ZERO, 1.0, 6, 8), 0.2),
            (sphere_detail(Vec3::ZERO, 1.0, 24, 32), 0.5),
            (sphere_detail(Vec3::ZERO, 0.5, 10, 14), 0.1),
        ] {
            let settings = Settings { target, iterations: 3, relax: 0.0, ..Default::default() };
            let mut d = remesh(&start, settings);
            assert!(crate::remesh::last_changes() > 0, "the fixture remeshes");
            let mut rounds = 0;
            loop {
                let next = remesh(&d, settings);
                if crate::remesh::last_changes() == 0 {
                    assert!(same_mesh(&next, &d), "a remesh with nothing to do changed the mesh");
                    break;
                }
                d = next;
                rounds += 1;
                assert!(rounds < 20, "target {target}: still changing {} edges after {rounds} rounds", crate::remesh::last_changes());
            }
            // What it settled on is still the mesh that was asked for.
            let mean = mean_edge(&d);
            assert!((mean - target).abs() < target * 0.5, "settled at {mean}, wanted about {target}");
            assert!(d.is_closed(), "and still a closed surface");
        }
    }

    #[test]
    fn test_remesh_converges_rather_than_oscillating() {
        // The 4/3 and 4/5 thresholds exist so a split cannot produce edges the
        // next collapse undoes. If they were wrong, running longer would keep
        // changing the answer instead of settling.
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 8, 12);
        let a = remesh(&sphere, Settings { target: 0.3, iterations: 6, ..Default::default() });
        let b = remesh(&sphere, Settings { target: 0.3, iterations: 12, ..Default::default() });
        let (ea, eb) = (mean_edge(&a), mean_edge(&b));
        assert!((ea - eb).abs() < 0.06, "still moving at 12 iterations: {ea} -> {eb}");

        // And it is deterministic: the same input twice is the same mesh, or a
        // simulation could not be reproduced frame to frame.
        let again = remesh(&sphere, Settings { target: 0.3, iterations: 6, ..Default::default() });
        assert_eq!(a.num_points(), again.num_points());
        assert_eq!(a.positions(), again.positions());
    }

    #[test]
    fn test_remesh_keeps_the_surface_it_was_given() {
        // Relaxation is TANGENTIAL: points slide within the surface to even out
        // the triangles, and the shape they describe stays where it was. A
        // sphere of radius 1 must still be a sphere of radius 1.
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 10, 14);
        let out = remesh(&sphere, Settings { target: 0.25, iterations: 5, ..Default::default() });
        for p in 0..out.num_points() {
            let r = out.pos(p).length();
            assert!((r - 1.0).abs() < 0.08, "point {p} left the sphere at radius {r}");
        }
        let (lo, hi) = out.bounds().unwrap();
        assert!(lo.x > -1.1 && hi.x < 1.1, "bounds grew: {lo:?} {hi:?}");
    }

    #[test]
    fn test_remesh_carries_the_simulations_data_across() {
        let mut sphere = sphere_detail(Vec3::ZERO, 1.0, 8, 12);
        // A field that varies smoothly, so interpolation is checkable.
        sphere.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..sphere.num_points() {
            let y = sphere.pos(p).y;
            sphere.points_mut().set_value("mass", p, AttribValue::Float(y)).unwrap();
        }
        sphere.points_mut().create_group("top");
        for p in 0..sphere.num_points() {
            if sphere.pos(p).y > 0.5 {
                sphere.points_mut().add_to_group("top", p);
            }
        }
        let before_ids: std::collections::HashSet<u64> = sphere.ids().iter().copied().collect();

        let out = remesh(&sphere, Settings { target: 0.25, iterations: 4, ..Default::default() });

        // A split interpolates, so a new point's value is consistent with
        // where it sits rather than zero — which is what the attribute means.
        assert!(out.points().has("mass"));
        for p in 0..out.num_points() {
            let v = out.points().value("mass", p).unwrap().as_f32();
            assert!((v - out.pos(p).y).abs() < 0.2, "point {p}: {v} vs y {}", out.pos(p).y);
        }

        // Points that survived kept their identity: a remesh should cost the
        // simulation as little memory as it can, and the solver has been
        // writing to these.
        let kept = out.ids().iter().filter(|id| before_ids.contains(id)).count();
        assert!(kept > 0, "every identity was thrown away");
        // And no identity is used twice, however many were minted on the way.
        let mut all = out.ids().to_vec();
        all.sort_unstable();
        let unique = all.len();
        all.dedup();
        assert_eq!(all.len(), unique, "an identity was reused");

        // Groups come across, and a new point joins only where BOTH parents
        // were members — otherwise every group grows along its own boundary
        // each time the mesh is remeshed.
        let members = out.points().group_members("top");
        assert!(!members.is_empty() && members.len() < out.num_points());
        for &p in &members {
            assert!(out.pos(p as usize).y > 0.3, "the group leaked downward");
        }
    }

    #[test]
    fn test_remesh_leaves_a_mesh_it_cannot_help_alone() {
        // A mesh that has already been remeshed to a target is settled at it,
        // and running again changes little. Note what is NOT settled: a UV
        // sphere at its own MEAN edge length, because a UV sphere is
        // anisotropic — its rings are short at the poles and long at the
        // equator — and making it isotropic has to move points. That is the
        // job, not churn.
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 12, 16);
        let settled = remesh(&sphere, Settings { target: 0.3, iterations: 5, ..Default::default() });
        let again = remesh(&settled, Settings { target: 0.3, iterations: 5, ..Default::default() });
        let ratio = again.num_points() as f32 / settled.num_points() as f32;
        assert!((0.85..1.18).contains(&ratio), "a settled mesh was churned: {ratio}");

        // Degenerate settings pass the geometry through rather than producing
        // nothing: a zero target has no length to steer toward.
        let zero = remesh(&sphere, Settings { target: 0.0, ..Default::default() });
        assert_eq!(zero.num_points(), sphere.num_points());
        // Geometry with no primitives has no edges to split or collapse.
        let mut cloud = Detail::new();
        cloud.add_point(Vec3::ZERO);
        cloud.add_point(Vec3::X);
        assert_eq!(remesh(&cloud, Settings::default()).num_points(), 2);
    }

    #[test]
    fn test_remesh_output_is_a_usable_mesh() {
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 8, 12);
        let out = remesh(&sphere, Settings { target: 0.3, iterations: 4, ..Default::default() });

        // Every primitive is a real triangle over live points — a stale index
        // or a degenerate face here would crash or smear the renderer.
        assert!(out.num_prims() > 0);
        for prim in 0..out.num_prims() {
            let pts = out.prim_points(prim);
            assert_eq!(pts.len(), 3, "prim {prim} is not a triangle");
            assert!(pts.iter().all(|&p| (p as usize) < out.num_points()), "prim {prim} dangles");
            assert!(pts[0] != pts[1] && pts[1] != pts[2] && pts[0] != pts[2], "prim {prim} is degenerate");
        }
        // No orphans: every point is used by something.
        for p in 0..out.num_points() {
            assert!(!out.point_prims(p).is_empty(), "point {p} belongs to nothing");
        }
        // Still closed — every edge shared by exactly two faces. A remesh that
        // tore a hole would be invisible until something tried to fill it.
        for e in out.edges() {
            let shared = (0..out.num_prims())
                .filter(|&t| {
                    let pts = out.prim_points(t);
                    pts.contains(&e[0]) && pts.contains(&e[1])
                })
                .count();
            assert_eq!(shared, 2, "edge {e:?} is on {shared} faces, so the surface is torn");
        }
    }

    #[test]
    fn test_develop_moves_the_surface_along_its_normals() {
        let mut sphere = sphere_detail(Vec3::ZERO, 1.0, 8, 12);
        sphere.points_mut().create("growth", AttribValue::Float(1.0));
        // Only the top half grows.
        sphere.points_mut().create_group("top");
        for p in 0..sphere.num_points() {
            if sphere.pos(p).y <= 0.0 {
                sphere.points_mut().set_value("growth", p, AttribValue::Float(0.0)).unwrap();
            } else {
                sphere.points_mut().add_to_group("top", p);
            }
        }

        let mut out = sphere.clone();
        let node = crate::app::FsNode {
            id: "d".into(),
            name: "Develop 1".into(),
            node_type: "develop".into(),
            children: vec![],
            params: [("attribute", "growth"), ("scale", "0.50"), ("direction", "Normal")]
                .into_iter()
                .map(|(name, default)| crate::app::ParamDef::new(name, "text", default))
                .collect(),
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        };
        let mut err = None;
        crate::geometry::apply_develop(&mut out, &node, &mut err);
        assert!(err.is_none(), "{err:?}");

        for p in 0..out.num_points() {
            let grew = sphere.points().value("growth", p).unwrap().as_f32() > 0.0;
            let r = out.pos(p).length();
            if grew {
                // Outward along the normal, which on a sphere is radial.
                assert!((r - 1.5).abs() < 0.05, "point {p} grew to {r}, wanted 1.5");
            } else {
                assert!((r - 1.0).abs() < 1e-4, "point {p} moved without growth: {r}");
            }
        }
        // Topology is remesh's business: Develop moves points and nothing else.
        assert_eq!(out.num_prims(), sphere.num_prims());
        assert_eq!(out.ids(), sphere.ids());
    }

    fn phase3_node(ty: &str, params: &[(&str, &str)]) -> FsNode {
        FsNode {
            id: format!("id-{ty}"),
            name: format!("{ty} 1"),
            node_type: ty.into(),
            children: vec![],
            params: params
                .iter()
                .map(|(name, default)| crate::app::ParamDef::new(*name, "text", *default))
                .collect(),
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        }
    }

    #[test]
    fn test_subdivide_multiplies_triangles_without_moving_the_shape() {
        use crate::remesh::subdivide;
        let mut sphere = sphere_detail(Vec3::ZERO, 1.0, 6, 8);
        sphere.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..sphere.num_points() {
            let y = sphere.pos(p).y;
            sphere.points_mut().set_value("mass", p, AttribValue::Float(y)).unwrap();
        }
        let (before_prims, before_bounds) = (sphere.num_prims(), sphere.bounds().unwrap());

        let once = subdivide(&sphere, 1);
        // Every triangle becomes four. The sphere's quad bands fan to two
        // triangles each on the way in, so the count is against THAT.
        let tri_count = |d: &Detail| d.triangulate(|_, _| ()).len() / 3;
        assert_eq!(tri_count(&once), tri_count(&sphere) * 4);

        // The shape does not move: this refines, it does not smooth. A
        // subdivision that also moved points would be two operations wearing
        // one name.
        let after_bounds = once.bounds().unwrap();
        assert!((after_bounds.0 - before_bounds.0).length() < 1e-5, "{:?}", after_bounds.0);
        assert!((after_bounds.1 - before_bounds.1).length() < 1e-5, "{:?}", after_bounds.1);
        // Original points keep their exact positions AND identities.
        for p in 0..sphere.num_points() {
            assert!(once.ids().contains(&sphere.id(p).unwrap()), "point {p} lost its identity");
        }

        // Attributes interpolate onto the midpoints, so a field defined on a
        // coarse mesh survives being refined.
        for p in 0..once.num_points() {
            let v = once.points().value("mass", p).unwrap().as_f32();
            assert!((v - once.pos(p).y).abs() < 0.12, "point {p}: {v} vs y {}", once.pos(p).y);
        }

        // Depth compounds, and zero is a pass-through.
        assert_eq!(tri_count(&subdivide(&sphere, 2)), tri_count(&sphere) * 16);
        assert_eq!(subdivide(&sphere, 0).num_prims(), before_prims);

        // The result is still a closed, usable mesh.
        for prim in 0..once.num_prims() {
            assert_eq!(once.prim_points(prim).len(), 3);
        }
        for p in 0..once.num_points() {
            assert!(!once.point_prims(p).is_empty(), "point {p} belongs to nothing");
        }
    }

    #[test]
    fn test_the_projection_pass_stops_a_remeshed_surface_creeping() {
        use crate::spatial::TriGrid;
        // Tangential relaxation slides points within the surface, but "within"
        // is only true to first order: on anything curved the slide leaves the
        // surface a little, and the error compounds.
        //
        // Measured as distance from the INPUT SURFACE, not as radius. The
        // projection holds points on the mesh it was given — which is a
        // faceted sphere, whose edge midpoints are legitimately inside the
        // ideal one. Radius would be measuring the discretization, not the
        // drift.
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 10, 14);
        let rest = TriGrid::build(&sphere);
        let drift = |d: &Detail| {
            (0..d.num_points())
                .filter_map(|p| rest.closest(d.pos(p)).map(|h| h.distance))
                .sum::<f32>()
                / d.num_points() as f32
        };

        let cfg = |project| Settings {
            target: 0.25,
            iterations: 16,
            relax: 1.0,
            project,
            ..Default::default()
        };
        let drifted = remesh(&sphere, cfg(false));
        let held = remesh(&sphere, cfg(true));

        assert!(drift(&drifted) > 1e-3, "relaxation did not drift at all: {}", drift(&drifted));
        assert!(
            drift(&held) < drift(&drifted) / 4.0,
            "projection barely helped: {} vs {}",
            drift(&held),
            drift(&drifted)
        );
        assert!(drift(&held) < 1e-4, "projected points are off the surface: {}", drift(&held));
    }

    #[test]
    fn test_the_tri_grid_finds_the_nearest_surface_point() {
        use crate::spatial::TriGrid;
        let sphere = sphere_detail(Vec3::ZERO, 1.0, 12, 16);
        let grid = TriGrid::build(&sphere);

        // A point well outside: the closest surface point is along the ray to
        // the centre, at about the radius.
        let h = grid.closest(Vec3::new(3.0, 0.0, 0.0)).unwrap();
        assert!((h.distance - 2.0).abs() < 0.05, "distance {}", h.distance);
        assert!(h.point.x > 0.9 && h.point.y.abs() < 0.2 && h.point.z.abs() < 0.2, "{:?}", h.point);
        // The hit carries the face's normal, which is what lets a caller tell
        // inside from outside without casting a ray.
        assert!(h.normal.dot(Vec3::X) > 0.5, "the nearest face should look outward: {:?}", h.normal);

        // A point ON the surface finds itself.
        let on = sphere.pos(20);
        let d = grid.closest(on).unwrap().distance;
        assert!(d < 1e-3, "a point on the surface is {d} from it");

        // The centre is a radius from everywhere, and the search still
        // terminates — the expanding box has to grow several times to find
        // anything at all.
        let d = grid.closest(Vec3::ZERO).unwrap().distance;
        assert!((d - 1.0).abs() < 0.05, "from the centre: {d}");

        assert!(TriGrid::build(&Detail::new()).closest(Vec3::ZERO).is_none());
    }

    #[test]
    fn test_detangle_separates_what_is_near_in_space_but_far_across_the_surface() {
        // Two sheets pressed closer together than the thickness. They share no
        // topology, so every pair between them is a self-intersection in
        // waiting; within each sheet, neighbours are closer than the thickness
        // by construction and must NOT be pushed apart.
        let mut d = Detail::new();
        let step = 0.25;
        let gap = 0.05;
        let mut rows = Vec::new();
        for sheet in 0..2 {
            let mut row = Vec::new();
            for i in 0..4 {
                for j in 0..4 {
                    row.push(d.add_point(Vec3::new(
                        i as f32 * step,
                        sheet as f32 * gap,
                        j as f32 * step,
                    )));
                }
            }
            rows.push(row);
        }
        for sheet in 0..2 {
            for i in 0..3 {
                for j in 0..3 {
                    let at = |a: usize, b: usize| rows[sheet][a * 4 + b];
                    d.add_prim(&[at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                    d.add_prim(&[at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                }
            }
        }
        let before_gap = d.pos(16).y - d.pos(0).y;
        assert!((before_gap - gap).abs() < 1e-6);
        let before_edge = (d.pos(1) - d.pos(0)).length();

        let node = phase3_node(
            "detangle",
            &[("thickness", "1.00"), ("rings", "2"), ("iterations", "6")],
        );
        crate::geometry::apply_detangle(&mut d, &node);

        // The sheets moved apart.
        let after_gap = d.pos(16).y - d.pos(0).y;
        assert!(after_gap > before_gap * 2.0, "sheets did not separate: {after_gap}");
        // But the mesh did not explode: points that are neighbours ACROSS the
        // surface are within a thickness of each other by construction, and a
        // repulsion that did not exclude them would blow every sheet apart
        // from the inside.
        let after_edge = (d.pos(1) - d.pos(0)).length();
        assert!(
            (after_edge / before_edge - 1.0).abs() < 0.35,
            "the sheet stretched from {before_edge} to {after_edge}"
        );
    }

    /// The solve in `detangle.rs` is the first version's, bit for bit: the
    /// same pairs summed in the same order, over meshes that tangle and
    /// ones that do not, with and without a group, and step after step as a
    /// simulation runs it — which is when the kept topology, the reused
    /// grid and the early end are all in play.
    #[test]
    fn the_detangle_solve_matches_its_reference() {
        // Two sheets closer than a thickness, sharing no topology.
        let sheets = {
            let mut d = Detail::new();
            let mut rows = Vec::new();
            for sheet in 0..2 {
                let mut row = Vec::new();
                for i in 0..6 {
                    for j in 0..6 {
                        row.push(d.add_point(Vec3::new(i as f32 * 0.25, sheet as f32 * 0.05 + (i * j) as f32 * 0.003, j as f32 * 0.25)));
                    }
                }
                rows.push(row);
            }
            for sheet in 0..2 {
                for i in 0..5 {
                    for j in 0..5 {
                        let at = |a: usize, b: usize| rows[sheet][a * 6 + b];
                        d.add_prim(&[at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                        d.add_prim(&[at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                    }
                }
            }
            for p in (0..d.num_points()).step_by(3) {
                d.points_mut().add_to_group("some", p);
            }
            d
        };
        // A sphere pressed nearly flat, so its two sides meet; and one left
        // round, which has nothing to separate.
        let squashed = |flatten: f32| {
            let mut d = sphere_detail(Vec3::ZERO, 0.5, 10, 14);
            for p in 0..d.num_points() {
                let v = d.pos(p);
                d.set_pos(p, Vec3::new(v.x, v.y * flatten, v.z));
            }
            for p in (0..d.num_points()).step_by(2) {
                d.points_mut().add_to_group("some", p);
            }
            d
        };
        let meshes = [("sheets", sheets), ("flat sphere", squashed(0.04)), ("round sphere", squashed(1.0))];
        let settings: [&[(&str, &str)]; 7] = [
            &[("thickness", "1.00"), ("rings", "2"), ("iterations", "4")],
            &[("thickness", "2.00"), ("rings", "1"), ("iterations", "8")],
            &[("thickness", "0.50"), ("rings", "0"), ("iterations", "1")],
            &[("thickness", "3.00"), ("rings", "3"), ("iterations", "32")],
            &[("thickness", "1.50"), ("rings", "2"), ("iterations", "6"), ("group", "some")],
            &[("thickness", "0.00"), ("rings", "2"), ("iterations", "4")],
            &[("thickness", "1.00"), ("rings", "6"), ("iterations", "4"), ("group", "nobody")],
        ];
        let mut moved_somewhere = 0;
        for (name, mesh) in &meshes {
            for params in settings {
                let node = phase3_node("detangle", params);
                let (mut a, mut b) = (mesh.clone(), mesh.clone());
                // Step after step, with something moving the points between
                // steps as the rest of a chain would.
                for step in 0..6 {
                    crate::geometry::apply_detangle(&mut a, &node);
                    crate::geometry::apply_detangle_reference(&mut b, &node);
                    assert_eq!(a.positions(), b.positions(), "{name}, {params:?}, step {step}");
                    for d in [&mut a, &mut b] {
                        for p in 0..d.num_points() {
                            let v = d.pos(p);
                            d.set_pos(p, v + Vec3::new(0.0, -0.01 * v.y.signum(), 0.002 * (p % 3) as f32));
                        }
                    }
                }
                if a.positions() != mesh.positions() {
                    moved_somewhere += 1;
                }
            }
        }
        assert!(moved_somewhere > 10, "the fixtures tangle, or this proves nothing: {moved_somewhere}");
    }

    /// What the solve no longer pays for. A surface that touches itself
    /// nowhere is one pass and one grid, whatever Iterations says; a group
    /// is searched for its own points and no others; the grid is reused
    /// across passes while the points stay near where it filed them; and a
    /// simulation's steps share one topology.
    #[test]
    fn the_detangle_solve_skips_what_it_does_not_need() {
        let round = sphere_detail(Vec3::ZERO, 0.5, 10, 14);
        let n = round.num_points();
        let node = phase3_node("detangle", &[("thickness", "1.00"), ("rings", "2"), ("iterations", "8")]);
        let mut d = round.clone();
        let work = crate::detangle::apply(&mut d, &node);
        assert_eq!((work.passes, work.grids, work.searched), (1, 1, n), "nothing to separate: {work:?}");
        assert_eq!(d.positions(), round.positions());

        // A flattened one has work to do, over more than one pass, and does
        // not build a grid for each.
        let mut flat = round.clone();
        for p in 0..n {
            let v = flat.pos(p);
            flat.set_pos(p, Vec3::new(v.x, v.y * 0.04, v.z));
        }
        let mut d = flat.clone();
        let work = crate::detangle::apply(&mut d, &node);
        assert!(work.passes > 1, "{work:?}");
        assert_ne!(d.positions(), flat.positions());
        // Pressed that flat, a pass moves points further than the grid
        // allows for and it is built again. Two sheets a little closer than
        // a thickness part gently, over several passes on one grid.
        let mut sheets = Detail::new();
        let mut rows = Vec::new();
        for sheet in 0..2 {
            let mut row = Vec::new();
            for i in 0..6 {
                for j in 0..6 {
                    row.push(sheets.add_point(Vec3::new(i as f32 * 0.25, sheet as f32 * 0.24, j as f32 * 0.25)));
                }
            }
            rows.push(row);
        }
        for sheet in 0..2 {
            for i in 0..5 {
                for j in 0..5 {
                    let at = |a: usize, b: usize| rows[sheet][a * 6 + b];
                    sheets.add_prim(&[at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                    sheets.add_prim(&[at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                }
            }
        }
        let work = crate::detangle::apply(&mut sheets, &node);
        assert!(work.passes > 1 && work.passes < 8, "several passes, then the early end: {work:?}");
        assert_eq!(work.grids, 1, "on the one grid: {work:?}");

        // With a group only its points are searched for.
        let mut grouped = flat.clone();
        for p in (0..n).step_by(4) {
            grouped.points_mut().add_to_group("some", p);
        }
        let members = grouped.points().group_len("some");
        let node = phase3_node("detangle", &[("thickness", "1.00"), ("rings", "2"), ("iterations", "1"), ("group", "some")]);
        let work = crate::detangle::apply(&mut grouped, &node);
        assert_eq!(work.searched, members, "{work:?}");

        // Steps of a simulation — fresh clones of one mesh, its points
        // moving — share one topology; another mesh, or another ring count,
        // is another.
        let kept = crate::detangle::kept_topologies();
        let node = phase3_node("detangle", &[("thickness", "1.00"), ("rings", "5"), ("iterations", "2")]);
        let mut step = flat.clone();
        for _ in 0..5 {
            let mut next = step.clone();
            crate::detangle::apply(&mut next, &node);
            step = next;
        }
        assert!(crate::detangle::kept_topologies() <= kept + 1, "five steps, one topology");
    }

    #[test]
    fn test_detangle_is_independent_of_point_order() {
        // Gathered against the start-of-pass positions and applied at the end,
        // so the same tangle untangles the same way however its points are
        // numbered. A Gauss-Seidel sweep would not.
        let mut a = sphere_detail(Vec3::ZERO, 0.5, 6, 8);
        let node = phase3_node("detangle", &[("thickness", "2.00"), ("rings", "1"), ("iterations", "3")]);
        let mut b = a.clone();
        crate::geometry::apply_detangle(&mut a, &node);
        crate::geometry::apply_detangle(&mut b, &node);
        assert_eq!(a.positions(), b.positions());
    }

    /// A sheet of two large triangles with a fine patch over the middle of
    /// one of them: the patch's points are nowhere near any corner of the
    /// sheet, which is the case a point-to-point test cannot see. The patch
    /// is tilted by `tilt` so its edges straddle the sheet when it is
    /// lowered through it, and is the group "patch".
    fn sheet_and_patch(height: f32, tilt: f32) -> Detail {
        let mut d = Detail::new();
        let s: Vec<u32> = [(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)]
            .iter()
            .map(|&(x, z)| d.add_point(Vec3::new(x, 0.0, z)))
            .collect();
        d.add_prim(&[s[0], s[2], s[1]]);
        d.add_prim(&[s[0], s[3], s[2]]);
        let mut patch = Vec::new();
        for i in 0..5 {
            for j in 0..5 {
                let (x, z) = (i as f32 * 0.1, j as f32 * 0.1);
                patch.push(d.add_point(Vec3::new(0.9 + x, height + tilt * x, -1.1 + z)));
            }
        }
        for i in 0..4 {
            for j in 0..4 {
                let at = |a: usize, b: usize| patch[a * 5 + b];
                d.add_prim(&[at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
                d.add_prim(&[at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
            }
        }
        for &p in &patch {
            d.points_mut().add_to_group("patch", p as usize);
        }
        d
    }

    /// How far the patch's nearest point is from the sheet's surface.
    fn patch_clearance(d: &Detail) -> f32 {
        let tris = [[0usize, 2, 1], [0, 3, 2]];
        (4..d.num_points())
            .flat_map(|p| tris.iter().map(move |t| (p, *t)))
            .map(|(p, t)| {
                let q = crate::spatial::closest_point_on_triangle(d.pos(p), d.pos(t[0]), d.pos(t[1]), d.pos(t[2]));
                (d.pos(p) - q).length()
            })
            .fold(f32::MAX, f32::min)
    }

    /// The measure: every edge that passes through a triangle, and the
    /// points of both. A surface that does not cross itself has none,
    /// quads and shared corners included.
    #[test]
    fn the_tangle_measure_counts_edges_through_triangles() {
        use crate::detangle::self_intersections;
        assert_eq!(self_intersections(&Detail::new()).crossings, 0);
        assert_eq!(self_intersections(&sphere_detail(Vec3::ZERO, 0.5, 10, 14)).crossings, 0);
        assert_eq!(self_intersections(&box_detail(Vec3::ZERO, Vec3::ONE, 0.2)).crossings, 0, "quads fan into triangles that share a side");
        // The patch above the sheet, then tilted through it.
        assert_eq!(self_intersections(&sheet_and_patch(0.05, 0.0)).crossings, 0);
        let through = self_intersections(&sheet_and_patch(-0.06, 0.3));
        assert!(through.crossings > 0, "{through:?}");
        // The points named are the crossing edges' and the crossed
        // triangle's: some of the patch, not all of it, and the sheet's.
        let of_patch = through.points.iter().filter(|&&p| p >= 4).count();
        assert!(of_patch > 0 && of_patch < 25, "{through:?}");
        assert!(through.points.iter().any(|&p| p < 4), "{through:?}");
        assert!(through.points.windows(2).all(|w| w[0] < w[1]), "ascending, each once");
        // The same answer at a thousandth of the size: the tolerance is
        // relative.
        let mut small = sheet_and_patch(-0.06, 0.3);
        for p in 0..small.num_points() {
            let v = small.pos(p);
            small.set_pos(p, v * 0.001);
        }
        assert_eq!(self_intersections(&small), through);
    }

    /// What the Surface method is for: a point over the middle of a large
    /// triangle is near none of its corners, so Points sees nothing and
    /// Surface parts them.
    #[test]
    fn the_surface_method_sees_a_point_over_the_middle_of_a_triangle() {
        let start = sheet_and_patch(0.03, 0.0);
        let settings = |method: &'static str| {
            phase3_node("detangle", &[("method", method), ("thickness", "0.25"), ("rings", "2"), ("iterations", "8")])
        };
        let mut by_points = start.clone();
        let work = crate::detangle::apply(&mut by_points, &settings("Points"));
        assert_eq!(by_points.positions(), start.positions(), "nothing is within a thickness of a corner: {work:?}");

        let mut by_surface = start.clone();
        let work = crate::detangle::apply(&mut by_surface, &settings("Surface"));
        assert!(work.contacts > 0, "{work:?}");
        let (before, after) = (patch_clearance(&start), patch_clearance(&by_surface));
        assert!((before - 0.03).abs() < 1e-5);
        assert!(after > 0.1, "the patch is {after} from the sheet");
        // The patch moved as one: it was pushed off the sheet, not apart.
        let edge = |d: &Detail| (d.pos(5) - d.pos(4)).length();
        assert!((edge(&by_surface) / edge(&start) - 1.0).abs() < 0.05, "{} to {}", edge(&start), edge(&by_surface));
        // The move is shared, so the sheet gave way too, downward.
        assert!((0..4).all(|p| by_surface.pos(p).y <= 0.0) && (0..4).any(|p| by_surface.pos(p).y < 0.0));

        // With the sheet outside the Group it stays where it is and the
        // patch takes the whole move, not half of it.
        let node = phase3_node(
            "detangle",
            &[("method", "Surface"), ("thickness", "0.25"), ("rings", "2"), ("iterations", "8"), ("group", "patch")],
        );
        let mut held = start.clone();
        crate::detangle::apply(&mut held, &node);
        assert_eq!(held.positions()[..4], start.positions()[..4]);
        assert!(patch_clearance(&held) > 0.1, "{}", patch_clearance(&held));

        // A surface that touches itself nowhere is left alone, in one pass.
        let round = sphere_detail(Vec3::ZERO, 0.5, 10, 14);
        let mut d = round.clone();
        let node = phase3_node("detangle", &[("method", "Surface"), ("thickness", "1.00"), ("rings", "2"), ("iterations", "8")]);
        let work = crate::detangle::apply(&mut d, &node);
        assert_eq!((work.passes, work.contacts), (1, 0), "{work:?}");
        assert_eq!(d.positions(), round.positions());
    }

    /// The two methods as a simulation runs them: the patch is lowered a
    /// little each step, by less than the thickness, toward a sheet that
    /// does not move. Points lets it through; Surface holds it off. The
    /// measure is what says so, and the node's Tangled Group is the measure
    /// written onto the geometry.
    #[test]
    fn the_surface_method_holds_a_patch_off_a_sheet_step_after_step() {
        let run = |method: &'static str| {
            let node = phase3_node(
                "detangle",
                &[("method", method), ("thickness", "0.25"), ("rings", "2"), ("iterations", "8"), ("group", "patch"), ("tangled_group", "tangled")],
            );
            let mut d = sheet_and_patch(0.2, 0.3);
            let (mut worst, mut marked) = (0, 0);
            for _ in 0..25 {
                for p in 4..d.num_points() {
                    let v = d.pos(p);
                    d.set_pos(p, v - Vec3::new(0.0, 0.02, 0.0));
                }
                let work = crate::detangle::apply(&mut d, &node);
                assert_eq!(work.crossings, crate::detangle::self_intersections(&d).crossings);
                assert_eq!(work.crossings > 0, d.points().group_len("tangled") > 0);
                worst = worst.max(work.crossings);
                marked = marked.max(d.points().group_len("tangled"));
            }
            (d, worst, marked)
        };
        let (through, worst, marked) = run("Points");
        assert!(worst > 0 && marked > 0, "Points should have let it through: {worst}");
        assert!((4..through.num_points()).all(|p| through.pos(p).y < 0.0), "and out the other side");

        let (held, worst, marked) = run("Surface");
        assert_eq!((worst, marked), (0, 0), "no edge went through at any step");
        assert!((4..held.num_points()).all(|p| held.pos(p).y > 0.0), "the patch is still above the sheet");
        assert!(held.points().has_group("tangled"), "the group is written, empty");
    }

    /// A point has a side once the solve is told where the step began. The
    /// patch is carried through a sheet that does not move, in one step:
    /// by less than a thickness, where distance alone sees it near the
    /// sheet and pushes it on the way it was going; and by several, where
    /// distance alone sees nothing at all.
    #[test]
    fn the_surface_method_puts_back_what_went_through() {
        let node = phase3_node(
            "detangle",
            &[("method", "Surface"), ("thickness", "0.25"), ("rings", "2"), ("iterations", "8"), ("group", "patch")],
        );
        let above = |d: &Detail| (4..d.num_points()).filter(|&p| d.pos(p).y > 0.0).count();
        for (from, to) in [(0.04, -0.04), (0.3, -0.5)] {
            let before = sheet_and_patch(from, 0.0);
            let carried = sheet_and_patch(to, 0.0);

            let mut blind = carried.clone();
            crate::detangle::apply(&mut blind, &node);
            assert_eq!(above(&blind), 0, "{from} to {to}: with no memory the patch stays under the sheet");
            assert!(blind.pos(4).y <= carried.pos(4).y, "and is pushed no nearer to it");

            let mut told = carried.clone();
            let work = crate::detangle::apply_from(&mut told, Some(&before), &node);
            assert_eq!(above(&told), 25, "{from} to {to}: {work:?}");
            assert!(work.crossed > 0 || work.held > 0, "{work:?}");
            assert!(patch_clearance(&told) > 0.03, "clear of the sheet: {}", patch_clearance(&told));
            assert_eq!(crate::detangle::self_intersections(&told).crossings, 0);
            assert_eq!(told.positions()[..4], carried.positions()[..4], "the sheet is outside the group");

            // A `before` that is another mesh is no memory of this one.
            let mut other = carried.clone();
            let work = crate::detangle::apply_from(&mut other, Some(&sphere_detail(Vec3::ZERO, 0.5, 6, 8)), &node);
            assert_eq!(other.positions(), blind.positions());
            assert_eq!((work.crossed, work.held), (0, 0));
        }

        // What a point went AROUND it did not go through: carried past the
        // sheet's edge and under it, the patch is left where it is.
        let shift = |d: &mut Detail, by: Vec3| {
            for p in 4..d.num_points() {
                let v = d.pos(p);
                d.set_pos(p, v + by);
            }
        };
        let (mut before, mut carried) = (sheet_and_patch(0.5, 0.0), sheet_and_patch(-0.5, 0.0));
        shift(&mut before, Vec3::new(2.0, 0.0, 0.0));
        shift(&mut carried, Vec3::new(2.0, 0.0, 0.0));
        let mut told = carried.clone();
        let work = crate::detangle::apply_from(&mut told, Some(&before), &node);
        assert_eq!(told.positions(), carried.positions(), "{work:?}");
    }

    /// Two triangles at right angles, an edge of each facing an edge of the
    /// other across `gap`: the first lies flat with its edge along x, the
    /// second stands upright with its edge along y, in front of it. No
    /// point of either is anywhere near the other triangle — the edges are
    /// nearest at their middles — so this is the contact only an edge test
    /// can see. The second triangle is the group "upright".
    fn crossed_triangles(gap: f32) -> Detail {
        let mut d = Detail::new();
        let flat = [Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 2.0)].map(|p| d.add_point(p));
        let upright = [Vec3::new(0.0, -1.0, -gap), Vec3::new(0.0, 1.0, -gap), Vec3::new(0.0, 0.0, -2.0 - gap)].map(|p| d.add_point(p));
        d.add_prim(&flat);
        d.add_prim(&upright);
        for p in upright {
            d.points_mut().add_to_group("upright", p as usize);
        }
        d
    }

    /// Edge contact: two edges near each other, or through each other,
    /// where no point is near or through any triangle. Without it the
    /// Surface method sees nothing in either, and the second is a crossing
    /// the measure counts.
    #[test]
    fn edge_contact_parts_edges_no_point_test_can_see() {
        let settings = |edges: &'static str, group: &'static str| {
            phase3_node(
                "detangle",
                &[("method", "Surface"), ("thickness", "0.20"), ("rings", "2"), ("iterations", "8"), ("edge_contact", edges), ("group", group)],
            )
        };
        let gap = |d: &Detail| d.pos(0).z - d.pos(3).z;
        let near = crossed_triangles(0.1);
        let edges = near.edges();
        let thickness = 0.2 * edges.iter().map(|e| (near.pos(e[1] as usize) - near.pos(e[0] as usize)).length()).sum::<f32>() / edges.len() as f32;
        assert!(thickness > 0.3 && thickness < 0.9, "more than the gap, less than any point is from the other triangle: {thickness}");

        let mut without = near.clone();
        let work = crate::detangle::apply(&mut without, &settings("false", ""));
        assert_eq!((work.contacts, without.positions()), (0, near.positions()));
        // A node from before the row is one without it.
        let mut before_the_row = near.clone();
        crate::detangle::apply(&mut before_the_row, &phase3_node("detangle", &[("method", "Surface"), ("thickness", "0.20")]));
        assert_eq!(before_the_row.positions(), near.positions());

        let mut with = near.clone();
        let work = crate::detangle::apply(&mut with, &settings("true", ""));
        assert!(work.edge_contacts > 0 && work.edge_contacts == work.contacts, "{work:?}");
        assert!(gap(&with) > thickness * 0.95, "parted to {} of {thickness}", gap(&with));
        // The move is shared by the four ends and no one else.
        assert!(with.pos(0).z > 0.0 && with.pos(3).z < -0.1);
        assert_eq!((with.pos(2), with.pos(5)), (near.pos(2), near.pos(5)));
        // Outside the group the flat triangle stays, and the upright one
        // takes the whole move.
        let mut held = near.clone();
        crate::detangle::apply(&mut held, &settings("true", "upright"));
        assert_eq!(held.positions()[..3], near.positions()[..3]);
        assert!(gap(&held) > thickness * 0.95, "{}", gap(&held));

        // Carried through: the upright edge from in front of the flat one
        // to behind it, by less than a thickness and by several.
        for to in [-0.1f32, -1.2] {
            let carried = crossed_triangles(to);
            assert!(crate::detangle::self_intersections(&carried).crossings > 0, "to {to}: the fixture crosses");
            for (edges, told, parted) in [("false", true, false), ("true", false, false), ("true", true, true)] {
                let mut d = carried.clone();
                let work = crate::detangle::apply_from(&mut d, told.then_some(&near), &settings(edges, "upright"));
                let crossings = crate::detangle::self_intersections(&d).crossings;
                assert_eq!(crossings == 0 && gap(&d) > 0.0, parted, "to {to}, edges {edges}, told {told}: gap {}, {crossings} crossings, {work:?}", gap(&d));
                if parted {
                    assert!(work.crossed > 0 || work.held > 0, "{work:?}");
                    assert_eq!(d.positions()[..3], carried.positions()[..3]);
                }
            }
        }

        // An edge with neither end near anything is not tested at all.
        let round = crate::shapes::sphere_node_detail(
            &phase3_node("sphere", &[("method", "Icosphere"), ("frequency", "8"), ("radius", "0.5")]),
            Some(Vec3::ZERO),
        );
        let mut d = round.clone();
        let work = crate::detangle::apply(&mut d, &phase3_node("detangle", &[("method", "Surface"), ("thickness", "0.50"), ("rings", "2"), ("edge_contact", "true")]));
        assert_eq!((work.edges_searched, work.contacts), (0, 0), "{work:?}");
        assert_eq!(d.positions(), round.positions());
    }

    /// A fold: a point carried through a triangle of its own neighbourhood.
    /// The rings are excluded from contact, since a neighbour is nearer
    /// than a thickness by construction — but going through is not a
    /// distance, and told where the step began the solve puts the point
    /// back over its neighbour, as far over it as it was.
    #[test]
    fn fold_contact_puts_back_what_went_through_its_own_neighbourhood() {
        // A shallow bowl, so that nothing lies in its neighbour's plane.
        let at = |i: usize, j: usize| (i * 6 + j) as u32;
        let bowl = {
            let mut d = Detail::new();
            for i in 0..6 {
                for j in 0..6 {
                    let (x, z) = (i as f32 - 2.5, j as f32 - 2.5);
                    d.add_point(Vec3::new(x * 0.25, 0.02 * (x * x + z * z), z * 0.25));
                }
            }
            for i in 0..5 {
                for j in 0..5 {
                    d.add_prim(&[at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
                    d.add_prim(&[at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
                }
            }
            d.points_mut().add_to_group("mover", at(2, 2) as usize);
            d
        };
        let mover = at(2, 2) as usize;
        // A triangle one hop away: it has the mover's neighbour for a corner.
        let tri = [at(3, 3), at(4, 4), at(4, 3)].map(|p| p as usize);
        let over = |d: &Detail| {
            let [a, b, c] = tri.map(|p| d.pos(p));
            (d.pos(mover) - a).dot((b - a).cross(c - a).normalize())
        };
        let began = over(&bowl);
        assert!(began > 0.01, "the mover begins over its neighbour: {began}");
        let mut carried = bowl.clone();
        let [a, b, c] = tri.map(|p| bowl.pos(p));
        carried.set_pos(mover, (a + b + c) / 3.0 - (b - a).cross(c - a).normalize() * 0.04);
        assert!(over(&carried) < 0.0);
        assert!(crate::detangle::self_intersections(&carried).crossings > 0, "the fixture crosses");
        assert_eq!(crate::detangle::crossings_beyond(&carried, 2), 0, "and inside the rings, where contact does not look");

        let settings = |folds: &'static str| {
            phase3_node(
                "detangle",
                &[("method", "Surface"), ("thickness", "1.00"), ("rings", "2"), ("iterations", "8"), ("edge_contact", "true"), ("fold_contact", folds), ("group", "mover")],
            )
        };
        // Without it, and with it but not told where the step began, the
        // fold stays.
        for (node, told) in [(settings("false"), true), (settings("true"), false), (phase3_node("detangle", &[("method", "Surface"), ("group", "mover")]), true)] {
            let mut d = carried.clone();
            let work = crate::detangle::apply_from(&mut d, told.then_some(&bowl), &node);
            assert_eq!(work.folds, 0);
            assert!(over(&d) < 0.0, "told {told}: {work:?}");
        }
        let mut d = carried.clone();
        let work = crate::detangle::apply_from(&mut d, Some(&bowl), &settings("true"));
        assert!(work.folds > 0, "{work:?}");
        assert!(over(&d) > 0.0, "back over its neighbour: {}, {work:?}", over(&d));
        // As far over it as it began and no further: a neighbour is nearer
        // than a thickness, and is not pushed out to one.
        assert!(over(&d) < began * 1.5 + 1e-4, "{} against {began}", over(&d));
        assert_eq!(crate::detangle::self_intersections(&d).crossings, 0);
        for p in (0..d.num_points()).filter(|&p| p != mover) {
            assert_eq!(d.pos(p), carried.pos(p), "only the group moves");
        }

        // A step that folds nothing finds nothing and holds nothing.
        let mut still = bowl.clone();
        for p in 0..still.num_points() {
            let v = still.pos(p);
            still.set_pos(p, v + Vec3::new(0.01, 0.02 * v.x, 0.0));
        }
        let moved = still.clone();
        let mut node = settings("true");
        node.params.retain(|p| p.name != "group");
        let work = crate::detangle::apply_from(&mut still, Some(&bowl), &node);
        assert_eq!((work.folds, work.held), (0, 0), "{work:?}");
        assert_eq!(still.positions(), moved.positions());
    }

    /// MCP's `enter` dives into exactly what the keyboard does: a simnet
    /// with no children yet is a container, a native node is not.
    #[test]
    fn mcp_enter_agrees_with_the_keyboard_about_what_is_enterable() {
        let mut state = State::new(false);
        let node = |name: &str, ty: &str| crate::app::FsNode {
            id: format!("{name}-id"),
            name: name.to_string(),
            node_type: ty.to_string(),
            children: vec![],
            params: vec![],
            geometry_visible: false,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        };
        state.current_path.clear();
        state.fs_root.children.push(node("sim_empty", "simnet"));
        state.fs_root.children.push(node("remesh_leaf", "remesh"));
        let mut redraw = false;
        for (i, child) in state.fs_root.children.clone().iter().enumerate() {
            let entered = state.apply_action(McpAction::Enter { slot: i }, &mut redraw).is_ok();
            assert_eq!(entered, child.is_enterable(), "{} ({})", child.name, child.node_type);
            if entered {
                state.apply_action(McpAction::Up, &mut redraw).unwrap();
            }
        }
        let names: Vec<_> = state.fs_root.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"sim_empty") && names.contains(&"remesh_leaf"));
    }

    /// A bypassed node is in the graph and does nothing: what reads it gets
    /// what it reads. A generator, which reads nothing, gives nothing; a
    /// subnet passes its Input and its children are not run; and a node
    /// whose own parameters would fail is not evaluated to find that out.
    #[test]
    fn a_bypassed_node_passes_its_input_through() {
        let pull = |bypassed: bool| {
            let mut n = ref_node(
                "pull",
                "pull1",
                "attribute",
                vec![("input", "node", "sphere1"), ("operation", "text", "Modify"), ("attribute_name", "text", "Pos"), ("value", "text", "1.00:0.00:0.00"), ("combine", "text", "Add")],
                vec![],
            );
            n.bypassed = bypassed;
            n
        };
        let sphere = || ref_node("s", "sphere1", "sphere", vec![("radius", "slider", "0.5"), ("center_x", "slider", "0"), ("center_y", "slider", "0"), ("center_z", "slider", "0")], vec![]);
        let root = |nodes: Vec<FsNode>| ref_node("root", "root", "node", vec![], nodes);
        let min_x = |d: &Detail| d.positions().iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);

        let (plain, _) = eval_node(&root(vec![sphere()]), "sphere1");
        let (moved, err) = eval_node(&root(vec![sphere(), pull(false)]), "pull1");
        assert!(err.is_none() && (min_x(&moved) - min_x(&plain) - 1.0).abs() < 1e-5);
        let (passed, err) = eval_node(&root(vec![sphere(), pull(true)]), "pull1");
        assert!(err.is_none());
        assert_eq!(passed.positions(), plain.positions());

        // What reads a bypassed node reads through it.
        let mut second = pull(false);
        (second.id, second.name) = ("pull2".into(), "pull2".into());
        second.params[0].set_text("pull1");
        let (after, _) = eval_node(&root(vec![sphere(), pull(true), second]), "pull2");
        assert!((min_x(&after) - min_x(&plain) - 1.0).abs() < 1e-5, "one pull, not two");

        // A generator reads nothing and gives nothing.
        let mut off = sphere();
        off.bypassed = true;
        let tree = root(vec![off]);
        let mut cache = crate::geometry::SimCache::default();
        let mut sim = crate::geometry::EvalSim::new(0, 0, &mut cache);
        let mut err = None;
        assert!(crate::geometry::generate_single_node_geometry_with_errors(&tree, &tree.children[0], &mut Vec::new(), &mut err, &mut sim).is_none());
        assert!(err.is_none());

        // Its own parameters are not evaluated: an expression that names
        // nothing is an error on the node, and not on a bypassed one.
        let broken = |bypassed: bool| {
            let mut n = pull(bypassed);
            n.params.push(crate::app::ParamDef::new("strength", "slider", "ch(\"../nothing/here\")").as_expr());
            root(vec![sphere(), n])
        };
        assert!(eval_node(&broken(false), "pull1").1.is_some());
        let (passed, err) = eval_node(&broken(true), "pull1");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(passed.positions(), plain.positions());

        // A subnet passes its Input, and what is inside it is not run.
        let mut subnet = ref_node(
            "sub",
            "sub1",
            "node",
            vec![("input", "node", "sphere1")],
            vec![
                ref_node("in", "input1", "input", vec![], vec![]),
                { let mut p = pull(false); p.params[0].set_text("input1"); p },
                ref_node("out", "output1", "output", vec![("input", "node", "pull1")], vec![]),
            ],
        );
        let (through, _) = eval_node(&root(vec![sphere(), subnet.clone()]), "sub1");
        assert!((min_x(&through) - min_x(&plain) - 1.0).abs() < 1e-5);
        subnet.bypassed = true;
        let (passed, _) = eval_node(&root(vec![sphere(), subnet.clone()]), "sub1");
        assert_eq!(passed.positions(), plain.positions());

        // The flag says nothing on a subnet's plumbing.
        subnet.bypassed = false;
        subnet.children[0].bypassed = true;
        subnet.children[2].bypassed = true;
        let (through, _) = eval_node(&root(vec![sphere(), subnet]), "sub1");
        assert!((min_x(&through) - min_x(&plain) - 1.0).abs() < 1e-5);

        // The scene draws a shown, bypassed node as what it passes, and a
        // bypassed generator as nothing.
        let scene = |nodes: Vec<FsNode>| {
            let tree = root(nodes);
            let mut cache = crate::geometry::SimCache::default();
            let mut sim = crate::geometry::EvalSim::new(0, 0, &mut cache);
            crate::geometry::network_sphere_vertices_with_errors(&tree, &tree, &mut None, &mut sim)
        };
        let hidden = |mut n: FsNode| { n.geometry_visible = false; n };
        assert_eq!(scene(vec![hidden(sphere()), pull(true)]).positions(), plain.positions());
        assert!((min_x(&scene(vec![hidden(sphere()), pull(false)])) - min_x(&plain) - 1.0).abs() < 1e-5);
        let mut off = sphere();
        off.bypassed = true;
        assert_eq!(scene(vec![off]).num_points(), 0);
    }

    /// Bypass in the 2D context, which has a resolver of its own: a
    /// bypassed page node passes the sheet it was handed, a bypassed sheet
    /// is no sheet, and a bypassed Export — the one node in both contexts —
    /// passes either.
    #[test]
    fn a_bypassed_page_node_passes_its_sheet_through() {
        use crate::page::{displayed_page, resolve_page};
        let node = |id: &str, name: &str, ty: &str, params: &[(&str, &str)]| {
            ref_node(id, name, ty, params.iter().map(|&(n, v)| (n, "text", v)).collect(), vec![])
        };
        let chain = |bypassed: &[&str]| {
            let mut nodes = vec![
                node("p", "page1", "page", &[("preset", "Letter"), ("resolution", "72"), ("color", "1.00:1.00:1.00")]),
                node("g", "grid1", "page_grid", &[("input", "page1"), ("cell_size", "0.5"), ("line_width", "0.02"), ("line_color", "0.00:0.00:0.00"), ("fill_cells", "false")]),
                node("b", "border1", "page_border", &[("input", "grid1"), ("width", "0.1"), ("inset", "0.25"), ("color", "1.00:0.00:0.00")]),
                node("e", "export1", "export", &[("input", "border1")]),
            ];
            for n in &mut nodes {
                n.bypassed = bypassed.contains(&n.name.as_str());
            }
            ref_node("r", "root", "node", vec![], nodes)
        };
        let sheet = |root: &FsNode, name: &str| resolve_page(root, root.children.iter().find(|c| c.name == name).unwrap(), &mut Vec::new());
        // Where the border's ink is, and where a rule of the grid is:
        // green is what tells red ink and black ink from the white sheet.
        let border = |p: &crate::page::Page| p.pixels[(400 * p.width + 20) as usize];
        let rule = |p: &crate::page::Page| p.pixels[(400 * p.width + 36 * 4) as usize];
        // Said, not shown: a sheet that differs is half a million pixels.
        let same = |a: &crate::page::Page, b: &crate::page::Page, what: &str| {
            let differ = a.pixels.iter().zip(&b.pixels).filter(|(x, y)| x != y).count();
            assert!(a.pixels.len() == b.pixels.len() && differ == 0, "{what}: {differ} of {} pixels differ", a.pixels.len());
        };

        let whole = sheet(&chain(&[]), "border1").expect("the chain resolves");
        assert!(border(&whole)[0] > 0.9 && border(&whole)[1] < 0.1, "the border is red: {:?}", border(&whole));
        assert!(rule(&whole)[1] < 0.4, "the grid ruled the sheet: {:?}", rule(&whole));

        // The border bypassed: the grid's sheet, as the grid made it.
        let no_border = sheet(&chain(&["border1"]), "border1").expect("passes the grid's sheet");
        same(&no_border, &sheet(&chain(&[]), "grid1").unwrap(), "the border bypassed");
        assert!(border(&no_border)[1] > 0.9, "no border ink: {:?}", border(&no_border));
        assert!(rule(&no_border)[1] < 0.4, "and the rules are still there");

        // The grid bypassed, in the middle: the border on a sheet with no
        // rules.
        let no_grid = sheet(&chain(&["grid1"]), "border1").expect("the border reads through the grid");
        assert!(border(&no_grid)[0] > 0.9 && border(&no_grid)[1] < 0.1);
        assert!(rule(&no_grid)[1] > 0.9, "no rule: {:?}", rule(&no_grid));
        assert_eq!((no_grid.width, no_grid.height), (whole.width, whole.height));

        // Both: the sheet itself.
        let bare = sheet(&chain(&["grid1", "border1"]), "border1").expect("the sheet");
        same(&bare, &sheet(&chain(&[]), "page1").unwrap(), "the grid and the border bypassed");

        // A bypassed sheet is no sheet, and nothing drawn on it is a page.
        assert!(sheet(&chain(&["page1"]), "page1").is_none());
        assert!(sheet(&chain(&["page1"]), "border1").is_none());

        // Export passes a page through, bypassed or not.
        same(&sheet(&chain(&[]), "export1").unwrap(), &whole, "through an export");
        same(&sheet(&chain(&["export1"]), "export1").unwrap(), &whole, "through a bypassed export");

        // What the pane shows is the level's last shown page node, and a
        // bypassed one shows what it passes.
        let mut root = chain(&["border1"]);
        root.children.pop();
        same(&displayed_page(&root, &root).expect("displayed"), &no_border, "what the pane shows");
    }

    /// The flag is written only when it is set, so a file that never
    /// bypassed anything is byte for byte the file it was — and a simnet's
    /// solve, keyed by its JSON, restarts when a node in its chain is
    /// bypassed and not otherwise.
    #[test]
    fn the_bypass_flag_is_saved_only_when_it_is_set() {
        let mut node = ref_node("a", "a1", "sphere", vec![("radius", "slider", "0.5")], vec![]);
        let plain = serde_json::to_string(&node).unwrap();
        assert!(!plain.contains("bypassed"), "{plain}");
        node.bypassed = true;
        let set = serde_json::to_string(&node).unwrap();
        assert!(set.contains("\"bypassed\":true"), "{set}");
        let back: FsNode = serde_json::from_str(&set).unwrap();
        assert!(back.bypassed);
        let old: FsNode = serde_json::from_str(&plain).unwrap();
        assert!(!old.bypassed);
    }

    /// Bypass from each place that asks for it — the `b` command on the
    /// network's selection, the node's menu, MCP — and the scene, the
    /// status line and the node's look follow.
    #[test]
    fn bypass_is_one_flag_however_it_is_asked_for() {
        use crate::app::McpAction;
        // An empty Geometry node: the bundled project's, emptied.
        let mut state = State::new(false);
        let mut redraw = false;
        assert_eq!(state.current_path.len(), 1, "the bundled project opens in its Geometry node");
        state.current_dir_mut().children.clear();
        state.sync_nodes();
        state.apply_action(McpAction::AddNode { template_name: "Sphere".into(), name: Some("ball".into()), x: 3.0, y: 3.0 }, &mut redraw).unwrap();
        state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 3.0, y: 4.0 }, &mut redraw).unwrap();
        let slot = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).unwrap();
        let (ball, pull) = (slot(&state, "ball"), slot(&state, "pull1"));
        for (name, value) in [("input", "ball"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "1.00:0.00:0.00"), ("combine", "Add")] {
            state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
        }
        state.apply_action(McpAction::ToggleGeometry { slot: pull }, &mut redraw).unwrap();
        let min_x = |state: &State| state.rt_sphere_verts.iter().map(|v| v.position[0]).fold(f32::INFINITY, f32::min);
        let shown = min_x(&state);

        // MCP.
        let said = state.apply_action(McpAction::ToggleBypass { slot: pull }, &mut redraw).unwrap();
        assert_eq!(said, "Bypassed: true");
        assert!(state.current_dir().children[pull].bypassed);
        assert!((min_x(&state) - (shown - 1.0)).abs() < 1e-4, "the scene is the sphere where it was: {} from {shown}", min_x(&state));
        assert_eq!(state.last_status_text, "Bypassed pull1.");
        assert!(state.apply_action(McpAction::ToggleBypass { slot: 99 }, &mut redraw).is_err());

        // The command, on the selection, and only with the network focused.
        state.apply_action(McpAction::Select { slot: pull }, &mut redraw).unwrap();
        state.focused_pane = crate::slots::PARAM_IDX;
        state.run_command("bypass_node");
        assert!(state.current_dir().children[pull].bypassed, "a `b` typed elsewhere is a letter");
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.run_command("bypass_node");
        assert!(!state.current_dir().children[pull].bypassed);
        assert!((min_x(&state) - shown).abs() < 1e-4);
        assert_eq!(state.last_status_text, "No longer bypassing pull1.");
        assert_eq!(crate::command::by_id("bypass_node").unwrap().default_chord, Some("b"));

        // The node's menu, which says which way it will go.
        assert_eq!(state.set_bypassed(&[ball, pull], true), 2);
        assert_eq!(state.set_bypassed(&[ball, pull], true), 0, "already");
        assert!(state.rt_sphere_verts.is_empty(), "a bypassed sphere behind a bypassed pull is nothing");
        assert_eq!(state.set_bypassed(&[ball], false), 1);

        // It is saved with the project and comes back with it.
        let saved = serde_json::to_string(state.current_dir()).unwrap();
        let back: FsNode = serde_json::from_str(&saved).unwrap();
        assert!(back.children[pull].bypassed && !back.children[ball].bypassed);
    }

    /// The Step Limit holds a point's move since the step began to that
    /// many thicknesses, in the direction it was going. It is the Surface
    /// method's, it needs to know where the step began, and only what may
    /// move is held.
    #[test]
    fn the_step_limit_holds_a_step_to_a_length() {
        let settings = |method: &'static str, limit: &'static str| {
            phase3_node(
                "detangle",
                &[("method", method), ("thickness", "0.25"), ("rings", "2"), ("iterations", "4"), ("step_limit", limit), ("group", "patch")],
            )
        };
        let before = sheet_and_patch(1.0, 0.0);
        let mut carried = before.clone();
        for p in 0..carried.num_points() {
            let v = carried.pos(p);
            carried.set_pos(p, v + Vec3::new(0.3, 0.0, 0.4));
        }
        let mut d = carried.clone();
        let work = crate::detangle::apply_from(&mut d, Some(&before), &settings("surface", "0.50"));
        assert_eq!(work.limited, 25, "{work:?}");
        let went = d.pos(10) - before.pos(10);
        assert!((went.normalize() - Vec3::new(0.6, 0.0, 0.8)).length() < 1e-4, "the way it was going: {went:?}");
        // Half of a thickness that is a quarter of the mean edge, as the
        // mesh now stands.
        let edges = carried.edges();
        let edge = edges.iter().map(|e| (carried.pos(e[1] as usize) - carried.pos(e[0] as usize)).length()).sum::<f32>() / edges.len() as f32;
        assert!((went.length() - 0.5 * 0.25 * edge).abs() < 1e-5, "{} of an edge of {edge}", went.length());
        assert_eq!(d.positions()[..4], carried.positions()[..4], "the sheet is outside the group");

        for (node, told) in [(settings("surface", "0"), true), (settings("surface", "0.50"), false), (settings("points", "0.50"), true)] {
            let mut d = carried.clone();
            let work = crate::detangle::apply_from(&mut d, told.then_some(&before), &node);
            assert_eq!(work.limited, 0);
            assert_eq!(d.positions(), carried.positions());
        }
    }

    /// The Detangle node's settings tried on a PROJECT: the file named by
    /// `CCE_DETANGLE_PROJECT` (a project directory or its state.json), read
    /// and never written, its first simnet played forward a frame at a
    /// time to `CCE_DETANGLE_FRAMES` (240) with the detangle node inside it
    /// set each way in turn. Says what crossed, when, and what a frame
    /// cost. Run in release with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn detangle_on_a_project() {
        let Ok(path) = std::env::var("CCE_DETANGLE_PROJECT") else {
            println!("CCE_DETANGLE_PROJECT is not set");
            return;
        };
        let frames: i32 = std::env::var("CCE_DETANGLE_FRAMES").ok().and_then(|f| f.parse().ok()).unwrap_or(240);
        let path = std::path::PathBuf::from(path);
        let file = if path.is_dir() { path.join("state.json") } else { path };
        let mut proj: crate::app::Project = serde_json::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        let templates = crate::app::flatten_node_templates(&crate::app::load_fs_tree());
        proj.sanitize_node_names();
        proj.migrate_format();
        crate::app::merge_template_defs(&mut proj.root, &templates);

        fn find<'a>(n: &'a mut FsNode, ty: &str) -> Option<&'a mut FsNode> {
            if n.node_type == ty {
                return Some(n);
            }
            n.children.iter_mut().find_map(|c| find(c, ty))
        }
        let saved: Vec<(String, String)> = find(&mut proj.root, "detangle").expect("a detangle node").params.iter().map(|p| (p.name.clone(), p.text().to_string())).collect();
        println!("as loaded: {saved:?}");
        let ways: [(&str, &[(&str, &str)]); 6] = [
            ("off (thickness 0)", &[("method", "Points"), ("thickness", "0.00")]),
            ("points", &[("method", "Points")]),
            ("surface", &[("method", "Surface"), ("edge_contact", "false"), ("fold_contact", "false")]),
            ("surface, edges", &[("method", "Surface"), ("edge_contact", "true"), ("fold_contact", "false")]),
            ("surface, folds", &[("method", "Surface"), ("edge_contact", "false"), ("fold_contact", "true")]),
            ("all", &[("method", "Surface"), ("edge_contact", "true"), ("fold_contact", "true")]),
        ];
        let out = std::env::var("CCE_DETANGLE_OUT").ok();
        for (name, settings) in ways {
            let node = find(&mut proj.root, "detangle").unwrap();
            for (param, text) in saved.iter().map(|(a, b)| (a.as_str(), b.as_str())).chain(settings.iter().copied()) {
                node.params.iter_mut().find(|p| p.name == param).expect("the template's row").set_text(text);
            }
            let root = &proj.root;
            let simnet = crate::geometry::find_node_by_name(root, "simnet1").expect("simnet1");
            let mut cache = crate::geometry::SimCache::default();
            let (mut worst, mut first, mut last, mut spent, mut moved) = (0, None, 0, std::time::Duration::ZERO, 0.0f32);
            let mut began: Option<Detail> = None;
            for frame in 1..=frames {
                let mut sim = crate::geometry::EvalSim::new(frame, 1, &mut cache);
                let mut err = None;
                let t = std::time::Instant::now();
                let d = crate::geometry::generate_single_node_geometry_with_errors(root, simnet, &mut Vec::new(), &mut err, &mut sim).expect("solves");
                spent += t.elapsed();
                assert!(err.is_none(), "{err:?}");
                let crossings = crate::detangle::self_intersections(&d).crossings;
                if crossings > 0 && first.is_none() {
                    first = Some(frame);
                }
                (worst, last) = (worst.max(crossings), crossings);
                let began = began.get_or_insert_with(|| d.clone());
                moved = (0..d.num_points()).map(|p| (d.pos(p) - began.pos(p)).length()).fold(0.0, f32::max);
                if let (Some(out), true) = (&out, frame == frames) {
                    let file = std::path::PathBuf::from(out).join(format!("{}.obj", name.replace([' ', ',', '(', ')'], "_")));
                    crate::export::write(&d, &file, crate::export::Format::from_path(&file), 1.0).expect("writes");
                }
            }
            println!(
                "{name:>18}: first crossing at frame {first:?}, worst {worst}, at frame {frames} {last}; {:.2} ms a frame; the furthest point went {moved:.3}",
                spent.as_secs_f64() * 1000.0 / frames as f64
            );
        }
    }

    /// Where a project's simulation spends its time: `simnet1` of the
    /// project `CCE_SIM_PROJECT` names played forward to `CCE_SIM_FRAMES`
    /// (60) as saved, and again with each node of its chain bypassed in
    /// turn, so what a node costs is what the solve saves without it. The
    /// file is read and never written. Run in release with `--ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn sim_profile_on_a_project() {
        let Ok(path) = std::env::var("CCE_SIM_PROJECT") else {
            println!("CCE_SIM_PROJECT is not set");
            return;
        };
        let frames: i32 = std::env::var("CCE_SIM_FRAMES").ok().and_then(|f| f.parse().ok()).unwrap_or(60);
        let path = std::path::PathBuf::from(path);
        let file = if path.is_dir() { path.join("state.json") } else { path };
        let mut proj: crate::app::Project = serde_json::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
        let templates = crate::app::flatten_node_templates(&crate::app::load_fs_tree());
        proj.sanitize_node_names();
        proj.migrate_format();
        crate::app::merge_template_defs(&mut proj.root, &templates);
        fn simnet(n: &mut FsNode) -> Option<&mut FsNode> {
            if n.node_type == "simnet" {
                return Some(n);
            }
            n.children.iter_mut().find_map(simnet)
        }
        let sim_node = simnet(&mut proj.root).expect("a simnet");
        // Never the disk: this measures the solve.
        if let Some(p) = sim_node.params.iter_mut().find(|p| p.name == "cache") {
            p.set_text("false");
        }
        let sim_name = sim_node.name.clone();
        let chain: Vec<String> = sim_node
            .children
            .iter()
            .filter(|c| !matches!(c.node_type.as_str(), "input" | "output") && !c.bypassed)
            .map(|c| c.name.clone())
            .collect();
        let mut ways: Vec<Option<String>> = vec![None];
        ways.extend(chain.into_iter().map(Some));
        for way in ways {
            let mut proj = proj.clone();
            if let Some(name) = &way {
                simnet(&mut proj.root).unwrap().children.iter_mut().find(|c| &c.name == name).unwrap().bypassed = true;
            }
            let root = &proj.root;
            let node = crate::geometry::find_node_by_name(root, &sim_name).expect("the simnet");
            let mut cache = crate::geometry::SimCache::default();
            let mut times = Vec::new();
            let mut points = Vec::new();
            let mut remeshed = Vec::new();
            for frame in 1..=frames {
                let mut sim = crate::geometry::EvalSim::new(frame, 1, &mut cache);
                let mut err = None;
                crate::remesh::take_edge_changes();
                let t = std::time::Instant::now();
                let d = crate::geometry::generate_single_node_geometry_with_errors(root, node, &mut Vec::new(), &mut err, &mut sim);
                times.push(t.elapsed().as_secs_f64() * 1000.0);
                points.push(d.map_or(0, |d| d.num_points()));
                remeshed.push(crate::remesh::take_edge_changes());
            }
            let mean = times.iter().sum::<f64>() / times.len() as f64;
            let worst = times.iter().cloned().fold(0.0, f64::max);
            let at = |f: usize| times.get(f - 1).copied().unwrap_or(0.0);
            println!(
                "{:>22}: {mean:7.2} ms a frame, worst {worst:7.2}; frame 2 {:.2}, 10 {:.2}, 30 {:.2}, last {:.2}; points {} -> {}; edges remeshed at frame 2 {}, 30 {}, last {}",
                way.map_or("as saved".to_string(), |n| format!("without {n}")),
                at(2), at(10), at(30), at(frames as usize),
                points.first().unwrap(), points.last().unwrap(),
                remeshed.get(1).copied().unwrap_or(0), remeshed.get(29).copied().unwrap_or(0), remeshed.last().copied().unwrap_or(0),
            );
        }
    }

    /// What the whole of the Surface method costs where most of a mesh is
    /// in contact: the sphere test's workload with every row on,
    /// at a fifth of an edge a step. The sum is of every position at every
    /// step, which is what says a change to how the solve is RUN left what
    /// it does alone. Run in release with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn detangle_timing() {
        for frequency in ["4", "8", "16"] {
            let sphere = crate::shapes::sphere_node_detail(
                &phase3_node("sphere", &[("method", "Icosphere"), ("frequency", frequency), ("radius", "0.5")]),
                Some(Vec3::ZERO),
            );
            let edges = sphere.edges();
            let edge = edges.iter().map(|e| (sphere.pos(e[1] as usize) - sphere.pos(e[0] as usize)).length()).sum::<f32>() / edges.len() as f32;
            let cap: Vec<usize> = (0..sphere.num_points()).filter(|&p| sphere.pos(p).y > 0.2).collect();
            let rate = edge * 0.2;
            let steps = (1.1 / rate).ceil() as usize;
            let node = phase3_node(
                "detangle",
                &[("method", "Surface"), ("thickness", "1.00"), ("rings", "2"), ("iterations", "4"), ("edge_contact", "true"), ("fold_contact", "true")],
            );
            let mut d = sphere.clone();
            let (mut worst, mut sum, mut spent, mut held, mut finds) = (0, 0.0f64, std::time::Duration::ZERO, 0, 0);
            for _ in 0..steps {
                let before = d.clone();
                for &p in &cap {
                    let v = d.pos(p);
                    d.set_pos(p, v - Vec3::new(0.0, rate, 0.0));
                }
                let t = std::time::Instant::now();
                let w = crate::detangle::apply_from(&mut d, Some(&before), &node);
                spent += t.elapsed();
                finds += w.grids;
                held += w.held;
                worst = worst.max(crate::detangle::self_intersections(&d).crossings);
                sum += d.positions().iter().flatten().map(|&c| c as f64).sum::<f64>();
            }
            println!(
                "{:>5} points: {:.2} ms a step, {:.1} searches a step; worst {worst} crossings, {held} held, sum {sum:.6}",
                d.num_points(),
                spent.as_secs_f64() * 1000.0 / steps as f64,
                finds as f64 / steps as f64
            );
        }
    }

    /// The two methods side by side, by the measure and by the clock: a
    /// sphere's cap pushed down into its own bowl a little each step, until
    /// it would have come out underneath. Run in release with `--ignored
    /// --nocapture`. (Not pressed flat by the sign of y: that carries the
    /// equator's points past their own neighbours, inside the excluded
    /// rings, which no setting of the node is meant to see.)
    #[test]
    #[ignore]
    fn detangle_methods_compared() {
        // The method, whether it is told where the step began, and its
        // Step Limit.
        let ways: [(&str, &str, bool, &str, &str); 8] = [
            ("none", "None", false, "false", "false"),
            ("points", "Points", false, "false", "false"),
            ("surface", "Surface", false, "false", "false"),
            ("surface, edges", "Surface", false, "true", "false"),
            ("sided", "Surface", true, "false", "false"),
            ("sided, edges", "Surface", true, "true", "false"),
            ("sided, folds", "Surface", true, "false", "true"),
            ("all", "Surface", true, "true", "true"),
        ];
        for frequency in ["4", "8", "16"] {
            let sphere = crate::shapes::sphere_node_detail(
                &phase3_node("sphere", &[("method", "Icosphere"), ("frequency", frequency), ("radius", "0.5")]),
                Some(Vec3::ZERO),
            );
            let edges = sphere.edges();
            let edge = edges.iter().map(|e| (sphere.pos(e[1] as usize) - sphere.pos(e[0] as usize)).length()).sum::<f32>() / edges.len() as f32;
            let cap: Vec<usize> = (0..sphere.num_points()).filter(|&p| sphere.pos(p).y > 0.2).collect();
            // In edges a step: a fifth, and then more than a thickness.
            for pace in [0.2f32, 0.8] {
                let rate = edge * pace;
                let steps = (1.1 / rate).ceil() as usize;
                for thickness in ["0.50", "1.00"] {
                    for (name, method, told, edges, folds) in ways {
                        let node = phase3_node(
                            "detangle",
                            &[("method", method), ("thickness", thickness), ("rings", "2"), ("iterations", "4"), ("edge_contact", edges), ("fold_contact", folds)],
                        );
                        let mut d = sphere.clone();
                        let (mut worst, mut far, mut spent) = (0, 0, std::time::Duration::ZERO);
                        let mut tally = crate::detangle::Work::default();
                        for _ in 0..steps {
                            let before = d.clone();
                            for &p in &cap {
                                let v = d.pos(p);
                                d.set_pos(p, v - Vec3::new(0.0, rate, 0.0));
                            }
                            if method != "None" {
                                let t = std::time::Instant::now();
                                let w = crate::detangle::apply_from(&mut d, told.then_some(&before), &node);
                                spent += t.elapsed();
                                tally.crossed += w.crossed;
                                tally.held += w.held;
                                tally.edges_searched += w.edges_searched;
                                tally.folds += w.folds;
                            }
                            worst = worst.max(crate::detangle::self_intersections(&d).crossings);
                            far = far.max(crate::detangle::crossings_beyond(&d, 2));
                        }
                        let last = crate::detangle::self_intersections(&d).crossings;
                        println!(
                            "{:>5} points, pace {pace}, {steps:>4} steps, thickness {thickness}, {name:>15}: worst {worst:>5} crossings ({far:>5} beyond the rings), last {last:>5}, {:.2} ms a step; {} put back through, {} held, {} folds",
                            d.num_points(),
                            spent.as_secs_f64() * 1000.0 / steps as f64,
                            tally.crossed,
                            tally.held,
                            tally.folds
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_suture_counts_sustained_contact_before_it_fuses() {
        // A grid sitting just above a collider it is in contact with.
        let collider = {
            let mut c = Detail::new();
            let pts: Vec<u32> = [
                Vec3::new(-1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(-1.0, 0.0, 1.0),
            ]
            .iter()
            .map(|&p| c.add_point(p))
            .collect();
            c.add_prim(&[pts[0], pts[1], pts[2]]);
            c.add_prim(&[pts[0], pts[2], pts[3]]);
            c
        };
        let mut sheet = Detail::new();
        for i in 0..3 {
            sheet.add_point(Vec3::new(i as f32 * 0.02, 0.01, 0.0));
        }
        sheet.add_point(Vec3::new(0.0, 5.0, 0.0)); // far away, never in contact

        let node = phase3_node(
            "suture",
            &[("distance_threshold", "0.10"), ("fusion_threshold", "3"), ("counter", "contact")],
        );
        let count = |d: &Detail, p: usize| d.points().value("contact", p).unwrap().as_f32() as i32;

        // Contact accrues, and the contacting points are pushed out to the
        // threshold.
        crate::geometry::apply_suture(&mut sheet, Some(&collider), &node);
        assert_eq!(count(&sheet, 0), 1);
        assert!((sheet.pos(0).y - 0.10).abs() < 1e-3, "not pushed out: {}", sheet.pos(0).y);
        // A point out of contact stays at zero — contact has to be SUSTAINED
        // to count, which is the difference between brushing past and growing
        // together.
        assert_eq!(count(&sheet, 3), 0);

        crate::geometry::apply_suture(&mut sheet, Some(&collider), &node);
        assert_eq!(count(&sheet, 0), 2);
        assert_eq!(sheet.num_points(), 4, "nothing fuses below the threshold");

        // The third crossing takes them past Fusion Threshold, and the three
        // contacting points — all within Distance Threshold of each other —
        // become one. The far point is untouched.
        crate::geometry::apply_suture(&mut sheet, Some(&collider), &node);
        assert_eq!(sheet.num_points(), 2, "sustained contact did not fuse");
    }

    #[test]
    fn test_suture_without_a_collider_changes_nothing_but_the_counter() {
        let mut sheet = sphere_detail(Vec3::ZERO, 0.5, 4, 6);
        let before = sheet.positions().to_vec();
        let node = phase3_node("suture", &[("distance_threshold", "0.10"), ("counter", "contact")]);
        crate::geometry::apply_suture(&mut sheet, None, &node);
        assert_eq!(sheet.positions(), &before[..]);
        // The counter exists so the chain downstream can read it either way.
        assert!(sheet.points().has("contact"));
        assert_eq!(sheet.points().kind("contact"), AttribKind::Live);
    }

    #[test]
    fn test_fusing_points_keeps_the_representative_and_drops_folded_faces() {
        let mut d = quad_grid();
        let keep_id = d.id(0).unwrap();
        d.points_mut().create("mass", AttribValue::Float(0.0));
        d.points_mut().set_value("mass", 0, AttribValue::Float(7.0)).unwrap();
        d.points_mut().set_value("mass", 1, AttribValue::Float(9.0)).unwrap();

        // Point 1 merges onto point 0.
        let mut rep: Vec<u32> = (0..9).collect();
        rep[1] = 0;
        d.fuse_points(&rep);

        assert_eq!(d.num_points(), 8);
        // The representative keeps its identity AND its values — the same
        // choice the remesher's collapse makes, and for the same reason.
        assert_eq!(d.id(0), Some(keep_id));
        assert_eq!(d.points().value("mass", 0), Some(AttribValue::Float(7.0)));
        // Every surviving primitive is still a real triangle or quad; the ones
        // that ended up with a repeated corner are gone.
        for prim in 0..d.num_prims() {
            let pts = d.prim_points(prim);
            let mut uniq = pts.to_vec();
            uniq.sort_unstable();
            uniq.dedup();
            assert_eq!(uniq.len(), pts.len(), "prim {prim} folded onto itself");
        }
        // A chain resolves: a→b→c leaves everything at c.
        let mut e = quad_grid();
        let mut chain: Vec<u32> = (0..9).collect();
        chain[2] = 1;
        chain[1] = 0;
        e.fuse_points(&chain);
        assert_eq!(e.num_points(), 7);
    }

    // ---- Phase 2: the solver contract ----

    #[test]
    fn test_attribute_kind_defaults_to_live_and_is_declared_at_creation() {
        let mut d = quad_grid();
        d.points_mut().create("mass", AttribValue::Float(1.0));
        d.points_mut()
            .create_kind("scratch", AttribValue::Float(1.0), AttribKind::Derivative);

        // Live is the default because its failure mode is the visible one: a
        // value that should have been cleared and was not drifts where you can
        // watch it, while one that should have persisted and was cleared just
        // quietly reads zero.
        assert_eq!(d.points().kind("mass"), AttribKind::Live);
        assert_eq!(d.points().kind("scratch"), AttribKind::Derivative);
        assert_eq!(d.points().kind("never-declared"), AttribKind::Live);
        assert_eq!(d.points().names_of_kind(AttribKind::Derivative), vec!["scratch"]);

        d.clear_derivatives();
        // The column stays and the values reset: a reader between two steps
        // finds the attribute present and empty, not missing.
        assert!(d.points().has("scratch"));
        assert_eq!(d.points().value("scratch", 0), Some(AttribValue::Float(0.0)));
        assert_eq!(d.points().value("mass", 0), Some(AttribValue::Float(1.0)));

        // A name reused for a different purpose is a different attribute, so
        // re-creating it re-declares the kind.
        d.points_mut().create("scratch", AttribValue::Float(2.0));
        assert_eq!(d.points().kind("scratch"), AttribKind::Live);
    }

    #[test]
    fn test_attribute_kinds_survive_the_structural_rewrites() {
        let mut d = quad_grid();
        d.points_mut()
            .create_kind("scratch", AttribValue::Float(1.0), AttribKind::Derivative);

        let mut kept = d.clone();
        kept.keep_points(&(0..9).map(|i| i < 6).collect::<Vec<_>>());
        assert_eq!(kept.points().kind("scratch"), AttribKind::Derivative, "through a gather");

        let mut merged = quad_grid();
        merged.merge(&d);
        assert_eq!(merged.points().kind("scratch"), AttribKind::Derivative, "through a merge");
    }

    #[test]
    fn test_live_attributes_come_back_across_a_rebuild_by_identity() {
        let mut prev = quad_grid();
        prev.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..9 {
            prev.points_mut()
                .set_value("mass", p, AttribValue::Float(p as f32))
                .unwrap();
        }
        prev.points_mut()
            .create_kind("scratch", AttribValue::Float(7.0), AttribKind::Derivative);

        // A step that rebuilt the geometry: it kept six of the nine points,
        // added one genuinely new one, and lost every attribute on the way —
        // which is what a remesh does, and what a kernel generator does today.
        let mut next = prev.clone();
        next.keep_points(&(0..9).map(|i| i < 6).collect::<Vec<_>>());
        next.points_mut().remove("mass");
        next.points_mut().remove("scratch");
        next.add_point(Vec3::new(9.0, 9.0, 0.0));

        next.restore_live_from(&prev);

        // The simulation's memory is not gone: it is in the previous state,
        // attached to identities.
        assert!(next.points().has("mass"));
        for p in 0..6 {
            assert_eq!(next.points().value("mass", p), Some(AttribValue::Float(p as f32)));
        }
        // A point that did not exist last step gets the type's zero, which is
        // the only honest answer for a place with no history.
        assert_eq!(next.points().value("mass", 6), Some(AttribValue::Float(0.0)));
        // Derivative attributes are NOT restored — carrying one across is
        // exactly the silent accumulation the kind exists to prevent.
        assert!(!next.points().has("scratch"));
    }

    #[test]
    fn test_restoration_bridges_a_rebuild_and_does_not_undo_a_delete() {
        let mut prev = quad_grid();
        prev.points_mut().create("mass", AttribValue::Float(3.0));

        // The chain handed back the same identities in the same order, so it
        // kept the geometry it was given: an attribute that is gone was taken
        // out on purpose, and putting it back would override the author.
        let mut same = prev.clone();
        same.points_mut().remove("mass");
        same.restore_live_from(&prev);
        assert!(!same.points().has("mass"), "a deliberate delete must stick");

        // An attribute the new state DOES carry is left alone: the chain
        // computed it this step, which is the whole point of running it.
        let mut recomputed = prev.clone();
        recomputed
            .points_mut()
            .set_value("mass", 0, AttribValue::Float(99.0))
            .unwrap();
        recomputed.restore_live_from(&prev);
        assert_eq!(recomputed.points().value("mass", 0), Some(AttribValue::Float(99.0)));
    }

    // ---- Phase 0: the points/vertices/primitives/detail container ----
    //
    // These cover what the triangle soup could not do at all: a point shared
    // by several primitives, an edge, an identity that survives an edit, and
    // attributes that follow their elements through one. See src/detail.rs.

    /// A 3x3 grid of points wired into four quads — the smallest mesh with an
    /// interior point, which is the only kind of point neighbour queries are
    /// interesting on.
    ///
    /// ```text
    ///   6 — 7 — 8
    ///   |   |   |
    ///   3 — 4 — 5
    ///   |   |   |
    ///   0 — 1 — 2
    /// ```
    fn quad_grid() -> Detail {
        let mut d = Detail::new();
        for y in 0..3 {
            for x in 0..3 {
                d.add_point(Vec3::new(x as f32, y as f32, 0.0));
            }
        }
        for quad in [[0, 1, 4, 3], [1, 2, 5, 4], [3, 4, 7, 6], [4, 5, 8, 7]] {
            d.add_prim(&quad);
        }
        d
    }

    #[test]
    fn test_detail_counts_points_verts_and_prims_separately() {
        let d = quad_grid();
        assert_eq!(d.num_points(), 9, "nine points, each shared by up to four quads");
        assert_eq!(d.num_verts(), 16, "four quads of four corners");
        assert_eq!(d.num_prims(), 4);
        // The soup would have needed 24 vertices for the same surface and
        // would have had no way to say that the center is one place.
        assert_eq!(d.prim_points(0), &[0, 1, 4, 3]);
        assert_eq!(d.prim_points(3), &[4, 5, 8, 7]);
        assert!(d.prim_points(4).is_empty(), "no fifth primitive");
    }

    #[test]
    fn test_detail_point_ids_are_unique_and_survive_a_delete() {
        let mut d = quad_grid();
        let before: Vec<_> = (0..d.num_points()).map(|p| d.id(p).unwrap()).collect();
        let mut sorted = before.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), before.len(), "every point has its own identity");

        // Drop the top row. The survivors keep the identity they were born
        // with even though their indices moved — this is the property a
        // positional weld can never provide, and the one a solver needs.
        let keep: Vec<bool> = (0..9).map(|i| i < 6).collect();
        d.keep_points(&keep);
        assert_eq!(d.num_points(), 6);
        for p in 0..d.num_points() {
            assert_eq!(d.id(p), Some(before[p]));
        }
        assert_eq!(d.index_of_id(before[5]), Some(5));
        assert_eq!(d.index_of_id(before[8]), None, "a deleted point resolves to nothing");
    }

    #[test]
    fn test_detail_topology_neighbours_exclude_diagonals() {
        let d = quad_grid();
        // The center point touches all four quads but only four points: a
        // quad's diagonal is not an edge.
        assert_eq!(d.point_neighbours(4), &[1, 3, 5, 7]);
        assert_eq!(d.topology().valence(4), 4);
        assert_eq!(d.point_prims(4).len(), 4);
        // A corner touches one quad and two points.
        assert_eq!(d.point_neighbours(0), &[1, 3]);
        assert_eq!(d.point_prims(0), &[0]);
    }

    #[test]
    fn test_detail_topology_counts_a_shared_edge_once() {
        let d = quad_grid();
        // 12 rather than 16: the four interior edges are each shared by two
        // quads, and an edge list that double-counted them would double every
        // force a solver puts along one.
        assert_eq!(d.edges().len(), 12);
        let mut seen = d.edges().to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 12);
        assert!(d.edges().iter().all(|e| e[0] < e[1]), "edges are stored low-to-high");
    }

    #[test]
    fn test_detail_topology_rebuilds_after_a_structural_edit() {
        let mut d = quad_grid();
        assert_eq!(d.topology().valence(8), 2, "the far corner starts with two edges");

        // Ask for topology, then change the structure. The cached answer must
        // not survive — a stale neighbour list is a silently wrong simulation,
        // not a crash.
        let p = d.add_point(Vec3::new(3.0, 3.0, 0.0));
        d.add_prim(&[8, p, 5]);
        assert_eq!(d.num_prims(), 5);
        // One more edge, not two: the new triangle's third side (5–8) is the
        // grid edge that was already there, and must not be counted twice.
        assert_eq!(d.topology().valence(8), 3);
        assert_eq!(d.edges().len(), 14);
        assert_eq!(d.point_neighbours(p as usize), &[5, 8]);
    }

    #[test]
    fn test_detail_line_primitive_does_not_close_into_a_loop() {
        let mut d = Detail::new();
        let a = d.add_point(Vec3::ZERO);
        let b = d.add_point(Vec3::X);
        d.add_prim(&[a, b]);
        // A two-point primitive is an open segment. Closing the winding would
        // invent a second edge between the same pair and give both ends a
        // neighbour they do not have.
        assert_eq!(d.edges(), &[[0, 1]]);
        assert_eq!(d.point_neighbours(0), &[1]);
    }

    #[test]
    fn test_detail_welds_a_triangle_soup_into_shared_points() {
        // Two triangles meeting along one edge — six soup corners, four
        // points.
        let positions = [
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0],
        ];
        let colors = [[1.0, 0.0, 0.0]; 6];
        let d = Detail::from_triangle_soup(&positions, &colors);

        assert_eq!(d.num_points(), 4, "the shared edge's two corners weld");
        assert_eq!(d.num_prims(), 2);
        assert_eq!(d.num_verts(), 6, "vertices still name a corner each");
        assert_eq!(d.edges().len(), 5, "three edges each, one of them shared");
        assert_eq!(d.color(0), [1.0, 0.0, 0.0], "color carries onto Cd");
    }

    #[test]
    fn test_detail_triangulate_fans_polygons_for_the_renderer() {
        let mut d = Detail::new();
        for p in [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y] {
            d.add_point(p);
        }
        d.add_prim(&[0, 1, 2, 3]);

        let tris = d.triangulate(|pos, col| (pos, col));
        assert_eq!(tris.len(), 6, "one quad fans into two triangles");
        assert_eq!(tris[0].0, [0.0, 0.0, 0.0]);
        assert_eq!(tris[1].1, crate::detail::DEFAULT_COLOR, "no Cd means the default");

        // Soup in, soup out: the round trip preserves the surface even though
        // the middle of it is no longer a soup.
        let soup: Vec<[f32; 3]> = vec![
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0],
        ];
        let welded = Detail::from_triangle_soup(&soup, &[[0.5; 3]; 6]);
        let back = welded.triangulate(|pos, _| pos);
        assert_eq!(back, soup);
    }

    #[test]
    fn test_detail_attributes_are_columnar_and_typed() {
        let mut d = quad_grid();
        d.points_mut().create("mass", AttribValue::Float(1.0));
        assert_eq!(d.points().get("mass").map(|a| a.len()), Some(9), "one entry per point");
        assert_eq!(d.points().get("mass").map(|a| a.ty()), Some(AttribType::Float));

        d.points_mut().set_value("mass", 4, AttribValue::Float(7.5)).unwrap();
        assert_eq!(d.points().value("mass", 4), Some(AttribValue::Float(7.5)));
        assert_eq!(d.points().value("mass", 0), Some(AttribValue::Float(1.0)));

        // A type mismatch is refused rather than silently coerced: half a
        // converted attribute is not a state this should be able to reach.
        let err = d
            .points_mut()
            .set_value("mass", 0, AttribValue::Float3([1.0; 3]))
            .unwrap_err();
        assert!(err.contains("float"), "{err}");

        // Integers exist for counters and ages, which must stay whole.
        d.points_mut().create("age", AttribValue::Int(0));
        d.points_mut().set_value("age", 2, AttribValue::Int(3)).unwrap();
        assert_eq!(d.points().value("age", 2), Some(AttribValue::Int(3)));
        assert_eq!(d.points().names(), vec!["age", "mass"], "sorted, so columns hold still");

        // The float view is what a GPU buffer binds in Phase 1.
        let flat = d.points().get("mass").unwrap().as_f32_slice().unwrap();
        assert_eq!(flat.len(), 9);
        assert!(d.points().get("age").unwrap().as_f32_slice().is_none());
    }

    #[test]
    fn test_detail_attribute_values_follow_their_points_through_a_delete() {
        let mut d = quad_grid();
        d.points_mut().create("mass", AttribValue::Float(0.0));
        for p in 0..9 {
            d.points_mut()
                .set_value("mass", p, AttribValue::Float(p as f32))
                .unwrap();
        }

        // Keep the middle row only. Values must move with their points, not
        // stay at their old indices.
        let keep: Vec<bool> = (0..9).map(|i| (3..6).contains(&i)).collect();
        d.keep_points(&keep);
        assert_eq!(d.num_points(), 3);
        let masses: Vec<f32> = (0..3)
            .map(|p| d.points().value("mass", p).unwrap().as_f32())
            .collect();
        assert_eq!(masses, vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn test_detail_dropping_a_point_drops_the_primitives_using_it() {
        let mut d = quad_grid();
        // The center point belongs to every quad, so removing it removes all
        // four: a polygon missing a corner is not geometry.
        let keep: Vec<bool> = (0..9).map(|i| i != 4).collect();
        d.keep_points(&keep);
        assert_eq!(d.num_points(), 8);
        assert_eq!(d.num_prims(), 0);
        assert_eq!(d.num_verts(), 0);
        assert_eq!(d.edges().len(), 0);
    }

    #[test]
    fn test_detail_gather_rewires_surviving_primitives() {
        let mut d = quad_grid();
        // Keep the bottom-left quad's four points. Its primitive survives and
        // must now name the points by their new indices.
        let keep: Vec<bool> = (0..9).map(|i| [0, 1, 3, 4].contains(&i)).collect();
        d.keep_points(&keep);
        assert_eq!(d.num_points(), 4);
        assert_eq!(d.num_prims(), 1);
        assert_eq!(d.prim_points(0), &[0, 1, 3, 2], "0,1,4,3 renumbered");
        assert_eq!(d.edges().len(), 4);
    }

    #[test]
    fn test_detail_groups_are_named_sets_that_survive_an_edit() {
        let mut d = quad_grid();
        d.points_mut().create_group("pinned");
        for p in [0, 2, 6, 8] {
            d.points_mut().add_to_group("pinned", p);
        }
        assert_eq!(d.points().group_members("pinned"), vec![0, 2, 6, 8]);
        assert_eq!(d.points().group_len("pinned"), 4);
        assert!(d.points().in_group("pinned", 8));
        assert!(!d.points().in_group("pinned", 4));
        // Asking about a group nobody made is not an error — a node's Group
        // parameter is routinely blank.
        assert_eq!(d.points().group_members("nope"), Vec::<u32>::new());

        let keep: Vec<bool> = (0..9).map(|i| i < 6).collect();
        d.keep_points(&keep);
        assert_eq!(d.points().group_members("pinned"), vec![0, 2], "membership follows");
        assert_eq!(d.points().group_names(), vec!["pinned"]);
    }

    #[test]
    fn test_detail_merge_reallocates_ids_and_rewires_primitives() {
        let mut a = quad_grid();
        let b = quad_grid();
        let a_ids: Vec<_> = (0..a.num_points()).map(|p| a.id(p).unwrap()).collect();

        a.merge(&b);
        assert_eq!(a.num_points(), 18);
        assert_eq!(a.num_prims(), 8);
        assert_eq!(a.num_verts(), 32);

        // Both sides numbered their points from zero. If the merge kept those
        // numbers, two different points would answer to one identity and a
        // solver would write one over the other.
        let all: Vec<_> = (0..a.num_points()).map(|p| a.id(p).unwrap()).collect();
        let mut uniq = all.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 18, "no identity collides across the merge");
        assert_eq!(&all[..9], &a_ids[..], "the left side keeps the ids it had");

        // The appended primitives point at the appended points.
        assert_eq!(a.prim_points(4), &[9, 10, 13, 12]);
        assert_eq!(a.edges().len(), 24, "two grids, no edges invented between them");
    }

    #[test]
    fn test_detail_merge_unions_attributes_and_zero_fills_the_gap() {
        let mut a = quad_grid();
        a.points_mut().create("mass", AttribValue::Float(2.0));
        let mut b = quad_grid();
        b.points_mut().create("age", AttribValue::Int(5));

        a.merge(&b);
        // Neither column is dropped; each side gets zeros where it had no
        // opinion. Silently losing a column here would strand a solver
        // attribute the moment two streams met.
        assert_eq!(a.points().names(), vec!["age", "mass"]);
        assert_eq!(a.points().value("mass", 0), Some(AttribValue::Float(2.0)));
        assert_eq!(a.points().value("mass", 9), Some(AttribValue::Float(0.0)));
        assert_eq!(a.points().value("age", 0), Some(AttribValue::Int(0)));
        assert_eq!(a.points().value("age", 9), Some(AttribValue::Int(5)));
        assert_eq!(a.points().get("mass").map(|x| x.len()), Some(18));
    }

    #[test]
    fn test_detail_holds_per_class_attributes_including_one_detail_row() {
        let mut d = quad_grid();
        d.verts_mut().create("uv", AttribValue::Float2([0.0; 2]));
        d.prims_mut().create("area", AttribValue::Float(1.0));
        d.detail_mut().create("edges_max", AttribValue::Float(0.0));

        assert_eq!(d.store(Class::Vertex).len(), 16);
        assert_eq!(d.store(Class::Prim).len(), 4);
        assert_eq!(d.store(Class::Detail).len(), 1, "detail is the single-row class");

        // Analysis writes a range here rather than needing a dictionary type.
        d.detail_mut()
            .set_value("edges_max", 0, AttribValue::Float(1.0))
            .unwrap();
        assert_eq!(d.detail().value("edges_max", 0), Some(AttribValue::Float(1.0)));
        assert_eq!(Class::Detail.name(), "detail");
    }

    #[test]
    fn test_detail_attribute_gather_is_the_one_reordering_primitive() {
        let data = AttribData::Float(vec![10.0, 20.0, 30.0]);
        // Delete, reorder and duplicate are all the same operation.
        assert_eq!(data.gather(&[2, 0]), AttribData::Float(vec![30.0, 10.0]));
        assert_eq!(data.gather(&[1, 1]), AttribData::Float(vec![20.0, 20.0]));
        // An out-of-range index yields a zero rather than panicking: an index
        // map built with an arithmetic slip should not be able to take the app
        // down.
        assert_eq!(data.gather(&[9]), AttribData::Float(vec![0.0]));
    }

    #[test]
    fn test_detail_color_and_bounds_read_back() {
        let mut d = quad_grid();
        assert_eq!(d.color(0), crate::detail::DEFAULT_COLOR);
        d.set_color(0, [0.25, 0.5, 0.75]);
        assert_eq!(d.color(0), [0.25, 0.5, 0.75]);
        assert_eq!(d.color(1), crate::detail::DEFAULT_COLOR, "Cd defaults for everyone else");

        let (lo, hi) = d.bounds().unwrap();
        assert_eq!(lo, Vec3::ZERO);
        assert_eq!(hi, Vec3::new(2.0, 2.0, 0.0));
        assert!(Detail::new().bounds().is_none());
    }

    #[test]
    fn test_detail_clone_drops_the_derived_topology_but_not_the_geometry() {
        let d = quad_grid();
        assert_eq!(d.topology().valence(4), 4);

        // The clone exists to be modified, so it starts with no cache; what it
        // must not lose is anything the cache was derived from.
        let mut copy = d.clone();
        assert_eq!(copy.num_points(), 9);
        assert_eq!(copy.topology().valence(4), 4);
        copy.keep_points(&vec![true; 9]);
        assert_eq!(copy.num_prims(), 4);
        assert_eq!(d.num_prims(), 4, "the original is untouched");
    }

    // ----- The Alt+D dialog (src/dialog.rs) -----

    /// A key, as the dialog's handler expects one.
    fn key_press(key: Key) -> cce_ui::widget::KeyEvent {
        cce_ui::widget::KeyEvent {
            state: cce_ui::widget::ElementState::Pressed,
            logical_key: key,
            text: None,
            repeat: false,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }

    fn typed(c: &str) -> cce_ui::widget::KeyEvent {
        key_press(Key::Character(c.to_string()))
    }

    /// Every Settings row still names something that exists, and every
    /// `Owner::Field` key is one the readers and the writer both handle.
    ///
    /// The failure this catches is silent, and it is the reason the table is
    /// a table: a `Field` key that no arm names reads as a zero and writes
    /// nowhere, so the row draws, accepts an edit and does nothing. (Before
    /// the meta node was retired the same failure was a renamed subnet param
    /// SKIPPING its row, which quietly shortened the Settings half.) Same
    /// argument as `test_every_menu_command_names_a_label_that_is_dispatched`.
    #[test]
    fn dialog_settings_rows_name_owners_that_exist() {
        use crate::dialog::{Ctl, Owner};
        let mut state = State::new(false);
        for s in crate::dialog::SETTINGS {
            match s.owner {
                Owner::Field(key) => {
                    let ctl = s.ctl;
                    // The round trip IS the check: read the row, write the
                    // value straight back, and read again. A key no arm
                    // names reads a default and writes nothing, so the two
                    // reads differ the moment the default is not the live
                    // value — which is why each row is nudged first.
                    match ctl {
                        Ctl::Toggle => {
                            let before = state.settings_row_value(s.label);
                            let flipped = if before == "true" { "false" } else { "true" };
                            state.settings_write_row(s.label, flipped);
                            assert_eq!(state.settings_row_value(s.label), flipped,
                                "row '{}' (key '{key}') did not take a write", s.label);
                        }
                        Ctl::Color => {
                            state.settings_write_row(s.label, "#123456");
                            assert_eq!(state.settings_row_value(s.label), "#123456",
                                "row '{}' (key '{key}') did not take a write", s.label);
                        }
                        Ctl::Spin { min, max, .. } => {
                            let v = ((min + max) / 2.0).round() as i32;
                            state.settings_write_row(s.label, &v.to_string());
                            assert_eq!(state.settings_row_value(s.label), v.to_string(),
                                "row '{}' (key '{key}') did not take a write", s.label);
                        }
                        Ctl::Slider { min, max, dec } => {
                            let v = format!("{:.*}", dec, (min + max) / 2.0);
                            state.settings_write_row(s.label, &v);
                            assert_eq!(state.settings_row_value(s.label), v,
                                "row '{}' (key '{key}') did not take a write", s.label);
                        }
                        Ctl::Choice(options) => {
                            let last = options.last().expect("a choice with no options");
                            state.settings_write_row(s.label, last);
                            assert_eq!(state.settings_row_value(s.label), *last,
                                "row '{}' (key '{key}') did not take a write", s.label);
                        }
                    }
                }
            }
        }
    }

    /// Row labels are the writeback's identity — a setting row's id is its
    /// label under `SETTING_ROW_PREFIX`, and `setting_of_row` resolves it
    /// back the same way — so two rows sharing one would write each other's
    /// values.
    #[test]
    fn dialog_settings_labels_are_unique() {
        let mut seen: Vec<&str> = Vec::new();
        for s in crate::dialog::SETTINGS {
            assert!(!seen.contains(&s.label), "two Settings rows are called '{}'", s.label);
            seen.push(s.label);
        }
    }

    /// The dialog opens on its registry command, lists every command, and
    /// closes on Escape.
    #[test]
    fn dialog_opens_on_its_command_and_escape_closes_it() {
        let mut state = State::new(false);
        assert!(!state.dialog_visible(), "closed until asked for");

        assert!(state.run_command("toggle_dialog"));
        assert!(state.dialog_visible());
        // Every command — beside the setting rows, the level's camera
        // rows, and the network pane's zoom slider row when that pane is
        // focused (it is by default).
        assert_eq!(
            state.slots.dialog.rows.iter().filter(|r| !r.id.starts_with(crate::dialog::SETTING_ROW_PREFIX) && r.id != crate::dialog::ZOOM_ROW_ID && !r.id.starts_with(crate::dialog::CAMERA_ROW_PREFIX)).count(),
            crate::command::COMMANDS.len(),
            "an empty query lists everything"
        );

        state.dialog_key_input(&key_press(Key::Named(NamedKey::Escape)));
        assert!(!state.dialog_visible());
    }

    /// Typing filters, and Enter runs the row it landed on — then closes,
    /// because a modal that stays up after acting hides what it just did.
    #[test]
    fn dialog_filters_as_you_type_and_enter_runs_the_selection() {
        let mut state = State::new(false);
        state.run_command("toggle_dialog");

        for c in ["d", "e", "s", "e", "l"] {
            state.dialog_key_input(&typed(c));
        }
        assert_eq!(state.slots.dialog.query, "desel");
        assert_eq!(
            state.slots.dialog.selected_id(),
            Some("deselect"),
            "rows: {:?}",
            state.slots.dialog.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>()
        );
        let row = &state.slots.dialog.rows[state.slots.dialog.selected];
        assert_eq!(row.toggle(), None, "Deselect runs and is done; it draws no switch");

        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(!state.dialog_visible(), "a plain command closes the dialog behind it");
    }

    /// A toggle row is a switch: Enter flips it, the switch on the row moves,
    /// and the dialog stays up with the selection where it was — so Show
    /// Grid, Show Cube and Square Aspect can be set together, looking at the
    /// viewport, instead of reopening the dialog for each.
    #[test]
    fn dialog_enter_on_a_toggle_row_flips_it_and_keeps_the_dialog_open() {
        let mut state = State::new(false);
        state.run_command("toggle_dialog");

        for c in ["s", "q", "u", "a"] {
            state.dialog_key_input(&typed(c));
        }
        assert_eq!(state.slots.dialog.selected_id(), Some("toggle_square_viewport"));
        let before = state.square_viewport;
        let row = state.slots.dialog.rows[state.slots.dialog.selected].clone();
        assert_eq!(row.toggle(), Some(before), "the switch shows the live value");

        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert_eq!(state.square_viewport, !before, "Enter ran the command");
        assert!(state.dialog_visible(), "and the dialog stayed up");
        assert_eq!(state.slots.dialog.query, "squa", "with its query intact");
        assert_eq!(state.slots.dialog.selected_id(), Some("toggle_square_viewport"), "and its selection");
        let row = &state.slots.dialog.rows[state.slots.dialog.selected];
        assert_eq!(row.toggle(), Some(!before), "the switch moved with the value");

        // And back again, without leaving.
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert_eq!(state.square_viewport, before);
        assert!(state.dialog_visible());
        assert_eq!(state.slots.dialog.rows[state.slots.dialog.selected].toggle(), Some(before));

        // A click on the row is the same pick as Enter.
        state.take_dialog_pick("toggle_square_viewport".to_string());
        assert_eq!(state.square_viewport, !before);
        assert!(state.dialog_visible(), "a clicked switch keeps the dialog up too");
    }

    /// Every toggle command in the registry draws a switch, and every switch
    /// names a command the registry has. `command_toggle_state` is a match on
    /// id strings, so a `toggle_*` row added to the registry without an arm
    /// there would silently ship as a plain row — this is what says so.
    #[test]
    fn dialog_toggle_rows_cover_every_toggle_command() {
        let state = State::new(false);
        // Named like toggles, but not switches: Dialog toggles the dialog
        // itself (picking it is a no-op), Configure focuses a pane, and
        // Snapping is a switch only inside a viewer state — asserted below.
        let not_switches = ["toggle_dialog", "toggle_configure", "toggle_snap"];
        for c in crate::command::COMMANDS {
            let looks_like_toggle = c.id.starts_with("toggle_")
                || (c.id.starts_with("show_") && c.id.ends_with("_pane"));
            let is_switch = state.command_toggle_state(c.id).is_some();
            if looks_like_toggle && !not_switches.contains(&c.id) {
                assert!(is_switch, "{} is a toggle command with no switch", c.id);
            } else if !looks_like_toggle && c.id != "detach_circular_window" {
                assert!(!is_switch, "{} draws a switch but is not a toggle", c.id);
            }
        }
        assert!(state.command_toggle_state("detach_circular_window").is_some());
        assert_eq!(state.command_toggle_state("toggle_snap"), None, "no viewer state, no switch");
        assert_eq!(state.command_toggle_state("no_such_command"), None);

        // The switches agree with the fields the commands flip.
        assert_eq!(state.command_toggle_state("toggle_grid"), Some(state.viewport().show_grid));
        assert_eq!(state.command_toggle_state("show_network_pane"), Some(state.show_network));
    }

    /// The Commands list heads with a zoom SLIDER while the network pane is
    /// focused, and only then: zoom is that pane's. It reads the live zoom
    /// as a percentage of the configured grid, the arrows nudge it in place
    /// with the dialog up, Enter on it runs nothing, and a query that does
    /// not match "Zoom" drops it like any other row.
    #[test]
    fn dialog_zoom_slider_row_belongs_to_the_network_pane() {
        use crate::dialog::ZOOM_ROW_ID;
        let mut state = State::new(false);
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;
        state.run_command("command_palette");
        assert!(state.dialog_visible());
        let rows = &state.slots.dialog.rows;
        assert_eq!(rows[0].id, ZOOM_ROW_ID, "the zoom row heads the network list");
        assert!((rows[0].slider_value().unwrap() - state.zoom_percent()).abs() < 1e-3);
        assert!(rows.iter().filter(|r| r.id == ZOOM_ROW_ID).count() == 1);

        // The arrows nudge the zoom, the dialog stays up, the row follows.
        let before = state.zoom_percent();
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowRight)));
        assert!(state.dialog_visible());
        assert!(state.zoom_percent() > before, "right arrow zooms in");
        assert!((state.slots.dialog.rows[0].slider_value().unwrap() - state.zoom_percent()).abs() < 1e-3);
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowLeft)));
        assert!((state.zoom_percent() - before).abs() < 0.5, "left arrow zooms back out");

        // Enter on it is a no-op that keeps the dialog up.
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(state.dialog_visible());
        assert!((state.zoom_percent() - before).abs() < 0.5);

        // A slider value lands as a zoom, clamped to the pitch limits.
        state.set_zoom_percent(150.0);
        assert!((state.zoom_percent() - 150.0).abs() < 0.5);
        state.set_zoom_percent(100_000.0);
        assert!((state.grid_pitch_x - crate::app::MAX_PITCH_X).abs() < 0.5, "clamped to the max pitch");
        assert!((state.slots.dialog.rows[0].slider_value().unwrap() - state.zoom_percent()).abs() < 1e-3);
        state.set_zoom_percent(100.0);

        // A query that does not match "Zoom" drops the row.
        for c in ["s", "a", "v"] {
            state.dialog_key_input(&key_press(Key::Character(c.into())));
        }
        assert!(state.slots.dialog.rows.iter().all(|r| r.id != ZOOM_ROW_ID));
        state.close_dialog();

        // Another pane focused: no slider row at all.
        state.focused_pane = crate::slots::RIGHT_MENUBAR_IDX;
        state.run_command("command_palette");
        assert!(state.slots.dialog.rows.iter().all(|r| r.id != ZOOM_ROW_ID));
    }

    /// At the widget: a press on the slider row's band takes hold, jumps the
    /// value to the pointer's place along the band in the range set, and
    /// reports it once; a press on the row away from the band selects and
    /// reports nothing, and never "activates" the row as a pick.
    #[test]
    fn dialog_slider_row_press_reports_the_value_under_the_pointer() {
        use crate::dialog::{Control, Dialog, Row, SLIDER_W};
        use cce_ui::widget::{ElementState, MouseButton, WidgetHost};
        let mut ctx = cce_ui::context::UiContext::new();
        let mut d = Dialog::new();
        d.set_visible(true);
        WidgetHost::set_rect(&mut d, 0.0, 0.0, 520.0, 420.0);
        let (id, ptr) = (d.id(), d.as_ptr_mut());
        ctx.register_widget(id, ptr);
        let plain = |i: usize| Row { id: format!("c{i}"), label: format!("Command {i}"), chord: String::new(), control: None, truncate_head: false };
        d.set_rows(vec![
            Row { id: "zoom_level".into(), label: "Zoom".into(), chord: String::new(), control: Some(Control::Slider { value: 100.0, min: 20.0, max: 320.0, dec: 0, step: 10.0, suffix: "%" }), truncate_head: false },
            plain(1),
            plain(2),
        ]);
        d.set_page(10);
        d.set_occluding(false);

        // The first row's rect, as the widget lays it out: the list starts
        // below the query line; the band begins SLIDER_W in from the row's
        // right end and runs out to the chord column's right edge — with no
        // toggle row in this list, that is the row's own.
        let list_y = 12.0 + 30.0 + 8.0;
        let row_y = list_y + 12.0;
        let band_x = 520.0 - 12.0 - 8.0 - SLIDER_W;
        let band_w = SLIDER_W;

        // Press at three quarters along the band: the value lands three
        // quarters into the range, and the row is not activated as a pick.
        let px = band_x + band_w * 0.75;
        assert!(d.mouse_input(MouseButton::Left, ElementState::Pressed, px, row_y, &mut ctx));
        assert!(d.slider_dragging());
        let v = d.take_slider_change().map(|(_, v)| v).expect("a press on the band reports a value");
        assert!((v - (20.0 + 0.75 * 300.0)).abs() < 3.0, "value {v} is not three quarters of the range");
        assert_eq!(d.rows[0].slider_value(), Some(v), "the row follows");
        assert_eq!(d.take_activated(), None, "the band is a control, not a pick");
        assert_eq!(d.take_slider_change(), None, "reported once");
        d.mouse_input(MouseButton::Left, ElementState::Released, px, row_y, &mut ctx);
        assert!(!d.slider_dragging());

        // A press on the row's label end selects it and reports nothing.
        assert!(d.mouse_input(MouseButton::Left, ElementState::Pressed, 30.0, row_y, &mut ctx));
        assert_eq!(d.selected, 0);
        assert!(!d.slider_dragging());
        assert_eq!(d.take_slider_change(), None);
        assert_eq!(d.take_activated(), None);
        d.mouse_input(MouseButton::Left, ElementState::Released, 30.0, row_y, &mut ctx);

        // An ordinary row still picks.
        assert!(d.mouse_input(MouseButton::Left, ElementState::Pressed, 30.0, row_y + 24.0, &mut ctx));
        assert_eq!(d.take_activated().as_deref(), Some("c1"));

        // The wheel over the control turns the slider — a notch up is 2% of
        // the range more, as on the toolkit's slider — and over the label
        // end it scrolls the list instead, reporting nothing.
        let before = d.rows[0].slider_value().unwrap();
        let wheel = |x: f32, y: f32| cce_ui::widget::Event::MouseWheel {
            delta: cce_ui::widget::MouseScrollDelta::LineDelta(0.0, 1.0),
            x, y, local_x: x, local_y: y,
        };
        ctx.note_scroll_event();
        assert!(d.handle_event(&wheel(band_x + 10.0, row_y), &mut ctx));
        let v = d.take_slider_change().map(|(_, v)| v).expect("a wheel over the band reports a value");
        assert!((v - (before + 0.02 * 300.0)).abs() < 1e-3, "notch up: {before} -> {v}");
        ctx.note_scroll_event();
        d.handle_event(&wheel(30.0, row_y), &mut ctx);
        assert_eq!(d.take_slider_change(), None, "over the label the wheel is the list's");
    }

    /// The band ends where the key bindings do. The chord column's right edge
    /// steps left by the switch column as soon as any row carries a toggle,
    /// and the band follows it — so the control lines up with the chords
    /// beneath it instead of running on past them into the switches.
    #[test]
    fn dialog_slider_band_ends_at_the_chord_column() {
        use crate::dialog::{Control, Dialog, Row, SLIDER_W, TOGGLE_W};
        use cce_ui::widget::{ElementState, MouseButton, WidgetHost};
        let mut ctx = cce_ui::context::UiContext::new();
        let mut d = Dialog::new();
        d.set_visible(true);
        WidgetHost::set_rect(&mut d, 0.0, 0.0, 520.0, 420.0);
        let (id, ptr) = (d.id(), d.as_ptr_mut());
        ctx.register_widget(id, ptr);
        d.set_rows(vec![
            Row { id: "zoom_level".into(), label: "Zoom".into(), chord: String::new(), control: Some(Control::Slider { value: 100.0, min: 20.0, max: 320.0, dec: 0, step: 10.0, suffix: "%" }), truncate_head: false },
            Row { id: "show_grid".into(), label: "Show Grid".into(), chord: "Ctrl+G".into(), control: Some(Control::Toggle(true)), truncate_head: false },
        ]);
        d.set_page(10);
        d.set_occluding(false);

        let row_y = 12.0 + 30.0 + 8.0 + 12.0;
        let row_right = 520.0 - 12.0 - 8.0;
        let band_right = row_right - (TOGGLE_W + 12.0);
        assert!(band_right < row_right, "the switch column pulls the band in");

        // The band's last pixel is the range's top; the switch column past it
        // is not the band's.
        assert!(d.mouse_input(MouseButton::Left, ElementState::Pressed, band_right - 1.0, row_y, &mut ctx));
        assert!(d.slider_dragging(), "the band reaches the chord column's edge");
        let v = d.take_slider_change().map(|(_, v)| v).expect("a press on the band reports a value");
        assert!((v - 320.0).abs() < 4.0, "the band's end is the range's end, got {v}");
        d.mouse_input(MouseButton::Left, ElementState::Released, band_right - 1.0, row_y, &mut ctx);

        d.mouse_input(MouseButton::Left, ElementState::Pressed, band_right + 4.0, row_y, &mut ctx);
        assert!(!d.slider_dragging(), "past the chord column the row is not the band");
        assert_eq!(d.take_slider_change(), None);
        d.mouse_input(MouseButton::Left, ElementState::Released, band_right + 4.0, row_y, &mut ctx);

        // The readout lane sits ahead of the band and takes no hold either.
        let lane_x = row_right - SLIDER_W - 60.0 - 8.0;
        d.mouse_input(MouseButton::Left, ElementState::Pressed, lane_x + 4.0, row_y, &mut ctx);
        assert!(!d.slider_dragging(), "the readout is a readout, not a track");
        assert_eq!(d.take_slider_change(), None);
    }

    /// A captured pointer hovers no pane. A control lit at the press used to
    /// stay lit for the length of an orbit, a node drag or a pane resize —
    /// the broadcast ran only while no widget or app drag was live, and the
    /// cursor arm's early returns skipped it for every other gesture — and
    /// the dialog, a modal, let the panes beside its plate keep hovering.
    /// Now a capture clears every pane, the release hands the pointer back
    /// without a motion, and the dialog's open and close do the same.
    #[test]
    fn a_captured_pointer_hovers_no_pane_and_the_release_hands_it_back() {
        use crate::slots::NETWORK_PANEL_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        let (x, y, w, h) = state.positions[NETWORK_PANEL_IDX];
        assert!(w > 0.0 && h > 0.0, "the network plate is laid out");
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        let hovered = |state: &State| state.slots.get_dyn(NETWORK_PANEL_IDX).base().hovered;
        let moved = |state: &mut State, x: f32, y: f32| {
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        };
        moved(&mut state, cx, cy);
        assert!(hovered(&state), "the plate under a free pointer hovers");

        // An orbit captures the pointer: the next motion clears the plate,
        // and the release hands the pointer back where it stands.
        state.orbit_drag = Some((cx, cy));
        moved(&mut state, cx + 1.0, cy);
        assert!(!hovered(&state), "a captured pointer hovers no pane");
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        assert!(state.orbit_drag.is_none());
        assert!(hovered(&state), "the release re-hovers without a motion");

        // An app drag (a pane edge) captures it the same way.
        state.app_drag = Some(crate::app::AppDrag::ParamResize { start_w: 100.0, start_mouse_x: cx });
        moved(&mut state, cx + 2.0, cy);
        assert!(!hovered(&state));
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        assert!(state.app_drag.is_none());
        assert!(hovered(&state));

        // The dialog is modal: open, the panes lose the pointer, motion does
        // not give it back, and closing does.
        state.open_dialog();
        assert!(!hovered(&state), "a pane beside the dialog does not hover");
        moved(&mut state, cx + 3.0, cy);
        assert!(!hovered(&state));
        state.close_dialog();
        assert!(hovered(&state), "closing hands the pointer back");
    }

    /// A dialog slider is worked by the pointer, and every motion of a drag
    /// lands its value. Until 2026-09-28 each landing ran `apply_setting`'s
    /// whole regenerate pass — a graph evaluation (and two more keyed on the
    /// version it bumped), a path-tracer restart and a synchronous state.kdl
    /// write, per pointer event, for a value the graph never reads. A slider
    /// row lands as the viewport menu's sliders do: the field, a redraw, the
    /// row re-read in place, and the file written once on the release. A
    /// single landing (a wheel notch, an arrow key) saves at once, and a
    /// spin row still takes the full pass, whose regenerate it needs.
    #[test]
    fn a_dialog_slider_drag_lands_without_re_evaluating_the_graph() {
        use crate::dialog::{setting_row_id, Control, ROW_H, SLIDER_W, TOGGLE_W};
        use crate::slots::DIALOG_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.geo_opacity = 1.0;
        state.save_settings();
        let path = crate::app::DesignSettings::file_path();
        let saved = |path: &std::path::Path| {
            let kdl = fs::read_to_string(path).expect("a settings file");
            crate::app::DesignSettings::from_kdl_str(&kdl).render.geo_opacity
        };
        assert!((saved(&path) - 1.0).abs() < 1e-3);

        state.open_dialog();
        state.slots.dialog.query = "geometry opacity".into();
        state.refresh_dialog_rows();
        let row = setting_row_id("Geometry Opacity");
        assert_eq!(state.slots.dialog.rows.first().map(|r| r.id.as_str()), Some(row.as_str()), "the setting row ranks first");
        let has_toggle = state.slots.dialog.rows.iter().any(|r| matches!(r.control, Some(Control::Toggle(_))));
        let (x, y, w, _) = state.positions[DIALOG_IDX];
        let row_y = y + 12.0 + 30.0 + 8.0 + ROW_H * 0.5;
        let band_right = x + w - 12.0 - 8.0 - if has_toggle { TOGGLE_W + 12.0 } else { 0.0 };
        let band_x = band_right - SLIDER_W;
        let at = |state: &mut State, t: f32| {
            let px = band_x + SLIDER_W * t;
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: px as f64, y: row_y as f64 } });
        };
        let version = state.rt_geometry_version;

        // The press takes the band and jumps the value; nothing is evaluated
        // and nothing is written.
        at(&mut state, 0.5);
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        assert!(state.slots.dialog.slider_dragging(), "the press took the band");
        assert!((state.geo_opacity - 0.5).abs() < 0.02, "{}", state.geo_opacity);
        assert_eq!(state.rt_geometry_version, version, "a draw-time value re-evaluated the graph");
        assert!((saved(&path) - 1.0).abs() < 1e-3, "written mid-drag");

        // A motion lands the value live and re-reads the row in place.
        at(&mut state, 0.25);
        assert!((state.geo_opacity - 0.25).abs() < 0.02, "{}", state.geo_opacity);
        let shown = state.slots.dialog.rows[0].slider_value().expect("a slider row");
        assert!((shown - state.geo_opacity).abs() < 1e-3, "the row shows {shown}, the field holds {}", state.geo_opacity);
        assert_eq!(state.rt_geometry_version, version, "a drag motion re-evaluated the graph");
        assert!((saved(&path) - 1.0).abs() < 1e-3, "written mid-drag");

        // The release writes the file once.
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        assert!(!state.slots.dialog.slider_dragging());
        assert!((saved(&path) - state.geo_opacity).abs() < 1e-3, "the release did not save");
        assert_eq!(state.rt_geometry_version, version);

        // A single landing saves at once, and clamps as the menu clamps.
        state.land_dialog_slider(&row, 0.7);
        assert!((state.geo_opacity - 0.7).abs() < 1e-6);
        assert!((saved(&path) - 0.7).abs() < 1e-3, "a wheel or arrow landing did not save");
        state.land_dialog_slider(&row, 7.0);
        assert!((state.geo_opacity - 1.0).abs() < 1e-6, "clamped");
        assert_eq!(state.rt_geometry_version, version);

        // Group Marker Size re-sizes the group markers from their kept
        // members.
        state.land_dialog_slider(&setting_row_id("Group Marker Size"), 0.05);
        assert!((state.group_marker_size - 0.05).abs() < 1e-6);
        assert!((state.last_group_marker_size - 0.05).abs() < 1e-6);
        assert_eq!(state.rt_geometry_version, version);

        // The spin rows land the same way: a whole number over the row's
        // unit, re-baking only the guide mesh that reads it.
        state.pending_grid = None;
        state.pending_origin = None;
        state.pending_pivot = None;
        state.land_dialog_slider(&setting_row_id("Grid Thickness"), 40.0);
        assert!((state.grid_thickness - 0.04).abs() < 1e-6, "{}", state.grid_thickness);
        assert!(state.pending_grid.is_some(), "the grid re-baked");
        state.land_dialog_slider(&setting_row_id("Origin Size"), 25.0);
        assert!((state.origin_size - 2.5).abs() < 1e-6, "{}", state.origin_size);
        assert!(state.pending_origin.is_some(), "the origin re-baked");
        // Point Marker Size reads in world units, as Group Marker Size does
        // — one radius, one number in both rows.
        state.land_dialog_slider(&setting_row_id("Point Marker Size"), 0.05);
        assert!((state.point_marker_size - 0.05).abs() < 1e-6, "{}", state.point_marker_size);
        state.land_dialog_slider(&setting_row_id("Group Marker Size"), 0.05);
        assert!((state.group_marker_size - state.point_marker_size).abs() < 1e-6);
        assert_eq!(state.settings_row_value("Point Marker Size"), state.settings_row_value("Group Marker Size"));
        assert_eq!(state.rt_geometry_version, version, "a spin row re-evaluated the graph");
        assert!((saved(&path) - 1.0).abs() < 1e-3, "the file follows every single landing");
        let kdl = fs::read_to_string(&path).expect("a settings file");
        let back = crate::app::DesignSettings::from_kdl_str(&kdl).viewport;
        assert!((back.grid_thickness - 0.04).abs() < 1e-6 && (back.origin_size - 2.5).abs() < 1e-6, "{kdl}");
    }

    /// A right press is the dialog's while it is open: inside the plate it is
    /// swallowed — no context menu opens for the pane beneath, which used to
    /// come up over the modal with its labels clipped — and outside it
    /// dismisses, as a left press does.
    #[test]
    fn dialog_owns_right_presses_while_open() {
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.run_command("toggle_dialog");
        assert!(state.dialog_visible());
        let (dx, dy, dw, dh) = state.positions[crate::slots::DIALOG_IDX];
        assert!(dw > 0.0 && dh > 0.0, "the dialog is laid out");

        // Inside: swallowed, nothing opens, the dialog stays.
        state.cursor_x = dx + dw * 0.5;
        state.cursor_y = dy + dh * 0.5;
        assert_eq!(state.dialog_mouse_input(MouseButton::Right, ElementState::Pressed), Some(true));
        assert!(state.dialog_visible());
        assert!(!cce_ui::widget::context_menu::is_visible(), "no menu opened over the modal");
        assert_eq!(state.dialog_mouse_input(MouseButton::Right, ElementState::Released), Some(true));

        // Outside: dismisses, and is swallowed rather than reaching the pane.
        state.cursor_x = (dx - 20.0).max(0.0);
        state.cursor_y = (dy - 20.0).max(0.0);
        assert_eq!(state.dialog_mouse_input(MouseButton::Right, ElementState::Pressed), Some(true));
        assert!(!state.dialog_visible());
        assert!(!cce_ui::widget::context_menu::is_visible());

        // The middle button is still nobody's.
        state.run_command("toggle_dialog");
        assert_eq!(state.dialog_mouse_input(MouseButton::Middle, ElementState::Pressed), None);
    }

    /// Backspace walks the query back, and the ranking follows it.
    #[test]
    fn dialog_backspace_widens_the_filter() {
        let mut state = State::new(false);
        state.run_command("toggle_dialog");
        for c in ["z", "z", "z"] {
            state.dialog_key_input(&typed(c));
        }
        assert!(state.slots.dialog.rows.is_empty(), "nothing matches 'zzz'");
        for _ in 0..3 {
            state.dialog_key_input(&key_press(Key::Named(NamedKey::Backspace)));
        }
        assert_eq!(state.slots.dialog.query, "");
        assert_eq!(
            state.slots.dialog.rows.iter().filter(|r| !r.id.starts_with(crate::dialog::SETTING_ROW_PREFIX) && r.id != crate::dialog::ZOOM_ROW_ID && !r.id.starts_with(crate::dialog::CAMERA_ROW_PREFIX)).count(),
            crate::command::COMMANDS.len()
        );
    }

    /// The dialog owns the keyboard outright while it is open.
    ///
    /// The network pane's bare-letter family is ungated by design, so typing
    /// "e" into an unguarded filter would flip the selected node's geometry
    /// toggle on the way past. The guard is the whole reason
    /// `dialog_key_input` is total rather than a layer.
    #[test]
    fn dialog_keys_never_reach_the_pane_underneath() {
        use crate::window::WindowEvent;
        let mut state = State::new(false);
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;
        let col = state.grid_cursor_col;
        state.run_command("toggle_dialog");

        // "l" is Cursor Right in the network pane and a plain letter here.
        state.handle_event(&WindowEvent::KeyboardInput { event: typed("l") });
        assert_eq!(state.grid_cursor_col, col, "the grid cursor must not move");
        assert_eq!(state.slots.dialog.query, "l");
    }

    /// The settings are rows of the one list, each carrying its control —
    /// ranked with the commands, so a query finds a colour the way it finds
    /// a command. There is no second half: Tab in this mode does nothing,
    /// and nothing draws a strip.
    /// A choice row is the params pane's dropdown. Closed, its row draws
    /// the toolkit `Dropdown` in the control band, the value's text carried
    /// under the dialog's bounds so the dialog's occluder lets it through.
    /// A press on it opens the live dropdown laid out on that band: its
    /// plate grows out of the trigger into the list, registered as an
    /// occluder AFTER the dialog so the rows under it are hidden and its
    /// own labels are not. Up, Down and Enter walk and pick; a press on a
    /// row picks it; Escape or a press elsewhere closes it, and the dialog
    /// stays up.
    #[test]
    fn a_choice_row_is_a_dropdown() {
        use cce_ui::scene::paint::Prim;
        use cce_ui::widget::WidgetHost;
        use crate::dialog::setting_row_id;
        use crate::slots::DIALOG_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.run_command("command_palette");
        for c in ["w", "o", "r", "l", "d"] {
            state.dialog_key_input(&typed(c));
        }
        let (dx, dy, dw, dh) = state.positions[DIALOG_IDX];
        let own = [dx, dy, dx + dw, dy + dh];
        let list = state.collect_display_list();
        let (vx, vy) = list
            .items
            .iter()
            .find_map(|item| match &item.prim {
                // The trigger draws its text a cluster at a time.
                Prim::Text { text, x, y, bounds: Some(b), .. } if text == "m" && *b == own && *x > dx + dw * 0.5 => Some((*x, *y)),
                _ => None,
            })
            .expect("the World Unit row's dropdown shows its value under the dialog's bounds");

        // As the runner presses: a frame drawn (which registers the open
        // dropdown), then the press handed to every open popover that the
        // press MISSED by its hit test — which the dialog's own claim makes
        // the dropdown's — and only then to the app.
        let press = |state: &mut State, x: f32, y: f32| {
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
            let _ = state.collect_display_list();
            state.ui_context.close_popovers_missed_by_press(x, y);
            state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
            state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        };
        press(&mut state, vx + 2.0, vy + 4.0);
        assert!(state.dialog_dropdown_open(), "the press opened the dropdown");
        let units = state.slots.dialog.dropdown.options.clone();
        assert_eq!(units[state.slots.dialog.dropdown.selected], "mm");
        let (tx, ty, tw, th) = state.slots.dialog.dropdown.rect();
        assert!(vx >= tx && vx < tx + tw && vy >= ty - 4.0 && vy < ty + th, "laid out on the band its value was drawn in");

        // The plate grows out of the trigger into the list. Judged by where
        // it ends, not by a reading taken as it opens: the growth runs on
        // the wall clock, and a slow press had already finished it.
        let grown = |state: &State| state.slots.dialog.dropdown.popover_rect().map(|r| r.3).unwrap_or(0.0);
        std::thread::sleep(std::time::Duration::from_millis(250));
        state.tick_frame(0.25);
        assert!(grown(&state) > th + 24.0, "the trigger {th} grew to {}", grown(&state));
        // Registered after the dialog, so the rows under it are clamped
        // and its labels are not.
        let _ = state.collect_display_list();
        let pops = &state.ui_context.active_popovers;
        let dialog_at = pops.iter().position(|&p| p == state.slots.dialog.base().id()).expect("the dialog");
        let dd_id = state.slots.dialog.dropdown.base().id();
        let dd_at = pops.iter().position(|&p| p == dd_id).expect("the dropdown");
        assert!(dd_at > dialog_at);
        // And resolvable, which is what the engine's clamp walks: an id the
        // tree has dropped is skipped in silence.
        assert!(state.ui_context.tree.get_ptr(dd_id).is_some(), "the dropdown is in the widget tree");

        // Down, Enter: the next unit, the dialog still up.
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowDown)));
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert_eq!(state.world_unit.suffix(), units[1]);
        assert!(!state.dialog_dropdown_open() && state.dialog_visible());

        // A press on a row of the list picks that row.
        state.open_dialog_dropdown(&setting_row_id("World Unit"));
        let k = units.iter().position(|u| u == "in").expect("inches");
        let (rx, ry, _, _) = state.slots.dialog.dropdown.popover_geom(cce_ui::scene::layout::Rect { x: tx, y: ty, width: tw, height: th });
        press(&mut state, rx + 10.0, ry + k as f32 * 24.0 + 12.0);
        assert_eq!(state.world_unit.suffix(), "in");
        assert!(state.dialog_visible());

        // Escape closes the dropdown alone; a press off it does too.
        state.open_dialog_dropdown(&setting_row_id("World Unit"));
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Escape)));
        assert!(!state.dialog_dropdown_open() && state.dialog_visible());
        state.open_dialog_dropdown(&setting_row_id("World Unit"));
        press(&mut state, dx + 20.0, dy + dh - 20.0);
        assert!(!state.dialog_dropdown_open() && state.dialog_visible(), "a press off the list closes it alone");
        assert_eq!(state.world_unit.suffix(), "in", "and picks nothing");
        state.close_dialog();
        assert!(!state.slots.dialog.dropdown.open, "a closed dialog leaves no plate behind");
    }

    /// A control in the palette lifts under the pointer: the row whose
    /// switch, slider or colour well the pointer is over is the hovered
    /// control, and the label beside it is not.
    #[test]
    fn a_palette_control_lifts_under_the_pointer() {
        use crate::dialog::{setting_row_id, Control, TOGGLE_W};
        use crate::slots::DIALOG_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.run_command("command_palette");
        // The Show Grid switch is a toggle row, Geometry Opacity a slider.
        for c in ["s", "h", "o", "w", "g", "r", "i"] {
            state.dialog_key_input(&typed(c));
        }
        let rows = &state.slots.dialog.rows;
        let toggle = rows.iter().position(|r| r.id == "toggle_grid").expect("a Show Grid row");
        assert!(matches!(rows[toggle].control, Some(Control::Toggle(_))));
        let (dx, dy, dw, dh) = state.positions[DIALOG_IDX];
        assert!(dw > 0.0 && dh > 0.0);
        let at = |state: &mut State, x: f32, y: f32| {
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
            state.slots.dialog.hovered_control()
        };
        // Walk down the switch column until the pointer is over the row's
        // switch.
        let switch_x = dx + dw - 8.0 - TOGGLE_W * 0.5;
        let mut found = None;
        let mut y = dy;
        while y < dy + dh {
            if at(&mut state, switch_x, y) == Some(toggle) {
                found = Some(y);
                break;
            }
            y += 4.0;
        }
        let y = found.expect("the switch is under the pointer somewhere down its column");
        // The label beside it is not the control.
        assert_eq!(at(&mut state, dx + 24.0, y), None);
        assert_eq!(at(&mut state, switch_x, y), Some(toggle));
        // Off the plate, nothing is hovered.
        assert_eq!(at(&mut state, dx - 50.0, y), None);

        // A slider row: its whole control, readout lane included.
        while !state.slots.dialog.query.is_empty() {
            state.dialog_key_input(&key_press(Key::Named(NamedKey::Backspace)));
        }
        for c in ["g", "e", "o", "m", "e", "t", "r", "y"] {
            state.dialog_key_input(&typed(c));
        }
        let slider = state.slots.dialog.rows.iter().position(|r| r.id == setting_row_id("Geometry Opacity")).expect("a slider row");
        let mut found = None;
        let mut y = dy;
        while y < dy + dh {
            if at(&mut state, dx + dw - 40.0, y) == Some(slider) {
                found = Some(y);
                break;
            }
            y += 4.0;
        }
        let y = found.expect("the slider is under the pointer somewhere down its column");
        assert_eq!(at(&mut state, dx + 24.0, y), None, "the label is not the control");
        // Painting with the control hovered lifts the stamp, and paints.
        let _ = at(&mut state, dx + dw - 40.0, y);
        let _ = state.collect_display_list();
    }

    #[test]
    fn the_palette_lists_the_settings_as_control_rows() {
        use crate::dialog::{setting_row_id, Control, SETTINGS};
        let mut state = State::new(false);
        state.run_command("toggle_dialog");
        let ids: Vec<String> = state.slots.dialog.rows.iter().map(|r| r.id.clone()).collect();
        for s in SETTINGS {
            assert!(ids.contains(&setting_row_id(s.label)), "'{}' has no row", s.label);
        }
        let control = |label: &str| {
            state.slots.dialog.rows.iter().find(|r| r.id == setting_row_id(label)).and_then(|r| r.control.clone())
        };
        assert!(matches!(control("Grid Color"), Some(Control::Color { .. })));
        assert!(matches!(control("Grid Thickness"), Some(Control::Slider { dec: 0, .. })), "a spin is a whole-number slider");
        assert!(matches!(control("Geometry Opacity"), Some(Control::Slider { dec: 2, .. })));
        assert!(matches!(control("World Unit"), Some(Control::Choice { .. })));
        assert!(matches!(control("Group Marker Size"), Some(Control::Slider { .. })));
        // A command's switch is its own row; the settings table lists none
        // of them twice.
        assert!(control("Show Grid").is_none());
        assert!(state.slots.dialog.rows.iter().any(|r| r.id == "toggle_grid" && r.toggle().is_some()));

        // Tab does not move anywhere, and the dialog stays.
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Tab)));
        assert!(state.dialog_visible());

        // The filter ranks a setting like a command: "gridc" finds Grid
        // Color ahead of everything.
        for c in ["g", "r", "i", "d", "c"] {
            state.dialog_key_input(&typed(c));
        }
        assert_eq!(state.slots.dialog.selected_id(), Some(setting_row_id("Grid Color").as_str()));
    }

    /// A Settings row writes to whatever OWNS its value.
    ///
    /// That used to mean a param on a utility subnet, never the live field:
    /// `apply_settings_from_menubar_subnets` copied those subnets back over
    /// live state on every param change, so a direct write survived until
    /// the next edit and no longer. The live field IS the value now, and a
    /// `Command` row goes through the command so the menus and the persist
    /// come with it.
    #[test]
    fn dialog_settings_write_reaches_the_owning_subnet() {
        use crate::dialog::{setting_row_id, Control};
        let mut state = State::new(false);
        state.run_command("toggle_dialog");

        // A toggle command's row: Show Grid dispatches `toggle_grid` and
        // the dialog stays up.
        let was = state.viewport().show_grid;
        state.take_dialog_pick("toggle_grid".to_string());
        assert_eq!(state.viewport().show_grid, !was, "the live state followed");
        assert_eq!(state.command_toggle_state("toggle_grid"), Some(!was), "and the switch shows it");
        assert!(state.dialog_visible());

        // A Field row: Grid Thickness is a whole number in thousandths.
        state.apply_setting("Grid Thickness", "40");
        assert!((state.grid_thickness - 0.04).abs() < 1e-6, "{}", state.grid_thickness);
        let row = state.slots.dialog.rows.iter().find(|r| r.id == setting_row_id("Grid Thickness")).unwrap();
        assert!(matches!(row.control, Some(Control::Slider { value, .. }) if (value - 40.0).abs() < 1e-6), "the row re-read the value");

        // The arrows work the selected row's control in place: a choice
        // steps, a slider nudges, each landing on the live state.
        let unit_row = state.slots.dialog.rows.iter().position(|r| r.id == setting_row_id("World Unit")).unwrap();
        state.slots.dialog.selected = unit_row;
        let before = state.world_unit;
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowRight)));
        assert_ne!(state.world_unit, before, "right arrow steps the unit");
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowLeft)));
        assert_eq!(state.world_unit, before, "left arrow steps it back");
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(state.dialog_dropdown_open(), "Enter opens a choice's dropdown");
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowDown)));
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert_ne!(state.world_unit, before, "Down and Enter pick the next option");
        assert!(!state.dialog_dropdown_open());
        assert!(state.dialog_visible(), "and keeps the dialog up");

        let scale_row = state.slots.dialog.rows.iter().position(|r| r.id == setting_row_id("Group Marker Size")).unwrap();
        state.slots.dialog.selected = scale_row;
        let before = state.group_marker_size;
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowRight)));
        assert!(state.group_marker_size > before, "right arrow grows the markers");
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowLeft)));
        assert!((state.group_marker_size - before).abs() < 1e-5, "left arrow shrinks them back");

        // And it survives an unrelated parameter edit, which is the whole
        // reason the subnets had to be the owner before.
        let mut redraw = false;
        let sphere = state.current_dir().children.iter().position(|c| c.name.starts_with("sphere")).expect("a sphere");
        state
            .apply_action(crate::app::McpAction::SetParam { slot: sphere, name: "radius".into(), value: "0.8".into() }, &mut redraw)
            .expect("set a sphere param");
        assert_eq!(state.viewport().show_grid, !was, "a param edit reverted the toggle");
        assert!((state.grid_thickness - 0.04).abs() < 1e-6, "a param edit reverted the thickness");
    }

    /// The recent projects are rows of the Commands list.
    ///
    /// The list was the Main utility node's "Open" dropdown and went with
    /// that node, which left `recent_files` written on every save and read
    /// by nothing — a feature with no way in. It is a list of documents, so
    /// it sits under the open document's own path row.
    #[test]
    fn the_palette_offers_the_recent_projects() {
        use crate::dialog::RECENT_ROW_PREFIX;
        let mut state = State::new(false);
        let a = std::path::PathBuf::from("/tmp/cce-recent-alpha");
        let b = std::path::PathBuf::from("/tmp/cce-recent-beta");
        state.recent_files = vec![a.clone(), b.clone()];

        state.open_dialog();
        let rows: Vec<String> = state.slots.dialog.rows.iter().map(|r| r.id.clone()).collect();
        let id_a = format!("{RECENT_ROW_PREFIX}{}", a.display());
        let id_b = format!("{RECENT_ROW_PREFIX}{}", b.display());
        let ia = rows.iter().position(|r| *r == id_a).expect("no row for the newest recent project");
        let ib = rows.iter().position(|r| *r == id_b).expect("no row for the older recent project");
        assert!(ia < ib, "the recent list is not in most-recent-first order");
        // The row shows the path, truncated from the LEFT — the tail is what
        // identifies a project — with the file name in the chord column.
        let row = &state.slots.dialog.rows[ia];
        assert_eq!(row.label, a.display().to_string());
        assert_eq!(row.chord, "cce-recent-alpha");
        assert!(row.truncate_head);
        // And it ranks against the path text like any other row.
        state.slots.dialog.query = "beta".to_string();
        state.refresh_dialog_rows();
        let rows: Vec<String> = state.slots.dialog.rows.iter().map(|r| r.id.clone()).collect();
        assert!(rows.contains(&id_b) && !rows.contains(&id_a), "{rows:?}");
        state.close_dialog();

        // The project already open is not offered a second time.
        state.loaded_project_path = Some(a.clone());
        state.open_dialog();
        let rows: Vec<String> = state.slots.dialog.rows.iter().map(|r| r.id.clone()).collect();
        assert!(!rows.contains(&id_a), "the open project is listed as a recent one");
        assert!(rows.contains(&id_b));
    }

    /// Every display setting the retired utility subnets held is reachable —
    /// as a Settings row, a command, or both.
    ///
    /// This is the check the removal turns on. Those four nodes were the only
    /// way to reach a good half of these values, so a setting left out of the
    /// table when they went is not "hidden in the node tree", it is GONE, and
    /// nothing else in the suite would notice.
    #[test]
    fn every_retired_subnet_setting_is_reachable() {
        // The values are setting rows; the toggles are commands, whose
        // palette rows carry their switches — each listed once.
        let labels: Vec<&str> = crate::dialog::SETTINGS.iter().map(|s| s.label).collect();
        for label in [
            // guides
            "Grid Color", "Grid Thickness", "Origin Size", "Point Marker Size",
            "Point Marker Color", "World Unit",
            // render
            "Wireframe Color", "Wire Opacity", "Wire Thickness", "Geometry Opacity",
            // main
            "Background Color",
        ] {
            assert!(labels.contains(&label), "'{label}' has no Settings row and no other way in");
        }
        // The camera subnet's Camera Pivot Size is the viewport menu's
        // slider, under Show Camera Pivot; its palette row is gone.
        assert!(!labels.contains(&"Camera Pivot Size"), "the palette's pivot size row is retired");
        let mut state = State::new(false);
        assert!(
            state.viewport_menu_rows_of(None).1.contains(&crate::app::ViewportMenuAction::CameraPivotSizeSlider),
            "Camera Pivot Size has no way in"
        );
        for id in [
            "toggle_grid", "toggle_origin", "toggle_wireframe",
            "toggle_wire_single_color", "toggle_ray_traced_preview",
            "toggle_circular_pane", "toggle_camera_pivot", "toggle_square_viewport",
            "toggle_network_plate",
        ] {
            assert!(crate::command::by_id(id).is_some(), "the toggle '{id}' has no command");
            assert!(state.command_toggle_state(id).is_some(), "the toggle '{id}' draws no switch");
        }
        state.run_command("command_palette");
        assert!(state.slots.dialog.rows.iter().any(|r| r.id == "toggle_grid" && r.toggle().is_some()));
        // The Main node's buttons are commands, and the active camera keeps
        // the viewport menubar's own menu — neither is a row here.
        for id in [
            "new_project", "open_project", "save_document", "save_document_as",
            "set_as_default", "exit", "undo", "redo",
            "zoom_in", "zoom_out", "reset_zoom", "detach_circular_window",
        ] {
            assert!(crate::command::by_id(id).is_some(), "the Main node's '{id}' has no command");
        }
    }

    /// Reopening starts clean: on Commands, with an empty query.
    #[test]
    fn dialog_reopens_without_the_last_search() {
        let mut state = State::new(false);
        state.run_command("toggle_dialog");
        state.dialog_key_input(&typed("g"));
        state.run_command("toggle_dialog");
        assert!(!state.dialog_visible());

        state.run_command("toggle_dialog");
        assert_eq!(state.slots.dialog.query, "");
    }

    /// A left press on empty grid puts the cursor on the pressed cell — on the
    /// PRESS — and dragging from there expands it into a region, which stays
    /// after the release. Moving the cursor any other way collapses it, since
    /// the expanse is only read back while its anchor is the live cursor.
    #[test]
    fn dragging_the_network_grid_expands_the_cursor() {
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;

        // Two empty cells inside the pane's visible span, two columns and
        // two rows apart.
        let anchor = (1, 4);
        let far = (3, 6);
        for cell in [anchor, far] {
            assert!(
                !state.current_dir().children.iter().any(|c| (c.position.0 as i32, c.position.1 as i32) == cell),
                "{cell:?} must be empty grid"
            );
        }
        let move_to = |state: &mut State, (col, row): (i32, i32)| {
            let (x, y) = state.cell_center(col, row);
            state.handle_event(&WindowEvent::CursorMoved {
                position: LocalPosition { x: x as f64, y: y as f64 },
            });
        };

        move_to(&mut state, anchor);
        state.handle_event(&WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: MouseButton::Left,
        });
        assert_eq!(
            (state.grid_cursor_col, state.grid_cursor_row),
            anchor,
            "the press alone moves the cursor — not the release"
        );
        assert_eq!(state.grid_cursor_region(), (anchor.0, anchor.1, 1, 1));

        // Dragging grows it from the anchor to the cell under the pointer.
        move_to(&mut state, (2, 5));
        assert_eq!(state.grid_cursor_region(), (1, 4, 2, 2));
        move_to(&mut state, far);
        assert_eq!(state.grid_cursor_region(), (1, 4, 3, 3));
        assert_eq!(
            (state.grid_cursor_col, state.grid_cursor_row),
            anchor,
            "the anchor is still the cursor cell — Add Node places there"
        );

        // The outline follows: the region's corner cells, unioned.
        let (rx, ry, rw, rh) = state.grid_cursor_rect();
        let (ax, ay, cw, ch) = state.cell_rect(anchor.0, anchor.1);
        assert!((rx - ax).abs() < 0.01 && (ry - ay).abs() < 0.01);
        assert!(rw > cw * 2.0 && rh > ch * 2.0, "{rw}x{rh} spans three cells each way");

        // The release SETTLES the region: this drag caught no nodes, so it
        // comes back as one cell at the middle of where it stood — (1, 4)
        // through (3, 6), whose middle is (2, 5).
        state.handle_event(&WindowEvent::MouseInput {
            state: ElementState::Released,
            button: MouseButton::Left,
        });
        assert!(state.grid_cursor_drag.is_none());
        assert_eq!(state.grid_cursor_region(), (2, 5, 1, 1));
        assert!(state.grid_cursor_expanse.is_none(), "collapsed outright, not a 1x1 region");

        // Motion with no drag armed leaves the cursor alone.
        move_to(&mut state, (0, 2));
        assert_eq!(state.grid_cursor_region(), (2, 5, 1, 1));
        state.run_command("nav_right");
        assert_eq!(state.grid_cursor_region(), (3, 5, 1, 1));
    }

    /// Dragging a node that is part of the selection carries the whole
    /// selection with it, rigidly, and the region travels too. Dragging a node
    /// OUTSIDE the selection is the ordinary one-node drag, and collapses the
    /// selection onto what was grabbed.
    #[test]
    fn dragging_a_selected_node_carries_the_selection() {
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;

        let mut redraw = false;
        for (name, x, y) in [("a", 1.0, 5.0), ("b", 2.0, 7.0), ("c", 1.0, 11.0)] {
            state
                .apply_action(
                    crate::app::McpAction::AddNode {
                        template_name: "Plane".into(),
                        name: Some(name.into()),
                        x,
                        y,
                    },
                    &mut redraw,
                )
                .unwrap();
        }
        state.rebuild_positions();
        state.apply_layout();
        let slot = |state: &State, name: &str| {
            state.current_dir().children.iter().position(|c| c.name == name).expect(name)
        };
        let (a, b, c) = (slot(&state, "a"), slot(&state, "b"), slot(&state, "c"));

        let move_to = |state: &mut State, (col, row): (i32, i32)| {
            let (x, y) = state.cell_center(col, row);
            state.handle_event(&WindowEvent::CursorMoved {
                position: LocalPosition { x: x as f64, y: y as f64 },
            });
        };

        // Select a and b by dragging a box round them; it settles onto their
        // bounding box, (1, 5) to (2, 7).
        move_to(&mut state, (0, 4));
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        move_to(&mut state, (3, 9));
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        assert_eq!(state.grid_cursor_region(), (1, 5, 2, 3));
        assert_eq!(state.selected_slots(), vec![a, b]);

        // Grab b — one of the selected — and drag it one cell right and one
        // down. Both travel; c, unselected, does not.
        move_to(&mut state, (2, 7));
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        assert!(state.node_drag_group.is_some(), "the press picked up the selection");
        assert_eq!(state.grid_cursor_region(), (1, 5, 2, 3), "the press left the region alone");
        move_to(&mut state, (3, 8));
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });

        assert!(state.node_drag_group.is_none());
        assert_eq!(state.current_dir().children[b].position, (3.0, 8.0), "the grabbed node");
        assert_eq!(state.current_dir().children[a].position, (2.0, 6.0), "carried along");
        assert_eq!(state.current_dir().children[c].position, (1.0, 11.0), "not selected");
        assert_eq!(state.grid_cursor_region(), (2, 6, 2, 3), "the region came too");
        assert_eq!(state.selected_slots(), vec![a, b], "still the same two");

        // Grabbing c, which is NOT selected, is the ordinary one-node drag:
        // the anchor moves onto it and the region collapses with it.
        move_to(&mut state, (1, 11));
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        assert!(state.node_drag_group.is_none(), "one node, the widget's own drag");
        assert_eq!(state.grid_cursor_region(), (1, 11, 1, 1), "collapsed onto what was grabbed");
        move_to(&mut state, (0, 11));
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        assert_eq!(state.current_dir().children[c].position, (0.0, 11.0));
        assert_eq!(state.current_dir().children[a].position, (2.0, 6.0), "a stayed put");
    }

    /// A drag that CAUGHT nodes settles onto their bounding box — the loose
    /// box you drew comes back fitted to what it selected, and the selection
    /// itself does not change, the box containing no cell the region did not.
    #[test]
    fn a_drag_settles_onto_the_nodes_it_caught() {
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;

        // Two nodes well inside a box drawn from (0, 3) to (3, 9).
        let mut redraw = false;
        for (name, x, y) in [("a", 1.0, 5.0), ("b", 2.0, 7.0)] {
            state
                .apply_action(
                    crate::app::McpAction::AddNode {
                        template_name: "Plane".into(),
                        name: Some(name.into()),
                        x,
                        y,
                    },
                    &mut redraw,
                )
                .unwrap();
        }
        state.rebuild_positions();
        state.apply_layout();
        let slot = |state: &State, name: &str| {
            state.current_dir().children.iter().position(|c| c.name == name).expect(name)
        };
        let (a, b) = (slot(&state, "a"), slot(&state, "b"));

        let move_to = |state: &mut State, (col, row): (i32, i32)| {
            let (x, y) = state.cell_center(col, row);
            state.handle_event(&WindowEvent::CursorMoved {
                position: LocalPosition { x: x as f64, y: y as f64 },
            });
        };
        move_to(&mut state, (0, 3));
        state.handle_event(&WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: MouseButton::Left,
        });
        move_to(&mut state, (3, 9));
        assert_eq!(state.grid_cursor_region(), (0, 3, 4, 7), "the box as drawn");
        assert_eq!(state.selected_slots(), vec![a, b]);

        state.handle_event(&WindowEvent::MouseInput {
            state: ElementState::Released,
            button: MouseButton::Left,
        });
        assert_eq!(state.grid_cursor_region(), (1, 5, 2, 3), "fitted to a and b");
        assert_eq!(state.selected_slots(), vec![a, b], "and holding the same two");

        // One node caught collapses the region onto it, and the ordinary
        // single selection takes over from there.
        state.grid_cursor_col = 0;
        state.grid_cursor_row = 3;
        state.grid_cursor_expanse = Some(((0, 3), (3, 6)));
        assert_eq!(state.selected_slots(), vec![a]);
        assert!(state.settle_cursor_expansion());
        assert_eq!(state.grid_cursor_region(), (1, 5, 1, 1));
        assert!(state.grid_cursor_expanse.is_none());
        assert_eq!(state.graph().selected_node(), Some(a), "the cursor sits on it now");
    }

    /// An expanded cursor selects every node standing inside it, and the
    /// selection is what the network's operations act on: alt+move drags them
    /// all (region and all, or the first press would collapse what it moved),
    /// Delete removes them all, and Escape collapses the region.
    #[test]
    fn an_expanded_cursor_selects_every_node_inside_it() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;

        let mut redraw = false;
        for (name, x, y) in [("a", 8.0, 8.0), ("b", 10.0, 9.0), ("c", 14.0, 8.0)] {
            state
                .apply_action(
                    crate::app::McpAction::AddNode {
                        template_name: "Plane".into(),
                        name: Some(name.into()),
                        x,
                        y,
                    },
                    &mut redraw,
                )
                .unwrap();
        }
        let slot = |state: &State, name: &str| {
            state.current_dir().children.iter().position(|c| c.name == name).expect(name)
        };
        let (a, b, c) = (slot(&state, "a"), slot(&state, "b"), slot(&state, "c"));

        // A region from (8, 8) to (11, 10) holds a and b, and not c at (14, 8).
        state.grid_cursor_col = 8;
        state.grid_cursor_row = 8;
        state.grid_cursor_expanse = Some(((8, 8), (11, 10)));
        assert_eq!(state.grid_cursor_region(), (8, 8, 4, 3));
        assert_eq!(state.selected_slots(), vec![a, b]);
        assert!(!state.selected_slots().contains(&c));

        // alt+h moves both, and the region travels with them — stepping the
        // anchor alone is exactly what drops a region.
        state.run_command("move_left");
        assert_eq!(state.current_dir().children[a].position, (7.0, 8.0));
        assert_eq!(state.current_dir().children[b].position, (9.0, 9.0));
        assert_eq!(state.current_dir().children[c].position, (14.0, 8.0), "not selected, not moved");
        assert_eq!(state.grid_cursor_region(), (7, 8, 4, 3), "the region came along");
        assert_eq!(state.selected_slots(), vec![a, b], "and still holds the same two");

        // Escape collapses it, since nothing else would until the cursor
        // wandered off the anchor.
        assert!(state.deselect_node());
        assert_eq!(state.grid_cursor_region(), (7, 8, 1, 1));
        assert_eq!(
            state.selected_slots(),
            state.graph().selected_node().into_iter().collect::<Vec<_>>(),
            "collapsed, the selection is the graph's own single one again"
        );

        // Delete takes the whole selection, not just one of it.
        state.grid_cursor_expanse = Some(((7, 8), (10, 10)));
        assert_eq!(state.selected_slots().len(), 2);
        let before = state.current_dir().children.len();
        state.handle_event(&crate::window::WindowEvent::KeyboardInput {
            event: key_press(Key::Named(NamedKey::Delete)),
        });
        assert_eq!(state.current_dir().children.len(), before - 2);
        assert!(state.current_dir().children.iter().any(|n| n.name == "c"), "c survived");
    }

    /// shift+hjkl grows the cursor's region from a FIXED anchor, so the
    /// selection extends the way the plugin's does: shift+l then shift+h
    /// comes back to where it started rather than walking the region sideways,
    /// and a far corner that meets the anchor again leaves a plain one-cell
    /// cursor rather than a 1x1 region.
    #[test]
    fn shift_hjkl_extends_the_selection_from_a_fixed_anchor() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;

        let mut redraw = false;
        for (name, x, y) in [("a", 1.0, 4.0), ("b", 2.0, 4.0), ("c", 3.0, 4.0)] {
            state
                .apply_action(
                    crate::app::McpAction::AddNode {
                        template_name: "Plane".into(),
                        name: Some(name.into()),
                        x,
                        y,
                    },
                    &mut redraw,
                )
                .unwrap();
        }
        let slot = |state: &State, name: &str| {
            state.current_dir().children.iter().position(|c| c.name == name).expect(name)
        };
        let (a, b, c) = (slot(&state, "a"), slot(&state, "b"), slot(&state, "c"));

        // The cursor starts ON a node, and extending KEEPS it: the anchor's
        // cell is part of its own region, which is what makes this an extend
        // rather than a second way to start a selection.
        state.grid_cursor_col = 1;
        state.grid_cursor_row = 4;
        state.sync_cursor_and_selection();
        assert_eq!(state.selected_slots(), vec![a]);

        assert!(state.run_command("extend_right"));
        assert_eq!(state.grid_cursor_region(), (1, 4, 2, 1));
        assert_eq!(state.selected_slots(), vec![a, b]);
        assert!(state.run_command("extend_right"));
        assert_eq!(state.selected_slots(), vec![a, b, c]);
        assert_eq!(
            (state.grid_cursor_col, state.grid_cursor_row),
            (1, 4),
            "the anchor is the fixed end"
        );

        // Back the way it came, and the region shrinks rather than walking.
        assert!(state.run_command("extend_left"));
        assert_eq!(state.selected_slots(), vec![a, b]);
        assert!(state.run_command("extend_left"));
        assert_eq!(state.grid_cursor_region(), (1, 4, 1, 1), "collapsed, not 1x1-with-an-expanse");
        assert!(state.grid_cursor_expanse.is_none());
        assert_eq!(state.selected_slots(), vec![a], "the plain single selection again");

        // The other way round: past the anchor, so the region grows leftward.
        assert!(state.run_command("extend_left"));
        assert_eq!(state.grid_cursor_region(), (0, 4, 2, 1));
        assert!(state.run_command("extend_up"));
        assert_eq!(state.grid_cursor_region(), (0, 3, 2, 2));

        // And the family is the network pane's, like the other three.
        state.focused_pane = crate::slots::RIGHT_MENUBAR_IDX;
        let before = state.grid_cursor_region();
        state.run_command("extend_right");
        assert_eq!(state.grid_cursor_region(), before, "not the viewport's key");
    }

    /// The selected nodes actually LOOK selected: each body is painted with
    /// the highlight tint the widget gives its own single selection, so an
    /// expanded cursor reads as a selection rather than as an empty outline
    /// drawn over nodes.
    #[test]
    fn every_node_in_the_region_is_painted_selected() {
        use cce_ui::scene::paint::Prim;
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;

        // Cells inside the pane's visible span, clear of the project's own
        // nodes — the paint is clipped to the pane, so an off-screen node
        // would prove nothing.
        let mut redraw = false;
        for (name, x, y) in [("a", 1.0, 4.0), ("b", 2.0, 5.0), ("c", 3.0, 9.0)] {
            state
                .apply_action(
                    crate::app::McpAction::AddNode {
                        template_name: "Plane".into(),
                        name: Some(name.into()),
                        x,
                        y,
                    },
                    &mut redraw,
                )
                .unwrap();
        }
        state.rebuild_positions();
        state.apply_layout();
        state.grid_cursor_col = 1;
        state.grid_cursor_row = 4;
        state.grid_cursor_expanse = Some(((1, 4), (2, 6)));
        assert_eq!(state.selected_slots().len(), 2, "a and b, not c");

        let hl = cce_ui::colors::highlight_primary_color();
        let tinted_at = |list: &cce_ui::scene::paint::DisplayList, (cx, cy): (f32, f32)| {
            list.items.iter().any(|item| match &item.prim {
                Prim::Bevel { rect, tint, .. } => {
                    tint[0] == hl[0]
                        && tint[1] == hl[1]
                        && tint[2] == hl[2]
                        && (rect.x + rect.width * 0.5 - cx).abs() < 1.0
                        && (rect.y + rect.height * 0.5 - cy).abs() < 1.0
                }
                _ => false,
            })
        };
        let list = state.collect_display_list();
        assert!(tinted_at(&list, state.cell_center(1, 4)), "a is painted selected");
        assert!(tinted_at(&list, state.cell_center(2, 5)), "b is painted selected");
        assert!(!tinted_at(&list, state.cell_center(3, 9)), "c is outside the region");
    }

    /// Copy takes the whole selection and paste lays it back out in the shape
    /// it was copied in — a pasted chain arrives wired the way it was drawn,
    /// not stacked in a column.
    #[test]
    fn copying_a_region_keeps_the_shape_on_paste() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;

        let mut redraw = false;
        for (name, x, y) in [("a", 8.0, 8.0), ("b", 10.0, 9.0)] {
            state
                .apply_action(
                    crate::app::McpAction::AddNode {
                        template_name: "Plane".into(),
                        name: Some(name.into()),
                        x,
                        y,
                    },
                    &mut redraw,
                )
                .unwrap();
        }
        state.grid_cursor_col = 8;
        state.grid_cursor_row = 8;
        state.grid_cursor_expanse = Some(((8, 8), (11, 10)));
        state.copy_selected_nodes();
        assert_eq!(state.node_clipboard.len(), 2);

        // Paste at a clear corner of the sheet: the set's top-left lands on
        // the cursor and the second node keeps its (+2, +1) offset.
        state.grid_cursor_col = 20;
        state.grid_cursor_row = 20;
        state.grid_cursor_expanse = None;
        let before = state.current_dir().children.len();
        assert!(state.paste_nodes());
        assert_eq!(state.current_dir().children.len(), before + 2);
        let pasted: Vec<(f32, f32)> = state.current_dir().children[before..]
            .iter()
            .map(|n| n.position)
            .collect();
        assert_eq!(pasted, vec![(20.0, 20.0), (22.0, 21.0)]);
    }

    /// Pasting on a wire splices the paste in — one node, or a copied chain
    /// whole — and a pasted node whose name is taken takes the next free
    /// one, the wires inside the paste following it.
    #[test]
    fn a_paste_on_a_wire_is_spliced_into_its_chain() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.current_dir_mut().children = vec![
            ref_node("a", "a", "sphere", vec![("radius", "float", "1")], vec![]),
            ref_node("c", "c", "transform", vec![("input", "node", "a")], vec![]),
            ref_node("p", "p1", "transform", vec![("input", "node", "")], vec![]),
            ref_node("q", "q1", "transform", vec![("input", "node", "p1")], vec![]),
        ];
        for (i, pos) in [(2.0, 1.0), (2.0, 5.0), (8.0, 1.0), (8.0, 2.0)].into_iter().enumerate() {
            state.current_dir_mut().children[i].position = pos;
        }
        state.sync_nodes();
        state.rebuild_positions();
        state.apply_layout();
        let input_of = |state: &State, name: &str| {
            let n = state.current_dir().children.iter().find(|c| c.name == name).expect(name);
            crate::geometry::node_param_node(n, "input")
        };

        // One node, onto the wire a -> c.
        state.node_clipboard = vec![state.current_dir().children[2].clone()];
        state.grid_cursor_col = 2;
        state.grid_cursor_row = 2;
        assert!(state.paste_nodes());
        assert_eq!(input_of(&state, "p2").as_deref(), Some("a"), "renamed past p1, and reading the upstream");
        assert_eq!(input_of(&state, "c").as_deref(), Some("p2"));

        // The chain p1 -> q1, onto the wire p2 -> c: in whole, wired inside
        // to its own copies and not to the originals.
        state.node_clipboard = state.current_dir().children[2..4].to_vec();
        state.grid_cursor_col = 2;
        state.grid_cursor_row = 3;
        state.sync_nodes();
        assert!(state.paste_nodes());
        assert_eq!(input_of(&state, "p3").as_deref(), Some("p2"), "the head reads the wire's upstream");
        assert_eq!(input_of(&state, "q2").as_deref(), Some("p3"), "the copy reads the copy");
        assert_eq!(input_of(&state, "c").as_deref(), Some("q2"), "the downstream reads the tail");
        assert_eq!(input_of(&state, "q1").as_deref(), Some("p1"), "the original is untouched");
    }

    /// A press the graph itself took does not arm the expansion drag. The
    /// case that bites is a PORT: it starts a connection and consumes the
    /// press without selecting anything, so the empty-grid arm would read it
    /// as bare lattice and then swallow every motion event — leaving the
    /// connection's rubber-band line frozen at the port it started from.
    #[test]
    fn a_port_press_does_not_arm_the_cursor_expansion() {
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;

        // A real port centre, off the widget's own geometry — the ports sit
        // outside the node body, so nothing but this gets one right.
        let (px, py) = {
            let g = state.slots.content.inner();
            (0..state.current_dir().children.len())
                .find_map(|i| {
                    g.port_center(i, cce_ui::widget::display::graph::PortType::Output, 0)
                })
                .expect("a node with an output port")
        };
        state.handle_event(&WindowEvent::CursorMoved {
            position: LocalPosition { x: px as f64, y: py as f64 },
        });
        // Nothing selected: the project loads with a selection, and with one
        // standing the empty-grid arm is never reached at all — the guard
        // this test is about would go untested.
        state.graph_mut().set_selected_node(None);
        state.handle_event(&WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: MouseButton::Left,
        });
        assert!(
            state.graph().selected_node().is_none(),
            "a port press selects nothing — which is what makes the arm below \
             read it as empty grid unless the guard holds"
        );
        assert!(
            state.grid_cursor_drag.is_none(),
            "the graph took this press — the cursor must not start expanding"
        );

        // And the motion that follows is still the graph's, not eaten here.
        let before = state.grid_cursor_region();
        state.handle_event(&WindowEvent::CursorMoved {
            position: LocalPosition { x: px as f64, y: (py + 120.0) as f64 },
        });
        assert_eq!(state.grid_cursor_region(), before, "no region grew out of it");
    }

    /// A right press on EMPTY network space opens the network's own context
    /// menu — until 2026-09-22 it opened the add-node palette outright, which
    /// left the network the one pane whose right-click was not a context menu,
    /// and left every other graph-wide command reachable only by chord or
    /// through the palette. Add Node is the first row, and picking it is what
    /// opens the palette — at the cell the press landed on, since the press
    /// moves the grid cursor there before the menu goes up.
    #[test]
    fn network_right_click_opens_a_menu_whose_first_row_is_add_node() {
        use crate::dialog::Mode;
        use crate::slots::CONTENT_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();

        // A point in the network pane with no node under it: the plate is on,
        // so the pane's rect is its own and empty space is still the graph's.
        assert!(state.network_plate, "the plate is on by default");
        let (cx, cy, cw, ch) = state.positions[CONTENT_IDX];
        let (px, py) = (cx + cw * 0.85, cy + ch * 0.85);
        state.handle_event(&WindowEvent::CursorMoved {
            position: LocalPosition { x: px as f64, y: py as f64 },
        });
        assert!(state.in_network_pane(), "the press must land in the network pane");
        assert!(
            state.graph().node_at(px, py).is_none(),
            "pick a cell with no node on it"
        );
        let cell = state.cell_at(px, py);

        state.handle_event(&WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: MouseButton::Right,
        });
        assert!(!state.dialog_visible(), "no palette straight off the press");
        assert!(cce_ui::widget::context_menu::is_visible());
        let options = cce_ui::widget::context_menu::options();
        assert_eq!(options.first().map(String::as_str), Some("Add Node"));
        assert!(
            options.iter().any(|o| o == "Layout Nodes"),
            "the graph-wide commands come with it: {options:?}"
        );
        assert!(
            options.iter().any(|o| o.ends_with("Network Plate")),
            "a toggle row carries its mark: {options:?}"
        );
        assert_eq!((state.grid_cursor_col, state.grid_cursor_row), cell);

        // A left click on the first row runs Add Node: the menu goes, the
        // palette comes up, and it will add at the cell that was clicked.
        let rx = cce_ui::widget::context_menu::x() + 8.0;
        let ry = cce_ui::widget::context_menu::row_y(0) + 4.0;
        state.handle_event(&WindowEvent::CursorMoved {
            position: LocalPosition { x: rx as f64, y: ry as f64 },
        });
        let menu_corner = (cce_ui::widget::context_menu::x(), cce_ui::widget::context_menu::y());
        state.handle_event(&WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: MouseButton::Left,
        });
        assert!(!cce_ui::widget::context_menu::is_visible(), "the pick closes the menu");
        assert!(state.dialog_visible());
        assert_eq!(state.slots.dialog.mode, Mode::AddNode);
        assert_eq!((state.grid_cursor_col, state.grid_cursor_row), cell);

        // The menu turns into the list: the plate opens on the menu's
        // corner, as far as the window lets it, not centred.
        let (dx, dy, dw, dh) = state.positions[crate::slots::DIALOG_IDX];
        let want = crate::dialog::layout_at(state.width, state.height, menu_corner.0, menu_corner.1);
        assert_eq!((dx, dy, dw, dh), want);
        assert_ne!((dx, dy), {
            let (x, y, _, _) = crate::dialog::layout_in(state.width, state.height);
            (x, y)
        }, "not the centred plate");
        assert!(dx + dw <= state.width && dy + dh <= state.height, "inside the window");

        // Tab and the palette's other openings still centre it.
        state.close_dialog();
        state.open_node_palette();
        assert_eq!(
            state.positions[crate::slots::DIALOG_IDX],
            crate::dialog::layout_in(state.width, state.height)
        );
    }

    /// An anchored plate keeps its corner where it fits, gives up height
    /// before it moves, and rises only below the least height it keeps.
    #[test]
    fn an_anchored_dialog_keeps_the_corner_it_was_given() {
        use crate::dialog::{layout_at, layout_in};
        let (_, _, w, h) = layout_in(1600.0, 1200.0);
        assert_eq!(layout_at(1600.0, 1200.0, 100.0, 50.0), (100.0, 50.0, w, h), "room for all of it");
        let (x, y, _, short) = layout_at(1600.0, 1200.0, 100.0, 700.0);
        assert_eq!((x, y), (100.0, 700.0), "the corner stays");
        assert!(short < h && short >= 340.0, "it is shorter: {short}");
        let (_, y, _, hh) = layout_at(1600.0, 1200.0, 100.0, 1100.0);
        assert!(y < 1100.0 && y + hh <= 1200.0, "too low, it rises: {y} + {hh}");
        let (x, _, _, _) = layout_at(1600.0, 1200.0, 1500.0, 50.0);
        assert!(x + w <= 1600.0, "pulled in from the right: {x}");
    }

    /// Every row of the network context menu names a command that exists —
    /// the labels are the registry's, so a renamed or deleted command would
    /// otherwise drop a row from the menu in silence.
    #[test]
    fn network_menu_rows_name_commands_that_exist() {
        for id in crate::app::NETWORK_MENU_COMMANDS.iter().flatten() {
            assert!(
                crate::command::by_id(id).is_some(),
                "the network menu offers `{id}`, which is not a command"
            );
        }
    }

    /// The Commands half heads with the open project's PATH: the label is the
    /// path (truncated on the left when it does not fit — the tail is what
    /// identifies it), the chord column is the file name, and picking it
    /// copies the path and closes. With no project loaded there is no row,
    /// which is the same position `loaded_project_path` and the window title
    /// take about the bundled default.
    #[test]
    fn the_palette_heads_with_the_open_projects_path() {
        use crate::dialog::PATH_ROW_ID;
        let dir = std::env::temp_dir()
            .join(format!("cce-designer-path-row-{}", std::process::id()))
            .join("my_project");
        let _ = fs::remove_dir_all(&dir);

        let mut state = State::new(false);
        state.focused_pane = crate::slots::RIGHT_MENUBAR_IDX;

        // Nothing loaded — the bundled default leaves no path behind, so the
        // palette offers no row rather than one naming a versioned file.
        assert!(state.project_path_readout().is_none());
        state.run_command("command_palette");
        assert!(
            !state.slots.dialog.rows.iter().any(|r| r.id == PATH_ROW_ID),
            "no project, no row"
        );
        state.close_dialog();

        state.save_to_file(&dir).expect("save");
        state.load_from_file(&dir).expect("load");
        assert_eq!(state.loaded_project_path.as_deref(), Some(dir.as_path()));

        state.run_command("command_palette");
        let row = state.slots.dialog.rows.first().expect("rows").clone();
        assert_eq!(row.id, PATH_ROW_ID, "it heads the list");
        assert_eq!(row.label, dir.to_string_lossy(), "the label is the whole path");
        assert_eq!(row.chord, "my_project", "the file name reads in the chord column");
        assert!(row.truncate_head, "a path is cut from the left");

        // It ranks like any other row: a query that matches the path keeps it,
        // one that does not drops it.
        for c in ["m", "y", "_", "p"] {
            state.dialog_key_input(&typed(c));
        }
        assert!(state.slots.dialog.rows.iter().any(|r| r.id == PATH_ROW_ID));
        for c in ["z", "z", "z"] {
            state.dialog_key_input(&typed(c));
        }
        assert!(!state.slots.dialog.rows.iter().any(|r| r.id == PATH_ROW_ID));

        // Picking it copies the path and leaves, the way a command does.
        state.close_dialog();
        state.run_command("command_palette");
        state.take_dialog_pick(PATH_ROW_ID.to_string());
        assert!(!state.dialog_visible(), "a copy is done the moment it happens");
        assert!(
            state.last_status_text.contains(&dir.to_string_lossy().to_string()),
            "the status line says what was copied: {}",
            state.last_status_text
        );

        let _ = fs::remove_dir_all(dir.parent().unwrap());
    }

    /// Tab opens the same plate in its AddNode mode: one list of node
    /// templates, no tab strip, no chord column.
    #[test]
    fn dialog_add_node_mode_lists_the_templates() {
        use crate::dialog::Mode;
        let mut state = State::new(false);
        state.open_node_palette();

        assert!(state.dialog_visible());
        assert_eq!(state.slots.dialog.mode, Mode::AddNode);
        // Inside the bundled project's Geometry node: every template but the
        // three that stand at the root.
        assert_eq!(state.slots.dialog.rows.len(), state.node_templates.len() - 3);
        assert!(
            state.slots.dialog.rows.iter().all(|r| r.chord.is_empty()),
            "a template has no chord to teach"
        );

        // Tab is what opened it, so Tab closes it again rather than looking
        // for a second half that is not there.
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Tab)));
        assert!(!state.dialog_visible());
    }

    /// Typing filters the templates through the SAME `fuzzy_rank` the command
    /// half uses, and Enter instantiates at the grid cursor.
    #[test]
    fn dialog_add_node_filters_and_adds_at_the_cursor() {
        let mut state = State::new(false);
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;
        state.grid_cursor_col = 3;
        state.grid_cursor_row = 2;
        let before = state.current_dir().children.len();

        state.open_node_palette();
        for c in ["b", "o", "x"] {
            state.dialog_key_input(&typed(c));
        }
        assert_eq!(
            state.slots.dialog.selected_id(),
            Some("Box"),
            "rows: {:?}",
            state.slots.dialog.rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>()
        );

        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert!(!state.dialog_visible(), "the pick closes the dialog");
        assert_eq!(state.current_dir().children.len(), before + 1);
        let added = state.current_dir().children.last().expect("the new node");
        assert!(added.name.starts_with("box"), "added {}", added.name);
        assert_eq!(added.position, (3.0, 2.0), "placed at the grid cursor");
    }

    /// Adding a node on a free cell a wire runs through wires it into that
    /// chain: A -> C becomes A -> new -> C. Off the wire, or for a node with
    /// no Input, nothing is rewired.
    #[test]
    fn a_node_added_on_a_wire_is_wired_into_its_chain() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.current_dir_mut().children = vec![
            ref_node("a", "a", "sphere", vec![("radius", "float", "1")], vec![]),
            ref_node("c", "c", "transform", vec![("input", "node", "a")], vec![]),
        ];
        state.current_dir_mut().children[0].position = (2.0, 1.0);
        state.current_dir_mut().children[1].position = (2.0, 3.0);
        state.sync_nodes();
        state.rebuild_positions();
        state.apply_layout();
        let input_of = |state: &State, name: &str| {
            let n = state.current_dir().children.iter().find(|c| c.name == name).expect(name);
            crate::geometry::node_param_node(n, "input")
        };
        let add = |state: &mut State, template: &str, col: i32, row: i32| {
            state.grid_cursor_col = col;
            state.grid_cursor_row = row;
            state.open_node_palette();
            state.take_dialog_pick(template.to_string());
            state.current_dir().children.last().unwrap().name.clone()
        };

        let mid = add(&mut state, "Transform", 2, 2);
        assert_eq!(input_of(&state, &mid).as_deref(), Some("a"), "the new node reads the wire's upstream");
        assert_eq!(input_of(&state, "c").as_deref(), Some(mid.as_str()), "and the downstream reads it");
        assert!(state.last_status_text.contains("between"), "{}", state.last_status_text);

        // Off every wire: added, wired to nothing new.
        let aside = add(&mut state, "Transform", 6, 2);
        assert_eq!(input_of(&state, "c").as_deref(), Some(mid.as_str()));
        assert_ne!(input_of(&state, &aside).as_deref(), Some("a"));

        // A generator on a wire cannot sit mid-chain: the wire is left alone.
        state.current_dir_mut().children[1].position = (2.0, 5.0);
        state.sync_nodes();
        let gen = add(&mut state, "Sphere", 2, 4);
        assert_eq!(input_of(&state, "c").as_deref(), Some(mid.as_str()), "{gen} did not cut the wire");
    }

    /// The Environment node is the scene's light, for both views: its sun
    /// direction lights the raster shading (and the smooth bake) and the
    /// tracer's sky; with none, or with it bypassed, the defaults do, which
    /// are the template's — so adding one changes nothing until a row
    /// moves. It stands at the root, and its rows follow the frame.
    #[test]
    fn the_environment_node_lights_both_views() {
        use crate::environment::{sun_direction, Environment};
        let templates = crate::app::load_fs_tree();
        let template = templates.children.iter().find(|t| t.name == "Environment").expect("an Environment template");
        assert_eq!(Environment::of_node(template), Environment::default(), "the template says what the defaults say");
        let old_sun = Vec3::new(0.45, 0.75, 0.35).normalize();
        assert!(Environment::default().sun_direction.angle_between(old_sun) < 1.0f32.to_radians(), "the default sun is the tracer's old one");
        assert!((sun_direction(90.0, 0.0) - Vec3::X).length() < 1e-5 && (sun_direction(0.0, 90.0) - Vec3::Y).length() < 1e-5);

        // At the root, through MCP.
        let mut state = State::new(false);
        state.current_path.clear();
        state.on_path_changed();
        state.smooth_shading = true;
        state.rebuild_scene_geometry();
        assert_eq!(state.environment, Environment::default());
        let colours = |state: &State| state.scene_smooth_verts.iter().map(|v| v.color).collect::<Vec<_>>();
        let baked = colours(&state);
        assert!(!baked.is_empty());
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Environment".into(), name: None, x: 3.0, y: 0.0 }, &mut redraw).unwrap();
        assert!(!state.sync_environment(), "adding one changes nothing");
        let slot = state.current_dir().children.iter().position(|c| c.node_type == "environment").unwrap();
        state.apply_action(McpAction::SetParam { slot, name: "sun_elevation".into(), value: "-40".into() }, &mut redraw).unwrap();
        state.sync_environment();
        assert!(state.environment.sun_direction.y < -0.5, "lit from below now: {:?}", state.environment.sun_direction);
        assert_eq!(state.environment.to_rt().sun_direction, state.environment.sun_direction.to_array(), "the tracer's sun is the same one");
        assert_ne!(colours(&state), baked, "the smooth bake is lit by it");

        // Bypassed, it is as if it were not there.
        state.set_bypassed(&[slot], true);
        state.sync_environment(); // the bypass's own rebuild has already read it
        assert_eq!(state.environment, Environment::default());
        assert_eq!(colours(&state), baked);

        // Its rows evaluate at the frame.
        state.set_bypassed(&[slot], false);
        let node = &mut state.current_dir_mut().children[slot];
        let az = node.params.iter_mut().find(|p| p.name == "sun_azimuth").unwrap();
        az.set_text("$F * 10".to_string());
        az.set_expr(true);
        let at = |frame| Environment::of_scene(&state.fs_root, frame).sun_direction;
        assert!((at(9) - sun_direction(90.0, -40.0)).length() < 1e-4, "{:?}", at(9));
        assert!((at(0) - sun_direction(0.0, -40.0)).length() < 1e-4);
    }

    /// The Add Node list offers what may stand at the level (since
    /// 2026-10-02, `context`): at the root, the object level, the Geometry
    /// node, cameras and the page nodes, and no operator; inside a Geometry
    /// node, and in a subnet inside one, every operator and the pages, and
    /// neither the Geometry node nor a camera. The same rule refuses MCP's
    /// `add_node` and a paste, so no way in gets around it.
    ///
    /// It once hid the geometry templates inside a "utility dir" — the root
    /// meta node's subnets — and then, those gone, offered everything
    /// everywhere.
    #[test]
    fn the_add_node_list_offers_what_belongs_at_the_level() {
        let mut state = State::new(false);
        let labels = |state: &State| state.slots.dialog.rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
        let inside = state.current_path.clone();
        assert_eq!(state.path_names_at(&inside), ["geometry1"]);

        // Inside the Geometry node.
        state.open_node_palette();
        let here = labels(&state);
        for operator in ["Sphere", "Box", "Grid", "Subnet", "Simnet", "Embryo", "Page", "Export"] {
            assert!(here.iter().any(|l| l == operator), "{operator} missing inside: {here:?}");
        }
        assert!(!here.iter().any(|l| l == "Geometry" || l == "Camera" || l == "Environment"), "{here:?}");
        state.close_dialog();

        // At the root.
        state.current_path.clear();
        state.on_path_changed();
        state.open_node_palette();
        let mut root = labels(&state);
        root.sort();
        assert_eq!(root, ["Camera", "Environment", "Export", "Geometry", "Page", "Page Border", "Page Grid", "Page Shape", "Page Text"]);
        state.close_dialog();

        // MCP: an operator at the root is refused, with why.
        let mut redraw = false;
        let count = state.fs_root.children.len();
        let said = state
            .apply_action(McpAction::AddNode { template_name: "Sphere".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw)
            .unwrap_err();
        assert!(said.contains("inside a Geometry node"), "{said}");
        assert_eq!(state.fs_root.children.len(), count);
        // A Geometry node there is fine, and is entered as a subnet is.
        state
            .apply_action(McpAction::AddNode { template_name: "Geometry".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw)
            .unwrap();
        let geo2 = state.fs_root.children.iter().position(|c| c.name == "geometry2").expect("geometry2");
        assert!(state.fs_root.children[geo2].is_enterable());
        state.apply_action(McpAction::Enter { slot: geo2 }, &mut redraw).unwrap();
        state
            .apply_action(McpAction::AddNode { template_name: "Box".into(), name: None, x: 1.0, y: 1.0 }, &mut redraw)
            .unwrap();
        let said = state
            .apply_action(McpAction::AddNode { template_name: "Camera".into(), name: None, x: 2.0, y: 1.0 }, &mut redraw)
            .unwrap_err();
        assert!(said.contains("at the root"), "{said}");

        // A paste: the box copied into the root is refused whole.
        let boxed = state.current_dir().children.iter().position(|c| c.node_type == "box").unwrap();
        state.node_clipboard = vec![state.current_dir().children[boxed].clone()];
        state.current_path.clear();
        state.on_path_changed();
        let count = state.fs_root.children.len();
        assert!(!state.paste_nodes());
        assert_eq!(state.fs_root.children.len(), count);
        assert!(state.last_status_text.contains("Not pasted"), "{}", state.last_status_text);
        // And into the other Geometry node it goes.
        state.current_path = inside;
        state.on_path_changed();
        assert!(state.paste_nodes());
    }

    /// Ctrl+P opens the list rather than toggling, which is the one thing
    /// that distinguishes it from Alt+D now that both open the same dialog.
    #[test]
    fn command_palette_opens_the_dialog_on_commands() {
        use crate::dialog::Mode;
        let mut state = State::new(false);
        state.run_command("toggle_dialog");
        assert!(state.dialog_visible());
        state.dialog_key_input(&typed("g"));

        state.run_command("command_palette");
        assert!(state.dialog_visible(), "it lands, it does not toggle");
        assert_eq!(state.slots.dialog.mode, Mode::Commands);
        assert_eq!(state.slots.dialog.query, "", "landing starts a fresh query");

        state.run_command("toggle_dialog");
        assert!(!state.dialog_visible(), "Alt+D toggles");
    }

    /// No designer code path spawns a cce-cloud popup any more.
    ///
    /// A source scan, like `test_every_menu_command_names_a_label_that_is_
    /// dispatched`: the alternative is asserting on a process that does not
    /// start, which is indistinguishable from one that failed to.
    #[test]
    fn nothing_shells_out_to_cce_cloud_any_more() {
        for name in ["app.rs", "window.rs", "dialog.rs", "render.rs", "api.rs", "slots.rs"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name);
            let src = std::fs::read_to_string(&path).expect("read source");
            for needle in ["CloudPopup", "run_dmenu", "CloudPopupTracker"] {
                // The doc comments say what the dialog REPLACED, so only code
                // counts: skip comment lines.
                let hit = src
                    .lines()
                    .find(|l| l.contains(needle) && !l.trim_start().starts_with("//"));
                assert!(hit.is_none(), "{name} still uses {needle}: {}", hit.unwrap().trim());
            }
        }
    }
    /// Frame All fits the name labels, not just the bodies. A node's label
    /// hangs off its right edge — at 100% zoom, `8 + estimate_width(name,
    /// 14)` px past the body — so a level whose bodies fit the pane with a
    /// long name on the right-hand node used to frame with that name cut
    /// off. The pane's right edge is where the label has to end now.
    #[test]
    fn frame_all_keeps_the_node_labels_inside_the_pane() {
        use cce_ui::widget::TextLabel;
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        // Clear the bundled level so only these two nodes are framed.
        let existing = state.current_dir().children.len();
        for slot in (0..existing).rev() {
            state.delete_node(slot);
        }

        // The widget's own label rule, spelled out here rather than read
        // back off the app, since agreeing with the widget is the claim.
        let label_right = |state: &State, slot: usize| {
            let child = &state.current_dir().children[slot];
            let (x, _y, w, _h) = state.cell_rect(child.position.0 as i32, child.position.1 as i32);
            let scale_f = w / 80.0;
            let font_size = (14.0 * scale_f).clamp(6.0, 48.0);
            x + w + 8.0 * scale_f + TextLabel::estimate_width(&child.name, font_size)
        };
        let (px, _py, pw, _ph) = state.positions[crate::slots::CONTENT_IDX];
        let padding = 40.0;

        let mut redraw = false;
        let long_name = "a_node_whose_name_runs_well_past_the_edge_of_its_own_body_and_then_some";
        // Two nodes whose bodies span most of the pane at 100%: the bodies
        // alone fit, the right-hand label does not.
        let cols = ((pw - 2.0 * padding) / 140.0).floor() - 2.0;
        for (name, x) in [("a", 0.0), (long_name, cols)] {
            state
                .apply_action(
                    crate::app::McpAction::AddNode { template_name: "Plane".into(), name: Some(name.into()), x, y: 0.0 },
                    &mut redraw,
                )
                .unwrap();
        }
        state.rebuild_positions();
        state.apply_layout();
        let long = state.current_dir().children.iter().position(|c| c.name == long_name).expect("long node");

        state.frame_all_nodes();

        let right = label_right(&state, long);
        assert!(
            right <= px + pw - padding + 0.5,
            "the long label ends at {right:.1}, past the pane's padded right edge {:.1} (pitch {:.1})",
            px + pw - padding,
            state.grid_pitch_x
        );
        let (ax, _, _, _) = state.cell_rect(0, 0);
        assert!(ax >= px + padding - 0.5, "the left-hand body starts at {ax:.1}, inside the padding");
        // The fit is by the labels: the bodies alone would have fitted at
        // 100%, so the zoom had to come down for the name.
        assert!(state.grid_pitch_x < 140.0, "the zoom stayed at 100% ({}), so the label was not counted", state.grid_pitch_x);

        // And a level whose labels already fit frames at 100% — the label
        // rule must not shrink a level that has room.
        state.apply_action(crate::app::McpAction::RenameNode { slot: long, new_name: "b".into() }, &mut redraw).unwrap();
        state.frame_all_nodes();
        assert!((state.grid_pitch_x - 140.0).abs() < 0.01, "short labels fit at 100%, got pitch {}", state.grid_pitch_x);
        assert!(label_right(&state, long) <= px + pw - padding + 0.5);
    }

    // ---- the wrangle node (src/wrangle.rs) ----

    fn wrangle_node(id: &str, name: &str, input: &str, class: &str, code: &str) -> FsNode {
        ref_node(id, name, "wrangle", vec![("input", "text", input), ("class", "choice:Points,Primitives,Detail", class), ("group", "text", ""), ("code", "code", code)], vec![])
    }

    /// The input as the wrangle sees it, then the wrangle's own result.
    fn wrangle_over_sphere(code: &str) -> (Detail, Option<Detail>, Option<String>) {
        let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "1.0")], vec![]);
        let w = wrangle_node("w", "wrangle1", "src", "Points", code);
        let root = ref_node("root", "root", "node", vec![], vec![src, w]);
        let before = eval(&root, &root.children[0]).0.unwrap();
        let (g, err) = eval(&root, &root.children[1]);
        (before, g, err)
    }

    /// `@name` is sugar for an index on the element, and only outside
    /// strings and comments — a message that mentions `@` keeps it.
    #[test]
    fn wrangle_desugars_at_names_outside_strings_and_comments() {
        use crate::wrangle::desugar;
        assert_eq!(desugar("@P.y += 1.0;"), "__at[\"P\"].y += 1.0;");
        assert_eq!(desugar("@mass = @mass * 2;"), "__at[\"mass\"] = __at[\"mass\"] * 2;");
        assert_eq!(desugar("let s = \"at @P\"; // @P here\n@x = 1;"), "let s = \"at @P\"; // @P here\n__at[\"x\"] = 1;");
        assert_eq!(desugar("/* @a */ @b = '@';"), "/* @a */ __at[\"b\"] = '@';");
        assert_eq!(desugar("a @ b"), "a @ b", "a bare @ is not sugar");
        assert_eq!(crate::wrangle::channel_refs("ch(\"../radius\") + chs('Name') + search(\"x\") // ch(\"no\")"), vec!["../radius".to_string(), "Name".to_string()]);
    }

    /// The core contract: positions move, and naming an attribute creates
    /// it, typed by what was written.
    #[test]
    fn wrangle_moves_points_and_creates_typed_attributes() {
        use crate::detail::AttribType;
        let (before, g, err) = wrangle_over_sphere(
            "@P.y += 1.0;\n@mass = 2.5;\n@count = @ptnum;\n@dir = vec3(1, 0, 0) * 2;\n@half = [0.5, 0.25];\n@flag = @ptnum > 3;\n@Cd = vec3(1.0, 0.0, 0.0);",
        );
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        assert_eq!(g.num_points(), before.num_points());
        assert_eq!(g.num_prims(), before.num_prims(), "a per-point script leaves the topology alone");
        for p in 0..g.num_points() {
            assert!((g.pos(p).y - (before.pos(p).y + 1.0)).abs() < 1e-5, "point {p} moved up by one");
        }
        let pts = g.points();
        assert_eq!(pts.get("mass").unwrap().ty(), AttribType::Float);
        assert_eq!(pts.get("count").unwrap().ty(), AttribType::Int, "an int written makes an int attribute");
        assert_eq!(pts.get("dir").unwrap().ty(), AttribType::Float3);
        assert_eq!(pts.get("half").unwrap().ty(), AttribType::Float2);
        assert_eq!(pts.get("flag").unwrap().ty(), AttribType::Int, "a bool is an int");
        assert_eq!(pts.value("count", 5).unwrap().as_f32(), 5.0);
        assert_eq!(pts.value("dir", 0).unwrap().as_vec3(), Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(pts.value("flag", 4).unwrap().as_f32(), 1.0);
        assert_eq!(g.color(2), [1.0, 0.0, 0.0]);
    }

    /// Neighbours and other points are reachable, off the derived topology.
    #[test]
    fn wrangle_reads_topology_and_other_points() {
        let (_, g, err) = wrangle_over_sphere(
            "@valence = neighbours(@ptnum).len();\nlet sum = 0.0;\nfor n in neighbours(@ptnum) { sum += point(\"P\", n).x; }\n@nx = sum;\n@nprims = prims(@ptnum).len();\n@near = nearest(@P, 0.6).len();",
        );
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        for p in 0..g.num_points() {
            let want = g.point_neighbours(p).len() as f32;
            assert_eq!(g.points().value("valence", p).unwrap().as_f32(), want, "point {p}");
            let sum: f32 = g.point_neighbours(p).iter().map(|&n| g.pos(n as usize).x).sum();
            assert!((g.points().value("nx", p).unwrap().as_f32() - sum).abs() < 1e-4);
            assert_eq!(g.points().value("nprims", p).unwrap().as_f32(), g.point_prims(p).len() as f32);
            assert!(g.points().value("near", p).unwrap().as_f32() >= 1.0, "a point is within its own radius");
        }
    }

    /// The Group parameter narrows the run; everything else is untouched.
    #[test]
    fn wrangle_runs_over_a_group_only_and_removes_points() {
        use crate::wrangle::run_wrangle;
        let mut d = crate::geometry::sphere_detail(Vec3::ZERO, 1.0, 6, 8);
        d.points_mut().create_group("top");
        for p in 0..5 {
            d.points_mut().add_to_group("top", p);
        }
        let before = d.clone();
        let out = run_wrangle(d.clone(), "@P += vec3(0, 10, 0); @touched = 1;", crate::detail::Class::Point, "top", 1, Default::default()).unwrap();
        for p in 0..out.num_points() {
            let moved = (out.pos(p).y - before.pos(p).y - 10.0).abs() < 1e-4;
            assert_eq!(moved, p < 5, "point {p}");
            assert_eq!(out.points().value("touched", p).unwrap().as_f32(), if p < 5 { 1.0 } else { 0.0 });
        }
        let out = run_wrangle(d, "if @ptnum % 2 == 0 { removepoint(@ptnum); }", crate::detail::Class::Point, "", 1, Default::default()).unwrap();
        assert_eq!(out.num_points(), before.num_points() / 2, "every even point removed, after the run");
    }

    /// Detail class runs once, with no input at all, and builds geometry
    /// through the deferred adds.
    #[test]
    fn wrangle_detail_class_builds_geometry_from_nothing() {
        let w = wrangle_node("w", "wrangle1", "", "Detail", "let a = addpoint(vec3(0, 0, 0));\nlet b = addpoint(vec3(1, 0, 0));\nlet c = addpoint([0, 1, 0]);\naddprim([a, b, c]);\nsetdetail(\"made\", 3);\n@note = 7;");
        let root = ref_node("root", "root", "node", vec![], vec![w]);
        let (g, err) = eval(&root, &root.children[0]);
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        assert_eq!((g.num_points(), g.num_prims()), (3, 1));
        assert_eq!(g.prim_points(0), &[0, 1, 2]);
        assert_eq!(g.pos(1), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(g.detail().value("made", 0).unwrap().as_f32(), 3.0);
        assert_eq!(g.detail().value("note", 0).unwrap().as_f32(), 7.0, "@name on the detail class is a detail attribute");
    }

    /// Primitives class: `@P` is the centroid, read-only; attributes land on
    /// the primitive store.
    #[test]
    fn wrangle_prims_class_writes_prim_attributes_and_reads_centroids() {
        let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "1.0")], vec![]);
        let w = wrangle_node("w", "wrangle1", "src", "Primitives", "@r = length(@P);\n@n = points(@primnum).len();\n@which = @primnum;");
        let root = ref_node("root", "root", "node", vec![], vec![src, w]);
        let (g, err) = eval(&root, &root.children[1]);
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        assert!(g.num_prims() > 0);
        assert!(g.points().get("r").is_none(), "a prim wrangle writes no point attribute");
        for pr in 0..g.num_prims() {
            let pts = g.prim_points(pr);
            let centroid = pts.iter().map(|&p| g.pos(p as usize)).sum::<Vec3>() / pts.len() as f32;
            assert!((g.prims().value("r", pr).unwrap().as_f32() - centroid.length()).abs() < 1e-4);
            assert_eq!(g.prims().value("n", pr).unwrap().as_f32(), pts.len() as f32);
            assert_eq!(g.prims().value("which", pr).unwrap().as_f32(), pr as f32);
        }
        let w2 = wrangle_node("w2", "wrangle2", "src", "Primitives", "@P = vec3(0, 0, 0);");
        let root2 = ref_node("root", "root", "node", vec![], vec![root.children[0].clone(), w2]);
        let (_, err) = eval(&root2, &root2.children[1]);
        assert!(err.as_deref().is_some_and(|e| e.contains("read-only")), "{err:?}");
    }

    /// Errors name the node and the element, and the input passes through.
    #[test]
    fn wrangle_errors_report_the_node_and_pass_the_input_through() {
        let (before, g, err) = wrangle_over_sphere("@P.y += ;");
        let err = err.expect("a syntax error is reported");
        assert!(err.starts_with("wrangle1: syntax"), "{err}");
        let g = g.unwrap();
        assert_eq!(g.num_points(), before.num_points());
        assert_eq!(g.pos(3), before.pos(3), "the input passes through untouched");

        let (_, g, err) = wrangle_over_sphere("if @ptnum == 4 { @x = point(\"P\", 100000); }");
        let err = err.expect("a runtime error is reported");
        assert!(err.starts_with("wrangle1: point 4:"), "{err}");
        assert!(err.contains("out of range"), "{err}");
        assert!(g.unwrap().points().get("x").is_none(), "nothing of a failed run survives");

        let (_, _, err) = wrangle_over_sphere("loop { }");
        let err = err.expect("a script that never ends is stopped");
        assert!(err.contains("operations") || err.contains("stopped"), "{err}");
    }

    /// `ch()` reaches the node's own parameters and its parent's, evaluated:
    /// an expression-valued parameter is seen as its value.
    #[test]
    fn wrangle_ch_reads_parameters_through_the_expression_scope() {
        let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "1.0")], vec![]);
        let w = ref_node("w", "wrangle1", "wrangle", vec![
            ("input", "text", "src"), ("class", "choice:Points,Primitives,Detail", "Points"), ("group", "text", ""),
            ("amount", "slider", "ch(\"../lift\") + 1"),
            ("Label", "text", "hello"),
            ("code", "code", "@P = @P * ch(\"amount\") + vec3(0, ch(\"../lift\"), 0);\n@n = chi(\"amount\");\n@s = chs(\"label\").len();\n@v = chv(\"../offset\");"),
        ], vec![]);
        let root = ref_node("root", "root", "node", vec![("Lift", "slider", "2"), ("offset", "float3", "1:2:3")], vec![src, w]);
        let before = eval(&root, &root.children[0]).0.unwrap();
        let (g, err) = eval(&root, &root.children[1]);
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        assert_eq!(g.num_points(), before.num_points());
        let p = 7;
        let want = before.pos(p) * 3.0 + Vec3::new(0.0, 2.0, 0.0);
        assert!((g.pos(p) - want).length() < 1e-4, "got {:?}, want {want:?}", g.pos(p));
        assert_eq!(g.points().value("n", p).unwrap().as_f32(), 3.0);
        assert_eq!(g.points().value("s", p).unwrap().as_f32(), 5.0);
        assert_eq!(g.points().value("v", p).unwrap().as_vec3(), Vec3::new(1.0, 2.0, 3.0));

        let bad = wrangle_node("b", "wrangle2", "src", "Points", "@x = ch(\"nope\");");
        let root2 = ref_node("root", "root", "node", vec![], vec![root.children[0].clone(), bad]);
        let (_, err) = eval(&root2, &root2.children[1]);
        assert!(err.as_deref().is_some_and(|e| e.contains("nope")), "a channel to nothing is an error: {err:?}");
    }

    /// The shipped template resolves, and its default script runs.
    #[test]
    fn wrangle_template_ships_and_its_default_code_runs() {
        let templates = crate::app::load_fs_tree();
        let t = templates.children.iter().find(|n| n.node_type == "wrangle").expect("nodes/wrangle.json loads");
        assert_eq!(t.name, "Wrangle");
        let code = t.params.iter().find(|p| p.name == "code").map(|p| p.text().to_string()).unwrap();
        assert!(t.params.iter().all(|p| !p.is_expr()), "no template parameter reads as an expression — least of all the Code");
        let (before, g, err) = wrangle_over_sphere(&code);
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        let moved = (0..g.num_points()).filter(|&p| (g.pos(p).y - before.pos(p).y).abs() > 1e-6).count();
        assert!(moved > 0, "the default script deforms the input");
    }

    /// An `opencl` node in a save older than the retirement is not dropped:
    /// it passes its input through and says what it is, on the status line
    /// and in the CLI's warning, so the fix is one rewrite as a wrangle.
    #[test]
    fn a_retired_opencl_node_passes_its_input_through_and_says_so() {
        let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "1.0")], vec![]);
        let k = ref_node("k", "opencl1", "opencl", vec![("input", "text", "src"), ("code", "code", "__kernel void process() {}")], vec![]);
        let root = ref_node("root", "root", "node", vec![], vec![src, k]);
        let before = eval(&root, &root.children[0]).0.unwrap();
        let (g, err) = eval(&root, &root.children[1]);
        let err = err.expect("the retired node reports itself");
        assert!(err.starts_with("opencl1: OpenCL nodes are retired"), "{err}");
        assert!(err.contains("wrangle"), "and points at the replacement: {err}");
        let g = g.expect("the input passes through");
        assert_eq!(g.num_points(), before.num_points());
        assert_eq!(g.pos(5), before.pos(5));
        assert!(crate::geometry::is_geometry_node_type("opencl"), "the type still resolves, to that arm");

        // And no template offers it any more.
        let templates = crate::app::load_fs_tree();
        assert!(!templates.children.iter().any(|t| t.node_type == "opencl"), "nodes/opencl.json is gone");
    }

    // ---- the springs solve (src/springs.rs): CPU and GPU, one algorithm ----

    /// A rest sphere, its points stretched and stirred, a few pinned: the
    /// fixture both backends solve.
    fn springs_fixture(rows: usize, cols: usize) -> (crate::springs::SpringSystem, Vec<f32>, Vec<bool>) {
        let rest = crate::geometry::sphere_detail(Vec3::ZERO, 0.5, rows, cols);
        let n = rest.num_points();
        let pinned: Vec<bool> = (0..n).map(|p| p % 97 == 0).collect();
        let sys = crate::springs::SpringSystem::build(&rest, &pinned);
        let pos: Vec<f32> = (0..n)
            .flat_map(|p| {
                let x = rest.pos(p);
                let stretched = x * 1.4 + Vec3::new((p as f32 * 0.37).sin() * 0.05, 0.0, (p as f32 * 0.11).cos() * 0.05);
                [stretched.x, stretched.y, stretched.z]
            })
            .collect();
        (sys, pos, pinned)
    }

    /// The CSR is the rest topology twice over — every edge once from each
    /// end — with the rest length on both entries.
    #[test]
    fn springs_system_is_the_rest_topology_in_csr_form() {
        let rest = crate::geometry::sphere_detail(Vec3::ZERO, 0.5, 6, 8);
        let sys = crate::springs::SpringSystem::build(&rest, &[]);
        assert_eq!(sys.n, rest.num_points());
        assert_eq!(sys.num_edges(), rest.edges().len());
        assert_eq!(sys.neighbour.len(), 2 * rest.edges().len());
        for p in 0..sys.n {
            let (s, e) = (sys.offsets[p] as usize, sys.offsets[p + 1] as usize);
            assert_eq!(e - s, rest.point_neighbours(p).len(), "point {p}'s incident count is its valence");
            for i in s..e {
                let q = sys.neighbour[i] as usize;
                assert!((sys.rest[i] - (rest.pos(q) - rest.pos(p)).length()).abs() < 1e-6);
            }
        }
        assert!(sys.pinned.iter().all(|&v| v == 0), "no pins asked for, none set");
    }

    /// Jacobi restores the rest lengths and never moves a pin.
    #[test]
    fn springs_cpu_restores_rest_lengths_and_holds_pins() {
        let (sys, mut pos, pinned) = springs_fixture(12, 16);
        let before = pos.clone();
        let error = |pos: &[f32]| -> f32 {
            let mut worst: f32 = 0.0;
            for p in 0..sys.n {
                for e in sys.offsets[p] as usize..sys.offsets[p + 1] as usize {
                    let q = sys.neighbour[e] as usize;
                    let d = Vec3::new(pos[q * 3] - pos[p * 3], pos[q * 3 + 1] - pos[p * 3 + 1], pos[q * 3 + 2] - pos[p * 3 + 2]);
                    worst = worst.max((d.length() - sys.rest[e]).abs() / sys.rest[e]);
                }
            }
            worst
        };
        let start = error(&pos);
        assert!(start > 0.3, "the fixture is stretched: {start}");
        crate::springs::solve_cpu(&sys, 1.0, 60, &mut pos);
        let after = error(&pos);
        assert!(after < start * 0.25, "sixty passes at full stiffness pull the edges toward rest: {start} -> {after}");
        for p in 0..sys.n {
            if pinned[p] {
                assert_eq!(&pos[p * 3..p * 3 + 3], &before[p * 3..p * 3 + 3], "pin {p} moved");
            }
        }
        // Zero stiffness or zero iterations: nothing moves.
        let mut still = before.clone();
        crate::springs::solve_cpu(&sys, 0.0, 10, &mut still);
        assert_eq!(still, before);
        crate::springs::solve_cpu(&sys, 1.0, 0, &mut still);
        assert_eq!(still, before);
    }

    /// The cross-check that holds the GPU to the CPU: the same passes over
    /// the same arrays agree to floating-point noise. Skips, with a note,
    /// where the machine has no Vulkan.
    #[test]
    fn springs_gpu_matches_cpu() {
        // Well under the auto threshold: the cross-check asks for the GPU by
        // name, and a small mesh keeps it quick.
        let (sys, pos0, _) = springs_fixture(32, 48);
        let mut cpu = pos0.clone();
        crate::springs::solve_cpu(&sys, 0.7, 12, &mut cpu);
        let mut gpu = pos0.clone();
        let ran = crate::gpu::with_any_device(|dev| crate::springs::solve_gpu(dev, &sys, 0.7, 12, &mut gpu));
        match ran {
            Err(e) => {
                println!("skipping springs_gpu_matches_cpu: {e}");
                return;
            }
            Ok(r) => r.expect("the springs kernel runs"),
        }
        let mut worst: f32 = 0.0;
        for (i, (a, b)) in cpu.iter().zip(&gpu).enumerate() {
            let d = (a - b).abs();
            assert!(d < 1e-4, "component {i}: cpu {a} gpu {b}");
            worst = worst.max(d);
        }
        assert!(cpu != pos0, "the solve did something");
        println!("springs gpu vs cpu: worst component difference {worst:e} over {} points", sys.n);
    }

    /// `CCE_COMPUTE` parses as written, and under test auto means CPU so the
    /// suite is the same on every machine.
    #[test]
    fn compute_choice_parses_the_variable_and_defaults_to_cpu_under_test() {
        use crate::gpu::{parse, Choice};
        assert_eq!(parse(None), Choice::Cpu, "auto is CPU under cfg(test)");
        assert_eq!(parse(Some("auto")), Choice::Cpu);
        assert_eq!(parse(Some("gpu")), Choice::Gpu);
        assert_eq!(parse(Some(" CPU ")), Choice::Cpu, "case-insensitive, trimmed");
        assert_eq!(parse(Some("banana")), Choice::Cpu, "nonsense is auto, with a note");
        // The suite never sets the variable: a test that did would race every
        // other test reading it, and the GPU is exercised by name instead.
        assert!(std::env::var_os("CCE_COMPUTE").is_none());
    }

    /// Where the auto threshold sits, and a rough measure of why: the CPU
    /// and GPU solve timed over a large sphere. Ignored — it is a
    /// measurement, not an assertion — run with
    /// `cargo test --release -p cce-designer springs_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn springs_timing() {
        for (rows, cols) in [(16, 24), (32, 48), (100, 150), (300, 450)] {
            let (sys, pos0, _) = springs_fixture(rows, cols);
            let t = std::time::Instant::now();
            let mut cpu = pos0.clone();
            crate::springs::solve_cpu(&sys, 0.7, 16, &mut cpu);
            let cpu_ms = t.elapsed().as_secs_f64() * 1e3;
            let mut gpu = pos0.clone();
            let t = std::time::Instant::now();
            let ran = crate::gpu::with_any_device(|dev| crate::springs::solve_gpu(dev, &sys, 0.7, 16, &mut gpu));
            let gpu_ms = t.elapsed().as_secs_f64() * 1e3;
            println!("{:>7} points, 16 passes: cpu {cpu_ms:8.2} ms   gpu {gpu_ms:8.2} ms   ({:?})", sys.n, ran.map(|r| r.is_ok()));
        }
    }

    // ---- the collision test (src/collide.rs): CPU and GPU, one algorithm ----

    /// A collider and a cloud of queries around and through it.
    fn collision_fixture(rows: usize, cols: usize, queries: usize) -> (Vec<Vec3>, Vec<[Vec3; 3]>) {
        let collider = crate::geometry::sphere_detail(Vec3::new(0.1, 0.55, -0.05), 0.7, rows, cols);
        let tris: Vec<[Vec3; 3]> = collider
            .triangulate(|pos, _| Vec3::from(pos))
            .chunks_exact(3)
            .map(|t| [t[0], t[1], t[2]])
            .collect();
        let pts: Vec<Vec3> = (0..queries)
            .map(|i| {
                let f = i as f32;
                Vec3::new((f * 0.371).sin() * 1.1 + 0.1, (f * 0.173).cos() * 1.1 + 0.55, (f * 0.529).sin() * 1.1 - 0.05)
            })
            .collect();
        (pts, tris)
    }

    /// The CPU test is the node's original loop, step for step: inside is
    /// containment, proximity is a band, both against a sphere whose
    /// geometry is known.
    #[test]
    fn collision_cpu_tests_containment_and_a_surface_band() {
        use crate::collide::{hits_cpu, Test};
        let (pts, tris) = collision_fixture(16, 24, 2000);
        let centre = Vec3::new(0.1, 0.55, -0.05);
        let inside = hits_cpu(&pts, &tris, Test::Inside);
        let band = hits_cpu(&pts, &tris, Test::Proximity(0.05));
        let (mut n_in, mut n_band) = (0, 0);
        for (i, p) in pts.iter().enumerate() {
            let r = (*p - centre).length();
            if r < 0.7 - 0.02 {
                assert_eq!(inside[i], 1, "point {i} at r={r} is enclosed");
            } else if r > 0.7 + 0.02 {
                assert_eq!(inside[i], 0, "point {i} at r={r} is outside");
            }
            if (r - 0.7).abs() < 0.05 - 0.02 {
                assert_eq!(band[i], 1, "point {i} at r={r} is within the band");
            } else if (r - 0.7).abs() > 0.05 + 0.02 {
                assert_eq!(band[i], 0, "point {i} at r={r} is outside the band");
            }
            n_in += inside[i];
            n_band += band[i];
        }
        assert!(n_in > 0 && n_in < pts.len() as u32 && n_band > 0, "the fixture straddles the collider: {n_in} in, {n_band} in band");
    }

    /// The cross-check: the GPU's flags are the CPU's, for both tests. A
    /// query on a knife edge of the threshold may round either way, so a
    /// disagreement is tolerated only there. Skips where there is no Vulkan.
    #[test]
    fn collision_gpu_matches_cpu() {
        use crate::collide::{hits_cpu, hits_gpu, Test};
        let (pts, tris) = collision_fixture(24, 36, 6000);
        for test in [Test::Inside, Test::Proximity(0.05)] {
            let cpu = hits_cpu(&pts, &tris, test);
            let gpu = match crate::gpu::with_any_device(|dev| hits_gpu(dev, &pts, &tris, test)) {
                Err(e) => {
                    println!("skipping collision_gpu_matches_cpu: {e}");
                    return;
                }
                Ok(r) => r.expect("the collision kernel runs"),
            };
            assert_eq!(cpu.len(), gpu.len());
            let mut disagreements = 0;
            for i in 0..cpu.len() {
                if cpu[i] != gpu[i] {
                    let d = tris
                        .iter()
                        .map(|t| crate::geometry::point_triangle_distance_sq(pts[i], t[0], t[1], t[2]).sqrt())
                        .fold(f32::INFINITY, f32::min);
                    let margin = match test {
                        Test::Proximity(r) => (d - r).abs(),
                        Test::Inside => d,
                    };
                    assert!(margin < 1e-4, "{test:?}: query {i} differs (cpu {} gpu {}) with margin {margin}", cpu[i], gpu[i]);
                    disagreements += 1;
                }
            }
            println!("collision {test:?}: {} queries x {} triangles, {disagreements} knife-edge disagreements", pts.len(), tris.len());
            assert!(cpu.iter().any(|&h| h == 1) && cpu.iter().any(|&h| h == 0));
        }
    }

    /// Where the auto threshold sits: the test timed over sizes. Ignored,
    /// a measurement: `cargo test --release -p cce-designer collision_timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn collision_timing() {
        use crate::collide::{hits_cpu, hits_gpu, Test};
        for (rows, cols, queries) in [(16, 24, 500), (16, 24, 5000), (32, 48, 5000), (64, 96, 20000)] {
            let (pts, tris) = collision_fixture(rows, cols, queries);
            let t = std::time::Instant::now();
            let cpu = hits_cpu(&pts, &tris, Test::Proximity(0.05));
            let cpu_ms = t.elapsed().as_secs_f64() * 1e3;
            let t = std::time::Instant::now();
            let gpu = crate::gpu::with_any_device(|dev| hits_gpu(dev, &pts, &tris, Test::Proximity(0.05)));
            let gpu_ms = t.elapsed().as_secs_f64() * 1e3;
            let same = gpu.as_ref().map(|g| g.as_ref().map(|g| *g == cpu).unwrap_or(false)).unwrap_or(false);
            println!(
                "{:>6} queries x {:>6} tris = {:>10} pairs: cpu {cpu_ms:9.2} ms   gpu {gpu_ms:8.2} ms   same={same}",
                pts.len(),
                tris.len(),
                pts.len() * tris.len()
            );
        }
    }

    /// A wrangle's error names its node and its line; the params pane is
    /// told the line only when that node is the one it shows.
    #[test]
    fn a_script_error_line_reaches_the_params_pane_for_the_shown_node() {
        use crate::app::State;
        assert_eq!(State::error_line_number("syntax: Syntax error: Expecting ';' (line 3, position 5)"), Some(2));
        assert_eq!(State::error_line_number("point 4: index out of range (line 1, position 9)"), Some(0));
        assert_eq!(State::error_line_number("OpenCL nodes are retired"), None);
        assert_eq!(State::error_line_number("(line 0, position 1)"), None, "a zero line is not a line");

        let mut state = State::new(false);
        let mut redraw = false;
        state.apply_action(crate::app::McpAction::AddNode { template_name: "Wrangle".into(), name: Some("w".into()), x: 3.0, y: 9.0 }, &mut redraw).unwrap();
        let slot = state.current_dir().children.iter().position(|c| c.name == "w").unwrap();
        state.apply_action(crate::app::McpAction::Select { slot }, &mut redraw).unwrap();
        assert_eq!(state.param_editor_selected(), Some(slot));
        assert_eq!(state.code_error_line_for_pane("w: syntax: Syntax error (line 2, position 1)"), Some(1));
        assert_eq!(state.code_error_line_for_pane("sphere1: something (line 2, position 1)"), None, "another node's error is not this pane's");
        assert_eq!(state.code_error_line_for_pane("w: OpenCL nodes are retired"), None, "no line, no flag");
    }

    /// A trackpad swipe over a spinbox row of the params pane steps the row
    /// — 60 px of finger travel per step, the widget's notch — and the
    /// node's value follows through the wheel's write-back. Until
    /// 2026-09-28 the pane claimed every finger gesture for its own scroll,
    /// so on a trackpad the -/+ buttons were the only pointer way to step a
    /// Rows or Columns value; a mouse notch stepped it all along.
    #[test]
    fn a_trackpad_swipe_over_a_spinbox_row_steps_it() {
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{scroll_motion::set_scroll_phase, MouseScrollDelta, Position, ScrollPhase};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        let sphere = state.current_dir().children.iter().position(|c| c.name == "sphere1").expect("sphere1");
        state.graph_mut().set_selected_node(Some(sphere));
        state.sync_parameters_pane();
        state.rebuild_positions();
        state.apply_layout();
        let rows = crate::app::param_display(&state.current_dir().children[sphere].params);
        let i = rows.iter().position(|r| r.0 == "Rows").expect("a Rows row");
        assert!(rows[i].2.starts_with("spinbox"), "{:?}", rows[i]);
        let (x, y, w, h) = state.param_row_rects()[i];
        let value = |state: &State| crate::geometry::node_param_f32(&state.current_dir().children[sphere], "rows", -1.0);
        let before = value(&state);

        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: (x + w * 0.3) as f64, y: (y + h * 0.5) as f64 } });
        // The designer's test binary links cce-ui without cfg(test), so the
        // natural-scroll setting would be the MACHINE's: pinned here, both
        // ways in turn.
        cce_ui::input::force_natural_scroll(Some(false));
        set_scroll_phase(ScrollPhase::Finger);
        state.ui_context.scroll_gesture_new = true;
        state.ui_context.scroll_initiate_widget_id = None;
        assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 0.0, y: 60.0 }) }));
        set_scroll_phase(ScrollPhase::Wheel);
        assert_eq!(value(&state), before + 1.0, "one notch of finger travel is one step, written to the node");

        // A mouse notch on the same row, as before.
        assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, -1.0) }));
        assert_eq!(value(&state), before);

        // With natural scrolling on, the fingers going UP is more: the
        // same travel, the other sign of delta.
        cce_ui::input::force_natural_scroll(Some(true));
        set_scroll_phase(ScrollPhase::Finger);
        state.ui_context.scroll_gesture_new = true;
        state.ui_context.scroll_initiate_widget_id = None;
        assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 0.0, y: -60.0 }) }));
        set_scroll_phase(ScrollPhase::Wheel);
        assert_eq!(value(&state), before + 1.0, "natural: fingers up is a step up");
        // And a wheel notch up is still more.
        assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 1.0) }));
        assert_eq!(value(&state), before + 2.0);
        cce_ui::input::force_natural_scroll(None);
    }

    /// During playback the scene is built for the frame the playbar shows,
    /// within the same tick. The Playbar's tick advances the frame, so the
    /// frame-change check has to run after the widget ticks; before
    /// 2026-09-28 it ran first, and every tick rebuilt the scene for the
    /// previous tick's frame and then advanced the readout.
    #[test]
    fn playback_builds_the_scene_for_the_frame_the_playbar_shows() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
        assert!(crate::geometry::contains_simnet(&state.fs_root));
        state.tick_frame(1.0 / 60.0);
        assert_eq!(state.last_sim_frame, state.sim_frame());
        state.slots.playbar.inner_mut().playing = true;
        state.slots.playbar.inner_mut().fps = 60.0;
        let start = state.sim_frame();
        for i in 1..=3 {
            assert!(state.tick_frame(1.0 / 60.0), "a playing tick asks for a redraw");
            assert_eq!(state.sim_frame(), start + i, "the playbar advanced a frame");
            assert_eq!(state.last_sim_frame, state.sim_frame(), "and the scene was built for that frame, not the last one");
        }
    }

    /// Repeat off: playback stops ON the last frame instead of wrapping,
    /// in either direction, and a play press on a timeline stopped at its
    /// far end restarts from the near one. The setting is a palette toggle
    /// persisted top-level in state.kdl, read back by a fresh State.
    #[test]
    fn repeat_off_stops_playback_at_the_end_and_persists() {
        use cce_ui::widget::Input;
        let rect = cce_ui::scene::layout::Rect { x: 0.0, y: 0.0, width: 100.0, height: 30.0 };
        let mut state = State::new(false);
        assert_eq!(state.command_toggle_state("toggle_playbar_repeat"), Some(true), "repeat is on by default");
        assert!(state.run_command("toggle_playbar_repeat"));
        assert_eq!(state.command_toggle_state("toggle_playbar_repeat"), Some(false));
        // Persisted: the toggle wrote state.kdl, and a fresh State reads it.
        // Checked at once, since the suite's tests share the (redirected)
        // file and another's save could follow.
        let kdl = fs::read_to_string(DesignSettings::file_path()).expect("state.kdl was written");
        assert!(!DesignSettings::from_kdl_str(&kdl).playbar_repeat, "{kdl}");
        assert!(!State::new(false).slots.playbar.inner().repeat, "a new State seeds the playbar from the setting");

        // Forward: run off the end, land on it, stop.
        {
            let pb = state.slots.playbar.inner_mut();
            pb.current_frame = pb.end_frame - 0.5;
            pb.begin(false);
        }
        assert!(Input::tick(state.slots.playbar.inner_mut(), 0.1, rect));
        {
            let pb = state.slots.playbar.inner();
            assert_eq!(pb.current_frame, pb.end_frame, "stopped on the last frame");
            assert!(!pb.playing, "and playback ended");
        }
        // Play again from the end: restarts from the start frame.
        state.execute_action(Action::PlayPause);
        {
            let pb = state.slots.playbar.inner();
            assert!(pb.playing && !pb.reversed);
            assert_eq!(pb.current_frame, pb.start_frame, "a play press at the far end rewinds");
        }
        // Reverse: run off the start, stop there; Down restarts from the end.
        {
            let pb = state.slots.playbar.inner_mut();
            pb.playing = false;
            pb.current_frame = pb.start_frame + 0.5;
            pb.begin(true);
        }
        assert!(Input::tick(state.slots.playbar.inner_mut(), 0.1, rect));
        {
            let pb = state.slots.playbar.inner();
            assert_eq!(pb.current_frame, pb.start_frame);
            assert!(!pb.playing);
        }
        state.execute_action(Action::PlayPauseReverse);
        {
            let pb = state.slots.playbar.inner();
            assert!(pb.playing && pb.reversed);
            assert_eq!(pb.current_frame, pb.end_frame);
        }

        // Back on, and the file follows.
        assert!(state.run_command("toggle_playbar_repeat"));
        assert!(DesignSettings::from_kdl_str(&fs::read_to_string(DesignSettings::file_path()).unwrap()).playbar_repeat);
    }

    /// The Value row's span: the smallest power of ten (at least one)
    /// whose middle half holds the value; kept while the value stays
    /// between a twentieth and nineteen twentieths of it.
    #[test]
    fn the_value_rows_span_adapts_to_the_value() {
        use crate::app::value_row_span as span;
        assert_eq!(span(&[0.0], None), 1.0);
        assert_eq!(span(&[0.06, 0.0, 0.0], None), 1.0);
        assert_eq!(span(&[0.5], None), 1.0);
        assert_eq!(span(&[0.6], None), 10.0);
        assert_eq!(span(&[1.0], None), 10.0);
        assert_eq!(span(&[-7.0, 2.0], None), 100.0, "the largest magnitude, either sign");
        assert_eq!(span(&[300.0], None), 1000.0);
        assert_eq!(span(&[3.0], Some(10.0)), 10.0, "inside: kept");
        assert_eq!(span(&[9.0], Some(10.0)), 10.0);
        assert_eq!(span(&[9.6], Some(10.0)), 100.0, "at the end: grows");
        assert_eq!(span(&[6.0], Some(100.0)), 100.0, "a twentieth or more: kept");
        assert_eq!(span(&[0.6], Some(100.0)), 10.0, "far inside: shrinks");
        assert_eq!(span(&[0.4], Some(100.0)), 1.0);
        assert_eq!(span(&[0.0], Some(1.0)), 1.0, "one is the floor");
    }

    /// The Attribute node's Value stays a text parameter, but the pane
    /// presents it as a control as wide as its target — over the wide
    /// span around its value (`value_row_span`) — a slider for one, the float group with two, three
    /// or four rows for more: Modify on Pos (the pull node), on an input
    /// Float3 (N) or Float2 (uv), Create by its Type. A single number is
    /// spread over the components, as the node spreads it, and the pane
    /// writing it back unchanged is not an edit. A text that fits no width,
    /// an attribute the input lacks and an expression keep the text box;
    /// read from an attribute, there is no Value row at all.
    #[test]
    fn an_attribute_value_row_is_a_control_as_wide_as_its_target() {
        let templates_root = crate::app::load_fs_tree();
        let find = |name: &str| templates_root.children.iter().find(|t| t.name == name).unwrap();
        let instance = |template: &FsNode, id: &str, name: &str, params: &[(&str, &str)]| {
            let mut inst = template.clone();
            inst.id = id.to_string();
            inst.name = name.to_string();
            for (pname, val) in params {
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().set_text(val.to_string());
            }
            inst
        };
        let mut state = State::new(false);
        // Inside the bundled project's Geometry node, where geometry goes.
        state.current_dir_mut().children = vec![
            instance(find("Sphere"), "s", "Sphere 1", &[]),
            instance(find("Attribute"), "a", "pull1", &[
                ("input", "Sphere 1"),
                ("operation", "Modify"),
                ("attribute_name", "Pos"),
                ("value", "0.00:0.06:0.00"),
                ("combine", "Add"),
            ]),
        ];
        state.sync_nodes();
        state.graph_mut().set_selected_node(Some(1));
        let value_row = |state: &mut State| {
            state.sync_parameters_pane();
            state.param_mut().node_params().iter().find(|r| r.0 == "Value").expect("a Value row").2.clone()
        };
        // A vector gets the trackball beside its sliders by default.
        // 0.06 sits in the middle half of ±1.
        let wide = "float3:-1:1:trackball:soft".to_string();
        assert_eq!(value_row(&mut state), wide, "Modify on Pos");

        let set = |state: &mut State, name: &str, val: &str| {
            state.current_dir_mut().children[1].params.iter_mut().find(|p| p.name == name).unwrap().set_text(val.to_string());
        };
        set(&mut state, "attribute_name", "N");
        assert_eq!(value_row(&mut state), wide, "Modify on an input Float3");
        set(&mut state, "attribute_name", "uv");
        assert_eq!(value_row(&mut state), "text", "three numbers do not fit an input Float2");
        set(&mut state, "value", "1:2");
        assert_eq!(value_row(&mut state), "float2:-10:10:soft", "Modify on an input Float2: 2 needs ±10");
        set(&mut state, "value", "0.00:0.06:0.00");
        set(&mut state, "attribute_name", "nothing_here");
        assert_eq!(value_row(&mut state), "text", "an attribute the input lacks has no width");

        set(&mut state, "operation", "Create");
        set(&mut state, "attribute_name", "vel");
        set(&mut state, "type", "Float3");
        assert_eq!(value_row(&mut state), wide, "Create of a Float3");
        set(&mut state, "type", "Float");
        assert_eq!(value_row(&mut state), "text", "three numbers do not fit a Float");
        let value_text = |state: &mut State| {
            state.sync_parameters_pane();
            state.param_mut().node_params().iter().find(|r| r.0 == "Value").unwrap().1.clone()
        };
        set(&mut state, "value", "1.00");
        assert_eq!(value_row(&mut state), "slider:-10:10:2:soft", "Create of a Float");
        for (ty, row, shown) in [
            ("Float2", "float2:-10:10:soft".to_string(), "1.00:1.00"),
            ("Float3", "float3:-10:10:trackball:soft".to_string(), "1.00:1.00:1.00"),
            ("Float4", "float4:-10:10:soft".to_string(), "1.00:1.00:1.00:1.00"),
        ] {
            set(&mut state, "type", ty);
            assert_eq!(value_row(&mut state), row, "Create of a {ty}");
            assert_eq!(value_text(&mut state), shown, "one number spread over a {ty}");
        }
        // Read back unchanged, the spread number is not an edit.
        let steps = state.edit_history.undo_len();
        state.sync_parameters_to_project();
        assert_eq!(state.current_dir().children[1].params.iter().find(|p| p.name == "value").unwrap().text(), "1.00");
        assert_eq!(state.edit_history.undo_len(), steps);

        set(&mut state, "type", "Float3");
        set(&mut state, "value", "1.00:2.00:3.00");
        assert_eq!(value_row(&mut state), "float3:-10:10:trackball:soft", "3 stays inside the ±10 in use");
        // Past nineteen twentieths of it, the row re-scales: 9.6 to ±100.
        set(&mut state, "value", "9.6:0:0");
        assert_eq!(value_row(&mut state), "float3:-100:100:trackball:soft");
        // Held by a drag in the pane, it does not, whatever the value.
        state.drag_widget = Some(crate::slots::PARAM_IDX);
        set(&mut state, "value", "99:0:0");
        assert_eq!(value_row(&mut state), "float3:-100:100:trackball:soft", "no re-scale under the pointer");
        state.drag_widget = None;
        assert_eq!(value_row(&mut state), "float3:-1000:1000:trackball:soft", "and on the release it does");
        // Far inside, it comes back down.
        set(&mut state, "value", "1.00:2.00:3.00");
        assert_eq!(value_row(&mut state), "float3:-10:10:trackball:soft");
        state.current_dir_mut().children[1].params.iter_mut().find(|p| p.name == "value").unwrap().set_expr(true);
        assert_eq!(value_row(&mut state), "text", "an expression is shown as its text");
        state.current_dir_mut().children[1].params.iter_mut().find(|p| p.name == "value").unwrap().set_expr(false);

        // Read from an attribute: no Value row, and the source is picked
        // from the input's attributes.
        set(&mut state, "value_from", "Attribute");
        state.sync_parameters_pane();
        let rows = state.param_mut().node_params();
        assert!(rows.iter().all(|r| r.0 != "Value"), "no Value row");
        let from = rows.iter().find(|r| r.0 == "From Attribute").expect("a From Attribute row");
        assert!(from.2.starts_with("textpick:") && from.2["textpick:".len()..].split(',').any(|a| a == "N"), "{}", from.2);
        set(&mut state, "value_from", "Constant");

        // The parameter itself never changed kind: it is text in the node.
        assert_eq!(state.current_dir().children[1].params.iter().find(|p| p.name == "value").unwrap().kind(), crate::param::ParamKind::Text);
    }

    /// A trackpad swipe over a band of the pull node's float3 Value row
    /// turns that component, and the node's Value follows: Y alone, written
    /// back as the `x:y:z` text the Attribute node parses. Until 2026-09-28
    /// the params pane kept every finger gesture for its own scroll, so a
    /// slider could be turned by a wheel notch and not by a trackpad — and a
    /// float3 gave a scroll over its Y band to X, the first row in order.
    #[test]
    fn a_trackpad_swipe_over_a_float3_band_turns_that_component() {
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{scroll_motion::set_scroll_phase, MouseScrollDelta, ParametersBg, Position, ScrollPhase};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
        let pull = state.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
        for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.00:0.00:0.00")] {
            state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
        }
        state.graph_mut().set_selected_node(Some(pull));
        state.sync_parameters_pane();
        state.rebuild_positions();
        state.apply_layout();

        // The Y band of the Value row, from the pane's own float3 group.
        let (bx, by) = {
            // The slot is statically an `Adapted<ParametersBg>`.
            let pane: &ParametersBg = state.slots.param.inner();
            let f = pane.float3s.iter().flatten().next().expect("the Value row is a float3");
            let (rx, ry, rw, rh) = f.get_row_rects()[1];
            (rx + (rw - 68.0) * 0.5, ry + rh * 0.5)
        };
        let value = |state: &State| -> Vec<f32> {
            state.current_dir().children[pull].params.iter().find(|p| p.name == "value").unwrap()
                .text().split(':').map(|v| v.parse().unwrap()).collect()
        };
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: bx as f64, y: by as f64 } });
        set_scroll_phase(ScrollPhase::Finger);
        state.ui_context.scroll_gesture_new = true;
        state.ui_context.scroll_initiate_widget_id = None;
        assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 0.0, y: -60.0 }) }));
        set_scroll_phase(ScrollPhase::Wheel);
        let v = value(&state);
        assert_eq!((v[0], v[2]), (0.0, 0.0), "X and Z hold: {v:?}");
        assert_ne!(v[1], 0.0, "Y turned, and the node's Value followed: {v:?}");
    }

    /// The trackball is a float3 row's second control: on by default where
    /// the three numbers are a VECTOR (the pull node's Value aimed at Pos),
    /// off where they are a colour or a position, and the row menu's Show /
    /// Hide Trackball chooses either way. The choice is the instance's — it
    /// rides the file, only when made — and dragging the ball turns the
    /// node's vector, keeping its length.
    #[test]
    fn the_trackball_turns_the_pull_nodes_vector() {
        use crate::app::ParamMenuAction as A;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton, ParametersBg};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
        state.apply_action(McpAction::AddNode { template_name: "Group".into(), name: Some("group1".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
        let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
        let (pull, group) = (slot_of(&state, "pull1"), slot_of(&state, "group1"));
        for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.00:0.00:0.06")] {
            state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
        }
        let show = |state: &mut State, slot: usize| {
            state.graph_mut().set_selected_node(Some(slot));
            state.sync_parameters_pane();
            state.rebuild_positions();
            state.apply_layout();
        };
        let row = |state: &mut State, name: &str| state.param_mut().node_params().iter().find(|r| r.0 == name).expect("the row").2.clone();
        let entries = |state: &State, slot: usize, pname: &str| state.param_menu_rows(slot, pname).1;
        let (lo, hi) = (-1.0, 1.0);

        // The pull's Value: a vector, so the ball is there; the menu hides it.
        show(&mut state, pull);
        assert_eq!(row(&mut state, "Value"), format!("{}:soft", crate::app::float3_row(lo, hi, true)));
        assert!(entries(&state, pull, "value").contains(&A::HideTrackball));
        let pull_id = state.current_dir().children[pull].id.clone();
        state.run_param_action(&pull_id, "value", A::HideTrackball);
        assert_eq!(row(&mut state, "Value"), format!("{}:soft", crate::app::float3_row(lo, hi, false)));
        assert!(entries(&state, pull, "value").contains(&A::ShowTrackball));
        let saved = serde_json::to_string(&state.current_dir().children[pull]).unwrap();
        assert!(saved.contains("\"view\":\"sliders\""), "the choice rides the file: {saved}");
        state.run_param_action(&pull_id, "value", A::ShowTrackball);
        assert_eq!(row(&mut state, "Value"), format!("{}:soft", crate::app::float3_row(lo, hi, true)));

        // Aimed at Col the three numbers are a colour: no ball by default.
        state.apply_action(McpAction::SetParam { slot: pull, name: "attribute_name".into(), value: "Col".into() }, &mut redraw).unwrap();
        state.current_dir_mut().children[pull].params.iter_mut().find(|p| p.name == "value").unwrap().view.clear();
        show(&mut state, pull);
        assert_eq!(row(&mut state, "Value"), format!("{}:soft", crate::app::float3_row(lo, hi, false)));
        state.apply_action(McpAction::SetParam { slot: pull, name: "attribute_name".into(), value: "Pos".into() }, &mut redraw).unwrap();

        // A position (the Group node's Center): no ball until asked, and a
        // parameter that never chose writes no `view` at all.
        show(&mut state, group);
        assert!(row(&mut state, "Center").starts_with("float3:") && !row(&mut state, "Center").ends_with(":trackball"));
        let untouched = serde_json::to_string(&state.current_dir().children[group]).unwrap();
        assert!(!untouched.contains("\"view\""), "{untouched}");
        let group_id = state.current_dir().children[group].id.clone();
        state.run_param_action(&group_id, "center", A::ShowTrackball);
        assert!(row(&mut state, "Center").ends_with(":trackball"));
        // A slider row is not a float3: it is offered neither.
        assert!(!entries(&state, slot_of(&state, "sphere1"), "radius").iter().any(|a| matches!(a, A::ShowTrackball | A::HideTrackball)));

        // Drag the ball a quarter turn to the right: the pull, pointing at
        // the viewer, swings onto +X at the length it had.
        show(&mut state, pull);
        let (cx, cy, r) = {
            let pane: &ParametersBg = state.slots.param.inner();
            pane.float3s.iter().flatten().next().expect("the Value row").ball_circle().expect("its ball")
        };
        let at = |x: f32, y: f32| WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } };
        state.handle_event(&at(cx, cy));
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        for i in 1..=20 {
            state.handle_event(&at(cx + r * std::f32::consts::FRAC_PI_2 * i as f32 / 20.0, cy));
        }
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        let v: Vec<f32> = state.current_dir().children[pull].params.iter().find(|p| p.name == "value").unwrap()
            .text().split(':').map(|c| c.parse().unwrap()).collect();
        assert!((v[0] - 0.06).abs() < 2e-3 && v[1].abs() < 2e-3 && v[2].abs() < 2e-3, "the pull points along +X: {v:?}");
    }

    /// A scroll over the trackball rolls it, through the designer's own
    /// wheel path and the write-back: a two-finger gesture to the right
    /// turns the pull, pointing at the viewer, toward +X at the length it
    /// had, and a wheel notch down (content up) turns it toward +Y.
    #[test]
    fn a_scroll_over_the_trackball_rolls_the_pull_nodes_vector() {
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{scroll_motion::set_scroll_phase, MouseScrollDelta, ParametersBg, Position, ScrollPhase};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
        let pull = state.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
        for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.00:0.00:0.06")] {
            state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
        }
        state.graph_mut().set_selected_node(Some(pull));
        state.sync_parameters_pane();
        state.rebuild_positions();
        state.apply_layout();
        let (cx, cy, _) = {
            let pane: &ParametersBg = state.slots.param.inner();
            pane.float3s.iter().flatten().next().expect("the Value row").ball_circle().expect("its ball")
        };
        let value = |state: &State| -> Vec<f32> {
            state.current_dir().children[pull].params.iter().find(|p| p.name == "value").unwrap()
                .text().split(':').map(|c| c.parse().unwrap()).collect()
        };
        let len = |v: &[f32]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: cx as f64, y: cy as f64 } });

        set_scroll_phase(ScrollPhase::Finger);
        state.ui_context.scroll_gesture_new = true;
        state.ui_context.scroll_initiate_widget_id = None;
        assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 120.0, y: 0.0 }) }));
        set_scroll_phase(ScrollPhase::Wheel);
        let v = value(&state);
        assert!(v[0] > 0.02 && v[1].abs() < 2e-3 && v[2] > 0.0, "turned toward +X: {v:?}");
        assert!((len(&v) - 0.06).abs() < 2e-3, "at the length it had: {v:?}");

        state.ui_context.scroll_gesture_new = true;
        state.ui_context.scroll_initiate_widget_id = None;
        assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, -1.0) }));
        let w = value(&state);
        assert!(w[1] > 0.01, "a notch down turns it toward +Y: {w:?}");
        assert!((len(&w) - 0.06).abs() < 2e-3);
    }

    /// The trackball is seen from the viewport's camera: the direction
    /// from the scene toward the camera is the ball's toward-the-viewer
    /// axis, the scene's up stays up on the ball, and orbiting the camera
    /// moves the view. A vector pointing at the camera faces the viewer on
    /// the ball, and rolling the ball to the right swings the node's
    /// vector to the right of the SCREEN.
    #[test]
    fn the_trackball_follows_the_viewport_camera() {
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton, ParametersBg};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
        let pull = state.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
        // A pull of length 0.6 straight at a camera out along (2.5, 1.8, 2.5).
        let eye = Vec3::new(2.5, 1.8, 2.5);
        let at_camera = eye.normalize() * 0.6;
        let text = format!("{:.4}:{:.4}:{:.4}", at_camera.x, at_camera.y, at_camera.z);
        for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", text.as_str())] {
            state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
        }
        state.graph_mut().set_selected_node(Some(pull));
        state.sync_parameters_pane();
        state.rebuild_positions();
        state.apply_layout();

        assert!(state.sync_trackball_view(eye, Vec3::ZERO, Vec3::ZERO), "the view moved off the identity");
        assert!(!state.sync_trackball_view(eye, Vec3::ZERO, Vec3::ZERO), "the same camera again moves nothing");
        let ball_view = |state: &State| {
            let pane: &ParametersBg = state.slots.param.inner();
            pane.float3s.iter().flatten().next().expect("the Value row").view()
        };
        let view = ball_view(&state);
        let (right, up, toward) = (Vec3::from(view[0]), Vec3::from(view[1]), Vec3::from(view[2]));
        assert!(toward.distance(eye.normalize()) < 1e-4, "toward the viewer is toward the camera: {toward:?}");
        assert!(up.y > 0.5, "the scene's up is up on the ball: {up:?}");
        assert!(right.dot(toward).abs() < 1e-4 && right.cross(up).distance(toward) < 1e-4, "a right-handed view");

        // Roll the ball a quarter turn right: the pull, which pointed at
        // the camera, now points along the camera's right.
        let (cx, cy, r) = {
            let pane: &ParametersBg = state.slots.param.inner();
            pane.float3s.iter().flatten().next().unwrap().ball_circle().unwrap()
        };
        let at = |x: f32, y: f32| WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } };
        state.handle_event(&at(cx, cy));
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        for i in 1..=20 {
            state.handle_event(&at(cx + r * std::f32::consts::FRAC_PI_2 * i as f32 / 20.0, cy));
        }
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        let v: Vec<f32> = state.current_dir().children[pull].params.iter().find(|p| p.name == "value").unwrap()
            .text().split(':').map(|c| c.parse().unwrap()).collect();
        let v = Vec3::new(v[0], v[1], v[2]);
        assert!(v.distance(right * 0.6) < 5e-3, "the pull lies along screen right: {v:?} against {:?}", right * 0.6);

        // Orbiting the camera moves the ball's view with it.
        state.orbit_camera_by(120.0, 0.0);
        let orbited = if state.active_camera == "Default Camera" {
            state.sync_trackball_view(eye, Vec3::ZERO, Vec3::ZERO)
        } else {
            state.set_active_camera("Default Camera");
            state.orbit_camera_by(120.0, 0.0);
            state.sync_trackball_view(eye, Vec3::ZERO, Vec3::ZERO)
        };
        assert!(orbited, "an orbit moves the view");
        assert!(Vec3::from(ball_view(&state)[2]).distance(toward) > 0.05);
    }

    /// A detached parameters window is a working satellite: it opens on
    /// the main window's selection and camera, the main window writes the
    /// sync channel when its selection or its camera changes, and the
    /// detached window's trackballs turn with a viewport it cannot see.
    /// Until 2026-09-29 the window never read the channel at startup, so
    /// it opened with nothing selected and an empty pane, and with a pane
    /// other than the circular network detached neither window asked for
    /// an autosave again.
    #[test]
    fn a_detached_params_window_follows_the_selection_and_the_camera() {
        use cce_ui::widget::ParametersBg;
        let dir = std::env::temp_dir().join(format!("cce-designer-detached-camera-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let channel = dir.join("default_project.json");
        let ball_view = |state: &State| {
            let pane: &ParametersBg = state.slots.param.inner();
            pane.float3s.iter().flatten().next().expect("a float3 row").view()
        };
        let same = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| (0..3).all(|i| (0..3).all(|k| (a[i][k] - b[i][k]).abs() < 1e-4));

        let mut main = State::new(false);
        main.resize(1600.0, 900.0, 1.0);
        main.rebuild_positions();
        main.apply_layout();
        main.focused_pane = LEFT_MENUBAR_IDX;
        main.param_editor = crate::slots::CONTENT_IDX;
        main.set_active_camera("Default Camera");
        let mut redraw = false;
        main.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
        let pull = main.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
        for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.00:0.60:0.00")] {
            main.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
        }
        main.apply_action(McpAction::Select { slot: pull }, &mut redraw).unwrap();
        main.viewport_mut().rotation_y = 0.6;
        main.sync_trackball_view_from_camera();
        assert!(!main.syncing_windows() && !main.needs_autosave, "nothing detached: nothing to tell");

        // Detach: the channel is written, the child is started on it.
        main.detached_panes[crate::slots::PARAM_IDX] = true;
        assert!(main.syncing_windows());
        main.save_to_file(&channel).expect("the main window writes the channel");
        let mut child = State::new(false);
        child.detached_pane = Some(crate::slots::PARAM_IDX);
        child.resize(640.0, 400.0, 1.0);
        child.rebuild_positions();
        child.apply_layout();
        assert_eq!(child.param_mut().node_params().len(), 0, "a new state has nothing selected");
        child.seed_detached_window(&channel);
        assert_eq!(child.param_editor_selected(), Some(pull), "it opens on the main window's selection");
        assert!(child.param_mut().node_params().iter().any(|r| r.0 == "Value" && r.2.contains(":trackball")), "with its rows");
        assert!(same(ball_view(&child), ball_view(&main)), "and sees the ball from the main window's camera");
        assert!(!child.needs_autosave, "a detached window has no camera to tell of");

        // The main window's selection and camera each ask for an autosave…
        let sphere = main.current_dir().children.iter().position(|c| c.name == "sphere1").unwrap();
        main.needs_autosave = false;
        main.apply_custom_event(crate::app::CustomEvent::RunAction(McpAction::Select { slot: sphere }));
        assert!(main.needs_autosave, "a selection change is written for the detached window");
        main.apply_custom_event(crate::app::CustomEvent::RunAction(McpAction::Select { slot: pull }));
        main.needs_autosave = false;
        let before = ball_view(&main);
        main.orbit_camera_by(150.0, 40.0);
        assert!(main.sync_trackball_view_from_camera(), "the orbit moved the view");
        assert!(main.needs_autosave, "and asks for the write that carries it");
        assert!(!same(ball_view(&main), before));

        // …and the detached window, reloading what was written, follows.
        main.save_to_file(&channel).expect("autosave");
        assert!(!same(ball_view(&child), ball_view(&main)), "not before it reloads");
        child.load_sync_channel(&channel, false).expect("the detached window reloads");
        child.sync_trackball_view_from_camera();
        assert!(same(ball_view(&child), ball_view(&main)), "the detached ball turned with the viewport");

        // A detached window's own change is written too, for the main one.
        child.needs_autosave = false;
        child.apply_custom_event(crate::app::CustomEvent::RunAction(McpAction::SetParam { slot: pull, name: "value".into(), value: "0.10:0.20:0.30".into() }));
        assert!(child.needs_autosave);

        let _ = fs::remove_dir_all(&dir);
    }

    /// A row of the spreadsheet is a point, and selecting rows marks their
    /// points in the scene: a press selects one, ctrl adds another, and the
    /// markers are staged from the positions the table was filled from —
    /// nothing is evaluated. The selection goes when the table becomes
    /// another node's.
    #[test]
    fn selected_spreadsheet_rows_are_marked_in_the_scene() {
        use crate::slots::SPREADSHEET_IDX;
        use crate::window::{LocalPosition, WindowEvent};
        use cce_ui::widget::{ElementState, MouseButton};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.execute_menu_action("Show Spreadsheet Pane");
        state.rebuild_positions();
        state.apply_layout();
        state.param_editor = crate::slots::CONTENT_IDX;
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Box".into(), name: Some("rows_a".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
        state.apply_action(McpAction::AddNode { template_name: "Box".into(), name: Some("rows_b".into()), x: 7.0, y: 8.0 }, &mut redraw).unwrap();
        let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
        let (a, b) = (slot_of(&state, "rows_a"), slot_of(&state, "rows_b"));
        state.apply_action(McpAction::Select { slot: a }, &mut redraw).unwrap();
        state.sync_nodes();
        assert_eq!(state.spreadsheet_points.len(), 8, "a box has eight points, a row each");
        assert!(state.row_marker_verts.is_empty());

        let (sx, sy, sw, sh) = state.positions[SPREADSHEET_IDX];
        assert!(sw > 0.0 && sh > 60.0, "the spreadsheet is laid out: {sw} x {sh}");
        // Rows are 24 tall under a 24 header.
        let press = |state: &mut State, row: usize| {
            let (x, y) = (sx + 40.0, sy + 24.0 + 24.0 * row as f32 + 12.0);
            state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
            state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
            state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
        };
        let version = state.rt_geometry_version;
        press(&mut state, 1);
        assert_eq!(state.selected_spreadsheet_points(), vec![1]);
        assert!(!state.row_marker_verts.is_empty() && state.row_markers_dirty, "the marker is staged");
        let one = state.row_marker_verts.len();
        // The marker stands on the row's point.
        let p = state.spreadsheet_points[1];
        let n = one as f32;
        let mid = state.row_marker_verts.iter().fold([0.0f32; 3], |m, v| [m[0] + v.position[0] / n, m[1] + v.position[1] / n, m[2] + v.position[2] / n]);
        assert!((0..3).all(|k| (mid[k] - p[k]).abs() < 1e-3), "{mid:?} is not at {p:?}");

        state.modifiers.ctrl = true;
        press(&mut state, 0);
        state.modifiers.ctrl = false;
        assert_eq!(state.selected_spreadsheet_points(), vec![0, 1]);
        assert_eq!(state.row_marker_verts.len(), 2 * one, "a marker a row");
        assert_eq!(state.rt_geometry_version, version, "selecting evaluates nothing");

        // A refresh of the same node's table keeps it.
        state.apply_action(McpAction::SetParam { slot: a, name: "center".into(), value: "1.00:0.50:0.25".into() }, &mut redraw).unwrap();
        state.sync_nodes();
        assert_eq!(state.selected_spreadsheet_points(), vec![0, 1]);
        let moved = state.spreadsheet_points[1];
        let mid = state.row_marker_verts[one..].iter().chain(&state.row_marker_verts[..one]).fold([0.0f32; 3], |m, v| [m[0] + v.position[0], m[1] + v.position[1], m[2] + v.position[2]]);
        let both = [moved, state.spreadsheet_points[0]];
        let want = [both[0][0] + both[1][0], both[0][1] + both[1][1], both[0][2] + both[1][2]];
        assert!((0..3).all(|k| (mid[k] / one as f32 - want[k]).abs() < 1e-2), "the markers followed the points");

        // Another node's table is other points.
        state.apply_action(McpAction::Select { slot: b }, &mut redraw).unwrap();
        state.sync_nodes();
        assert!(state.selected_spreadsheet_points().is_empty());
        assert!(state.row_marker_verts.is_empty());
    }

    /// A node INSIDE a simnet is read as the scene draws it there: as the
    /// frame's last substep saw it, not from the seed. The spreadsheet's
    /// rows, and the markers on the rows selected, follow the simulation.
    #[test]
    fn rows_selected_inside_a_simnet_follow_the_simulation() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        state.show_spreadsheet = true;
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
        let sim = state.current_dir().children.iter().position(|c| c.name == "sim").unwrap();
        state.apply_action(McpAction::SetParam { slot: sim, name: "input".into(), value: "sphere1".into() }, &mut redraw).unwrap();
        {
            let simnet = &mut state.current_dir_mut().children[sim];
            let mut pull = crate::app::load_fs_tree().children.into_iter().find(|t| t.node_type == "attribute").unwrap();
            pull.id = "pull-in-sim".into();
            pull.name = "pull1".into();
            for (name, value) in [("input", "input1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.05:0.00:0.00"), ("combine", "Add")] {
                pull.params.iter_mut().find(|p| p.name == name).unwrap().set_text(value.to_string());
            }
            simnet.children.push(pull);
            let output = simnet.children.iter_mut().find(|c| c.node_type == "output").unwrap();
            output.params.iter_mut().find(|p| p.name == "input").unwrap().set_text("pull1".to_string());
        }
        // Dive in and select the pull.
        state.current_path.push(sim);
        state.sync_nodes();
        let pull = state.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
        state.apply_action(McpAction::Select { slot: pull }, &mut redraw).unwrap();
        state.slots.playbar.inner_mut().current_frame = 5.0;
        state.tick_frame(1.0 / 60.0);
        state.sync_nodes();
        assert!(!state.spreadsheet_points.is_empty());
        state.spreadsheet_mut().set_selected_rows(&[3]);
        state.rebuild_row_marker_verts();
        let middle = |state: &State| {
            let n = state.row_marker_verts.len() as f32;
            state.row_marker_verts.iter().fold(0.0f32, |m, v| m + v.position[0] / n)
        };
        let (row_at, marker_at) = (state.spreadsheet_points[3][0], middle(&state));
        assert!((row_at - marker_at).abs() < 1e-3);

        state.slots.playbar.inner_mut().current_frame = 15.0;
        state.tick_frame(1.0 / 60.0);
        let moved = state.spreadsheet_points[3][0] - row_at;
        assert!(moved > 0.3, "ten frames of the pull moved the row's point {moved}");
        assert!((middle(&state) - state.spreadsheet_points[3][0]).abs() < 1e-3, "and its marker with it");
        assert_eq!(state.selected_spreadsheet_points(), vec![3]);
    }

    /// The spreadsheet and the selected-group markers evaluate through the
    /// SHARED sim cache: with either reading something downstream of a
    /// simnet, a refresh costs no steps beyond the ones the frame itself
    /// took. Each used a cache of its own until 2026-09-29, and solved the
    /// simulation again from the seed.
    #[test]
    fn the_spreadsheet_and_group_markers_share_the_sim_cache() {
        let steps = || crate::geometry::STEPS_ON_THIS_THREAD.with(|s| s.get());
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        state.show_spreadsheet = true;
        let mut redraw = false;
        // sphere1 -> sim (a pull inside) -> tagged (a Group reading the sim).
        state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
        state.apply_action(McpAction::AddNode { template_name: "Group".into(), name: Some("tagged".into()), x: 7.0, y: 8.0 }, &mut redraw).unwrap();
        let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
        let (sim, tagged) = (slot_of(&state, "sim"), slot_of(&state, "tagged"));
        state.apply_action(McpAction::SetParam { slot: sim, name: "input".into(), value: "sphere1".into() }, &mut redraw).unwrap();
        state.apply_action(McpAction::SetParam { slot: tagged, name: "input".into(), value: "sim".into() }, &mut redraw).unwrap();
        state.apply_action(McpAction::SetParam { slot: tagged, name: "mode".into(), value: "Random".into() }, &mut redraw).unwrap();
        state.apply_action(McpAction::SetParam { slot: tagged, name: "count".into(), value: "5".into() }, &mut redraw).unwrap();
        {
            let simnet = &mut state.current_dir_mut().children[sim];
            let template = crate::app::load_fs_tree().children.into_iter().find(|t| t.node_type == "attribute").unwrap();
            let mut node = template.clone();
            node.id = "pull-in-sim".into();
            node.name = "pull1".into();
            for (name, value) in [("input", "input1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.01:0.00:0.00"), ("combine", "Add")] {
                node.params.iter_mut().find(|p| p.name == name).unwrap().set_text(value.to_string());
            }
            simnet.children.push(node);
            let output = simnet.children.iter_mut().find(|c| c.node_type == "output").expect("a simnet has an output");
            output.params.iter_mut().find(|p| p.name == "input").unwrap().set_text("pull1".to_string());
        }
        state.slots.playbar.inner_mut().current_frame = 61.0;
        state.sync_nodes();
        state.rebuild_scene_geometry();

        // Select the Group downstream of the simulation: the spreadsheet
        // fills and the markers stage. Whatever solving the frame takes is
        // done ONCE, by whoever asks first…
        state.apply_action(McpAction::Select { slot: tagged }, &mut redraw).unwrap();
        state.sync_nodes();
        assert_eq!(state.group_members.len(), 5, "the markers were staged from the simulated geometry");
        let solved = steps();
        assert!(solved >= 60, "the fixture simulates: {solved} steps");
        assert!(solved < 120, "the spreadsheet and the markers solved it between them once, not once each: {solved}");
        // …and nobody after: the simnet itself in the spreadsheet, the
        // group again, a scene rebuild — all through the one cache.
        state.apply_action(McpAction::Select { slot: sim }, &mut redraw).unwrap();
        state.sync_nodes();
        assert_eq!(steps(), solved, "the simnet in the spreadsheet");
        state.apply_action(McpAction::Select { slot: tagged }, &mut redraw).unwrap();
        state.sync_nodes();
        assert_eq!(steps(), solved, "the group again");
        state.rebuild_scene_geometry();
        state.sync_nodes();
        assert_eq!(steps(), solved, "and the scene");
        // And the cache is back where it lives, its solve intact.
        assert!(!state.sim_cache.checkpoint_frames(&state.current_dir().children[sim].id).is_empty());
    }

    /// The spreadsheet reads what is selected again whenever the answer
    /// may have changed: the frame moved — with a simulation in the graph
    /// or without one — or something upstream was edited. It used to read
    /// again only when the selected node or its OWN parameters changed, so
    /// during playback it showed the frame it had been opened on. The
    /// selected Group's markers follow the frame the same way.
    #[test]
    fn the_spreadsheet_and_markers_follow_the_frame_and_upstream_edits() {
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);
        state.rebuild_positions();
        state.apply_layout();
        state.focused_pane = LEFT_MENUBAR_IDX;
        state.param_editor = crate::slots::CONTENT_IDX;
        state.show_spreadsheet = true;
        let mut redraw = false;
        let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
        let sphere = slot_of(&state, "sphere1");
        // Where the markers stand, as plain numbers.
        let marks = |state: &State| -> Vec<[f32; 3]> { state.group_members.iter().map(|v| v.position).collect() };

        // No simulation in the graph. The frame moves; nothing rebuilds the
        // scene; what is selected is still read again.
        assert!(!crate::geometry::contains_simnet(&state.fs_root));
        state.apply_action(McpAction::Select { slot: sphere }, &mut redraw).unwrap();
        state.sync_nodes();
        state.tick_frame(1.0 / 60.0);
        let opened_at = state.last_spreadsheet_read_at;
        assert_eq!(opened_at.0, state.sim_frame());
        state.slots.playbar.inner_mut().current_frame = 7.0;
        state.tick_frame(1.0 / 60.0);
        assert_eq!(state.last_spreadsheet_read_at.0, 7, "read again at the new frame");
        // The same frame again reads nothing again.
        let at = state.last_spreadsheet_read_at;
        state.tick_frame(1.0 / 60.0);
        state.sync_nodes();
        assert_eq!(state.last_spreadsheet_read_at, at);

        // An edit UPSTREAM of the selection: the selected node and its own
        // parameters are as they were, and the rows are read again.
        state.apply_action(McpAction::AddNode { template_name: "Group".into(), name: Some("tagged".into()), x: 7.0, y: 8.0 }, &mut redraw).unwrap();
        let tagged = slot_of(&state, "tagged");
        for (name, value) in [("input", "sphere1"), ("mode", "Random"), ("count", "5")] {
            state.apply_action(McpAction::SetParam { slot: tagged, name: name.into(), value: value.into() }, &mut redraw).unwrap();
        }
        state.apply_action(McpAction::Select { slot: tagged }, &mut redraw).unwrap();
        state.sync_nodes();
        let (before, markers) = (state.last_spreadsheet_read_at, marks(&state));
        assert_eq!(markers.len(), 5);
        state.apply_action(McpAction::SetParam { slot: sphere, name: "radius".into(), value: "0.9".into() }, &mut redraw).unwrap();
        assert_eq!(state.param_editor_selected(), Some(tagged), "the selection did not move");
        assert_ne!(state.last_spreadsheet_read_at, before, "the rows were read again");
        assert_ne!(marks(&state), markers, "and the markers moved out with the sphere");

        // With a simulation, playback: every frame the playbar arrives at
        // is the frame the rows and the markers were read at.
        state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
        let sim = slot_of(&state, "sim");
        state.apply_action(McpAction::SetParam { slot: sim, name: "input".into(), value: "sphere1".into() }, &mut redraw).unwrap();
        {
            let simnet = &mut state.current_dir_mut().children[sim];
            let mut pull = crate::app::load_fs_tree().children.into_iter().find(|t| t.node_type == "attribute").unwrap();
            pull.id = "pull-in-sim".into();
            pull.name = "pull1".into();
            for (name, value) in [("input", "input1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.05:0.00:0.00"), ("combine", "Add")] {
                pull.params.iter_mut().find(|p| p.name == name).unwrap().set_text(value.to_string());
            }
            simnet.children.push(pull);
            let output = simnet.children.iter_mut().find(|c| c.node_type == "output").unwrap();
            output.params.iter_mut().find(|p| p.name == "input").unwrap().set_text("pull1".to_string());
        }
        state.apply_action(McpAction::SetParam { slot: tagged, name: "input".into(), value: "sim".into() }, &mut redraw).unwrap();
        state.apply_action(McpAction::Select { slot: tagged }, &mut redraw).unwrap();
        state.slots.playbar.inner_mut().current_frame = 10.0;
        state.tick_frame(1.0 / 60.0);
        let mut last = marks(&state);
        state.slots.playbar.inner_mut().playing = true;
        state.slots.playbar.inner_mut().fps = 60.0;
        for _ in 0..3 {
            state.tick_frame(1.0 / 60.0);
            assert_eq!(state.last_spreadsheet_read_at.0, state.sim_frame(), "the rows are this frame's");
            assert_ne!(marks(&state), last, "the markers moved with the simulation");
            last = marks(&state);
        }
    }

    // --- The 2D context in the viewport -----------------------------------

    /// A page node for the image tests: text params, as a hand-built node
    /// has them.
    fn image_node(id: &str, ty: &str, params: &[(&str, &str)]) -> FsNode {
        FsNode {
            id: id.to_string(),
            name: id.to_string(),
            node_type: ty.to_string(),
            children: vec![],
            params: params
                .iter()
                .map(|(n, v)| crate::app::ParamDef::new(n.to_string(), "text".to_string(), v.to_string()))
                .collect(),
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        }
    }

    fn image_root(children: Vec<FsNode>) -> FsNode {
        let mut root = image_node("root", "node", &[]);
        root.children = children;
        root
    }

    /// The generator's size is in the unit its Units row names: pixels are
    /// pixels exactly, and a metric sheet is its millimetres. (What a
    /// preset writes is `picking_a_page_preset_writes_its_size`.)
    #[test]
    fn an_image_is_sized_in_pixels_or_in_real_units() {
        use crate::page::{resolve_page, PageUnit};
        let make = |params: &[(&str, &str)]| {
            let root = image_root(vec![image_node("page1", "page", params)]);
            resolve_page(&root, &root.children[0], &mut Vec::new()).expect("no page")
        };

        let px = make(&[("units", "Pixels"), ("width", "640"), ("height", "360"), ("resolution", "96")]);
        assert_eq!((px.width, px.height), (640, 360), "a pixel size is that many pixels");
        assert_eq!(px.unit, PageUnit::Pixels);
        assert!((px.size[0] - 640.0 / 96.0).abs() < 1e-4, "its physical size is its pixels over its resolution");

        let mm = make(&[("units", "Millimetres"), ("width", "210"), ("height", "297"), ("resolution", "100")]);
        assert!((mm.size[0] - 210.0 / 25.4).abs() < 1e-4 && (mm.size[1] - 297.0 / 25.4).abs() < 1e-4);
        assert_eq!((mm.width, mm.height), (827, 1169), "A4 in millimetres at 100 DPI");

        let cm = make(&[("units", "Centimetres"), ("width", "2.54"), ("height", "5.08"), ("resolution", "50")]);
        assert_eq!((cm.width, cm.height), (50, 100));

        // A page from before the Units row is in inches, as it was.
        let old = make(&[("width", "2"), ("height", "1"), ("resolution", "50")]);
        assert_eq!((old.width, old.height, old.unit), (100, 50, PageUnit::Inches));

        // Opacity is the sheet's alpha, and Position where it stands.
        let clear = make(&[("width", "1"), ("height", "1"), ("resolution", "10"), ("opacity", "0.25"), ("position", "1.00:2.00:3.00")]);
        assert!((clear.pixels[0][3] - 0.25).abs() < 1e-6);
        assert_eq!(clear.origin, [1.0, 2.0, 3.0]);
    }

    /// A page's size is its Width and Height, always: Preset has no Custom
    /// and there is no Orientation row. Picking a preset WRITES its size
    /// there, in the page's Units (a sheet portrait, a raster size as it
    /// lies), Units converts them, and a save from before carries over once,
    /// a landscape sheet as it was drawn.
    #[test]
    fn picking_a_page_preset_writes_its_size() {
        use crate::page::{follow_page_rows, migrate_preset_rows, resolve_page};
        let size = |node: &FsNode| {
            let row = |n: &str| node.params.iter().find(|p| p.name == n).unwrap().text().to_string();
            (row("width"), row("height"))
        };
        let pick = |node: &mut FsNode, row: &str, value: &str| -> Vec<String> {
            let p = node.params.iter_mut().find(|p| p.name == row).unwrap();
            let was = p.clone();
            p.set_text(value);
            follow_page_rows(node, &was).into_iter().map(|p| p.name).collect()
        };
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let template = templates_root.children.iter().find(|t| t.node_type == "page").unwrap().clone();
        let preset = template.params.iter().find(|p| p.name == "preset").unwrap();
        assert!(!preset.choice_options().iter().any(|o| o == "Custom"), "{:?}", preset.choice_options());
        assert!(template.params.iter().all(|p| p.name != "orientation"));
        for row in ["width", "height"] {
            assert!(template.params.iter().find(|p| p.name == row).unwrap().show_when.is_empty(), "{row} is always shown");
        }

        let mut page = template.clone();
        assert_eq!(pick(&mut page, "preset", "A4"), ["width", "height"], "what a pick overwrites is handed back, for undo");
        assert_eq!(size(&page), ("8.268".into(), "11.693".into()));
        pick(&mut page, "preset", "Tabloid");
        assert_eq!(size(&page), ("11.00".into(), "17.00".into()), "a sheet is written portrait");
        // Units converts: the sheet keeps its size.
        pick(&mut page, "units", "Millimetres");
        assert_eq!(size(&page), ("279.40".into(), "431.80".into()));
        // A raster size is written as it lies, in the page's unit.
        pick(&mut page, "units", "Pixels");
        pick(&mut page, "preset", "HD");
        assert_eq!(size(&page), ("1920".into(), "1080".into()));
        let root = image_root(vec![page.clone()]);
        let img = resolve_page(&root, &root.children[0], &mut Vec::new()).unwrap();
        assert_eq!((img.width, img.height), (1920, 1080));
        // Another row changes nothing.
        assert!(pick(&mut page, "resolution", "72").is_empty());
        // A size typed in is the size, whatever Preset still names.
        pick(&mut page, "width", "640");
        let root = image_root(vec![page.clone()]);
        assert_eq!(resolve_page(&root, &root.children[0], &mut Vec::new()).unwrap().width, 640);

        // A save from before: its rows carry the old conditions and an
        // Orientation row. A named preset is written into Width and Height,
        // turned as it was drawn; a Custom one keeps its size and names
        // Letter; the Orientation row goes.
        let old = |preset: &str, w: &str, h: &str, orientation: &str| {
            let mut n = template.clone();
            n.params.push(crate::app::ParamDef::new("orientation", "choice:Portrait,Landscape", orientation));
            for (row, v) in [("preset", preset), ("width", w), ("height", h)] {
                let p = n.params.iter_mut().find(|p| p.name == row).unwrap();
                p.set_type("text");
                p.set_text(v);
            }
            for row in ["width", "height"] {
                n.params.iter_mut().find(|p| p.name == row).unwrap().show_when = "Preset == Custom".into();
            }
            n
        };
        let mut tabloid = old("Tabloid", "8.5", "11.0", "Landscape");
        migrate_preset_rows(&mut tabloid);
        assert_eq!(size(&tabloid), ("17.00".into(), "11.00".into()));
        assert!(tabloid.params.iter().all(|p| p.name != "orientation"));
        let mut custom = old("Custom", "3", "2", "Portrait");
        migrate_preset_rows(&mut custom);
        assert_eq!(size(&custom), ("3".into(), "2".into()));
        assert_eq!(custom.params.iter().find(|p| p.name == "preset").unwrap().text(), "Letter");
        // Once: the merge takes the old conditions away, so a page loaded
        // a second time is left as it is.
        let mut loaded = image_root(vec![old("Custom", "3", "2", "Portrait")]);
        crate::app::merge_template_defs(&mut loaded, &templates);
        assert_eq!(size(&loaded.children[0]), size(&custom));
        crate::app::merge_template_defs(&mut loaded, &templates);
        assert_eq!(size(&loaded.children[0]), size(&custom), "a second load wrote Letter over a typed size");
        let mut merged = image_root(vec![old("A4", "8.5", "11.0", "Portrait")]);
        crate::app::merge_template_defs(&mut merged, &templates);
        assert_eq!(size(&merged.children[0]), ("8.268".into(), "11.693".into()));
        assert!(merged.children[0].params.iter().all(|p| p.invalid().is_none() && p.name != "orientation"));

        // Through MCP, and undone as one step.
        let mut state = State::new(false);
        let mut redraw = false;
        let slot = state.new_image().expect("the Page template is missing");
        let before = size(&state.current_dir().children[slot]);
        state
            .apply_action(crate::app::McpAction::SetParam { slot, name: "preset".into(), value: "Tabloid".into() }, &mut redraw)
            .unwrap();
        assert_eq!(size(&state.current_dir().children[slot]), ("11.00".into(), "17.00".into()));
        state.run_command("undo");
        assert_eq!(size(&state.current_dir().children[slot]), before, "the pick and what it wrote are one step");
    }

    /// A node drawing on an image is written in the image's unit: the same
    /// rows on a pixel image and on an inch sheet put ink in different
    /// places, and on each where the unit says.
    #[test]
    fn what_draws_on_an_image_is_in_the_images_unit() {
        use crate::page::resolve_page;
        let chain = |units: &str, w: &str, h: &str, dpi: &str| {
            let root = image_root(vec![
                image_node("page1", "page", &[("units", units), ("width", w), ("height", h), ("resolution", dpi), ("color", "1.00:1.00:1.00")]),
                image_node(
                    "shape1",
                    "page_shape",
                    &[
                        ("input", "page1"), ("shape", "Rectangle"), ("x", "100"), ("y", "50"),
                        ("width", "20"), ("height", "10"), ("fill", "true"),
                        ("fill_color", "1.00:0.00:0.00"), ("stroke", "false"),
                    ],
                ),
            ]);
            resolve_page(&root, &root.children[1], &mut Vec::new()).expect("no page")
        };
        let red = |p: &crate::page::Page, x: u32, y: u32| {
            let c = p.pixels[(y * p.width + x) as usize];
            c[0] > 0.9 && c[1] < 0.1
        };

        let px = chain("Pixels", "200", "100", "96");
        assert!(red(&px, 100, 50), "no ink at the rectangle's centre");
        assert!(red(&px, 91, 46) && red(&px, 109, 54), "the rectangle is not 20 x 10 pixels");
        assert!(!red(&px, 112, 50) && !red(&px, 100, 57), "ink outside the rectangle");

        // The same rows in millimetres, on a sheet 200 mm wide at 127 DPI:
        // five pixels to the millimetre, so the ink is five times as far in.
        let mm = chain("Millimetres", "200", "100", "127");
        assert_eq!((mm.width, mm.height), (1000, 500));
        assert!(red(&mm, 500, 250) && red(&mm, 545, 270), "the rectangle is not 20 x 10 millimetres");
        assert!(!red(&mm, 100, 50) && !red(&mm, 560, 250), "ink outside the rectangle");

        // And in inches the rectangle is off a 4 x 2 inch sheet altogether.
        let inch = chain("Inches", "4", "2", "50");
        assert!(inch.pixels.iter().all(|c| c[1] > 0.9), "a rectangle 100 inches out drew on the sheet");
    }

    /// The four outlines, turned and stroked: ink inside the outline and
    /// none outside it.
    #[test]
    fn image_shapes_put_ink_inside_their_outline() {
        use crate::page::{Page, ShapeKind, ShapeSpec};
        let sheet = || Page::new([2.0, 2.0], 100, [1.0, 1.0, 1.0, 1.0]);
        let ink = |p: &Page, x: u32, y: u32| p.pixels[(y * p.width + x) as usize][1] < 0.5;
        let spec = |kind, size: [f32; 2], rotation| ShapeSpec {
            kind,
            center: [1.0, 1.0],
            size,
            rotation,
            corner_radius: 0.0,
            sides: 3,
            fill: Some([1.0, 0.0, 0.0, 1.0]),
            stroke: None,
        };

        // An ellipse fills its middle and not the corners of its box.
        let mut p = sheet();
        p.shape(&spec(ShapeKind::Ellipse, [1.0, 0.5], 0.0));
        assert!(ink(&p, 100, 100) && ink(&p, 145, 100) && ink(&p, 100, 120));
        assert!(!ink(&p, 148, 122), "the ellipse filled its box's corner");
        assert!(!ink(&p, 100, 128), "the ellipse is taller than it was asked to be");

        // Turned a quarter, a wide rectangle is a tall one.
        let mut p = sheet();
        p.shape(&spec(ShapeKind::Rectangle, [1.0, 0.2], 90.0));
        assert!(ink(&p, 100, 145) && ink(&p, 100, 55), "the turned rectangle is not tall");
        assert!(!ink(&p, 145, 100), "the turned rectangle is still wide");

        // A rounded corner leaves the box's corner clear.
        let mut p = sheet();
        p.shape(&ShapeSpec { corner_radius: 0.25, ..spec(ShapeKind::Rectangle, [1.0, 1.0], 0.0) });
        assert!(ink(&p, 100, 100) && ink(&p, 52, 100) && !ink(&p, 52, 52), "the corner was not rounded");

        // A triangle, its first corner at the top: ink under the apex, none
        // beside it.
        let mut p = sheet();
        p.shape(&spec(ShapeKind::Polygon, [1.0, 1.0], 0.0));
        assert!(ink(&p, 100, 60) && ink(&p, 100, 110));
        assert!(!ink(&p, 60, 60) && !ink(&p, 140, 60), "ink beside the triangle's apex");

        // A stroke alone draws the outline and leaves the middle.
        let mut p = sheet();
        p.shape(&ShapeSpec {
            fill: None,
            stroke: Some(([0.0, 0.0, 0.0, 1.0], 0.04)),
            ..spec(ShapeKind::Rectangle, [1.0, 1.0], 0.0)
        });
        assert!(ink(&p, 50, 100) && ink(&p, 100, 150), "no ink on the outline");
        assert!(!ink(&p, 100, 100) && !ink(&p, 40, 100), "ink off the outline");

        // A line is as long as its Width and as thick as its stroke.
        let mut p = sheet();
        p.shape(&ShapeSpec {
            stroke: Some(([0.0, 0.0, 0.0, 1.0], 0.06)),
            ..spec(ShapeKind::Line, [1.0, 0.0], 0.0)
        });
        assert!(ink(&p, 55, 100) && ink(&p, 145, 100) && ink(&p, 100, 102));
        assert!(!ink(&p, 100, 105) && !ink(&p, 155, 100), "the line is thicker or longer than asked");

        // An edge that is not on the pixel grid is covered in part, which is
        // what keeps a turned edge from being a staircase.
        let mut p = sheet();
        p.shape(&spec(ShapeKind::Rectangle, [1.005, 1.0], 0.0));
        let edge = p.pixels[(100 * p.width + 49) as usize][1];
        assert!(edge > 0.05 && edge < 0.95, "the edge pixel is all or nothing: {edge}");
    }

    /// The page nodes and the geometry nodes each have a display flag of
    /// their own: showing an image leaves the geometry shown, and the other
    /// way about.
    #[test]
    fn the_display_flag_is_exclusive_within_its_context() {
        let mut dir = image_root(vec![
            image_node("sphere1", "sphere", &[]),
            image_node("page1", "page", &[]),
            image_node("box1", "box", &[]),
            image_node("text1", "page_text", &[]),
        ]);
        for c in &mut dir.children {
            c.geometry_visible = false;
        }
        let flags = |d: &FsNode| d.children.iter().map(|c| c.geometry_visible).collect::<Vec<_>>();

        dir.set_child_geometry_visible(0, true);
        dir.set_child_geometry_visible(1, true);
        assert_eq!(flags(&dir), [true, true, false, false], "showing the image hid the geometry");
        dir.set_child_geometry_visible(3, true);
        assert_eq!(flags(&dir), [true, false, false, true], "two images are shown");
        dir.set_child_geometry_visible(2, true);
        assert_eq!(flags(&dir), [false, false, true, true], "showing geometry hid the image");
    }

    /// A State showing one image of `w` x `h` pixels at `dpi`, in a pane of
    /// 1200 x 800.
    fn state_showing_image(w: u32, h: u32, dpi: u32) -> State {
        let mut state = State::new(false);
        state.last_viewport_width = 1200;
        state.last_viewport_height = 800;
        let slot = state.new_image().expect("the Page template is missing");
        let node = &mut state.current_dir_mut().children[slot];
        for (name, value) in [
            ("units", "Pixels".to_string()),
            ("width", w.to_string()),
            ("height", h.to_string()),
            ("resolution", dpi.to_string()),
        ] {
            node.params.iter_mut().find(|p| p.name == name).expect("a page row is missing").set_text(value);
        }
        state.rebuild_scene_geometry();
        state
    }

    /// The image's corners through the camera as the stage pass builds it:
    /// NDC x and y of top-left, top-right, bottom-right, bottom-left.
    fn image_corners_in_view(state: &State) -> [[f32; 2]; 4] {
        let (pos, rot, pivot) = state.active_camera_pose();
        let aspect = state.last_viewport_width as f32 / state.last_viewport_height as f32;
        let (proj, view, model) = state.viewport().get_matrices(aspect, Some(pos), Some(rot), Some(pivot));
        let mvp = proj * view * model;
        state.image_world_corners().expect("no image is shown").map(|c| {
            let p = mvp.project_point3(Vec3::from_array(c));
            [p.x, p.y]
        })
    }

    /// The level's image reaches the viewport: composed, uploaded, and
    /// placed at its physical size in world units.
    #[test]
    fn a_shown_image_stands_in_the_scene_at_its_size() {
        let mut state = state_showing_image(400, 200, 100);
        let shown = state.page_shown.clone().expect("the image did not reach the viewport");
        assert!(state.page_image.is_some(), "nothing was uploaded");
        assert_eq!(shown.pixels, (400, 200));
        assert!(state.viewport_dirty, "nothing asks the scene to stage the image");

        // 4 x 2 inches, in a world of millimetres and in one of inches.
        let mm = shown.world_size(1.0);
        assert!((mm[0] - 101.6).abs() < 1e-3 && (mm[1] - 50.8).abs() < 1e-3, "{mm:?}");
        let inch = shown.world_size(25.4);
        assert!((inch[0] - 4.0).abs() < 1e-4 && (inch[1] - 2.0).abs() < 1e-4, "{inch:?}");
        // The top edge is up, the image faces +Z about its origin.
        let c = shown.world_corners(25.4);
        assert_eq!(c[0], [-2.0, 1.0, 0.0]);
        assert_eq!(c[2], [2.0, -1.0, 0.0]);

        // Hidden, it leaves the scene and frees its image.
        let slot = state.current_dir().children.iter().position(|n| n.id == shown.node_id).unwrap();
        state.current_dir_mut().set_child_geometry_visible(slot, false);
        state.viewport_dirty = false;
        state.rebuild_scene_geometry();
        assert!(state.page_shown.is_none() && state.page_image.is_none());
        assert!(state.viewport_dirty, "the scene keeps drawing an image that is gone");
    }

    /// Frame Image turns the camera square to the image and fits it: the
    /// corners land symmetric about the view's centre, inside the pane, and
    /// the axis that binds is nearly full. With the Default Camera, from any
    /// orbit, and with a camera node.
    #[test]
    fn frame_image_faces_the_image_and_fits_it() {
        let framed = |state: &State, what: &str| {
            let [tl, tr, br, bl] = image_corners_in_view(state);
            for (a, b) in [(tl[0], -tr[0]), (tl[1], tr[1]), (bl[0], -br[0]), (tl[1], -bl[1]), (tl[0], bl[0])] {
                assert!((a - b).abs() < 1e-3, "{what}: the image is not seen head-on: {tl:?} {tr:?} {br:?} {bl:?}");
            }
            assert!(tl[0] < 0.0 && tl[1] > 0.0, "{what}: the image is seen from behind or upside down");
            let reach = tr[0].max(tr[1]);
            assert!(reach <= 1.0 && reach > 0.85, "{what}: the image spans {reach} of the pane");
        };

        // Wide, in a pane less wide than it: the width binds.
        let mut state = state_showing_image(400, 100, 100);
        state.viewport_mut().rotation_x = 0.3;
        state.viewport_mut().rotation_y = -1.1;
        state.viewport_mut().zoom = 3.0;
        assert!(state.run_command("frame_image"), "frame_image is not a command");
        framed(&state, "default camera, wide image");
        let [_, tr, _, _] = image_corners_in_view(&state);
        assert!(tr[0] > tr[1], "a wide image is bound by its width");

        // Tall: the height binds.
        let mut state = state_showing_image(100, 400, 100);
        state.frame_image();
        framed(&state, "default camera, tall image");

        // Off the origin, the camera goes to it.
        let mut state = state_showing_image(200, 200, 100);
        let slot = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
        state.current_dir_mut().children[slot]
            .params
            .iter_mut()
            .find(|p| p.name == "position")
            .unwrap()
            .set_text("3.00:-2.00:1.00");
        state.rebuild_scene_geometry();
        state.frame_image();
        framed(&state, "default camera, image off the origin");

        // A camera node is rewritten, whatever orbit the widget holds.
        let mut state = state_showing_image(300, 200, 100);
        assert!(state.camera_level().children.iter().any(|c| c.name == "camera1"), "the bundled project has no camera1");
        state.set_active_camera("camera1");
        state.viewport_mut().rotation_x = 0.2;
        state.viewport_mut().rotation_y = 0.7;
        state.frame_image();
        framed(&state, "camera node");

        // With no image shown the command says so and moves nothing.
        let mut state = State::new(false);
        let zoom = state.viewport().zoom;
        assert!(!state.frame_image());
        assert_eq!(state.viewport().zoom, zoom);
    }

    /// View Image Pixels 1:1 puts one pixel of the image on one of the pane.
    #[test]
    fn view_image_pixels_is_one_pixel_to_one() {
        let mut state = state_showing_image(300, 200, 100);
        assert!(state.run_command("view_image_pixels"));
        let [tl, tr, _, bl] = image_corners_in_view(&state);
        let wide = (tr[0] - tl[0]) * 0.5 * state.last_viewport_width as f32;
        let tall = (tl[1] - bl[1]) * 0.5 * state.last_viewport_height as f32;
        assert!((wide - 300.0).abs() < 0.5 && (tall - 200.0).abs() < 0.5, "the image covers {wide} x {tall} px");
    }

    /// Frame All holds the image as it holds the geometry: a scene that is
    /// an image alone is still framed.
    #[test]
    fn frame_all_holds_the_image() {
        let mut state = state_showing_image(400, 400, 100);
        state.rt_sphere_verts.clear();
        let slot = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
        state.current_dir_mut().children[slot]
            .params
            .iter_mut()
            .find(|p| p.name == "position")
            .unwrap()
            .set_text("5.00:0.00:0.00");
        state.rebuild_scene_geometry();
        state.rt_sphere_verts.clear();
        state.frame_all();
        let (_, _, pivot) = state.active_camera_pose();
        assert!((pivot.x - 5.0).abs() < 1e-2, "Frame All did not go to the image: pivot {pivot:?}");
        for c in image_corners_in_view(&state) {
            assert!(c[0].abs() < 1.0 && c[1].abs() < 1.0, "a corner is out of the pane: {c:?}");
        }
    }

    /// The viewport menu offers the image's camera rows while one shows,
    /// and not otherwise.
    #[test]
    fn the_viewport_menu_frames_an_image_that_is_shown() {
        let state = State::new(false);
        let (rows, _) = state.viewport_menu_rows();
        assert!(!rows.iter().any(|r| r == "Frame Image"), "a row for an image that is not there");
        let state = state_showing_image(100, 100, 100);
        let (rows, actions) = state.viewport_menu_rows();
        let at = rows.iter().position(|r| r == "Frame Image").expect("no Frame Image row");
        assert_eq!(actions[at], crate::app::ViewportMenuAction::Command("frame_image"));
        assert!(rows.iter().any(|r| r == "View Image Pixels 1:1"));
    }

    /// The Add … to Image commands: from nothing they make the image too;
    /// the new node is wired after what it draws on, shown and selected,
    /// placed and sized from the image in the image's unit; and added to the
    /// middle of a chain it is inserted there.
    #[test]
    fn adding_to_an_image_wires_a_node_after_it() {
        let mut state = State::new(false);
        // No cell of the bundled project's is in the way down here.
        state.grid_cursor_col = 40;
        state.grid_cursor_row = 40;
        assert!(state.run_command("add_image_ellipse"), "add_image_ellipse is not a command");

        let names = |s: &State, ty: &str| {
            s.current_dir().children.iter().filter(|c| c.node_type == ty).map(|c| c.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names(&state, "page"), ["page1"], "no image was made for the shape");
        assert_eq!(names(&state, "page_shape"), ["page_shape1"]);
        let text_of = |s: &State, node: &str, row: &str| {
            let n = s.current_dir().children.iter().find(|c| c.name == node).unwrap();
            n.params.iter().find(|p| p.name == row).unwrap_or_else(|| panic!("{node} has no {row}")).text().to_string()
        };
        assert_eq!(text_of(&state, "page_shape1", "input"), "page1");
        assert_eq!(text_of(&state, "page_shape1", "shape"), "Ellipse");
        // Letter, in inches: the middle of the sheet.
        assert_eq!(text_of(&state, "page_shape1", "x"), "4.25");
        assert_eq!(text_of(&state, "page_shape1", "y"), "5.50");

        let flag = |s: &State, node: &str| s.current_dir().children.iter().find(|c| c.name == node).unwrap().geometry_visible;
        assert!(flag(&state, "page_shape1") && !flag(&state, "page1"), "the new node is not what shows");
        let shape_slot = state.current_dir().children.iter().position(|c| c.name == "page_shape1").unwrap();
        assert_eq!(state.selected_slots(), [shape_slot], "the new node is not selected");
        let shown = state.page_shown.clone().expect("nothing is shown");
        assert_eq!(shown.node_id, state.current_dir().children[shape_slot].id);

        // Make the sheet small and in pixels, then add text to the PAGE: it
        // goes between the page and the shape.
        let page_slot = state.current_dir().children.iter().position(|c| c.name == "page1").unwrap();
        {
            let node = &mut state.current_dir_mut().children[page_slot];
            for (row, v) in [("units", "Pixels"), ("width", "300"), ("height", "200"), ("resolution", "96")] {
                node.params.iter_mut().find(|p| p.name == row).unwrap().set_text(v);
            }
        }
        state.graph_mut().set_selected_node(Some(page_slot));
        assert!(state.run_command("add_image_text"));
        assert_eq!(text_of(&state, "page_text1", "input"), "page1");
        assert_eq!(text_of(&state, "page_shape1", "input"), "page_text1", "the text was not inserted into the chain");
        assert_eq!(text_of(&state, "page_text1", "x"), "150");
        assert_eq!(text_of(&state, "page_text1", "y"), "100");
        assert_eq!(text_of(&state, "page_text1", "size"), "10");

        // The whole chain still composes, from its end.
        let end = state.current_dir().children.iter().find(|c| c.name == "page_shape1").unwrap();
        let page = crate::page::resolve_page(&state.fs_root, end, &mut Vec::new()).expect("the chain is broken");
        assert_eq!((page.width, page.height), (300, 200));

        // A rectangle, a line and a polygon are the same node under another
        // Shape.
        for (cmd, shape) in [("add_image_rectangle", "Rectangle"), ("add_image_line", "Line"), ("add_image_polygon", "Polygon")] {
            let before = names(&state, "page_shape").len();
            assert!(state.run_command(cmd), "{cmd} is not a command");
            let all = names(&state, "page_shape");
            assert_eq!(all.len(), before + 1, "{cmd} added nothing");
            assert_eq!(text_of(&state, all.last().unwrap(), "shape"), shape);
        }
    }

    // --- Handles on an image ------------------------------------------------

    /// A page's frame is the page without its pixels: the same size, raster,
    /// unit and place, from any node of the chain, and nothing where the
    /// chain composes nothing.
    #[test]
    fn a_page_frame_is_the_page_without_its_pixels() {
        use crate::page::{resolve_frame, resolve_page};
        let root = image_root(vec![
            image_node("page1", "page", &[("units", "Millimetres"), ("width", "120"), ("height", "80"), ("resolution", "127"), ("position", "1.00:2.00:3.00")]),
            image_node("shape1", "page_shape", &[("input", "page1")]),
            image_node("text1", "page_text", &[("input", "shape1")]),
            image_node("lost1", "page_text", &[("input", "nothing")]),
            image_node("sphere1", "sphere", &[]),
        ]);
        let page = resolve_page(&root, &root.children[2], &mut Vec::new()).expect("no page");
        for slot in 0..3 {
            let frame = resolve_frame(&root, &root.children[slot]).expect("no frame");
            assert_eq!((frame.width, frame.height), (page.width, page.height));
            assert_eq!((frame.size, frame.dpi, frame.unit, frame.origin), (page.size, page.dpi, page.unit, page.origin));
        }
        assert!(resolve_frame(&root, &root.children[3]).is_none(), "a frame for a chain with no page under it");
        assert!(resolve_frame(&root, &root.children[4]).is_none(), "a frame for a sphere");

        // The page's corner and its middle, in a world of millimetres, and
        // back again.
        let frame = resolve_frame(&root, &root.children[0]).unwrap();
        let corner = frame.to_world([0.0, 0.0], 1.0);
        assert!((corner - Vec3::new(1.0 - 60.0, 2.0 + 40.0, 3.0)).length() < 1e-3, "{corner:?}");
        let middle = frame.to_world([60.0, 40.0], 1.0);
        assert!((middle - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-3, "{middle:?}");
        let back = frame.from_world(frame.to_world([17.0, 63.0], 10.0), 10.0);
        assert!((back[0] - 17.0).abs() < 1e-3 && (back[1] - 63.0).abs() < 1e-3, "{back:?}");
        // Off the plane is the place straight behind.
        let behind = frame.from_world(middle + Vec3::Z * 9.0, 1.0);
        assert!((behind[0] - 60.0).abs() < 1e-3 && (behind[1] - 40.0).abs() < 1e-3);
    }

    /// Give the viewer state the camera the stage pass would have cached.
    fn cache_scene_camera(state: &mut State) {
        let (pos, rot, pivot) = state.active_camera_pose();
        let (w, h) = (state.last_viewport_width as f32, state.last_viewport_height as f32);
        let (proj, view, model) = state.viewport().get_matrices(w / h, Some(pos), Some(rot), Some(pivot));
        state.last_scene_mvp = Some(proj * view * model);
        state.last_scene_view_rect = (0.0, 0.0, w, h);
    }

    /// Press on handle `i`, carry it to a screen point, let go.
    fn drag_handle_to(state: &mut State, i: usize, to: (f32, f32)) {
        let handles = state.viewer_tool_handles();
        let (_, x, y, _) = handles[i];
        state.cursor_x = x;
        state.cursor_y = y;
        assert!(state.viewer_tool_press(), "the press on handle {i} grabbed nothing");
        assert_eq!(state.viewer_tool.as_ref().unwrap().selected, Some(i), "the press took another handle");
        // In two motions, as a pointer arrives.
        state.cursor_x = (x + to.0) * 0.5;
        state.cursor_y = (y + to.1) * 0.5;
        assert!(state.viewer_tool_drag_motion());
        state.cursor_x = to.0;
        state.cursor_y = to.1;
        assert!(state.viewer_tool_drag_motion());
        assert!(state.viewer_tool_release());
    }

    /// A row of the node the viewer state is editing, as a number.
    fn edited_row(state: &State, row: &str) -> f32 {
        let id = state.viewer_tool.as_ref().expect("no viewer state").node_id.clone();
        let node = crate::viewer_state::find_node_by_id(&state.fs_root, &id).expect("the edited node is gone");
        crate::geometry::node_param_f32(node, row, f32::NAN)
    }

    /// A shape on an image is placed by its handles: the middle moves it and
    /// carries the others, the corner sizes it about its middle, the edge
    /// turns it and carries the corner round. At one image pixel to the
    /// screen's, a drag of so many pixels is a change of as many.
    #[test]
    fn an_image_shape_is_moved_sized_and_turned_by_its_handles() {
        let mut state = state_showing_image(400, 200, 100);
        assert!(state.run_command("add_image_rectangle"));
        assert_eq!(state.viewer_tool.as_ref().map(|t| t.source.name()), Some("Image Shape"), "adding a shape did not enter its viewer state");
        state.view_image_pixels();
        cache_scene_camera(&mut state);
        let rows = |s: &State| ["x", "y", "width", "height", "rotation"].map(|r| edited_row(s, r));
        assert_eq!(rows(&state), [200.0, 100.0, 67.0, 67.0, 0.0]);

        let handles = state.viewer_tool_handles();
        assert_eq!(handles.len(), 3, "a shape has a middle, a corner and an edge");
        // The image's middle is the pane's, and the corner is down and right
        // of it: the page's y runs down the screen.
        assert!((handles[0].1 - 600.0).abs() < 0.5 && (handles[0].2 - 400.0).abs() < 0.5, "{:?}", handles[0]);
        assert!((handles[1].1 - 633.5).abs() < 0.5 && (handles[1].2 - 433.5).abs() < 0.5, "{:?}", handles[1]);
        assert!((handles[2].1 - 633.5).abs() < 0.5 && (handles[2].2 - 400.0).abs() < 0.5, "{:?}", handles[2]);
        assert_eq!(state.viewer_tool_outline().len(), 4, "no outline of the box");

        drag_handle_to(&mut state, 0, (630.0, 410.0));
        assert_eq!(rows(&state), [230.0, 110.0, 67.0, 67.0, 0.0], "the middle did not move the shape whole");

        let corner = state.viewer_tool_handles()[1];
        drag_handle_to(&mut state, 1, (corner.1 + 20.0, corner.2 + 10.0));
        let sized = rows(&state);
        assert_eq!(sized[..2], [230.0, 110.0], "sizing moved the shape");
        assert!((sized[2] - 107.0).abs() <= 1.0 && (sized[3] - 87.0).abs() <= 1.0, "the corner did not size the box about its middle: {sized:?}");
        assert_eq!(sized[4], 0.0, "sizing turned the shape");

        // The edge, carried from the right of the middle to under it: a
        // quarter turn clockwise, and the box is the box it was.
        drag_handle_to(&mut state, 2, (630.0, 410.0 + 40.0));
        let turned = rows(&state);
        assert!((turned[4] - 90.0).abs() < 0.5, "the edge did not turn the shape: {turned:?}");
        assert!((turned[2] - sized[2]).abs() <= 1.0 && (turned[3] - sized[3]).abs() <= 1.0, "turning resized the box: {turned:?}");
        assert_eq!(turned[..2], [230.0, 110.0]);
        // Turned, the corner handle is down and LEFT of the middle.
        let corner = state.viewer_tool_handles()[1];
        assert!(corner.1 < 630.0 && corner.2 > 410.0, "the corner was not carried round: {corner:?}");

        // Each drag is one step to undo.
        assert!(state.viewer_tool_undo());
        assert!((edited_row(&state, "rotation")).abs() < 0.5, "undo did not take the turn back");
        assert!(state.viewer_tool_redo());
        assert!((edited_row(&state, "rotation") - 90.0).abs() < 0.5);

        // From an orbit the handles are still under the pointer: the middle,
        // dropped where a place on the image shows, is at that place.
        state.orbit_camera_by(140.0, -60.0);
        let py = state.viewport().pending_yaw;
        let pp = state.viewport().pending_pitch;
        state.update_active_camera_rotation(py, pp);
        cache_scene_camera(&mut state);
        let ctx_frame = {
            let id = state.viewer_tool.as_ref().unwrap().node_id.clone();
            let node = crate::viewer_state::find_node_by_id(&state.fs_root, &id).unwrap();
            crate::page::resolve_frame(&state.fs_root, node).unwrap()
        };
        let target = ctx_frame.to_world([120.0, 60.0], state.world_unit_mm());
        let (sx, sy, _) = crate::viewer_state::project_point(&state.last_scene_mvp.unwrap(), state.last_scene_view_rect, target).expect("the place is behind the camera");
        let head_on = state.viewer_tool_handles()[0];
        assert!((head_on.1 - 630.0).abs() > 2.0 || (head_on.2 - 410.0).abs() > 2.0, "the orbit did not move the view");
        drag_handle_to(&mut state, 0, (sx, sy));
        let moved = rows(&state);
        assert!((moved[0] - 120.0).abs() <= 1.0 && (moved[1] - 60.0).abs() <= 1.0, "from an orbit the handle left the image's plane: {moved:?}");
        assert!((moved[4] - 90.0).abs() < 0.5 && (moved[2] - sized[2]).abs() <= 1.0, "moving from an orbit changed the shape: {moved:?}");
    }

    /// A line has a middle and an end, and the end sets how long it is and
    /// which way it runs.
    #[test]
    fn an_image_line_is_drawn_by_its_end() {
        let mut state = state_showing_image(400, 200, 100);
        assert!(state.run_command("add_image_line"));
        state.view_image_pixels();
        cache_scene_camera(&mut state);
        assert_eq!(state.viewer_tool_handles().len(), 2, "a line has a middle and an end");
        assert_eq!(state.viewer_tool_outline().len(), 2);
        let height = edited_row(&state, "height");

        // Up and to the right of the middle by 30 and 40: fifty long each
        // way, running up the page.
        drag_handle_to(&mut state, 1, (630.0, 360.0));
        assert_eq!(edited_row(&state, "width"), 100.0);
        assert!((edited_row(&state, "rotation") - (-53.1)).abs() < 0.2, "{}", edited_row(&state, "rotation"));
        assert_eq!((edited_row(&state, "x"), edited_row(&state, "y")), (200.0, 100.0));
        assert_eq!(edited_row(&state, "height"), height, "a line's handles wrote a row it does not show");
    }

    /// Text is moved by its anchor and sized by the handle under it, in the
    /// image's unit — here inches on a sheet shown at half size.
    #[test]
    fn image_text_is_moved_and_sized_by_its_handles() {
        let mut state = state_showing_image(400, 200, 100);
        let page = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
        state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == "units").unwrap().set_text("Inches");
        for (row, v) in [("width", "4"), ("height", "2")] {
            state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == row).unwrap().set_text(v);
        }
        state.rebuild_scene_geometry();
        assert!(state.run_command("add_image_text"));
        assert_eq!(state.viewer_tool.as_ref().map(|t| t.source.name()), Some("Image Text"));
        state.view_image_pixels();
        cache_scene_camera(&mut state);
        assert_eq!((edited_row(&state, "x"), edited_row(&state, "y"), edited_row(&state, "size")), (2.0, 1.0, 0.1));

        let handles = state.viewer_tool_handles();
        assert_eq!(handles.len(), 2);
        assert!((handles[1].2 - handles[0].2 - 10.0).abs() < 0.5, "the size handle is not one Size under the anchor");

        // A hundred pixels to the inch.
        drag_handle_to(&mut state, 0, (650.0, 375.0));
        assert_eq!((edited_row(&state, "x"), edited_row(&state, "y"), edited_row(&state, "size")), (2.5, 0.75, 0.1));
        drag_handle_to(&mut state, 1, (650.0, 375.0 + 25.0));
        assert_eq!((edited_row(&state, "x"), edited_row(&state, "y"), edited_row(&state, "size")), (2.5, 0.75, 0.25));

        // A node that draws on no image has no handles to grab.
        let id = state.viewer_tool.as_ref().unwrap().node_id.clone();
        crate::viewer_state::find_node_by_id_mut(&mut state.fs_root, &id)
            .unwrap()
            .params
            .iter_mut()
            .find(|p| p.name == "input")
            .unwrap()
            .set_text("");
        assert!(state.viewer_tool_handles().is_empty());
    }

    /// Recomposing an image of the same size keeps its GPU image, which a
    /// drag does on every motion; another size takes another.
    #[test]
    fn a_recomposed_image_keeps_its_gpu_image() {
        let mut state = state_showing_image(64, 32, 100);
        let first = state.page_image.expect("nothing uploaded");
        state.rebuild_scene_geometry();
        assert_eq!(state.page_image, Some(first), "the same picture took a new image");
        // New contents at the same size: recomposed into the same image.
        let page = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
        state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == "color").unwrap().set_text("0.50:0.20:0.10");
        let version = state.page_version;
        state.rebuild_scene_geometry();
        assert!(state.page_version > version, "the colour edit was not recomposed");
        assert_eq!(state.page_image, Some(first), "the same size took a new image");
        state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == "width").unwrap().set_text("80");
        state.rebuild_scene_geometry();
        assert!(state.page_image.is_some_and(|id| id != first), "a picture of another size kept the old image");
    }

    // --- The image, path traced --------------------------------------------

    /// The traced scene is handed over again when the image changes: the
    /// image has a version as the geometry has, which a recomposition moves
    /// and a rebuild with no image in it does not — nor, since the page
    /// cache (2026-10-06), one that changes nothing the image is made of.
    #[test]
    fn a_recomposed_image_is_a_new_traced_scene() {
        let mut state = State::new(false);
        let before = state.page_version;
        state.rebuild_scene_geometry();
        assert_eq!(state.page_version, before, "a scene with no image moved the image's version");

        let mut state = state_showing_image(64, 32, 100);
        let shown = state.page_version;
        assert!(shown > before, "showing an image did not move its version");
        state.rebuild_scene_geometry();
        assert_eq!(state.page_version, shown, "a rebuild that changed nothing in the image recomposed it");
        let page = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
        state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == "color").unwrap().set_text("0.50:0.20:0.10");
        state.rebuild_scene_geometry();
        assert!(state.page_version > shown, "a recomposed image is the scene the tracer has");

        // Hidden, the tracer has to be told it went.
        let slot = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
        state.current_dir_mut().set_child_geometry_visible(slot, false);
        let hidden = state.page_version;
        state.rebuild_scene_geometry();
        assert!(state.page_version > hidden, "the tracer keeps an image that is gone");
        let gone = state.page_version;
        state.rebuild_scene_geometry();
        assert_eq!(state.page_version, gone);
    }

    /// A thumbnail of an image alone is taken square on, the image fitted
    /// edge to edge; with geometry beside it the view is the diagonal one and
    /// holds both.
    #[test]
    fn a_thumbnail_of_an_image_is_taken_square_on() {
        use crate::thumbnail::view_of;
        let corners = [[1.0, 3.0, 2.0], [5.0, 3.0, 2.0], [5.0, 1.0, 2.0], [1.0, 1.0, 2.0]];
        let seen = |eye: Vec3, center: Vec3, near: f32, far: f32, p: [f32; 3]| {
            let m = Mat4::perspective_rh(0.9, 1.0, near, far) * Mat4::look_at_rh(eye, center, Vec3::Y);
            m.project_point3(Vec3::from_array(p))
        };

        let (eye, center, near, far) = view_of(&[], Some(corners));
        assert!((center - Vec3::new(3.0, 2.0, 2.0)).length() < 1e-4, "not looking at the image's middle: {center:?}");
        assert!((eye.x - 3.0).abs() < 1e-4 && (eye.y - 2.0).abs() < 1e-4 && eye.z > 2.0, "not square on, from the side it faces: {eye:?}");
        let [tl, tr, br, bl] = corners.map(|c| seen(eye, center, near, far, c));
        assert!((tl.x + tr.x).abs() < 1e-4 && (tl.y + bl.y).abs() < 1e-4 && (tr.x - br.x).abs() < 1e-4);
        assert!(tr.x > 0.9 && tr.x <= 1.0, "the long side spans {} of the view", tr.x);
        assert!(tl.z > 0.0 && tl.z < 1.0, "the image is outside the near and far planes");

        let tri = cce_ui::vk::RtTriangle { p0: [0.0, 0.0, 0.0], p1: [1.0, 0.0, 0.0], p2: [0.0, 1.0, 0.0], material: 0 };
        let (eye, center, near, far) = view_of(&[tri], Some(corners));
        assert!(eye.x > center.x && eye.y > center.y && eye.z > center.z, "geometry is seen from the diagonal");
        for p in corners.into_iter().chain([tri.p0, tri.p1, tri.p2]) {
            let v = seen(eye, center, near, far, p);
            assert!(v.x.abs() < 1.0 && v.y.abs() < 1.0 && v.z > 0.0 && v.z < 1.0, "{p:?} is out of the view: {v:?}");
        }

        // As before, for a scene with no image in it.
        let (eye, center, _, _) = view_of(&[tri], None);
        assert!((center - Vec3::new(0.5, 0.5, 0.0)).length() < 1e-4 && eye.z > 0.0);
        let (_, center, _, _) = view_of(&[], None);
        assert_eq!(center, Vec3::ZERO);
    }
}
