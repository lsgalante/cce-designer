//! The widget roster: the app's top-level widgets in fixed, statically typed slots.
//!
//! The designer does not build a dynamic widget tree — every top-level pane, bar and
//! plate lives in a named field of [`WidgetSlots`], and the `*_IDX` constants address
//! those same fields positionally for the genuinely index-driven paths (draw order,
//! focus cycling, broadcast loops). Split out of `app.rs` so that adding or reordering
//! a slot is a change to one file: the constant, the field, and the four dispatch
//! arms are all here, as are the typed accessors that assert each slot's concrete type.

use cce_ui::widget::{
    Adapted, Breadcrumb, Graph, MenuBar, ParametersBg, Splitter, Spreadsheet, StatusBar,
    WidgetHost,
};

use crate::playbar::Playbar;
use crate::viewport_3d::Viewport3D;

pub const HEADER_IDX: usize = 0;
pub const CONTENT_IDX: usize = 1;
pub const SPLITTER1_IDX: usize = 2;
pub const VIEWPORT_IDX: usize = 3;
pub const SPLITTER2_IDX: usize = 4;
pub const PARAM_IDX: usize = 5;
pub const CANVAS_IDX: usize = 6;
pub const LEFT_MENUBAR_IDX: usize = 7;
pub const RIGHT_MENUBAR_IDX: usize = 8;
pub const PARAM_MENUBAR_IDX: usize = 9;
pub const STATUS_IDX: usize = 10;
pub const BREADCRUMB_IDX: usize = 11;
pub const SPREADSHEET_IDX: usize = 12;
pub const SPREADSHEET_MENUBAR_IDX: usize = 13;
pub const NETWORK_PANEL_IDX: usize = 14;
pub const PLAYBAR_IDX: usize = 15;

pub const WIDGET_COUNT: usize = 16;

/// The roster, concretely typed (Phase 6bb): every slot's type is statically known — the
/// old `Vec<Box<dyn WidgetHost>>` erased that and pinned `WidgetHost`'s full surface through the
/// broadcast loops. Boxed as a whole so registered widget pointers stay stable while the
/// containing `State` moves. The `*_IDX` constants keep addressing the same slots through
/// `get_dyn`/`get_dyn_mut` for the genuinely index-driven paths (draw order, focus cycling,
/// broadcast loops); everything else reaches the concrete field.
pub struct WidgetSlots {
    pub header: Adapted<MenuBar>,
    pub content: Adapted<Graph>,
    pub splitter1: Adapted<Splitter>,
    pub viewport: Adapted<Viewport3D>,
    pub splitter2: Adapted<Splitter>,
    pub param: Adapted<ParametersBg>,
    pub canvas: Adapted<Canvas>,
    pub left_menubar: Adapted<MenuBar>,
    pub right_menubar: Adapted<MenuBar>,
    pub param_menubar: Adapted<MenuBar>,
    pub status: Adapted<StatusBar>,
    pub breadcrumb: Adapted<Breadcrumb>,
    pub spreadsheet: Adapted<Spreadsheet>,
    pub spreadsheet_menubar: Adapted<MenuBar>,
    pub network_panel: Adapted<PassivePlate>,
    pub playbar: Adapted<Playbar>,
}

impl WidgetSlots {

    // Per-slot drag queries (the ControlPanel endgame took `draggable`/`is_dragging`
    // off `WidgetHost`): the roster routes an index to the concrete slot's inherent
    // `Adapted` read, like the other value drains.
    pub fn draggable(&self, idx: usize) -> bool {
        match idx {
            HEADER_IDX => self.header.draggable(),
            CONTENT_IDX => self.content.draggable(),
            SPLITTER1_IDX => self.splitter1.draggable(),
            VIEWPORT_IDX => self.viewport.draggable(),
            SPLITTER2_IDX => self.splitter2.draggable(),
            PARAM_IDX => self.param.draggable(),
            CANVAS_IDX => self.canvas.draggable(),
            LEFT_MENUBAR_IDX => self.left_menubar.draggable(),
            RIGHT_MENUBAR_IDX => self.right_menubar.draggable(),
            PARAM_MENUBAR_IDX => self.param_menubar.draggable(),
            STATUS_IDX => self.status.draggable(),
            BREADCRUMB_IDX => self.breadcrumb.draggable(),
            SPREADSHEET_IDX => self.spreadsheet.draggable(),
            SPREADSHEET_MENUBAR_IDX => self.spreadsheet_menubar.draggable(),
            NETWORK_PANEL_IDX => self.network_panel.draggable(),
            PLAYBAR_IDX => self.playbar.draggable(),
            _ => panic!("widget slot index out of range: {idx}"),
        }
    }

    pub fn is_dragging(&self, idx: usize) -> bool {
        match idx {
            HEADER_IDX => self.header.is_dragging(),
            CONTENT_IDX => self.content.is_dragging(),
            SPLITTER1_IDX => self.splitter1.is_dragging(),
            VIEWPORT_IDX => self.viewport.is_dragging(),
            SPLITTER2_IDX => self.splitter2.is_dragging(),
            PARAM_IDX => self.param.is_dragging(),
            CANVAS_IDX => self.canvas.is_dragging(),
            LEFT_MENUBAR_IDX => self.left_menubar.is_dragging(),
            RIGHT_MENUBAR_IDX => self.right_menubar.is_dragging(),
            PARAM_MENUBAR_IDX => self.param_menubar.is_dragging(),
            STATUS_IDX => self.status.is_dragging(),
            BREADCRUMB_IDX => self.breadcrumb.is_dragging(),
            SPREADSHEET_IDX => self.spreadsheet.is_dragging(),
            SPREADSHEET_MENUBAR_IDX => self.spreadsheet_menubar.is_dragging(),
            NETWORK_PANEL_IDX => self.network_panel.is_dragging(),
            PLAYBAR_IDX => self.playbar.is_dragging(),
            _ => panic!("widget slot index out of range: {idx}"),
        }
    }

    pub fn get_dyn(&self, idx: usize) -> &(dyn WidgetHost + 'static) {
        match idx {
            HEADER_IDX => &self.header,
            CONTENT_IDX => &self.content,
            SPLITTER1_IDX => &self.splitter1,
            VIEWPORT_IDX => &self.viewport,
            SPLITTER2_IDX => &self.splitter2,
            PARAM_IDX => &self.param,
            CANVAS_IDX => &self.canvas,
            LEFT_MENUBAR_IDX => &self.left_menubar,
            RIGHT_MENUBAR_IDX => &self.right_menubar,
            PARAM_MENUBAR_IDX => &self.param_menubar,
            STATUS_IDX => &self.status,
            BREADCRUMB_IDX => &self.breadcrumb,
            SPREADSHEET_IDX => &self.spreadsheet,
            SPREADSHEET_MENUBAR_IDX => &self.spreadsheet_menubar,
            NETWORK_PANEL_IDX => &self.network_panel,
            PLAYBAR_IDX => &self.playbar,
            _ => panic!("widget slot index out of range: {idx}"),
        }
    }

    pub fn get_dyn_mut(&mut self, idx: usize) -> &mut (dyn WidgetHost + 'static) {
        match idx {
            HEADER_IDX => &mut self.header,
            CONTENT_IDX => &mut self.content,
            SPLITTER1_IDX => &mut self.splitter1,
            VIEWPORT_IDX => &mut self.viewport,
            SPLITTER2_IDX => &mut self.splitter2,
            PARAM_IDX => &mut self.param,
            CANVAS_IDX => &mut self.canvas,
            LEFT_MENUBAR_IDX => &mut self.left_menubar,
            RIGHT_MENUBAR_IDX => &mut self.right_menubar,
            PARAM_MENUBAR_IDX => &mut self.param_menubar,
            STATUS_IDX => &mut self.status,
            BREADCRUMB_IDX => &mut self.breadcrumb,
            SPREADSHEET_IDX => &mut self.spreadsheet,
            SPREADSHEET_MENUBAR_IDX => &mut self.spreadsheet_menubar,
            NETWORK_PANEL_IDX => &mut self.network_panel,
            PLAYBAR_IDX => &mut self.playbar,
            _ => panic!("widget slot index out of range: {idx}"),
        }
    }

    /// Roster index of the slot at `target_addr` (a thin widget address — the comparison
    /// never dereferences; callers pass `ptr as *const ()`).
    pub fn find_index(&self, target_addr: *const ()) -> Option<usize> {
        (0..WIDGET_COUNT).position(|i| {
            let w_ptr = self.get_dyn(i) as *const dyn WidgetHost as *const ();
            w_ptr == target_addr
        })
    }

    // Roster accessors on CONCRETE types (Phase 6aw, controller decision option 2): each
    // index's type is known statically, so the capability traits are reached by downcast +
    // Deref instead of WidgetHost's deleted as_*_controller discovery hooks. Signatures keep
    // returning the narrow trait objects so the ~40 call sites stay unchanged. The dynamic
    // `idx` of menu()/menu_mut() only ever receives the five menubar indexes.
    pub fn viewport(&self) -> &Viewport3D {
        self.viewport
            .as_any()
            .downcast_ref::<Viewport3D>()
            .expect("VIEWPORT_IDX must be a Viewport3D")
    }

    pub fn viewport_mut(&mut self) -> &mut Viewport3D {
        self.viewport
            .as_any_mut()
            .downcast_mut::<Viewport3D>()
            .expect("VIEWPORT_IDX must be a Viewport3D")
    }

    /// The menu-capable roster entries are exactly the `Adapted<MenuBar>` bars (Phase 6aw
    /// concrete typing); `None` for everything else.
    pub fn menubar_at(&self, idx: usize) -> Option<&MenuBar> {
        // Adapted::as_any exposes the INNER widget, so the downcast targets MenuBar itself.
        self.get_dyn(idx).as_any().downcast_ref::<MenuBar>()
    }

    pub fn menu(&self, idx: usize) -> &dyn cce_ui::widget::MenuController {
        self.get_dyn(idx).as_any().downcast_ref::<MenuBar>().expect("not a MenuBar")
    }

    pub fn menu_mut(&mut self, idx: usize) -> &mut dyn cce_ui::widget::MenuController {
        self.get_dyn_mut(idx).as_any_mut().downcast_mut::<MenuBar>().expect("not a MenuBar")
    }

    pub fn graph(&self) -> &dyn cce_ui::widget::GraphController {
        self.content.as_any().downcast_ref::<Graph>().expect("CONTENT_IDX must be a Graph")
    }

    pub fn graph_mut(&mut self) -> &mut dyn cce_ui::widget::GraphController {
        self.content.as_any_mut().downcast_mut::<Graph>().expect("CONTENT_IDX must be a Graph")
    }

    pub fn param(&self) -> &dyn cce_ui::widget::ParamController {
        self.param.as_any().downcast_ref::<ParametersBg>().expect("PARAM_IDX must be a ParametersBg")
    }

    pub fn param_mut(&mut self) -> &mut dyn cce_ui::widget::ParamController {
        self.param.as_any_mut().downcast_mut::<ParametersBg>().expect("PARAM_IDX must be a ParametersBg")
    }

    pub fn spreadsheet_mut(&mut self) -> &mut dyn cce_ui::widget::SpreadsheetController {
        self.spreadsheet.as_any_mut().downcast_mut::<Spreadsheet>().expect("SPREADSHEET_IDX must be a Spreadsheet")
    }

    pub fn path_mut(&mut self) -> &mut dyn cce_ui::widget::PathController {
        self.breadcrumb.as_any_mut().downcast_mut::<Breadcrumb>().expect("BREADCRUMB_IDX must be a Breadcrumb")
    }
}


/// Dissolved cce-ui `Plate` (Phase 6as): a passive panel wearing the parameter
/// plate's fill — same tint, opacity, and blur-behind marker (`param_plate_fill`),
/// so the network plate's bevel rolls exactly like the params plate's and tracks a
/// live retint/opacity/blur toggle — scaled by the network fade. No children, no
/// events.
pub struct PassivePlate {
    pub network_opacity: f32,
    /// Circular hit shape while the network pane is round (the legacy Plate marker).
    curved_circle: Option<(f32, f32, f32)>,
}

impl PassivePlate {
    pub fn new() -> cce_ui::widget::Adapted<PassivePlate> {
        cce_ui::widget::Adapted::new(Self {
            network_opacity: 1.0,
            curved_circle: None,
        })
    }

    pub fn set_network_opacity(&mut self, opacity: f32) {
        self.network_opacity = opacity;
    }

    pub fn set_curved_circle(&mut self, circle: Option<(f32, f32, f32)>) {
        self.curved_circle = circle;
    }
}

impl cce_ui::widget::Layout for PassivePlate {}

impl cce_ui::widget::Paint for PassivePlate {
    /// Nothing: the plate's fill AND border are drawn by `append_widget_plate` (or, in
    /// circular mode, the circle+arc branch) in the designer's hand-ordered paint walk.
    /// The default `paint` would emit a plain square-cornered quad of the whole rect,
    /// which the walk then re-drew through `extra_quads` ON TOP of the rounded plate —
    /// square corners over the rounded ones.
    fn paint(&self, _rect: cce_ui::scene::layout::Rect, _ctx: &mut cce_ui::scene::paint::PaintCtx) {}

    fn color(&self) -> [f32; 4] {
        // `param_plate_fill` folds in the plate opacity and the blur marker; the
        // network fade scales the (possibly negative) alpha without flipping its sign.
        let mut c = cce_ui::colors::param_plate_fill();
        c[3] *= self.network_opacity;
        c
    }

    fn corner_style(&self, _rect: cce_ui::scene::layout::Rect) -> Option<(f32, (bool, bool, bool, bool))> {
        let r = cce_ui::layout::plate_corner_radius();
        let on = r > 0.0;
        Some((r, (on, on, on, on)))
    }

    fn solid_border(&self) -> Option<([f32; 4], f32)> {
        if let Some(bc) = cce_ui::colors::plate_border_color() {
            Some((bc, cce_ui::colors::plate_border_thickness()))
        } else {
            None
        }
    }
}

impl cce_ui::widget::Input for PassivePlate {
    fn hit(&self, rect: cce_ui::scene::layout::Rect, x: f32, y: f32) -> bool {
        if let Some((cx, cy, r)) = self.curved_circle {
            let dx = x - cx;
            let dy = y - cy;
            return dx * dx + dy * dy <= r * r;
        }
        // Exclusive right/bottom edges, like the legacy Plate hit test.
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    }
}


/// App-owned copy of the dissolved cce-ui `Canvas` (Phase 6ay part 2): the transparent
/// hit-through pane behind the network area. Verbatim; dies with the machinery retype.
pub struct Canvas;

impl Canvas {
    pub fn new() -> cce_ui::widget::Adapted<Canvas> { cce_ui::widget::Adapted::new(Canvas) }
}

impl cce_ui::widget::Layout for Canvas {}

impl cce_ui::widget::Paint for Canvas {
    fn color(&self) -> [f32; 4] { [0.0, 0.0, 0.0, 0.0] }
}

impl cce_ui::widget::Input for Canvas {
    // Hit-through: the pane never claims the pointer (the graph decides its own hits).
    fn hit(&self, _rect: cce_ui::scene::layout::Rect, _x: f32, _y: f32) -> bool { false }
}
