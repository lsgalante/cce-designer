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
    proj.migrate_param_refs();
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

    // Frame the scene: bounding sphere fit into a 0.9 rad vertical FOV from a
    // pleasant high-diagonal direction. An empty scene still renders (sky).
    let (center, radius) = bounds(&tris);
    let fov = 0.9f32;
    let dist = (radius / (fov * 0.5).sin()).max(0.5) * 1.15;
    let eye = center + Vec3::new(1.0, 0.65, 1.0).normalize() * dist;
    let near = (dist - radius * 2.0).max(dist * 0.01);
    let far = dist + radius * 4.0 + 1.0;
    let proj_m = Mat4::perspective_rh(fov, 1.0, near, far);
    let view_m = Mat4::look_at_rh(eye, center, Vec3::Y);
    let camera =
        cce_ui::vk::RtCamera { inv_mvp: (proj_m * view_m).inverse().to_cols_array_2d() };

    let mut off = cce_ui::vk::RtOffscreen::new();
    off.set_scene(&tris, &mats);
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

fn bounds(tris: &[cce_ui::vk::RtTriangle]) -> (Vec3, f32) {
    if tris.is_empty() {
        return (Vec3::ZERO, 1.0);
    }
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for t in tris {
        for p in [t.p0, t.p1, t.p2] {
            let v = Vec3::from_array(p);
            min = min.min(v);
            max = max.max(v);
        }
    }
    let center = (min + max) * 0.5;
    let radius = (max - center).length().max(1e-3);
    (center, radius)
}
