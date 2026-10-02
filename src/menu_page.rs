//! Page rows: a row of a context menu that TURNS the menu into another
//! plate where it stands — a page of more rows, or the dialog — with a way
//! back.
//!
//! Until 2026-10-02 there were two kinds of such row. The viewport menu's
//! Style and Markers flew a second menu out beside themselves on hover
//! (cce-ui's submenu), while the network menu's Add Node, the plate menu's
//! Add Tab, the viewport menu's Attribute Visualizers and the node menu's
//! Rename replaced the menu with another plate on a click — one with a Back
//! row, the others with no way back at all. One idea, two gestures, and the
//! user could not tell from a row which it was.
//!
//! Now every such row is a PAGE row (cce-ui's `context_menu::set_row_page`):
//! it wears `›`, and a press on it, or a two-finger swipe to the side with
//! the pointer on it, turns the menu into what it names, the new plate's
//! top-left where the menu's was. A swipe the other way goes back, from
//! anywhere on the page; a page that is a menu also has a back band across
//! its top (`‹ Viewport`) for a press. Which way is forward follows the
//! content, as a horizontal list does: see `cce_ui::widget::side_swipe`.
//!
//! - A page of rows is shown by the menu that owns it (`put_up_menu` with
//!   an `at`): the viewport menu's Style and Markers, the plate rows' Add
//!   Tab — whose back band returns to whichever menu it was turned from,
//!   the network or playbar menu or a stub's plate menu.
//! - The dialog turned to from a row (`State::dialog_from`) goes back to
//!   that menu, shown again at the dialog's corner; inside the dialog, a
//!   mode turned to from another (Group Markers from the palette, one
//!   visualizer from the list) goes back to it first (`State::dialog_trail`).
//!
//! What runs a turn is [`State::run_menu_turn`], which a press
//! ([`State::press_menu_turn`]) and a swipe ([`State::take_menu_turn`]) both
//! reach, so the two cannot disagree about where a row leads.

use crate::app::{NetworkMenuAction, NodeMenuAction, PlaybarMenuAction, State, ViewportMenuAction};
use crate::dialog::Mode;
use crate::plate_menu::{plate_title, PlateMenuAction};
use crate::slots::{DIALOG_IDX, NETWORK_PANEL_IDX, PLAYBAR_IDX};
use cce_ui::widget::context_menu::{self, PageTurn};
use cce_ui::widget::WidgetId;

/// A context menu a page or the dialog was turned to from: where a turn back
/// goes, and what the back band names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuOrigin {
    Viewport,
    Network,
    Playbar,
    /// A pane's plate menu shown alone (a stub's, the params pane's…).
    Plate(usize),
    /// A node's menu, by slot on the current level.
    Node(usize),
}

impl MenuOrigin {
    /// The menu a plate's rows are part of: the network and playbar menus
    /// carry their plates' rows, every other plate's are a menu alone.
    pub fn of_plate(idx: usize) -> MenuOrigin {
        match idx {
            NETWORK_PANEL_IDX => MenuOrigin::Network,
            PLAYBAR_IDX => MenuOrigin::Playbar,
            other => MenuOrigin::Plate(other),
        }
    }
}

impl ViewportMenuAction {
    /// Whether the row turns the menu: its pages, and the visualizers'
    /// editor, which is the dialog.
    pub fn leads_to_page(self) -> bool {
        matches!(self, ViewportMenuAction::Page(_) | ViewportMenuAction::Command("attribute_visualizers"))
    }
}

impl NetworkMenuAction {
    /// Add Node turns the menu into the add-node list; Add Tab into its page.
    pub fn leads_to_page(self) -> bool {
        matches!(self, NetworkMenuAction::Command("add_node") | NetworkMenuAction::Plate(PlateMenuAction::AddTabMenu))
    }
}

/// Mark the rows of the menu just shown that lead to a page.
pub fn mark_page_rows<A: Copy>(actions: &[A], leads: impl Fn(A) -> bool) {
    for (i, a) in actions.iter().enumerate() {
        if leads(*a) {
            context_menu::set_row_page(i);
        }
    }
}

impl State {
    /// Show a menu: at the pointer (`at` None), as a menu opens, or with its
    /// top-left at `at` in place of the plate that stood there — under a
    /// back band to `back` when there is somewhere to go back to.
    pub(crate) fn put_up_menu(&self, at: Option<(f32, f32)>, back: Option<MenuOrigin>, options: Vec<String>, header_count: usize, target: WidgetId) {
        match at {
            None => context_menu::show(self.cursor_x, self.cursor_y, options, header_count, target),
            Some((x, y)) => {
                let title = back.map(|o| self.menu_title(o));
                context_menu::show_page(x, y, title.as_deref(), options, header_count, target);
            }
        }
    }

    /// What a back band to `origin` reads.
    pub fn menu_title(&self, origin: MenuOrigin) -> String {
        match origin {
            MenuOrigin::Viewport => "Viewport".to_string(),
            MenuOrigin::Network => "Network".to_string(),
            MenuOrigin::Playbar => "Playbar".to_string(),
            MenuOrigin::Plate(idx) => plate_title(idx).to_string(),
            MenuOrigin::Node(slot) => self.current_dir().children.get(slot).map(|n| n.name.clone()).unwrap_or_else(|| "Node".to_string()),
        }
    }

    /// Which of the app's menus is up, if one is.
    pub fn open_menu_origin(&self) -> Option<MenuOrigin> {
        if !context_menu::is_visible() {
            return None;
        }
        if self.viewport_menu_active {
            Some(MenuOrigin::Viewport)
        } else if self.network_menu_active {
            Some(MenuOrigin::Network)
        } else if self.playbar_menu_active {
            Some(MenuOrigin::Playbar)
        } else if let Some(idx) = self.plate_menu_slot {
            Some(MenuOrigin::Plate(idx))
        } else {
            self.node_menu_slot.map(MenuOrigin::Node)
        }
    }

    /// Close whichever of the app's menus is up.
    fn close_open_menu(&mut self) {
        match self.open_menu_origin() {
            Some(MenuOrigin::Viewport) => self.close_viewport_menu(),
            Some(MenuOrigin::Network) => self.close_network_menu(),
            Some(MenuOrigin::Playbar) => self.close_playbar_menu(),
            Some(MenuOrigin::Plate(_)) => self.close_plate_menu(),
            Some(MenuOrigin::Node(_)) => self.close_node_menu(),
            None => context_menu::hide(),
        }
    }

    /// Show `origin`'s menu again with its top-left at `(x, y)`: a turn back
    /// to it from a page or the dialog.
    pub fn reopen_menu(&mut self, origin: MenuOrigin, x: f32, y: f32) {
        let at = Some((x, y));
        match origin {
            MenuOrigin::Viewport => self.show_viewport_menu_page(None, at),
            MenuOrigin::Network => self.open_network_context_menu_at(at),
            MenuOrigin::Playbar => self.open_playbar_context_menu_at(at),
            MenuOrigin::Plate(idx) => self.open_plate_menu_at(idx, at),
            MenuOrigin::Node(slot) => self.open_node_context_menu_at(slot, at),
        }
    }

    /// A left press on a page row or a back band of the open menu turns it.
    /// `true` when it did, and the press is spent.
    pub fn press_menu_turn(&mut self) -> bool {
        if self.open_menu_origin().is_none() {
            return false;
        }
        match context_menu::turn_at(self.cursor_x, self.cursor_y) {
            Some(turn) => self.run_menu_turn(turn),
            None => false,
        }
    }

    /// The turn a side swipe over the open menu asked for, run.
    pub fn take_menu_turn(&mut self) -> bool {
        match context_menu::take_turn() {
            Some(turn) => self.run_menu_turn(turn),
            None => false,
        }
    }

    /// Turn the open menu: into what row `n` leads to, or back to the menu
    /// this page was turned from. `false` when the menu has no such turn.
    pub fn run_menu_turn(&mut self, turn: PageTurn) -> bool {
        let Some(origin) = self.open_menu_origin() else { return false };
        let at = (context_menu::x(), context_menu::y());
        match turn {
            PageTurn::Back => match origin {
                MenuOrigin::Viewport if self.viewport_menu_page.is_some() => {
                    self.show_viewport_menu_page(None, Some(at));
                }
                MenuOrigin::Plate(_) if self.plate_page_from.is_some() => {
                    let from = self.plate_page_from.take().unwrap();
                    self.close_plate_menu();
                    self.reopen_menu(from, at.0, at.1);
                }
                _ => return false,
            },
            PageTurn::Into(n) => match origin {
                MenuOrigin::Viewport => match self.viewport_menu_actions.get(n).copied() {
                    Some(ViewportMenuAction::Page(page)) => self.show_viewport_menu_page(Some(page), Some(at)),
                    Some(ViewportMenuAction::Command("attribute_visualizers")) => {
                        self.close_viewport_menu();
                        self.vis_editing = None;
                        self.open_dialog_from(Mode::Visualizers, origin, at);
                    }
                    _ => return false,
                },
                MenuOrigin::Network => match self.network_menu_actions.get(n).copied() {
                    Some(NetworkMenuAction::Command("add_node")) => {
                        self.close_network_menu();
                        self.open_dialog_from(Mode::AddNode, origin, at);
                    }
                    Some(NetworkMenuAction::Plate(PlateMenuAction::AddTabMenu)) => {
                        self.close_network_menu();
                        self.open_plate_add_tab_menu(NETWORK_PANEL_IDX, at, origin);
                    }
                    _ => return false,
                },
                MenuOrigin::Playbar => match self.playbar_menu_actions.get(n).copied() {
                    Some(PlaybarMenuAction::Plate(PlateMenuAction::AddTabMenu)) => {
                        self.close_playbar_menu();
                        self.open_plate_add_tab_menu(PLAYBAR_IDX, at, origin);
                    }
                    _ => return false,
                },
                MenuOrigin::Plate(idx) => match self.plate_menu_actions.get(n).copied() {
                    Some(PlateMenuAction::AddTabMenu) => {
                        self.close_plate_menu();
                        self.open_plate_add_tab_menu(idx, at, origin);
                    }
                    _ => return false,
                },
                MenuOrigin::Node(slot) => match self.node_menu_actions.get(n).copied() {
                    Some(NodeMenuAction::Rename) => {
                        self.close_node_menu();
                        self.open_rename_dialog_from(slot, Some((origin, at)));
                    }
                    _ => return false,
                },
            },
        }
        true
    }

    /// Turn the dialog back: to the mode it was turned to from, else to the
    /// menu it was turned to from, shown at the dialog's corner. `false`
    /// when it came from neither.
    pub fn dialog_back(&mut self) -> bool {
        if !self.dialog_visible() {
            return false;
        }
        if let Some(&prev) = self.dialog_trail.last() {
            self.open_dialog_mode(prev);
            return true;
        }
        let Some(origin) = self.dialog_from else { return false };
        let (x, y, _, _) = self.positions[DIALOG_IDX];
        self.close_dialog();
        self.close_open_menu();
        self.reopen_menu(origin, x, y);
        true
    }
}
