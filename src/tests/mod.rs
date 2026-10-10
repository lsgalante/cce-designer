//! cce-designer's tests: headless — `State::new(false)` driven by synthetic
//! events, no GPU or Wayland. They lived in one `mod tests` in main.rs (the
//! crate has no lib.rs) until 2026-10-10; each area is a child module now,
//! and every child sees this module's imports and helpers through
//! `use super::*`.

use crate::test_prelude::*;
use crate::app::{get_next_visible_pane, DesignSettings, FsNode, Project, ProjectViewState};
use crate::slots::{LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX};
use crate::shortcut::{Shortcut, ShortcutManager, Action};
use crate::geometry::{box_detail, detail_vertices};
use crate::detail::{AttribData, AttribKind, AttribType, AttribValue, Class, Detail};
use crate::geometry::sphere_detail;
use crate::remesh::{remesh, Settings};
use crate::volume::Volume;

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
    use crate::slots::{PARAM_IDX, PLAYBAR_IDX, SPREADSHEET_IDX};
    use cce_ui::widget::{context_menu, MouseButton};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.show_spreadsheet = true;
    state.show_playbar = true;
    state.rebuild_positions();
    state.apply_layout();
    let collapse = "Collapse".to_string();

    // The spreadsheet: the plate menu alone.
    let (x, y, w, h) = state.slots.get_dyn(&state.ui_context, SPREADSHEET_IDX).rect();
    press_at(&mut state, x + w - 6.0, y + h - 6.0, MouseButton::Right);
    assert_eq!(state.plate_menu_slot, Some(SPREADSHEET_IDX));
    assert!(state.plate_menu_actions.contains(&PlateMenuAction::Collapse));
    press_at(&mut state, 2.0, 2.0, MouseButton::Left);
    assert!(!state.plate_menu_open(), "a press outside dismisses it");

    // The params HUD, with its plate on, off a row and where no plate
    // covers it: its menu alone, which detaches and does not collapse.
    // A node selected, so the HUD has rows and a plate fitted to them.
    let mut redraw = false;
    let slot = geo(&state.fs_root).children.iter().position(|c| c.node_type == "sphere").unwrap();
    state.apply_action(crate::app::McpAction::Select { slot }, &mut redraw).unwrap();
    state.apply_layout();
    state.params_plate = true;
    let (x, y, w, h) = state.slots.get_dyn(&state.ui_context, PARAM_IDX).rect();
    let free = (0..(h as i32))
        .rev()
        .map(|dy| (x + w - 6.0, y + dy as f32))
        .find(|&(px, py)| state.params_claims(px, py) && state.param_row_at(px, py).is_none())
        .expect("a free spot on the HUD");
    press_at(&mut state, free.0, free.1, MouseButton::Right);
    assert_eq!(state.plate_menu_slot, Some(PARAM_IDX));
    assert!(state.plate_menu_actions.contains(&PlateMenuAction::Detach));
    assert!(!state.plate_menu_actions.contains(&PlateMenuAction::Collapse), "the HUD does not collapse");
    press_at(&mut state, 2.0, 2.0, MouseButton::Left);
    state.params_plate = false;

    // The network: its menu is the viewport menu's Network page (empty
    // graph space is the scene's), and it has no Plate page — the
    // network has no plate and is in no dock.
    let (cx, cy, cw, ch) = state.positions[crate::slots::CONTENT_IDX];
    let (px, py) = (cx + cw * 0.3, cy + ch * 0.6);
    assert!(state.graph().node_at(px, py).is_none() && !state.over_floating_pane_at(px, py));
    press_at(&mut state, px, py, MouseButton::Right);
    assert!(state.viewport_menu_open());
    press_at(&mut state, context_menu::x() + 8.0, context_menu::row_y(0) + 4.0, MouseButton::Left);
    assert!(state.network_menu_active, "the Network row turned the menu into the network's");
    let options = cce_ui::widget::context_menu::options();
    assert_eq!(options.first().map(String::as_str), Some("Add Node"));
    assert!(!options.iter().any(|o| o == "Plate" || *o == collapse || o.starts_with("Move To")), "no plate rows: {options:?}");
    press_at(&mut state, 2.0, 2.0, MouseButton::Left);

    // The playbar: its transport, then the Plate page row.
    let (x, y, w, h) = state.positions[PLAYBAR_IDX];
    press_at(&mut state, x + w * 0.5, y + h * 0.5, MouseButton::Right);
    let options = context_menu::options();
    assert!(!options.contains(&collapse), "{options:?}");
    assert!(state.playbar_menu_actions.contains(&crate::app::PlaybarMenuAction::PlatePage));
    let plate = options.iter().position(|o| o == "Plate").unwrap();
    press_at(&mut state, context_menu::x() + 8.0, context_menu::row_y(plate) + 4.0, MouseButton::Left);
    assert_eq!(state.plate_menu_slot, Some(PLAYBAR_IDX));
    assert!(state.plate_menu_actions.contains(&PlateMenuAction::Collapse));
    assert_eq!(context_menu::back_title().as_deref(), Some("Playbar"));
    press_at(&mut state, 2.0, 2.0, MouseButton::Left);
}

/// There are no docks (since 2026-10-07): the spreadsheet is the strip
/// along the bottom, a gap in from the sides, and nothing moves it there
/// or elsewhere; the plate menus hold the window actions alone; only the
/// spreadsheet's top edge resizes it; and an older save that had moved it
/// into a side dock opens with it along the bottom.
#[test]
fn the_plates_have_no_docks() {
    use crate::plate_menu::PlateMenuAction;
    use crate::slots::{NETWORK_PANEL_IDX, PARAM_IDX, PLAYBAR_IDX, SPREADSHEET_IDX};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.execute_menu_action("Show Spreadsheet Pane");
    if !state.show_playbar {
        state.execute_menu_action("Show Playbar Pane");
    }
    let (x, y, w, h) = state.positions[SPREADSHEET_IDX];
    let table = state.slots.spreadsheet(&state.ui_context).content_width().ceil();
    assert_eq!((x, w), (18.0, table.clamp(State::SPREADSHEET_MIN_W, 1600.0 - 36.0)), "as wide as its table, within the window");
    assert_eq!(y + h, state.positions[PLAYBAR_IDX].1 - 18.0, "a gap above the playbar");

    for idx in [SPREADSHEET_IDX, PARAM_IDX, NETWORK_PANEL_IDX, PLAYBAR_IDX] {
        for (label, action) in state.plate_menu_rows(idx).0.iter().zip(state.plate_menu_rows(idx).1) {
            assert!(
                matches!(action, PlateMenuAction::Collapse | PlateMenuAction::Expand | PlateMenuAction::Detach | PlateMenuAction::Reattach),
                "{label} on plate {idx}"
            );
        }
    }

    // Only the top edge resizes: the sides tucked under a dock's plate.
    assert!(state.on_spreadsheet_resize_edge(x + w * 0.5, y));
    assert!(!state.on_spreadsheet_resize_edge(x, y + h * 0.5));
    assert!(!state.on_spreadsheet_resize_edge(x + w, y + h * 0.5));

    // An older save with the spreadsheet moved to the left dock, its
    // width and tucks recorded: it loads along the bottom, and the HUD
    // width it recorded still loads.
    let dir = std::env::temp_dir().join(format!("cce-designer-no-docks-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    state.save_to_file(&dir).expect("save");
    let file = dir.join("state.json");
    let mut json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    json["view_state"]["dock_tabs"] = serde_json::json!([["spreadsheet"], [], []]);
    json["view_state"]["plates"] = serde_json::json!({
        "network_width": 0.4, "params_width": 0.25, "spreadsheet_height": 0.3,
        "spreadsheet_inset_left": 0.1, "spreadsheet_inset_right": 0.0,
    });
    fs::write(&file, serde_json::to_string(&json).unwrap()).unwrap();
    let mut b = State::new(false);
    b.resize(1600.0, 900.0, 1.0);
    b.load_from_file(&dir).expect("load");
    let (bx, _, bw, _) = b.positions[SPREADSHEET_IDX];
    assert_eq!((bx, bw), (18.0, b.floating_spreadsheet_rect().2), "along the bottom");
    assert!((b.params_hud_width - 400.0).abs() < 0.5, "an older save's params width is the HUD's: {}", b.params_hud_width);
    assert!((b.floating_spreadsheet_height - 270.0).abs() < 0.5);
    let _ = fs::remove_dir_all(&dir);
}

/// Collapse must actually reclaim the plate AND take its body with it, and
/// expanding must put both back — a stub that still hosts a full-height
/// graph would paint the pane over the viewport it just freed.
#[test]
fn test_collapse_shrinks_the_plate_and_restores_it() {
    use crate::plate_menu::STUB_H;
    use crate::slots::{NETWORK_PANEL_IDX, SPREADSHEET_IDX};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.set_pane_collapsed(NETWORK_PANEL_IDX, true);
    assert!(!state.pane_is_collapsed(NETWORK_PANEL_IDX), "the network does not collapse");
    state.execute_menu_action("Show Spreadsheet Pane");

    let (_, _, _, full_h) = state.slots.get_dyn(&state.ui_context, SPREADSHEET_IDX).rect();
    assert!(full_h > STUB_H, "the plate starts taller than a stub");

    state.set_pane_collapsed(SPREADSHEET_IDX, true);
    let (_, _, _, stub_h) = state.slots.get_dyn(&state.ui_context, SPREADSHEET_IDX).rect();
    assert_eq!(stub_h, STUB_H, "collapsed plate is not the stub height");
    // A right press on the stub offers Expand; a left press expands it.
    let (x, y, w, h) = state.slots.get_dyn(&state.ui_context, SPREADSHEET_IDX).rect();
    press_at(&mut state, x + w * 0.5, y + h * 0.5, cce_ui::widget::MouseButton::Right);
    assert!(state.plate_menu_actions.contains(&crate::plate_menu::PlateMenuAction::Expand));
    state.close_plate_menu();
    press_at(&mut state, x + w * 0.5, y + h * 0.5, cce_ui::widget::MouseButton::Left);
    let (_, _, _, back_h) = state.slots.get_dyn(&state.ui_context, SPREADSHEET_IDX).rect();
    assert_eq!(back_h, full_h, "expanding did not restore the plate height");
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
        let visible = child.slots.get_dyn(&child.ui_context, i).visible();
        assert_eq!(visible, i == PARAM_IDX, "slot {i} visibility in a detached window");
    }
    let (x, y, w, h) = child.slots.get_dyn(&child.ui_context, PARAM_IDX).rect();
    assert_eq!((x, y), (DETACHED_MARGIN, DETACHED_MARGIN));
    assert_eq!(w, 600.0 - 2.0 * DETACHED_MARGIN);
    assert_eq!(h, 400.0 - 2.0 * DETACHED_MARGIN);

    // Parent: the window that handed the pane out keeps a STUB, because the
    // stub's plate menu is the only way to reattach.
    use crate::plate_menu::STUB_H;
    let mut parent = State::new(false);
    parent.resize(1600.0, 900.0, 1.0);
    assert!(parent.slots.get_dyn(&parent.ui_context, PARAM_IDX).visible(), "params starts in the parent");
    let (_, _, _, full_h) = parent.slots.get_dyn(&parent.ui_context, PARAM_IDX).rect();

    parent.detached_panes[PARAM_IDX] = true;
    parent.rebuild_positions();
    parent.apply_layout();

    let (_, _, _, stub_h) = parent.slots.get_dyn(&parent.ui_context, PARAM_IDX).rect();
    assert!(full_h > stub_h, "detaching did not shrink the pane in the parent");
    assert_eq!(stub_h, STUB_H, "the parent's leftover is not a stub");
    let (sx, sy, sw, sh) = parent.slots.get_dyn(&parent.ui_context, PARAM_IDX).rect();
    press_at(&mut parent, sx + sw * 0.5, sy + sh * 0.5, cce_ui::widget::MouseButton::Right);
    assert_eq!(parent.plate_menu_actions, vec![crate::plate_menu::PlateMenuAction::Reattach],
        "the stub's right press offers Reattach — nothing else can reattach the pane");
    parent.close_plate_menu();
    // Collapsed and detached stubs must not read the same.
    let label = parent.pane_stub_label(PARAM_IDX).expect("a detached pane is stubbed");
    assert!(label.contains("detached"), "stub does not say the pane is detached: {label}");
    assert!(parent.slots.get_dyn(&parent.ui_context, VIEWPORT_IDX).visible(), "the rest of the parent survived");
}

/// The network has no plate, so a right press on empty
/// graph space is the scene's and opens the VIEWPORT menu, so the
/// network's own menu is a page of it: a Network row at its head that
/// turns into the network menu under a band back to the viewport's.
/// Turning there focuses the network and puts the grid cursor on the
/// cell the menu was opened over, where Add Node will place; a menu
/// opened over no network area moves no cursor. In the circular pane
/// there is no such row: its empty space opens the network's menu itself.
#[test]
fn the_network_menu_is_a_page_of_the_viewport_menu_without_the_plate() {
    use crate::app::{NetworkMenuAction, ViewportMenuAction as A};
    use crate::menu_page::MenuOrigin;
    use crate::slots::LEFT_MENUBAR_IDX;
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::context_menu::{self, PageTurn};
    use cce_ui::widget::{ElementState, MouseButton};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    assert!(state.network_overlay(), "the network overlays the scene");
    let press = |state: &mut State, x: f32, y: f32, b: MouseButton| {
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: b });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: b });
    };

    // An empty cell of the graph, away from the cursor and the HUD.
    let (cx, cy, cw, ch) = state.positions[crate::slots::CONTENT_IDX];
    let (px, py) = (cx + cw * 0.3, cy + ch * 0.6);
    assert!(state.graph().node_at(px, py).is_none() && !state.over_floating_pane_at(px, py));
    let cell = state.cell_at(px, py);
    assert_ne!((state.grid_cursor_col, state.grid_cursor_row), cell, "pick a cell the cursor is not on");
    press(&mut state, px, py, MouseButton::Right);
    assert!(state.viewport_menu_open(), "empty graph space is the scene's");
    assert_eq!(state.viewport_menu_actions.first(), Some(&A::NetworkPage));
    assert!(context_menu::leads_to_page(0), "a page row");
    assert_eq!(context_menu::options().first().map(String::as_str), Some("Network"));
    assert_ne!((state.grid_cursor_col, state.grid_cursor_row), cell, "opening the menu moves no cursor");

    // The turn: the network menu, under a band back to the viewport's.
    assert!(state.run_menu_turn(PageTurn::Into(0)));
    assert_eq!(state.open_menu_origin(), Some(MenuOrigin::Network));
    assert_eq!(state.network_menu_actions.first(), Some(&NetworkMenuAction::Command("add_node")));
    assert_eq!(state.network_menu_from, Some(MenuOrigin::Viewport));
    assert_eq!((state.grid_cursor_col, state.grid_cursor_row), cell, "the cursor is on the cell pressed");
    assert_eq!(state.focused_pane, LEFT_MENUBAR_IDX, "the network has focus");

    // Back to the viewport menu, then forward again by its row.
    assert!(state.run_menu_turn(PageTurn::Back));
    assert_eq!(state.open_menu_origin(), Some(MenuOrigin::Viewport));
    assert_eq!(state.network_menu_from, None);
    state.run_viewport_menu_action(A::NetworkPage);
    assert_eq!(state.open_menu_origin(), Some(MenuOrigin::Network));

    // Add Node from there, and back out of the dialog to the network
    // menu, which still goes back to the viewport's.
    let add = state.network_menu_actions.iter().position(|a| *a == NetworkMenuAction::Command("add_node")).unwrap();
    assert!(state.run_menu_turn(PageTurn::Into(add)));
    assert!(state.dialog_visible());
    assert!(state.dialog_back());
    assert_eq!(state.open_menu_origin(), Some(MenuOrigin::Network));
    assert!(state.run_menu_turn(PageTurn::Back));
    assert_eq!(state.open_menu_origin(), Some(MenuOrigin::Viewport));
    state.close_viewport_menu();

    // The circular pane takes its own empty space: no Network row.
    state.circular_network_pane = true;
    assert!(!state.viewport_menu_rows_of(None).1.contains(&A::NetworkPage));
}

/// With the network's plate off its graph spans the window, so a scroll
/// cannot go to it by rect: a gesture begun on empty space orbits the
/// camera, one begun on a node pans the graph, and the target is held
/// to the gesture's end — a pan slides the node out from under the
/// pointer, and the rest of the swipe must not become an orbit.
#[test]
fn a_scroll_over_the_plateless_network_orbits_unless_it_begins_on_a_node() {
    use crate::slots::{LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX};
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::scroll_motion::ScrollPhase;
    use cce_ui::widget::{MouseScrollDelta, Position};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    assert!(state.network_overlay(), "the network overlays the scene");
    state.set_active_camera("Default Camera");
    let at = |state: &mut State, x: f32, y: f32| {
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
    };
    let wheel = |state: &mut State| {
        state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 0.0, y: 30.0 }) });
    };
    let orbit = |state: &State| (state.viewport().rotation_x, state.viewport().rotation_y);

    // Empty space: the camera turns and the graph stays.
    let (cx, cy, cw, ch) = state.positions[crate::slots::CONTENT_IDX];
    let (ex, ey) = (cx + cw * 0.3, cy + ch * 0.6);
    assert!(state.graph().node_at(ex, ey).is_none() && !state.over_floating_pane_at(ex, ey));
    at(&mut state, ex, ey);
    let (turned, pan) = (orbit(&state), (state.pan_x, state.pan_y));
    wheel(&mut state);
    assert_ne!(orbit(&state), turned, "a scroll over empty space orbits");
    assert_eq!((state.pan_x, state.pan_y), pan, "and does not pan the graph");
    assert_eq!(state.focused_pane, RIGHT_MENUBAR_IDX);

    // On a node: the graph's, not the camera's.
    state.overlay_wheel = None;
    let find_node = |state: &State| state
        .current_dir()
        .children
        .iter()
        .map(|n| state.cell_center(n.position.0 as i32, n.position.1 as i32))
        .find(|&(x, y)| !state.over_floating_pane_at(x, y) && state.graph().node_at(x, y).is_some())
        .expect("a node in the clear");
    let (nx, ny) = find_node(&state);
    at(&mut state, nx, ny);
    let turned = orbit(&state);
    wheel(&mut state);
    assert_eq!(orbit(&state), turned, "a scroll on a node does not orbit");
    assert_eq!(state.focused_pane, LEFT_MENUBAR_IDX, "it is the network's");

    // The latch: the node slides from under the pointer, and the
    // gesture is still the graph's to its lift; the next one is not.
    // (That scroll panned the graph: find the node again.)
    let (nx, ny) = find_node(&state);
    at(&mut state, nx, ny);
    state.overlay_wheel = None;
    assert!(state.overlay_wheel_to_graph(ScrollPhase::Finger));
    state.pan_x += cw;
    state.sync_grid_settings();
    assert!(state.graph().node_at(nx, ny).is_none(), "the node moved away");
    assert!(state.overlay_wheel_to_graph(ScrollPhase::Finger), "held mid-gesture");
    assert!(state.overlay_wheel_to_graph(ScrollPhase::FingerEnd), "held to the lift");
    assert!(!state.overlay_wheel_to_graph(ScrollPhase::Finger), "a new gesture over empty space is the scene's");
}

/// The network plate is retired; a settings file or a project display
/// block from when it was a switch still says `network_plate`, and must
/// still load — the key is ignored, not an error that would read the
/// whole block as the defaults.
#[test]
fn a_retired_network_plate_key_still_loads() {
    let state = State::new(false);
    let mut json = serde_json::to_value(state.display_settings()).unwrap();
    json["viewport"]["network_plate"] = serde_json::json!(true);
    json["viewport"]["params_plate"] = serde_json::json!(false);
    let d: crate::app::DisplaySettings = serde_json::from_value(json).expect("a display block naming network_plate loads");
    assert!(!d.viewport.params_plate, "and the rest of it is read");
}

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

mod ui;
mod param_rows;
mod volumes;
mod switch_and_refs;
mod hull_scatter;
mod mesh_export;
mod modelling;
mod surface_development;
mod solver_contract;
mod detail_container;
mod alt_d_dialog;
mod wrangle;
mod springs;
mod collide;
mod viewport_2d;
mod image_handles;
mod image_trace;

// Helpers more than one area builds its fixtures with.
use detail_container::quad_grid;
use modelling::modelling_root;
use surface_development::phase3_node;
use switch_and_refs::ref_node;
use viewport_2d::{image_node, image_root, state_showing_image};
use switch_and_refs::eval;
use modelling::eval_node;
use alt_d_dialog::{key_press, typed};
use volumes::box_mesh;
