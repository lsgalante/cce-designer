//! `State::camera`: the viewport's camera as the app drives it — which
//! camera is looking, the orbit and pan gestures that turn and slide it,
//! the viewer state that takes the pointer from it, and the view the scene
//! was last staged from, which everything the 2D frame draws over the scene
//! is projected through. The camera's pose itself is the viewport widget's
//! (the Default Camera) or a camera node's; this is what the app keeps
//! beside them.

use glam::{Mat4, Vec3};

pub struct CameraView {
    /// The camera the viewport looks through: "Default Camera", or the
    /// name of a camera node at the root (`State::camera_level`). The
    /// viewport widget keeps a copy its wheel routes by, so
    /// `State::set_active_camera` is the only writer of either.
    pub active_camera: String,
    /// An in-flight camera orbit drag: the cursor position the last motion was
    /// measured from. `None` when no orbit drag is running.
    pub orbit_drag: Option<(f32, f32)>,
    /// An in-flight camera PAN drag (middle button, or shift and the left):
    /// the cursor position the last motion was measured from.
    pub pan_drag: Option<(f32, f32)>,
    /// The camera node a pan last wrote, with the Pivot and Position it
    /// wrote IN FULL. The node holds them as text, to four decimals; a pan
    /// is hundreds of small moves, and each read back from the text would
    /// lose what the text could not hold. Used while the node still says
    /// what was written.
    pub pan_exact: Option<(String, Vec3, Vec3)>,
    /// The active curve viewer state (viewport point editing), if any.
    pub viewer_tool: Option<crate::viewer_state::ViewerTool>,
    /// The raster scene's model-view-projection and the viewport pane rect in
    /// LOGICAL px, cached at staging so the 2D pass can project 3D overlays.
    pub last_scene_mvp: Option<Mat4>,
    /// The eye the scene was last staged for, in mesh space, beside the
    /// matrix: what a scene rebuild dims the numbers by until the stage
    /// pass has staged the new geometry.
    pub last_scene_eye: Vec3,
    pub last_scene_view_rect: (f32, f32, f32, f32),
    /// What the numbers' dimming was last worked out for: the view, the
    /// geometry, the opacity, whether the fill is seen through, how many.
    pub number_alpha_key: Option<([u32; 16], u64, u32, bool, usize)>,
}
