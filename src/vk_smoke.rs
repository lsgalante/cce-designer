//! Smoke test for the ash renderer: opens its own XDG window and drives
//! `VkRenderer` with designer-style 2D primitives — rounded window corners,
//! alpha-blended quads, a circle-clipped quad, a blur-behind plate (negative
//! alpha), and an animated color to prove continuous presentation.
//!
//! Run inside a Wayland session:
//!   cargo run -p cce-designer --bin vk-smoke

#[path = "vk/mod.rs"]
mod vk;

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
use glyphon::cosmic_text::{Attrs, Buffer as TextBuffer, Family, Metrics, Shaping};
use glyphon::{FontSystem, SwashCache};
use vk::{TextSpan, VkRenderer};

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

/// Designer-style test scene in logical coordinates.
fn build_scene(lw: f32, lh: f32, scale: f32, t: f32) -> Vec<Vertex> {
    let mut verts: Vec<Vertex> = Vec::new();

    // Window background (full-surface quad; the shader rounds the window corners).
    verts.extend(quad_vertices(0.0, 0.0, lw, lh, lw, lh, [0.09, 0.09, 0.11, 1.0]));

    // A header bar and a side panel, like the app's chrome.
    verts.extend(quad_vertices(0.0, 0.0, lw, 36.0, lw, lh, [0.13, 0.13, 0.17, 1.0]));
    verts.extend(quad_vertices(0.0, 36.0, 56.0, lh - 36.0, lw, lh, [0.11, 0.11, 0.145, 1.0]));

    // A row of alpha-blended quads.
    let palette = [
        [0.90, 0.35, 0.30, 0.9],
        [0.95, 0.75, 0.25, 0.9],
        [0.35, 0.80, 0.45, 0.9],
        [0.30, 0.55, 0.95, 0.9],
    ];
    for (i, color) in palette.iter().enumerate() {
        let x = 90.0 + i as f32 * 90.0;
        verts.extend(quad_vertices(x, 70.0, 70.0, 70.0, lw, lh, *color));
    }

    // Animated quad: hue-cycled color proves per-frame uploads and presentation.
    let pulse = |phase: f32| 0.5 + 0.5 * (t * 2.0 + phase).sin();
    verts.extend(quad_vertices(
        90.0,
        170.0,
        160.0,
        90.0,
        lw,
        lh,
        [pulse(0.0), pulse(2.1), pulse(4.2), 1.0],
    ));

    // Circle-clipped quad: exercises the clip_circle fragment path. The clip
    // center/radius are in physical pixels (the shader tests clip_position).
    let (ccx, ccy, ccr) = (420.0f32, 215.0f32, 45.0f32);
    let clip = [ccx * scale, ccy * scale, ccr * scale];
    for v in quad_vertices(ccx - 60.0, ccy - 60.0, 120.0, 120.0, lw, lh, [0.85, 0.45, 0.85, 1.0]) {
        verts.push(Vertex { clip_circle: clip, ..v });
    }

    // Blur-behind plate (negative alpha): mixes with the backdrop texture — a 1x1
    // placeholder for now, so it reads as a darkened plate.
    verts.extend(quad_vertices(90.0, 290.0, 375.0, 80.0, lw, lh, [0.45, 0.55, 0.95, -0.55]));

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
            },
            TextSpan {
                buffer: &body_buf,
                left: 66.0 * s,
                top: 44.0 * s,
                scale: s,
                bounds: None,
                // Animated color: proves per-frame vertex rebuilds.
                default_color: [pulse, 0.80, 0.55, 1.0],
            },
            TextSpan {
                buffer: &clipped_buf,
                left: 66.0 * s,
                top: 148.0 * s,
                scale: s,
                bounds: Some([
                    (66.0 * s) as i32,
                    (148.0 * s) as i32,
                    (260.0 * s) as i32,
                    (166.0 * s) as i32,
                ]),
                default_color: [0.70, 0.85, 1.00, 1.0],
            },
        ];
        if let Some(renderer) = &mut app.renderer {
            renderer.prepare_text(&mut font_system, &mut swash_cache, &spans);
            // FIFO present paces this loop to the display's refresh rate.
            renderer.draw_frame(&verts);
        }
    }

    // Tear down the renderer (and its swapchain) before the surface dies.
    app.renderer.take();
}
