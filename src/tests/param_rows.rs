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
