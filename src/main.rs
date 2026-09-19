
pub mod app;
pub mod application;
pub mod curve_tool;
pub mod detail;
pub mod remesh;
pub mod spatial;

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
    use crate::geometry::{GAttribute, GVertex, Geometry, line_vertices};
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
            NETWORK_PANEL2_IDX, CONTENT2_IDX, BREADCRUMB2_IDX,
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
        m.register("Ctrl+s", Action::Save).unwrap();
        m.register("Ctrl+Shift+s", Action::SaveAs).unwrap();
        let ctrl = crate::app::ModifiersState { ctrl: true, ..Default::default() };
        let ctrl_shift = crate::app::ModifiersState { ctrl: true, shift: true, ..Default::default() };
        // The REAL event shapes: xkb delivers the shifted character when
        // Shift is held — "S", not "s". The first version of this test fed
        // lowercase for both and passed against a matcher that could never
        // fire in practice.
        let lower = cce_ui::widget::Key::Character("s".into());
        let upper = cce_ui::widget::Key::Character("S".into());
        assert_eq!(m.match_action(&ctrl, &lower), Some(Action::Save));
        assert_eq!(m.match_action(&ctrl_shift, &upper), Some(Action::SaveAs));
        assert_eq!(m.match_action(&ctrl_shift, &lower), Some(Action::SaveAs));
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
            .position(|c| c.name == "Sphere 1")
            .expect("default project has Sphere 1");
        state.current_path2 = vec![sphere];
        state.sync_nodes();
        assert!(state.current_path.is_empty(), "primary path must not follow");
        assert_eq!(state.path_names_at(&state.current_path2), vec!["Sphere 1".to_string()]);

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

        let sphere = state.fs_root.children.iter().position(|c| c.name == "Sphere 1").unwrap();
        let camera = state.fs_root.children.iter().position(|c| c.name == "Camera 1").unwrap();

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
            .position(|c| c.name == "Sphere 1")
            .expect("default project has Sphere 1");
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
        m.register("Up", Action::PlayPause).unwrap();
        m.register("Right", Action::FrameNext).unwrap();
        m.register("Left", Action::FramePrev).unwrap();
        let plain = crate::app::ModifiersState::default();
        let ctrl = crate::app::ModifiersState { ctrl: true, ..Default::default() };
        assert_eq!(m.match_action(&plain, &Key::Named(NamedKey::ArrowUp)), Some(Action::PlayPause));
        assert_eq!(m.match_action(&plain, &Key::Named(NamedKey::ArrowRight)), Some(Action::FrameNext));
        assert_eq!(m.match_action(&plain, &Key::Named(NamedKey::ArrowLeft)), Some(Action::FramePrev));
        assert_eq!(m.match_action(&ctrl, &Key::Named(NamedKey::ArrowUp)), None);
        m.register("Down", Action::PlayPauseReverse).unwrap();
        assert_eq!(m.match_action(&plain, &Key::Named(NamedKey::ArrowDown)), Some(Action::PlayPauseReverse));
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
        assert_eq!(proj.view_state.active_camera, "Camera 1");
        assert_eq!(proj.root.name, "root");
        assert_eq!(proj.root.children.len(), 2);
        assert_eq!(proj.root.children[0].name, "Camera 1");
        assert_eq!(proj.root.children[0].position, (1.0, 1.0));
        assert_eq!(proj.root.children[1].name, "Sphere 1");
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
        inner.active_camera = "Camera 1".to_string();
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

        state.toggle_curve_tool(slot);
        assert!(state.curve_tool.is_some());

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
        assert!(state.curve_tool_press(), "press on a handle must grab");
        state.cursor_x = 50.0;
        state.cursor_y = 50.0;
        assert!(state.curve_tool_drag_motion());
        assert!(state.curve_tool_release());
        let pts = points_of(&state, slot);
        assert!(pts[0].length() < 1e-4, "dragged point should sit at the origin, got {:?}", pts[0]);
        // The other curve is untouched.
        assert_eq!(points_of(&state, slot2)[0], default_first);

        // Press on empty space appends a point there (at the last point's
        // depth — z=0 here) and immediately drags it.
        state.cursor_x = 90.0;
        state.cursor_y = 90.0;
        assert!(state.curve_tool_press());
        let pts = points_of(&state, slot);
        assert_eq!(pts.len(), 5);
        assert!((pts[4] - Vec3::new(0.8, -0.8, 0.0)).length() < 1e-4, "added at {:?}", pts[4]);
        assert!(state.curve_tool_release());

        // Delete the (selected) new point, then right-press-delete the one
        // parked at the pane center.
        assert!(state.curve_tool_delete_selected());
        assert_eq!(points_of(&state, slot).len(), 4);
        state.cursor_x = 50.0;
        state.cursor_y = 50.0;
        assert!(state.curve_tool_delete_at_cursor());
        assert_eq!(points_of(&state, slot).len(), 3);

        // Toggling on the same node exits the state.
        state.toggle_curve_tool(slot);
        assert!(state.curve_tool.is_none());
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
        state.toggle_curve_tool(slot);
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
            let t = state.curve_tool.as_ref().expect("tool active");
            (t.history.undo_len(), t.history.redo_len())
        };

        let initial = points_of(&state);
        assert!(!state.curve_tool_undo(), "nothing to undo yet");
        assert!(!state.curve_tool_redo(), "nothing to redo yet");

        // Grab and release without moving: no history.
        state.cursor_x = sx(initial[0].x);
        state.cursor_y = sy(initial[0].y);
        assert!(state.curve_tool_press());
        assert!(state.curve_tool_release());
        assert_eq!(history(&state), (0, 0));

        // Gesture 1: drag the first point to the pane center, over several
        // motion events — still one entry.
        assert!(state.curve_tool_press());
        for (x, y) in [(55.0, 55.0), (52.0, 52.0), (50.0, 50.0)] {
            state.cursor_x = x;
            state.cursor_y = y;
            assert!(state.curve_tool_drag_motion());
        }
        assert!(state.curve_tool_release());
        let after_drag = points_of(&state);
        assert!(after_drag[0].length() < 1e-4);
        assert_eq!(history(&state), (1, 0));

        // Gesture 2: add a point (press on empty space + drag + release).
        state.cursor_x = 90.0;
        state.cursor_y = 90.0;
        assert!(state.curve_tool_press());
        state.cursor_x = 85.0;
        state.cursor_y = 85.0;
        assert!(state.curve_tool_drag_motion());
        assert!(state.curve_tool_release());
        let after_add = points_of(&state);
        assert_eq!(after_add.len(), initial.len() + 1);
        assert_eq!(history(&state), (2, 0));

        // Gesture 3: delete the selected (new) point.
        assert!(state.curve_tool_delete_selected());
        let after_delete = points_of(&state);
        assert_eq!(after_delete.len(), initial.len());
        assert_eq!(history(&state), (3, 0));

        // Undo walks back through all three.
        assert!(state.curve_tool_undo());
        assert_eq!(points_of(&state), after_add);
        assert!(state.curve_tool_undo());
        assert_eq!(points_of(&state), after_drag);
        assert!(state.curve_tool_undo());
        assert_eq!(points_of(&state), initial);
        assert_eq!(history(&state), (0, 3));
        assert!(!state.curve_tool_undo(), "history exhausted");

        // Redo walks forward again.
        assert!(state.curve_tool_redo());
        assert_eq!(points_of(&state), after_drag);
        assert!(state.curve_tool_redo());
        assert_eq!(points_of(&state), after_add);
        assert_eq!(history(&state), (2, 1));

        // A new gesture after an undo forks: the redo branch is gone.
        state.cursor_x = 50.0;
        state.cursor_y = 50.0;
        assert!(state.curve_tool_delete_at_cursor());
        assert_eq!(history(&state), (3, 0));
        assert!(!state.curve_tool_redo());

        // Undo mid-drag abandons the drag and clamps the selection.
        let pts = points_of(&state);
        state.cursor_x = sx(pts[pts.len() - 1].x);
        state.cursor_y = sy(pts[pts.len() - 1].y);
        assert!(state.curve_tool_press());
        state.cursor_x += 5.0;
        assert!(state.curve_tool_drag_motion());
        assert!(state.curve_tool_undo());
        let tool = state.curve_tool.as_ref().unwrap();
        assert!(tool.drag.is_none());
        assert!(tool.selected.map(|i| i < points_of(&state).len()).unwrap_or(true));
        assert!(!state.curve_tool_drag_motion(), "no drag survives an undo");

        // Leaving the state drops its history.
        state.toggle_curve_tool(slot);
        assert!(state.curve_tool.is_none());
        assert!(!state.curve_tool_undo());
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
        assert_eq!(names, ["Radius", "Rows", "Columns", "Center X", "Center Y", "Center Z"]);
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
        mgr.register("Ctrl+g", Action::ToggleGrid).unwrap();
        mgr.register("`", Action::ToggleSpreadsheet).unwrap();

        // Matches with ctrl and g
        let mods_ctrl = ModifiersState { ctrl: true, alt: false, shift: false, logo: false };
        let key_g = Key::Character("g".to_string());
        assert_eq!(mgr.match_action(&mods_ctrl, &key_g), Some(Action::ToggleGrid));

        // No match with ctrl and a
        let key_a = Key::Character("a".to_string());
        assert_eq!(mgr.match_action(&mods_ctrl, &key_a), None);

        // Matches backtick with no modifiers
        let mods_none = ModifiersState::default();
        let key_tick = Key::Character("`".to_string());
        assert_eq!(mgr.match_action(&mods_none, &key_tick), Some(Action::ToggleSpreadsheet));

        // Context cycling chords: exact modifier match separates next from previous
        mgr.register("Ctrl+Tab", Action::NextContext).unwrap();
        mgr.register("Ctrl+Shift+Tab", Action::PrevContext).unwrap();
        let key_tab = Key::Named(NamedKey::Tab);
        let mods_ctrl_shift = ModifiersState { ctrl: true, alt: false, shift: true, logo: false };
        assert_eq!(mgr.match_action(&mods_ctrl, &key_tab), Some(Action::NextContext));
        assert_eq!(mgr.match_action(&mods_ctrl_shift, &key_tab), Some(Action::PrevContext));
        assert_eq!(mgr.match_action(&mods_none, &key_tab), None);
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
}
