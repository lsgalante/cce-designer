//! The wrangle node (src/wrangle.rs).

use super::*;

fn wrangle_node(id: &str, name: &str, input: &str, class: &str, code: &str) -> FsNode {
    ref_node(id, name, "wrangle", vec![("input", "text", input), ("class", "choice:Points,Primitives,Detail", class), ("group", "text", ""), ("code", "code", code)], vec![])
}

/// The input as the wrangle sees it, then the wrangle's own result.
fn wrangle_over_sphere(code: &str) -> (Detail, Option<Detail>, Option<String>) {
    let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "1.0")], vec![]);
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
    assert_eq!(crate::wrangle::channel_refs("ch(\"../radius\") + chs('Name') + search(\"x\") // ch(\"no\")"), vec!["../radius".to_string(), "Name".to_string()]);
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
    let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "1.0")], vec![]);
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
    let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "1.0")], vec![]);
    let w = ref_node("w", "wrangle1", "wrangle", vec![
        ("input", "text", "src"), ("class", "choice:Points,Primitives,Detail", "Points"), ("group", "text", ""),
        ("amount", "slider", "ch(\"../lift\") + 1"),
        ("Label", "text", "hello"),
        ("code", "code", "@P = @P * ch(\"amount\") + vec3(0, ch(\"../lift\"), 0);\n@n = chi(\"amount\");\n@s = chs(\"label\").len();\n@v = chv(\"../offset\");"),
    ], vec![]);
    let root = ref_node("root", "root", "node", vec![("Lift", "slider", "2"), ("offset", "float3", "1:2:3")], vec![src, w]);
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

    let bad = wrangle_node("b", "wrangle2", "src", "Points", "@x = ch(\"nope\");");
    let root2 = ref_node("root", "root", "node", vec![], vec![root.children[0].clone(), bad]);
    let (_, err) = eval(&root2, &root2.children[1]);
    assert!(err.as_deref().is_some_and(|e| e.contains("nope")), "a channel to nothing is an error: {err:?}");
}

/// The shipped template resolves, and its default script runs.
#[test]
fn wrangle_template_ships_and_its_default_code_runs() {
    let templates = crate::app::load_fs_tree();
    let t = templates.children.iter().find(|n| n.node_type == "wrangle").expect("nodes/wrangle.json loads");
    assert_eq!(t.name, "Wrangle");
    let code = t.params.iter().find(|p| p.name == "code").map(|p| p.text().to_string()).unwrap();
    assert!(t.params.iter().all(|p| !p.is_expr()), "no template parameter reads as an expression — least of all the Code");
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
    let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "1.0")], vec![]);
    let k = ref_node("k", "opencl1", "opencl", vec![("input", "text", "src"), ("code", "code", "__kernel void process() {}")], vec![]);
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
