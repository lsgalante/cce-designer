//! Parameter references and the Switch node (src/geometry.rs).

use super::*;

/// The shell end to end, through the resolver the viewport calls.
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

pub(super) fn ref_node(id: &str, name: &str, node_type: &str, params: Vec<(&str, &str, &str)>, children: Vec<FsNode>) -> FsNode {
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

pub(super) fn eval(root: &FsNode, target: &FsNode) -> (Option<Detail>, Option<String>) {
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

    // The view: the editor is inside, on xform1.
    assert_eq!(proj.view_state.current_path, vec![3]);
    assert_eq!(proj.view_state.selected_node, Some(1));

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
