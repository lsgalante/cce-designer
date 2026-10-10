//! Phase 2: the solver contract.

use super::*;

#[test]
fn test_attribute_kind_defaults_to_live_and_is_declared_at_creation() {
    let mut d = quad_grid();
    d.points_mut().create("mass", AttribValue::Float(1.0));
    d.points_mut()
        .create_kind("scratch", AttribValue::Float(1.0), AttribKind::Derivative);

    // Live is the default because its failure mode is the visible one: a
    // value that should have been cleared and was not drifts where you can
    // watch it, while one that should have persisted and was cleared just
    // quietly reads zero.
    assert_eq!(d.points().kind("mass"), AttribKind::Live);
    assert_eq!(d.points().kind("scratch"), AttribKind::Derivative);
    assert_eq!(d.points().kind("never-declared"), AttribKind::Live);
    assert_eq!(d.points().names_of_kind(AttribKind::Derivative), vec!["scratch"]);

    d.clear_derivatives();
    // The column stays and the values reset: a reader between two steps
    // finds the attribute present and empty, not missing.
    assert!(d.points().has("scratch"));
    assert_eq!(d.points().value("scratch", 0), Some(AttribValue::Float(0.0)));
    assert_eq!(d.points().value("mass", 0), Some(AttribValue::Float(1.0)));

    // A name reused for a different purpose is a different attribute, so
    // re-creating it re-declares the kind.
    d.points_mut().create("scratch", AttribValue::Float(2.0));
    assert_eq!(d.points().kind("scratch"), AttribKind::Live);
}

#[test]
fn test_attribute_kinds_survive_the_structural_rewrites() {
    let mut d = quad_grid();
    d.points_mut()
        .create_kind("scratch", AttribValue::Float(1.0), AttribKind::Derivative);

    let mut kept = d.clone();
    kept.keep_points(&(0..9).map(|i| i < 6).collect::<Vec<_>>());
    assert_eq!(kept.points().kind("scratch"), AttribKind::Derivative, "through a gather");

    let mut merged = quad_grid();
    merged.merge(&d);
    assert_eq!(merged.points().kind("scratch"), AttribKind::Derivative, "through a merge");
}

#[test]
fn test_live_attributes_come_back_across_a_rebuild_by_identity() {
    let mut prev = quad_grid();
    prev.points_mut().create("mass", AttribValue::Float(0.0));
    for p in 0..9 {
        prev.points_mut()
            .set_value("mass", p, AttribValue::Float(p as f32))
            .unwrap();
    }
    prev.points_mut()
        .create_kind("scratch", AttribValue::Float(7.0), AttribKind::Derivative);

    // A step that rebuilt the geometry: it kept six of the nine points,
    // added one genuinely new one, and lost every attribute on the way —
    // which is what a remesh does, and what a kernel generator does today.
    let mut next = prev.clone();
    next.keep_points(&(0..9).map(|i| i < 6).collect::<Vec<_>>());
    next.points_mut().remove("mass");
    next.points_mut().remove("scratch");
    next.add_point(Vec3::new(9.0, 9.0, 0.0));

    next.restore_live_from(&prev);

    // The simulation's memory is not gone: it is in the previous state,
    // attached to identities.
    assert!(next.points().has("mass"));
    for p in 0..6 {
        assert_eq!(next.points().value("mass", p), Some(AttribValue::Float(p as f32)));
    }
    // A point that did not exist last step gets the type's zero, which is
    // the only honest answer for a place with no history.
    assert_eq!(next.points().value("mass", 6), Some(AttribValue::Float(0.0)));
    // Derivative attributes are NOT restored — carrying one across is
    // exactly the silent accumulation the kind exists to prevent.
    assert!(!next.points().has("scratch"));
}

#[test]
fn test_restoration_bridges_a_rebuild_and_does_not_undo_a_delete() {
    let mut prev = quad_grid();
    prev.points_mut().create("mass", AttribValue::Float(3.0));

    // The chain handed back the same identities in the same order, so it
    // kept the geometry it was given: an attribute that is gone was taken
    // out on purpose, and putting it back would override the author.
    let mut same = prev.clone();
    same.points_mut().remove("mass");
    same.restore_live_from(&prev);
    assert!(!same.points().has("mass"), "a deliberate delete must stick");

    // An attribute the new state DOES carry is left alone: the chain
    // computed it this step, which is the whole point of running it.
    let mut recomputed = prev.clone();
    recomputed
        .points_mut()
        .set_value("mass", 0, AttribValue::Float(99.0))
        .unwrap();
    recomputed.restore_live_from(&prev);
    assert_eq!(recomputed.points().value("mass", 0), Some(AttribValue::Float(99.0)));
}
