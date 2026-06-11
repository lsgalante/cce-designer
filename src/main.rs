use std::time::Instant;
use std::fs;
use std::path::Path;
use std::net::TcpListener;
use std::io::{BufRead, BufReader, Read, Write};

use serde::{Deserialize, Serialize};

use clear_ui::widget::{ElementState, MouseButton, MouseScrollDelta, KeyEvent, Key, NamedKey};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window, delegate_output,
    registry::{ProvidesRegistryState, RegistryState},
    output::{OutputHandler, OutputState},
    seat::{
        keyboard::KeyboardHandler,
        pointer::{PointerHandler, ThemedPointer, ThemeSpec, CursorIcon},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        xdg::{
            window::{Window as XdgWindow, WindowConfigure, WindowHandler, WindowDecorations},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle, Proxy,
};
use calloop_wayland_source::WaylandSource;

use wgpu::util::DeviceExt;
use clear_ui::widget::{Breadcrumb, Canvas, MenuBar, Plate, ParametersBg, Splitter, Spreadsheet, StatusBar, TextLabel, ViewportBg, Element, GraphNode, Graph, Paginator, Button, Checkbox, Slider, Spinbox, ScrollingList, Label, ColorSelector, Switcher};
use clear_ui::colors;
use glyphon::{Attrs, Buffer, Cache, FontSystem, Metrics, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport};
use glam::{Mat4, Vec3};

pub mod app;
pub mod graphics;
pub mod api;
pub mod window;
pub mod geometry;
pub mod project;
pub mod render;
pub mod shortcut;

use app::{State, CustomEvent, HttpAction, ModifiersState, TouchPhase, DesignSettings, param_display};
use window::{AppState, WindowEvent};
use api::start_http_server;
use geometry::*;
use project::*;
use render::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_detached_network = args.iter().any(|arg| arg == "--detached-network");

    let conn = Connection::connect_to_env().unwrap();
    let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor_state = CompositorState::bind(&globals, &qh).unwrap();
    let xdg_shell_state = XdgShell::bind(&globals, &qh).unwrap();
    let shm_state = Shm::bind(&globals, &qh).unwrap();
    let seat_state = SeatState::new(&globals, &qh);
    let output_state = OutputState::new(&globals, &qh);
    let inspector = globals.bind(&qh, 1..=1, ()).ok();
    let (sender, channel) = calloop::channel::channel::<CustomEvent>();

    let mut app = AppState {
        registry_state: RegistryState::new(&globals),
        compositor_state,
        xdg_shell_state,
        shm_state,
        seat_state,
        output_state,
        seats: Vec::new(),
        pointer: None,
        keyboard: None,
        window: None,
        surface: None,
        state: None,
        exit: false,
        redraw: true,
        pressed_key: None,
        inspector,
        pending_resize: None,
        _sender: sender.clone(),
    };

    // Perform a roundtrip to populate output_state with active output scales
    event_queue.roundtrip(&mut app).unwrap();

    let scale = clear_ui::wayland::detect_scale_factor(&app.output_state);

    let (pw, ph) = if is_detached_network {
        ((400.0 * scale) as u32, (400.0 * scale) as u32)
    } else {
        ((1280.0 * scale) as u32, (800.0 * scale) as u32)
    };

    let state = pollster::block_on(State::new(
        &conn,
        &qh,
        &app.compositor_state,
        &app.xdg_shell_state,
        pw, ph,
        scale,
        is_detached_network,
    ));

    app.window = Some(state.window.clone());
    app.surface = Some(state.wl_surface.clone());
    app.state = Some(state);

    if let Some(ref inspector) = app.inspector {
        if let Some(ref surface) = app.surface {
            inspector.register_client(surface);
        }
    }

    if !is_detached_network {
        let server_sender = sender.clone();
        std::thread::spawn(move || {
            let listener = match TcpListener::bind("127.0.0.1:3000") {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("Failed to bind HTTP server to port 3000: {:?}", e);
                    return;
                }
            };
            println!("Embedded HTTP Server listening on http://127.0.0.1:3000");

            for stream in listener.incoming() {
                let stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let server_sender = server_sender.clone();
                std::thread::spawn(move || {
                    let mut write_stream = match stream.try_clone() {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    let mut reader = BufReader::new(stream);
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() {
                        return;
                    }

                    if request_line.starts_with("GET /state") {
                        let (tx, rx) = std::sync::mpsc::channel();
                        if server_sender.send(CustomEvent::GetState(tx)).is_ok() {
                            let response_body = rx.recv().unwrap_or_else(|_| "null".to_string());
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                response_body.len(),
                                response_body
                            );
                            let _ = write_stream.write_all(response.as_bytes());
                        }
                    } else if request_line.starts_with("POST /action") {
                        let mut content_length = 0;
                        loop {
                            let mut header_line = String::new();
                            if reader.read_line(&mut header_line).is_err() || header_line == "\r\n" || header_line == "\n" || header_line.is_empty() {
                                break;
                            }
                            let lower = header_line.to_lowercase();
                            if lower.starts_with("content-length:") {
                                if let Some(val) = lower.split(':').nth(1) {
                                    if let Ok(len) = val.trim().parse::<usize>() {
                                        content_length = len;
                                    }
                                }
                            }
                        }

                        let mut body = vec![0; content_length];
                        if reader.read_exact(&mut body).is_ok() {
                            let body_str = String::from_utf8_lossy(&body);
                            if let Ok(action) = serde_json::from_str::<HttpAction>(&body_str) {
                                let (tx, rx) = std::sync::mpsc::channel();
                                if server_sender.send(CustomEvent::PostAction(action, tx)).is_ok() {
                                    let res = rx.recv().unwrap_or_else(|_| Err("internal error".to_string()));
                                    let response = match res {
                                        Ok(msg) => {
                                            let body = format!("{{\"status\":\"success\",\"message\":\"{}\"}}", msg);
                                            format!(
                                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                                body.len(),
                                                body
                                            )
                                        }
                                        Err(err) => {
                                            let body = format!("{{\"status\":\"error\",\"error\":\"{}\"}}", err);
                                            format!(
                                                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                                body.len(),
                                                body
                                            )
                                        }
                                    };
                                    let _ = write_stream.write_all(response.as_bytes());
                                } else {
                                    let body = "{\"status\":\"error\",\"error\":\"failed to send action to event loop\"}";
                                    let response = format!(
                                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                        body.len(),
                                        body
                                    );
                                    let _ = write_stream.write_all(response.as_bytes());
                                }
                            } else {
                                let body = "{\"status\":\"error\",\"error\":\"failed to parse action JSON\"}";
                                let response = format!(
                                    "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                );
                                let _ = write_stream.write_all(response.as_bytes());
                            }
                        } else {
                            let body = "{\"status\":\"error\",\"error\":\"failed to read complete body\"}";
                            let response = format!(
                                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = write_stream.write_all(response.as_bytes());
                        }
                } else {
                    let body = "{\"error\":\"not found\"}";
                    let response = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = write_stream.write_all(response.as_bytes());
                }
                let _ = write_stream.flush();
            });
        }
    });
    }

    let mut event_loop = calloop::EventLoop::try_new().unwrap();
    let loop_handle = event_loop.handle();

    WaylandSource::new(conn, event_queue).insert(loop_handle.clone()).unwrap();

    loop_handle.insert_source(channel, |event, _metadata, app_state: &mut AppState| {
        if let calloop::channel::Event::Msg(msg) = event {
            app_state.handle_user_event(msg);
        }
    }).unwrap();

    const KEY_REPEAT_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
    const KEY_REPEAT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

    loop {
        let timeout = std::time::Duration::from_millis(16);
        event_loop.dispatch(timeout, &mut app).unwrap();

        if app.exit || app.state.as_ref().map(|s| s.exit_requested).unwrap_or(false) {
            if let Some(state) = &mut app.state {
                if state.needs_autosave && (state.is_detached_network || state.detached_circular_network) {
                    let default_proj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
                    let _ = state.save_to_file(&default_proj_path);
                }
            }
            break;
        }

        if let Some(state) = &mut app.state {
            if let Some(ref inspector) = app.inspector {
                let now = std::time::Instant::now();
                if now.duration_since(state.last_inspector_check) >= std::time::Duration::from_millis(250) {
                    state.last_inspector_check = now;
                    inspector.get_inspected_surfaces();
                }
            }

            if state.needs_autosave && (state.is_detached_network || state.detached_circular_network) {
                let now = std::time::Instant::now();
                if now.duration_since(state.last_autosave_time) >= std::time::Duration::from_millis(200) {
                    state.needs_autosave = false;
                    state.last_autosave_time = now;
                    let default_proj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
                    if let Err(e) = state.save_to_file(&default_proj_path) {
                        eprintln!("Failed to auto-save default project in main loop: {:?}", e);
                    } else if let Ok(m) = std::fs::metadata(&default_proj_path) {
                        if let Ok(mod_time) = m.modified() {
                            state.last_project_mod_time = Some(mod_time);
                        }
                    }
                }
            }

            if state.is_detached_network || state.detached_circular_network {
                let now = std::time::Instant::now();
                if now.duration_since(state.last_project_check) >= std::time::Duration::from_millis(100) {
                    state.last_project_check = now;
                    let default_proj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
                    if let Ok(m) = std::fs::metadata(&default_proj_path) {
                        if let Ok(mod_time) = m.modified() {
                            if Some(mod_time) != state.last_project_mod_time {
                                state.last_project_mod_time = Some(mod_time);
                                if let Err(e) = state.load_from_file(&default_proj_path) {
                                    eprintln!("Failed to auto-reload project: {:?}", e);
                                } else {
                                    app.redraw = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(ref mut pk) = app.pressed_key {
            let now = std::time::Instant::now();
            if now.duration_since(pk.first_pressed) >= KEY_REPEAT_DELAY {
                if now.duration_since(pk.last_repeated) >= KEY_REPEAT_INTERVAL {
                    pk.last_repeated = now;
                    if let Some(st) = &mut app.state {
                        let custom_event = clear_ui::widget::KeyEvent {
                            state: clear_ui::widget::ElementState::Pressed,
                            logical_key: pk.logical_key.clone(),
                            text: pk.text.clone(),
                            repeat: true,
                            ctrl: st.modifiers.ctrl,
                            shift: st.modifiers.shift,
                        };
                        let ev = WindowEvent::KeyboardInput { event: custom_event };
                        app.process_event(ev);
                    }
                }
            }
        }

fn create_memfd_with_data(name: &str, data: &[u8]) -> std::io::Result<std::os::unix::io::RawFd> {
    use std::io::{Seek, Write};
    use std::os::unix::io::FromRawFd;
    use std::os::unix::io::IntoRawFd;

    let c_name = std::ffi::CString::new(name).unwrap();
    let fd = unsafe { libc::memfd_create(c_name.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(data)?;
    file.seek(std::io::SeekFrom::Start(0))?;
    Ok(file.into_raw_fd())
}

        if app.redraw {
            app.redraw = false;
            if let Some(state) = &mut app.state {
                if state.render() {
                    app.redraw = true;
                }
                if let Some(ref inspector) = app.inspector {
                    if let Some(ref surface) = app.surface {
                        let now = std::time::Instant::now();
                        if now.duration_since(state.last_inspector_update) >= std::time::Duration::from_millis(100) {
                            state.last_inspector_update = now;
                            let json = clear_ui::widget::serialize_widgets(&state.widgets);
                            if let Ok(raw_fd) = create_memfd_with_data("clear_ui_state", json.as_bytes()) {
                                use std::os::unix::io::{FromRawFd, AsFd};
                                let file = unsafe { std::fs::File::from_raw_fd(raw_fd) };
                                inspector.update_state(surface, file.as_fd(), json.len() as u32);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_default_project() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
        let content = fs::read_to_string(&path).expect("failed to read default project");
        let proj: Project = serde_json::from_str(&content).expect("failed to deserialize project");
        assert_eq!(proj.name, "Default Project");
        assert_eq!(proj.view_state.active_camera, "Camera 1");
        assert_eq!(proj.root.name, "root");
        assert_eq!(proj.root.children.len(), 3);
        assert_eq!(proj.root.children[0].name, "Camera 1");
        assert_eq!(proj.root.children[0].position, (1.0, 1.0));
        assert_eq!(proj.root.children[1].name, "Sphere 1");
        assert_eq!(proj.root.children[1].position, (4.0, 2.0));
        assert_eq!(proj.root.children[2].name, "Transform 1");
        assert_eq!(proj.root.children[2].position, (4.0, 3.0));
    }

    #[test]
    fn test_project_serialization_roundtrip() {
        let root = FsNode {
            name: "test_root".to_string(),
            node_type: "node".to_string(),
            children: vec![
                FsNode {
                    name: "child1".to_string(),
                    node_type: "sphere".to_string(),
                    children: vec![],
                    params: vec![],
                    geometry_visible: true,
                    position: (5.0, 6.0),
                }
            ],
            params: vec![],
            geometry_visible: true,
            position: (0.0, 0.0),
        };
        let view_state = ProjectViewState {
            active_camera: "child1".to_string(),
            pan: (1.5, -2.5),
            current_path: vec![0],
            selected_node: Some(2),
        };
        let proj = Project {
            name: "Test Project".to_string(),
            root,
            view_state,
        };

        let content = serde_json::to_string(&proj).expect("failed to serialize");
        let proj2: Project = serde_json::from_str(&content).expect("failed to deserialize");

        assert_eq!(proj.name, proj2.name);
        assert_eq!(proj.view_state.active_camera, proj2.view_state.active_camera);
        assert_eq!(proj.view_state.pan, proj2.view_state.pan);
        assert_eq!(proj.view_state.current_path, proj2.view_state.current_path);
        assert_eq!(proj.view_state.selected_node, proj2.view_state.selected_node);
        assert_eq!(proj.root.name, proj2.root.name);
        assert_eq!(proj.root.children.len(), proj2.root.children.len());
        assert_eq!(proj.root.children[0].name, proj2.root.children[0].name);
        assert_eq!(proj.root.children[0].position, proj2.root.children[0].position);
    }

    #[test]
    fn test_geometry_attributes_system() {
        let mut attrs1 = std::collections::HashMap::new();
        attrs1.insert("UV".to_string(), GAttribute::Float2([0.1, 0.2]));
        attrs1.insert("ID".to_string(), GAttribute::Float(42.0));

        let v1 = GVertex {
            pos: [1.0, 2.0, 3.0],
            col: [1.0, 0.0, 0.0],
            attributes: attrs1,
        };

        let mut attrs2 = std::collections::HashMap::new();
        attrs2.insert("Norm".to_string(), GAttribute::Float3([0.0, 1.0, 0.0]));
        attrs2.insert("UV".to_string(), GAttribute::Float2([0.3, 0.4]));

        let v2 = GVertex {
            pos: [4.0, 5.0, 6.0],
            col: [0.0, 1.0, 0.0],
            attributes: attrs2,
        };

        let mut geom1 = Geometry { vertices: vec![v1] };
        let geom2 = Geometry { vertices: vec![v2] };

        geom1.merge(geom2);
        assert_eq!(geom1.vertices.len(), 2);

        let render_verts = geom1.to_vertex3d_vec();
        assert_eq!(render_verts.len(), 2);
        assert_eq!(render_verts[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(render_verts[0].color, [1.0, 0.0, 0.0]);
        assert_eq!(render_verts[1].position, [4.0, 5.0, 6.0]);
        assert_eq!(render_verts[1].color, [0.0, 1.0, 0.0]);

        let (headers, rows) = State::geometry_to_spreadsheet_data(&geom1);

        let expected_headers = vec![
            "Vertex".to_string(),
            "Pos.x".to_string(),
            "Pos.y".to_string(),
            "Pos.z".to_string(),
            "Col.r".to_string(),
            "Col.g".to_string(),
            "Col.b".to_string(),
            "ID".to_string(),
            "Norm.x".to_string(),
            "Norm.y".to_string(),
            "Norm.z".to_string(),
            "UV.x".to_string(),
            "UV.y".to_string(),
        ];
        assert_eq!(headers, expected_headers);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "0");
        assert_eq!(rows[0][1], "1.0000"); // Pos X
        assert_eq!(rows[0][7], "42.0000"); // ID
        assert_eq!(rows[0][8], "-"); // Norm.x
        assert_eq!(rows[0][9], "-"); // Norm.y
        assert_eq!(rows[0][10], "-"); // Norm.z
        assert_eq!(rows[0][11], "0.1000"); // UV.x
        assert_eq!(rows[0][12], "0.2000"); // UV.y

        assert_eq!(rows[1][0], "1");
        assert_eq!(rows[1][1], "4.0000"); // Pos X
        assert_eq!(rows[1][7], "-"); // ID
        assert_eq!(rows[1][8], "0.0000"); // Norm.x
        assert_eq!(rows[1][9], "1.0000"); // Norm.y
        assert_eq!(rows[1][10], "0.0000"); // Norm.z
        assert_eq!(rows[1][11], "0.3000"); // UV.x
        assert_eq!(rows[1][12], "0.4000"); // UV.y
    }

    #[test]
    fn test_line_geometry_generation() {
        let start = Vec3::new(0.0, 0.0, 0.0);
        let end = Vec3::new(0.0, 1.0, 0.0);
        let geom = line_vertices(start, end, 0.02);
        
        // A box line should contain 36 vertices (6 faces * 2 triangles * 3 vertices)
        assert_eq!(geom.vertices.len(), 36);
        
        // Every vertex should have "Norm" and "UV" attributes
        for v in &geom.vertices {
            assert!(v.attributes.contains_key("Norm"));
            assert!(v.attributes.contains_key("UV"));
        }
    }

    #[test]
    fn test_design_settings_serialization_roundtrip() {
        let json_without_pivot = r#"
        {
            "grid_snap_enabled": true,
            "network_grid_enabled": true,
            "grid_size_x": 80.0,
            "grid_size_y": 40.0,
            "skipped_row_h": 20.0,
            "skipped_col_w": 20.0,
            "show_grid_enabled": true,
            "show_cube_enabled": false,
            "show_origin_enabled": true,
            "origin_size": 1.0,
            "viewport_bg_color": [0.05, 0.05, 0.10],
            "square_viewport": false,
            "grid_thickness": 0.03
        }
        "#;
        
        let settings: DesignSettings = serde_json::from_str(json_without_pivot).unwrap();
        assert_eq!(settings.show_camera_pivot_enabled, false);
        assert_eq!(settings.camera_pivot_size, 1.0);
        assert_eq!(settings.grid_color, [0.35, 0.35, 0.40]);
        assert_eq!(settings.uniform_background, false);
        assert_eq!(settings.network_opacity, 0.95);
        assert_eq!(settings.cell_color, [0.13, 0.13, 0.16]);
        assert_eq!(settings.gap_color, [0.07, 0.07, 0.09]);
        
        let serialized = serde_json::to_string(&settings).unwrap();
        let settings_roundtrip: DesignSettings = serde_json::from_str(&serialized).unwrap();
        assert_eq!(settings_roundtrip.show_camera_pivot_enabled, false);
        assert_eq!(settings_roundtrip.camera_pivot_size, 1.0);
        assert_eq!(settings_roundtrip.grid_color, [0.35, 0.35, 0.40]);
        assert_eq!(settings_roundtrip.uniform_background, false);
        assert_eq!(settings_roundtrip.network_opacity, 0.95);
        assert_eq!(settings_roundtrip.cell_color, [0.13, 0.13, 0.16]);
        assert_eq!(settings_roundtrip.gap_color, [0.07, 0.07, 0.09]);
    }

    #[test]
    fn test_http_action_parsing() {
        let json_str = "{\"action\": \"add_node\", \"template_name\": \"Sphere\", \"name\": \"MySphere\", \"x\": 5.0, \"y\": 3.0}";
        let action: HttpAction = serde_json::from_str(json_str).unwrap();
        match action {
            HttpAction::AddNode { template_name, name, x, y } => {
                assert_eq!(template_name, "Sphere");
                assert_eq!(name, Some("MySphere".to_string()));
                assert_eq!(x, 5.0);
                assert_eq!(y, 3.0);
            }
            _ => panic!("Expected AddNode"),
        }
    }

    #[test]
    fn test_get_next_visible_pane() {
        // Without spreadsheet (3 panes: LEFT, RIGHT, PARAM)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, false, false), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, false, false), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, false, false), LEFT_MENUBAR_IDX);

        // With spreadsheet (4 panes: LEFT, RIGHT, PARAM, SPREADSHEET)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, true, false), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, true, false), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, true, false), SPREADSHEET_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(SPREADSHEET_MENUBAR_IDX, true, true, true, true, false), LEFT_MENUBAR_IDX);

        // Reverse cycling with shift key (without spreadsheet)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, false, true), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, false, true), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, false, true), LEFT_MENUBAR_IDX);

        // Reverse cycling with shift key (with spreadsheet)
        assert_eq!(get_next_visible_pane(LEFT_MENUBAR_IDX, true, true, true, true, true), SPREADSHEET_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(SPREADSHEET_MENUBAR_IDX, true, true, true, true, true), PARAM_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(PARAM_MENUBAR_IDX, true, true, true, true, true), RIGHT_MENUBAR_IDX);
        assert_eq!(get_next_visible_pane(RIGHT_MENUBAR_IDX, true, true, true, true, true), LEFT_MENUBAR_IDX);
    }

    #[test]
    fn test_inertial_settings_fallback_and_ranges() {
        let friction_raw = 90_u16;
        let friction_f32 = (friction_raw as f32 / 100.0).clamp(0.5, 0.99);
        assert!(friction_f32 >= 0.5 && friction_f32 <= 0.99);
        
        let friction_raw_low = 30_u16;
        let friction_f32_low = (friction_raw_low as f32 / 100.0).clamp(0.5, 0.99);
        assert_eq!(friction_f32_low, 0.5);
    }

    #[test]
    fn test_decay_formula() {
        let friction = 0.90_f32;
        let dt_60 = 1.0 / 60.0;
        let decay_60 = friction.powf(dt_60 * 60.0);
        assert!((decay_60 - friction).abs() < 1e-5);

        let decay_0 = friction.powf(0.0 * 60.0);
        assert_eq!(decay_0, 1.0);
    }

    #[test]
    fn test_circular_network_clamping_math() {
        let header_h = 26.0;
        let status_h = 0.0;
        let gap = 18.0;

        let clamp_layout = |width: f32, height: f32, layout_x: f32, layout_y: f32, layout_r: f32| -> (f32, f32, f32) {
            let max_r = ((width - 2.0 * gap).min(height - header_h - status_h - 2.0 * gap) / 2.0).max(50.0);
            let r = layout_r.clamp(50.0, max_r);

            let min_x = gap + r;
            let max_x = (width - gap - r).max(min_x);
            let x = layout_x.clamp(min_x, max_x);

            let min_y = header_h + gap + r;
            let max_y = (height - status_h - gap - r).max(min_y);
            let y = layout_y.clamp(min_y, max_y);

            (x, y, r)
        };

        let (x, y, r) = clamp_layout(1000.0, 800.0, 500.0, 400.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 500.0);
        assert_eq!(y, 400.0);

        let (x, y, r) = clamp_layout(1000.0, 800.0, 50.0, 400.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 198.0);
        assert_eq!(y, 400.0);

        let (x, y, r) = clamp_layout(1000.0, 800.0, 500.0, 100.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 500.0);
        assert_eq!(y, 224.0);

        let (x, y, r) = clamp_layout(1000.0, 800.0, 500.0, 750.0, 180.0);
        assert_eq!(r, 180.0);
        assert_eq!(x, 500.0);
        assert_eq!(y, 602.0);

        let (x, y, r) = clamp_layout(50.0, 50.0, 10.0, 10.0, 180.0);
        assert!(r >= 50.0);
        assert!(x >= 0.0);
        assert!(y >= 0.0);
    }

    #[test]
    fn test_floating_rectangular_pane_clamping() {
        let width = 1000.0_f32;
        let height = 800.0_f32;
        let header_h = 26.0_f32;
        let status_h = 0.0_f32;
        let gap = 18.0_f32;

        let clamp_floating = |_fx: f32, _fy: f32, fw: f32, _fh: f32| -> (f32, f32, f32, f32) {
            let fw = fw.clamp(150.0, (width - 2.0 * gap).max(150.0));
            let fx = gap;
            let fy = header_h + gap;
            let fh = (height - header_h - status_h - 2.0 * gap).max(100.0);
            (fx, fy, fw, fh)
        };

        let expected_h = height - header_h - status_h - 2.0 * gap;

        // Standard center case (fx anchored to gap, fy/fh to full height)
        let (x, y, w, h) = clamp_floating(100.0, 200.0, 400.0, 300.0);
        assert_eq!((x, y, w, h), (gap, header_h + gap, 400.0, expected_h));

        // Off-screen left/top
        let (x, y, w, h) = clamp_floating(-50.0, 0.0, 400.0, 300.0);
        assert_eq!((x, y, w, h), (gap, header_h + gap, 400.0, expected_h));

        // Off-screen right/bottom
        let (x, y, w, h) = clamp_floating(900.0, 700.0, 400.0, 300.0);
        assert_eq!((x, y, w, h), (gap, header_h + gap, 400.0, expected_h));

        let clamp_param = |pw: f32| -> (f32, f32, f32, f32) {
            let pw = pw.clamp(150.0, (width - 2.0 * gap).max(150.0));
            let px = width - gap - pw;
            let py = header_h + gap;
            let ph = (height - header_h - status_h - 2.0 * gap).max(100.0);
            (px, py, pw, ph)
        };

        // Standard param case
        let (px, py, pw, ph) = clamp_param(300.0);
        assert_eq!((px, py, pw, ph), (width - gap - 300.0, header_h + gap, 300.0, expected_h));

        // Off-screen/overflow width param case
        let (px, py, pw, ph) = clamp_param(1200.0);
        let max_w = width - 2.0 * gap;
        assert_eq!((px, py, pw, ph), (gap, header_h + gap, max_w, expected_h));

        let clamp_ss = |ss_h_val: f32, show_net: bool, show_param: bool| -> (f32, f32, f32, f32) {
            let fx = gap;
            let fw = 400.0;
            let param_w = 300.0;
            let param_x = width - gap - param_w;

            let ss_x = if show_net { fx + fw + gap } else { gap };
            let ss_w_end = if show_param { param_x - gap } else { width - gap };
            let ss_w = (ss_w_end - ss_x).max(150.0);
            let ss_y_end = height - status_h - gap;
            let ss_h = ss_h_val.clamp(100.0, (ss_y_end - header_h - gap).max(100.0));
            let ss_y = ss_y_end - ss_h;
            (ss_x, ss_y, ss_w, ss_h)
        };

        // Standard spreadsheet case (both net and param visible)
        let (sx, sy, sw, sh) = clamp_ss(250.0, true, true);
        assert_eq!(sx, gap + 400.0 + gap);
        assert_eq!(sw, (width - gap - 300.0 - gap) - (gap + 400.0 + gap));
        assert_eq!(sh, 250.0);
        assert_eq!(sy, height - status_h - gap - 250.0);

        // Neither net nor param visible
        let (sx2, _sy2, sw2, sh2) = clamp_ss(200.0, false, false);
        assert_eq!(sx2, gap);
        assert_eq!(sw2, width - 2.0 * gap);
        assert_eq!(sh2, 200.0);

        // Clamping height to min (100.0)
        let (_, _, _, sh_min) = clamp_ss(50.0, true, true);
        assert_eq!(sh_min, 100.0);

        // Clamping height to max
        let max_possible_h = height - status_h - gap - header_h - gap;
        let (_, _, _, sh_max) = clamp_ss(1000.0, true, true);
        assert_eq!(sh_max, max_possible_h);
    }

    #[test]
    fn test_paginator_collapsed_layout() {
        let width = 1000.0_f32;

        let get_paginator_w = |page_hidden: bool| -> f32 {
            if page_hidden {
                56.0_f32
            } else {
                236.0_f32
            }
        };

        // Collapsed layout
        let w_collapsed = get_paginator_w(true);
        assert_eq!(w_collapsed, 56.0);

        // Expanded layout
        let w_expanded = get_paginator_w(false);
        assert_eq!(w_expanded, 236.0);

        // Verify how other viewport bounds adjust relative to paginator_w
        let check_viewport_layout = |page_hidden: bool| -> (f32, f32) {
            let paginator_w = get_paginator_w(page_hidden);
            let col_c_x = paginator_w;
            let col_c_w = width - paginator_w;
            (col_c_x, col_c_w)
        };

        // When collapsed, viewport/canvas has more space
        let (vx_c, vw_c) = check_viewport_layout(true);
        assert_eq!(vx_c, 56.0);
        assert_eq!(vw_c, 944.0);

        // When expanded, viewport/canvas has default space
        let (vx_e, vw_e) = check_viewport_layout(false);
        assert_eq!(vx_e, 236.0);
        assert_eq!(vw_e, 764.0);
    }

    #[test]
    fn test_keyboard_shortcut_system() {
        // Test parsing simple shortcut
        let ctrl_g = Shortcut::parse("Ctrl+g").unwrap();
        assert_eq!(ctrl_g.ctrl, true);
        assert_eq!(ctrl_g.shift, false);
        assert_eq!(ctrl_g.alt, false);
        assert_eq!(ctrl_g.logo, false);
        assert_eq!(ctrl_g.key, Key::Character("g".to_string()));

        // Test parsing complex shortcut
        let complex = Shortcut::parse("Ctrl+Shift+Alt+Logo+s").unwrap();
        assert_eq!(complex.ctrl, true);
        assert_eq!(complex.shift, true);
        assert_eq!(complex.alt, true);
        assert_eq!(complex.logo, true);
        assert_eq!(complex.key, Key::Character("s".to_string()));

        // Test parsing named keys
        let tab_sc = Shortcut::parse("Tab").unwrap();
        assert_eq!(tab_sc.key, Key::Named(NamedKey::Tab));

        // Test parsing case insensitivity
        let case_sc = Shortcut::parse("cTrL+sHiFt+ArrowDown").unwrap();
        assert_eq!(case_sc.ctrl, true);
        assert_eq!(case_sc.shift, true);
        assert_eq!(case_sc.key, Key::Named(NamedKey::ArrowDown));

        // Test register and match
        let mut mgr = ShortcutManager::new();
        mgr.register("Ctrl+g", Action::ToggleGrid).unwrap();
        mgr.register("`", Action::ToggleSpreadsheet).unwrap();

        // Matches with ctrl and g
        let mods_ctrl = ModifiersState { ctrl: true, alt: false, shift: false, logo: false };
        let key_g = Key::Character("g".to_string());
        assert_eq!(mgr.match_action(&mods_ctrl, &key_g), Some(Action::ToggleGrid));

        // No match with ctrl and a
        let key_a = Key::Character("a".to_string());
        assert_eq!(mgr.match_action(&mods_ctrl, &key_a), None);

        // Matches backtick with no modifiers
        let mods_none = ModifiersState::default();
        let key_tick = Key::Character("`".to_string());
        assert_eq!(mgr.match_action(&mods_none, &key_tick), Some(Action::ToggleSpreadsheet));
    }
}

