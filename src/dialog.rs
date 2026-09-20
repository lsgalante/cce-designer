//! The dialog (`Alt+D`): run a command, or change a setting, without leaving
//! the keyboard.
//!
//! Two halves behind one chord because they answer the same question — "make
//! the app do the thing". The Commands half is the registry
//! ([`crate::command`]) fuzzy-filtered in place; the Settings half is the
//! viewport/graph display state that `DesignSettings` persists.
//!
//! This widget owns the FRAME — plate, tab strip, query line, command list —
//! and not the settings controls. Those are a second roster slot
//! (`DIALOG_PARAMS_IDX`, a `ParametersBg`) laid out inside this one's body, so
//! a slider in the dialog is the same slider as a slider in the params pane
//! rather than a second implementation that drifts from it. The values behind
//! those rows stay owned by the meta node's utility subnets, which is where
//! `apply_settings_from_menubar_subnets` reads them from — see
//! `State::dialog_settings_rows`.
//!
//! App-owned on the narrow traits wrapped in `Adapted<Dialog>`, like
//! [`crate::playbar::Playbar`], and a subtree painter for the same reason:
//! `paint` authors geometry AND text, so `append_frame_text` skips the slot.
//! Geometry helpers take the laid-out `rect` rather than caching one, again
//! like the playbar — the designer computes the same rects from
//! `positions[DIALOG_IDX]` when it needs them outside a paint.

use cce_ui::colors;
use cce_ui::scene::layout::Rect;
use cce_ui::scene::paint::PaintCtx;
use cce_ui::widget::*;

/// Which half of the dialog is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Commands,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Commands, Tab::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Commands => "Commands",
            Tab::Settings => "Settings",
        }
    }
}

/// What the dialog was opened to do.
///
/// One widget, two entry points, because a picker and a settings page are the
/// same plate with the same keys — what differs is the strip at the top and
/// what a row MEANS. Splitting them into two widgets is how an app ends up
/// with two filterable lists that behave differently, which is the thing this
/// dialog replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `Alt+D`: the tab strip, Commands and Settings.
    Tabbed,
    /// `Tab` in the network pane: one list, a title where the strip goes, and
    /// a pick that instantiates a node template at the grid cursor. No tabs —
    /// adding a node is a contextual act, not a peer of the app's settings.
    AddNode,
}

/// One row: what picking it means, plus what to draw.
#[derive(Debug, Clone)]
pub struct Row {
    /// What the app does with this row: a command id in [`Mode::Tabbed`], a
    /// node template's name in [`Mode::AddNode`]. Owned rather than
    /// `&'static str` because a template name is read off disk.
    pub id: String,
    pub label: String,
    /// The chord as a human reads it, empty when there is none. Drawn in its
    /// own right-hand column so the dialog teaches the keyboard rather than
    /// replacing it — which the `cce-cloud` palette could only approximate by
    /// padding the label out, since all it could send was one line of text.
    pub chord: String,
}

/// The dialog's outer size. Fixed rather than proportional: it is a focused
/// list, and a list that grows with the window turns into a wall of rows with
/// the one you want somewhere in it.
const DIALOG_W: f32 = 520.0;
const DIALOG_H: f32 = 420.0;

const PAD: f32 = 12.0;
/// Tab strip height, and the query line's.
const TAB_H: f32 = 30.0;
const QUERY_H: f32 = 30.0;
pub const ROW_H: f32 = 24.0;
/// Gap between the tab strip, the query line and the list.
const GAP: f32 = 8.0;

/// The dialog's rect inside a `width` x `height` window: centered
/// horizontally, and a little above centre vertically so the list grows into
/// the window's roomier half rather than down over the status bar.
pub fn layout_in(width: f32, height: f32) -> (f32, f32, f32, f32) {
    let w = DIALOG_W.min((width - 2.0 * PAD).max(200.0));
    let h = DIALOG_H.min((height - 2.0 * PAD).max(160.0));
    let x = ((width - w) * 0.5).max(0.0).round();
    let y = ((height - h) * 0.4).max(0.0).round();
    (x, y, w, h)
}

fn tab_strip(rect: Rect) -> Rect {
    Rect { x: rect.x + PAD, y: rect.y + PAD, width: (rect.width - 2.0 * PAD).max(0.0), height: TAB_H }
}

/// One tab's segment of the strip — the strip split evenly, which is what
/// makes the pair read as one segmented control rather than two buttons.
fn tab_rect(rect: Rect, tab: Tab) -> Rect {
    let strip = tab_strip(rect);
    let n = Tab::ALL.len() as f32;
    let w = strip.width / n;
    let i = Tab::ALL.iter().position(|t| *t == tab).unwrap_or(0) as f32;
    Rect { x: strip.x + i * w, y: strip.y, width: w, height: strip.height }
}

/// The query line. Commands only — the Settings half has no filter, since its
/// rows are a fixed handful and a filter over them would hide more than it
/// found.
fn query_rect(rect: Rect) -> Rect {
    let strip = tab_strip(rect);
    Rect { x: strip.x, y: strip.y + strip.height + GAP, width: strip.width, height: QUERY_H }
}

/// The command list's viewport.
fn list_rect(rect: Rect) -> Rect {
    let q = query_rect(rect);
    let top = q.y + q.height + GAP;
    Rect { x: q.x, y: top, width: q.width, height: (rect.y + rect.height - PAD - top).max(0.0) }
}

/// The Settings half's body — where the dialog's `ParametersBg` goes. It
/// starts where the query line would, there being no query line.
pub fn settings_rect(x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let strip = tab_strip(Rect { x, y, width: w, height: h });
    let top = strip.y + strip.height + GAP;
    (strip.x, top, strip.width, (y + h - PAD - top).max(0.0))
}

/// How many rows the command list can show at once, for a dialog of this size.
pub fn visible_rows(x: f32, y: f32, w: f32, h: f32) -> usize {
    (list_rect(Rect { x, y, width: w, height: h }).height / ROW_H).floor().max(0.0) as usize
}

pub struct Dialog {
    pub mode: Mode,
    pub tab: Tab,
    /// What has been typed into the Commands half's filter.
    pub query: String,
    /// The filtered, ranked rows — rebuilt by the app whenever `query`
    /// changes, never here: ranking needs the registry AND the focused pane,
    /// and this widget knows neither.
    pub rows: Vec<Row>,
    /// Which row Enter would run. Kept in range by [`Dialog::set_rows`].
    pub selected: usize,
    /// First visible row, scrolled to keep `selected` in view.
    pub scroll: usize,
    /// How many rows fit — pushed in from the layout, since `on_event` and the
    /// app's key handling both need it and neither has the rect to hand.
    page: usize,
    hover_row: Option<usize>,
    hover_tab: Option<Tab>,
    /// A row the pointer activated, drained by the app.
    activated: Option<String>,
    /// A tab the pointer chose, drained by the app.
    tab_click: Option<Tab>,
    /// Whether the dialog is currently claiming its rect as an occluder — see
    /// [`Paint::popover`]. Lowered for the length of an event dispatch into
    /// the dialog, because the one claim serves two mechanisms that want
    /// opposite answers.
    occluding: bool,
}

impl Dialog {
    pub fn new() -> Adapted<Dialog> {
        let mut d = Adapted::new(Dialog {
            mode: Mode::Tabbed,
            tab: Tab::Commands,
            query: String::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            page: 1,
            hover_row: None,
            hover_tab: None,
            activated: None,
            tab_click: None,
            occluding: true,
        });
        d.set_visible(false);
        d
    }

    /// Record how many rows fit, from the laid-out rect.
    pub fn set_page(&mut self, page: usize) {
        self.page = page.max(1);
        self.scroll_to_selected();
    }

    /// How many rows fit — what PageUp/PageDown step by.
    pub fn page_len(&self) -> usize {
        self.page.max(1)
    }

    /// Claim, or stop claiming, the dialog's rect as an occluder.
    pub fn set_occluding(&mut self, on: bool) {
        self.occluding = on;
    }

    /// Whether the body is the filterable row list — everything but the
    /// Settings half, which hands its body to `DIALOG_PARAMS_IDX`.
    pub fn shows_list(&self) -> bool {
        self.mode == Mode::AddNode || self.tab == Tab::Commands
    }

    fn row_rect(&self, rect: Rect, i: usize) -> Option<Rect> {
        let list = list_rect(rect);
        if i < self.scroll {
            return None;
        }
        let offset = (i - self.scroll) as f32 * ROW_H;
        if offset + ROW_H > list.height {
            return None;
        }
        Some(Rect { x: list.x, y: list.y + offset, width: list.width, height: ROW_H })
    }

    fn row_at(&self, rect: Rect, x: f32, y: f32) -> Option<usize> {
        let list = list_rect(rect);
        if x < list.x || x >= list.x + list.width || y < list.y || y >= list.y + list.height {
            return None;
        }
        let i = self.scroll + ((y - list.y) / ROW_H).floor() as usize;
        (i < self.rows.len()).then_some(i)
    }

    fn tab_at(&self, rect: Rect, x: f32, y: f32) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| {
            let r = tab_rect(rect, *t);
            x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
        })
    }

    /// Keep `selected` inside the scrolled window.
    pub fn scroll_to_selected(&mut self) {
        let per_page = self.page.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + per_page {
            self.scroll = self.selected + 1 - per_page;
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        let n = self.rows.len() as i32;
        // Wrapping, not clamping: a list you can fall off the bottom of makes
        // the last row harder to reach than the first, and this one is short.
        self.selected = (self.selected as i32 + delta).rem_euclid(n) as usize;
        self.scroll_to_selected();
    }

    /// What Enter would pick.
    pub fn selected_id(&self) -> Option<&str> {
        self.rows.get(self.selected).map(|r| r.id.as_str())
    }

    pub fn take_activated(&mut self) -> Option<String> {
        self.activated.take()
    }

    pub fn take_tab_click(&mut self) -> Option<Tab> {
        self.tab_click.take()
    }

    /// Swap in a freshly ranked row list, keeping the selection in range.
    ///
    /// The selection goes back to the top rather than trying to follow the
    /// command it was on: the rows are re-ranked by the query, so "the same
    /// row" after a keystroke is a different command, and the best match
    /// being preselected is the whole point of ranking them.
    pub fn set_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        self.selected = 0;
        self.scroll = 0;
    }
}

impl Layout for Dialog {
    /// Above every pane, and above the second network editor's plates: the
    /// dialog is modal in practice — a press inside it never reaches what it
    /// covers — so it has to be drawn that way too. The dialog's params body
    /// sits one above this (see `DIALOG_PARAMS_IDX`).
    fn z_order(&self) -> i32 {
        900
    }
}

impl Paint for Dialog {
    /// `paint` authors geometry AND text, so the Text prims pass through
    /// `paint_self` verbatim instead of the single-font own-labels bridge —
    /// the chord column needs its own family and its own clip bounds.
    fn paints_own_subtree(&self) -> bool {
        true
    }

    /// The dialog IS its own plate, the contract every floating surface in
    /// this app wears: the parameter plate's fill, so it tracks the configured
    /// tint, opacity and blur-behind marker with the panes.
    fn color(&self) -> [f32; 4] {
        colors::param_plate_fill()
    }

    fn solid_border(&self) -> Option<([f32; 4], f32)> {
        colors::plate_border_color().map(|bc| (bc, colors::plate_border_thickness()))
    }

    fn corner_style(&self, _rect: Rect) -> Option<(f32, (bool, bool, bool, bool))> {
        let r = cce_ui::layout::plate_corner_radius();
        (r > 0.0).then_some((r, (true, true, true, true)))
    }

    /// The dialog's whole rect, as an occluder.
    ///
    /// Text is not painted in display-list order — the engine collects every
    /// Text prim and lays them all out at the end — so a plate drawn over a
    /// label does not hide it, whatever the z. What hides it is the
    /// popover-occlusion clamp, which reads `UiContext::active_popovers`; the
    /// designer registers every visible widget whose `popover_rect` is `Some`,
    /// so claiming one here is how the graph's node labels and the viewport's
    /// readouts stop bleeding through the plate.
    ///
    /// The clamp exempts text whose OWN bounds coincide with the occluder, so
    /// everything drawn inside the dialog — this widget's labels and, via
    /// `append_dialog`, the settings body's — carries these exact bounds and
    /// does its own truncating.
    /// **`occluding` exists because one claim serves two mechanisms that want
    /// opposite answers.** `UiContext::is_coordinate_covered` reads the same
    /// `popover_rect` — off every REGISTERED widget, not only the ones in
    /// `active_popovers` — to decide that a press has landed under something
    /// else. With the claim standing, every control inside the dialog is
    /// covered by the plate it is drawn on and nothing in the Settings half
    /// can be clicked. `State::dispatch_uncovered` lowers the flag for the
    /// length of a dispatch into the dialog and puts it straight back.
    fn popover(&self, rect: Rect) -> Option<(f32, f32, f32, f32)> {
        self.occluding.then_some((rect.x, rect.y, rect.width, rect.height))
    }

    fn paint(&self, rect: Rect, ctx: &mut PaintCtx) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let (family, font_size) = cce_ui::layout::control_label_font_parsed();
        let accent = colors::highlight_primary_color();
        let tint = [accent[0], accent[1], accent[2]];
        let depth = colors::plate_bevel_width();
        let ctrl_r = cce_ui::layout::control_corner_radius();
        let radii = (ctrl_r, ctrl_r, ctrl_r, ctrl_r);
        // The occlusion-clamp exemption (see `popover`): every label in here
        // carries the dialog's own rect, so none of them is clipped away by
        // the occluder the dialog itself registers. The cost is that bounds
        // no longer trim an overlong label, so the rows truncate by hand.
        let own = Some([rect.x, rect.y, rect.x + rect.width, rect.y + rect.height]);
        let fit = |text: &str, width: f32| -> String {
            if width <= 0.0 {
                return String::new();
            }
            let cols = (width / display::measure_text_width("M", &family, font_size).max(1.0))
                .floor() as usize;
            display::truncate_tail(text, cols)
        };

        // --- The header. In AddNode there are no halves to move between, so
        // the strip's band carries a title instead: the same plate, saying
        // what this opening of it is for.
        if self.mode == Mode::AddNode {
            let strip = tab_strip(rect);
            let title = "Add Node";
            let tw = display::measure_text_width(title, &family, font_size);
            let tx = strip.x + (strip.width - tw) * 0.5;
            let ty = cce_ui::layout::align_text_y(strip.y, strip.height, font_size, 0.0);
            ctx.text_with(title, tx, ty, font_size, [0xf0, 0xf0, 0xf6], Some(family.clone()), own);
        }
        // --- The tab strip: one segmented control, so the seam between the
        // two halves reads as a seam and not as a gap.
        for tab in Tab::ALL {
            if self.mode != Mode::Tabbed {
                break;
            }
            let r = tab_rect(rect, tab);
            let active = tab == self.tab;
            // Active wears the focus language the rest of the app uses for
            // "this is the live one": the tinted bevel, a glint on the
            // control's own silhouette.
            if active {
                ctx.bevel_tinted(r, radii, &cce_ui::scene::Material::from_fill(colors::param_plate_fill()), depth, tint);
            } else if self.hover_tab == Some(tab) {
                ctx.rounded_rect(r, ctrl_r, (true, true, true, true), [1.0, 1.0, 1.0, 0.05]);
            }
            let label = tab.label();
            let tw = display::measure_text_width(label, &family, font_size);
            let tx = r.x + (r.width - tw) * 0.5;
            let ty = cce_ui::layout::align_text_y(r.y, r.height, font_size, 0.0);
            let color = if active { [0xf0, 0xf0, 0xf6] } else { [0x9a, 0x9a, 0xa6] };
            ctx.text_with(
                label,
                tx,
                ty,
                font_size,
                color,
                Some(family.clone()),
                own,
            );
        }

        // The Settings half's body is a separate roster slot, painted by the
        // designer's own walk — nothing more to draw here.
        if self.mode == Mode::Tabbed && self.tab == Tab::Settings {
            return;
        }

        // --- The query line: a well, like the text rows in the params pane.
        // The caret is a plain rule and does not blink: the dialog owns the
        // keyboard outright while it is open, so there is no focus to signal.
        let q = query_rect(rect);
        ctx.rounded_rect(q, ctrl_r, (true, true, true, true), [0.0, 0.0, 0.0, 0.22]);
        let qty = cce_ui::layout::align_text_y(q.y, q.height, font_size, 0.0);
        let qtx = q.x + 8.0;
        let q_w = q.width - 16.0;
        if self.query.is_empty() {
            let hint = fit(
                match self.mode {
                    Mode::Tabbed => "Type to filter commands",
                    Mode::AddNode => "Type to filter nodes",
                },
                q_w,
            );
            ctx.text_with(hint, qtx, qty, font_size, [0x70, 0x70, 0x7c], Some(family.clone()), own);
        } else {
            // Head-truncated: what matters while typing is the end of the
            // query, which is where the caret is.
            let shown = display::truncate_head(&self.query, (q_w / display::measure_text_width("M", &family, font_size).max(1.0)).floor() as usize);
            ctx.text_with(shown, qtx, qty, font_size, [0xe6, 0xe6, 0xee], Some(family.clone()), own);
        }
        let caret_x = qtx + display::measure_text_width(&self.query, &family, font_size) + 1.0;
        if caret_x < q.x + q.width - 4.0 {
            ctx.quad(
                Rect { x: caret_x, y: q.y + 6.0, width: 1.0, height: q.height - 12.0 },
                [accent[0], accent[1], accent[2], 0.9],
            );
        }

        // --- The rows. The chord column is right-aligned against the list's
        // right edge rather than padded out to a fixed width: the label is
        // what gets read, so it is the label that keeps the stable left edge.
        let list = list_rect(rect);
        if self.rows.is_empty() {
            let ty = cce_ui::layout::align_text_y(list.y, ROW_H, font_size, 0.0);
            let empty = match self.mode {
                Mode::Tabbed => "No matching command",
                Mode::AddNode => "No matching node",
            };
            ctx.text_with(empty, list.x + 8.0, ty, font_size, [0x70, 0x70, 0x7c], Some(family.clone()), own);
            return;
        }
        for i in self.scroll..self.rows.len() {
            let Some(r) = self.row_rect(rect, i) else { break };
            let row = &self.rows[i];
            if i == self.selected {
                ctx.rounded_rect(r, ctrl_r, (true, true, true, true), [accent[0], accent[1], accent[2], 0.16]);
                ctx.bevel_tinted(r, radii, &cce_ui::scene::Material::from_fill([0.0; 4]), depth, tint);
            } else if self.hover_row == Some(i) {
                ctx.rounded_rect(r, ctrl_r, (true, true, true, true), [1.0, 1.0, 1.0, 0.05]);
            }
            let ty = cce_ui::layout::align_text_y(r.y, r.height, font_size, 0.0);
            let chord_w = if row.chord.is_empty() {
                0.0
            } else {
                display::measure_text_width(&row.chord, &family, font_size)
            };
            // The label's clip stops short of the chord column so a long
            // label is cut by it rather than running under it.
            let label_right = r.x + r.width - 8.0 - if chord_w > 0.0 { chord_w + 12.0 } else { 0.0 };
            let label_color = if i == self.selected { [0xf4, 0xf4, 0xfa] } else { [0xcc, 0xcc, 0xd4] };
            ctx.text_with(
                fit(&row.label, label_right - (r.x + 8.0)),
                r.x + 8.0,
                ty,
                font_size,
                label_color,
                Some(family.clone()),
                own,
            );
            if chord_w > 0.0 {
                ctx.text_with(
                    row.chord.clone(),
                    r.x + r.width - 8.0 - chord_w,
                    ty,
                    font_size,
                    [0x85, 0x85, 0x92],
                    Some(family.clone()),
                    own,
                );
            }
        }
    }
}

impl Input for Dialog {
    /// The whole rect, always — this is what makes the dialog modal over what
    /// it covers: the designer's press cascade asks the dialog first and a hit
    /// never falls through to the pane underneath.
    fn hit(&self, rect: Rect, x: f32, y: f32) -> bool {
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    }

    fn on_event(&mut self, event: &Event, ectx: &mut EventCtx) -> bool {
        let rect = ectx.rect;
        match event {
            Event::MouseButton { button: MouseButton::Left, state: ElementState::Pressed, x, y, .. } => {
                if self.mode == Mode::Tabbed {
                    if let Some(tab) = self.tab_at(rect, *x, *y) {
                        self.tab_click = Some(tab);
                        return true;
                    }
                }
                if self.shows_list() {
                    if let Some(i) = self.row_at(rect, *x, *y) {
                        self.selected = i;
                        self.activated = self.rows.get(i).map(|r| r.id.clone());
                        return true;
                    }
                }
                // Inside the plate but on no control: consumed anyway, so the
                // press cannot reach the pane the dialog is covering.
                true
            }
            Event::PointerMove { x, y, .. } => {
                let row = self.shows_list().then(|| self.row_at(rect, *x, *y)).flatten();
                let tab = (self.mode == Mode::Tabbed).then(|| self.tab_at(rect, *x, *y)).flatten();
                let changed = row != self.hover_row || tab != self.hover_tab;
                self.hover_row = row;
                self.hover_tab = tab;
                changed
            }
            Event::MouseWheel { delta, .. } => {
                if !self.shows_list() || self.rows.is_empty() {
                    return false;
                }
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -*y,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / ROW_H,
                };
                let max_scroll = self.rows.len().saturating_sub(self.page.max(1));
                let next = (self.scroll as f32 + lines).round().clamp(0.0, max_scroll as f32) as usize;
                let changed = next != self.scroll;
                self.scroll = next;
                changed
            }
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// The designer's half: what the dialog shows, and what choosing a row does.
// ---------------------------------------------------------------------------

use crate::app::{param_display, ParamDef, State};
use crate::slots::{DIALOG_IDX, DIALOG_PARAMS_IDX};

/// Where a Settings row's value actually lives.
///
/// Not the live `State` fields, and not `DesignSettings`: both are DOWNSTREAM
/// of the meta node. `apply_settings_from_menubar_subnets` copies the utility
/// subnets onto the live state on every param change, so a write straight to
/// `State::grid_thickness` would survive exactly until the next one. The
/// subnet param is the value's owner; this enum says which owner each row has,
/// so the dialog edits values in the one place that keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// A param on a utility subnet under the root meta node (`Main`, `View`,
    /// `Guides`), named here by subnet and param name.
    Subnet(&'static str, &'static str),
    /// A toggle the command registry already owns end to end: the command does
    /// the live flip, the menu checkmark AND the per-camera writeback in one
    /// place. Square Aspect and Show Camera Pivot are per-CAMERA settings with
    /// no node at all behind the Default Camera, and their commands are the
    /// only code that gets both cases right — so the row dispatches instead of
    /// writing, and reads its displayed value off the live state.
    Command(&'static str),
    /// A param on the ACTIVE camera node, with the live field as the fallback:
    /// the Default Camera has no node, so there is nothing to write but the
    /// field, and `apply_settings_from_menubar_subnets` leaves it alone.
    ActiveCamera(&'static str),
}

/// One row of the Settings half.
pub struct Setting {
    /// What the dialog calls it — and, because `param_display` keys a row by
    /// its label, the identity the writeback resolves back to this row. Unique
    /// across the table, section titles included.
    ///
    /// Spelled out rather than borrowed from the owning param's own label: the
    /// subnets' labels are tuned for the section they sit in ("Plate", under
    /// the View node's Network section) and stop making sense anywhere else.
    pub label: &'static str,
    pub owner: Option<Owner>,
}

impl Setting {
    const fn section(label: &'static str) -> Self {
        Setting { label, owner: None }
    }

    const fn row(label: &'static str, owner: Owner) -> Self {
        Setting { label, owner: Some(owner) }
    }
}

/// The Settings half, in order.
///
/// Scope is exactly what `DesignSettings` persists — the viewport and graph
/// DISPLAY state, which is the part of the app's configuration that is a
/// preference rather than part of a project. What is deliberately NOT here:
/// the render subnet (per-project look), the pane-visibility toggles (the
/// View menu and the plate corners already own those, and a settings dialog
/// is a strange place to hide a pane from), and keybindings, which this DE
/// edits as `input.kdl` on purpose.
pub const SETTINGS: &[Setting] = &[
    Setting::section("Viewport"),
    Setting::row("Background Color", Owner::Subnet("Main", "Background Color")),
    Setting::row("Square Aspect", Owner::Command("toggle_square_viewport")),
    Setting::section("Grid"),
    Setting::row("Show Grid", Owner::Subnet("Guides", "Show Grid Guide")),
    Setting::row("Grid Color", Owner::Subnet("Guides", "Grid Color")),
    Setting::row("Grid Thickness", Owner::Subnet("Guides", "Grid Thickness")),
    Setting::section("Guides"),
    Setting::row("Show Origin Axes", Owner::Subnet("Guides", "Show Origin Axes")),
    Setting::row("Origin Size", Owner::Subnet("Guides", "Origin Guide Size")),
    Setting::row("Show Reference Cube", Owner::Subnet("Guides", "Show Reference Cube")),
    Setting::section("Camera"),
    Setting::row("Show Camera Pivot", Owner::Command("toggle_camera_pivot")),
    Setting::row("Camera Pivot Size", Owner::ActiveCamera("Camera Pivot Size")),
    Setting::section("Network"),
    Setting::row("Show Network Plate", Owner::Subnet("View", "Show Network Plate")),
];

fn setting_by_label(label: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.label == label)
}

/// A synthetic `ParamDef` carrying `label` and the shape of `src`, so the row
/// renders as whatever control its owner already uses.
fn relabel(src: &ParamDef, label: &'static str) -> ParamDef {
    ParamDef { label: label.to_string(), ..src.clone() }
}

fn bool_param(label: &'static str, on: bool) -> ParamDef {
    ParamDef {
        name: label.to_string(),
        label: label.to_string(),
        param_type: "toggle".to_string(),
        default: if on { "true" } else { "false" }.to_string(),
        options: Vec::new(),
        min: None,
        max: None,
        step: None,
        show_when: String::new(),
    }
}

impl State {
    pub fn dialog_visible(&self) -> bool {
        self.slots.dialog.visible()
    }

    /// The dialog's tab, or `Commands` when it is closed.
    pub fn dialog_tab(&self) -> Tab {
        self.slots.dialog.tab
    }

    pub fn toggle_dialog(&mut self) {
        if self.dialog_visible() && self.slots.dialog.mode == Mode::Tabbed {
            self.close_dialog();
        } else {
            self.open_dialog();
        }
    }

    /// Open the tabbed dialog on Commands — `Alt+D`, and what `Ctrl+P` now
    /// reaches instead of spawning a popup process.
    pub fn open_dialog(&mut self) {
        self.open_dialog_in(Mode::Tabbed);
    }

    /// The add-node palette: the same plate, one list, and a pick that
    /// instantiates a template at the grid cursor.
    ///
    /// Was a `cce-cloud --dmenu` popup — a second process with its own
    /// window, fed one line of text per row and answering with one line back.
    /// It could not show a chord in a column of its own, could not be styled
    /// with the app, and put a second filterable list in front of the user
    /// that looked nothing like the first.
    pub fn open_node_palette(&mut self) {
        if self.dialog_visible() && self.slots.dialog.mode == Mode::AddNode {
            self.close_dialog();
            return;
        }
        self.open_dialog_in(Mode::AddNode);
    }

    fn open_dialog_in(&mut self, mode: Mode) {
        // Always with an empty query, and the tabbed mode always on Commands:
        // a dialog that reopens holding the last search has to be cleared
        // before it can be used, which is a step every single time to save
        // one occasionally.
        self.slots.dialog.mode = mode;
        self.slots.dialog.tab = Tab::Commands;
        self.slots.dialog.query.clear();
        self.slots.dialog.set_visible(true);
        self.refresh_dialog_rows();
        if mode == Mode::Tabbed {
            self.refresh_dialog_settings();
        }
        self.rebuild_positions();
        self.apply_layout();
        self.update_status_text(match mode {
            Mode::Tabbed => "Dialog: type to filter, Tab switches halves, Escape closes.",
            Mode::AddNode => "Add Node: type to filter, Enter adds at the cursor, Escape closes.",
        });
    }

    pub fn close_dialog(&mut self) {
        if !self.dialog_visible() {
            return;
        }
        // The settings body keeps whatever a half-finished text edit left in
        // it; drop that focus so the next open starts clean.
        self.slots.dialog_params.unfocus();
        self.slots.dialog.set_visible(false);
        self.slots.dialog_params.set_visible(false);
        if self.focused_widget == Some(DIALOG_IDX) || self.focused_widget == Some(DIALOG_PARAMS_IDX) {
            self.focused_widget = None;
        }
        self.rebuild_positions();
        self.apply_layout();
    }

    pub fn set_dialog_tab(&mut self, tab: Tab) {
        if self.slots.dialog.mode != Mode::Tabbed || self.slots.dialog.tab == tab {
            return;
        }
        self.slots.dialog.tab = tab;
        if tab == Tab::Settings {
            // Re-read on every entry: a chord or a menu may have changed one
            // of these while the Commands half was up.
            self.refresh_dialog_settings();
        } else {
            self.refresh_dialog_rows();
        }
        self.rebuild_positions();
        self.apply_layout();
    }

    /// Re-rank the row list against the current query, for whichever mode is
    /// up.
    ///
    /// Commands rank through [`crate::command::palette_entries`] — the fuzzy
    /// rank plus the focused-pane-first partition. Node templates rank
    /// through the same [`crate::command::fuzzy_rank`], so typing means the
    /// same thing in both lists; they carry no chord, so the column is simply
    /// empty for them.
    pub fn refresh_dialog_rows(&mut self) {
        let query = self.slots.dialog.query.clone();
        let rows: Vec<Row> = match self.slots.dialog.mode {
            Mode::Tabbed => crate::command::palette_entries(&query, self.focused_context())
                .iter()
                .map(|c| Row {
                    id: c.id.to_string(),
                    label: c.label.to_string(),
                    chord: self
                        .shortcut_manager
                        .chord_for(c.id)
                        .map(|s| s.describe())
                        .unwrap_or_default(),
                })
                .collect(),
            Mode::AddNode => {
                // In a utility dir geometry templates are rejected at
                // placement — don't offer them.
                let in_utility = self.in_settings_dir();
                let offered: Vec<&str> = self
                    .node_templates
                    .iter()
                    .filter(|t| {
                        !in_utility
                            || !crate::geometry::is_geometry_node_type(&t.node.node_type)
                    })
                    .map(|t| t.label.as_str())
                    .collect();
                crate::command::fuzzy_rank(&query, &offered)
                    .into_iter()
                    .map(|i| Row {
                        id: offered[i].to_string(),
                        label: offered[i].to_string(),
                        chord: String::new(),
                    })
                    .collect()
            }
        };
        self.slots.dialog.set_rows(rows);
    }

    /// The Settings half's rows, each read from whatever owns its value.
    ///
    /// Returns `ParamDef`s rather than display triples so the encoding
    /// (`spinbox:min:max:step`, `choice:a,b`) stays in `param_display` — one
    /// place, shared with the params pane, instead of a second copy here that
    /// could disagree about what a spinbox is.
    fn dialog_settings_params(&self) -> Vec<ParamDef> {
        let subnet_param = |subnet: &str, name: &str| -> Option<&ParamDef> {
            self.session_node()?
                .children
                .iter()
                .find(|c| c.name == subnet)?
                .params
                .iter()
                .find(|p| p.name == name)
        };
        let camera_param = |name: &str| -> Option<&ParamDef> {
            if self.active_camera == "Default Camera" {
                return None;
            }
            self.current_dir()
                .children
                .iter()
                .find(|c| c.node_type == "camera" && c.name == self.active_camera)?
                .params
                .iter()
                .find(|p| p.name == name)
        };

        let mut out = Vec::with_capacity(SETTINGS.len());
        for s in SETTINGS {
            match s.owner {
                None => out.push(ParamDef {
                    name: s.label.to_string(),
                    label: s.label.to_string(),
                    param_type: "section".to_string(),
                    default: String::new(),
                    options: Vec::new(),
                    min: None,
                    max: None,
                    step: None,
                    show_when: String::new(),
                }),
                Some(Owner::Subnet(subnet, name)) => {
                    // A subnet param that does not exist is a row that cannot
                    // work, so it is not offered — the subnets are recreated
                    // on every load, but a detached window has no meta node at
                    // all.
                    if let Some(p) = subnet_param(subnet, name) {
                        out.push(relabel(p, s.label));
                    }
                }
                Some(Owner::Command(id)) => {
                    let on = match id {
                        "toggle_square_viewport" => self.square_viewport,
                        "toggle_camera_pivot" => self.viewport().show_camera_pivot,
                        _ => false,
                    };
                    out.push(bool_param(s.label, on));
                }
                Some(Owner::ActiveCamera(name)) => match camera_param(name) {
                    Some(p) => out.push(relabel(p, s.label)),
                    // No camera node behind the Default Camera: the live field
                    // is the value, in the same tenths the camera param uses.
                    None => out.push(ParamDef {
                        name: s.label.to_string(),
                        label: s.label.to_string(),
                        param_type: "spinbox".to_string(),
                        default: ((self.camera_pivot_size * 10.0).round() as i32).to_string(),
                        options: Vec::new(),
                        min: Some(1.0),
                        max: Some(50.0),
                        step: Some(1.0),
                        show_when: String::new(),
                    }),
                },
            }
        }
        // A section with nothing under it is a header for an empty list.
        let mut trimmed: Vec<ParamDef> = Vec::with_capacity(out.len());
        for (i, p) in out.iter().enumerate() {
            let empty_section = p.param_type == "section"
                && out.get(i + 1).is_none_or(|n| n.param_type == "section");
            if !empty_section {
                trimmed.push(p.clone());
            }
        }
        trimmed
    }

    /// Push the Settings rows into the dialog's params body, and remember them
    /// as the baseline the writeback diffs against.
    pub fn refresh_dialog_settings(&mut self) {
        let rows = param_display(&self.dialog_settings_params());
        self.slots.dialog_params_mut().set_display_params(&rows);
        self.dialog_settings_shown = rows;
    }

    /// Apply whatever the Settings half's controls changed.
    ///
    /// The params pane's writeback (`sync_parameters_to_project`), pointed at
    /// [`SETTINGS`] instead of the selected node: read the controls, diff
    /// against what was put into them, write each changed row to its owner,
    /// and then run the one apply-and-persist pass. Polled rather than pushed
    /// for the same reason the params pane is — a `ParametersBg` reports its
    /// values, it does not emit events.
    pub fn sync_dialog_settings_to_project(&mut self) {
        if !self.dialog_visible() || self.slots.dialog.shows_list() {
            return;
        }
        let updated = self.slots.dialog_params().node_params();
        let mut commands: Vec<&'static str> = Vec::new();
        let mut changed = false;
        for (key, value, _) in &updated {
            let was = self.dialog_settings_shown.iter().find(|(k, _, _)| k == key);
            if was.is_none_or(|(_, v, _)| v == value) {
                continue;
            }
            let Some(setting) = setting_by_label(key) else { continue };
            let Some(owner) = setting.owner else { continue };
            changed = true;
            match owner {
                Owner::Subnet(subnet, name) => {
                    if let Some(session) = self.session_node_mut() {
                        if let Some(node) = session.children.iter_mut().find(|c| c.name == subnet) {
                            if let Some(p) = node.params.iter_mut().find(|p| p.name == name) {
                                p.default = value.clone();
                            }
                        }
                    }
                }
                Owner::Command(id) => commands.push(id),
                Owner::ActiveCamera(name) => {
                    let active = self.active_camera.clone();
                    let wrote = {
                        let dir = self.current_dir_mut();
                        match dir
                            .children
                            .iter_mut()
                            .find(|c| c.node_type == "camera" && c.name == active)
                            .and_then(|c| c.params.iter_mut().find(|p| p.name == name))
                        {
                            Some(p) => {
                                p.default = value.clone();
                                true
                            }
                            None => false,
                        }
                    };
                    if !wrote {
                        if let Ok(v) = value.parse::<f32>() {
                            self.camera_pivot_size = v / 10.0;
                        }
                    }
                }
            }
        }
        if !changed {
            return;
        }

        // The commands run FIRST and on their own: each is a toggle whose
        // whole job is to flip live state, mark the menus and persist, and
        // running them after the apply below would have them flip against
        // values the apply had just settled.
        for id in commands {
            self.run_command(id);
        }

        self.apply_settings_from_menubar_subnets();
        // The viewport meshes bake their sizes and colors in, so a changed
        // thickness/size/tint is a re-generate, not a re-draw. This is the
        // same set the settings-file reload in `tick_frame` regenerates.
        self.update_grid_geometry();
        self.update_origin_geometry();
        self.update_pivot_geometry();
        self.update_viewport_bg_geometry();
        self.sync_grid_settings();
        self.rebuild_scene_geometry();
        self.sync_nodes();
        self.save_settings();
        // The params pane may be showing one of these very nodes.
        self.sync_parameters_pane();
        // Re-read rather than patching the baseline: an apply can normalize a
        // value (a spinbox clamp), and the baseline has to be what the
        // controls now hold or the next poll reports a phantom change.
        self.refresh_dialog_settings();
    }

    /// Lay the dialog and its settings body out over the window.
    ///
    /// Called at the end of `rebuild_positions`, after every layout branch has
    /// run — like the 2D page pane, the rect it wants never depends on which
    /// branch produced the panes underneath it.
    pub(crate) fn layout_dialog(&mut self) {
        let open = self.dialog_visible();
        if !open {
            self.positions[DIALOG_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.positions[DIALOG_PARAMS_IDX] = (0.0, 0.0, 0.0, 0.0);
            self.slots.dialog_params.set_visible(false);
            return;
        }
        let (x, y, w, h) = layout_in(self.width, self.height);
        self.positions[DIALOG_IDX] = (x, y, w, h);
        self.slots.dialog.set_page(visible_rows(x, y, w, h));

        let settings = self.slots.dialog.tab == Tab::Settings;
        self.positions[DIALOG_PARAMS_IDX] =
            if settings { settings_rect(x, y, w, h) } else { (0.0, 0.0, 0.0, 0.0) };
        self.slots.dialog_params.set_visible(settings);
    }

    /// Every key, while the dialog is open.
    ///
    /// Total, not layered: the branch that calls this returns whatever it
    /// returns, so nothing below reaches the panes. A modal that leaks its
    /// typing is worse than no modal — typing "frame" into the filter would
    /// otherwise step the grid cursor and flip a node's geometry flag on the
    /// way past, since the network pane's bare-letter family is ungated.
    pub(crate) fn dialog_key_input(&mut self, event: &KeyEvent) -> bool {
        if event.state != ElementState::Pressed {
            return true;
        }
        // The dialog's own chord closes it, wherever the user has bound it —
        // asked for by id rather than hardcoded to Alt+D, so a rebind in
        // `input.kdl` keeps working both ways.
        if self.shortcut_manager.match_command(&self.modifiers, &event.logical_key)
            == Some("toggle_dialog")
        {
            self.close_dialog();
            return true;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.close_dialog();
                return true;
            }
            Key::Named(NamedKey::Tab) => {
                // One key for two halves, in both directions: there are only
                // two, so Tab and Shift+Tab are the same move. In AddNode
                // there are no halves — and Tab is what OPENED it, so the
                // same key closes it again.
                if self.slots.dialog.mode == Mode::AddNode {
                    self.close_dialog();
                } else {
                    let next = if self.slots.dialog.tab == Tab::Commands {
                        Tab::Settings
                    } else {
                        Tab::Commands
                    };
                    self.set_dialog_tab(next);
                }
                return true;
            }
            _ => {}
        }

        if !self.slots.dialog.shows_list() {
            // The settings body is a real `ParametersBg` with real text
            // fields; hand it the key and poll what it did, exactly as the
            // params pane's own key path does.
            let taken = {
                let ptr = &mut self.slots.dialog_params as *mut cce_ui::widget::Adapted<ParametersBg>;
                unsafe { (*ptr).keyboard_input(event, &mut self.ui_context) }
            };
            if taken {
                self.sync_dialog_settings_to_project();
            }
            // Consumed either way: an unhandled key inside a modal does
            // nothing, it does not fall through to the network pane.
            return true;
        }

        match &event.logical_key {
            Key::Named(NamedKey::ArrowDown) => self.slots.dialog.move_selection(1),
            Key::Named(NamedKey::ArrowUp) => self.slots.dialog.move_selection(-1),
            Key::Named(NamedKey::PageDown) => {
                let page = self.slots.dialog.page_len() as i32;
                self.slots.dialog.move_selection(page);
            }
            Key::Named(NamedKey::PageUp) => {
                let page = self.slots.dialog.page_len() as i32;
                self.slots.dialog.move_selection(-page);
            }
            Key::Named(NamedKey::Home) => {
                self.slots.dialog.selected = 0;
                self.slots.dialog.scroll_to_selected();
            }
            Key::Named(NamedKey::End) => {
                let last = self.slots.dialog.rows.len().saturating_sub(1);
                self.slots.dialog.selected = last;
                self.slots.dialog.scroll_to_selected();
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(id) = self.slots.dialog.selected_id().map(str::to_string) {
                    self.take_dialog_pick(id);
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if self.slots.dialog.query.pop().is_some() {
                    self.refresh_dialog_rows();
                }
            }
            Key::Named(NamedKey::Space) => {
                self.slots.dialog.query.push(' ');
                self.refresh_dialog_rows();
            }
            Key::Character(c) => {
                // Bare typing only: a modified key is a chord, and the ones
                // this dialog answers to are handled above.
                if !self.modifiers.control_key()
                    && !self.modifiers.alt_key()
                    && !self.modifiers.super_key()
                {
                    self.slots.dialog.query.push_str(c);
                    self.refresh_dialog_rows();
                }
            }
            _ => {}
        }
        true
    }

    /// Run a command the dialog chose, and close.
    ///
    /// Closing FIRST, so a command that opens something of its own (the
    /// `cce-cloud` palette, a file chooser) does not come up behind the
    /// dialog. The dialog's own row is the exception: toggling it here would
    /// reopen what was just closed.
    pub(crate) fn take_dialog_pick(&mut self, id: String) {
        let mode = self.slots.dialog.mode;
        let (gx, gy) = (self.grid_cursor_col as f32, self.grid_cursor_row as f32);
        self.close_dialog();
        match mode {
            // Not `toggle_dialog`: toggling here would reopen what was just
            // closed. Picking the dialog's own row is a no-op, which is the
            // least surprising thing it could be.
            Mode::Tabbed => {
                if id != "toggle_dialog" {
                    self.run_command(&id);
                }
            }
            // Fire-and-forget at the grid cursor, exactly as the popup's
            // answer used to arrive — read BEFORE the close, since closing
            // relays the panes.
            Mode::AddNode => {
                let mut redraw = false;
                let action = crate::app::McpAction::AddNode {
                    template_name: id,
                    name: None,
                    x: gx,
                    y: gy,
                };
                if let Err(e) = self.apply_action(action, &mut redraw) {
                    // The one refusal this can hit is a geometry template in
                    // a utility dir, which `refresh_dialog_rows` already
                    // filters out — but the rule lives in `apply_action`, so
                    // say what it said rather than assume it cannot fire.
                    self.update_status_text(&e);
                }
            }
        }
    }

    /// Drain what the pointer did inside the dialog, after an event reached
    /// one of its two slots. Returns whether anything changed.
    pub(crate) fn drain_dialog_clicks(&mut self) -> bool {
        let mut changed = false;
        if let Some(tab) = self.slots.dialog.take_tab_click() {
            self.set_dialog_tab(tab);
            changed = true;
        }
        if let Some(id) = self.slots.dialog.take_activated() {
            self.take_dialog_pick(id);
            changed = true;
        }
        changed
    }

    /// A mouse button, while the dialog is open.
    ///
    /// `None` hands the press back to the ordinary cascade — which happens
    /// only for the buttons the dialog has no use for, so a right-click still
    /// reaches whatever is under it outside the plate. `Some(handled)` means
    /// the dialog dealt with it and nothing else should.
    pub(crate) fn dialog_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
    ) -> Option<bool> {
        if button != MouseButton::Left {
            return None;
        }
        let (x, y) = (self.cursor_x, self.cursor_y);

        // A slider drag started in the settings body ends wherever the pointer
        // happens to be — including outside the plate. Ending it has to come
        // before the dismiss test below, or dragging a value past the dialog's
        // edge and letting go would close the dialog instead of committing.
        if state == ElementState::Released && self.drag_widget == Some(DIALOG_PARAMS_IDX) {
            let ptr = &mut self.slots.dialog_params as *mut cce_ui::widget::Adapted<ParametersBg>;
            unsafe {
                (*ptr).handle_event(&cce_ui::widget::Event::DragEnd, &mut self.ui_context);
                (*ptr).handle_event(
                    &cce_ui::widget::Event::MouseButton { button, state, x, y, local_x: x, local_y: y },
                    &mut self.ui_context,
                );
            }
            self.drag_widget = None;
            self.drag_press_cursor = None;
            self.sync_dialog_settings_to_project();
            return Some(true);
        }

        let hits_dialog = self.in_dialog_slot(DIALOG_IDX, x, y);
        let hits_settings = self.in_dialog_slot(DIALOG_PARAMS_IDX, x, y);

        if !hits_dialog && !hits_settings {
            // Outside: a press dismisses, a release is the tail of that press
            // and is simply eaten.
            if state == ElementState::Pressed {
                self.close_dialog();
            }
            return Some(true);
        }

        // The settings body first — it sits INSIDE the dialog's rect, so the
        // dialog's own (deliberately total) hit test would otherwise claim
        // every press meant for a control.
        if hits_settings {
            let ev = cce_ui::widget::Event::MouseButton { button, state, x, y, local_x: x, local_y: y };
            let taken = self.dispatch_uncovered(DIALOG_PARAMS_IDX, &ev);
            if taken {
                self.sync_dialog_settings_to_project();
            }
            // Sliders and ramps drag; arm the same drag the params pane arms.
            if state == ElementState::Pressed && self.slots.draggable(DIALOG_PARAMS_IDX) {
                let ev = cce_ui::widget::Event::DragStart { start_x: x, start_y: y };
                let ptr = &mut self.slots.dialog_params
                    as *mut cce_ui::widget::Adapted<ParametersBg>;
                unsafe {
                    (*ptr).handle_event(&ev, &mut self.ui_context);
                }
                self.drag_widget = Some(DIALOG_PARAMS_IDX);
                self.drag_press_cursor = Some((x, y));
            }
            return Some(true);
        }

        let ev = cce_ui::widget::Event::MouseButton { button, state, x, y, local_x: x, local_y: y };
        self.dispatch_uncovered(DIALOG_IDX, &ev);
        self.drain_dialog_clicks();
        Some(true)
    }

    /// The wheel, while the dialog is open: to whichever half is showing, and
    /// no further. Both halves scroll their own list, so there is nothing to
    /// fall through to.
    pub(crate) fn dialog_mouse_wheel(&mut self, delta: MouseScrollDelta) -> bool {
        let (x, y) = (self.cursor_x, self.cursor_y);
        let ev = cce_ui::widget::Event::MouseWheel { delta, x, y, local_x: x, local_y: y };
        if self.in_dialog_slot(DIALOG_PARAMS_IDX, x, y) {
            return self.dispatch_uncovered(DIALOG_PARAMS_IDX, &ev);
        }
        if self.in_dialog_slot(DIALOG_IDX, x, y) {
            return self.dispatch_uncovered(DIALOG_IDX, &ev);
        }
        false
    }

    /// Is `(x, y)` inside a dialog slot's laid-out rect?
    ///
    /// The rect, not `hit_test`: the dialog registers itself as a text
    /// occluder (see `Dialog::popover`) and `Adapted::hit_test` reads that
    /// same list as COVERAGE, so every point inside the dialog reports as
    /// covered — by the dialog's own plate. Inside a modal the geometry IS
    /// the answer.
    pub(crate) fn in_dialog_slot(&self, idx: usize, x: f32, y: f32) -> bool {
        if !self.slots.get_dyn(idx).visible() {
            return false;
        }
        let (rx, ry, rw, rh) = self.positions[idx];
        rw > 0.0 && rh > 0.0 && x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    /// Deliver `ev` to a dialog slot with the dialog's occluder claim lowered.
    ///
    /// `Adapted::handle_event` hit-gates presses and wheels through
    /// `is_coordinate_covered`, which asks every REGISTERED widget for its
    /// `popover_rect` — including the dialog's, which covers the whole plate
    /// (see [`Dialog::popover`]). So while the claim stands, every press
    /// aimed at a settings control is rejected as covered by the surface the
    /// control is drawn on, and the Settings half is inert. The dialog's own
    /// routing has already decided who gets this event, so the claim comes
    /// down for the dispatch and goes straight back up.
    pub(crate) fn dispatch_uncovered(&mut self, idx: usize, ev: &cce_ui::widget::Event) -> bool {
        self.slots.dialog.set_occluding(false);
        // The coverage answer is memoized per point, so lowering the claim is
        // not enough — a query from earlier this frame is served from the
        // cache, and the engine makes one on every left press
        // (`close_popovers_missed_by_press`).
        self.ui_context.invalidate_coverage_cache();
        let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn cce_ui::widget::WidgetHost + 'static);
        let taken = unsafe { (*ptr).handle_event(ev, &mut self.ui_context) };
        self.slots.dialog.set_occluding(true);
        self.ui_context.invalidate_coverage_cache();
        taken
    }
}
