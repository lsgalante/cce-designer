
pub mod app;
pub mod param;
pub mod edit_history;
pub mod application;
pub mod curve_tool;
pub mod soft_transform_tool;
pub mod viewer_state;
pub mod detail;
pub mod export;
pub mod export_cli;
pub mod remesh;
pub mod spatial;
pub mod detangle;
pub mod volume;
pub mod wrangle;
pub mod shapes;
pub mod gpu;
pub mod springs;
pub mod collide;
pub mod surface_flow;

// Root-level aliases some modules import via `crate::` paths.
#[allow(unused_imports)]
use app::{CustomEvent, McpAction, ModifiersState};
pub mod plate_menu;
pub mod menu_page;
pub mod visualizer;
pub mod playbar;
pub mod viewport_3d;
pub mod api;
pub mod window;
pub mod geometry;
pub mod context;
pub mod environment;
pub mod project;
pub mod render;
pub mod shortcut;
pub mod slots;
pub mod command;
pub mod dialog;
pub mod layout;
pub mod panes;
pub mod mold;
pub mod hull;
pub mod scatter;
pub mod page;
pub mod image_tools;
pub mod image_handles;
pub mod expr;
pub mod thumbnail;

#[cfg(test)]
mod test_prelude {
    pub use std::fs;
    pub use std::path::Path;
    pub use glam::{Mat4, Vec3};
    pub use cce_ui::widget::{Key, NamedKey};
    pub use crate::app::{State, McpAction, ModifiersState};

    /// The root's first Geometry node: where a loaded save's geometry
    /// stands since the root became the object level (format 5).
    pub fn geo(root: &crate::app::FsNode) -> &crate::app::FsNode {
        root.children
            .iter()
            .find(|c| crate::context::is_geometry_container(&c.node_type))
            .expect("a geometry node at the root")
    }

    pub fn geo_mut(root: &mut crate::app::FsNode) -> &mut crate::app::FsNode {
        root.children
            .iter_mut()
            .find(|c| crate::context::is_geometry_container(&c.node_type))
            .expect("a geometry node at the root")
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Headless thumbnail mode: no Wayland, no window — render and exit.
    //   cce-designer --thumbnail <project-dir-or-state.json> <out.png> [--size N]
    if let Some(i) = args.iter().position(|a| a == "--thumbnail") {
        let _ = env_logger::try_init();
        let (Some(project), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: cce-designer --thumbnail <project> <out.png> [--size N] [--samples N] [--frame N]");
            std::process::exit(2);
        };
        let size = args
            .windows(2)
            .find(|w| w[0] == "--size")
            .and_then(|w| w[1].parse::<u32>().ok())
            .unwrap_or(256)
            .clamp(16, 2048);
        let samples = args
            .windows(2)
            .find(|w| w[0] == "--samples")
            .and_then(|w| w[1].parse::<u32>().ok());
        let frame = args
            .windows(2)
            .find(|w| w[0] == "--frame")
            .and_then(|w| w[1].parse::<i32>().ok());
        match thumbnail::run(std::path::Path::new(project), std::path::Path::new(out), size, samples, frame) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("cce-designer --thumbnail: {e}");
                std::process::exit(1);
            }
        }
    }

    // Headless export: evaluate a project and write its geometry to a file.
    //   cce-designer --export <project> <out.stl|out.obj> [--frame N] [--node NAME] [--scale S]
    //
    // The counterpart of --thumbnail, and the same argument for existing: work
    // that can only leave the app through a window is work that cannot be
    // scripted, diffed or checked.
    if let Some(i) = args.iter().position(|a| a == "--export") {
        let _ = env_logger::try_init();
        let (Some(project), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!(
                "usage: cce-designer --export <project> <out.stl|out.obj> [--frame N] [--node NAME] [--scale S]"
            );
            std::process::exit(2);
        };
        let flag = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
        let frame = flag("--frame").and_then(|v| v.parse::<i32>().ok());
        let scale = flag("--scale").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        match export_cli::run(
            std::path::Path::new(project),
            std::path::Path::new(out),
            frame,
            flag("--node"),
            scale,
        ) {
            Ok(msg) => {
                println!("{msg}");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("cce-designer --export: {e}");
                std::process::exit(1);
            }
        }
    }

    // The GPU setting, as CCE_VK_DEVICE for the renderer the engine is about
    // to create. Only here: the thumbnail and export modes above exit first,
    // so cce-files' preview cache never wakes a discrete GPU.
    app::apply_gpu_preference();

    // Everything windowed runs on the cce-ui engine (application.rs holds the
    // Application impl; --detached-network is read there).
    cce_ui::engine::run::<app::State>();
}

#[cfg(test)]
mod tests;
