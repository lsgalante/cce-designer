//! The plate corner control: a small circular menu trigger riding the top-right
//! of every pane that draws a plate of its own, and the menu it opens.
//!
//! Geometry is derived from the slot's LIVE rect rather than computed alongside
//! `positions[..]`, because `rebuild_positions` lays the panes out in three
//! different branches (normal, circular network, detached window) and a corner
//! computed per-branch would be three things to keep in step. The circular
//! network pane is the one shape whose "top-right" is not a rect corner, so it
//! is special-cased onto the arc.
//!
//! The control follows the DE's closed-menu-trigger language (see
//! `cce-ui`'s popover conventions): a transparent face over an inset trough,
//! here on a fully-round radius so the trough reads as a ring.

use crate::app::State;
use crate::slots::{NETWORK_PANEL_IDX, PARAM_IDX, PLAYBAR_IDX, SPREADSHEET_IDX};

/// Radius of the control itself.
pub const CORNER_R: f32 = 8.0;

/// Centre inset from the plate's top-right corner, on both axes. Clears the
/// plate's own corner arc at the radii the DE ships.
pub const CORNER_INSET: f32 = 14.0;

/// A plate needs at least this much room before it earns a corner control —
/// below it the trigger would cover the pane it belongs to.
const MIN_PLATE_SPAN: f32 = 3.0 * CORNER_INSET;

/// The plates that carry a corner control. The viewport is deliberately absent:
/// its "plate" is the window-spanning lip, so a top-right control would sit on
/// the window corner rather than on a pane.
pub const PLATE_SLOTS: [usize; 4] = [NETWORK_PANEL_IDX, PARAM_IDX, SPREADSHEET_IDX, PLAYBAR_IDX];

/// What the corner menu can do to its plate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlateMenuAction {
    /// Shrink the plate to its title stub (or restore it).
    Collapse,
    Expand,
    /// Move the pane out into its own window.
    Detach,
}

impl State {
    /// Centre of `idx`'s corner control, or `None` when the plate is hidden or
    /// too small to carry one.
    pub fn plate_corner_center(&self, idx: usize) -> Option<(f32, f32)> {
        if !PLATE_SLOTS.contains(&idx) {
            return None;
        }
        let w = self.slots.get_dyn(idx);
        if !w.visible() {
            return None;
        }

        // The circular network pane: put the control where the plate's own
        // top-right actually is — on the arc, at 45°.
        if idx == NETWORK_PANEL_IDX && self.circular_network_pane {
            let c = &self.circular_network_layout;
            if c.r < MIN_PLATE_SPAN {
                return None;
            }
            let d = std::f32::consts::FRAC_1_SQRT_2 * (c.r - CORNER_INSET);
            return Some((c.x + d, c.y - d));
        }

        let (x, y, pw, ph) = w.rect();
        if pw < MIN_PLATE_SPAN {
            return None;
        }
        if self.collapsed_panes[idx] {
            // The stub is BUILT to carry the control, and is shorter than the
            // minimum span a full pane must clear — applying that guard here
            // deleted the only control that can expand the pane again.
            return Some((x + pw - CORNER_INSET, y + ph / 2.0));
        }
        if ph < MIN_PLATE_SPAN {
            return None;
        }
        Some((x + pw - CORNER_INSET, y + CORNER_INSET))
    }

    /// The plate whose corner control is under `(px, py)`, if any. Searched in
    /// reverse draw order so an overlapping pane's control wins, matching what
    /// the user sees on top.
    pub fn plate_corner_at(&self, px: f32, py: f32) -> Option<usize> {
        PLATE_SLOTS.iter().rev().copied().find(|&idx| {
            self.plate_corner_center(idx).is_some_and(|(cx, cy)| {
                let (dx, dy) = (px - cx, py - cy);
                dx * dx + dy * dy <= CORNER_R * CORNER_R
            })
        })
    }
}

/// The pane's display name — the stub's label, and what the menu is "about".
pub fn plate_title(idx: usize) -> &'static str {
    match idx {
        NETWORK_PANEL_IDX => "Network",
        PARAM_IDX => "Parameters",
        SPREADSHEET_IDX => "Spreadsheet",
        PLAYBAR_IDX => "Playbar",
        _ => "Pane",
    }
}

impl State {
    /// Open the corner menu for `idx`, anchored under its control. Items are
    /// contextual: a collapsed plate offers Expand instead of Collapse, and
    /// Detach only appears where a detached window exists for that pane.
    pub fn open_plate_menu(&mut self, idx: usize) {
        let Some((cx, cy)) = self.plate_corner_center(idx) else { return };

        let mut options: Vec<String> = Vec::new();
        let mut actions: Vec<PlateMenuAction> = Vec::new();

        if self.collapsed_panes[idx] {
            options.push("Expand".to_string());
            actions.push(PlateMenuAction::Expand);
        } else {
            options.push("Collapse".to_string());
            actions.push(PlateMenuAction::Collapse);
        }

        if self.plate_can_detach(idx) {
            options.push("Detach".to_string());
            actions.push(PlateMenuAction::Detach);
        }

        let target = self.slots.get_dyn(idx).base().id();
        cce_ui::widget::context_menu::show(cx - CORNER_R, cy + CORNER_R, options, 0, target);
        self.plate_menu_slot = Some(idx);
        self.plate_menu_actions = actions;
    }

    pub fn plate_menu_open(&self) -> bool {
        cce_ui::widget::context_menu::is_visible() && self.plate_menu_slot.is_some()
    }

    pub fn close_plate_menu(&mut self) {
        cce_ui::widget::context_menu::hide();
        self.plate_menu_slot = None;
        self.plate_menu_actions.clear();
    }

    /// Route a left press while the corner menu is open — same contract as
    /// `handle_node_menu_click`.
    pub fn handle_plate_menu_click(&mut self) -> bool {
        if !self.plate_menu_open() {
            return false;
        }
        if cce_ui::widget::context_menu::hit_test(self.cursor_x, self.cursor_y) {
            let my = cce_ui::widget::context_menu::y();
            let row = ((self.cursor_y - my) / 24.0).floor() as usize;
            let picked = self.plate_menu_slot.zip(self.plate_menu_actions.get(row).copied());
            self.close_plate_menu();
            if let Some((idx, action)) = picked {
                self.dispatch_plate_menu(idx, action);
            }
            return true;
        }
        self.close_plate_menu();
        false
    }

    fn dispatch_plate_menu(&mut self, idx: usize, action: PlateMenuAction) {
        match action {
            PlateMenuAction::Collapse => self.set_pane_collapsed(idx, true),
            PlateMenuAction::Expand => self.set_pane_collapsed(idx, false),
            PlateMenuAction::Detach => self.detach_plate(idx),
        }
    }

    pub fn set_pane_collapsed(&mut self, idx: usize, collapsed: bool) {
        if !PLATE_SLOTS.contains(&idx) || self.collapsed_panes[idx] == collapsed {
            return;
        }
        self.collapsed_panes[idx] = collapsed;
        self.rebuild_positions();
        self.apply_layout();
    }
}

impl State {
    /// Whether `idx` has somewhere to detach TO. Today only the network pane
    /// has a detached-window mode (`--detached-network`); the others gain one
    /// as that path is generalized, and until then they simply do not offer
    /// the item rather than offering one that does nothing.
    pub fn plate_can_detach(&self, idx: usize) -> bool {
        idx == NETWORK_PANEL_IDX && !self.is_detached_network
    }

    fn detach_plate(&mut self, idx: usize) {
        if idx == NETWORK_PANEL_IDX {
            self.execute_action(crate::shortcut::Action::DetachCircularWindow);
        }
    }
}

/// Height of a collapsed plate: its title stub. Deep enough for the title text
/// and the corner control that restores it, and no deeper.
pub const STUB_H: f32 = 26.0;

impl State {
    /// Rewrite the collapsed plates' rects down to their stubs.
    ///
    /// A post-pass over `positions[..]` rather than a branch in each layout
    /// arm: `rebuild_positions` lays panes out three different ways (floating,
    /// circular network, detached window) and collapse means the same thing in
    /// all of them — keep the plate's origin and width, take its height down to
    /// the stub. In the floating layout, which is what the main window uses,
    /// that reclaims the space outright: the panes float over a full-bleed
    /// viewport, so nothing has to reflow around them.
    ///
    /// The panes whose body is a SEPARATE slot (the network plate owns the
    /// graph and the breadcrumb) also hide those, since a stub has no room for
    /// them and they would otherwise keep painting over the viewport.
    pub(crate) fn apply_collapsed_panes(&mut self) {
        for idx in PLATE_SLOTS {
            if !self.collapsed_panes[idx] {
                continue;
            }
            let (x, y, w, h) = self.positions[idx];
            if w <= 0.0 || h <= 0.0 {
                // Already laid out as hidden — collapse has nothing to say.
                continue;
            }
            self.positions[idx] = (x, y, w, STUB_H.min(h));

            if idx == NETWORK_PANEL_IDX {
                for child in [crate::slots::CONTENT_IDX, crate::slots::BREADCRUMB_IDX] {
                    self.positions[child] = (0.0, 0.0, 0.0, 0.0);
                    self.slots.get_dyn_mut(child).set_visible(false);
                }
            }
        }
    }

    /// Whether `idx` is currently drawn as a stub — the render pass asks before
    /// painting a pane's body, and the title text only appears here.
    pub fn pane_is_collapsed(&self, idx: usize) -> bool {
        PLATE_SLOTS.contains(&idx) && self.collapsed_panes[idx]
    }
}
