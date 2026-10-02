//! `cce-designer --thumbnail <project> <out.png> [--size N]` — headless
//! path-traced project thumbnails (RT-renderer phase 3).
//!
//! Loads a project's `state.json`, regenerates its geometry from the node
//! graph (the same `network_sphere_vertices_with_errors` the viewport uses,
//! wrangles included), auto-frames a camera on the scene bounds, and
//! renders through `cce_ui::vk::RtOffscreen` — no window, no compositor, any
//! graphics-capable Vulkan device. cce-files shells out to this for its
//! preview cache.

use std::path::Path;

use glam::{Mat4, Vec3};

use crate::app::Project;
use crate::geometry::{network_sphere_vertices_with_errors, rt_scene_from_verts};

const SAMPLES: u32 = 96;

/// Render `project` (a project directory or a `state.json` path) to a square
/// `size`×`size` PNG at `out`, with `samples` paths per pixel (None = 96).
pub fn run(project: &Path, out: &Path, size: u32, samples: Option<u32>, frame: Option<i32>) -> Result<(), String> {
    let state_file = if project.is_dir() { project.join("state.json") } else { project.to_path_buf() };
    let content = std::fs::read_to_string(&state_file)
        .map_err(|e| format!("read {}: {e}", state_file.display()))?;
    let mut proj: Project =
        serde_json::from_str(&content).map_err(|e| format!("parse {}: {e}", state_file.display()))?;
    // Same template merge the app applies on load, so a thumbnail of an old
    // scene shows what opening it would show.
    let templates = crate::app::flatten_node_templates(&crate::app::load_fs_tree());
    proj.sanitize_node_names();
    proj.migrate_format();
    crate::app::merge_template_defs(&mut proj.root, &templates);

    let mut ocl_error = None;
    // Without `--frame`, a headless thumbnail has no timeline and simnets
    // render at their seed. WITH it, the solve runs to that frame — which is
    // the only way to look at a simulation without a Wayland session, and so
    // the only way to check that a growth chain does what it claims.
    //
    // The start frame is the playbar's default of 1, the same number the app
    // uses, so a frame number here means what it means in the window. A simnet
    // with its own Start Frame answers for itself either way.
    let mut sim_cache = crate::geometry::SimCache::default();
    let mut sim = crate::geometry::EvalSim::new(frame.unwrap_or(0), 1, &mut sim_cache);
    // Thumbnails always show the whole scene from the top, regardless of the
    // network level the project was saved at: root as both eval and walk root.
    let geom = network_sphere_vertices_with_errors(&proj.root, &proj.root, &mut ocl_error, &mut sim);
    if let Some(e) = ocl_error {
        // Non-fatal: a failing node just contributes nothing, like the viewport.
        eprintln!("thumbnail: node error (geometry partially skipped): {e}");
    }
    let verts = crate::geometry::detail_vertices(&geom);
    let (tris, mats) = rt_scene_from_verts(&verts);

    // The image the top level shows stands in the scene as it does in the
    // viewport, at its physical size in the project's world unit.
    let page = crate::page::displayed_page(&proj.root, &proj.root);
    let corners = page.as_ref().map(|page| {
        crate::page::PageShown {
            node_id: String::new(),
            size: page.size,
            pixels: (page.width, page.height),
            origin: page.origin,
        }
        .world_corners(world_unit_mm(&proj))
    });
    let rgba = page.as_ref().map(|page| page.to_rgba8());

    let (eye, center, near, far) = view_of(&tris, corners);
    let proj_m = Mat4::perspective_rh(FOV, 1.0, near, far);
    let view_m = Mat4::look_at_rh(eye, center, Vec3::Y);
    let camera =
        cce_ui::vk::RtCamera { inv_mvp: (proj_m * view_m).inverse().to_cols_array_2d() };

    let mut off = cce_ui::vk::RtOffscreen::new();
    let image = match (&page, &rgba, corners) {
        (Some(page), Some(rgba), Some(corners)) => Some(cce_ui::vk::RtImagePixels {
            pixels: rgba,
            width: page.width,
            height: page.height,
            corners,
            opacity: 1.0,
        }),
        _ => None,
    };
    off.set_scene_with_image(&tris, &mats, image);
    // Behind the scene, the Background Color the project was saved with, as
    // the traced viewport shows it; the sky, which still lights the scene,
    // for a save without display settings.
    off.set_background(
        proj.view_state
            .display
            .as_ref()
            .map(|d| cce_ui::colors::to_linear_rgb(d.viewport.bg_color)),
    );
    let pixels = off.render(camera, size, size, samples.unwrap_or(SAMPLES));

    let file = std::fs::File::create(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    crate::page::mark_srgb(&mut encoder);
    let mut writer = encoder.write_header().map_err(|e| format!("png header: {e}"))?;
    writer.write_image_data(&pixels).map_err(|e| format!("png write: {e}"))?;
    writer.finish().map_err(|e| format!("png finish: {e}"))?;
    Ok(())
}

/// The thumbnail camera's vertical field of view.
const FOV: f32 = 0.9;

/// One world unit of the project in millimetres: the unit its display
/// settings name, or the app's own default for a save from before it
/// carried them.
fn world_unit_mm(proj: &Project) -> f32 {
    let name = proj
        .view_state
        .display
        .as_ref()
        .map(|d| d.viewport.world_unit.clone())
        .unwrap_or_else(|| crate::app::ViewportSettings::default().world_unit);
    let unit = cce_ui::units::Unit::parse(&name).unwrap_or(cce_ui::units::Unit::Mm);
    let metric = cce_ui::units::metric();
    cce_ui::units::Len::new(1.0, unit).convert(cce_ui::units::Unit::Mm, &metric).value
}

/// Where the thumbnail is taken from: the eye, what it looks at, and the
/// near and far planes.
///
/// Geometry is seen from a pleasant high diagonal, its bounding sphere fitted
/// to the view, and an image standing with it is inside that sphere. An
/// image ALONE is seen square on and fitted edge to edge: from the diagonal
/// a picture is a slanted sliver of itself, and the thumbnail of a picture
/// is the picture. An empty scene still renders (sky).
pub(crate) fn view_of(
    tris: &[cce_ui::vk::RtTriangle],
    image: Option<[[f32; 3]; 4]>,
) -> (Vec3, Vec3, f32, f32) {
    if let (true, Some(corners)) = (tris.is_empty(), image) {
        let [tl, tr, br, _] = corners.map(Vec3::from_array);
        let center = (tl + br) * 0.5;
        let longest = (tr - tl).length().max((br - tr).length()).max(1e-3);
        let dist = longest / (2.0 * (FOV * 0.5).tan()) * 1.05;
        return (center + Vec3::Z * dist, center, dist * 0.01, dist * 4.0);
    }
    let points = tris
        .iter()
        .flat_map(|t| [t.p0, t.p1, t.p2])
        .chain(image.into_iter().flatten())
        .map(Vec3::from_array);
    let (center, radius) = bounds(points);
    let dist = (radius / (FOV * 0.5).sin()).max(0.5) * 1.15;
    let eye = center + Vec3::new(1.0, 0.65, 1.0).normalize() * dist;
    let near = (dist - radius * 2.0).max(dist * 0.01);
    let far = dist + radius * 4.0 + 1.0;
    (eye, center, near, far)
}

fn bounds(points: impl Iterator<Item = Vec3>) -> (Vec3, f32) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for v in points {
        min = min.min(v);
        max = max.max(v);
    }
    if !min.is_finite() {
        return (Vec3::ZERO, 1.0);
    }
    let center = (min + max) * 0.5;
    let radius = (max - center).length().max(1e-3);
    (center, radius)
}
