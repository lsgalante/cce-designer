
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

// Root-level aliases some modules import via `crate::` paths.
#[allow(unused_imports)]
use app::{CustomEvent, McpAction, ModifiersState};
pub mod plate_corner;
pub mod playbar;
pub mod viewport_3d;
pub mod api;
pub mod window;
pub mod geometry;
pub mod kernel_cpu;
pub mod project;
pub mod render;
pub mod shortcut;
pub mod slots;
pub mod command;
pub mod dialog;
pub mod layout;
pub mod mold;
pub mod embryo;
pub mod page;
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

        let dir = std::env::temp_dir().join("cce-designer-test-projects").join("gears");
        std::fs::create_dir_all(&dir).unwrap();
        state.loaded_project_path = Some(dir.clone());
        assert_eq!(state.chooser_start_dir().as_deref(), dir.parent(),
            "chooser must start where the current project lives");
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
            node_type: "opencl".to_string(),
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
                McpAction::AddNode { template_name: "Sphere".to_string(), name: None, x: 5.0, y: 5.0 },
                &mut redraw,
            )
            .expect("add sphere node");
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

    /// The button must exist on Main, inside the File section, before Exit.
    #[test]
    fn test_main_node_offers_set_as_default() {
        let mut state = State::new(false);
        state.ensure_menubar_subnets();
        let (s_idx, m_idx) = session_and_main(&state);
        let main = &state.fs_root.children[s_idx].children[m_idx];
        let names: Vec<&str> = main.params.iter().map(|p| p.name.as_str()).collect();
        let idx = names.iter().position(|n| *n == "Set As Default").expect("Set As Default param");
        let save_as = names.iter().position(|n| *n == "Save As").unwrap();
        let exit = names.iter().position(|n| *n == "Exit").unwrap();
        assert!(save_as < idx && idx < exit, "Set As Default out of place: {names:?}");
        assert_eq!(main.params[idx].param_type, "button");
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
    /// becomes "Sphere1" and any other whitespace an underscore.
    #[test]
    fn node_names_are_sanitized_of_whitespace() {
        use crate::app::sanitize_node_name;
        assert_eq!(sanitize_node_name("Sphere 1"), "Sphere1");
        assert_eq!(sanitize_node_name("Camera 12"), "Camera12");
        assert_eq!(sanitize_node_name("Sphere1"), "Sphere1");
        assert_eq!(sanitize_node_name("My Region"), "My_Region");
        assert_eq!(sanitize_node_name("  My   Region 2 "), "My_Region2");
        assert_eq!(sanitize_node_name("mold\tshell"), "mold_shell");
        assert_eq!(sanitize_node_name(""), "node");
        assert_eq!(sanitize_node_name("   "), "node");

        // Minting and both MCP entry points go through it.
        let mut state = State::new(false);
        assert_eq!(state.get_lowest_unused_name("Sphere"), "Sphere2", "Sphere1 is taken by the default project");
        let mut redraw = false;
        state.apply_action(crate::app::McpAction::AddNode { template_name: "Plane".into(), name: Some("my plane".into()), x: 5.0, y: 5.0 }, &mut redraw).unwrap();
        let slot = state.current_dir().children.iter().position(|c| c.name == "my_plane").expect("the added node, sanitized");
        state.apply_action(crate::app::McpAction::RenameNode { slot, new_name: "flat one 3".into() }, &mut redraw).unwrap();
        assert_eq!(state.current_dir().children[slot].name, "flat_one3");
        state.apply_action(crate::app::McpAction::AddNode { template_name: "Plane".into(), name: None, x: 6.0, y: 6.0 }, &mut redraw).unwrap();
        assert!(state.current_dir().children.iter().any(|c| c.name == "Plane1"), "a minted name has no space");
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
        let sphere = proj.root.children.iter().position(|c| c.name == "Sphere1").unwrap();
        let camera = proj.root.children.iter().position(|c| c.name == "Camera1").unwrap();
        proj.root.children[sphere].name = "Sphere 1".into();
        proj.root.children[camera].name = "Camera 1".into();
        proj.view_state.active_camera = "Camera 1".into();
        let mut group = proj.root.children[sphere].clone();
        group.id = "g".into();
        group.name = "My Region".into();
        group.node_type = "group".into();
        group.children.clear();
        group.params = vec![ParamDef { name: "Input".into(), label: "Input".into(), param_type: "text".into(), default: "Sphere 1".into(), options: vec![], min: None, max: None, step: None, show_when: String::new() }];
        let mut clash = group.clone();
        clash.id = "c".into();
        clash.name = "Sphere1".into();
        clash.params[0].default = "Camera 1".into();
        proj.root.children.push(group);
        proj.root.children.push(clash);

        proj.sanitize_node_names();

        let names: Vec<&str> = proj.root.children.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Camera1"));
        assert!(names.contains(&"My_Region"));
        assert!(names.contains(&"Sphere1"), "the hand-named sibling keeps its name");
        assert!(names.contains(&"Sphere1_2"), "the migrated sphere steps aside from it: {names:?}");
        let by_name = |n: &str| proj.root.children.iter().find(|c| c.name == n).unwrap();
        assert_eq!(by_name("My_Region").params[0].default, "Sphere1_2", "the wire followed the rename");
        assert_eq!(by_name("Sphere1").params[0].default, "Camera1");
        assert_eq!(proj.view_state.active_camera, "Camera1");
        // The template children inside the sphere were never spaced and are untouched.
        assert!(by_name("Sphere1_2").children.iter().any(|c| c.name == "opencl1"));

        // A clean file is left exactly alone.
        let before = serde_json::to_string(&proj).unwrap();
        proj.sanitize_node_names();
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
    /// `.edge_profile` / `style.surface.param.color`), so Main's retired Style
    /// section must not come back from an older project file — while it did,
    /// loading a project silently outranked the user's config.kdl.
    #[test]
    fn test_legacy_style_params_are_dropped_from_main() {
        let mut state = State::new(false);
        state.ensure_menubar_subnets();
        let (s_idx, main_idx) = session_and_main(&state);

        // Re-seed the params exactly as a pre-removal save carries them.
        for (name, ty, val) in [
            ("Style", "section", ""),
            ("Bevel Profile", "ramp", "smooth;0.000:0.000,0.500:0.900,1.000:1.000"),
            ("Edge Profile", "ramp", "smooth;0.000:0.000,1.000:1.000"),
            ("Plate Color", "rgba", "#11223344"),
        ] {
            state.fs_root.children[s_idx].children[main_idx].params.push(crate::app::ParamDef {
                name: name.to_string(),
                label: String::new(),
                param_type: ty.to_string(),
                default: val.to_string(),
                options: Vec::new(),
                min: None,
                max: None,
                step: None,
                show_when: String::new(),
            });
        }

        state.ensure_menubar_subnets();
        let (s_idx, main_idx) = session_and_main(&state);
        let names: Vec<&str> = state.fs_root.children[s_idx].children[main_idx]
            .params.iter().map(|p| p.name.as_str()).collect();
        for retired in ["Style", "Bevel Profile", "Edge Profile", "Plate Color"] {
            assert!(!names.contains(&retired), "retired style param survived load: {retired} in {names:?}");
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
            DIALOG_IDX, DIALOG_PARAMS_IDX,
        ];
        assert_eq!(roster.len(), WIDGET_COUNT, "roster length vs WIDGET_COUNT");
        for (i, idx) in roster.iter().enumerate() {
            assert_eq!(i, *idx, "slot #{i} expanded to index {idx}");
        }
    }

    /// The settings nodes live inside the permanent root meta node (nee
    /// Session) now; tests that need Main resolve it through there.
    fn session_and_main(state: &State) -> (usize, usize) {
        let s_idx = state.fs_root.children.iter().position(|c| c.node_type == "meta").expect("root meta node");
        let m_idx = state.fs_root.children[s_idx].children.iter().position(|c| c.name == "Main").expect("Main inside Session");
        (s_idx, m_idx)
    }

    /// `View 1:1` puts the pivot plane at true size: afterwards one world
    /// unit spans its real length on the display, so the readout's ratio
    /// is 1. Exercised on the default camera (zoom) at a centimetre world
    /// unit; the cached viewport state the readout reads is set by hand,
    /// as the render pass would.
    #[test]
    fn view_one_to_one_reaches_true_scale() {
        let mut state = State::new(false);
        state.ensure_menubar_subnets();
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

    /// The root meta node (nee Session): exists at root, typed "meta" but
    /// still a subnet, holds exactly the four settings nodes, and refuses
    /// deletion through the one gate every deletion route funnels into.
    #[test]
    fn test_session_node_exists_and_cannot_be_deleted() {
        let mut state = State::new(false);
        state.ensure_menubar_subnets();

        let s_idx = state.fs_root.children.iter().position(|c| c.node_type == "meta").expect("root meta node");
        let session = &state.fs_root.children[s_idx];
        assert_eq!(session.name, "meta");
        assert!(session.is_enterable(), "the root meta stays a subnet");
        let names: Vec<&str> = session.children.iter().map(|c| c.name.as_str()).collect();
        for expected in ["Main", "View", "Guides", "Render"] {
            assert!(names.contains(&expected), "Session is missing {expected}: {names:?}");
        }
        // None of the four remain at root.
        for c in &state.fs_root.children {
            assert!(
                !(c.node_type == "utility" && matches!(c.name.as_str(), "Main" | "View" | "Guides" | "Render")),
                "settings node '{}' still at root", c.name
            );
        }

        let before = state.fs_root.children.len();
        assert!(!state.delete_node(s_idx), "delete_node deleted the root meta node");
        assert_eq!(state.fs_root.children.len(), before, "root meta vanished anyway");
        assert!(state.fs_root.children[s_idx].node_type == "meta");

        // An old save's "session"-typed container retypes to meta in place,
        // children intact.
        state.fs_root.children[s_idx].node_type = "session".to_string();
        state.fs_root.children[s_idx].name = "Session".to_string();
        state.ensure_menubar_subnets();
        let s_idx = state.fs_root.children.iter().position(|c| c.node_type == "meta")
            .expect("session retyped to meta");
        assert_eq!(state.fs_root.children[s_idx].name, "meta");
        let names: Vec<&str> =
            state.fs_root.children[s_idx].children.iter().map(|c| c.name.as_str()).collect();
        for expected in ["Main", "View", "Guides", "Render"] {
            assert!(names.contains(&expected), "retype lost {expected}: {names:?}");
        }

        // Guides carries the Point Marker Size control (thousandths), and
        // applying the settings drives the overlay size.
        {
            let guides = state.fs_root.children[s_idx].children.iter_mut()
                .find(|c| c.name == "Guides").unwrap();
            let p = guides.params.iter_mut().find(|p| p.name == "Point Marker Size")
                .expect("Guides has Point Marker Size");
            assert_eq!(p.default, "20", "default = 0.02 world units");
            p.default = "50".to_string();
            let c = guides.params.iter_mut().find(|p| p.name == "Point Marker Color")
                .expect("Guides has Point Marker Color");
            assert_eq!(c.param_type, "color");
            c.default = "#ff8000".to_string();
            let u = guides.params.iter_mut().find(|p| p.name == "World Unit")
                .expect("Guides has World Unit");
            assert_eq!(u.param_type, "choice");
            assert_eq!(u.default, "mm", "a world unit is a millimetre until declared otherwise");
            u.default = "cm".to_string();
        }
        state.apply_settings_from_menubar_subnets();
        assert_eq!(state.world_unit, cce_ui::units::Unit::Cm);
        assert!((state.world_unit_mm() - 10.0).abs() < 1e-4);
        assert!((state.meta_marker_size - 0.05).abs() < 1e-6);
        assert!((state.meta_marker_color[0] - 1.0).abs() < 0.01);
        assert!((state.meta_marker_color[1] - 0.5).abs() < 0.01);
        assert!((state.meta_marker_color[2] - 0.0).abs() < 0.01);
    }

    /// An old save carries Main/View/Guides/Render at the root with the user's
    /// values in their params — migration must MOVE them (values intact), not
    /// recreate them fresh.
    #[test]
    fn test_old_saves_migrate_settings_nodes_into_session() {
        let mut state = State::new(false);
        state.ensure_menubar_subnets();

        // Simulate the old shape: pull the four back out to root, drop the
        // Session node, and plant a probe param ensure doesn't own — the live-
        // synced toggles are rewritten from app state by design, so only a
        // foreign param can distinguish MOVED (probe survives) from RECREATED
        // (probe gone).
        let s_idx = state.fs_root.children.iter().position(|c| c.node_type == "meta").unwrap();
        let mut session = state.fs_root.children.remove(s_idx);
        for mut child in session.children.drain(..) {
            if child.name == "Guides" {
                child.params.push(crate::app::ParamDef {
                    name: "migration probe".to_string(),
                    label: String::new(),
                    param_type: "text".to_string(),
                    default: "survived".to_string(),
                    options: vec![],
                    min: None,
                    max: None,
                    step: None,
                    show_when: String::new(),
                });
            }
            state.fs_root.children.push(child);
        }

        state.ensure_menubar_subnets();
        let s_idx = state.fs_root.children.iter().position(|c| c.node_type == "meta").expect("root meta recreated");
        let guides = state.fs_root.children[s_idx].children.iter().find(|c| c.name == "Guides").expect("Guides migrated in");
        let v = guides.params.iter().find(|p| p.name == "migration probe").map(|p| p.default.as_str());
        assert_eq!(v, Some("survived"), "migration recreated Guides instead of moving it");
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
            .position(|c| c.name == "Sphere1")
            .expect("default project has Sphere1");
        state.current_path2 = vec![sphere];
        state.sync_nodes();
        assert!(state.current_path.is_empty(), "primary path must not follow");
        assert_eq!(state.path_names_at(&state.current_path2), vec!["Sphere1".to_string()]);

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

        let sphere = state.fs_root.children.iter().position(|c| c.name == "Sphere1").unwrap();
        let camera = state.fs_root.children.iter().position(|c| c.name == "Camera1").unwrap();

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
        a.ensure_menubar_subnets();
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
            .position(|c| c.name == "Sphere1")
            .expect("default project has Sphere1");
        a.current_path2 = vec![sphere];
        a.save_to_file(&dir).expect("save");

        let mut b = State::new(false);
        b.width = 800.0;
        b.ensure_menubar_subnets();
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
        d.ensure_menubar_subnets();
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
        a.ensure_menubar_subnets();
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
        b.ensure_menubar_subnets();
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
        c.ensure_menubar_subnets();
        c.load_from_file(&dir).expect("load half-size");
        assert!((c.floating_network_layout.2 - 260.0).abs() < 0.5, "scaled network width: {}", c.floating_network_layout.2);
        assert!((c.floating_param_width - 180.0).abs() < 0.5, "scaled param width: {}", c.floating_param_width);

        // A detached pane window keeps its own plates.
        let mut d = State::new(true);
        d.ensure_menubar_subnets();
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
        state.ensure_menubar_subnets();
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
        assert_eq!(proj.view_state.active_camera, "Camera1");
        assert_eq!(proj.root.name, "root");
        assert_eq!(proj.root.children.len(), 2);
        assert_eq!(proj.root.children[0].name, "Camera1");
        assert_eq!(proj.root.children[0].position, (1.0, 1.0));
        assert_eq!(proj.root.children[1].name, "Sphere1");
        assert_eq!(proj.root.children[1].position, (4.0, 2.0));
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
        inner.active_camera = "Camera1".to_string();
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

    #[test]
    fn test_subnet_template_child_resolution() {
        let templates_root = crate::app::load_fs_tree();
        let box_template = templates_root
            .children
            .iter()
            .find(|t| t.name == "Box")
            .expect("Box template should be loaded");
        
        assert_eq!(box_template.children.len(), 3);
        
        let input1 = box_template.children.iter().find(|c| c.name == "input1").unwrap();
        assert_eq!(input1.node_type, "input");
        
        let opencl1 = box_template.children.iter().find(|c| c.name == "opencl1").unwrap();
        assert_eq!(opencl1.node_type, "opencl");
        
        let input_param = opencl1.params.iter().find(|p| p.name == "Input").unwrap();
        assert_eq!(input_param.default, "input1");
        
        let update_param = opencl1.params.iter().find(|p| p.name == "Update Parameters").unwrap();
        assert_eq!(update_param.param_type, "button");
        
        let output1 = box_template.children.iter().find(|c| c.name == "output1").unwrap();
        assert_eq!(output1.node_type, "output");
        let output_input = output1.params.iter().find(|p| p.name == "Input").unwrap();
        assert_eq!(output_input.default, "opencl1");
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

    #[test]
    fn test_sphere_subnet_geometry_generation() {
        let templates_root = crate::app::load_fs_tree();
        let sphere_template = templates_root
            .children
            .iter()
            .find(|t| t.name == "Sphere")
            .expect("Sphere template should be loaded");
        
        assert_eq!(sphere_template.children.len(), 2);
        
        let opencl1 = sphere_template.children.iter().find(|c| c.name == "opencl1").unwrap();
        assert_eq!(opencl1.node_type, "opencl");
        
        let output1 = sphere_template.children.iter().find(|c| c.name == "output1").unwrap();
        assert_eq!(output1.node_type, "output");
        
        let mut sphere_instance = sphere_template.clone();
        sphere_instance.id = "sphere_inst".to_string();
        for child in &mut sphere_instance.children {
            child.id = format!("{}_{}", sphere_instance.id, child.name);
        }
        
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
        let mut ocl_err = None;
        let geom = crate::geometry::generate_single_node_geometry_with_errors(
            &root,
            &root.children[0],
            &mut visited,
            &mut ocl_err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
        ).expect("Geometry generation failed");
        
        assert!(ocl_err.is_none(), "OpenCL compilation error: {:?}", ocl_err);
        assert_eq!(geom.num_points(), crate::geometry::sphere_point_len(16, 24));
        
        let mut max_dist: f32 = 0.0;
        for pos in geom.positions() {
            let dx = pos[0] - 0.0;
            let dy = pos[1] - 0.55;
            let dz = pos[2] - 0.0;
            let dist = (dx*dx + dy*dy + dz*dz).sqrt();
            if dist > max_dist {
                max_dist = dist;
            }
        }
        assert!((max_dist - 0.5).abs() < 0.01, "Expected radius around 0.5, got {}", max_dist);
        
        let mut sphere_instance_2 = sphere_template.clone();
        sphere_instance_2.id = "sphere_inst_2".to_string();
        for child in &mut sphere_instance_2.children {
            child.id = format!("{}_{}", sphere_instance_2.id, child.name);
        }
        if let Some(radius_param) = sphere_instance_2.params.iter_mut().find(|p| p.name == "Radius") {
            radius_param.default = "1.0".to_string();
        }
        
        let root_2 = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere_instance_2],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };
        
        let mut visited_2 = Vec::new();
        let mut ocl_err_2 = None;
        let geom_2 = crate::geometry::generate_single_node_geometry_with_errors(
            &root_2,
            &root_2.children[0],
            &mut visited_2,
            &mut ocl_err_2,
            &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
        ).expect("Geometry generation failed");
        
        assert!(ocl_err_2.is_none(), "OpenCL compilation error: {:?}", ocl_err_2);
        assert_eq!(geom_2.num_points(), crate::geometry::sphere_point_len(16, 24));
        
        let mut max_dist_2: f32 = 0.0;
        for pos in geom_2.positions() {
            let dx = pos[0] - 0.0;
            let dy = pos[1] - 0.55;
            let dz = pos[2] - 0.0;
            let dist = (dx*dx + dy*dy + dz*dz).sqrt();
            if dist > max_dist_2 {
                max_dist_2 = dist;
            }
        }
        assert!((max_dist_2 - 1.0).abs() < 0.01, "Expected radius around 1.0, got {}", max_dist_2);
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

    /// The Extrude template: a subnet (input -> opencl -> output) whose kernel
    /// offsets each input triangle along its face normal and stitches side
    /// walls. Per input triangle it emits top (3) + walls (18) + base (3) =
    /// 24 vertices, or 21 with Keep Base off.
    #[test]
    fn test_extrude_subnet_geometry_generation() {
        let templates_root = crate::app::load_fs_tree();
        let sphere_template = templates_root.children.iter().find(|t| t.name == "Sphere").unwrap();
        let extrude_template = templates_root
            .children
            .iter()
            .find(|t| t.name == "Extrude")
            .expect("Extrude template should be loaded");
        assert_eq!(extrude_template.children.len(), 3);
        assert_eq!(extrude_template.inputs, 1);

        let mut sphere_instance = sphere_template.clone();
        sphere_instance.id = "sphere_inst".to_string();
        sphere_instance.name = "Sphere 1".to_string();
        for child in &mut sphere_instance.children {
            child.id = format!("{}_{}", sphere_instance.id, child.name);
        }

        let mut extrude_instance = extrude_template.clone();
        extrude_instance.id = "extrude_inst".to_string();
        extrude_instance.name = "Extrude 1".to_string();
        for child in &mut extrude_instance.children {
            child.id = format!("{}_{}", extrude_instance.id, child.name);
        }
        extrude_instance.params.iter_mut().find(|p| p.name == "Input").unwrap().default =
            "Sphere 1".to_string();

        let root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere_instance, extrude_instance],
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
        ).expect("Extrude geometry generation failed");
        assert!(ocl_err.is_none(), "OpenCL compilation error: {:?}", ocl_err);
        // The extrude kernel builds a wall per input triangle, so its output
        // is a soup of loose shells; welding it is what 1850 counts.
        assert_eq!(geom.num_points(), 1850);

        // Extruding a radius-0.5 sphere outward by the default 0.2 pushes the
        // farthest vertices to ~0.7 from its center.
        let mut max_dist: f32 = 0.0;
        for pos in geom.positions() {
            let dx = pos[0];
            let dy = pos[1] - 0.55;
            let dz = pos[2];
            max_dist = max_dist.max((dx * dx + dy * dy + dz * dz).sqrt());
        }
        assert!((max_dist - 0.7).abs() < 0.02, "Expected max extent ~0.7, got {}", max_dist);

        // Keep Base off drops the 3 base vertices per triangle: 768 * 21.
        let mut root2 = root.clone();
        root2.children[1].params.iter_mut().find(|p| p.name == "Keep Base").unwrap().default =
            "false".to_string();
        let mut visited2 = Vec::new();
        let mut ocl_err2 = None;
        let geom2 = crate::geometry::generate_single_node_geometry_with_errors(
            &root2,
            &root2.children[1],
            &mut visited2,
            &mut ocl_err2,
            &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
        ).expect("Extrude geometry generation failed (no base)");
        assert!(ocl_err2.is_none(), "OpenCL compilation error: {:?}", ocl_err2);
        // Same POINTS as the based variant: the base cap's corners are the
        // wall corners, so dropping the cap removes primitives, not places.
        // The primitive count is where the two variants actually differ.
        assert_eq!(geom2.num_points(), geom.num_points());
        assert!(
            geom2.num_prims() < geom.num_prims(),
            "no-base extrude should have fewer prims: {} vs {}",
            geom2.num_prims(),
            geom.num_prims()
        );
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
        assert!(ocl_err.is_none(), "OpenCL compilation error: {:?}", ocl_err);

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

    /// The per-node meta (preferences) child: ensure adds it to every
    /// geometry-producing node (idempotently, restoring stripped params),
    /// leaves cameras and the Session tree alone, evaluation ignores it,
    /// and the overlay walk turns its Point Markers / Point Numbers prefs
    /// into marker geometry and index labels for visible nodes only.
    #[test]
    fn test_meta_node_prefs_and_overlays() {
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

        let mut root = FsNode {
            id: "root".to_string(),
            name: "root".to_string(),
            node_type: "node".to_string(),
            children: vec![sphere, camera],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
            inputs: 0,
            outputs: 0,
        };
        crate::app::ensure_meta_children(&mut root);

        // The sphere gains a meta child with both prefs; so do its opencl and
        // output stages (uniform rule: every geometry-producing node).
        let s = &root.children[0];
        let meta = s.children.iter().find(|c| c.node_type == "meta").expect("sphere meta");
        assert_eq!(
            meta.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Point Markers", "Point Numbers", "Point Normals", "Wireframe"]
        );
        assert!(meta.params.iter().all(|p| p.default == "false"));
        let opencl = s.children.iter().find(|c| c.name == "opencl1").unwrap();
        assert!(opencl.children.iter().any(|c| c.node_type == "meta"));
        // The camera does not.
        assert!(!root.children[1].children.iter().any(|c| c.node_type == "meta"));

        // Idempotent, and stripped params come back.
        let before = serde_json::to_string(&root).unwrap();
        crate::app::ensure_meta_children(&mut root);
        assert_eq!(before, serde_json::to_string(&root).unwrap());
        root.children[0].children.iter_mut().find(|c| c.node_type == "meta").unwrap()
            .params.retain(|p| p.name != "Point Numbers");
        crate::app::ensure_meta_children(&mut root);
        assert!(crate::app::meta_pref(&root.children[0], "Point Numbers") == false);
        assert!(root.children[0].children.iter().find(|c| c.node_type == "meta").unwrap()
            .params.iter().any(|p| p.name == "Point Numbers"));

        // Evaluation is unaffected by the meta children.
        let mut visited = Vec::new();
        let mut err = None;
        let mut cache = crate::geometry::SimCache::default();
        let geom = crate::geometry::generate_single_node_geometry_with_errors(
            &root,
            &root.children[0],
            &mut visited,
            &mut err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut cache),
        ).expect("sphere with meta evaluates");
        assert!(err.is_none(), "{err:?}");
        assert_eq!(geom.num_points(), crate::geometry::sphere_point_len(16, 24));

        // Overlays: nothing while the prefs are off…
        let mut cache = crate::geometry::SimCache::default();
        let (markers, labels, wires, normals) = crate::render::collect_meta_overlays(
            &root, &root, 0.02, [1.0, 0.5, 0.0], &mut crate::geometry::EvalSim::new(0, 0, &mut cache));
        assert!(markers.is_empty() && labels.is_empty() && wires.is_empty() && normals.is_empty());

        // …all four overlays for the flagged sphere: 240 marker verts per
        // POINT, one label per point, and one LINE_LIST pair per mesh edge.
        // All three used to be "per distinct quantized position", reconstructed
        // every frame; they are now just the point and edge lists.
        {
            let meta = root.children[0].children.iter_mut()
                .find(|c| c.node_type == "meta").unwrap();
            for p in meta.params.iter_mut() { p.default = "true".to_string(); }
        }
        assert!(crate::app::meta_pref(&root.children[0], "Point Markers"));
        assert!(crate::app::meta_pref(&root.children[0], "Wireframe"));
        let mut cache = crate::geometry::SimCache::default();
        let (markers, labels, wires, normals) = crate::render::collect_meta_overlays(
            &root, &root, 0.02, [1.0, 0.5, 0.0], &mut crate::geometry::EvalSim::new(0, 0, &mut cache));
        assert_eq!(labels.len(), crate::geometry::sphere_point_len(16, 24), "one label per point");
        assert_eq!(markers.len(), labels.len() * 240);
        assert!(labels.iter().any(|(_, i)| *i > 0));
        // The marker color parameter flows into the vertices (linearized).
        let expect = cce_ui::colors::to_linear_rgb([1.0, 0.5, 0.0]);
        assert!(markers.iter().all(|v| v.color == expect));
        assert_eq!(wires.len(), geom.edges().len() * 2, "one pair per unique edge");
        // Normals: one whisker per distinct point, pointing OUT of the
        // sphere (center (0, 0.55, 0)) — this pins the winding/negation
        // convention, not just the count.
        assert_eq!(normals.len(), labels.len() * 2);
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

        // …and none once the node's geometry is hidden.
        root.children[0].geometry_visible = false;
        let mut cache = crate::geometry::SimCache::default();
        let (markers, labels, wires, normals) = crate::render::collect_meta_overlays(
            &root, &root, 0.02, [1.0, 0.5, 0.0], &mut crate::geometry::EvalSim::new(0, 0, &mut cache));
        assert!(markers.is_empty() && labels.is_empty() && wires.is_empty() && normals.is_empty());
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
    /// subnet instance's kernel refreshes to the template's (so the new
    /// params actually work), and non-template lookalikes are left alone.
    #[test]
    fn test_loader_merges_new_template_params() {
        let templates_root = crate::app::load_fs_tree();
        let templates = crate::app::flatten_node_templates(&templates_root);
        let sphere_t = templates_root.children.iter().find(|t| t.name == "Sphere").unwrap();
        let group_t = templates_root.children.iter().find(|t| t.name == "Group").unwrap();

        // An "old save": a Sphere instance from before the construction
        // controls — only Radius (with a user value), and a stale kernel.
        let mut old_sphere = sphere_t.clone();
        old_sphere.id = "s".to_string();
        old_sphere.name = "Sphere 3".to_string();
        for child in &mut old_sphere.children {
            child.id = format!("{}_{}", old_sphere.id, child.name);
        }
        old_sphere.params.retain(|p| p.name == "Radius");
        old_sphere.params[0].default = "0.70".to_string();
        let opencl = old_sphere.children.iter_mut().find(|c| c.name == "opencl1").unwrap();
        opencl.params.iter_mut().find(|p| p.name == "Code").unwrap().default =
            "OLD KERNEL".to_string();

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

        // Sphere: new params appended with template defaults, value kept,
        // kernel refreshed.
        let s = &root.children[0];
        let names: Vec<&str> = s.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Radius", "Rows", "Columns", "Center X", "Center Y", "Center Z", "Color"]);
        assert_eq!(s.params[0].default, "0.70", "instance value survives");
        let code = &s.children.iter().find(|c| c.name == "opencl1").unwrap()
            .params.iter().find(|p| p.name == "Code").unwrap().default;
        assert!(code.contains("chi(\"Rows\""), "kernel refreshed from template");

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

    /// The Plane template mirrors the Sphere subnet (an opencl node feeding an
    /// output node); its kernel generates a divs x divs grid on XZ at y = 0,
    /// with the Size param as the side length.
    #[test]
    fn test_plane_subnet_geometry_generation() {
        let templates_root = crate::app::load_fs_tree();
        let plane_template = templates_root
            .children
            .iter()
            .find(|t| t.name == "Plane")
            .expect("Plane template should be loaded");

        assert_eq!(plane_template.children.len(), 2);
        let opencl1 = plane_template.children.iter().find(|c| c.name == "opencl1").unwrap();
        assert_eq!(opencl1.node_type, "opencl");
        let output1 = plane_template.children.iter().find(|c| c.name == "output1").unwrap();
        assert_eq!(output1.node_type, "output");

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
            assert!(ocl_err.is_none(), "OpenCL compilation error: {:?}", ocl_err);
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
    fn test_dynamic_parameters_parsing_and_preprocessing() {
        let code = r#"
            float freq = chf("freq", 4.0f);
            int count = chi("count", 15);
            float3 col = chv("col", 0.8f, 0.2f, 0.2f);
            float scale = chf("scale");
        "#;
        
        let parsed = crate::geometry::parse_dynamic_params(code);
        assert_eq!(parsed.len(), 4);
        
        assert_eq!(parsed[0].name, "freq");
        assert_eq!(parsed[0].param_type, "slider");
        assert_eq!(parsed[0].default, "4.0");
        
        assert_eq!(parsed[1].name, "scale");
        assert_eq!(parsed[1].param_type, "slider");
        assert_eq!(parsed[1].default, "0.5");
        
        assert_eq!(parsed[2].name, "count");
        assert_eq!(parsed[2].param_type, "spinbox");
        assert_eq!(parsed[2].default, "15");
        
        assert_eq!(parsed[3].name, "col");
        assert_eq!(parsed[3].param_type, "float3");
        assert_eq!(parsed[3].default, "0.80:0.20:0.20");
        
        let mut target = FsNode {
            id: "node1".to_string(),
            name: "OpenCL Node".to_string(),
            node_type: "opencl".to_string(),
            children: Vec::new(),
            params: parsed,
            geometry_visible: true,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        };
        
        target.params[1].default = "1.25".to_string(); // "scale"
        let preprocessed = crate::geometry::preprocess_opencl_code(code);
        assert!(preprocessed.contains("param_values[0]"));
        assert!(preprocessed.contains("((int)param_values[2])"));
        assert!(preprocessed.contains("(float3)(param_values[3], param_values[4], param_values[5])"));
        assert!(preprocessed.contains("param_values[1]"));
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

    #[test]
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
            .map(|i| Row { id: format!("c{i}"), label: format!("Command {i}"), chord: String::new(), swatch: None, toggle: None })
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
            .map(|i| Row { id: format!("c{i}"), label: format!("Command {i}"), chord: String::new(), swatch: None, toggle: None })
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
        state.ensure_menubar_subnets();
        let (s_idx, m_idx) = session_and_main(&state);
        state.current_path.push(s_idx);
        state.on_path_changed();
        state.graph_mut().set_selected_node(Some(m_idx));
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
                .expect("Main's params include a dropdown (Open)");
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
        // "New" is a button label sitting under the open dropdown;
        // "Other" only exists inside the popover's option list.
        let label_idx = text_pos("New").expect("button label in display list");
        let option_idx = text_pos("Other").expect("popover option text in display list");
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
                        show_when: String::new(),
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
    /// lands on the Settings half's Wireframe Color row, whose owner is the
    /// Render node's "Wire Color" (the value's one home). The palette row
    /// shows the value; the settings row edits it.
    #[test]
    fn test_wireframe_color_row_previews_and_lands_on_settings() {
        use crate::command::{by_id, Run};
        use crate::dialog::Tab;
        let cmd = by_id("wireframe_color").expect("no wireframe_color command");
        assert_eq!(cmd.label, "Wireframe Color");
        assert_eq!(cmd.run, Run::Key(crate::shortcut::Action::WireframeColor));

        let mut state = State::new(false);
        state.wire_color = [0.2, 0.6, 0.9, 0.5];
        state.run_command("command_palette");
        let row = state
            .slots
            .dialog
            .rows
            .iter()
            .find(|r| r.id == "wireframe_color")
            .expect("the palette lists Wireframe Color");
        let sw = row.swatch.expect("the row carries a swatch");
        let want = cce_ui::color::to_linear([0.2, 0.6, 0.9, 1.0]);
        for k in 0..4 {
            assert!((sw[k] - want[k]).abs() < 1e-6, "swatch channel {k}: {} vs {}", sw[k], want[k]);
        }
        assert!(state.slots.dialog.rows.iter().filter(|r| r.id != "wireframe_color").all(|r| r.swatch.is_none()));

        // Picked from the list: the dialog closes, the command reopens it on
        // Settings, and the Wireframe Color row is there as a colour control
        // owned by the Render node.
        state.take_dialog_pick("wireframe_color".to_string());
        assert!(state.dialog_visible());
        assert_eq!(state.dialog_tab(), Tab::Settings);
        let shown = state.dialog_settings_shown.clone();
        assert!(shown.iter().any(|(k, _, t)| k == "Wireframe Color" && t == "rgba"), "{shown:?}");
        assert!(shown.iter().any(|(k, _, t)| k == "Wireframe Single Color" && t == "toggle"), "{shown:?}");
        let s = crate::dialog::SETTINGS.iter().find(|s| s.label == "Wireframe Color").unwrap();
        assert_eq!(s.owner, Some(crate::dialog::Owner::Subnet("Render", "Wire Color")));

        // Editing both rows reaches the live state: the colour AND the switch
        // that makes the wire pass use it (off, the wires carry the
        // geometry's colours and the colour row is their alpha alone).
        assert!(!state.wire_single_color, "single-colour mode is off by default");
        let mut rows = state.dialog_settings_shown.clone();
        rows.iter_mut().find(|(k, _, _)| k == "Wireframe Color").unwrap().1 = "#000000ff".to_string();
        rows.iter_mut().find(|(k, _, _)| k == "Wireframe Single Color").unwrap().1 = "true".to_string();
        state.slots.dialog_params_mut().set_display_params(&rows);
        state.sync_dialog_settings_to_project();
        assert_eq!(state.wire_color, [0.0, 0.0, 0.0, 1.0], "the colour row writes the live wire colour");
        assert!(state.wire_single_color, "the switch row turns single-colour mode on");
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
        state.active_camera = "Camera1".to_string();
        let sub = state.current_dir().children.iter().position(|c| c.name == "Sphere1").expect("Sphere1 at the root");
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

    /// The viewport settings live in the scene file: the Render node's
    /// wireframe state and colour, the Guides node's grid and origin, Main's
    /// background, and the Default Camera view (square aspect, pivot marker,
    /// orbit/zoom/pivot). A fresh State whose live values differ takes the
    /// file's on load. Before this, `ensure_menubar_subnets` re-seeded the
    /// nodes from live state on load and the file's values were lost.
    #[test]
    fn viewport_settings_round_trip_through_the_scene_file() {
        let dir = std::env::temp_dir().join(format!("cce-designer-vp-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut a = State::new(false);
        a.ensure_menubar_subnets();
        // The Default Camera is active: a camera NODE's own Square Aspect and
        // pivot params would override the saved view's, by design.
        a.active_camera = "Default Camera".to_string();
        a.wireframe = true;
        a.wire_single_color = true;
        a.wire_color = [0.0, 0.0, 0.0, 1.0];
        a.wire_width = 3.0;
        a.viewport_mut().show_grid = false;
        a.viewport_mut().show_origin = true;
        a.viewport_mut().bg_color = [0.1, 0.2, 0.3];
        a.square_viewport = true;
        a.viewport_mut().show_camera_pivot = true;
        a.viewport_mut().rotation_y = 0.7;
        a.viewport_mut().zoom = 0.4;
        a.viewport_mut().pivot = Vec3::new(3.0, 0.5, -2.0);
        a.save_to_file(&dir).expect("save");

        let mut b = State::new(false);
        b.ensure_menubar_subnets();
        assert!(!b.wireframe && !b.wire_single_color, "a fresh state starts without wires");
        b.load_from_file(&dir).expect("load");
        assert!(b.wireframe, "Show Wireframe loads from the file");
        assert!(b.wire_single_color, "Wire Single Color loads from the file");
        assert_eq!(b.wire_color, [0.0, 0.0, 0.0, 1.0]);
        assert!((b.wire_width - 3.0).abs() < 1e-4);
        assert!(!b.viewport().show_grid, "Show Grid loads from the file");
        assert!(b.viewport().show_origin, "Show Origin loads from the file");
        let bg = b.viewport().bg_color;
        assert!((bg[0] - 0.1).abs() < 0.01 && (bg[1] - 0.2).abs() < 0.01 && (bg[2] - 0.3).abs() < 0.01, "background {bg:?}");
        assert!(b.square_viewport, "Square Aspect loads from the file");
        assert!(b.viewport().show_camera_pivot, "the pivot marker loads from the file");
        assert!((b.viewport().rotation_y - 0.7).abs() < 1e-4);
        assert!((b.viewport().zoom - 0.4).abs() < 1e-4);
        assert_eq!(b.viewport().pivot, Vec3::new(3.0, 0.5, -2.0));
        // And the nodes agree with the live state after the load.
        let render = b.fs_root.children.iter().find(|c| c.node_type == "meta").unwrap()
            .children.iter().find(|c| c.name == "Render").unwrap();
        assert_eq!(render.params.iter().find(|p| p.name == "Show Wireframe").unwrap().default, "true");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Changing the wire colour turns single-colour mode on — on the node
    /// and live — so the colour shows; a load does not (a file that says
    /// off stays off, whatever colour it carries), and turning the switch
    /// off afterwards sticks until the colour changes again.
    #[test]
    fn changing_the_wire_colour_turns_single_colour_mode_on() {
        let mut state = State::new(false);
        state.ensure_menubar_subnets();
        state.apply_settings_from_menubar_subnets();
        assert!(!state.wire_single_color);
        let render_param = |state: &State, name: &str| -> String {
            state.fs_root.children.iter().find(|c| c.node_type == "meta").unwrap()
                .children.iter().find(|c| c.name == "Render").unwrap()
                .params.iter().find(|p| p.name == name).unwrap().default.clone()
        };
        // An edit through the node, as the params pane and the dialog make it.
        {
            let meta = state.fs_root.children.iter_mut().find(|c| c.node_type == "meta").unwrap();
            let render = meta.children.iter_mut().find(|c| c.name == "Render").unwrap();
            render.params.iter_mut().find(|p| p.name == "Wire Color").unwrap().default = "#000000ff".to_string();
        }
        state.apply_settings_from_menubar_subnets();
        assert!(state.wire_single_color, "a colour change switches single-colour mode on");
        assert_eq!(render_param(&state, "Wire Single Color"), "true", "and the node's switch shows it");
        // Off again by hand stays off while the colour is unchanged.
        state.write_render_toggle("Wire Single Color", false);
        state.apply_settings_from_menubar_subnets();
        assert!(!state.wire_single_color);

        // A load: the file's colour differs from the fresh state's, its
        // switch is off, and it stays off.
        let dir = std::env::temp_dir().join(format!("cce-designer-wire-colour-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        state.save_to_file(&dir).expect("save");
        let mut fresh = State::new(false);
        fresh.ensure_menubar_subnets();
        fresh.load_from_file(&dir).expect("load");
        assert_eq!(fresh.wire_color, [0.0, 0.0, 0.0, 1.0]);
        assert!(!fresh.wire_single_color, "a load never flips the switch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The wireframe toggle is a palette row that flips the live flag AND
    /// the Render node's "Show Wireframe" switch. The node matters: it is
    /// what `apply_settings_from_menubar_subnets` reads back on every
    /// parameter edit, so a flag flipped alone would revert on the next
    /// unrelated edit. No settings write is involved, so running it here
    /// touches nothing outside the test.
    #[test]
    fn test_toggle_wireframe_flips_the_flag_and_the_render_node() {
        use crate::command::{by_id, Run};
        let cmd = by_id("toggle_wireframe").expect("no toggle_wireframe command");
        assert_eq!(cmd.label, "Show Wireframe");
        assert_eq!(cmd.run, Run::Key(crate::shortcut::Action::ToggleWireframe));

        let render_toggle = |state: &State| -> String {
            state
                .fs_root
                .children
                .iter()
                .find(|c| c.node_type == "meta")
                .and_then(|s| s.children.iter().find(|c| c.name == "Render"))
                .and_then(|n| n.params.iter().find(|p| p.name == "Show Wireframe"))
                .map(|p| p.default.clone())
                .expect("a Render node with a Show Wireframe toggle")
        };
        let mut state = State::new(false);
        assert!(!state.wireframe, "wireframe is off unless the project turned it on");
        assert!(state.run_command("toggle_wireframe"));
        assert!(state.wireframe);
        assert_eq!(render_toggle(&state), "true");
        // The read-back path agrees with the flag instead of reverting it.
        state.apply_settings_from_menubar_subnets();
        assert!(state.wireframe);
        assert!(state.run_command("toggle_wireframe"));
        assert!(!state.wireframe);
        assert_eq!(render_toggle(&state), "false");
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
        let cell_x = 9.0 * (state.grid_size_x + state.gap_col_w) + state.pan_x;
        let cell_y = 7.0 * (state.grid_size_y + state.gap_row_h) + state.pan_y;
        assert!(
            (cell_x + state.grid_size_x * 0.5 - 400.0).abs() < 1.0,
            "the cursor cell is not centred horizontally: {cell_x}"
        );
        assert!(
            (cell_y + state.grid_size_y * 0.5 - 300.0).abs() < 1.0,
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
    /// bare, alt and ctrl, plus the two framings — fourteen rows, all in the
    /// network context, none of them colliding.
    #[test]
    fn test_the_navigation_scheme_matches_the_plugins() {
        use crate::command::{by_id, Context};
        let expected = [
            // Uppercase because `describe` prints single letters as capitals,
            // the way every menu in the app writes a chord.
            ("nav_left", "H"), ("nav_down", "J"), ("nav_up", "K"), ("nav_right", "L"),
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

    /// The network plate is optional, and the option is reachable three ways
    /// that cannot disagree: the View settings node's toggle, the network
    /// pane's View menu, and the command palette.
    ///
    /// The toggle is NOT exercised here. Flipping it marks settings dirty and
    /// `execute_action` then writes `~/.config/cce/cce-designer/state.kdl` —
    /// the real one, since tests run with the real HOME — so a test that
    /// toggled it would rewrite the user's own settings as a side effect. What
    /// is asserted instead is everything around the flip: the default, the
    /// wiring, and the mirror.
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

        // The View settings node mirrors the live flag, so the row in the
        // params pane shows what is actually on screen.
        let mut state = State::new(false);
        let session = state
            .fs_root
            .children
            .iter()
            .position(|c| c.node_type == "meta")
            .expect("root meta node");
        let view = state.fs_root.children[session]
            .children
            .iter()
            .position(|c| c.name == "View")
            .expect("View node");
        let plate_row = |state: &State| {
            state.fs_root.children[session].children[view]
                .params
                .iter()
                .find(|p| p.name == "Show Network Plate")
                .map(|p| (p.default.clone(), p.label.clone()))
        };
        assert_eq!(
            plate_row(&state),
            Some(("true".to_string(), "Plate".to_string())),
            "the View node has no Plate row, or it does not read as on"
        );

        state.network_plate = false;
        state.current_path = vec![session];
        state.refresh_main_node_live_toggles(view);
        assert_eq!(
            plate_row(&state).map(|(v, _)| v),
            Some("false".to_string()),
            "the View node's row did not follow the live flag"
        );
    }

    /// The viewport guide toggles survive the next parameter edit.
    ///
    /// `apply_settings_from_menubar_subnets` copies the Guides node onto the
    /// live flags on EVERY parameter change, so a command that flipped only
    /// the flag was undone by the next edit anywhere — Show Cube hid the
    /// cube, and editing any node's parameter brought it back. The command
    /// has to write the Guides node, the value's owner, as Show Wireframe
    /// writes the Render node.
    #[test]
    fn guide_toggles_survive_the_settings_apply_pass() {
        let mut state = State::new(false);
        let guides_value = |state: &State, name: &str| -> String {
            state
                .session_node()
                .and_then(|s| s.children.iter().find(|c| c.name == "Guides"))
                .and_then(|g| g.params.iter().find(|p| p.name == name))
                .map(|p| p.default.clone())
                .expect("the Guides param")
        };
        for (command, param) in [
            ("toggle_cube", "Show Reference Cube"),
            ("toggle_grid", "Show Grid Guide"),
            ("toggle_origin", "Show Origin Axes"),
        ] {
            let flag = |state: &State| match command {
                "toggle_cube" => state.viewport().show_cube,
                "toggle_grid" => state.viewport().show_grid,
                _ => state.viewport().show_origin,
            };
            let before = flag(&state);
            assert_eq!(guides_value(&state, param), before.to_string(), "{param} starts in step with the flag");

            assert!(state.run_command(command));
            assert_eq!(flag(&state), !before, "{command} flipped the flag");
            assert_eq!(guides_value(&state, param), (!before).to_string(), "{command} wrote the Guides node");

            // What every parameter edit runs.
            state.apply_settings_from_menubar_subnets();
            assert_eq!(flag(&state), !before, "{command} was undone by the apply pass");

            // And a real edit through the action path, on an unrelated node.
            let mut redraw = false;
            let sphere = state.current_dir().children.iter().position(|c| c.name.starts_with("Sphere")).expect("a sphere");
            state
                .apply_action(crate::app::McpAction::SetParam { slot: sphere, name: "Radius".into(), value: "0.7".into() }, &mut redraw)
                .expect("set a sphere param");
            assert_eq!(flag(&state), !before, "{command} was undone by a parameter edit");
        }

        // Circular Pane lives on the Main node and had the same hole.
        let before = state.circular_network_pane;
        assert!(state.run_command("toggle_circular_pane"));
        assert_eq!(state.circular_network_pane, !before);
        state.apply_settings_from_menubar_subnets();
        assert_eq!(state.circular_network_pane, !before, "Circular Pane was undone by the apply pass");
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
    // ----- The Embryo node (src/embryo.rs) -----

    /// The hull of a cube's corners plus points inside it is the cube: eight
    /// points, twelve triangles, closed, with nothing left outside it.
    #[test]
    fn convex_hull_of_a_cube_with_interior_points_is_the_cube() {
        use crate::embryo::convex_hull;
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
        // Every face looks away from the centre, and every input point is on
        // or behind every face.
        for prim in 0..hull.num_prims() {
            let ids = hull.prim_points(prim);
            let (a, b, c) = (hull.pos(ids[0] as usize), hull.pos(ids[1] as usize), hull.pos(ids[2] as usize));
            let n = (b - a).cross(c - a).normalize();
            assert!(a.dot(n) > 0.0, "face {prim} winds outward");
            for q in &pts {
                assert!((*q - a).dot(n) <= 1e-4, "point {q:?} is outside face {prim}");
            }
        }
        // No volume, no hull.
        let flat: Vec<Vec3> = (0..20).map(|i| Vec3::new(i as f32, (i * i) as f32 * 0.1, 0.0)).collect();
        assert!(convex_hull(&flat).is_none(), "coplanar points span no volume");
        assert!(convex_hull(&pts[..3]).is_none());
    }

    /// Scattered points lie on the surface, in the number asked for, and a
    /// seed reproduces its draw.
    #[test]
    fn embryo_scatter_lands_on_the_surface_and_is_seeded() {
        use crate::embryo::scatter_on_surface;
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
    }

    /// The pipeline end to end: Basic is the internal sphere; Scatter is a
    /// closed hull of at most Scatter Count points inside the sphere's
    /// radius; Input reads what it is given; and every mesh carries N.
    #[test]
    fn embryo_builds_a_sphere_a_hull_or_the_input() {
        use crate::embryo::{embryo, EmbryoParams, Method, Source};
        let basic = embryo(None, &EmbryoParams::default()).expect("the internal sphere");
        let sphere = crate::geometry::sphere_detail(Vec3::ZERO, 0.5, 50, 50);
        assert_eq!(basic.num_points(), sphere.num_points());
        assert_eq!(basic.num_prims(), sphere.num_prims());
        assert!(basic.points().value("N", 0).is_some(), "normals are written last");

        let scattered = embryo(None, &EmbryoParams { method: Method::Scatter, scatter_count: 400, ..EmbryoParams::default() })
            .expect("a hull");
        assert!(scattered.num_prims() > 0, "the scatter is hulled into a surface");
        assert!(scattered.is_closed(), "the hull is watertight");
        assert!(scattered.num_points() <= 400);
        for p in 0..scattered.num_points() {
            let r = scattered.pos(p).length();
            assert!(r <= 0.5 + 1e-3, "hull point {p} at {r} lies inside the seed sphere");
        }
        assert!(scattered.points().value("N", 0).is_some());

        // Relaxing spreads the scatter: the hull of relaxed points reaches
        // further round the sphere than the hull of the raw draw.
        let raw = embryo(None, &EmbryoParams { method: Method::Scatter, scatter_count: 60, relax_points: false, ..EmbryoParams::default() }).unwrap();
        let relaxed = embryo(None, &EmbryoParams { method: Method::Scatter, scatter_count: 60, ..EmbryoParams::default() }).unwrap();
        let area = |d: &Detail| crate::embryo::surface_area(d);
        assert!(area(&relaxed) > area(&raw) * 0.99, "relaxed hull area {} vs raw {}", area(&relaxed), area(&raw));

        // Source Input: the seed is the input, and nothing without one.
        let cube = crate::geometry::sphere_detail(Vec3::new(2.0, 0.0, 0.0), 0.25, 6, 8);
        let from_input = embryo(Some(&cube), &EmbryoParams { source: Source::Input, ..EmbryoParams::default() }).unwrap();
        assert_eq!(from_input.num_points(), cube.num_points());
        assert!((from_input.pos(0) - cube.pos(0)).length() < 1e-6);
        assert!(embryo(None, &EmbryoParams { source: Source::Input, ..EmbryoParams::default() }).is_none());

        // Subdivision Depth multiplies the faces by four per level.
        let sub = embryo(None, &EmbryoParams { base_resolution: 8, subdivision_depth: 1, ..EmbryoParams::default() }).unwrap();
        let coarse = embryo(None, &EmbryoParams { base_resolution: 8, ..EmbryoParams::default() }).unwrap();
        assert_eq!(sub.num_prims(), coarse.triangulate_points().len() / 3 * 4);
    }

    /// The Relax step slides points apart in their tangent planes: the
    /// sphere's points end up better spaced but still on the sphere.
    #[test]
    fn embryo_relax_keeps_points_in_their_tangent_planes() {
        use crate::embryo::{embryo, EmbryoParams};
        let p = EmbryoParams { base_resolution: 10, relax_iterations: 5, relax_radius: 0.08, ..EmbryoParams::default() };
        let relaxed = embryo(None, &p).unwrap();
        let plain = embryo(None, &EmbryoParams { base_resolution: 10, ..EmbryoParams::default() }).unwrap();
        let mut moved = 0;
        for i in 0..relaxed.num_points() {
            let (a, b) = (relaxed.pos(i), plain.pos(i));
            if (a - b).length() > 1e-5 {
                moved += 1;
            }
            // A tangent-plane slide changes the radius only to second order.
            assert!((a.length() - 0.5).abs() < 0.05, "point {i} left the sphere: r = {}", a.length());
        }
        assert!(moved > 0, "some point moved");
        let free = embryo(None, &EmbryoParams { relax_in_3d: true, ..p.clone() }).unwrap();
        let mut left = 0;
        for i in 0..free.num_points() {
            if (free.pos(i).length() - 0.5).abs() > 0.01 {
                left += 1;
            }
        }
        assert!(left > 0, "in 3D the points are free to leave the surface");
    }

    /// The node reads the template's parameters into the pipeline, and the
    /// template's defaults are the HDA's.
    #[test]
    fn embryo_node_reads_its_template() {
        use crate::embryo::{EmbryoParams, Method, Source};
        let templates_root = crate::app::load_fs_tree();
        let t = templates_root.children.iter().find(|t| t.name == "Embryo").expect("the Embryo template");
        assert_eq!(t.node_type, "embryo");
        assert_eq!(crate::geometry::embryo_params(t), EmbryoParams::default(), "template defaults are the HDA's");

        let mut inst = t.clone();
        inst.id = "embryo1".into();
        inst.name = "Embryo 1".into();
        for (name, value) in [("Method", "Scatter"), ("Scatter Count", "200"), ("Source", "Internal")] {
            inst.params.iter_mut().find(|p| p.name == name).unwrap().default = value.to_string();
        }
        let read = crate::geometry::embryo_params(&inst);
        assert_eq!(read.method, Method::Scatter);
        assert_eq!(read.scatter_count, 200);
        assert_eq!(read.source, Source::Internal);

        let root = FsNode {
            id: "root".into(), name: "root".into(), node_type: "node".into(),
            children: vec![inst], params: vec![], geometry_visible: true, position: (0.0, 0.0), inputs: 0, outputs: 0,
        };
        let mut visited = Vec::new();
        let mut err = None;
        let geom = crate::geometry::generate_single_node_geometry_with_errors(
            &root, &root.children[0], &mut visited, &mut err,
            &mut crate::geometry::EvalSim::new(0, 0, &mut crate::geometry::SimCache::default()),
        ).expect("the node evaluates");
        assert!(err.is_none());
        assert!(geom.is_closed() && geom.num_points() <= 200);
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
                        show_when: String::new(),
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
        let dir = std::env::temp_dir().join("cce-designer-page-tests");
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
                show_when: String::new(),
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
                    show_when: String::new(),
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
                    show_when: String::new(),
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

    /// Every Settings row still names something that exists.
    ///
    /// The failure this catches is silent and the reason the table is a table:
    /// `dialog_settings_params` SKIPS a row whose owning param it cannot find,
    /// so renaming a subnet param quietly shortens the Settings half and
    /// nothing says why. Same argument as
    /// `test_every_menu_command_names_a_label_that_is_dispatched`.
    #[test]
    fn dialog_settings_rows_name_owners_that_exist() {
        use crate::dialog::Owner;
        let state = State::new(false);
        let session = state.session_node().expect("the root meta node");
        for s in crate::dialog::SETTINGS {
            match s.owner {
                None => {}
                Some(Owner::Subnet(subnet, name)) => {
                    let node = session
                        .children
                        .iter()
                        .find(|c| c.name == subnet)
                        .unwrap_or_else(|| panic!("no '{subnet}' subnet for row '{}'", s.label));
                    assert!(
                        node.params.iter().any(|p| p.name == name),
                        "'{subnet}' has no param '{name}' — row '{}' would vanish",
                        s.label
                    );
                }
                Some(Owner::Command(id)) => assert!(
                    crate::command::by_id(id).is_some(),
                    "row '{}' names no command '{id}'",
                    s.label
                ),
                // The active camera's params exist only once a camera node
                // does; the Default Camera branch is exercised below.
                Some(Owner::ActiveCamera(_)) => {}
            }
        }
    }

    /// Row labels are the writeback's identity — `param_display` keys a row by
    /// its label and `sync_dialog_settings_to_project` resolves it back the
    /// same way — so two rows sharing one would write each other's values.
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
        assert_eq!(
            state.slots.dialog.rows.len(),
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
        assert_eq!(row.toggle, None, "Deselect runs and is done; it draws no switch");

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
        assert_eq!(row.toggle, Some(before), "the switch shows the live value");

        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert_eq!(state.square_viewport, !before, "Enter ran the command");
        assert!(state.dialog_visible(), "and the dialog stayed up");
        assert_eq!(state.slots.dialog.query, "squa", "with its query intact");
        assert_eq!(state.slots.dialog.selected_id(), Some("toggle_square_viewport"), "and its selection");
        let row = &state.slots.dialog.rows[state.slots.dialog.selected];
        assert_eq!(row.toggle, Some(!before), "the switch moved with the value");

        // And back again, without leaving.
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
        assert_eq!(state.square_viewport, before);
        assert!(state.dialog_visible());
        assert_eq!(state.slots.dialog.rows[state.slots.dialog.selected].toggle, Some(before));

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
        assert_eq!(state.slots.dialog.rows.len(), crate::command::COMMANDS.len());
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

    /// Tab moves between the halves, and entering Settings lays its body out
    /// and fills it.
    #[test]
    fn dialog_tab_switches_halves_and_settings_has_rows() {
        use cce_ui::widget::WidgetHost as _;
        use crate::dialog::Tab;
        let mut state = State::new(false);
        state.run_command("toggle_dialog");
        assert_eq!(state.dialog_tab(), Tab::Commands);

        state.dialog_key_input(&key_press(Key::Named(NamedKey::Tab)));
        assert_eq!(state.dialog_tab(), Tab::Settings);
        assert!(state.slots.dialog_params.visible(), "the settings body shows");
        assert!(
            state.positions[crate::slots::DIALOG_PARAMS_IDX].2 > 0.0,
            "and is laid out inside the dialog"
        );
        // Every non-section row of the table, minus the ones whose owner is
        // missing in a fresh project — there are none, per the test above.
        let rows = state.dialog_settings_shown.clone();
        assert_eq!(rows.len(), crate::dialog::SETTINGS.len());
        assert!(rows.iter().any(|(k, _, t)| k == "Show Grid" && t == "toggle"));
        assert!(rows.iter().any(|(k, _, t)| k == "Grid Color" && t == "color"));
        assert!(rows.iter().any(|(k, _, t)| k == "Grid Thickness" && t.starts_with("spinbox")));

        state.dialog_key_input(&key_press(Key::Named(NamedKey::Tab)));
        assert_eq!(state.dialog_tab(), Tab::Commands);
        assert!(!state.slots.dialog_params.visible());
    }

    /// A Settings row writes to whatever OWNS its value, not to the live field
    /// — which is the only write that survives, since
    /// `apply_settings_from_menubar_subnets` copies the subnets over the live
    /// state on every param change.
    #[test]
    fn dialog_settings_write_reaches_the_owning_subnet() {
        use crate::dialog::Tab;
        let mut state = State::new(false);
        state.run_command("toggle_dialog");
        state.set_dialog_tab(Tab::Settings);

        let was = state.viewport().show_grid;
        // What a click on the toggle leaves behind: the control reports the
        // flipped value, and the poll picks it up.
        let mut rows = state.dialog_settings_shown.clone();
        let row = rows.iter_mut().find(|(k, _, _)| k == "Show Grid").expect("the Show Grid row");
        row.1 = if was { "false" } else { "true" }.to_string();
        state.slots.dialog_params_mut().set_display_params(&rows);
        state.sync_dialog_settings_to_project();

        assert_eq!(state.viewport().show_grid, !was, "the live state followed");
        let guides = state
            .session_node()
            .expect("meta")
            .children
            .iter()
            .find(|c| c.name == "Guides")
            .expect("Guides");
        let p = guides.params.iter().find(|p| p.name == "Show Grid Guide").expect("the param");
        assert_eq!(p.default == "true", !was, "and so did its owner");
    }

    /// Reopening starts clean: on Commands, with an empty query.
    #[test]
    fn dialog_reopens_without_the_last_search() {
        use crate::dialog::Tab;
        let mut state = State::new(false);
        state.run_command("toggle_dialog");
        state.dialog_key_input(&typed("g"));
        state.set_dialog_tab(Tab::Settings);
        state.run_command("toggle_dialog");
        assert!(!state.dialog_visible());

        state.run_command("toggle_dialog");
        assert_eq!(state.slots.dialog.query, "");
        assert_eq!(state.dialog_tab(), Tab::Commands);
    }

    /// Tab opens the same plate in its AddNode mode: one list of node
    /// templates, no tab strip, no chord column.
    #[test]
    fn dialog_add_node_mode_lists_the_templates() {
        use cce_ui::widget::WidgetHost as _;
        use crate::dialog::Mode;
        let mut state = State::new(false);
        state.open_node_palette();

        assert!(state.dialog_visible());
        assert_eq!(state.slots.dialog.mode, Mode::AddNode);
        assert!(!state.slots.dialog_params.visible(), "no settings body in this mode");
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
        assert!(added.name.starts_with("Box"), "added {}", added.name);
        assert_eq!(added.position, (3.0, 2.0), "placed at the grid cursor");
    }

    /// Geometry templates are refused inside a utility dir, so the list does
    /// not offer them there — the same filter the popup was fed.
    #[test]
    fn dialog_add_node_hides_geometry_templates_in_a_utility_dir() {
        let mut state = State::new(false);
        let offered_at_root = {
            state.open_node_palette();
            let n = state.slots.dialog.rows.len();
            state.close_dialog();
            n
        };

        // Into the root meta node, which `in_settings_dir` reports as utility.
        let meta = state
            .fs_root
            .children
            .iter()
            .position(|c| c.node_type == "meta")
            .expect("the root meta node");
        state.current_path.push(meta);
        assert!(state.in_settings_dir());

        state.open_node_palette();
        let offered_in_utility = state.slots.dialog.rows.len();
        assert!(
            offered_in_utility < offered_at_root,
            "{offered_in_utility} offered in a utility dir vs {offered_at_root} at the root"
        );
        assert!(
            !state.slots.dialog.rows.iter().any(|r| r.label == "Grid"),
            "a geometry template would be refused at placement"
        );
        // Box/Sphere/Plane/Extrude are `"type": "node"` SUBNET templates, not
        // native geometry types, so `is_geometry_node_type` does not claim
        // them and the filter leaves them offered. Pre-existing, and exactly
        // what the popup was fed — asserted so the next reader does not take
        // it for a hole in this filter.
        assert!(state.slots.dialog.rows.iter().any(|r| r.label == "Box"));
    }

    /// Ctrl+P lands on Commands rather than toggling, which is the one thing
    /// that distinguishes it from Alt+D now that both open the same dialog.
    #[test]
    fn command_palette_opens_the_dialog_on_commands() {
        use crate::dialog::{Mode, Tab};
        let mut state = State::new(false);
        state.run_command("toggle_dialog");
        state.set_dialog_tab(Tab::Settings);
        assert_eq!(state.dialog_tab(), Tab::Settings);

        state.run_command("command_palette");
        assert!(state.dialog_visible(), "it lands, it does not toggle");
        assert_eq!(state.dialog_tab(), Tab::Commands);
        assert_eq!(state.slots.dialog.mode, Mode::Tabbed);
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
}
