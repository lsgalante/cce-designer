//! App-owned copy of the dissolved cce-ui `Viewport3D` (Phase 6ay part 2): the designer
//! is the only consumer — the 3D preview pane of the roster, on the narrow traits
//! wrapped in `Adapted<Viewport3D>` (Phase 6az). The roster keeps it as
//! `Box<dyn WidgetHost>`; `as_any` downcasts reach this model.

use cce_ui::colors;
use cce_ui::widget::*;
use glam::{Mat4, Vec3};

#[derive(Debug, Clone)]
pub struct Viewport3D {
    pub rotation_x: f32,
    pub rotation_y: f32,
    pub zoom: f32,
    /// The point the Default Camera view orbits and looks at — the pivot a
    /// camera NODE carries as its "Pivot" param, for the view that has no
    /// node. The origin until Frame All moves it to the displayed geometry's
    /// centre; the fixed eye ray (2.5, 1.8, 2.5) is taken FROM here, so the
    /// view's direction never changes, only what it is centred on.
    pub pivot: Vec3,
    pub active_camera: String,
    pub bg_color: [f32; 3],
    pub grid_color: [f32; 3],
    pub show_grid: bool,
    pub show_origin: bool,
    pub show_camera_pivot: bool,
    /// Path-traced preview: the pane renders through `cce_ui::vk`'s compute
    /// tracer instead of the raster 3D pass.
    pub rt_mode: bool,

    // Pending rotation to be consumed by the application when not using the default camera
    pub pending_yaw: f32,
    pub pending_pitch: f32,

    // Modifiers state
    ctrl_pressed: bool,
    shift_pressed: bool,
    alt_pressed: bool,

    /// The scroll orbit as cce-ui's `ScrollMotion`, in wheel px — x turns
    /// the yaw, y the pitch — the model the network pane's pan runs on: a
    /// finger tracks 1:1, the lift coasts on the velocity of the finger's
    /// own events, a wheel notch glides. The positions are accumulators;
    /// what moves the camera is how far they moved (`orbit_by_px`).
    orbit: ScrollMotion,
    /// Ctrl-scroll zoom, the same way, on its y axis.
    zoom_motion: ScrollMotion,
    /// A trackpad orbit keeps to one axis once it clearly favours it:
    /// 0 undecided, 1 yaw only, 2 pitch only. Decided per gesture.
    scroll_lock: u8,
    lock_accum: (f32, f32),
    pub scroll_speed: f32,
    /// `input.inertial.inertial_scroll` in config.kdl: off, a lift stops
    /// the orbit dead. How long a coast runs is cce-ui's `scroll_friction`
    /// in input.kdl, as it is for every pane that coasts.
    pub inertial_scroll: bool,
}

/// Radians of orbit per wheel px, and px per wheel notch (a notch turns
/// 0.05 rad, as it always did).
const ORBIT_RAD_PER_PX: f32 = 0.005;
const ORBIT_PX_PER_LINE: f32 = 10.0;
/// Log-zoom per wheel px, and px per notch (0.15 a notch).
const ZOOM_PER_PX: f32 = 0.005;
const ZOOM_PX_PER_LINE: f32 = 30.0;
/// How far a trackpad orbit runs before its axis lock is decided, in px.
const LOCK_DECIDE_PX: f32 = 0.4;

impl Viewport3D {
    /// Hard pitch limit for the orbit: just short of the poles. Past ±90° the
    /// up-vector flips and the view rolls — from there orbiting reads as the
    /// geometry tumbling with the camera instead of the camera moving around it.
    pub const MAX_PITCH: f32 = 89.9 * std::f32::consts::PI / 180.0;

    /// The default camera's base pitch above the horizon (position (2.5,1.8,2.5)
    /// looking at the origin — the `get_matrices` defaults).
    fn default_pitch0() -> f32 {
        (1.8f32 / Vec3::new(2.5, 1.8, 2.5).length()).asin()
    }

    /// Clamp the default-camera scroll orbit short of the poles
    /// (total pitch = pitch0 - rotation_x).
    pub(crate) fn clamp_orbit_pitch(&mut self) {
        let p0 = Self::default_pitch0();
        self.rotation_x = self.rotation_x.clamp(p0 - Self::MAX_PITCH, p0 + Self::MAX_PITCH);
    }

    pub fn new() -> Adapted<Viewport3D> {
        Adapted::new(Self {
            rotation_x: 0.0,
            rotation_y: 0.0,
            zoom: 1.0,
            pivot: Vec3::ZERO,
            active_camera: "Default Camera".to_string(),
            bg_color: [0.10, 0.10, 0.13],
            grid_color: [0.18, 0.18, 0.22],
            show_grid: true,
            show_origin: true,
            show_camera_pivot: true,
            rt_mode: false,
            pending_yaw: 0.0,
            pending_pitch: 0.0,
            ctrl_pressed: false,
            shift_pressed: false,
            alt_pressed: false,
            orbit: ScrollMotion::new(),
            zoom_motion: ScrollMotion::new(),
            scroll_lock: 0,
            lock_accum: (0.0, 0.0),
            scroll_speed: 1.0,
            inertial_scroll: true,
        })
    }

    pub fn with_scroll_speed(mut self, speed: f32) -> Self {
        self.scroll_speed = speed;
        self
    }

    pub fn with_inertial_scroll(mut self, enabled: bool) -> Self {
        self.inertial_scroll = enabled;
        self
    }

    /// Stop every scroll motion in flight: a coast, a notch's glide. What
    /// takes the camera over (a drag, Frame All, a load) calls this.
    pub fn reset_velocity(&mut self) {
        self.orbit = ScrollMotion::new();
        self.zoom_motion = ScrollMotion::new();
        self.scroll_lock = 0;
        self.lock_accum = (0.0, 0.0);
    }

    /// Whether a scroll is still moving the camera on its own — a coast
    /// after the lift, or a wheel notch's glide.
    pub fn is_coasting(&self) -> bool {
        self.orbit.is_animating() || self.zoom_motion.is_animating()
    }

    /// The zoom range. The top is generous because `View 1:1` on a small
    /// world unit needs the camera far out; the far plane follows it.
    pub const MAX_ZOOM: f32 = 400.0;

    /// Trackpad pinch: direct-manipulation camera zoom. `factor` is the
    /// scale change since the last gesture update (engine `handle_pinch`
    /// semantics), applied 1:1 — spreading fingers 2x halves the camera
    /// distance. It does not coast: the runner keeps a pinch's end to
    /// itself. It does stop a ctrl-scroll zoom that is.
    pub fn pinch_zoom(&mut self, factor: f32) {
        if factor <= 0.0 {
            return;
        }
        self.zoom_motion = ScrollMotion::new();
        self.zoom = (self.zoom / factor).clamp(0.05, Self::MAX_ZOOM);
    }

    /// Turn the camera by a scroll's worth of wheel px: the Default Camera's
    /// own orbit, or a camera node's pending one for `tick_frame` to write.
    fn orbit_by_px(&mut self, dx: f32, dy: f32) {
        let (yaw, pitch) = (dx * ORBIT_RAD_PER_PX, dy * ORBIT_RAD_PER_PX);
        if yaw == 0.0 && pitch == 0.0 {
            return;
        }
        if self.active_camera != "Default Camera" {
            self.pending_yaw += yaw;
            self.pending_pitch -= pitch;
        } else {
            self.rotation_y += yaw;
            self.rotation_x -= pitch;
            self.clamp_orbit_pitch();
        }
    }

    /// Zoom by a scroll's worth of wheel px.
    fn zoom_by_px(&mut self, dy: f32) {
        if dy != 0.0 {
            self.zoom = (self.zoom * (-dy * ZOOM_PER_PX).exp()).clamp(0.05, Self::MAX_ZOOM);
        }
    }

    pub fn get_matrices(&self, aspect: f32, custom_camera_pos: Option<Vec3>, custom_camera_rot: Option<Vec3>, custom_pivot: Option<Vec3>) -> (Mat4, Mat4, Mat4) {
        let pivot = custom_pivot.unwrap_or(self.pivot);
        let camera_pos = custom_camera_pos.unwrap_or(pivot + Vec3::new(2.5, 1.8, 2.5));
        let rot = custom_camera_rot.unwrap_or(Vec3::ZERO);
        let rx = rot.x;
        let ry = rot.y;
        let rz = rot.z;

        let base_offset = camera_pos - pivot;
        let distance = base_offset.length();
        let yaw0 = base_offset.x.atan2(base_offset.z);
        let pitch0 = (base_offset.y / distance.max(1e-5)).asin();

        // The default-camera scroll orbit (`rotation_x`/`rotation_y`) folds into
        // the CAMERA's orbit around the pivot — subtracted, because moving the
        // camera one way spins the view the way rotating the world the other way
        // used to. The old path put these angles in the model matrix, which
        // rotated the geometry within world space (visible against the pivot
        // marker, and it swung the shading) instead of moving the camera.
        let total_ry = ry.to_radians() + yaw0 - self.rotation_y;
        // Safety clamp short of the poles regardless of what the stored camera
        // state says: past ±90° the up-vector flips and the whole view rolls.
        let total_rx =
            (rx.to_radians() + pitch0 - self.rotation_x).clamp(-Self::MAX_PITCH, Self::MAX_PITCH);

        let view_rot_pos = Mat4::from_rotation_y(total_ry) * Mat4::from_rotation_x(-total_rx);
        let camera_up = view_rot_pos.transform_vector3(Vec3::Y);
        let camera_world_pos = pivot + view_rot_pos.transform_vector3(Vec3::new(0.0, 0.0, distance) * self.zoom);
        let view_mat = Mat4::from_rotation_z(rz.to_radians()) * Mat4::look_at_rh(camera_world_pos, pivot, camera_up);

        // Geometry stays stationary in world space; the camera does the moving.
        let model = Mat4::IDENTITY;
        // The far plane follows the camera out: `View 1:1` on a millimetre
        // world unit parks the camera a few hundred units away, and a fixed
        // 100 would clip the pivot itself.
        let far = (distance * self.zoom * 4.0).max(100.0);
        let proj = Mat4::perspective_rh(0.9, aspect, 0.1, far);

        (proj, view_mat, model)
    }
}

impl cce_ui::widget::Layout for Viewport3D {}

impl cce_ui::widget::Paint for Viewport3D {
    fn color(&self) -> [f32; 4] {
        colors::VIEWPORT_BG
    }
}

impl cce_ui::widget::Input for Viewport3D {
    fn set_modifiers(&mut self, ctrl: bool, shift: bool, alt: bool) {
        self.ctrl_pressed = ctrl;
        self.shift_pressed = shift;
        self.alt_pressed = alt;
    }

    fn on_event(&mut self, event: &Event, _ectx: &mut cce_ui::widget::EventCtx) -> bool {
        // Wheel arrives hit-gated to the pane rect. Every wheel goes through
        // a ScrollMotion, whose phase (published by the runner from the
        // Wayland axis source and stop) decides what it is: a finger's
        // motion, the finger's lift, or a notch. The lift is a zero delta
        // and must reach `apply_px` as one — the hand-rolled coast this
        // replaced read it as more motion and killed its own velocity on
        // every lift, so the viewport never coasted.
        let Event::MouseWheel { delta, .. } = event else { return false };
        self.wheel(delta, cce_ui::widget::scroll_motion::current_scroll_phase())
    }

    /// Advance a coast or a notch's glide. True while either still moves,
    /// which keeps the frames coming.
    fn tick(&mut self, dt: f32, _rect: cce_ui::scene::layout::Rect) -> bool {
        let free = Bounds::UNBOUNDED;
        let (ox, oy) = (self.orbit.x.pos(), self.orbit.y.pos());
        let mut changed = self.orbit.tick(dt, free, free);
        if changed {
            self.orbit_by_px(self.orbit.x.pos() - ox, self.orbit.y.pos() - oy);
        }
        let z = self.zoom_motion.y.pos();
        if self.zoom_motion.tick(dt, free, free) {
            self.zoom_by_px(self.zoom_motion.y.pos() - z);
            changed = true;
        }
        changed || self.is_coasting()
    }
}

impl Viewport3D {
    /// One wheel event in the gesture phase `phase` (the runner's, for a
    /// real event — see `on_event`): ctrl zooms, otherwise it orbits. A
    /// notch is always `Wheel`, whatever the phase says.
    pub fn wheel(&mut self, delta: &MouseScrollDelta, phase: ScrollPhase) -> bool {
        let scale = cce_ui::scale::scale_factor().max(0.001);
        let discrete = matches!(delta, MouseScrollDelta::LineDelta(..));
        let phase = if discrete { ScrollPhase::Wheel } else { phase };
        let free = Bounds::UNBOUNDED;
        let speed = self.scroll_speed;
        if self.ctrl_pressed {
            let dy = match delta {
                MouseScrollDelta::LineDelta(_x, y) => *y * ZOOM_PX_PER_LINE,
                MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / scale,
            } * speed;
            let before = self.zoom_motion.y.pos();
            self.zoom_motion.apply_phase(phase, 0.0, dy, discrete, free, free);
            if phase == ScrollPhase::FingerEnd && !self.inertial_scroll {
                self.zoom_motion = ScrollMotion::new();
                return true;
            }
            self.zoom_by_px(self.zoom_motion.y.pos() - before);
            return true;
        }
        let (mut dx, mut dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (*x * ORBIT_PX_PER_LINE, *y * ORBIT_PX_PER_LINE),
            MouseScrollDelta::PixelDelta(pos) => (pos.x as f32 / scale, pos.y as f32 / scale),
        };
        dx *= speed;
        dy *= speed;
        match phase {
            ScrollPhase::Finger => {
                // Once a trackpad orbit clearly favours one axis it keeps to
                // it, so a vertical swipe does not wander in yaw.
                if self.scroll_lock == 0 {
                    self.lock_accum.0 += dx;
                    self.lock_accum.1 += dy;
                    let (ax, ay) = (self.lock_accum.0.abs(), self.lock_accum.1.abs());
                    if ax > LOCK_DECIDE_PX || ay > LOCK_DECIDE_PX {
                        if ay > 1.2 * ax {
                            self.scroll_lock = 2;
                        } else if ax > 1.2 * ay {
                            self.scroll_lock = 1;
                        }
                    }
                }
                match self.scroll_lock {
                    1 => dy = 0.0,
                    2 => dx = 0.0,
                    _ => {}
                }
            }
            ScrollPhase::FingerEnd | ScrollPhase::Wheel => {
                self.scroll_lock = 0;
                self.lock_accum = (0.0, 0.0);
            }
        }
        let before = (self.orbit.x.pos(), self.orbit.y.pos());
        self.orbit.apply_phase(phase, dx, dy, discrete, free, free);
        if phase == ScrollPhase::FingerEnd && !self.inertial_scroll {
            self.orbit = ScrollMotion::new();
            return true;
        }
        self.orbit_by_px(self.orbit.x.pos() - before.0, self.orbit.y.pos() - before.1);
        true
    }
}
