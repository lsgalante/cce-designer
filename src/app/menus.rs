//! `State::menus`: the context menus the designer routes itself. Every
//! one is cce-ui's one `context_menu` thread-local — its geometry and
//! paint live there — so what the app keeps is which of them is open
//! (`*_active`, or the slot it was opened on) and what each shown row
//! does (`*_actions`, parallel to the rows), plus where a menu, a page or
//! the dialog was turned from (`crate::menu_page`) and the node the
//! rename dialog is open on. The menus' row actions are defined here too.

pub struct ContextMenus {
    /// The node right-click context menu: the targeted node slot and the
    /// actions parallel to the visible items pushed into `context_menu::show`.
    /// `None` when no menu is open. The menu's geometry/paint lives in the
    /// cce-ui `context_menu` thread-local; these just remember what to do on
    /// click, since the designer routes menu clicks itself.
    pub node_menu_slot: Option<usize>,
    pub node_menu_actions: Vec<NodeMenuAction>,
    /// The viewport right-click menu (same thread-local `context_menu`
    /// machinery as the node menu; this flag says the open menu is OURS).
    pub viewport_menu_active: bool,
    pub viewport_menu_actions: Vec<ViewportMenuAction>,
    /// Which page of the viewport menu is up: `None` for the menu itself.
    pub viewport_menu_page: Option<ViewportMenuPage>,
    /// The network editor's right-click menu — the same thread-local again,
    /// with the flag saying the open menu is this one.
    pub network_menu_active: bool,
    pub network_menu_actions: Vec<NetworkMenuAction>,
    /// The menu the network menu was turned to from — the viewport's, while
    /// the network overlays the scene — which its back band returns to.
    /// None when a right press on the graph opened it.
    pub network_menu_from: Option<crate::menu_page::MenuOrigin>,
    /// The grid cell under the viewport menu's right press, while the
    /// network has no plate and the press was over its area: where the
    /// grid cursor goes if the menu is turned to the network's, so Add Node
    /// places at the cell pointed at, as the plated network's press does.
    pub network_menu_cell: Option<(i32, i32)>,
    /// A parameter row's right-click menu — the same thread-local; the
    /// target is (node id, parameter name) rather than a slot and a row, so
    /// it holds across a re-layout of the pane.
    pub param_menu_active: bool,
    pub param_menu_actions: Vec<ParamMenuAction>,
    /// The row the parameter menu is open on: (node id, parameter name).
    pub param_menu_target: Option<(String, String)>,
    /// The playbar's right-click menu, the same contract.
    pub playbar_menu_active: bool,
    pub playbar_menu_actions: Vec<PlaybarMenuAction>,
    /// The plate menu — the same `context_menu` thread-local again; the
    /// slot says which plate's menu is open.
    pub plate_menu_slot: Option<usize>,
    pub plate_menu_actions: Vec<crate::plate_menu::PlateMenuAction>,
    /// The menu a plate PAGE was turned to from (the playbar's Plate row),
    /// which its back band returns to (`open_plate_page`).
    pub plate_page_from: Option<crate::menu_page::MenuOrigin>,
    /// The menu the dialog was turned to from by a page row (Add Node,
    /// Rename), which a swipe back from it shows again — see
    /// `crate::menu_page`.
    pub dialog_from: Option<crate::menu_page::MenuOrigin>,
    /// The dialog's own modes it was turned through on the way to the one
    /// up, nearest last: Commands under Group Markers.
    pub dialog_trail: Vec<crate::dialog::Mode>,
    /// The node the rename dialog is open on, by id.
    pub rename_target: Option<String>,
}

/// One entry in a node's right-click context menu, parallel to the visible
/// labels shown via `context_menu::show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeMenuAction {
    /// Dive into the node's subnet (the double-click behavior).
    Enter,
    /// Flip the node's geometry visibility.
    ToggleGeometry,
    ToggleBypass,
    /// Enter/exit the curve viewer state (curve nodes only).
    EditCurve,
    /// Open the dialog on the node's name.
    Rename,
    /// Remove the node.
    Delete,
}

/// One entry in a parameter row's right-click menu — Houdini's Copy
/// Parameter / Paste Relative References, and the switch between a value
/// and an expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMenuAction {
    CopyParameter,
    PasteRelative,
    PasteAbsolute,
    /// Keep the value, but as an expression: the row turns to text so
    /// arithmetic can be typed around it.
    EditExpression,
    /// Houdini's Delete Channels: the expression's CURRENT value, as a value.
    DeleteExpression,
    /// Show a float3 row's trackball beside its sliders, or the sliders
    /// alone — the parameter's `view`, kept with the instance.
    ShowTrackball,
    HideTrackball,
    Separator,
    /// A header row that reads something out — the control's kind, the
    /// value's type — and runs nothing.
    Info,
}

/// A page of the viewport menu: a row of the menu that the menu turns into
/// (see `crate::menu_page`), with a way back to the menu at its top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewportMenuPage {
    /// How the geometry itself is drawn: the wireframe and the surface.
    Style,
    /// What is drawn ON it: the points, and the markers, numbers and
    /// normals of its points, primitives and vertices.
    Markers,
}

impl ViewportMenuPage {
    pub fn label(self) -> &'static str {
        match self {
            ViewportMenuPage::Style => "Style",
            ViewportMenuPage::Markers => "Markers",
        }
    }
}

/// The 3D viewport's right-click context menu actions: the menu's own rows
/// and its pages'.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewportMenuAction {
    /// A row that turns the menu into one of its pages.
    Page(ViewportMenuPage),
    /// The Network row, while the network has no plate: a page turning the
    /// menu into the network's own menu, which a right press on empty graph
    /// space can no longer reach — that space is the scene's.
    NetworkPage,
    /// Move the active camera so the visible node geometry fills the view.
    FrameAll,
    /// Put the pivot plane at true size: one world unit (the Guides "World
    /// Unit") spans its real length on this display.
    OneToOne,
    /// Run a registry command — the display toggles, so the menu's rows are
    /// the palette's and a row is exactly as scriptable as its command.
    Command(&'static str),
    /// Set the shading mode: smooth (true) or flat. A radio pair over the
    /// one `toggle_smooth_shading` flag, running the toggle only when the
    /// pick differs, so picking the mode already on is a no-op rather than
    /// a flip.
    Shading(bool),
    /// The polygon (geometry fill) opacity, as a SLIDER row (cce-ui's
    /// `context_menu::MenuSlider`): the wheel steps it by 5%, a press on its
    /// band drags it, and the menu stays open. Picking the row runs nothing
    /// — the slider's changes arrive through `drain_viewport_menu_slider`.
    OpacitySlider,
    /// The wire pass's thickness in px, a slider row under Show Wireframe:
    /// 1–8 like the palette's Wire Thickness row, half a pixel a notch.
    WireThicknessSlider,
    /// The wire pass's own opacity, in percent like the polygon Opacity row
    /// and stepped the same 5%: the two are independent, so a translucent
    /// fill can carry a solid lattice and the other way round.
    WireOpacitySlider,
    /// Point Marker Size in world units, the Show Point Markers overlay's
    /// radius: the palette row's 0.005–0.1.
    PointMarkerSizeSlider,
    /// Group Marker Size in world units, the Selected-Group markers'
    /// radius: the palette row's 0–0.2.
    GroupMarkerSizeSlider,
    /// Pull Arrow Scale, the pull arrows' length as a multiple of the true
    /// displacement: the palette row's 0.25–10.
    PullArrowScaleSlider,
    /// Camera Pivot Size, the pivot marker's scale, under its switch: 0–1
    /// by a twentieth. The palette row is the coarse one, in tenths up
    /// to 5.
    CameraPivotSizeSlider,
    /// A "-" row: engraved, inert.
    Separator,
}

/// The playbar's right-click menu (2026-09-30): the transport's commands,
/// its Repeat switch, and the settings that are the timeline's — the
/// playback rate and the frame range — as slider rows, the viewport menu's
/// shape. Rows dispatch as the network menu's do, by command id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybarMenuAction {
    /// Run `command::by_id(id)`; a toggle command's row carries its mark.
    Command(&'static str),
    /// Playback Rate, in frames a second: 1–120 by one.
    FpsSlider,
    /// The frame range's near end, 1–999 by one; kept below the far end.
    StartFrameSlider,
    /// The frame range's far end, 2–1000 by one; kept above the near end.
    EndFrameSlider,
    /// The Plate row: a page turning the menu into the playbar's plate rows
    /// (Collapse, Detach).
    PlatePage,
    /// A "-" row: engraved, inert.
    Separator,
}

/// The network editor's right-click context menu (on empty space — a press on
/// a node still opens that node's menu). Every row but the separator names a
/// COMMAND ID rather than a piece of work, so the labels cannot drift from the
/// palette's and a row is exactly as scriptable as the command behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMenuAction {
    /// Run `command::by_id(id)` — the row's label came from the same row.
    Command(&'static str),
    /// A "-" row: engraved, inert.
    Separator,
}

/// The command ids the network context menu offers, in order; `None` is a
/// separator. Add Node leads because right-clicking empty space USED to open
/// the add-node palette outright, and that is still the common reason to come
/// here.
pub const NETWORK_MENU_COMMANDS: &[Option<&'static str>] = &[
    Some("add_node"),
    None,
    Some("layout_nodes"),
    Some("frame_all"),
    Some("frame_cursor"),
    None,
    Some("reset_zoom"),
    Some("toggle_circular_pane"),
];
