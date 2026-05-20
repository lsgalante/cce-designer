use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes};

use clear_ui::layout::Layout;
use clear_ui::widget::{ContentBg, Header, Panel, Sidebar, StatusBar, Widget};

use glyphon::{
    Attrs, Buffer, Cache, FontSystem, Metrics, Resolution, SwashCache, TextArea, TextAtlas,
    TextBounds, TextRenderer, Viewport,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl Vertex {
    const ATTRIBS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x4,
    ];

    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBS,
        }
    }
}

fn quad_vertices(
    x: f32, y: f32, w: f32, h: f32,
    surface_w: f32, surface_h: f32,
    color: [f32; 4],
) -> [Vertex; 6] {
    let x0 = (x / surface_w) * 2.0 - 1.0;
    let y0 = 1.0 - (y / surface_h) * 2.0;
    let x1 = ((x + w) / surface_w) * 2.0 - 1.0;
    let y1 = 1.0 - ((y + h) / surface_h) * 2.0;

    [
        Vertex { position: [x0, y0], color },
        Vertex { position: [x1, y0], color },
        Vertex { position: [x0, y1], color },
        Vertex { position: [x1, y0], color },
        Vertex { position: [x1, y1], color },
        Vertex { position: [x0, y1], color },
    ]
}

fn widget_vertices(w: &dyn Widget, sw: f32, sh: f32) -> Vec<Vertex> {
    let (x, y, ww, h) = w.rect();
    quad_vertices(x, y, ww, h, sw, sh, w.color()).to_vec()
}

fn make_text_buffer(font_system: &mut FontSystem, text: &str, size: f32) -> Buffer {
    let metrics = Metrics::new(size, size * 1.4);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text(font_system, text, Attrs::new(), glyphon::Shaping::Advanced);
    buffer.shape_until_scroll(font_system, true);
    buffer
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    vertex_count: u32,

    widgets: Vec<Box<dyn Widget>>,
    layout: Layout,

    font_system: FontSystem,
    swash_cache: SwashCache,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_viewport: Viewport,

    title_buffer: Buffer,
    status_buffer: Buffer,

    drag_widget: Option<usize>,

    cursor_x: f32,
    cursor_y: f32,

    width: u32,
    height: u32,
}

impl State {
    async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let sw = size.width as f32;
        let sh = size.height as f32;

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .expect("Failed to create surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("GPU Device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                        .using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await
            .expect("Failed to create device");

        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("Failed to get surface config");
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let mut text_atlas = TextAtlas::new(&device, &queue, &cache, config.format);
        let text_renderer = TextRenderer::new(&mut text_atlas, &device, wgpu::MultisampleState::default(), None);

        let mut text_viewport = Viewport::new(&device, &cache);
        text_viewport.update(&queue, Resolution { width: size.width, height: size.height });

        let title_buffer = make_text_buffer(&mut font_system, "Clear Designer", 16.0);
        let status_buffer = make_text_buffer(&mut font_system, "Ready", 12.0);

        let layout = Layout::new(sw, sh);

        let widgets: Vec<Box<dyn Widget>> = vec![
            Box::new(Header::new()),
            Box::new(Sidebar::new(60.0)),
            Box::new(ContentBg::new()),
            Box::new(Panel::new(0.0, 0.0, 320.0, 240.0)),
            Box::new(Panel::new(0.0, 0.0, 240.0, 180.0)),
            Box::new(StatusBar::new()),
        ];

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vertex Buffer"),
            size: 1,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut state = Self {
            window,
            surface,
            device,
            queue,
            config,
            render_pipeline,
            vertex_buffer,
            vertex_count: 0,
            widgets,
            layout,
            font_system,
            swash_cache,
            text_atlas,
            text_renderer,
            text_viewport,
            title_buffer,
            status_buffer,
            drag_widget: None,
            cursor_x: 0.0,
            cursor_y: 0.0,
            width: size.width,
            height: size.height,
        };

        state.apply_layout();
        state.upload_vertices();
        state
    }

    fn apply_layout(&mut self) {
        for (i, node_id) in self.layout.widget_nodes.iter().enumerate() {
            if let Some(widget) = self.widgets.get_mut(i) {
                if widget.is_dragging() {
                    continue;
                }
                let (x, y, w, h) = self.layout.widget_rect(*node_id);
                widget.set_rect(x, y, w, h);
            }
        }
    }

    fn collect_vertices(&self) -> Vec<Vertex> {
        let sw = self.width as f32;
        let sh = self.height as f32;
        let mut verts = Vec::new();
        for w in &self.widgets {
            verts.extend(widget_vertices(w.as_ref(), sw, sh));
        }
        verts
    }

    fn upload_vertices(&mut self) {
        let verts = self.collect_vertices();
        self.vertex_count = verts.len() as u32;
        let data = bytemuck::cast_slice(&verts);
        let needed = data.len() as wgpu::BufferAddress;
        if needed > self.vertex_buffer.size() {
            self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Vertex Buffer"),
                size: needed,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue.write_buffer(&self.vertex_buffer, 0, data);
    }

    fn update_status_text(&mut self, text: &str) {
        self.status_buffer = make_text_buffer(&mut self.font_system, text, 12.0);
    }

    fn prepare_text(&mut self) {
        let Self {
            ref mut text_renderer,
            ref device,
            ref queue,
            ref mut font_system,
            ref mut text_atlas,
            ref mut text_viewport,
            ref mut swash_cache,
            ref title_buffer,
            ref status_buffer,
            width,
            height,
            ..
        } = self;

        let viewport = Resolution { width: *width, height: *height };
        text_viewport.update(queue, viewport);

        let areas = [
            TextArea {
                buffer: title_buffer,
                left: 80.0,
                top: 12.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: *width as i32,
                    bottom: *height as i32,
                },
                default_color: glyphon::Color::rgb(0xcc, 0xcc, 0xd4),
                custom_glyphs: &[],
            },
            TextArea {
                buffer: status_buffer,
                left: 12.0,
                top: *height as f32 - 24.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: *width as i32,
                    bottom: *height as i32,
                },
                default_color: glyphon::Color::rgb(0x55, 0x55, 0x66),
                custom_glyphs: &[],
            },
        ];

        text_renderer
            .prepare(device, queue, font_system, text_atlas, text_viewport, areas, swash_cache)
            .unwrap();
    }

    fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.width = new_size.width;
            self.height = new_size.height;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            self.layout.resize(new_size.width as f32, new_size.height as f32);
            self.apply_layout();
            self.upload_vertices();
        }
    }

    fn handle_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_x = position.x as f32;
                self.cursor_y = position.y as f32;
                let mut changed = false;

                if let Some(idx) = self.drag_widget {
                    if self.widgets[idx].drag_update(self.cursor_x, self.cursor_y) {
                        changed = true;
                    }
                }

                if self.drag_widget.is_none() {
                    for w in &mut self.widgets {
                        if w.cursor_moved(self.cursor_x, self.cursor_y) {
                            changed = true;
                        }
                    }
                }
                changed
            }
            WindowEvent::MouseInput { state: btn_state, button, .. } => {
                if *button != MouseButton::Left { return false; }
                let mut changed = false;

                match btn_state {
                    ElementState::Pressed => {
                        for i in (0..self.widgets.len()).rev() {
                            if self.widgets[i].hit_test(self.cursor_x, self.cursor_y) {
                                if self.widgets[i].mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y) {
                                    changed = true;
                                }
                                if self.widgets[i].draggable() {
                                    self.widgets[i].drag_begin(self.cursor_x, self.cursor_y);
                                    self.drag_widget = Some(i);
                                }
                                break;
                            }
                        }
                    }
                    ElementState::Released => {
                        if let Some(idx) = self.drag_widget {
                            self.widgets[idx].drag_end();
                            self.drag_widget = None;
                            changed = true;
                        }
                        for w in &mut self.widgets {
                            if w.mouse_input(*button, *btn_state, self.cursor_x, self.cursor_y) {
                                changed = true;
                            }
                        }
                    }
                }
                changed
            }
            _ => false,
        }
    }

    fn render(&mut self) {
        self.prepare_text();

        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(wgpu::SurfaceError::Timeout) => return,
            Err(e) => {
                eprintln!("Surface error: {e:?}");
                return;
            }
        };

        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.06, g: 0.06, b: 0.08, a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.render_pipeline);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.draw(0..self.vertex_count, 0..1);

            self.text_renderer.render(&self.text_atlas, &self.text_viewport, &mut pass).unwrap();
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.window.pre_present_notify();
        output.present();
    }
}

struct App {
    state: Option<State>,
}

impl App {
    fn new() -> Self { Self { state: None } }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() { return; }

        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("Clear Designer")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280, 800)),
                )
                .unwrap(),
        );

        let state = pollster::block_on(State::new(window));
        self.state = Some(state);
        self.state.as_ref().unwrap().window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let needs_redraw = match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                true
            }
            WindowEvent::Resized(size) => {
                if let Some(state) = &mut self.state {
                    state.resize(size);
                }
                true
            }
            WindowEvent::RedrawRequested => {
                if let Some(state) = &mut self.state {
                    state.render();
                    state.window.request_redraw();
                }
                true
            }
            _ => {
                if let Some(state) = &mut self.state {
                    let changed = state.handle_event(&event);
                    if changed {
                        state.upload_vertices();
                    }
                    state.update_status_text(&format!(
                        "x: {:.0}  y: {:.0}",
                        state.cursor_x, state.cursor_y
                    ));
                    changed
                } else {
                    false
                }
            }
        };
        if needs_redraw {
            if let Some(state) = &mut self.state {
                state.window.request_redraw();
            }
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new();
    event_loop.run_app(&mut app).unwrap();
}
