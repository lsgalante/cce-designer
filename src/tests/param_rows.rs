//! The parameter pane's conditional rows.

use super::*;

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
            for clause in p.show_when.split("||").flat_map(|alt| alt.split("&&")) {
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
        ("Create", true), ("Remap", true), ("Clip", true),
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

/// A run of rows can grow: a family's button adds a member under the
/// last (here through the pane, as a press writes it), numbered and
/// labelled after the template's first; a member is taken away from its
/// row menu while it is not the last, the ones after it moving up a
/// number with their values; each is one undo step that puts the whole
/// list back; MCP's `set_param` grows a family to the member it names and
/// `delete_param` takes one away as the menu does; and a saved node's
/// members are put back together, in order and ahead of the button, by
/// the template merge.
#[test]
fn a_run_of_rows_can_grow() {
    use crate::app::{McpAction, ParamKind, ParamMenuAction as A};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = LEFT_MENUBAR_IDX;
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("sum1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
    let slot = state.current_dir().children.iter().position(|c| c.name == "sum1").unwrap();
    for (name, value) in [("input", "sphere1"), ("operation", "Composite"), ("combine_op", "Add"), ("attribute_name", "Pos"), ("operand_1", "0:1:0")] {
        state.apply_action(McpAction::SetParam { slot, name: name.into(), value: value.into() }, &mut redraw).unwrap();
    }
    state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
    let id = state.current_dir().children[slot].id.clone();
    let names = |state: &State| -> Vec<String> {
        state.current_dir().children[slot].params.iter().filter(|p| p.multi == "operand").map(|p| p.name.clone()).collect()
    };
    let keys = |state: &mut State| -> Vec<String> {
        state.sync_parameters_pane();
        state.param_mut().node_params().iter().map(|r| r.0.clone()).filter(|k| k.contains("Operand")).collect()
    };
    assert_eq!(names(&state), ["operand_1", "add_operand"]);
    assert_eq!(keys(&mut state), ["Operand 1", "Add Operand"]);

    // Pressed in the pane: the button's row written `clicked`.
    let mut rows = state.param_mut().node_params();
    rows.iter_mut().find(|r| r.0 == "Add Operand").unwrap().1 = "clicked".into();
    state.param_mut().set_display_params(&rows);
    state.sync_parameters_to_project();
    assert_eq!(names(&state), ["operand_1", "operand_2", "add_operand"]);
    assert_eq!(keys(&mut state), ["Operand 1", "Operand 2", "Add Operand"]);
    let second = state.current_dir().children[slot].params.iter().find(|p| p.name == "operand_2").unwrap();
    assert_eq!((second.kind(), second.text(), second.show_when.as_str()), (ParamKind::Operand, "", "operation == Composite && combine_op != Length"));
    assert!(state.current_dir().children[slot].params.iter().all(|p| p.text() != "clicked"));

    // One undo step, which puts the whole list back, and a redo.
    assert!(state.history_step(true));
    assert_eq!(names(&state), ["operand_1", "add_operand"]);
    assert!(state.history_step(false));
    assert_eq!(names(&state), ["operand_1", "operand_2", "add_operand"]);

    // MCP grows the family to the member it names.
    state.apply_action(McpAction::SetParam { slot, name: "operand_4".into(), value: "4".into() }, &mut redraw).unwrap();
    assert_eq!(names(&state), ["operand_1", "operand_2", "operand_3", "operand_4", "add_operand"]);
    let menu = state.param_menu_rows(slot, "operand_4").0;
    assert!(menu.contains(&"Default: ".to_string()), "a later member's default is the first's: {menu:?}");
    state.apply_action(McpAction::SetParam { slot, name: "operand_3".into(), value: "3".into() }, &mut redraw).unwrap();
    assert!(state.apply_action(McpAction::SetParam { slot, name: "operand_99".into(), value: "1".into() }, &mut redraw).is_err(), "not a hundred rows for a typo");

    // The row menu takes one away; the rest move up a number, values and all.
    assert!(state.param_menu_rows(slot, "operand_2").1.contains(&A::RemoveMember));
    state.run_param_action(&id, "operand_2", A::RemoveMember);
    assert_eq!(names(&state), ["operand_1", "operand_2", "operand_3", "add_operand"]);
    let text = |state: &State, n: &str| state.current_dir().children[slot].params.iter().find(|p| p.name == n).unwrap().text().to_string();
    assert_eq!((text(&state, "operand_2"), text(&state, "operand_3")), ("3".into(), "4".into()));
    assert_eq!(keys(&mut state), ["Operand 1", "Operand 2", "Operand 3", "Add Operand"]);
    state.history_step(true);
    assert_eq!(text(&state, "operand_2"), "", "undone, the list is as it stood");
    state.history_step(false);
    // MCP's delete_param the same way, down to one, which stays.
    state.apply_action(McpAction::DeleteParam { slot, name: "operand_1".into() }, &mut redraw).unwrap();
    assert_eq!(text(&state, "operand_1"), "3");
    state.apply_action(McpAction::DeleteParam { slot, name: "operand_2".into() }, &mut redraw).unwrap();
    assert!(state.apply_action(McpAction::DeleteParam { slot, name: "operand_1".into() }, &mut redraw).is_err());
    assert!(!state.param_menu_rows(slot, "operand_1").1.contains(&A::RemoveMember), "the last is not offered");
    assert_eq!(names(&state), ["operand_1", "add_operand"]);

    // An operand switches between a constant and an attribute.
    assert!(state.param_menu_rows(slot, "operand_1").1.contains(&A::OperandAttribute));
    state.run_param_action(&id, "operand_1", A::OperandAttribute);
    assert_eq!(text(&state, "operand_1"), "");
    assert!(state.param_menu_rows(slot, "operand_1").1.contains(&A::OperandConstant));
    state.run_param_action(&id, "operand_1", A::OperandConstant);
    assert_eq!(text(&state, "operand_1"), "0");

    // A save's members, out of order and without the button or their UI,
    // are put back together by the merge.
    let mut saved = state.current_dir().children[slot].clone();
    saved.params.retain(|p| p.multi != "operand");
    saved.params.push(crate::app::ParamDef::new("operand_2", "text", "b"));
    saved.params.insert(2, crate::app::ParamDef::new("operand_1", "text", "a"));
    let mut root = crate::app::FsNode { children: vec![saved], ..state.doc.fs_root.clone() };
    crate::app::merge_template_defs(&mut root, &state.doc.node_templates);
    let merged: Vec<(String, String, String)> = root.children[0]
        .params
        .iter()
        .filter(|p| p.multi == "operand")
        .map(|p| (p.name.clone(), p.label.clone(), p.text().to_string()))
        .collect();
    let row = |n: &str, l: &str, t: &str| (n.to_string(), l.to_string(), t.to_string());
    assert_eq!(merged, [row("operand_1", "Operand 1", "a"), row("operand_2", "Operand 2", "b"), row("add_operand", "Add Operand", "")]);
    assert!(root.children[0].params.iter().filter(|p| p.multi == "operand").take(2).all(|p| p.kind() == ParamKind::Operand));
}
