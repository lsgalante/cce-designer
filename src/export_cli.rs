//! `--export`: a project in, a mesh file out, no window.
//!
//! The counterpart of `--thumbnail`. Both exist for the same reason: work that
//! can only leave the app through a window is work that cannot be scripted,
//! diffed, or checked by a test.

use crate::export::{self, Format};

/// Evaluate `project` and write its geometry to `out`.
///
/// Without `--node` the whole visible scene is written, which is what the
/// viewport shows. With it, that one node's output is written whether or not
/// it is visible — an Export node's input is normally drawn by something else,
/// so the node itself usually has its geometry flag off.
pub fn run(
    project: &std::path::Path,
    out: &std::path::Path,
    frame: Option<i32>,
    node: Option<String>,
    scale: f32,
) -> Result<String, String> {
    // Loaded the same way --thumbnail does: a project is a directory holding
    // state.json, or that file directly.
    let state_file = if project.is_dir() {
        project.join("state.json")
    } else {
        project.to_path_buf()
    };
    let content = std::fs::read_to_string(&state_file)
        .map_err(|e| format!("read {}: {e}", state_file.display()))?;
    let mut proj: crate::app::Project = serde_json::from_str(&content)
        .map_err(|e| format!("parse {}: {e}", state_file.display()))?;
    let templates = crate::app::flatten_node_templates(&crate::app::load_fs_tree());
    crate::app::merge_template_defs(&mut proj.root, &templates);

    let mut ocl_error = None;
    let mut sim_cache = crate::geometry::SimCache::default();
    // Start frame 1, the playbar's default, so a frame number here means what
    // it means in the window — the same contract --thumbnail makes.
    let mut sim = crate::geometry::EvalSim::new(frame.unwrap_or(0), 1, &mut sim_cache);

    let geom = match &node {
        Some(name) => {
            let target = crate::geometry::find_node_by_name(&proj.root, name)
                .ok_or_else(|| format!("no node named '{name}'"))?
                .clone();
            let mut visited = Vec::new();
            crate::geometry::generate_single_node_geometry_with_errors(
                &proj.root,
                &target,
                &mut visited,
                &mut ocl_error,
                &mut sim,
            )
            .ok_or_else(|| format!("'{name}' produced no geometry"))?
        }
        None => crate::geometry::network_sphere_vertices_with_errors(
            &proj.root,
            &proj.root,
            &mut ocl_error,
            &mut sim,
        ),
    };
    if let Some(e) = ocl_error {
        // Non-fatal, like the thumbnail: the rest of the scene still exports,
        // and a silent partial file would be worse than a warning.
        eprintln!("cce-designer --export: OpenCL error (geometry partially skipped): {e}");
    }
    if geom.num_prims() == 0 {
        return Err("the geometry has no primitives".to_string());
    }

    let format = Format::from_path(out);
    let bytes = export::write(&geom, out, format, scale)?;
    Ok(format!(
        "{} points, {} primitives -> {} as {} ({bytes} bytes)",
        geom.num_points(),
        geom.num_prims(),
        out.display(),
        format.label()
    ))
}
