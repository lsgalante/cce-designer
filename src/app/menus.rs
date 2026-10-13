//! `State::menus`: the context menus the designer routes itself. Every
//! one is cce-ui's one `context_menu` thread-local — its geometry and
//! paint live there — so what the app keeps is which of them is open
//! (`*_active`, or the slot it was opened on) and what each shown row
//! does (`*_actions`, parallel to the rows), plus where a menu, a page or
//! the dialog was turned from (`crate::menu_page`) and the node the
//! rename dialog is open on.

use super::{NetworkMenuAction, NodeMenuAction, ParamMenuAction, PlaybarMenuAction, ViewportMenuAction, ViewportMenuPage};

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
