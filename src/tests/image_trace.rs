//! The image, path traced.

use super::*;

/// The traced scene is handed over again when the image changes: the
/// image has a version as the geometry has, which a recomposition moves
/// and a rebuild with no image in it does not — nor, since the page
/// cache (2026-10-06), one that changes nothing the image is made of.
#[test]
fn a_recomposed_image_is_a_new_traced_scene() {
    let mut state = State::new(false);
    let before = state.page_version;
    state.rebuild_scene_geometry();
    assert_eq!(state.page_version, before, "a scene with no image moved the image's version");

    let mut state = state_showing_image(64, 32, 100);
    let shown = state.page_version;
    assert!(shown > before, "showing an image did not move its version");
    state.rebuild_scene_geometry();
    assert_eq!(state.page_version, shown, "a rebuild that changed nothing in the image recomposed it");
    let page = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
    state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == "color").unwrap().set_text("0.50:0.20:0.10");
    state.rebuild_scene_geometry();
    assert!(state.page_version > shown, "a recomposed image is the scene the tracer has");

    // Hidden, the tracer has to be told it went.
    let slot = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
    state.current_dir_mut().set_child_geometry_visible(slot, false);
    let hidden = state.page_version;
    state.rebuild_scene_geometry();
    assert!(state.page_version > hidden, "the tracer keeps an image that is gone");
    let gone = state.page_version;
    state.rebuild_scene_geometry();
    assert_eq!(state.page_version, gone);
}

/// A thumbnail of an image alone is taken square on, the image fitted
/// edge to edge; with geometry beside it the view is the diagonal one and
/// holds both.
#[test]
fn a_thumbnail_of_an_image_is_taken_square_on() {
    use crate::thumbnail::view_of;
    let corners = [[1.0, 3.0, 2.0], [5.0, 3.0, 2.0], [5.0, 1.0, 2.0], [1.0, 1.0, 2.0]];
    let seen = |eye: Vec3, center: Vec3, near: f32, far: f32, p: [f32; 3]| {
        let m = Mat4::perspective_rh(0.9, 1.0, near, far) * Mat4::look_at_rh(eye, center, Vec3::Y);
        m.project_point3(Vec3::from_array(p))
    };

    let (eye, center, near, far) = view_of(&[], Some(corners));
    assert!((center - Vec3::new(3.0, 2.0, 2.0)).length() < 1e-4, "not looking at the image's middle: {center:?}");
    assert!((eye.x - 3.0).abs() < 1e-4 && (eye.y - 2.0).abs() < 1e-4 && eye.z > 2.0, "not square on, from the side it faces: {eye:?}");
    let [tl, tr, br, bl] = corners.map(|c| seen(eye, center, near, far, c));
    assert!((tl.x + tr.x).abs() < 1e-4 && (tl.y + bl.y).abs() < 1e-4 && (tr.x - br.x).abs() < 1e-4);
    assert!(tr.x > 0.9 && tr.x <= 1.0, "the long side spans {} of the view", tr.x);
    assert!(tl.z > 0.0 && tl.z < 1.0, "the image is outside the near and far planes");

    let tri = cce_ui::vk::RtTriangle { p0: [0.0, 0.0, 0.0], p1: [1.0, 0.0, 0.0], p2: [0.0, 1.0, 0.0], material: 0 };
    let (eye, center, near, far) = view_of(&[tri], Some(corners));
    assert!(eye.x > center.x && eye.y > center.y && eye.z > center.z, "geometry is seen from the diagonal");
    for p in corners.into_iter().chain([tri.p0, tri.p1, tri.p2]) {
        let v = seen(eye, center, near, far, p);
        assert!(v.x.abs() < 1.0 && v.y.abs() < 1.0 && v.z > 0.0 && v.z < 1.0, "{p:?} is out of the view: {v:?}");
    }

    // As before, for a scene with no image in it.
    let (eye, center, _, _) = view_of(&[tri], None);
    assert!((center - Vec3::new(0.5, 0.5, 0.0)).length() < 1e-4 && eye.z > 0.0);
    let (_, center, _, _) = view_of(&[], None);
    assert_eq!(center, Vec3::ZERO);
}
