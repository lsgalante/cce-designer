//! The widget roster: the app's top-level widgets in fixed, statically typed slots.
//!
//! The designer does not build a dynamic widget tree — every top-level pane, bar and
//! plate lives in a named field of [`WidgetSlots`], and the `*_IDX` constants address
//! those same fields positionally for the genuinely index-driven paths (draw order,
//! focus cycling, broadcast loops). The whole roster is declared once, in the
//! `widget_roster!` invocation below — one line per slot, from which the constants,
//! the struct fields and every index→field dispatch are generated. The typed accessors
//! that assert each slot's concrete type are hand-written, below the macro.

use cce_ui::widget::{
    Adapted, Breadcrumb, Graph, ImageView, MenuBar, ParametersBg, Splitter, Spreadsheet,
    StatusBar, WidgetHost,
};

use crate::playbar::Playbar;
use crate::viewport_3d::Viewport3D;

/// Declares the whole roster from one line per slot: `INDEX_CONST: field: WidgetType`.
///
/// Declaration order *is* slot order: the `*_IDX` constants are numbered from it and
/// `WIDGET_COUNT` falls out of the length. The single list below generates the
/// constants, the `WidgetSlots` fields, and all four index→field dispatch matches, so
/// adding or reordering a pane is one line instead of six hand-kept edits whose only
/// backstop was a runtime panic.
macro_rules! widget_roster {
    ($($idx:ident : $field:ident : $ty:ty),+ $(,)?) => {
        widget_roster!(@number 0usize; $($idx)+);

        /// The roster, concretely typed (Phase 6bb): every slot's type is statically known — the
        /// old `Vec<Box<dyn WidgetHost>>` erased that and pinned `WidgetHost`'s full surface through the
        /// broadcast loops. Boxed as a whole so registered widget pointers stay stable while the
        /// containing `State` moves. The `*_IDX` constants keep addressing the same slots through
        /// `get_dyn`/`get_dyn_mut` for the genuinely index-driven paths (draw order, focus cycling,
        /// broadcast loops); everything else reaches the concrete field.
        pub struct WidgetSlots {
            $(pub $field: Adapted<$ty>,)+
        }

        impl WidgetSlots {
            // Per-slot drag queries (the ControlPanel endgame took `draggable`/`is_dragging`
            // off `WidgetHost`): the roster routes an index to the concrete slot's inherent
            // `Adapted` read, like the other value drains.
            pub fn draggable(&self, idx: usize) -> bool {
                match idx {
                    $($idx => self.$field.draggable(),)+
                    _ => slot_out_of_range(idx),
                }
            }

            pub fn is_dragging(&self, idx: usize) -> bool {
                match idx {
                    $($idx => self.$field.is_dragging(),)+
                    _ => slot_out_of_range(idx),
                }
            }

            pub fn get_dyn(&self, idx: usize) -> &(dyn WidgetHost + 'static) {
                match idx {
                    $($idx => &self.$field,)+
                    _ => slot_out_of_range(idx),
                }
            }

            pub fn get_dyn_mut(&mut self, idx: usize) -> &mut (dyn WidgetHost + 'static) {
                match idx {
                    $($idx => &mut self.$field,)+
                    _ => slot_out_of_range(idx),
                }
            }
        }
    };

    // Number the constants in declaration order; what is left over at the end is the count.
    (@number $n:expr;) => {
        pub const WIDGET_COUNT: usize = $n;
    };
    (@number $n:expr; $head:ident $($rest:ident)*) => {
        pub const $head: usize = $n;
        widget_roster!(@number $n + 1; $($rest)*);
    };
}

/// Every dispatch match needs a `_` arm — the compiler cannot see that the constant
/// patterns cover `0..WIDGET_COUNT` — and `!` fits all four return types.
#[cold]
#[inline(never)]
fn slot_out_of_range(idx: usize) -> ! {
    panic!("widget slot index out of range: {idx}")
}

widget_roster! {
    HEADER_IDX:              header:              MenuBar,
    CONTENT_IDX:             content:             Graph,
    SPLITTER1_IDX:           splitter1:           Splitter,
    VIEWPORT_IDX:            viewport:            Viewport3D,
    SPLITTER2_IDX:           splitter2:           Splitter,
    PARAM_IDX:               param:               ParametersBg,
    CANVAS_IDX:              canvas:              Canvas,
    LEFT_MENUBAR_IDX:        left_menubar:        MenuBar,
    RIGHT_MENUBAR_IDX:       right_menubar:       MenuBar,
    PARAM_MENUBAR_IDX:       param_menubar:       MenuBar,
    STATUS_IDX:              status:              StatusBar,
    BREADCRUMB_IDX:          breadcrumb:          Breadcrumb,
    SPREADSHEET_IDX:         spreadsheet:         Spreadsheet,
    SPREADSHEET_MENUBAR_IDX: spreadsheet_menubar: MenuBar,
    NETWORK_PANEL_IDX:       network_panel:       PassivePlate,
    PLAYBAR_IDX:             playbar:             Playbar,
    // The second network editor (plate + graph + breadcrumb): an independent
    // VIEW of the same project with its own current path, created and placed
    // through the plate corner menus' tab rows. Appended so the established
    // slot indexes stay stable.
    NETWORK_PANEL2_IDX:      network_panel2:      PassivePlate,
    CONTENT2_IDX:            content2:            Graph,
    BREADCRUMB2_IDX:         breadcrumb2:         Breadcrumb,
    // The 2D page context's surface, sharing the viewport's rect and shown in
    // its place when the displayed level holds a page. Appended, like the
    // second network editor, so established slot indexes stay stable.
    PAGE_IDX:                page_view:           ImageView,
    // The Alt+D dialog: the frame (plate, tabs, query line, command list) and
    // its settings body, a second ParametersBg so a control in the dialog is
    // the same control as one in the params pane. Appended, like every slot
    // since the second network editor, so established indexes stay stable.
    DIALOG_IDX:              dialog:              crate::dialog::Dialog,
    DIALOG_PARAMS_IDX:       dialog_params:       ParametersBg,
}

impl WidgetSlots {
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

    /// The dialog's settings body. A `ParametersBg` like `PARAM_IDX`, reached
    /// through the same controller trait — the writeback in
    /// `sync_dialog_settings_to_project` is the params pane's, pointed at the
    /// dialog's row table instead of the selected node's params.
    pub fn dialog_params(&self) -> &dyn cce_ui::widget::ParamController {
        self.dialog_params
            .as_any()
            .downcast_ref::<ParametersBg>()
            .expect("DIALOG_PARAMS_IDX must be a ParametersBg")
    }

    pub fn dialog_params_mut(&mut self) -> &mut dyn cce_ui::widget::ParamController {
        self.dialog_params
            .as_any_mut()
            .downcast_mut::<ParametersBg>()
            .expect("DIALOG_PARAMS_IDX must be a ParametersBg")
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
