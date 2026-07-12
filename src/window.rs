#![allow(unused_imports)]
use std::path::Path;

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
            window::{Window as XdgWindow, WindowConfigure, WindowHandler},
            XdgShell,
        },
    },
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
    Connection, QueueHandle,
};

use cce_ui::widget::WidgetHost;
use crate::shortcut::Action;
use crate::app::{State, CustomEvent, HttpAction, ModifiersState, TouchPhase, LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX, HEADER_IDX, CONTENT_IDX, BREADCRUMB_IDX, VIEWPORT_IDX, PARAM_IDX, SPREADSHEET_IDX, get_next_visible_pane, Project, ProjectViewState, ParamDef, param_display};

#[derive(Debug, Clone, Copy)]
pub struct LocalPosition {
    pub x: f64,
    pub y: f64,
}

pub enum WindowEvent {
    MouseWheel { delta: cce_ui::widget::MouseScrollDelta, phase: TouchPhase },
    PinchGesture { delta: f64 },
    CursorMoved { position: LocalPosition },
    MouseInput { state: cce_ui::widget::ElementState, button: cce_ui::widget::MouseButton },
    ModifiersChanged(ModifiersState),
    KeyboardInput { event: cce_ui::widget::KeyEvent },
}

pub struct PressedKey {
    pub logical_key: cce_ui::widget::Key,
    pub text: Option<String>,
    pub first_pressed: std::time::Instant,
    pub last_repeated: std::time::Instant,
}

pub fn is_repeatable_key(key: &cce_ui::widget::Key) -> bool {
    use cce_ui::widget::{Key, NamedKey};
    match key {
        Key::Named(NamedKey::Backspace) |
        Key::Named(NamedKey::Delete) |
        Key::Named(NamedKey::ArrowLeft) |
        Key::Named(NamedKey::ArrowRight) |
        Key::Named(NamedKey::ArrowUp) |
        Key::Named(NamedKey::ArrowDown) |
        Key::Named(NamedKey::Home) |
        Key::Named(NamedKey::End) |
        Key::Character(_) => true,
        _ => false,
    }
}

pub struct PendingResize {
    pub serial: u32,
    pub edge: smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge,
    pub start_x: f32,
    pub start_y: f32,
    pub is_move: bool,
}

pub struct AppState {
    pub registry_state: RegistryState,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShell,
    pub shm_state: Shm,
    pub seat_state: SeatState,
    pub output_state: OutputState,

    pub seats: Vec<wl_seat::WlSeat>,
    pub pointer: Option<ThemedPointer>,
    pub keyboard: Option<wl_keyboard::WlKeyboard>,

    pub window: Option<XdgWindow>,
    pub surface: Option<wl_surface::WlSurface>,

    pub state: Option<State>,
    pub exit: bool,
    pub redraw: bool,
    pub pressed_key: Option<PressedKey>,
    pub inspector: Option<cce_ui::protocol::zcce_inspector_v1::ZcceInspectorV1>,
    pub pending_resize: Option<PendingResize>,
    pub _sender: calloop::channel::Sender<CustomEvent>,
}

impl CompositorHandler for AppState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        scale_factor: i32,
    ) {
        _surface.set_buffer_scale(scale_factor);
        if let Some(state) = &mut self.state {
            state.scale = scale_factor as f64;
            cce_ui::scale::set_scale_factor(scale_factor as f32);
            let pw = (state.width as f64 * state.scale) as u32;
            let ph = (state.height as f64 * state.scale) as u32;
            state.resize(pw, ph);
            self.redraw = true;
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {}

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {}

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {}

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {}
}

impl OutputHandler for AppState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {}

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {}

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {}
}

impl SeatHandler for AppState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.seats.push(seat);
        // eprintln!("DEBUG SEAT: new_seat called, total seats now: {}", self.seats.len());
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        // eprintln!("DEBUG SEAT: new_capability: {:?}", capability);
        if capability == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor_state.create_surface(qh);
            let themed_pointer = self.seat_state.get_pointer_with_theme(
                qh,
                &seat,
                self.shm_state.wl_shm(),
                surface,
                ThemeSpec::System,
            ).unwrap();
            self.pointer = Some(themed_pointer);
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let keyboard = self
                .seat_state
                .get_keyboard(qh, &seat, None)
                .unwrap();
            self.keyboard = Some(keyboard);
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            self.pointer = None;
        }
        if capability == Capability::Keyboard {
            self.keyboard = None;
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        self.seats.retain(|s| s != &seat);
        // eprintln!("DEBUG SEAT: remove_seat called, total seats now: {}", self.seats.len());
    }
}

impl ShmHandler for AppState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm_state
    }
}

impl PointerHandler for AppState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[smithay_client_toolkit::seat::pointer::PointerEvent],
    ) {
        use smithay_client_toolkit::seat::pointer::PointerEventKind;
        for event in events {
            if let Some(st) = &mut self.state {
                st.cursor_x = event.position.0 as f32;
                st.cursor_y = event.position.1 as f32;
                match &event.kind {
                    PointerEventKind::Motion { .. } => {
                        if st.is_detached_network {
                            let lx = event.position.0 as f32;
                            let ly = event.position.1 as f32;
                            let dx = lx - st.circular_network_layout.x;
                            let dy = ly - st.circular_network_layout.y;
                            let dist = (dx * dx + dy * dy).sqrt();
                            let on_border = dist >= st.circular_network_layout.r - 12.0 && dist <= st.circular_network_layout.r;
                            let hits_any_menu = st.menu(LEFT_MENUBAR_IDX).get_menu_items_at(lx, ly).is_some();

                            if let Some(ref themed_pointer) = self.pointer {
                                if on_border && !hits_any_menu {
                                    let nx = dx / dist;
                                    let ny = dy / dist;
                                    let mut cursor = CursorIcon::Default;
                                    if ny < -0.382 {
                                        if nx < -0.382 {
                                            cursor = CursorIcon::NwResize;
                                        } else if nx > 0.382 {
                                            cursor = CursorIcon::NeResize;
                                        } else {
                                            cursor = CursorIcon::NResize;
                                        }
                                    } else if ny > 0.382 {
                                        if nx < -0.382 {
                                            cursor = CursorIcon::SwResize;
                                        } else if nx > 0.382 {
                                            cursor = CursorIcon::SeResize;
                                        } else {
                                            cursor = CursorIcon::SResize;
                                        }
                                    } else {
                                        if nx < -0.382 {
                                            cursor = CursorIcon::WResize;
                                        } else if nx > 0.382 {
                                            cursor = CursorIcon::EResize;
                                        }
                                    }
                                    let _ = themed_pointer.set_cursor(_conn, cursor);
                                } else {
                                    let _ = themed_pointer.set_cursor(_conn, CursorIcon::Default);
                                }
                            }
                        } else {
                            let lx = event.position.0 as f32;
                            let ly = event.position.1 as f32;
                            let border = 8.0f32;
                            let mut cursor = CursorIcon::Default;
                            if ly < border {
                                if lx < border {
                                    cursor = CursorIcon::NwResize;
                                } else if lx > st.width - border {
                                    cursor = CursorIcon::NeResize;
                                } else {
                                    cursor = CursorIcon::NResize;
                                }
                            } else if ly > st.height - border {
                                if lx < border {
                                    cursor = CursorIcon::SwResize;
                                } else if lx > st.width - border {
                                    cursor = CursorIcon::SeResize;
                                } else {
                                    cursor = CursorIcon::SResize;
                                }
                            } else if lx < border {
                                cursor = CursorIcon::WResize;
                            } else if lx > st.width - border {
                                cursor = CursorIcon::EResize;
                            }

                            if let Some(ref themed_pointer) = self.pointer {
                                let _ = themed_pointer.set_cursor(_conn, cursor);
                            }
                        }

                        if let Some(ref pending) = self.pending_resize {
                            let lx = event.position.0 as f32;
                            let ly = event.position.1 as f32;
                            let rx = lx - pending.start_x;
                            let ry = ly - pending.start_y;
                            let rdist = (rx * rx + ry * ry).sqrt();
                            if rdist > 4.0 {
                                if let Some(ref window) = self.window {
                                    let seat = self.seats.first().cloned().or_else(|| self.seat_state.seats().next());
                                    if let Some(ref seat) = seat {
                                        if pending.is_move {
                                            // eprintln!("DEBUG DRAG INITIATING window.move_ with serial={}", pending.serial);
                                            window.move_(seat, pending.serial);
                                        } else {
                                            // eprintln!("DEBUG RESIZE INITIATING window.resize with edge={:?}, serial={}", pending.edge, pending.serial);
                                            window.resize(seat, pending.serial, pending.edge);
                                        }
                                    }
                                }
                                self.pending_resize = None;
                            }
                        }

                        let ev = WindowEvent::CursorMoved {
                            position: LocalPosition {
                                x: event.position.0,
                                y: event.position.1,
                            },
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Press { button, serial, .. } => {
                        let btn = match *button {
                            272 => cce_ui::widget::MouseButton::Left,
                            273 => cce_ui::widget::MouseButton::Right,
                            274 => cce_ui::widget::MouseButton::Middle,
                            _ => continue,
                        };
                        // eprintln!("DEBUG MOUSE PRESS: button={:?}, pos={:?}, local=({}, {})", btn, event.position, cx, cy);

                        if let Some(ref st) = self.state {
                            if st.is_detached_network && btn == cce_ui::widget::MouseButton::Left {
                                let lx = event.position.0 as f32;
                                let ly = event.position.1 as f32;
                                let dx = lx - st.circular_network_layout.x;
                                let dy = ly - st.circular_network_layout.y;
                                let dist = (dx * dx + dy * dy).sqrt();
                                let on_border = dist >= st.circular_network_layout.r - 12.0 && dist <= st.circular_network_layout.r;
                                let in_menubar_bg = dy < 0.0 && dist >= st.circular_network_layout.r - 35.0 && dist <= st.circular_network_layout.r;
                                let hits_any_menu = st.menu(LEFT_MENUBAR_IDX).get_menu_items_at(lx, ly).is_some();

                                // eprintln!("DEBUG DRAG: lx={}, ly={}, cx={}, cy={}, r={}, dx={}, dy={}, dist={}, on_border={}, in_menubar_bg={}, hits_any_menu={}, seats_len={}, has_window={}",
                                //     lx, ly, st.circular_network_layout.x, st.circular_network_layout.y, st.circular_network_layout.r,
                                //     dx, dy, dist, on_border, in_menubar_bg, hits_any_menu, self.seats.len(), self.window.is_some());

                                if (on_border || in_menubar_bg) && !hits_any_menu {
                                    if let Some(ref _window) = self.window {
                                        let seat = self.seats.first().cloned().or_else(|| self.seat_state.seats().next());
                                        if let Some(ref _seat) = seat {
                                            if on_border {
                                                use smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
                                                let nx = dx / dist;
                                                let ny = dy / dist;
                                                let mut edge = ResizeEdge::None;
                                                if ny < -0.382 {
                                                    if nx < -0.382 {
                                                        edge = ResizeEdge::TopLeft;
                                                    } else if nx > 0.382 {
                                                        edge = ResizeEdge::TopRight;
                                                    } else {
                                                        edge = ResizeEdge::Top;
                                                    }
                                                } else if ny > 0.382 {
                                                    if nx < -0.382 {
                                                        edge = ResizeEdge::BottomLeft;
                                                    } else if nx > 0.382 {
                                                        edge = ResizeEdge::BottomRight;
                                                    } else {
                                                        edge = ResizeEdge::Bottom;
                                                    }
                                                } else {
                                                    if nx < -0.382 {
                                                        edge = ResizeEdge::Left;
                                                    } else if nx > 0.382 {
                                                        edge = ResizeEdge::Right;
                                                    }
                                                }
                                                self.pending_resize = Some(PendingResize {
                                                    serial: *serial,
                                                    edge,
                                                    start_x: lx,
                                                    start_y: ly,
                                                    is_move: false,
                                                });
                                                continue;
                                            } else {
                                                self.pending_resize = Some(PendingResize {
                                                    serial: *serial,
                                                    edge: smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge::None,
                                                    start_x: lx,
                                                    start_y: ly,
                                                    is_move: true,
                                                });
                                                continue;
                                            }
                                        }
                                    }
                                }
                            } else if !st.is_detached_network && btn == cce_ui::widget::MouseButton::Left {
                                let lx = event.position.0 as f32;
                                let ly = event.position.1 as f32;
                                let border = 8.0f32;
                                use smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
                                let mut edge = ResizeEdge::None;
                                if ly < border {
                                    if lx < border {
                                        edge = ResizeEdge::TopLeft;
                                    } else if lx > st.width - border {
                                        edge = ResizeEdge::TopRight;
                                    } else {
                                        edge = ResizeEdge::Top;
                                    }
                                } else if ly > st.height - border {
                                    if lx < border {
                                        edge = ResizeEdge::BottomLeft;
                                    } else if lx > st.width - border {
                                        edge = ResizeEdge::BottomRight;
                                    } else {
                                        edge = ResizeEdge::Bottom;
                                    }
                                } else if lx < border {
                                    edge = ResizeEdge::Left;
                                } else if lx > st.width - border {
                                    edge = ResizeEdge::Right;
                                }

                                if edge != ResizeEdge::None {
                                    self.pending_resize = Some(PendingResize {
                                        serial: *serial,
                                        edge,
                                        start_x: lx,
                                        start_y: ly,
                                        is_move: false,
                                    });
                                    continue;
                                }
                            }
                        }

                        let ev = WindowEvent::MouseInput {
                            state: cce_ui::widget::ElementState::Pressed,
                            button: btn,
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Release { button, .. } => {
                        let btn = match *button {
                            272 => cce_ui::widget::MouseButton::Left,
                            273 => cce_ui::widget::MouseButton::Right,
                            274 => cce_ui::widget::MouseButton::Middle,
                            _ => continue,
                        };
                        // eprintln!("DEBUG MOUSE RELEASE: button={:?}, pos={:?}, local=({}, {})", btn, event.position, cx, cy);
                        if btn == cce_ui::widget::MouseButton::Left {
                            self.pending_resize = None;
                        }
                        let ev = WindowEvent::MouseInput {
                            state: cce_ui::widget::ElementState::Released,
                            button: btn,
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Axis { horizontal, vertical, .. } => {
                        let h_val = horizontal.absolute as f32;
                        let v_val = vertical.absolute as f32;
                        // eprintln!("DEBUG AXIS EVENT: horizontal={:?}, vertical={:?}, scale={}", horizontal, vertical, st.scale);
                        let ev = WindowEvent::MouseWheel {
                            delta: cce_ui::widget::MouseScrollDelta::LineDelta(-h_val / 10.0, -v_val / 10.0),
                            phase: TouchPhase::Moved,
                        };
                        self.process_event(ev);
                    }
                    PointerEventKind::Enter { .. } => {
                        if let Some(ref themed_pointer) = self.pointer {
                            let _ = themed_pointer.set_cursor(_conn, CursorIcon::Default);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

impl KeyboardHandler for AppState {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw_modifiers: &[u32],
        _keysyms: &[xkeysym::Keysym],
    ) {}

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        self.pressed_key = None;
        if let Some(st) = &mut self.state {
            st.modifiers = ModifiersState::default();
        }
    }


    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: smithay_client_toolkit::seat::keyboard::KeyEvent,
    ) {
        self.handle_key(event, cce_ui::widget::ElementState::Pressed);
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: smithay_client_toolkit::seat::keyboard::KeyEvent,
    ) {
        self.handle_key(event, cce_ui::widget::ElementState::Released);
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: smithay_client_toolkit::seat::keyboard::Modifiers,
        _layout: u32,
    ) {
        if let Some(st) = &mut self.state {
            st.modifiers.ctrl = modifiers.ctrl;
            st.modifiers.alt = modifiers.alt;
            st.modifiers.shift = modifiers.shift;
            st.modifiers.logo = modifiers.logo;
        }
    }
}

impl WindowHandler for AppState {
    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &XdgWindow,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let (w, h) = configure.new_size;
        // eprintln!("DEBUG CONFIGURE: new_size={:?}, configure={:?}", configure.new_size, configure);
        if let (Some(w), Some(h)) = (w, h) {
            let width = w.get();
            let height = h.get();
            if let Some(state) = &mut self.state {
                let pw = (width as f64 * state.scale) as u32;
                let ph = (height as f64 * state.scale) as u32;
                state.resize(pw, ph);
            }
        }
        self.redraw = true;
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &XdgWindow) {
        self.exit = true;
    }
}

impl ProvidesRegistryState for AppState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    
    fn runtime_add_global(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _name: u32,
        _interface: &str,
        _version: u32,
    ) {}
    
    fn runtime_remove_global(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _name: u32,
        _interface: &str,
    ) {}
}

impl wayland_client::Dispatch<cce_ui::protocol::zcce_inspector_v1::ZcceInspectorV1, ()> for AppState {
    fn event(
        state: &mut Self,
        _proxy: &cce_ui::protocol::zcce_inspector_v1::ZcceInspectorV1,
        event: cce_ui::protocol::zcce_inspector_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            cce_ui::protocol::zcce_inspector_v1::Event::InspectedSurface { app_id, x, y, .. } => {
                if let Some(ref mut st) = state.state {
                    let expected_id = if st.is_detached_network {
                        "circular-network-pane"
                    } else {
                        "cce-designer"
                    };
                    if app_id == expected_id {
                        st.window_x = x;
                        st.window_y = y;
                    }
                }
            }
            _ => {}
        }
    }
}

delegate_compositor!(AppState);
delegate_xdg_shell!(AppState);
delegate_xdg_window!(AppState);
delegate_shm!(AppState);
delegate_seat!(AppState);
delegate_pointer!(AppState);
delegate_keyboard!(AppState);
delegate_registry!(AppState);
delegate_output!(AppState);

impl AppState {
    fn handle_key(&mut self, event: smithay_client_toolkit::seat::keyboard::KeyEvent, state: cce_ui::widget::ElementState) {
        use cce_ui::widget::{Key, KeyEvent, NamedKey};
        let logical_key = match event.keysym {
            xkeysym::Keysym::Escape => Key::Named(NamedKey::Escape),
            xkeysym::Keysym::Return => Key::Named(NamedKey::Enter),
            xkeysym::Keysym::BackSpace => Key::Named(NamedKey::Backspace),
            xkeysym::Keysym::Down => Key::Named(NamedKey::ArrowDown),
            xkeysym::Keysym::Up => Key::Named(NamedKey::ArrowUp),
            xkeysym::Keysym::Left => Key::Named(NamedKey::ArrowLeft),
            xkeysym::Keysym::Right => Key::Named(NamedKey::ArrowRight),
            xkeysym::Keysym::Tab => Key::Named(NamedKey::Tab),
            xkeysym::Keysym::Delete => Key::Named(NamedKey::Delete),
            xkeysym::Keysym::space => Key::Named(NamedKey::Space),
            xkeysym::Keysym::comma => Key::Character(",".into()),
            xkeysym::Keysym::g | xkeysym::Keysym::G => Key::Character("g".into()),
            xkeysym::Keysym::e | xkeysym::Keysym::E => Key::Character("e".into()),
            xkeysym::Keysym::a | xkeysym::Keysym::A => Key::Character("a".into()),
            xkeysym::Keysym::d | xkeysym::Keysym::D => Key::Character("d".into()),
            xkeysym::Keysym::f | xkeysym::Keysym::F => Key::Character("f".into()),
            xkeysym::Keysym::h | xkeysym::Keysym::H => Key::Character("h".into()),
            xkeysym::Keysym::j | xkeysym::Keysym::J => Key::Character("j".into()),
            xkeysym::Keysym::k | xkeysym::Keysym::K => Key::Character("k".into()),
            xkeysym::Keysym::l | xkeysym::Keysym::L => Key::Character("l".into()),
            xkeysym::Keysym::s | xkeysym::Keysym::S => Key::Character("s".into()),
            xkeysym::Keysym::c | xkeysym::Keysym::C => Key::Character("c".into()),
            xkeysym::Keysym::x | xkeysym::Keysym::X => Key::Character("x".into()),
            xkeysym::Keysym::v | xkeysym::Keysym::V => Key::Character("v".into()),
            xkeysym::Keysym::grave => Key::Character("`".into()),
            _ => {
                if let Some(ref text) = event.utf8 {
                    Key::Character(text.clone())
                } else if let Some(ch) = event.keysym.key_char() {
                    Key::Character(ch.to_string())
                } else {
                    return;
                }
            }
        };

        // eprintln!("DEBUG KEY: keysym={:?}, state={:?}, logical_key={:?}", event.keysym, state, logical_key);

        if let Some(st) = &mut self.state {
            match event.keysym {
                xkeysym::Keysym::Control_L | xkeysym::Keysym::Control_R => {
                    st.modifiers.ctrl = state == cce_ui::widget::ElementState::Pressed;
                }
                xkeysym::Keysym::Alt_L | xkeysym::Keysym::Alt_R => {
                    st.modifiers.alt = state == cce_ui::widget::ElementState::Pressed;
                }
                xkeysym::Keysym::Shift_L | xkeysym::Keysym::Shift_R => {
                    st.modifiers.shift = state == cce_ui::widget::ElementState::Pressed;
                }
                xkeysym::Keysym::Super_L | xkeysym::Keysym::Super_R => {
                    st.modifiers.logo = state == cce_ui::widget::ElementState::Pressed;
                }
                _ => {}
            }

            let custom_event = KeyEvent {
                state,
                logical_key,
                text: event.utf8.clone(),
                repeat: false,
                ctrl: st.modifiers.ctrl,
                shift: st.modifiers.shift,
            };

            if state == cce_ui::widget::ElementState::Pressed {
                if is_repeatable_key(&custom_event.logical_key) {
                    self.pressed_key = Some(PressedKey {
                        logical_key: custom_event.logical_key.clone(),
                        text: custom_event.text.clone(),
                        first_pressed: std::time::Instant::now(),
                        last_repeated: std::time::Instant::now(),
                    });
                } else {
                    self.pressed_key = None;
                }
            } else if state == cce_ui::widget::ElementState::Released {
                if let Some(ref pk) = self.pressed_key {
                    if pk.logical_key == custom_event.logical_key {
                        self.pressed_key = None;
                    }
                }
            }

            let ev = WindowEvent::KeyboardInput { event: custom_event };
            self.process_event(ev);
        }
    }

    pub fn process_event(&mut self, ev: WindowEvent) {
        if let Some(state) = &mut self.state {
            let mut changed = state.handle_event(&ev);

            if let Some(seg) = state.path_mut().path_click() {
                if seg < state.current_path.len() {
                    let exited_idx = state.current_path.get(seg).copied();
                    state.current_path.truncate(seg);
                    state.on_path_changed();
                    if let Some(idx) = exited_idx {
                        let pos = {
                            let dir = state.current_dir();
                            if idx < dir.children.len() {
                                Some(dir.children[idx].position)
                            } else {
                                None
                            }
                        };
                        if let Some((pos_x, pos_y)) = pos {
                            state.grid_cursor_col = pos_x as i32;
                            state.grid_cursor_row = pos_y as i32;
                            state.sync_cursor_and_selection();
                            state.upload_vertices();
                        }
                    }
                    changed = true;
                }
            }

            if let Some(action) = state.pending_action.take() {
                state.execute_action(action);
                changed = true;
            }


            // Check context switcher dropdown changes
            for &widget_idx in &[HEADER_IDX, LEFT_MENUBAR_IDX, RIGHT_MENUBAR_IDX, PARAM_MENUBAR_IDX, SPREADSHEET_MENUBAR_IDX] {
                if let Some(new_sel) = state.menu_mut(widget_idx).take_context_change() {
                    let target_pane = match new_sel {
                        0 => LEFT_MENUBAR_IDX,
                        1 => RIGHT_MENUBAR_IDX,
                        2 => PARAM_MENUBAR_IDX,
                        3 => SPREADSHEET_MENUBAR_IDX,
                        4 => HEADER_IDX,
                        _ => continue,
                    };
                    if state.focused_pane != target_pane {
                        state.focused_pane = target_pane;
                        // Make sure the switched pane is visible!
                        match target_pane {
                            LEFT_MENUBAR_IDX => {
                                if !state.show_network {
                                    state.show_network = true;
                                    state.slots.content.set_visible(true);
                                    state.slots.left_menubar.set_visible(true);
                                    state.slots.breadcrumb.set_visible(true);
                                    state.menu_mut(HEADER_IDX).set_item_checked(2, 4, true);
                                }
                            }
                            RIGHT_MENUBAR_IDX => {
                                if !state.show_viewport {
                                    state.show_viewport = true;
                                    state.slots.viewport.set_visible(true);
                                    state.slots.right_menubar.set_visible(true);
                                    state.menu_mut(HEADER_IDX).set_item_checked(2, 5, true);
                                }
                            }
                            PARAM_MENUBAR_IDX => {
                                if !state.show_parameters {
                                    state.show_parameters = true;
                                    state.slots.param.set_visible(true);
                                    state.slots.param_menubar.set_visible(true);
                                    state.menu_mut(HEADER_IDX).set_item_checked(2, 6, true);
                                }
                            }
                            SPREADSHEET_MENUBAR_IDX => {
                                if !state.show_spreadsheet {
                                    state.show_spreadsheet = true;
                                    state.slots.spreadsheet.set_visible(true);
                                    state.slots.spreadsheet_menubar.set_visible(true);
                                    state.menu_mut(HEADER_IDX).set_item_checked(2, 7, true);
                                }
                            }
                            _ => {}
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.menu_mut(HEADER_IDX).menu_click() {
                if menu_idx == 0 { // File
                    match item_idx {
                        0 => { // New Project
                            state.new_project();
                            changed = true;
                        }
                        1 => { // Open
                            state.open_file_chooser();
                            changed = true;
                        }
                        2 => { // Save
                            let path_opt = state.loaded_project_path.clone();
                            if let Some(path) = path_opt {
                                if let Err(e) = state.save_to_file(&path) {
                                    eprintln!("Failed to save project: {:?}", e);
                                    state.update_status_text(&format!("Failed to save: {:?}", e));
                                } else {
                                    state.update_status_text(&format!("Project saved to {}", path.display()));
                                    state.add_recent_file(path);
                                }
                            } else {
                                state.save_file_chooser();
                            }
                            changed = true;
                        }
                        3 => { // Save As
                            state.save_file_chooser();
                            changed = true;
                        }
                        4 => { // Exit
                            state.exit_requested = true;
                        }
                        _ => {}
                    }
                } else if menu_idx == 2 { // View
                    match item_idx {
                        0 => { // Zoom In
                            state.zoom(1.15, None);
                            changed = true;
                        }
                        1 => { // Zoom Out
                            state.zoom(1.0 / 1.15, None);
                            changed = true;
                        }
                        2 => { // Reset Zoom
                            state.grid_size_x = 150.0;
                            state.grid_size_y = 75.0;
                            state.skipped_col_w = 37.5;
                            state.skipped_row_h = 37.5;
                            state.sync_grid_settings();
                            changed = true;
                        }
                        3 => { // Detach Circular Window
                            state.execute_action(Action::DetachCircularWindow);
                            changed = true;
                        }
                        4 => { // Show Network Pane
                            state.show_network = !state.show_network;
                            state.slots.content.set_visible(state.show_network);
                            state.slots.left_menubar.set_visible(state.show_network);
                            state.slots.breadcrumb.set_visible(state.show_network);
                            let val = state.show_network;
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 4, val);
                            if !state.show_network && state.focused_pane == LEFT_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        5 => { // Show Viewport Pane
                            state.show_viewport = !state.show_viewport;
                            state.slots.viewport.set_visible(state.show_viewport);
                            state.slots.right_menubar.set_visible(state.show_viewport);
                            let val = state.show_viewport;
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 5, val);
                            if !state.show_viewport && state.focused_pane == RIGHT_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        6 => { // Show Parameters Pane
                            state.show_parameters = !state.show_parameters;
                            state.slots.param.set_visible(state.show_parameters);
                            state.slots.param_menubar.set_visible(state.show_parameters);
                            let val = state.show_parameters;
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 6, val);
                            if !state.show_parameters && state.focused_pane == PARAM_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        7 => { // Show Spreadsheet Pane
                            state.show_spreadsheet = !state.show_spreadsheet;
                            state.slots.spreadsheet.set_visible(state.show_spreadsheet);
                            state.slots.spreadsheet_menubar.set_visible(state.show_spreadsheet);
                            let val = state.show_spreadsheet;
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 7, val);
                            if !state.show_spreadsheet && state.focused_pane == SPREADSHEET_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.menu_mut(LEFT_MENUBAR_IDX).menu_click() {
                if menu_idx == 0 { // File
                    match item_idx {
                        0 => { // New
                            state.new_project();
                            changed = true;
                        }
                        1 => { // Open
                            state.open_file_chooser();
                            changed = true;
                        }
                        2 => { // Save
                            let path_opt = state.loaded_project_path.clone();
                            if let Some(path) = path_opt {
                                if let Err(e) = state.save_to_file(&path) {
                                    eprintln!("Failed to save project: {:?}", e);
                                    state.update_status_text(&format!("Failed to save: {:?}", e));
                                } else {
                                    state.update_status_text(&format!("Project saved to {}", path.display()));
                                    state.add_recent_file(path);
                                }
                            } else {
                                state.save_file_chooser();
                            }
                            changed = true;
                        }
                        3 => { // Save As
                            state.save_file_chooser();
                            changed = true;
                        }
                        _ => {}
                    }
                } else if menu_idx == 2 { // View
                    match item_idx {
                        0 => { // Zoom In
                            state.zoom(1.15, None);
                            changed = true;
                        }
                        1 => { // Zoom Out
                            state.zoom(1.0 / 1.15, None);
                            changed = true;
                        }
                        2 => {
                            state.circular_network_pane = !state.circular_network_pane;
                            let val = state.circular_network_pane;
                            state.menu_mut(LEFT_MENUBAR_IDX).set_item_checked(2, 2, val);
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_grid_settings();
                            changed = true;
                        }
                        3 => { // Detach Pane
                            state.execute_action(Action::DetachCircularWindow);
                            changed = true;
                        }
                        4 => { // Close Pane
                            state.show_network = false;
                            state.slots.content.set_visible(false);
                            state.slots.left_menubar.set_visible(false);
                            state.slots.breadcrumb.set_visible(false);
                            state.menu_mut(HEADER_IDX).set_item_checked(2, 4, false);
                            if state.focused_pane == LEFT_MENUBAR_IDX {
                                state.focused_pane = get_next_visible_pane(
                                    state.focused_pane,
                                    state.show_network,
                                    state.show_viewport,
                                    state.show_parameters,
                                    state.show_spreadsheet,
                                    false,
                                );
                            }
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_pane_focus();
                            state.sync_nodes();
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.menu_mut(RIGHT_MENUBAR_IDX).menu_click() {
                if menu_idx == 0 {
                    let camera_nodes: Vec<String> = state.current_dir().children.iter()
                        .filter(|c| c.node_type == "camera")
                        .map(|c| c.name.clone())
                        .collect();
                    let mut items = vec!["Default Camera".to_string()];
                    items.extend(camera_nodes);
                    if item_idx < items.len() {
                        state.active_camera = items[item_idx].clone();
                        let active_cam = state.active_camera.clone();
                        for (i, item) in items.iter().enumerate() {
                            state.menu_mut(RIGHT_MENUBAR_IDX).set_item_checked(0, i, item == &active_cam);
                        }
                        changed = true;
                    }
                } else if menu_idx == 3 { // View
                    if item_idx == 0 { // Close Pane
                        state.show_viewport = false;
                        state.slots.viewport.set_visible(false);
                        state.slots.right_menubar.set_visible(false);
                        state.menu_mut(HEADER_IDX).set_item_checked(2, 5, false);
                        if state.focused_pane == RIGHT_MENUBAR_IDX {
                            state.focused_pane = get_next_visible_pane(
                                state.focused_pane,
                                state.show_network,
                                state.show_viewport,
                                state.show_parameters,
                                state.show_spreadsheet,
                                false,
                            );
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
                } else {
                    let action = if menu_idx == 1 {
                        Some(Action::ToggleSquareViewport)
                    } else if menu_idx == 2 {
                        match item_idx {
                            0 => Some(Action::ToggleGrid),
                            1 => Some(Action::ToggleCube),
                            2 => Some(Action::ToggleOrigin),
                            3 => Some(Action::ToggleCameraPivot),
                            _ => None,
                        }
                    } else { None };
                    if let Some(a) = action {
                        state.execute_action(a);
                        changed = true;
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.menu_mut(PARAM_MENUBAR_IDX).menu_click() {
                if menu_idx == 0 { // Preset
                    if let Some(slot_idx) = state.graph().selected_node() {
                        let node_type = state.current_dir().children[slot_idx].node_type.clone();
                        let template_params = state.node_templates.iter()
                            .find(|t| t.node.node_type == node_type)
                            .map(|t| t.node.params.clone());
                        if let Some(params_to_reset) = template_params {
                            if item_idx == 0 { // Default
                                for template_param in &params_to_reset {
                                    if let Some(p) = state.current_dir_mut().children[slot_idx].params.iter_mut().find(|p| p.name == template_param.name) {
                                        p.default = template_param.default.clone();
                                    }
                                }
                            } else if item_idx == 1 { // Custom
                                for template_param in &params_to_reset {
                                    if let Some(p) = state.current_dir_mut().children[slot_idx].params.iter_mut().find(|p| p.name == template_param.name) {
                                        if let Ok(v) = template_param.default.parse::<f32>() {
                                            p.default = format!("{:.2}", v * 1.5);
                                        } else if let Ok(v) = template_param.default.parse::<i32>() {
                                            p.default = format!("{}", v * 2);
                                        } else if template_param.default.contains(':') {
                                            let parts: Vec<&str> = template_param.default.split(':').collect();
                                            let custom_parts: Vec<String> = parts.iter().map(|p_str| {
                                                if let Ok(v) = p_str.parse::<f32>() {
                                                    format!("{:.2}", v * 1.5)
                                                } else {
                                                    p_str.to_string()
                                                }
                                            }).collect();
                                            p.default = custom_parts.join(":");
                                        } else {
                                            p.default = template_param.default.clone();
                                        }
                                    }
                                }
                            }
                            state.sync_nodes();
                            state.rebuild_scene_geometry();
                            state.upload_vertices();
                            changed = true;
                        }
                    }
                } else if menu_idx == 1 { // Reset
                    if item_idx == 0 { // All
                        if let Some(slot_idx) = state.graph().selected_node() {
                            let node_type = state.current_dir().children[slot_idx].node_type.clone();
                            let template_params = state.node_templates.iter()
                                .find(|t| t.node.node_type == node_type)
                                .map(|t| t.node.params.clone());
                            if let Some(params_to_reset) = template_params {
                                for template_param in &params_to_reset {
                                    if let Some(p) = state.current_dir_mut().children[slot_idx].params.iter_mut().find(|p| p.name == template_param.name) {
                                        p.default = template_param.default.clone();
                                    }
                                }
                                state.sync_nodes();
                                state.rebuild_scene_geometry();
                                state.upload_vertices();
                                changed = true;
                            }
                        }
                    }
                } else if menu_idx == 2 { // View
                    if item_idx == 0 { // Close Pane
                        state.show_parameters = false;
                        state.slots.param.set_visible(false);
                        state.slots.param_menubar.set_visible(false);
                        state.menu_mut(HEADER_IDX).set_item_checked(2, 6, false);
                        if state.focused_pane == PARAM_MENUBAR_IDX {
                            state.focused_pane = get_next_visible_pane(
                                state.focused_pane,
                                state.show_network,
                                state.show_viewport,
                                state.show_parameters,
                                state.show_spreadsheet,
                                false,
                            );
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
                }
            }

            if let Some((menu_idx, item_idx)) = state.menu_mut(SPREADSHEET_MENUBAR_IDX).menu_click() {
                if menu_idx == 0 { // View
                    if item_idx == 0 { // Close Pane
                        state.show_spreadsheet = false;
                        state.slots.spreadsheet.set_visible(false);
                        state.slots.spreadsheet_menubar.set_visible(false);
                        state.menu_mut(HEADER_IDX).set_item_checked(2, 7, false);
                        if state.focused_pane == SPREADSHEET_MENUBAR_IDX {
                            state.focused_pane = get_next_visible_pane(
                                state.focused_pane,
                                state.show_network,
                                state.show_viewport,
                                state.show_parameters,
                                state.show_spreadsheet,
                                false,
                            );
                        }
                        state.rebuild_positions();
                        state.apply_layout();
                        state.sync_pane_focus();
                        state.sync_nodes();
                        changed = true;
                    }
                }
            }

            if changed {
                state.sync_layout();
                state.read_panel_offsets();
                state.sync_cursor_and_selection();

                if state.drag_widget == Some(PARAM_IDX) && state.slots.param.is_dragging() {
                    state.sync_parameters_to_project();
                }

                state.sync_nodes();

                // Sync Parameters pane with selected node
                let params = if !state.is_detached_network {
                    state.graph().selected_node().and_then(|sel_idx| {
                        let dir = state.current_dir();
                        if sel_idx < dir.children.len() {
                            Some(param_display(&dir.children[sel_idx].params))
                        } else { None }
                    }).unwrap_or_default()
                } else {
                    vec![]
                };
                state.param_mut().set_display_params(&params);

                state.upload_vertices();
            }

            state.update_status_text(&format!(
                "col: {:.0}  vp: {:.0}  params: {:.0}",
                state.content_left_w(), state.viewport_w(), state.param_w(),
            ));

            if changed {
                state.update_window_title();
                if state.is_detached_network || state.detached_circular_network {
                    state.needs_autosave = true;
                }
                self.redraw = true;
            }
        }
    }

    pub fn handle_user_event(&mut self, event: CustomEvent) {
        let mut needs_redraw = false;
        if let Some(state) = &mut self.state {
            match event {
                CustomEvent::GetState(tx) => {
                    let proj = Project {
                        name: "Project".to_string(),
                        root: state.fs_root.clone(),
                        view_state: ProjectViewState {
                            active_camera: state.active_camera.clone(),
                            pan: (state.pan_x, state.pan_y),
                            current_path: state.current_path.clone(),
                            selected_node: state.graph().selected_node(),
                        },
                    };
                    let json = serde_json::to_string_pretty(&proj).unwrap_or_default();
                    let _ = tx.send(json);
                }
                CustomEvent::PostAction(action, tx) => {
                    let res = match action {
                        HttpAction::Up => {
                            if state.move_up() {
                                needs_redraw = true;
                                Ok("Moved up".to_string())
                            } else {
                                Err("Already at root".to_string())
                            }
                        }
                        HttpAction::Enter { slot } => {
                            let dir = state.current_dir();
                            if slot < dir.children.len() && (dir.children[slot].node_type == "node" || dir.children[slot].node_type == "utility" || !dir.children[slot].children.is_empty()) {
                                state.current_path.push(slot);
                                state.on_path_changed();
                                needs_redraw = true;
                                Ok("Entered subnet".to_string())
                            } else {
                                Err("Not a valid subnet".to_string())
                            }
                        }
                        HttpAction::SetParam { slot, name, value } => {
                            let dir = state.current_dir_mut();
                            if let Some(child) = dir.children.get_mut(slot) {
                                if let Some(p) = child.params.iter_mut().find(|p| p.name == name) {
                                    p.default = value;
                                    state.sync_nodes();
                                    state.rebuild_scene_geometry();
                                    state.upload_vertices();
                                    needs_redraw = true;
                                    Ok("Parameter updated".to_string())
                                } else {
                                    Err(format!("Parameter {} not found", name))
                                }
                            } else {
                                Err("Slot index out of bounds".to_string())
                            }
                        }
                        HttpAction::ResetCamera => {
                            if state.active_camera != "Default Camera" {
                                state.update_active_camera_rotation_reset();
                            } else {
                                state.viewport_mut().rotation_y = 0.0;
                                state.viewport_mut().rotation_x = 0.0;
                            }
                            state.viewport_mut().zoom = 1.0;
                            state.viewport_mut().reset_velocity();
                            needs_redraw = true;
                            Ok("Camera reset".to_string())
                        }
                        HttpAction::Load { path } => {
                            if let Err(e) = state.load_from_file(Path::new(&path)) {
                                Err(format!("Load failed: {:?}", e))
                            } else {
                                needs_redraw = true;
                                Ok("Project loaded".to_string())
                            }
                        }
                        HttpAction::Save { path } => {
                            if let Err(e) = state.save_to_file(Path::new(&path)) {
                                Err(format!("Save failed: {:?}", e))
                            } else {
                                let path_buf = Path::new(&path).to_path_buf();
                                state.loaded_project_path = Some(path_buf.clone());
                                state.add_recent_file(path_buf);
                                needs_redraw = true;
                                Ok("Project saved".to_string())
                            }
                        }
                        HttpAction::ToggleGeometry { slot } => {
                            let active_nodes = state.current_dir().children.len();
                            if slot < active_nodes {
                                if state.current_dir().children[slot].node_type == "utility" {
                                    Err("Cannot toggle geometry visibility on utility nodes".to_string())
                                } else {
                                    let visible = !state.current_dir().children[slot].geometry_visible;
                                    state.current_dir_mut().children[slot].geometry_visible = visible;
                                    state.sync_nodes();
                                    state.rebuild_scene_geometry();
                                    needs_redraw = true;
                                    Ok(format!("Geometry visible: {}", visible))
                                }
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::AddNode { template_name, name, x, y } => {
                            let template_idx = state.node_templates.iter().position(|t| {
                                t.label.to_lowercase() == template_name.to_lowercase()
                                    || t.node.name.to_lowercase() == template_name.to_lowercase()
                            });
                            if let Some(idx) = template_idx {
                                let mut node = state.node_templates[idx].node.clone();
                                let mut allowed = true;
                                let is_in_utility = !state.current_path.is_empty() && state.fs_root.children[state.current_path[0]].node_type == "utility";
                                if is_in_utility {
                                    if crate::geometry::is_geometry_node_type(&node.node_type) {
                                        allowed = false;
                                    }
                                }
                                if !allowed {
                                    Err("Utility nodes cannot contain geometry.".to_string())
                                } else {
                                    let (nx, ny) = state.find_empty_cell(x, y, None);
                                    node.position = (nx, ny);
                                    if let Some(n) = name {
                                        node.name = n;
                                    } else {
                                        node.name = state.get_lowest_unused_name(&node.name);
                                    }
                                    state.current_dir_mut().children.push(node);
                                    state.sync_nodes();
                                    state.rebuild_positions();
                                    state.apply_layout();
                                    state.update_panel_bounds();
                                    state.rebuild_scene_geometry();
                                    state.upload_vertices();
                                    needs_redraw = true;
                                    Ok("Node added".to_string())
                                }
                            } else {
                                Err(format!("Template '{}' not found", template_name))
                            }
                        }
                        HttpAction::DeleteNode { slot } => {
                            if state.delete_node(slot) {
                                needs_redraw = true;
                                Ok("Node deleted".to_string())
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::RenameNode { slot, new_name } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                state.current_dir_mut().children[slot].name = new_name;
                                state.sync_nodes();
                                needs_redraw = true;
                                Ok("Node renamed".to_string())
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::MoveNode { slot, x, y } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                let (nx, ny) = state.find_empty_cell(x, y, Some(slot));
                                state.current_dir_mut().children[slot].position = (nx, ny);
                                state.sync_nodes();
                                state.rebuild_positions();
                                state.apply_layout();
                                state.update_panel_bounds();
                                state.upload_vertices();
                                needs_redraw = true;
                                Ok("Node moved".to_string())
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::AddParam { slot, name, param_type, default } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                let param = ParamDef {
                                    name,
                                    label: String::new(),
                                    param_type,
                                    default,
                                    options: vec![],
                                    min: None,
                                    max: None,
                                    step: None,
                                };
                                state.current_dir_mut().children[slot].params.push(param);
                                state.sync_nodes();
                                needs_redraw = true;
                                Ok("Parameter added".to_string())
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::DeleteParam { slot, name } => {
                            let len = state.current_dir().children.len();
                            if slot < len {
                                let params = &mut state.current_dir_mut().children[slot].params;
                                if let Some(pos) = params.iter().position(|p| p.name == name) {
                                    params.remove(pos);
                                    state.sync_nodes();
                                    needs_redraw = true;
                                    Ok("Parameter deleted".to_string())
                                } else {
                                    Err(format!("Parameter '{}' not found", name))
                                }
                            } else {
                                Err("Slot out of bounds".to_string())
                            }
                        }
                        HttpAction::ToggleCircularPane => {
                            state.circular_network_pane = !state.circular_network_pane;
                            let val = state.circular_network_pane;
                            state.menu_mut(LEFT_MENUBAR_IDX).set_item_checked(2, 2, val);
                            state.rebuild_positions();
                            state.apply_layout();
                            state.sync_grid_settings();
                            state.upload_vertices();
                            needs_redraw = true;
                            Ok(format!("Circular pane: {}", state.circular_network_pane))
                        }
                        HttpAction::MenuClick { widget_idx, menu_idx, item_idx } => {
                            state.menu_mut(widget_idx).trigger_menu_click(menu_idx, item_idx);
                            self.process_event(WindowEvent::CursorMoved { position: LocalPosition { x: -9999.0, y: -9999.0 } });
                            needs_redraw = true;
                            Ok("Menu clicked".to_string())
                        }
                        HttpAction::MenuClosed { widget_idx, menu_idx } => {
                            if state.active_menu_cloud_idx == Some((widget_idx, menu_idx)) {
                                state.active_menu_cloud_pid = None;
                                state.active_menu_cloud_idx = None;
                            }
                            Ok("Menu closed".to_string())
                        }

                    };
                    let _ = tx.send(res);
                }
            }
        } else {
            match event {
                CustomEvent::GetState(tx) => {
                    let _ = tx.send("null".to_string());
                }
                CustomEvent::PostAction(_, tx) => {
                    let _ = tx.send(Err("State not initialized".to_string()));
                }
            }
        }
        if needs_redraw {
            if let Some(state) = &mut self.state {
                state.update_window_title();
                if state.is_detached_network || state.detached_circular_network {
                    state.needs_autosave = true;
                }
            }
            self.redraw = true;
        }
    }
}


