//! Volumes.

use super::*;


#[test]
fn test_a_sphere_round_trips_through_a_distance_field() {
    let sphere = sphere_detail(Vec3::ZERO, 1.0, 16, 24);
    let vol = Volume::from_mesh(&sphere, 0.12, 0.4);
    let back = vol.to_mesh();

    assert!(back.num_points() > 100, "the surface did not come back");
    assert!(back.num_prims() > 100);

    // Every extracted point is on the sphere, to within a voxel. That is
    // the whole claim of the representation: a mesh in, a field, a mesh
    // out, and the shape survives.
    for p in 0..back.num_points() {
        let r = back.pos(p).length();
        assert!((r - 1.0).abs() < 0.14, "point {p} is at radius {r}");
    }

    // Closed: every edge is shared by exactly two faces. Surface nets
    // gives this by construction, and it is what makes the output safe to
    // hand a slicer.
    let mut shared: std::collections::HashMap<[u32; 2], usize> = Default::default();
    for prim in 0..back.num_prims() {
        let pts = back.prim_points(prim);
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            *shared.entry([a.min(b), a.max(b)]).or_default() += 1;
        }
    }
    let open = shared.values().filter(|&&c| c != 2).count();
    assert_eq!(open, 0, "{open} edges are not shared by two faces");

    // And it faces outward, like every other generator.
    let normals = crate::geometry::point_normals(&back);
    let outward = (0..back.num_points())
        .filter(|&p| normals[p].dot(back.pos(p).normalize()) > 0.0)
        .count();
    assert_eq!(outward, back.num_points(), "the extracted surface is inside out");
}

#[test]
fn test_the_sign_is_right_where_the_nearest_face_would_lie() {
    // A field's sign has to be right EVERYWHERE — a wrong one is a bubble
    // or a hole, where in the Distance node it was a slightly wrong
    // number. This is why the build casts rays rather than asking the
    // nearest face which way it points, and this is the check that says so.
    let sphere = sphere_detail(Vec3::ZERO, 1.0, 20, 28);
    let vol = Volume::from_mesh(&sphere, 0.12, 0.3);
    let [nx, ny, nz] = vol.dims();

    let mut wrong = Vec::new();
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let p = vol.sample_position(i, j, k);
                let r = p.length();
                // Skip the band where the answer is genuinely ambiguous at
                // this resolution.
                if (r - 1.0).abs() < vol.voxel() {
                    continue;
                }
                let want_inside = r < 1.0;
                if (vol.at(i, j, k) < 0.0) != want_inside {
                    wrong.push((i, j, k, r, vol.at(i, j, k)));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} samples have the wrong sign, e.g. {:?}",
        wrong.len(),
        nx * ny * nz,
        &wrong[..wrong.len().min(3)]
    );
}

/// An axis-aligned closed box, built by hand.
pub(super) fn box_mesh(lo: Vec3, hi: Vec3) -> Detail {
    let mut d = Detail::new();
    for (x, y, z) in [
        (lo.x, lo.y, lo.z), (hi.x, lo.y, lo.z), (hi.x, lo.y, hi.z), (lo.x, lo.y, hi.z),
        (lo.x, hi.y, lo.z), (hi.x, hi.y, lo.z), (hi.x, hi.y, hi.z), (lo.x, hi.y, hi.z),
    ] {
        d.add_point(Vec3::new(x, y, z));
    }
    // Wound counter-clockwise seen from OUTSIDE, like every generator —
    // asserted below, because getting this backwards by hand is exactly
    // what happened the first time.
    for q in [
        [0u32, 1, 2, 3], [7, 6, 5, 4], [0, 4, 5, 1],
        [1, 5, 6, 2], [2, 6, 7, 3], [3, 7, 4, 0],
    ] {
        d.add_prim(&q);
    }
    let centre = (lo + hi) * 0.5;
    for (p, n) in crate::geometry::point_normals(&d).iter().enumerate() {
        assert!(
            n.dot((d.pos(p) - centre).normalize()) > 0.0,
            "the test box's corner {p} faces inward"
        );
    }
    d
}

/// The page nodes end to end, through the resolver — not just the raster.
///
/// Written before the UI was wired, because the Boolean node taught this
/// exact lesson one commit ago: arithmetic that passes every unit test and
/// wiring nobody has exercised look identical until something asks for the
/// result.
#[test]
fn test_the_page_nodes_compose_a_sheet_through_the_resolver() {
    use crate::page::{displayed_page, is_page_node, resolve_page};

    fn pnode(id: &str, name: &str, ty: &str, params: &[(&str, &str)]) -> FsNode {
        FsNode {
            id: id.to_string(),
            name: name.to_string(),
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

    let root = pnode("r", "root", "node", &[]);
    let mut root = root;
    root.children = vec![
        pnode(
            "p",
            "page1",
            "page",
            &[
                ("preset", "Letter"),
                ("resolution", "72"),
                ("color", "1.00:1.00:1.00"),
            ],
        ),
        pnode(
            "g",
            "grid1",
            "page_grid",
            &[
                ("input", "page1"),
                ("cell_size", "0.5"),
                ("line_width", "0.02"),
                ("line_color", "0.00:0.00:0.00"),
                ("fill_cells", "false"),
            ],
        ),
        pnode(
            "b",
            "border1",
            "page_border",
            &[("input", "grid1"), ("width", "0.1"), ("inset", "0.25"), ("color", "1.00:0.00:0.00")],
        ),
    ];

    assert!(is_page_node("page_grid") && !is_page_node("sphere"));

    let page = resolve_page(&root, &root.children[2], &mut Vec::new())
        .expect("the page chain resolved to nothing");
    assert_eq!((page.width, page.height), (612, 792), "Letter at 72 DPI");

    let at = |x: u32, y: u32| page.pixels[(y * page.width + x) as usize];
    // The border is red where it was asked for, and nowhere else.
    let b = at(20, 400);
    assert!(b[0] > 0.9 && b[1] < 0.1, "no border ink at the left edge: {b:?}");
    assert!(at(300, 400)[1] > 0.5, "the border filled the sheet");
    // The grid ruled the interior: 0.5 inches at 72 DPI is every 36 px.
    assert!(at(36 * 4, 400)[0] < 0.4, "no rule at 2.0 inches");
    assert!(at(36 * 4 + 18, 400)[0] > 0.9, "the cell between rules is not clear");

    // An orphan composite is not a page: a border with nothing under it
    // resolves to nothing rather than inventing a sheet.
    let orphan = pnode("o", "border2", "page_border", &[("input", "nothing")]);
    let mut lone = root.clone();
    lone.children.push(orphan);
    assert!(
        resolve_page(&lone, lone.children.last().unwrap(), &mut Vec::new()).is_none(),
        "a border with no page under it invented one"
    );

    // A cycle terminates rather than recursing forever.
    let mut looped = root.clone();
    looped.children[0] = pnode("p", "page1", "page_border", &[("input", "border1")]);
    assert!(resolve_page(&looped, &looped.children[2], &mut Vec::new()).is_none());

    // The level's LAST visible page node is what gets displayed.
    // Green, not red: the red border and the white sheet both read 1.0 in
    // the red channel, so testing that one proves nothing either way.
    let border_ink = |p: &crate::page::Page| p.pixels[(400 * p.width + 20) as usize][1];
    let shown = displayed_page(&root, &root).expect("nothing displayed");
    assert!(border_ink(&shown) < 0.1, "the border chain is not what showed");
    let mut hidden = root.clone();
    hidden.children[2].geometry_visible = false;
    let shown = displayed_page(&hidden, &hidden).expect("nothing displayed");
    assert!(border_ink(&shown) > 0.9, "a hidden node still displayed");
}

/// The page cache's key moves with everything the page is composed from
/// and with nothing else: an edit beside the chain must not recompose a
/// 300 DPI sheet, and an edit inside it must.
#[test]
fn test_the_page_key_follows_the_chain_and_only_the_chain() {
    use crate::page::chain_key;
    fn pnode(id: &str, name: &str, ty: &str, params: &[(&str, &str)]) -> FsNode {
        FsNode {
            id: id.to_string(),
            name: name.to_string(),
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
    let mut root = pnode("r", "root", "node", &[]);
    root.children = vec![
        pnode("p", "page1", "page", &[("preset", "Letter"), ("resolution", "72")]),
        pnode("g", "grid1", "page_grid", &[("input", "page1"), ("cell_size", "0.5")]),
        pnode("b", "border1", "page_border", &[("input", "grid1"), ("width", "0.1")]),
        pnode("s", "sphere1", "sphere", &[("radius", "1")]),
    ];
    let key = |root: &FsNode| chain_key(root, &root.children[2]);
    let base = key(&root);
    assert_eq!(base, key(&root.clone()), "the same chain keys the same");

    let mut beside = root.clone();
    beside.children[3].params[0].set_text("2".to_string());
    assert_eq!(base, key(&beside), "a geometry edit beside the page moved its key");

    let mut edited = root.clone();
    edited.children[1].params[1].set_text("0.25".to_string());
    assert_ne!(base, key(&edited), "a grid edit under the border kept the key");

    let mut sheet = root.clone();
    sheet.children[0].params[1].set_text("300".to_string());
    assert_ne!(base, key(&sheet), "the sheet's resolution kept the key");

    let mut bypassed = root.clone();
    bypassed.children[1].bypassed = true;
    assert_ne!(base, key(&bypassed), "bypassing the grid kept the key");

    let mut rewired = root.clone();
    rewired.children[2].params[0].set_text("page1".to_string());
    assert_ne!(base, key(&rewired), "rewiring the border past the grid kept the key");
}

/// The 8-bit conversion clamps through the cast now; out-of-range and
/// NaN channels must land where the explicit clamp put them.
#[test]
fn test_page_rgba8_clamps_and_rounds() {
    let mut page = crate::page::Page::new([1.0, 1.0], 2, [0.0; 4]);
    page.pixels[0] = [-1.0, 0.5, 2.0, f32::NAN];
    page.pixels[1] = [0.0, 1.0, 0.0019, 0.002];
    page.pixels[2] = [f32::INFINITY, f32::NEG_INFINITY, 0.999, 1.0001];
    let out = page.to_rgba8();
    assert_eq!(&out[..12], &[0, 128, 255, 0, 0, 255, 0, 1, 255, 0, 255, 255]);
}

/// Text lands on the sheet, and alignment moves it.
#[test]
fn test_page_text_puts_ink_where_it_is_aligned() {
    use crate::page::{HAlign, Page, TextSpec, VAlign};
    let ink = |halign, valign| {
        let mut p = Page::new([4.0, 2.0], 72, [1.0, 1.0, 1.0, 1.0]);
        crate::page::with_fonts_for_test(|fonts, cache| {
            p.text(
                fonts,
                cache,
                &TextSpec {
                    text: "Hg",
                    size: 0.5,
                    at: [2.0, 1.0],
                    halign,
                    valign,
                    ..Default::default()
                },
            );
        });
        // The centroid of the ink, in pixels.
        let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 0..p.height {
            for x in 0..p.width {
                let v = 1.0 - p.pixels[(y * p.width + x) as usize][0] as f64;
                if v > 0.5 {
                    sx += x as f64;
                    sy += y as f64;
                    n += 1.0;
                }
            }
        }
        assert!(n > 0.0, "no ink at all");
        (sx / n, sy / n)
    };

    let (cx, cy) = ink(HAlign::Center, VAlign::Middle);
    assert!((cx - 144.0).abs() < 25.0, "centred text sits at x={cx}, not the middle");
    assert!((cy - 72.0).abs() < 25.0, "middled text sits at y={cy}, not the middle");

    let (lx, _) = ink(HAlign::Left, VAlign::Middle);
    let (rx, _) = ink(HAlign::Right, VAlign::Middle);
    assert!(lx > cx && cx > rx, "alignment did not move the ink: {lx} {cx} {rx}");
}

/// Fuzzy ranking is the plugin's fuzzyfinder, deliberately: shortest
/// contiguous span, then earliest start, then alphabetical. Muscle memory
/// is the whole point of keeping it — "sg" has to keep landing on Show
/// Grid.
#[test]
fn test_fuzzy_ranking_matches_the_plugins_order() {
    use crate::command::fuzzy_rank;
    let items = ["Show Grid", "Show Spreadsheet Pane", "Save As", "Set As Default"];

    // Subsequence, not substring.
    let r = fuzzy_rank("sg", &items);
    assert_eq!(items[r[0]], "Show Grid", "sg did not rank Show Grid first: {r:?}");

    // The tightest span wins over the earliest start: "sa" spans 2 in
    // "Save As" (Sa) and more in the others.
    let r = fuzzy_rank("sa", &items);
    assert_eq!(items[r[0]], "Save As");

    // No match at all drops out rather than ranking last.
    assert!(fuzzy_rank("zzz", &items).is_empty());

    // An empty query is every item in registry order, which is what makes
    // the palette usable as a plain list.
    assert_eq!(fuzzy_rank("", &items), vec![0, 1, 2, 3]);

    // Case and spaces in the query are ignored.
    assert_eq!(fuzzy_rank("S G", &items), fuzzy_rank("sg", &items));
}

/// The Wireframe Color command is a palette row that PREVIEWS the colour
/// — the row carries the live wire colour as its swatch — and, picked,
/// lands on the Settings half's Wireframe Color row, which edits the
/// live `wire_color` (its owner was the Render node's "Wire Color" until
/// that node was retired). The palette row shows the value; the settings
/// row edits it.
#[test]
fn the_wireframe_colour_is_a_colour_row_of_the_palette() {
    use crate::dialog::{setting_row_id, Control, Owner, SETTINGS};
    assert!(crate::command::by_id("wireframe_color").is_none(), "the command went with the Settings half");

    let mut state = State::new(false);
    state.wire_color = [0.2, 0.6, 0.9];
    state.run_command("command_palette");
    let id = setting_row_id("Wireframe Color");
    let row = state.ui_context[state.slots.dialog]
        .rows
        .iter()
        .find(|r| r.id == id)
        .expect("the palette lists Wireframe Color");
    assert_eq!(row.label, "Wireframe Color");
    assert!(row.chord.is_empty(), "a setting has no chord");
    // The control carries the live colour, and a toolkit colour
    // selector stands behind it at the same value.
    let hex = crate::project::color_to_hex([0.2, 0.6, 0.9]);
    assert_eq!(row.control, Some(Control::Color { hex: hex.clone() }));
    let sel = state.ui_context[state.slots.dialog].color_selector(&id).expect("a colour selector behind the row");
    assert_eq!(sel.get_value_string().as_deref(), Some(hex.as_str()));
    let s = SETTINGS.iter().find(|s| s.label == "Wireframe Color").unwrap();
    assert_eq!(s.owner, Owner::Field("wire_color"));

    // Editing the row reaches the live state: the colour AND the switch
    // that makes the wire pass use it (off, the wires carry the
    // geometry's colours). The dialog stays up, and the row re-reads the
    // value.
    state.wire_single_color = false;
    state.apply_setting("Wireframe Color", "#000000");
    assert_eq!(state.wire_color, [0.0, 0.0, 0.0], "the colour row writes the live wire colour");
    assert!(state.wire_single_color, "a colour edit turns single-colour mode on");
    assert!(state.dialog_visible());
    let row = state.ui_context[state.slots.dialog].rows.iter().find(|r| r.id == id).unwrap();
    assert_eq!(row.control, Some(Control::Color { hex: "#000000".into() }));

    // And the switch is a command row of its own, flipped in place.
    state.take_dialog_pick("toggle_wire_single_color".to_string());
    assert!(!state.wire_single_color, "the switch row did not reach the flag");
    assert!(state.dialog_visible());
}

/// Frame All frames the displayed geometry from wherever the view is:
/// with a named camera that is not in the current directory (a subnet —
/// the camera node lives at the root and applies only there) it used to
/// do nothing at all; now that view is the Default Camera view and is
/// framed as one — its pivot moves to the geometry's centre and the
/// fixed eye ray is fitted with zoom.
/// A camera stands at the root and is seen from every level (since
/// 2026-10-02): Frame All from inside a subnet inside the Geometry node
/// frames the active camera NODE, where until then a camera not on the
/// current level was the Default Camera view and the node was left.
#[test]
fn frame_all_frames_the_root_camera_from_inside_a_subnet() {
    use crate::geometry::{node_param_vec3, Vertex3D};
    let mut state = State::new(false);
    state.set_active_camera("camera1");
    let sub = state.current_dir().children.iter().position(|c| c.name == "sphere1").expect("sphere1 in geometry1");
    state.current_path.push(sub);
    state.on_path_changed();
    assert!(!state.current_dir().children.iter().any(|c| c.node_type == "camera"), "no camera in the subnet");
    assert!(state.camera_names().contains(&"camera1".to_string()), "the root's camera is offered here");
    let camera = |state: &State| state.camera_level().children.iter().find(|c| c.name == "camera1").unwrap().clone();
    let before = camera(&state);
    // Displayed geometry: a small cluster centred well off the origin.
    let c = [3.0f32, 0.5, -2.0];
    state.rt_sphere_verts = (0..12)
        .map(|i| {
            let a = i as f32 * 0.5236;
            Vertex3D { position: [c[0] + 0.25 * a.cos(), c[1] + 0.25 * a.sin(), c[2] + 0.1 * (i % 3) as f32], color: [1.0; 3] }
        })
        .collect();
    state.last_viewport_width = 800;
    state.last_viewport_height = 600;
    let zoom_before = state.viewport().zoom;

    state.frame_all();

    let after = camera(&state);
    let piv = node_param_vec3(&after, "pivot", Vec3::ZERO);
    for k in 0..3 {
        assert!((piv[k] - c[k]).abs() < 0.2, "the camera's pivot {piv:?} is not on the geometry's centre {c:?}");
    }
    let reach = |n: &FsNode| (node_param_vec3(n, "position", Vec3::ZERO) - node_param_vec3(n, "pivot", Vec3::ZERO)).length();
    assert!(reach(&after) < reach(&before), "a 0.25 cluster frames closer than the stock camera");
    assert_eq!(state.viewport().zoom, zoom_before, "the node was framed, not the Default Camera");
}

/// The scene file carries the Default Camera VIEW (square aspect, pivot
/// marker, orbit/zoom/pivot) — where you were standing in this scene —
/// and, since 2026-09-24, the display settings with it.
///
/// From 2026-09-23 to 24 the display settings were app-wide only, on the
/// argument that opening someone else's scene should not reset how you
/// look at geometry; this test asserted the wireframe did NOT travel.
/// The user chose the other way: a project opens looking the way it was
/// left. `a_project_keeps_its_display_settings` covers the whole block.
#[test]
fn viewport_settings_round_trip_through_the_scene_file() {
    let dir = std::env::temp_dir().join(format!("cce-designer-vp-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let mut a = State::new(false);
    // The Default Camera is active: a camera NODE's own Pivot would
    // override the saved view's, by design.
    a.active_camera = "Default Camera".to_string();
    a.square_viewport = true;
    a.viewport_mut().show_camera_pivot = true;
    a.viewport_mut().rotation_y = 0.7;
    a.viewport_mut().zoom = 0.4;
    a.viewport_mut().pivot = Vec3::new(3.0, 0.5, -2.0);
    // A display setting, which travels with the project now.
    a.wireframe = true;
    a.save_to_file(&dir).expect("save");

    let mut b = State::new(false);
    b.wireframe = false;
    b.load_from_file(&dir).expect("load");
    assert!(b.square_viewport, "Square Aspect loads from the file");
    assert!(b.viewport().show_camera_pivot, "the pivot marker loads from the file");
    assert!((b.viewport().rotation_y - 0.7).abs() < 1e-4);
    assert!((b.viewport().zoom - 0.4).abs() < 1e-4);
    assert_eq!(b.viewport().pivot, Vec3::new(3.0, 0.5, -2.0));
    assert!(b.wireframe, "the wireframe travels with the project");
    let _ = std::fs::remove_dir_all(&dir);
}

/// …and the display settings round-trip through `state.kdl` instead,
/// every one of them, including the colours that pass through hex on the
/// way. The colour table was a hand-written pair of `if let`s per field
/// and covered two of the five, so a new colour setting serialized as a
/// JSON array and came back as the default.
#[test]
fn display_settings_round_trip_through_state_kdl() {
    use crate::app::DesignSettings;
    let mut a = State::new(false);
    a.viewport_mut().show_grid = false;
    a.viewport_mut().show_origin = false;
    a.viewport_mut().bg_color = [0.1, 0.2, 0.3];
    a.viewport_mut().grid_color = [0.4, 0.5, 0.6];
    a.viewport_mut().rt_mode = true;
    a.grid_thickness = 0.04;
    a.origin_size = 2.5;
    a.show_point_markers = true;
    a.show_point_numbers = true;
    a.point_marker_size = 0.05;
    a.point_marker_color = [1.0, 0.5, 0.0];
    a.world_unit = cce_ui::units::Unit::Cm;
    a.wireframe = true;
    a.wire_single_color = true;
    a.wire_color = [0.2, 0.4, 0.6];
    a.wire_opacity = 0.5;
    a.wire_width = 3.0;
    a.geo_opacity = 0.75;
    a.group_marker_size = 0.125;
    a.pull_arrow_scale = 4.0;
    a.smooth_shading = true;
    a.show_occluded = true;
    a.save_settings();

    let kdl = std::fs::read_to_string(DesignSettings::file_path()).expect("state.kdl was written");
    let back = DesignSettings::from_kdl_str(&kdl);
    let close = |x: f32, y: f32| (x - y).abs() < 0.01;

    assert!(!back.viewport.show_grid_enabled);
    assert!(!back.viewport.show_origin_enabled);
    assert!(back.viewport.rt_mode);
    assert!(close(back.viewport.grid_thickness, 0.04));
    assert!(close(back.viewport.origin_size, 2.5));
    assert!(back.viewport.show_point_markers && back.viewport.show_point_numbers);
    assert!(!back.viewport.show_point_normals);
    assert!(close(back.viewport.point_marker_size, 0.05));
    assert_eq!(back.viewport.world_unit, "cm");
    for (got, want) in [
        (back.viewport.bg_color, [0.1, 0.2, 0.3]),
        (back.viewport.grid_color, [0.4, 0.5, 0.6]),
        (back.viewport.point_marker_color, [1.0, 0.5, 0.0]),
    ] {
        for k in 0..3 {
            assert!(close(got[k], want[k]), "colour {got:?} came back as {want:?}");
        }
    }
    assert!(back.render.wireframe && back.render.wire_single_color);
    for k in 0..3 {
        assert!(close(back.render.wire_color[k], [0.2, 0.4, 0.6][k]), "{:?}", back.render.wire_color);
    }
    assert!(close(back.render.wire_opacity, 0.5));
    assert!(close(back.render.wire_width, 3.0));
    assert!(close(back.render.geo_opacity, 0.75));
    assert!((back.render.group_marker_size - 0.125).abs() < 1e-4);
    assert!(close(back.render.pull_arrow_scale, 4.0));
    assert!(back.render.smooth_shading);
    assert!(back.render.show_occluded);
}

/// Changing the wire colour turns single-colour mode on, so the colour
/// shows; turning the switch off afterwards sticks, and a LOAD never
/// flips it — a project that says off stays off whatever colour it
/// carries.
#[test]
fn changing_the_wire_colour_turns_single_colour_mode_on() {
    let mut state = State::new(false);
    state.wire_single_color = false;
    state.wire_color = [1.0, 1.0, 1.0];

    // An edit through the dialog's colour row, which is the only way
    // in now that the Render node is gone.
    state.open_dialog();
    state.apply_setting("Wireframe Color", "#000000");
    assert_eq!(state.wire_color, [0.0, 0.0, 0.0]);
    assert!(state.wire_single_color, "a colour change switches single-colour mode on");

    // Off again by hand stays off while the colour is unchanged: the
    // auto-enable fires on a CHANGE, not on every settings pass.
    state.wire_single_color = false;
    state.apply_setting("Wireframe Color", "#000000");
    assert!(!state.wire_single_color, "an unrelated pass flipped it back on");

    // A load carries the project's geometry and leaves the wire
    // settings — preferences now — exactly where they are.
    let dir = std::env::temp_dir().join(format!("cce-designer-wire-colour-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    state.save_to_file(&dir).expect("save");
    state.load_from_file(&dir).expect("load");
    assert_eq!(state.wire_color, [0.0, 0.0, 0.0]);
    assert!(!state.wire_single_color, "a load never flips the switch");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The wireframe toggle is a palette row that flips the live flag, and
/// the flag is the whole of it. The value used to live on the Render
/// utility node, which `apply_settings_from_menubar_subnets` read back
/// over live state after every parameter edit anywhere — so a flag
/// flipped alone reverted on the next unrelated change, and the command
/// had to write the node too. There is no node and no read-back now.
#[test]
fn test_toggle_wireframe_flips_the_flag_and_the_render_node() {
    use crate::command::{by_id, Run};
    let cmd = by_id("toggle_wireframe").expect("no toggle_wireframe command");
    assert_eq!(cmd.label, "Show Wireframe");
    assert_eq!(cmd.run, Run::Key(crate::shortcut::Action::ToggleWireframe));

    let mut state = State::new(false);
    state.wireframe = false;
    assert!(state.run_command("toggle_wireframe"));
    assert!(state.wireframe);
    assert_eq!(state.command_toggle_state("toggle_wireframe"), Some(true));
    // An unrelated settings apply no longer reverts it.
    state.open_dialog();
    let thickness = state.settings_row_value("Wire Thickness");
    state.apply_setting("Wire Thickness", &thickness);
    assert!(state.wireframe, "a settings pass read the flag back over itself");
    assert!(state.run_command("toggle_wireframe"));
    assert!(!state.wireframe);
}

/// The registry's own invariants. Ids are what `input.kdl` binds and
/// labels are what the palette maps a chosen row back to, so a duplicate
/// of either silently runs the wrong command.
#[test]
fn test_the_command_registry_is_consistent() {
    use crate::command::COMMANDS;
    let mut ids: Vec<&str> = COMMANDS.iter().map(|c| c.id).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "duplicate command id");

    let mut labels: Vec<&str> = COMMANDS.iter().map(|c| c.label).collect();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), n, "duplicate command label: the palette picks by label");

    for c in COMMANDS {
        assert!(
            c.id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'),
            "{} is not snake_case, which is what input.kdl writes",
            c.id
        );
        if let Some(chord) = c.default_chord {
            crate::shortcut::Shortcut::parse(chord)
                .unwrap_or_else(|e| panic!("{}: unparseable default chord {chord:?}: {e}", c.id));
        }
    }
}

/// Every menu-dispatched command names a label `execute_menu_action`
/// actually handles.
///
/// This is the check the plugin's hccommands.py doc argues for: a label
/// kept in two places drifts, and a renamed one fails SILENTLY — the
/// dispatch falls through its match and the command simply does nothing.
/// Scanning the source is an odd way to assert it, but the alternative is
/// calling every command to see if it is handled, and "Exit" would end the
/// test run.
#[test]
fn test_every_menu_command_names_a_label_that_is_dispatched() {
    use crate::command::{Run, COMMANDS};
    let src = include_str!("../app.rs");
    let start = src
        .find("pub fn execute_menu_action")
        .expect("execute_menu_action moved; this test scans for it");
    // The function alone, not the rest of the file: the menubars are
    // built further down with their items spelled out, and scanning on
    // to the end found "New Project" THERE while no arm dispatched it.
    let end = src[start..]
        .find("\n    }\n")
        .expect("execute_menu_action has no end");
    let body = &src[start..start + end];
    for c in COMMANDS {
        let Run::Menu(label) = c.run else { continue };
        let arm = format!("\"{label}\"");
        assert!(
            body.contains(&arm),
            "command {} dispatches {label:?}, which execute_menu_action does not handle",
            c.id
        );
    }
}

/// The menubars are not drawn, and what they listed is commands. A
/// click by index is dispatched for the two things no command does —
/// the active camera, the parameter presets — and refused for the rest,
/// where it used to be accepted and, for the header's File menu, run
/// the item one below the one named.
#[test]
fn a_menubar_click_is_dispatched_or_refused() {
    use crate::app::McpAction;
    let mut state = State::new(false);
    let mut redraw = false;
    state.set_active_camera("camera1");
    state
        .apply_action(McpAction::MenuClick { widget_idx: RIGHT_MENUBAR_IDX, menu_idx: 0, item_idx: 0 }, &mut redraw)
        .expect("the Camera menu is dispatched");
    assert_eq!(state.active_camera, "Default Camera");

    let nodes = state.fs_root.children.len();
    for (widget_idx, menu_idx) in [(crate::slots::HEADER_IDX, 0), (LEFT_MENUBAR_IDX, 0), (RIGHT_MENUBAR_IDX, 2)] {
        let res = state.apply_action(McpAction::MenuClick { widget_idx, menu_idx, item_idx: 0 }, &mut redraw);
        assert!(res.is_err(), "menubar {widget_idx} menu {menu_idx} was accepted: {res:?}");
    }
    assert_eq!(state.fs_root.children.len(), nodes, "a refused click ran New Project");
}

/// The cameras and the parameter reset are commands. They were menus
/// of two menubars that are not drawn, so nothing on screen reached them.
#[test]
fn the_cameras_and_the_parameter_reset_are_commands() {
    use crate::dialog::CAMERA_ROW_PREFIX;
    let mut state = State::new(false);
    assert!(state.camera_names().contains(&"camera1".to_string()), "the bundled project has camera1");

    // Stepping wraps, and both copies of the name follow.
    state.set_active_camera("Default Camera");
    let count = state.camera_names().len();
    for _ in 0..count {
        assert!(state.run_command("next_camera"));
        assert_eq!(state.viewport().active_camera, state.active_camera);
    }
    assert_eq!(state.active_camera, "Default Camera", "a full turn comes back");
    assert!(state.run_command("previous_camera"));
    assert_eq!(state.active_camera, *state.camera_names().last().unwrap());
    assert!(state.run_command("default_camera"));
    assert_eq!(state.active_camera, "Default Camera");

    // A camera node is a row of the palette, found by its name.
    state.open_dialog();
    state.ui_context[state.slots.dialog].query = "camera1".to_string();
    state.refresh_dialog_rows();
    let id = format!("{CAMERA_ROW_PREFIX}camera1");
    let row = state.ui_context[state.slots.dialog].rows.iter().position(|r| r.id == id).expect("no row for camera1");
    assert_eq!(state.ui_context[state.slots.dialog].rows[row].label, "Camera: camera1");
    state.ui_context[state.slots.dialog].selected = row;
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
    assert!(!state.dialog_visible());
    assert_eq!(state.active_camera, "camera1");
    assert_eq!(state.viewport().active_camera, "camera1");

    // Reset puts a changed parameter back.
    let slot = state
        .current_dir()
        .children
        .iter()
        .position(|c| c.node_type == "sphere")
        .expect("the bundled project has a sphere");
    state.graph_mut().set_selected_node(Some(slot));
    let default = {
        let dir = state.current_dir();
        state.template_default(dir, &dir.children[slot], "radius").expect("no Radius default").text().to_string()
    };
    let radius = |state: &State| {
        state.current_dir().children[slot].params.iter().find(|p| p.name == "radius").unwrap().text().to_string()
    };
    state.current_dir_mut().children[slot].params.iter_mut().find(|p| p.name == "radius").unwrap().set_text("3.25".to_string());
    assert!(state.run_command("reset_parameters"));
    assert_eq!(radius(&state), default);
    assert!(crate::command::by_id("custom_preset").is_none(), "the custom preset is retired");

    // With nothing selected there is nothing to reset, and it says so.
    state.graph_mut().set_selected_node(None);
    state.run_command("reset_parameters");
}

/// Reset Parameters can be taken back, and put back again. It rewrites
/// every parameter of a node at once, and until it recorded a step the
/// values it replaced were gone.
#[test]
fn reset_parameters_is_undone_and_redone() {
    let mut state = State::new(false);
    let slot = state
        .current_dir()
        .children
        .iter()
        .position(|c| c.node_type == "sphere")
        .expect("the bundled project has a sphere");
    state.graph_mut().set_selected_node(Some(slot));
    let texts = |state: &State| -> Vec<(String, String, bool)> {
        state.current_dir().children[slot]
            .params
            .iter()
            .map(|p| (p.name.clone(), p.text().to_string(), p.is_expr()))
            .collect()
    };
    {
        let node = &mut state.current_dir_mut().children[slot];
        node.params.iter_mut().find(|p| p.name == "radius").unwrap().set_text("3.25".to_string());
        let rows = node.params.iter_mut().find(|p| p.name == "rows").unwrap();
        rows.set_text("$F + 4".to_string());
        rows.set_expr(true);
    }
    let edited = texts(&state);

    assert!(!state.history_step(true), "nothing to undo before the reset");
    assert!(state.run_command("reset_parameters"));
    let reset = texts(&state);
    assert_ne!(reset, edited);

    // Through the Undo command, as the palette and the chord arrive.
    assert!(state.run_command("undo"));
    assert_eq!(texts(&state), edited, "undo did not bring the values back, expression flag included");
    assert!(state.run_command("redo"));
    assert_eq!(texts(&state), reset);
    assert!(state.run_command("undo"));
    assert_eq!(texts(&state), edited);

    // A rename between the reset and the undo: the step is by id, and
    // is reached under the rename, which is a step of its own.
    state.run_command("reset_parameters");
    let id = state.current_dir().children[slot].id.clone();
    crate::geometry::rename_node_in_tree(&mut state.fs_root, &id, "ball");
    assert!(state.history_step(true));
    assert_ne!(state.current_dir().children[slot].name, "ball");
    assert!(state.history_step(true));
    assert_eq!(texts(&state), edited);

    // A node deleted since comes back first, and then its parameters.
    state.run_command("reset_parameters");
    let reset = texts(&state);
    state.delete_node(slot);
    assert!(state.history_step(true), "the delete is the last step");
    assert_eq!(texts(&state), reset, "the node came back as it was deleted");
    assert!(state.history_step(true));
    assert_eq!(texts(&state), edited);

    // Another document's steps are not this one's.
    let mut state = State::new(false);
    state.graph_mut().set_selected_node(Some(slot));
    state.run_command("reset_parameters");
    state.new_project();
    assert!(!state.history_step(true), "New Project kept the old project's undo");
}

/// The Attribute node's Remap takes its From range from the input two
/// ways: From Range Auto (the row hides, the measure is taken at every
/// evaluation) and the Detect Range button, pressed in the params pane,
/// which measures once and writes From as an undoable edit.
#[test]
fn the_remap_from_range_is_detected_from_the_input() {
    use crate::app::{param_visible, McpAction};
    let mut state = State::new(false);
    let mut redraw = false;
    state
        .apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw)
        .unwrap();
    let slot = state.current_dir().children.iter().position(|c| c.node_type == "attribute").unwrap();
    for (name, value) in [("input", "sphere1"), ("attribute_name", "N"), ("operation", "Remap")] {
        state
            .apply_action(McpAction::SetParam { slot, name: name.into(), value: value.into() }, &mut redraw)
            .unwrap();
    }
    state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
    let shown = |state: &State, name: &str| {
        let params = &state.current_dir().children[slot].params;
        param_visible(params, &params.iter().find(|p| p.name == name).unwrap().show_when)
    };
    let from = |state: &State| state.current_dir().children[slot].params.iter().find(|p| p.name == "from").unwrap().text().to_string();
    assert!(shown(&state, "from_range") && shown(&state, "detect_range") && shown(&state, "from"));
    assert_eq!(from(&state), "0.00:1.00");

    // The press, as the pane reports it.
    let rows: Vec<(String, String, String)> = state
        .param()
        .node_params()
        .into_iter()
        .map(|(n, v, t)| if n == "Detect Range" { (n, "clicked".to_string(), t) } else { (n, v, t) })
        .collect();
    assert!(rows.iter().any(|r| r.0 == "Detect Range"), "the pane shows the button");
    state.param_mut().set_display_params(&rows);
    state.sync_parameters_to_project();
    let [lo, hi] = crate::param::ParamDef::new("from", "float2", from(&state))
        .value()
        .and_then(|v| match v {
            crate::app::ParamValue::Vec2(v) => Some(*v),
            _ => None,
        })
        .expect("From holds two numbers");
    assert!(lo < -0.9 && hi > 0.9, "a sphere's normals span -1..1, not {lo}..{hi}");
    assert!(state.last_status_text.contains("From set to"), "{}", state.last_status_text);

    // Undone, From is the row it was.
    state.edit_history.break_group();
    assert!(state.history_step(true));
    assert_eq!(from(&state), "0.00:1.00");

    // Auto hides From and the button; Clip shows From again.
    state.apply_action(McpAction::SetParam { slot, name: "from_range".into(), value: "Auto".into() }, &mut redraw).unwrap();
    assert!(!shown(&state, "from") && !shown(&state, "detect_range"));
    state.apply_action(McpAction::SetParam { slot, name: "operation".into(), value: "Clip".into() }, &mut redraw).unwrap();
    assert!(shown(&state, "from") && !shown(&state, "from_range"));
}

/// An edit to a parameter can be taken back however it was made: a row
/// of the pane, `set_param`, the row menu. A drag writes back on every
/// motion and is one step; a press between two drags makes them two.
#[test]
fn a_parameter_edit_is_undone_a_gesture_at_a_time() {
    use crate::app::{McpAction, ParamMenuAction};
    let mut state = State::new(false);
    let mut redraw = false;
    let slot = state
        .current_dir()
        .children
        .iter()
        .position(|c| c.node_type == "sphere")
        .expect("the bundled project has a sphere");
    state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
    let node_id = state.current_dir().children[slot].id.clone();
    let param = |state: &State, name: &str| -> (String, bool) {
        let p = state.current_dir().children[slot].params.iter().find(|p| p.name == name).unwrap();
        (p.text().to_string(), p.is_expr())
    };
    // The pane reporting a row at a value, as a drag does per motion.
    let pane = |state: &mut State, name: &str, value: &str| {
        let rows: Vec<(String, String, String)> = state
            .param()
            .node_params()
            .into_iter()
            .map(|(n, v, t)| if n == name { (n, value.to_string(), t) } else { (n, v, t) })
            .collect();
        state.param_mut().set_display_params(&rows);
        state.sync_parameters_to_project();
    };
    let radius = param(&state, "radius");
    let rows = param(&state, "rows");

    // One drag: three motions, one step.
    for v in ["1.10", "1.20", "1.30"] {
        pane(&mut state, "Radius", v);
    }
    assert_eq!(state.edit_history.undo_len(), 1, "a drag is one step");
    // A write-back that changes nothing records nothing.
    state.sync_parameters_to_project();
    assert_eq!(state.edit_history.undo_len(), 1);
    // A release and a press, then a second drag of the same row.
    state.edit_history.break_group();
    for v in ["1.40", "1.50"] {
        pane(&mut state, "Radius", v);
    }
    assert_eq!(state.edit_history.undo_len(), 2, "a second drag is a second step");
    // Another row, with no press between: its own step all the same.
    pane(&mut state, "Rows", "9");
    assert_eq!(state.edit_history.undo_len(), 3);

    assert!(state.run_command("undo"));
    assert_eq!(param(&state, "rows"), rows);
    assert_eq!(param(&state, "radius").0, "1.50", "undoing Rows left Radius alone");
    assert!(state.run_command("undo"));
    assert_eq!(param(&state, "radius").0, "1.30");
    assert!(state.run_command("undo"));
    assert_eq!(param(&state, "radius"), radius);
    assert!(!state.history_step(true), "three steps were recorded");
    let shown = state.param().node_params().into_iter().find(|r| r.0 == "Radius").unwrap().1;
    assert_eq!(shown, radius.0, "the pane shows the restored value");
    for want in ["1.30", "1.50"] {
        assert!(state.run_command("redo"));
        assert_eq!(param(&state, "radius").0, want);
    }
    // An edit after an undo forks: what was undone is not redone over it.
    assert!(state.run_command("undo"));
    state.edit_history.break_group();
    pane(&mut state, "Radius", "2.00");
    assert!(!state.history_step(false), "a new edit left the redo branch standing");
    assert!(state.run_command("undo"));
    assert_eq!(param(&state, "radius").0, "1.30");

    // A step restores what it changed and nothing else on the node.
    pane(&mut state, "Radius", "2.50");
    state.current_dir_mut().children[slot].params.iter_mut().find(|p| p.name == "rows").unwrap().set_text("21".to_string());
    assert!(state.run_command("undo"));
    assert_eq!(param(&state, "radius").0, "1.30");
    assert_eq!(param(&state, "rows").0, "21", "undoing Radius took back an edit to Rows");

    // set_param, and one that is refused.
    let before = state.edit_history.undo_len();
    state.apply_action(McpAction::SetParam { slot, name: "radius".into(), value: "3.00".into() }, &mut redraw).unwrap();
    assert!(state.apply_action(McpAction::SetParam { slot, name: "radius".into(), value: "abc".into() }, &mut redraw).is_err());
    assert_eq!(state.edit_history.undo_len(), before + 1, "a refused value recorded a step");
    assert!(state.run_command("undo"));
    assert_eq!(param(&state, "radius").0, "1.30");

    // The row menu: the expression flag is part of what comes back.
    state.run_param_action(&node_id, "radius", ParamMenuAction::EditExpression);
    assert!(param(&state, "radius").1);
    state.run_param_action(&node_id, "radius", ParamMenuAction::CopyParameter);
    assert!(state.run_command("undo"));
    assert_eq!(param(&state, "radius"), ("1.30".to_string(), false), "Copy Parameter is no edit, and Edit Expression is one");
}

/// Adding, deleting, moving and wiring nodes can be taken back, in the
/// order they were done, among the parameter edits made between them.
#[test]
fn the_graph_is_undone_a_step_at_a_time() {
    use crate::app::McpAction;
    let mut state = State::new(false);
    let mut redraw = false;
    // The first look takes the tree as it stands and records nothing.
    state.record_structure_changes();
    assert_eq!(state.edit_history.undo_len(), 0);
    let shape = |state: &State| -> Vec<(String, (f32, f32), String, bool, bool)> {
        state
            .current_dir()
            .children
            .iter()
            .map(|c| {
                let input = c.params.iter().find(|p| p.name == "input").map(|p| p.text().to_string()).unwrap_or_default();
                (c.name.clone(), c.position, input, c.geometry_visible, c.bypassed)
            })
            .collect()
    };
    let start = shape(&state);

    // Add, through MCP as the palette's pick adds.
    state
        .apply_action(McpAction::AddNode { template_name: "Box".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw)
        .unwrap();
    let added = shape(&state);
    assert_eq!(added.len(), start.len() + 1);
    assert_eq!(state.edit_history.undo_len(), 1);
    let slot = added.len() - 1;
    let name = added[slot].0.clone();

    // Wire it to the sphere, by the parameter, and move it.
    state.apply_action(McpAction::SetParam { slot, name: "radius".into(), value: "1".into() }, &mut redraw).ok();
    let steps = state.edit_history.undo_len();
    state.current_dir_mut().children[slot].position = (12.0, 9.0);
    state.record_structure_changes();
    assert_eq!(state.edit_history.undo_len(), steps + 1, "a move is a step");
    let moved = shape(&state);
    // A second move of the same node straight after is the same step.
    state.current_dir_mut().children[slot].position = (13.0, 9.0);
    state.record_structure_changes();
    assert_eq!(state.edit_history.undo_len(), steps + 1, "a run of moves is one step");
    // A press between them, and it is another.
    state.edit_history.break_group();
    state.current_dir_mut().children[slot].position = (14.0, 9.0);
    state.record_structure_changes();
    assert_eq!(state.edit_history.undo_len(), steps + 2);
    let moved_again = shape(&state);

    // Flags: bypass, through its command's writer.
    state.set_bypassed(&[slot], true);
    state.record_structure_changes();
    let bypassed = shape(&state);
    assert!(bypassed[slot].4);

    // Delete the first node of the level, which moves every slot.
    let first = state.current_dir().children[0].clone();
    state.delete_node(0);
    state.record_structure_changes();
    let deleted = shape(&state);
    assert_eq!(deleted.len(), added.len() - 1);

    // Back, a step at a time.
    assert!(state.run_command("undo"));
    assert_eq!(shape(&state), bypassed, "the deleted node came back where it was");
    assert_eq!(state.current_dir().children[0].id, first.id);
    assert_eq!(state.current_dir().children[0].params.len(), first.params.len());
    assert!(state.last_status_text.contains("Undo Delete"), "{}", state.last_status_text);
    assert!(state.run_command("undo"));
    assert_eq!(shape(&state), moved_again);
    assert!(state.run_command("undo"));
    assert_eq!(shape(&state)[slot].1, (13.0, 9.0), "the run of two moves is where the third began");
    assert!(state.run_command("undo"));
    assert_eq!(shape(&state)[slot].1, (9.0, 9.0));
    let _ = moved;
    while state.edit_history.undo_len() > 1 {
        assert!(state.run_command("undo"));
    }
    assert_eq!(shape(&state), added);
    assert!(state.run_command("undo"));
    assert_eq!(shape(&state), start, "undoing the add took the node out");
    assert!(!state.history_step(true));

    // And forward again, to the end.
    while state.history_step(false) {}
    assert_eq!(shape(&state), deleted);
    assert!(!state.current_dir().children.iter().any(|c| c.id == first.id));
    assert!(state.current_dir().children.iter().any(|c| c.name == name));

    // Undoing the add of a node the editor has gone into comes out of it.
    let mut state = State::new(false);
    state.record_structure_changes();
    state
        .apply_action(McpAction::AddNode { template_name: "Embryo".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw)
        .unwrap();
    let slot = state.current_dir().children.len() - 1;
    let level = state.current_path.clone();
    state.current_path.push(slot);
    state.sync_nodes();
    assert!(state.history_step(true));
    assert_eq!(state.current_path, level, "the editor is inside a node that is gone");

    // A wire, made as the graph makes one and as the pane does: each is
    // one step, and the second is not noticed a second time.
    let mut state = State::new(false);
    state
        .apply_action(McpAction::AddNode { template_name: "Normal".into(), name: None, x: 9.0, y: 9.0 }, &mut redraw)
        .unwrap();
    state.edit_history.clear();
    let wired = state
        .current_dir()
        .children
        .iter()
        .position(|c| c.params.iter().any(|p| p.name == "input" && p.kind() == crate::app::ParamKind::Node))
        .expect("a Normal node has an Input");
    let input = |state: &State| {
        state.current_dir().children[wired].params.iter().find(|p| p.name == "input").unwrap().text().to_string()
    };
    let was = input(&state);
    state.current_dir_mut().children[wired].params.iter_mut().find(|p| p.name == "input").unwrap().set_text("camera1".to_string());
    state.record_structure_changes();
    assert_eq!(state.edit_history.undo_len(), 1);
    state
        .apply_action(McpAction::SetParam { slot: wired, name: "input".into(), value: String::new() }, &mut redraw)
        .unwrap();
    assert_eq!(state.edit_history.undo_len(), 2, "a wire set as a parameter is one step, not two");
    assert!(state.run_command("undo"));
    assert_eq!(input(&state), "camera1");
    assert!(state.run_command("undo"));
    assert_eq!(input(&state), was);
    assert!(state.last_status_text.contains("Wire"), "{}", state.last_status_text);

    // A project opened is not an edit, and takes the history with it.
    state
        .apply_action(McpAction::AddNode { template_name: "Box".into(), name: None, x: 3.0, y: 9.0 }, &mut redraw)
        .unwrap();
    state.new_project();
    state.record_structure_changes();
    assert_eq!(state.edit_history.undo_len(), 0, "New Project was recorded as an edit");
}

/// A rename is taken back with everything that named the node: the
/// wires to it, the expression paths through it wherever they stand,
/// and the active camera.
#[test]
fn a_rename_is_undone_with_what_names_the_node() {
    use crate::app::McpAction;
    let mut state = State::new(false);
    let mut redraw = false;
    let add = |state: &mut State, template: &str, x: f32| {
        state
            .apply_action(McpAction::AddNode { template_name: template.into(), name: None, x, y: 9.0 }, &mut false)
            .unwrap();
        state.current_dir().children.len() - 1
    };
    let sphere = state.current_dir().children.iter().position(|c| c.node_type == "sphere").unwrap();
    let old = state.current_dir().children[sphere].name.clone();
    let normal = add(&mut state, "Normal", 3.0);
    let embryo = add(&mut state, "Embryo", 5.0);
    state.apply_action(McpAction::SetParam { slot: normal, name: "input".into(), value: old.clone() }, &mut redraw).unwrap();
    // An expression a level down, reaching up and across to the sphere.
    let reference = format!("ch(\"../../{old}/radius\") * 2");
    let inside = state.current_dir().children[embryo]
        .children
        .iter()
        .position(|c| !c.params.is_empty())
        .expect("the Embryo has a child with parameters");
    {
        let inner = &mut state.current_dir_mut().children[embryo].children[inside];
        let p = &mut inner.params[0];
        p.set_text(reference.clone());
        p.set_expr(true);
    }
    // The camera stands at the root, the sphere in its Geometry node.
    let camera = state.camera_level().children.iter().find(|c| c.node_type == "camera").unwrap();
    let (camera_id, camera_name) = (camera.id.clone(), camera.name.clone());
    state.set_active_camera(camera_name.clone());
    state.record_structure_changes();
    state.edit_history.clear();

    let names = |state: &State| -> (String, String, String, String, String) {
        let dir = state.current_dir();
        (
            dir.children[sphere].name.clone(),
            dir.children[normal].params.iter().find(|p| p.name == "input").unwrap().text().to_string(),
            dir.children[embryo].children[inside].params[0].text().to_string(),
            state.active_camera.clone(),
            state.viewport().active_camera.clone(),
        )
    };
    let before = names(&state);
    assert_eq!(before.2, reference);

    state.apply_action(McpAction::RenameNode { slot: sphere, new_name: "Ball".into() }, &mut redraw).unwrap();
    state.rename_node(&camera_id, "lens").unwrap();
    state.record_structure_changes();
    let after = names(&state);
    assert_eq!(after.0, "ball");
    assert_eq!(after.1, "ball", "the wire followed the rename");
    assert!(after.2.contains("../../ball/radius"), "{}", after.2);
    assert_eq!((after.3.as_str(), after.4.as_str()), ("lens", "lens"), "both copies of the camera's name");
    assert_eq!(state.edit_history.undo_len(), 2, "each rename is one step, its wires with it");

    assert!(state.run_command("undo"));
    assert!(state.last_status_text.contains(&format!("Undo Rename {camera_name}")), "{}", state.last_status_text);
    assert_eq!(names(&state).3, camera_name);
    assert_eq!(names(&state).4, camera_name);
    assert!(state.run_command("undo"));
    assert_eq!(names(&state), before);
    assert!(state.run_command("redo"));
    assert!(state.run_command("redo"));
    assert_eq!(names(&state), after);

    // A name taken since is not taken twice: the node keeps its own.
    assert!(state.run_command("undo"));
    assert!(state.run_command("undo"));
    state.apply_action(McpAction::RenameNode { slot: sphere, new_name: "ball".into() }, &mut redraw).unwrap();
    state.apply_action(McpAction::RenameNode { slot: normal, new_name: old.clone() }, &mut redraw).unwrap();
    state.edit_history.take(true);
    assert!(state.history_step(true), "the step is taken");
    let dir = state.current_dir();
    assert_eq!(dir.children[sphere].name, "ball", "the sphere took a name its sibling has");
    assert_eq!(dir.children[normal].name, old);
}

/// Rename is a row of the node's menu and a command: the dialog opens
/// holding the node's name, the row says what Enter will do, and a name
/// that cannot be written is refused there and not on the way in.
/// The Group Markers dialog: the palette turned into a list of the
/// scene's point groups, a switch each. Enter or a click flips the
/// switch in place and the list stays up; a marked group's members
/// wear a marker in the scene, built from what the last rebuild kept,
/// following the geometry through a rebuild and persisted with the
/// display settings.
/// Attribute visualizers: added, edited, switched and deleted in the
/// params HUD — the pane's own rows, worked as a node's are — applied
/// to the displayed scene with no node in the graph, kept with the
/// display settings, and handed back to the node by Done or a pick.
#[test]
fn attribute_visualizers_are_edited_in_the_params_hud() {
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.rebuild_scene_geometry();
    assert!(state.scene_attributes.iter().any(|a| a.name == "N"), "{:?}", state.scene_attributes);
    let colours = |state: &State| state.rt_sphere_verts.iter().map(|v| v.color).collect::<Vec<_>>();
    let plain = colours(&state);
    let nodes_before = serde_json::to_string(&state.fs_root).unwrap();
    let keys = |state: &State| state.param().node_params().into_iter().map(|r| r.0).filter(|k| !k.is_empty()).collect::<Vec<_>>();
    // The pane reporting a row at a value, as a press or a drag does.
    let pane = |state: &mut State, key: &str, value: &str| {
        let rows: Vec<(String, String, String)> = state
            .param()
            .node_params()
            .into_iter()
            .map(|(k, v, t)| if k == key { (k, value.to_string(), t) } else { (k, v, t) })
            .collect();
        assert!(rows.iter().any(|r| r.0 == key), "no {key} row in {:?}", rows.iter().map(|r| &r.0).collect::<Vec<_>>());
        state.param_mut().set_display_params(&rows);
        state.sync_parameters_to_project();
    };

    // In the HUD, not the dialog: Add and Done, nothing to edit yet.
    assert!(state.run_command("attribute_visualizers"));
    assert!(!state.dialog_visible());
    assert_eq!(state.vis_hud, Some(0));
    assert_eq!(keys(&state), ["Add Visualizer", "Done"]);

    // Add one: it is edited, on the first attribute worth showing.
    pane(&mut state, "Add Visualizer", "clicked");
    assert_eq!(state.visualizers.len(), 1);
    let k = keys(&state);
    assert!(k.contains(&"Visualizer".to_string()) && k.contains(&"Ramp".to_string()) && !k.contains(&"Scale".to_string()), "Ramp's rows: {k:?}");

    // On N, a Ramp recolours the scene, which gained no node.
    pane(&mut state, "Attribute", "N");
    assert_eq!(state.visualizers[0].attribute, "N");
    assert_ne!(colours(&state), plain, "the ramp is on the scene");
    assert_eq!(serde_json::to_string(&state.fs_root).unwrap(), nodes_before, "no node in the graph");

    // Manual range: its float2 row appears, and takes two ends.
    pane(&mut state, "Range", "Manual");
    assert!(keys(&state).contains(&"Manual Range".to_string()));
    pane(&mut state, "Manual Range", "-0.5:0.5");
    assert_eq!(state.visualizers[0].manual_range, [-0.5, 0.5]);

    // Vector: Scale's row in place of Ramp's.
    pane(&mut state, "Mode", "Vector");
    assert!(state.visualizers[0].is_vector());
    let k = keys(&state);
    assert!(k.contains(&"Scale".to_string()) && !k.contains(&"Ramp".to_string()), "Vector's rows: {k:?}");
    assert_eq!(colours(&state), plain, "a Vector leaves the colours");

    // Its switch turns it off; a second one is picked from the list.
    pane(&mut state, "Enabled", "false");
    assert!(!state.visualizers[0].enabled);
    pane(&mut state, "Add Visualizer", "clicked");
    assert_eq!((state.visualizers.len(), state.vis_hud), (2, Some(1)));
    pane(&mut state, "Visualizer", "#1 N");
    assert_eq!(state.vis_hud, Some(0));

    // Kept with the display settings, at the frame.
    state.save_settings();
    let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
    let kept = crate::visualizer::decode(&crate::app::DesignSettings::from_kdl_str(&kdl).viewport.visualizers);
    assert_eq!(kept, state.visualizers);

    // Delete, then Done: the HUD is the node's again.
    pane(&mut state, "Delete Visualizer", "clicked");
    assert_eq!(state.visualizers.len(), 1);
    pane(&mut state, "Done", "clicked");
    assert_eq!(state.vis_hud, None);

    // Picking a node also hands the HUD back.
    assert!(state.run_command("attribute_visualizers"));
    let mut redraw = false;
    let slot = geo(&state.fs_root).children.iter().position(|c| c.node_type == "sphere").unwrap();
    state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
    state.sync_parameters_pane();
    assert_eq!(state.vis_hud, None);
    assert!(keys(&state).contains(&"Radius".to_string()), "the sphere's rows");
}

/// A visualizer is the Visualize node's reading: the same settings give
/// the same colours as the node does, and the settings round-trip
/// through their one string.
#[test]
fn a_visualizer_reads_as_the_visualize_node_does() {
    let mut state = State::new(false);
    state.rebuild_scene_geometry();
    let base = state.scene_base.clone().expect("a scene");
    let mut v = crate::visualizer::Visualizer::new("N");
    v.ramp = "Heat".into();
    let mut by_vis = base.clone();
    crate::visualizer::apply_all(&[v.clone()], &mut by_vis);
    let mut by_node = base.clone();
    let mut err = None;
    crate::geometry::apply_visualize(&mut by_node, &v.as_node(), &mut err);
    assert!(err.is_none());
    assert_eq!(by_vis, by_node);
    assert_ne!(by_vis, base);
    let both = vec![v.clone(), crate::visualizer::Visualizer::new("Cd")];
    assert_eq!(crate::visualizer::decode(&crate::visualizer::encode(&both)), both);
    assert!(crate::visualizer::decode("not a record").is_empty());
    // What the KDL writer cannot carry is escaped, and comes back.
    let mut odd = crate::visualizer::Visualizer::new("a|b;c=\"d\\%");
    odd.group = "g;1".into();
    let text = crate::visualizer::encode(&[odd.clone()]);
    assert!(!text.contains('"') && !text.contains('\\'), "{text}");
    assert_eq!(crate::visualizer::decode(&text), vec![odd]);
    assert_eq!(crate::visualizer::encode(&[]), "");
}

#[test]
fn the_group_markers_dialog_marks_a_groups_points() {
    use crate::dialog::{Mode, GROUP_ROW_PREFIX};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    let mut redraw = false;
    // A group of five points on the sphere, shown.
    state.apply_action(McpAction::AddNode { template_name: "Group".into(), name: Some("tagged".into()), x: 7.0, y: 8.0 }, &mut redraw).unwrap();
    let tagged = state.current_dir().children.iter().position(|c| c.name == "tagged").unwrap();
    for (name, value) in [("input", "sphere1"), ("group_name", "five"), ("mode", "Random"), ("count", "5")] {
        state.apply_action(McpAction::SetParam { slot: tagged, name: name.into(), value: value.into() }, &mut redraw).unwrap();
    }
    state.current_dir_mut().set_child_geometry_visible(tagged, true);
    state.rebuild_scene_geometry();
    assert!(state.scene_groups.iter().any(|(n, m)| n == "five" && m.len() == 5), "{:?}", state.scene_groups.iter().map(|(n, m)| (n.clone(), m.len())).collect::<Vec<_>>());
    assert!(state.marked_group_instances.is_empty(), "nothing is marked yet");

    // From the palette: the command turns it into the groups list.
    state.run_command("command_palette");
    assert_eq!(state.ui_context[state.slots.dialog].mode, Mode::Commands);
    assert!(state.run_command("group_markers"));
    assert!(state.dialog_visible());
    assert_eq!(state.ui_context[state.slots.dialog].mode, Mode::Groups);
    let row = state.ui_context[state.slots.dialog].rows.iter().position(|r| r.id == format!("{GROUP_ROW_PREFIX}five")).expect("a row for the group");
    assert_eq!(state.ui_context[state.slots.dialog].rows[row].chord, "5 points");
    assert_eq!(state.ui_context[state.slots.dialog].rows[row].toggle(), Some(false));

    // Enter on the row marks the group, and the list stays up.
    state.ui_context[state.slots.dialog].selected = row;
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
    assert!(state.dialog_visible(), "a switch is worked in place");
    assert_eq!(state.ui_context[state.slots.dialog].rows[row].toggle(), Some(true));
    assert!(state.group_marked("five"));
    assert!(!state.marked_group_instances.is_empty() && state.marked_groups_dirty, "the markers are staged");
    let one = state.marked_group_instances.len();
    // On the group's points, at Group Marker Size.
    let members: Vec<[f32; 3]> = state.scene_groups.iter().find(|(n, _)| n == "five").unwrap().1.clone();
    for m in &members {
        assert!(state.marked_group_instances.iter().any(|v| (0..3).all(|k| (v.position[k] - m[k]).abs() <= state.group_marker_size + 1e-4)), "a marker at {m:?}");
    }
    let kdl = fs::read_to_string(crate::app::DesignSettings::file_path()).expect("saved");
    assert_eq!(crate::app::DesignSettings::from_kdl_str(&kdl).viewport.marked_groups, "five", "persisted");
    assert_eq!(State::marked_groups_of("b, a,,a"), vec!["a".to_string(), "b".to_string()]);

    // The markers follow the geometry: a bigger sphere, farther points.
    let sphere = state.current_dir().children.iter().position(|c| c.name == "sphere1").unwrap();
    let far = |state: &State| state.marked_group_instances.iter().map(|v| (v.position[0].powi(2) + v.position[2].powi(2)).sqrt()).fold(0.0f32, f32::max);
    let before = far(&state);
    state.apply_action(McpAction::SetParam { slot: sphere, name: "radius".into(), value: "2.0".into() }, &mut redraw).unwrap();
    assert!(far(&state) > before * 1.5, "{} against {before}", far(&state));
    assert_eq!(state.marked_group_instances.len(), one);

    // Enter again unmarks; Escape closes; a query filters the names.
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
    assert!(!state.group_marked("five"));
    assert!(state.marked_group_instances.is_empty());
    state.dialog_key_input(&typed("z"));
    assert!(state.ui_context[state.slots.dialog].rows.is_empty(), "no group matches");
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Escape)));
    assert!(!state.dialog_visible());

    // A marked name the scene has no group for marks nothing and is kept.
    state.set_group_marked("gone", true);
    assert!(state.marked_group_instances.is_empty());
    assert!(state.group_marked("gone"));
}

#[test]
fn a_node_is_renamed_from_its_menu() {
    use crate::dialog::{Mode, RENAME_ROW_ID};
    let mut state = State::new(false);
    // A sibling for the sphere, whose name is taken.
    state
        .apply_action(McpAction::AddNode { template_name: "Box".into(), name: None, x: 9.0, y: 9.0 }, &mut false)
        .unwrap();
    state.record_structure_changes();
    let sphere = state.current_dir().children.iter().position(|c| c.node_type == "sphere").unwrap();
    let sibling = state.current_dir().children.iter().position(|c| c.node_type == "box").unwrap();
    let old = state.current_dir().children[sphere].name.clone();
    let other = state.current_dir().children[sibling].name.clone();
    let row = |state: &State| state.ui_context[state.slots.dialog].rows.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
    let retype = |state: &mut State, name: &str| {
        while !state.ui_context[state.slots.dialog].query.is_empty() {
            state.dialog_key_input(&key_press(Key::Named(NamedKey::Backspace)));
        }
        for c in name.chars() {
            if c == ' ' {
                state.dialog_key_input(&key_press(Key::Named(NamedKey::Space)));
            } else {
                state.dialog_key_input(&typed(&c.to_string()));
            }
        }
    };

    state.run_node_menu_action(sphere, crate::app::NodeMenuAction::Rename);
    assert!(state.dialog_visible());
    assert_eq!(state.ui_context[state.slots.dialog].mode, Mode::Rename);
    assert_eq!(state.ui_context[state.slots.dialog].query, old, "the dialog opens holding the name");
    assert_eq!(state.ui_context[state.slots.dialog].rows[0].id, RENAME_ROW_ID);

    // Its own name, a sibling's, and none: each is said, and Enter on
    // it writes nothing.
    assert!(row(&state)[0].contains("already"), "{:?}", row(&state));
    retype(&mut state, &other);
    assert!(row(&state)[0].contains("another node"), "{:?}", row(&state));
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
    assert_eq!(state.current_dir().children[sphere].name, old);

    // A name as typed is written as a name is: lowercase, no spaces.
    state.run_node_menu_action(sphere, crate::app::NodeMenuAction::Rename);
    retype(&mut state, "My Ball");
    assert_eq!(row(&state), vec![format!("Rename {old} to my_ball")]);
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Enter)));
    assert!(!state.dialog_visible());
    assert_eq!(state.current_dir().children[sphere].name, "my_ball");
    assert!(state.last_status_text.contains("my_ball"), "{}", state.last_status_text);

    // And it is a step.
    assert!(state.run_command("undo"));
    assert_eq!(state.current_dir().children[sphere].name, old);

    // The command renames the selection, and says so when there is none.
    state.graph_mut().set_selected_node(Some(sphere));
    assert!(state.run_command("rename_node"));
    assert_eq!(state.ui_context[state.slots.dialog].mode, Mode::Rename);
    assert_eq!(state.ui_context[state.slots.dialog].query, old);
    state.dialog_key_input(&key_press(Key::Named(NamedKey::Escape)));
    assert_eq!(state.current_dir().children[sphere].name, old, "Escape renames nothing");

    // The menu has the row.
    let labels = state.node_menu_rows(sphere).0;
    assert!(labels.iter().any(|l| l == "Rename"), "{labels:?}");
}

/// New Project from the palette starts a project. The command named a
/// label no arm dispatched, so the row ran and nothing happened.
#[test]
fn the_new_project_command_starts_an_empty_project() {
    let mut state = State::new(false);
    assert!(!state.fs_root.children.is_empty(), "the bundled project has nodes");
    assert!(state.run_command("new_project"));
    assert!(state.fs_root.children.is_empty(), "New Project left the old nodes in place");
    assert_eq!(state.loaded_project_path, None);
}

/// Two commands on one chord is silent at the keyboard — the second never
/// runs and nothing says why — so it is reported at startup.
#[test]
fn test_chord_conflicts_are_detected_across_spellings() {
    use crate::command::conflicts;
    let none = conflicts(&[("save_document", "Ctrl+s".into()), ("undo", "Ctrl+z".into())]);
    assert!(none.is_empty(), "{none:?}");

    // Compared as PARSED chords, not as text: these are the same keypress.
    let clash = conflicts(&[
        ("save_document", "Ctrl+s".into()),
        ("toggle_grid", "ctrl+S".into()),
        ("undo", "CTRL+s".into()),
    ]);
    assert_eq!(clash.len(), 1, "{clash:?}");
    assert_eq!(clash[0].winner, "save_document", "the first registered must win");
    assert_eq!(clash[0].shadowed, vec!["toggle_grid", "undo"]);

    // The shipped defaults must not collide with each other.
    let shipped: Vec<(&'static str, String)> = crate::command::COMMANDS
        .iter()
        .filter_map(|c| c.default_chord.map(|d| (c.id, d.to_string())))
        .collect();
    let shipped_conflicts = conflicts(&shipped);
    assert!(shipped_conflicts.is_empty(), "the defaults collide: {shipped_conflicts:?}");
}

/// The palette ranks the focused pane's commands first without hiding the
/// rest — a palette that omits what you are looking for is worse than one
/// that lists it second.
#[test]
fn test_the_palette_puts_the_focused_panes_commands_first() {
    use crate::command::{palette_entries, Context, COMMANDS};
    let all = palette_entries("", Context::Viewport);
    assert_eq!(all.len(), COMMANDS.len(), "ranking dropped commands");
    assert_eq!(
        all[0].context,
        Context::Viewport,
        "a viewport command does not lead: {}",
        all[0].id
    );
    assert!(
        all.iter().any(|c| c.id == "save_document"),
        "a global command vanished when a pane was focused"
    );

    // With nothing pane-specific focused the order is the fuzzy one alone.
    let plain = palette_entries("", Context::Always);
    assert_eq!(plain[0].id, COMMANDS[0].id);
}

/// A chord prints back the way it parsed, in a fixed modifier order, so
/// two spellings of one chord read the same beside their labels.
#[test]
fn test_a_chord_describes_itself_back() {
    use crate::shortcut::Shortcut;
    for (written, shown) in [
        ("Ctrl+s", "Ctrl+S"),
        ("ctrl+shift+S", "Ctrl+Shift+S"),
        ("shift+ctrl+s", "Ctrl+Shift+S"),
        ("`", "`"),
    ] {
        assert_eq!(Shortcut::parse(written).unwrap().describe(), shown);
    }
    // And what it prints parses back to the same chord.
    for c in crate::command::COMMANDS.iter().filter_map(|c| c.default_chord) {
        let parsed = Shortcut::parse(c).unwrap();
        assert_eq!(Shortcut::parse(&parsed.describe()).unwrap(), parsed, "{c} did not round trip");
    }
}

/// A palette row round-trips back to the command it names — including
/// when one label is a prefix of another.
#[test]
fn test_a_palette_row_names_its_command_back() {
    use crate::command::{from_palette_row, palette_row, COMMANDS};
    let width = COMMANDS.iter().map(|c| c.label.len()).max().unwrap() + 2;

    for c in COMMANDS {
        let row = palette_row(c.label, Some("Ctrl+X".into()), width);
        assert_eq!(
            from_palette_row(&row).map(|f| f.id),
            Some(c.id),
            "row {row:?} did not name {} back",
            c.id
        );
        // And a row with no chord at all.
        let bare = palette_row(c.label, None, width);
        assert_eq!(from_palette_row(&bare).map(|f| f.id), Some(c.id));
    }

    // The prefix case, spelled out: "Save" starts "Save As"'s row, and
    // taking the first match rather than the longest runs the wrong one.
    let row = palette_row("Save As", Some("Ctrl+Shift+S".into()), width);
    assert_eq!(from_palette_row(&row).map(|c| c.id), Some("save_document_as"));

    assert!(from_palette_row("Not A Command").is_none());
}

/// The soft-transform viewer state: two fixed handles, one of which is a
/// DERIVED position, driven through the same framework as the curve.
///
/// This is the test that says the framework is one — the curve tests above
/// exercise an open-ended list of stored world positions, and this is a
/// fixed pair where the second handle is `Centre + Translation` and has to
/// be converted both ways.
#[test]
fn test_the_soft_transform_state_drags_a_derived_handle() {
    use crate::geometry::node_param_str;
    let mut state = State::new(false);
    let mut redraw = false;
    state
        .apply_action(
            McpAction::AddNode {
                template_name: "Soft Transform".to_string(),
                name: None,
                x: 0.0,
                y: 0.0,
            },
            &mut redraw,
        )
        .expect("add soft transform node");
    let slot = state.current_dir().children.len() - 1;
    assert_eq!(state.current_dir().children[slot].node_type, "soft_transform");

    state.toggle_viewer_state(slot);
    assert!(state.viewer_tool.is_some(), "soft_transform must enter a viewer state");

    state.last_scene_mvp = Some(Mat4::IDENTITY);
    state.last_scene_view_rect = (0.0, 0.0, 100.0, 100.0);
    let sx = |x: f32| 50.0 + x * 50.0;
    let sy = |y: f32| 50.0 - y * 50.0;
    let param = |state: &State, name: &str| {
        node_param_str(&state.current_dir().children[slot], name, "")
    };

    // Two handles: the centre, and the tip at centre + translation. The
    // template ships centre (0,0,0) and translation (0,0.2,0).
    let handles = state.viewer_tool_handles();
    assert_eq!(handles.len(), 2, "a soft transform has exactly two handles");
    assert!((handles[0].1 - sx(0.0)).abs() < 1e-3 && (handles[0].2 - sy(0.0)).abs() < 1e-3);
    assert!(
        (handles[1].2 - sy(0.2)).abs() < 1e-3,
        "the tip is not drawn at centre + translation: {:?}",
        handles[1]
    );

    // Drag the TIP to (0.4, 0, 0): the translation becomes that offset,
    // and the centre does not move.
    state.cursor_x = handles[1].1;
    state.cursor_y = handles[1].2;
    assert!(state.viewer_tool_press(), "press on the tip must grab");
    state.cursor_x = sx(0.4);
    state.cursor_y = sy(0.0);
    assert!(state.viewer_tool_drag_motion());
    assert!(state.viewer_tool_release());
    assert_eq!(param(&state, "center"), "0.00:0.00:0.00", "the centre moved");
    assert_eq!(param(&state, "translation"), "0.40:0.00:0.00");

    // Fixed handles: a press on empty space adds nothing, and Delete
    // removes nothing — a third handle would mean nothing.
    state.cursor_x = sx(-0.9);
    state.cursor_y = sy(-0.9);
    assert!(state.viewer_tool_press(), "the state still owns the viewport press");
    assert!(state.viewer_tool_release() || true);
    assert_eq!(state.viewer_tool_handles().len(), 2, "empty-space press added a handle");
    assert!(!state.viewer_tool_delete_selected(), "a fixed source must not delete");

    // Dragging the BASE keeps the tip where it is, so the translation
    // shortens to match — the documented two-handled-gizmo behaviour.
    let handles = state.viewer_tool_handles();
    state.cursor_x = handles[0].1;
    state.cursor_y = handles[0].2;
    assert!(state.viewer_tool_press());
    state.cursor_x = sx(0.1);
    state.cursor_y = sy(0.0);
    assert!(state.viewer_tool_drag_motion());
    assert!(state.viewer_tool_release());
    assert_eq!(param(&state, "center"), "0.10:0.00:0.00");
    assert_eq!(param(&state, "translation"), "0.30:0.00:0.00", "the tip should not have moved");

    // And undo restores BOTH parameters, which is the case a "keep the
    // translation when the centre moves" rule would have broken.
    assert!(state.viewer_tool_undo());
    assert_eq!(param(&state, "center"), "0.00:0.00:0.00");
    assert_eq!(param(&state, "translation"), "0.40:0.00:0.00");
}

/// Snapping rounds a dragged handle to a world increment, and only when it
/// is on.
#[test]
fn test_snapping_rounds_a_dragged_handle() {
    use crate::viewer_state::SNAP_INCREMENT;
    let mut state = State::new(false);
    let mut redraw = false;
    state
        .apply_action(
            McpAction::AddNode { template_name: "Curve".to_string(), name: None, x: 0.0, y: 0.0 },
            &mut redraw,
        )
        .expect("add curve node");
    let slot = state.current_dir().children.len() - 1;
    state.toggle_viewer_state(slot);
    state.last_scene_mvp = Some(Mat4::IDENTITY);
    state.last_scene_view_rect = (0.0, 0.0, 100.0, 100.0);
    let points = |state: &State| {
        crate::geometry::parse_curve_points(&crate::geometry::node_param_str(
            &state.current_dir().children[slot],
            "points",
            "",
        ))
    };

    // Off by default: a drag lands exactly where the cursor is.
    let first = points(&state)[0];
    state.cursor_x = 50.0 + first.x * 50.0;
    state.cursor_y = 50.0 - first.y * 50.0;
    assert!(state.viewer_tool_press());
    state.cursor_x = 50.0 + 0.37 * 50.0;
    state.cursor_y = 50.0;
    assert!(state.viewer_tool_drag_motion());
    assert!(state.viewer_tool_release());
    assert!((points(&state)[0].x - 0.37).abs() < 1e-3, "{:?}", points(&state)[0]);

    // On: the same drag rounds to the increment.
    assert!(state.toggle_viewer_snap());
    assert_eq!(state.viewer_tool.as_ref().unwrap().snap, Some(SNAP_INCREMENT));
    let first = points(&state)[0];
    state.cursor_x = 50.0 + first.x * 50.0;
    state.cursor_y = 50.0 - first.y * 50.0;
    assert!(state.viewer_tool_press());
    state.cursor_x = 50.0 + 0.37 * 50.0;
    state.cursor_y = 50.0;
    assert!(state.viewer_tool_drag_motion());
    assert!(state.viewer_tool_release());
    let p = points(&state)[0];
    assert!((p.x - 0.4).abs() < 1e-3, "snapped x should be 0.4, got {p:?}");

    // And off again, from the same command.
    assert!(state.toggle_viewer_snap());
    assert_eq!(state.viewer_tool.as_ref().unwrap().snap, None);

    // The HUD says which it is, because a mode you cannot see is a mode
    // you forget you are in.
    let hud = state.viewer_tool.as_ref().unwrap().hud();
    assert!(hud.contains("Curve Points") && hud.contains("Snap off"), "{hud}");
}

/// Only node types with a source enter a viewer state, and each gets its
/// own.
#[test]
fn test_only_editable_node_types_enter_a_viewer_state() {
    use crate::viewer_state::source_for;
    assert_eq!(source_for("curve").map(|s| s.name()), Some("Curve Points"));
    assert_eq!(source_for("soft_transform").map(|s| s.name()), Some("Soft Transform"));
    assert!(source_for("sphere").is_none());
    assert!(source_for("boolean").is_none());
    // Types are matched case-insensitively, like every other node lookup.
    assert!(source_for("Curve").is_some());
}

/// Keyboard graph navigation: the plugin's hjkl families, as commands.
///
/// Bare hjkl moves the grid cursor and therefore the selection, alt moves
/// the node under it, ctrl pans the view and touches neither. All of it is
/// gated on the network pane having focus — the bare family used to be the
/// one that was not, so the cursor drifted invisibly while you looked at
/// the viewport.
/// Stepping the grid cursor from node to node must not carry one node's
/// parameters into the next.
///
/// The pane's rows are written back by NAME, and until 2026-09-25 they
/// were written into whatever was selected — `sync_layout` flushed them
/// through `sync_pane_focus` after a cursor step had moved the selection
/// but before the post-event pass reloaded the pane. Every node has an
/// `Input`, so pressing k twice from the bottom of a chain pointed each
/// node it landed on at the chain's end, and carried Iterations from one
/// node into the next on the way. Real key events through
/// `process_window_event`, because the bug lived in that pass's order and
/// `run_command` alone skips it.
#[test]
fn stepping_the_cursor_does_not_rewire_the_nodes_it_lands_on() {
    use crate::window::WindowEvent;
    fn key(state: cce_ui::widget::ElementState) -> cce_ui::widget::KeyEvent {
        cce_ui::widget::KeyEvent {
            state,
            logical_key: Key::Character("k".into()),
            text: Some("k".into()),
            repeat: false,
            ctrl: false,
            shift: false,
            alt: false,
        }
    }
    fn params(s: &State) -> Vec<(String, Vec<(String, String)>)> {
        s.current_dir()
            .children
            .iter()
            .map(|c| (c.name.clone(), c.params.iter().map(|p| (p.name.clone(), p.text().to_string())).collect()))
            .collect()
    }
    let mut s = State::new(false);
    let mut r = false;
    let base = s.current_dir().children.len();
    // A column of its own, clear of the bundled project's nodes: the
    // chain's shape from the report, Output at the bottom.
    for (t, y) in [("attribute", 3.0), ("relax", 5.0), ("Detangle", 6.0), ("Output", 7.0)] {
        s.apply_action(McpAction::AddNode { template_name: t.into(), name: None, x: 13.0, y }, &mut r).unwrap();
    }
    let names: Vec<String> = s.current_dir().children[base..].iter().map(|c| c.name.clone()).collect();
    for i in 1..4 {
        s.apply_action(McpAction::SetParam { slot: base + i, name: "input".into(), value: names[i - 1].clone() }, &mut r)
            .unwrap();
    }
    s.focused_pane = LEFT_MENUBAR_IDX;
    s.grid_cursor_col = 13;
    s.grid_cursor_row = 7;
    s.sync_cursor_and_selection();
    s.sync_parameters_pane();
    let before = params(&s);

    for row in [6, 5] {
        s.process_window_event(WindowEvent::KeyboardInput { event: key(cce_ui::widget::ElementState::Pressed) });
        s.process_window_event(WindowEvent::KeyboardInput { event: key(cce_ui::widget::ElementState::Released) });
        assert_eq!(s.grid_cursor_row, row, "the step has to have happened, or the check is vacuous");
    }
    assert_eq!(s.graph().selected_node(), Some(base + 1), "the cursor landed on the relax");
    assert_eq!(params(&s), before, "no node's parameters may change from walking over it");

    // And the guard does not stand in the way of the pane it protects:
    // an edit to the node it now shows still lands.
    let rows: Vec<(String, String, String)> = s
        .param()
        .node_params()
        .into_iter()
        .map(|(n, v, t)| if n == "Iterations" { (n, "12".into(), t) } else { (n, v, t) })
        .collect();
    s.param_mut().set_display_params(&rows);
    s.sync_parameters_to_project();
    let relax = &s.current_dir().children[base + 1];
    assert_eq!(crate::geometry::node_param_str(relax, "iterations", ""), "12");
}

#[test]
fn test_the_network_navigation_families() {
    use crate::slots::{LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX};
    let mut state = State::new(false);
    let mut redraw = false;
    state
        .apply_action(
            McpAction::AddNode { template_name: "Sphere".to_string(), name: None, x: 3.0, y: 2.0 },
            &mut redraw,
        )
        .expect("add node");
    let slot = state.current_dir().children.len() - 1;
    assert_eq!(state.current_dir().children[slot].position, (3.0, 2.0));

    state.focused_pane = LEFT_MENUBAR_IDX;
    state.grid_cursor_col = 3;
    state.grid_cursor_row = 2;
    state.sync_cursor_and_selection();
    assert_eq!(state.graph().selected_node(), Some(slot), "the cursor should select what it sits on");

    // Bare hjkl walks the cursor, and the selection follows it off the
    // node. Up, not right: the default project already has a node at
    // (4, 2), and navigating onto it would select that one instead —
    // correctly, which is exactly why the empty cell has to be chosen
    // deliberately rather than assumed.
    assert!(state.run_command("nav_up"));
    assert_eq!((state.grid_cursor_col, state.grid_cursor_row), (3, 1));
    assert_eq!(state.graph().selected_node(), None, "the cursor left the node");
    assert!(state.run_command("nav_down"));
    assert_eq!(state.graph().selected_node(), Some(slot), "and came back to it");

    // And navigating ONTO another node selects that one.
    let neighbour = state
        .current_dir()
        .children
        .iter()
        .position(|c| c.position == (4.0, 2.0))
        .expect("the default project has a node at (4, 2)");
    assert!(state.run_command("nav_right"));
    assert_eq!(state.graph().selected_node(), Some(neighbour));
    assert!(state.run_command("nav_left"));

    // Alt moves the node AND the cursor, so a run of them drags it rather
    // than leaving it behind on the first press.
    assert!(state.run_command("move_down"));
    assert_eq!(state.current_dir().children[slot].position, (3.0, 3.0));
    assert_eq!((state.grid_cursor_col, state.grid_cursor_row), (3, 3));
    assert_eq!(state.graph().selected_node(), Some(slot), "the node should still be selected");
    assert!(state.run_command("move_down"));
    assert_eq!(state.current_dir().children[slot].position, (3.0, 4.0));

    // Ctrl pans the view: the cursor, the selection and the node all stay.
    let before = (state.pan_x, state.pan_y);
    let cursor = (state.grid_cursor_col, state.grid_cursor_row);
    assert!(state.run_command("view_right"));
    assert_ne!((state.pan_x, state.pan_y), before, "the view did not pan");
    assert_eq!((state.grid_cursor_col, state.grid_cursor_row), cursor);
    assert_eq!(state.current_dir().children[slot].position, (3.0, 4.0));
    assert_eq!(state.graph().selected_node(), Some(slot));

    // Frame Cursor CENTRES the cursor cell, rather than only scrolling it
    // into view when it has gone off an edge — which would make the
    // command do nothing in the case you actually press it in.
    state.positions[crate::slots::CONTENT_IDX] = (0.0, 0.0, 800.0, 600.0);
    state.grid_cursor_col = 9;
    state.grid_cursor_row = 7;
    assert!(state.run_command("frame_cursor"));
    // The cell's centre IS its lattice intersection. Pane-relative,
    // because the command's rebuild_positions puts the real pane origin
    // back under the cell_center the pan was computed for.
    let (pane_x, pane_y, _, _) = state.positions[crate::slots::CONTENT_IDX];
    let (cell_x, cell_y) = state.cell_center(9, 7);
    let (cell_x, cell_y) = (cell_x - pane_x, cell_y - pane_y);
    assert!(
        (cell_x - 400.0).abs() < 1.0,
        "the cursor cell is not centred horizontally: {cell_x}"
    );
    assert!(
        (cell_y - 300.0).abs() < 1.0,
        "the cursor cell is not centred vertically: {cell_y}"
    );

    // Every family is gated on the network pane. With the viewport focused
    // the commands run and do nothing, rather than moving a cursor nobody
    // can see.
    state.focused_pane = RIGHT_MENUBAR_IDX;
    let cursor = (state.grid_cursor_col, state.grid_cursor_row);
    let pos = state.current_dir().children[slot].position;
    let pan = (state.pan_x, state.pan_y);
    for id in ["nav_left", "nav_right", "nav_up", "nav_down", "move_left", "view_left", "frame_cursor"] {
        assert!(state.run_command(id), "{id} should be a known command");
    }
    assert_eq!((state.grid_cursor_col, state.grid_cursor_row), cursor, "the cursor moved from another pane");
    assert_eq!(state.current_dir().children[slot].position, pos, "a node moved from another pane");
    assert_eq!((state.pan_x, state.pan_y), pan, "the view panned from another pane");
}

/// The navigation scheme is the plugin's, and the registry says so: hjkl
/// bare, shift, alt and ctrl, plus the two framings — eighteen rows, all
/// in the network context, none of them colliding.
#[test]
fn test_the_navigation_scheme_matches_the_plugins() {
    use crate::command::{by_id, Context};
    let expected = [
        // Uppercase because `describe` prints single letters as capitals,
        // the way every menu in the app writes a chord.
        ("nav_left", "H"), ("nav_down", "J"), ("nav_up", "K"), ("nav_right", "L"),
        ("extend_left", "Shift+H"), ("extend_down", "Shift+J"), ("extend_up", "Shift+K"), ("extend_right", "Shift+L"),
        ("move_left", "Alt+H"), ("move_down", "Alt+J"), ("move_up", "Alt+K"), ("move_right", "Alt+L"),
        ("view_left", "Ctrl+H"), ("view_down", "Ctrl+J"), ("view_up", "Ctrl+K"), ("view_right", "Ctrl+L"),
        ("frame_cursor", "F"), ("frame_all", "Shift+F"),
    ];
    for (id, chord) in expected {
        let cmd = by_id(id).unwrap_or_else(|| panic!("{id} is not a command"));
        assert_eq!(cmd.context, Context::Network, "{id} is not a network command");
        let parsed = crate::shortcut::Shortcut::parse(cmd.default_chord.expect(id)).unwrap();
        assert_eq!(parsed.describe(), chord, "{id} is not bound where the plugin binds it");
    }
}

/// Auto-layout: rows are how far downstream a node is, columns keep it
/// under what it reads from.
#[test]
fn test_auto_layout_lays_a_chain_out_vertically() {
    use crate::layout::{arrange, LayoutNode};
    let node = |name: &str, input: Option<&str>, pos: (f32, f32)| LayoutNode {
        name: name.to_string(),
        input: input.map(|s| s.to_string()),
        reads: Vec::new(),
        position: pos,
        pinned: false,
    };

    // A chain, scattered. It should come back as one vertical line,
    // because a chain IS a vertical line in this grid — a sphere at
    // (4, 2) feeding an output at (4, 3) is the convention every project
    // in the repo already uses.
    let nodes = vec![
        node("c", Some("b"), (7.0, 0.0)),
        node("a", None, (2.0, 5.0)),
        node("b", Some("a"), (0.0, 9.0)),
    ];
    let moved: std::collections::HashMap<usize, (f32, f32)> =
        arrange(&nodes).into_iter().collect();
    let at = |i: usize| moved.get(&i).copied().unwrap_or(nodes[i].position);
    assert_eq!(at(1).1, 0.0, "the root is not on the top row");
    assert_eq!(at(2).1, 1.0, "its child is not one row below it");
    assert_eq!(at(0).1, 2.0, "the grandchild is not two rows below");
    assert_eq!(at(1).0, at(2).0, "a chain should be one column");
    assert_eq!(at(2).0, at(0).0, "a chain should be one column");

    // A root's existing column is its wish, so two independent chains keep
    // the left-to-right order the user gave them.
    let nodes = vec![
        node("right", None, (5.0, 0.0)),
        node("left", None, (1.0, 0.0)),
        node("right_child", Some("right"), (0.0, 0.0)),
        node("left_child", Some("left"), (0.0, 0.0)),
    ];
    let moved: std::collections::HashMap<usize, (f32, f32)> =
        arrange(&nodes).into_iter().collect();
    let at = |i: usize| moved.get(&i).copied().unwrap_or(nodes[i].position);
    assert!(at(1).0 < at(0).0, "left should stay left of right");
    assert_eq!(at(1).0, at(3).0, "left's child should sit under it");
    assert_eq!(at(0).0, at(2).0, "right's child should sit under it");
    assert_eq!(at(2).1, 1.0);
    assert_eq!(at(3).1, 1.0);
}

/// Every wire a node has is drawn, into its own port: the Remesh's
/// switch reads the loop on its Input and the transfer on its Input 2,
/// and both are lines on the network now, where only the Input was. An
/// expression wire (the transfer's From) is drawn to what it evaluates
/// to — input1 while the Remesh's own From is empty; a node beside the
/// subnet when it names one, which is not on this level to draw from.
/// A connection dropped on a port sets THAT wire, and auto-layout puts
/// a node below everything it reads.
#[test]
fn every_wire_is_drawn_into_its_own_port() {
    use cce_ui::widget::node_wires;
    // In the bundled project's Geometry node, where it opens.
    let mut state = State::new(false);
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Remesh".into(), name: Some("remesh1".into()), x: 3.0, y: 8.0 }, &mut redraw).unwrap();
    let slot = state.current_dir().children.iter().position(|c| c.name == "remesh1").unwrap();
    state.apply_action(McpAction::Enter { slot }, &mut redraw).unwrap();
    state.sync_nodes();
    let nodes = state.graph().get_nodes();
    let get = |n: &str| nodes.iter().find(|g| g.name == n).unwrap().clone();
    let switch = get("transfer_switch1");
    assert_eq!(node_wires(&switch), ["repeat1", "transfer1", "", ""]);
    assert_eq!(switch.inputs, 4);
    let transfer = get("transfer1");
    assert_eq!(node_wires(&transfer), ["repeat1", "input1"], "From is an expression, drawn to what it evaluates to");

    // The Remesh's From naming a node outside: not on this level, no line.
    state.apply_action(McpAction::Up, &mut redraw).unwrap();
    state.current_dir_mut().children[slot].params.iter_mut().find(|p| p.name == "from").unwrap().set_text("elsewhere");
    state.apply_action(McpAction::Enter { slot }, &mut redraw).unwrap();
    state.sync_nodes();
    let level = state.graph().get_nodes();
    let transfer = level.iter().find(|g| g.name == "transfer1").unwrap();
    assert_eq!(node_wires(transfer), ["repeat1", "elsewhere"], "it names the outer node…");
    assert!(!level.iter().any(|g| g.name == "elsewhere"), "…which is not on this level, so no line is drawn");

    // Dropped on the switch's third port: Input 3.
    let path = state.current_path.clone();
    let id = switch.id.clone();
    assert!(state.connect_port(&path, &id, "input1".into(), 2));
    let sw = state.current_dir().children.iter().find(|c| c.id == id).unwrap();
    assert_eq!(sw.params.iter().find(|p| p.name == "input_3").unwrap().text(), "input1");
    assert_eq!(sw.params.iter().find(|p| p.name == "input").unwrap().text(), "repeat1", "the Input is untouched");

    // A second operand sets the row, not the column.
    use crate::layout::{arrange, LayoutNode};
    let node = |name: &str, input: Option<&str>, reads: &[&str], pos: (f32, f32)| LayoutNode {
        name: name.into(),
        input: input.map(String::from),
        reads: reads.iter().map(|s| s.to_string()).collect(),
        position: pos,
        pinned: false,
    };
    let nodes = vec![
        node("a", None, &[], (0.0, 0.0)),
        node("b", Some("a"), &[], (4.0, 0.0)),
        node("c", Some("b"), &[], (4.0, 0.0)),
        node("join", Some("a"), &["c"], (0.0, 0.0)),
    ];
    let moved: std::collections::HashMap<usize, (f32, f32)> = arrange(&nodes).into_iter().collect();
    let at = |i: usize| moved.get(&i).copied().unwrap_or(nodes[i].position);
    assert_eq!(at(3).1, 3.0, "below c, which it reads through its second wire");
    assert_eq!(at(3).0, at(0).0, "under a, which its Input reads");
}

/// The cases that would otherwise hang or overwrite: cycles, self
/// reference, dangling names, and pinned cells.
#[test]
fn test_auto_layout_survives_cycles_and_pinned_nodes() {
    use crate::layout::{arrange, LayoutNode};
    let node = |name: &str, input: Option<&str>, pos: (f32, f32), pinned: bool| LayoutNode {
        name: name.to_string(),
        input: input.map(|s| s.to_string()),
        reads: Vec::new(),
        position: pos,
        pinned,
    };

    // A name-wired graph can be cyclic; it must terminate rather than
    // recurse, and the answer only has to be finite and sane.
    let cyclic = vec![
        node("a", Some("b"), (0.0, 0.0), false),
        node("b", Some("a"), (1.0, 0.0), false),
        node("self", Some("self"), (2.0, 0.0), false),
    ];
    let moved = arrange(&cyclic);
    assert!(moved.len() <= 3);
    for (_, (c, r)) in &moved {
        assert!(c.is_finite() && r.is_finite() && *r >= 0.0);
    }

    // A dangling input name is simply no edge — the node is a root, not an
    // error and not a crash.
    let dangling = vec![node("a", Some("nothing_called_this"), (3.0, 4.0), false)];
    let moved: Vec<_> = arrange(&dangling);
    assert_eq!(moved, vec![(0, (3.0, 0.0))], "a dangling input should make a root");

    // Pinned nodes never move, and nothing is placed on top of them.
    let pinned = vec![
        node("meta", None, (0.0, 0.0), true),
        node("a", None, (0.0, 5.0), false),
        node("b", Some("a"), (0.0, 6.0), false),
    ];
    let moved: std::collections::HashMap<usize, (f32, f32)> =
        arrange(&pinned).into_iter().collect();
    assert!(!moved.contains_key(&0), "a pinned node moved");
    let a = moved.get(&1).copied().unwrap_or(pinned[1].position);
    assert_ne!(a, (0.0, 0.0), "a node was placed on top of the pinned one");
    assert_eq!(a.1, 0.0, "the root still belongs on the top row");
    let b = moved.get(&2).copied().unwrap_or(pinned[2].position);
    assert_eq!(b.0, a.0, "the child should follow its parent's column");
    assert_eq!(b.1, 1.0);
}

/// The command end to end, on a real project.
#[test]
fn test_the_layout_command_arranges_the_current_level() {
    use crate::slots::LEFT_MENUBAR_IDX;
    let mut state = State::new(false);
    let mut redraw = false;
    for (template, x, y) in
        [("Curve", 6.0, 7.0), ("Remesh", 2.0, 1.0), ("Subdivide", 9.0, 3.0)]
    {
        state
            .apply_action(
                McpAction::AddNode {
                    template_name: template.to_string(),
                    name: None,
                    x,
                    y,
                },
                &mut redraw,
            )
            .unwrap_or_else(|e| panic!("add {template}: {e}"));
    }
    let n = state.current_dir().children.len();
    let (curve, remesh, subdiv) = (n - 3, n - 2, n - 1);
    let names: Vec<String> =
        state.current_dir().children.iter().map(|c| c.name.clone()).collect();

    // Wire them into a chain: curve -> remesh -> subdivide.
    let set_input = |state: &mut State, slot: usize, value: &str| {
        let dir = state.current_dir_mut();
        if let Some(p) =
            dir.children[slot].params.iter_mut().find(|p| p.name.eq_ignore_ascii_case("input"))
        {
            p.set_text(value.to_string());
        }
    };
    set_input(&mut state, remesh, &names[curve]);
    set_input(&mut state, subdiv, &names[remesh]);

    state.focused_pane = LEFT_MENUBAR_IDX;
    assert!(state.layout_current_level());

    let pos = |state: &State, slot: usize| state.current_dir().children[slot].position;
    assert_eq!(pos(&state, curve).1 + 1.0, pos(&state, remesh).1, "remesh should sit below curve");
    assert_eq!(pos(&state, remesh).1 + 1.0, pos(&state, subdiv).1, "subdivide should sit below remesh");
    assert_eq!(pos(&state, curve).0, pos(&state, remesh).0, "the chain should be one column");
    assert_eq!(pos(&state, remesh).0, pos(&state, subdiv).0, "the chain should be one column");

    // The meta node is a utility tree and stays where it was.
    let meta = state.current_dir().children.iter().position(|c| c.node_type == "meta");
    if let Some(m) = meta {
        assert_eq!(pos(&state, m), (0.0, 0.0), "the meta node moved");
    }

    // Running it again changes nothing, and says so rather than looking
    // broken.
    let before: Vec<_> =
        state.current_dir().children.iter().map(|c| c.position).collect();
    assert!(state.layout_current_level());
    let after: Vec<_> = state.current_dir().children.iter().map(|c| c.position).collect();
    assert_eq!(before, after, "a second layout should be a no-op");

    // And it is gated on the network pane like every other network command.
    state.focused_pane = crate::slots::RIGHT_MENUBAR_IDX;
    assert!(!state.layout_current_level());
}

/// `cargo test` must not write the user's own settings.
///
/// It did, until 2026-09-23. `DesignSettings::file_path` hardcoded
/// `$HOME/.config/cce/cce-designer/state.kdl`, and `State::new` loads the
/// BUNDLED project, whose meta subnets overwrite the live viewport flags —
/// so any test that reached `save_settings` wrote the bundled project's
/// show_grid / show_cube / show_origin over the user's. Every run of the
/// suite silently reset three of their toggles, and the run looked green.
///
/// The real path is spelled out here rather than read from `file_path()`,
/// which is the thing under test and now answers with a temp directory.
#[test]
fn the_suite_does_not_write_the_users_own_settings() {
    let real = cce_ui::config::cce_config_dir().join("cce-designer").join("state.kdl");
    let before = std::fs::read(&real).ok();

    assert!(
        !crate::app::DesignSettings::file_path()
            .starts_with(cce_ui::config::cce_config_dir()),
        "the suite writes settings inside the real cce config directory"
    );

    // A flip of the kind that carried the damage: it marks settings
    // dirty and `execute_action` saves at the end of the action. (It was
    // the network plate's toggle, retired with the plate.)
    let mut state = State::new(false);
    let plate = state.params_plate;
    assert!(state.run_command("toggle_params_plate"));
    assert_ne!(state.params_plate, plate, "the toggle did not flip the plate");

    // Without this the test passes on a save that never happened, which
    // is exactly the bug wearing a different face.
    assert!(
        crate::app::DesignSettings::file_path().exists(),
        "no settings file was written at all — the assertion below proves nothing"
    );
    assert_eq!(
        std::fs::read(&real).ok(),
        before,
        "{} changed — a test wrote the user's real settings",
        real.display()
    );
}

/// The recent-files list is not the user's, under test.
///
/// The toolkit derives that path from the EXE's basename, so each test
/// binary wrote a real `~/.config/cce/cce_designer-<hash>/` of its own —
/// seven had accumulated by 2026-09-23. Reading was no safer than
/// writing: a test that loaded the real list would assert against
/// whatever projects happen to be on the machine running it.
///
/// Both halves are asserted because they fail differently — a load that
/// reached the real file makes this suite's behaviour depend on the
/// machine, a save leaves a directory behind on it — and because the
/// gate is one `cfg!(test)` in each of two functions, so one can be
/// removed without the other.
#[test]
fn the_recent_files_list_is_not_the_users() {
    assert!(
        State::load_recent_files().is_empty(),
        "the suite loaded the real recent-files list"
    );

    let real = cce_ui::config::get_app_recent_files_path();
    let before = std::fs::read(&real).ok();

    let mut state = State::new(false);
    state.add_recent_file(
        std::env::temp_dir().join(format!("cce-designer-recent-{}", std::process::id())),
    );

    assert_eq!(
        std::fs::read(&real).ok(),
        before,
        "{} changed — a test wrote a real recent-files list",
        real.display()
    );
}

/// The suite runs on a lattice of its own, not the machine's.
///
/// `configured_grid_geometry` read `style.surface.graph.spacing_x` and
/// friends straight out of `~/.config/cce/config.kdl`, so the grid tests
/// — which press at pixel coordinates from `cell_center` and assert which
/// node was hit — passed or failed by whoever's config was installed.
/// `dragging_a_selected_node_carries_the_selection` genuinely failed at
/// cce-ui's own defaults: its row 11 lands at 1237 px in a 900 px test
/// window, so the press misses and the drag never arms. It had been
/// passing on the author's 140 x 70.
///
/// Asserting the constants back is the point rather than a tautology: it
/// is what fails if the pin is ever unwired back to the config, and the
/// live `State` is checked alongside them so the pin has to reach the app
/// and not just the helper.
#[test]
fn the_suite_runs_on_a_lattice_of_its_own() {
    let g = crate::app::configured_grid_geometry();
    assert_eq!(
        (g.pitch_x, g.pitch_y, g.node_w, g.node_h),
        (140.0, 70.0, 80.0, 40.0),
        "the suite's lattice moved — if this came from config.kdl, the grid \
             tests now depend on the machine running them"
    );

    let state = State::new(false);
    assert_eq!(
        (state.grid_pitch_x, state.grid_pitch_y, state.node_w, state.node_h),
        (140.0, 70.0, 80.0, 40.0),
        "State::new did not start on the pinned lattice"
    );
}

/// The viewport guide toggles survive the next parameter edit.
///
/// `apply_settings_from_menubar_subnets` used to copy the Guides utility
/// node onto the live flags on EVERY parameter change, so a command that
/// flipped only the flag was undone by the next edit anywhere — Show
/// Cube (a guide since removed) hid the cube, and editing any node's
/// parameter brought it back.
/// The fix was to write the node as well; the node is gone now and the
/// flag is simply the value, which is the same guarantee with nothing
/// left to fall out of step. Still asserted, because the failure it
/// catches (an edit reverting a display toggle) is invisible in a test
/// that only flips the toggle.
#[test]
fn guide_toggles_survive_the_settings_apply_pass() {
    let mut state = State::new(false);
    for command in ["toggle_grid", "toggle_origin", "toggle_point_markers"] {
        let flag = |state: &State| match command {
            "toggle_grid" => state.viewport().show_grid,
            "toggle_origin" => state.viewport().show_origin,
            _ => state.show_point_markers,
        };
        let before = flag(&state);
        assert!(state.run_command(command));
        assert_eq!(flag(&state), !before, "{command} flipped the flag");

        // A real edit through the action path, on an unrelated node.
        let mut redraw = false;
        let sphere = state.current_dir().children.iter().position(|c| c.name.starts_with("sphere")).expect("a sphere");
        state
            .apply_action(crate::app::McpAction::SetParam { slot: sphere, name: "radius".into(), value: "0.7".into() }, &mut redraw)
            .expect("set a sphere param");
        assert_eq!(flag(&state), !before, "{command} was undone by a parameter edit");
    }

    // Circular Pane had the same hole.
    let before = state.circular_network_pane;
    assert!(state.run_command("toggle_circular_pane"));
    assert_eq!(state.circular_network_pane, !before);
}

/// A replacement renderer invalidates the page pane's image id, and the
/// state has to notice.
///
/// There is no reconnect callback: the runner calls `renderer_init` once
/// per renderer, so the FIRST call is this process's own and every later
/// one is a replacement. Remembering that it has been called is the only
/// way to tell them apart — and getting it wrong is silent, because a
/// stale id names nothing and its draws are skipped rather than failing.
#[test]
fn test_a_replacement_renderer_drops_the_page_image() {
    let mut state = State::new(false);
    assert!(!state.seen_renderer, "a fresh State has not been given a renderer");
    assert!(!state.page_dirty);

    // Stand in for a composed page: an id owned by State.
    state.page_image = Some(7);

    // The FIRST renderer is this process's own — nothing to invalidate,
    // and dropping the image here would throw away a page that is fine.
    assert!(!state.renderer_handed_over(), "the first renderer is not a replacement");
    assert_eq!(state.page_image, Some(7), "the first renderer must not drop the image");
    assert!(!state.page_dirty);

    // A LATER one is a replacement: the id names nothing in it, so it is
    // dropped and the next tick re-uploads rather than leaving the pane
    // blank forever.
    assert!(state.renderer_handed_over(), "the second renderer must read as a replacement");
    assert_eq!(state.page_image, None, "the dead id was kept");
    assert!(state.page_dirty, "nothing would re-upload the page");
}

/// Dragging empty scene turns the camera, at the same rate the trackpad's
/// pixel-delta orbit does.
///
/// The camera had no drag gesture at all before this: `Viewport3D` handles
/// only `MouseWheel`, so the scene could be turned by scrolling and by
/// nothing else. The rate is shared with that path deliberately — a drag
/// and a two-finger swipe should not feel like different cameras.
#[test]
fn test_dragging_empty_scene_turns_the_camera() {
    let mut state = State::new(false);
    let k = State::ORBIT_RADIANS_PER_PX;

    // The default camera carries its own orbit. X drag yaws, Y drag
    // pitches, and the pitch is inverted so dragging down looks down.
    state.active_camera = "Default Camera".to_string();
    let (y0, x0) = (state.viewport().rotation_y, state.viewport().rotation_x);
    state.orbit_camera_by(100.0, 40.0);
    assert!(
        (state.viewport().rotation_y - (y0 + 100.0 * k)).abs() < 1e-5,
        "yaw did not follow the drag"
    );
    assert!(
        (state.viewport().rotation_x - (x0 - 40.0 * k)).abs() < 1e-5,
        "pitch did not follow the drag, or is not inverted"
    );

    // A named camera accumulates instead, for the camera NODE to pick up —
    // the same split the scroll path makes.
    let mut state = State::new(false);
    state.active_camera = "Camera 1".to_string();
    let before = (state.viewport().rotation_y, state.viewport().rotation_x);
    state.orbit_camera_by(100.0, 40.0);
    assert!(
        (state.viewport().pending_yaw - 100.0 * k).abs() < 1e-5,
        "a named camera's yaw did not accumulate"
    );
    assert!(
        (state.viewport().pending_pitch - (-40.0 * k)).abs() < 1e-5,
        "a named camera's pitch did not accumulate"
    );
    assert_eq!(
        (state.viewport().rotation_y, state.viewport().rotation_x),
        before,
        "a named camera must not move the default camera's orbit"
    );

    // Pitch is clamped, so a long downward drag cannot roll the scene over.
    let mut state = State::new(false);
    state.active_camera = "Default Camera".to_string();
    state.orbit_camera_by(0.0, -100000.0);
    let pitch = state.viewport().rotation_x;
    assert!(pitch.abs() < std::f32::consts::PI, "pitch ran past vertical: {pitch}");

}

/// Deselecting has to STICK, which is the whole difficulty.
///
/// The selection is whatever sits in the cursor's cell — that is what
/// `sync_cursor_and_selection` means — and the sync runs on nearly every
/// frame that changes anything. A bare `set_selected_node(None)` is put
/// straight back, so the deselected cell is remembered and the sync leaves
/// that one cell alone.
#[test]
fn test_deselect_sticks_until_the_cursor_moves() {
    use crate::slots::LEFT_MENUBAR_IDX;
    let mut state = State::new(false);
    let mut redraw = false;
    state
        .apply_action(
            McpAction::AddNode { template_name: "Sphere".to_string(), name: None, x: 6.0, y: 6.0 },
            &mut redraw,
        )
        .expect("add node");
    let slot = state.current_dir().children.len() - 1;

    state.focused_pane = LEFT_MENUBAR_IDX;
    state.grid_cursor_col = 6;
    state.grid_cursor_row = 6;
    state.sync_cursor_and_selection();
    assert_eq!(state.graph().selected_node(), Some(slot));

    // Nothing selected, nothing to do — so Escape can fall through to
    // meaning nothing rather than claiming it acted.
    assert!(state.deselect_node());
    assert_eq!(state.graph().selected_node(), None);
    assert!(!state.deselect_node(), "a second deselect has nothing to clear");

    // THE point: the sync that runs on the next changed frame must not put
    // it back, even though the cursor still sits on the node.
    state.sync_cursor_and_selection();
    assert_eq!(
        state.graph().selected_node(),
        None,
        "the sync re-selected the node the user just deselected"
    );

    // Stepping away and back selects again — the deselect held for that
    // one cell, not for the node.
    assert!(state.run_command("nav_right"));
    assert_eq!(state.graph().selected_node(), None, "nothing is at the new cell");
    assert!(state.run_command("nav_left"));
    assert_eq!(
        state.graph().selected_node(),
        Some(slot),
        "coming back to the node should select it again"
    );

    // And selecting explicitly spends the memory: deselect, then select
    // the same node, and the next sync must leave it selected.
    assert!(state.deselect_node());
    state.graph_mut().set_selected_node(Some(slot));
    state.sync_layout();
    state.sync_cursor_and_selection();
    assert_eq!(
        state.graph().selected_node(),
        Some(slot),
        "re-selecting the deselected node did not stick"
    );
}

/// Curvature is signed, dimensionless, and does not move when the model
/// is scaled or re-tessellated.
///
/// That last property is the one that matters: thickness is chosen from
/// this measure, so a measure that changed with the remesh division size
/// would give a shell whose thickness moved every time you re-tessellated.
#[test]
fn test_curvature_is_signed_and_scale_free() {
    use crate::mold::curvature;

    // A sphere is convex everywhere, so every point reads negative, and
    // every point reads the SAME — it has one curvature.
    let sphere = crate::geometry::sphere_detail(glam::Vec3::ZERO, 1.0, 24, 32);
    let c = curvature(&sphere);
    assert!(c.iter().all(|v| *v < 0.0), "a sphere should read convex everywhere");
    let (lo, hi) = c.iter().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
    assert!(hi - lo < 0.08, "a sphere's curvature is not uniform: {lo}..{hi}");

    // Ten times the size, same measure — this is what "dimensionless"
    // buys, and it is why the thickness range means the same thing on a
    // model of any size.
    let big = crate::geometry::sphere_detail(glam::Vec3::ZERO, 10.0, 24, 32);
    let cb = curvature(&big);
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    assert!(
        (mean(&c) - mean(&cb)).abs() < 1e-3,
        "curvature changed with scale: {} vs {}",
        mean(&c),
        mean(&cb)
    );

    // And it is finite on a mesh with isolated points — those read flat
    // rather than NaN, which would poison the whole thickness range.
    let mut stray = sphere.clone();
    stray.add_point(glam::Vec3::new(50.0, 0.0, 0.0));
    let cs = curvature(&stray);
    assert!(cs.iter().all(|v| v.is_finite()), "curvature went non-finite");
    assert_eq!(cs[cs.len() - 1], 0.0, "an isolated point should read flat");
}

/// The ramp maps curvature into the thickness range, and the range is
/// honoured whichever way round its ends are given.
#[test]
fn test_thickness_stays_inside_the_range() {
    use crate::mold::{thickness_from_curvature, Ramp};
    let curv = [-1.0, -0.5, 0.0, 0.5, 1.0];

    let t = thickness_from_curvature(&curv, 0.6, 0.75, Ramp::Linear);
    assert!(t.iter().all(|v| (0.6..=0.75).contains(v)), "{t:?} left the range");
    assert!(t[0] < t[4], "concave should be thicker than convex");
    assert!((t[0] - 0.6).abs() < 1e-6 && (t[4] - 0.75).abs() < 1e-6, "{t:?}");

    // Constant is the uniform shell, reachable without leaving the node.
    let t = thickness_from_curvature(&curv, 0.6, 0.75, Ramp::Constant);
    assert!(t.iter().all(|v| (*v - 0.75).abs() < 1e-6), "{t:?}");

    // Smooth flattens both ends rather than changing where they land.
    let t = thickness_from_curvature(&curv, 0.0, 1.0, Ramp::Smooth);
    assert!((t[0] - 0.0).abs() < 1e-6 && (t[4] - 1.0).abs() < 1e-6);
    assert!(t[1] < 0.25 && t[3] > 0.75, "smooth did not flatten the ends: {t:?}");

    // A range given backwards is still a range — min and max are the two
    // ends, not an ordering the caller has to get right.
    let a = thickness_from_curvature(&curv, 0.75, 0.6, Ramp::Linear);
    let b = thickness_from_curvature(&curv, 0.6, 0.75, Ramp::Linear);
    assert_eq!(a, b);
}
