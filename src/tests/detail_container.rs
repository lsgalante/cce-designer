//! Phase 0: the points/vertices/primitives/detail container.

use super::*;

//
// These cover what the triangle soup could not do at all: a point shared
// by several primitives, an edge, an identity that survives an edit, and
// attributes that follow their elements through one. See src/detail.rs.

/// A 3x3 grid of points wired into four quads — the smallest mesh with an
/// interior point, which is the only kind of point neighbour queries are
/// interesting on.
///
/// ```text
///   6 — 7 — 8
///   |   |   |
///   3 — 4 — 5
///   |   |   |
///   0 — 1 — 2
/// ```
pub(super) fn quad_grid() -> Detail {
    let mut d = Detail::new();
    for y in 0..3 {
        for x in 0..3 {
            d.add_point(Vec3::new(x as f32, y as f32, 0.0));
        }
    }
    for quad in [[0, 1, 4, 3], [1, 2, 5, 4], [3, 4, 7, 6], [4, 5, 8, 7]] {
        d.add_prim(&quad);
    }
    d
}

#[test]
fn test_detail_counts_points_verts_and_prims_separately() {
    let d = quad_grid();
    assert_eq!(d.num_points(), 9, "nine points, each shared by up to four quads");
    assert_eq!(d.num_verts(), 16, "four quads of four corners");
    assert_eq!(d.num_prims(), 4);
    // The soup would have needed 24 vertices for the same surface and
    // would have had no way to say that the center is one place.
    assert_eq!(d.prim_points(0), &[0, 1, 4, 3]);
    assert_eq!(d.prim_points(3), &[4, 5, 8, 7]);
    assert!(d.prim_points(4).is_empty(), "no fifth primitive");
}

#[test]
fn test_detail_point_ids_are_unique_and_survive_a_delete() {
    let mut d = quad_grid();
    let before: Vec<_> = (0..d.num_points()).map(|p| d.id(p).unwrap()).collect();
    let mut sorted = before.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), before.len(), "every point has its own identity");

    // Drop the top row. The survivors keep the identity they were born
    // with even though their indices moved — this is the property a
    // positional weld can never provide, and the one a solver needs.
    let keep: Vec<bool> = (0..9).map(|i| i < 6).collect();
    d.keep_points(&keep);
    assert_eq!(d.num_points(), 6);
    for p in 0..d.num_points() {
        assert_eq!(d.id(p), Some(before[p]));
    }
    assert_eq!(d.index_of_id(before[5]), Some(5));
    assert_eq!(d.index_of_id(before[8]), None, "a deleted point resolves to nothing");
}

#[test]
fn test_detail_topology_neighbours_exclude_diagonals() {
    let d = quad_grid();
    // The center point touches all four quads but only four points: a
    // quad's diagonal is not an edge.
    assert_eq!(d.point_neighbours(4), &[1, 3, 5, 7]);
    assert_eq!(d.topology().valence(4), 4);
    assert_eq!(d.point_prims(4).len(), 4);
    // A corner touches one quad and two points.
    assert_eq!(d.point_neighbours(0), &[1, 3]);
    assert_eq!(d.point_prims(0), &[0]);
}

#[test]
fn test_detail_topology_counts_a_shared_edge_once() {
    let d = quad_grid();
    // 12 rather than 16: the four interior edges are each shared by two
    // quads, and an edge list that double-counted them would double every
    // force a solver puts along one.
    assert_eq!(d.edges().len(), 12);
    let mut seen = d.edges().to_vec();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 12);
    assert!(d.edges().iter().all(|e| e[0] < e[1]), "edges are stored low-to-high");
}

#[test]
fn test_detail_topology_rebuilds_after_a_structural_edit() {
    let mut d = quad_grid();
    assert_eq!(d.topology().valence(8), 2, "the far corner starts with two edges");

    // Ask for topology, then change the structure. The cached answer must
    // not survive — a stale neighbour list is a silently wrong simulation,
    // not a crash.
    let p = d.add_point(Vec3::new(3.0, 3.0, 0.0));
    d.add_prim(&[8, p, 5]);
    assert_eq!(d.num_prims(), 5);
    // One more edge, not two: the new triangle's third side (5–8) is the
    // grid edge that was already there, and must not be counted twice.
    assert_eq!(d.topology().valence(8), 3);
    assert_eq!(d.edges().len(), 14);
    assert_eq!(d.point_neighbours(p as usize), &[5, 8]);
}

#[test]
fn test_detail_line_primitive_does_not_close_into_a_loop() {
    let mut d = Detail::new();
    let a = d.add_point(Vec3::ZERO);
    let b = d.add_point(Vec3::X);
    d.add_prim(&[a, b]);
    // A two-point primitive is an open segment. Closing the winding would
    // invent a second edge between the same pair and give both ends a
    // neighbour they do not have.
    assert_eq!(d.edges(), &[[0, 1]]);
    assert_eq!(d.point_neighbours(0), &[1]);
}

#[test]
fn test_detail_welds_a_triangle_soup_into_shared_points() {
    // Two triangles meeting along one edge — six soup corners, four
    // points.
    let positions = [
        [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0],
    ];
    let colors = [[1.0, 0.0, 0.0]; 6];
    let d = Detail::from_triangle_soup(&positions, &colors);

    assert_eq!(d.num_points(), 4, "the shared edge's two corners weld");
    assert_eq!(d.num_prims(), 2);
    assert_eq!(d.num_verts(), 6, "vertices still name a corner each");
    assert_eq!(d.edges().len(), 5, "three edges each, one of them shared");
    assert_eq!(d.color(0), [1.0, 0.0, 0.0], "color carries onto Cd");
}

#[test]
fn test_detail_triangulate_fans_polygons_for_the_renderer() {
    let mut d = Detail::new();
    for p in [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y] {
        d.add_point(p);
    }
    d.add_prim(&[0, 1, 2, 3]);

    let tris = d.triangulate(|pos, col| (pos, col));
    assert_eq!(tris.len(), 6, "one quad fans into two triangles");
    assert_eq!(tris[0].0, [0.0, 0.0, 0.0]);
    assert_eq!(tris[1].1, crate::detail::DEFAULT_COLOR, "no Cd means the default");

    // Soup in, soup out: the round trip preserves the surface even though
    // the middle of it is no longer a soup.
    let soup: Vec<[f32; 3]> = vec![
        [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0],
    ];
    let welded = Detail::from_triangle_soup(&soup, &[[0.5; 3]; 6]);
    let back = welded.triangulate(|pos, _| pos);
    assert_eq!(back, soup);
}

#[test]
fn test_detail_attributes_are_columnar_and_typed() {
    let mut d = quad_grid();
    d.points_mut().create("mass", AttribValue::Float(1.0));
    assert_eq!(d.points().get("mass").map(|a| a.len()), Some(9), "one entry per point");
    assert_eq!(d.points().get("mass").map(|a| a.ty()), Some(AttribType::Float));

    d.points_mut().set_value("mass", 4, AttribValue::Float(7.5)).unwrap();
    assert_eq!(d.points().value("mass", 4), Some(AttribValue::Float(7.5)));
    assert_eq!(d.points().value("mass", 0), Some(AttribValue::Float(1.0)));

    // A type mismatch is refused rather than silently coerced: half a
    // converted attribute is not a state this should be able to reach.
    let err = d
        .points_mut()
        .set_value("mass", 0, AttribValue::Float3([1.0; 3]))
        .unwrap_err();
    assert!(err.contains("float"), "{err}");

    // Integers exist for counters and ages, which must stay whole.
    d.points_mut().create("age", AttribValue::Int(0));
    d.points_mut().set_value("age", 2, AttribValue::Int(3)).unwrap();
    assert_eq!(d.points().value("age", 2), Some(AttribValue::Int(3)));
    assert_eq!(d.points().names(), vec!["age", "mass"], "sorted, so columns hold still");

    // The float view is what a GPU buffer binds in Phase 1.
    let flat = d.points().get("mass").unwrap().as_f32_slice().unwrap();
    assert_eq!(flat.len(), 9);
    assert!(d.points().get("age").unwrap().as_f32_slice().is_none());
}

#[test]
fn test_detail_attribute_values_follow_their_points_through_a_delete() {
    let mut d = quad_grid();
    d.points_mut().create("mass", AttribValue::Float(0.0));
    for p in 0..9 {
        d.points_mut()
            .set_value("mass", p, AttribValue::Float(p as f32))
            .unwrap();
    }

    // Keep the middle row only. Values must move with their points, not
    // stay at their old indices.
    let keep: Vec<bool> = (0..9).map(|i| (3..6).contains(&i)).collect();
    d.keep_points(&keep);
    assert_eq!(d.num_points(), 3);
    let masses: Vec<f32> = (0..3)
        .map(|p| d.points().value("mass", p).unwrap().as_f32())
        .collect();
    assert_eq!(masses, vec![3.0, 4.0, 5.0]);
}

#[test]
fn test_detail_dropping_a_point_drops_the_primitives_using_it() {
    let mut d = quad_grid();
    // The center point belongs to every quad, so removing it removes all
    // four: a polygon missing a corner is not geometry.
    let keep: Vec<bool> = (0..9).map(|i| i != 4).collect();
    d.keep_points(&keep);
    assert_eq!(d.num_points(), 8);
    assert_eq!(d.num_prims(), 0);
    assert_eq!(d.num_verts(), 0);
    assert_eq!(d.edges().len(), 0);
}

#[test]
fn test_detail_gather_rewires_surviving_primitives() {
    let mut d = quad_grid();
    // Keep the bottom-left quad's four points. Its primitive survives and
    // must now name the points by their new indices.
    let keep: Vec<bool> = (0..9).map(|i| [0, 1, 3, 4].contains(&i)).collect();
    d.keep_points(&keep);
    assert_eq!(d.num_points(), 4);
    assert_eq!(d.num_prims(), 1);
    assert_eq!(d.prim_points(0), &[0, 1, 3, 2], "0,1,4,3 renumbered");
    assert_eq!(d.edges().len(), 4);
}

#[test]
fn test_detail_groups_are_named_sets_that_survive_an_edit() {
    let mut d = quad_grid();
    d.points_mut().create_group("pinned");
    for p in [0, 2, 6, 8] {
        d.points_mut().add_to_group("pinned", p);
    }
    assert_eq!(d.points().group_members("pinned"), vec![0, 2, 6, 8]);
    assert_eq!(d.points().group_len("pinned"), 4);
    assert!(d.points().in_group("pinned", 8));
    assert!(!d.points().in_group("pinned", 4));
    // Asking about a group nobody made is not an error — a node's Group
    // parameter is routinely blank.
    assert_eq!(d.points().group_members("nope"), Vec::<u32>::new());

    let keep: Vec<bool> = (0..9).map(|i| i < 6).collect();
    d.keep_points(&keep);
    assert_eq!(d.points().group_members("pinned"), vec![0, 2], "membership follows");
    assert_eq!(d.points().group_names(), vec!["pinned"]);
}

#[test]
fn test_detail_merge_reallocates_ids_and_rewires_primitives() {
    let mut a = quad_grid();
    let b = quad_grid();
    let a_ids: Vec<_> = (0..a.num_points()).map(|p| a.id(p).unwrap()).collect();

    a.merge(&b);
    assert_eq!(a.num_points(), 18);
    assert_eq!(a.num_prims(), 8);
    assert_eq!(a.num_verts(), 32);

    // Both sides numbered their points from zero. If the merge kept those
    // numbers, two different points would answer to one identity and a
    // solver would write one over the other.
    let all: Vec<_> = (0..a.num_points()).map(|p| a.id(p).unwrap()).collect();
    let mut uniq = all.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), 18, "no identity collides across the merge");
    assert_eq!(&all[..9], &a_ids[..], "the left side keeps the ids it had");

    // The appended primitives point at the appended points.
    assert_eq!(a.prim_points(4), &[9, 10, 13, 12]);
    assert_eq!(a.edges().len(), 24, "two grids, no edges invented between them");
}

#[test]
fn test_detail_merge_unions_attributes_and_zero_fills_the_gap() {
    let mut a = quad_grid();
    a.points_mut().create("mass", AttribValue::Float(2.0));
    let mut b = quad_grid();
    b.points_mut().create("age", AttribValue::Int(5));

    a.merge(&b);
    // Neither column is dropped; each side gets zeros where it had no
    // opinion. Silently losing a column here would strand a solver
    // attribute the moment two streams met.
    assert_eq!(a.points().names(), vec!["age", "mass"]);
    assert_eq!(a.points().value("mass", 0), Some(AttribValue::Float(2.0)));
    assert_eq!(a.points().value("mass", 9), Some(AttribValue::Float(0.0)));
    assert_eq!(a.points().value("age", 0), Some(AttribValue::Int(0)));
    assert_eq!(a.points().value("age", 9), Some(AttribValue::Int(5)));
    assert_eq!(a.points().get("mass").map(|x| x.len()), Some(18));
}

#[test]
fn test_detail_holds_per_class_attributes_including_one_detail_row() {
    let mut d = quad_grid();
    d.verts_mut().create("uv", AttribValue::Float2([0.0; 2]));
    d.prims_mut().create("area", AttribValue::Float(1.0));
    d.detail_mut().create("edges_max", AttribValue::Float(0.0));

    assert_eq!(d.store(Class::Vertex).len(), 16);
    assert_eq!(d.store(Class::Prim).len(), 4);
    assert_eq!(d.store(Class::Detail).len(), 1, "detail is the single-row class");

    // Analysis writes a range here rather than needing a dictionary type.
    d.detail_mut()
        .set_value("edges_max", 0, AttribValue::Float(1.0))
        .unwrap();
    assert_eq!(d.detail().value("edges_max", 0), Some(AttribValue::Float(1.0)));
    assert_eq!(Class::Detail.name(), "detail");
}

#[test]
fn test_detail_attribute_gather_is_the_one_reordering_primitive() {
    let data = AttribData::Float(vec![10.0, 20.0, 30.0]);
    // Delete, reorder and duplicate are all the same operation.
    assert_eq!(data.gather(&[2, 0]), AttribData::Float(vec![30.0, 10.0]));
    assert_eq!(data.gather(&[1, 1]), AttribData::Float(vec![20.0, 20.0]));
    // An out-of-range index yields a zero rather than panicking: an index
    // map built with an arithmetic slip should not be able to take the app
    // down.
    assert_eq!(data.gather(&[9]), AttribData::Float(vec![0.0]));
}

#[test]
fn test_detail_color_and_bounds_read_back() {
    let mut d = quad_grid();
    assert_eq!(d.color(0), crate::detail::DEFAULT_COLOR);
    d.set_color(0, [0.25, 0.5, 0.75]);
    assert_eq!(d.color(0), [0.25, 0.5, 0.75]);
    assert_eq!(d.color(1), crate::detail::DEFAULT_COLOR, "Cd defaults for everyone else");

    let (lo, hi) = d.bounds().unwrap();
    assert_eq!(lo, Vec3::ZERO);
    assert_eq!(hi, Vec3::new(2.0, 2.0, 0.0));
    assert!(Detail::new().bounds().is_none());
}

#[test]
fn test_detail_clone_drops_the_derived_topology_but_not_the_geometry() {
    let d = quad_grid();
    assert_eq!(d.topology().valence(4), 4);

    // The clone exists to be modified, so it starts with no cache; what it
    // must not lose is anything the cache was derived from.
    let mut copy = d.clone();
    assert_eq!(copy.num_points(), 9);
    assert_eq!(copy.topology().valence(4), 4);
    copy.keep_points(&vec![true; 9]);
    assert_eq!(copy.num_prims(), 4);
    assert_eq!(d.num_prims(), 4, "the original is untouched");
}
