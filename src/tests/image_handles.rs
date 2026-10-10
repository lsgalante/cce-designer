//! Handles on an image.

use super::*;

/// A page's frame is the page without its pixels: the same size, raster,
/// unit and place, from any node of the chain, and nothing where the
/// chain composes nothing.
#[test]
fn a_page_frame_is_the_page_without_its_pixels() {
    use crate::page::{resolve_frame, resolve_page};
    let root = image_root(vec![
        image_node("page1", "page", &[("units", "Millimetres"), ("width", "120"), ("height", "80"), ("resolution", "127"), ("position", "1.00:2.00:3.00")]),
        image_node("shape1", "page_shape", &[("input", "page1")]),
        image_node("text1", "page_text", &[("input", "shape1")]),
        image_node("lost1", "page_text", &[("input", "nothing")]),
        image_node("sphere1", "sphere", &[]),
    ]);
    let page = resolve_page(&root, &root.children[2], &mut Vec::new()).expect("no page");
    for slot in 0..3 {
        let frame = resolve_frame(&root, &root.children[slot]).expect("no frame");
        assert_eq!((frame.width, frame.height), (page.width, page.height));
        assert_eq!((frame.size, frame.dpi, frame.unit, frame.origin), (page.size, page.dpi, page.unit, page.origin));
    }
    assert!(resolve_frame(&root, &root.children[3]).is_none(), "a frame for a chain with no page under it");
    assert!(resolve_frame(&root, &root.children[4]).is_none(), "a frame for a sphere");

    // The page's corner and its middle, in a world of millimetres, and
    // back again.
    let frame = resolve_frame(&root, &root.children[0]).unwrap();
    let corner = frame.to_world([0.0, 0.0], 1.0);
    assert!((corner - Vec3::new(1.0 - 60.0, 2.0 + 40.0, 3.0)).length() < 1e-3, "{corner:?}");
    let middle = frame.to_world([60.0, 40.0], 1.0);
    assert!((middle - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-3, "{middle:?}");
    let back = frame.from_world(frame.to_world([17.0, 63.0], 10.0), 10.0);
    assert!((back[0] - 17.0).abs() < 1e-3 && (back[1] - 63.0).abs() < 1e-3, "{back:?}");
    // Off the plane is the place straight behind.
    let behind = frame.from_world(middle + Vec3::Z * 9.0, 1.0);
    assert!((behind[0] - 60.0).abs() < 1e-3 && (behind[1] - 40.0).abs() < 1e-3);
}

/// Give the viewer state the camera the stage pass would have cached.
fn cache_scene_camera(state: &mut State) {
    let (pos, rot, pivot) = state.active_camera_pose();
    let (w, h) = (state.last_viewport_width as f32, state.last_viewport_height as f32);
    let (proj, view, model) = state.viewport().get_matrices(w / h, Some(pos), Some(rot), Some(pivot));
    state.last_scene_mvp = Some(proj * view * model);
    state.last_scene_view_rect = (0.0, 0.0, w, h);
}

/// Press on handle `i`, carry it to a screen point, let go.
fn drag_handle_to(state: &mut State, i: usize, to: (f32, f32)) {
    let handles = state.viewer_tool_handles();
    let (_, x, y, _) = handles[i];
    state.cursor_x = x;
    state.cursor_y = y;
    assert!(state.viewer_tool_press(), "the press on handle {i} grabbed nothing");
    assert_eq!(state.viewer_tool.as_ref().unwrap().selected, Some(i), "the press took another handle");
    // In two motions, as a pointer arrives.
    state.cursor_x = (x + to.0) * 0.5;
    state.cursor_y = (y + to.1) * 0.5;
    assert!(state.viewer_tool_drag_motion());
    state.cursor_x = to.0;
    state.cursor_y = to.1;
    assert!(state.viewer_tool_drag_motion());
    assert!(state.viewer_tool_release());
}

/// A row of the node the viewer state is editing, as a number.
fn edited_row(state: &State, row: &str) -> f32 {
    let id = state.viewer_tool.as_ref().expect("no viewer state").node_id.clone();
    let node = crate::viewer_state::find_node_by_id(&state.fs_root, &id).expect("the edited node is gone");
    crate::geometry::node_param_f32(node, row, f32::NAN)
}

/// A shape on an image is placed by its handles: the middle moves it and
/// carries the others, the corner sizes it about its middle, the edge
/// turns it and carries the corner round. At one image pixel to the
/// screen's, a drag of so many pixels is a change of as many.
#[test]
fn an_image_shape_is_moved_sized_and_turned_by_its_handles() {
    let mut state = state_showing_image(400, 200, 100);
    assert!(state.run_command("add_image_rectangle"));
    assert_eq!(state.viewer_tool.as_ref().map(|t| t.source.name()), Some("Image Shape"), "adding a shape did not enter its viewer state");
    state.view_image_pixels();
    cache_scene_camera(&mut state);
    let rows = |s: &State| ["x", "y", "width", "height", "rotation"].map(|r| edited_row(s, r));
    assert_eq!(rows(&state), [200.0, 100.0, 67.0, 67.0, 0.0]);

    let handles = state.viewer_tool_handles();
    assert_eq!(handles.len(), 3, "a shape has a middle, a corner and an edge");
    // The image's middle is the pane's, and the corner is down and right
    // of it: the page's y runs down the screen.
    assert!((handles[0].1 - 600.0).abs() < 0.5 && (handles[0].2 - 400.0).abs() < 0.5, "{:?}", handles[0]);
    assert!((handles[1].1 - 633.5).abs() < 0.5 && (handles[1].2 - 433.5).abs() < 0.5, "{:?}", handles[1]);
    assert!((handles[2].1 - 633.5).abs() < 0.5 && (handles[2].2 - 400.0).abs() < 0.5, "{:?}", handles[2]);
    assert_eq!(state.viewer_tool_outline().len(), 4, "no outline of the box");

    drag_handle_to(&mut state, 0, (630.0, 410.0));
    assert_eq!(rows(&state), [230.0, 110.0, 67.0, 67.0, 0.0], "the middle did not move the shape whole");

    let corner = state.viewer_tool_handles()[1];
    drag_handle_to(&mut state, 1, (corner.1 + 20.0, corner.2 + 10.0));
    let sized = rows(&state);
    assert_eq!(sized[..2], [230.0, 110.0], "sizing moved the shape");
    assert!((sized[2] - 107.0).abs() <= 1.0 && (sized[3] - 87.0).abs() <= 1.0, "the corner did not size the box about its middle: {sized:?}");
    assert_eq!(sized[4], 0.0, "sizing turned the shape");

    // The edge, carried from the right of the middle to under it: a
    // quarter turn clockwise, and the box is the box it was.
    drag_handle_to(&mut state, 2, (630.0, 410.0 + 40.0));
    let turned = rows(&state);
    assert!((turned[4] - 90.0).abs() < 0.5, "the edge did not turn the shape: {turned:?}");
    assert!((turned[2] - sized[2]).abs() <= 1.0 && (turned[3] - sized[3]).abs() <= 1.0, "turning resized the box: {turned:?}");
    assert_eq!(turned[..2], [230.0, 110.0]);
    // Turned, the corner handle is down and LEFT of the middle.
    let corner = state.viewer_tool_handles()[1];
    assert!(corner.1 < 630.0 && corner.2 > 410.0, "the corner was not carried round: {corner:?}");

    // Each drag is one step to undo.
    assert!(state.viewer_tool_undo());
    assert!((edited_row(&state, "rotation")).abs() < 0.5, "undo did not take the turn back");
    assert!(state.viewer_tool_redo());
    assert!((edited_row(&state, "rotation") - 90.0).abs() < 0.5);

    // From an orbit the handles are still under the pointer: the middle,
    // dropped where a place on the image shows, is at that place.
    state.orbit_camera_by(140.0, -60.0);
    let py = state.viewport().pending_yaw;
    let pp = state.viewport().pending_pitch;
    state.update_active_camera_rotation(py, pp);
    cache_scene_camera(&mut state);
    let ctx_frame = {
        let id = state.viewer_tool.as_ref().unwrap().node_id.clone();
        let node = crate::viewer_state::find_node_by_id(&state.fs_root, &id).unwrap();
        crate::page::resolve_frame(&state.fs_root, node).unwrap()
    };
    let target = ctx_frame.to_world([120.0, 60.0], state.world_unit_mm());
    let (sx, sy, _) = crate::viewer_state::project_point(&state.last_scene_mvp.unwrap(), state.last_scene_view_rect, target).expect("the place is behind the camera");
    let head_on = state.viewer_tool_handles()[0];
    assert!((head_on.1 - 630.0).abs() > 2.0 || (head_on.2 - 410.0).abs() > 2.0, "the orbit did not move the view");
    drag_handle_to(&mut state, 0, (sx, sy));
    let moved = rows(&state);
    assert!((moved[0] - 120.0).abs() <= 1.0 && (moved[1] - 60.0).abs() <= 1.0, "from an orbit the handle left the image's plane: {moved:?}");
    assert!((moved[4] - 90.0).abs() < 0.5 && (moved[2] - sized[2]).abs() <= 1.0, "moving from an orbit changed the shape: {moved:?}");
}

/// A line has a middle and an end, and the end sets how long it is and
/// which way it runs.
#[test]
fn an_image_line_is_drawn_by_its_end() {
    let mut state = state_showing_image(400, 200, 100);
    assert!(state.run_command("add_image_line"));
    state.view_image_pixels();
    cache_scene_camera(&mut state);
    assert_eq!(state.viewer_tool_handles().len(), 2, "a line has a middle and an end");
    assert_eq!(state.viewer_tool_outline().len(), 2);
    let height = edited_row(&state, "height");

    // Up and to the right of the middle by 30 and 40: fifty long each
    // way, running up the page.
    drag_handle_to(&mut state, 1, (630.0, 360.0));
    assert_eq!(edited_row(&state, "width"), 100.0);
    assert!((edited_row(&state, "rotation") - (-53.1)).abs() < 0.2, "{}", edited_row(&state, "rotation"));
    assert_eq!((edited_row(&state, "x"), edited_row(&state, "y")), (200.0, 100.0));
    assert_eq!(edited_row(&state, "height"), height, "a line's handles wrote a row it does not show");
}

/// Text is moved by its anchor and sized by the handle under it, in the
/// image's unit — here inches on a sheet shown at half size.
#[test]
fn image_text_is_moved_and_sized_by_its_handles() {
    let mut state = state_showing_image(400, 200, 100);
    let page = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
    state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == "units").unwrap().set_text("Inches");
    for (row, v) in [("width", "4"), ("height", "2")] {
        state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == row).unwrap().set_text(v);
    }
    state.rebuild_scene_geometry();
    assert!(state.run_command("add_image_text"));
    assert_eq!(state.viewer_tool.as_ref().map(|t| t.source.name()), Some("Image Text"));
    state.view_image_pixels();
    cache_scene_camera(&mut state);
    assert_eq!((edited_row(&state, "x"), edited_row(&state, "y"), edited_row(&state, "size")), (2.0, 1.0, 0.1));

    let handles = state.viewer_tool_handles();
    assert_eq!(handles.len(), 2);
    assert!((handles[1].2 - handles[0].2 - 10.0).abs() < 0.5, "the size handle is not one Size under the anchor");

    // A hundred pixels to the inch.
    drag_handle_to(&mut state, 0, (650.0, 375.0));
    assert_eq!((edited_row(&state, "x"), edited_row(&state, "y"), edited_row(&state, "size")), (2.5, 0.75, 0.1));
    drag_handle_to(&mut state, 1, (650.0, 375.0 + 25.0));
    assert_eq!((edited_row(&state, "x"), edited_row(&state, "y"), edited_row(&state, "size")), (2.5, 0.75, 0.25));

    // A node that draws on no image has no handles to grab.
    let id = state.viewer_tool.as_ref().unwrap().node_id.clone();
    crate::viewer_state::find_node_by_id_mut(&mut state.fs_root, &id)
        .unwrap()
        .params
        .iter_mut()
        .find(|p| p.name == "input")
        .unwrap()
        .set_text("");
    assert!(state.viewer_tool_handles().is_empty());
}

/// Recomposing an image of the same size keeps its GPU image, which a
/// drag does on every motion; another size takes another.
#[test]
fn a_recomposed_image_keeps_its_gpu_image() {
    let mut state = state_showing_image(64, 32, 100);
    let first = state.page_image.expect("nothing uploaded");
    state.rebuild_scene_geometry();
    assert_eq!(state.page_image, Some(first), "the same picture took a new image");
    // New contents at the same size: recomposed into the same image.
    let page = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
    state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == "color").unwrap().set_text("0.50:0.20:0.10");
    let version = state.page_version;
    state.rebuild_scene_geometry();
    assert!(state.page_version > version, "the colour edit was not recomposed");
    assert_eq!(state.page_image, Some(first), "the same size took a new image");
    state.current_dir_mut().children[page].params.iter_mut().find(|p| p.name == "width").unwrap().set_text("80");
    state.rebuild_scene_geometry();
    assert!(state.page_image.is_some_and(|id| id != first), "a picture of another size kept the old image");
}
