//! The scene's light: the Environment node (since 2026-10-02).
//!
//! One sun direction lights BOTH views. The raster pass's flat shading and
//! the smooth bake light by it (`VkRenderer::set_scene_light`,
//! `geometry::shade_factor`), and the path tracer's sky puts its sun there
//! (`RtEnvironment`). Until this the two had a light each, hard-coded and
//! pointing different ways — the raster one, read the right way round,
//! from BELOW — so switching modes moved the lit side of a model.
//!
//! The `environment` node stands at the root, the object level
//! (`context::placement`). The first one there that is not bypassed is the
//! scene's environment; with none, [`Environment::default`] is, which is
//! the template's defaults — so adding the node changes nothing until a
//! row is moved, and bypassing it is how to compare. Its rows evaluate at
//! the current frame, so a sun can move with `$F`.
//!
//! What each view takes from it: the raster pass takes the DIRECTION only —
//! its shading is a fixed 0.55..1 wrap of the surface colour, not a light
//! with a strength. The tracer takes all of it: the sun's colour times its
//! intensity, and the sky's two colours times theirs. Colours are LINEAR,
//! as the tracer reads them, and may be pushed past 1 by an intensity.

use glam::Vec3;

use crate::app::FsNode;
use crate::geometry::{node_param_f32, node_param_vec3, resolve_param_refs};

/// The node type.
pub const ENVIRONMENT: &str = "environment";

/// The template's defaults — `nodes/environment.json` must say the same,
/// which `the_environment_node_lights_both_views` checks. The sun is the
/// tracer's old one, (0.45, 0.75, 0.35), to the nearest degree.
pub const DEFAULT_SUN_AZIMUTH: f32 = 52.0;
pub const DEFAULT_SUN_ELEVATION: f32 = 53.0;
pub const DEFAULT_SUN_INTENSITY: f32 = 8.0;
pub const DEFAULT_SUN_COLOR: [f32; 3] = [1.0, 0.95, 0.85];
pub const DEFAULT_SKY_COLOR: [f32; 3] = [0.72, 0.82, 0.98];
pub const DEFAULT_GROUND_COLOR: [f32; 3] = [0.32, 0.31, 0.35];
pub const DEFAULT_SKY_INTENSITY: f32 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Environment {
    /// Toward the sun, unit length, world space.
    pub sun_direction: Vec3,
    /// The sun's colour times its intensity.
    pub sun_radiance: [f32; 3],
    /// The sky straight up and straight down, times the sky's intensity.
    pub sky_zenith: [f32; 3],
    pub sky_nadir: [f32; 3],
}

impl Default for Environment {
    fn default() -> Self {
        Self::from_rows(
            DEFAULT_SUN_AZIMUTH,
            DEFAULT_SUN_ELEVATION,
            DEFAULT_SUN_COLOR,
            DEFAULT_SUN_INTENSITY,
            DEFAULT_SKY_COLOR,
            DEFAULT_GROUND_COLOR,
            DEFAULT_SKY_INTENSITY,
        )
    }
}

/// The direction an azimuth and an elevation (degrees) name: azimuth turns
/// from +Z toward +X about the up axis, elevation lifts from the horizon
/// toward +Y.
pub fn sun_direction(azimuth: f32, elevation: f32) -> Vec3 {
    let (az, el) = (azimuth.to_radians(), elevation.to_radians());
    Vec3::new(az.sin() * el.cos(), el.sin(), az.cos() * el.cos()).normalize()
}

impl Environment {
    fn from_rows(
        azimuth: f32,
        elevation: f32,
        sun_color: [f32; 3],
        sun_intensity: f32,
        sky: [f32; 3],
        ground: [f32; 3],
        sky_intensity: f32,
    ) -> Self {
        let scale = |c: [f32; 3], k: f32| c.map(|v| (v * k).max(0.0));
        Self {
            sun_direction: sun_direction(azimuth, elevation),
            sun_radiance: scale(sun_color, sun_intensity),
            sky_zenith: scale(sky, sky_intensity),
            sky_nadir: scale(ground, sky_intensity),
        }
    }

    /// An environment node's rows, read as they stand (no expressions).
    pub fn of_node(node: &FsNode) -> Self {
        let color = |name: &str, fallback: [f32; 3]| node_param_vec3(node, name, Vec3::from_array(fallback)).to_array();
        Self::from_rows(
            node_param_f32(node, "sun_azimuth", DEFAULT_SUN_AZIMUTH),
            node_param_f32(node, "sun_elevation", DEFAULT_SUN_ELEVATION),
            color("sun_color", DEFAULT_SUN_COLOR),
            node_param_f32(node, "sun_intensity", DEFAULT_SUN_INTENSITY),
            color("sky_color", DEFAULT_SKY_COLOR),
            color("ground_color", DEFAULT_GROUND_COLOR),
            node_param_f32(node, "sky_intensity", DEFAULT_SKY_INTENSITY),
        )
    }

    /// The scene's environment at `frame`: the root's first environment
    /// node that is not bypassed, its expressions evaluated; the default
    /// when there is none.
    pub fn of_scene(root: &FsNode, frame: i32) -> Self {
        let Some(node) = root
            .children
            .iter()
            .find(|c| c.node_type.eq_ignore_ascii_case(ENVIRONMENT) && !c.bypassed)
        else {
            return Self::default();
        };
        let mut error = None;
        match resolve_param_refs(root, node, frame, &mut error) {
            Some(resolved) => Self::of_node(&resolved),
            None => Self::of_node(node),
        }
    }

    /// What the path tracer is handed.
    pub fn to_rt(&self) -> cce_ui::vk::RtEnvironment {
        cce_ui::vk::RtEnvironment {
            sun_direction: self.sun_direction.to_array(),
            sun_color: self.sun_radiance,
            sky_zenith: self.sky_zenith,
            sky_nadir: self.sky_nadir,
        }
    }
}
