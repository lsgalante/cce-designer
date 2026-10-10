//! The 2D context in the viewport.

use super::*;

/// A page node for the image tests: text params, as a hand-built node
/// has them.
pub(super) fn image_node(id: &str, ty: &str, params: &[(&str, &str)]) -> FsNode {
    FsNode {
        id: id.to_string(),
        name: id.to_string(),
        node_type: ty.to_string(),
        children: vec![],
        params: params
            .iter()
            .map(|(n, v)| crate::app::ParamDef::new(n.to_string(), "text".to_string(), v.to_string()))
            .collect(),
        geometry_visible: true,
        bypassed: false,
        position: (0.0, 0.0),
        inputs: 1,
        outputs: 1,
    }
}

pub(super) fn image_root(children: Vec<FsNode>) -> FsNode {
    let mut root = image_node("root", "node", &[]);
    root.children = children;
    root
}

/// The generator's size is in the unit its Units row names: pixels are
/// pixels exactly, and a metric sheet is its millimetres. (What a
/// preset writes is `picking_a_page_preset_writes_its_size`.)
#[test]
fn an_image_is_sized_in_pixels_or_in_real_units() {
    use crate::page::{resolve_page, PageUnit};
    let make = |params: &[(&str, &str)]| {
        let root = image_root(vec![image_node("page1", "page", params)]);
        resolve_page(&root, &root.children[0], &mut Vec::new()).expect("no page")
    };

    let px = make(&[("units", "Pixels"), ("width", "640"), ("height", "360"), ("resolution", "96")]);
    assert_eq!((px.width, px.height), (640, 360), "a pixel size is that many pixels");
    assert_eq!(px.unit, PageUnit::Pixels);
    assert!((px.size[0] - 640.0 / 96.0).abs() < 1e-4, "its physical size is its pixels over its resolution");

    let mm = make(&[("units", "Millimetres"), ("width", "210"), ("height", "297"), ("resolution", "100")]);
    assert!((mm.size[0] - 210.0 / 25.4).abs() < 1e-4 && (mm.size[1] - 297.0 / 25.4).abs() < 1e-4);
    assert_eq!((mm.width, mm.height), (827, 1169), "A4 in millimetres at 100 DPI");

    let cm = make(&[("units", "Centimetres"), ("width", "2.54"), ("height", "5.08"), ("resolution", "50")]);
    assert_eq!((cm.width, cm.height), (50, 100));

    // A page from before the Units row is in inches, as it was.
    let old = make(&[("width", "2"), ("height", "1"), ("resolution", "50")]);
    assert_eq!((old.width, old.height, old.unit), (100, 50, PageUnit::Inches));

    // Opacity is the sheet's alpha, and Position where it stands.
    let clear = make(&[("width", "1"), ("height", "1"), ("resolution", "10"), ("opacity", "0.25"), ("position", "1.00:2.00:3.00")]);
    assert!((clear.pixels[0][3] - 0.25).abs() < 1e-6);
    assert_eq!(clear.origin, [1.0, 2.0, 3.0]);
}

/// A page's size is its Width and Height, always: Preset has no Custom
/// and there is no Orientation row. Picking a preset WRITES its size
/// there, in the page's Units (a sheet portrait, a raster size as it
/// lies), Units converts them, and a save from before carries over once,
/// a landscape sheet as it was drawn.
#[test]
fn picking_a_page_preset_writes_its_size() {
    use crate::page::{follow_page_rows, migrate_preset_rows, resolve_page};
    let size = |node: &FsNode| {
        let row = |n: &str| node.params.iter().find(|p| p.name == n).unwrap().text().to_string();
        (row("width"), row("height"))
    };
    let pick = |node: &mut FsNode, row: &str, value: &str| -> Vec<String> {
        let p = node.params.iter_mut().find(|p| p.name == row).unwrap();
        let was = p.clone();
        p.set_text(value);
        follow_page_rows(node, &was).into_iter().map(|p| p.name).collect()
    };
    let templates_root = crate::app::load_fs_tree();
    let templates = crate::app::flatten_node_templates(&templates_root);
    let template = templates_root.children.iter().find(|t| t.node_type == "page").unwrap().clone();
    let preset = template.params.iter().find(|p| p.name == "preset").unwrap();
    assert!(!preset.choice_options().iter().any(|o| o == "Custom"), "{:?}", preset.choice_options());
    assert!(template.params.iter().all(|p| p.name != "orientation"));
    for row in ["width", "height"] {
        assert!(template.params.iter().find(|p| p.name == row).unwrap().show_when.is_empty(), "{row} is always shown");
    }

    let mut page = template.clone();
    assert_eq!(pick(&mut page, "preset", "A4"), ["width", "height"], "what a pick overwrites is handed back, for undo");
    assert_eq!(size(&page), ("8.268".into(), "11.693".into()));
    pick(&mut page, "preset", "Tabloid");
    assert_eq!(size(&page), ("11.00".into(), "17.00".into()), "a sheet is written portrait");
    // Units converts: the sheet keeps its size.
    pick(&mut page, "units", "Millimetres");
    assert_eq!(size(&page), ("279.40".into(), "431.80".into()));
    // A raster size is written as it lies, in the page's unit.
    pick(&mut page, "units", "Pixels");
    pick(&mut page, "preset", "HD");
    assert_eq!(size(&page), ("1920".into(), "1080".into()));
    let root = image_root(vec![page.clone()]);
    let img = resolve_page(&root, &root.children[0], &mut Vec::new()).unwrap();
    assert_eq!((img.width, img.height), (1920, 1080));
    // Another row changes nothing.
    assert!(pick(&mut page, "resolution", "72").is_empty());
    // A size typed in is the size, whatever Preset still names.
    pick(&mut page, "width", "640");
    let root = image_root(vec![page.clone()]);
    assert_eq!(resolve_page(&root, &root.children[0], &mut Vec::new()).unwrap().width, 640);

    // A save from before: its rows carry the old conditions and an
    // Orientation row. A named preset is written into Width and Height,
    // turned as it was drawn; a Custom one keeps its size and names
    // Letter; the Orientation row goes.
    let old = |preset: &str, w: &str, h: &str, orientation: &str| {
        let mut n = template.clone();
        n.params.push(crate::app::ParamDef::new("orientation", "choice:Portrait,Landscape", orientation));
        for (row, v) in [("preset", preset), ("width", w), ("height", h)] {
            let p = n.params.iter_mut().find(|p| p.name == row).unwrap();
            p.set_type("text");
            p.set_text(v);
        }
        for row in ["width", "height"] {
            n.params.iter_mut().find(|p| p.name == row).unwrap().show_when = "Preset == Custom".into();
        }
        n
    };
    let mut tabloid = old("Tabloid", "8.5", "11.0", "Landscape");
    migrate_preset_rows(&mut tabloid);
    assert_eq!(size(&tabloid), ("17.00".into(), "11.00".into()));
    assert!(tabloid.params.iter().all(|p| p.name != "orientation"));
    let mut custom = old("Custom", "3", "2", "Portrait");
    migrate_preset_rows(&mut custom);
    assert_eq!(size(&custom), ("3".into(), "2".into()));
    assert_eq!(custom.params.iter().find(|p| p.name == "preset").unwrap().text(), "Letter");
    // Once: the merge takes the old conditions away, so a page loaded
    // a second time is left as it is.
    let mut loaded = image_root(vec![old("Custom", "3", "2", "Portrait")]);
    crate::app::merge_template_defs(&mut loaded, &templates);
    assert_eq!(size(&loaded.children[0]), size(&custom));
    crate::app::merge_template_defs(&mut loaded, &templates);
    assert_eq!(size(&loaded.children[0]), size(&custom), "a second load wrote Letter over a typed size");
    let mut merged = image_root(vec![old("A4", "8.5", "11.0", "Portrait")]);
    crate::app::merge_template_defs(&mut merged, &templates);
    assert_eq!(size(&merged.children[0]), ("8.268".into(), "11.693".into()));
    assert!(merged.children[0].params.iter().all(|p| p.invalid().is_none() && p.name != "orientation"));

    // Through MCP, and undone as one step.
    let mut state = State::new(false);
    let mut redraw = false;
    let slot = state.new_image().expect("the Page template is missing");
    let before = size(&state.current_dir().children[slot]);
    state
        .apply_action(crate::app::McpAction::SetParam { slot, name: "preset".into(), value: "Tabloid".into() }, &mut redraw)
        .unwrap();
    assert_eq!(size(&state.current_dir().children[slot]), ("11.00".into(), "17.00".into()));
    state.run_command("undo");
    assert_eq!(size(&state.current_dir().children[slot]), before, "the pick and what it wrote are one step");
}

/// A node drawing on an image is written in the image's unit: the same
/// rows on a pixel image and on an inch sheet put ink in different
/// places, and on each where the unit says.
#[test]
fn what_draws_on_an_image_is_in_the_images_unit() {
    use crate::page::resolve_page;
    let chain = |units: &str, w: &str, h: &str, dpi: &str| {
        let root = image_root(vec![
            image_node("page1", "page", &[("units", units), ("width", w), ("height", h), ("resolution", dpi), ("color", "1.00:1.00:1.00")]),
            image_node(
                "shape1",
                "page_shape",
                &[
                    ("input", "page1"), ("shape", "Rectangle"), ("x", "100"), ("y", "50"),
                    ("width", "20"), ("height", "10"), ("fill", "true"),
                    ("fill_color", "1.00:0.00:0.00"), ("stroke", "false"),
                ],
            ),
        ]);
        resolve_page(&root, &root.children[1], &mut Vec::new()).expect("no page")
    };
    let red = |p: &crate::page::Page, x: u32, y: u32| {
        let c = p.pixels[(y * p.width + x) as usize];
        c[0] > 0.9 && c[1] < 0.1
    };

    let px = chain("Pixels", "200", "100", "96");
    assert!(red(&px, 100, 50), "no ink at the rectangle's centre");
    assert!(red(&px, 91, 46) && red(&px, 109, 54), "the rectangle is not 20 x 10 pixels");
    assert!(!red(&px, 112, 50) && !red(&px, 100, 57), "ink outside the rectangle");

    // The same rows in millimetres, on a sheet 200 mm wide at 127 DPI:
    // five pixels to the millimetre, so the ink is five times as far in.
    let mm = chain("Millimetres", "200", "100", "127");
    assert_eq!((mm.width, mm.height), (1000, 500));
    assert!(red(&mm, 500, 250) && red(&mm, 545, 270), "the rectangle is not 20 x 10 millimetres");
    assert!(!red(&mm, 100, 50) && !red(&mm, 560, 250), "ink outside the rectangle");

    // And in inches the rectangle is off a 4 x 2 inch sheet altogether.
    let inch = chain("Inches", "4", "2", "50");
    assert!(inch.pixels.iter().all(|c| c[1] > 0.9), "a rectangle 100 inches out drew on the sheet");
}

/// The four outlines, turned and stroked: ink inside the outline and
/// none outside it.
#[test]
fn image_shapes_put_ink_inside_their_outline() {
    use crate::page::{Page, ShapeKind, ShapeSpec};
    let sheet = || Page::new([2.0, 2.0], 100, [1.0, 1.0, 1.0, 1.0]);
    let ink = |p: &Page, x: u32, y: u32| p.pixels[(y * p.width + x) as usize][1] < 0.5;
    let spec = |kind, size: [f32; 2], rotation| ShapeSpec {
        kind,
        center: [1.0, 1.0],
        size,
        rotation,
        corner_radius: 0.0,
        sides: 3,
        fill: Some([1.0, 0.0, 0.0, 1.0]),
        stroke: None,
    };

    // An ellipse fills its middle and not the corners of its box.
    let mut p = sheet();
    p.shape(&spec(ShapeKind::Ellipse, [1.0, 0.5], 0.0));
    assert!(ink(&p, 100, 100) && ink(&p, 145, 100) && ink(&p, 100, 120));
    assert!(!ink(&p, 148, 122), "the ellipse filled its box's corner");
    assert!(!ink(&p, 100, 128), "the ellipse is taller than it was asked to be");

    // Turned a quarter, a wide rectangle is a tall one.
    let mut p = sheet();
    p.shape(&spec(ShapeKind::Rectangle, [1.0, 0.2], 90.0));
    assert!(ink(&p, 100, 145) && ink(&p, 100, 55), "the turned rectangle is not tall");
    assert!(!ink(&p, 145, 100), "the turned rectangle is still wide");

    // A rounded corner leaves the box's corner clear.
    let mut p = sheet();
    p.shape(&ShapeSpec { corner_radius: 0.25, ..spec(ShapeKind::Rectangle, [1.0, 1.0], 0.0) });
    assert!(ink(&p, 100, 100) && ink(&p, 52, 100) && !ink(&p, 52, 52), "the corner was not rounded");

    // A triangle, its first corner at the top: ink under the apex, none
    // beside it.
    let mut p = sheet();
    p.shape(&spec(ShapeKind::Polygon, [1.0, 1.0], 0.0));
    assert!(ink(&p, 100, 60) && ink(&p, 100, 110));
    assert!(!ink(&p, 60, 60) && !ink(&p, 140, 60), "ink beside the triangle's apex");

    // A stroke alone draws the outline and leaves the middle.
    let mut p = sheet();
    p.shape(&ShapeSpec {
        fill: None,
        stroke: Some(([0.0, 0.0, 0.0, 1.0], 0.04)),
        ..spec(ShapeKind::Rectangle, [1.0, 1.0], 0.0)
    });
    assert!(ink(&p, 50, 100) && ink(&p, 100, 150), "no ink on the outline");
    assert!(!ink(&p, 100, 100) && !ink(&p, 40, 100), "ink off the outline");

    // A line is as long as its Width and as thick as its stroke.
    let mut p = sheet();
    p.shape(&ShapeSpec {
        stroke: Some(([0.0, 0.0, 0.0, 1.0], 0.06)),
        ..spec(ShapeKind::Line, [1.0, 0.0], 0.0)
    });
    assert!(ink(&p, 55, 100) && ink(&p, 145, 100) && ink(&p, 100, 102));
    assert!(!ink(&p, 100, 105) && !ink(&p, 155, 100), "the line is thicker or longer than asked");

    // An edge that is not on the pixel grid is covered in part, which is
    // what keeps a turned edge from being a staircase.
    let mut p = sheet();
    p.shape(&spec(ShapeKind::Rectangle, [1.005, 1.0], 0.0));
    let edge = p.pixels[(100 * p.width + 49) as usize][1];
    assert!(edge > 0.05 && edge < 0.95, "the edge pixel is all or nothing: {edge}");
}

/// The page nodes and the geometry nodes each have a display flag of
/// their own: showing an image leaves the geometry shown, and the other
/// way about.
#[test]
fn the_display_flag_is_exclusive_within_its_context() {
    let mut dir = image_root(vec![
        image_node("sphere1", "sphere", &[]),
        image_node("page1", "page", &[]),
        image_node("box1", "box", &[]),
        image_node("text1", "page_text", &[]),
    ]);
    for c in &mut dir.children {
        c.geometry_visible = false;
    }
    let flags = |d: &FsNode| d.children.iter().map(|c| c.geometry_visible).collect::<Vec<_>>();

    dir.set_child_geometry_visible(0, true);
    dir.set_child_geometry_visible(1, true);
    assert_eq!(flags(&dir), [true, true, false, false], "showing the image hid the geometry");
    dir.set_child_geometry_visible(3, true);
    assert_eq!(flags(&dir), [true, false, false, true], "two images are shown");
    dir.set_child_geometry_visible(2, true);
    assert_eq!(flags(&dir), [false, false, true, true], "showing geometry hid the image");
}

/// A State showing one image of `w` x `h` pixels at `dpi`, in a pane of
/// 1200 x 800.
pub(super) fn state_showing_image(w: u32, h: u32, dpi: u32) -> State {
    let mut state = State::new(false);
    state.last_viewport_width = 1200;
    state.last_viewport_height = 800;
    let slot = state.new_image().expect("the Page template is missing");
    let node = &mut state.current_dir_mut().children[slot];
    for (name, value) in [
        ("units", "Pixels".to_string()),
        ("width", w.to_string()),
        ("height", h.to_string()),
        ("resolution", dpi.to_string()),
    ] {
        node.params.iter_mut().find(|p| p.name == name).expect("a page row is missing").set_text(value);
    }
    state.rebuild_scene_geometry();
    state
}

/// The image's corners through the camera as the stage pass builds it:
/// NDC x and y of top-left, top-right, bottom-right, bottom-left.
fn image_corners_in_view(state: &State) -> [[f32; 2]; 4] {
    let (pos, rot, pivot) = state.active_camera_pose();
    let aspect = state.last_viewport_width as f32 / state.last_viewport_height as f32;
    let (proj, view, model) = state.viewport().get_matrices(aspect, Some(pos), Some(rot), Some(pivot));
    let mvp = proj * view * model;
    state.image_world_corners().expect("no image is shown").map(|c| {
        let p = mvp.project_point3(Vec3::from_array(c));
        [p.x, p.y]
    })
}

/// The level's image reaches the viewport: composed, uploaded, and
/// placed at its physical size in world units.
#[test]
fn a_shown_image_stands_in_the_scene_at_its_size() {
    let mut state = state_showing_image(400, 200, 100);
    let shown = state.page_shown.clone().expect("the image did not reach the viewport");
    assert!(state.page_image.is_some(), "nothing was uploaded");
    assert_eq!(shown.pixels, (400, 200));
    assert!(state.viewport_dirty, "nothing asks the scene to stage the image");

    // 4 x 2 inches, in a world of millimetres and in one of inches.
    let mm = shown.world_size(1.0);
    assert!((mm[0] - 101.6).abs() < 1e-3 && (mm[1] - 50.8).abs() < 1e-3, "{mm:?}");
    let inch = shown.world_size(25.4);
    assert!((inch[0] - 4.0).abs() < 1e-4 && (inch[1] - 2.0).abs() < 1e-4, "{inch:?}");
    // The top edge is up, the image faces +Z about its origin.
    let c = shown.world_corners(25.4);
    assert_eq!(c[0], [-2.0, 1.0, 0.0]);
    assert_eq!(c[2], [2.0, -1.0, 0.0]);

    // Hidden, it leaves the scene and frees its image.
    let slot = state.current_dir().children.iter().position(|n| n.id == shown.node_id).unwrap();
    state.current_dir_mut().set_child_geometry_visible(slot, false);
    state.viewport_dirty = false;
    state.rebuild_scene_geometry();
    assert!(state.page_shown.is_none() && state.page_image.is_none());
    assert!(state.viewport_dirty, "the scene keeps drawing an image that is gone");
}

/// Frame Image turns the camera square to the image and fits it: the
/// corners land symmetric about the view's centre, inside the pane, and
/// the axis that binds is nearly full. With the Default Camera, from any
/// orbit, and with a camera node.
#[test]
fn frame_image_faces_the_image_and_fits_it() {
    let framed = |state: &State, what: &str| {
        let [tl, tr, br, bl] = image_corners_in_view(state);
        for (a, b) in [(tl[0], -tr[0]), (tl[1], tr[1]), (bl[0], -br[0]), (tl[1], -bl[1]), (tl[0], bl[0])] {
            assert!((a - b).abs() < 1e-3, "{what}: the image is not seen head-on: {tl:?} {tr:?} {br:?} {bl:?}");
        }
        assert!(tl[0] < 0.0 && tl[1] > 0.0, "{what}: the image is seen from behind or upside down");
        let reach = tr[0].max(tr[1]);
        assert!(reach <= 1.0 && reach > 0.85, "{what}: the image spans {reach} of the pane");
    };

    // Wide, in a pane less wide than it: the width binds.
    let mut state = state_showing_image(400, 100, 100);
    state.viewport_mut().rotation_x = 0.3;
    state.viewport_mut().rotation_y = -1.1;
    state.viewport_mut().zoom = 3.0;
    assert!(state.run_command("frame_image"), "frame_image is not a command");
    framed(&state, "default camera, wide image");
    let [_, tr, _, _] = image_corners_in_view(&state);
    assert!(tr[0] > tr[1], "a wide image is bound by its width");

    // Tall: the height binds.
    let mut state = state_showing_image(100, 400, 100);
    state.frame_image();
    framed(&state, "default camera, tall image");

    // Off the origin, the camera goes to it.
    let mut state = state_showing_image(200, 200, 100);
    let slot = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
    state.current_dir_mut().children[slot]
        .params
        .iter_mut()
        .find(|p| p.name == "position")
        .unwrap()
        .set_text("3.00:-2.00:1.00");
    state.rebuild_scene_geometry();
    state.frame_image();
    framed(&state, "default camera, image off the origin");

    // A camera node is rewritten, whatever orbit the widget holds.
    let mut state = state_showing_image(300, 200, 100);
    assert!(state.camera_level().children.iter().any(|c| c.name == "camera1"), "the bundled project has no camera1");
    state.set_active_camera("camera1");
    state.viewport_mut().rotation_x = 0.2;
    state.viewport_mut().rotation_y = 0.7;
    state.frame_image();
    framed(&state, "camera node");

    // With no image shown the command says so and moves nothing.
    let mut state = State::new(false);
    let zoom = state.viewport().zoom;
    assert!(!state.frame_image());
    assert_eq!(state.viewport().zoom, zoom);
}

/// View Image Pixels 1:1 puts one pixel of the image on one of the pane.
#[test]
fn view_image_pixels_is_one_pixel_to_one() {
    let mut state = state_showing_image(300, 200, 100);
    assert!(state.run_command("view_image_pixels"));
    let [tl, tr, _, bl] = image_corners_in_view(&state);
    let wide = (tr[0] - tl[0]) * 0.5 * state.last_viewport_width as f32;
    let tall = (tl[1] - bl[1]) * 0.5 * state.last_viewport_height as f32;
    assert!((wide - 300.0).abs() < 0.5 && (tall - 200.0).abs() < 0.5, "the image covers {wide} x {tall} px");
}

/// Frame All holds the image as it holds the geometry: a scene that is
/// an image alone is still framed.
#[test]
fn frame_all_holds_the_image() {
    let mut state = state_showing_image(400, 400, 100);
    state.rt_sphere_verts.clear();
    let slot = state.current_dir().children.iter().position(|n| n.node_type == "page").unwrap();
    state.current_dir_mut().children[slot]
        .params
        .iter_mut()
        .find(|p| p.name == "position")
        .unwrap()
        .set_text("5.00:0.00:0.00");
    state.rebuild_scene_geometry();
    state.rt_sphere_verts.clear();
    state.frame_all();
    let (_, _, pivot) = state.active_camera_pose();
    assert!((pivot.x - 5.0).abs() < 1e-2, "Frame All did not go to the image: pivot {pivot:?}");
    for c in image_corners_in_view(&state) {
        assert!(c[0].abs() < 1.0 && c[1].abs() < 1.0, "a corner is out of the pane: {c:?}");
    }
}

/// The viewport menu offers the image's camera rows while one shows,
/// and not otherwise.
#[test]
fn the_viewport_menu_frames_an_image_that_is_shown() {
    let state = State::new(false);
    let (rows, _) = state.viewport_menu_rows();
    assert!(!rows.iter().any(|r| r == "Frame Image"), "a row for an image that is not there");
    let state = state_showing_image(100, 100, 100);
    let (rows, actions) = state.viewport_menu_rows();
    let at = rows.iter().position(|r| r == "Frame Image").expect("no Frame Image row");
    assert_eq!(actions[at], crate::app::ViewportMenuAction::Command("frame_image"));
    assert!(rows.iter().any(|r| r == "View Image Pixels 1:1"));
}

/// The Add … to Image commands: from nothing they make the image too;
/// the new node is wired after what it draws on, shown and selected,
/// placed and sized from the image in the image's unit; and added to the
/// middle of a chain it is inserted there.
#[test]
fn adding_to_an_image_wires_a_node_after_it() {
    let mut state = State::new(false);
    // No cell of the bundled project's is in the way down here.
    state.grid_cursor_col = 40;
    state.grid_cursor_row = 40;
    assert!(state.run_command("add_image_ellipse"), "add_image_ellipse is not a command");

    let names = |s: &State, ty: &str| {
        s.current_dir().children.iter().filter(|c| c.node_type == ty).map(|c| c.name.clone()).collect::<Vec<_>>()
    };
    assert_eq!(names(&state, "page"), ["page1"], "no image was made for the shape");
    assert_eq!(names(&state, "page_shape"), ["page_shape1"]);
    let text_of = |s: &State, node: &str, row: &str| {
        let n = s.current_dir().children.iter().find(|c| c.name == node).unwrap();
        n.params.iter().find(|p| p.name == row).unwrap_or_else(|| panic!("{node} has no {row}")).text().to_string()
    };
    assert_eq!(text_of(&state, "page_shape1", "input"), "page1");
    assert_eq!(text_of(&state, "page_shape1", "shape"), "Ellipse");
    // Letter, in inches: the middle of the sheet.
    assert_eq!(text_of(&state, "page_shape1", "x"), "4.25");
    assert_eq!(text_of(&state, "page_shape1", "y"), "5.50");

    let flag = |s: &State, node: &str| s.current_dir().children.iter().find(|c| c.name == node).unwrap().geometry_visible;
    assert!(flag(&state, "page_shape1") && !flag(&state, "page1"), "the new node is not what shows");
    let shape_slot = state.current_dir().children.iter().position(|c| c.name == "page_shape1").unwrap();
    assert_eq!(state.selected_slots(), [shape_slot], "the new node is not selected");
    let shown = state.page_shown.clone().expect("nothing is shown");
    assert_eq!(shown.node_id, state.current_dir().children[shape_slot].id);

    // Make the sheet small and in pixels, then add text to the PAGE: it
    // goes between the page and the shape.
    let page_slot = state.current_dir().children.iter().position(|c| c.name == "page1").unwrap();
    {
        let node = &mut state.current_dir_mut().children[page_slot];
        for (row, v) in [("units", "Pixels"), ("width", "300"), ("height", "200"), ("resolution", "96")] {
            node.params.iter_mut().find(|p| p.name == row).unwrap().set_text(v);
        }
    }
    state.graph_mut().set_selected_node(Some(page_slot));
    assert!(state.run_command("add_image_text"));
    assert_eq!(text_of(&state, "page_text1", "input"), "page1");
    assert_eq!(text_of(&state, "page_shape1", "input"), "page_text1", "the text was not inserted into the chain");
    assert_eq!(text_of(&state, "page_text1", "x"), "150");
    assert_eq!(text_of(&state, "page_text1", "y"), "100");
    assert_eq!(text_of(&state, "page_text1", "size"), "10");

    // The whole chain still composes, from its end.
    let end = state.current_dir().children.iter().find(|c| c.name == "page_shape1").unwrap();
    let page = crate::page::resolve_page(&state.fs_root, end, &mut Vec::new()).expect("the chain is broken");
    assert_eq!((page.width, page.height), (300, 200));

    // A rectangle, a line and a polygon are the same node under another
    // Shape.
    for (cmd, shape) in [("add_image_rectangle", "Rectangle"), ("add_image_line", "Line"), ("add_image_polygon", "Polygon")] {
        let before = names(&state, "page_shape").len();
        assert!(state.run_command(cmd), "{cmd} is not a command");
        let all = names(&state, "page_shape");
        assert_eq!(all.len(), before + 1, "{cmd} added nothing");
        assert_eq!(text_of(&state, all.last().unwrap(), "shape"), shape);
    }
}
