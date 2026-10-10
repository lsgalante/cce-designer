//! The collision test (src/collide.rs): CPU and GPU, one algorithm.

use super::*;

/// A collider and a cloud of queries around and through it.
fn collision_fixture(rows: usize, cols: usize, queries: usize) -> (Vec<Vec3>, Vec<[Vec3; 3]>) {
    let collider = crate::geometry::sphere_detail(Vec3::new(0.1, 0.55, -0.05), 0.7, rows, cols);
    let tris: Vec<[Vec3; 3]> = collider
        .triangulate(|pos, _| Vec3::from(pos))
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    let pts: Vec<Vec3> = (0..queries)
        .map(|i| {
            let f = i as f32;
            Vec3::new((f * 0.371).sin() * 1.1 + 0.1, (f * 0.173).cos() * 1.1 + 0.55, (f * 0.529).sin() * 1.1 - 0.05)
        })
        .collect();
    (pts, tris)
}

/// The CPU test is the node's original loop, step for step: inside is
/// containment, proximity is a band, both against a sphere whose
/// geometry is known.
#[test]
fn collision_cpu_tests_containment_and_a_surface_band() {
    use crate::collide::{hits_cpu, Test};
    let (pts, tris) = collision_fixture(16, 24, 2000);
    let centre = Vec3::new(0.1, 0.55, -0.05);
    let inside = hits_cpu(&pts, &tris, Test::Inside);
    let band = hits_cpu(&pts, &tris, Test::Proximity(0.05));
    let (mut n_in, mut n_band) = (0, 0);
    for (i, p) in pts.iter().enumerate() {
        let r = (*p - centre).length();
        if r < 0.7 - 0.02 {
            assert_eq!(inside[i], 1, "point {i} at r={r} is enclosed");
        } else if r > 0.7 + 0.02 {
            assert_eq!(inside[i], 0, "point {i} at r={r} is outside");
        }
        if (r - 0.7).abs() < 0.05 - 0.02 {
            assert_eq!(band[i], 1, "point {i} at r={r} is within the band");
        } else if (r - 0.7).abs() > 0.05 + 0.02 {
            assert_eq!(band[i], 0, "point {i} at r={r} is outside the band");
        }
        n_in += inside[i];
        n_band += band[i];
    }
    assert!(n_in > 0 && n_in < pts.len() as u32 && n_band > 0, "the fixture straddles the collider: {n_in} in, {n_band} in band");
}

/// The cross-check: the GPU's flags are the CPU's, for both tests. A
/// query on a knife edge of the threshold may round either way, so a
/// disagreement is tolerated only there. Skips where there is no Vulkan.
#[test]
fn collision_gpu_matches_cpu() {
    use crate::collide::{hits_cpu, hits_gpu, Test};
    let (pts, tris) = collision_fixture(24, 36, 6000);
    for test in [Test::Inside, Test::Proximity(0.05)] {
        let cpu = hits_cpu(&pts, &tris, test);
        let gpu = match crate::gpu::with_any_device(|dev| hits_gpu(dev, &pts, &tris, test)) {
            Err(e) => {
                println!("skipping collision_gpu_matches_cpu: {e}");
                return;
            }
            Ok(r) => r.expect("the collision kernel runs"),
        };
        assert_eq!(cpu.len(), gpu.len());
        let mut disagreements = 0;
        for i in 0..cpu.len() {
            if cpu[i] != gpu[i] {
                let d = tris
                    .iter()
                    .map(|t| crate::geometry::point_triangle_distance_sq(pts[i], t[0], t[1], t[2]).sqrt())
                    .fold(f32::INFINITY, f32::min);
                let margin = match test {
                    Test::Proximity(r) => (d - r).abs(),
                    Test::Inside => d,
                };
                assert!(margin < 1e-4, "{test:?}: query {i} differs (cpu {} gpu {}) with margin {margin}", cpu[i], gpu[i]);
                disagreements += 1;
            }
        }
        println!("collision {test:?}: {} queries x {} triangles, {disagreements} knife-edge disagreements", pts.len(), tris.len());
        assert!(cpu.iter().any(|&h| h == 1) && cpu.iter().any(|&h| h == 0));
    }
}

/// Where the auto threshold sits: the test timed over sizes. Ignored,
/// a measurement: `cargo test --release -p cce-designer collision_timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn collision_timing() {
    use crate::collide::{hits_cpu, hits_gpu, Test};
    for (rows, cols, queries) in [(16, 24, 500), (16, 24, 5000), (32, 48, 5000), (64, 96, 20000)] {
        let (pts, tris) = collision_fixture(rows, cols, queries);
        let t = std::time::Instant::now();
        let cpu = hits_cpu(&pts, &tris, Test::Proximity(0.05));
        let cpu_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = std::time::Instant::now();
        let gpu = crate::gpu::with_any_device(|dev| hits_gpu(dev, &pts, &tris, Test::Proximity(0.05)));
        let gpu_ms = t.elapsed().as_secs_f64() * 1e3;
        let same = gpu.as_ref().map(|g| g.as_ref().map(|g| *g == cpu).unwrap_or(false)).unwrap_or(false);
        println!(
            "{:>6} queries x {:>6} tris = {:>10} pairs: cpu {cpu_ms:9.2} ms   gpu {gpu_ms:8.2} ms   same={same}",
            pts.len(),
            tris.len(),
            pts.len() * tris.len()
        );
    }
}

/// A wrangle's error names its node and its line; the params pane is
/// told the line only when that node is the one it shows.
#[test]
fn a_script_error_line_reaches_the_params_pane_for_the_shown_node() {
    use crate::app::State;
    assert_eq!(State::error_line_number("syntax: Syntax error: Expecting ';' (line 3, position 5)"), Some(2));
    assert_eq!(State::error_line_number("point 4: index out of range (line 1, position 9)"), Some(0));
    assert_eq!(State::error_line_number("OpenCL nodes are retired"), None);
    assert_eq!(State::error_line_number("(line 0, position 1)"), None, "a zero line is not a line");

    let mut state = State::new(false);
    let mut redraw = false;
    state.apply_action(crate::app::McpAction::AddNode { template_name: "Wrangle".into(), name: Some("w".into()), x: 3.0, y: 9.0 }, &mut redraw).unwrap();
    let slot = state.current_dir().children.iter().position(|c| c.name == "w").unwrap();
    state.apply_action(crate::app::McpAction::Select { slot }, &mut redraw).unwrap();
    assert_eq!(state.param_editor_selected(), Some(slot));
    assert_eq!(state.code_error_line_for_pane("w: syntax: Syntax error (line 2, position 1)"), Some(1));
    assert_eq!(state.code_error_line_for_pane("sphere1: something (line 2, position 1)"), None, "another node's error is not this pane's");
    assert_eq!(state.code_error_line_for_pane("w: OpenCL nodes are retired"), None, "no line, no flag");
}

/// A trackpad swipe over a spinbox row of the params pane steps the row
/// — 60 px of finger travel per step, the widget's notch — and the
/// node's value follows through the wheel's write-back. Until
/// 2026-09-28 the pane claimed every finger gesture for its own scroll,
/// so on a trackpad the -/+ buttons were the only pointer way to step a
/// Rows or Columns value; a mouse notch stepped it all along.
#[test]
fn a_trackpad_swipe_over_a_spinbox_row_steps_it() {
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::{scroll_motion::set_scroll_phase, MouseScrollDelta, Position, ScrollPhase};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = LEFT_MENUBAR_IDX;
    let sphere = state.current_dir().children.iter().position(|c| c.name == "sphere1").expect("sphere1");
    state.graph_mut().set_selected_node(Some(sphere));
    state.sync_parameters_pane();
    state.rebuild_positions();
    state.apply_layout();
    let rows = crate::app::param_display(&state.current_dir().children[sphere].params);
    let i = rows.iter().position(|r| r.0 == "Rows").expect("a Rows row");
    assert!(rows[i].2.starts_with("spinbox"), "{:?}", rows[i]);
    let (x, y, w, h) = state.param_row_rects()[i];
    let value = |state: &State| crate::geometry::node_param_f32(&state.current_dir().children[sphere], "rows", -1.0);
    let before = value(&state);

    state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: (x + w * 0.3) as f64, y: (y + h * 0.5) as f64 } });
    // The designer's test binary links cce-ui without cfg(test), so the
    // natural-scroll setting would be the MACHINE's: pinned here, both
    // ways in turn.
    cce_ui::input::force_natural_scroll(Some(false));
    set_scroll_phase(ScrollPhase::Finger);
    state.ui_context.scroll_gesture_new = true;
    state.ui_context.scroll_initiate_widget_id = None;
    assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 0.0, y: 60.0 }) }));
    set_scroll_phase(ScrollPhase::Wheel);
    assert_eq!(value(&state), before + 1.0, "one notch of finger travel is one step, written to the node");

    // A mouse notch on the same row, as before.
    assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, -1.0) }));
    assert_eq!(value(&state), before);

    // With natural scrolling on, the fingers going UP is more: the
    // same travel, the other sign of delta.
    cce_ui::input::force_natural_scroll(Some(true));
    set_scroll_phase(ScrollPhase::Finger);
    state.ui_context.scroll_gesture_new = true;
    state.ui_context.scroll_initiate_widget_id = None;
    assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 0.0, y: -60.0 }) }));
    set_scroll_phase(ScrollPhase::Wheel);
    assert_eq!(value(&state), before + 1.0, "natural: fingers up is a step up");
    // And a wheel notch up is still more.
    assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, 1.0) }));
    assert_eq!(value(&state), before + 2.0);
    cce_ui::input::force_natural_scroll(None);
}

/// During playback the scene is built for the frame the playbar shows,
/// within the same tick. The Playbar's tick advances the frame, so the
/// frame-change check has to run after the widget ticks; before
/// 2026-09-28 it ran first, and every tick rebuilt the scene for the
/// previous tick's frame and then advanced the readout.
#[test]
fn playback_builds_the_scene_for_the_frame_the_playbar_shows() {
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
    assert!(crate::geometry::contains_simnet(&state.fs_root));
    state.tick_frame(1.0 / 60.0);
    assert_eq!(state.last_sim_frame, state.sim_frame());
    state.ui_context[state.slots.playbar].inner_mut().playing = true;
    state.ui_context[state.slots.playbar].inner_mut().fps = 60.0;
    let start = state.sim_frame();
    for i in 1..=3 {
        assert!(state.tick_frame(1.0 / 60.0), "a playing tick asks for a redraw");
        assert_eq!(state.sim_frame(), start + i, "the playbar advanced a frame");
        assert_eq!(state.last_sim_frame, state.sim_frame(), "and the scene was built for that frame, not the last one");
    }
}

/// The edge list is built by bucket, not by one sort of every edge, and
/// without the rest of the topology for the wire pass: the same edges in
/// the same order either way — on meshes of triangles, quads and
/// polygons, open segments, a primitive that names a point twice, and a
/// hand-built one naming a point past the end (which takes the sort).
#[test]
fn unique_edges_match_a_sort_of_every_edge() {
    let reference = |d: &crate::detail::Detail| -> Vec<[u32; 2]> {
        let mut all = Vec::new();
        for prim in 0..d.num_prims() {
            let pts = d.prim_points(prim);
            let n = pts.len();
            if n < 2 {
                continue;
            }
            let span = if n == 2 { 1 } else { n };
            for i in 0..span {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                if a != b {
                    all.push([a.min(b), a.max(b)]);
                }
            }
        }
        all.sort_unstable();
        all.dedup();
        all
    };
    let mut meshes = vec![
        crate::geometry::sphere_detail(Vec3::ZERO, 1.0, 9, 14),
        crate::geometry::box_detail(Vec3::ZERO, Vec3::ONE, 0.2),
    ];
    // A scramble of triangles, quads, pentagons, segments and a
    // degenerate polygon over 60 points, numbered out of order.
    let mut d = crate::detail::Detail::new();
    for i in 0..60 {
        d.add_point(Vec3::new(i as f32, (i * 7 % 11) as f32, 0.0));
    }
    let mut k = 17u32;
    for prim in 0..120 {
        let n = [2, 3, 3, 4, 5][prim % 5];
        let pts: Vec<u32> = (0..n).map(|_| { k = (k * 31 + 7) % 60; k }).collect();
        d.add_prim(&pts);
    }
    d.add_prim(&[5, 5, 9, 5]);
    meshes.push(d);
    let mut past = crate::detail::Detail::new();
    for i in 0..5 {
        past.add_point(Vec3::new(i as f32, 0.0, 0.0));
    }
    past.add_prim(&[0, 1, 2]);
    past.add_prim(&[3, 99]);
    meshes.push(past);
    for d in &meshes {
        let want = reference(d);
        assert_eq!(d.edge_list().into_owned(), want, "built alone");
        assert!(matches!(d.edge_list(), std::borrow::Cow::Owned(_)), "without building the topology");
        assert_eq!(d.edges(), want.as_slice(), "built with the topology");
        assert!(matches!(d.edge_list(), std::borrow::Cow::Borrowed(_)), "and taken from it once built");
    }
}

/// The wire pass's vertices are written a piece of edges a thread, into
/// one buffer: what one thread writes, vertex for vertex, on a mesh too
/// small to share out and on one large enough to.
#[test]
fn the_wire_vertices_are_the_edges_in_order() {
    for (lat, lon) in [(6, 9), (120, 240)] {
        let mut g = crate::geometry::sphere_detail(Vec3::new(0.2, -0.1, 0.3), 1.0, lat, lon);
        let n = g.num_points();
        g.points_mut().insert(crate::detail::CD, crate::detail::AttribData::Float3((0..n).map(|p| [p as f32 / n as f32, 0.5, 0.1]).collect())).unwrap();
        let want: Vec<[f32; 6]> = g
            .edges()
            .iter()
            .flat_map(|e| e.iter().map(|&p| { let (a, c) = (g.pos(p as usize).to_array(), g.color(p as usize)); [a[0], a[1], a[2], c[0], c[1], c[2]] }).collect::<Vec<_>>())
            .collect();
        let got: Vec<[f32; 6]> = crate::render::scene_edge_verts(&g)
            .iter()
            .map(|v| [v.position[0], v.position[1], v.position[2], v.color[0], v.color[1], v.color[2]])
            .collect();
        assert_eq!(got, want, "{} edges", g.edges().len());
    }
}

/// A copy of a Detail shares its topology, built once; an edit that
/// moves points or writes attributes keeps it, a structural edit drops
/// it; a Detail merged into an empty one keeps it; and a simulation's
/// solved frame comes out of the cache with it built, so the scene's
/// edges cost nothing on a replay.
#[test]
fn a_copy_shares_the_topology_until_it_is_edited() {
    let mut g = crate::geometry::sphere_detail(Vec3::ZERO, 1.0, 5, 8);
    assert!(!g.has_topology());
    let edges = g.edges().to_vec();
    let mut copy = g.clone();
    assert!(copy.has_topology(), "shared by the copy");
    copy.positions_mut()[0][0] += 1.0;
    copy.points_mut().create("mass", crate::detail::AttribValue::Float(2.0));
    assert!(copy.has_topology(), "moving points and writing attributes keep it");
    assert_eq!(copy.edges(), edges.as_slice());
    let mut merged = crate::detail::Detail::new();
    merged.merge(&g);
    assert!(merged.has_topology(), "merged into nothing keeps it");
    assert_eq!(merged.edges(), edges.as_slice());
    merged.merge(&g);
    assert!(!merged.has_topology(), "merged into something does not");
    copy.add_prim(&[0, 1, 2]);
    assert!(!copy.has_topology(), "a structural edit drops it");
    assert_ne!(copy.edges().len(), 0);
    assert!(g.has_topology(), "and the original's is its own");

    // A simnet's solved frame, taken back out of the cache, has it.
    let mut state = State::new(false);
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
    let slot = state.current_dir().children.iter().position(|c| c.name == "sim").unwrap();
    state.apply_action(McpAction::SetParam { slot, name: "input".into(), value: "sphere1".into() }, &mut redraw).unwrap();
    let simnet = state.current_dir().children[slot].clone();
    let mut cache = crate::geometry::SimCache::default();
    for frame in [4, 4, 2] {
        let mut sim = crate::geometry::EvalSim::new(frame, 1, &mut cache);
        let solved = crate::geometry::resolve_simnet_geometry_with_errors(&state.fs_root, &simnet, &mut Vec::new(), &mut None, &mut sim).unwrap();
        assert!(solved.has_topology(), "frame {frame} comes with its topology");
    }
}

/// The spreadsheet's plate is as wide as its table — the columns, each
/// as wide as its content — along the left, never narrower than an
/// empty table's, and no wider than the window less a gap each side,
/// where the table scrolls. A refill that changes the table's width
/// lays the plate out again, and leaves the selection alone.
#[test]
fn the_spreadsheet_plate_is_as_wide_as_its_table() {
    use crate::slots::SPREADSHEET_IDX;
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    if !state.show_spreadsheet {
        state.execute_menu_action("Show Spreadsheet Pane");
    }
    let mut redraw = false;
    let slot = state.current_dir().children.iter().position(|c| c.name == "sphere1").unwrap();
    state.apply_action(McpAction::Select { slot }, &mut redraw).unwrap();
    state.sync_nodes();
    let table = state.slots.spreadsheet(&state.ui_context).content_width();
    let (x, _, w, _) = state.positions[SPREADSHEET_IDX];
    assert!(table > State::SPREADSHEET_MIN_W && table < 1600.0 - 36.0, "a sphere's table: {table}");
    assert_eq!((x, w), (18.0, table.ceil()), "the plate is the table's width, on the left");
    assert_eq!(cce_ui::widget::WidgetHost::rect(&state.ui_context[state.slots.spreadsheet]).2, w, "and the widget has it");
    assert_eq!(state.param_editor_selected(), Some(slot), "the selection is as it was");

    state.spreadsheet_mut().set_spreadsheet_data(vec![], vec![]);
    state.rebuild_positions();
    assert_eq!(state.positions[SPREADSHEET_IDX].2, State::SPREADSHEET_MIN_W, "an empty table");
    let headers: Vec<String> = (0..60).map(|i| format!("column{i}")).collect();
    state.spreadsheet_mut().set_spreadsheet_data(headers, vec![]);
    state.rebuild_positions();
    assert_eq!(state.positions[SPREADSHEET_IDX].2, 1600.0 - 36.0, "a table wider than the window fills its room");
}

/// `merge_owned` takes a detail's arrays where `merge` copies them,
/// and the result is `merge`'s: every point, identity (renumbered),
/// attribute, kind and group, the detail attributes left behind, and
/// the topology carried when built — into an empty detail, and into one
/// that is not, where it is `merge` itself.
#[test]
fn merge_owned_is_merge() {
    use crate::detail::{AttribData, AttribKind, AttribValue, Detail};
    let mut other = crate::geometry::sphere_detail(Vec3::new(0.3, 0.1, -0.2), 0.8, 6, 9);
    let n = other.num_points();
    other.points_mut().insert("mass", AttribData::Float((0..n).map(|p| p as f32 * 0.5).collect())).unwrap();
    other.points_mut().create_kind("vel", AttribValue::Float3([0.0, 1.0, 0.0]), AttribKind::Derivative);
    other.points_mut().create_group("tip");
    other.points_mut().add_to_group("tip", 3);
    other.prims_mut().create("pid", AttribValue::Int(7));
    other.detail_mut().create("note", AttribValue::Float(9.0));
    for built in [false, true] {
        if built {
            other.edges();
        }
        let mut copied = Detail::new();
        copied.merge(&other);
        let mut moved = Detail::new();
        moved.merge_owned(other.clone());
        assert!(moved == copied, "into an empty detail (topology built: {built})");
        assert_eq!(moved.has_topology(), copied.has_topology());
        assert_eq!((0..moved.num_points()).map(|p| moved.id(p)).collect::<Vec<_>>(), (0..copied.num_points()).map(|p| copied.id(p)).collect::<Vec<_>>());

        let mut into_copy = crate::geometry::sphere_detail(Vec3::ZERO, 1.0, 3, 4);
        let mut into_move = into_copy.clone();
        into_copy.merge(&other);
        into_move.merge_owned(other.clone());
        assert!(into_move == into_copy, "into a detail that is not empty");
    }
}

/// The fill's vertices are `triangulate`'s, vertex for vertex, written
/// a stretch of primitives a thread on a mesh large enough to share
/// out and on one thread below that — with polygons, segments and a
/// primitive naming a point past the end among them; and the vector
/// markers are what one thread writes, in order.
#[test]
fn detail_vertices_are_the_triangulation() {
    use crate::geometry::{detail_vertices, Vertex3D};
    let bits = |v: &[Vertex3D]| v.iter().map(|v| (v.position.map(f32::to_bits), v.color.map(f32::to_bits))).collect::<Vec<_>>();
    let mut meshes = vec![crate::geometry::sphere_detail(Vec3::ZERO, 1.0, 5, 7), crate::geometry::sphere_detail(Vec3::new(0.1, 0.2, 0.3), 2.0, 140, 280)];
    let mut odd = crate::geometry::sphere_detail(Vec3::ZERO, 1.0, 4, 6);
    odd.add_prim(&[0, 1]);
    odd.add_prim(&[2, 3, 4, 5, 6]);
    odd.add_prim(&[1, 2, 999]);
    meshes.push(odd);
    for d in &mut meshes {
        let n = d.num_points();
        d.points_mut().insert(crate::detail::CD, crate::detail::AttribData::Float3((0..n).map(|p| [p as f32 / n as f32, 0.25, 0.5]).collect())).unwrap();
        let want = d.triangulate(|position, color| Vertex3D { position, color });
        assert_eq!(bits(&detail_vertices(d)), bits(&want), "{} prims", d.num_prims());

        d.points_mut().create(&format!("{}dir", crate::detail::VIS_PREFIX), crate::detail::AttribValue::Float3([0.0; 3]));
        let name = format!("{}dir", crate::detail::VIS_PREFIX);
        for p in (0..n).step_by(3) {
            d.points_mut().set_value(&name, p, crate::detail::AttribValue::Float3([0.1, p as f32 * 0.001, -0.2])).unwrap();
        }
        let lin = cce_ui::colors::to_linear_rgb;
        let mut want = Vec::new();
        for p in 0..n {
            let dir = d.points().value(&name, p).unwrap().as_vec3();
            if dir.length_squared() < 1e-12 {
                continue;
            }
            let color = lin(d.color(p));
            want.push(Vertex3D { position: d.positions()[p], color });
            want.push(Vertex3D { position: (d.pos(p) + dir).to_array(), color });
        }
        assert_eq!(bits(&crate::geometry::vis_marker_vertices(d, lin)), bits(&want));
    }
}

/// The playbar's cache strip, as a rule: a frame is cached when every
/// simnet in the tree holds it, stale when one of them holds it from
/// the chain as it was — before an edit the solve went on across, or
/// anywhere in a solve whose simnet was edited since — and in no run
/// otherwise. A solve of a simnet no longer in the tree counts for
/// nothing.
#[test]
fn the_playbar_cache_runs_say_what_is_cached_and_what_is_stale() {
    use crate::geometry::SolvedRange;
    use crate::playbar::{CacheRun, CacheState::*};
    let run = |from, to, state| CacheRun { from, to, state };
    let solve = |id: &str, start, reach, stale_to| SolvedRange { id: id.into(), start, reach, stale_to, chain: 7 };
    let chains = |ids: &[&str]| ids.iter().map(|i| (i.to_string(), 7u64)).collect::<std::collections::HashMap<_, _>>();
    let runs = crate::app::playbar_cache_runs;

    assert_eq!(runs(&[solve("a", 1, 20, None)], &chains(&["a"]), 1, 100), vec![run(1, 20, Cached)]);
    assert_eq!(runs(&[solve("a", 1, 30, Some(20))], &chains(&["a"]), 1, 100), vec![run(1, 1, Cached), run(2, 20, Stale), run(21, 30, Cached)]);
    // Edited since it was solved: stale up to where it reached; the seed is the seed.
    let mut edited = chains(&["a"]);
    edited.insert("a".into(), 8);
    assert_eq!(runs(&[solve("a", 1, 30, None)], &edited, 1, 100), vec![run(1, 1, Cached), run(2, 30, Stale)]);
    // Two simnets: a frame is held when both hold it, and before a
    // simnet's start it holds its seed.
    assert_eq!(
        runs(&[solve("a", 1, 30, None), solve("b", 10, 20, Some(15))], &chains(&["a", "b"]), 1, 100),
        vec![run(1, 10, Cached), run(11, 15, Stale), run(16, 20, Cached)]
    );
    // Deleted: not in the tree, nothing to show; and the range clips.
    assert!(runs(&[solve("gone", 1, 30, None)], &chains(&["a"]), 1, 100).is_empty());
    assert_eq!(runs(&[solve("a", 1, 30, None)], &chains(&["a"]), 5, 12), vec![run(5, 12, Cached)]);
}

/// The playbar shows what a simnet's solve holds as it is played, edited
/// and scrubbed: the frames played are cached; an edit at frame 20 makes
/// 2–20 stale (the solve goes on from the frame in hand); playing on
/// caches the frames after it; and a scrub back clears the stale frames,
/// the solve beginning again from the seed.
#[test]
fn the_playbar_shows_the_cached_and_the_stale_frames() {
    use crate::playbar::{CacheRun, CacheState::*};
    let run = |from, to, state| CacheRun { from, to, state };
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
    let slot = state.current_dir().children.iter().position(|c| c.name == "sim").unwrap();
    if !state.current_dir().children[slot].geometry_visible {
        state.apply_action(McpAction::ToggleGeometry { slot }, &mut redraw).unwrap();
    }
    let go = |state: &mut State, frame: f32| {
        state.ui_context[state.slots.playbar].inner_mut().current_frame = frame;
        state.tick_frame(1.0 / 60.0);
    };
    let strip = |state: &State| state.ui_context[state.slots.playbar].inner().cache.clone();
    for f in 1..=20 {
        go(&mut state, f as f32);
    }
    assert_eq!(strip(&state), vec![run(1, 20, Cached)]);

    state.apply_action(McpAction::SetParam { slot, name: "substeps".into(), value: "2".into() }, &mut redraw).unwrap();
    state.tick_frame(1.0 / 60.0);
    assert_eq!(strip(&state), vec![run(1, 1, Cached), run(2, 20, Stale)], "the frames solved before the edit are stale");

    for f in 21..=25 {
        go(&mut state, f as f32);
    }
    assert_eq!(strip(&state), vec![run(1, 1, Cached), run(2, 20, Stale), run(21, 25, Cached)]);

    go(&mut state, 10.0);
    assert_eq!(strip(&state), vec![run(1, 10, Cached)], "a scrub back solves again from the seed");
    // A scrub back in a clean solve keeps the frame it left.
    go(&mut state, 5.0);
    assert_eq!(strip(&state), vec![run(1, 10, Cached)]);
}

/// Repeat off: playback stops ON the last frame instead of wrapping,
/// in either direction, and a play press on a timeline stopped at its
/// far end restarts from the near one. The setting is a palette toggle
/// persisted top-level in state.kdl, read back by a fresh State.
#[test]
fn repeat_off_stops_playback_at_the_end_and_persists() {
    use cce_ui::widget::Input;
    let rect = cce_ui::scene::layout::Rect { x: 0.0, y: 0.0, width: 100.0, height: 30.0 };
    let mut state = State::new(false);
    assert_eq!(state.command_toggle_state("toggle_playbar_repeat"), Some(true), "repeat is on by default");
    assert!(state.run_command("toggle_playbar_repeat"));
    assert_eq!(state.command_toggle_state("toggle_playbar_repeat"), Some(false));
    // Persisted: the toggle wrote state.kdl, and a fresh State reads it.
    // Checked at once, since the suite's tests share the (redirected)
    // file and another's save could follow.
    let kdl = fs::read_to_string(DesignSettings::file_path()).expect("state.kdl was written");
    assert!(!DesignSettings::from_kdl_str(&kdl).playbar_repeat, "{kdl}");
    let fresh = State::new(false);
    assert!(!fresh.ui_context[fresh.slots.playbar].inner().repeat, "a new State seeds the playbar from the setting");

    // Forward: run off the end, land on it, stop.
    {
        let pb = state.ui_context[state.slots.playbar].inner_mut();
        pb.current_frame = pb.end_frame - 0.5;
        pb.begin(false);
    }
    assert!(Input::tick(state.ui_context[state.slots.playbar].inner_mut(), 0.1, rect));
    {
        let pb = state.ui_context[state.slots.playbar].inner();
        assert_eq!(pb.current_frame, pb.end_frame, "stopped on the last frame");
        assert!(!pb.playing, "and playback ended");
    }
    // Play again from the end: restarts from the start frame.
    state.execute_action(Action::PlayPause);
    {
        let pb = state.ui_context[state.slots.playbar].inner();
        assert!(pb.playing && !pb.reversed);
        assert_eq!(pb.current_frame, pb.start_frame, "a play press at the far end rewinds");
    }
    // Reverse: run off the start, stop there; Down restarts from the end.
    {
        let pb = state.ui_context[state.slots.playbar].inner_mut();
        pb.playing = false;
        pb.current_frame = pb.start_frame + 0.5;
        pb.begin(true);
    }
    assert!(Input::tick(state.ui_context[state.slots.playbar].inner_mut(), 0.1, rect));
    {
        let pb = state.ui_context[state.slots.playbar].inner();
        assert_eq!(pb.current_frame, pb.start_frame);
        assert!(!pb.playing);
    }
    state.execute_action(Action::PlayPauseReverse);
    {
        let pb = state.ui_context[state.slots.playbar].inner();
        assert!(pb.playing && pb.reversed);
        assert_eq!(pb.current_frame, pb.end_frame);
    }

    // Back on, and the file follows.
    assert!(state.run_command("toggle_playbar_repeat"));
    assert!(DesignSettings::from_kdl_str(&fs::read_to_string(DesignSettings::file_path()).unwrap()).playbar_repeat);
}

/// The Value row's span: the smallest power of ten (at least one)
/// whose middle half holds the value; kept while the value stays
/// between a twentieth and nineteen twentieths of it.
#[test]
fn the_value_rows_span_adapts_to_the_value() {
    use crate::app::value_row_span as span;
    assert_eq!(span(&[0.0], None), 1.0);
    assert_eq!(span(&[0.06, 0.0, 0.0], None), 1.0);
    assert_eq!(span(&[0.5], None), 1.0);
    assert_eq!(span(&[0.6], None), 10.0);
    assert_eq!(span(&[1.0], None), 10.0);
    assert_eq!(span(&[-7.0, 2.0], None), 100.0, "the largest magnitude, either sign");
    assert_eq!(span(&[300.0], None), 1000.0);
    assert_eq!(span(&[3.0], Some(10.0)), 10.0, "inside: kept");
    assert_eq!(span(&[9.0], Some(10.0)), 10.0);
    assert_eq!(span(&[9.6], Some(10.0)), 100.0, "at the end: grows");
    assert_eq!(span(&[6.0], Some(100.0)), 100.0, "a twentieth or more: kept");
    assert_eq!(span(&[0.6], Some(100.0)), 10.0, "far inside: shrinks");
    assert_eq!(span(&[0.4], Some(100.0)), 1.0);
    assert_eq!(span(&[0.0], Some(1.0)), 1.0, "one is the floor");
}

/// The Attribute node's Value stays a text parameter, but the pane
/// presents it as a control as wide as its target — over the wide
/// span around its value (`value_row_span`) — a slider for one, the float group with two, three
/// or four rows for more: Modify on Pos (the pull node), on an input
/// Float3 (N) or Float2 (uv), Create by its Type. A single number is
/// spread over the components, as the node spreads it, and the pane
/// writing it back unchanged is not an edit. A text that fits no width,
/// an attribute the input lacks and an expression keep the text box;
/// read from an attribute, there is no Value row at all.
#[test]
fn an_attribute_value_row_is_a_control_as_wide_as_its_target() {
    let templates_root = crate::app::load_fs_tree();
    let find = |name: &str| templates_root.children.iter().find(|t| t.name == name).unwrap();
    let instance = |template: &FsNode, id: &str, name: &str, params: &[(&str, &str)]| {
        let mut inst = template.clone();
        inst.id = id.to_string();
        inst.name = name.to_string();
        for (pname, val) in params {
            inst.params.iter_mut().find(|p| p.name == *pname).unwrap().set_text(val.to_string());
        }
        inst
    };
    let mut state = State::new(false);
    // Inside the bundled project's Geometry node, where geometry goes.
    state.current_dir_mut().children = vec![
        instance(find("Sphere"), "s", "Sphere 1", &[]),
        instance(find("Attribute"), "a", "pull1", &[
            ("input", "Sphere 1"),
            ("operation", "Modify"),
            ("attribute_name", "Pos"),
            ("value", "0.00:0.06:0.00"),
            ("combine", "Add"),
        ]),
    ];
    state.sync_nodes();
    state.graph_mut().set_selected_node(Some(1));
    let value_row = |state: &mut State| {
        state.sync_parameters_pane();
        state.param_mut().node_params().iter().find(|r| r.0 == "Value").expect("a Value row").2.clone()
    };
    // A vector gets the trackball beside its sliders by default.
    // 0.06 sits in the middle half of ±1.
    let wide = "float3:-1:1:trackball:soft".to_string();
    assert_eq!(value_row(&mut state), wide, "Modify on Pos");

    let set = |state: &mut State, name: &str, val: &str| {
        state.current_dir_mut().children[1].params.iter_mut().find(|p| p.name == name).unwrap().set_text(val.to_string());
    };
    set(&mut state, "attribute_name", "N");
    assert_eq!(value_row(&mut state), wide, "Modify on an input Float3");
    set(&mut state, "attribute_name", "uv");
    assert_eq!(value_row(&mut state), "text", "three numbers do not fit an input Float2");
    set(&mut state, "value", "1:2");
    assert_eq!(value_row(&mut state), "float2:-10:10:soft", "Modify on an input Float2: 2 needs ±10");
    set(&mut state, "value", "0.00:0.06:0.00");
    set(&mut state, "attribute_name", "nothing_here");
    assert_eq!(value_row(&mut state), "text", "an attribute the input lacks has no width");

    set(&mut state, "operation", "Create");
    set(&mut state, "attribute_name", "vel");
    set(&mut state, "type", "Float3");
    assert_eq!(value_row(&mut state), wide, "Create of a Float3");
    set(&mut state, "type", "Float");
    assert_eq!(value_row(&mut state), "text", "three numbers do not fit a Float");
    let value_text = |state: &mut State| {
        state.sync_parameters_pane();
        state.param_mut().node_params().iter().find(|r| r.0 == "Value").unwrap().1.clone()
    };
    set(&mut state, "value", "1.00");
    assert_eq!(value_row(&mut state), "slider:-10:10:2:soft", "Create of a Float");
    for (ty, row, shown) in [
        ("Float2", "float2:-10:10:soft".to_string(), "1.00:1.00"),
        ("Float3", "float3:-10:10:trackball:soft".to_string(), "1.00:1.00:1.00"),
        ("Float4", "float4:-10:10:soft".to_string(), "1.00:1.00:1.00:1.00"),
    ] {
        set(&mut state, "type", ty);
        assert_eq!(value_row(&mut state), row, "Create of a {ty}");
        assert_eq!(value_text(&mut state), shown, "one number spread over a {ty}");
    }
    // Read back unchanged, the spread number is not an edit.
    let steps = state.edit_history.undo_len();
    state.sync_parameters_to_project();
    assert_eq!(state.current_dir().children[1].params.iter().find(|p| p.name == "value").unwrap().text(), "1.00");
    assert_eq!(state.edit_history.undo_len(), steps);

    set(&mut state, "type", "Float3");
    set(&mut state, "value", "1.00:2.00:3.00");
    assert_eq!(value_row(&mut state), "float3:-10:10:trackball:soft", "3 stays inside the ±10 in use");
    // Past nineteen twentieths of it, the row re-scales: 9.6 to ±100.
    set(&mut state, "value", "9.6:0:0");
    assert_eq!(value_row(&mut state), "float3:-100:100:trackball:soft");
    // Held by a drag in the pane, it does not, whatever the value.
    state.drag_widget = Some(crate::slots::PARAM_IDX);
    set(&mut state, "value", "99:0:0");
    assert_eq!(value_row(&mut state), "float3:-100:100:trackball:soft", "no re-scale under the pointer");
    state.drag_widget = None;
    assert_eq!(value_row(&mut state), "float3:-1000:1000:trackball:soft", "and on the release it does");
    // Far inside, it comes back down.
    set(&mut state, "value", "1.00:2.00:3.00");
    assert_eq!(value_row(&mut state), "float3:-10:10:trackball:soft");
    state.current_dir_mut().children[1].params.iter_mut().find(|p| p.name == "value").unwrap().set_expr(true);
    assert_eq!(value_row(&mut state), "text", "an expression is shown as its text");
    state.current_dir_mut().children[1].params.iter_mut().find(|p| p.name == "value").unwrap().set_expr(false);

    // Read from an attribute: no Value row, and the source is picked
    // from the input's attributes.
    set(&mut state, "value_from", "Attribute");
    state.sync_parameters_pane();
    let rows = state.param_mut().node_params();
    assert!(rows.iter().all(|r| r.0 != "Value"), "no Value row");
    let from = rows.iter().find(|r| r.0 == "From Attribute").expect("a From Attribute row");
    assert!(from.2.starts_with("textpick:") && from.2["textpick:".len()..].split(',').any(|a| a == "N"), "{}", from.2);
    set(&mut state, "value_from", "Constant");

    // The parameter itself never changed kind: it is text in the node.
    assert_eq!(state.current_dir().children[1].params.iter().find(|p| p.name == "value").unwrap().kind(), crate::param::ParamKind::Text);
}

/// A trackpad swipe over a band of the pull node's float3 Value row
/// turns that component, and the node's Value follows: Y alone, written
/// back as the `x:y:z` text the Attribute node parses. Until 2026-09-28
/// the params pane kept every finger gesture for its own scroll, so a
/// slider could be turned by a wheel notch and not by a trackpad — and a
/// float3 gave a scroll over its Y band to X, the first row in order.
#[test]
fn a_trackpad_swipe_over_a_float3_band_turns_that_component() {
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::{scroll_motion::set_scroll_phase, MouseScrollDelta, ParametersBg, Position, ScrollPhase};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = LEFT_MENUBAR_IDX;
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
    let pull = state.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
    for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.00:0.00:0.00")] {
        state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
    }
    state.graph_mut().set_selected_node(Some(pull));
    state.sync_parameters_pane();
    state.rebuild_positions();
    state.apply_layout();

    // The Y band of the Value row, from the pane's own float3 group.
    let (bx, by) = {
        // The slot is statically an `Adapted<ParametersBg>`.
        let pane: &ParametersBg = state.ui_context[state.slots.param].inner();
        let f = pane.float3s.iter().flatten().next().expect("the Value row is a float3");
        let (rx, ry, rw, rh) = f.get_row_rects()[1];
        (rx + (rw - 68.0) * 0.5, ry + rh * 0.5)
    };
    let value = |state: &State| -> Vec<f32> {
        state.current_dir().children[pull].params.iter().find(|p| p.name == "value").unwrap()
            .text().split(':').map(|v| v.parse().unwrap()).collect()
    };
    state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: bx as f64, y: by as f64 } });
    set_scroll_phase(ScrollPhase::Finger);
    state.ui_context.scroll_gesture_new = true;
    state.ui_context.scroll_initiate_widget_id = None;
    assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 0.0, y: -60.0 }) }));
    set_scroll_phase(ScrollPhase::Wheel);
    let v = value(&state);
    assert_eq!((v[0], v[2]), (0.0, 0.0), "X and Z hold: {v:?}");
    assert_ne!(v[1], 0.0, "Y turned, and the node's Value followed: {v:?}");
}

/// The trackball is a float3 row's second control: on by default where
/// the three numbers are a VECTOR (the pull node's Value aimed at Pos),
/// off where they are a colour or a position, and the row menu's Show /
/// Hide Trackball chooses either way. The choice is the instance's — it
/// rides the file, only when made — and dragging the ball turns the
/// node's vector, keeping its length.
#[test]
fn the_trackball_turns_the_pull_nodes_vector() {
    use crate::app::ParamMenuAction as A;
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::{ElementState, MouseButton, ParametersBg};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = LEFT_MENUBAR_IDX;
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
    state.apply_action(McpAction::AddNode { template_name: "Group".into(), name: Some("group1".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
    let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
    let (pull, group) = (slot_of(&state, "pull1"), slot_of(&state, "group1"));
    for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.00:0.00:0.06")] {
        state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
    }
    let show = |state: &mut State, slot: usize| {
        state.graph_mut().set_selected_node(Some(slot));
        state.sync_parameters_pane();
        state.rebuild_positions();
        state.apply_layout();
    };
    let row = |state: &mut State, name: &str| state.param_mut().node_params().iter().find(|r| r.0 == name).expect("the row").2.clone();
    let entries = |state: &State, slot: usize, pname: &str| state.param_menu_rows(slot, pname).1;
    let (lo, hi) = (-1.0, 1.0);

    // The pull's Value: a vector, so the ball is there; the menu hides it.
    show(&mut state, pull);
    assert_eq!(row(&mut state, "Value"), format!("{}:soft", crate::app::float3_row(lo, hi, true)));
    assert!(entries(&state, pull, "value").contains(&A::HideTrackball));
    let pull_id = state.current_dir().children[pull].id.clone();
    state.run_param_action(&pull_id, "value", A::HideTrackball);
    assert_eq!(row(&mut state, "Value"), format!("{}:soft", crate::app::float3_row(lo, hi, false)));
    assert!(entries(&state, pull, "value").contains(&A::ShowTrackball));
    let saved = serde_json::to_string(&state.current_dir().children[pull]).unwrap();
    assert!(saved.contains("\"view\":\"sliders\""), "the choice rides the file: {saved}");
    state.run_param_action(&pull_id, "value", A::ShowTrackball);
    assert_eq!(row(&mut state, "Value"), format!("{}:soft", crate::app::float3_row(lo, hi, true)));

    // Aimed at Col the three numbers are a colour: no ball by default.
    state.apply_action(McpAction::SetParam { slot: pull, name: "attribute_name".into(), value: "Col".into() }, &mut redraw).unwrap();
    state.current_dir_mut().children[pull].params.iter_mut().find(|p| p.name == "value").unwrap().view.clear();
    show(&mut state, pull);
    assert_eq!(row(&mut state, "Value"), format!("{}:soft", crate::app::float3_row(lo, hi, false)));
    state.apply_action(McpAction::SetParam { slot: pull, name: "attribute_name".into(), value: "Pos".into() }, &mut redraw).unwrap();

    // A position (the Group node's Center): no ball until asked, and a
    // parameter that never chose writes no `view` at all.
    show(&mut state, group);
    assert!(row(&mut state, "Center").starts_with("float3:") && !row(&mut state, "Center").ends_with(":trackball"));
    let untouched = serde_json::to_string(&state.current_dir().children[group]).unwrap();
    assert!(!untouched.contains("\"view\""), "{untouched}");
    let group_id = state.current_dir().children[group].id.clone();
    state.run_param_action(&group_id, "center", A::ShowTrackball);
    assert!(row(&mut state, "Center").ends_with(":trackball"));
    // A slider row is not a float3: it is offered neither.
    assert!(!entries(&state, slot_of(&state, "sphere1"), "radius").iter().any(|a| matches!(a, A::ShowTrackball | A::HideTrackball)));

    // Drag the ball a quarter turn to the right: the pull, pointing at
    // the viewer, swings onto +X at the length it had.
    show(&mut state, pull);
    let (cx, cy, r) = {
        let pane: &ParametersBg = state.ui_context[state.slots.param].inner();
        pane.float3s.iter().flatten().next().expect("the Value row").ball_circle().expect("its ball")
    };
    let at = |x: f32, y: f32| WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } };
    state.handle_event(&at(cx, cy));
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
    for i in 1..=20 {
        state.handle_event(&at(cx + r * std::f32::consts::FRAC_PI_2 * i as f32 / 20.0, cy));
    }
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
    let v: Vec<f32> = state.current_dir().children[pull].params.iter().find(|p| p.name == "value").unwrap()
        .text().split(':').map(|c| c.parse().unwrap()).collect();
    assert!((v[0] - 0.06).abs() < 2e-3 && v[1].abs() < 2e-3 && v[2].abs() < 2e-3, "the pull points along +X: {v:?}");
}

/// A scroll over the trackball rolls it, through the designer's own
/// wheel path and the write-back: a two-finger gesture to the right
/// turns the pull, pointing at the viewer, toward +X at the length it
/// had, and a wheel notch down (content up) turns it toward +Y.
#[test]
fn a_scroll_over_the_trackball_rolls_the_pull_nodes_vector() {
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::{scroll_motion::set_scroll_phase, MouseScrollDelta, ParametersBg, Position, ScrollPhase};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = LEFT_MENUBAR_IDX;
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
    let pull = state.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
    for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.00:0.00:0.06")] {
        state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
    }
    state.graph_mut().set_selected_node(Some(pull));
    state.sync_parameters_pane();
    state.rebuild_positions();
    state.apply_layout();
    let (cx, cy, _) = {
        let pane: &ParametersBg = state.ui_context[state.slots.param].inner();
        pane.float3s.iter().flatten().next().expect("the Value row").ball_circle().expect("its ball")
    };
    let value = |state: &State| -> Vec<f32> {
        state.current_dir().children[pull].params.iter().find(|p| p.name == "value").unwrap()
            .text().split(':').map(|c| c.parse().unwrap()).collect()
    };
    let len = |v: &[f32]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: cx as f64, y: cy as f64 } });

    set_scroll_phase(ScrollPhase::Finger);
    state.ui_context.scroll_gesture_new = true;
    state.ui_context.scroll_initiate_widget_id = None;
    assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::PixelDelta(Position { x: 120.0, y: 0.0 }) }));
    set_scroll_phase(ScrollPhase::Wheel);
    let v = value(&state);
    assert!(v[0] > 0.02 && v[1].abs() < 2e-3 && v[2] > 0.0, "turned toward +X: {v:?}");
    assert!((len(&v) - 0.06).abs() < 2e-3, "at the length it had: {v:?}");

    state.ui_context.scroll_gesture_new = true;
    state.ui_context.scroll_initiate_widget_id = None;
    assert!(state.handle_event(&WindowEvent::MouseWheel { delta: MouseScrollDelta::LineDelta(0.0, -1.0) }));
    let w = value(&state);
    assert!(w[1] > 0.01, "a notch down turns it toward +Y: {w:?}");
    assert!((len(&w) - 0.06).abs() < 2e-3);
}

/// The trackball is seen from the viewport's camera: the direction
/// from the scene toward the camera is the ball's toward-the-viewer
/// axis, the scene's up stays up on the ball, and orbiting the camera
/// moves the view. A vector pointing at the camera faces the viewer on
/// the ball, and rolling the ball to the right swings the node's
/// vector to the right of the SCREEN.
#[test]
fn the_trackball_follows_the_viewport_camera() {
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::{ElementState, MouseButton, ParametersBg};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = LEFT_MENUBAR_IDX;
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
    let pull = state.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
    // A pull of length 0.6 straight at a camera out along (2.5, 1.8, 2.5).
    let eye = Vec3::new(2.5, 1.8, 2.5);
    let at_camera = eye.normalize() * 0.6;
    let text = format!("{:.4}:{:.4}:{:.4}", at_camera.x, at_camera.y, at_camera.z);
    for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", text.as_str())] {
        state.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
    }
    state.graph_mut().set_selected_node(Some(pull));
    state.sync_parameters_pane();
    state.rebuild_positions();
    state.apply_layout();

    assert!(state.sync_trackball_view(eye, Vec3::ZERO, Vec3::ZERO), "the view moved off the identity");
    assert!(!state.sync_trackball_view(eye, Vec3::ZERO, Vec3::ZERO), "the same camera again moves nothing");
    let ball_view = |state: &State| {
        let pane: &ParametersBg = state.ui_context[state.slots.param].inner();
        pane.float3s.iter().flatten().next().expect("the Value row").view()
    };
    let view = ball_view(&state);
    let (right, up, toward) = (Vec3::from(view[0]), Vec3::from(view[1]), Vec3::from(view[2]));
    assert!(toward.distance(eye.normalize()) < 1e-4, "toward the viewer is toward the camera: {toward:?}");
    assert!(up.y > 0.5, "the scene's up is up on the ball: {up:?}");
    assert!(right.dot(toward).abs() < 1e-4 && right.cross(up).distance(toward) < 1e-4, "a right-handed view");

    // Roll the ball a quarter turn right: the pull, which pointed at
    // the camera, now points along the camera's right.
    let (cx, cy, r) = {
        let pane: &ParametersBg = state.ui_context[state.slots.param].inner();
        pane.float3s.iter().flatten().next().unwrap().ball_circle().unwrap()
    };
    let at = |x: f32, y: f32| WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } };
    state.handle_event(&at(cx, cy));
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
    for i in 1..=20 {
        state.handle_event(&at(cx + r * std::f32::consts::FRAC_PI_2 * i as f32 / 20.0, cy));
    }
    state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
    let v: Vec<f32> = state.current_dir().children[pull].params.iter().find(|p| p.name == "value").unwrap()
        .text().split(':').map(|c| c.parse().unwrap()).collect();
    let v = Vec3::new(v[0], v[1], v[2]);
    assert!(v.distance(right * 0.6) < 5e-3, "the pull lies along screen right: {v:?} against {:?}", right * 0.6);

    // Orbiting the camera moves the ball's view with it.
    state.orbit_camera_by(120.0, 0.0);
    let orbited = if state.active_camera == "Default Camera" {
        state.sync_trackball_view(eye, Vec3::ZERO, Vec3::ZERO)
    } else {
        state.set_active_camera("Default Camera");
        state.orbit_camera_by(120.0, 0.0);
        state.sync_trackball_view(eye, Vec3::ZERO, Vec3::ZERO)
    };
    assert!(orbited, "an orbit moves the view");
    assert!(Vec3::from(ball_view(&state)[2]).distance(toward) > 0.05);
}

/// A detached parameters window is a working satellite: it opens on
/// the main window's selection and camera, the main window writes the
/// sync channel when its selection or its camera changes, and the
/// detached window's trackballs turn with a viewport it cannot see.
/// Until 2026-09-29 the window never read the channel at startup, so
/// it opened with nothing selected and an empty pane, and with a pane
/// other than the circular network detached neither window asked for
/// an autosave again.
#[test]
fn a_detached_params_window_follows_the_selection_and_the_camera() {
    use cce_ui::widget::ParametersBg;
    let dir = std::env::temp_dir().join(format!("cce-designer-detached-camera-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let channel = dir.join("default_project.json");
    let ball_view = |state: &State| {
        let pane: &ParametersBg = state.ui_context[state.slots.param].inner();
        pane.float3s.iter().flatten().next().expect("a float3 row").view()
    };
    let same = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| (0..3).all(|i| (0..3).all(|k| (a[i][k] - b[i][k]).abs() < 1e-4));

    let mut main = State::new(false);
    main.resize(1600.0, 900.0, 1.0);
    main.rebuild_positions();
    main.apply_layout();
    main.focused_pane = LEFT_MENUBAR_IDX;
    main.set_active_camera("Default Camera");
    let mut redraw = false;
    main.apply_action(McpAction::AddNode { template_name: "Attribute".into(), name: Some("pull1".into()), x: 5.0, y: 8.0 }, &mut redraw).unwrap();
    let pull = main.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
    for (name, value) in [("input", "sphere1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.00:0.60:0.00")] {
        main.apply_action(McpAction::SetParam { slot: pull, name: name.into(), value: value.into() }, &mut redraw).unwrap();
    }
    main.apply_action(McpAction::Select { slot: pull }, &mut redraw).unwrap();
    main.viewport_mut().rotation_y = 0.6;
    main.sync_trackball_view_from_camera();
    assert!(!main.syncing_windows() && !main.needs_autosave, "nothing detached: nothing to tell");

    // Detach: the channel is written, the child is started on it.
    main.detached_panes[crate::slots::PARAM_IDX] = true;
    assert!(main.syncing_windows());
    main.save_to_file(&channel).expect("the main window writes the channel");
    let mut child = State::new(false);
    child.detached_pane = Some(crate::slots::PARAM_IDX);
    child.resize(640.0, 400.0, 1.0);
    child.rebuild_positions();
    child.apply_layout();
    assert_eq!(child.param_mut().node_params().len(), 0, "a new state has nothing selected");
    child.seed_detached_window(&channel);
    assert_eq!(child.param_editor_selected(), Some(pull), "it opens on the main window's selection");
    assert!(child.param_mut().node_params().iter().any(|r| r.0 == "Value" && r.2.contains(":trackball")), "with its rows");
    assert!(same(ball_view(&child), ball_view(&main)), "and sees the ball from the main window's camera");
    assert!(!child.needs_autosave, "a detached window has no camera to tell of");

    // The main window's selection and camera each ask for an autosave…
    let sphere = main.current_dir().children.iter().position(|c| c.name == "sphere1").unwrap();
    main.needs_autosave = false;
    main.apply_custom_event(crate::app::CustomEvent::RunAction(McpAction::Select { slot: sphere }));
    assert!(main.needs_autosave, "a selection change is written for the detached window");
    main.apply_custom_event(crate::app::CustomEvent::RunAction(McpAction::Select { slot: pull }));
    main.needs_autosave = false;
    let before = ball_view(&main);
    main.orbit_camera_by(150.0, 40.0);
    assert!(main.sync_trackball_view_from_camera(), "the orbit moved the view");
    assert!(main.needs_autosave, "and asks for the write that carries it");
    assert!(!same(ball_view(&main), before));

    // …and the detached window, reloading what was written, follows.
    main.save_to_file(&channel).expect("autosave");
    assert!(!same(ball_view(&child), ball_view(&main)), "not before it reloads");
    child.load_sync_channel(&channel, false).expect("the detached window reloads");
    child.sync_trackball_view_from_camera();
    assert!(same(ball_view(&child), ball_view(&main)), "the detached ball turned with the viewport");

    // A detached window's own change is written too, for the main one.
    child.needs_autosave = false;
    child.apply_custom_event(crate::app::CustomEvent::RunAction(McpAction::SetParam { slot: pull, name: "value".into(), value: "0.10:0.20:0.30".into() }));
    assert!(child.needs_autosave);

    let _ = fs::remove_dir_all(&dir);
}

/// A row of the spreadsheet is a point, and selecting rows marks their
/// points in the scene: a press selects one, ctrl adds another, and the
/// markers are staged from the positions the table was filled from —
/// nothing is evaluated. The selection goes when the table becomes
/// another node's.
#[test]
fn selected_spreadsheet_rows_are_marked_in_the_scene() {
    use crate::slots::SPREADSHEET_IDX;
    use crate::window::{LocalPosition, WindowEvent};
    use cce_ui::widget::{ElementState, MouseButton};
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.execute_menu_action("Show Spreadsheet Pane");
    state.rebuild_positions();
    state.apply_layout();
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Box".into(), name: Some("rows_a".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
    state.apply_action(McpAction::AddNode { template_name: "Box".into(), name: Some("rows_b".into()), x: 7.0, y: 8.0 }, &mut redraw).unwrap();
    let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
    let (a, b) = (slot_of(&state, "rows_a"), slot_of(&state, "rows_b"));
    state.apply_action(McpAction::Select { slot: a }, &mut redraw).unwrap();
    state.sync_nodes();
    assert_eq!(state.spreadsheet_points.len(), 8, "a box has eight points, a row each");
    assert!(state.row_marker_instances.is_empty());

    let (sx, sy, sw, sh) = state.positions[SPREADSHEET_IDX];
    assert!(sw > 0.0 && sh > 60.0, "the spreadsheet is laid out: {sw} x {sh}");
    // Rows are 24 tall under a 24 header.
    let press = |state: &mut State, row: usize| {
        let (x, y) = (sx + 40.0, sy + 24.0 + 24.0 * row as f32 + 12.0);
        state.handle_event(&WindowEvent::CursorMoved { position: LocalPosition { x: x as f64, y: y as f64 } });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left });
        state.handle_event(&WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left });
    };
    let version = state.rt_geometry_version;
    press(&mut state, 1);
    assert_eq!(state.selected_spreadsheet_points(), vec![1]);
    assert!(!state.row_marker_instances.is_empty() && state.row_markers_dirty, "the marker is staged");
    let one = state.row_marker_instances.len();
    // The marker stands on the row's point.
    let p = state.spreadsheet_points[1];
    let n = one as f32;
    let mid = state.row_marker_instances.iter().fold([0.0f32; 3], |m, v| [m[0] + v.position[0] / n, m[1] + v.position[1] / n, m[2] + v.position[2] / n]);
    assert!((0..3).all(|k| (mid[k] - p[k]).abs() < 1e-3), "{mid:?} is not at {p:?}");

    state.modifiers.ctrl = true;
    press(&mut state, 0);
    state.modifiers.ctrl = false;
    assert_eq!(state.selected_spreadsheet_points(), vec![0, 1]);
    assert_eq!(state.row_marker_instances.len(), 2 * one, "a marker a row");
    assert_eq!(state.rt_geometry_version, version, "selecting evaluates nothing");

    // A refresh of the same node's table keeps it.
    state.apply_action(McpAction::SetParam { slot: a, name: "center".into(), value: "1.00:0.50:0.25".into() }, &mut redraw).unwrap();
    state.sync_nodes();
    assert_eq!(state.selected_spreadsheet_points(), vec![0, 1]);
    let moved = state.spreadsheet_points[1];
    let mid = state.row_marker_instances[one..].iter().chain(&state.row_marker_instances[..one]).fold([0.0f32; 3], |m, v| [m[0] + v.position[0], m[1] + v.position[1], m[2] + v.position[2]]);
    let both = [moved, state.spreadsheet_points[0]];
    let want = [both[0][0] + both[1][0], both[0][1] + both[1][1], both[0][2] + both[1][2]];
    assert!((0..3).all(|k| (mid[k] / one as f32 - want[k]).abs() < 1e-2), "the markers followed the points");

    // Another node's table is other points.
    state.apply_action(McpAction::Select { slot: b }, &mut redraw).unwrap();
    state.sync_nodes();
    assert!(state.selected_spreadsheet_points().is_empty());
    assert!(state.row_marker_instances.is_empty());
}

/// A node INSIDE a simnet is read as the scene draws it there: as the
/// frame's last substep saw it, not from the seed. The spreadsheet's
/// rows, and the markers on the rows selected, follow the simulation.
#[test]
fn rows_selected_inside_a_simnet_follow_the_simulation() {
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = LEFT_MENUBAR_IDX;
    state.show_spreadsheet = true;
    let mut redraw = false;
    state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
    let sim = state.current_dir().children.iter().position(|c| c.name == "sim").unwrap();
    state.apply_action(McpAction::SetParam { slot: sim, name: "input".into(), value: "sphere1".into() }, &mut redraw).unwrap();
    {
        let simnet = &mut state.current_dir_mut().children[sim];
        let mut pull = crate::app::load_fs_tree().children.into_iter().find(|t| t.node_type == "attribute").unwrap();
        pull.id = "pull-in-sim".into();
        pull.name = "pull1".into();
        for (name, value) in [("input", "input1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.05:0.00:0.00"), ("combine", "Add")] {
            pull.params.iter_mut().find(|p| p.name == name).unwrap().set_text(value.to_string());
        }
        simnet.children.push(pull);
        let output = simnet.children.iter_mut().find(|c| c.node_type == "output").unwrap();
        output.params.iter_mut().find(|p| p.name == "input").unwrap().set_text("pull1".to_string());
    }
    // Dive in and select the pull.
    state.current_path.push(sim);
    state.sync_nodes();
    let pull = state.current_dir().children.iter().position(|c| c.name == "pull1").unwrap();
    state.apply_action(McpAction::Select { slot: pull }, &mut redraw).unwrap();
    state.ui_context[state.slots.playbar].inner_mut().current_frame = 5.0;
    state.tick_frame(1.0 / 60.0);
    state.sync_nodes();
    assert!(!state.spreadsheet_points.is_empty());
    state.spreadsheet_mut().set_selected_rows(&[3]);
    state.rebuild_row_markers();
    let middle = |state: &State| {
        let n = state.row_marker_instances.len() as f32;
        state.row_marker_instances.iter().fold(0.0f32, |m, v| m + v.position[0] / n)
    };
    let (row_at, marker_at) = (state.spreadsheet_points[3][0], middle(&state));
    assert!((row_at - marker_at).abs() < 1e-3);

    state.ui_context[state.slots.playbar].inner_mut().current_frame = 15.0;
    state.tick_frame(1.0 / 60.0);
    let moved = state.spreadsheet_points[3][0] - row_at;
    assert!(moved > 0.3, "ten frames of the pull moved the row's point {moved}");
    assert!((middle(&state) - state.spreadsheet_points[3][0]).abs() < 1e-3, "and its marker with it");
    assert_eq!(state.selected_spreadsheet_points(), vec![3]);
}

/// The spreadsheet and the selected-group markers evaluate through the
/// SHARED sim cache: with either reading something downstream of a
/// simnet, a refresh costs no steps beyond the ones the frame itself
/// took. Each used a cache of its own until 2026-09-29, and solved the
/// simulation again from the seed.
#[test]
fn the_spreadsheet_and_group_markers_share_the_sim_cache() {
    let steps = || crate::geometry::STEPS_ON_THIS_THREAD.with(|s| s.get());
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = LEFT_MENUBAR_IDX;
    state.show_spreadsheet = true;
    let mut redraw = false;
    // sphere1 -> sim (a pull inside) -> tagged (a Group reading the sim).
    state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
    state.apply_action(McpAction::AddNode { template_name: "Group".into(), name: Some("tagged".into()), x: 7.0, y: 8.0 }, &mut redraw).unwrap();
    let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
    let (sim, tagged) = (slot_of(&state, "sim"), slot_of(&state, "tagged"));
    state.apply_action(McpAction::SetParam { slot: sim, name: "input".into(), value: "sphere1".into() }, &mut redraw).unwrap();
    state.apply_action(McpAction::SetParam { slot: tagged, name: "input".into(), value: "sim".into() }, &mut redraw).unwrap();
    state.apply_action(McpAction::SetParam { slot: tagged, name: "mode".into(), value: "Random".into() }, &mut redraw).unwrap();
    state.apply_action(McpAction::SetParam { slot: tagged, name: "count".into(), value: "5".into() }, &mut redraw).unwrap();
    {
        let simnet = &mut state.current_dir_mut().children[sim];
        let template = crate::app::load_fs_tree().children.into_iter().find(|t| t.node_type == "attribute").unwrap();
        let mut node = template.clone();
        node.id = "pull-in-sim".into();
        node.name = "pull1".into();
        for (name, value) in [("input", "input1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.01:0.00:0.00"), ("combine", "Add")] {
            node.params.iter_mut().find(|p| p.name == name).unwrap().set_text(value.to_string());
        }
        simnet.children.push(node);
        let output = simnet.children.iter_mut().find(|c| c.node_type == "output").expect("a simnet has an output");
        output.params.iter_mut().find(|p| p.name == "input").unwrap().set_text("pull1".to_string());
    }
    state.ui_context[state.slots.playbar].inner_mut().current_frame = 61.0;
    state.sync_nodes();
    state.rebuild_scene_geometry();

    // Select the Group downstream of the simulation: the spreadsheet
    // fills and the markers stage. Whatever solving the frame takes is
    // done ONCE, by whoever asks first…
    state.apply_action(McpAction::Select { slot: tagged }, &mut redraw).unwrap();
    state.sync_nodes();
    assert_eq!(state.group_members.len(), 5, "the markers were staged from the simulated geometry");
    let solved = steps();
    assert!(solved >= 60, "the fixture simulates: {solved} steps");
    assert!(solved < 120, "the spreadsheet and the markers solved it between them once, not once each: {solved}");
    // …and nobody after: the simnet itself in the spreadsheet, the
    // group again, a scene rebuild — all through the one cache.
    state.apply_action(McpAction::Select { slot: sim }, &mut redraw).unwrap();
    state.sync_nodes();
    assert_eq!(steps(), solved, "the simnet in the spreadsheet");
    state.apply_action(McpAction::Select { slot: tagged }, &mut redraw).unwrap();
    state.sync_nodes();
    assert_eq!(steps(), solved, "the group again");
    state.rebuild_scene_geometry();
    state.sync_nodes();
    assert_eq!(steps(), solved, "and the scene");
    // And the cache is back where it lives, its solve intact.
    assert!(!state.sim_cache.checkpoint_frames(&state.current_dir().children[sim].id).is_empty());
}

/// The spreadsheet reads what is selected again whenever the answer
/// may have changed: the frame moved — with a simulation in the graph
/// or without one — or something upstream was edited. It used to read
/// again only when the selected node or its OWN parameters changed, so
/// during playback it showed the frame it had been opened on. The
/// selected Group's markers follow the frame the same way.
#[test]
fn the_spreadsheet_and_markers_follow_the_frame_and_upstream_edits() {
    let mut state = State::new(false);
    state.resize(1600.0, 900.0, 1.0);
    state.rebuild_positions();
    state.apply_layout();
    state.focused_pane = LEFT_MENUBAR_IDX;
    state.show_spreadsheet = true;
    let mut redraw = false;
    let slot_of = |state: &State, name: &str| state.current_dir().children.iter().position(|c| c.name == name).expect(name);
    let sphere = slot_of(&state, "sphere1");
    // Where the markers stand, as plain numbers.
    let marks = |state: &State| -> Vec<[f32; 3]> { state.group_members.iter().map(|v| v.position).collect() };

    // No simulation in the graph. The frame moves; nothing rebuilds the
    // scene; what is selected is still read again.
    assert!(!crate::geometry::contains_simnet(&state.fs_root));
    state.apply_action(McpAction::Select { slot: sphere }, &mut redraw).unwrap();
    state.sync_nodes();
    state.tick_frame(1.0 / 60.0);
    let opened_at = state.last_spreadsheet_read_at;
    assert_eq!(opened_at.0, state.sim_frame());
    state.ui_context[state.slots.playbar].inner_mut().current_frame = 7.0;
    state.tick_frame(1.0 / 60.0);
    assert_eq!(state.last_spreadsheet_read_at.0, 7, "read again at the new frame");
    // The same frame again reads nothing again.
    let at = state.last_spreadsheet_read_at;
    state.tick_frame(1.0 / 60.0);
    state.sync_nodes();
    assert_eq!(state.last_spreadsheet_read_at, at);

    // An edit UPSTREAM of the selection: the selected node and its own
    // parameters are as they were, and the rows are read again.
    state.apply_action(McpAction::AddNode { template_name: "Group".into(), name: Some("tagged".into()), x: 7.0, y: 8.0 }, &mut redraw).unwrap();
    let tagged = slot_of(&state, "tagged");
    for (name, value) in [("input", "sphere1"), ("mode", "Random"), ("count", "5")] {
        state.apply_action(McpAction::SetParam { slot: tagged, name: name.into(), value: value.into() }, &mut redraw).unwrap();
    }
    state.apply_action(McpAction::Select { slot: tagged }, &mut redraw).unwrap();
    state.sync_nodes();
    let (before, markers) = (state.last_spreadsheet_read_at, marks(&state));
    assert_eq!(markers.len(), 5);
    state.apply_action(McpAction::SetParam { slot: sphere, name: "radius".into(), value: "0.9".into() }, &mut redraw).unwrap();
    assert_eq!(state.param_editor_selected(), Some(tagged), "the selection did not move");
    assert_ne!(state.last_spreadsheet_read_at, before, "the rows were read again");
    assert_ne!(marks(&state), markers, "and the markers moved out with the sphere");

    // With a simulation, playback: every frame the playbar arrives at
    // is the frame the rows and the markers were read at.
    state.apply_action(McpAction::AddNode { template_name: "Simnet".into(), name: Some("sim".into()), x: 6.0, y: 8.0 }, &mut redraw).unwrap();
    let sim = slot_of(&state, "sim");
    state.apply_action(McpAction::SetParam { slot: sim, name: "input".into(), value: "sphere1".into() }, &mut redraw).unwrap();
    {
        let simnet = &mut state.current_dir_mut().children[sim];
        let mut pull = crate::app::load_fs_tree().children.into_iter().find(|t| t.node_type == "attribute").unwrap();
        pull.id = "pull-in-sim".into();
        pull.name = "pull1".into();
        for (name, value) in [("input", "input1"), ("operation", "Modify"), ("attribute_name", "Pos"), ("value", "0.05:0.00:0.00"), ("combine", "Add")] {
            pull.params.iter_mut().find(|p| p.name == name).unwrap().set_text(value.to_string());
        }
        simnet.children.push(pull);
        let output = simnet.children.iter_mut().find(|c| c.node_type == "output").unwrap();
        output.params.iter_mut().find(|p| p.name == "input").unwrap().set_text("pull1".to_string());
    }
    state.apply_action(McpAction::SetParam { slot: tagged, name: "input".into(), value: "sim".into() }, &mut redraw).unwrap();
    state.apply_action(McpAction::Select { slot: tagged }, &mut redraw).unwrap();
    state.ui_context[state.slots.playbar].inner_mut().current_frame = 10.0;
    state.tick_frame(1.0 / 60.0);
    let mut last = marks(&state);
    state.ui_context[state.slots.playbar].inner_mut().playing = true;
    state.ui_context[state.slots.playbar].inner_mut().fps = 60.0;
    for _ in 0..3 {
        state.tick_frame(1.0 / 60.0);
        assert_eq!(state.last_spreadsheet_read_at.0, state.sim_frame(), "the rows are this frame's");
        assert_ne!(marks(&state), last, "the markers moved with the simulation");
        last = marks(&state);
    }
}
