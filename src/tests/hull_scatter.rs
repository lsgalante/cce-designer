//! Hull, surface scatter, repel relax, and the Embryo template.

use super::*;

/// The hull of a cube's corners plus points inside it is the cube: eight
/// points, twelve triangles, closed, with nothing left outside it.
#[test]
fn convex_hull_of_a_cube_with_interior_points_is_the_cube() {
    use crate::hull::convex_hull;
    let mut pts = Vec::new();
    for x in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for z in [-1.0, 1.0] {
                pts.push(Vec3::new(x, y, z));
            }
        }
    }
    for i in 0..50 {
        let t = i as f32 / 50.0;
        pts.push(Vec3::new(t * 0.9 - 0.45, (t * 7.0).sin() * 0.5, (t * 3.0).cos() * 0.5));
    }
    let hull = convex_hull(&pts).expect("a cube spans a volume");
    assert_eq!(hull.num_points(), 8, "only the corners are on the hull");
    assert_eq!(hull.num_prims(), 12);
    assert!(hull.is_closed(), "a hull is watertight and consistently wound");
    for prim in 0..hull.num_prims() {
        let ids = hull.prim_points(prim);
        let (a, b, c) = (hull.pos(ids[0] as usize), hull.pos(ids[1] as usize), hull.pos(ids[2] as usize));
        let n = (b - a).cross(c - a).normalize();
        assert!(a.dot(n) > 0.0, "face {prim} winds outward");
        for q in &pts {
            assert!((*q - a).dot(n) <= 1e-4, "point {q:?} is outside face {prim}");
        }
    }
    let flat: Vec<Vec3> = (0..20).map(|i| Vec3::new(i as f32, (i * i) as f32 * 0.1, 0.0)).collect();
    assert!(convex_hull(&flat).is_none(), "coplanar points span no volume");
    assert!(convex_hull(&pts[..3]).is_none());

    // The node: a hull of the input's points; too few to hull passes through.
    let src = ref_node("s", "src", "points", vec![("shape", "text", "Spiral"), ("points", "spinbox", "60"), ("markers", "text", "false")], vec![]);
    let hull_node = ref_node("h", "hull1", "hull", vec![("input", "text", "src")], vec![]);
    let root = ref_node("root", "root", "node", vec![], vec![src, hull_node]);
    let (g, err) = eval(&root, &root.children[1]);
    assert!(err.is_none(), "{err:?}");
    let g = g.unwrap();
    assert!(g.num_prims() > 0 && g.is_closed(), "the spiral hulls into a closed mesh");
    let line = ref_node("l", "line", "points", vec![("shape", "text", "Line"), ("points", "spinbox", "5"), ("markers", "text", "false")], vec![]);
    let hull2 = ref_node("h2", "hull2", "hull", vec![("input", "text", "line")], vec![]);
    let root2 = ref_node("root", "root", "node", vec![], vec![line, hull2]);
    let g = eval(&root2, &root2.children[1]).0.unwrap();
    assert_eq!((g.num_points(), g.num_prims()), (5, 0), "a line of points passes through unhulled");
}

/// Scattered points lie on the surface, in the number asked for, and a
/// seed reproduces its draw; the node's Surface mode emits them, relaxed
/// apart when asked.
#[test]
fn scatter_surface_mode_lands_on_the_surface_and_is_seeded() {
    use crate::scatter::scatter_on_surface;
    let sphere = crate::geometry::sphere_detail(Vec3::ZERO, 0.5, 12, 16);
    let a = scatter_on_surface(&sphere, 300, 1.1);
    assert_eq!(a.len(), 300);
    let grid = crate::spatial::TriGrid::build(&sphere);
    for p in &a {
        let hit = grid.closest(*p).unwrap();
        assert!(hit.distance < 1e-4, "point {p:?} is {} off the surface", hit.distance);
    }
    assert_eq!(a, scatter_on_surface(&sphere, 300, 1.1), "same seed, same points");
    assert_ne!(a, scatter_on_surface(&sphere, 300, 2.0), "another seed, another draw");
    assert!(scatter_on_surface(&Detail::new(), 10, 1.0).is_empty());

    let scatter = |relax: &str| {
        let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "0.5")], vec![]);
        let sc = ref_node("sc", "scatter1", "scatter", vec![
            ("input", "text", "src"), ("mode", "choice:Volume,Surface", "Surface"), ("points", "spinbox", "80"),
            ("seed", "slider", "1.1"), ("relax_points", "toggle", relax), ("relax_iterations", "spinbox", "30"),
            ("markers", "choice:true,false", "false"),
        ], vec![]);
        let root = ref_node("root", "root", "node", vec![], vec![src, sc]);
        let (g, err) = eval(&root, &root.children[1]);
        assert!(err.is_none(), "{err:?}");
        (g.unwrap(), eval(&root, &root.children[0]).0.unwrap())
    };
    let (raw, src) = scatter("false");
    assert_eq!(raw.num_points(), 80, "bare points, one per location");
    let grid = crate::spatial::TriGrid::build(&src);
    for p in 0..raw.num_points() {
        assert!(grid.closest(raw.pos(p)).unwrap().distance < 1e-3);
    }
    let (relaxed, _) = scatter("true");
    assert_eq!(relaxed.num_points(), 80);
    let nearest = |d: &Detail| -> f32 {
        let mut worst = f32::MAX;
        for i in 0..d.num_points() {
            let mut best = f32::MAX;
            for j in 0..d.num_points() {
                if i != j { best = best.min((d.pos(i) - d.pos(j)).length()); }
            }
            worst = worst.min(best);
        }
        worst
    };
    assert!(nearest(&relaxed) > nearest(&raw), "relaxing spreads the closest pair: {} vs {}", nearest(&relaxed), nearest(&raw));
    for p in 0..relaxed.num_points() {
        assert!(grid.closest(relaxed.pos(p)).unwrap().distance < 1e-3, "relaxed points stay on the surface");
    }
}

/// Relax in Repel mode slides a mesh's points apart in their tangent
/// planes, so they stay on the shape; In 3D Space lets them leave it.
#[test]
fn relax_repel_mode_keeps_points_in_their_tangent_planes() {
    let relax = |in_3d: &str, iterations: &str| {
        let src = ref_node("s", "src", "sphere", vec![("radius", "slider", "0.5")], vec![]);
        let rx = ref_node("r", "relax1", "relax", vec![
            ("input", "text", "src"), ("mode", "choice:Springs,Repel", "Repel"), ("iterations", "spinbox", iterations),
            ("radius", "slider", "0.08"), ("in_3d_space", "toggle", in_3d),
        ], vec![]);
        let root = ref_node("root", "root", "node", vec![], vec![src, rx]);
        (eval(&root, &root.children[1]).0.unwrap(), eval(&root, &root.children[0]).0.unwrap())
    };
    let (relaxed, plain) = relax("false", "5");
    let centre = (0..plain.num_points()).map(|i| plain.pos(i)).sum::<Vec3>() / plain.num_points() as f32;
    let mut moved = 0;
    for i in 0..relaxed.num_points() {
        if (relaxed.pos(i) - plain.pos(i)).length() > 1e-5 { moved += 1; }
        assert!(((relaxed.pos(i) - centre).length() - 0.5).abs() < 0.05, "point {i} left the sphere");
    }
    assert!(moved > 0, "some point moved");
    let (free, _) = relax("true", "5");
    assert!((0..free.num_points()).any(|i| ((free.pos(i) - centre).length() - 0.5).abs() > 0.01), "in 3D the points are free to leave");
    let (off, _) = relax("false", "0");
    assert!((0..off.num_points()).all(|i| (off.pos(i) - plain.pos(i)).length() < 1e-6), "zero iterations is off");
}

/// The Embryo template, end to end: an instance whose children read
/// its controls through references. Basic is the internal sphere;
/// Scatter is a closed hull of at most Scatter Count points inside the
/// sphere's radius; Input reads what it is given; every mesh carries N.
#[test]
fn embryo_template_builds_a_sphere_a_hull_or_the_input() {
    let templates_root = crate::app::load_fs_tree();
    let t = templates_root.children.iter().find(|t| t.name == "Embryo").expect("the Embryo template");
    assert_eq!(t.node_type, "node", "the Embryo is a subnet of nodes");
    let names: Vec<&str> = t.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["input1", "sphere1", "source1", "scatter1", "hull1", "method1", "relax1", "subdivide1", "normal1", "output1"]);
    for c in &t.children {
        // The last real node is the one that draws — as the Sphere's
        // kernel node is — since a subnet viewed from outside shows its
        // internals by their flags and output children only draw at the
        // displayed level. Every chain node visible drew the hull five
        // times over and cost seconds per edit.
        assert_eq!(c.geometry_visible, c.name == "normal1", "only normal1 draws: {} is {}", c.name, c.geometry_visible);
    }
    // Nested template resolution: the sphere inside carries its own
    // resolved children, kernel node params included.
    let sphere1 = t.children.iter().find(|c| c.name == "sphere1").unwrap();
    assert_eq!(sphere1.node_type, "sphere", "the nested sphere is the native Sphere");
    assert!(sphere1.params.iter().any(|p| p.name == "method"), "with its template's whole surface");
    assert!(sphere1.params.iter().find(|p| p.name == "radius").unwrap().is_expr(), "and the Embryo's reference on its Radius");

    let instance = |overrides: &[(&str, &str)], extra: Vec<FsNode>| {
        let mut inst = t.clone();
        crate::app::regenerate_node_ids(&mut inst);
        inst.name = "embryo1".into();
        for (n, v) in overrides {
            inst.params.iter_mut().find(|p| p.name == *n).unwrap_or_else(|| panic!("param {n}")).set_text(v.to_string());
        }
        let mut children = extra;
        children.push(inst);
        ref_node("root", "root", "node", vec![], children)
    };
    let run = |root: &FsNode| {
        let e = root.children.iter().find(|c| c.name == "embryo1").unwrap();
        let (g, err) = eval(root, e);
        assert!(err.is_none(), "{err:?}");
        g.expect("the embryo evaluates")
    };
    let extent = |g: &Detail| g.positions().iter().map(|p| Vec3::from(*p).length()).fold(0.0, f32::max);

    let basic = run(&instance(&[], vec![]));
    assert!(basic.num_prims() > 0);
    assert!((extent(&basic) - 0.5).abs() < 0.02, "Basic is the internal sphere of Radius 0.5: {}", extent(&basic));
    assert!(basic.points().value("N", 0).is_some(), "normals are written last");
    let big = run(&instance(&[("radius", "1.5"), ("base_resolution", "8")], vec![]));
    assert!((extent(&big) - 1.5).abs() < 0.05, "Radius reaches the sphere through chf: {}", extent(&big));
    assert!(big.num_points() < basic.num_points(), "Base Resolution reaches Rows and Columns through chi");

    let scattered = run(&instance(&[("method", "Scatter"), ("scatter_count", "400"), ("base_resolution", "16")], vec![]));
    assert!(scattered.num_prims() > 0 && scattered.is_closed(), "Scatter hulls the points into a closed mesh");
    assert!(scattered.num_points() <= 400);
    assert!(extent(&scattered) <= 0.5 + 1e-3, "the hull lies inside the seed sphere");
    assert!(scattered.points().value("N", 0).is_some());

    let seed = ref_node("seed", "seed", "sphere", vec![("radius", "slider", "0.25")], vec![]);
    let root = instance(&[("source", "Input"), ("input", "seed")], vec![seed]);
    let from_input = run(&root);
    let seed_geom = eval(&root, &root.children[0]).0.unwrap();
    assert_eq!(from_input.num_points(), seed_geom.num_points(), "Source Input is the wired node");
    assert!((from_input.pos(0) - seed_geom.pos(0)).length() < 1e-6);
    let e = instance(&[("source", "Input")], vec![]);
    assert!(eval(&e, &e.children[0]).0.is_none(), "Source Input with nothing wired seeds nothing");

    let coarse = run(&instance(&[("base_resolution", "8")], vec![]));
    let sub = run(&instance(&[("base_resolution", "8"), ("subdivision_depth", "1")], vec![]));
    // Against the real subdivide of the same mesh rather than ×4: the
    // kernel sphere's pole triangles are degenerate and subdivide drops
    // them.
    assert_eq!(sub.num_prims(), crate::remesh::subdivide(&coarse, 1).num_prims(), "Subdivision Depth reaches Depth");
}

/// A native embryo from 2026-09-21 loads as an instance of the template,
/// with its values, its identity and its meta child intact.
#[test]
fn a_native_embryo_recomposes_on_load() {
    let templates_root = crate::app::load_fs_tree();
    let templates = crate::app::flatten_node_templates(&templates_root);
    let param = |n: &str, v: &str| crate::app::ParamDef::new(n, "text", v);
    let meta = ref_node("m", "meta", "meta", vec![("Point Markers", "toggle", "true")], vec![]);
    let mut native = ref_node("old-id", "embryo1", "embryo", vec![], vec![meta]);
    native.params = vec![param("input", ""), param("method", "Scatter"), param("scatter_count", "150"), param("radius", "0.7"), param("base_resolution", "16")];
    native.position = (3.0, 4.0);
    native.geometry_visible = true;
    let mut root = ref_node("root", "root", "node", vec![], vec![native]);
    crate::app::merge_template_defs(&mut root, &templates);
    let e = &root.children[0];
    assert_eq!(e.node_type, "node", "recomposed as a subnet");
    assert_eq!((e.id.as_str(), e.name.as_str(), e.position, e.geometry_visible), ("old-id", "embryo1", (3.0, 4.0), true));
    let get = |n: &str| e.params.iter().find(|p| p.name == n).unwrap().text().to_string();
    assert_eq!(get("method"), "Scatter");
    assert_eq!(get("scatter_count"), "150");
    assert_eq!(get("radius"), "0.7");
    assert_eq!(get("scatter_seed"), "1.1", "a param the native node lacked takes the template default");
    assert!(e.children.iter().any(|c| c.name == "hull1"));
    // The per-node meta child an older save carried is stripped, here as
    // everywhere else: merge_template_defs takes them before it matches
    // anything, so a recompose never has one to carry over.
    assert!(!e.children.iter().any(|c| c.node_type == "meta"), "a meta child survived the recompose");
    let (g, err) = eval(&root, e);
    assert!(err.is_none(), "{err:?}");
    let g = g.unwrap();
    assert!(g.is_closed() && g.num_points() <= 150, "and it evaluates as the scatter it was");
    let extent = g.positions().iter().map(|p| Vec3::from(*p).length()).fold(0.0, f32::max);
    assert!(extent <= 0.7 + 1e-3 && extent > 0.5);
}

#[test]
fn test_the_mold_shell_node_builds_a_two_sided_shell() {
    use crate::geometry::resolve_mold_shell_geometry_with_errors;
    fn mnode(id: &str, name: &str, ty: &str, params: &[(&str, &str)]) -> FsNode {
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
    let sphere = mnode("id-s", "sphere1", "sphere", &[("radius", "0.8")]);
    let shell = mnode(
        "id-m",
        "mold1",
        "mold_shell",
        &[
            ("input", "sphere1"),
            ("maximum_thickness", "0.20"),
            ("minimum_thickness", "0.10"),
            ("remesh_division_size", "0.30"),
            ("ramp", "Linear"),
        ],
    );
    let mut root = mnode("id-root", "root", "node", &[]);
    root.children = vec![sphere, shell];

    let mut err = None;
    let mut cache = crate::geometry::SimCache::default();
    let mut sim = crate::geometry::EvalSim::new(0, 0, &mut cache);
    let out = resolve_mold_shell_geometry_with_errors(
        &root,
        &root.children[1],
        &mut Vec::new(),
        &mut err,
        &mut sim,
    )
    .expect("the mold shell resolved to nothing");
    assert!(err.is_none(), "{err:?}");
    assert!(out.num_prims() > 0);

    // Two surfaces: the outer one at the sphere's radius, the inner one
    // pulled in by between the minimum and the maximum thickness.
    let centre = {
        let (lo, hi) = out.bounds().unwrap();
        (lo + hi) * 0.5
    };
    let radii: Vec<f32> = (0..out.num_points()).map(|p| (out.pos(p) - centre).length()).collect();
    let far = radii.iter().cloned().fold(0.0f32, f32::max);
    let near = radii.iter().cloned().fold(f32::MAX, f32::min);
    assert!((far - 0.8).abs() < 0.12, "the outer surface is at {far}, not the sphere's 0.8");
    assert!(
        near < far - 0.08 && near > far - 0.30,
        "the inner surface is {near} against an outer {far}; the gap should be the thickness range"
    );

    // Closed: the pair is a solid, not two loose surfaces. A sphere has no
    // rim, so the two shells close each other.
    assert!(out.is_closed(), "the shell is not a closed surface");
}

/// A page's raster is its physical size times its resolution — the
/// property that makes DPI a page parameter rather than an export one.
#[test]
fn test_a_page_is_its_physical_size_times_its_resolution() {
    use crate::page::Page;
    let p = Page::new([8.5, 11.0], 300, [1.0; 4]);
    assert_eq!((p.width, p.height), (2550, 3300));
    assert!((p.scale() - 300.0).abs() < 0.01);

    // The same sheet at a different resolution is the same sheet.
    let q = Page::new([8.5, 11.0], 72, [1.0; 4]);
    assert_eq!((q.width, q.height), (612, 792));
    assert!(
        ((p.width as f32 / p.height as f32) - (q.width as f32 / q.height as f32)).abs() < 1e-3
    );

    // A sheet nobody could print clamps rather than allocating: aspect
    // survives, resolution does not.
    let huge = Page::new([100.0, 50.0], 1200, [1.0; 4]);
    assert!(
        (huge.width as u64) * (huge.height as u64) <= 356_000_000,
        "{}x{} is not clamped",
        huge.width,
        huge.height
    );
    assert!(
        ((huge.width as f32 / huge.height as f32) - 2.0).abs() < 0.01,
        "the clamp changed the aspect: {}x{}",
        huge.width,
        huge.height
    );
}

/// Rect coverage is exact area, not a test of the pixel centre.
///
/// This is what keeps a ruled sheet's lines from alternating between one
/// and two pixels wide down its length — which prints as a wobble in the
/// paper rather than as aliasing.
#[test]
fn test_rect_coverage_is_exact_area() {
    use crate::page::Page;
    // Ten pixels per inch, so one pixel is a tenth of an inch and the
    // arithmetic is readable.
    let mut p = Page::new([1.0, 1.0], 10, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!((p.width, p.height), (10, 10));

    // A rect covering exactly the left half of pixel (0,0).
    p.rect(0.0, 0.0, 0.05, 0.1, [1.0, 1.0, 1.0, 1.0]);
    let v = p.pixels[0][0];
    assert!((v - 0.5).abs() < 1e-4, "half a pixel of white over black read {v}, not 0.5");

    // A whole-pixel rect is fully opaque, and its neighbour is untouched.
    let mut p = Page::new([1.0, 1.0], 10, [0.0, 0.0, 0.0, 1.0]);
    p.rect(0.2, 0.0, 0.3, 0.1, [1.0, 1.0, 1.0, 1.0]);
    assert!((p.pixels[2][0] - 1.0).abs() < 1e-4, "a whole pixel is not solid");
    assert!(p.pixels[1][0] < 1e-4, "the rect bled into its neighbour");
    assert!(p.pixels[3][0] < 1e-4, "the rect bled into its neighbour");

    // Off the sheet entirely is a no-op, not a panic or a wrap.
    p.rect(-5.0, -5.0, -4.0, -4.0, [1.0, 0.0, 0.0, 1.0]);
    p.rect(50.0, 50.0, 60.0, 60.0, [1.0, 0.0, 0.0, 1.0]);
    assert!(p.pixels.iter().all(|px| px[0] == px[1] && px[1] == px[2]), "red leaked in");
}

/// Two grids at cell and 2x cell share their rules exactly, which is the
/// whole reason for drawing a second one.
#[test]
fn test_a_second_grid_lands_on_the_first_ones_rules() {
    use crate::page::Page;
    let mut p = Page::new([2.0, 2.0], 100, [1.0, 1.0, 1.0, 1.0]);
    p.grid(0.25, 0.02, [0.0; 4], [0.0, 0.0, 0.0, 1.0]);
    // Column of the rule at x = 0.5 inches: 50 px in.
    let row = 37; // anywhere between two horizontal rules
    assert!(p.pixels[(row * p.width + 50) as usize][0] < 0.1, "no rule at 0.50 inches");
    assert!(p.pixels[(row * p.width + 37) as usize][0] > 0.9, "the cell is not clear");

    let mut q = Page::new([2.0, 2.0], 100, [1.0, 1.0, 1.0, 1.0]);
    q.grid(0.5, 0.02, [0.0; 4], [0.0, 0.0, 0.0, 1.0]);
    assert!(q.pixels[(row * q.width + 50) as usize][0] < 0.1, "the 2x grid missed the rule");

    // And both rule the sheet's own edge, so neither looks like it stopped
    // a line short.
    assert!(p.pixels[(row * p.width) as usize][0] < 0.6, "the left edge is not ruled");
}

/// A border puts its ink INSIDE the sheet: half a border off the paper is
/// half a border.
#[test]
fn test_a_border_stays_on_the_paper() {
    use crate::page::Page;
    let mut p = Page::new([2.0, 2.0], 100, [1.0, 1.0, 1.0, 1.0]);
    p.border(0.25, 0.0, [0.0, 0.0, 0.0, 1.0]);
    let at = |x: u32, y: u32| p.pixels[(y * p.width + x) as usize][0];
    assert!(at(0, 100) < 0.1, "the outermost pixel is not inked");
    assert!(at(24, 100) < 0.1, "the border is thinner than asked");
    assert!(at(30, 100) > 0.9, "the border is thicker than asked");
    assert!(at(100, 100) > 0.9, "the border filled the page");
    // All four sides, not just the two that a copy-paste would reach.
    assert!(at(199, 100) < 0.1 && at(100, 0) < 0.1 && at(100, 199) < 0.1, "a side is missing");
}

/// The PNG carries the physical size, so a printer lays the sheet out at
/// the size it was composed at instead of guessing 96 DPI.
#[test]
fn test_the_png_knows_its_own_physical_size() {
    use crate::page::Page;
    let dir = std::env::temp_dir()
        .join(format!("cce-designer-page-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sheet.png");
    let p = Page::new([8.5, 11.0], 300, [1.0, 1.0, 1.0, 1.0]);
    p.write_png(&path).expect("write");

    let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
    let reader = decoder.read_info().unwrap();
    let info = reader.info();
    assert_eq!((info.width, info.height), (2550, 3300));
    let dims = info.pixel_dims.expect("no pHYs chunk: the printer would guess");
    assert!(matches!(dims.unit, png::Unit::Meter));
    // pHYs is pixels per METRE, the only unit PNG offers, so the DPI
    // round-trips through a conversion and comes back a hair off.
    let dpi = dims.xppu as f32 / 39.370_08;
    assert!((dpi - 300.0).abs() < 0.01, "the PNG says {dpi} DPI, not 300");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_a_thin_slab_is_solid_all_the_way_through() {
    // The flood fill decides what is enclosed, and it must not be able to
    // walk THROUGH a wall. A slab only a few voxels thick is where that
    // goes wrong: at a band narrower than a voxel, two adjacent samples
    // straddling the surface can both read as "far", the flood steps
    // between them, and the slab comes back hollow — which a boolean
    // against it then fails to cut with.
    let slab = box_mesh(Vec3::new(-1.0, -0.1, -1.0), Vec3::new(1.0, 0.1, 1.0));
    assert!(slab.is_closed());
    let vol = Volume::from_mesh(&slab, 0.05, 0.15);
    let [nx, ny, nz] = vol.dims();

    let mut inside_wrong = Vec::new();
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let p = vol.sample_position(i, j, k);
                let deep = p.x.abs() < 0.8 && p.z.abs() < 0.8 && p.y.abs() < 0.04;
                if deep && vol.at(i, j, k) >= 0.0 {
                    inside_wrong.push((p, vol.at(i, j, k)));
                }
            }
        }
    }
    assert!(
        inside_wrong.is_empty(),
        "{} samples inside the slab read as outside, e.g. {:?}",
        inside_wrong.len(),
        &inside_wrong[..inside_wrong.len().min(3)]
    );

    // And it cuts: subtracting the slab from a box that contains it leaves
    // a gap where the slab was.
    let block = box_mesh(Vec3::splat(-0.6), Vec3::splat(0.6));
    let (lo, hi) = (Vec3::splat(-1.3), Vec3::splat(1.3));
    let mut vb = Volume::build(&block, lo, hi, 0.05, 0.15);
    let vs = Volume::build(&slab, lo, hi, 0.05, 0.15);
    vb.subtract(&vs);
    let out = vb.to_mesh();
    let survivors = (0..out.num_points())
        .map(|p| out.pos(p))
        .filter(|q| q.y.abs() < 0.06 && q.x.abs() < 0.4 && q.z.abs() < 0.4)
        .count();
    assert_eq!(survivors, 0, "points survive where the slab cut through");

    // And an INSIDE-OUT input gives the same field. The band test asks the
    // nearest face which way it points, so a mesh wound the other way
    // would otherwise come back riddled with holes — which is how the
    // winding measurement got written.
    let mut flipped = Detail::new();
    for p in 0..slab.num_points() {
        flipped.add_point(slab.pos(p));
    }
    for prim in 0..slab.num_prims() {
        let mut pts = slab.prim_points(prim).to_vec();
        pts.reverse();
        flipped.add_prim(&pts);
    }
    let inverted = Volume::build(&flipped, lo, hi, 0.05, 0.15);
    let mut differ = 0;
    for k in 0..vs.dims()[2] {
        for j in 0..vs.dims()[1] {
            for i in 0..vs.dims()[0] {
                if (vs.at(i, j, k) < 0.0) != (inverted.at(i, j, k) < 0.0) {
                    differ += 1;
                }
            }
        }
    }
    assert_eq!(differ, 0, "{differ} samples disagree when the input is wound inside out");
}

#[test]
fn test_offsetting_is_subtraction() {
    let sphere = sphere_detail(Vec3::ZERO, 1.0, 14, 20);
    let radius = |d: &Detail| {
        (0..d.num_points()).map(|p| d.pos(p).length()).sum::<f32>() / d.num_points() as f32
    };

    let mut grown = Volume::from_mesh(&sphere, 0.15, 0.6);
    grown.offset(0.3);
    let out = grown.to_mesh();
    assert!(
        (radius(&out) - 1.3).abs() < 0.12,
        "a 0.3 offset should give radius 1.3, got {}",
        radius(&out)
    );

    // Inward too, which is what a shell's inner wall is.
    let mut shrunk = Volume::from_mesh(&sphere, 0.15, 0.6);
    shrunk.offset(-0.3);
    assert!((radius(&shrunk.to_mesh()) - 0.7).abs() < 0.12);
}

#[test]
fn test_the_booleans_are_a_minimum_and_a_maximum() {
    // Two overlapping spheres, sampled over ONE grid so the operations are
    // elementwise. Sharing the grid is what makes them arithmetic rather
    // than a geometry problem.
    let a = sphere_detail(Vec3::new(-0.35, 0.0, 0.0), 0.8, 14, 20);
    let b = sphere_detail(Vec3::new(0.35, 0.0, 0.0), 0.8, 14, 20);
    let (lo, hi) = (Vec3::splat(-1.6), Vec3::splat(1.6));
    let va = Volume::from_mesh_in(&a, lo, hi, 0.14);
    let vb = Volume::from_mesh_in(&b, lo, hi, 0.14);
    assert!(va.aligned_with(&vb), "the two fields do not share a grid");

    let width = |d: &Detail| d.bounds().map(|(l, h)| h.x - l.x).unwrap_or(0.0);

    let mut u = va.clone();
    u.union(&vb);
    let mut i = va.clone();
    i.intersect(&vb);
    let mut s = va.clone();
    s.subtract(&vb);
    // Extracted ONCE each: surface extraction is not free, and an
    // assertion message that re-runs it is a slow test nobody runs.
    let (um, im, sm, am) = (u.to_mesh(), i.to_mesh(), s.to_mesh(), va.to_mesh());

    // The union spans both, the intersection is the lens between them, and
    // the difference is narrower than the whole of A.
    assert!(width(&um) > 2.2, "union is {}", width(&um));
    assert!(width(&im) < 1.0, "intersection is {}", width(&im));
    assert!(width(&sm) < width(&am) + 0.01, "the difference grew");
    // Every result is still a closed surface — which a mesh boolean has to
    // work for and a field gets for free.
    for m in [&um, &im, &sm] {
        assert!(m.num_prims() > 50);
    }

    // Fields on different grids refuse to combine rather than reading each
    // other's memory in the wrong order.
    let elsewhere = Volume::from_mesh_in(&b, lo, hi, 0.25);
    assert!(!va.aligned_with(&elsewhere));
    let mut guarded = va.clone();
    guarded.union(&elsewhere);
    assert_eq!(
        guarded.to_mesh().num_points(),
        am.num_points(),
        "a mismatched grid was combined"
    );
}
