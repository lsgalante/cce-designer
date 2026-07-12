//! App-owned copy of the dissolved cce-ui `Viewport3D` (Phase 6ay part 2): the designer
//! is the only consumer — the 3D preview pane of the roster. Verbatim from cce-ui; the
//! `Element` impl dies with the machinery retype.

use cce_ui::colors;
use cce_ui::widget::*;
use glam::{Mat4, Vec3};

#[derive(Debug, Clone)]
pub struct Viewport3D {
    base: Widget,
    pub rotation_x: f32,
    pub rotation_y: f32,
    pub zoom: f32,
    pub active_camera: String,
    pub bg_color: [f32; 3],
    pub grid_color: [f32; 3],
    pub show_grid: bool,
    pub show_cube: bool,
    pub show_origin: bool,
    pub show_camera_pivot: bool,

    // Pending rotation to be consumed by the application when not using the default camera
    pub pending_yaw: f32,
    pub pending_pitch: f32,

    // Modifiers state
    ctrl_pressed: bool,
    shift_pressed: bool,
    alt_pressed: bool,

    // Drag & scroll state tracking
    is_rotating: bool,
    is_zooming: bool,
    scroll_lock: u8,
    rotate_accum_yaw: f32,
    rotate_accum_pitch: f32,
    zoom_accum: f32,
    rotate_velocity_yaw: f32,
    rotate_velocity_pitch: f32,
    zoom_velocity: f32,
    last_rotate_time: std::time::Instant,
    last_zoom_time: std::time::Instant,
    pub scroll_speed: f32,
    pub inertial_scroll: bool,
    pub scroll_friction: f32,
}

impl Viewport3D {
    pub fn new() -> Self {
        let base = Widget::new();
        Self {
            base,
            rotation_x: 0.0,
            rotation_y: 0.0,
            zoom: 1.0,
            active_camera: "Default Camera".to_string(),
            bg_color: [0.10, 0.10, 0.13],
            grid_color: [0.18, 0.18, 0.22],
            show_grid: true,
            show_cube: true,
            show_origin: true,
            show_camera_pivot: true,
            pending_yaw: 0.0,
            pending_pitch: 0.0,
            ctrl_pressed: false,
            shift_pressed: false,
            alt_pressed: false,
            is_rotating: false,
            is_zooming: false,
            scroll_lock: 0,
            rotate_accum_yaw: 0.0,
            rotate_accum_pitch: 0.0,
            zoom_accum: 0.0,
            rotate_velocity_yaw: 0.0,
            rotate_velocity_pitch: 0.0,
            zoom_velocity: 0.0,
            last_rotate_time: std::time::Instant::now(),
            last_zoom_time: std::time::Instant::now(),
            scroll_speed: 1.0,
            inertial_scroll: true,
            scroll_friction: 0.90,
        }
    }

    pub fn with_scroll_speed(mut self, speed: f32) -> Self {
        self.scroll_speed = speed;
        self
    }

    pub fn with_inertial_scroll(mut self, enabled: bool) -> Self {
        self.inertial_scroll = enabled;
        self
    }

    pub fn with_scroll_friction(mut self, friction: f32) -> Self {
        self.scroll_friction = friction;
        self
    }

    pub fn reset_velocity(&mut self) {
        self.rotate_velocity_yaw = 0.0;
        self.rotate_velocity_pitch = 0.0;
        self.zoom_velocity = 0.0;
        self.is_rotating = false;
        self.is_zooming = false;
        self.scroll_lock = 0;
    }

    pub fn get_matrices(&self, aspect: f32, custom_camera_pos: Option<Vec3>, custom_camera_rot: Option<Vec3>, custom_pivot: Option<Vec3>) -> (Mat4, Mat4, Mat4) {
        let camera_pos = custom_camera_pos.unwrap_or(Vec3::new(2.5, 1.8, 2.5));
        let pivot = custom_pivot.unwrap_or(Vec3::ZERO);
        let rot = custom_camera_rot.unwrap_or(Vec3::ZERO);
        let rx = rot.x;
        let ry = rot.y;
        let rz = rot.z;

        let base_offset = camera_pos - pivot;
        let distance = base_offset.length();
        let yaw0 = base_offset.x.atan2(base_offset.z);
        let pitch0 = (base_offset.y / distance.max(1e-5)).asin();

        let total_ry = ry.to_radians() + yaw0;
        let total_rx = rx.to_radians() + pitch0;

        let view_rot_pos = Mat4::from_rotation_y(total_ry) * Mat4::from_rotation_x(-total_rx);
        let camera_up = view_rot_pos.transform_vector3(Vec3::Y);
        let camera_world_pos = pivot + view_rot_pos.transform_vector3(Vec3::new(0.0, 0.0, distance) * self.zoom);
        let view_mat = Mat4::from_rotation_z(rz.to_radians()) * Mat4::look_at_rh(camera_world_pos, pivot, camera_up);

        let model = Mat4::from_rotation_y(self.rotation_y) * Mat4::from_rotation_x(self.rotation_x);
        let proj = Mat4::perspective_rh(0.9, aspect, 0.1, 100.0);

        (proj, view_mat, model)
    }
}

impl Element for Viewport3D {
    cce_ui::impl_widget_base!(Viewport3D);

    fn color(&self) -> [f32; 4] {
        colors::VIEWPORT_BG
    }

    fn set_modifiers(&mut self, ctrl: bool, shift: bool, alt: bool) {
        self.ctrl_pressed = ctrl;
        self.shift_pressed = shift;
        self.alt_pressed = alt;
    }

    fn hit_test(&self, px: f32, py: f32, _ctx: &UiContext) -> bool {
        let (x, y, w, h) = self.rect();
        px >= x && px <= x + w && py >= y && py <= y + h
    }

    fn mouse_wheel(&mut self, delta: &MouseScrollDelta, px: f32, py: f32, ctx: &mut UiContext) -> bool {
        if !self.hit_test(px, py, ctx) {
            return false;
        }
        let scale = cce_ui::scale::scale_factor();
        if self.ctrl_pressed {
            match delta {
                MouseScrollDelta::LineDelta(_x, y) => {
                    let dy = *y * 0.15 * self.scroll_speed;
                    self.zoom *= (-dy).exp();
                    self.zoom = self.zoom.clamp(0.05, 20.0);
                    self.is_zooming = false;
                    
                    let dt = 0.016;
                    self.zoom_velocity = self.zoom_velocity * 0.4 + (dy / dt) * 0.6;
                    true
                }
                MouseScrollDelta::PixelDelta(pos) => {
                    let dy = (pos.y as f32 / scale) * 0.005 * self.scroll_speed;
                    self.zoom *= (-dy).exp();
                    self.zoom = self.zoom.clamp(0.05, 20.0);
                    self.is_zooming = true;
                    self.last_zoom_time = std::time::Instant::now();
                    self.zoom_accum += dy;
                    true
                }
            }
        } else {
            match delta {
                MouseScrollDelta::LineDelta(x, y) => {
                    self.scroll_lock = 0;
                    let dx = *x * 0.05 * self.scroll_speed;
                    let dy = *y * 0.05 * self.scroll_speed;
                    
                    if self.active_camera != "Default Camera" {
                        self.pending_yaw += dx;
                        self.pending_pitch += -dy;
                    } else {
                        self.rotation_y += dx;
                        self.rotation_x -= dy;
                    }
                    
                    self.is_rotating = false;
                    let dt = 0.016;
                    self.rotate_velocity_yaw = self.rotate_velocity_yaw * 0.4 + (dx / dt) * 0.6;
                    self.rotate_velocity_pitch = self.rotate_velocity_pitch * 0.4 + (-dy / dt) * 0.6;
                    true
                }
                MouseScrollDelta::PixelDelta(pos) => {
                    let mut dx = (pos.x as f32 / scale) * 0.005 * self.scroll_speed;
                    let mut dy = (pos.y as f32 / scale) * 0.005 * self.scroll_speed;

                    self.rotate_accum_yaw += dx;
                    self.rotate_accum_pitch -= dy;
                    if self.scroll_lock == 0 {
                        if self.rotate_accum_yaw.abs() > 0.002 || self.rotate_accum_pitch.abs() > 0.002 {
                            if self.rotate_accum_pitch.abs() > 1.2 * self.rotate_accum_yaw.abs() {
                                self.scroll_lock = 2;
                            } else if self.rotate_accum_yaw.abs() > 1.2 * self.rotate_accum_pitch.abs() {
                                self.scroll_lock = 1;
                            }
                        }
                    } else {
                        if self.scroll_lock == 1 {
                            dy = 0.0;
                        } else {
                            dx = 0.0;
                        }
                    }

                    if self.active_camera != "Default Camera" {
                        self.pending_yaw += dx;
                        self.pending_pitch += -dy;
                    } else {
                        self.rotation_y += dx;
                        self.rotation_x -= dy;
                    }

                    self.is_rotating = true;
                    self.last_rotate_time = std::time::Instant::now();
                    true
                }
            }
        }
    }

    fn tick(&mut self, dt: f32, _ctx: &mut UiContext) -> bool {
        let now = std::time::Instant::now();
        let mut changed = false;

        if self.is_rotating {
            if now.duration_since(self.last_rotate_time).as_secs_f32() > 0.05 {
                self.is_rotating = false;
                self.scroll_lock = 0;
            } else if dt > 1e-5 {
                let vel_yaw = self.rotate_accum_yaw / dt;
                let vel_pitch = self.rotate_accum_pitch / dt;
                self.rotate_velocity_yaw = self.rotate_velocity_yaw * 0.4 + vel_yaw * 0.6;
                self.rotate_velocity_pitch = self.rotate_velocity_pitch * 0.4 + vel_pitch * 0.6;
            }
            self.rotate_accum_yaw = 0.0;
            self.rotate_accum_pitch = 0.0;
        }

        if self.is_zooming {
            if now.duration_since(self.last_zoom_time).as_secs_f32() > 0.05 {
                self.is_zooming = false;
            } else if dt > 1e-5 {
                let vel_zoom = self.zoom_accum / dt;
                self.zoom_velocity = self.zoom_velocity * 0.4 + vel_zoom * 0.6;
            }
            self.zoom_accum = 0.0;
        }

        if !self.is_rotating && (self.rotate_velocity_yaw.abs() > 0.001 || self.rotate_velocity_pitch.abs() > 0.001) {
            if !self.inertial_scroll {
                self.rotate_velocity_yaw = 0.0;
                self.rotate_velocity_pitch = 0.0;
            } else {
                let dx = self.rotate_velocity_yaw * dt;
                let dy = self.rotate_velocity_pitch * dt;
                
                if self.active_camera != "Default Camera" {
                    self.pending_yaw += dx;
                    self.pending_pitch += dy;
                } else {
                    self.rotation_y += dx;
                    self.rotation_x += dy;
                }

                let decay = self.scroll_friction.powf(dt * 60.0);
                self.rotate_velocity_yaw *= decay;
                self.rotate_velocity_pitch *= decay;

                if self.rotate_velocity_yaw.abs() < 0.01 { self.rotate_velocity_yaw = 0.0; }
                if self.rotate_velocity_pitch.abs() < 0.01 { self.rotate_velocity_pitch = 0.0; }
                changed = true;
            }
        }

        if !self.is_zooming && self.zoom_velocity.abs() > 0.001 {
            if !self.inertial_scroll {
                self.zoom_velocity = 0.0;
            } else {
                let d_zoom = self.zoom_velocity * dt;
                self.zoom *= (-d_zoom).exp();
                self.zoom = self.zoom.clamp(0.05, 20.0);

                let decay = self.scroll_friction.powf(dt * 60.0);
                self.zoom_velocity *= decay;
                if self.zoom_velocity.abs() < 0.01 { self.zoom_velocity = 0.0; }
                changed = true;
            }
        }

        changed
    }
}
