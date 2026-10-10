//! The Alt+D dialog (src/dialog.rs).

use super::*;

/// A key, as the dialog's handler expects one.
pub(super) fn key_press(key: Key) -> cce_ui::widget::KeyEvent {
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

pub(super) fn typed(c: &str) -> cce_ui::widget::KeyEvent {
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
        state.ui_context[state.slots.dialog].rows.iter().filter(|r| !r.id.starts_with(crate::dialog::SETTING_ROW_PREFIX) && r.id != crate::dialog::ZOOM_ROW_ID && !r.id.starts_with(crate::dialog::CAMERA_ROW_PREFIX)).count(),
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
    assert_eq!(state.ui_context[state.slots.dialog].query, "desel");
    assert_eq!(
        state.ui_context[state.slots.dialog].selected_id(),
        Some("deselect"),
        "rows: {:?}",
        state.ui_context[state.slots.dialog].rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>()
    );
    let row = &state.ui_context[state.slots.dialog].rows[state.ui_context[state.slots.dialog].selected];
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
    assert_eq!(state.ui_context[state.slots.dialog].selected_id(), Some("toggle_square_viewport"));
    let before = state.square_viewport;
    let row = state.ui_context[state.slots.dialog].rows[state.ui_context[state.slots.dialog].selected].clone();
    assert_eq!(row.toggle(), Some(before), "the switch shows the live value");

    state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
    assert_eq!(state.square_viewport, !before, "Enter ran the command");
    assert!(state.dialog_visible(), "and the dialog stayed up");
    assert_eq!(state.ui_context[state.slots.dialog].query, "squa", "with its query intact");
    assert_eq!(state.ui_context[state.slots.dialog].selected_id(), Some("toggle_square_viewport"), "and its selection");
    let row = &state.ui_context[state.slots.dialog].rows[state.ui_context[state.slots.dialog].selected];
    assert_eq!(row.toggle(), Some(!before), "the switch moved with the value");

    // And back again, without leaving.
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
    assert_eq!(state.square_viewport, before);
    assert!(state.dialog_visible());
    assert_eq!(state.ui_context[state.slots.dialog].rows[state.ui_context[state.slots.dialog].selected].toggle(), Some(before));

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
    let rows = &state.ui_context[state.slots.dialog].rows;
    assert_eq!(rows[0].id, ZOOM_ROW_ID, "the zoom row heads the network list");
    assert!((rows[0].slider_value().unwrap() - state.zoom_percent()).abs() < 1e-3);
    assert!(rows.iter().filter(|r| r.id == ZOOM_ROW_ID).count() == 1);

    // The arrows nudge the zoom, the dialog stays up, the row follows.
    let before = state.zoom_percent();
    state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowRight)));
    assert!(state.dialog_visible());
    assert!(state.zoom_percent() > before, "right arrow zooms in");
    assert!((state.ui_context[state.slots.dialog].rows[0].slider_value().unwrap() - state.zoom_percent()).abs() < 1e-3);
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
    assert!((state.ui_context[state.slots.dialog].rows[0].slider_value().unwrap() - state.zoom_percent()).abs() < 1e-3);
    state.set_zoom_percent(100.0);

    // A query that does not match "Zoom" drops the row.
    for c in ["s", "a", "v"] {
        state.dialog_key_input(&key_press(Key::Character(c.into())));
    }
    assert!(state.ui_context[state.slots.dialog].rows.iter().all(|r| r.id != ZOOM_ROW_ID));
    state.close_dialog();

    // Another pane focused: no slider row at all.
    state.focused_pane = crate::slots::RIGHT_MENUBAR_IDX;
    state.run_command("command_palette");
    assert!(state.ui_context[state.slots.dialog].rows.iter().all(|r| r.id != ZOOM_ROW_ID));
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
    let d = ctx.insert(Dialog::new());
    ctx[d].set_visible(true);
    WidgetHost::set_rect(&mut ctx[d], 0.0, 0.0, 520.0, 420.0);
    let plain = |i: usize| Row { id: format!("c{i}"), label: format!("Command {i}"), chord: String::new(), control: None, truncate_head: false };
    ctx[d].set_rows(vec![
        Row { id: "zoom_level".into(), label: "Zoom".into(), chord: String::new(), control: Some(Control::Slider { value: 100.0, min: 20.0, max: 320.0, dec: 0, step: 10.0, suffix: "%" }), truncate_head: false },
        plain(1),
        plain(2),
    ]);
    ctx[d].set_page(10);
    ctx[d].set_occluding(false);

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
    assert!(ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Pressed, px, row_y, ctx)).unwrap());
    assert!(ctx[d].slider_dragging());
    let v = ctx[d].take_slider_change().map(|(_, v)| v).expect("a press on the band reports a value");
    assert!((v - (20.0 + 0.75 * 300.0)).abs() < 3.0, "value {v} is not three quarters of the range");
    assert_eq!(ctx[d].rows[0].slider_value(), Some(v), "the row follows");
    assert_eq!(ctx[d].take_activated(), None, "the band is a control, not a pick");
    assert_eq!(ctx[d].take_slider_change(), None, "reported once");
    ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Released, px, row_y, ctx)).unwrap();
    assert!(!ctx[d].slider_dragging());

    // A press on the row's label end selects it and reports nothing.
    assert!(ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Pressed, 30.0, row_y, ctx)).unwrap());
    assert_eq!(ctx[d].selected, 0);
    assert!(!ctx[d].slider_dragging());
    assert_eq!(ctx[d].take_slider_change(), None);
    assert_eq!(ctx[d].take_activated(), None);
    ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Released, 30.0, row_y, ctx)).unwrap();

    // An ordinary row still picks.
    assert!(ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Pressed, 30.0, row_y + 24.0, ctx)).unwrap());
    assert_eq!(ctx[d].take_activated().as_deref(), Some("c1"));

    // The wheel over the control turns the slider — a notch up is 2% of
    // the range more, as on the toolkit's slider — and over the label
    // end it scrolls the list instead, reporting nothing.
    let before = ctx[d].rows[0].slider_value().unwrap();
    let wheel = |x: f32, y: f32| cce_ui::widget::Event::MouseWheel {
        delta: cce_ui::widget::MouseScrollDelta::LineDelta(0.0, 1.0),
        x, y, local_x: x, local_y: y,
    };
    ctx.note_scroll_event();
    assert!(ctx.lend_h(d, |w, ctx| w.handle_event(&wheel(band_x + 10.0, row_y), ctx)).unwrap());
    let v = ctx[d].take_slider_change().map(|(_, v)| v).expect("a wheel over the band reports a value");
    assert!((v - (before + 0.02 * 300.0)).abs() < 1e-3, "notch up: {before} -> {v}");
    ctx.note_scroll_event();
    ctx.lend_h(d, |w, ctx| w.handle_event(&wheel(30.0, row_y), ctx)).unwrap();
    assert_eq!(ctx[d].take_slider_change(), None, "over the label the wheel is the list's");
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
    let d = ctx.insert(Dialog::new());
    ctx[d].set_visible(true);
    WidgetHost::set_rect(&mut ctx[d], 0.0, 0.0, 520.0, 420.0);
    ctx[d].set_rows(vec![
        Row { id: "zoom_level".into(), label: "Zoom".into(), chord: String::new(), control: Some(Control::Slider { value: 100.0, min: 20.0, max: 320.0, dec: 0, step: 10.0, suffix: "%" }), truncate_head: false },
        Row { id: "show_grid".into(), label: "Show Grid".into(), chord: "Ctrl+G".into(), control: Some(Control::Toggle(true)), truncate_head: false },
    ]);
    ctx[d].set_page(10);
    ctx[d].set_occluding(false);

    let row_y = 12.0 + 30.0 + 8.0 + 12.0;
    let row_right = 520.0 - 12.0 - 8.0;
    let band_right = row_right - (TOGGLE_W + 12.0);
    assert!(band_right < row_right, "the switch column pulls the band in");

    // The band's last pixel is the range's top; the switch column past it
    // is not the band's.
    assert!(ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Pressed, band_right - 1.0, row_y, ctx)).unwrap());
    assert!(ctx[d].slider_dragging(), "the band reaches the chord column's edge");
    let v = ctx[d].take_slider_change().map(|(_, v)| v).expect("a press on the band reports a value");
    assert!((v - 320.0).abs() < 4.0, "the band's end is the range's end, got {v}");
    ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Released, band_right - 1.0, row_y, ctx)).unwrap();

    ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Pressed, band_right + 4.0, row_y, ctx)).unwrap();
    assert!(!ctx[d].slider_dragging(), "past the chord column the row is not the band");
    assert_eq!(ctx[d].take_slider_change(), None);
    ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Released, band_right + 4.0, row_y, ctx)).unwrap();

    // The readout lane sits ahead of the band and takes no hold either.
    let lane_x = row_right - SLIDER_W - 60.0 - 8.0;
    ctx.lend_h(d, |w, ctx| w.mouse_input(MouseButton::Left, ElementState::Pressed, lane_x + 4.0, row_y, ctx)).unwrap();
    assert!(!ctx[d].slider_dragging(), "the readout is a readout, not a track");
    assert_eq!(ctx[d].take_slider_change(), None);
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
    let hovered = |state: &State| state.slots.get_dyn(&state.ui_context, NETWORK_PANEL_IDX).base().hovered;
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
    state.app_drag = Some(crate::app::AppDrag::HudResize { start_w: 300.0, start_mouse_x: cx });
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
    state.ui_context[state.slots.dialog].query = "geometry opacity".into();
    state.refresh_dialog_rows();
    let row = setting_row_id("Geometry Opacity");
    assert_eq!(state.ui_context[state.slots.dialog].rows.first().map(|r| r.id.as_str()), Some(row.as_str()), "the setting row ranks first");
    let has_toggle = state.ui_context[state.slots.dialog].rows.iter().any(|r| matches!(r.control, Some(Control::Toggle(_))));
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
    assert!(state.ui_context[state.slots.dialog].slider_dragging(), "the press took the band");
    assert!((state.geo_opacity - 0.5).abs() < 0.02, "{}", state.geo_opacity);
    assert_eq!(state.rt_geometry_version, version, "a draw-time value re-evaluated the graph");
    assert!((saved(&path) - 1.0).abs() < 1e-3, "written mid-drag");

    // A motion lands the value live and re-reads the row in place.
    at(&mut state, 0.25);
    assert!((state.geo_opacity - 0.25).abs() < 0.02, "{}", state.geo_opacity);
    let shown = state.ui_context[state.slots.dialog].rows[0].slider_value().expect("a slider row");
    assert!((shown - state.geo_opacity).abs() < 1e-3, "the row shows {shown}, the field holds {}", state.geo_opacity);
    assert_eq!(state.rt_geometry_version, version, "a drag motion re-evaluated the graph");
    assert!((saved(&path) - 1.0).abs() < 1e-3, "written mid-drag");

    // The release writes the file once.
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
    assert!(!state.ui_context[state.slots.dialog].slider_dragging());
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
    assert!(state.ui_context[state.slots.dialog].rows.is_empty(), "nothing matches 'zzz'");
    for _ in 0..3 {
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Backspace)));
    }
    assert_eq!(state.ui_context[state.slots.dialog].query, "");
    assert_eq!(
        state.ui_context[state.slots.dialog].rows.iter().filter(|r| !r.id.starts_with(crate::dialog::SETTING_ROW_PREFIX) && r.id != crate::dialog::ZOOM_ROW_ID && !r.id.starts_with(crate::dialog::CAMERA_ROW_PREFIX)).count(),
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
    assert_eq!(state.ui_context[state.slots.dialog].query, "l");
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
    let units = state.ui_context[state.dialog_dd()].options.clone();
    assert_eq!(units[state.ui_context[state.dialog_dd()].selected], "mm");
    let (tx, ty, tw, th) = state.ui_context[state.dialog_dd()].rect();
    assert!(vx >= tx && vx < tx + tw && vy >= ty - 4.0 && vy < ty + th, "laid out on the band its value was drawn in");

    // The plate grows out of the trigger into the list. Judged by where
    // it ends, not by a reading taken as it opens: the growth runs on
    // the wall clock, and a slow press had already finished it.
    let grown = |state: &State| state.ui_context[state.dialog_dd()].popover_rect().map(|r| r.3).unwrap_or(0.0);
    std::thread::sleep(std::time::Duration::from_millis(250));
    state.tick_frame(0.25);
    assert!(grown(&state) > th + 24.0, "the trigger {th} grew to {}", grown(&state));
    // Registered after the dialog, so the rows under it are clamped
    // and its labels are not.
    let _ = state.collect_display_list();
    let pops = &state.ui_context.active_popovers;
    let dialog_at = pops.iter().position(|&p| p == state.ui_context[state.slots.dialog].base().id()).expect("the dialog");
    let dd_id = state.ui_context[state.dialog_dd()].base().id();
    let dd_at = pops.iter().position(|&p| p == dd_id).expect("the dropdown");
    assert!(dd_at > dialog_at);
    // And resolvable, which is what the engine's clamp walks: an id the
    // tree has dropped is skipped in silence.
    assert!(state.ui_context.tree.is_registered(dd_id), "the dropdown is in the widget tree");

    // Down, Enter: the next unit, the dialog still up.
    state.dialog_key_input(&key_press(Key::Named(NamedKey::ArrowDown)));
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
    assert_eq!(state.world_unit.suffix(), units[1]);
    assert!(!state.dialog_dropdown_open() && state.dialog_visible());

    // A press on a row of the list picks that row.
    state.open_dialog_dropdown(&setting_row_id("World Unit"));
    let k = units.iter().position(|u| u == "in").expect("inches");
    let (rx, ry, _, _) = state.ui_context[state.dialog_dd()].popover_geom(cce_ui::scene::layout::Rect { x: tx, y: ty, width: tw, height: th });
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
    assert!(!state.ui_context[state.dialog_dd()].open, "a closed dialog leaves no plate behind");
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
    let rows = &state.ui_context[state.slots.dialog].rows;
    let toggle = rows.iter().position(|r| r.id == "toggle_grid").expect("a Show Grid row");
    assert!(matches!(rows[toggle].control, Some(Control::Toggle(_))));
    let (dx, dy, dw, dh) = state.positions[DIALOG_IDX];
    assert!(dw > 0.0 && dh > 0.0);
    let at = |state: &mut State, x: f32, y: f32| {
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        state.ui_context[state.slots.dialog].hovered_control()
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
    while !state.ui_context[state.slots.dialog].query.is_empty() {
        state.dialog_key_input(&key_press(Key::Named(NamedKey::Backspace)));
    }
    for c in ["g", "e", "o", "m", "e", "t", "r", "y"] {
        state.dialog_key_input(&typed(c));
    }
    let slider = state.ui_context[state.slots.dialog].rows.iter().position(|r| r.id == setting_row_id("Geometry Opacity")).expect("a slider row");
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
    let ids: Vec<String> = state.ui_context[state.slots.dialog].rows.iter().map(|r| r.id.clone()).collect();
    for s in SETTINGS {
        assert!(ids.contains(&setting_row_id(s.label)), "'{}' has no row", s.label);
    }
    let control = |label: &str| {
        state.ui_context[state.slots.dialog].rows.iter().find(|r| r.id == setting_row_id(label)).and_then(|r| r.control.clone())
    };
    assert!(matches!(control("Grid Color"), Some(Control::Color { .. })));
    assert!(matches!(control("Grid Thickness"), Some(Control::Slider { dec: 0, .. })), "a spin is a whole-number slider");
    assert!(matches!(control("Geometry Opacity"), Some(Control::Slider { dec: 2, .. })));
    assert!(matches!(control("World Unit"), Some(Control::Choice { .. })));
    assert!(matches!(control("Group Marker Size"), Some(Control::Slider { .. })));
    // A command's switch is its own row; the settings table lists none
    // of them twice.
    assert!(control("Show Grid").is_none());
    assert!(state.ui_context[state.slots.dialog].rows.iter().any(|r| r.id == "toggle_grid" && r.toggle().is_some()));

    // Tab does not move anywhere, and the dialog stays.
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Tab)));
    assert!(state.dialog_visible());

    // The filter ranks a setting like a command: "gridc" finds Grid
    // Color ahead of everything.
    for c in ["g", "r", "i", "d", "c"] {
        state.dialog_key_input(&typed(c));
    }
    assert_eq!(state.ui_context[state.slots.dialog].selected_id(), Some(setting_row_id("Grid Color").as_str()));
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
    let row = state.ui_context[state.slots.dialog].rows.iter().find(|r| r.id == setting_row_id("Grid Thickness")).unwrap();
    assert!(matches!(row.control, Some(Control::Slider { value, .. }) if (value - 40.0).abs() < 1e-6), "the row re-read the value");

    // The arrows work the selected row's control in place: a choice
    // steps, a slider nudges, each landing on the live state.
    let unit_row = state.ui_context[state.slots.dialog].rows.iter().position(|r| r.id == setting_row_id("World Unit")).unwrap();
    state.ui_context[state.slots.dialog].selected = unit_row;
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

    let scale_row = state.ui_context[state.slots.dialog].rows.iter().position(|r| r.id == setting_row_id("Group Marker Size")).unwrap();
    state.ui_context[state.slots.dialog].selected = scale_row;
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
    let rows: Vec<String> = state.ui_context[state.slots.dialog].rows.iter().map(|r| r.id.clone()).collect();
    let id_a = format!("{RECENT_ROW_PREFIX}{}", a.display());
    let id_b = format!("{RECENT_ROW_PREFIX}{}", b.display());
    let ia = rows.iter().position(|r| *r == id_a).expect("no row for the newest recent project");
    let ib = rows.iter().position(|r| *r == id_b).expect("no row for the older recent project");
    assert!(ia < ib, "the recent list is not in most-recent-first order");
    // The row shows the path, truncated from the LEFT — the tail is what
    // identifies a project — with the file name in the chord column.
    let row = &state.ui_context[state.slots.dialog].rows[ia];
    assert_eq!(row.label, a.display().to_string());
    assert_eq!(row.chord, "cce-recent-alpha");
    assert!(row.truncate_head);
    // And it ranks against the path text like any other row.
    state.ui_context[state.slots.dialog].query = "beta".to_string();
    state.refresh_dialog_rows();
    let rows: Vec<String> = state.ui_context[state.slots.dialog].rows.iter().map(|r| r.id.clone()).collect();
    assert!(rows.contains(&id_b) && !rows.contains(&id_a), "{rows:?}");
    state.close_dialog();

    // The project already open is not offered a second time.
    state.loaded_project_path = Some(a.clone());
    state.open_dialog();
    let rows: Vec<String> = state.ui_context[state.slots.dialog].rows.iter().map(|r| r.id.clone()).collect();
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
        "toggle_params_plate",
    ] {
        assert!(crate::command::by_id(id).is_some(), "the toggle '{id}' has no command");
        assert!(state.command_toggle_state(id).is_some(), "the toggle '{id}' draws no switch");
    }
    state.run_command("command_palette");
    assert!(state.ui_context[state.slots.dialog].rows.iter().any(|r| r.id == "toggle_grid" && r.toggle().is_some()));
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
    assert_eq!(state.ui_context[state.slots.dialog].query, "");
}

/// A ctrl+left press on empty grid puts the cursor on the pressed cell — on
/// the PRESS — and dragging from there expands it into a region (without
/// ctrl the press is the scene's, and orbits), which stays
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

    // A plain press on empty space is the scene's: it orbits, and the
    // cursor stays. With ctrl it is the grid's.
    move_to(&mut state, anchor);
    let before = (state.grid_cursor_col, state.grid_cursor_row);
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
    assert!(state.orbit_drag.is_some() && state.grid_cursor_drag.is_none(), "a plain press orbits");
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
    assert_eq!((state.grid_cursor_col, state.grid_cursor_row), before);
    state.modifiers.ctrl = true;
    state.handle_event(&WindowEvent::MouseInput {
        state: ElementState::Pressed,
        button: MouseButton::Left,
    });
    state.modifiers.ctrl = false;
    assert_eq!(state.focused_pane, crate::slots::LEFT_MENUBAR_IDX, "the network has focus");
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
/// The params plate collapses into a small circle in the HUD's top
/// right corner while there are no rows to show, and grows back into
/// the plate fitted to the rows — eased, a tick at a time.
#[test]
fn the_params_plate_collapses_to_a_circle_with_no_rows() {
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    let mut redraw = false;
    let slot = geo(&state.fs_root).children.iter().position(|c| c.node_type == "sphere").unwrap();
    state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
    state.apply_layout();
    assert!(state.params_have_rows());
    let fitted = state.params_plate_target().expect("a plate");
    assert_eq!(fitted[4], 0.0, "the plate fitted to the rows");
    // Settle on it.
    while state.animate_params_plate(1.0 / 60.0) {}
    assert_eq!(state.params_plate_shown, Some(fitted));

    // Nothing selected: the target is the circle at the HUD's top right.
    state.deselect_node();
    state.sync_parameters_pane();
    state.apply_layout();
    assert!(!state.params_have_rows(), "no node, no rows");
    let (hx, hy, hw, _) = state.positions[crate::slots::PARAM_IDX];
    let dot = state.params_plate_target().expect("a circle");
    let d = crate::app::PARAMS_DOT_D;
    assert_eq!(dot, [hx + hw - d, hy, d, d, 1.0]);
    assert_eq!(state.params_claim(), (dot[0], dot[1], dot[2], dot[3]), "the circle is the HUD's");
    assert!(!state.on_param_resize_edge(hx, hy + 10.0), "a circle has no edge to drag");
    // Eased: one tick goes part of the way, and it arrives.
    assert!(state.animate_params_plate(1.0 / 60.0));
    let mid = state.params_plate_shown.unwrap();
    assert!(mid[2] < fitted[2] && mid[2] > dot[2], "on its way: {mid:?}");
    let (_, r) = state.params_plate_drawn().unwrap();
    assert!(r < mid[2].min(mid[3]) * 0.5 + 0.01);
    let mut ticks = 0;
    while state.animate_params_plate(1.0 / 60.0) {
        ticks += 1;
        assert!(ticks < 120, "it settles");
    }
    assert_eq!(state.params_plate_shown, Some(dot));
    let (_, r) = state.params_plate_drawn().unwrap();
    assert_eq!(r, d * 0.5, "a circle: corners half its side");

    // The plate off: no plate at all.
    state.params_plate = false;
    assert!(state.params_plate_target().is_none());
    state.animate_params_plate(1.0 / 60.0);
    assert!(state.params_plate_shown.is_none());
}

/// A config.kdl edit to the grid's spacing shows at once, at the zoom
/// in hand: the reload re-applies the configured geometry scaled as the
/// live one was. It used to be read at startup and only zoomed after.
#[test]
fn a_grid_spacing_edit_applies_at_the_zoom_in_hand() {
    use crate::app::{configured_grid_geometry, GridGeometry};
    let mut state = State::new(false);
    let cfg = configured_grid_geometry();
    assert_eq!(state.grid_base, cfg);
    // As if the config had said 90 rows apart, and the view is at 150%.
    state.grid_base = GridGeometry { pitch_y: 90.0, ..cfg };
    state.set_grid_geometry(GridGeometry { pitch_x: cfg.pitch_x * 1.5, pitch_y: 135.0, node_w: cfg.node_w * 1.5, node_h: cfg.node_h * 1.5 });
    // The file now says what `cfg` says.
    state.update_graph_settings_from_config();
    assert_eq!(state.grid_base, cfg);
    assert_eq!(state.grid_pitch_y, cfg.pitch_y * 1.5, "the new spacing, at the same 150%");
    assert_eq!(state.grid_pitch_x, cfg.pitch_x * 1.5);
    assert_eq!((state.node_w, state.node_h), (cfg.node_w * 1.5, cfg.node_h * 1.5));
    // A reload that changes nothing leaves the zoom alone.
    state.update_graph_settings_from_config();
    assert_eq!(state.grid_pitch_x, cfg.pitch_x * 1.5);
}

/// A node dropped on another node swaps places with it, connections
/// and all: in sphere1 → a → b → c, dragging b onto a leaves b where a
/// was and a where b was, wired sphere1 → b → a → c. One undo puts both
/// the places and the wires back.
#[test]
fn dropping_a_node_on_a_node_swaps_their_places() {
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::{ElementState, MouseButton};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;
    let mut redraw = false;
    for (name, y) in [("a", 5.0), ("b", 6.0), ("c", 7.0)] {
        state
            .apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some(name.into()), x: 1.0, y }, &mut redraw)
            .unwrap();
    }
    let slot = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
    for (name, from) in [("a", "sphere1"), ("b", "a"), ("c", "b")] {
        let slot = slot(&state, name);
        state.apply_action(McpAction::SetParam { slot, name: "input".into(), value: from.into() }, &mut redraw).unwrap();
    }
    state.edit_history.break_group();
    state.rebuild_positions();
    state.apply_layout();
    let input = |state: &State, name: &str| {
        crate::geometry::node_param_str(&state.current_dir().children[slot(state, name)], "input", "").to_string()
    };
    let at = |state: &State, name: &str| state.current_dir().children[slot(state, name)].position;
    let move_to = |state: &mut State, (col, row): (i32, i32)| {
        let (x, y) = state.cell_center(col, row);
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
    };

    // Grab b and drop it on a — the pointer between cells, not on a
    // crossing: the node snaps, whatever the config's grid_snap says.
    state.update_graph_settings_from_config();
    assert!(state.grid_snap_enabled, "a dragged node always snaps");
    move_to(&mut state, (1, 6));
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
    let (ax, ay) = state.cell_center(1, 5);
    state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: (ax + 23.0) as f64, y: (ay + 11.0) as f64 } });
    let ghost = state.ui_context[state.slots.content].inner().node_rect(slot(&state, "b")).unwrap();
    let cell = state.cell_rect(1, 5);
    assert!((ghost.0 - cell.0).abs() < 0.5 && (ghost.1 - cell.1).abs() < 0.5, "the dragged node sits on a's cell: {ghost:?} vs {cell:?}");
    move_to(&mut state, (1, 5));
    assert_eq!(state.ui_context[state.slots.content].inner().swap_target_idx(), Some(slot(&state, "a")), "a is the swap target");
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });

    assert_eq!((at(&state, "b"), at(&state, "a")), ((1.0, 5.0), (1.0, 6.0)), "the two traded places");
    assert_eq!(
        (input(&state, "b"), input(&state, "a"), input(&state, "c")),
        ("sphere1".to_string(), "b".to_string(), "a".to_string()),
        "and their connections: sphere1 → b → a → c"
    );

    // One step back.
    state.edit_history.break_group();
    assert!(state.history_step(true));
    assert_eq!((at(&state, "a"), at(&state, "b")), ((1.0, 5.0), (1.0, 6.0)));
    assert_eq!(
        (input(&state, "a"), input(&state, "b"), input(&state, "c")),
        ("sphere1".to_string(), "a".to_string(), "b".to_string())
    );
}

/// The rule a swap trades wires by: a renaming of the two, applied to
/// every wire, with the two nodes' own wires traded port for port. A
/// port only one of them has keeps its own wire.
#[test]
fn swapping_places_trades_wires_port_for_port() {
    use crate::app::{swap_places, FsNode, ParamDef};
    let node = |name: &str, wires: &[(&str, &str)]| FsNode {
        id: name.into(),
        name: name.into(),
        node_type: "attribute".into(),
        children: vec![],
        params: wires.iter().map(|(p, v)| ParamDef::new(*p, "node", *v)).collect(),
        geometry_visible: true,
        bypassed: false,
        position: (0.0, 0.0),
        inputs: 1,
        outputs: 1,
    };
    let mut dir = node("dir", &[]);
    dir.children = vec![
        node("i", &[]),
        node("a", &[("input", "i")]),
        node("b", &[("input", "a"), ("rest", "i")]),
        node("c", &[("input", "b"), ("with", "a")]),
    ];
    assert!(swap_places(&mut dir, "a", "b"));
    let wires = |n: usize| dir.children[n].params.iter().map(|p| p.text().to_string()).collect::<Vec<_>>();
    assert_eq!(wires(1), ["b"], "a reads b now, b standing where a stood");
    assert_eq!(wires(2), ["i", "i"], "b takes a's input; its Rest, which a lacks, stays");
    assert_eq!(wires(3), ["a", "b"], "c's wires follow the swap");
    assert!(!swap_places(&mut dir, "a", "nope"));
}

#[test]
fn dragging_a_selected_node_carries_the_selection() {
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::{ElementState, MouseButton};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = crate::slots::LEFT_MENUBAR_IDX;

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

    // Select a and b by dragging a box round them, ctrl held as the
    // press lands on empty space; it settles onto their
    // bounding box, (1, 5) to (2, 7).
    move_to(&mut state, (0, 4));
    state.modifiers.ctrl = true;
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
    state.modifiers.ctrl = false;
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
    state.modifiers.ctrl = true;
    state.handle_event(&WindowEvent::MouseInput {
        state: ElementState::Pressed,
        button: MouseButton::Left,
    });
    state.modifiers.ctrl = false;
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
        let g = state.ui_context[state.slots.content].inner();
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
        !state.ui_context[state.slots.dialog].rows.iter().any(|r| r.id == PATH_ROW_ID),
        "no project, no row"
    );
    state.close_dialog();

    state.save_to_file(&dir).expect("save");
    state.load_from_file(&dir).expect("load");
    assert_eq!(state.loaded_project_path.as_deref(), Some(dir.as_path()));

    state.run_command("command_palette");
    let row = state.ui_context[state.slots.dialog].rows.first().expect("rows").clone();
    assert_eq!(row.id, PATH_ROW_ID, "it heads the list");
    assert_eq!(row.label, dir.to_string_lossy(), "the label is the whole path");
    assert_eq!(row.chord, "my_project", "the file name reads in the chord column");
    assert!(row.truncate_head, "a path is cut from the left");

    // It ranks like any other row: a query that matches the path keeps it,
    // one that does not drops it.
    for c in ["m", "y", "_", "p"] {
        state.dialog_key_input(&typed(c));
    }
    assert!(state.ui_context[state.slots.dialog].rows.iter().any(|r| r.id == PATH_ROW_ID));
    for c in ["z", "z", "z"] {
        state.dialog_key_input(&typed(c));
    }
    assert!(!state.ui_context[state.slots.dialog].rows.iter().any(|r| r.id == PATH_ROW_ID));

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
    assert_eq!(state.ui_context[state.slots.dialog].mode, Mode::AddNode);
    // Inside the bundled project's Geometry node: every template but the
    // three that stand at the root.
    assert_eq!(state.ui_context[state.slots.dialog].rows.len(), state.node_templates.len() - 3);
    assert!(
        state.ui_context[state.slots.dialog].rows.iter().all(|r| r.chord.is_empty()),
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
        state.ui_context[state.slots.dialog].selected_id(),
        Some("Box"),
        "rows: {:?}",
        state.ui_context[state.slots.dialog].rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>()
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
    let labels = |state: &State| state.ui_context[state.slots.dialog].rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
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
    assert_eq!(state.ui_context[state.slots.dialog].mode, Mode::Commands);
    assert_eq!(state.ui_context[state.slots.dialog].query, "", "landing starts a fresh query");

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
