
pub mod app;
pub mod application;
pub mod curve_tool;
pub mod soft_transform_tool;
pub mod viewer_state;
pub mod detail;
pub mod export;
pub mod export_cli;
pub mod remesh;
pub mod spatial;
pub mod volume;
pub mod wrangle;
pub mod shapes;
pub mod gpu;
pub mod springs;
pub mod collide;

// Root-level aliases some modules import via `crate::` paths.
#[allow(unused_imports)]
use app::{CustomEvent, McpAction, ModifiersState};
pub mod plate_corner;
pub mod playbar;
pub mod viewport_3d;
pub mod api;
pub mod window;
pub mod geometry;
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
pub mod expr;
pub mod thumbnail;

#[cfg(test)]
mod test_prelude {
    pub use std::fs;
    pub use std::path::Path;
    pub use glam::{Mat4, Vec3};
    pub use cce_ui::widget::{Key, NamedKey};
    pub use crate::app::{State, McpAction, ModifiersState};
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
    use crate::geometry::line_vertices;
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
                {"name":"Points","type":"spinbox","default":"250"}
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
        let points = node.params.iter().find(|p| p.name == "Points").unwrap();
        assert_eq!(points.default, "250", "instance owns its values");
        let shape = node.params.iter().find(|p| p.name == "Shape").expect("Shape appended");
        assert_eq!(shape.default, "None");
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

    /// The corner control has to land ON its plate: derived from the slot's live
    /// rect, an off-by-one in the inset would put the trigger outside the pane
    /// (unclickable, and painted over the neighbour) with nothing to catch it —
    /// the render pass draws wherever it is told.
    #[test]
    fn test_plate_corner_sits_inside_its_plate() {
        use crate::plate_corner::{CORNER_R, PLATE_SLOTS};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);

        let mut checked = 0;
        for idx in PLATE_SLOTS {
            let Some((cx, cy)) = state.plate_corner_center(idx) else { continue };
            let (x, y, w, h) = state.slots.get_dyn(idx).rect();
            checked += 1;

            // Inside the plate, with the whole disc clear of every edge.
            assert!(cx - CORNER_R >= x && cx + CORNER_R <= x + w,
                "slot {idx}: corner x {cx} escapes plate {x}..{}", x + w);
            assert!(cy - CORNER_R >= y && cy + CORNER_R <= y + h,
                "slot {idx}: corner y {cy} escapes plate {y}..{}", y + h);
            // ...and in the TOP-RIGHT quadrant of it, not merely somewhere inside.
            assert!(cx > x + w / 2.0, "slot {idx}: corner is not on the right");
            assert!(cy < y + h / 2.0, "slot {idx}: corner is not at the top");

            // The hit test must agree with where it is painted.
            assert_eq!(state.plate_corner_at(cx, cy), Some(idx), "slot {idx}: centre misses");
            assert_eq!(state.plate_corner_at(cx + CORNER_R * 2.0, cy), None,
                "slot {idx}: hit radius reaches past the control");
        }
        assert!(checked >= 2, "expected at least the network and params plates, checked {checked}");
    }

    /// Collapse must actually reclaim the plate AND take its body with it, and
    /// expanding must put both back — a stub that still hosts a full-height
    /// graph would paint the pane over the viewport it just freed.
    #[test]
    fn test_collapse_shrinks_the_plate_and_restores_it() {
        use crate::plate_corner::STUB_H;
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
        // The control that expands it again must still be there.
        assert!(state.plate_corner_center(NETWORK_PANEL_IDX).is_some(),
            "collapsed plate lost its corner control — nothing can expand it");

        state.set_pane_collapsed(NETWORK_PANEL_IDX, false);
        let (_, _, _, back_h) = state.slots.get_dyn(NETWORK_PANEL_IDX).rect();
        assert_eq!(back_h, full_h, "expanding did not restore the plate height");
        assert!(state.slots.get_dyn(CONTENT_IDX).visible(), "graph did not come back");
    }

    /// Both sides of a detach. The child must show ONE pane and nothing else —
    /// a stray visible slot would paint over it — and the parent must stop
    /// laying the pane out, or the space it held is never released.
    #[test]
    fn test_detached_pane_claims_its_window_and_leaves_the_parent() {
        use crate::plate_corner::DETACHED_MARGIN;
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
        // stub carries the corner control that is the only way to reattach.
        use crate::plate_corner::STUB_H;
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
        assert!(parent.plate_corner_center(PARAM_IDX).is_some(),
            "the stub has no corner control — nothing can reattach the pane");
        // Collapsed and detached stubs must not read the same.
        let label = parent.pane_stub_label(PARAM_IDX).expect("a detached pane is stubbed");
        assert!(label.contains("detached"), "stub does not say the pane is detached: {label}");
        assert!(parent.slots.get_dyn(VIEWPORT_IDX).visible(), "the rest of the parent survived");
    }

    /// Dock swap: dragging a plate's dot to another region swaps occupants,
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
        use crate::app::{ParamDef, Project};
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
        group.params = vec![ParamDef { name: "Input".into(), label: "Input".into(), param_type: "text".into(), default: "Sphere 1".into(), options: vec![], min: None, max: None, step: None, show_when: String::new(), expr: false }];
        let mut clash = group.clone();
        clash.id = "c".into();
        clash.name = "sphere1".into();
        clash.params[0].default = "Camera 1".into();
        proj.root.children.push(group);
        proj.root.children.push(clash);

        proj.sanitize_node_names();

        proj.migrate_param_refs();

        let names: Vec<&str> = proj.root.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"camera1"));
        assert!(names.contains(&"my_region"));
        assert!(names.contains(&"sphere1"), "the hand-named sibling keeps its name");
        assert!(names.contains(&"sphere1_2"), "the migrated sphere steps aside from it: {names:?}");
        let by_name = |n: &str| proj.root.children.iter().find(|c| c.name == n).unwrap();
        assert_eq!(by_name("my_region").params[0].default, "sphere1_2", "the wire followed the rename");
        assert_eq!(by_name("sphere1").params[0].default, "camera1");
        assert_eq!(proj.view_state.active_camera, "camera1");
        // The sphere is the native node the bundled file now holds.
        assert_eq!(by_name("sphere1_2").node_type, "sphere");

        // A clean file is left exactly alone.
        let before = serde_json::to_string(&proj).unwrap();
        proj.sanitize_node_names();
        proj.migrate_param_refs();
        assert_eq!(serde_json::to_string(&proj).unwrap(), before);
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

    /// The drop-region mapping: lower band is the bottom dock, the rest
    /// splits into halves.
    #[test]
    fn test_dock_region_mapping() {
        use crate::app::Dock;
        let mut state = State::new(false);
        state.resize(1000.0, 1000.0, 1.0);
        assert_eq!(state.dock_region_at(100.0, 100.0), Dock::Left);
        assert_eq!(state.dock_region_at(900.0, 100.0), Dock::Right);
        assert_eq!(state.dock_region_at(500.0, 900.0), Dock::Bottom);
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
        use crate::plate_corner::PlateMenuAction;
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
        use crate::plate_corner::{PlateMenuAction, PLATE_SLOTS};
        let mut state = State::new(false);
        state.resize(1600.0, 900.0, 1.0);

        let idx = PLATE_SLOTS.iter().copied()
            .find(|&i| state.plate_corner_center(i).is_some())
            .expect("some plate carries a corner control at this size");

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

    /// DE chrome is config-owned (`style.surface.relief.profile` /
    /// `.edge_profile` / `style.surface.param.color`), and a project file
    /// must not outrank the user's config.kdl. It did while the Main utility
    /// node carried a Style section; the retirement of that whole node tree
    /// is what closes it for good, so what is asserted now is that a save
    /// carrying those params brings nothing back.
    #[test]
    fn test_legacy_style_params_are_dropped_from_main() {
        let mut state = State::new(false);
        // A pre-removal save: the four utility subnets flat at the root,
        // Main carrying its retired Style section.
        let style = |name: &str, ty: &str, val: &str| crate::app::ParamDef {
            name: name.to_string(),
            label: String::new(),
            param_type: ty.to_string(),
            default: val.to_string(),
            options: Vec::new(),
            min: None,
            max: None,
            step: None,
            show_when: String::new(), expr: false,
        };
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
            NETWORK_PANEL2_IDX, CONTENT2_IDX, BREADCRUMB2_IDX, PAGE_IDX,
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
        let p = |name: &str, ty: &str, val: &str| crate::app::ParamDef {
            name: name.to_string(),
            label: String::new(),
            param_type: ty.to_string(),
            default: val.to_string(),
            options: Vec::new(),
            min: None,
            max: None,
            step: None,
            show_when: String::new(), expr: false,
        };
        let subnet = |name: &str, params: Vec<crate::app::ParamDef>| crate::app::FsNode {
            id: format!("legacy-{name}"),
            name: name.to_string(),
            node_type: "utility".to_string(),
            children: vec![],
            params,
            geometry_visible: true,
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
                    p("Point Color", "color", "#00ff00"),
                ]),
                subnet("main", vec![p("Circular Pane", "toggle", "true")]),
            ],
            params: vec![],
            geometry_visible: true,
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
        assert!(state.viewport().show_cube);
        assert!(state.wireframe);
        assert!((state.wire_width - 4.0).abs() < 1e-6);
        assert_eq!(state.point_color[1], 1.0);
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
                    position: (0.0, 4.0),
                    inputs: 1,
                    outputs: 1,
                },
                mine,
            ],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        });

        state.migrate_meta_settings_node();

        assert!(!state.fs_root.children.iter().any(|c| c.node_type == "meta"));
        let kept = state
            .fs_root
            .children
            .iter()
            .find(|c| c.id == "mine")
            .expect("the user's node was eaten with the meta subnet");
        assert_eq!(kept.name, "my_notes");
        // Re-homed onto a free cell — the root may already have something
        // standing where it was.
        assert!(
            state.fs_root.children.iter().filter(|c| c.position == kept.position).count() == 1,
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
            params: vec![crate::app::ParamDef {
                name: "Show Grid Guide".to_string(),
                label: String::new(),
                param_type: "toggle".to_string(),
                default: "false".to_string(),
                options: vec![],
                min: None,
                max: None,
                step: None,
                show_when: String::new(), expr: false,
            }],
            geometry_visible: true,
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

        let sphere = state
            .fs_root
            .children
            .iter()
            .position(|c| c.name == "sphere1")
            .expect("default project has sphere1");
        state.current_path2 = vec![sphere];
        state.sync_nodes();
        assert!(state.current_path.is_empty(), "primary path must not follow");
        assert_eq!(state.path_names_at(&state.current_path2), vec!["sphere1".to_string()]);

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

        let sphere = state.fs_root.children.iter().position(|c| c.name == "sphere1").unwrap();
        let camera = state.fs_root.children.iter().position(|c| c.name == "camera1").unwrap();

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
        let sphere_id = state.fs_root.children[sphere].id.clone();
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
            .fs_root
            .children
            .iter()
            .position(|c| c.name == "sphere1")
            .expect("default project has sphere1");
        a.current_path2 = vec![sphere];
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
        assert_eq!(b.current_path2, vec![sphere], "the second editor's path must round-trip");

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
        assert!(sphere1.params.iter().any(|p| p.name == "Method"), "the base template's params arrive");
        let radius = sphere1.params.iter().find(|p| p.name == "Radius").unwrap();
        assert!(radius.expr && radius.default.contains("Radius"), "the override is the reference: {} (expr {})", radius.default, radius.expr);
        assert_eq!(sphere1.params.iter().find(|p| p.name == "Center Y").unwrap().default, "0.0");

        let output1 = embryo.children.iter().find(|c| c.name == "output1").unwrap();
        assert_eq!(output1.node_type, "output");
        let output_input = output1.params.iter().find(|p| p.name == "Input").unwrap();
        assert_eq!(output_input.default, "normal1");
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
        let eval_template = |name: &str| -> crate::geometry::Geometry {
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
            crate::geometry::detail_to_soup(&g)
        };
        let tri_cross = |g: &crate::geometry::Geometry, tri: usize| -> [f32; 3] {
            let a = g.vertices[tri * 3].pos;
            let b = g.vertices[tri * 3 + 1].pos;
            let d = g.vertices[tri * 3 + 2].pos;
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
            let n = g.vertices.len() as f32;
            let mut c = [0.0f32; 3];
            for v in &g.vertices {
                for k in 0..3 {
                    c[k] += v.pos[k] / n;
                }
            }
            let (mut outward, mut total) = (0usize, 0usize);
            for tri in 0..g.vertices.len() / 3 {
                let nrm = tri_cross(&g, tri);
                let a = g.vertices[tri * 3].pos;
                let b = g.vertices[tri * 3 + 1].pos;
                let d = g.vertices[tri * 3 + 2].pos;
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
        for tri in 0..g.vertices.len() / 3 {
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
        let color = template.params.iter().find(|p| p.name == "Color").expect("a Color param");
        assert_eq!(color.param_type, "toggle");
        assert_eq!(color.default, "true", "coloured by default, as it always was");

        let generate = |on: &str| {
            let mut inst = template.clone();
            inst.id = format!("{template_name}_{on}");
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            inst.params.iter_mut().find(|p| p.name == "Color").unwrap().default = on.to_string();
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
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
        assert!(geom.points().has("Norm") && geom.points().has("UV") && geom.points().has("Cd"));
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
        instance.params.iter_mut().find(|p| p.name == "Points").unwrap().default =
            "0 0 0; 1 0 0".to_string();
        let root = make_root(instance.clone());
        assert_eq!(eval(&root).num_points(), 8 * 8);
        assert_eq!(
            crate::geometry::network_sphere_vertices(&root).num_points(),
            8 * 8,
            "scene walk and single-node eval disagree"
        );

        // No parseable points: empty geometry, not a panic.
        instance.params.iter_mut().find(|p| p.name == "Points").unwrap().default =
            "not points".to_string();
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
            "Points",
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
                "Points",
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
                "Points",
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
                "Points",
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

        let plane = ref_node("p", "plane1", "plane", vec![("Rows", "spinbox", "2"), ("Columns", "spinbox", "2"), ("Width", "slider", "1"), ("Length", "slider", "1")], vec![]);
        let extrude = |keep: &str| {
            ref_node("e", "extrude1", "extrude", vec![("Input", "text", "plane1"), ("Distance", "slider", "0.2"), ("Keep Base", "toggle", keep)], vec![])
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
        assert!(g.points().has("Cd") && g.points().has("UV"), "point attributes ride to the top");

        let root2 = ref_node("root", "root", "node", vec![], vec![plane, extrude("false")]);
        let g2 = eval(&root2, &root2.children[1]).0.unwrap();
        assert_eq!(g2.num_points(), 18, "the base's corners are the walls' corners: same places, fewer faces");
        assert_eq!(g2.num_prims(), 4 + 8);
        assert!(!g2.is_closed(), "no base, open bottom");

        // A closed input: no boundary, so no walls — an outer and an inner
        // skin, the farthest points Distance beyond the sphere.
        let sphere = ref_node("s", "sphere1", "sphere", vec![("Radius", "slider", "0.5"), ("Center X", "slider", "0"), ("Center Y", "slider", "0.55"), ("Center Z", "slider", "0")], vec![]);
        let ext = ref_node("e", "extrude1", "extrude", vec![("Input", "text", "sphere1"), ("Distance", "slider", "0.2"), ("Keep Base", "toggle", "true")], vec![]);
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
            inst.params.iter_mut().find(|p| p.name == name).unwrap().default = val.to_string();
        };
        set(&mut group_instance, "Input", "Sphere 1");
        set(&mut group_instance, "Center", "0.00:0.80:0.00");
        set(&mut group_instance, "Size", "2.00:0.50:2.00");

        let root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere_instance, group_instance],
            params: vec![],
            geometry_visible: true,
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
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().default =
                    val.to_string();
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
                ("Input", "Sphere 1"),
                ("Operation", "Create"),
                ("Attribute Name", "mass"),
                ("Type", "Float"),
                ("Value", "2.50"),
            ]),
            instance(attr_t, "a2", "Attr 2", &[
                ("Input", "Attr 1"),
                ("Operation", "Modify"),
                ("Attribute Name", "mass"),
                ("Combine", "Multiply"),
                ("Value", "2.00"),
            ]),
            instance(attr_t, "a3", "Attr 3", &[
                ("Input", "Attr 2"),
                ("Operation", "Delete"),
                ("Attribute Name", "mass"),
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
                ("Input", "Sphere 1"),
                ("Operation", "Modify"),
                ("Attribute Name", "Col"),
                ("Combine", "Multiply"),
                ("Value", "0.50"),
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
                ("Input", "Sphere 1"),
                ("Operation", "Modify"),
                ("Attribute Name", "Pos"),
                ("Combine", "Add"),
                ("Value", "0.00:0.10:0.00"),
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
                ("Input", "Sphere 1"),
                ("Center", "0.00:0.80:0.00"),
                ("Size", "2.00:0.50:2.00"),
            ]),
            instance(attr_t, "a1", "Attr 1", &[
                ("Input", "Group 1"),
                ("Operation", "Create"),
                ("Attribute Name", "mass"),
                ("Value", "1.00"),
                ("Group", "group1"),
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
                ("Input", "Sphere 1"),
                ("Operation", "Create"),
                ("Attribute Name", "mass"),
                ("Value", "abc"),
            ]),
        ]);
        let (geom, err) = eval(&root, 1);
        let geom = geom.expect("bad Value still passes geometry through");
        assert!(err.is_some(), "bad Value must surface an error");
        assert_eq!(geom.num_points(), base.num_points());
        assert!(!geom.points().has("mass"));
    }

    /// The param pane's attribute/group pickers: selecting an Attribute node
    /// upgrades its name/group text rows to textpick rows whose candidates
    /// are read off the INPUT geometry (groups from group: tags, attributes
    /// plus the Pos/Col built-ins); a Group node with a group-less input
    /// keeps a plain text row (no empty menu).
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
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().default =
                    val.to_string();
            }
            inst
        };

        let mut state = State::new(false);
        state.fs_root.children = vec![
            instance(find("Sphere"), "s", "Sphere 1", &[]),
            instance(find("Group"), "g", "Group 1", &[
                ("Input", "Sphere 1"),
                ("Center", "0.00:0.80:0.00"),
                ("Size", "2.00:0.50:2.00"),
            ]),
            instance(find("Attribute"), "a", "Attr 1", &[("Input", "Group 1")]),
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
        let attr_ty = row("Attribute Name");
        assert!(attr_ty.starts_with("textpick:"), "got {attr_ty}");
        for expected in ["Norm", "UV", "Pos", "Col"] {
            assert!(attr_ty.contains(expected), "{expected} missing from {attr_ty}");
        }
        assert_eq!(row("Group"), "textpick:group1");
        // The Input row stays plain text.
        assert_eq!(row("Input"), "text");

        // The Group node's own Group Name: its input (the sphere) carries no
        // groups, so the row degrades to plain text.
        state.graph_mut().set_selected_node(Some(1));
        state.sync_parameters_pane();
        let rows = state.param_mut().node_params();
        let gn = rows.iter().find(|r| r.0 == "Group Name").unwrap();
        assert_eq!(gn.2, "text");
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
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().default =
                    val.to_string();
            }
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
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
        assert_eq!(build(&[("Rows", "2"), ("Columns", "3")]).num_points(), 4 * 3);

        // Center: lifts to y = 0.3 and shifts x by 1 (span [0.5, 1.5]).
        let moved = build(&[("Center X", "1.0"), ("Center Y", "0.3")]);
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
            params: vec![crate::app::ParamDef {
                name: "Code".into(), label: String::new(), param_type: "code".into(), default: "OLD KERNEL".into(),
                options: vec![], min: None, max: None, step: None, show_when: String::new(), expr: false,
            }],
            geometry_visible: true,
            position: (4.0, 2.0),
            inputs: 1,
            outputs: 1,
        };
        let mut output1 = output_t.clone();
        output1.id = "s_output1".to_string();
        output1.name = "output1".to_string();
        output1.params.iter_mut().find(|p| p.name == "Input").unwrap().default = "opencl1".to_string();
        let old_sphere = FsNode {
            id: "s".to_string(),
            name: "Sphere 3".to_string(),
            node_type: "node".to_string(),
            children: vec![opencl1, output1],
            params: vec![crate::app::ParamDef {
                name: "Radius".into(), label: String::new(), param_type: "slider".into(), default: "0.70".into(),
                options: vec![], min: None, max: None, step: None, show_when: String::new(), expr: false,
            }],
            geometry_visible: true,
            position: (3.0, 1.0),
            inputs: 0,
            outputs: 1,
        };

        // An old Group missing a later-added param, with a kept value.
        let mut old_group = group_t.clone();
        old_group.id = "g".to_string();
        old_group.name = "My Region".to_string(); // renamed: native nodes match by TYPE
        old_group.params.retain(|p| p.name != "Highlight");
        old_group.params.iter_mut().find(|p| p.name == "Center").unwrap().default =
            "0.00:0.80:0.00".to_string();

        // A hand-built subnet that happens to share the Sphere name.
        let lookalike = FsNode {
            id: "fake".to_string(),
            name: "Sphere 9".to_string(),
            node_type: "node".to_string(),
            children: vec![],
            params: vec![],
            geometry_visible: true,
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
        assert_eq!(names, ["Method", "Radius", "Rows", "Columns", "Frequency", "Resolution", "Center X", "Center Y", "Center Z", "Color"]);
        assert_eq!(s.params.iter().find(|p| p.name == "Radius").unwrap().default, "0.70", "instance value survives");

        // And the merged instance evaluates with the new controls live.
        let mut merged_sphere_root = root.clone();
        merged_sphere_root.children.truncate(1);
        merged_sphere_root.children[0].params.iter_mut()
            .find(|p| p.name == "Rows").unwrap().default = "4".to_string();
        merged_sphere_root.children[0].params.iter_mut()
            .find(|p| p.name == "Columns").unwrap().default = "6".to_string();
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
        assert!(g.params.iter().any(|p| p.name == "Highlight" && p.default == "true"));
        assert_eq!(g.params.iter().find(|p| p.name == "Center").unwrap().default, "0.00:0.80:0.00");

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
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().default =
                    val.to_string();
            }
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
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
        let coarse = build(&[("Rows", "4"), ("Columns", "6")]);
        assert_eq!(coarse.num_points(), crate::geometry::sphere_point_len(4, 6));

        // Center X shifts the whole sphere: default spans x in [-0.5, 0.5],
        // shifted spans [0.5, 1.5].
        let shifted = build(&[("Center X", "1.0")]);
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        for pos in shifted.positions() {
            min_x = min_x.min(pos[0]);
            max_x = max_x.max(pos[0]);
        }
        assert!((min_x - 0.5).abs() < 0.01, "min x {min_x}");
        assert!((max_x - 1.5).abs() < 0.01, "max x {max_x}");

        // Degenerate resolutions clamp instead of emitting nothing.
        assert_eq!(
            build(&[("Rows", "0"), ("Columns", "0")]).num_points(),
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
        let method = sphere_t.params.iter().find(|p| p.name == "Method").expect("a Method dropdown");
        assert_eq!(method.param_type, "choice:UV,Icosphere,Cube");
        assert_eq!(method.default, "UV", "the default stays the sphere every saved project was built with");
        assert_eq!(sphere_t.params[0].name, "Method", "the method heads the pane, above the radius it governs");
        // And it heads the pane of a sphere SAVED before it existed too: the
        // bundled project's sphere1 gains it through the loader's merge, at
        // the template's position rather than below Color.
        let state = State::new(false);
        let saved = state.current_dir().children.iter().find(|c| c.name == "sphere1").expect("the bundled sphere1");
        assert_eq!(saved.params[0].name, "Method", "merged order: {:?}", saved.params.iter().map(|p| &p.name).collect::<Vec<_>>());
        let build = |params: &[(&str, &str)]| {
            let mut inst = sphere_t.clone();
            inst.id = "s".to_string();
            inst.name = "sphere1".to_string();
            for child in &mut inst.children {
                child.id = format!("{}_{}", inst.id, child.name);
            }
            for (pname, val) in params {
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().default = val.to_string();
            }
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
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

        let uv = build(&[("Method", "UV")]);
        assert_eq!(uv.num_points(), crate::geometry::sphere_point_len(16, 24));

        for (freq, expect) in [("1", 12), ("2", 42), ("4", 162), ("7", 492)] {
            let ico = build(&[("Method", "Icosphere"), ("Frequency", freq)]);
            assert_eq!(ico.num_points(), expect, "icosphere at frequency {freq}");
            assert_eq!(ico.num_prims(), 20 * freq.parse::<usize>().unwrap().pow(2));
            assert!(ico.is_closed(), "icosphere at frequency {freq} is not closed");
            on_sphere(&ico, 0.5);
        }

        for (res, expect) in [("1", 8), ("3", 56), ("8", 386)] {
            let cube = build(&[("Method", "Cube"), ("Resolution", res), ("Radius", "0.8")]);
            assert_eq!(cube.num_points(), expect, "cube sphere at resolution {res}");
            assert_eq!(cube.num_prims(), 6 * res.parse::<usize>().unwrap().pow(2), "one quad per cell — the kernel fanned them");
            assert!(cube.is_closed(), "cube sphere at resolution {res} is not closed");
            on_sphere(&cube, 0.8);
        }

        // The out-of-range guards: a frequency of 0 builds the icosahedron.
        assert_eq!(build(&[("Method", "Icosphere"), ("Frequency", "0")]).num_points(), 12);
    }

    /// `param_number` is what a kernel's `chi()` reads: a choice is its
    /// option index, a toggle 0 or 1, a number itself, and text 0.
    #[test]
    fn a_choice_reads_as_its_option_index_from_a_kernel() {
        use crate::geometry::param_number;
        let p = |ty: &str, val: &str| crate::app::ParamDef {
            name: "X".into(), label: String::new(), param_type: ty.into(), default: val.into(),
            options: vec![], min: None, max: None, step: None, show_when: String::new(), expr: false,
        };
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
                inst.params.iter_mut().find(|p| p.name == *pname).unwrap().default =
                    val.to_string();
            }
            inst
        };
        let root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![
                instance(find("Sphere"), "s", "Sphere 1", &[]),
                instance(find("Scatter"), "sc", "Scatter 1", &[("Input", "Sphere 1")]),
                instance(find("Attribute"), "a", "Attr 1", &[
                    ("Input", "Scatter 1"),
                    ("Operation", "Create"),
                    ("Attribute Name", "mass"),
                    ("Value", "1.00"),
                ]),
            ],
            params: vec![],
            geometry_visible: true,
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
                inst.params.iter_mut().find(|p| p.name == *name).unwrap().default = value.to_string();
            }
            let root = FsNode {
                id: "root".to_string(),
                name: "root".to_string(),
                node_type: "node".to_string(),
                children: vec![inst],
                params: vec![],
                geometry_visible: true,
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
        let geom_2 = generate(&[("Width", "2.0"), ("Length", "3.0")], "plane_inst_2");
        let max_x_2 = geom_2.positions().iter().map(|p| p[0].abs()).fold(0.0f32, f32::max);
        let max_z_2 = geom_2.positions().iter().map(|p| p[2].abs()).fold(0.0f32, f32::max);
        assert!((max_x_2 - 1.0).abs() < 0.01, "Expected half-width 1.0 on X, got {}", max_x_2);
        assert!((max_z_2 - 1.5).abs() < 0.01, "Expected half-length 1.5 on Z, got {}", max_z_2);

        // Columns/Rows control the cell counts per axis.
        let geom_3 = generate(&[("Columns", "4"), ("Rows", "8")], "plane_inst_3");
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
                    position: (5.0, 6.0),
                    inputs: 0,
                    outputs: 1,
                }
            ],
            params: vec![],
            geometry_visible: true,
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
                "Point", "Pos.x", "Pos.y", "Pos.z", "Col.r", "Col.g", "Col.b",
                "ID", "UV.x", "UV.y", "g:pinned", "d:mass_max~",
            ]
        );

        assert_eq!(rows.len(), 2, "one row per point");
        assert_eq!(rows[0][0], "0");
        assert_eq!(rows[0][1], "1.0000"); // Pos.x
        assert_eq!(rows[0][4], "1.0000"); // Col.r
        assert_eq!(rows[0][7], "42"); // ID, an integer and printed as one
        assert_eq!(rows[0][8], "0.1000"); // UV.x
        assert_eq!(rows[0][10], "", "point 0 is not in the group");

        assert_eq!(rows[1][0], "1");
        assert_eq!(rows[1][1], "4.0000");
        assert_eq!(rows[1][7], "0", "unwritten is the type's zero, not a dash");
        assert_eq!(rows[1][9], "0.4000"); // UV.y
        assert_eq!(rows[1][10], "1", "point 1 is in the group");
        assert_eq!(rows[0][11], "9.5000");
        assert_eq!(rows[1][11], "9.5000", "a detail value repeats down the column");
        // The trailing ~ says this one resets at every step boundary.
        assert!(headers[11].ends_with('~'), "{}", headers[11]);
    }

    #[test]
    fn test_line_geometry_generation() {
        let start = Vec3::new(0.0, 0.0, 0.0);
        let end = Vec3::new(0.0, 1.0, 0.0);
        let geom = line_vertices(start, end, 0.02);
        
        // A box line is 36 soup vertices: 6 faces * 2 triangles * 3 corners.
        assert_eq!(geom.vertices.len(), 36);

        // Every corner still carries Norm and UV through the soup adapter.
        for v in &geom.vertices {
            assert!(v.attributes.contains_key("Norm"));
            assert!(v.attributes.contains_key("UV"));
        }
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
        state.fs_root.children.push(scatter);
        let idx = state.fs_root.children.len() - 1;
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
        let has_bg_between = list.items[label_idx..option_idx]
            .iter()
            .any(|item| matches!(item.prim, cce_ui::scene::paint::Prim::Quad { .. }));
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
                &[("Rows", "4"), ("Columns", "6"), ("Width", "2.00"), ("Length", "3.00")],
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
            let mut ps = vec![("Radius", "1.00")];
            ps.extend_from_slice(params);
            let root = modelling_root("1.0", vec![phase3_node("polygon", &ps)]);
            eval_node(&root, "polygon 1").0
        };

        // Three sides is a triangle, four a square, thirty-two a circle: the
        // same shape with one number changed.
        let tri = build(&[("Sides", "3")]);
        assert_eq!(tri.num_prims(), 3, "a filled triangle is three fan triangles");
        assert_eq!(tri.num_points(), 4, "three corners and a hub");
        let circle = build(&[("Sides", "32")]);
        assert_eq!(circle.num_points(), 33);

        // A circle's corners all sit at the radius; a square's do too.
        for d in [&circle, &build(&[("Sides", "4")])] {
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
        let star = build(&[("Sides", "5"), ("Inner Radius", "0.40")]);
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
        let ring = build(&[("Sides", "6"), ("Fill", "false")]);
        assert_eq!(ring.num_points(), 6, "no hub when there is no fill");
        assert_eq!(ring.num_prims(), 6);
        assert_eq!(ring.edges().len(), 6, "the outline closes");
        for p in 0..ring.num_points() {
            assert_eq!(ring.point_neighbours(p).len(), 2, "point {p} is not on a loop");
        }
    }

    // ---- The parameter pane's conditional rows ----

    fn pd(name: &str, value: &str, show_when: &str) -> crate::app::ParamDef {
        crate::app::ParamDef {
            name: name.into(),
            label: String::new(),
            param_type: "text".into(),
            default: value.into(),
            options: vec![],
            min: None,
            max: None,
            step: None,
            show_when: show_when.into(),
            expr: false,
        }
    }

    #[test]
    fn test_a_row_shows_only_when_its_condition_holds() {
        use crate::app::{param_display, param_visible};
        let params = vec![
            pd("Mode", "Twist", ""),
            pd("Angle", "1.0", "Mode == Twist"),
            pd("Bend Axis", "Y", "Mode == Bend"),
            pd("Shared", "x", "Mode == Twist|Bend"),
            pd("Not Bleed", "x", "Mode != Bleed"),
        ];
        let shown: Vec<String> = param_display(&params).into_iter().map(|r| r.0).collect();
        assert_eq!(shown, vec!["Mode", "Angle", "Shared", "Not Bleed"]);

        // Flip the driving parameter and a different set applies. This is the
        // whole point: collapsing fifty operators into ten traded node count
        // for parameter count, and a pane showing twelve irrelevant rows is
        // worse than the twelve nodes it replaced.
        let mut bent = params.clone();
        bent[0].default = "Bend".into();
        let shown: Vec<String> = param_display(&bent).into_iter().map(|r| r.0).collect();
        assert_eq!(shown, vec!["Mode", "Bend Axis", "Shared", "Not Bleed"]);

        // Bleed matches none of the conditions, so only the driving row is
        // left — which is a node with one relevant control showing one.
        let mut bleeding = params.clone();
        bleeding[0].default = "Bleed".into();
        let shown: Vec<String> = param_display(&bleeding).into_iter().map(|r| r.0).collect();
        assert_eq!(shown, vec!["Mode"]);

        // The condition is evaluated against siblings' CURRENT values, which
        // is where this app keeps them.
        assert!(param_visible(&params, "Mode == Twist"));
        assert!(!param_visible(&bleeding, "Mode == Twist"));
    }

    #[test]
    fn test_conditions_and_together_and_compare_without_case() {
        use crate::app::param_visible;
        let params = vec![
            pd("Mode", "Align", ""),
            pd("Target", "Constant", ""),
        ];
        assert!(param_visible(&params, "Mode == Align && Target == Constant"));
        assert!(!param_visible(&params, "Mode == Align && Target == Attribute"));
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
        let params = vec![pd("Mode", "Twist", "")];
        // A misspelled sibling, and a clause that is not a comparison at all.
        // Both are template bugs; showing the row unconditionally would let
        // them pass unnoticed, and the row going missing is a complaint you
        // can act on.
        assert!(!param_visible(&params, "Moed == Twist"));
        assert!(!param_visible(&params, "Mode"));
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
    fn test_hiding_a_row_does_not_lose_its_value() {
        use crate::app::param_display;
        // Write-back resolves a row by its display key, not by position, so a
        // hidden parameter is simply not reported and keeps what it had. A
        // user who sets a Remap range, switches to Clip and switches back must
        // find their numbers still there.
        let mut params = vec![
            pd("Operation", "Remap", ""),
            pd("To Max", "7.5", "Operation == Remap"),
        ];
        assert_eq!(param_display(&params).len(), 2);
        params[0].default = "Clip".into();
        assert_eq!(param_display(&params).len(), 1, "the row hid");
        assert_eq!(params[1].default, "7.5", "but the value is untouched");
        params[0].default = "Remap".into();
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
                    .map(|(n, v)| crate::app::ParamDef {
                        name: n.to_string(),
                        label: String::new(),
                        param_type: "text".to_string(),
                        default: v.to_string(),
                        options: vec![],
                        min: None,
                        max: None,
                        step: None,
                        show_when: String::new(), expr: false,
                    })
                    .collect(),
                geometry_visible: true,
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
                    ("Preset", "Letter"),
                    ("Orientation", "Portrait"),
                    ("Resolution", "72"),
                    ("Color", "1.00:1.00:1.00"),
                ],
            ),
            pnode(
                "g",
                "grid1",
                "page_grid",
                &[
                    ("Input", "page1"),
                    ("Cell Size", "0.5"),
                    ("Line Width", "0.02"),
                    ("Line Color", "0.00:0.00:0.00"),
                    ("Fill Cells", "false"),
                ],
            ),
            pnode(
                "b",
                "border1",
                "page_border",
                &[("Input", "grid1"), ("Width", "0.1"), ("Inset", "0.25"), ("Color", "1.00:0.00:0.00")],
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
        let orphan = pnode("o", "border2", "page_border", &[("Input", "nothing")]);
        let mut lone = root.clone();
        lone.children.push(orphan);
        assert!(
            resolve_page(&lone, lone.children.last().unwrap(), &mut Vec::new()).is_none(),
            "a border with no page under it invented one"
        );

        // A cycle terminates rather than recursing forever.
        let mut looped = root.clone();
        looped.children[0] = pnode("p", "page1", "page_border", &[("Input", "border1")]);
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
        state.wire_color = [0.2, 0.6, 0.9, 0.5];
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
        // The control carries the live colour with its alpha, and a
        // toolkit colour selector stands behind it at the same value.
        assert_eq!(row.control, Some(Control::Color { hex: crate::project::color_to_hex8([0.2, 0.6, 0.9, 0.5]), alpha: true }));
        let sel = state.slots.dialog.color_selector(&id).expect("a colour selector behind the row");
        assert_eq!(sel.get_value_string().as_deref(), Some(crate::project::color_to_hex8([0.2, 0.6, 0.9, 0.5]).as_str()));
        let s = SETTINGS.iter().find(|s| s.label == "Wireframe Color").unwrap();
        assert_eq!(s.owner, Owner::Field("wire_color"));

        // Editing the row reaches the live state: the colour AND the switch
        // that makes the wire pass use it (off, the wires carry the
        // geometry's colours and the colour row is their alpha alone). The
        // dialog stays up, and the row re-reads the value.
        state.wire_single_color = false;
        state.apply_setting("Wireframe Color", "#000000ff");
        assert_eq!(state.wire_color, [0.0, 0.0, 0.0, 1.0], "the colour row writes the live wire colour");
        assert!(state.wire_single_color, "a colour edit turns single-colour mode on");
        assert!(state.dialog_visible());
        let row = state.slots.dialog.rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.control, Some(Control::Color { hex: "#000000ff".into(), alpha: true }));

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
    #[test]
    fn frame_all_frames_off_centre_geometry_without_a_camera_node_in_the_dir() {
        use crate::geometry::Vertex3D;
        let mut state = State::new(false);
        // The root holds Camera 1; a subnet holds no camera at all.
        state.active_camera = "camera1".to_string();
        let sub = state.current_dir().children.iter().position(|c| c.name == "sphere1").expect("sphere1 at the root");
        state.current_path.push(sub);
        state.on_path_changed();
        assert!(!state.current_dir().children.iter().any(|c| c.node_type == "camera"), "no camera in the subnet");
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
        assert_eq!(state.viewport().pivot, Vec3::ZERO);

        state.frame_all();

        let piv = state.viewport().pivot;
        for k in 0..3 {
            assert!((piv[k] - c[k]).abs() < 0.2, "pivot {piv:?} is not on the geometry's centre {c:?}");
        }
        assert!(state.viewport().zoom != zoom_before, "the fixed ray was fitted");
        assert!(state.viewport().zoom < 1.0, "a 0.25 sphere frames closer than the stock view: zoom {}", state.viewport().zoom);
    }

    /// What the scene file carries, and what it no longer does.
    ///
    /// It used to carry the viewport DISPLAY settings — the Render node's
    /// wireframe state and colour, the Guides node's grid and origin, Main's
    /// background — because the nodes holding them rode `fs_root` into the
    /// file. That made a preference part of the project: opening someone
    /// else's scene reset how you looked at geometry. Those settings persist
    /// to `state.kdl` now, and the scene file keeps what is genuinely the
    /// project's: the Default Camera VIEW (square aspect, pivot marker,
    /// orbit/zoom/pivot), which is where you were standing in this scene.
    #[test]
    fn viewport_settings_round_trip_through_the_scene_file() {
        let dir = std::env::temp_dir().join(format!("cce-designer-vp-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut a = State::new(false);
        // The Default Camera is active: a camera NODE's own Square Aspect and
        // pivot params would override the saved view's, by design.
        a.active_camera = "Default Camera".to_string();
        a.square_viewport = true;
        a.viewport_mut().show_camera_pivot = true;
        a.viewport_mut().rotation_y = 0.7;
        a.viewport_mut().zoom = 0.4;
        a.viewport_mut().pivot = Vec3::new(3.0, 0.5, -2.0);
        // A display setting, deliberately NOT expected to travel.
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
        assert!(!b.wireframe, "a display preference rode the project file");
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
        a.viewport_mut().show_cube = true;
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
        a.wire_color = [0.2, 0.4, 0.6, 0.5];
        a.wire_width = 3.0;
        a.geo_opacity = 0.75;
        a.render_points = true;
        a.point_size = 0.05;
        a.point_color = [0.0, 1.0, 0.0];
        a.group_marker_scale = 2.5;
        a.save_settings();

        let kdl = std::fs::read_to_string(DesignSettings::file_path()).expect("state.kdl was written");
        let back = DesignSettings::from_kdl_str(&kdl);
        let close = |x: f32, y: f32| (x - y).abs() < 0.01;

        assert!(!back.viewport.show_grid_enabled);
        assert!(back.viewport.show_cube_enabled);
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
            (back.render.point_color, [0.0, 1.0, 0.0]),
        ] {
            for k in 0..3 {
                assert!(close(got[k], want[k]), "colour {got:?} came back as {want:?}");
            }
        }
        assert!(back.render.wireframe && back.render.wire_single_color);
        // The wire colour is the four-component one: its ALPHA is the wire's
        // own opacity, and dropping it would silently make every wireframe
        // fully opaque.
        for k in 0..4 {
            assert!(close(back.render.wire_color[k], [0.2, 0.4, 0.6, 0.5][k]), "{:?}", back.render.wire_color);
        }
        assert!(close(back.render.wire_width, 3.0));
        assert!(close(back.render.geo_opacity, 0.75));
        assert!(back.render.render_points);
        assert!(close(back.render.point_size, 0.05));
        assert!(close(back.render.group_marker_scale, 2.5));
    }

    /// Changing the wire colour turns single-colour mode on, so the colour
    /// shows; turning the switch off afterwards sticks, and a LOAD never
    /// flips it — a project that says off stays off whatever colour it
    /// carries.
    #[test]
    fn changing_the_wire_colour_turns_single_colour_mode_on() {
        let mut state = State::new(false);
        state.wire_single_color = false;
        state.wire_color = [1.0, 1.0, 1.0, 1.0];

        // An edit through the dialog's colour row, which is the only way
        // in now that the Render node is gone.
        state.open_dialog();
        state.apply_setting("Wireframe Color", "#000000ff");
        assert_eq!(state.wire_color, [0.0, 0.0, 0.0, 1.0]);
        assert!(state.wire_single_color, "a colour change switches single-colour mode on");

        // Off again by hand stays off while the colour is unchanged: the
        // auto-enable fires on a CHANGE, not on every settings pass.
        state.wire_single_color = false;
        state.apply_setting("Wireframe Color", "#000000ff");
        assert!(!state.wire_single_color, "an unrelated pass flipped it back on");

        // A load carries the project's geometry and leaves the wire
        // settings — preferences now — exactly where they are.
        let dir = std::env::temp_dir().join(format!("cce-designer-wire-colour-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        state.save_to_file(&dir).expect("save");
        state.load_from_file(&dir).expect("load");
        assert_eq!(state.wire_color, [0.0, 0.0, 0.0, 1.0]);
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
        let body = &src[start..];
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
        assert_eq!(param(&state, "Center"), "0.00:0.00:0.00", "the centre moved");
        assert_eq!(param(&state, "Translation"), "0.40:0.00:0.00");

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
        assert_eq!(param(&state, "Center"), "0.10:0.00:0.00");
        assert_eq!(param(&state, "Translation"), "0.30:0.00:0.00", "the tip should not have moved");

        // And undo restores BOTH parameters, which is the case a "keep the
        // translation when the centre moves" rule would have broken.
        assert!(state.viewer_tool_undo());
        assert_eq!(param(&state, "Center"), "0.00:0.00:0.00");
        assert_eq!(param(&state, "Translation"), "0.40:0.00:0.00");
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
                "Points",
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

    /// The cases that would otherwise hang or overwrite: cycles, self
    /// reference, dangling names, and pinned cells.
    #[test]
    fn test_auto_layout_survives_cycles_and_pinned_nodes() {
        use crate::layout::{arrange, LayoutNode};
        let node = |name: &str, input: Option<&str>, pos: (f32, f32), pinned: bool| LayoutNode {
            name: name.to_string(),
            input: input.map(|s| s.to_string()),
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
                p.default = value.to_string();
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
    /// Cube hid the cube, and editing any node's parameter brought it back.
    /// The fix was to write the node as well; the node is gone now and the
    /// flag is simply the value, which is the same guarantee with nothing
    /// left to fall out of step. Still asserted, because the failure it
    /// catches (an edit reverting a display toggle) is invisible in a test
    /// that only flips the toggle.
    #[test]
    fn guide_toggles_survive_the_settings_apply_pass() {
        let mut state = State::new(false);
        for command in ["toggle_cube", "toggle_grid", "toggle_origin", "toggle_point_markers"] {
            let flag = |state: &State| match command {
                "toggle_cube" => state.viewport().show_cube,
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
                .apply_action(crate::app::McpAction::SetParam { slot: sphere, name: "Radius".into(), value: "0.7".into() }, &mut redraw)
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

        // Stand in for a composed page: an id owned by State and borrowed by
        // the view.
        state.page_image = Some(7);
        state.slots.page_view.set_image(Some((7, 100, 100)));

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
        assert!(state.slots.page_view.image.is_none(), "the view still borrows a dead id");
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

    fn ref_node(id: &str, name: &str, node_type: &str, params: Vec<(&str, &str, &str)>, children: Vec<FsNode>) -> FsNode {
        use crate::app::ParamDef;
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
                    ParamDef { name: n.into(), label: String::new(), param_type: ptype, default: d.into(), options, min: None, max: None, step: None, show_when: String::new(), expr: crate::expr::looks_like_expression(d) }
                })
                .collect(),
            geometry_visible: true,
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
                    "../Radius" => Value::Num(0.5),
                    "../sphere1/Rows" => Value::Num(16.0),
                    "Mode" => Value::Num(1.0),
                    "../text1/Font" => Value::Str("Inter".into()),
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
        assert_eq!(ev("ch(\"../Radius\") * 2 + 1"), Value::Num(2.0));
        assert_eq!(ev("chi(\"../sphere1/Rows\") / 3"), Value::Num(16.0 / 3.0));
        assert_eq!(ev("chb(\"Mode\")"), Value::Num(1.0));
        assert_eq!(ev("chs(\"../text1/Font\") + \" Bold\""), Value::Str("Inter Bold".into()));
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
        assert!(looks_like_expression("ch(\"../sphere1/Radius\")"));
        assert!(looks_like_expression("chf(\"../Radius\") * 2"));
        assert!(looks_like_expression("$F / 24"));
        assert!(!looks_like_expression("1 + 2"), "arithmetic alone is asked for through Edit Expression");
        assert!(!looks_like_expression("0.5"));
        assert!(!looks_like_expression("sphere1"));
        assert!(!looks_like_expression("true"));
        assert!(!looks_like_expression("0.00:0.80:0.00"));
        assert!(!looks_like_expression("float r = chf(\"Radius\", 0.5);"), "a kernel is not a reference");
        let kernel = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/nodes/sphere.json")).unwrap();
        assert!(!looks_like_expression(&kernel));
    }

    /// A rename rewrites the channel paths that pass through the node —
    /// textually, so the user's spacing survives — and leaves every other
    /// path alone.
    #[test]
    fn rename_rewrites_the_paths_through_a_node() {
        use crate::expr::rewrite_paths;
        let src = "ch( \"../sphere1/Radius\" ) * chs('../text1/Font') + chf(\"/sphere1/Rows\")";
        let out = rewrite_paths(src, |path| path.contains("sphere1").then(|| path.replace("sphere1", "ball")));
        assert_eq!(out, "ch( \"../ball/Radius\" ) * chs('../text1/Font') + chf(\"/ball/Rows\")");
        assert_eq!(rewrite_paths("touch(\"x\")", |_| Some("no".into())), "touch(\"x\")", "only channel calls are paths");
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
        proj.root.children[0].children[0].params[0].expr = false;
        proj.migrate_param_refs();
        let r = &proj.root.children[0].children[0].params[0];
        assert_eq!(r.default, "chf(\"../Size\")");
        assert!(r.expr);
        assert_eq!(proj.format, crate::app::PROJECT_FORMAT);
        // A NEW file's bare name is the node's own parameter and stays.
        proj.root.children[0].children[0].params[0].default = "chf(\"Radius\")".into();
        proj.migrate_param_refs();
        assert_eq!(proj.root.children[0].children[0].params[0].default, "chf(\"Radius\")");
    }

    #[test]
    fn param_references_resolve_against_the_enclosing_subnet() {
        use crate::geometry::resolve_param_refs;
        let sphere = ref_node("s", "sphere1", "sphere", vec![("Radius", "slider", "ch(\"../Radius\")")], vec![]);
        let output = ref_node("o", "output1", "output", vec![("Input", "text", "sphere1")], vec![]);
        let inner = ref_node("sub", "shape1", "node",
            vec![("Radius", "slider", "chf(\"../Size\")"), ("Mode", "choice:Basic,Scatter", "Scatter"), ("On", "toggle", "true")],
            vec![sphere, output]);
        let probe = ref_node("p", "probe", "switch",
            vec![("Index", "spinbox", "chi(\"../Mode\")"), ("Flag", "text", "chb(\"../On\")"), ("Name", "text", "chs(\"../Mode\")"), ("Plain", "text", "kept")],
            vec![]);
        let mut inner = inner;
        inner.children.push(probe);
        let outer = ref_node("outer", "outer1", "node", vec![("Size", "slider", "0.8")], vec![inner]);
        let root = ref_node("root", "root", "node", vec![], vec![outer]);

        // The probe's params, resolved against shape1.
        let probe = &root.children[0].children[0].children[2];
        let mut err = None;
        let resolved = resolve_param_refs(&root, probe, 0, &mut err).expect("it has references");
        assert!(err.is_none(), "{err:?}");
        let get = |n: &str| resolved.params.iter().find(|p| p.name == n).unwrap().default.clone();
        assert_eq!(get("Index"), "1", "chi on a choice is its option index");
        assert_eq!(get("Flag"), "1", "chb into a text row is 1 or 0");
        assert_eq!(get("Name"), "Scatter");
        assert_eq!(get("Plain"), "kept");

        // Evaluated, the sphere's Radius chains: sphere1 → shape1's Radius,
        // which is itself chf("../Size") → outer1's 0.8.
        let shape = &root.children[0].children[0];
        let (g, err) = eval(&root, shape);
        assert!(err.is_none(), "{err:?}");
        assert!((radius_of(&g.expect("geometry")) - 0.8).abs() < 0.02, "the radius came from the outermost control");

        // A node with no references is left alone: no clone, no error.
        let mut none = None;
        assert!(resolve_param_refs(&root, &root.children[0].children[0].children[1], 0, &mut none).is_none());
        assert!(none.is_none());

        // A reference to nothing is reported, and the value left as written.
        let bad = ref_node("b", "bad1", "sphere", vec![("Radius", "slider", "ch(\"../Nope\")")], vec![]);
        let holder = ref_node("h", "holder1", "node", vec![], vec![bad]);
        let root2 = ref_node("root", "root", "node", vec![], vec![holder]);
        let mut err = None;
        let r = resolve_param_refs(&root2, &root2.children[0].children[0], 0, &mut err).unwrap();
        assert_eq!(r.params[0].default, "ch(\"../Nope\")");
        assert!(err.as_deref().unwrap_or("").contains("names no parameter Nope on holder1"), "{err:?}");
        // Too many levels up, likewise.
        let far = ref_node("f", "far1", "sphere", vec![("Radius", "slider", "ch(\"../../../X\")")], vec![]);
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
        let a = ref_node("a", "a1", "sphere", vec![("Radius", "slider", "0.25"), ("Center", "float3", "1:2:3")], vec![]);
        let b = ref_node("b", "b1", "sphere", vec![
            ("Radius", "slider", "ch(\"../a1/Radius\") * 2"),
            ("Rows", "spinbox", "ch(\"Radius\") * 100"),
            ("Y", "slider", "ch(\"../a1/Center.y\") + ch(\"/sub1/a1/Center.z\")"),
            ("Frame", "slider", "$F / 2"),
            ("Up", "slider", "ch(\"../Size\") + ch(\"/Top\")"),
            ("Mode", "choice:Basic,Scatter", "1"),
            ("On", "toggle", "ch(\"../a1/Radius\") > 0"),
            ("Label", "text", "chs(\"../a1/Radius\") + \" units\""),
            ("Center", "float3", "chf(\"../a1/Center.x\"):0:ch(\"../Size\")"),
        ], vec![]);
        let sub = ref_node("sub", "sub1", "node", vec![("Size", "slider", "0.5")], vec![a, b]);
        let root = ref_node("root", "root", "node", vec![("Top", "slider", "10")], vec![sub]);
        // The choice's value is an index written as an expression; flag it.
        let mut root = root;
        root.children[0].children[1].params.iter_mut().find(|p| p.name == "Mode").unwrap().expr = true;

        let b = &root.children[0].children[1];
        let mut err = None;
        let r = resolve_param_refs(&root, b, 12, &mut err).expect("b1 has expressions");
        assert!(err.is_none(), "{err:?}");
        let get = |n: &str| r.params.iter().find(|p| p.name == n).unwrap().default.clone();
        assert_eq!(get("Radius"), "0.5", "a sibling by path");
        assert_eq!(get("Rows"), "50", "a bare name is the node's OWN parameter, read through its expression");
        assert_eq!(get("Y"), "5", "components, relative and absolute");
        assert_eq!(get("Frame"), "6");
        assert_eq!(get("Up"), "10.5", "the parent and the root");
        assert_eq!(get("Mode"), "Scatter", "a number into a choice picks the option");
        assert_eq!(get("On"), "true", "a number into a toggle is true or false");
        assert_eq!(get("Label"), "0.25 units");
        assert_eq!(get("Center"), "1:0:0.5", "a float3 is three expressions");
        assert!(r.params.iter().all(|p| !p.expr), "the resolved clone holds values");

        // A circle: two parameters reading each other.
        let x = ref_node("x", "x1", "sphere", vec![("Radius", "slider", "ch(\"../y1/Radius\")")], vec![]);
        let y = ref_node("y", "y1", "sphere", vec![("Radius", "slider", "ch(\"../x1/Radius\") + 1")], vec![]);
        let ring = ref_node("root", "root", "node", vec![], vec![x, y]);
        let mut err = None;
        let r = resolve_param_refs(&ring, &ring.children[0], 0, &mut err).unwrap();
        assert!(err.as_deref().unwrap_or("").contains("circular"), "{err:?}");
        assert_eq!(r.params[0].default, "ch(\"../y1/Radius\")", "left as written");
        // A parameter reading itself is the shortest circle.
        let me = ref_node("m", "me", "sphere", vec![("Radius", "slider", "ch(\"Radius\") + 1")], vec![]);
        let solo = ref_node("root", "root", "node", vec![], vec![me]);
        let mut err = None;
        resolve_param_refs(&solo, &solo.children[0], 0, &mut err);
        assert!(err.as_deref().unwrap_or("").contains("circular"), "{err:?}");

        // A path to a node that is not there names the step that failed.
        let lost = ref_node("l", "lost", "sphere", vec![("Radius", "slider", "ch(\"../nope/Radius\")")], vec![]);
        let root4 = ref_node("root", "root", "node", vec![], vec![lost]);
        let mut err = None;
        resolve_param_refs(&root4, &root4.children[0], 0, &mut err);
        assert!(err.as_deref().unwrap_or("").contains("no node `nope`"), "{err:?}");
        // A syntax error names the parameter.
        let broken = ref_node("k", "broken", "sphere", vec![("Radius", "slider", "1 +")], vec![]);
        let mut broken = broken;
        broken.params[0].expr = true;
        let root5 = ref_node("root", "root", "node", vec![], vec![broken]);
        let mut err = None;
        resolve_param_refs(&root5, &root5.children[0], 0, &mut err);
        assert!(err.as_deref().unwrap_or("").contains("broken: Radius"), "{err:?}");
    }

    /// The paths a paste writes, and what a rename does to the paths that
    /// stand: `rename_node_in_tree` rewrites every expression whose path
    /// passes through the node and every wire naming it, and leaves a
    /// same-named node elsewhere alone.
    #[test]
    fn renaming_a_node_carries_its_references() {
        use crate::geometry::{absolute_ref_path, relative_ref_path, rename_node_in_tree};
        let a = ref_node("a", "a1", "sphere", vec![("Radius", "slider", "0.25")], vec![]);
        let b = ref_node("b", "b1", "sphere", vec![
            ("Radius", "slider", "ch( \"../a1/Radius\" ) * 2"),
            ("Input", "text", "a1"),
        ], vec![]);
        let deep = ref_node("d", "deep1", "sphere", vec![("Radius", "slider", "chf(\"/sub1/a1/Radius\") + ch(\"../../a1/Radius\")")], vec![]);
        let inner = ref_node("in", "inner1", "node", vec![], vec![deep]);
        let other = ref_node("oa", "a1", "sphere", vec![("Radius", "slider", "ch(\"../a1/Radius\")")], vec![]);
        let elsewhere = ref_node("el", "elsewhere", "node", vec![], vec![other]);
        let sub = ref_node("sub", "sub1", "node", vec![("Size", "slider", "0.5")], vec![a, b, inner]);
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
            node.params.iter().find(|p| p.name == n).unwrap().default.clone()
        };
        assert_eq!(root.children[0].children[0].name, "ball");
        assert_eq!(get(&root, &[0, 1], "Radius"), "ch( \"../ball/Radius\" ) * 2", "spacing kept");
        assert_eq!(get(&root, &[0, 1], "Input"), "ball", "the wire follows");
        assert_eq!(get(&root, &[0, 2, 0], "Radius"), "chf(\"/sub1/ball/Radius\") + ch(\"../../ball/Radius\")");
        assert_eq!(get(&root, &[1, 0], "Radius"), "ch(\"../a1/Radius\")", "the OTHER a1 is not this one");
        assert!(!rename_node_in_tree(&mut root, "a", "ball"), "a rename to the same name is nothing");
        assert!(!rename_node_in_tree(&mut root, "zzz", "x"), "and so is one of a node that is not there");
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
        show(&mut state, sphere);
        let (x, y) = row_center(&state, "Radius");
        assert_eq!(state.param_row_at(x, y), Some((sphere, "Radius".to_string())));
        assert_eq!(state.param_row_at(x, state.positions[crate::slots::PARAM_IDX].1 - 5.0), None, "above the pane is no row");

        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right });
        assert!(state.param_menu_open(), "a right press on a row opens its menu");
        assert!(!state.viewport_menu_open());
        assert_eq!(state.param_menu_actions, vec![ParamMenuAction::CopyParameter, ParamMenuAction::Separator, ParamMenuAction::EditExpression], "nothing copied yet, and the row holds a value");
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right });

        // Copy, then paste onto ball's Radius — a sibling, so `../sphere1`.
        let sphere_id = state.current_dir().children[sphere].id.clone();
        let ball_id = state.current_dir().children[ball].id.clone();
        state.run_param_action(&sphere_id, "Radius", ParamMenuAction::CopyParameter);
        assert_eq!(state.copied_param, Some((sphere_id.clone(), "Radius".to_string())));
        show(&mut state, ball);
        let (x, y) = row_center(&state, "Radius");
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right });
        assert!(state.param_menu_actions.contains(&ParamMenuAction::PasteRelative), "with a copy, paste is offered");
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Right });
        state.run_param_action(&ball_id, "Radius", ParamMenuAction::PasteRelative);
        let radius = |state: &State, slot: usize| state.current_dir().children[slot].params.iter().find(|p| p.name == "Radius").unwrap().clone();
        assert_eq!(radius(&state, ball).default, "ch(\"../sphere1/Radius\")");
        assert!(radius(&state, ball).expr);
        let rows = crate::app::param_display(&state.current_dir().children[ball].params);
        assert_eq!(rows.iter().find(|r| r.0 == "Radius").unwrap().2, "text", "the pane shows an expression as text");

        // The reference is live: ball follows sphere1's Radius.
        state.apply_action(McpAction::SetParam { slot: sphere, name: "Radius".into(), value: "0.9".into() }, &mut redraw).unwrap();
        let mut err = None;
        let r = crate::geometry::resolve_param_refs(&state.fs_root, &state.current_dir().children[ball], 0, &mut err).unwrap();
        assert!(err.is_none(), "{err:?}");
        assert_eq!(r.params.iter().find(|p| p.name == "Radius").unwrap().default, "0.9");

        // Absolute paste, then Delete Expression bakes the current value.
        state.run_param_action(&ball_id, "Radius", ParamMenuAction::PasteAbsolute);
        assert_eq!(radius(&state, ball).default, "ch(\"/sphere1/Radius\")");
        state.run_param_action(&ball_id, "Radius", ParamMenuAction::DeleteExpression);
        assert_eq!(radius(&state, ball).default, "0.9");
        assert!(!radius(&state, ball).expr);
        // Edit Expression flags without changing.
        state.run_param_action(&ball_id, "Radius", ParamMenuAction::EditExpression);
        assert_eq!(radius(&state, ball).default, "0.9");
        assert!(radius(&state, ball).expr);

        // And a reference typed straight into a row (or scripted) becomes one.
        state.apply_action(McpAction::SetParam { slot: ball, name: "Rows".into(), value: "chi(\"../sphere1/Rows\") * 2".into() }, &mut redraw).unwrap();
        let rows_p = state.current_dir().children[ball].params.iter().find(|p| p.name == "Rows").unwrap();
        assert!(rows_p.expr);

        // A rename carries the paste along.
        state.apply_action(McpAction::RenameNode { slot: sphere, new_name: "orb".into() }, &mut redraw).unwrap();
        assert_eq!(state.current_dir().children[ball].params.iter().find(|p| p.name == "Rows").unwrap().default, "chi(\"../orb/Rows\") * 2");
    }

    /// A code parameter never becomes an expression, however its text reads:
    /// a one-line script that IS `ch("../a/Radius")` is a program to run,
    /// and flagging it would evaluate it to a number first. Neither the
    /// template loader nor a scripted set_param flags one.
    #[test]
    fn a_code_parameter_is_never_an_expression() {
        use crate::app::{infer_template_exprs, McpAction};
        let mut node = ref_node("w", "w1", "wrangle", vec![("Code", "code", "ch(\"../a/Radius\")"), ("Radius", "slider", "ch(\"../a/Radius\")")], vec![]);
        for p in &mut node.params {
            p.expr = false;
        }
        infer_template_exprs(&mut node);
        assert!(!node.params[0].expr, "the code stays a program");
        assert!(node.params[1].expr, "the slider becomes an expression");

        let mut state = State::new(false);
        let mut redraw = false;
        state.apply_action(McpAction::AddNode { template_name: "Wrangle".into(), name: Some("k".into()), x: 3.0, y: 9.0 }, &mut redraw).unwrap();
        let k = state.current_dir().children.iter().position(|c| c.name == "k").unwrap();
        state.apply_action(McpAction::SetParam { slot: k, name: "Code".into(), value: "chf(\"../sphere1/Radius\")".into() }, &mut redraw).unwrap();
        let code = state.current_dir().children[k].params.iter().find(|p| p.name == "Code").unwrap();
        assert!(!code.expr);
        assert_eq!(code.param_type, "code");
    }

    /// Inside the SECOND instance of a subnet, a child wired to a sibling by
    /// name finds its own sibling, not the first instance's.
    #[test]
    fn input_lookups_prefer_siblings() {
        let make = |id: &str, name: &str, radius: &str| {
            let src = ref_node(&format!("{id}-src"), "src", "sphere", vec![("Radius", "slider", radius)], vec![]);
            let out = ref_node(&format!("{id}-out"), "output1", "output", vec![("Input", "text", "src")], vec![]);
            ref_node(id, name, "node", vec![], vec![src, out])
        };
        let root = ref_node("root", "root", "node", vec![], vec![make("a", "shape1", "0.3"), make("b", "shape2", "0.9")]);
        let (g, _) = eval(&root, &root.children[1]);
        assert!((radius_of(&g.unwrap()) - 0.9).abs() < 0.02, "shape2's output read shape1's src");
        // Sibling-first, then anywhere: a name with no sibling still resolves globally.
        let global = ref_node("g", "global1", "sphere", vec![("Radius", "slider", "0.6")], vec![]);
        let user = ref_node("u", "user1", "node", vec![], vec![ref_node("u-out", "output1", "output", vec![("Input", "text", "global1")], vec![])]);
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
        assert_eq!(switch_input_param(0), "Input");
        assert_eq!(switch_input_param(1), "Input 2");
        assert_eq!(switch_input_param(3), "Input 4");
        let a = ref_node("a", "a1", "sphere", vec![("Radius", "slider", "0.2")], vec![]);
        let b = ref_node("b", "b1", "sphere", vec![("Radius", "slider", "0.7")], vec![]);
        let sw = |index: &str| ref_node("sw", "switch1", "switch",
            vec![("Input", "text", "a1"), ("Input 2", "text", "b1"), ("Input 3", "text", ""), ("Input 4", "text", ""), ("Index", "spinbox", index)], vec![]);
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
        let a = ref_node("a", "small", "sphere", vec![("Radius", "slider", "0.2")], vec![]);
        let b = ref_node("b", "big", "sphere", vec![("Radius", "slider", "0.7")], vec![]);
        let sw = ref_node("sw", "switch1", "switch",
            vec![("Input", "text", "small"), ("Input 2", "text", "big"), ("Index", "spinbox", "chi(\"../Size\")")], vec![]);
        let out = ref_node("o", "output1", "output", vec![("Input", "text", "switch1")], vec![]);
        let mut a = a; a.geometry_visible = false;
        let mut b = b; b.geometry_visible = false;
        let sub = |size: &str| ref_node("sub", "pick1", "node", vec![("Size", "choice:Small,Big", size)], vec![a.clone(), b.clone(), sw.clone(), out.clone()]);
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

    /// A Sphere TEMPLATE instance reads its kernel's chf("Radius") through
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
        let radius = sphere.params.iter_mut().find(|p| p.name == "Radius").unwrap();
        radius.default = "ch(\"../Radius\")".into();
        radius.expr = true;
        let out = ref_node("o", "output1", "output", vec![("Input", "text", "sphere1")], vec![]);
        let sub = ref_node("sub", "subnet1", "node", vec![("Input", "text", ""), ("Radius", "slider", "0.9")], vec![sphere, out]);
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
        use crate::app::ParamDef;
        let params = vec![
            ParamDef { name: "Radius".into(), label: String::new(), param_type: "slider".into(), default: "ch(\"../Radius\")".into(), options: vec![], min: Some(0.0), max: Some(2.0), step: None, show_when: String::new(), expr: true },
            ParamDef { name: "Rows".into(), label: String::new(), param_type: "spinbox".into(), default: "16".into(), options: vec![], min: Some(2.0), max: Some(128.0), step: Some(1.0), show_when: String::new(), expr: false },
        ];
        let rows = crate::app::param_display(&params);
        assert_eq!(rows[0], ("Radius".to_string(), "ch(\"../Radius\")".to_string(), "text".to_string()));
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
        let src = ref_node("s", "src", "points", vec![("Shape", "text", "Spiral"), ("Points", "spinbox", "60"), ("Markers", "text", "false")], vec![]);
        let hull_node = ref_node("h", "hull1", "hull", vec![("Input", "text", "src")], vec![]);
        let root = ref_node("root", "root", "node", vec![], vec![src, hull_node]);
        let (g, err) = eval(&root, &root.children[1]);
        assert!(err.is_none(), "{err:?}");
        let g = g.unwrap();
        assert!(g.num_prims() > 0 && g.is_closed(), "the spiral hulls into a closed mesh");
        let line = ref_node("l", "line", "points", vec![("Shape", "text", "Line"), ("Points", "spinbox", "5"), ("Markers", "text", "false")], vec![]);
        let hull2 = ref_node("h2", "hull2", "hull", vec![("Input", "text", "line")], vec![]);
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
            let src = ref_node("s", "src", "sphere", vec![("Radius", "slider", "0.5")], vec![]);
            let sc = ref_node("sc", "scatter1", "scatter", vec![
                ("Input", "text", "src"), ("Mode", "choice:Volume,Surface", "Surface"), ("Points", "spinbox", "80"),
                ("Seed", "slider", "1.1"), ("Relax Points", "toggle", relax), ("Relax Iterations", "spinbox", "30"),
                ("Markers", "choice:true,false", "false"),
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
            let src = ref_node("s", "src", "sphere", vec![("Radius", "slider", "0.5")], vec![]);
            let rx = ref_node("r", "relax1", "relax", vec![
                ("Input", "text", "src"), ("Mode", "choice:Springs,Repel", "Repel"), ("Iterations", "spinbox", iterations),
                ("Radius", "slider", "0.08"), ("In 3D Space", "toggle", in_3d),
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
        assert!(sphere1.params.iter().any(|p| p.name == "Method"), "with its template's whole surface");
        assert!(sphere1.params.iter().find(|p| p.name == "Radius").unwrap().expr, "and the Embryo's reference on its Radius");

        let instance = |overrides: &[(&str, &str)], extra: Vec<FsNode>| {
            let mut inst = t.clone();
            crate::app::regenerate_node_ids(&mut inst);
            inst.name = "embryo1".into();
            for (n, v) in overrides {
                inst.params.iter_mut().find(|p| p.name == *n).unwrap_or_else(|| panic!("param {n}")).default = v.to_string();
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
        let big = run(&instance(&[("Radius", "1.5"), ("Base Resolution", "8")], vec![]));
        assert!((extent(&big) - 1.5).abs() < 0.05, "Radius reaches the sphere through chf: {}", extent(&big));
        assert!(big.num_points() < basic.num_points(), "Base Resolution reaches Rows and Columns through chi");

        let scattered = run(&instance(&[("Method", "Scatter"), ("Scatter Count", "400"), ("Base Resolution", "16")], vec![]));
        assert!(scattered.num_prims() > 0 && scattered.is_closed(), "Scatter hulls the points into a closed mesh");
        assert!(scattered.num_points() <= 400);
        assert!(extent(&scattered) <= 0.5 + 1e-3, "the hull lies inside the seed sphere");
        assert!(scattered.points().value("N", 0).is_some());

        let seed = ref_node("seed", "seed", "sphere", vec![("Radius", "slider", "0.25")], vec![]);
        let root = instance(&[("Source", "Input"), ("Input", "seed")], vec![seed]);
        let from_input = run(&root);
        let seed_geom = eval(&root, &root.children[0]).0.unwrap();
        assert_eq!(from_input.num_points(), seed_geom.num_points(), "Source Input is the wired node");
        assert!((from_input.pos(0) - seed_geom.pos(0)).length() < 1e-6);
        let e = instance(&[("Source", "Input")], vec![]);
        assert!(eval(&e, &e.children[0]).0.is_none(), "Source Input with nothing wired seeds nothing");

        let coarse = run(&instance(&[("Base Resolution", "8")], vec![]));
        let sub = run(&instance(&[("Base Resolution", "8"), ("Subdivision Depth", "1")], vec![]));
        // Against the real subdivide of the same mesh rather than ×4: the
        // kernel sphere's pole triangles are degenerate and subdivide drops
        // them.
        assert_eq!(sub.num_prims(), crate::remesh::subdivide(&coarse, 1).num_prims(), "Subdivision Depth reaches Depth");
    }

    /// A native embryo from 2026-09-21 loads as an instance of the template,
    /// with its values, its identity and its meta child intact.
    #[test]
    fn a_native_embryo_recomposes_on_load() {
        use crate::app::ParamDef;
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let param = |n: &str, v: &str| ParamDef { name: n.into(), label: String::new(), param_type: "text".into(), default: v.into(), options: vec![], min: None, max: None, step: None, show_when: String::new(), expr: false };
        let meta = ref_node("m", "meta", "meta", vec![("Point Markers", "toggle", "true")], vec![]);
        let mut native = ref_node("old-id", "embryo1", "embryo", vec![], vec![meta]);
        native.params = vec![param("Input", ""), param("Method", "Scatter"), param("Scatter Count", "150"), param("Radius", "0.7"), param("Base Resolution", "16")];
        native.position = (3.0, 4.0);
        native.geometry_visible = true;
        let mut root = ref_node("root", "root", "node", vec![], vec![native]);
        crate::app::merge_template_defs(&mut root, &templates);
        let e = &root.children[0];
        assert_eq!(e.node_type, "node", "recomposed as a subnet");
        assert_eq!((e.id.as_str(), e.name.as_str(), e.position, e.geometry_visible), ("old-id", "embryo1", (3.0, 4.0), true));
        let get = |n: &str| e.params.iter().find(|p| p.name == n).unwrap().default.clone();
        assert_eq!(get("Method"), "Scatter");
        assert_eq!(get("Scatter Count"), "150");
        assert_eq!(get("Radius"), "0.7");
        assert_eq!(get("Scatter Seed"), "1.1", "a param the native node lacked takes the template default");
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
                    .map(|(n, v)| crate::app::ParamDef {
                        name: n.to_string(),
                        label: String::new(),
                        param_type: "text".to_string(),
                        default: v.to_string(),
                        options: vec![],
                        min: None,
                        max: None,
                        step: None,
                        show_when: String::new(), expr: false,
                    })
                    .collect(),
                geometry_visible: true,
                position: (0.0, 0.0),
                inputs: 1,
                outputs: 1,
            }
        }
        let sphere = mnode("id-s", "sphere1", "sphere", &[("Radius", "0.8")]);
        let shell = mnode(
            "id-m",
            "mold1",
            "mold_shell",
            &[
                ("Input", "sphere1"),
                ("Maximum Thickness", "0.20"),
                ("Minimum Thickness", "0.10"),
                ("Remesh Division Size", "0.30"),
                ("Ramp", "Linear"),
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
            vec![phase3_node("polygon", &[("Sides", "5"), ("Fill", "false")])],
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
        let mut children = vec![phase3_node("sphere", &[("Radius", radius)])];
        children.extend(nodes);
        FsNode {
            id: "root".into(),
            name: "root".into(),
            node_type: "node".into(),
            children,
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        }
    }

    #[test]
    fn test_normal_publishes_the_surface_normal_as_data() {
        let root = modelling_root(
            "1.0",
            vec![phase3_node("normal", &[("Input", "sphere 1"), ("Attribute", "N")])],
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
            vec![phase3_node("normal", &[("Input", "sphere 1"), ("Flip", "true")])],
        );
        let (f, _) = eval_node(&flipped, "normal 1");
        let (lo, hi) = f.bounds().unwrap();
        let radial = f.pos(0) - (lo + hi) * 0.5;
        assert!(f.points().value("N", 0).unwrap().as_vec3().dot(radial) < 0.0);
    }

    #[test]
    fn test_bounds_measures_into_detail_attributes() {
        let root = modelling_root(
            "2.0",
            vec![phase3_node("bounds", &[("Input", "sphere 1"), ("Prefix", "bb")])],
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
                        ("Input", "points 1"),
                        ("To", "sphere 1"),
                        ("Attribute", "dist"),
                        ("Direction", "toward"),
                    ],
                ),
            ],
        );
        // Put the sample points somewhere known: a Points node in "Line" mode
        // lays them along X.
        root.children[1]
            .params
            .push(crate::app::ParamDef {
                name: "Shape".into(),
                label: String::new(),
                param_type: "text".into(),
                default: "Line".into(),
                options: vec![],
                min: None,
                max: None,
                step: None,
                show_when: String::new(), expr: false,
            });

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
            vec![phase3_node("distance", &[("Input", "sphere 1"), ("To", "nope")])],
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

        let node = phase3_node("connectivity", &[("Attribute", "piece")]);
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
            let n = phase3_node("connectivity", &[("Attribute", "piece")]);
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
                phase3_node("connectivity", &[("Input", "sphere 1"), ("Attribute", "piece")]),
                phase3_node(
                    "cull",
                    &[
                        ("Input", "connectivity 1"),
                        ("Attribute", "piece"),
                        ("Comparison", "Below"),
                        ("Threshold", "1.00"),
                        ("Invert", "false"),
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
            .find(|p| p.name == "Invert")
            .unwrap()
            .default = "true".into();
        let (kept, _) = eval_node(&kept_root, "cull 1");
        assert_eq!(kept.num_points(), sphere_detail(Vec3::ZERO, 1.0, 16, 24).num_points());
        assert!(kept.num_prims() > 0, "the surface survived with its primitives");

        // A missing attribute is reported, and nothing is deleted on a guess.
        let broken = modelling_root(
            "1.0",
            vec![phase3_node("cull", &[("Input", "sphere 1"), ("Attribute", "nope")])],
        );
        let (g, err) = eval_node(&broken, "cull 1");
        assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
        assert!(g.num_points() > 0, "nothing should be culled on an error");

        // With neither a group nor an attribute there is no selection at all.
        let idle = modelling_root("1.0", vec![phase3_node("cull", &[("Input", "sphere 1")])]);
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
                phase3_node("points", &[("Shape", "Line"), ("Points", "6"), ("Markers", "false")]),
                phase3_node(
                    "transfer",
                    &[
                        ("Input", "points 1"),
                        ("From", "sphere 1"),
                        ("Attributes", "Norm"),
                        ("Maximum Distance", "0.00"),
                    ],
                ),
            ],
        );
        let (g, err) = eval_node(&root, "transfer 1");
        assert!(err.is_none(), "{err:?}");
        // No limit: everything finds a nearest source point however far.
        assert!(g.points().has("Norm"));
        assert!(
            (0..g.num_points()).any(|p| g.points().value("Norm", p).unwrap().as_vec3() != Vec3::ZERO),
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
            .find(|p| p.name == "Maximum Distance")
            .unwrap()
            .default = "0.01".into();
        let (g, _) = eval_node(&limited, "transfer 1");
        assert!(g.points().has("Norm"), "the column exists even where nothing was near");
        assert!(
            (0..g.num_points()).all(|p| g.points().value("Norm", p).unwrap().as_vec3() == Vec3::ZERO),
            "something transferred from out of range"
        );

        // A source it cannot resolve is reported rather than silently doing
        // nothing.
        let broken = modelling_root(
            "1.0",
            vec![phase3_node("transfer", &[("Input", "sphere 1"), ("From", "nope")])],
        );
        let (_, err) = eval_node(&broken, "transfer 1");
        assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
    }

    #[test]
    fn test_valence_counts_what_the_remesher_steers_toward() {
        use crate::remesh::{remesh, Settings};
        let root = modelling_root(
            "1.0",
            vec![phase3_node("valence", &[("Input", "sphere 1"), ("Attribute", "valence")])],
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
            let node = phase3_node("deform", &[("Mode", mode), ("Axis", "Y"), ("Amount", amount)]);
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
            vec![phase3_node("points", &[("Shape", "Line"), ("Points", "5")])],
        );
        let (with, _) = eval_node(&markers, "points 1");
        assert!(with.num_prims() > 0, "markers are geometry");
        assert!(with.num_points() > 5);

        let bare = modelling_root(
            "1.0",
            vec![phase3_node(
                "points",
                &[("Shape", "Line"), ("Points", "5"), ("Markers", "false")],
            )],
        );
        let (without, _) = eval_node(&bare, "points 1");
        assert_eq!(without.num_points(), 5, "one point per location");
        assert_eq!(without.num_prims(), 0, "bare points are not geometry");

        // Default is unchanged, so no existing project looks different.
        let defaulted = modelling_root(
            "1.0",
            vec![phase3_node("points", &[("Shape", "Line"), ("Points", "5")])],
        );
        assert_eq!(eval_node(&defaulted, "points 1").0.num_points(), with.num_points());
    }

    #[test]
    fn test_copy_puts_geometry_at_every_target_point() {
        let root = modelling_root(
            "0.2",
            vec![
                phase3_node("points", &[("Shape", "Line"), ("Points", "5")]),
                phase3_node("copy", &[("Input", "sphere 1"), ("To", "points 1")]),
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
                phase3_node("points", &[("Shape", "Line"), ("Points", "4")]),
                phase3_node(
                    "normal",
                    &[("Input", "points 1"), ("Attribute", "N")],
                ),
                phase3_node(
                    "copy",
                    &[
                        ("Input", "sphere 1"),
                        ("To", "points 1"),
                        ("Scale", "2.00"),
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
            .find(|p| p.name == "Scale")
            .unwrap()
            .default = "0.50".into();
        let (small, _) = eval_node(&root, "copy 1");
        let small_size = small.bounds().map(|(a, b)| (b - a).length()).unwrap();
        assert!(small_size < plain_size, "{small_size} should be under {plain_size}");
        assert_eq!(small.num_points(), plain.num_points());

        // A copy big enough to hang the app is refused with a number rather
        // than attempted.
        let huge = modelling_root(
            "1.0",
            vec![
                phase3_node("points", &[("Shape", "Grid"), ("Points", "9000")]),
                phase3_node("copy", &[("Input", "sphere 1"), ("To", "points 1")]),
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
                ("Translation", "0.00:1.00:0.00"),
                ("Group", "top"),
                ("Radius", "0.80"),
                ("Falloff", "Smooth"),
                ("Attribute", "falloff"),
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
            &[("Translation", "0.00:0.50:0.00"), ("Group", "ring"), ("Radius", "0.30")],
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
                phase3_node("connectivity", &[("Input", "sphere 1"), ("Attribute", "piece")]),
                phase3_node(
                    "normal",
                    &[("Input", "connectivity 1"), ("Attribute", "N")],
                ),
                phase3_node(
                    "group",
                    &[
                        ("Input", "normal 1"),
                        ("Group Name", "up"),
                        ("Mode", "Attribute"),
                        ("Attribute", "N"),
                        ("Comparison", "Above"),
                        ("Threshold", "0.50"),
                        ("Highlight", "false"),
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
                ("Input", "group 1"),
                ("Group Name", "wider"),
                ("Mode", "Expand"),
                ("Source Group", "up"),
                ("Rings", "2"),
                ("Highlight", "false"),
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
            .find(|p| p.name == "Rings")
            .unwrap()
            .default = "-1".into();
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
            params: [("Attribute", "growth"), ("Scale", "0.50"), ("Direction", "Normal")]
                .into_iter()
                .map(|(name, default)| crate::app::ParamDef {
                    name: name.into(),
                    label: String::new(),
                    param_type: "text".into(),
                    default: default.into(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                    show_when: String::new(), expr: false,
                })
                .collect(),
            geometry_visible: true,
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
                .map(|(name, default)| crate::app::ParamDef {
                    name: (*name).into(),
                    label: String::new(),
                    param_type: "text".into(),
                    default: (*default).into(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                    show_when: String::new(), expr: false,
                })
                .collect(),
            geometry_visible: true,
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
            &[("Thickness", "1.00"), ("Rings", "2"), ("Iterations", "6")],
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

    #[test]
    fn test_detangle_is_independent_of_point_order() {
        // Gathered against the start-of-pass positions and applied at the end,
        // so the same tangle untangles the same way however its points are
        // numbered. A Gauss-Seidel sweep would not.
        let mut a = sphere_detail(Vec3::ZERO, 0.5, 6, 8);
        let node = phase3_node("detangle", &[("Thickness", "2.00"), ("Rings", "1"), ("Iterations", "3")]);
        let mut b = a.clone();
        crate::geometry::apply_detangle(&mut a, &node);
        crate::geometry::apply_detangle(&mut b, &node);
        assert_eq!(a.positions(), b.positions());
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
            &[("Distance Threshold", "0.10"), ("Fusion Threshold", "3"), ("Counter", "contact")],
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
        let node = phase3_node("suture", &[("Distance Threshold", "0.10"), ("Counter", "contact")]);
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
                        Ctl::Rgba => {
                            state.settings_write_row(s.label, "#12345678");
                            assert_eq!(state.settings_row_value(s.label), "#12345678",
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
                // The active camera's params exist only once a camera node
                // does; the Default Camera branch is exercised below.
                Owner::ActiveCamera(_) => {}
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
        // Every command — beside the setting rows, and the network pane's
        // zoom slider row when that pane is focused (it is by default).
        assert_eq!(
            state.slots.dialog.rows.iter().filter(|r| !r.id.starts_with(crate::dialog::SETTING_ROW_PREFIX) && r.id != crate::dialog::ZOOM_ROW_ID).count(),
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
            state.slots.dialog.rows.iter().filter(|r| !r.id.starts_with(crate::dialog::SETTING_ROW_PREFIX) && r.id != crate::dialog::ZOOM_ROW_ID).count(),
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
        assert!(matches!(control("Grid Color"), Some(Control::Color { alpha: false, .. })));
        assert!(matches!(control("Grid Thickness"), Some(Control::Slider { dec: 0, .. })), "a spin is a whole-number slider");
        assert!(matches!(control("Geometry Opacity"), Some(Control::Slider { dec: 2, .. })));
        assert!(matches!(control("World Unit"), Some(Control::Choice { .. })));
        assert!(matches!(control("Group Marker Scale"), Some(Control::Slider { .. })));
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
        assert_ne!(state.world_unit, before, "Enter steps a choice too");
        assert!(state.dialog_visible(), "and keeps the dialog up");

        let scale_row = state.slots.dialog.rows.iter().position(|r| r.id == setting_row_id("Group Marker Scale")).unwrap();
        state.slots.dialog.selected = scale_row;
        let before = state.group_marker_scale;
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowRight)));
        assert!(state.group_marker_scale > before, "right arrow grows the markers");
        state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowLeft)));
        assert!((state.group_marker_scale - before).abs() < 1e-5, "left arrow shrinks them back");

        // And it survives an unrelated parameter edit, which is the whole
        // reason the subnets had to be the owner before.
        let mut redraw = false;
        let sphere = state.current_dir().children.iter().position(|c| c.name.starts_with("sphere")).expect("a sphere");
        state
            .apply_action(crate::app::McpAction::SetParam { slot: sphere, name: "Radius".into(), value: "0.8".into() }, &mut redraw)
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
            "Wireframe Color", "Wire Thickness", "Geometry Opacity", "Point Size", "Point Color",
            // main
            "Background Color",
            // camera
            "Camera Pivot Size",
        ] {
            assert!(labels.contains(&label), "'{label}' has no Settings row and no other way in");
        }
        let mut state = State::new(false);
        for id in [
            "toggle_grid", "toggle_origin", "toggle_cube", "toggle_wireframe",
            "toggle_wire_single_color", "toggle_render_points", "toggle_ray_traced_preview",
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
        state.handle_event(&WindowEvent::MouseInput {
            state: ElementState::Pressed,
            button: MouseButton::Left,
        });
        assert!(!cce_ui::widget::context_menu::is_visible(), "the pick closes the menu");
        assert!(state.dialog_visible());
        assert_eq!(state.slots.dialog.mode, Mode::AddNode);
        assert_eq!((state.grid_cursor_col, state.grid_cursor_row), cell);
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
        assert_eq!(state.slots.dialog.rows.len(), state.node_templates.len());
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

    /// The Add Node list offers every template, everywhere.
    ///
    /// It used to hide the geometry ones inside a "utility dir" — the root
    /// meta node and its `main`/`view`/`guides`/`render` subnets, where
    /// placing geometry was refused. Those nodes are gone with the settings
    /// they held, so there is no such directory left to be in and no filter
    /// to apply.
    #[test]
    fn dialog_add_node_hides_geometry_templates_in_a_utility_dir() {
        let mut state = State::new(false);
        state.open_node_palette();
        let at_root = state.slots.dialog.rows.len();
        assert_eq!(at_root, state.node_templates.len(), "the palette dropped templates");
        assert!(state.slots.dialog.rows.iter().any(|r| r.label == "Grid"));
        assert!(state.slots.dialog.rows.iter().any(|r| r.label == "Box"));
        state.close_dialog();

        // Inside a subnet, the same list.
        let sphere = state
            .fs_root
            .children
            .iter()
            .position(|c| c.name.starts_with("sphere"))
            .expect("a sphere at the root");
        state.current_path.push(sphere);
        state.on_path_changed();
        state.open_node_palette();
        assert_eq!(state.slots.dialog.rows.len(), at_root);
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
        ref_node(id, name, "wrangle", vec![("Input", "text", input), ("Class", "choice:Points,Primitives,Detail", class), ("Group", "text", ""), ("Code", "code", code)], vec![])
    }

    /// The input as the wrangle sees it, then the wrangle's own result.
    fn wrangle_over_sphere(code: &str) -> (Detail, Option<Detail>, Option<String>) {
        let src = ref_node("s", "src", "sphere", vec![("Radius", "slider", "1.0")], vec![]);
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
        assert_eq!(crate::wrangle::channel_refs("ch(\"../Radius\") + chs('Name') + search(\"x\") // ch(\"no\")"), vec!["../Radius".to_string(), "Name".to_string()]);
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
        let src = ref_node("s", "src", "sphere", vec![("Radius", "slider", "1.0")], vec![]);
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
        let src = ref_node("s", "src", "sphere", vec![("Radius", "slider", "1.0")], vec![]);
        let w = ref_node("w", "wrangle1", "wrangle", vec![
            ("Input", "text", "src"), ("Class", "choice:Points,Primitives,Detail", "Points"), ("Group", "text", ""),
            ("Amount", "slider", "ch(\"../Lift\") + 1"),
            ("Label", "text", "hello"),
            ("Code", "code", "@P = @P * ch(\"Amount\") + vec3(0, ch(\"../Lift\"), 0);\n@n = chi(\"Amount\");\n@s = chs(\"Label\").len();\n@v = chv(\"../Offset\");"),
        ], vec![]);
        let root = ref_node("root", "root", "node", vec![("Lift", "slider", "2"), ("Offset", "float3", "1:2:3")], vec![src, w]);
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

        let bad = wrangle_node("b", "wrangle2", "src", "Points", "@x = ch(\"Nope\");");
        let root2 = ref_node("root", "root", "node", vec![], vec![root.children[0].clone(), bad]);
        let (_, err) = eval(&root2, &root2.children[1]);
        assert!(err.as_deref().is_some_and(|e| e.contains("Nope")), "a channel to nothing is an error: {err:?}");
    }

    /// The shipped template resolves, and its default script runs.
    #[test]
    fn wrangle_template_ships_and_its_default_code_runs() {
        let templates = crate::app::load_fs_tree();
        let t = templates.children.iter().find(|n| n.node_type == "wrangle").expect("nodes/wrangle.json loads");
        assert_eq!(t.name, "Wrangle");
        let code = t.params.iter().find(|p| p.name == "Code").map(|p| p.default.clone()).unwrap();
        assert!(t.params.iter().all(|p| !p.expr), "no template parameter reads as an expression — least of all the Code");
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
        let src = ref_node("s", "src", "sphere", vec![("Radius", "slider", "1.0")], vec![]);
        let k = ref_node("k", "opencl1", "opencl", vec![("Input", "text", "src"), ("Code", "code", "__kernel void process() {}")], vec![]);
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
}
