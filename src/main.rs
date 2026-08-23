
pub mod app;
pub mod application;

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
            eprintln!("usage: cce-designer --thumbnail <project> <out.png> [--size N]");
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
        match thumbnail::run(std::path::Path::new(project), std::path::Path::new(out), size, samples) {
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
        let main_idx = state.fs_root.children.iter().position(|c| c.name == "Main").expect("Main node");

        // Re-seed the params exactly as a pre-removal save carries them.
        for (name, ty, val) in [
            ("Style", "section", ""),
            ("Bevel Profile", "ramp", "smooth;0.000:0.000,0.500:0.900,1.000:1.000"),
            ("Edge Profile", "ramp", "smooth;0.000:0.000,1.000:1.000"),
            ("Plate Color", "rgba", "#11223344"),
        ] {
            state.fs_root.children[main_idx].params.push(crate::app::ParamDef {
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
        let names: Vec<&str> = state.fs_root.children[main_idx]
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
        ];
        assert_eq!(roster.len(), WIDGET_COUNT, "roster length vs WIDGET_COUNT");
        for (i, idx) in roster.iter().enumerate() {
            assert_eq!(i, *idx, "slot #{i} expanded to index {idx}");
        }
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
        ).expect("Geometry generation failed");
        
        assert!(ocl_err.is_none(), "OpenCL compilation error: {:?}", ocl_err);
        assert_eq!(geom.vertices.len(), 2304);
        
        let mut max_dist: f32 = 0.0;
        for v in &geom.vertices {
            let dx = v.pos[0] - 0.0;
            let dy = v.pos[1] - 0.55;
            let dz = v.pos[2] - 0.0;
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
        ).expect("Geometry generation failed");
        
        assert!(ocl_err_2.is_none(), "OpenCL compilation error: {:?}", ocl_err_2);
        assert_eq!(geom_2.vertices.len(), 2304);
        
        let mut max_dist_2: f32 = 0.0;
        for v in &geom_2.vertices {
            let dx = v.pos[0] - 0.0;
            let dy = v.pos[1] - 0.55;
            let dz = v.pos[2] - 0.0;
            let dist = (dx*dx + dy*dy + dz*dz).sqrt();
            if dist > max_dist_2 {
                max_dist_2 = dist;
            }
        }
        assert!((max_dist_2 - 1.0).abs() < 0.01, "Expected radius around 1.0, got {}", max_dist_2);
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
        ).expect("Extrude geometry generation failed");
        assert!(ocl_err.is_none(), "OpenCL compilation error: {:?}", ocl_err);
        // 2304 sphere vertices = 768 triangles; 768 * 24 = 18432.
        assert_eq!(geom.vertices.len(), 18432);

        // Extruding a radius-0.5 sphere outward by the default 0.2 pushes the
        // farthest vertices to ~0.7 from its center.
        let mut max_dist: f32 = 0.0;
        for v in &geom.vertices {
            let dx = v.pos[0];
            let dy = v.pos[1] - 0.55;
            let dz = v.pos[2];
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
        ).expect("Extrude geometry generation failed (no base)");
        assert!(ocl_err2.is_none(), "OpenCL compilation error: {:?}", ocl_err2);
        assert_eq!(geom2.vertices.len(), 16128);
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
            ).expect("Geometry generation failed");
            assert!(ocl_err.is_none(), "OpenCL compilation error: {:?}", ocl_err);
            geom
        };

        // Defaults (Width/Length 1.0, Columns/Rows 16): a 16x16 grid of
        // two-triangle cells, flat at y = 0, spanning [-0.5, 0.5] on X and Z.
        let geom = generate(&[], "plane_inst");
        assert_eq!(geom.vertices.len(), 16 * 16 * 6);
        let mut max_x: f32 = 0.0;
        let mut max_z: f32 = 0.0;
        for v in &geom.vertices {
            assert!(v.pos[1].abs() < 1e-6, "Expected flat plane at y=0, got y={}", v.pos[1]);
            max_x = max_x.max(v.pos[0].abs());
            max_z = max_z.max(v.pos[2].abs());
        }
        assert!((max_x - 0.5).abs() < 0.01, "Expected half-width 0.5 on X, got {}", max_x);
        assert!((max_z - 0.5).abs() < 0.01, "Expected half-length 0.5 on Z, got {}", max_z);

        // Width and Length size their axes independently.
        let geom_2 = generate(&[("Width", "2.0"), ("Length", "3.0")], "plane_inst_2");
        let max_x_2 = geom_2.vertices.iter().map(|v| v.pos[0].abs()).fold(0.0f32, f32::max);
        let max_z_2 = geom_2.vertices.iter().map(|v| v.pos[2].abs()).fold(0.0f32, f32::max);
        assert!((max_x_2 - 1.0).abs() < 0.01, "Expected half-width 1.0 on X, got {}", max_x_2);
        assert!((max_z_2 - 1.5).abs() < 0.01, "Expected half-length 1.5 on Z, got {}", max_z_2);

        // Columns/Rows control the cell counts per axis.
        let geom_3 = generate(&[("Columns", "4"), ("Rows", "8")], "plane_inst_3");
        assert_eq!(geom_3.vertices.len(), 4 * 8 * 6);
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
        let mut attrs1 = std::collections::HashMap::new();
        attrs1.insert("UV".to_string(), GAttribute::Float2([0.1, 0.2]));
        attrs1.insert("ID".to_string(), GAttribute::Float(42.0));

        let v1 = GVertex {
            pos: [1.0, 2.0, 3.0],
            col: [1.0, 0.0, 0.0],
            attributes: attrs1,
        };

        let mut attrs2 = std::collections::HashMap::new();
        attrs2.insert("Norm".to_string(), GAttribute::Float3([0.0, 1.0, 0.0]));
        attrs2.insert("UV".to_string(), GAttribute::Float2([0.3, 0.4]));

        let v2 = GVertex {
            pos: [4.0, 5.0, 6.0],
            col: [0.0, 1.0, 0.0],
            attributes: attrs2,
        };

        let mut geom1 = Geometry { vertices: vec![v1] };
        let geom2 = Geometry { vertices: vec![v2] };

        geom1.merge(geom2);
        assert_eq!(geom1.vertices.len(), 2);

        let render_verts = geom1.to_vertex3d_vec();
        assert_eq!(render_verts.len(), 2);
        assert_eq!(render_verts[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(render_verts[0].color, [1.0, 0.0, 0.0]);
        assert_eq!(render_verts[1].position, [4.0, 5.0, 6.0]);
        assert_eq!(render_verts[1].color, [0.0, 1.0, 0.0]);

        let (headers, rows) = State::geometry_to_spreadsheet_data(&geom1);

        let expected_headers = vec![
            "Vertex".to_string(),
            "Pos.x".to_string(),
            "Pos.y".to_string(),
            "Pos.z".to_string(),
            "Col.r".to_string(),
            "Col.g".to_string(),
            "Col.b".to_string(),
            "ID".to_string(),
            "Norm.x".to_string(),
            "Norm.y".to_string(),
            "Norm.z".to_string(),
            "UV.x".to_string(),
            "UV.y".to_string(),
        ];
        assert_eq!(headers, expected_headers);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "0");
        assert_eq!(rows[0][1], "1.0000"); // Pos X
        assert_eq!(rows[0][7], "42.0000"); // ID
        assert_eq!(rows[0][8], "-"); // Norm.x
        assert_eq!(rows[0][9], "-"); // Norm.y
        assert_eq!(rows[0][10], "-"); // Norm.z
        assert_eq!(rows[0][11], "0.1000"); // UV.x
        assert_eq!(rows[0][12], "0.2000"); // UV.y

        assert_eq!(rows[1][0], "1");
        assert_eq!(rows[1][1], "4.0000"); // Pos X
        assert_eq!(rows[1][7], "-"); // ID
        assert_eq!(rows[1][8], "0.0000"); // Norm.x
        assert_eq!(rows[1][9], "1.0000"); // Norm.y
        assert_eq!(rows[1][10], "0.0000"); // Norm.z
        assert_eq!(rows[1][11], "0.3000"); // UV.x
        assert_eq!(rows[1][12], "0.4000"); // UV.y
    }

    #[test]
    fn test_line_geometry_generation() {
        let start = Vec3::new(0.0, 0.0, 0.0);
        let end = Vec3::new(0.0, 1.0, 0.0);
        let geom = line_vertices(start, end, 0.02);
        
        // A box line should contain 36 vertices (6 faces * 2 triangles * 3 vertices)
        assert_eq!(geom.vertices.len(), 36);
        
        // Every vertex should have "Norm" and "UV" attributes
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
        let main_idx = state.fs_root.children.iter().position(|c| c.name == "Main").expect("Main node");
        state.graph_mut().set_selected_node(Some(main_idx));
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
}

