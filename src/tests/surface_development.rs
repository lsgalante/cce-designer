//! Phase 3: surface development.

use super::*;


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

/// The same mesh in every particular: points, identities, primitives,
/// attributes.
fn same_mesh(a: &Detail, b: &Detail) -> bool {
    a.positions() == b.positions()
        && a.ids() == b.ids()
        && a.num_prims() == b.num_prims()
        && (0..a.num_prims()).all(|p| a.prim_points(p) == b.prim_points(p))
        && a.points().names() == b.points().names()
        && a.points().names().iter().all(|n| (0..a.num_points()).all(|p| a.points().value(n, p) == b.points().value(n, p)))
}

/// The Remesh is a subnet of nodes (since 2026-09-30) — a Repeat of
/// Split Edges, Collapse Edges, Flip Edges, a Tangential Relax and a
/// Project, then a Transfer — and it makes the mesh the native remesh
/// makes, bit for bit: points, identities, the counter new points draw
/// from, primitives, attributes and groups. Twice over, the second
/// remesh eating the first as a simulation's next step does, so an
/// identity a collapse took out is not handed back. With no relaxation
/// asked and nothing to do, the input comes back as it came.
#[test]
fn the_remesh_subnet_is_the_remesh() {
    let templates = crate::app::load_fs_tree();
    let t = templates.children.iter().find(|t| t.name == "Remesh").expect("the Remesh template");
    assert_eq!(t.node_type, "node", "the Remesh is a subnet");
    let names: Vec<&str> = t.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["input1", "repeat1", "transfer1", "transfer_switch1", "output1"]);
    let repeat = &t.children[1];
    assert_eq!(repeat.node_type, "repeat");
    let passes: Vec<(&str, &str)> = repeat.children.iter().map(|c| (c.name.as_str(), c.node_type.as_str())).collect();
    assert_eq!(passes, [("input1", "input"), ("seed1", "seed"), ("split1", "split_edges"), ("collapse1", "collapse_edges"),
        ("flip1", "flip_edges"), ("relax1", "relax"), ("project1", "project"), ("output1", "output")]);
    for c in &t.children {
        assert_eq!(c.geometry_visible, c.name == "transfer_switch1", "only transfer_switch1 draws: {}", c.name);
    }

    let tagged = [
        phase3_node("group", &[("input", "sphere 1"), ("group_name", "top"), ("mode", "Box"), ("center", "-1.875:1.55:0.00"), ("size", "4.00:2.00:4.00")]),
        phase3_node("attribute", &[("input", "group 1"), ("operation", "Create"), ("attribute_name", "mass"), ("type", "Float"), ("value", "3")]),
    ];
    let native = |name: &str, input: &str, s: &[(&str, &str)]| {
        let mut n = phase3_node("remesh", &[("input", input)]);
        n.id = format!("id-{name}");
        n.name = name.into();
        for (k, v) in s.iter().chain([("Split", "true"), ("Collapse", "true"), ("flip", "true"), ("Project", "true")].iter()) {
            n.params.push(crate::app::ParamDef::new(*k, "text", *v));
        }
        n
    };
    let subnet = |name: &str, input: &str, s: &[(&str, &str)]| {
        let mut n = t.clone();
        crate::app::regenerate_node_ids(&mut n);
        n.name = name.into();
        n.params.iter_mut().find(|p| p.name == "input").unwrap().set_text(input.to_string());
        for (k, v) in s {
            n.params.iter_mut().find(|p| p.name == *k).unwrap_or_else(|| panic!("the subnet has {k}")).set_text(v.to_string());
        }
        n
    };
    let same = |a: &Detail, b: &Detail, what: &str| {
        assert_eq!((a.num_points(), a.num_prims()), (b.num_points(), b.num_prims()), "{what}: counts");
        assert_eq!(a.positions(), b.positions(), "{what}: positions");
        assert_eq!(a.ids(), b.ids(), "{what}: identities");
        assert!(a == b, "{what}: attributes, groups or the id counter");
    };
    for settings in [
        vec![("target_length", "0.5"), ("iterations", "3"), ("relax", "0.5"), ("transfer", "false")],
        vec![("target_length", "0.25"), ("iterations", "4"), ("relax", "0.04"), ("transfer", "true")],
        vec![("target_length", "0.8"), ("iterations", "2"), ("relax", "1.0"), ("transfer", "true"), ("transfer_groups", "true")],
    ] {
        let mut children = tagged.to_vec();
        children.push(native("native1", "attribute 1", &settings));
        children.push(native("native2", "native1", &settings));
        children.push(subnet("remesh1", "attribute 1", &settings));
        children.push(subnet("remesh2", "remesh1", &settings));
        let root = modelling_root("1.0", children);
        for (n, s) in [("native1", "remesh1"), ("native2", "remesh2")] {
            let (a, err) = eval_node(&root, n);
            assert!(err.is_none(), "{err:?}");
            let (b, err) = eval_node(&root, s);
            assert!(err.is_none(), "{err:?}");
            assert!(b.num_points() > 0 && b.points().has("mass"), "{settings:?}");
            same(&a, &b, &format!("{s} {settings:?}"));
        }
    }

    // No relaxation, and a mesh already at its length: the input back.
    let settings = [("target_length", "0.5"), ("iterations", "3"), ("relax", "0"), ("transfer", "false")];
    let root = modelling_root("1.0", vec![
        subnet("remesh1", "sphere 1", &settings),
        subnet("remesh2", "remesh1", &settings),
        native("native1", "sphere 1", &settings),
    ]);
    let (first, _) = eval_node(&root, "remesh1");
    let (native_first, _) = eval_node(&root, "native1");
    same(&native_first, &first, "relax 0");
    let (again, _) = eval_node(&root, "remesh2");
    let settled = crate::remesh::remesh(&first, crate::remesh::Settings { target: 0.5, iterations: 3, relax: 0.0, ..Default::default() });
    assert!(settled == first, "the fixture has settled: a native remesh of it is itself");
    assert!(again == first, "a remesh with nothing to do hands its input back");
}

/// A native remesh from before 2026-09-30 loads as the Remesh subnet —
/// wherever it stands, a simnet's chain included — with its values and
/// its identity, and a pass it had switched off BYPASSED inside, since
/// the switches are the nodes now. It makes the mesh it made.
#[test]
fn a_native_remesh_recomposes_on_load() {
    let templates_root = crate::app::load_fs_tree();
    let templates = crate::app::flatten_node_templates(&templates_root);
    let mut old = phase3_node("remesh", &[("input", "sphere 1"), ("target_length", "0.3"), ("iterations", "2"), ("relax", "0.04"),
        ("split", "false"), ("collapse", "true"), ("flip", "true"), ("project", "true"), ("transfer", "true"), ("from", "")]);
    old.id = "old-id".into();
    old.name = "remesh1".into();
    old.position = (2.0, 5.0);
    let mut root = modelling_root("1.0", vec![ref_node("sub", "sub1", "node", vec![], vec![]), old.clone()]);
    root.children[1].children.push({ let mut inner = old.clone(); inner.id = "inner-id".into(); inner });
    crate::app::merge_template_defs(&mut root, &templates);
    for r in [&root.children[2], &root.children[1].children[0]] {
        assert_eq!(r.node_type, "node", "recomposed as a subnet, nested ones too");
        assert!(r.is_enterable());
        let get = |n: &str| r.params.iter().find(|p| p.name == n).map(|p| p.text().to_string());
        assert_eq!(get("target_length").as_deref(), Some("0.3"));
        assert_eq!(get("relax").as_deref(), Some("0.04"));
        assert_eq!(get("transfer").as_deref(), Some("true"));
        assert_eq!(get("split"), None, "the pass switches are nodes now");
        let repeat = r.children.iter().find(|c| c.name == "repeat1").unwrap();
        let bypassed: Vec<&str> = repeat.children.iter().filter(|c| c.bypassed).map(|c| c.name.as_str()).collect();
        assert_eq!(bypassed, ["split1"], "Split was off");
    }
    let r = &root.children[2];
    assert_eq!((r.id.as_str(), r.name.as_str(), r.position), ("old-id", "remesh1", (2.0, 5.0)));
    // The native node, beside it after the load (which gives the bare
    // sphere its template's Center too), is what it has to match.
    let mut native = old.clone();
    native.id = "native-id".into();
    native.name = "native1".into();
    root.children.push(native);
    let (made, err) = eval_node(&root, "native1");
    assert!(err.is_none(), "{err:?}");
    let (g, err) = eval_node(&root, "remesh1");
    assert!(err.is_none(), "{err:?}");
    assert!(g.num_points() > 0 && g == made, "the recomposed remesh makes the mesh the native one made");
}

/// The Remesh subnet's switch was `result1` for a day; a Remesh saved
/// then loads with it renamed, and its output wired to the new name.
#[test]
fn a_remesh_saved_with_result1_is_renamed_on_load() {
    let templates_root = crate::app::load_fs_tree();
    let templates = crate::app::flatten_node_templates(&templates_root);
    let mut old = templates_root.children.iter().find(|t| t.name == "Remesh").unwrap().clone();
    crate::app::regenerate_node_ids(&mut old);
    old.name = "remesh1".into();
    for k in &mut old.children {
        if k.name == "transfer_switch1" {
            k.name = "result1".into();
        }
        for p in &mut k.params {
            if p.text() == "transfer_switch1" {
                p.set_text("result1");
            }
        }
    }
    let mut root = modelling_root("1.0", vec![old]);
    root.children[1].params.iter_mut().find(|p| p.name == "input").unwrap().set_text("sphere 1");
    crate::app::merge_template_defs(&mut root, &templates);
    let r = &root.children[1];
    let names: Vec<&str> = r.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["input1", "repeat1", "transfer1", "transfer_switch1", "output1"]);
    let out = r.children.iter().find(|c| c.name == "output1").unwrap();
    assert_eq!(out.params.iter().find(|p| p.name == "input").unwrap().text(), "transfer_switch1");
    let (g, err) = eval_node(&root, "remesh1");
    assert!(err.is_none() && g.num_points() > 0, "{err:?}");
}

/// A Repeat runs its chain Iterations times, each pass on the last one's
/// result; a Seed inside reads what the loop began from. Dived in, its
/// chain is shown as the LAST pass saw it, as a simnet's is shown as its
/// last substep saw it — and a level inside a loop (a subnet in a
/// simnet) is walked with the loop's feedback pushed, so it shows this
/// frame and not the seed.
#[test]
fn a_repeat_loops_its_chain_and_shows_its_last_pass() {
    let templates_root = crate::app::load_fs_tree();
    let t = templates_root.children.iter().find(|t| t.name == "Repeat").expect("the Repeat template");
    assert!(t.is_enterable());
    let mut repeat = t.clone();
    crate::app::regenerate_node_ids(&mut repeat);
    repeat.name = "repeat1".into();
    repeat.geometry_visible = true;
    repeat.params.iter_mut().find(|p| p.name == "input").unwrap().set_text("sphere 1");
    repeat.params.iter_mut().find(|p| p.name == "iterations").unwrap().set_text("3");
    let mut step = ref_node("step", "step1", "transform", vec![("input", "node", "input1"), ("translation", "float3", "1:0:0")], vec![]);
    step.geometry_visible = false;
    repeat.children.push(step);
    repeat.children.iter_mut().find(|c| c.name == "output1").unwrap().params[0].set_text("step1");
    let root = modelling_root("1.0", vec![repeat]);
    let (sphere, _) = eval_node(&root, "sphere 1");
    let min_x = |g: &Detail| g.positions().iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
    let x0 = min_x(&sphere);
    let (g, err) = eval_node(&root, "repeat1");
    assert!(err.is_none(), "{err:?}");
    assert!((min_x(&g) - (x0 + 3.0)).abs() < 1e-4, "three passes of +1: {}", min_x(&g) - x0);

    // Dived in: the output draws the result, the step as the last pass
    // saw it (+2 in, +3 out), and the seed what the loop began from.
    let mut root = root;
    let r = root.children.iter_mut().find(|c| c.name == "repeat1").unwrap();
    r.children.iter_mut().find(|c| c.name == "output1").unwrap().geometry_visible = false;
    r.children.iter_mut().find(|c| c.name == "input1").unwrap().geometry_visible = false;
    r.children.iter_mut().find(|c| c.name == "step1").unwrap().geometry_visible = true;
    r.children.iter_mut().find(|c| c.name == "seed1").unwrap().geometry_visible = true;
    let level = root.children.iter().find(|c| c.name == "repeat1").unwrap();
    let mut cache = crate::geometry::SimCache::default();
    let mut sim = crate::geometry::EvalSim::new(0, 0, &mut cache);
    let mut err = None;
    let shown = crate::geometry::network_sphere_vertices_with_errors(&root, level, &mut err, &mut sim);
    assert!(err.is_none(), "{err:?}");
    assert_eq!(shown.num_points(), 2 * sphere.num_points(), "the step and the seed");
    let mut xs: Vec<f32> = shown.positions().iter().map(|p| p[0]).collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!((xs[0] - x0).abs() < 1e-4, "the seed is the sphere");
    assert!((xs[sphere.num_points()] - (x0 + 3.0)).abs() < 1e-4, "the step as its last pass made it: {}", xs[sphere.num_points()] - x0);

    // A subnet inside a simnet, dived into, shows the frame.
    let sub = ref_node("sub", "sub1", "node", vec![("input", "node", "input1")], vec![
        ref_node("sub-in", "input1", "input", vec![], vec![]),
        { let mut n = ref_node("sub-step", "step1", "transform", vec![("input", "node", "input1"), ("translation", "float3", "1:0:0")], vec![]); n.geometry_visible = true; n },
        { let mut n = ref_node("sub-out", "output1", "output", vec![("input", "node", "step1")], vec![]); n.geometry_visible = false; n },
    ]);
    let simnet = ref_node("sim", "simnet1", "simnet", vec![("input", "node", "sphere 1"), ("substeps", "spinbox", "1")], vec![
        ref_node("sim-in", "input1", "input", vec![], vec![]),
        sub,
        ref_node("sim-out", "output1", "output", vec![("input", "node", "sub1")], vec![]),
    ]);
    let root = modelling_root("1.0", vec![simnet]);
    let level = &root.children[1].children[1];
    let mut cache = crate::geometry::SimCache::default();
    let mut sim = crate::geometry::EvalSim::new(3, 0, &mut cache);
    let shown = crate::geometry::network_sphere_vertices_with_errors(&root, level, &mut err, &mut sim);
    assert!(err.is_none(), "{err:?}");
    // Its input shows what the frame's step read (+2), its step what it
    // made (+3); read off the seed, they were +0 and +1.
    let max_x = |g: &Detail| g.positions().iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
    assert_eq!(shown.num_points(), 2 * sphere.num_points());
    assert!((min_x(&shown) - (x0 + 2.0)).abs() < 1e-4, "the input at frame 3: {}", min_x(&shown) - x0);
    assert!((max_x(&shown) - (max_x(&sphere) + 3.0)).abs() < 1e-4, "the step at frame 3: {}", max_x(&shown) - max_x(&sphere));
}

/// The faster flip pass and the faster closest-point search are HOW a
/// remesh is run and not what it does: against the passes as they were
/// first written, the same mesh bit for bit, step after step of a
/// surface being pulled about — and the same hit for a query on the
/// surface, off it and far from it.
#[test]
fn the_remesh_matches_its_reference() {
    use crate::remesh::remesh_reference;
    let fixtures = [
        (sphere_detail(Vec3::ZERO, 1.0, 6, 8), 0.2),
        (sphere_detail(Vec3::ZERO, 1.0, 24, 32), 0.5),
        (sphere_detail(Vec3::new(0.3, -0.2, 1.0), 0.5, 10, 14), 0.1),
    ];
    for (start, target) in fixtures {
        for relax in [0.0, 0.5] {
            let settings = Settings { target, iterations: 3, relax, ..Default::default() };
            let (mut a, mut b) = (start.clone(), start.clone());
            for step in 0..6 {
                // A pull between remeshes, as a simulation makes one.
                for d in [&mut a, &mut b] {
                    for p in 0..d.num_points() {
                        let at = d.pos(p);
                        let pull = Vec3::new(0.03, 0.0, 0.0) * (at.y * 3.0 + step as f32).sin();
                        d.set_pos(p, at + pull);
                    }
                }
                a = remesh(&a, settings);
                b = remesh_reference(&b, settings);
                assert!(same_mesh(&a, &b), "target {target}, relax {relax}, step {step}: {} points against {}", a.num_points(), b.num_points());
            }
            assert!(a.num_points() != start.num_points() || target == 0.1, "the fixture remeshes");
        }
    }

    let sphere = sphere_detail(Vec3::ZERO, 1.0, 12, 16);
    let grid = crate::spatial::TriGrid::build(&sphere);
    let mut asked = 0;
    for i in 0..4000 {
        // On the surface, near it, inside it, far outside it.
        let f = i as f32;
        let dir = Vec3::new((f * 0.37).sin(), (f * 0.73).cos(), (f * 1.31).sin()).normalize_or_zero();
        let p = dir * [1.0, 0.98, 1.05, 0.3, 0.0, 4.0, 40.0][i % 7];
        let (new, old) = (grid.closest(p).unwrap(), grid.closest_reference(p).unwrap());
        assert_eq!((new.point, new.distance, new.normal), (old.point, old.distance, old.normal), "query {p:?}");
        asked += 1;
    }
    for p in 0..sphere.num_points() {
        let (new, old) = (grid.closest(sphere.pos(p)).unwrap(), grid.closest_reference(sphere.pos(p)).unwrap());
        assert_eq!((new.point, new.distance, new.normal), (old.point, old.distance, old.normal), "point {p}");
    }
    assert_eq!(asked, 4000);
}

/// A point in a group survives a remesh: a pulled point at the tip of
/// a spike keeps its identity, its values and its membership while the
/// mesh around it is split and collapsed. A collapse that would leave a
/// corner with fewer than three triangles is refused — it stranded the
/// pulled point, which `into_detail` then dropped with everything it
/// was — and where the pulled point is one end of a collapsed edge it
/// is the end that survives, joining the other end's groups.
#[test]
fn a_grouped_point_survives_a_remesh() {
    let mut d = sphere_detail(Vec3::ZERO, 1.0, 10, 14);
    let tip = 37usize;
    let id = d.ids()[tip];
    d.points_mut().create_group("pull");
    d.points_mut().add_to_group("pull", tip);
    d.points_mut().create("mass", crate::detail::AttribValue::Float(0.0));
    d.points_mut().set_value("mass", tip, crate::detail::AttribValue::Float(7.5)).unwrap();
    let settings = Settings { target: 0.25, iterations: 3, relax: 0.0, ..Default::default() };
    let mut outward = d.pos(tip).normalize();
    for step in 0..40 {
        // The pull: the tip out along its ray, a little each step, so
        // the edges around it stretch and the remesh works there.
        let p = d.points().group_members("pull");
        assert_eq!(p.len(), 1, "step {step}: the group has one member, not {}", p.len());
        let at = p[0] as usize;
        assert_eq!(d.ids()[at], id, "step {step}: the member is the point it was");
        assert_eq!(d.points().value("mass", at), Some(crate::detail::AttribValue::Float(7.5)), "step {step}: with its values");
        let pos = d.pos(at);
        outward = if pos.length() > 1e-3 { pos.normalize() } else { outward };
        d.set_pos(at, pos + outward * 0.08);
        d = remesh(&d, settings);
        assert!(d.is_closed(), "step {step}: still a closed surface");
    }
    assert!(d.pos(d.points().group_members("pull")[0] as usize).length() > 3.0, "the tip went out with the pull");
}

/// A remesh SETTLES: run again on what it made, it comes to a mesh it
/// finds nothing to do to, and hands that back as it was given — the
/// primitives and their order untouched, which is what lets a step of
/// a simulation at rest keep what it knows of the topology. Until
/// 2026-09-29 the flip pass, judging by valence alone, turned back the
/// long edges the split and collapse had just turned, and the three
/// went round on the same edges for ever.
#[test]
fn a_remesh_settles_and_then_leaves_the_mesh_alone() {
    for (start, target) in [
        (sphere_detail(Vec3::ZERO, 1.0, 6, 8), 0.2),
        (sphere_detail(Vec3::ZERO, 1.0, 24, 32), 0.5),
        (sphere_detail(Vec3::ZERO, 0.5, 10, 14), 0.1),
    ] {
        let settings = Settings { target, iterations: 3, relax: 0.0, ..Default::default() };
        let mut d = remesh(&start, settings);
        assert!(crate::remesh::last_changes() > 0, "the fixture remeshes");
        let mut rounds = 0;
        loop {
            let next = remesh(&d, settings);
            if crate::remesh::last_changes() == 0 {
                assert!(same_mesh(&next, &d), "a remesh with nothing to do changed the mesh");
                break;
            }
            d = next;
            rounds += 1;
            assert!(rounds < 20, "target {target}: still changing {} edges after {rounds} rounds", crate::remesh::last_changes());
        }
        // What it settled on is still the mesh that was asked for.
        let mean = mean_edge(&d);
        assert!((mean - target).abs() < target * 0.5, "settled at {mean}, wanted about {target}");
        assert!(d.is_closed(), "and still a closed surface");
    }
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
        params: [("attribute", "growth"), ("scale", "0.50"), ("direction", "N")]
            .into_iter()
            .map(|(name, default)| crate::app::ParamDef::new(name, "text", default))
            .collect(),
        geometry_visible: true,
        bypassed: false,
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

/// Develop's Direction names the attribute points move along: N is the
/// normal, worked out when the input carries none; any other vector
/// attribute is followed, its length scaling the move; a name the input
/// lacks is an error and moves nothing.
#[test]
fn develop_moves_along_the_attribute_its_direction_names() {
    let mut sphere = sphere_detail(Vec3::ZERO, 1.0, 8, 12);
    sphere.points_mut().create("growth", AttribValue::Float(1.0));
    sphere.points_mut().create("up", AttribValue::Float3([0.0, 2.0, 0.0]));
    let run = |geom: &Detail, dir: &str| {
        let node = crate::app::FsNode {
            id: "d".into(),
            name: "develop1".into(),
            node_type: "develop".into(),
            children: vec![],
            params: [("attribute", "growth"), ("scale", "0.50"), ("direction", dir)]
                .into_iter()
                .map(|(name, v)| crate::app::ParamDef::new(name, "text", v))
                .collect(),
            geometry_visible: true,
            bypassed: false,
            position: (0.0, 0.0),
            inputs: 1,
            outputs: 1,
        };
        let mut out = geom.clone();
        let mut err = None;
        crate::geometry::apply_develop(&mut out, &node, &mut err);
        (out, err)
    };
    // Along `up`, length 2: every point one unit up.
    let (out, err) = run(&sphere, "up");
    assert!(err.is_none(), "{err:?}");
    for p in 0..out.num_points() {
        assert!((out.pos(p) - sphere.pos(p) - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5);
    }
    // N on an input without one: the surface normals, outward.
    let mut bare = sphere.clone();
    bare.points_mut().remove("N");
    assert!(!bare.points().has("N"));
    let (out, err) = run(&bare, "N");
    assert!(err.is_none(), "{err:?}");
    assert!((0..out.num_points()).all(|p| (out.pos(p).length() - 1.5).abs() < 0.05));
    // An N the input carries is followed as it is.
    let mut with_n = sphere.clone();
    with_n.points_mut().remove("N");
    with_n.points_mut().create("N", AttribValue::Float3([1.0, 0.0, 0.0]));
    let (out, err) = run(&with_n, "N");
    assert!(err.is_none(), "{err:?}");
    assert!((out.pos(0) - with_n.pos(0) - Vec3::new(0.5, 0.0, 0.0)).length() < 1e-5);
    // A name the input lacks moves nothing and says so.
    let (out, err) = run(&sphere, "nope");
    assert!(err.is_some_and(|e| e.contains("nope")));
    assert_eq!(out.pos(3), sphere.pos(3));
}

/// Format 8: an older Develop's Normal / Attribute choice and its Source
/// row become one Direction naming the attribute.
#[test]
fn an_older_develop_direction_becomes_an_attribute_name() {
    use crate::app::{FsNode, ParamDef, Project};
    let develop = |id: &str, dir: &str, src: &str| FsNode {
        id: id.into(),
        name: id.into(),
        node_type: "develop".into(),
        children: vec![],
        params: vec![ParamDef::new("direction", "choice:Normal,Attribute", dir), ParamDef::new("source", "attribute", src)],
        geometry_visible: true,
        bypassed: false,
        position: (0.0, 0.0),
        inputs: 1,
        outputs: 1,
    };
    let mut root = develop("root", "", "");
    root.node_type = "subnet".into();
    root.params.clear();
    root.children = vec![develop("by_n", "Normal", "vel"), develop("by_attr", "Attribute", "vel"), develop("attr_empty", "Attribute", "")];
    let mut proj = Project { name: "p".into(), root, view_state: Default::default(), format: 7 };
    proj.migrate_format();
    let dir = |i: usize| {
        let n = &proj.root.children[i];
        assert!(!n.params.iter().any(|p| p.name == "source"), "the Source row goes");
        n.params.iter().find(|p| p.name == "direction").unwrap().text().to_string()
    };
    assert_eq!(dir(0), "N", "Normal is the normal");
    assert_eq!(dir(1), "vel", "Attribute is what Source named");
    assert_eq!(dir(2), "N", "an Attribute naming nothing moved along nothing; N is the sane reading");
}

pub(super) fn phase3_node(ty: &str, params: &[(&str, &str)]) -> FsNode {
    FsNode {
        id: format!("id-{ty}"),
        name: format!("{ty} 1"),
        node_type: ty.into(),
        children: vec![],
        params: params
            .iter()
            .map(|(name, default)| crate::app::ParamDef::new(*name, "text", *default))
            .collect(),
        geometry_visible: true,
        bypassed: false,
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
        &[("thickness", "1.00"), ("rings", "2"), ("iterations", "6")],
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

/// The solve in `detangle.rs` is the first version's, bit for bit: the
/// same pairs summed in the same order, over meshes that tangle and
/// ones that do not, with and without a group, and step after step as a
/// simulation runs it — which is when the kept topology, the reused
/// grid and the early end are all in play.
#[test]
fn the_detangle_solve_matches_its_reference() {
    // Two sheets closer than a thickness, sharing no topology.
    let sheets = {
        let mut d = Detail::new();
        let mut rows = Vec::new();
        for sheet in 0..2 {
            let mut row = Vec::new();
            for i in 0..6 {
                for j in 0..6 {
                    row.push(d.add_point(Vec3::new(i as f32 * 0.25, sheet as f32 * 0.05 + (i * j) as f32 * 0.003, j as f32 * 0.25)));
                }
            }
            rows.push(row);
        }
        for sheet in 0..2 {
            for i in 0..5 {
                for j in 0..5 {
                    let at = |a: usize, b: usize| rows[sheet][a * 6 + b];
                    d.add_prim(&[at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                    d.add_prim(&[at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
                }
            }
        }
        for p in (0..d.num_points()).step_by(3) {
            d.points_mut().add_to_group("some", p);
        }
        d
    };
    // A sphere pressed nearly flat, so its two sides meet; and one left
    // round, which has nothing to separate.
    let squashed = |flatten: f32| {
        let mut d = sphere_detail(Vec3::ZERO, 0.5, 10, 14);
        for p in 0..d.num_points() {
            let v = d.pos(p);
            d.set_pos(p, Vec3::new(v.x, v.y * flatten, v.z));
        }
        for p in (0..d.num_points()).step_by(2) {
            d.points_mut().add_to_group("some", p);
        }
        d
    };
    let meshes = [("sheets", sheets), ("flat sphere", squashed(0.04)), ("round sphere", squashed(1.0))];
    let settings: [&[(&str, &str)]; 7] = [
        &[("thickness", "1.00"), ("rings", "2"), ("iterations", "4")],
        &[("thickness", "2.00"), ("rings", "1"), ("iterations", "8")],
        &[("thickness", "0.50"), ("rings", "0"), ("iterations", "1")],
        &[("thickness", "3.00"), ("rings", "3"), ("iterations", "32")],
        &[("thickness", "1.50"), ("rings", "2"), ("iterations", "6"), ("group", "some")],
        &[("thickness", "0.00"), ("rings", "2"), ("iterations", "4")],
        &[("thickness", "1.00"), ("rings", "6"), ("iterations", "4"), ("group", "nobody")],
    ];
    let mut moved_somewhere = 0;
    for (name, mesh) in &meshes {
        for params in settings {
            let node = phase3_node("detangle", params);
            let (mut a, mut b) = (mesh.clone(), mesh.clone());
            // Step after step, with something moving the points between
            // steps as the rest of a chain would.
            for step in 0..6 {
                crate::geometry::apply_detangle(&mut a, &node);
                crate::geometry::apply_detangle_reference(&mut b, &node);
                assert_eq!(a.positions(), b.positions(), "{name}, {params:?}, step {step}");
                for d in [&mut a, &mut b] {
                    for p in 0..d.num_points() {
                        let v = d.pos(p);
                        d.set_pos(p, v + Vec3::new(0.0, -0.01 * v.y.signum(), 0.002 * (p % 3) as f32));
                    }
                }
            }
            if a.positions() != mesh.positions() {
                moved_somewhere += 1;
            }
        }
    }
    assert!(moved_somewhere > 10, "the fixtures tangle, or this proves nothing: {moved_somewhere}");
}

/// What the solve no longer pays for. A surface that touches itself
/// nowhere is one pass and one grid, whatever Iterations says; a group
/// is searched for its own points and no others; the grid is reused
/// across passes while the points stay near where it filed them; and a
/// simulation's steps share one topology.
#[test]
fn the_detangle_solve_skips_what_it_does_not_need() {
    let round = sphere_detail(Vec3::ZERO, 0.5, 10, 14);
    let n = round.num_points();
    let node = phase3_node("detangle", &[("thickness", "1.00"), ("rings", "2"), ("iterations", "8")]);
    let mut d = round.clone();
    let work = crate::detangle::apply(&mut d, &node);
    assert_eq!((work.passes, work.grids, work.searched), (1, 1, n), "nothing to separate: {work:?}");
    assert_eq!(d.positions(), round.positions());

    // A flattened one has work to do, over more than one pass, and does
    // not build a grid for each.
    let mut flat = round.clone();
    for p in 0..n {
        let v = flat.pos(p);
        flat.set_pos(p, Vec3::new(v.x, v.y * 0.04, v.z));
    }
    let mut d = flat.clone();
    let work = crate::detangle::apply(&mut d, &node);
    assert!(work.passes > 1, "{work:?}");
    assert_ne!(d.positions(), flat.positions());
    // Pressed that flat, a pass moves points further than the grid
    // allows for and it is built again. Two sheets a little closer than
    // a thickness part gently, over several passes on one grid.
    let mut sheets = Detail::new();
    let mut rows = Vec::new();
    for sheet in 0..2 {
        let mut row = Vec::new();
        for i in 0..6 {
            for j in 0..6 {
                row.push(sheets.add_point(Vec3::new(i as f32 * 0.25, sheet as f32 * 0.24, j as f32 * 0.25)));
            }
        }
        rows.push(row);
    }
    for sheet in 0..2 {
        for i in 0..5 {
            for j in 0..5 {
                let at = |a: usize, b: usize| rows[sheet][a * 6 + b];
                sheets.add_prim(&[at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
                sheets.add_prim(&[at(i, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
    }
    let work = crate::detangle::apply(&mut sheets, &node);
    assert!(work.passes > 1 && work.passes < 8, "several passes, then the early end: {work:?}");
    assert_eq!(work.grids, 1, "on the one grid: {work:?}");

    // With a group only its points are searched for.
    let mut grouped = flat.clone();
    for p in (0..n).step_by(4) {
        grouped.points_mut().add_to_group("some", p);
    }
    let members = grouped.points().group_len("some");
    let node = phase3_node("detangle", &[("thickness", "1.00"), ("rings", "2"), ("iterations", "1"), ("group", "some")]);
    let work = crate::detangle::apply(&mut grouped, &node);
    assert_eq!(work.searched, members, "{work:?}");

    // Steps of a simulation — fresh clones of one mesh, its points
    // moving — share one topology; another mesh, or another ring count,
    // is another.
    let kept = crate::detangle::kept_topologies();
    let node = phase3_node("detangle", &[("thickness", "1.00"), ("rings", "5"), ("iterations", "2")]);
    let mut step = flat.clone();
    for _ in 0..5 {
        let mut next = step.clone();
        crate::detangle::apply(&mut next, &node);
        step = next;
    }
    assert!(crate::detangle::kept_topologies() <= kept + 1, "five steps, one topology");
}

#[test]
fn test_detangle_is_independent_of_point_order() {
    // Gathered against the start-of-pass positions and applied at the end,
    // so the same tangle untangles the same way however its points are
    // numbered. A Gauss-Seidel sweep would not.
    let mut a = sphere_detail(Vec3::ZERO, 0.5, 6, 8);
    let node = phase3_node("detangle", &[("thickness", "2.00"), ("rings", "1"), ("iterations", "3")]);
    let mut b = a.clone();
    crate::geometry::apply_detangle(&mut a, &node);
    crate::geometry::apply_detangle(&mut b, &node);
    assert_eq!(a.positions(), b.positions());
}

/// A sheet of two large triangles with a fine patch over the middle of
/// one of them: the patch's points are nowhere near any corner of the
/// sheet, which is the case a point-to-point test cannot see. The patch
/// is tilted by `tilt` so its edges straddle the sheet when it is
/// lowered through it, and is the group "patch".
fn sheet_and_patch(height: f32, tilt: f32) -> Detail {
    let mut d = Detail::new();
    let s: Vec<u32> = [(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)]
        .iter()
        .map(|&(x, z)| d.add_point(Vec3::new(x, 0.0, z)))
        .collect();
    d.add_prim(&[s[0], s[2], s[1]]);
    d.add_prim(&[s[0], s[3], s[2]]);
    let mut patch = Vec::new();
    for i in 0..5 {
        for j in 0..5 {
            let (x, z) = (i as f32 * 0.1, j as f32 * 0.1);
            patch.push(d.add_point(Vec3::new(0.9 + x, height + tilt * x, -1.1 + z)));
        }
    }
    for i in 0..4 {
        for j in 0..4 {
            let at = |a: usize, b: usize| patch[a * 5 + b];
            d.add_prim(&[at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
            d.add_prim(&[at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
        }
    }
    for &p in &patch {
        d.points_mut().add_to_group("patch", p as usize);
    }
    d
}

/// How far the patch's nearest point is from the sheet's surface.
fn patch_clearance(d: &Detail) -> f32 {
    let tris = [[0usize, 2, 1], [0, 3, 2]];
    (4..d.num_points())
        .flat_map(|p| tris.iter().map(move |t| (p, *t)))
        .map(|(p, t)| {
            let q = crate::spatial::closest_point_on_triangle(d.pos(p), d.pos(t[0]), d.pos(t[1]), d.pos(t[2]));
            (d.pos(p) - q).length()
        })
        .fold(f32::MAX, f32::min)
}

/// The measure: every edge that passes through a triangle, and the
/// points of both. A surface that does not cross itself has none,
/// quads and shared corners included.
#[test]
fn the_tangle_measure_counts_edges_through_triangles() {
    use crate::detangle::self_intersections;
    assert_eq!(self_intersections(&Detail::new()).crossings, 0);
    assert_eq!(self_intersections(&sphere_detail(Vec3::ZERO, 0.5, 10, 14)).crossings, 0);
    assert_eq!(self_intersections(&box_detail(Vec3::ZERO, Vec3::ONE, 0.2)).crossings, 0, "quads fan into triangles that share a side");
    // The patch above the sheet, then tilted through it.
    assert_eq!(self_intersections(&sheet_and_patch(0.05, 0.0)).crossings, 0);
    let through = self_intersections(&sheet_and_patch(-0.06, 0.3));
    assert!(through.crossings > 0, "{through:?}");
    // The points named are the crossing edges' and the crossed
    // triangle's: some of the patch, not all of it, and the sheet's.
    let of_patch = through.points.iter().filter(|&&p| p >= 4).count();
    assert!(of_patch > 0 && of_patch < 25, "{through:?}");
    assert!(through.points.iter().any(|&p| p < 4), "{through:?}");
    assert!(through.points.windows(2).all(|w| w[0] < w[1]), "ascending, each once");
    // The same answer at a thousandth of the size: the tolerance is
    // relative.
    let mut small = sheet_and_patch(-0.06, 0.3);
    for p in 0..small.num_points() {
        let v = small.pos(p);
        small.set_pos(p, v * 0.001);
    }
    assert_eq!(self_intersections(&small), through);
}

/// What the Surface method is for: a point over the middle of a large
/// triangle is near none of its corners, so Points sees nothing and
/// Surface parts them.
#[test]
fn the_surface_method_sees_a_point_over_the_middle_of_a_triangle() {
    let start = sheet_and_patch(0.03, 0.0);
    let settings = |method: &'static str| {
        phase3_node("detangle", &[("method", method), ("thickness", "0.25"), ("rings", "2"), ("iterations", "8")])
    };
    let mut by_points = start.clone();
    let work = crate::detangle::apply(&mut by_points, &settings("Points"));
    assert_eq!(by_points.positions(), start.positions(), "nothing is within a thickness of a corner: {work:?}");

    let mut by_surface = start.clone();
    let work = crate::detangle::apply(&mut by_surface, &settings("Surface"));
    assert!(work.contacts > 0, "{work:?}");
    let (before, after) = (patch_clearance(&start), patch_clearance(&by_surface));
    assert!((before - 0.03).abs() < 1e-5);
    assert!(after > 0.1, "the patch is {after} from the sheet");
    // The patch moved as one: it was pushed off the sheet, not apart.
    let edge = |d: &Detail| (d.pos(5) - d.pos(4)).length();
    assert!((edge(&by_surface) / edge(&start) - 1.0).abs() < 0.05, "{} to {}", edge(&start), edge(&by_surface));
    // The move is shared, so the sheet gave way too, downward.
    assert!((0..4).all(|p| by_surface.pos(p).y <= 0.0) && (0..4).any(|p| by_surface.pos(p).y < 0.0));

    // With the sheet outside the Group it stays where it is and the
    // patch takes the whole move, not half of it.
    let node = phase3_node(
        "detangle",
        &[("method", "Surface"), ("thickness", "0.25"), ("rings", "2"), ("iterations", "8"), ("group", "patch")],
    );
    let mut held = start.clone();
    crate::detangle::apply(&mut held, &node);
    assert_eq!(held.positions()[..4], start.positions()[..4]);
    assert!(patch_clearance(&held) > 0.1, "{}", patch_clearance(&held));

    // A surface that touches itself nowhere is left alone, in one pass.
    let round = sphere_detail(Vec3::ZERO, 0.5, 10, 14);
    let mut d = round.clone();
    let node = phase3_node("detangle", &[("method", "Surface"), ("thickness", "1.00"), ("rings", "2"), ("iterations", "8")]);
    let work = crate::detangle::apply(&mut d, &node);
    assert_eq!((work.passes, work.contacts), (1, 0), "{work:?}");
    assert_eq!(d.positions(), round.positions());
}

/// The two methods as a simulation runs them: the patch is lowered a
/// little each step, by less than the thickness, toward a sheet that
/// does not move. Points lets it through; Surface holds it off. The
/// measure is what says so, and the node's Tangled Group is the measure
/// written onto the geometry.
#[test]
fn the_surface_method_holds_a_patch_off_a_sheet_step_after_step() {
    let run = |method: &'static str| {
        let node = phase3_node(
            "detangle",
            &[("method", method), ("thickness", "0.25"), ("rings", "2"), ("iterations", "8"), ("group", "patch"), ("tangled_group", "tangled")],
        );
        let mut d = sheet_and_patch(0.2, 0.3);
        let (mut worst, mut marked) = (0, 0);
        for _ in 0..25 {
            for p in 4..d.num_points() {
                let v = d.pos(p);
                d.set_pos(p, v - Vec3::new(0.0, 0.02, 0.0));
            }
            let work = crate::detangle::apply(&mut d, &node);
            assert_eq!(work.crossings, crate::detangle::self_intersections(&d).crossings);
            assert_eq!(work.crossings > 0, d.points().group_len("tangled") > 0);
            worst = worst.max(work.crossings);
            marked = marked.max(d.points().group_len("tangled"));
        }
        (d, worst, marked)
    };
    let (through, worst, marked) = run("Points");
    assert!(worst > 0 && marked > 0, "Points should have let it through: {worst}");
    assert!((4..through.num_points()).all(|p| through.pos(p).y < 0.0), "and out the other side");

    let (held, worst, marked) = run("Surface");
    assert_eq!((worst, marked), (0, 0), "no edge went through at any step");
    assert!((4..held.num_points()).all(|p| held.pos(p).y > 0.0), "the patch is still above the sheet");
    assert!(held.points().has_group("tangled"), "the group is written, empty");
}

/// A point has a side once the solve is told where the step began. The
/// patch is carried through a sheet that does not move, in one step:
/// by less than a thickness, where distance alone sees it near the
/// sheet and pushes it on the way it was going; and by several, where
/// distance alone sees nothing at all.
#[test]
fn the_surface_method_puts_back_what_went_through() {
    let node = phase3_node(
        "detangle",
        &[("method", "Surface"), ("thickness", "0.25"), ("rings", "2"), ("iterations", "8"), ("group", "patch")],
    );
    let above = |d: &Detail| (4..d.num_points()).filter(|&p| d.pos(p).y > 0.0).count();
    for (from, to) in [(0.04, -0.04), (0.3, -0.5)] {
        let before = sheet_and_patch(from, 0.0);
        let carried = sheet_and_patch(to, 0.0);

        let mut blind = carried.clone();
        crate::detangle::apply(&mut blind, &node);
        assert_eq!(above(&blind), 0, "{from} to {to}: with no memory the patch stays under the sheet");
        assert!(blind.pos(4).y <= carried.pos(4).y, "and is pushed no nearer to it");

        let mut told = carried.clone();
        let work = crate::detangle::apply_from(&mut told, Some(&before), &node);
        assert_eq!(above(&told), 25, "{from} to {to}: {work:?}");
        assert!(work.crossed > 0 || work.held > 0, "{work:?}");
        assert!(patch_clearance(&told) > 0.03, "clear of the sheet: {}", patch_clearance(&told));
        assert_eq!(crate::detangle::self_intersections(&told).crossings, 0);
        assert_eq!(told.positions()[..4], carried.positions()[..4], "the sheet is outside the group");

        // A `before` that is another mesh is no memory of this one.
        let mut other = carried.clone();
        let work = crate::detangle::apply_from(&mut other, Some(&sphere_detail(Vec3::ZERO, 0.5, 6, 8)), &node);
        assert_eq!(other.positions(), blind.positions());
        assert_eq!((work.crossed, work.held), (0, 0));
    }

    // What a point went AROUND it did not go through: carried past the
    // sheet's edge and under it, the patch is left where it is.
    let shift = |d: &mut Detail, by: Vec3| {
        for p in 4..d.num_points() {
            let v = d.pos(p);
            d.set_pos(p, v + by);
        }
    };
    let (mut before, mut carried) = (sheet_and_patch(0.5, 0.0), sheet_and_patch(-0.5, 0.0));
    shift(&mut before, Vec3::new(2.0, 0.0, 0.0));
    shift(&mut carried, Vec3::new(2.0, 0.0, 0.0));
    let mut told = carried.clone();
    let work = crate::detangle::apply_from(&mut told, Some(&before), &node);
    assert_eq!(told.positions(), carried.positions(), "{work:?}");
}

/// Two triangles at right angles, an edge of each facing an edge of the
/// other across `gap`: the first lies flat with its edge along x, the
/// second stands upright with its edge along y, in front of it. No
/// point of either is anywhere near the other triangle — the edges are
/// nearest at their middles — so this is the contact only an edge test
/// can see. The second triangle is the group "upright".
fn crossed_triangles(gap: f32) -> Detail {
    let mut d = Detail::new();
    let flat = [Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 2.0)].map(|p| d.add_point(p));
    let upright = [Vec3::new(0.0, -1.0, -gap), Vec3::new(0.0, 1.0, -gap), Vec3::new(0.0, 0.0, -2.0 - gap)].map(|p| d.add_point(p));
    d.add_prim(&flat);
    d.add_prim(&upright);
    for p in upright {
        d.points_mut().add_to_group("upright", p as usize);
    }
    d
}

/// Edge contact: two edges near each other, or through each other,
/// where no point is near or through any triangle. Without it the
/// Surface method sees nothing in either, and the second is a crossing
/// the measure counts.
#[test]
fn edge_contact_parts_edges_no_point_test_can_see() {
    let settings = |edges: &'static str, group: &'static str| {
        phase3_node(
            "detangle",
            &[("method", "Surface"), ("thickness", "0.20"), ("rings", "2"), ("iterations", "8"), ("edge_contact", edges), ("group", group)],
        )
    };
    let gap = |d: &Detail| d.pos(0).z - d.pos(3).z;
    let near = crossed_triangles(0.1);
    let edges = near.edges();
    let thickness = 0.2 * edges.iter().map(|e| (near.pos(e[1] as usize) - near.pos(e[0] as usize)).length()).sum::<f32>() / edges.len() as f32;
    assert!(thickness > 0.3 && thickness < 0.9, "more than the gap, less than any point is from the other triangle: {thickness}");

    let mut without = near.clone();
    let work = crate::detangle::apply(&mut without, &settings("false", ""));
    assert_eq!((work.contacts, without.positions()), (0, near.positions()));
    // A node from before the row is one without it.
    let mut before_the_row = near.clone();
    crate::detangle::apply(&mut before_the_row, &phase3_node("detangle", &[("method", "Surface"), ("thickness", "0.20")]));
    assert_eq!(before_the_row.positions(), near.positions());

    let mut with = near.clone();
    let work = crate::detangle::apply(&mut with, &settings("true", ""));
    assert!(work.edge_contacts > 0 && work.edge_contacts == work.contacts, "{work:?}");
    assert!(gap(&with) > thickness * 0.95, "parted to {} of {thickness}", gap(&with));
    // The move is shared by the four ends and no one else.
    assert!(with.pos(0).z > 0.0 && with.pos(3).z < -0.1);
    assert_eq!((with.pos(2), with.pos(5)), (near.pos(2), near.pos(5)));
    // Outside the group the flat triangle stays, and the upright one
    // takes the whole move.
    let mut held = near.clone();
    crate::detangle::apply(&mut held, &settings("true", "upright"));
    assert_eq!(held.positions()[..3], near.positions()[..3]);
    assert!(gap(&held) > thickness * 0.95, "{}", gap(&held));

    // Carried through: the upright edge from in front of the flat one
    // to behind it, by less than a thickness and by several.
    for to in [-0.1f32, -1.2] {
        let carried = crossed_triangles(to);
        assert!(crate::detangle::self_intersections(&carried).crossings > 0, "to {to}: the fixture crosses");
        for (edges, told, parted) in [("false", true, false), ("true", false, false), ("true", true, true)] {
            let mut d = carried.clone();
            let work = crate::detangle::apply_from(&mut d, told.then_some(&near), &settings(edges, "upright"));
            let crossings = crate::detangle::self_intersections(&d).crossings;
            assert_eq!(crossings == 0 && gap(&d) > 0.0, parted, "to {to}, edges {edges}, told {told}: gap {}, {crossings} crossings, {work:?}", gap(&d));
            if parted {
                assert!(work.crossed > 0 || work.held > 0, "{work:?}");
                assert_eq!(d.positions()[..3], carried.positions()[..3]);
            }
        }
    }

    // An edge with neither end near anything is not tested at all.
    let round = crate::shapes::sphere_node_detail(
        &phase3_node("sphere", &[("method", "Icosphere"), ("frequency", "8"), ("radius", "0.5")]),
        Some(Vec3::ZERO),
    );
    let mut d = round.clone();
    let work = crate::detangle::apply(&mut d, &phase3_node("detangle", &[("method", "Surface"), ("thickness", "0.50"), ("rings", "2"), ("edge_contact", "true")]));
    assert_eq!((work.edges_searched, work.contacts), (0, 0), "{work:?}");
    assert_eq!(d.positions(), round.positions());
}

/// A fold: a point carried through a triangle of its own neighbourhood.
/// The rings are excluded from contact, since a neighbour is nearer
/// than a thickness by construction — but going through is not a
/// distance, and told where the step began the solve puts the point
/// back over its neighbour, as far over it as it was.
#[test]
fn fold_contact_puts_back_what_went_through_its_own_neighbourhood() {
    // A shallow bowl, so that nothing lies in its neighbour's plane.
    let at = |i: usize, j: usize| (i * 6 + j) as u32;
    let bowl = {
        let mut d = Detail::new();
        for i in 0..6 {
            for j in 0..6 {
                let (x, z) = (i as f32 - 2.5, j as f32 - 2.5);
                d.add_point(Vec3::new(x * 0.25, 0.02 * (x * x + z * z), z * 0.25));
            }
        }
        for i in 0..5 {
            for j in 0..5 {
                d.add_prim(&[at(i, j), at(i + 1, j + 1), at(i + 1, j)]);
                d.add_prim(&[at(i, j), at(i, j + 1), at(i + 1, j + 1)]);
            }
        }
        d.points_mut().add_to_group("mover", at(2, 2) as usize);
        d
    };
    let mover = at(2, 2) as usize;
    // A triangle one hop away: it has the mover's neighbour for a corner.
    let tri = [at(3, 3), at(4, 4), at(4, 3)].map(|p| p as usize);
    let over = |d: &Detail| {
        let [a, b, c] = tri.map(|p| d.pos(p));
        (d.pos(mover) - a).dot((b - a).cross(c - a).normalize())
    };
    let began = over(&bowl);
    assert!(began > 0.01, "the mover begins over its neighbour: {began}");
    let mut carried = bowl.clone();
    let [a, b, c] = tri.map(|p| bowl.pos(p));
    carried.set_pos(mover, (a + b + c) / 3.0 - (b - a).cross(c - a).normalize() * 0.04);
    assert!(over(&carried) < 0.0);
    assert!(crate::detangle::self_intersections(&carried).crossings > 0, "the fixture crosses");
    assert_eq!(crate::detangle::crossings_beyond(&carried, 2), 0, "and inside the rings, where contact does not look");

    let settings = |folds: &'static str| {
        phase3_node(
            "detangle",
            &[("method", "Surface"), ("thickness", "1.00"), ("rings", "2"), ("iterations", "8"), ("edge_contact", "true"), ("fold_contact", folds), ("group", "mover")],
        )
    };
    // Without it, and with it but not told where the step began, the
    // fold stays.
    for (node, told) in [(settings("false"), true), (settings("true"), false), (phase3_node("detangle", &[("method", "Surface"), ("group", "mover")]), true)] {
        let mut d = carried.clone();
        let work = crate::detangle::apply_from(&mut d, told.then_some(&bowl), &node);
        assert_eq!(work.folds, 0);
        assert!(over(&d) < 0.0, "told {told}: {work:?}");
    }
    let mut d = carried.clone();
    let work = crate::detangle::apply_from(&mut d, Some(&bowl), &settings("true"));
    assert!(work.folds > 0, "{work:?}");
    assert!(over(&d) > 0.0, "back over its neighbour: {}, {work:?}", over(&d));
    // As far over it as it began and no further: a neighbour is nearer
    // than a thickness, and is not pushed out to one.
    assert!(over(&d) < began * 1.5 + 1e-4, "{} against {began}", over(&d));
    assert_eq!(crate::detangle::self_intersections(&d).crossings, 0);
    for p in (0..d.num_points()).filter(|&p| p != mover) {
        assert_eq!(d.pos(p), carried.pos(p), "only the group moves");
    }

    // A step that folds nothing finds nothing and holds nothing.
    let mut still = bowl.clone();
    for p in 0..still.num_points() {
        let v = still.pos(p);
        still.set_pos(p, v + Vec3::new(0.01, 0.02 * v.x, 0.0));
    }
    let moved = still.clone();
    let mut node = settings("true");
    node.params.retain(|p| p.name != "group");
    let work = crate::detangle::apply_from(&mut still, Some(&bowl), &node);
    assert_eq!((work.folds, work.held), (0, 0), "{work:?}");
    assert_eq!(still.positions(), moved.positions());
}

/// MCP's `enter` dives into exactly what the keyboard does: a simnet
/// with no children yet is a container, a native node is not.
#[test]
fn mcp_enter_agrees_with_the_keyboard_about_what_is_enterable() {
    let mut state = State::new(false);
    let node = |name: &str, ty: &str| crate::app::FsNode {
        id: format!("{name}-id"),
        name: name.to_string(),
        node_type: ty.to_string(),
        children: vec![],
        params: vec![],
        geometry_visible: false,
        bypassed: false,
        position: (0.0, 0.0),
        inputs: 1,
        outputs: 1,
    };
    state.current_path.clear();
    state.fs_root.children.push(node("sim_empty", "simnet"));
    state.fs_root.children.push(node("remesh_leaf", "remesh"));
    let mut redraw = false;
    for (i, child) in state.fs_root.children.clone().iter().enumerate() {
        let entered = state.apply_action(McpAction::Enter { slot: i }, &mut redraw).is_ok();
        assert_eq!(entered, child.is_enterable(), "{} ({})", child.name, child.node_type);
        if entered {
            state.apply_action(McpAction::Up, &mut redraw).unwrap();
        }
    }
    let names: Vec<_> = state.fs_root.children.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"sim_empty") && names.contains(&"remesh_leaf"));
}

/// A bypassed node is in the graph and does nothing: what reads it gets
/// what it reads. A generator, which reads nothing, gives nothing; a
/// subnet passes its Input and its children are not run; and a node
/// whose own parameters would fail is not evaluated to find that out.
#[test]
fn a_bypassed_node_passes_its_input_through() {
    let pull = |bypassed: bool| {
        let mut n = ref_node(
            "pull",
            "pull1",
            "attribute",
            vec![("input", "node", "sphere1"), ("operation", "text", "Modify"), ("attribute_name", "text", "Pos"), ("value", "text", "1.00:0.00:0.00"), ("combine", "text", "Add")],
            vec![],
        );
        n.bypassed = bypassed;
        n
    };
    let sphere = || ref_node("s", "sphere1", "sphere", vec![("radius", "slider", "0.5"), ("center_x", "slider", "0"), ("center_y", "slider", "0"), ("center_z", "slider", "0")], vec![]);
    let root = |nodes: Vec<FsNode>| ref_node("root", "root", "node", vec![], nodes);
    let min_x = |d: &Detail| d.positions().iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);

    let (plain, _) = eval_node(&root(vec![sphere()]), "sphere1");
    let (moved, err) = eval_node(&root(vec![sphere(), pull(false)]), "pull1");
    assert!(err.is_none() && (min_x(&moved) - min_x(&plain) - 1.0).abs() < 1e-5);
    let (passed, err) = eval_node(&root(vec![sphere(), pull(true)]), "pull1");
    assert!(err.is_none());
    assert_eq!(passed.positions(), plain.positions());

    // What reads a bypassed node reads through it.
    let mut second = pull(false);
    (second.id, second.name) = ("pull2".into(), "pull2".into());
    second.params[0].set_text("pull1");
    let (after, _) = eval_node(&root(vec![sphere(), pull(true), second]), "pull2");
    assert!((min_x(&after) - min_x(&plain) - 1.0).abs() < 1e-5, "one pull, not two");

    // A generator reads nothing and gives nothing.
    let mut off = sphere();
    off.bypassed = true;
    let tree = root(vec![off]);
    let mut cache = crate::geometry::SimCache::default();
    let mut sim = crate::geometry::EvalSim::new(0, 0, &mut cache);
    let mut err = None;
    assert!(crate::geometry::generate_single_node_geometry_with_errors(&tree, &tree.children[0], &mut Vec::new(), &mut err, &mut sim).is_none());
    assert!(err.is_none());

    // Its own parameters are not evaluated: an expression that names
    // nothing is an error on the node, and not on a bypassed one.
    let broken = |bypassed: bool| {
        let mut n = pull(bypassed);
        n.params.push(crate::app::ParamDef::new("strength", "slider", "ch(\"../nothing/here\")").as_expr());
        root(vec![sphere(), n])
    };
    assert!(eval_node(&broken(false), "pull1").1.is_some());
    let (passed, err) = eval_node(&broken(true), "pull1");
    assert!(err.is_none(), "{err:?}");
    assert_eq!(passed.positions(), plain.positions());

    // A subnet passes its Input, and what is inside it is not run.
    let mut subnet = ref_node(
        "sub",
        "sub1",
        "node",
        vec![("input", "node", "sphere1")],
        vec![
            ref_node("in", "input1", "input", vec![], vec![]),
            { let mut p = pull(false); p.params[0].set_text("input1"); p },
            ref_node("out", "output1", "output", vec![("input", "node", "pull1")], vec![]),
        ],
    );
    let (through, _) = eval_node(&root(vec![sphere(), subnet.clone()]), "sub1");
    assert!((min_x(&through) - min_x(&plain) - 1.0).abs() < 1e-5);
    subnet.bypassed = true;
    let (passed, _) = eval_node(&root(vec![sphere(), subnet.clone()]), "sub1");
    assert_eq!(passed.positions(), plain.positions());

    // The flag says nothing on a subnet's plumbing.
    subnet.bypassed = false;
    subnet.children[0].bypassed = true;
    subnet.children[2].bypassed = true;
    let (through, _) = eval_node(&root(vec![sphere(), subnet]), "sub1");
    assert!((min_x(&through) - min_x(&plain) - 1.0).abs() < 1e-5);

    // The scene draws a shown, bypassed node as what it passes, and a
    // bypassed generator as nothing.
    let scene = |nodes: Vec<FsNode>| {
        let tree = root(nodes);
        let mut cache = crate::geometry::SimCache::default();
        let mut sim = crate::geometry::EvalSim::new(0, 0, &mut cache);
        crate::geometry::network_sphere_vertices_with_errors(&tree, &tree, &mut None, &mut sim)
    };
    let hidden = |mut n: FsNode| { n.geometry_visible = false; n };
    assert_eq!(scene(vec![hidden(sphere()), pull(true)]).positions(), plain.positions());
    assert!((min_x(&scene(vec![hidden(sphere()), pull(false)])) - min_x(&plain) - 1.0).abs() < 1e-5);
    let mut off = sphere();
    off.bypassed = true;
    assert_eq!(scene(vec![off]).num_points(), 0);
}

/// Bypass in the 2D context, which has a resolver of its own: a
/// bypassed page node passes the sheet it was handed, a bypassed sheet
/// is no sheet, and a bypassed Export — the one node in both contexts —
/// passes either.
#[test]
fn a_bypassed_page_node_passes_its_sheet_through() {
    use crate::page::{displayed_page, resolve_page};
    let node = |id: &str, name: &str, ty: &str, params: &[(&str, &str)]| {
        ref_node(id, name, ty, params.iter().map(|&(n, v)| (n, "text", v)).collect(), vec![])
    };
    let chain = |bypassed: &[&str]| {
        let mut nodes = vec![
            node("p", "page1", "page", &[("preset", "Letter"), ("resolution", "72"), ("color", "1.00:1.00:1.00")]),
            node("g", "grid1", "page_grid", &[("input", "page1"), ("cell_size", "0.5"), ("line_width", "0.02"), ("line_color", "0.00:0.00:0.00"), ("fill_cells", "false")]),
            node("b", "border1", "page_border", &[("input", "grid1"), ("width", "0.1"), ("inset", "0.25"), ("color", "1.00:0.00:0.00")]),
            node("e", "export1", "export", &[("input", "border1")]),
        ];
        for n in &mut nodes {
            n.bypassed = bypassed.contains(&n.name.as_str());
        }
        ref_node("r", "root", "node", vec![], nodes)
    };
    let sheet = |root: &FsNode, name: &str| resolve_page(root, root.children.iter().find(|c| c.name == name).unwrap(), &mut Vec::new());
    // Where the border's ink is, and where a rule of the grid is:
    // green is what tells red ink and black ink from the white sheet.
    let border = |p: &crate::page::Page| p.pixels[(400 * p.width + 20) as usize];
    let rule = |p: &crate::page::Page| p.pixels[(400 * p.width + 36 * 4) as usize];
    // Said, not shown: a sheet that differs is half a million pixels.
    let same = |a: &crate::page::Page, b: &crate::page::Page, what: &str| {
        let differ = a.pixels.iter().zip(&b.pixels).filter(|(x, y)| x != y).count();
        assert!(a.pixels.len() == b.pixels.len() && differ == 0, "{what}: {differ} of {} pixels differ", a.pixels.len());
    };

    let whole = sheet(&chain(&[]), "border1").expect("the chain resolves");
    assert!(border(&whole)[0] > 0.9 && border(&whole)[1] < 0.1, "the border is red: {:?}", border(&whole));
    assert!(rule(&whole)[1] < 0.4, "the grid ruled the sheet: {:?}", rule(&whole));

    // The border bypassed: the grid's sheet, as the grid made it.
    let no_border = sheet(&chain(&["border1"]), "border1").expect("passes the grid's sheet");
    same(&no_border, &sheet(&chain(&[]), "grid1").unwrap(), "the border bypassed");
    assert!(border(&no_border)[1] > 0.9, "no border ink: {:?}", border(&no_border));
    assert!(rule(&no_border)[1] < 0.4, "and the rules are still there");

    // The grid bypassed, in the middle: the border on a sheet with no
    // rules.
    let no_grid = sheet(&chain(&["grid1"]), "border1").expect("the border reads through the grid");
    assert!(border(&no_grid)[0] > 0.9 && border(&no_grid)[1] < 0.1);
    assert!(rule(&no_grid)[1] > 0.9, "no rule: {:?}", rule(&no_grid));
    assert_eq!((no_grid.width, no_grid.height), (whole.width, whole.height));

    // Both: the sheet itself.
    let bare = sheet(&chain(&["grid1", "border1"]), "border1").expect("the sheet");
    same(&bare, &sheet(&chain(&[]), "page1").unwrap(), "the grid and the border bypassed");

    // A bypassed sheet is no sheet, and nothing drawn on it is a page.
    assert!(sheet(&chain(&["page1"]), "page1").is_none());
    assert!(sheet(&chain(&["page1"]), "border1").is_none());

    // Export passes a page through, bypassed or not.
    same(&sheet(&chain(&[]), "export1").unwrap(), &whole, "through an export");
    same(&sheet(&chain(&["export1"]), "export1").unwrap(), &whole, "through a bypassed export");

    // What the pane shows is the level's last shown page node, and a
    // bypassed one shows what it passes.
    let mut root = chain(&["border1"]);
    root.children.pop();
    same(&displayed_page(&root, &root).expect("displayed"), &no_border, "what the pane shows");
}

/// The flag is written only when it is set, so a file that never
/// bypassed anything is byte for byte the file it was — and a simnet's
/// solve, keyed by its JSON, restarts when a node in its chain is
/// bypassed and not otherwise.
#[test]
fn the_bypass_flag_is_saved_only_when_it_is_set() {
    let mut node = ref_node("a", "a1", "sphere", vec![("radius", "slider", "0.5")], vec![]);
    let plain = serde_json::to_string(&node).unwrap();
    assert!(!plain.contains("bypassed"), "{plain}");
    node.bypassed = true;
    let set = serde_json::to_string(&node).unwrap();
    assert!(set.contains("\"bypassed\":true"), "{set}");
    let back: FsNode = serde_json::from_str(&set).unwrap();
    assert!(back.bypassed);
    let old: FsNode = serde_json::from_str(&plain).unwrap();
    assert!(!old.bypassed);
}

/// Bypass from each place that asks for it — the `b` command on the
/// network's selection, the node's menu, MCP — and the scene, the
/// status line and the node's look follow.
#[test]
fn bypass_is_one_flag_however_it_is_asked_for() {
    use crate::app::McpAction;
    // An empty Geometry node: the bundled project's, emptied.
    let mut state = State::new(false);
    let mut redraw = false;
    assert_eq!(state.current_path.len(), 1, "the bundled project opens in its Geometry node");
    state.current_dir_mut().children.clear();
    state.sync_nodes();
    state.apply_action(McpAction::AddNode { template_name: "Sphere".into(), name: Some("ball".into()), x: 3.0, y: 3.0 }, &mut redraw).unwrap();
    state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 3.0, y: 4.0 }, &mut redraw).unwrap();
    let slot = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).unwrap();
    let (ball, pull) = (slot(&state, "ball"), slot(&state, "pull1"));
    for (name, value) in [("input", "ball"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "1.00:0.00:0.00"), ("combine", "Add")] {
        state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
    }
    state.apply_action(McpAction::ToggleGeometry { slot: pull }, &mut redraw).unwrap();
    let min_x = |state: &State| state.rt_sphere_verts.iter().map(|v| v.position[0]).fold(f32::INFINITY, f32::min);
    let shown = min_x(&state);

    // MCP.
    let said = state.apply_action(McpAction::ToggleBypass { slot: pull }, &mut redraw).unwrap();
    assert_eq!(said, "Bypassed: true");
    assert!(state.current_dir().children[pull].bypassed);
    assert!((min_x(&state) - (shown - 1.0)).abs() < 1e-4, "the scene is the sphere where it was: {} from {shown}", min_x(&state));
    assert_eq!(state.last_status_text, "Bypassed pull1.");
    assert!(state.apply_action(McpAction::ToggleBypass { slot: 99 }, &mut redraw).is_err());

    // The command, on the selection, and only with the network focused.
    state.apply_action(McpAction::Select { slot: pull }, &mut redraw).unwrap();
    state.focused_pane = crate::slots::PARAM_IDX;
    state.run_command("bypass_node");
    assert!(state.current_dir().children[pull].bypassed, "a `b` typed elsewhere is a letter");
    state.focused_pane = LEFT_MENUBAR_IDX;
    state.run_command("bypass_node");
    assert!(!state.current_dir().children[pull].bypassed);
    assert!((min_x(&state) - shown).abs() < 1e-4);
    assert_eq!(state.last_status_text, "No longer bypassing pull1.");
    assert_eq!(crate::command::by_id("bypass_node").unwrap().default_chord, Some("b"));

    // The node's menu, which says which way it will go.
    assert_eq!(state.set_bypassed(&[ball, pull], true), 2);
    assert_eq!(state.set_bypassed(&[ball, pull], true), 0, "already");
    assert!(state.rt_sphere_verts.is_empty(), "a bypassed sphere behind a bypassed pull is nothing");
    assert_eq!(state.set_bypassed(&[ball], false), 1);

    // It is saved with the project and comes back with it.
    let saved = serde_json::to_string(state.current_dir()).unwrap();
    let back: FsNode = serde_json::from_str(&saved).unwrap();
    assert!(back.children[pull].bypassed && !back.children[ball].bypassed);
}

/// The Step Limit holds a point's move since the step began to that
/// many thicknesses, in the direction it was going. It is the Surface
/// method's, it needs to know where the step began, and only what may
/// move is held.
#[test]
fn the_step_limit_holds_a_step_to_a_length() {
    let settings = |method: &'static str, limit: &'static str| {
        phase3_node(
            "detangle",
            &[("method", method), ("thickness", "0.25"), ("rings", "2"), ("iterations", "4"), ("step_limit", limit), ("group", "patch")],
        )
    };
    let before = sheet_and_patch(1.0, 0.0);
    let mut carried = before.clone();
    for p in 0..carried.num_points() {
        let v = carried.pos(p);
        carried.set_pos(p, v + Vec3::new(0.3, 0.0, 0.4));
    }
    let mut d = carried.clone();
    let work = crate::detangle::apply_from(&mut d, Some(&before), &settings("surface", "0.50"));
    assert_eq!(work.limited, 25, "{work:?}");
    let went = d.pos(10) - before.pos(10);
    assert!((went.normalize() - Vec3::new(0.6, 0.0, 0.8)).length() < 1e-4, "the way it was going: {went:?}");
    // Half of a thickness that is a quarter of the mean edge, as the
    // mesh now stands.
    let edges = carried.edges();
    let edge = edges.iter().map(|e| (carried.pos(e[1] as usize) - carried.pos(e[0] as usize)).length()).sum::<f32>() / edges.len() as f32;
    assert!((went.length() - 0.5 * 0.25 * edge).abs() < 1e-5, "{} of an edge of {edge}", went.length());
    assert_eq!(d.positions()[..4], carried.positions()[..4], "the sheet is outside the group");

    for (node, told) in [(settings("surface", "0"), true), (settings("surface", "0.50"), false), (settings("points", "0.50"), true)] {
        let mut d = carried.clone();
        let work = crate::detangle::apply_from(&mut d, told.then_some(&before), &node);
        assert_eq!(work.limited, 0);
        assert_eq!(d.positions(), carried.positions());
    }
}

/// The Detangle node's settings tried on a PROJECT: the file named by
/// `CCE_DETANGLE_PROJECT` (a project directory or its state.json), read
/// and never written, its first simnet played forward a frame at a
/// time to `CCE_DETANGLE_FRAMES` (240) with the detangle node inside it
/// set each way in turn. Says what crossed, when, and what a frame
/// cost. Run in release with `--ignored --nocapture`.
#[test]
#[ignore]
fn detangle_on_a_project() {
    let Ok(path) = std::env::var("CCE_DETANGLE_PROJECT") else {
        println!("CCE_DETANGLE_PROJECT is not set");
        return;
    };
    let frames: i32 = std::env::var("CCE_DETANGLE_FRAMES").ok().and_then(|f| f.parse().ok()).unwrap_or(240);
    let path = std::path::PathBuf::from(path);
    let file = if path.is_dir() { path.join("state.json") } else { path };
    let mut proj: crate::app::Project = serde_json::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
    let templates = crate::app::flatten_node_templates(&crate::app::load_fs_tree());
    proj.sanitize_node_names();
    proj.migrate_format();
    crate::app::merge_template_defs(&mut proj.root, &templates);

    fn find<'a>(n: &'a mut FsNode, ty: &str) -> Option<&'a mut FsNode> {
        if n.node_type == ty {
            return Some(n);
        }
        n.children.iter_mut().find_map(|c| find(c, ty))
    }
    let saved: Vec<(String, String)> = find(&mut proj.root, "detangle").expect("a detangle node").params.iter().map(|p| (p.name.clone(), p.text().to_string())).collect();
    println!("as loaded: {saved:?}");
    let ways: [(&str, &[(&str, &str)]); 6] = [
        ("off (thickness 0)", &[("method", "Points"), ("thickness", "0.00")]),
        ("points", &[("method", "Points")]),
        ("surface", &[("method", "Surface"), ("edge_contact", "false"), ("fold_contact", "false")]),
        ("surface, edges", &[("method", "Surface"), ("edge_contact", "true"), ("fold_contact", "false")]),
        ("surface, folds", &[("method", "Surface"), ("edge_contact", "false"), ("fold_contact", "true")]),
        ("all", &[("method", "Surface"), ("edge_contact", "true"), ("fold_contact", "true")]),
    ];
    let out = std::env::var("CCE_DETANGLE_OUT").ok();
    for (name, settings) in ways {
        let node = find(&mut proj.root, "detangle").unwrap();
        for (param, text) in saved.iter().map(|(a, b)| (a.as_str(), b.as_str())).chain(settings.iter().copied()) {
            node.params.iter_mut().find(|p| p.name == param).expect("the template's row").set_text(text);
        }
        let root = &proj.root;
        let simnet = crate::geometry::find_node_by_name(root, "simnet1").expect("simnet1");
        let mut cache = crate::geometry::SimCache::default();
        let (mut worst, mut first, mut last, mut spent, mut moved) = (0, None, 0, std::time::Duration::ZERO, 0.0f32);
        let mut began: Option<Detail> = None;
        for frame in 1..=frames {
            let mut sim = crate::geometry::EvalSim::new(frame, 1, &mut cache);
            let mut err = None;
            let t = std::time::Instant::now();
            let d = crate::geometry::generate_single_node_geometry_with_errors(root, simnet, &mut Vec::new(), &mut err, &mut sim).expect("solves");
            spent += t.elapsed();
            assert!(err.is_none(), "{err:?}");
            let crossings = crate::detangle::self_intersections(&d).crossings;
            if crossings > 0 && first.is_none() {
                first = Some(frame);
            }
            (worst, last) = (worst.max(crossings), crossings);
            let began = began.get_or_insert_with(|| d.clone());
            moved = (0..d.num_points()).map(|p| (d.pos(p) - began.pos(p)).length()).fold(0.0, f32::max);
            if let (Some(out), true) = (&out, frame == frames) {
                let file = std::path::PathBuf::from(out).join(format!("{}.obj", name.replace([' ', ',', '(', ')'], "_")));
                crate::export::write(&d, &file, crate::export::Format::from_path(&file), 1.0).expect("writes");
            }
        }
        println!(
            "{name:>18}: first crossing at frame {first:?}, worst {worst}, at frame {frames} {last}; {:.2} ms a frame; the furthest point went {moved:.3}",
            spent.as_secs_f64() * 1000.0 / frames as f64
        );
    }
}

/// Where a project's simulation spends its time: `simnet1` of the
/// project `CCE_SIM_PROJECT` names played forward to `CCE_SIM_FRAMES`
/// (60) as saved, and again with each node of its chain bypassed in
/// turn, so what a node costs is what the solve saves without it. The
/// file is read and never written. Run in release with `--ignored
/// --nocapture`.
#[test]
#[ignore]
fn sim_profile_on_a_project() {
    let Ok(path) = std::env::var("CCE_SIM_PROJECT") else {
        println!("CCE_SIM_PROJECT is not set");
        return;
    };
    let frames: i32 = std::env::var("CCE_SIM_FRAMES").ok().and_then(|f| f.parse().ok()).unwrap_or(60);
    let path = std::path::PathBuf::from(path);
    let file = if path.is_dir() { path.join("state.json") } else { path };
    let mut proj: crate::app::Project = serde_json::from_str(&std::fs::read_to_string(&file).expect("reads")).expect("parses");
    let templates = crate::app::flatten_node_templates(&crate::app::load_fs_tree());
    proj.sanitize_node_names();
    proj.migrate_format();
    crate::app::merge_template_defs(&mut proj.root, &templates);
    fn simnet(n: &mut FsNode) -> Option<&mut FsNode> {
        if n.node_type == "simnet" {
            return Some(n);
        }
        n.children.iter_mut().find_map(simnet)
    }
    let sim_node = simnet(&mut proj.root).expect("a simnet");
    // Never the disk: this measures the solve.
    if let Some(p) = sim_node.params.iter_mut().find(|p| p.name == "cache") {
        p.set_text("false");
    }
    let sim_name = sim_node.name.clone();
    let chain: Vec<String> = sim_node
        .children
        .iter()
        .filter(|c| !matches!(c.node_type.as_str(), "input" | "output") && !c.bypassed)
        .map(|c| c.name.clone())
        .collect();
    let mut ways: Vec<Option<String>> = vec![None];
    ways.extend(chain.into_iter().map(Some));
    for way in ways {
        let mut proj = proj.clone();
        if let Some(name) = &way {
            simnet(&mut proj.root).unwrap().children.iter_mut().find(|c| &c.name == name).unwrap().bypassed = true;
        }
        let root = &proj.root;
        let node = crate::geometry::find_node_by_name(root, &sim_name).expect("the simnet");
        let mut cache = crate::geometry::SimCache::default();
        let mut times = Vec::new();
        let mut points = Vec::new();
        let mut remeshed = Vec::new();
        for frame in 1..=frames {
            let mut sim = crate::geometry::EvalSim::new(frame, 1, &mut cache);
            let mut err = None;
            crate::remesh::take_edge_changes();
            let t = std::time::Instant::now();
            let d = crate::geometry::generate_single_node_geometry_with_errors(root, node, &mut Vec::new(), &mut err, &mut sim);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            points.push(d.map_or(0, |d| d.num_points()));
            remeshed.push(crate::remesh::take_edge_changes());
        }
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let worst = times.iter().cloned().fold(0.0, f64::max);
        let at = |f: usize| times.get(f - 1).copied().unwrap_or(0.0);
        println!(
            "{:>22}: {mean:7.2} ms a frame, worst {worst:7.2}; frame 2 {:.2}, 10 {:.2}, 30 {:.2}, last {:.2}; points {} -> {}; edges remeshed at frame 2 {}, 30 {}, last {}",
            way.map_or("as saved".to_string(), |n| format!("without {n}")),
            at(2), at(10), at(30), at(frames as usize),
            points.first().unwrap(), points.last().unwrap(),
            remeshed.get(1).copied().unwrap_or(0), remeshed.get(29).copied().unwrap_or(0), remeshed.last().copied().unwrap_or(0),
        );
    }
}

/// What the whole of the Surface method costs where most of a mesh is
/// in contact: the sphere test's workload with every row on,
/// at a fifth of an edge a step. The sum is of every position at every
/// step, which is what says a change to how the solve is RUN left what
/// it does alone. Run in release with `--ignored --nocapture`.
#[test]
#[ignore]
fn detangle_timing() {
    for frequency in ["4", "8", "16"] {
        let sphere = crate::shapes::sphere_node_detail(
            &phase3_node("sphere", &[("method", "Icosphere"), ("frequency", frequency), ("radius", "0.5")]),
            Some(Vec3::ZERO),
        );
        let edges = sphere.edges();
        let edge = edges.iter().map(|e| (sphere.pos(e[1] as usize) - sphere.pos(e[0] as usize)).length()).sum::<f32>() / edges.len() as f32;
        let cap: Vec<usize> = (0..sphere.num_points()).filter(|&p| sphere.pos(p).y > 0.2).collect();
        let rate = edge * 0.2;
        let steps = (1.1 / rate).ceil() as usize;
        let node = phase3_node(
            "detangle",
            &[("method", "Surface"), ("thickness", "1.00"), ("rings", "2"), ("iterations", "4"), ("edge_contact", "true"), ("fold_contact", "true")],
        );
        let mut d = sphere.clone();
        let (mut worst, mut sum, mut spent, mut held, mut finds) = (0, 0.0f64, std::time::Duration::ZERO, 0, 0);
        for _ in 0..steps {
            let before = d.clone();
            for &p in &cap {
                let v = d.pos(p);
                d.set_pos(p, v - Vec3::new(0.0, rate, 0.0));
            }
            let t = std::time::Instant::now();
            let w = crate::detangle::apply_from(&mut d, Some(&before), &node);
            spent += t.elapsed();
            finds += w.grids;
            held += w.held;
            worst = worst.max(crate::detangle::self_intersections(&d).crossings);
            sum += d.positions().iter().flatten().map(|&c| c as f64).sum::<f64>();
        }
        println!(
            "{:>5} points: {:.2} ms a step, {:.1} searches a step; worst {worst} crossings, {held} held, sum {sum:.6}",
            d.num_points(),
            spent.as_secs_f64() * 1000.0 / steps as f64,
            finds as f64 / steps as f64
        );
    }
}

/// The two methods side by side, by the measure and by the clock: a
/// sphere's cap pushed down into its own bowl a little each step, until
/// it would have come out underneath. Run in release with `--ignored
/// --nocapture`. (Not pressed flat by the sign of y: that carries the
/// equator's points past their own neighbours, inside the excluded
/// rings, which no setting of the node is meant to see.)
#[test]
#[ignore]
fn detangle_methods_compared() {
    // The method, whether it is told where the step began, and its
    // Step Limit.
    let ways: [(&str, &str, bool, &str, &str); 8] = [
        ("none", "None", false, "false", "false"),
        ("points", "Points", false, "false", "false"),
        ("surface", "Surface", false, "false", "false"),
        ("surface, edges", "Surface", false, "true", "false"),
        ("sided", "Surface", true, "false", "false"),
        ("sided, edges", "Surface", true, "true", "false"),
        ("sided, folds", "Surface", true, "false", "true"),
        ("all", "Surface", true, "true", "true"),
    ];
    for frequency in ["4", "8", "16"] {
        let sphere = crate::shapes::sphere_node_detail(
            &phase3_node("sphere", &[("method", "Icosphere"), ("frequency", frequency), ("radius", "0.5")]),
            Some(Vec3::ZERO),
        );
        let edges = sphere.edges();
        let edge = edges.iter().map(|e| (sphere.pos(e[1] as usize) - sphere.pos(e[0] as usize)).length()).sum::<f32>() / edges.len() as f32;
        let cap: Vec<usize> = (0..sphere.num_points()).filter(|&p| sphere.pos(p).y > 0.2).collect();
        // In edges a step: a fifth, and then more than a thickness.
        for pace in [0.2f32, 0.8] {
            let rate = edge * pace;
            let steps = (1.1 / rate).ceil() as usize;
            for thickness in ["0.50", "1.00"] {
                for (name, method, told, edges, folds) in ways {
                    let node = phase3_node(
                        "detangle",
                        &[("method", method), ("thickness", thickness), ("rings", "2"), ("iterations", "4"), ("edge_contact", edges), ("fold_contact", folds)],
                    );
                    let mut d = sphere.clone();
                    let (mut worst, mut far, mut spent) = (0, 0, std::time::Duration::ZERO);
                    let mut tally = crate::detangle::Work::default();
                    for _ in 0..steps {
                        let before = d.clone();
                        for &p in &cap {
                            let v = d.pos(p);
                            d.set_pos(p, v - Vec3::new(0.0, rate, 0.0));
                        }
                        if method != "None" {
                            let t = std::time::Instant::now();
                            let w = crate::detangle::apply_from(&mut d, told.then_some(&before), &node);
                            spent += t.elapsed();
                            tally.crossed += w.crossed;
                            tally.held += w.held;
                            tally.edges_searched += w.edges_searched;
                            tally.folds += w.folds;
                        }
                        worst = worst.max(crate::detangle::self_intersections(&d).crossings);
                        far = far.max(crate::detangle::crossings_beyond(&d, 2));
                    }
                    let last = crate::detangle::self_intersections(&d).crossings;
                    println!(
                        "{:>5} points, pace {pace}, {steps:>4} steps, thickness {thickness}, {name:>15}: worst {worst:>5} crossings ({far:>5} beyond the rings), last {last:>5}, {:.2} ms a step; {} put back through, {} held, {} folds",
                        d.num_points(),
                        spent.as_secs_f64() * 1000.0 / steps as f64,
                        tally.crossed,
                        tally.held,
                        tally.folds
                    );
                }
            }
        }
    }
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
        &[("distance_threshold", "0.10"), ("fusion_threshold", "3"), ("counter", "contact")],
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
    let node = phase3_node("suture", &[("distance_threshold", "0.10"), ("counter", "contact")]);
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
