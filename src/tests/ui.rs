//! Panes, plates, menus, the viewport, parameters and project round trips: the app state driven through synthetic events.

use super::*;

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
/// Format 7: Composite's Length is the length of Name. A save from
/// before, which wrote |Source B| into Name (or Result), has Source B
/// moved into Name and the written attribute named as Result, and
/// computes what it computed.
#[test]
fn a_saved_composite_length_keeps_its_result() {
    use crate::app::{FsNode, ParamDef, Project};
    use crate::detail::AttribValue;
    let node = |id: &str, params: &[(&str, &str)]| FsNode {
        id: id.into(),
        name: id.into(),
        node_type: "attribute".into(),
        children: vec![],
        params: params.iter().map(|(k, v)| ParamDef::new(*k, "text", *v)).collect(),
        geometry_visible: true,
        bypassed: false,
        position: (0.0, 0.0),
        inputs: 1,
        outputs: 1,
    };
    let length = [("attribute_name", "a"), ("operation", "Composite"), ("source_b", "b"), ("combine_op", "Length")];
    let mut with_result = length.to_vec();
    with_result.push(("result", "r"));
    let mut root = node("root", &[]);
    root.node_type = "subnet".into();
    root.children = vec![
        node("in_place", &length),
        node("into_r", &with_result),
        node("no_b", &[("attribute_name", "a"), ("operation", "Composite"), ("combine_op", "Length")]),
        node("add", &[("attribute_name", "a"), ("operation", "Composite"), ("source_b", "b"), ("combine_op", "Add")]),
    ];
    let mut proj = Project { name: "p".into(), root, view_state: Default::default(), format: 6 };
    proj.migrate_format();
    let row = |i: usize, r: &str| {
        proj.root.children[i].params.iter().find(|p| p.name == r).map(|p| p.text().to_string()).unwrap_or_default()
    };
    assert_eq!((row(0, "attribute_name"), row(0, "result"), row(0, "source_b")), ("b".into(), "a".into(), "".into()));
    assert_eq!((row(1, "attribute_name"), row(1, "result")), ("b".into(), "r".into()), "a Result is kept");
    assert_eq!((row(2, "attribute_name"), row(2, "result")), ("a".into(), "".into()), "no Source B: left alone");
    assert_eq!(row(3, "source_b"), "b", "only Length moves");

    // It computes what it did: |b| in every component of a.
    let mut geom = crate::geometry::sphere_detail(glam::Vec3::ZERO, 0.5, 4, 6);
    geom.points_mut().create("a", AttribValue::Float3([1.0, 1.0, 1.0]));
    geom.points_mut().create("b", AttribValue::Float3([3.0, 4.0, 0.0]));
    let mut err = None;
    crate::geometry::apply_attribute(&mut geom, &proj.root.children[0], &mut err);
    assert!(err.is_none(), "{err:?}");
    assert_eq!(geom.points().value("a", 2), Some(AttribValue::Float3([5.0, 5.0, 5.0])));
    assert_eq!(geom.points().value("b", 2), Some(AttribValue::Float3([3.0, 4.0, 0.0])));
}

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

/// The way back. A detached pane's menu offers Reattach and nothing else,
/// and reattaching restores the pane in full.
#[test]
fn test_reattach_brings_a_detached_pane_back() {
    use crate::plate_menu::PlateMenuAction;
    use crate::slots::PARAM_IDX;
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    let (_, _, _, full_h) = state.slots.get_dyn(&state.ui_context, PARAM_IDX).rect();

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
    let (_, _, _, back_h) = state.slots.get_dyn(&state.ui_context, PARAM_IDX).rect();
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
    state.execute_menu_action("Show Spreadsheet Pane");

    // A plate that collapses: not the network nor the params HUD,
    // which are on the scene and not plates of a dock.
    let idx = PLATE_SLOTS.iter().copied()
        .filter(|&i| i != crate::slots::NETWORK_PANEL_IDX && i != crate::slots::PARAM_IDX)
        .find(|&i| state.slots.get_dyn(&state.ui_context, i).visible())
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

/// The network spans the body, shown though it fronts no dock (it was
/// in the left dock until 2026-10-07, when it left, and the docks went
/// the same day), and it does not collapse: its plate menu is Detach.
/// A press in the band its dock had along the top, off the
/// breadcrumb's segments, is the scene's.
#[test]
fn the_network_is_in_no_dock() {
    use crate::app::HEADER_H;
    use crate::plate_menu::PlateMenuAction;
    use crate::slots::{BREADCRUMB_IDX, LEFT_MENUBAR_IDX, NETWORK_PANEL_IDX, SPREADSHEET_IDX};
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::{ElementState, MouseButton};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.execute_menu_action("Show Spreadsheet Pane");
    assert_eq!(state.positions[NETWORK_PANEL_IDX], (0.0, HEADER_H, 1600.0, state.body_h()));
    assert!(state.slots.get_dyn(&state.ui_context, crate::slots::CONTENT_IDX).visible(), "shown");
    assert_eq!(state.positions[SPREADSHEET_IDX].0, 18.0, "the spreadsheet flush left, a gap in");

    state.set_pane_collapsed(NETWORK_PANEL_IDX, true);
    assert!(!state.pane_is_collapsed(NETWORK_PANEL_IDX));
    state.open_plate_menu(NETWORK_PANEL_IDX);
    assert_eq!(state.plate_menu_actions, vec![PlateMenuAction::Detach]);
    state.close_plate_menu();

    let at = |state: &mut State, x: f32, y: f32| {
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
    };
    let press = |state: &mut State, s: ElementState| {
        state.handle_event(&WindowEvent::MouseInput { state: s, button: MouseButton::Left });
    };

    // The old dock's top band, off the crumbs and off every node: the
    // scene's, where it used to focus the network and go nowhere.
    let (bx, by, _, bh) = state.positions[BREADCRUMB_IDX];
    let y = by + bh * 0.5;
    let x = (0..40)
        .map(|k| bx + 200.0 + k as f32 * 5.0)
        .find(|&x| {
            !state.slots.get_dyn(&state.ui_context, BREADCRUMB_IDX).hit_test(x, y, &state.ui_context)
                && state.graph().node_at(x, y).is_none()
                && !state.over_floating_pane_at(x, y)
        })
        .expect("a point in the band off the crumbs");
    assert!(x < 18.0 + 400.0, "inside the old dock's band");
    state.focused_pane = crate::slots::RIGHT_MENUBAR_IDX;
    at(&mut state, x, y);
    press(&mut state, ElementState::Pressed);
    assert!(state.orbit_drag.is_some(), "the press orbits");
    press(&mut state, ElementState::Released);
    assert_ne!(state.focused_pane, LEFT_MENUBAR_IDX);
}

/// An older save docked the network and sometimes the second network
/// editor (removed 2026-10-07), in front of it or waiting behind it as a
/// tab, and pinned panes to it. All of it is ignored — the docks went the
/// same day — and the save loads, the spreadsheet along the bottom.
#[test]
fn an_older_saves_second_network_editor_is_dropped() {
    use crate::slots::SPREADSHEET_IDX;
    let dir = std::env::temp_dir().join(format!("cce-designer-hidden-editor-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let mut a = State::new(false);
    a.save_to_file(&dir).expect("save");
    let file = dir.join("state.json");
    let load_with = |tabs: serde_json::Value| {
        let mut json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        json["view_state"]["dock_tabs"] = tabs;
        json["view_state"]["current_path2"] = serde_json::json!([0]);
        json["view_state"]["params_pin"] = serde_json::json!("network2");
        fs::write(&file, serde_json::to_string(&json).unwrap()).unwrap();
        let mut b = State::new(false);
        b.load_from_file(&dir).expect("load");
        b
    };
    for tabs in [
        serde_json::json!([["network", "network2"], [], ["spreadsheet"]]),
        serde_json::json!([["network2", "network"], [], ["spreadsheet"]]),
        serde_json::json!([[], ["network2"], ["spreadsheet"]]),
    ] {
        let b = load_with(tabs.clone());
        assert_eq!(b.floating_spreadsheet_rect().0, 18.0, "{tabs}");
        assert_eq!(b.positions[SPREADSHEET_IDX].0, if b.show_spreadsheet { 18.0 } else { 0.0 }, "{tabs}");
    }
    let _ = fs::remove_dir_all(&dir);
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
    a.set_pane_collapsed(crate::slots::SPREADSHEET_IDX, true);
    a.set_pane_collapsed(PARAM_IDX, true);
    assert!(!a.collapsed_panes[PARAM_IDX], "the params HUD does not collapse");
    a.splitter_layout.splitter1_x = 400.0;
    a.splitter_layout.splitter2_x = 1200.0;
    a.save_to_file(&dir).expect("save");

    let mut b = State::new(false);
    b.width = 800.0;
    b.load_from_file(&dir).expect("load");
    assert!(!b.show_viewport, "viewport hidden in the save must load hidden");
    assert!(b.show_spreadsheet, "spreadsheet shown in the save must load shown");
    assert!(b.collapsed_panes[crate::slots::SPREADSHEET_IDX], "a pane's collapse must round-trip");
    assert!(!b.collapsed_panes[PARAM_IDX]);
    assert!((b.splitter_layout.splitter1_x - 200.0).abs() < 1.0,
        "splitters restore as fractions: 400/1600 of an 800-wide window = 200, got {}",
        b.splitter_layout.splitter1_x);

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
    a.params_hud_width = 420.0;
    a.floating_spreadsheet_height = 300.0;
    a.rebuild_positions();
    a.save_to_file(&dir).expect("save");

    let plates = a.project_view_state().plates.expect("a sized window records its plates");
    assert!((plates.hud_width.unwrap() - 420.0 / 1600.0).abs() < 1e-4, "widths save as window fractions");
    assert!(plates.params_width.is_none(), "the retired right dock width is never written");

    let mut b = State::new(false);
    b.resize(1600.0, 900.0, 1.0);
    b.load_from_file(&dir).expect("load");
    assert!((b.params_hud_width - 420.0).abs() < 0.5, "HUD width: {}", b.params_hud_width);
    assert!((b.floating_spreadsheet_height - 300.0).abs() < 0.5, "spreadsheet height: {}", b.floating_spreadsheet_height);

    // Half the window: the same fractions land at half the pixels.
    let mut c = State::new(false);
    c.resize(800.0, 450.0, 1.0);
    c.load_from_file(&dir).expect("load half-size");
    assert!((c.params_hud_width - 210.0).abs() < 0.5, "scaled HUD width: {}", c.params_hud_width);
    assert!((c.floating_spreadsheet_height - 150.0).abs() < 0.5, "scaled spreadsheet height: {}", c.floating_spreadsheet_height);

    // A detached pane window keeps its own plates.
    let mut d = State::new(true);
    let before = d.params_hud_width;
    d.load_from_file(&dir).expect("load detached");
    assert_eq!(d.params_hud_width, before);

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
    a.params_hud_width = 310.0;
    a.floating_spreadsheet_height = 100.0;
    a.rebuild_positions();
    a.save_to_file(&dir).expect("save");

    // No resize before the load: this is the startup order.
    let mut b = State::new(false);
    assert!(!b.window_configured);
    b.load_from_file(&dir).expect("load");
    b.resize(1400.0, 1080.0, 1.0);
    assert!((b.params_hud_width - 310.0).abs() < 0.5, "HUD width: {}", b.params_hud_width);
    assert!((b.floating_spreadsheet_height - 100.0).abs() < 0.5, "spreadsheet height: {}", b.floating_spreadsheet_height);
    let pg = b.project_view_state().plates.expect("plates");
    assert!((pg.hud_width.unwrap() - 310.0 / 1400.0).abs() < 1e-4, "a save writes back what was loaded: {:?}", pg);

    // Only the FIRST configure: a later window resize keeps the plates'
    // pixels, as it always has.
    b.resize(1000.0, 1080.0, 1.0);
    assert!((b.params_hud_width - 310.0).abs() < 0.5, "a later resize rescaled the plate: {}", b.params_hud_width);

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
    a.params_hud_width = 310.0;
    a.floating_spreadsheet_height = 180.0;
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
    c.set_pane_collapsed(crate::slots::SPREADSHEET_IDX, true);
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
    s.params_hud_width = 650.0;
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
    assert!((s.params_hud_width - 650.0).abs() < 0.5, "HUD width: {}", s.params_hud_width);
    assert!((s.floating_spreadsheet_rect().3 - 600.0).abs() < 0.5, "spreadsheet height: {:?}", s.floating_spreadsheet_rect());
    assert!((s.positions[crate::slots::PARAM_IDX].2 - 650.0).abs() < 0.5, "drawn HUD width: {:?}", s.positions[crate::slots::PARAM_IDX]);
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
    main.params_hud_width = 650.0;
    main.floating_spreadsheet_height = 420.0;
    main.viewport_mut().rotation_x = 0.7;
    main.rebuild_positions();

    let mut child = State::new(false);
    child.detached_pane = Some(crate::slots::PARAM_IDX);
    child.resize(400.0, 300.0, 1.0);
    child.rebuild_positions();
    child.fs_root.children[0].name = "synced_edit".to_string();
    child.save_to_file(&channel).expect("child writes the channel");

    main.app_drag = Some(crate::app::AppDrag::HudResize { start_w: 650.0, start_mouse_x: 0.0 });
    main.load_sync_channel(&channel, true).expect("main reloads");

    assert!(main.fs_root.children.iter().any(|c| c.name == "synced_edit"), "the tree edit must sync");
    assert!((main.params_hud_width - 650.0).abs() < 0.5, "HUD width: {}", main.params_hud_width);
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
    main.params_hud_width = 650.0;
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
    let hud = plates.hud_width.expect("the HUD's width");
    assert!((hud - 650.0 / 1600.0).abs() < 1e-4, "HUD width: {hud}");
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

    state.floating_spreadsheet_height += 60.0;
    state.rebuild_positions();
    state.update_window_title();
    assert!(state.has_unsaved_changes(), "a plate resize must dirty the title");
    assert!(state.title.ends_with('*'), "title: {}", state.title);

    state.save_to_file(&dir).expect("save");
    assert!(!state.has_unsaved_changes(), "saving clears it");
    state.update_window_title(); // the event loop's refresh, after the save event
    assert!(!state.title.ends_with('*'), "title: {}", state.title);

    state.set_pane_collapsed(crate::slots::SPREADSHEET_IDX, true);
    assert!(state.has_unsaved_changes(), "a collapse is saved state too");
    state.save_to_file(&dir).expect("save");
    state.params_hud_width += 40.0;
    assert!(state.has_unsaved_changes(), "a HUD resize is saved state too");

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
    assert_eq!(state.ui_context[state.slots.content].inner().wire_style(), WireStyle::Bezier);
    assert_eq!(state.display_settings().viewport.node_wire_style, "bezier");
    state.save_to_file(&dir).expect("save");

    state.apply_setting("Node Wire Style", "Straight");
    assert!(state.has_unsaved_changes(), "a wire style is an edit to the file");
    state.load_from_file(&dir).expect("load");
    assert_eq!(state.ui_context[state.slots.content].inner().wire_style(), WireStyle::Bezier, "the file's style comes back");

    // A file that names no style hands the choice back to the config.
    let state_json = dir.join("state.json");
    let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&state_json).unwrap()).unwrap();
    v["view_state"]["display"]["viewport"].as_object_mut().unwrap().remove("node_wire_style");
    fs::write(&state_json, serde_json::to_string(&v).unwrap()).unwrap();
    state.load_from_file(&dir).expect("load an older save");
    assert_eq!(state.ui_context[state.slots.content].inner().chosen_wire_style(), None);
    assert_eq!(state.ui_context[state.slots.content].inner().wire_style(), WireStyle::configured());

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

    // Put the HUD, with a node's rows on its plate, over the middle of
    // the scene: the numbers under the plate go, the rest stay.
    let mut redraw = false;
    let slot = geo(&state.fs_root).children.iter().position(|c| c.node_type == "sphere").unwrap();
    state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
    state.params_plate = true;
    state.positions[crate::slots::PARAM_IDX] = (800.0, 200.0, 200.0, 690.0);
    state.slots.get_dyn_mut(&mut state.ui_context, crate::slots::PARAM_IDX).set_rect(800.0, 200.0, 200.0, 690.0);
    let plate = state.params_claim();
    assert!(plate.1 == 200.0 && plate.3 > 100.0 && plate.3 < 690.0, "the plate fits the rows: {plate:?}");
    let in_plate = |x: f32, y: f32| x >= 800.0 && x < 1000.0 && y + 6.0 >= plate.1 && y + 6.0 < plate.1 + plate.3;
    let fewer = state.point_number_labels();
    assert!(fewer.len() < all.len(), "{} of {} are left", fewer.len(), all.len());
    assert!(!fewer.is_empty());
    assert!(fewer.iter().all(|(_, x, y, ..)| !in_plate(*x, *y)));

    // Without its plate the HUD hides only what its rows stand on: the
    // band it claims is narrower, and more numbers show.
    state.params_plate = false;
    assert!(state.point_number_labels().len() >= fewer.len());
    state.params_plate = true;
    state.positions[crate::slots::PARAM_IDX] = (px, py, pw, ph);
    state.slots.get_dyn_mut(&mut state.ui_context, crate::slots::PARAM_IDX).set_rect(px, py, pw, ph);
    assert_eq!(state.point_number_labels().len(), all.len());
}

/// The playbar is attached to the window's bottom edge, the full width;
/// the plates above stop a gap short of its top, and the viewport's
/// bottom-anchored text stands on it rather than on its transport.
#[test]
fn the_playbar_is_attached_to_the_bottom_edge() {
    use crate::app::{playbar_shelf_h, PLAYBAR_H, STATUS_H};
    use crate::slots::{PLAYBAR_IDX, SPREADSHEET_IDX};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.execute_menu_action("Show Playbar Pane");
    state.execute_menu_action("Show Spreadsheet Pane");
    state.rebuild_positions();
    state.apply_layout();
    assert!(state.show_playbar && state.show_spreadsheet);
    let pb = state.positions[PLAYBAR_IDX];
    let lip = cce_ui::layout::bevel_width();
    assert_eq!(playbar_shelf_h(), PLAYBAR_H + lip);
    assert_eq!(pb, (0.0, 900.0 - STATUS_H - playbar_shelf_h(), 1600.0, playbar_shelf_h()), "flush to the bottom, the full width, down into the lip");
    let flags = cce_ui::scene::paint::PlateSpec::window_corner_flags(cce_ui::scene::layout::Rect { x: pb.0, y: pb.1, width: pb.2, height: pb.3 }, 1600.0, 900.0);
    assert_eq!(flags, (false, false, true, true), "its bottom corners are the window's");
    // A shelf of the window's edge: the transport stands clear of the
    // window's lip, which is drawn over the shelf's sides and bottom.
    assert_eq!(state.ui_context[state.slots.playbar].inner().frame, lip);
    let pb_rect = cce_ui::scene::layout::Rect { x: pb.0, y: pb.1, width: pb.2, height: pb.3 };
    let prev = state.ui_context[state.slots.playbar].inner().transport_button_rect(pb_rect, -1).expect("the step buttons are on");
    assert!(prev.x >= lip, "clear of the left lip: {prev:?}");
    assert!(prev.y + prev.height <= 900.0 - STATUS_H - lip, "clear of the bottom lip: {prev:?}");
    // (The network has no plate: it spans the window, under the shelf.)
    for idx in [SPREADSHEET_IDX] {
        let (_, y, _, h) = state.positions[idx];
        assert_eq!(y + h, pb.1 - 18.0, "the {idx} plate stops a gap above it");
    }
    assert_eq!(state.scene_text_floor(900.0), pb.1, "the viewport's bottom text stands on it");
    state.execute_menu_action("Show Playbar Pane");
    assert!(!state.show_playbar);
    assert_eq!(state.scene_text_floor(900.0), 900.0);
}

/// The params HUD lives on the scene: laid out from the viewport, under
/// every plate, so where one covers it the plate takes the pointer and
/// the HUD draws nothing — but it stops a gap above the spreadsheet and
/// the playbar along the bottom, and scrolls what does not fit. A plate
/// in the right dock, over its top, sizes nothing.
#[test]
fn the_params_hud_is_under_the_plates_and_stops_above_the_bottom_ones() {
    use crate::slots::{PARAM_IDX, PLAYBAR_IDX, SPREADSHEET_IDX};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    let hud = state.positions[PARAM_IDX];
    assert_eq!(hud, state.params_hud_rect());
    assert_eq!(hud.1 + hud.3, 900.0 - crate::app::STATUS_H - 18.0, "the viewport's height, a gap in");

    // The spreadsheet and the playbar below it: it stops a gap above
    // the higher of them. The plate is as wide as its table: a narrow
    // one, along the left, is not under the HUD and leaves it be.
    state.execute_menu_action("Show Spreadsheet Pane");
    state.execute_menu_action("Show Playbar Pane");
    state.floating_spreadsheet_height = 500.0;
    state.spreadsheet_mut().set_spreadsheet_data(vec!["a".into()], vec![vec!["1".into()]]);
    state.rebuild_positions();
    state.apply_layout();
    let (sx, _, sw, _) = state.positions[SPREADSHEET_IDX];
    assert!(sx + sw < hud.0, "a narrow table's plate stops short of the HUD");
    let below_playbar = state.positions[PARAM_IDX];
    assert!(below_playbar.1 + below_playbar.3 > state.positions[SPREADSHEET_IDX].1, "and the HUD runs past its top");
    // A table as wide as the window reaches under the HUD.
    let headers: Vec<String> = (0..40).map(|i| format!("column{i}")).collect();
    state.spreadsheet_mut().set_spreadsheet_data(headers.clone(), vec![headers.iter().map(|_| "0.0000".to_string()).collect()]);
    state.rebuild_positions();
    state.apply_layout();
    let (hx, hy, hw, hh) = state.positions[PARAM_IDX];
    let (sx, sy, sw, _) = state.positions[SPREADSHEET_IDX];
    assert_eq!((hx, hy, hw), (hud.0, hud.1, hud.2), "only its bottom moved");
    assert!(sx < hx + hw && sx + sw > hx, "the spreadsheet is under the HUD's span");
    assert_eq!(hy + hh, sy - 18.0, "it stops a gap above the spreadsheet");
    // Collapsed, the spreadsheet is a stub at its own top edge, and the
    // HUD stops above that.
    state.set_pane_collapsed(SPREADSHEET_IDX, true);
    let stub = state.positions[SPREADSHEET_IDX];
    assert_eq!(stub.1, sy);
    let (_, hy2, _, hh2) = state.positions[PARAM_IDX];
    assert_eq!(hy2 + hh2, stub.1 - 18.0);
    state.set_pane_collapsed(SPREADSHEET_IDX, false);
    // Hidden, the HUD stops above the playbar instead.
    state.execute_menu_action("Show Spreadsheet Pane");
    assert!(!state.show_spreadsheet);
    let (_, hy3, _, hh3) = state.positions[PARAM_IDX];
    assert_eq!(hy3 + hh3, state.positions[PLAYBAR_IDX].1 - 18.0, "above the playbar");
    state.execute_menu_action("Show Spreadsheet Pane");
    // Its own width still sizes it, about its right edge.
    state.params_hud_width = 420.0;
    state.rebuild_positions();
    assert_eq!(state.positions[PARAM_IDX].2, 420.0);
    assert_eq!(state.positions[PARAM_IDX].0 + 420.0, hud.0 + hud.2, "it keeps its right edge");

    // Rows that do not fit scroll, and the plate fills the HUD. (A
    // short window: the sphere's table is narrow, its plate stops short
    // of the HUD and the HUD runs down to the playbar.)
    state.resize(1600.0, 360.0, 1.0);
    let mut redraw = false;
    let slot = geo(&state.fs_root).children.iter().position(|c| c.node_type == "sphere").unwrap();
    state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
    state.params_plate = true;
    state.rebuild_positions();
    state.apply_layout();
    let (hx, hy, hw, hh) = state.positions[PARAM_IDX];
    let pb = state.slots.get_dyn(&state.ui_context, PARAM_IDX).as_any().downcast_ref::<cce_ui::widget::ParametersBg>().unwrap();
    assert!(pb.scrollbar_visible(), "the sphere's rows overflow a {hh} px HUD and scroll");
    assert_eq!(state.params_claim(), (hx, hy, hw, hh), "the plate fills the HUD");

    // (A plate in the right dock stood over the HUD's top and took the
    // pointer there, until the docks went, 2026-10-07; the spreadsheet
    // and the playbar it stops above are the plates left.)
    assert!(!state.plates_over_params().iter().any(|&(x, y, w, h)| {
        x < hx + hw && x + w > hx && y < hy + hh && y + h > hy
    }), "no plate stands over the HUD");
}

/// What the plates leave of the HUD, as rects that do not overlap.
#[test]
fn uncovered_takes_the_covers_out_of_a_rect() {
    use crate::render::uncovered;
    let r = cce_ui::scene::layout::Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
    let area = |v: &[cce_ui::scene::layout::Rect]| v.iter().map(|p| p.width * p.height).sum::<f32>();
    assert_eq!(uncovered(r, &[]).len(), 1);
    assert_eq!(area(&uncovered(r, &[(0.0, 60.0, 100.0, 40.0)])), 6000.0, "a strip across the bottom");
    assert!(uncovered(r, &[(-10.0, -10.0, 200.0, 200.0)]).is_empty(), "covered whole");
    let hole = uncovered(r, &[(40.0, 40.0, 20.0, 20.0)]);
    assert_eq!(area(&hole), 10000.0 - 400.0, "a hole in the middle");
    assert_eq!(area(&uncovered(r, &[(0.0, 60.0, 100.0, 40.0), (80.0, 0.0, 50.0, 100.0)])), 10000.0 - 4000.0 - 20.0 * 60.0);
    assert_eq!(area(&uncovered(r, &[(200.0, 0.0, 10.0, 10.0)])), 10000.0, "a cover elsewhere");
}

/// The params HUD's plate is fitted to its rows — padded under the last
/// as the first is under the top — and on by default; without it the
/// rows stand on the scene. Either way the HUD claims only the band its
/// rows cover: a press or the wheel under it is the viewport's.
#[test]
fn a_node_name_on_a_light_floor_is_written_dark() {
    let mut state = State::new(false);
    let light = cce_ui::color::parse_hex_rgba_linear("#d8dae4cc").unwrap();
    let dark = cce_ui::color::parse_hex_rgba_linear("#202028cc").unwrap();
    // No floor: a name stands on the bare scene and keeps the widget's
    // light ink, whatever the tint.
    state.node_compression = None;
    state.node_tint = Some(light);
    assert_eq!(state.node_ink(), None);
    // A floor in a dark tint keeps it too; a floor in a light one is
    // written dark.
    state.node_compression = Some(0.85);
    state.node_tint = Some(dark);
    assert_eq!(state.node_ink(), None);
    state.node_tint = Some(light);
    assert!(state.node_ink().is_some_and(|ink| ink.iter().all(|&c| c < 0x40)));
}

#[test]
fn the_params_plate_fits_its_rows() {
    let mut state = State::new(false);
    assert!(state.params_plate, "the plate is on by default");
    assert_eq!(state.command_toggle_state("toggle_params_plate"), Some(true));
    // A HUD with no rows collapses its plate to the small circle, and
    // claims that; with the plate off it claims nothing.
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    if !state.params_have_rows() {
        assert_eq!(state.params_claim(), state.params_dot_rect());
    }
    assert!(state.run_command("toggle_params_plate"));
    assert!(!state.params_plate);
    if !state.params_have_rows() {
        assert_eq!(state.params_claim().3, 0.0);
    }
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    let mut redraw = false;
    let slot = geo(&state.fs_root).children.iter().position(|c| c.node_type == "sphere").unwrap();
    state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
    state.apply_layout();

    let (px, py, pw, ph) = state.positions[crate::slots::PARAM_IDX];
    let rows = state.param_row_rects();
    let last = rows.iter().filter(|r| r.3 > 0.0).map(|r| r.1 + r.3).fold(py, f32::max);
    assert!(last + crate::app::PARAMS_CLAIM_PAD < py + ph, "the sphere's rows leave room under them");
    let (row_x, row_y) = (px + pw * 0.5, rows[0].1 + rows[0].3 * 0.5);
    let under = (px + pw * 0.5, (last + py + ph) * 0.5);

    // A row is the pane's; the space under the rows is the scene's.
    assert!(state.params_claims(row_x, row_y));
    assert!(!state.params_claims(under.0, under.1));
    state.cursor_x = under.0;
    state.cursor_y = under.1;
    assert!(state.cursor_in_viewport(), "under the rows is the viewport");
    assert!(!state.on_param_resize_edge(px, under.1), "and the pane's edge runs only as far as its rows");

    // With the plate: it runs as far under the last row as the first
    // row stands under the HUD's top, and no further.
    assert!(state.run_command("toggle_params_plate"));
    assert!(state.params_plate);
    let (_, cy, _, ch) = state.params_claim();
    let first = rows.iter().filter(|r| r.3 > 0.0).map(|r| r.1).fold(f32::INFINITY, f32::min);
    assert_eq!(cy, py);
    assert!((cy + ch - (last + (first - py))).abs() < 0.01, "padded alike: {} vs {}", cy + ch, last + first - py);
    assert!(state.params_claims(px + pw * 0.5, last + 2.0), "the plate's bottom margin is the HUD's");
    assert!(!state.params_claims(under.0, under.1), "under the plate is the scene's");
    assert!(state.cursor_in_viewport());
    assert!(!state.on_param_resize_edge(px, under.1));
    assert!(state.on_param_resize_edge(px, row_y));
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
    let band = context_menu::with_state(|m| m.borrow().slider_band(i));
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
    let mut state = State::new(false);
    let groups = |page: Option<P>| -> Vec<Vec<A>> {
        let (options, actions) = state.viewport_menu_rows_of(page);
        assert_eq!(options.len(), actions.len());
        assert!(options.iter().zip(&actions).all(|(o, a)| (o == "-") == (*a == A::Separator)), "separator rows line up");
        actions.split(|a| *a == A::Separator).map(|g| g.to_vec()).collect()
    };
    assert_eq!(
        groups(None),
        vec![
            vec![A::NetworkPage],
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
/// row is a row of the menu, not a page: it closes the menu and opens
/// them in the params HUD. A row of the menu itself still closes it.
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
    assert!([style, markers].iter().all(|&i| context_menu::leads_to_page(i)), "the two pages are page rows");
    assert!(!context_menu::leads_to_page(vis), "the visualizers are the HUD's, not a page");
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

    // A row of the menu itself runs and closes it.
    let grid = row_of(&state, A::Command("toggle_grid"));
    move_to(&mut state, context_menu::x() + 20.0, context_menu::row_y(grid) + 12.0);
    press(&mut state);
    assert!(!state.viewport_menu_open(), "a row of the menu closes it");

    // The visualizers' row too, and the HUD shows them.
    state.cursor_x = 300.0;
    state.cursor_y = 200.0;
    state.open_viewport_context_menu();
    let vis = row_of(&state, A::Command("attribute_visualizers"));
    move_to(&mut state, context_menu::x() + 20.0, context_menu::row_y(vis) + 12.0);
    press(&mut state);
    assert!(!state.viewport_menu_open() && !state.dialog_visible());
    assert!(state.vis_hud.is_some(), "the visualizers are in the params HUD");
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
    let drawn = |state: &State| state.drawn_markers(crate::app::MarkerKind::Overlay);
    assert!(drawn(&state).is_empty());
    state.run_command("toggle_vertex_markers");
    let built = drawn(&state);
    assert!(!built.is_empty() && !state.overlay_vertex_marker_points.is_empty());
    assert_eq!(built.len(), state.vertex_marker_instances.len() * 240, "a sphere drawn over each instance");
    state.point_marker_size *= 2.0;
    state.rebuild_overlay_markers();
    assert_eq!(drawn(&state).len(), built.len());
    assert_ne!(drawn(&state)[0].position, built[0].position, "re-sized");
    state.run_command("toggle_vertex_markers");
    assert!(drawn(&state).is_empty(), "and gone with the switch");

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
            vec![A::PlatePage],
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
    assert_eq!(state.ui_context[state.slots.playbar].inner().fps, 30.0);
    assert!(state.playbar_menu_open(), "a slider row keeps the menu up");
    let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
    assert_eq!(crate::app::DesignSettings::from_kdl_str(&kdl).playbar_fps, 30.0, "persisted");
    assert!(!state.has_unsaved_changes(), "the rate is a setting, not the project's");

    // The range: an end moved past the other carries it along, and the
    // playhead stays inside.
    state.ui_context[state.slots.playbar].inner_mut().current_frame = 200.0;
    let i = actions.iter().position(|a| *a == A::EndFrameSlider).unwrap();
    state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
    state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, -100.0) });
    let pb = state.ui_context[state.slots.playbar].inner();
    assert_eq!(pb.end_frame, 140.0, "{}", pb.end_frame);
    assert_eq!(pb.current_frame, 140.0, "the playhead is kept inside");
    assert!(state.has_unsaved_changes(), "the range is the project's");
    let i = actions.iter().position(|a| *a == A::StartFrameSlider).unwrap();
    state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
    state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 150.0) });
    let pb = state.ui_context[state.slots.playbar].inner();
    assert_eq!((pb.start_frame, pb.end_frame), (151.0, 152.0), "the far end is carried a frame ahead of the near");

    // A command row runs and closes.
    let i = actions.iter().position(|a| *a == A::Command("toggle_playbar_repeat")).unwrap();
    state.cursor_y = context_menu::row_y(i) + context_menu::ROW_H * 0.5;
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
    assert!(!state.playbar_menu_open());
    assert!(!state.ui_context[state.slots.playbar].inner().repeat, "Repeat was flipped");

    // The range rides the project file.
    state.save_to_file(&dir).expect("save");
    assert!(!state.has_unsaved_changes());
    let mut again = State::new(false);
    again.resize(1600.0, 900.0, 1.0);
    again.load_from_file(&dir).expect("load");
    let pb = again.ui_context[again.slots.playbar].inner();
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
    assert!(state.ui_context[state.slots.playbar].inner().step_buttons, "on by default");
    let press = |state: &mut State, r: Rect| {
        let (x, y) = (r.x + r.width * 0.5, r.y + r.height * 0.5);
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
    };
    let pb = state.ui_context[state.slots.playbar].inner();
    let prev = pb.transport_button_rect(rect, -1).expect("a Previous Frame button");
    let play = pb.transport_button_rect(rect, 0).expect("a play button");
    let next = pb.transport_button_rect(rect, 1).expect("a Next Frame button");
    assert!(prev.x + prev.width < play.x && play.x + play.width < next.x, "|< > >| left to right");

    state.ui_context[state.slots.playbar].inner_mut().current_frame = 10.4;
    press(&mut state, next);
    assert_eq!(state.ui_context[state.slots.playbar].inner().current_frame, 11.0, "a whole frame on, off the rounded one");
    press(&mut state, prev);
    press(&mut state, prev);
    assert_eq!(state.ui_context[state.slots.playbar].inner().current_frame, 9.0);
    assert!(!state.ui_context[state.slots.playbar].inner().playing, "a step does not start playback");
    state.ui_context[state.slots.playbar].inner_mut().current_frame = 1.0;
    press(&mut state, prev);
    assert_eq!(state.ui_context[state.slots.playbar].inner().current_frame, 1.0, "held inside the range");

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
    let pb = state.ui_context[state.slots.playbar].inner();
    assert!(!pb.step_buttons);
    assert!(pb.transport_button_rect(rect, -1).is_none() && pb.transport_button_rect(rect, 1).is_none());
    assert_eq!(pb.transport_button_rect(rect, 0).unwrap().x, prev.x, "the play button takes the first place");
    let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
    assert!(!crate::app::DesignSettings::from_kdl_str(&kdl).playbar_step_buttons, "persisted");

    // Where Next Frame stood is the track now: a press there scrubs.
    state.ui_context[state.slots.playbar].inner_mut().current_frame = 50.0;
    press(&mut state, next);
    assert_ne!(state.ui_context[state.slots.playbar].inner().current_frame, 51.0, "no step button there any more");
    state.run_command("toggle_playbar_step_buttons");
    assert!(state.ui_context[state.slots.playbar].inner().step_buttons);
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
    assert!(state.overlay_marker_instances.is_empty());
    state.run_viewport_menu_action(A::Command("toggle_point_markers"));
    assert!(state.show_point_markers);
    assert_eq!(row(&state), format!("● {label}"));
    assert!(!state.overlay_marker_instances.is_empty(), "the overlay was built");
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

    let band = context_menu::with_state(|m| m.borrow().slider_band(i));
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
    // Without the network the menu has no Network row at its head, so
    // its rows are the ones counted below.
    state.show_network = false;
    state.cursor_x = 300.0;
    state.cursor_y = 200.0;
    state.open_viewport_context_menu();
    let full = context_menu::with_state(|m| m.borrow().content_h);
    context_menu::place(300.0, 0.0, full * 0.5);

    // Over the first row, which is Frame All — an action, not a slider.
    state.cursor_x = context_menu::x() + 20.0;
    state.cursor_y = context_menu::row_y(0) + context_menu::ROW_H * 0.5;
    assert_eq!(state.viewport_menu_actions[0], A::FrameAll);
    state.handle_event(&WindowEvent::CursorMoved {
        position: LocalPosition { x: state.cursor_x as f64, y: state.cursor_y as f64 },
    });
    state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, -3.0) });
    let scroll = context_menu::with_state(|m| m.borrow().scroll);
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
    let resized: Vec<[f32; 3]> = state.drawn_markers(crate::app::MarkerKind::Overlay).iter().map(|v| v.position).collect();
    context_menu::hide();

    // The full path at the same size draws the same spheres.
    state.rebuild_scene_geometry();
    let rebuilt: Vec<[f32; 3]> = state.drawn_markers(crate::app::MarkerKind::Overlay).iter().map(|v| v.position).collect();
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
    state.rebuild_group_markers();
    let radius = |state: &State| {
        state.drawn_markers(crate::app::MarkerKind::Group).iter().map(|v| (v.position[1] - centre[1]).abs()).fold(0.0f32, f32::max)
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
        let pb = state.ui_context[state.slots.playbar].inner_mut();
        pb.start_frame = 1.0;
        pb.end_frame = 48.0;
        pb.current_frame = 17.5;
        pb.playing = true;
        pb.reversed = true;
    }
    assert!(state.run_command("frame_start"));
    {
        let pb = state.ui_context[state.slots.playbar].inner();
        assert!(!pb.playing, "a moving timeline stops");
        assert_eq!(pb.current_frame, 1.0, "and lands on the start frame");
    }
    // Stopped, mid-timeline: a plain jump.
    state.ui_context[state.slots.playbar].inner_mut().current_frame = 30.0;
    state.run_command("frame_start");
    assert_eq!(state.ui_context[state.slots.playbar].inner().current_frame, 1.0);
    assert!(!state.ui_context[state.slots.playbar].inner().playing);
    // Whatever the start frame is.
    state.ui_context[state.slots.playbar].inner_mut().start_frame = 5.0;
    state.ui_context[state.slots.playbar].inner_mut().current_frame = 30.0;
    state.run_command("frame_start");
    assert_eq!(state.ui_context[state.slots.playbar].inner().current_frame, 5.0);
}

/// Either play toggle pauses a moving timeline; direction only chooses
/// what starts from a stop — and the reverse tick runs the frame counter
/// down, wrapping start→end.
#[test]
fn test_reverse_playback_semantics_and_wrap() {
    let mut state = State::new(false);

    state.execute_action(Action::PlayPauseReverse);
    {
        let pb = state.ui_context[state.slots.playbar].inner();
        assert!(pb.playing && pb.reversed, "Down from stopped plays in reverse");
    }
    state.execute_action(Action::PlayPause);
    assert!(!state.ui_context[state.slots.playbar].inner().playing, "Up while reverse-playing pauses");
    state.execute_action(Action::PlayPause);
    {
        let pb = state.ui_context[state.slots.playbar].inner();
        assert!(pb.playing && !pb.reversed, "Up from stopped plays forward");
    }
    state.execute_action(Action::PlayPauseReverse);
    assert!(!state.ui_context[state.slots.playbar].inner().playing, "Down while forward-playing pauses");
    state.execute_action(Action::PlayPauseReverse);
    assert!(state.ui_context[state.slots.playbar].inner().reversed, "Down from stopped is reverse again");
    state.execute_action(Action::PlayPauseReverse);
    assert!(!state.ui_context[state.slots.playbar].inner().playing, "same-direction press pauses");

    {
        let pb = state.ui_context[state.slots.playbar].inner_mut();
        pb.playing = true;
        pb.reversed = true;
        pb.repeat = true;
        pb.current_frame = 1.5;
    }
    let rect = cce_ui::scene::layout::Rect { x: 0.0, y: 0.0, width: 100.0, height: 30.0 };
    let moved =
        cce_ui::widget::Input::tick(state.ui_context[state.slots.playbar].inner_mut(), 0.1, rect);
    assert!(moved, "reverse playback advances the frame");
    assert_eq!(state.ui_context[state.slots.playbar].inner().current_frame, 1.0, "the start frame is played, not stepped over");
    cce_ui::widget::Input::tick(state.ui_context[state.slots.playbar].inner_mut(), 0.1, rect);
    let f = state.ui_context[state.slots.playbar].inner().current_frame;
    assert_eq!(f.round(), 240.0, "running off the start wraps to the end, got {f}");
    assert!(state.ui_context[state.slots.playbar].inner().playing, "the wrap does not stop playback");
}

/// Playback plays every frame: a tick that came late moves the shown
/// frame by one, not by as many as the clock says, so a slow
/// simulation plays every step slower rather than skipping some — in
/// either direction and across the loop. A tick that keeps up still
/// plays at the rate.
#[test]
fn playback_plays_every_frame_however_late_the_tick() {
    use cce_ui::widget::Input;
    let rect = cce_ui::scene::layout::Rect { x: 0.0, y: 0.0, width: 100.0, height: 30.0 };
    let mut adapted = crate::playbar::Playbar::new();
    let pb = adapted.inner_mut();
    (pb.start_frame, pb.end_frame, pb.fps, pb.repeat) = (1.0, 10.0, 24.0, true);
    for reversed in [false, true] {
        pb.current_frame = 5.0;
        pb.begin(reversed);
        let mut shown = vec![5];
        for _ in 0..25 {
            // A quarter of a second a tick: six frames at the rate.
            Input::tick(&mut *pb, 0.25, rect);
            shown.push(pb.current_frame.round() as i32);
        }
        let step = if reversed { -1 } else { 1 };
        for w in shown.windows(2) {
            let expected = (w[0] - 1 + step).rem_euclid(10) + 1;
            assert_eq!(w[1], expected, "reversed {reversed}: {shown:?}");
        }
    }
    // On time: 48 ticks of a 48th of a second at 24 fps is 24 frames.
    pb.current_frame = 1.0;
    pb.end_frame = 100.0;
    pb.begin(false);
    for _ in 0..48 {
        Input::tick(&mut *pb, 1.0 / 48.0, rect);
    }
    assert_eq!(pb.current_frame.round(), 25.0);
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

    // The network lies over the whole window, and the middle button is
    // its pan wherever it is laid out: the graph slides, the camera not.
    let (pivot, turned) = (state.viewport().pivot, orbit(&state));
    let graph = (state.pan_x, state.pan_y);
    press(&mut state, MouseButton::Middle, ElementState::Pressed);
    assert!(state.pan_drag.is_none(), "over the network the middle button is the graph's");
    at(&mut state, cx + 40.0, cy + 10.0);
    press(&mut state, MouseButton::Middle, ElementState::Released);
    assert_ne!((state.pan_x, state.pan_y), graph, "the graph panned");
    assert_eq!(state.viewport().pivot, pivot);

    // With the network hidden it is the camera's.
    state.execute_menu_action("Show Network Pane");
    assert!(!state.show_network);
    at(&mut state, cx, cy);
    press(&mut state, MouseButton::Middle, ElementState::Pressed);
    assert!(state.pan_drag.is_some() && state.pointer_captured(), "a middle press arms the pan");
    at(&mut state, cx + 40.0, cy + 10.0);
    press(&mut state, MouseButton::Middle, ElementState::Released);
    assert!(state.pan_drag.is_none());
    assert_ne!(state.viewport().pivot, pivot, "the middle drag panned");
    assert_eq!(orbit(&state), turned, "and did not turn the camera");
    state.execute_menu_action("Show Network Pane");
    assert!(state.show_network);

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

/// A marker draw instances one white sphere over a marker a point
/// (cce-ui's `SceneDraw::instances`). What that draws — each instance's
/// position added to the sphere, its colour multiplying the white — is
/// the sphere copied to every point as markers were built until
/// 2026-10-06, to the bit: the reference below is that code.
#[test]
fn instanced_markers_draw_what_the_copied_spheres_drew() {
    use crate::geometry::{expand_instances, marker_instances, marker_sphere, Vertex3D};
    let reference = |src: &[Vertex3D], size: f32, color: [f32; 3]| -> Vec<Vertex3D> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let r = size.max(0.001);
        let pi = std::f32::consts::PI;
        for v in src {
            let key = ((v.position[0] * 1000.0).round() as i32, (v.position[1] * 1000.0).round() as i32, (v.position[2] * 1000.0).round() as i32);
            if !seen.insert(key) {
                continue;
            }
            let [cx, cy, cz] = v.position;
            let sp = |theta: f32, phi: f32| [cx + r * theta.sin() * phi.cos(), cy + r * theta.cos(), cz + r * theta.sin() * phi.sin()];
            for lat in 0..4 {
                let (t0, t1) = (pi * lat as f32 / 4.0, pi * (lat + 1) as f32 / 4.0);
                for lon in 0..10 {
                    let (p0, p1) = (2.0 * pi * lon as f32 / 10.0, 2.0 * pi * (lon + 1) as f32 / 10.0);
                    let (p00, p10, p11, p01) = (sp(t0, p0), sp(t1, p0), sp(t1, p1), sp(t0, p1));
                    for position in [p00, p11, p10, p00, p01, p11] {
                        out.push(Vertex3D { position, color });
                    }
                }
            }
        }
        out
    };
    let src: Vec<Vertex3D> = (0..200)
        .map(|i| {
            let t = i as f32 * 0.37;
            Vertex3D { position: [t.sin() * 3.1, (t * 0.7).cos() * -2.3 + 0.001 * (i % 3) as f32, t * 0.05 - 4.0], color: [0.0; 3] }
        })
        .collect();
    for (size, color) in [(0.02f32, [1.0, 0.5, 0.0]), (0.0005, [0.2, 0.9, 0.3]), (0.137, [0.11, 0.22, 0.33])] {
        let drawn = expand_instances(&marker_sphere(size), &marker_instances(&src, color));
        let want = reference(&src, size, color);
        assert_eq!(drawn.len(), want.len());
        for (a, b) in drawn.iter().zip(&want) {
            assert_eq!(a.position.map(f32::to_bits), b.position.map(f32::to_bits));
            assert_eq!(a.color.map(f32::to_bits), b.color.map(f32::to_bits));
        }
    }
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

    // …and all three off the one Detail: one marker instance per POINT
    // (drawn over the marker sphere), one label per point, one whisker
    // pair per point.
    let (markers, labels, normals) =
        crate::render::scene_point_overlays(&geom, true, true, true, 0.02, [1.0, 0.5, 0.0]);
    assert_eq!(labels.len(), points, "one label per point");
    assert_eq!(markers.len(), points);
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
        ("neighbour", "direction"), ("neighbour", "source"), ("distance", "direction"), ("develop", "direction"),
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

    let (headers, columns) = State::geometry_to_spreadsheet_columns(&geom);
    // The cells as the table writes them when it paints them.
    let rows: Vec<Vec<String>> = (0..geom.num_points()).map(|r| columns.iter().map(|c| c.cell(r)).collect()).collect();
    assert_eq!(columns.len(), headers.len(), "a column a header");
    assert!(columns.iter().all(|c| c.len() == geom.num_points()), "a cell a row");
    assert_eq!(
        headers,
        vec![
            "Point", "g:pinned", "Pos.x", "Pos.y", "Pos.z", "Col.r", "Col.g", "Col.b",
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
        let dropdown = state.ui_context[state.slots.param]
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
