//! Smoke test for the ash renderer: opens its own XDG window and drives
//! `VkRenderer` with designer-style 2D primitives — rounded window corners,
//! alpha-blended quads, a circle-clipped quad, a blur-behind plate (negative
//! alpha), and an animated color to prove continuous presentation.
//!
//! Run inside a Wayland session:
//!   cargo run -p cce-designer --bin vk-smoke

use cce_ui::vk;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_output, delegate_registry, delegate_xdg_shell,
    delegate_xdg_window,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        xdg::{
            window::{Window as XdgWindow, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
    Connection, Proxy, QueueHandle,
};
use calloop_wayland_source::WaylandSource;

use cce_ui::engine::{quad_vertices, Vertex};
use glam::{Mat4, Vec3};
use glyphon::cosmic_text::{Attrs, Buffer as TextBuffer, Family, Metrics, Shaping};
use glyphon::{FontSystem, SwashCache};
use vk::{Frame2D, ImageQuad, RtCamera, RtMaterial, RtTriangle, SceneDraw, TextSpan, Vertex3D, VkRenderer};

struct SmokeApp {
    registry_state: RegistryState,
    output_state: OutputState,
    window: Option<XdgWindow>,
    renderer: Option<VkRenderer>,
    exit: bool,
    configured: bool,
    logical_size: (u32, u32),
    scale: f64,
}

impl SmokeApp {
    fn apply_size(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            let pw = (self.logical_size.0 as f64 * self.scale) as u32;
            let ph = (self.logical_size.1 as f64 * self.scale) as u32;
            renderer.resize(pw, ph);
        }
    }
}

impl CompositorHandler for SmokeApp {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        scale_factor: i32,
    ) {
        surface.set_buffer_scale(scale_factor);
        self.scale = scale_factor as f64;
        self.apply_size();
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for SmokeApp {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl WindowHandler for SmokeApp {
    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &XdgWindow,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        if let (Some(w), Some(h)) = configure.new_size {
            self.logical_size = (w.get(), h.get());
            self.apply_size();
        }
        self.configured = true;
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &XdgWindow) {
        self.exit = true;
    }
}

impl ProvidesRegistryState for SmokeApp {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

delegate_compositor!(SmokeApp);
delegate_output!(SmokeApp);
delegate_xdg_shell!(SmokeApp);
delegate_xdg_window!(SmokeApp);
delegate_registry!(SmokeApp);

/// Shape a line the same way the app does (bundled control-label font).
fn make_buffer(font_system: &mut FontSystem, text: &str, size: f32) -> TextBuffer {
    let mut buffer = TextBuffer::new(font_system, Metrics::new(size, size * 1.4));
    let family = cce_ui::layout::control_label_font_parsed().0;
    buffer.set_size(font_system, Some(2000.0), Some(200.0));
    buffer.set_text(
        font_system,
        text,
        Attrs::new().family(Family::Name(&family)),
        Shaping::Advanced,
    );
    buffer.shape_until_scroll(font_system, true);
    buffer
}

fn quad3(v: &mut Vec<Vertex3D>, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3], col: [f32; 3]) {
    for p in [a, b, c, a, c, d] {
        v.push(Vertex3D { position: p, color: col });
    }
}

/// Colored cube, CCW-from-outside winding (the 3D pipeline culls back faces).
fn cube_verts() -> Vec<Vertex3D> {
    let s = 0.5;
    let mut v = Vec::new();
    quad3(&mut v, [-s, -s, s], [s, -s, s], [s, s, s], [-s, s, s], [0.85, 0.35, 0.30]);
    quad3(&mut v, [s, -s, -s], [-s, -s, -s], [-s, s, -s], [s, s, -s], [0.30, 0.55, 0.90]);
    quad3(&mut v, [s, -s, s], [s, -s, -s], [s, s, -s], [s, s, s], [0.90, 0.75, 0.25]);
    quad3(&mut v, [-s, -s, -s], [-s, -s, s], [-s, s, s], [-s, s, -s], [0.35, 0.80, 0.45]);
    quad3(&mut v, [-s, s, s], [s, s, s], [s, s, -s], [-s, s, -s], [0.80, 0.80, 0.85]);
    quad3(&mut v, [-s, -s, -s], [s, -s, -s], [s, -s, s], [-s, -s, s], [0.45, 0.40, 0.60]);
    v
}

/// Floor grid on the XZ plane, both windings (visible from above and below).
fn grid_verts() -> Vec<Vertex3D> {
    let col = [0.22, 0.22, 0.28];
    let (half, t) = (2.0, 0.01);
    let mut v = Vec::new();
    for i in -4i32..=4 {
        let p = i as f32 * 0.5;
        quad3(&mut v, [-half, 0.0, p - t], [half, 0.0, p - t], [half, 0.0, p + t], [-half, 0.0, p + t], col);
        quad3(&mut v, [-half, 0.0, p + t], [half, 0.0, p + t], [half, 0.0, p - t], [-half, 0.0, p - t], col);
        quad3(&mut v, [p - t, 0.0, -half], [p + t, 0.0, -half], [p + t, 0.0, half], [p - t, 0.0, half], col);
        quad3(&mut v, [p + t, 0.0, -half], [p - t, 0.0, -half], [p - t, 0.0, half], [p + t, 0.0, half], col);
    }
    v
}

/// The designer's viewport-background quad: z=9.99 triggers the shader's
/// far-plane special case, filling the scissor with the (linear) bg color.
fn viewport_bg_verts() -> Vec<Vertex3D> {
    let color = cce_ui::colors::to_linear_rgb([0.10, 0.10, 0.13]);
    let mut v = Vec::new();
    quad3(&mut v, [-1.0, -1.0, 9.99], [1.0, -1.0, 9.99], [1.0, 1.0, 9.99], [-1.0, 1.0, 9.99], color);
    v
}

/// The RT-mode demo scene (`CCE_VK_SMOKE_RT=1`): the reference cube on a gray
/// floor beneath an emissive panel — enough for visible indirect light, color
/// bleed, and soft shadows once the accumulation converges.
fn rt_demo_scene() -> (Vec<RtTriangle>, Vec<RtMaterial>) {
    let mut tris: Vec<RtTriangle> = Vec::new();
    let mut mats: Vec<RtMaterial> = Vec::new();
    let mat = |mats: &mut Vec<RtMaterial>, albedo: [f32; 3], emission: [f32; 3]| -> u32 {
        let m = RtMaterial { albedo, emission };
        if let Some(i) = mats.iter().position(|x| *x == m) {
            i as u32
        } else {
            mats.push(m);
            (mats.len() - 1) as u32
        }
    };

    // The cube, from the same triangle list the raster pass draws.
    for tri in cube_verts().chunks_exact(3) {
        let material = mat(&mut mats, tri[0].color, [0.0; 3]);
        tris.push(RtTriangle { p0: tri[0].position, p1: tri[1].position, p2: tri[2].position, material });
    }

    let quad = |tris: &mut Vec<RtTriangle>, c: [[f32; 3]; 4], material: u32| {
        tris.push(RtTriangle { p0: c[0], p1: c[1], p2: c[2], material });
        tris.push(RtTriangle { p0: c[0], p1: c[2], p2: c[3], material });
    };
    let floor = mat(&mut mats, [0.55, 0.55, 0.58], [0.0; 3]);
    quad(
        &mut tris,
        [[-6.0, -0.5, -6.0], [6.0, -0.5, -6.0], [6.0, -0.5, 6.0], [-6.0, -0.5, 6.0]],
        floor,
    );
    let panel = mat(&mut mats, [0.0; 3], [6.0, 5.6, 5.0]);
    quad(
        &mut tris,
        [[-1.0, 2.2, -1.0], [1.0, 2.2, -1.0], [1.0, 2.2, 1.0], [-1.0, 2.2, 1.0]],
        panel,
    );
    (tris, mats)
}

/// Designer-style 2D chrome in logical coordinates. No full-window background —
/// the 3D viewport (copied in as the backdrop) shows through everywhere the UI
/// doesn't paint, exactly like the app.
fn build_scene(lw: f32, lh: f32, scale: f32, t: f32) -> Vec<Vertex> {
    let mut verts: Vec<Vertex> = Vec::new();

    // Header bar + side panel chrome.
    verts.extend(quad_vertices(0.0, 0.0, lw, 36.0, lw, lh, [0.13, 0.13, 0.17, 1.0]));
    verts.extend(quad_vertices(0.0, 36.0, 56.0, lh - 36.0, lw, lh, [0.11, 0.11, 0.145, 1.0]));

    // Floating alpha-blended quads over the 3D scene.
    let palette = [[0.90, 0.35, 0.30, 0.9], [0.95, 0.75, 0.25, 0.9], [0.35, 0.80, 0.45, 0.9]];
    for (i, color) in palette.iter().enumerate() {
        verts.extend(quad_vertices(70.0 + i as f32 * 50.0, 46.0, 40.0, 40.0, lw, lh, *color));
    }

    // Animated quad: hue-cycled color proves per-frame uploads.
    let pulse = |phase: f32| 0.5 + 0.5 * (t * 2.0 + phase).sin();
    verts.extend(quad_vertices(70.0, 96.0, 80.0, 30.0, lw, lh, [pulse(0.0), pulse(2.1), pulse(4.2), 1.0]));

    // Circle-clipped quad (clip center/radius in physical pixels).
    let (ccx, ccy, ccr) = (320.0f32, 90.0f32, 26.0f32);
    let clip = [ccx * scale, ccy * scale, ccr * scale];
    for v in quad_vertices(ccx - 30.0, ccy - 30.0, 60.0, 60.0, lw, lh, [0.85, 0.45, 0.85, 1.0]) {
        verts.push(Vertex { clip_circle: clip, ..v });
    }

    // Blur-behind plate (negative alpha): blurs the real 3D backdrop beneath it.
    verts.extend(quad_vertices(110.0, 140.0, 200.0, 70.0, lw, lh, [0.45, 0.55, 0.95, -0.55]));

    verts
}

fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    let conn = Connection::connect_to_env().expect("No Wayland display");
    let (globals, event_queue) = registry_queue_init::<SmokeApp>(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh).unwrap();
    let xdg_shell_state = XdgShell::bind(&globals, &qh).unwrap();
    let output_state = OutputState::new(&globals, &qh);

    let mut app = SmokeApp {
        registry_state: RegistryState::new(&globals),
        output_state,
        window: None,
        renderer: None,
        exit: false,
        configured: false,
        logical_size: (720, 460),
        scale: 1.0,
    };

    let mut event_loop = calloop::EventLoop::<SmokeApp>::try_new().unwrap();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(event_loop.handle())
        .unwrap();

    // Roundtrip so outputs (and their scales) are known.
    event_loop.dispatch(std::time::Duration::ZERO, &mut app).unwrap();
    app.scale = cce_ui::wayland::detect_scale_factor(&app.output_state);

    let wl_surface = compositor_state.create_surface(&qh);
    wl_surface.set_buffer_scale(app.scale as i32);
    let window = xdg_shell_state.create_window(wl_surface.clone(), WindowDecorations::None, &qh);
    window.set_title("vk-smoke");
    window.set_app_id("cce-designer-vk-smoke");
    window.set_min_size(Some((360, 240)));
    window.commit();
    app.window = Some(window);

    // Wait for the first configure before creating the swapchain (a Vulkan
    // present attaches a buffer, which is illegal pre-configure).
    while !app.configured && !app.exit {
        event_loop
            .dispatch(std::time::Duration::from_millis(16), &mut app)
            .unwrap();
    }

    let display_ptr = conn.backend().display_id().as_ptr() as *mut std::ffi::c_void;
    let surface_ptr = wl_surface.id().as_ptr() as *mut std::ffi::c_void;
    let pw = (app.logical_size.0 as f64 * app.scale) as u32;
    let ph = (app.logical_size.1 as f64 * app.scale) as u32;
    let radius = cce_ui::color::backplate_corner_radius() * app.scale as f32;
    app.renderer =
        Some(unsafe { VkRenderer::new(display_ptr, surface_ptr, pw, ph, radius) });
    log::info!("vk-smoke: renderer up at {pw}x{ph} (scale {})", app.scale);

    // 3D meshes for the viewport scene.
    let (bg_mesh, grid_mesh, cube_mesh) = {
        let r = app.renderer.as_mut().unwrap();
        (
            r.create_mesh(&viewport_bg_verts()),
            r.create_mesh(&grid_verts()),
            r.create_mesh(&cube_verts()),
        )
    };

    // Text stack: same bundled fonts as the app, shaped once up front.
    let mut font_system = cce_ui::create_font_system();
    let mut swash_cache = SwashCache::new();
    let title_buf = make_buffer(&mut font_system, "vk-smoke — ash text stage", 14.0);
    let body_buf = make_buffer(
        &mut font_system,
        "cosmic-text shaping → swash raster → vulkan glyph atlas",
        13.0,
    );
    let clipped_buf = make_buffer(
        &mut font_system,
        "this line is clipped mid-glyph by span bounds ###########",
        13.0,
    );

    // User image: a checkerboard drawn between the animated quad (under) and
    // the blur plate (over) via z_before.
    let mut px = vec![0u8; 64 * 64 * 4];
    for y in 0..64usize {
        for x in 0..64usize {
            let on = ((x / 8) + (y / 8)) % 2 == 0;
            let i = (y * 64 + x) * 4;
            px[i..i + 4].copy_from_slice(if on { &[240, 90, 60, 255] } else { &[40, 200, 220, 255] });
        }
    }
    let checker = vk::upload_rgba(px, 64, 64);

    // RT mode: the pane runs the phase-2 compute path tracer instead of the
    // raster 3D pass. Static camera, so progressive accumulation visibly
    // converges from noise to a clean render.
    let rt_mode = std::env::var_os("CCE_VK_SMOKE_RT").is_some();
    if rt_mode {
        let (tris, mats) = rt_demo_scene();
        log::info!("vk-smoke: RT mode — {} triangles, {} materials", tris.len(), mats.len());
        app.renderer.as_mut().unwrap().set_rt_scene(&tris, &mats);
    }

    let start = std::time::Instant::now();
    while !app.exit {
        event_loop
            .dispatch(std::time::Duration::from_millis(16), &mut app)
            .unwrap();
        if app.exit {
            break;
        }
        let (lw, lh) = (app.logical_size.0 as f32, app.logical_size.1 as f32);
        let t = start.elapsed().as_secs_f32();
        let s = app.scale as f32;
        let verts = build_scene(lw, lh, s, t);
        let pulse = 0.75 + 0.25 * (t * 3.0).sin();
        let spans = [
            TextSpan {
                buffer: &title_buf,
                left: 12.0 * s,
                top: 9.0 * s,
                scale: s,
                bounds: None,
                default_color: [0.92, 0.92, 0.95, 1.0],
                rotation: None,
                clip_circle: [0.0; 3],
                clip_extents: [0.0; 2],
            },
            TextSpan {
                buffer: &body_buf,
                left: 120.0 * s,
                top: 158.0 * s,
                scale: s,
                bounds: None,
                // Animated color: proves per-frame vertex rebuilds.
                default_color: [pulse, 0.80, 0.55, 1.0],
                rotation: None,
                clip_circle: [0.0; 3],
                clip_extents: [0.0; 2],
            },
            TextSpan {
                buffer: &clipped_buf,
                left: 66.0 * s,
                top: 215.0 * s,
                scale: s,
                bounds: Some([
                    (66.0 * s) as i32,
                    (215.0 * s) as i32,
                    (260.0 * s) as i32,
                    (233.0 * s) as i32,
                ]),
                default_color: [0.70, 0.85, 1.00, 1.0],
                rotation: None,
                clip_circle: [0.0; 3],
                clip_extents: [0.0; 2],
            },
        ];
        // build_scene vertex layout: blur plate is the last 6 verts; the image
        // sorts just before it (above everything else, beneath the plate).
        let image_quads = [ImageQuad {
            image: checker,
            rect: (250.0 * s, 115.0 * s, 90.0 * s, 90.0 * s),
            alpha: 1.0,
            z_before: (verts.len() as u32).saturating_sub(6),
            clip: None,
        }];
        if let Some(renderer) = &mut app.renderer {
            // 3D pane: right of the side panel, below the header (physical px).
            // Full-frame NDC scissored to the pane, exactly like the app.
            let pane = (
                (56.0 * s) as u32,
                (36.0 * s) as u32,
                ((lw - 56.0) * s) as u32,
                ((lh - 36.0) * s) as u32,
            );
            let aspect = (lw - 56.0) / (lh - 36.0);
            let proj = Mat4::perspective_rh(0.9, aspect, 0.1, 100.0);
            let view = Mat4::look_at_rh(Vec3::new(2.5, 1.8, 2.5), Vec3::ZERO, Vec3::Y);
            if rt_mode {
                let inv_mvp = (proj * view).inverse().to_cols_array_2d();
                renderer.stage_rt(pane, RtCamera { inv_mvp });
            } else {
                let model = Mat4::from_rotation_y(t * 0.8);
                let mvp = (proj * view * model).to_cols_array_2d();
                renderer.stage_scene(
                    pane,
                    vec![
                        SceneDraw { mesh: bg_mesh, mvp: Mat4::IDENTITY.to_cols_array_2d(), wireframe: false, wire_tint: [0.0; 4], opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 },
                        SceneDraw { mesh: grid_mesh, mvp, wireframe: false, wire_tint: [0.0; 4], opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 },
                        SceneDraw { mesh: cube_mesh, mvp, wireframe: false, wire_tint: [0.0; 4], opacity: 1.0, line_width: 1.0, wire_base_width: 0.0 },
                    ],
                );
            }
            renderer.prepare_text(&mut font_system, &mut swash_cache, &spans);
            // FIFO present paces this loop to the display's refresh rate.
            renderer.draw_frame_2d(Frame2D {
                verts: &verts,
                batches: &[],
                overlay_verts: &[],
                images: &image_quads,
                plate_features: &[],
                clear_color: [0.0; 4],
            });
        }
    }

    // Tear down the renderer (and its swapchain) before the surface dies.
    app.renderer.take();
}
