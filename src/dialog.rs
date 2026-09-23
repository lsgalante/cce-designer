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
//! those rows are the live `State` fields, persisted by `DesignSettings` —
//! see [`SETTINGS`] and `Owner`.
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
    /// A colour the row previews, drawn as a swatch ahead of the label —
    /// linear RGBA, as the paint path takes it. `None` for the ordinary row.
    /// The Wireframe Color command carries the live wire colour here, so
    /// the palette shows what the setting currently is before it is opened.
    pub swatch: Option<[f32; 4]>,
    /// The current state of a TOGGLE command — Show Grid, Square Aspect,
    /// the pane toggles — drawn as a switch in a column of its own, so the
    /// list shows what each toggle currently is the way the View menu's
    /// checkmarks do. `None` for a command that runs and is done. A row
    /// that carries one is picked in place: the command flips, the switch
    /// moves, and the dialog stays up — see `State::take_dialog_pick`.
    pub toggle: Option<bool>,
    /// A SLIDER row's value, in the range `Dialog::set_slider_range` set —
    /// the network zoom, as a percentage of the configured grid. Drawn as
    /// the toolkit's `Slider` over the row's right end, dragged in place,
    /// nudged by the arrow keys while selected; picking it runs nothing.
    /// `None` for every other row. There is at most one such row.
    pub slider: Option<f32>,
    /// Truncate the label on the LEFT when it does not fit, rather than on
    /// the right: the tail of a path is what identifies it, and a row that
    /// cut `/home/me/projects/thing` down to `/home/me/pro...` would name
    /// every project in the directory equally badly.
    pub truncate_head: bool,
}

/// The dialog's outer size. Fixed rather than proportional: it is a focused
/// list, and a list that grows with the window turns into a wall of rows with
/// the one you want somewhere in it.
const DIALOG_W: f32 = 520.0;
/// The plate's height, clamped to the window by [`layout_in`].
///
/// 420 until 2026-09-23, which was sized for a Settings half of thirteen
/// rows. Retiring the root meta node moved everything its four utility
/// subnets held into that table — it is nearer thirty now — and a list that
/// shows eight of them is a list you scroll rather than read.
const DIALOG_H: f32 = 640.0;

const PAD: f32 = 12.0;
/// Tab strip height, and the query line's.
const TAB_H: f32 = 30.0;
const QUERY_H: f32 = 30.0;
pub const ROW_H: f32 = 24.0;
/// Side of a row's colour swatch, logical px.
pub const SWATCH_SIDE: f32 = 14.0;
/// A toggle row's switch: the toolkit's `Toggle`, at the row's height less a
/// hair of air, and about twice as wide as tall — the proportion the params
/// pane's toggles have.
pub const TOGGLE_W: f32 = 36.0;
const TOGGLE_H: f32 = ROW_H - 4.0;
/// How far in from the row's right end a slider row's BAND begins. It runs
/// from there out to the CHORD column's right edge, so it ends exactly where
/// every other row's key binding ends and the toggle column stays clear —
/// a band that stopped short of the chords read as a control someone had
/// forgotten to finish. Not reserved on the other rows: the slider row has
/// no chord, so it borrows the chord column rather than pushing every chord
/// in the list left by half the plate.
pub const SLIDER_W: f32 = 180.0;
/// The readout's width and its gap from the band. It sits to the LEFT of the
/// band, because the band's right end is spoken for. The readout is drawn by
/// the dialog, not by the toolkit slider's own: the dialog claims its rect as
/// a text occluder, and the clamp lets through only text carrying the
/// dialog's exact bounds (see `Dialog::popover`), so the stamp's readout
/// would paint and never show — as the Settings half's labels did before
/// they were re-emitted retagged.
const READOUT_W: f32 = 60.0;
const READOUT_GAP: f32 = 8.0;
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
    /// The list's scroll offset in px (0 = row 0 flush with the list top),
    /// the drawn value: rows paint at `i * ROW_H - scroll_px`, clipped to the
    /// list. Driven by `scroll_motion` — the DE's one wheel→offset model, so
    /// a notch glides and a trackpad tracks and coasts — and jumped by the
    /// keyboard (`scroll_to_selected`). Until 2026-09-20 this was a ROW
    /// index: every wheel event rounded to whole rows, so a trackpad's small
    /// deltas did nothing until one crossed half a row and then jumped it —
    /// the choppy commands list.
    pub scroll_px: f32,
    scroll_motion: cce_ui::widget::scroll_motion::ScrollMotion,
    /// The list's scrollbar, in the DE's sink-behind idiom (cce-mail's
    /// body bar, `ScrollRegion` with `sink_behind`): pills at the list's
    /// right edge that fade in on a scroll, stay while hovered or dragged,
    /// and fade out after the hold. Sunk, it is not drawn and takes no
    /// input — a press on its lane reaches the row beneath.
    sb_activity: cce_ui::widget::ScrollbarActivity,
    sb_dragging: bool,
    /// Where in the thumb the drag grabbed it, so the thumb does not jump
    /// to centre itself under the pointer.
    sb_drag_offset: f32,
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
    /// The switch a toggle row draws, off and on — the toolkit's own
    /// `Toggle`, painted by hand into the row, so a switch in the dialog IS
    /// the switch in the params pane. Two stamps rather than one set per row
    /// because `paint` takes `&self`, and building a widget per row per
    /// frame would be silly.
    toggle_stamps: [Adapted<Toggle>; 2],
    /// The slider a slider row draws — the toolkit's own `Slider`, so a
    /// slider in the dialog IS the slider in the params pane. One stamp,
    /// because there is at most one slider row; its value is kept in step
    /// with the row's at every mutation, since `paint` cannot set it.
    slider_stamp: Adapted<Slider>,
    /// A press landed on the slider's band and the pointer is moving it —
    /// the app drives this through its widget-drag protocol (`draggable`
    /// and the `drag_*` hooks), so the drag survives the pointer leaving
    /// the plate.
    slider_drag: bool,
    /// The band's (x, width) captured at the press, so a drag keeps
    /// mapping the pointer while the row scrolls under it.
    slider_track: (f32, f32),
    /// The value the pointer moved the slider to, drained by the app.
    slider_change: Option<f32>,
}

impl Dialog {
    pub fn new() -> Adapted<Dialog> {
        let mut off = Toggle::new();
        off.set_toggled(false);
        let mut on = Toggle::new();
        on.set_toggled(true);
        let mut slider_stamp = Slider::new().with_readout(false);
        slider_stamp.set_scroll(false);
        let mut d = Adapted::new(Dialog {
            mode: Mode::Tabbed,
            tab: Tab::Commands,
            query: String::new(),
            rows: Vec::new(),
            selected: 0,
            scroll_px: 0.0,
            scroll_motion: cce_ui::widget::scroll_motion::ScrollMotion::new(),
            sb_activity: cce_ui::widget::ScrollbarActivity::new(),
            sb_dragging: false,
            sb_drag_offset: 0.0,
            page: 1,
            hover_row: None,
            hover_tab: None,
            activated: None,
            tab_click: None,
            occluding: true,
            toggle_stamps: [off, on],
            slider_stamp,
            slider_drag: false,
            slider_track: (0.0, 1.0),
            slider_change: None,
        });
        d.set_visible(false);
        d
    }

    /// Record how many rows fit, from the laid-out rect. Re-clamps the
    /// offset to the new range and nothing more: this runs on EVERY
    /// relayout (`layout_dialog`, off `rebuild_positions`, which the frame
    /// tick reaches whenever anything animates), and snapping to the
    /// selection here undid every wheel and finger scroll within a frame
    /// (2026-09-20 — the list "would not scroll at all"). Keeping the
    /// selection in view is the keyboard's job: `move_selection` and
    /// `scroll_to_selected` at the call sites that change it.
    pub fn set_page(&mut self, page: usize) {
        self.page = page.max(1);
        self.set_scroll_px(self.scroll_px);
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

    /// The furthest the list scrolls: the last page flush with the bottom.
    fn max_scroll_px(&self) -> f32 {
        ((self.rows.len() as f32 - self.page.max(1) as f32) * ROW_H).max(0.0)
    }

    /// The first row with any part in view.
    fn first_row(&self) -> usize {
        (self.scroll_px / ROW_H).floor().max(0.0) as usize
    }

    /// Jump the list to `px` (clamped) — the keyboard's move, not a glide.
    fn set_scroll_px(&mut self, px: f32) {
        self.scroll_px = px.clamp(0.0, self.max_scroll_px());
        self.scroll_motion.y.jump_to(self.scroll_px);
    }

    /// The scrollbar's geometry — `(sb_x, track_y, sb_w, track_h, thumb_y,
    /// thumb_h)`, mirroring `ScrollRegion::scrollbar_geom` as cce-mail's
    /// body bar does — or `None` when the rows fit and there is no bar. The
    /// one source for paint, the press and the drag. The bar rides the
    /// plate's CENTRE line, as both of cce-mail's bars ride theirs — over
    /// the rows, reserving no lane, in front only while raised — at the
    /// page-level width the Settings half's pane uses, so the two halves'
    /// bars match; the track stops 4px short at each end like every toolkit
    /// bar.
    pub fn scrollbar_geom(&self, rect: Rect) -> Option<(f32, f32, f32, f32, f32, f32)> {
        if !self.shows_list() {
            return None;
        }
        let max_scroll = self.max_scroll_px();
        if max_scroll <= 0.0 {
            return None;
        }
        let list = list_rect(rect);
        let sb_w = cce_ui::layout::scrollbar_width() * 1.6;
        let sb_x = rect.x + (rect.width - sb_w) * 0.5;
        let track_y = list.y + 4.0;
        let track_h = (list.height - 8.0).max(0.0);
        let content_h = self.rows.len() as f32 * ROW_H;
        let visible_ratio = list.height / content_h.max(1.0);
        let thumb_h = if track_h <= 20.0 { track_h } else { (track_h * visible_ratio).clamp(20.0, track_h) };
        let thumb_y = track_y + (self.scroll_px / max_scroll) * (track_h - thumb_h);
        Some((sb_x, track_y, sb_w, track_h, thumb_y, thumb_h))
    }

    fn over_scrollbar(&self, rect: Rect, px: f32, py: f32) -> bool {
        self.scrollbar_geom(rect).is_some_and(|(sb_x, track_y, sb_w, track_h, _, _)| {
            px >= sb_x - 4.0 && px <= sb_x + sb_w + 4.0 && py >= track_y && py <= track_y + track_h
        })
    }

    /// A left press on the bar's strip (±4px slop, like `ScrollRegion`):
    /// grab the thumb where it was clicked, or jump the track there and
    /// drag from the thumb's centre. A sunk bar is not drawn and takes no
    /// input — the press falls through to the row beneath.
    fn sb_press(&mut self, rect: Rect, px: f32, py: f32) -> bool {
        if !self.sb_activity.raised() {
            return false;
        }
        let Some((sb_x, track_y, sb_w, track_h, thumb_y, thumb_h)) = self.scrollbar_geom(rect) else {
            return false;
        };
        if px < sb_x - 4.0 || px > sb_x + sb_w + 4.0 || py < track_y || py > track_y + track_h {
            return false;
        }
        self.sb_dragging = true;
        let click_offset = py - thumb_y;
        if (0.0..=thumb_h).contains(&click_offset) {
            self.sb_drag_offset = click_offset;
        } else {
            self.sb_drag_offset = thumb_h / 2.0;
            self.sb_drag_to(rect, py);
        }
        true
    }

    fn sb_drag_to(&mut self, rect: Rect, py: f32) -> bool {
        let Some((_, track_y, _, track_h, _, thumb_h)) = self.scrollbar_geom(rect) else {
            return false;
        };
        let target = py - self.sb_drag_offset;
        let ratio = if track_h - thumb_h > 0.0 { ((target - track_y) / (track_h - thumb_h)).clamp(0.0, 1.0) } else { 0.0 };
        let old = self.scroll_px;
        self.set_scroll_px(ratio * self.max_scroll_px());
        (self.scroll_px - old).abs() > 0.01
    }

    /// Whether the bar is raised — for the app's press cascade and tests.
    pub fn scrollbar_raised(&self) -> bool {
        self.sb_activity.raised()
    }

    /// A row's rect in the list, `Some` while any part of it is in view
    /// (the paint clips to the list, so a partly scrolled row draws cut).
    fn row_rect(&self, rect: Rect, i: usize) -> Option<Rect> {
        let list = list_rect(rect);
        let offset = i as f32 * ROW_H - self.scroll_px;
        if offset + ROW_H <= 0.0 || offset >= list.height {
            return None;
        }
        Some(Rect { x: list.x, y: list.y + offset, width: list.width, height: ROW_H })
    }

    fn row_at(&self, rect: Rect, x: f32, y: f32) -> Option<usize> {
        let list = list_rect(rect);
        if x < list.x || x >= list.x + list.width || y < list.y || y >= list.y + list.height {
            return None;
        }
        let i = ((y - list.y + self.scroll_px) / ROW_H).floor();
        (i >= 0.0 && (i as usize) < self.rows.len()).then_some(i as usize)
    }

    fn tab_at(&self, rect: Rect, x: f32, y: f32) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| {
            let r = tab_rect(rect, *t);
            x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
        })
    }

    /// Keep `selected` inside the scrolled window.
    pub fn scroll_to_selected(&mut self) {
        let view = self.page.max(1) as f32 * ROW_H;
        let top = self.selected as f32 * ROW_H;
        let bottom = top + ROW_H;
        if top < self.scroll_px {
            self.set_scroll_px(top);
            self.sb_activity.bump();
        } else if bottom > self.scroll_px + view {
            self.set_scroll_px(bottom - view);
            self.sb_activity.bump();
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.rows.is_empty() {
            self.selected = 0;
            self.set_scroll_px(0.0);
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

    /// The slider row's index, if the list has one.
    fn slider_row(&self) -> Option<usize> {
        self.rows.iter().position(|r| r.slider.is_some())
    }

    /// The range a slider row's value is read in (and the readout shows).
    pub fn set_slider_range(&mut self, min: f32, max: f32) {
        self.slider_stamp.set_range(min, max);
        self.sync_slider_stamp();
    }

    /// Move the slider row — and the stamp that draws it — to a value.
    pub fn set_slider_value(&mut self, v: f32) {
        if let Some(i) = self.slider_row() {
            self.rows[i].slider = Some(v);
        }
        self.slider_stamp.set_scaled_value(v);
    }

    fn sync_slider_stamp(&mut self) {
        if let Some(v) = self.slider_row().and_then(|i| self.rows[i].slider) {
            self.slider_stamp.set_scaled_value(v);
        }
    }

    pub fn take_slider_change(&mut self) -> Option<f32> {
        self.slider_change.take()
    }

    /// Whether a press has taken hold of the slider — the app arms its
    /// widget drag on this.
    pub fn slider_dragging(&self) -> bool {
        self.slider_drag
    }

    /// The switch column's width: reserved on EVERY row as soon as any row
    /// has a toggle, so the chord column keeps a straight edge.
    fn toggle_col(&self) -> f32 {
        if self.rows.iter().any(|r| r.toggle.is_some()) { TOGGLE_W + 12.0 } else { 0.0 }
    }

    /// The band a slider row draws — the stamp is painted over exactly this,
    /// so the pointer maps to the value where the band is drawn. It ends at
    /// the chord column's right edge, not the row's, which is why it needs
    /// the roster rather than the rect alone.
    fn slider_band_rect(&self, r: Rect) -> Rect {
        let x = r.x + r.width - 8.0 - SLIDER_W;
        let right = r.x + r.width - 8.0 - self.toggle_col();
        Rect { x, y: r.y + 2.0, width: (right - x).max(10.0), height: ROW_H - 4.0 }
    }

    /// The whole control: the band plus the readout lane ahead of it. This is
    /// what the pointer tests against, so the wheel turns the slider over the
    /// readout too.
    fn slider_rect(&self, r: Rect) -> Rect {
        let b = self.slider_band_rect(r);
        Rect { x: b.x - READOUT_W - READOUT_GAP, width: b.width + READOUT_W + READOUT_GAP, ..b }
    }

    /// The band's (x, width), captured at a press for the drag.
    fn slider_track_of(&self, r: Rect) -> (f32, f32) {
        let b = self.slider_band_rect(r);
        (b.x, b.width)
    }

    /// Step the slider by wheel notches: 2% of the range each, the toolkit
    /// slider's own rate, up meaning more — the sign the viewport's zoom
    /// wheel has, since this IS a zoom.
    fn scroll_slider(&mut self, notches: f32) -> bool {
        let (min, max) = self.slider_stamp.range();
        let Some(cur) = self.slider_row().and_then(|i| self.rows[i].slider) else { return false };
        let v = (cur + notches * 0.02 * (max - min)).clamp(min.min(max), max.max(min));
        if (v - cur).abs() < 1e-6 {
            return false;
        }
        self.set_slider_value(v);
        self.slider_change = Some(v);
        true
    }

    /// Put the slider where the pointer is along the captured band. Jumps,
    /// rather than dragging relative to a grab: the band has no thumb to
    /// grab, and a click on a zoom scale should mean "this much".
    fn slide_to(&mut self, px: f32) -> bool {
        let (tx, tw) = self.slider_track;
        let t = ((px - tx) / tw).clamp(0.0, 1.0);
        let (min, max) = self.slider_stamp.range();
        let v = min + t * (max - min);
        let old = self.slider_row().and_then(|i| self.rows[i].slider);
        self.set_slider_value(v);
        self.slider_change = Some(v);
        old != Some(v)
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
        self.set_scroll_px(0.0);
        self.sync_slider_stamp();
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
        let cols_for = |width: f32| -> usize {
            (width / display::measure_text_width("M", &family, font_size).max(1.0)).floor() as usize
        };
        let fit = |text: &str, width: f32| -> String {
            if width <= 0.0 {
                return String::new();
            }
            display::truncate_tail(text, cols_for(width))
        };
        // The same budget, cut from the other end — a path's tail is what
        // identifies it.
        let fit_head = |text: &str, width: f32| -> String {
            if width <= 0.0 {
                return String::new();
            }
            display::truncate_head(text, cols_for(width))
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
        //
        // The switches get a column of their own at the far right, reserved
        // for EVERY row as soon as any row has one, so the chord column keeps
        // a straight edge whether or not the row beside it toggles. Without
        // the reservation the chords step left on toggle rows and the column
        // reads as ragged, which is worse than the strip of air it costs.
        let list = list_rect(rect);
        let toggle_col = self.toggle_col();
        if self.rows.is_empty() {
            let ty = cce_ui::layout::align_text_y(list.y, ROW_H, font_size, 0.0);
            let empty = match self.mode {
                Mode::Tabbed => "No matching command",
                Mode::AddNode => "No matching node",
            };
            ctx.text_with(empty, list.x + 8.0, ty, font_size, [0x70, 0x70, 0x7c], Some(family.clone()), own);
            return;
        }
        ctx.clip(list, |ctx| {
        for i in self.first_row()..self.rows.len() {
            let Some(r) = self.row_rect(rect, i) else { break };
            let row = &self.rows[i];
            // The highlight stops short of the switch column. A switch has
            // no face of its own — it is carved out of whatever it stands
            // on, the DE's convention — and carved out of the selection's
            // tinted bevel it vanished outright: the selected row, the one
            // row whose state Enter is about to flip, was the one row whose
            // state could not be read. On the plate it reads like the rest.
            // A slider row's control is wider than the toggle column and
            // takes the chord column's place on that one row.
            let ctl_col = if row.slider.is_some() {
                (r.x + r.width) - self.slider_rect(r).x + 4.0
            } else {
                toggle_col
            };
            let hl = Rect { width: (r.width - ctl_col).max(0.0), ..r };
            if i == self.selected {
                ctx.rounded_rect(hl, ctrl_r, (true, true, true, true), [accent[0], accent[1], accent[2], 0.16]);
                ctx.bevel_tinted(hl, radii, &cce_ui::scene::Material::from_fill([0.0; 4]), depth, tint);
            } else if self.hover_row == Some(i) {
                ctx.rounded_rect(hl, ctrl_r, (true, true, true, true), [1.0, 1.0, 1.0, 0.05]);
            }
            let ty = cce_ui::layout::align_text_y(r.y, r.height, font_size, 0.0);
            let chord_w = if row.chord.is_empty() {
                0.0
            } else {
                display::measure_text_width(&row.chord, &family, font_size)
            };
            // The label's clip stops short of the chord column so a long
            // label is cut by it rather than running under it.
            let chord_right = r.x + r.width - 8.0 - ctl_col;
            let label_right = chord_right - if chord_w > 0.0 { chord_w + 12.0 } else { 0.0 };
            let label_color = if i == self.selected { [0xf4, 0xf4, 0xfa] } else { [0xcc, 0xcc, 0xd4] };
            // The swatch: a small rounded tile ahead of the label, ringed
            // faintly so a colour near the plate's own does not vanish into
            // it. The label steps right by the tile.
            let mut label_x = r.x + 8.0;
            if let Some(sw) = row.swatch {
                let side = SWATCH_SIDE;
                let tile = Rect { x: label_x, y: r.y + (r.height - side) * 0.5, width: side, height: side };
                let ring = Rect { x: tile.x - 1.0, y: tile.y - 1.0, width: side + 2.0, height: side + 2.0 };
                ctx.rounded_rect(ring, 4.0, (true, true, true, true), [1.0, 1.0, 1.0, 0.22]);
                ctx.rounded_rect(tile, 3.0, (true, true, true, true), sw);
                label_x += side + 8.0;
            }
            ctx.text_with(
                if row.truncate_head {
                    fit_head(&row.label, label_right - label_x)
                } else {
                    fit(&row.label, label_right - label_x)
                },
                label_x,
                ty,
                font_size,
                label_color,
                Some(family.clone()),
                own,
            );
            if chord_w > 0.0 {
                ctx.text_with(
                    row.chord.clone(),
                    chord_right - chord_w,
                    ty,
                    font_size,
                    [0x85, 0x85, 0x92],
                    Some(family.clone()),
                    own,
                );
            }
            if let Some(on) = row.toggle {
                let tr = Rect {
                    x: r.x + r.width - 8.0 - TOGGLE_W,
                    y: r.y + (r.height - TOGGLE_H) * 0.5,
                    width: TOGGLE_W,
                    height: TOGGLE_H,
                };
                Paint::paint(&*self.toggle_stamps[on as usize], tr, ctx);
            }
            if let Some(v) = row.slider {
                let band = self.slider_band_rect(r);
                Paint::paint(&*self.slider_stamp, band, ctx);
                // The readout, right-aligned in its lane ahead of the band —
                // the dialog's own text, so it clears the occlusion clamp.
                let readout = format!("{}%", v.round() as i64);
                let rw = display::measure_text_width(&readout, &family, font_size);
                ctx.text_with(
                    readout,
                    band.x - READOUT_GAP - rw,
                    ty,
                    font_size,
                    label_color,
                    Some(family.clone()),
                    own,
                );
            }
        }
        });

        // The scrollbar's fore copy, over the rows, at the activity's fade:
        // pills, as `ScrollRegion::push_scrollbar_prims` and cce-mail's body
        // bar draw them. Driven by the fade rather than the latch so it
        // draws all the way out. The dialog's plate is the host's, so there
        // is no under-plate copy to show through while sunk — sunk is
        // simply not drawn, as cce-mail's body bar over its opaque window.
        let a = self.sb_activity.fade().clamp(0.0, 1.0);
        if a > 0.001 {
            if let Some((sb_x, track_y, sb_w, track_h, thumb_y, thumb_h)) = self.scrollbar_geom(rect) {
                let dim = |mut c: [f32; 4]| {
                    c[3] *= a;
                    c
                };
                let all = (true, true, true, true);
                ctx.rounded_rect(
                    Rect { x: sb_x, y: track_y, width: sb_w, height: track_h },
                    sb_w.min(track_h) * 0.5,
                    all,
                    dim(cce_ui::color::scrollbar_track_color()),
                );
                ctx.rounded_rect(
                    Rect { x: sb_x, y: thumb_y, width: sb_w, height: thumb_h },
                    sb_w.min(thumb_h) * 0.5,
                    all,
                    dim(cce_ui::color::scrollbar_thumb_color()),
                );
            }
        }
    }
}

impl Input for Dialog {
    /// Advance the list's glide / coast (see `scroll_px`).
    fn tick(&mut self, dt: f32, rect: Rect) -> bool {
        if !self.shows_list() {
            return false;
        }
        let max = self.max_scroll_px();
        self.scroll_motion.reconcile(0.0, self.scroll_px);
        let moved = self.scroll_motion.tick(dt, cce_ui::widget::Bounds::max(0.0), cce_ui::widget::Bounds::max(max));
        if moved {
            self.scroll_px = self.scroll_motion.y.pos();
        }
        // The bar's raise/sink latch and its fade: frames keep coming while
        // the hold runs and while the fore copy is still chasing the latch,
        // so the sink actually renders instead of freezing mid-fade.
        let visible = self.scrollbar_geom(rect).is_some();
        let flipped = self.sb_activity.tick(dt, visible, self.sb_dragging);
        let fade = self.sb_activity.fade();
        let fading = if self.sb_activity.raised() { fade < 1.0 } else { fade > 0.0 };
        moved || self.scroll_motion.is_animating() || flipped || self.sb_activity.holding() || fading
    }

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
                    // The raised bar is in front of the rows; sunk, it is
                    // not there and the press reaches the row.
                    if self.sb_press(rect, *x, *y) {
                        return true;
                    }
                    if let Some(i) = self.row_at(rect, *x, *y) {
                        self.selected = i;
                        if self.rows[i].slider.is_some() {
                            // On the band: take hold and jump there. On the
                            // rest of the row — the readout lane included:
                            // selected, and nothing to run. A press tests the
                            // BAND rather than the whole control, or a click
                            // on the readout would jump the value to the end
                            // of the range nearest it.
                            if let Some(r) = self.row_rect(rect, i) {
                                let s = self.slider_band_rect(r);
                                if *x >= s.x && *x < s.x + s.width {
                                    self.slider_track = self.slider_track_of(r);
                                    self.slider_drag = true;
                                    self.slide_to(*x);
                                }
                            }
                            return true;
                        }
                        self.activated = self.rows.get(i).map(|r| r.id.clone());
                        return true;
                    }
                }
                // Inside the plate but on no control: consumed anyway, so the
                // press cannot reach the pane the dialog is covering.
                true
            }
            Event::MouseButton { button: MouseButton::Left, state: ElementState::Released, .. } => {
                if std::mem::take(&mut self.slider_drag) {
                    return true;
                }
                if std::mem::take(&mut self.sb_dragging) {
                    // The release starts the hold before the bar sinks.
                    self.sb_activity.bump();
                    return true;
                }
                false
            }
            Event::PointerMove { x, y, .. } => {
                if self.slider_drag {
                    return self.slide_to(*x);
                }
                if self.sb_dragging {
                    return self.sb_drag_to(rect, *y);
                }
                self.sb_activity.set_hover(self.over_scrollbar(rect, *x, *y));
                let row = self.shows_list().then(|| self.row_at(rect, *x, *y)).flatten();
                let tab = (self.mode == Mode::Tabbed).then(|| self.tab_at(rect, *x, *y)).flatten();
                let changed = row != self.hover_row || tab != self.hover_tab;
                self.hover_row = row;
                self.hover_tab = tab;
                changed
            }
            Event::MouseWheel { delta, x, y, .. } => {
                if !self.shows_list() || self.rows.is_empty() {
                    return false;
                }
                // Over the slider row's control the wheel turns the slider,
                // not the list — the rest of the row still scrolls.
                if let Some(i) = self.row_at(rect, *x, *y) {
                    if self.rows[i].slider.is_some() {
                        if let Some(r) = self.row_rect(rect, i) {
                            let s = self.slider_rect(r);
                            if *x >= s.x && *x < s.x + s.width {
                                return self.scroll_slider(delta.notches_y());
                            }
                        }
                    }
                }
                // The DE scroll model: a notch is one row and glides there, a
                // trackpad tracks 1:1 and coasts on the lift (`tick` advances).
                let max = self.max_scroll_px();
                self.scroll_motion.reconcile(0.0, self.scroll_px);
                let moved = self.scroll_motion.apply(
                    delta,
                    (ROW_H, ROW_H),
                    cce_ui::widget::Bounds::max(0.0),
                    cce_ui::widget::Bounds::max(max),
                );
                self.scroll_px = self.scroll_motion.y.pos();
                // A scroll raises the bar and starts its hold.
                self.sb_activity.bump();
                moved || self.scroll_motion.is_animating()
            }
            _ => false,
        }
    }

    // The slider drag rides the app's widget-drag protocol (armed by
    // `State::dialog_mouse_input` once a press has taken the band), so the
    // pointer keeps moving the value after it leaves the plate, as a params
    // pane slider's does.
    fn draggable(&self, _rect: Rect) -> bool {
        self.slider_drag
    }
    fn is_dragging(&self) -> bool {
        self.slider_drag
    }
    fn drag_begin(&mut self, px: f32, _py: f32, _rect: Rect) {
        self.slide_to(px);
    }
    fn drag_update(&mut self, px: f32, _py: f32, _rect: Rect) -> bool {
        self.slide_to(px)
    }
    fn drag_end(&mut self) {
        self.slider_drag = false;
    }
}

// ---------------------------------------------------------------------------
// The designer's half: what the dialog shows, and what choosing a row does.
// ---------------------------------------------------------------------------

use crate::app::{param_display, ParamDef, State};
use crate::slots::{DIALOG_IDX, DIALOG_PARAMS_IDX};

/// The Commands list's zoom row: not a registry command but a control — a
/// slider over the network zoom, present only while the network pane is
/// focused, since zoom is that pane's and a slider for a pane you are not
/// looking at would be a strange thing to offer. Picking it runs nothing;
/// dragging it, or the arrow keys while it is selected, zoom in place with
/// the dialog up, the way the toggle rows stay up.
pub const ZOOM_ROW_ID: &str = "zoom_level";

/// The Commands list's other non-command row: the open project's PATH, with
/// its file name in the chord column the way a command's chord sits there —
/// the palette's readout of what is being edited. Picking it copies the path
/// to the clipboard, which is the one thing anyone wants a path on screen
/// for. It heads the list, where it reads as the document the rest of the
/// commands act on.
pub const PATH_ROW_ID: &str = "project_path";

/// Prefix of a recent-project row's id; the rest is the path.
///
/// The recent list was the Main utility node's "Open" dropdown, and it went
/// with that node — leaving `State::recent_files` written on every save and
/// read by nothing. It is a list of documents, so it belongs where the open
/// document's own path already is: rows under the path row, each opening its
/// project. Ranked against the path text like everything else.
pub const RECENT_ROW_PREFIX: &str = "recent:";

/// How many recent projects the list offers. `recent_files` keeps ten; five
/// is what fits above the commands without the palette reading as a file
/// manager, and a query narrows the rest.
pub const RECENT_ROW_LIMIT: usize = 5;

/// Where a Settings row's value lives.
///
/// It used to be neither the live `State` fields nor `DesignSettings`: both
/// were DOWNSTREAM of the root meta node, whose utility subnets were copied
/// over live state on every param change, so a write straight to
/// `State::grid_thickness` survived exactly until the next one. With that
/// node retired the live field IS the value; this enum says which of the
/// three remaining kinds of owner each row has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// A display setting the app owns outright: a live field on `State`,
    /// persisted by `DesignSettings` into `state.kdl`. Named by the key
    /// `settings_field_read` / `settings_field_write` dispatch on.
    ///
    /// These were `Subnet(node, param)` — a param on a utility node under the
    /// root meta node — until 2026-09-23. That node tree WAS the store of
    /// record: `apply_settings_from_menubar_subnets` copied it onto the live
    /// state after every edit, so writing a live field directly survived
    /// until the next unrelated change and no longer. With the meta node
    /// retired the live field is simply the value, and the row writes it.
    Field(&'static str),
    /// A toggle the command registry already owns end to end: the command does
    /// the live flip, the menu checkmark AND any writeback in one place.
    /// Square Aspect and Show Camera Pivot are per-CAMERA settings with no
    /// node at all behind the Default Camera, and their commands are the only
    /// code that gets both cases right — so the row dispatches instead of
    /// writing, and reads its displayed value off `command_toggle_state`, the
    /// same table the palette's own switches read.
    Command(&'static str),
    /// A param on the ACTIVE camera node, with the live field as the
    /// fallback: the Default Camera has no node, so there is nothing to write
    /// but the field.
    ActiveCamera(&'static str),
}

/// The control a [`Setting`] row draws, for the rows that have no param
/// elsewhere to borrow a shape from.
///
/// `Owner::Field` rows need this because their value is a bare Rust field —
/// there is no `ParamDef` behind them carrying a type and a range the way a
/// subnet param did. Spelling it here keeps the table the single description
/// of the Settings half.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ctl {
    Toggle,
    /// `#rrggbb`.
    Color,
    /// `#rrggbbaa` — the wire colour, whose alpha is its own opacity.
    Rgba,
    /// An integer spinbox over `min..=max`. The stored float is scaled by
    /// `unit` (thousandths for Grid Thickness, tenths for Origin Size), which
    /// is the convention those params already used.
    Spin { min: f32, max: f32, unit: f32 },
    /// A float slider, `min..=max`, shown to `dec` decimals.
    Slider { min: f32, max: f32, dec: usize },
    /// A fixed set of strings.
    Choice(&'static [&'static str]),
}

/// One row of the Settings half.
pub struct Setting {
    /// What the dialog calls it — and, because `param_display` keys a row by
    /// its label, the identity the writeback resolves back to this row.
    /// Unique across the table, section titles included.
    pub label: &'static str,
    pub owner: Option<Owner>,
    /// The control, for `Owner::Field` rows. `Command` rows are always
    /// switches and `ActiveCamera` rows borrow the camera param's shape.
    pub ctl: Option<Ctl>,
}

impl Setting {
    const fn section(label: &'static str) -> Self {
        Setting { label, owner: None, ctl: None }
    }

    /// A row whose value the command registry owns.
    const fn cmd(label: &'static str, id: &'static str) -> Self {
        Setting { label, owner: Some(Owner::Command(id)), ctl: Some(Ctl::Toggle) }
    }

    /// A row over a live field.
    const fn field(label: &'static str, key: &'static str, ctl: Ctl) -> Self {
        Setting { label, owner: Some(Owner::Field(key)), ctl: Some(ctl) }
    }

    const fn camera(label: &'static str, name: &'static str) -> Self {
        Setting { label, owner: Some(Owner::ActiveCamera(name)), ctl: None }
    }
}

/// The Settings half, in order.
///
/// Scope is exactly what `DesignSettings` persists: the DISPLAY state, which
/// is the part of the app's configuration that is a preference rather than
/// part of a project. That is now the whole of it — the four utility subnets
/// under the root meta node (`main`, `view`, `guides`, `render`) held these
/// values until 2026-09-23, and every one of them that was reachable only by
/// selecting one of those nodes is a row here. Anything left out would not
/// be "hidden in the node tree", it would be gone.
///
/// Still deliberately NOT here: the pane-visibility toggles (the View menu
/// and the plate corners own those, and a settings dialog is a strange place
/// to hide a pane from), the active camera (the viewport menubar's own menu,
/// whose entries are the camera NODES and so cannot be a fixed table), and
/// keybindings, which this DE edits as `input.kdl` on purpose.
pub const SETTINGS: &[Setting] = &[
    Setting::section("Viewport"),
    Setting::field("Background Color", "bg_color", Ctl::Color),
    Setting::cmd("Square Aspect", "toggle_square_viewport"),
    Setting::cmd("Ray Traced Preview", "toggle_ray_traced_preview"),
    Setting::field("World Unit", "world_unit", Ctl::Choice(&["mm", "cm", "m", "in"])),
    Setting::section("Geometry"),
    Setting::field("Opacity", "geo_opacity", Ctl::Slider { min: 0.0, max: 1.0, dec: 2 }),
    Setting::section("Wireframe"),
    Setting::cmd("Show Wireframe", "toggle_wireframe"),
    Setting::field("Wireframe Color", "wire_color", Ctl::Rgba),
    // The colour applies only in single-colour mode (off, the wires carry
    // the geometry's vertex colours and the colour row sets their alpha
    // alone) — so the switch sits beside the colour, or a colour set here
    // looks ignored.
    Setting::cmd("Wireframe Single Color", "toggle_wire_single_color"),
    Setting::field("Wire Thickness", "wire_width", Ctl::Slider { min: 1.0, max: 8.0, dec: 1 }),
    Setting::section("Points"),
    Setting::cmd("Show Points", "toggle_render_points"),
    Setting::field("Point Size", "point_size", Ctl::Slider { min: 0.0, max: 0.1, dec: 3 }),
    Setting::field("Point Color", "point_color", Ctl::Color),
    Setting::cmd("Show Point Markers", "toggle_point_markers"),
    Setting::field("Point Marker Size", "point_marker_size", Ctl::Spin { min: 5.0, max: 100.0, unit: 1000.0 }),
    Setting::field("Point Marker Color", "point_marker_color", Ctl::Color),
    Setting::cmd("Show Point Numbers", "toggle_point_numbers"),
    Setting::cmd("Show Point Normals", "toggle_point_normals"),
    Setting::section("Grid"),
    Setting::cmd("Show Grid", "toggle_grid"),
    Setting::field("Grid Color", "grid_color", Ctl::Color),
    Setting::field("Grid Thickness", "grid_thickness", Ctl::Spin { min: 2.0, max: 200.0, unit: 1000.0 }),
    Setting::section("Guides"),
    Setting::cmd("Show Origin Axes", "toggle_origin"),
    Setting::field("Origin Size", "origin_size", Ctl::Spin { min: 1.0, max: 50.0, unit: 10.0 }),
    Setting::cmd("Show Reference Cube", "toggle_cube"),
    Setting::section("Camera"),
    Setting::cmd("Show Camera Pivot", "toggle_camera_pivot"),
    Setting::camera("Camera Pivot Size", "Camera Pivot Size"),
    Setting::section("Network"),
    Setting::cmd("Show Network Plate", "toggle_network_plate"),
    Setting::cmd("Circular Pane", "toggle_circular_pane"),
];

#[cfg(test)]
fn field_key(s: &Setting) -> &'static str {
    match s.owner {
        Some(Owner::Field(key)) => key,
        _ => panic!("'{}' is not a Field row", s.label),
    }
}

fn setting_by_label(label: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.label == label)
}

/// A synthetic `ParamDef` carrying `label` and the shape of `src`, so the row
/// renders as whatever control its owner already uses.
fn relabel(src: &ParamDef, label: &'static str) -> ParamDef {
    ParamDef { label: label.to_string(), ..src.clone() }
}

fn bool_param(label: &'static str, on: bool) -> ParamDef {
    shaped(label, "toggle", if on { "true" } else { "false" }.to_string(), None, None, None, &[])
}

/// A synthetic `ParamDef` for a row with no param of its own behind it: the
/// label is both its name and its display key, which is how the writeback
/// resolves a control back to its `Setting`.
fn shaped(
    label: &str,
    param_type: &str,
    default: String,
    min: Option<f32>,
    max: Option<f32>,
    step: Option<f32>,
    options: &[&str],
) -> ParamDef {
    ParamDef {
        name: label.to_string(),
        label: label.to_string(),
        param_type: param_type.to_string(),
        default,
        options: options.iter().map(|s| s.to_string()).collect(),
        min,
        max,
        step,
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

    /// Open the tabbed dialog on its Settings half — what the Wireframe
    /// Color command does, so a palette pick lands on the row that edits
    /// the value rather than on the list it was picked from.
    pub fn open_dialog_on_settings(&mut self) {
        if !self.dialog_visible() || self.slots.dialog.mode != Mode::Tabbed {
            self.open_dialog_in(Mode::Tabbed);
        }
        self.set_dialog_tab(Tab::Settings);
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
            Mode::Tabbed => {
                let mut rows: Vec<Row> = crate::command::palette_entries(&query, self.focused_context())
                .iter()
                .map(|c| Row {
                    id: c.id.to_string(),
                    label: c.label.to_string(),
                    chord: self
                        .shortcut_manager
                        .chord_for(c.id)
                        .map(|s| s.describe())
                        .unwrap_or_default(),
                    // The wire colour is kept sRGB-encoded (it round-trips
                    // through the Render node's hex); the swatch is a fill.
                    swatch: (c.id == "wireframe_color").then(|| {
                        cce_ui::color::to_linear([self.wire_color[0], self.wire_color[1], self.wire_color[2], 1.0])
                    }),
                    toggle: self.command_toggle_state(c.id),
                    slider: None,
                    truncate_head: false,
                })
                .collect();
                // The open project's path heads the list — ranked against
                // the path text, so typing any part of it (the project's
                // name included, that being the tail) finds or drops the row
                // like any other. Absent when no project is loaded: the
                // bundled `default_project.json` leaves `loaded_project_path`
                // None on purpose, and a row offering to copy a path to a
                // versioned file in the source tree would be a trap.
                if let Some((path, name)) = self.project_path_readout() {
                    if !crate::command::fuzzy_rank(&query, &[path.as_str()]).is_empty() {
                        rows.insert(
                            0,
                            Row {
                                id: PATH_ROW_ID.to_string(),
                                label: path,
                                chord: name,
                                swatch: None,
                                toggle: None,
                                slider: None,
                                truncate_head: true,
                            },
                        );
                    }
                }
                // The recent projects, under the path row — the open
                // document, then the ones before it. The project already
                // open is not offered again.
                let open_now = self.loaded_project_path.clone();
                let recent: Vec<std::path::PathBuf> = self
                    .recent_files
                    .iter()
                    .filter(|p| Some(*p) != open_now.as_ref())
                    .take(RECENT_ROW_LIMIT)
                    .cloned()
                    .collect();
                for path in recent.iter().rev() {
                    let text = path.to_string_lossy().to_string();
                    if crate::command::fuzzy_rank(&query, &[text.as_str()]).is_empty() {
                        continue;
                    }
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    rows.insert(
                        0,
                        Row {
                            id: format!("{RECENT_ROW_PREFIX}{text}"),
                            label: text,
                            chord: name,
                            swatch: None,
                            toggle: None,
                            slider: None,
                            // Same reason as the path row: the tail of a
                            // path is what identifies it.
                            truncate_head: true,
                        },
                    );
                }
                // The zoom slider heads the network pane's list, ranked like
                // a row labelled "Zoom" so a query still finds (or drops) it.
                if self.focused_context() == crate::command::Context::Network
                    && !crate::command::fuzzy_rank(&query, &["Zoom"]).is_empty()
                {
                    let cfg = crate::app::configured_grid_geometry();
                    let pct = |pitch: f32| pitch / cfg.pitch_x.max(1e-3) * 100.0;
                    self.slots.dialog.set_slider_range(pct(crate::app::MIN_PITCH_X), pct(crate::app::MAX_PITCH_X));
                    rows.insert(
                        0,
                        Row {
                            id: ZOOM_ROW_ID.to_string(),
                            label: "Zoom".to_string(),
                            chord: String::new(),
                            swatch: None,
                            toggle: None,
                            slider: Some(self.zoom_percent()),
                            truncate_head: false,
                        },
                    );
                }
                rows
            }
            Mode::AddNode => {
                // Every template, everywhere. The settings directories that
                // refused geometry were the root meta node's utility subnets,
                // and they are gone.
                let offered: Vec<&str> =
                    self.node_templates.iter().map(|t| t.label.as_str()).collect();
                crate::command::fuzzy_rank(&query, &offered)
                    .into_iter()
                    .map(|i| Row {
                        id: offered[i].to_string(),
                        label: offered[i].to_string(),
                        chord: String::new(),
                        swatch: None,
                        toggle: None,
                        slider: None,
                        truncate_head: false,
                    })
                    .collect()
            }
        };
        self.slots.dialog.set_rows(rows);
    }

    /// The open project's path and its file name, for the palette's path row
    /// — `None` when no project is loaded.
    ///
    /// The name is `file_name()`, which is the same thing the WINDOW TITLE
    /// shows, so the palette and the title bar cannot disagree about what is
    /// open. A project is a DIRECTORY holding `state.json`, so that name is
    /// the directory's; the bundled `default_project.json` is the one single
    /// file, and it never gets here because loading it leaves
    /// `loaded_project_path` None — the app's own position is that nothing is
    /// loaded, and Set As Default says the same.
    pub fn project_path_readout(&self) -> Option<(String, String)> {
        let path = self.loaded_project_path.as_ref()?;
        let name = path.file_name()?.to_string_lossy().into_owned();
        Some((path.to_string_lossy().into_owned(), name))
    }

    /// Put the open project's path on the clipboard, returning what was
    /// copied — the palette's path row, and the only thing a path on screen
    /// is ever wanted for. `None` when there is no project to name.
    pub fn copy_project_path(&mut self) -> Option<String> {
        let (path, _) = self.project_path_readout()?;
        // Not under test. `wl-copy` has to OUTLIVE its caller to serve the
        // selection, and it inherits the test binary's captured stdout — so a
        // test that really copied left cargo waiting on a pipe held open by a
        // clipboard daemon, which looks exactly like a hung test suite.
        #[cfg(not(test))]
        cce_ui::widget::clipboard::copy_to_clipboard(&path);
        self.update_status_text(&format!("Copied {path}"));
        Some(path)
    }

    /// The network zoom as the slider row reads it: the current x pitch as a
    /// percentage of the configured one, so 100 is Reset Zoom.
    pub fn zoom_percent(&self) -> f32 {
        let cfg = crate::app::configured_grid_geometry();
        if cfg.pitch_x > 0.0 {
            self.grid_pitch_x / cfg.pitch_x * 100.0
        } else {
            100.0
        }
    }

    /// Zoom the network to a percentage of the configured grid, about the
    /// cursor cell — what the slider row's drag lands on. `zoom` clamps, so
    /// the row is re-read afterwards rather than trusted.
    pub fn set_zoom_percent(&mut self, pct: f32) {
        let cfg = crate::app::configured_grid_geometry();
        if self.grid_pitch_x > 0.0 {
            let factor = cfg.pitch_x * pct / 100.0 / self.grid_pitch_x;
            if (factor - 1.0).abs() > 1e-4 {
                self.zoom(factor, None);
            }
        }
        self.refresh_dialog_zoom();
    }

    /// Re-read the slider row from the live zoom, in place.
    fn refresh_dialog_zoom(&mut self) {
        let pct = self.zoom_percent();
        self.slots.dialog.set_slider_value(pct);
    }

    /// What a toggle command's switch currently shows, or `None` for a
    /// command that is not a toggle.
    ///
    /// Read off the very field each command flips in `execute_action` /
    /// `execute_menu_action` — the same read the View menu's checkmarks are
    /// set from — so the switch cannot disagree with the menu. Snapping is
    /// a toggle only INSIDE a viewer state; outside one the command does
    /// nothing but say so, and a switch on a row that cannot flip would be a
    /// lie, so the row is plain until a state is entered.
    /// `dialog_toggle_rows_cover_every_toggle_command` keeps this list and
    /// the registry's `toggle_*` / `show_*_pane` rows in step.
    pub fn command_toggle_state(&self, id: &str) -> Option<bool> {
        Some(match id {
            "toggle_grid" => self.viewport().show_grid,
            "toggle_cube" => self.viewport().show_cube,
            "toggle_origin" => self.viewport().show_origin,
            "toggle_camera_pivot" => self.viewport().show_camera_pivot,
            "toggle_wireframe" => self.wireframe,
            "toggle_point_markers" => self.show_point_markers,
            "toggle_point_numbers" => self.show_point_numbers,
            "toggle_point_normals" => self.show_point_normals,
            "toggle_render_points" => self.render_points,
            "toggle_wire_single_color" => self.wire_single_color,
            "toggle_ray_traced_preview" => self.viewport().rt_mode,
            "toggle_square_viewport" => self.square_viewport,
            "toggle_network_plate" => self.network_plate,
            "toggle_circular_pane" => self.circular_network_pane,
            "detach_circular_window" => self.detached_circular_network,
            "toggle_spreadsheet" => self.show_spreadsheet,
            "show_network_pane" => self.show_network,
            "show_viewport_pane" => self.show_viewport,
            "show_parameters_pane" => self.show_parameters,
            "show_playbar_pane" => self.show_playbar,
            "toggle_snap" => self.viewer_tool.as_ref()?.snap.is_some(),
            _ => return None,
        })
    }

    /// Re-read every row's switch from the live state, touching nothing
    /// else — not the ranking, not the selection, not the scroll. This is
    /// what a toggle pick runs instead of `refresh_dialog_rows`: the rows are
    /// the same rows, only a switch has moved, and re-ranking would throw the
    /// selection back to the top of a list the user is still working down.
    fn refresh_dialog_toggles(&mut self) {
        if self.slots.dialog.mode != Mode::Tabbed {
            return;
        }
        let states: Vec<Option<bool>> =
            self.slots.dialog.rows.iter().map(|r| self.command_toggle_state(&r.id)).collect();
        for (row, state) in self.slots.dialog.rows.iter_mut().zip(states) {
            row.toggle = state;
        }
        self.refresh_dialog_zoom();
    }

    /// The Settings half's rows, each read from whatever owns its value.
    ///
    /// Returns `ParamDef`s rather than display triples so the encoding
    /// (`spinbox:min:max:step`, `choice:a,b`) stays in `param_display` — one
    /// place, shared with the params pane, instead of a second copy here that
    /// could disagree about what a spinbox is.
    fn dialog_settings_params(&self) -> Vec<ParamDef> {
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
                None => out.push(shaped(s.label, "section", String::new(), None, None, None, &[])),
                Some(Owner::Field(key)) => {
                    let ctl = s.ctl.expect("a Field row declares its control");
                    out.push(self.settings_field_param(s.label, key, ctl));
                }
                Some(Owner::Command(id)) => {
                    // The same table the palette's own switches read, so a
                    // row here and a row there cannot disagree about which
                    // way a toggle is set.
                    out.push(bool_param(s.label, self.command_toggle_state(id).unwrap_or(false)));
                }
                Some(Owner::ActiveCamera(name)) => match camera_param(name) {
                    Some(p) => out.push(relabel(p, s.label)),
                    // No camera node behind the Default Camera: the live
                    // field is the value, in the same tenths the camera param
                    // uses.
                    None => out.push(shaped(
                        s.label,
                        "spinbox",
                        ((self.camera_pivot_size * 10.0).round() as i32).to_string(),
                        Some(1.0),
                        Some(50.0),
                        Some(1.0),
                        &[],
                    )),
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

    /// One `Owner::Field` row: the live value, in the shape its `Ctl` names.
    fn settings_field_param(&self, label: &'static str, key: &str, ctl: Ctl) -> ParamDef {
        match ctl {
            Ctl::Toggle => bool_param(label, self.settings_field_bool(key)),
            Ctl::Color => {
                let c = self.settings_field_color(key);
                shaped(label, "color", crate::project::color_to_hex(c), None, None, None, &[])
            }
            Ctl::Rgba => {
                let c = match key {
                    "wire_color" => self.wire_color,
                    _ => [0.0, 0.0, 0.0, 1.0],
                };
                shaped(label, "rgba", crate::project::color_to_hex8(c), None, None, None, &[])
            }
            Ctl::Spin { min, max, unit } => shaped(
                label,
                "spinbox",
                ((self.settings_field_f32(key) * unit).round() as i32).to_string(),
                Some(min),
                Some(max),
                Some(1.0),
                &[],
            ),
            Ctl::Slider { min, max, dec } => shaped(
                label,
                &format!("slider:{min:.*}:{max:.*}:{dec}", dec, dec),
                format!("{:.*}", dec, self.settings_field_f32(key)),
                Some(min),
                Some(max),
                None,
                &[],
            ),
            Ctl::Choice(options) => {
                shaped(label, "choice", self.settings_field_text(key), None, None, None, options)
            }
        }
    }

    fn settings_field_bool(&self, key: &str) -> bool {
        match key {
            "wire_single_color" => self.wire_single_color,
            "render_points" => self.render_points,
            _ => false,
        }
    }

    fn settings_field_color(&self, key: &str) -> [f32; 3] {
        match key {
            "bg_color" => self.viewport().bg_color,
            "grid_color" => self.viewport().grid_color,
            "point_color" => self.point_color,
            "point_marker_color" => self.point_marker_color,
            _ => [0.0; 3],
        }
    }

    fn settings_field_f32(&self, key: &str) -> f32 {
        match key {
            "grid_thickness" => self.grid_thickness,
            "origin_size" => self.origin_size,
            "point_marker_size" => self.point_marker_size,
            "wire_width" => self.wire_width,
            "geo_opacity" => self.geo_opacity,
            "point_size" => self.point_size,
            _ => 0.0,
        }
    }

    fn settings_field_text(&self, key: &str) -> String {
        match key {
            "world_unit" => self.world_unit.suffix().to_string(),
            _ => String::new(),
        }
    }

    /// Write one `Owner::Field` row's new value onto the live state.
    ///
    /// `settings_field_keys_are_all_handled` walks the table against these
    /// four readers and this writer, because a key that no arm names reads
    /// as a default and writes nowhere — a row that looks live and is inert.
    fn settings_field_write(&mut self, key: &str, ctl: Ctl, value: &str) {
        match ctl {
            Ctl::Toggle => {
                let on = value == "true";
                match key {
                    "wire_single_color" => self.wire_single_color = on,
                    "render_points" => self.render_points = on,
                    _ => {}
                }
            }
            Ctl::Color => {
                let Some(c) = crate::project::hex_to_color(value) else { return };
                match key {
                    "bg_color" => self.viewport_mut().bg_color = c,
                    "grid_color" => self.viewport_mut().grid_color = c,
                    "point_color" => self.point_color = c,
                    "point_marker_color" => self.point_marker_color = c,
                    _ => {}
                }
            }
            Ctl::Rgba => {
                let Some(c) = crate::project::hex_to_rgba(value) else { return };
                if key == "wire_color" {
                    // Setting a wire colour means wanting to see it: the
                    // colour applies in single-colour mode only, so a colour
                    // edit turns that mode on if it was off. Twice read as
                    // "the colour did not take" (2026-09-21).
                    let changed = c != self.wire_color;
                    self.wire_color = c;
                    if changed && !self.wire_single_color {
                        self.wire_single_color = true;
                    }
                }
            }
            Ctl::Spin { unit, .. } => {
                let Ok(v) = value.parse::<f32>() else { return };
                let v = v / unit;
                match key {
                    "grid_thickness" => self.grid_thickness = v,
                    "origin_size" => self.origin_size = v,
                    "point_marker_size" => self.point_marker_size = v,
                    _ => {}
                }
            }
            Ctl::Slider { min, max, .. } => {
                let Ok(v) = value.parse::<f32>() else { return };
                let v = v.clamp(min, max);
                match key {
                    "wire_width" => self.wire_width = v,
                    "geo_opacity" => self.geo_opacity = v,
                    "point_size" => self.point_size = v,
                    _ => {}
                }
            }
            Ctl::Choice(_) => {
                if key == "world_unit" {
                    if let Some(u) = cce_ui::units::Unit::parse(value) {
                        self.world_unit = u;
                        self.viewport_dirty = true;
                    }
                }
            }
        }
    }

    /// One Settings row's value as the dialog would show it. Test-facing:
    /// `dialog_settings_rows_name_owners_that_exist` round-trips every
    /// `Owner::Field` row through this and [`State::settings_write_row`],
    /// which is the only way to catch a key that no dispatch arm names.
    #[cfg(test)]
    pub(crate) fn settings_row_value(&self, label: &str) -> String {
        let s = SETTINGS.iter().find(|s| s.label == label).expect("no such Settings row");
        let ctl = s.ctl.expect("that row declares no control");
        self.settings_field_param(s.label, field_key(s), ctl).default
    }

    /// Write one Settings row, as the writeback does.
    #[cfg(test)]
    pub(crate) fn settings_write_row(&mut self, label: &str, value: &str) {
        let s = SETTINGS.iter().find(|s| s.label == label).expect("no such Settings row");
        let ctl = s.ctl.expect("that row declares no control");
        self.settings_field_write(field_key(s), ctl, value);
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
                Owner::Field(key) => {
                    let ctl = setting.ctl.expect("a Field row declares its control");
                    self.settings_field_write(key, ctl, value);
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
            // The arrows nudge the selected slider row by a Zoom In / Zoom
            // Out step; on any other row they mean nothing here.
            Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::ArrowRight) => {
                if self.slots.dialog.selected_id() == Some(ZOOM_ROW_ID) {
                    let f = if matches!(event.logical_key, Key::Named(NamedKey::ArrowRight)) { 1.15 } else { 1.0 / 1.15 };
                    self.zoom(f, None);
                    self.refresh_dialog_zoom();
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
    ///
    /// A TOGGLE row does not close at all. It is a switch, and a switch you
    /// can only flip once before the panel it is on vanishes is a button
    /// with extra steps: Show Grid, Show Cube and Square Aspect are the
    /// kind of thing you set together, looking at the viewport, and the
    /// dialog staying up is what lets you. The command runs, the switches
    /// re-read, and the selection stays where it was — by Enter or by a
    /// click, since both arrive here.
    pub(crate) fn take_dialog_pick(&mut self, id: String) {
        let mode = self.slots.dialog.mode;
        // The zoom row is a control, not a command: Enter on it does nothing
        // and the dialog stays up for the arrows and the pointer.
        if mode == Mode::Tabbed && id == ZOOM_ROW_ID {
            return;
        }
        // The path row: copy, say so, and close. A copy is done the moment it
        // happens — unlike a toggle, there is nothing to sit and adjust — so
        // it leaves the way a command does.
        if mode == Mode::Tabbed && id == PATH_ROW_ID {
            self.close_dialog();
            self.copy_project_path();
            return;
        }
        // A recent project: open it, and say so if it will not open — a
        // path in this list can have been moved or deleted since.
        if mode == Mode::Tabbed {
            if let Some(path) = id.strip_prefix(RECENT_ROW_PREFIX) {
                let path = std::path::PathBuf::from(path);
                self.close_dialog();
                if let Err(e) = self.load_from_file(&path) {
                    self.update_status_text(&format!("Could not open {}: {e}", path.display()));
                }
                return;
            }
        }
        if mode == Mode::Tabbed && self.command_toggle_state(&id).is_some() {
            self.run_command(&id);
            self.refresh_dialog_toggles();
            return;
        }
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
        if let Some(pct) = self.slots.dialog.take_slider_change() {
            self.set_zoom_percent(pct);
            changed = true;
        }
        changed
    }

    /// A mouse button, while the dialog is open.
    ///
    /// `None` hands the press back to the ordinary cascade — only the middle
    /// button, which the dialog has no use for. `Some(handled)` means the
    /// dialog dealt with it and nothing else should. A RIGHT press is the
    /// dialog's too: inside the plate it is swallowed (nothing in the dialog
    /// has a context menu, and until 2026-09-22 it fell through to the pane
    /// beneath, whose menu then opened over a modal with its labels clipped
    /// by the dialog's occluder — a menu with no legible entries), outside
    /// it dismisses, exactly as a left press does.
    pub(crate) fn dialog_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
    ) -> Option<bool> {
        let (x, y) = (self.cursor_x, self.cursor_y);
        if button == MouseButton::Right {
            let inside = self.in_dialog_slot(DIALOG_IDX, x, y) || self.in_dialog_slot(DIALOG_PARAMS_IDX, x, y);
            if !inside && state == ElementState::Pressed {
                self.close_dialog();
            }
            return Some(true);
        }
        if button != MouseButton::Left {
            return None;
        }

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

        // The same for the Commands list's slider row, whose drag the
        // dialog slot itself drives.
        if state == ElementState::Released && self.drag_widget == Some(DIALOG_IDX) {
            let ptr = &mut self.slots.dialog as *mut cce_ui::widget::Adapted<Dialog>;
            unsafe {
                (*ptr).handle_event(&cce_ui::widget::Event::DragEnd, &mut self.ui_context);
                (*ptr).handle_event(
                    &cce_ui::widget::Event::MouseButton { button, state, x, y, local_x: x, local_y: y },
                    &mut self.ui_context,
                );
            }
            self.drag_widget = None;
            self.drag_press_cursor = None;
            self.drain_dialog_clicks();
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
        // A press that took the slider's band arms the same widget drag the
        // settings body's sliders arm, so the value follows the pointer
        // wherever it goes until the release.
        if state == ElementState::Pressed && self.slots.dialog.slider_dragging() {
            let ev = cce_ui::widget::Event::DragStart { start_x: x, start_y: y };
            let ptr = &mut self.slots.dialog as *mut cce_ui::widget::Adapted<Dialog>;
            unsafe {
                (*ptr).handle_event(&ev, &mut self.ui_context);
            }
            self.drag_widget = Some(DIALOG_IDX);
            self.drag_press_cursor = Some((x, y));
        }
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
            let taken = self.dispatch_uncovered(DIALOG_IDX, &ev);
            // A wheel over the zoom slider row moved it: land the value.
            self.drain_dialog_clicks();
            return taken;
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
        // Straight to handle_event, not propagate_event: so the gesture
        // bookkeeping the router would have done is done here, or the
        // dialog's panes inherit the main pane's gesture state.
        if matches!(ev, cce_ui::widget::Event::MouseWheel { .. }) {
            self.ui_context.note_scroll_event();
        }
        let ptr = self.slots.get_dyn_mut(idx) as *mut (dyn cce_ui::widget::WidgetHost + 'static);
        let taken = unsafe { (*ptr).handle_event(ev, &mut self.ui_context) };
        self.slots.dialog.set_occluding(true);
        self.ui_context.invalidate_coverage_cache();
        taken
    }
}
