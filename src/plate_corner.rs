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
use crate::slots::{NETWORK_PANEL_IDX, PARAM_IDX, PLAYBAR_IDX, SPREADSHEET_IDX, WIDGET_COUNT};

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
    /// Take a detached pane back, closing the window that held it.
    Reattach,
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
        if self.pane_is_stubbed(idx) {
            // A stub is BUILT to carry the control, and is shorter than the
            // minimum span a full pane must clear — applying that guard here
            // deleted the only control that can bring the pane back, whether it
            // was collapsed or detached.
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

        // A detached pane lives in another window: collapsing the stub it left
        // behind would mean nothing, so the only thing to offer is taking it back.
        if self.pane_is_detached(idx) {
            let target = self.slots.get_dyn(idx).base().id();
            cce_ui::widget::context_menu::show(
                cx - CORNER_R,
                cy + CORNER_R,
                vec!["Reattach".to_string()],
                0,
                target,
            );
            self.plate_menu_slot = Some(idx);
            self.plate_menu_actions = vec![PlateMenuAction::Reattach];
            return;
        }

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
            PlateMenuAction::Reattach => self.reattach_plate(idx),
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
        // A detached window never offers to detach its own pane again, and a
        // pane already handed out cannot be handed out twice.
        if self.is_detached_network || self.detached_pane.is_some() {
            return false;
        }
        match idx {
            NETWORK_PANEL_IDX => !self.detached_circular_network,
            other => pane_detach_flag(other).is_some() && !self.detached_panes[other],
        }
    }

    fn detach_plate(&mut self, idx: usize) {
        if idx == NETWORK_PANEL_IDX {
            self.execute_action(crate::shortcut::Action::DetachCircularWindow);
            return;
        }
        let Some(flag) = pane_detach_flag(idx) else { return };

        // The detached window reads the pane out of the shared project file and
        // then syncs through it, exactly as the network window does — so it has
        // to be on disk BEFORE the child starts.
        let shared = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("default_project.json");
        if let Err(e) = self.save_to_file(&shared) {
            eprintln!("Failed to save shared project before detaching: {e:?}");
            return;
        }

        match std::env::current_exe() {
            Ok(exe) => match std::process::Command::new(exe).arg(flag).spawn() {
                Ok(child) => {
                    self.detached_children.insert(idx, child);
                    self.detached_panes[idx] = true;
                    self.rebuild_positions();
                    self.apply_layout();
                }
                // Leave the pane in place if the child never started, rather
                // than hiding it into a window that does not exist.
                Err(e) => eprintln!("Failed to spawn detached {}: {e:?}", plate_title(idx)),
            },
            Err(e) => eprintln!("Cannot locate own executable to detach: {e:?}"),
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
            self.stub_slot(idx);
        }
    }

    /// Whether `idx` is currently drawn as a stub — the render pass asks before
    /// painting a pane's body, and the title text only appears here.
    pub fn pane_is_collapsed(&self, idx: usize) -> bool {
        PLATE_SLOTS.contains(&idx) && self.collapsed_panes[idx]
    }
}

/// The CLI flag that runs this pane as its own window, e.g. `--detached-params`.
/// The network keeps `--detached-network`, handled separately: its detached
/// window is circular, not merely detached.
pub fn pane_detach_flag(idx: usize) -> Option<&'static str> {
    match idx {
        PARAM_IDX => Some("--detached-params"),
        SPREADSHEET_IDX => Some("--detached-spreadsheet"),
        PLAYBAR_IDX => Some("--detached-playbar"),
        _ => None,
    }
}

/// The pane an argv entry asks for, if any — the inverse of [`pane_detach_flag`].
pub fn pane_from_detach_flag(arg: &str) -> Option<usize> {
    PLATE_SLOTS
        .iter()
        .copied()
        .find(|&idx| pane_detach_flag(idx) == Some(arg))
}

/// The detached window's `app_id`, which the compositor keys window rules off.
pub fn pane_app_id(idx: usize) -> &'static str {
    match idx {
        PARAM_IDX => "cce-designer-params",
        SPREADSHEET_IDX => "cce-designer-spreadsheet",
        PLAYBAR_IDX => "cce-designer-playbar",
        _ => "cce-designer",
    }
}

/// Inset of a detached pane inside its own window, so the plate keeps a visible
/// edge of its own instead of fusing with the window border.
pub const DETACHED_MARGIN: f32 = 8.0;

impl State {
    /// Resolve the detached-window arrangement, both sides of it.
    ///
    /// A post-pass for the same reason `apply_collapsed_panes` is one: detaching
    /// means one thing regardless of which of the three layout branches just
    /// ran. In the CHILD process the detached pane claims the whole window and
    /// every other slot goes dark; in the PARENT the panes it has handed out
    /// stop being laid out, so the space they held is released.
    pub(crate) fn apply_detached_panes(&mut self) {
        if let Some(idx) = self.detached_pane {
            for i in 0..WIDGET_COUNT {
                if i == idx {
                    continue;
                }
                self.positions[i] = (0.0, 0.0, 0.0, 0.0);
                self.slots.get_dyn_mut(i).set_visible(false);
            }
            let m = DETACHED_MARGIN;
            self.positions[idx] = (
                m,
                m,
                (self.width - 2.0 * m).max(0.0),
                (self.height - 2.0 * m).max(0.0),
            );
            self.slots.get_dyn_mut(idx).set_visible(true);
            return;
        }

        // The parent keeps a STUB for each pane it handed out rather than
        // dropping it: the stub carries the corner control, which is the only
        // way back. Hiding the pane outright left no way to reattach it.
        for idx in PLATE_SLOTS {
            if self.detached_panes[idx] {
                self.stub_slot(idx);
            }
        }
    }

    /// Shrink one slot to its title stub, taking any separate body slots with it.
    /// Shared by collapse and by the parent side of a detach.
    fn stub_slot(&mut self, idx: usize) {
        let (x, y, w, h) = self.positions[idx];
        if w <= 0.0 || h <= 0.0 {
            // Already laid out as hidden — there is no stub to make.
            return;
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

impl State {
    /// Is this pane currently living in a detached window? The network's flag
    /// is separate because its detached window is the circular one.
    pub fn pane_is_detached(&self, idx: usize) -> bool {
        if idx == NETWORK_PANEL_IDX {
            return self.detached_circular_network;
        }
        PLATE_SLOTS.contains(&idx) && self.detached_panes[idx]
    }

    /// Is this pane drawn as a stub rather than in full — collapsed, or left
    /// behind by a detach?
    pub fn pane_is_stubbed(&self, idx: usize) -> bool {
        self.pane_is_detached(idx) || self.pane_is_collapsed(idx)
    }

    /// The label a stubbed pane shows, or `None` when the pane is drawn in full.
    /// Collapsed and detached both stub, and they must not look alike: one is
    /// one click from expanding, the other is somewhere else entirely.
    pub fn pane_stub_label(&self, idx: usize) -> Option<String> {
        if self.pane_is_detached(idx) {
            Some(format!("{} — detached", plate_title(idx)))
        } else if self.pane_is_collapsed(idx) {
            Some(plate_title(idx).to_string())
        } else {
            None
        }
    }

    /// Detach or reattach a pane — the corner menu's two window actions, also
    /// the MCP surface's, so pane placement is scriptable like collapse is.
    pub fn set_pane_detached(&mut self, idx: usize, detached: bool) {
        if detached {
            if self.plate_can_detach(idx) {
                self.detach_plate(idx);
            }
        } else {
            self.reattach_plate(idx);
        }
    }

    /// Take a detached pane back and close the window that held it.
    pub fn reattach_plate(&mut self, idx: usize) {
        if !self.pane_is_detached(idx) {
            return;
        }
        self.close_detached_child(idx);

        if idx == NETWORK_PANEL_IDX {
            // Toggling the action back off is the network's own reattach — it
            // clears the flag and re-lays out without spawning anything.
            self.execute_action(crate::shortcut::Action::DetachCircularWindow);
            return;
        }

        self.detached_panes[idx] = false;
        self.rebuild_positions();
        self.apply_layout();
    }

    /// Close the detached child and reap it. Best-effort: a child the user
    /// already closed is simply gone, and reattaching must work anyway.
    fn close_detached_child(&mut self, idx: usize) {
        if let Some(mut child) = self.detached_children.remove(&idx) {
            let _ = child.kill();
            // Reap it, or the process table keeps a zombie for the rest of the
            // session — the same trap the liveness probe fell into.
            let _ = child.wait();
        }
    }

    /// Notice detached children the user closed themselves and take their panes
    /// back, so a closed window does not strand its pane as a dead stub. Called
    /// from the frame tick.
    ///
    /// `try_wait`, NOT `kill(pid, 0)`: the child is ours and unreaped, so once
    /// it exits it is a zombie — still present in the process table, so the
    /// signal probe reports it alive forever and the pane is never reclaimed.
    pub(crate) fn poll_detached_children(&mut self) -> bool {
        let mut reclaimed = false;
        for idx in PLATE_SLOTS {
            if !self.pane_is_detached(idx) {
                continue;
            }
            let exited = match self.detached_children.get_mut(&idx) {
                // `Ok(None)` is the only "still running" answer; an Err handle
                // is no more useful than an exited one.
                Some(child) => !matches!(child.try_wait(), Ok(None)),
                None => continue,
            };
            if !exited {
                continue;
            }
            self.detached_children.remove(&idx);
            if idx == NETWORK_PANEL_IDX {
                self.detached_circular_network = false;
                let val = false;
                self.menu_mut(crate::slots::LEFT_MENUBAR_IDX).set_item_checked(2, 3, val);
                self.menu_mut(crate::slots::HEADER_IDX).set_item_checked(2, 3, val);
            } else {
                self.detached_panes[idx] = false;
            }
            reclaimed = true;
        }
        if reclaimed {
            self.rebuild_positions();
            self.apply_layout();
        }
        reclaimed
    }
}
