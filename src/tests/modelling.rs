//! Phase 4: the modelling set.

use super::*;

/// Two spheres far apart: two connected pieces, the first much larger.
fn two_pieces() -> Detail {
    let mut d = sphere_detail(Vec3::ZERO, 1.0, 8, 12);
    d.merge(&sphere_detail(Vec3::new(10.0, 0.0, 0.0), 0.3, 4, 6));
    d
}

pub(super) fn eval_node(root: &FsNode, name: &str) -> (Detail, Option<String>) {
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
pub(super) fn modelling_root(radius: &str, nodes: Vec<FsNode>) -> FsNode {
    let mut children = vec![phase3_node("sphere", &[("radius", radius)])];
    children.extend(nodes);
    FsNode {
        id: "root".into(),
        name: "root".into(),
        node_type: "node".into(),
        children,
        params: vec![],
        geometry_visible: true,
        bypassed: false,
        position: (0.0, 0.0),
        inputs: 0,
        outputs: 0,
    }
}

#[test]
fn test_normal_publishes_the_surface_normal_as_data() {
    let root = modelling_root(
        "1.0",
        vec![phase3_node("normal", &[("input", "sphere 1"), ("attribute", "N")])],
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
        vec![phase3_node("normal", &[("input", "sphere 1"), ("flip", "true")])],
    );
    let (f, _) = eval_node(&flipped, "normal 1");
    let (lo, hi) = f.bounds().unwrap();
    let radial = f.pos(0) - (lo + hi) * 0.5;
    assert!(f.points().value("N", 0).unwrap().as_vec3().dot(radial) < 0.0);
}

/// The Normal node's Vertices class writes a normal per CORNER, cusped:
/// the faces around a point that turn less than the Cusp Angle from a
/// corner's own face are averaged into it, the rest are left out. On a
/// box, whose faces meet at 90 degrees, 60 keeps every corner to its
/// own face and 120 rounds them into the points' normals; the points'
/// own attribute is not written, and smooth shading lights each corner
/// by what the node wrote.
#[test]
fn the_normal_node_writes_cusped_vertex_normals() {
    let mut d = Detail::new();
    let p: Vec<u32> = (0..8)
        .map(|i| d.add_point(Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32)))
        .collect();
    // Six quads, wound to face outward.
    for q in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
        d.add_prim(&q.map(|i| p[i]));
    }
    assert!(d.is_closed());
    let face = |prim: usize| {
        let pts = d.prim_points(prim);
        (d.pos(pts[1] as usize) - d.pos(pts[0] as usize)).cross(d.pos(pts[2] as usize) - d.pos(pts[0] as usize)).normalize()
    };
    let points = crate::geometry::point_normals(&d);

    let hard = crate::geometry::vertex_normals(&d, 60.0);
    let soft = crate::geometry::vertex_normals(&d, 120.0);
    let all = crate::geometry::vertex_normals(&d, 180.0);
    let none = crate::geometry::vertex_normals(&d, 0.0);
    assert_eq!(hard.len(), d.num_verts());
    for prim in 0..d.num_prims() {
        for (v, &pt) in d.prim_verts(prim).zip(d.prim_points(prim)) {
            assert!((hard[v] - face(prim)).length() < 1e-5, "vertex {v}: a corner under the cusp keeps to its face");
            assert!((none[v] - face(prim)).length() < 1e-5);
            assert!((soft[v] - points[pt as usize]).length() < 1e-5, "vertex {v}: over it, the faces around the point");
            assert!((all[v] - points[pt as usize]).length() < 1e-5);
        }
    }

    // Through the node.
    let root = modelling_root(
        "1.0",
        vec![phase3_node("normal", &[("input", "sphere 1"), ("class", "Vertices"), ("cusp_angle", "180"), ("flip", "true")])],
    );
    let (g, err) = eval_node(&root, "normal 1");
    assert!(err.is_none(), "{err:?}");
    // The points' N is the sphere's own, as it came in: Vertices writes
    // the vertices and leaves it alone.
    let (input, _) = eval_node(&root, "sphere 1");
    let pn = |d: &crate::detail::Detail| (0..d.num_points()).map(|p| d.points().value("N", p).map(|v| v.as_vec3())).collect::<Vec<_>>();
    assert_eq!(pn(&g), pn(&input), "the points' is not written");
    let n = crate::geometry::own_vertex_normals(&g).expect("a normal a vertex");
    assert_eq!(n.len(), g.num_verts());
    let of_points = crate::geometry::point_normals(&g);
    for (v, &pt) in g.vert_points().iter().enumerate() {
        let got = n.get(v).unwrap().as_vec3();
        assert!((got + of_points[pt as usize]).length() < 1e-4, "vertex {v}: flipped, and its point's at 180");
    }

    // Smooth shading lights a corner by its own normal: cusped to the
    // faces, every corner of a face is lit alike, which point normals
    // on a box never are.
    let mut cusped = d.clone();
    cusped.verts_mut().create("N", crate::detail::AttribValue::Float3([0.0; 3]));
    cusped
        .verts_mut()
        .insert("N", crate::detail::AttribData::Float3(hard.iter().map(|n| n.to_array()).collect()))
        .unwrap();
    let lit = crate::geometry::smooth_lit_vertices(&cusped, crate::environment::Environment::default().sun_direction);
    let plain = crate::geometry::smooth_lit_vertices(&d, crate::environment::Environment::default().sun_direction);
    assert_eq!(lit.len(), plain.len());
    for tri in lit.chunks(3) {
        assert!(tri.iter().all(|c| (c.color[0] - tri[0].color[0]).abs() < 1e-6), "one face, one light");
    }
    assert!(plain.chunks(3).any(|tri| tri.iter().any(|c| (c.color[0] - tri[0].color[0]).abs() > 1e-3)));
}

#[test]
fn test_bounds_measures_into_detail_attributes() {
    let root = modelling_root(
        "2.0",
        vec![phase3_node("bounds", &[("input", "sphere 1"), ("prefix", "bb")])],
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
                    ("input", "points 1"),
                    ("to", "sphere 1"),
                    ("attribute", "dist"),
                    ("direction", "toward"),
                ],
            ),
        ],
    );
    // Put the sample points somewhere known: a Points node in "Line" mode
    // lays them along X.
    root.children[1]
        .params
        .push(crate::app::ParamDef::new("shape", "text", "Line"));

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
        vec![phase3_node("distance", &[("input", "sphere 1"), ("to", "nope")])],
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

    let node = phase3_node("connectivity", &[("attribute", "piece")]);
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
        let n = phase3_node("connectivity", &[("attribute", "piece")]);
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
            phase3_node("connectivity", &[("input", "sphere 1"), ("attribute", "piece")]),
            phase3_node(
                "cull",
                &[
                    ("input", "connectivity 1"),
                    ("attribute", "piece"),
                    ("comparison", "Below"),
                    ("threshold", "1.00"),
                    ("invert", "false"),
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
        .find(|p| p.name == "invert")
        .unwrap()
        .set_text("true");
    let (kept, _) = eval_node(&kept_root, "cull 1");
    assert_eq!(kept.num_points(), sphere_detail(Vec3::ZERO, 1.0, 16, 24).num_points());
    assert!(kept.num_prims() > 0, "the surface survived with its primitives");

    // A missing attribute is reported, and nothing is deleted on a guess.
    let broken = modelling_root(
        "1.0",
        vec![phase3_node("cull", &[("input", "sphere 1"), ("attribute", "nope")])],
    );
    let (g, err) = eval_node(&broken, "cull 1");
    assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
    assert!(g.num_points() > 0, "nothing should be culled on an error");

    // With neither a group nor an attribute there is no selection at all.
    let idle = modelling_root("1.0", vec![phase3_node("cull", &[("input", "sphere 1")])]);
    let (g, _) = eval_node(&idle, "cull 1");
    assert_eq!(g.num_points(), sphere_detail(Vec3::ZERO, 1.0, 16, 24).num_points());
}

#[test]
fn test_transfer_samples_a_field_onto_new_geometry() {
    // A field defined on one sphere, read onto a second one that shares
    // none of its points — which is what happens whenever a chain rebuilds
    // rather than deforms.
    let mut source = sphere_detail(Vec3::ZERO, 1.0, 12, 16);
    source.points_mut().create("mass", AttribValue::Float(0.0));
    for p in 0..source.num_points() {
        let y = source.pos(p).y;
        source.points_mut().set_value("mass", p, AttribValue::Float(y)).unwrap();
    }
    source
        .points_mut()
        .create_kind("scratch", AttribValue::Float(1.0), AttribKind::Derivative);

    let mut dest = sphere_detail(Vec3::ZERO, 1.0, 7, 9);
    assert!(dest.num_points() != source.num_points());

    // Exercised through the same nearest-point walk the node does.
    let src_pos: Vec<Vec3> = (0..source.num_points()).map(|p| source.pos(p)).collect();
    let grid = crate::spatial::PointGrid::build(&src_pos, 0.1);
    dest.points_mut().create("mass", AttribValue::Float(0.0));
    for p in 0..dest.num_points() {
        let (q, _) = grid.nearest(dest.pos(p)).unwrap();
        let v = source.points().value("mass", q as usize).unwrap();
        dest.points_mut().set_value("mass", p, v).unwrap();
    }

    // The field came across: a point's value matches where it sits, to
    // within the source's resolution.
    for p in 0..dest.num_points() {
        let v = dest.points().value("mass", p).unwrap().as_f32();
        assert!((v - dest.pos(p).y).abs() < 0.25, "point {p}: {v} vs y {}", dest.pos(p).y);
    }
}

#[test]
fn test_transfer_respects_its_maximum_distance_and_carries_the_kind() {
    let root = modelling_root(
        "1.0",
        vec![
            phase3_node("points", &[("shape", "Line"), ("points", "6"), ("markers", "false")]),
            phase3_node(
                "transfer",
                &[
                    ("input", "points 1"),
                    ("from", "sphere 1"),
                    ("attributes", "N"),
                    ("maximum_distance", "0.00"),
                ],
            ),
        ],
    );
    let (g, err) = eval_node(&root, "transfer 1");
    assert!(err.is_none(), "{err:?}");
    // No limit: everything finds a nearest source point however far.
    assert!(g.points().has("N"));
    assert!(
        (0..g.num_points()).any(|p| g.points().value("N", p).unwrap().as_vec3() != Vec3::ZERO),
        "nothing was transferred"
    );

    // With a tight limit the sphere is out of reach, so the column exists
    // and stays at the type's zero — present and empty, not missing.
    let mut limited = root.clone();
    limited
        .children
        .iter_mut()
        .find(|c| c.name == "transfer 1")
        .unwrap()
        .params
        .iter_mut()
        .find(|p| p.name == "maximum_distance")
        .unwrap()
        .set_text("0.01");
    let (g, _) = eval_node(&limited, "transfer 1");
    assert!(g.points().has("N"), "the column exists even where nothing was near");
    assert!(
        (0..g.num_points()).all(|p| g.points().value("N", p).unwrap().as_vec3() == Vec3::ZERO),
        "something transferred from out of range"
    );

    // A source it cannot resolve is reported rather than silently doing
    // nothing.
    let broken = modelling_root(
        "1.0",
        vec![phase3_node("transfer", &[("input", "sphere 1"), ("from", "nope")])],
    );
    let (_, err) = eval_node(&broken, "transfer 1");
    assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
}

/// Transfer carries GROUPS as it carries attributes: with Transfer
/// Groups on, each target point takes its nearest source point's
/// membership in the named groups — every group when none is named —
/// joining and leaving alike; off, or on a node from before the row,
/// no group moves. The Remesh node has the same transfer inside it:
/// from its own input when From names nothing, else the node named.
#[test]
fn transfer_carries_groups_and_remesh_has_a_copy() {
    use crate::detail::AttribValue;
    // The rule itself, on hand-built points: four source points along
    // x, the far two in `top`; four targets beside them, all put in
    // `top` beforehand.
    let mut source = Detail::new();
    for x in 0..4 {
        source.add_point(Vec3::new(x as f32, 0.0, 0.0));
    }
    source.points_mut().create("mass", AttribValue::Float(0.0));
    source.points_mut().set_value("mass", 3, AttribValue::Float(7.0)).unwrap();
    source.points_mut().create_group("top");
    source.points_mut().add_to_group("top", 2);
    source.points_mut().add_to_group("top", 3);
    source.points_mut().create_group("other");
    source.points_mut().add_to_group("other", 0);
    let mut target = Detail::new();
    for x in [0.1, 0.9, 2.1, 2.9] {
        target.add_point(Vec3::new(x, 0.0, 0.0));
    }
    for p in 0..4 {
        target.points_mut().add_to_group("top", p);
    }
    let member = |d: &Detail, g: &str| (0..d.num_points()).map(|p| d.points().in_group(g, p)).collect::<Vec<_>>();
    let mut all = target.clone();
    crate::geometry::transfer_onto(&mut all, &source, &[], Some(&[]), 0.0, "");
    assert_eq!(member(&all, "top"), vec![false, false, true, true], "membership is the nearest source point's, joining and leaving");
    assert_eq!(member(&all, "other"), vec![true, false, false, false], "every group, none named");
    assert_eq!(all.points().value("mass", 3), Some(AttribValue::Float(7.0)));
    let mut named = target.clone();
    crate::geometry::transfer_onto(&mut named, &source, &[], Some(&["nope".to_string(), "top".to_string()]), 0.0, "");
    assert_eq!(member(&named, "top"), vec![false, false, true, true]);
    assert!(!named.points().has_group("other"), "only the named");
    let mut none = target.clone();
    crate::geometry::transfer_onto(&mut none, &source, &[], None, 0.0, "");
    assert_eq!(member(&none, "top"), vec![true, true, true, true], "no groups asked, none touched");
    assert!(none.points().has("mass"), "the attributes still are");
    let mut near = target.clone();
    crate::geometry::transfer_onto(&mut near, &source, &[], Some(&[]), 0.15, "");
    assert_eq!(member(&near, "top"), vec![false, false, true, true], "within the limit");
    near.points_mut().add_to_group("top", 1);
    let mut far = target.clone();
    crate::geometry::transfer_onto(&mut far, &source, &[], Some(&[]), 0.05, "");
    assert_eq!(member(&far, "top"), vec![true, true, true, true], "nothing within 0.05: nothing moves");

    // The nodes. A sphere with a group and an attribute is the source.
    let up = phase3_node("group", &[("input", "sphere 1"), ("group_name", "top"), ("mode", "Box"), ("center", "-1.875:1.55:0.00"), ("size", "4.00:2.00:4.00")]);
    let tagged = phase3_node("attribute", &[("input", "group 1"), ("operation", "Create"), ("attribute_name", "mass"), ("type", "Float"), ("value", "3")]);
    let line = phase3_node("points", &[("shape", "Line"), ("points", "6"), ("markers", "false")]);
    let mut transfer = phase3_node("transfer", &[("input", "points 1"), ("from", "attribute 1"), ("attributes", "mass"), ("transfer_groups", "true"), ("groups", ""), ("maximum_distance", "0.00")]);
    let with = |t: FsNode| modelling_root("1.0", vec![up.clone(), tagged.clone(), line.clone(), t]);
    let (src, err) = eval_node(&with(transfer.clone()), "attribute 1");
    assert!(err.is_none(), "{err:?}");
    let members = src.points().group_members("top").len();
    assert!(members > 0 && members < src.num_points(), "a group of part of the sphere: {members}");
    let (g, err) = eval_node(&with(transfer.clone()), "transfer 1");
    assert!(err.is_none(), "{err:?}");
    assert!(g.points().has_group("top") && g.points().has("mass"), "the group and the attribute were carried");

    // Off, no group moves — and a node without the row is off.
    transfer.params.iter_mut().find(|p| p.name == "transfer_groups").unwrap().set_text("false");
    let (g, _) = eval_node(&with(transfer.clone()), "transfer 1");
    assert!(!g.points().has_group("top") && g.points().has("mass"), "nothing carried with the switch off");
    transfer.params.retain(|p| p.name != "transfer_groups" && p.name != "groups");
    let (g, _) = eval_node(&with(transfer.clone()), "transfer 1");
    assert!(!g.points().has_group("top"), "a node from before the row carries none");

    // Remesh's copy: a sphere remeshed coarse, its group read back off
    // its own input — no From, no wire — so the new points carry it.
    // Both ways a remesh is: the native node, and the subnet whose
    // Transfer node reads From through an expression and finds a node
    // beside the subnet, not inside it.
    let rows = [("input", "attribute 1"), ("target_length", "0.5"), ("iterations", "3"), ("relax", "0.0"), ("transfer", "true"), ("from", ""), ("attributes", ""), ("transfer_groups", "true"), ("groups", "")];
    let templates = crate::app::load_fs_tree();
    let mut composed = templates.children.iter().find(|t| t.name == "Remesh").unwrap().clone();
    crate::app::regenerate_node_ids(&mut composed);
    composed.name = "remesh 1".into();
    for (k, v) in rows {
        composed.params.iter_mut().find(|p| p.name == k).unwrap().set_text(v.to_string());
    }
    for remesh in [phase3_node("remesh", &rows), composed] {
    let (g, err) = eval_node(&with(remesh.clone()), "remesh 1");
    assert!(err.is_none(), "{err:?}");
    assert_ne!(g.num_points(), src.num_points(), "the remesh changed the points");
    assert!(g.points().has_group("top") && g.points().has("mass"), "the remesh carried the group and the attribute");
    let carried = g.points().group_members("top").len();
    assert!(carried > 0 && carried < g.num_points(), "the top half, on the new points: {carried} of {}", g.num_points());
    for p in 0..g.num_points() {
        // The group is the top half of the sphere by its box; each new
        // point's membership is its nearest old point's.
        let nearest = (0..src.num_points()).min_by(|&a, &b| src.pos(a).distance(g.pos(p)).partial_cmp(&src.pos(b).distance(g.pos(p))).unwrap()).unwrap();
        assert_eq!(g.points().in_group("top", p), src.points().in_group("top", nearest), "point {p}");
    }
    // Off, the remesh carries what a remesh carries: the group through
    // its splits and collapses, as before this row.
    let mut off = remesh.clone();
    off.params.iter_mut().find(|p| p.name == "transfer").unwrap().set_text("false");
    let (plain, err) = eval_node(&with(off), "remesh 1");
    assert!(err.is_none(), "{err:?}");
    assert!(plain.points().has_group("top"));
    // From another node: the line's points carry no group and no mass,
    // so there is nothing to lay over the remeshed sphere and it is as
    // the remesh left it.
    let mut from_line = remesh.clone();
    from_line.params.iter_mut().find(|p| p.name == "from").unwrap().set_text("points 1");
    let (g, err) = eval_node(&with(from_line), "remesh 1");
    assert!(err.is_none(), "{err:?}");
    assert_eq!(g.points().group_members("top"), plain.points().group_members("top"), "a source without the group leaves it as it was");
    // A From it cannot resolve is said.
    let mut broken = remesh.clone();
    broken.params.iter_mut().find(|p| p.name == "from").unwrap().set_text("nope");
    let (_, err) = eval_node(&with(broken), "remesh 1");
    assert!(err.as_deref().unwrap_or("").contains("nope"), "{err:?}");
    }
}

#[test]
fn test_valence_counts_what_the_remesher_steers_toward() {
    use crate::remesh::{remesh, Settings};
    let root = modelling_root(
        "1.0",
        vec![phase3_node("valence", &[("input", "sphere 1"), ("attribute", "valence")])],
    );
    let (g, err) = eval_node(&root, "valence 1");
    assert!(err.is_none(), "{err:?}");

    for p in 0..g.num_points() {
        let v = g.points().value("valence", p).unwrap().as_f32() as usize;
        assert_eq!(v, g.point_neighbours(p).len(), "point {p}");
    }
    // A UV sphere's poles are the irregular vertices: everything else on a
    // quad sphere has four neighbours.
    let counts: Vec<usize> = (0..g.num_points()).map(|p| g.point_neighbours(p).len()).collect();
    assert!(counts.iter().any(|&c| c > 4), "the poles should be irregular");

    // After remeshing to triangles, six is the regular valence — which is
    // the number the flip pass steers toward, and being able to see it is
    // how a settled mesh is told from an unsettled one.
    let settled = remesh(&g, Settings { target: 0.25, iterations: 6, ..Default::default() });
    let sixes = (0..settled.num_points())
        .filter(|&p| settled.point_neighbours(p).len() == 6)
        .count();
    assert!(
        sixes * 2 > settled.num_points(),
        "only {sixes} of {} points reached valence 6",
        settled.num_points()
    );
}

#[test]
fn test_deform_twists_bends_and_tapers_about_an_axis() {
    let base = sphere_detail(Vec3::ZERO, 1.0, 10, 14);
    let run = |mode: &str, amount: &str| {
        let mut g = base.clone();
        let node = phase3_node("deform", &[("mode", mode), ("axis", "Y"), ("amount", amount)]);
        crate::geometry::apply_deform(&mut g, &node);
        g
    };

    // The middle of the geometry is the still point, and the two ends go
    // opposite ways — which is what makes a twist read as a twist rather
    // than a rotation of the whole thing.
    let twisted = run("Twist", "2.00");
    let top = (0..base.num_points())
        .max_by(|&a, &b| base.pos(a).y.partial_cmp(&base.pos(b).y).unwrap())
        .unwrap();
    let equator = (0..base.num_points())
        .min_by(|&a, &b| base.pos(a).y.abs().partial_cmp(&base.pos(b).y.abs()).unwrap())
        .unwrap();
    assert!((twisted.pos(equator) - base.pos(equator)).length() < 0.15, "the middle moved");
    // A twist keeps every point's distance from the axis.
    for p in 0..base.num_points() {
        let r0 = Vec3::new(base.pos(p).x, 0.0, base.pos(p).z).length();
        let r1 = Vec3::new(twisted.pos(p).x, 0.0, twisted.pos(p).z).length();
        assert!((r0 - r1).abs() < 1e-4, "point {p} changed radius");
        assert!((twisted.pos(p).y - base.pos(p).y).abs() < 1e-4, "point {p} moved along the axis");
    }
    let _ = top;

    // A taper pinches one end and swells the other, and clamps at zero
    // rather than turning the geometry inside out.
    let tapered = run("Taper", "3.90");
    let radius = |d: &Detail, p: usize| Vec3::new(d.pos(p).x, 0.0, d.pos(p).z).length();
    let lower = (0..base.num_points())
        .filter(|&p| base.pos(p).y < -0.8)
        .collect::<Vec<_>>();
    for &p in &lower {
        assert!(radius(&tapered, p) <= radius(&base, p) + 1e-4, "point {p} swelled at the pinched end");
    }

    // A bend moves points along the axis, which neither of the others do.
    let bent = run("Bend", "1.50");
    assert!(
        (0..base.num_points()).any(|p| (bent.pos(p).y - base.pos(p).y).abs() > 0.05),
        "bend did not move anything along the axis"
    );

    // Zero amount is the identity, whatever the mode.
    for mode in ["Twist", "Bend", "Taper"] {
        let still = run(mode, "0.00");
        for p in 0..base.num_points() {
            assert!((still.pos(p) - base.pos(p)).length() < 1e-5, "{mode} moved at zero");
        }
    }
}

#[test]
fn test_points_and_scatter_can_emit_bare_points() {
    // Everything that generates locations drew marker spheres at them,
    // which is right for looking at and wrong for working with: Copy
    // placed one instance per marker VERTEX rather than one per location,
    // because the markers were the only points there were.
    let markers = modelling_root(
        "1.0",
        vec![phase3_node("points", &[("shape", "Line"), ("points", "5")])],
    );
    let (with, _) = eval_node(&markers, "points 1");
    assert!(with.num_prims() > 0, "markers are geometry");
    assert!(with.num_points() > 5);

    let bare = modelling_root(
        "1.0",
        vec![phase3_node(
            "points",
            &[("shape", "Line"), ("points", "5"), ("markers", "false")],
        )],
    );
    let (without, _) = eval_node(&bare, "points 1");
    assert_eq!(without.num_points(), 5, "one point per location");
    assert_eq!(without.num_prims(), 0, "bare points are not geometry");

    // Default is unchanged, so no existing project looks different.
    let defaulted = modelling_root(
        "1.0",
        vec![phase3_node("points", &[("shape", "Line"), ("points", "5")])],
    );
    assert_eq!(eval_node(&defaulted, "points 1").0.num_points(), with.num_points());
}

#[test]
fn test_copy_puts_geometry_at_every_target_point() {
    let root = modelling_root(
        "0.2",
        vec![
            phase3_node("points", &[("shape", "Line"), ("points", "5")]),
            phase3_node("copy", &[("input", "sphere 1"), ("to", "points 1")]),
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
            phase3_node("points", &[("shape", "Line"), ("points", "4")]),
            phase3_node(
                "normal",
                &[("input", "points 1"), ("attribute", "N")],
            ),
            phase3_node(
                "copy",
                &[
                    ("input", "sphere 1"),
                    ("to", "points 1"),
                    ("scale", "2.00"),
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
        .find(|p| p.name == "scale")
        .unwrap()
        .set_text("0.50");
    let (small, _) = eval_node(&root, "copy 1");
    let small_size = small.bounds().map(|(a, b)| (b - a).length()).unwrap();
    assert!(small_size < plain_size, "{small_size} should be under {plain_size}");
    assert_eq!(small.num_points(), plain.num_points());

    // A copy big enough to hang the app is refused with a number rather
    // than attempted.
    let huge = modelling_root(
        "1.0",
        vec![
            phase3_node("points", &[("shape", "Grid"), ("points", "9000")]),
            phase3_node("copy", &[("input", "sphere 1"), ("to", "points 1")]),
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
            ("translation", "0.00:1.00:0.00"),
            ("group", "top"),
            ("radius", "0.80"),
            ("falloff", "Smooth"),
            ("attribute", "falloff"),
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
        &[("translation", "0.00:0.50:0.00"), ("group", "ring"), ("radius", "0.30")],
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
            phase3_node("connectivity", &[("input", "sphere 1"), ("attribute", "piece")]),
            phase3_node(
                "normal",
                &[("input", "connectivity 1"), ("attribute", "N")],
            ),
            phase3_node(
                "group",
                &[
                    ("input", "normal 1"),
                    ("group_name", "up"),
                    ("mode", "Attribute"),
                    ("attribute", "N"),
                    ("comparison", "Above"),
                    ("threshold", "0.50"),
                    ("highlight", "false"),
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
            ("input", "group 1"),
            ("group_name", "wider"),
            ("mode", "Expand"),
            ("source_group", "up"),
            ("rings", "2"),
            ("highlight", "false"),
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
        .find(|p| p.name == "rings")
        .unwrap()
        .set_text("-1");
    let (shrunk, _) = eval_node(&shrunk_root, "group 2");
    assert!(shrunk.points().group_members("wider").len() < members.len());
}
